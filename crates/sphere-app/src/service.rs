//! The app's link to sphered. A background thread runs a small tokio runtime: it starts sphered when nothing
//! answers on the socket, connects as the app, takes a snapshot, subscribes, and keeps a fresh snapshot in a
//! shared slot for the Bevy systems. Commands (start a task, answer a request) go the other way on a channel.
use bevy::prelude::*;
use sphere_client::{Client, ClientKind, StartParams, Update};
use sphere_proto::api::Snapshot;
use sphere_proto::{AnswerKind, RequestView, TaskView};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;

#[derive(Clone, Debug, PartialEq)]
pub enum Link {
    Connecting,
    Starting,
    Connected,
    Down(String),
}

/// What the systems read. `version` bumps on every change, so systems can skip work when it is the same.
#[derive(Default)]
pub struct Shared {
    pub link: Option<Link>,
    pub tasks: Vec<TaskView>,
    pub requests: Vec<RequestView>,
    pub version: u64,
    /// Toasts for the overlay: errors from commands, "started", ...
    pub notes: Vec<String>,
}

pub enum Cmd {
    Start { repo: String, prompt: String },
    Answer { request: String, answer: AnswerKind, text: Option<String> },
    Send { task: String, text: String },
}

#[derive(Resource, Clone)]
pub struct Service {
    pub shared: Arc<Mutex<Shared>>,
    tx: mpsc::UnboundedSender<Cmd>,
}

impl Service {
    pub fn send(&self, c: Cmd) {
        let _ = self.tx.send(c);
    }
}

/// Starts the link thread. `offline` (screenshots, benches) never touches the service.
pub fn start(offline: bool) -> Service {
    let shared = Arc::new(Mutex::new(Shared { link: Some(Link::Connecting), ..default() }));
    let (tx, rx) = mpsc::unbounded_channel();
    if !offline {
        let sh = shared.clone();
        std::thread::Builder::new()
            .name("sphere-link".into())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("tokio runtime");
                rt.block_on(run(sh, rx));
            })
            .expect("link thread");
    }
    Service { shared, tx }
}

fn set_link(sh: &Mutex<Shared>, l: Link) {
    let mut s = sh.lock().unwrap();
    if s.link.as_ref() != Some(&l) {
        s.link = Some(l);
        s.version += 1;
    }
}

fn note(sh: &Mutex<Shared>, n: impl Into<String>) {
    let mut s = sh.lock().unwrap();
    s.notes.push(n.into());
    s.version += 1;
}

fn apply(sh: &Mutex<Shared>, snap: Snapshot) {
    let mut s = sh.lock().unwrap();
    s.tasks = snap.tasks;
    s.requests = snap.requests;
    s.version += 1;
}

/// `sphered` next to this executable, else on `PATH`.
fn sphered_exe() -> PathBuf {
    if let Ok(me) = std::env::current_exe() {
        if let Some(dir) = me.parent() {
            let p = dir.join("sphered");
            if p.exists() {
                return p;
            }
        }
    }
    PathBuf::from("sphered")
}

/// Starts sphered detached from the app (its own session), logging to `~/.sphere/sphered.log`.
fn spawn_sphered() -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    let dir = sphere_proto::paths::default_state_dir();
    let _ = std::fs::create_dir_all(&dir);
    let log = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("sphered.log")).map_err(|e| e.to_string())?;
    let err = log.try_clone().map_err(|e| e.to_string())?;
    let mut cmd = std::process::Command::new(sphered_exe());
    cmd.stdin(std::process::Stdio::null()).stdout(log).stderr(err);
    // SAFETY: setsid is async-signal-safe and only detaches the child from the app's session.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    cmd.spawn().map(|_| ()).map_err(|e| format!("could not start sphered ({}): {e}", sphered_exe().display()))
}

async fn connect(sh: &Mutex<Shared>) -> Client {
    // SPHERE_SOCKET points the app at another sphered (tests, a second state dir); the app then never
    // starts one itself.
    let custom = std::env::var_os("SPHERE_SOCKET").map(PathBuf::from);
    let socket = custom.clone().unwrap_or_else(sphere_proto::paths::default_socket);
    let mut started = custom.is_some();
    let mut backoff = Duration::from_millis(100);
    loop {
        match Client::connect(&socket).await {
            Ok(c) => return c,
            Err(e) => {
                if !started {
                    started = true;
                    set_link(sh, Link::Starting);
                    if let Err(e) = spawn_sphered() {
                        set_link(sh, Link::Down(e));
                    }
                } else if backoff >= Duration::from_secs(2) {
                    set_link(sh, Link::Down(format!("sphered is not answering: {e}")));
                }
            }
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(4));
    }
}

async fn run(sh: Arc<Mutex<Shared>>, mut rx: mpsc::UnboundedReceiver<Cmd>) {
    loop {
        let client = connect(&sh).await;
        if let Err(e) = session(&sh, &client, &mut rx).await {
            set_link(&sh, Link::Down(e));
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        set_link(&sh, Link::Connecting);
    }
}

/// One connection: snapshot, subscribe from its seq, then refresh the snapshot (at most every 80 ms) whenever
/// events arrive, while serving commands.
async fn session(sh: &Arc<Mutex<Shared>>, c: &Client, rx: &mut mpsc::UnboundedReceiver<Cmd>) -> Result<(), String> {
    c.hello(ClientKind::App).await.map_err(|e| e.to_string())?;
    let snap = c.snapshot().await.map_err(|e| e.to_string())?;
    let seq = snap.seq;
    apply(sh, snap);
    let (_, mut sub) = c.subscribe(seq, None).await.map_err(|e| e.to_string())?;
    set_link(sh, Link::Connected);
    let mut dirty = false;
    let mut tick = tokio::time::interval(Duration::from_millis(80));
    loop {
        tokio::select! {
            u = sub.next() => match u {
                Some(Update::Event(_)) => dirty = true,
                Some(Update::Live(_)) => {}
                Some(Update::Lagged(e)) => return Err(format!("resyncing: {}", e.message)),
                Some(Update::Closed) | None => return Err("sphered closed the connection".into()),
            },
            _ = tick.tick() => {
                if dirty {
                    dirty = false;
                    apply(sh, c.snapshot().await.map_err(|e| e.to_string())?);
                }
            }
            cmd = rx.recv() => {
                let Some(cmd) = cmd else { return Ok(()) };
                let c = c.clone();
                let sh = sh.clone();
                // Commands run beside the loop so a slow start never stalls updates.
                tokio::spawn(async move {
                    if let Err(e) = command(&c, cmd).await {
                        note(&sh, e);
                    }
                });
            }
        }
    }
}

async fn command(c: &Client, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::Start { repo, prompt } => {
            let p = StartParams {
                repo,
                prompt,
                title: None,
                in_place: false,
                budget_usd: None,
                mode: None,
                model: None,
                allow_rewrites: false,
            };
            c.start_task(p).await.map(|_| ()).map_err(|e| format!("could not start: {e}"))
        }
        Cmd::Answer { request, answer, text } => {
            let p = sphere_client::AnswerParams { request, answer, text, input: None, always: false };
            match c.answer(p).await {
                Ok(()) => Ok(()),
                Err(e) if e.is_already_answered() => Ok(()),
                Err(e) => Err(format!("could not answer: {e}")),
            }
        }
        Cmd::Send { task, text } => c.send(&task, &text).await.map_err(|e| format!("could not send: {e}")),
    }
}
