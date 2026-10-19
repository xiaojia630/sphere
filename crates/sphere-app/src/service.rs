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

/// A response time as the panels say it: "<1 ms", "42 ms", "1.3 s".
pub fn fmt_rtt(ms: f32) -> String {
    if ms < 1.0 {
        "<1 ms".to_string()
    } else if ms < 1000.0 {
        format!("{ms:.0} ms")
    } else {
        format!("{:.1} s", ms / 1000.0)
    }
}

/// The link to sphered in words ("sphered connected · 4 ms"), for the Signal ring's tooltip and click.
pub fn link_line(link: Option<&Link>, rtt_ms: Option<f32>, demo: bool) -> String {
    if demo {
        return "demo data, no sphered".to_string();
    }
    match link {
        Some(Link::Connected) => match rtt_ms {
            Some(ms) => format!("sphered connected · {}", fmt_rtt(ms)),
            None => "sphered connected".to_string(),
        },
        Some(Link::Down(why)) if !why.is_empty() => format!("sphered down · {why}"),
        Some(Link::Down(_)) => "sphered down".to_string(),
        _ => "sphered connecting…".to_string(),
    }
}

/// The word on the left edge after "SPHERE · CORE" (and "AGENT SWARM"): ONLINE, CONNECTING… or OFFLINE.
pub fn edge_word(link: Option<&Link>, demo: bool) -> &'static str {
    match link {
        _ if demo => "ONLINE",
        Some(Link::Connected) => "ONLINE",
        Some(Link::Down(_)) => "OFFLINE",
        _ => "CONNECTING…",
    }
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
    /// how long the last snapshot took to come back
    pub rtt_ms: Option<f32>,
    /// each task's run as the Working view shows it: steps, what it said, files
    pub runs: std::collections::HashMap<String, Run>,
    /// finished tasks whose branch is already in the repo (the app checks with git, see `live::check_merged`)
    pub merged: std::collections::HashSet<String>,
    /// `merged` has been worked out at least once (until then a finished task's branch may or may not be merged)
    pub merged_checked: bool,
    /// what today's agents have cost, and how many sphered runs at once (from the snapshot)
    pub cost_today_usd: f64,
    pub max_concurrent: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RunStep {
    pub id: String,
    pub kind: sphere_proto::StepKind,
    pub tool: String,
    pub title: String,
    pub detail: String,
    /// None while running
    pub ok: Option<bool>,
    pub output: String,
    pub at: String,
    /// its place among every step the run has had (stays put when old steps are dropped)
    pub seq: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Run {
    pub steps: Vec<RunStep>,
    /// what the agent said, oldest first
    pub says: Vec<String>,
    /// for each say, how many steps had started when it was said
    pub says_seq: Vec<u64>,
    seq: u64,
    pub files: Vec<sphere_proto::FileStat>,
    pub version: u64,
}

impl Run {
    /// Why the agent last edited `path`: what it said just before that edit (its plan for the change).
    pub fn reason_for(&self, path: &str) -> Option<(&str, &RunStep)> {
        let name = std::path::Path::new(path).file_name()?.to_string_lossy().to_string();
        let step = self.steps.iter().rev().find(|s| s.kind == sphere_proto::StepKind::Edit && (s.title.contains(path) || s.title.ends_with(&name) || s.detail.contains(path)))?;
        let i = self.says_seq.iter().rposition(|&q| q <= step.seq)?;
        Some((self.says[i].as_str(), step))
    }

    /// Folds one event into the run.
    pub fn apply(&mut self, ev: &sphere_proto::Event) {
        use sphere_proto::EventBody as B;
        match &ev.body {
            B::Step { id, step_kind, tool, title, detail, .. } => {
                self.steps.push(RunStep { id: id.clone(), kind: *step_kind, tool: tool.clone(), title: title.clone(), detail: detail.clone(), ok: None, output: String::new(), at: ev.at.clone(), seq: self.seq });
                self.seq += 1;
            }
            B::StepDone { id, ok, output } => {
                if let Some(s) = self.steps.iter_mut().rev().find(|s| &s.id == id) {
                    s.ok = Some(*ok);
                    if let Some(o) = output {
                        s.output = match &o.tail {
                            Some(t) => format!("{}\n…\n{}", o.head, t),
                            None => o.head.clone(),
                        };
                    }
                }
            }
            B::Say { text } => {
                self.says.push(text.clone());
                self.says_seq.push(self.seq);
            }
            B::FilesTouched { files } => self.files = files.clone(),
            _ => return,
        }
        // keep it bounded
        if self.steps.len() > 400 {
            self.steps.drain(..100);
        }
        if self.says.len() > 50 {
            self.says.drain(..10);
            self.says_seq.drain(..10);
        }
        self.version += 1;
    }
}

pub enum Cmd {
    Start { repo: String, prompt: String },
    Answer { request: String, answer: AnswerKind, text: Option<String> },
    Send { task: String, text: String },
    Interrupt { task: String },
    Stop { task: String },
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
    s.cost_today_usd = snap.cost_today_usd;
    s.max_concurrent = snap.max_concurrent;
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
    // backfill the runs of every active task and the most recent others up to the snapshot (the swarm shows each
    // active agent's steps), then the subscription carries on
    let mut recent: Vec<&TaskView> = snap.tasks.iter().collect();
    recent.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    let active = |t: &TaskView| matches!(t.state, sphere_proto::TaskState::Queued | sphere_proto::TaskState::Working | sphere_proto::TaskState::NeedsYou);
    let ids: Vec<String> = recent.iter().enumerate().filter(|(i, t)| *i < 8 || active(t)).map(|(_, t)| t.id.clone()).collect();
    let mut runs = std::collections::HashMap::new();
    for id in ids {
        let p = sphere_client::HistoryParams { task: Some(id.clone()), before: Some(seq + 1), limit: Some(2000) };
        if let Ok(evs) = c.history(p).await {
            let mut r = Run::default();
            for e in &evs {
                r.apply(e);
            }
            runs.insert(id, r);
        }
    }
    sh.lock().unwrap().runs = runs;
    apply(sh, snap);
    let (_, mut sub) = c.subscribe(seq, None).await.map_err(|e| e.to_string())?;
    set_link(sh, Link::Connected);
    let mut dirty = false;
    let mut ping = std::time::Instant::now();
    let mut tick = tokio::time::interval(Duration::from_millis(80));
    loop {
        tokio::select! {
            u = sub.next() => match u {
                Some(Update::Event(ev)) => {
                    dirty = true;
                    let mut s = sh.lock().unwrap();
                    s.runs.entry(ev.task.clone()).or_default().apply(&ev);
                    s.version += 1;
                }
                Some(Update::Live(_)) => {}
                Some(Update::Lagged(e)) => return Err(format!("resyncing: {}", e.message)),
                Some(Update::Closed) | None => return Err("sphered closed the connection".into()),
            },
            _ = tick.tick() => {
                if dirty {
                    dirty = false;
                    let t0 = std::time::Instant::now();
                    let snap = c.snapshot().await.map_err(|e| e.to_string())?;
                    sh.lock().unwrap().rtt_ms = Some(t0.elapsed().as_secs_f32() * 1000.0);
                    apply(sh, snap);
                } else if ping.elapsed() > Duration::from_secs(2) {
                    // the System panel's response time
                    ping = std::time::Instant::now();
                    let t0 = std::time::Instant::now();
                    if c.status().await.is_ok() {
                        sh.lock().unwrap().rtt_ms = Some(t0.elapsed().as_secs_f32() * 1000.0);
                    }
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
        Cmd::Interrupt { task } => c.interrupt(&task).await.map_err(|e| format!("could not interrupt: {e}")),
        Cmd::Stop { task } => c.stop(&task).await.map_err(|e| format!("could not stop: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sphere_proto::{Event, EventBody, StepKind};

    fn ev(body: EventBody) -> Event {
        Event { seq: 0, task: "t1".into(), at: "2026-10-17T00:00:00.000Z".into(), body }
    }

    #[test]
    fn a_files_reason_is_what_the_agent_said_before_editing_it() {
        let mut r = Run::default();
        r.apply(&ev(EventBody::Say { text: "Reading first.".into() }));
        r.apply(&ev(EventBody::Step { id: "a".into(), step_kind: StepKind::Read, tool: "Read".into(), title: "src/lib.rs".into(), detail: String::new(), parent: None }));
        r.apply(&ev(EventBody::Say { text: "Check the budget before taking the lock.".into() }));
        r.apply(&ev(EventBody::Step { id: "b".into(), step_kind: StepKind::Edit, tool: "Edit".into(), title: "Editing src/core.rs".into(), detail: String::new(), parent: None }));
        r.apply(&ev(EventBody::Say { text: "Done.".into() }));
        assert_eq!(r.reason_for("src/core.rs").map(|x| x.0), Some("Check the budget before taking the lock."));
        assert!(r.reason_for("src/other.rs").is_none());
    }
}

#[cfg(test)]
mod link_tests {
    use super::*;

    #[test]
    fn edge_text_follows_the_link() {
        assert_eq!(edge_word(Some(&Link::Connected), false), "ONLINE");
        assert_eq!(edge_word(Some(&Link::Connecting), false), "CONNECTING…");
        assert_eq!(edge_word(Some(&Link::Starting), false), "CONNECTING…");
        assert_eq!(edge_word(None, false), "CONNECTING…");
        assert_eq!(edge_word(Some(&Link::Down("x".into())), false), "OFFLINE");
        assert_eq!(edge_word(None, true), "ONLINE");
    }

    #[test]
    fn link_line_says_status_and_response_time() {
        assert_eq!(link_line(Some(&Link::Connected), Some(4.2), false), "sphered connected · 4 ms");
        assert_eq!(link_line(Some(&Link::Connected), Some(0.3), false), "sphered connected · <1 ms");
        assert_eq!(link_line(Some(&Link::Connected), Some(1500.0), false), "sphered connected · 1.5 s");
        assert_eq!(link_line(Some(&Link::Connected), None, false), "sphered connected");
        assert_eq!(link_line(Some(&Link::Starting), None, false), "sphered connecting…");
        assert_eq!(link_line(Some(&Link::Down(String::new())), None, false), "sphered down");
    }
}
