//! The app's link to sphered. A background thread runs a small tokio runtime: it starts sphered when nothing
//! answers on the socket, connects as the app, takes one snapshot, subscribes, and from then on folds each event into
//! its own copy of sphered's views (with the code sphered itself runs), so the shared slot the Bevy systems read is
//! always current and the link asks for another snapshot only after it missed an event. Commands (start a task,
//! answer a request) go the other way on a channel.
use bevy::prelude::*;
use sphere_client::{Client, ClientKind, StartParams, Update};
use sphere_proto::api::Snapshot;
use sphere_proto::{AnswerKind, Event, EventBody, Note, NoteSource, Protocol, RequestView, TaskState, TaskView, Views};
use std::collections::HashMap;
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
    /// every task, in the order they were created
    pub tasks: Vec<TaskView>,
    pub requests: Vec<RequestView>,
    pub version: u64,
    /// Toasts for the overlay: errors from commands, "started", ...
    pub notes: Vec<String>,
    /// how long the last ping took to come back
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
    /// Finer counters for work that only some changes call for. A system that remembers the last value it saw skips
    /// its pass while the counter has not moved. `roster`: a task joined the list (the titles). `states`: a task
    /// changed state (who finished or failed). `spans`: an arc of the Timeline started, ended or moved.
    pub roster: u64,
    pub states: u64,
    pub spans: u64,
    /// Claude sessions running outside Sphere (`session.outside`), polled while the window is visible
    pub outside: Vec<sphere_proto::api::OutsideSession>,
    /// the window is hidden, minimised or fully covered: nothing polls (set by `takeover::watch_visibility`)
    pub hidden: bool,
    /// take-overs that came back, for `takeover::finish` to pick up: (the session id asked for, the outcome)
    pub taken: Vec<(String, Result<sphere_proto::api::TakeOverResult, String>)>,
    /// seconds agents waited on you today over requests already closed (the mirror's; `Live` adds the open ones)
    pub waited_today_secs: u64,
    /// the user's notes for agents, newest first (the snapshot's, read again whenever sphered says they changed)
    pub memory: Vec<Note>,
    /// moves with `memory`, so `memory::sync` copies the notes only when they changed
    pub memory_rev: u64,
    /// the saved protocols, oldest first (the mirror's); `protocols_rev` moves whenever one is saved, changed or removed
    pub protocols: Vec<Protocol>,
    pub protocols_rev: u64,
    /// Toasts for something that went through ("Protocol saved · next Mon 09:00", "Remembered for sphere · 4 notes"):
    /// drawn with the green bar
    pub good: Vec<String>,
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
    /// where the session ran before it was taken over (`herdr`, `Terminal`, ...)
    pub taken_from: Option<String>,
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
            B::TakenOver { from } => self.taken_from = Some(from.clone()),
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
    /// `title`: the agent's name; none lets sphered take it from the prompt. `in_place`: the agent works in the repo
    /// itself and not in a worktree of its own.
    Start { repo: String, prompt: String, title: Option<String>, in_place: bool },
    /// `always`: also save the rule the agent suggested (a yes only)
    Answer { request: String, answer: AnswerKind, text: Option<String>, always: bool },
    Send { task: String, text: String },
    Interrupt { task: String },
    Stop { task: String },
    /// Sphere is on screen again: the link starts pinging and looking for outside sessions (both stop while the
    /// window is hidden).
    Shown,
    /// Stop an outside session where it runs and resume it here (`session.take_over`).
    TakeOver { session_id: String },
    /// Keep a note for a repo (a folder inside it will do), or for every repo with `None`.
    Remember { repo: Option<String>, text: String, source: NoteSource },
    /// Drop a note.
    Forget { id: u64 },
    /// Save a protocol from the line as typed (`protocol.add`); `repo` is the folder for a line that names none.
    ProtocolAdd { text: String, repo: Option<String> },
    /// Turn a protocol on or off (`protocol.set_enabled`).
    ProtocolEnable { id: String, enabled: bool },
    /// Run a protocol now, besides its schedule (`protocol.run_now`).
    ProtocolRunNow { id: String },
    ProtocolRemove { id: String },
}

#[derive(Resource, Clone)]
pub struct Service {
    pub shared: Arc<Mutex<Shared>>,
    tx: mpsc::UnboundedSender<Cmd>,
    /// No sphered behind it (screenshots, `--demo`): commands go nowhere, and the few things that can be done without
    /// one are done in the shared slot (see `memory`).
    pub offline: bool,
}

impl Service {
    pub fn send(&self, c: Cmd) {
        let _ = self.tx.send(c);
    }

    /// A service with no link thread, whose commands can be read back from the receiver (tests).
    #[cfg(test)]
    pub fn with_receiver() -> (Service, mpsc::UnboundedReceiver<Cmd>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Service { shared: Arc::new(Mutex::new(Shared::default())), tx, offline: false }, rx)
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
    Service { shared, tx, offline }
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

/// Takes a new list of notes; `memory_rev` moves only when it differs.
fn set_memory(s: &mut Shared, list: Vec<Note>) {
    if s.memory != list {
        s.memory = list;
        s.memory_rev += 1;
        s.version += 1;
    }
}

/// Reads the notes again, after sphered said they changed.
async fn read_notes(c: &Client, sh: &Mutex<Shared>) {
    if let Ok(list) = c.memory_list(None).await {
        set_memory(&mut sh.lock().unwrap(), list);
    }
}

/// A toast for something that went through (the green bar).
fn good(sh: &Mutex<Shared>, n: impl Into<String>) {
    let mut s = sh.lock().unwrap();
    s.good.push(n.into());
    s.version += 1;
}

/// Whether a task still runs or waits: its arc on the Timeline has no end yet.
pub fn is_live(t: &TaskView) -> bool {
    matches!(t.state, TaskState::Queued | TaskState::Working | TaskState::NeedsYou)
}

/// The UTC day (`YYYY-MM-DD`), the day sphered counts a cost under.
fn today() -> String {
    sphere_proto::now_rfc3339()[..10].to_string()
}

/// The app's copy of sphered's views. Each event folds into it with `Views::apply`, the code sphered itself runs, so
/// what the app shows is exact and no snapshot is fetched after the first. It holds only the open requests (a closed
/// one is never looked at again, and `apply` scans the ones it holds on each step). `at` says where each task sits in
/// `Shared::tasks`, which keeps the snapshot's order (creation) and which every other system reads.
struct Mirror {
    views: Views,
    at: HashMap<String, usize>,
}

impl Mirror {
    /// The mirror of a snapshot. The shared slot takes its tasks, open requests and cost, and every counter moves.
    fn connect(snap: Snapshot, s: &mut Shared) -> Mirror {
        s.tasks = snap.tasks.clone();
        s.requests = snap.requests.clone();
        s.max_concurrent = snap.max_concurrent;
        set_memory(s, snap.memory.clone());
        let at: HashMap<String, usize> = s.tasks.iter().enumerate().map(|(i, t)| (t.id.clone(), i)).collect();
        let day = today();
        let views = Views::from_snapshot(snap, &day);
        s.cost_today_usd = views.cost_on(&day);
        s.waited_today_secs = views.waited_on(&day);
        s.protocols = views.protocols().into_iter().cloned().collect();
        s.protocols_rev += 1;
        // only a finished task can read merged
        s.merged.retain(|id| at.get(id).is_some_and(|&i| s.tasks[i].state == TaskState::Done));
        s.roster += 1;
        s.states += 1;
        s.spans += 1;
        s.version += 1;
        Mirror { views, at }
    }

    /// Folds one event into the mirror and the shared slot, in the order sphered saw it. An event already folded (the
    /// replay overlaps the snapshot) is skipped. A gap means events were lost: the caller starts over from a snapshot.
    fn fold(&mut self, ev: &Event, s: &mut Shared) -> Result<(), String> {
        let seen = self.views.seq;
        if ev.seq <= seen {
            return Ok(());
        }
        if ev.seq != seen + 1 {
            return Err(format!("missed events {}..{}, resyncing", seen + 1, ev.seq - 1));
        }
        // an event of no task (a note changed, a protocol saved or removed) is nobody's run
        if !ev.task.is_empty() {
            match s.runs.get_mut(&ev.task) {
                Some(r) => r.apply(ev),
                None => {
                    let mut r = Run::default();
                    r.apply(ev);
                    s.runs.insert(ev.task.clone(), r);
                }
            }
        }
        self.views.apply(ev);
        if matches!(ev.body, EventBody::ProtocolSaved { .. } | EventBody::ProtocolRemoved { .. }) {
            s.protocols = self.views.protocols().into_iter().cloned().collect();
            s.protocols_rev += 1;
        }
        if matches!(ev.body, EventBody::Request { .. } | EventBody::RequestSnoozed { .. } | EventBody::RequestAnswered { .. } | EventBody::RequestWithdrawn { .. }) {
            self.views.forget_closed_requests();
            s.requests = self.views.open_requests().into_iter().cloned().collect();
        }
        // `apply` changes the event's task and no other
        if let Some(t) = self.views.task(&ev.task) {
            put(&mut self.at, t, s);
        }
        let day = today();
        let cost = self.views.cost_on(&day);
        if cost != s.cost_today_usd {
            s.cost_today_usd = cost;
        }
        s.waited_today_secs = self.views.waited_on(&day);
        s.version += 1;
        Ok(())
    }
}

/// Puts a task's new view in its place in the list (or at the end, if it is new) and moves the counters it concerns.
fn put(at: &mut HashMap<String, usize>, t: &TaskView, s: &mut Shared) {
    let Some(&i) = at.get(&t.id) else {
        at.insert(t.id.clone(), s.tasks.len());
        s.tasks.push(t.clone());
        s.roster += 1;
        s.states += 1;
        s.spans += 1;
        return;
    };
    let old = &s.tasks[i];
    if old.state != t.state {
        s.states += 1;
    }
    // a running task's arc stays open; a finished one's ends when it last changed
    if is_live(old) != is_live(t) || (!is_live(t) && old.updated_at != t.updated_at) {
        s.spans += 1;
    }
    if t.state != TaskState::Done && !s.merged.is_empty() {
        s.merged.remove(&t.id);
    }
    s.tasks[i] = t.clone();
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

/// The sphered socket, and whether it is the app's own. SPHERE_SOCKET points the app at another sphered (tests, a
/// second state dir); the app then never starts or restarts one itself.
fn socket() -> (PathBuf, bool) {
    match std::env::var_os("SPHERE_SOCKET") {
        Some(s) => (PathBuf::from(s), false),
        None => (sphere_proto::paths::default_socket(), true),
    }
}

async fn connect(sh: &Mutex<Shared>) -> Client {
    let (socket, own) = socket();
    let mut started = !own;
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
    // what was done about an out-of-date sphered, kept across reconnects so it happens once
    let mut up = crate::restart::Upgrade::default();
    loop {
        let client = connect(&sh).await;
        if let Err(e) = session(&sh, &client, &mut rx, &mut up).await {
            set_link(&sh, Link::Down(e));
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        set_link(&sh, Link::Connecting);
    }
}

/// How often the System panel's response time is measured while Sphere is on screen.
const PING: Duration = Duration::from_secs(2);

/// One connection: take a snapshot, subscribe from its seq, then fold every event into the shared slot as it arrives
/// and serve commands. A snapshot is taken again only if an event was missed (or one is not understood): the mirror
/// starts over from it and goes on. Sphered dropping us ends the session and the next one starts afresh. The two
/// timers, the ping and the look for Claude sessions running outside Sphere, run only while Sphere is on screen (the
/// window coming back wakes them with `Cmd::Shown`). An out-of-date sphered is restarted once no agent runs (`restart`):
/// checked on connecting and, while the versions differ, after each event.
async fn session(sh: &Arc<Mutex<Shared>>, c: &Client, rx: &mut mpsc::UnboundedReceiver<Cmd>, up: &mut crate::restart::Upgrade) -> Result<(), String> {
    c.hello(ClientKind::App).await.map_err(|e| e.to_string())?;
    let daemon = c.status().await.map_err(|e| e.to_string())?.version;
    let snap = c.snapshot().await.map_err(|e| e.to_string())?;
    if upgrade(sh, up, &daemon, running(&snap.tasks)).await {
        return Ok(());
    }
    let seq = snap.seq;
    // backfill the runs of every active task and the most recent others up to the snapshot (the swarm shows each
    // active agent's steps), then the subscription carries on
    let mut recent: Vec<&TaskView> = snap.tasks.iter().collect();
    recent.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    let ids: Vec<String> = recent.iter().enumerate().filter(|(i, t)| *i < 8 || is_live(t)).map(|(_, t)| t.id.clone()).collect();
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
    let mut mirror = {
        let mut s = sh.lock().unwrap();
        s.runs = runs;
        Mirror::connect(snap, &mut s)
    };
    let (_, mut sub) = c.subscribe(seq, None).await.map_err(|e| e.to_string())?;
    set_link(sh, Link::Connected);
    let ping = tokio::time::sleep(PING);
    tokio::pin!(ping);
    let mut armed = crate::summon::on_screen();
    // Claude sessions running outside Sphere: asked at once, then every `OUTSIDE_EVERY` while the window can be seen
    let outside = tokio::time::sleep(Duration::ZERO);
    tokio::pin!(outside);
    let mut outside_on = true;
    let mut outside_at: Option<std::time::Instant> = None;
    let outside_busy = Arc::new(std::sync::atomic::AtomicBool::new(false));
    loop {
        tokio::select! {
            u = sub.next() => match u {
                Some(Update::Event(ev)) => {
                    let folded = mirror.fold(&ev, &mut sh.lock().unwrap());
                    if let Err(gap) = folded {
                        // an event never came (or was not understood): start over from sphered's own picture, which
                        // already covers `ev`; if it does not, there is nothing to rely on, and the session ends
                        let snap = c.snapshot().await.map_err(|e| e.to_string())?;
                        mirror = Mirror::connect(snap, &mut sh.lock().unwrap());
                        mirror.fold(&ev, &mut sh.lock().unwrap()).map_err(|e| format!("{gap}; {e}"))?;
                    }
                    // the notes changed (here, in the CLI, anywhere): read them again, before the next event
                    if matches!(ev.body, EventBody::MemoryChanged { .. }) {
                        read_notes(c, sh).await;
                    }
                    // an out-of-date sphered waiting for the agents to finish
                    if daemon != env!("CARGO_PKG_VERSION") {
                        let n = running(&sh.lock().unwrap().tasks);
                        if upgrade(sh, up, &daemon, n).await {
                            return Ok(());
                        }
                    }
                }
                // typing frames: the app shows nothing of them
                Some(Update::Live(_)) => {}
                Some(Update::Lagged(e)) => return Err(format!("resyncing: {}", e.message)),
                Some(Update::Closed) | None => return Err("sphered closed the connection".into()),
            },
            _ = &mut outside, if outside_on => {
                let busy = outside_busy.load(std::sync::atomic::Ordering::Relaxed);
                let hidden = sh.lock().unwrap().hidden || !crate::summon::on_screen();
                if outside_due(hidden, busy, outside_at.map(|t| t.elapsed())) {
                    outside_at = Some(std::time::Instant::now());
                    outside_busy.store(true, std::sync::atomic::Ordering::Relaxed);
                    let (c, sh, busy) = (c.clone(), sh.clone(), outside_busy.clone());
                    tokio::spawn(async move {
                        if let Ok(list) = c.session_outside().await {
                            set_outside(&sh, list);
                        }
                        busy.store(false, std::sync::atomic::Ordering::Relaxed);
                    });
                }
                if hidden {
                    // nobody sees the list: the timer stays off until `Shown`, and the first look then is at once
                    outside_on = false;
                    outside_at = None;
                } else {
                    // the last ask still out: look again shortly
                    outside.as_mut().reset(tokio::time::Instant::now() + if busy { Duration::from_millis(250) } else { OUTSIDE_EVERY });
                }
            }
            _ = &mut ping, if armed => {
                if crate::summon::on_screen() {
                    // the System panel's response time
                    let t0 = std::time::Instant::now();
                    if c.status().await.is_ok() {
                        sh.lock().unwrap().rtt_ms = Some(t0.elapsed().as_secs_f32() * 1000.0);
                    }
                    ping.as_mut().reset(tokio::time::Instant::now() + PING);
                } else {
                    // hidden: nobody sees the number, so the timer stays off until `Shown`
                    armed = false;
                }
            }
            cmd = rx.recv() => {
                let Some(cmd) = cmd else { return Ok(()) };
                if matches!(cmd, Cmd::Shown) {
                    if !armed {
                        armed = true;
                        ping.as_mut().reset(tokio::time::Instant::now());
                    }
                    if !outside_on {
                        outside_on = true;
                        outside.as_mut().reset(tokio::time::Instant::now());
                    }
                    continue;
                }
                let c = c.clone();
                let sh = sh.clone();
                // Commands run beside the loop so a slow start never stalls updates.
                tokio::spawn(async move {
                    if let Err(e) = command(&c, cmd, &sh).await {
                        note(&sh, e);
                    }
                });
            }
        }
    }
}

/// Agents sphered is running or about to: queued, working or waiting on you.
fn running(tasks: &[TaskView]) -> usize {
    tasks.iter().filter(|t| matches!(t.state, sphere_proto::TaskState::Queued | sphere_proto::TaskState::Working | sphere_proto::TaskState::NeedsYou)).count()
}

/// Acts on a sphered whose version differs from the app's (see `restart`). True when sphered was stopped, so the
/// session ends and the reconnect starts the bundled one. Only the app's own sphered is touched; sphere-host never.
async fn upgrade(sh: &Mutex<Shared>, up: &mut crate::restart::Upgrade, daemon: &str, running: usize) -> bool {
    use crate::restart::{words, Act};
    let app = env!("CARGO_PKG_VERSION");
    let (socket, own) = socket();
    if !own {
        return false;
    }
    let Some(act) = up.step(app, daemon, running) else { return false };
    if act != Act::Restart {
        if let Some(w) = words(act, app, daemon) {
            note(sh, w);
        }
        return false;
    }
    let stopped = tokio::task::spawn_blocking(move || crate::restart::stop_sphered(&crate::restart::Real, &socket, Duration::from_secs(5))).await.map_err(|e| e.to_string()).and_then(|r| r);
    match stopped {
        Ok(()) => {
            set_link(sh, Link::Starting);
            true
        }
        Err(e) => {
            up.give_up();
            note(sh, format!("Background service is out of date (sphered {daemon}, app {app}); could not restart it: {e}"));
            false
        }
    }
}

/// How often `session.outside` is asked while the window is visible.
pub const OUTSIDE_EVERY: Duration = Duration::from_secs(5);

/// Whether to ask for the outside sessions now: never while the window is hidden, minimised or covered, not while
/// the last ask is still out, and then once at the start and every `OUTSIDE_EVERY`.
pub fn outside_due(hidden: bool, busy: bool, since_last: Option<Duration>) -> bool {
    !hidden && !busy && since_last.is_none_or(|d| d >= OUTSIDE_EVERY)
}

fn set_outside(sh: &Mutex<Shared>, list: Vec<sphere_proto::api::OutsideSession>) {
    let mut s = sh.lock().unwrap();
    if s.outside != list {
        s.outside = list;
        s.version += 1;
    }
}

/// What sphered is asked for by a `Cmd::Start`: the budget, mode and model are its own defaults.
fn start_params(repo: String, prompt: String, title: Option<String>, in_place: bool) -> StartParams {
    StartParams { repo, prompt, title, in_place, budget_usd: None, mode: None, model: None, allow_rewrites: false }
}

async fn command(c: &Client, cmd: Cmd, sh: &Mutex<Shared>) -> Result<(), String> {
    use sphere_proto::api::kind;
    match cmd {
        Cmd::TakeOver { session_id } => {
            let r = c.take_over(&session_id).await.map_err(|e| e.to_string());
            let mut s = sh.lock().unwrap();
            s.taken.push((session_id, r));
            s.version += 1;
            Ok(())
        }
        Cmd::Start { repo, prompt, title, in_place } => {
            let p = start_params(repo, prompt, title, in_place);
            c.start_task(p).await.map(|_| ()).map_err(|e| format!("could not start: {e}"))
        }
        Cmd::Answer { request, answer, text, always } => {
            let p = sphere_client::AnswerParams { request, answer, text, input: None, always };
            match c.answer(p).await {
                Ok(()) => Ok(()),
                // answered elsewhere (the card, the CLI, a notification) or gone: nothing left to answer, so not an error
                Err(e) if matches!(e.kind(), Some(k) if k == kind::ALREADY_ANSWERED || k == kind::NOT_FOUND || k == kind::WITHDRAWN) => Ok(()),
                Err(e) => Err(format!("could not answer: {e}")),
            }
        }
        Cmd::Send { task, text } => c.send(&task, &text).await.map_err(|e| format!("could not send: {e}")),
        Cmd::Interrupt { task } => c.interrupt(&task).await.map_err(|e| format!("could not interrupt: {e}")),
        Cmd::Stop { task } => c.stop(&task).await.map_err(|e| format!("could not stop: {e}")),
        Cmd::Remember { repo, text, source } => {
            let added = c.memory_add(repo.as_deref(), &text, source).await.map_err(|e| format!("could not remember: {e}"))?;
            good(sh, crate::memory::remembered(&added));
            Ok(())
        }
        Cmd::Forget { id } => match c.memory_remove(id).await {
            Ok(()) => Ok(()),
            // removed elsewhere in the meantime: it is gone, which is what was asked
            Err(e) if matches!(e.kind(), Some(k) if k == kind::NOT_FOUND) => Ok(()),
            Err(e) => Err(format!("could not forget: {e}")),
        },
        Cmd::ProtocolAdd { text, repo } => {
            let p = c.protocol_add(&text, repo.as_deref()).await.map_err(|e| format!("could not save the protocol: {e}"))?;
            good(sh, saved_words(&p, crate::core::unix_now()));
            Ok(())
        }
        Cmd::ProtocolEnable { id, enabled } => {
            let p = c.protocol_set_enabled(&id, enabled).await.map_err(|e| format!("could not turn it {}: {e}", if enabled { "on" } else { "off" }))?;
            good(sh, if enabled { format!("Protocol on · {}", next_words(&p, crate::core::unix_now())) } else { "Protocol off".to_string() });
            Ok(())
        }
        Cmd::ProtocolRunNow { id } => {
            c.protocol_run_now(&id).await.map_err(|e| format!("could not run it: {e}"))?;
            good(sh, "Protocol running");
            Ok(())
        }
        Cmd::ProtocolRemove { id } => {
            c.protocol_remove(&id).await.map_err(|e| format!("could not remove it: {e}"))?;
            good(sh, "Protocol removed");
            Ok(())
        }
        Cmd::Shown => Ok(()),
    }
}

/// "next Mon 09:00": when a protocol runs next, in local time (a one-shot that has passed or a protocol that is off has none).
pub fn next_words(p: &Protocol, now: i64) -> String {
    match p.next_run {
        Some(t) => format!("next {}", sphere_proto::protocol::when_words(t, now, &sphere_proto::protocol::LocalZone)),
        None => "no next run".to_string(),
    }
}

/// The toast after a protocol is saved: "Protocol saved · next Mon 09:00".
pub fn saved_words(p: &Protocol, now: i64) -> String {
    format!("Protocol saved · {}", next_words(p, now))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sphere_proto::StepKind;

    #[test]
    fn the_outside_sessions_are_asked_only_while_the_window_is_visible() {
        let s = Duration::from_secs;
        // visible: at once, then every five seconds
        assert!(outside_due(false, false, None));
        assert!(!outside_due(false, false, Some(s(2))));
        assert!(outside_due(false, false, Some(s(5))));
        // hidden, minimised or covered: never, however long ago it was
        assert!(!outside_due(true, false, None));
        assert!(!outside_due(true, false, Some(s(60))));
        // not while the last ask is still out
        assert!(!outside_due(false, true, Some(s(60))));
    }

    #[test]
    fn a_start_asks_for_a_worktree_or_for_the_repo_itself() {
        for in_place in [false, true] {
            let p = start_params("/work/sphere".into(), "fix the test".into(), Some("Fix: make".into()), in_place);
            assert_eq!((p.repo.as_str(), p.prompt.as_str(), p.title.as_deref(), p.in_place), ("/work/sphere", "fix the test", Some("Fix: make"), in_place));
            assert!(p.budget_usd.is_none() && p.mode.is_none() && p.model.is_none() && !p.allow_rewrites);
        }
    }

    #[test]
    fn a_taken_over_event_marks_the_run() {
        let mut r = Run::default();
        let v = r.version;
        r.apply(&ev(EventBody::TakenOver { from: "herdr".into() }));
        assert_eq!(r.taken_from.as_deref(), Some("herdr"));
        assert!(r.version > v);
    }

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

    // ---- the mirror

    /// A moment long ago, so no cost counts as today's unless a test says so.
    fn at(sec: u32) -> String {
        format!("2020-01-01T00:00:{sec:02}.000Z")
    }

    fn event(seq: u64, task: &str, body: EventBody) -> Event {
        Event { seq, task: task.into(), at: at(seq as u32 % 60), body }
    }

    fn created(seq: u64, task: &str) -> Event {
        event(seq, task, EventBody::TaskCreated { title: format!("Task {task}"), prompt: "p".into(), repo: "/r".into(), in_place: false, budget_usd: 1.0, mode: None, model: None, allow_rewrites: false, resume_session: None, parent_task: None, protocol: None })
    }

    fn started(seq: u64, task: &str) -> Event {
        event(seq, task, EventBody::AgentStarted { child: "c".into(), pid: 7, resumed: false, briefed: 0 })
    }

    fn step(seq: u64, task: &str, id: &str) -> Event {
        event(seq, task, EventBody::Step { id: id.into(), step_kind: StepKind::Run, tool: "Bash".into(), title: format!("Running {id}"), detail: String::new(), parent: None })
    }

    fn step_done(seq: u64, task: &str, id: &str) -> Event {
        event(seq, task, EventBody::StepDone { id: id.into(), ok: true, output: None })
    }

    fn turn(seq: u64, task: &str, cost: f64) -> Event {
        event(seq, task, EventBody::Turn { ok: true, interrupted: false, summary: "all done".into(), turns: Some(1), duration_ms: None, cost_usd: Some(cost), cost_delta_usd: cost })
    }

    fn ask(seq: u64, task: &str, id: &str) -> Event {
        let info = sphere_proto::RequestInfo {
            id: id.into(),
            task: task.into(),
            kind: sphere_proto::RequestKind::Permission,
            tool: "Bash".into(),
            display_name: None,
            description: None,
            summary: String::new(),
            input: serde_json::json!({"command": "git push"}),
            suggestions: vec![],
            blocked_path: None,
            tool_use_id: None,
            agent_request: None,
            revises: None,
        };
        event(seq, task, EventBody::Request { request: info })
    }

    fn answer(seq: u64, task: &str, id: &str) -> Event {
        event(seq, task, EventBody::RequestAnswered { request: id.into(), answer: AnswerKind::Yes, text: None, always: false })
    }

    fn empty_snapshot() -> Snapshot {
        Snapshot { seq: 0, tasks: vec![], requests: vec![], cost_today_usd: 0.0, daily_cap_usd: None, max_concurrent: Some(3), waited_today_secs: 0, memory: vec![], protocols: vec![] }
    }

    fn kept(id: u64, repo: Option<&str>, text: &str) -> Note {
        Note { id, repo: repo.map(str::to_string), text: text.into(), created_at: at(1), source: NoteSource::Typed }
    }

    fn memory_changed(seq: u64, notes: u32) -> Event {
        event(seq, "", EventBody::MemoryChanged { notes })
    }

    /// A mirror next to the server's own views: both fed the same events.
    struct Pair {
        server: Views,
        mirror: Mirror,
        s: Shared,
    }

    impl Pair {
        fn new() -> Pair {
            let mut s = Shared::default();
            let mirror = Mirror::connect(empty_snapshot(), &mut s);
            Pair { server: Views::default(), mirror, s }
        }
        fn feed(&mut self, e: Event) {
            self.server.apply(&e);
            self.mirror.fold(&e, &mut self.s).unwrap();
            self.same();
        }
        fn same(&self) {
            let tasks: Vec<TaskView> = self.server.tasks().into_iter().cloned().collect();
            assert_eq!(self.s.tasks, tasks, "tasks");
            let open: Vec<RequestView> = self.server.open_requests().into_iter().cloned().collect();
            assert_eq!(self.s.requests, open, "open requests");
            assert_eq!(self.s.cost_today_usd, self.server.cost_on(&today()), "cost");
            assert_eq!(self.s.waited_today_secs, self.server.waited_on(&today()), "waited");
            let protocols: Vec<Protocol> = self.server.protocols().into_iter().cloned().collect();
            assert_eq!(self.s.protocols, protocols, "protocols");
        }
        fn counters(&self) -> (u64, u64, u64) {
            (self.s.roster, self.s.states, self.s.spans)
        }
    }

    #[test]
    fn events_fold_into_the_slot_like_a_fresh_snapshot_would_read() {
        let mut p = Pair::new();
        let now = sphere_proto::now_rfc3339();
        let mut seq = 0;
        let mut next = |mut e: Event| {
            seq += 1;
            e.seq = seq;
            e
        };
        for t in ["t1", "t2", "t3"] {
            p.feed(next(created(0, t)));
            p.feed(next(started(0, t)));
        }
        p.feed(next(step(0, "t1", "a")));
        p.feed(next(ask(0, "t1", "r1")));
        p.feed(next(step(0, "t2", "b")));
        p.feed(next(ask(0, "t2", "r2")));
        assert_eq!(p.s.requests.len(), 2);
        assert_eq!(p.s.tasks[0].state, TaskState::NeedsYou);
        p.feed(next(answer(0, "t1", "r1")));
        assert_eq!(p.s.requests.len(), 1);
        assert_eq!(p.s.tasks[0].state, TaskState::Working);
        p.feed(next(step_done(0, "t1", "a")));
        let mut done = turn(0, "t1", 0.5);
        done.at = now;
        p.feed(next(done));
        assert_eq!(p.s.tasks[0].state, TaskState::Done);
        assert_eq!(p.s.cost_today_usd, 0.5, "a turn today counts today");
        p.feed(next(event(0, "t3", EventBody::Say { text: "hm".into() })));
        p.feed(next(event(0, "t9", EventBody::Say { text: "who".into() })));
        p.feed(next(event(0, "t1", EventBody::Warning { message: "careful".into() })));
        p.feed(next(created(0, "t4")));
        assert_eq!(p.s.tasks.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["t1", "t2", "t3", "t4"]);
        assert_eq!(p.s.runs["t1"].steps.len(), 1, "the run follows the steps");
        assert_eq!(p.s.runs["t3"].says, ["hm"]);
    }

    #[test]
    fn a_mirror_started_from_a_snapshot_goes_on_where_the_server_is() {
        let mut server = Views::default();
        for e in [created(1, "t1"), started(2, "t1"), step(3, "t1", "a"), ask(4, "t1", "r1"), created(5, "t2")] {
            server.apply(&e);
        }
        let snap = Snapshot { seq: server.seq, tasks: server.tasks().into_iter().cloned().collect(), requests: server.open_requests().into_iter().cloned().collect(), cost_today_usd: 1.25, daily_cap_usd: None, max_concurrent: Some(2), waited_today_secs: 90, memory: vec![], protocols: server.protocols().into_iter().cloned().collect() };
        let mut s = Shared::default();
        let mut mirror = Mirror::connect(snap, &mut s);
        assert_eq!((s.tasks.len(), s.requests.len(), s.cost_today_usd, s.max_concurrent, s.waited_today_secs), (2, 1, 1.25, Some(2), 90));
        assert!(s.roster > 0 && s.states > 0 && s.spans > 0 && s.version > 0);
        for e in [answer(6, "t1", "r1"), step_done(7, "t1", "a"), turn(8, "t1", 0.5)] {
            server.apply(&e);
            mirror.fold(&e, &mut s).unwrap();
        }
        let tasks: Vec<TaskView> = server.tasks().into_iter().cloned().collect();
        assert_eq!(s.tasks, tasks);
        assert!(s.requests.is_empty());
        assert_eq!(s.cost_today_usd, 1.25, "a turn on another day leaves today's cost alone");
        assert_eq!(s.waited_today_secs, 90, "so does a request answered on another day");
    }

    #[test]
    fn the_notes_come_with_the_snapshot_and_move_their_counter_only_when_they_change() {
        let mut s = Shared::default();
        let snap = Snapshot { memory: vec![kept(2, Some("/r"), "Use pnpm"), kept(1, None, "Keep commits small")], ..empty_snapshot() };
        Mirror::connect(snap.clone(), &mut s);
        assert_eq!((s.memory.len(), s.memory_rev), (2, 1));
        // a resync with the same notes is not news; with others it is
        let v = s.version;
        Mirror::connect(snap, &mut s);
        assert_eq!((s.memory_rev, s.version > v), (1, true), "the version moves with the snapshot, the notes' counter does not");
        Mirror::connect(empty_snapshot(), &mut s);
        assert_eq!((s.memory.len(), s.memory_rev), (0, 2));
    }

    #[test]
    fn a_notes_event_belongs_to_no_task_and_leaves_the_runs_alone() {
        let mut p = Pair::new();
        p.feed(created(1, "t1"));
        p.feed(memory_changed(2, 1));
        p.feed(step(3, "t1", "a"));
        assert_eq!(p.s.runs.keys().collect::<Vec<_>>(), ["t1"], "no run for the empty task");
        assert_eq!(p.s.runs["t1"].steps.len(), 1);
    }

    #[test]
    fn the_counters_move_with_what_they_stand_for() {
        let mut p = Pair::new();
        let c0 = p.counters();
        p.feed(created(1, "t1"));
        let c1 = p.counters();
        assert_eq!((c1.0 - c0.0, c1.1 - c0.1, c1.2 - c0.2), (1, 1, 1), "a new task moves all three");
        p.feed(started(2, "t1"));
        let c2 = p.counters();
        assert_eq!((c2.0, c2.1, c2.2), (c1.0, c1.1 + 1, c1.2), "queued to working: a new state, the arc is still open");
        let before = p.s.version;
        for (i, e) in [step(3, "t1", "a"), event(4, "t1", EventBody::Say { text: "x".into() }), step_done(5, "t1", "a"), event(6, "t1", EventBody::Warning { message: "w".into() })].into_iter().enumerate() {
            p.feed(e);
            assert_eq!(p.counters(), c2, "event {i}: steps and words change none of them");
        }
        assert_eq!(p.s.version, before + 4, "but each event is a new version");
        p.feed(ask(7, "t1", "r1"));
        let c3 = p.counters();
        assert_eq!((c3.0, c3.1, c3.2), (c2.0, c2.1 + 1, c2.2), "needing you is a state, not a new arc");
        p.feed(answer(8, "t1", "r1"));
        p.feed(turn(9, "t1", 0.1));
        let c4 = p.counters();
        assert_eq!((c4.0, c4.2), (c3.0, c3.2 + 1), "finishing closes the arc");
        assert!(c4.1 > c3.1);
        p.feed(event(10, "t1", EventBody::AgentExited { code: Some(0), signal: None, reason: "exit".into() }));
        let c5 = p.counters();
        assert_eq!((c5.0, c5.1, c5.2), (c4.0, c4.1, c4.2 + 1), "a late event moves the end of a finished arc, not its state");
    }

    #[test]
    fn a_gap_is_reported_and_a_repeat_changes_nothing() {
        let mut p = Pair::new();
        p.feed(created(1, "t1"));
        p.feed(started(2, "t1"));
        let (tasks, v) = (p.s.tasks.clone(), p.s.version);
        // the replay may overlap what the snapshot held
        p.mirror.fold(&started(2, "t1"), &mut p.s).unwrap();
        p.mirror.fold(&created(1, "t1"), &mut p.s).unwrap();
        assert_eq!((p.s.tasks.clone(), p.s.version), (tasks, v));
        // seq 4 without 3: an event was lost
        let err = p.mirror.fold(&step(4, "t1", "a"), &mut p.s).unwrap_err();
        assert!(err.contains("missed events 3..3"), "{err}");
        assert_eq!(p.s.version, v, "nothing was folded");
        p.mirror.fold(&step(3, "t1", "a"), &mut p.s).unwrap();
        assert_eq!(p.s.tasks[0].steps, 1);
    }

    #[test]
    fn a_task_that_goes_back_to_work_is_no_longer_merged() {
        let mut p = Pair::new();
        for e in [created(1, "t1"), created(2, "t2"), started(3, "t1"), started(4, "t2"), turn(5, "t1", 0.1), turn(6, "t2", 0.1)] {
            p.feed(e);
        }
        p.s.merged.insert("t1".into());
        p.s.merged.insert("t2".into());
        p.feed(event(7, "t1", EventBody::UserMessage { text: "more".into() }));
        assert!(!p.s.merged.contains("t1"), "working again");
        assert!(p.s.merged.contains("t2"), "t2 is untouched");
        p.feed(event(8, "t2", EventBody::SessionId { session_id: "s".into() }));
        assert!(p.s.merged.contains("t2"), "a late event on a finished task keeps what was found until the next look");
    }

    #[test]
    fn a_new_connection_prunes_merged_to_finished_tasks() {
        let mut p = Pair::new();
        for e in [created(1, "t1"), created(2, "t2"), started(3, "t1"), turn(4, "t1", 0.1)] {
            p.feed(e);
        }
        p.s.merged.extend(["t1".to_string(), "t2".to_string(), "gone".to_string()]);
        let snap = Snapshot { seq: p.server.seq, tasks: p.server.tasks().into_iter().cloned().collect(), ..empty_snapshot() };
        let m = Mirror::connect(snap, &mut p.s);
        assert_eq!(p.s.merged.iter().cloned().collect::<Vec<_>>(), ["t1"]);
        assert_eq!(m.at["t2"], 1);
    }

    /// A made-up history (requests are closed only while open), fed to the mirror and the server: they agree after
    /// every event.
    #[test]
    fn the_slot_stays_exact_over_a_long_made_up_history() {
        let mut x: u64 = 7;
        let mut next = move |m: u64| {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (x >> 33) % m
        };
        let mut p = Pair::new();
        let (mut made, mut open, mut steps) = (0u64, Vec::<(String, String)>::new(), 0u64);
        for seq in 1..=600u64 {
            let mut task = format!("t{}", 1 + next(5));
            let kind = next(13);
            let e = if made < 5 && (kind < 3 || task == format!("t{}", made + 1)) {
                made += 1;
                task = format!("t{made}");
                created(seq, &task)
            } else {
                match kind {
                    0 | 1 => started(seq, &task),
                    2..=4 => {
                        steps += 1;
                        step(seq, &task, &format!("s{steps}"))
                    }
                    5 => step_done(seq, &task, &format!("s{}", 1 + next(steps.max(1)))),
                    6 => event(seq, &task, EventBody::Say { text: format!("said {seq}") }),
                    7 => {
                        let id = format!("r{seq}");
                        open.push((id.clone(), task.clone()));
                        ask(seq, &task, &id)
                    }
                    8 if !open.is_empty() => {
                        let (id, t) = open.remove(next(open.len() as u64) as usize);
                        answer(seq, &t, &id)
                    }
                    9 if !open.is_empty() => {
                        let (id, t) = open.remove(next(open.len() as u64) as usize);
                        event(seq, &t, EventBody::RequestWithdrawn { request: id, reason: "gone".into() })
                    }
                    10 => turn(seq, &task, 0.125 * next(4) as f64),
                    11 => event(seq, &task, EventBody::UserMessage { text: "again".into() }),
                    _ => event(seq, &task, EventBody::Warning { message: "w".into() }),
                }
            };
            p.feed(e);
        }
        assert_eq!(p.s.tasks.len(), 5);
    }

    #[test]
    fn a_live_task_is_one_that_runs_or_waits() {
        let mut p = Pair::new();
        p.feed(created(1, "t1"));
        assert!(is_live(&p.s.tasks[0]), "queued");
        p.feed(started(2, "t1"));
        p.feed(ask(3, "t1", "r1"));
        assert!(is_live(&p.s.tasks[0]), "needs you");
        p.feed(answer(4, "t1", "r1"));
        p.feed(turn(5, "t1", 0.0));
        assert!(!is_live(&p.s.tasks[0]), "done");
    }

    // ---- protocols

    fn protocol(id: &str, enabled: bool) -> Protocol {
        use sphere_proto::protocol::{Schedule, Weekday};
        Protocol {
            id: id.into(),
            text: format!("every weekday 9:00 sphere: update {id}"),
            schedule: Schedule::Weekly { days: Weekday::WEEKDAYS.to_vec(), hour: 9, minute: 0 },
            repo: "/home/me/Projects/sphere".into(),
            prompt: format!("update {id}"),
            enabled,
            created_at: 1_000,
            last_run: None,
            next_run: enabled.then_some(4_000_000_000),
        }
    }

    fn saved(seq: u64, id: &str, enabled: bool) -> Event {
        event(seq, "", EventBody::ProtocolSaved { protocol: protocol(id, enabled) })
    }

    #[test]
    fn protocols_fold_in_beside_the_tasks_and_are_nobodys_run() {
        let mut p = Pair::new();
        let rev = p.s.protocols_rev;
        p.feed(created(1, "t1"));
        p.feed(saved(2, "p1", true));
        p.feed(saved(3, "p2", true));
        assert_eq!(p.s.protocols.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), ["p1", "p2"]);
        assert_eq!(p.s.protocols_rev, rev + 2);
        assert!(!p.s.runs.contains_key(""), "an event of no task makes no run");
        // saved again: replaced in its place; a task starting changes none of them
        p.feed(saved(4, "p1", false));
        assert!(!p.s.protocols[0].enabled);
        assert_eq!(p.s.protocols_rev, rev + 3);
        p.feed(started(5, "t1"));
        assert_eq!(p.s.protocols_rev, rev + 3, "tasks leave the protocols alone");
        p.feed(event(6, "", EventBody::ProtocolRemoved { id: "p1".into() }));
        assert_eq!(p.s.protocols.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), ["p2"]);
        assert_eq!(p.s.tasks.len(), 1);
    }

    fn created_by(seq: u64, task: &str, protocol: &str) -> Event {
        let mut e = created(seq, task);
        if let EventBody::TaskCreated { protocol: p, .. } = &mut e.body {
            *p = Some(protocol.to_string());
        }
        e
    }

    #[test]
    fn a_snapshot_brings_the_protocols_and_the_tasks_they_started_keep_their_tag() {
        let snap = picture(&[saved(1, "p1", true), created_by(2, "t1", "p1"), started(3, "t1")]);
        let mut s = Shared::default();
        let mut mirror = Mirror::connect(snap, &mut s);
        assert_eq!(s.protocols, vec![protocol("p1", true)]);
        assert_eq!(s.tasks[0].protocol.as_deref(), Some("p1"));
        let rev = s.protocols_rev;
        mirror.fold(&saved(4, "p2", false), &mut s).unwrap();
        assert_eq!(s.protocols.len(), 2);
        assert!(s.protocols_rev > rev);
    }

    #[test]
    fn the_toast_after_saving_says_when_it_runs_next() {
        let p = protocol("p1", true);
        let words = saved_words(&p, 4_000_000_000 - 3 * 3600);
        assert!(words.starts_with("Protocol saved · next ") && words.ends_with(char::is_numeric), "{words}");
        let off = Protocol { enabled: false, next_run: None, ..p };
        assert_eq!(next_words(&off, 0), "no next run");
    }

    // ---- the link, against a stand-in for sphered

    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    /// A sphered that serves one connection. It answers the calls the link makes (the snapshots are the ones given, one
    /// per call and the last one again after that; the history is empty), and once the link subscribes it sends each of
    /// `pushes` (whole messages) and, with `hang_up`, closes. `log` collects the method of every call it gets.
    struct Fake {
        log: Arc<Mutex<Vec<String>>>,
        /// the notes it holds: `memory.list` gives them, `memory.add` and `memory.remove` change them
        notes: Arc<Mutex<Vec<Note>>>,
        path: PathBuf,
    }

    impl Fake {
        fn start(name: &str, snaps: Vec<Snapshot>, pushes: Vec<serde_json::Value>, hang_up: bool) -> Fake {
            let path = std::env::temp_dir().join(format!("sphere-link-{}-{name}.sock", std::process::id()));
            let _ = std::fs::remove_file(&path);
            let listener = tokio::net::UnixListener::bind(&path).unwrap();
            let log = Arc::new(Mutex::new(Vec::new()));
            let seen = log.clone();
            let notes = Arc::new(Mutex::new(Vec::new()));
            let held = notes.clone();
            tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                let (r, mut w) = stream.into_split();
                let mut lines = BufReader::new(r).lines();
                let mut taken = 0;
                while let Ok(Some(line)) = lines.next_line().await {
                    let req: serde_json::Value = serde_json::from_str(&line).unwrap();
                    let method = req["method"].as_str().unwrap().to_string();
                    seen.lock().unwrap().push(method.clone());
                    let result = match method.as_str() {
                        "snapshot" => {
                            taken += 1;
                            serde_json::to_value(&snaps[(taken - 1).min(snaps.len() - 1)]).unwrap()
                        }
                        "history" => serde_json::json!({"events": []}),
                        "protocol.add" | "protocol.set_enabled" => serde_json::to_value(protocol("p1", true)).unwrap(),
                        "protocol.run_now" => serde_json::json!({"task": "t9"}),
                        "subscribe" => serde_json::json!({"seq": snaps[0].seq}),
                        "status" => serde_json::json!({"seq": snaps[0].seq, "version": env!("CARGO_PKG_VERSION"), "active": 0, "queued": 0, "needs_you": 0, "cost_today_usd": 0.0, "tasks": [], "requests": [], "host_pid": null}),
                        "memory.list" => serde_json::json!({"notes": *held.lock().unwrap()}),
                        "memory.add" => {
                            let p = &req["params"];
                            let mut list = held.lock().unwrap();
                            let note = Note { id: list.len() as u64 + 1, repo: p["repo"].as_str().map(str::to_string), text: p["text"].as_str().unwrap().to_string(), created_at: at(1), source: serde_json::from_value(p["source"].clone()).unwrap() };
                            list.insert(0, note.clone());
                            serde_json::json!({"note": note, "added": true, "count": list.len()})
                        }
                        "memory.remove" => {
                            let id = req["params"]["id"].as_u64().unwrap();
                            let found = {
                                let mut list = held.lock().unwrap();
                                let before = list.len();
                                list.retain(|n| n.id != id);
                                list.len() != before
                            };
                            if !found {
                                let gone = serde_json::json!({"jsonrpc": "2.0", "id": req["id"], "error": {"code": -32000, "message": "no such note", "data": {"kind": "not_found"}}});
                                w.write_all(format!("{gone}\n").as_bytes()).await.unwrap();
                                continue;
                            }
                            serde_json::json!({"ok": true})
                        }
                        _ => serde_json::json!({"ok": true}),
                    };
                    let reply = serde_json::json!({"jsonrpc": "2.0", "id": req["id"], "result": result});
                    w.write_all(format!("{reply}\n").as_bytes()).await.unwrap();
                    if method == "subscribe" {
                        for m in &pushes {
                            w.write_all(format!("{m}\n").as_bytes()).await.unwrap();
                        }
                        if hang_up {
                            return;
                        }
                    }
                }
            });
            Fake { log, notes, path }
        }

        /// How many calls of `method` it has had.
        fn calls(&self, method: &str) -> usize {
            self.log.lock().unwrap().iter().filter(|m| *m == method).count()
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    fn pushed(e: &Event) -> serde_json::Value {
        serde_json::json!({"jsonrpc": "2.0", "method": "event", "params": e})
    }

    /// What sphered holds after `events`.
    fn picture(events: &[Event]) -> Snapshot {
        let mut v = Views::default();
        for e in events {
            v.apply(e);
        }
        Snapshot { seq: v.seq, tasks: v.tasks().into_iter().cloned().collect(), requests: v.open_requests().into_iter().cloned().collect(), cost_today_usd: 0.0, daily_cap_usd: None, max_concurrent: Some(3), waited_today_secs: 0, memory: vec![], protocols: v.protocols().into_iter().cloned().collect() }
    }

    /// A sphered at event 3 (one task that works), and the two events it sends next: a question, then a second task.
    fn scene() -> (Snapshot, Vec<serde_json::Value>) {
        (picture(&[created(1, "t1"), started(2, "t1"), step(3, "t1", "a")]), vec![pushed(&ask(4, "t1", "r1")), pushed(&created(5, "t2"))])
    }

    /// Runs `session` on its own task against `fake`.
    async fn connected(fake: &Fake) -> (Arc<Mutex<Shared>>, mpsc::UnboundedSender<Cmd>, tokio::task::JoinHandle<Result<(), String>>) {
        let sh = Arc::new(Mutex::new(Shared::default()));
        let (tx, mut rx) = mpsc::unbounded_channel();
        let client = Client::connect(&fake.path).await.unwrap();
        let s = sh.clone();
        (sh, tx, tokio::spawn(async move { session(&s, &client, &mut rx, &mut crate::restart::Upgrade::default()).await }))
    }

    /// Waits (five seconds at most) for `ok` to hold.
    async fn until(mut ok: impl FnMut() -> bool) {
        for _ in 0..500 {
            if ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("it did not come about in time");
    }

    /// The error a session ends with when the server does `pushes` (and hangs up, if asked).
    async fn ends_with(name: &str, pushes: Vec<serde_json::Value>, hang_up: bool) -> String {
        let (snap, _) = scene();
        let fake = Fake::start(name, vec![snap], pushes, hang_up);
        let (_sh, _tx, task) = connected(&fake).await;
        tokio::time::timeout(Duration::from_secs(5), task).await.expect("the session did not end").unwrap().unwrap_err()
    }

    #[tokio::test(flavor = "current_thread")]
    async fn events_fold_in_and_no_snapshot_is_fetched_after_the_first() {
        let (snap, pushes) = scene();
        let fake = Fake::start("fold", vec![snap], pushes, false);
        let (sh, _tx, task) = connected(&fake).await;
        until(|| {
            let s = sh.lock().unwrap();
            s.tasks.len() == 2 && s.requests.len() == 1
        })
        .await;
        // the link used to fetch a snapshot some 80 ms after an event: give it three times that
        tokio::time::sleep(Duration::from_millis(250)).await;
        // the one status is the version check on connecting; a ping would be a second
        assert_eq!((fake.calls("hello"), fake.calls("snapshot"), fake.calls("subscribe"), fake.calls("status")), (1, 1, 1, 1));
        {
            let s = sh.lock().unwrap();
            assert_eq!(s.link, Some(Link::Connected));
            assert_eq!((s.tasks[0].state, s.tasks[1].state), (TaskState::NeedsYou, TaskState::Queued));
            assert_eq!(s.requests[0].info.id, "r1");
            assert!(s.runs.contains_key("t1") && s.runs.contains_key("t2"), "a run for the task that was backfilled and for the one that arrived");
        }
        assert!(!task.is_finished());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn what_the_snapshot_already_holds_is_not_applied_twice() {
        let (snap, _) = scene();
        // the replay may start inside what the snapshot covers: the step at 3 is in the snapshot already
        let fake = Fake::start("overlap", vec![snap], vec![pushed(&step(3, "t1", "a")), pushed(&ask(4, "t1", "r1"))], false);
        let (sh, _tx, task) = connected(&fake).await;
        until(|| sh.lock().unwrap().requests.len() == 1).await;
        assert_eq!(sh.lock().unwrap().tasks[0].steps, 1);
        assert!(!task.is_finished());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_ping_goes_out_while_idle_and_its_time_is_kept() {
        let (snap, _) = scene();
        let fake = Fake::start("ping", vec![snap], vec![], false);
        let (sh, _tx, _task) = connected(&fake).await;
        until(|| sh.lock().unwrap().link == Some(Link::Connected)).await;
        // a ping is a status call, after the one that checks the version on connecting
        assert_eq!(fake.calls("status"), 1, "no ping at once");
        until(|| fake.calls("status") > 1).await;
        until(|| sh.lock().unwrap().rtt_ms.is_some()).await;
        assert_eq!(fake.calls("snapshot"), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn commands_are_served_while_idle_and_shown_asks_for_nothing() {
        let (snap, _) = scene();
        let fake = Fake::start("cmd", vec![snap], vec![], false);
        let (sh, tx, _task) = connected(&fake).await;
        until(|| sh.lock().unwrap().link == Some(Link::Connected)).await;
        tx.send(Cmd::Shown).unwrap();
        tx.send(Cmd::Interrupt { task: "t1".into() }).unwrap();
        until(|| fake.calls("task.interrupt") == 1).await;
        assert_eq!(fake.calls("status"), 1, "the window being shown again changes nothing while the timer is already on");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn protocol_commands_reach_sphered_and_what_went_through_is_announced() {
        let (snap, _) = scene();
        let fake = Fake::start("protocols", vec![snap], vec![], false);
        let (sh, tx, _task) = connected(&fake).await;
        until(|| sh.lock().unwrap().link == Some(Link::Connected)).await;
        tx.send(Cmd::ProtocolAdd { text: "every weekday 9:00 sphere: update".into(), repo: None }).unwrap();
        tx.send(Cmd::ProtocolEnable { id: "p1".into(), enabled: false }).unwrap();
        tx.send(Cmd::ProtocolRunNow { id: "p1".into() }).unwrap();
        tx.send(Cmd::ProtocolRemove { id: "p1".into() }).unwrap();
        until(|| sh.lock().unwrap().good.len() == 4).await;
        assert_eq!((fake.calls("protocol.add"), fake.calls("protocol.set_enabled"), fake.calls("protocol.run_now"), fake.calls("protocol.remove")), (1, 1, 1, 1));
        let s = sh.lock().unwrap();
        for want in ["Protocol saved · next ", "Protocol off", "Protocol running", "Protocol removed"] {
            assert!(s.good.iter().any(|g| g.starts_with(want)), "{want}: {:?}", s.good);
        }
        assert!(s.notes.is_empty(), "no errors: {:?}", s.notes);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_lost_event_is_made_up_with_a_new_snapshot_and_the_stream_goes_on() {
        // event 5 never comes: after the question at 4 the next one is 6, so the link takes a new snapshot (which
        // holds 6) and carries on with 7
        let history = [created(1, "t1"), started(2, "t1"), step(3, "t1", "a"), ask(4, "t1", "r1"), step(5, "t1", "b"), created(6, "t2")];
        let fake = Fake::start("lost", vec![picture(&history[..3]), picture(&history)], vec![pushed(&history[3]), pushed(&history[5]), pushed(&started(7, "t2"))], false);
        let (sh, _tx, task) = connected(&fake).await;
        until(|| sh.lock().unwrap().tasks.get(1).is_some_and(|t| t.state == TaskState::Working)).await;
        {
            let s = sh.lock().unwrap();
            assert_eq!((s.tasks[0].steps, s.tasks[0].state, s.requests.len()), (2, TaskState::NeedsYou, 1), "the step that never came is in");
            assert_eq!(s.link, Some(Link::Connected), "no drop of the link, no toast");
        }
        assert_eq!((fake.calls("hello"), fake.calls("snapshot")), (1, 2));
        assert!(!task.is_finished());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_notes_event_makes_the_link_read_the_notes_again() {
        let (snap, _) = scene();
        let fake = Fake::start("notes", vec![snap], vec![pushed(&memory_changed(4, 2))], false);
        *fake.notes.lock().unwrap() = vec![kept(2, Some("/r"), "Use pnpm"), kept(1, None, "Keep commits small")];
        let (sh, _tx, task) = connected(&fake).await;
        until(|| sh.lock().unwrap().memory.len() == 2).await;
        let s = sh.lock().unwrap();
        assert_eq!(s.memory[0].text, "Use pnpm");
        assert_eq!(s.memory_rev, 1, "the snapshot had none");
        assert_eq!((fake.calls("memory.list"), fake.calls("snapshot")), (1, 1), "the event was in sequence: no new snapshot");
        assert!(!task.is_finished());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn remembering_and_forgetting_are_calls_to_sphered_and_the_toast_waits_for_its_answer() {
        let (snap, _) = scene();
        let fake = Fake::start("remember", vec![snap], vec![], false);
        let (sh, tx, _task) = connected(&fake).await;
        until(|| sh.lock().unwrap().link == Some(Link::Connected)).await;
        tx.send(Cmd::Remember { repo: Some("/code/sphere".into()), text: "Use pnpm".into(), source: NoteSource::Change }).unwrap();
        until(|| !sh.lock().unwrap().good.is_empty()).await;
        assert_eq!(sh.lock().unwrap().good, ["Remembered for sphere · 1 note"]);
        {
            let held = fake.notes.lock().unwrap();
            assert_eq!((held[0].repo.as_deref(), held[0].text.as_str(), held[0].source), (Some("/code/sphere"), "Use pnpm", NoteSource::Change));
        }
        // for every repo
        tx.send(Cmd::Remember { repo: None, text: "Keep commits small".into(), source: NoteSource::Typed }).unwrap();
        until(|| sh.lock().unwrap().good.len() == 2).await;
        assert_eq!(sh.lock().unwrap().good[1], "Remembered for all repos · 2 notes");
        assert_eq!(fake.notes.lock().unwrap()[0].repo, None);
        tx.send(Cmd::Forget { id: 1 }).unwrap();
        until(|| fake.notes.lock().unwrap().len() == 1).await;
        assert_eq!(fake.calls("memory.add"), 2);
        assert!(sh.lock().unwrap().notes.is_empty(), "nothing went wrong");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn forgetting_a_note_that_is_already_gone_is_not_an_error() {
        let (snap, _) = scene();
        let fake = Fake::start("forget", vec![snap], vec![], false);
        let (sh, tx, task) = connected(&fake).await;
        until(|| sh.lock().unwrap().link == Some(Link::Connected)).await;
        tx.send(Cmd::Forget { id: 9 }).unwrap();
        until(|| fake.calls("memory.remove") == 1).await;
        // a call after it is served, so the answer to the first has been read
        tx.send(Cmd::Interrupt { task: "t1".into() }).unwrap();
        until(|| fake.calls("task.interrupt") == 1).await;
        assert!(sh.lock().unwrap().notes.is_empty() && !task.is_finished());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_snapshot_that_does_not_cover_the_gap_a_lag_and_a_hang_up_each_end_the_session() {
        // sphered's picture is older than the event it just sent: nothing to rely on
        let behind = ends_with("gap", vec![pushed(&ask(4, "t1", "r1")), pushed(&created(7, "t2"))], false).await;
        assert!(behind.starts_with("missed events 5..6, resyncing; missed events 4..6"), "{behind}");
        let lag = serde_json::json!({"jsonrpc": "2.0", "error": {"code": -32000, "message": "too slow", "data": {"kind": "lagged"}}});
        assert_eq!(ends_with("lag", vec![lag], false).await, "resyncing: too slow");
        assert_eq!(ends_with("hangup", vec![], true).await, "sphered closed the connection");
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
