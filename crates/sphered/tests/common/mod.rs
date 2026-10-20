//! Test harness: starts the real `sphered` and `sphere-host` binaries on
//! temporary directories, pointed at `fake-claude`, and drives them through
//! `sphere-client`.
#![allow(dead_code)]

use serde_json::Value;
use sphere_client::{Client, LiveFrame, StartParams, Subscription, Update};
use sphere_proto::{Event, EventBody};
use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::os::unix::net::UnixStream as StdStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

pub struct Bins {
    pub sphered: PathBuf,
    pub host: PathBuf,
    pub fake: PathBuf,
    pub sphere: PathBuf,
}

/// The binaries under test. `cargo test --workspace` has built them all; run
/// on one package alone, build the others first.
pub fn bins() -> &'static Bins {
    static B: OnceLock<Bins> = OnceLock::new();
    B.get_or_init(|| {
        let sphered = PathBuf::from(env!("CARGO_BIN_EXE_sphered"));
        let dir = sphered.parent().unwrap().to_path_buf();
        let b = Bins {
            host: dir.join("sphere-host"),
            fake: dir.join("fake-claude"),
            sphere: dir.join("sphere"),
            sphered,
        };
        if !(b.host.exists() && b.fake.exists() && b.sphere.exists()) {
            let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
            let st = Command::new(cargo)
                .args([
                    "build",
                    "-p",
                    "sphere-host",
                    "-p",
                    "fake-claude",
                    "-p",
                    "sphere-cli",
                ])
                .status()
                .expect("run cargo build");
            assert!(st.success(), "could not build the test binaries");
        }
        b
    })
}

pub fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../fake-claude/fixtures")
        .join(format!("{name}.jsonl"))
}

pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

pub fn init_repo(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.email", "t@example.com"]);
    git(dir, &["config", "user.name", "T"]);
    std::fs::write(dir.join("README.md"), "hello\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "init"]);
}

pub struct Env {
    pub tmp: tempfile::TempDir,
    pub state: PathBuf,
    pub socket: PathBuf,
    pub host_socket: PathBuf,
    pub repo: PathBuf,
    pub fixture: String,
    settings: BTreeMap<String, String>,
    daemon: Option<Child>,
}

impl Env {
    pub fn new(fixture_name: &str) -> Env {
        let tmp = tempfile::Builder::new()
            .prefix("sp")
            .tempdir_in("/tmp")
            .unwrap();
        let root = tmp.path().to_path_buf();
        let repo = root.join("repo");
        init_repo(&repo);
        let b = bins();
        let mut settings = BTreeMap::new();
        let q = |p: &Path| format!("{:?}", p.to_string_lossy());
        settings.insert("state_dir".into(), q(&root.join("state")));
        settings.insert("socket".into(), q(&root.join("s.sock")));
        settings.insert("host_socket".into(), q(&root.join("h.sock")));
        settings.insert("host_command".into(), q(&b.host));
        settings.insert("agent_command".into(), q(&b.fake));
        settings.insert(
            "agent_args".into(),
            format!(
                "[\"--fixture\", {:?}]",
                fixture(fixture_name).to_string_lossy()
            ),
        );
        settings.insert("stop_grace_ms".into(), "1000".into());
        settings.insert("interrupt_wait_ms".into(), "300".into());
        Env {
            state: root.join("state"),
            socket: root.join("s.sock"),
            host_socket: root.join("h.sock"),
            repo,
            fixture: fixture_name.to_string(),
            settings,
            daemon: None,
            tmp,
        }
    }

    /// Set a config key to a raw TOML value.
    pub fn set(mut self, key: &str, toml_value: &str) -> Env {
        self.settings.insert(key.into(), toml_value.into());
        self
    }

    pub fn set_str(self, key: &str, value: &str) -> Env {
        let v = format!("{value:?}");
        self.set(key, &v)
    }

    pub fn root(&self) -> &Path {
        self.tmp.path()
    }

    pub fn write_state_file(&self, name: &str, text: &str) -> PathBuf {
        std::fs::create_dir_all(&self.state).unwrap();
        let p = self.state.join(name);
        std::fs::write(&p, text).unwrap();
        p
    }

    fn config_text(&self) -> String {
        self.settings
            .iter()
            .map(|(k, v)| format!("{k} = {v}\n"))
            .collect()
    }

    /// Start (or restart) sphered and wait until it answers.
    pub async fn start_daemon(&mut self) {
        assert!(self.daemon.is_none(), "already started");
        std::fs::create_dir_all(&self.state).unwrap();
        let cfg = self.root().join("config.toml");
        std::fs::write(&cfg, self.config_text()).unwrap();
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.root().join("sphered.log"))
            .unwrap();
        let child = Command::new(&bins().sphered)
            .arg("--config")
            .arg(&cfg)
            .env("RUST_LOG", "sphered=debug,sphere_host=debug")
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()
            .expect("start sphered");
        self.daemon = Some(child);
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Ok(c) = Client::connect(&self.socket).await {
                if c.status().await.is_ok() {
                    return;
                }
            }
            if let Some(Ok(Some(st))) = self.daemon.as_mut().map(|d| d.try_wait()) {
                panic!("sphered exited early ({st}):\n{}", self.daemon_log());
            }
            assert!(
                Instant::now() < deadline,
                "sphered did not start:\n{}",
                self.daemon_log()
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    pub fn daemon_log(&self) -> String {
        std::fs::read_to_string(self.root().join("sphered.log")).unwrap_or_default()
    }

    pub fn daemon_pid(&self) -> u32 {
        self.daemon.as_ref().expect("daemon not running").id()
    }

    /// kill -9: no goodbye, nothing flushed beyond what was committed.
    pub fn kill_daemon(&mut self) {
        if let Some(mut d) = self.daemon.take() {
            let _ = d.kill();
            let _ = d.wait();
            // A child sphered had only just started (ps, git) holds a copy of its listening socket until its exec is
            // through, which on a busy machine can be long enough to see: until nothing answers there, a new sphered
            // takes this one for still running.
            let deadline = Instant::now() + Duration::from_secs(5);
            while StdStream::connect(&self.socket).is_ok() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    pub async fn client(&self) -> Client {
        let c = Client::connect(&self.socket)
            .await
            .expect("connect to sphered");
        c.hello(sphere_client::ClientKind::Cli).await.unwrap();
        c
    }

    pub async fn watcher(&self, since: u64, task: Option<&str>) -> Watcher {
        let c = Client::connect(&self.socket)
            .await
            .expect("connect to sphered");
        Watcher::new(&c, since, task).await
    }

    pub async fn start_task(&self, c: &Client, title: &str, prompt: &str) -> String {
        c.start_task(StartParams {
            repo: self.repo.to_string_lossy().to_string(),
            prompt: prompt.into(),
            title: Some(title.into()),
            ..Default::default()
        })
        .await
        .expect("start task")
    }

    /// Talk to the host directly (std only, so it also works from Drop).
    pub fn host_call(&self, method: &str) -> Option<Value> {
        let mut s = StdStream::connect(&self.host_socket).ok()?;
        s.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
        writeln!(s, "{{\"id\":1,\"method\":\"{method}\",\"params\":{{}}}}").ok()?;
        let mut line = String::new();
        std::io::BufReader::new(s).read_line(&mut line).ok()?;
        serde_json::from_str(&line).ok()
    }

    pub fn host_pid(&self) -> u32 {
        self.host_call("ping")
            .expect("host answers")
            .pointer("/result/pid")
            .unwrap()
            .as_u64()
            .unwrap() as u32
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        self.kill_daemon();
        let _ = self.host_call("shutdown");
        // Give the host a moment to remove its socket before the dir goes.
        std::thread::sleep(Duration::from_millis(150));
    }
}

/// Collects a subscription so a test can wait for events in order, and look
/// back at everything seen so far.
pub struct Watcher {
    sub: Subscription,
    pub log: Vec<Event>,
    pub live: Vec<LiveFrame>,
    pub lagged: bool,
    pub closed: bool,
}

impl Watcher {
    pub async fn new(c: &Client, since: u64, task: Option<&str>) -> Watcher {
        let (_, sub) = c
            .subscribe(since, task.map(str::to_string))
            .await
            .expect("subscribe");
        Watcher {
            sub,
            log: vec![],
            live: vec![],
            lagged: false,
            closed: false,
        }
    }

    async fn recv(&mut self, wait: Duration) -> bool {
        match tokio::time::timeout(wait, self.sub.next()).await {
            Ok(Some(Update::Event(e))) => self.log.push(e),
            Ok(Some(Update::Live(f))) => self.live.push(f),
            Ok(Some(Update::Lagged(_))) => self.lagged = true,
            Ok(Some(Update::Closed)) | Ok(None) => self.closed = true,
            Err(_) => return false,
        }
        true
    }

    /// Take in whatever arrives for `ms` milliseconds.
    pub async fn settle(&mut self, ms: u64) {
        let end = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < end {
            self.recv(end - Instant::now()).await;
        }
    }

    pub async fn wait_from<F: Fn(&Event) -> bool>(
        &mut self,
        min_seq: u64,
        what: &str,
        f: F,
    ) -> Event {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut scanned = 0;
        loop {
            while scanned < self.log.len() {
                let e = &self.log[scanned];
                scanned += 1;
                if e.seq > min_seq && f(e) {
                    return e.clone();
                }
            }
            if Instant::now() > deadline || self.closed || self.lagged {
                let tail: Vec<String> = self
                    .log
                    .iter()
                    .rev()
                    .take(25)
                    .rev()
                    .map(|e| {
                        format!(
                            "{} {} {}",
                            e.seq,
                            e.task,
                            serde_json::to_string(&e.body).unwrap_or_default()
                        )
                    })
                    .collect();
                panic!(
                    "timed out waiting for {what} (closed={} lagged={}); last events:\n{}",
                    self.closed,
                    self.lagged,
                    tail.join("\n")
                );
            }
            self.recv(Duration::from_millis(200)).await;
        }
    }

    pub async fn wait<F: Fn(&Event) -> bool>(&mut self, what: &str, f: F) -> Event {
        self.wait_from(0, what, f).await
    }

    pub fn count<F: Fn(&Event) -> bool>(&self, f: F) -> usize {
        self.log.iter().filter(|e| f(e)).count()
    }

    pub fn find<F: Fn(&Event) -> bool>(&self, f: F) -> Option<&Event> {
        self.log.iter().find(|e| f(e))
    }
}

// ---- predicates -------------------------------------------------------------

pub fn is_kind(name: &'static str) -> impl Fn(&Event) -> bool {
    move |e| e.body.kind_name() == name
}

pub fn say_has(needle: &'static str) -> impl Fn(&Event) -> bool {
    move |e| matches!(&e.body, EventBody::Say { text } if text.contains(needle))
}

pub fn is_turn(e: &Event) -> bool {
    matches!(e.body, EventBody::Turn { .. })
}

pub fn is_request(e: &Event) -> bool {
    matches!(e.body, EventBody::Request { .. })
}

pub fn say_text(e: &Event) -> String {
    match &e.body {
        EventBody::Say { text } => text.clone(),
        other => panic!("not a say: {other:?}"),
    }
}

/// Poll `f` until it returns Some, for up to `secs`.
pub async fn eventually<T, Fut: std::future::Future<Output = Option<T>>>(
    what: &str,
    secs: u64,
    mut f: impl FnMut() -> Fut,
) -> T {
    let end = Instant::now() + Duration::from_secs(secs);
    loop {
        if let Some(v) = f().await {
            return v;
        }
        assert!(Instant::now() < end, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

pub fn pid_alive(pid: i32) -> bool {
    unsafe { libc::kill(pid, 0) == 0 }
}

pub fn pid_from_say(text: &str, prefix: &str) -> i32 {
    text.split(prefix)
        .nth(1)
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or_else(|| panic!("no pid in {text:?}"))
}
