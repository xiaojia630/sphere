//! The app's link to sphered. A background thread runs a small tokio runtime: it starts sphered when nothing
//! answers on the socket, connects as the app, takes one snapshot, subscribes, and from then on folds each event into
//! its own copy of sphered's views (with the code sphered itself runs), so the shared slot the Bevy systems read is
//! always current and the link asks for another snapshot only after it missed an event. Commands (start a task,
//! answer a request) go the other way on a channel.
use bevy::prelude::*;
use sphere_client::{Client, ClientKind, StartParams, Update};
use sphere_proto::api::Snapshot;
use sphere_proto::{AnswerKind, Event, EventBody, RequestView, TaskState, TaskView, Views};
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
    /// Sphere is on screen again: the link starts pinging (it stops while the window is hidden).
    Shown,
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
        let at: HashMap<String, usize> = s.tasks.iter().enumerate().map(|(i, t)| (t.id.clone(), i)).collect();
        let day = today();
        let views = Views::from_snapshot(snap, &day);
        s.cost_today_usd = views.cost_on(&day);
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
        match s.runs.get_mut(&ev.task) {
            Some(r) => r.apply(ev),
            None => {
                let mut r = Run::default();
                r.apply(ev);
                s.runs.insert(ev.task.clone(), r);
            }
        }
        self.views.apply(ev);
        if matches!(ev.body, EventBody::Request { .. } | EventBody::RequestSnoozed { .. } | EventBody::RequestAnswered { .. } | EventBody::RequestWithdrawn { .. }) {
            self.views.forget_closed_requests();
            s.requests = self.views.open_requests().into_iter().cloned().collect();
        }
        // `apply` changes the event's task and no other
        if let Some(t) = self.views.task(&ev.task) {
            put(&mut self.at, t, s);
        }
        let cost = self.views.cost_on(&today());
        if cost != s.cost_today_usd {
            s.cost_today_usd = cost;
        }
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

/// How often the System panel's response time is measured while Sphere is on screen.
const PING: Duration = Duration::from_secs(2);

/// One connection: take a snapshot, subscribe from its seq, then fold every event into the shared slot as it arrives
/// and serve commands. A snapshot is taken again only if an event was missed (or one is not understood): the mirror
/// starts over from it and goes on. Sphered dropping us ends the session and the next one starts afresh. No timer runs
/// while nothing happens, and the ping runs only while Sphere is on screen (the window coming back wakes it with
/// `Cmd::Shown`).
async fn session(sh: &Arc<Mutex<Shared>>, c: &Client, rx: &mut mpsc::UnboundedReceiver<Cmd>) -> Result<(), String> {
    c.hello(ClientKind::App).await.map_err(|e| e.to_string())?;
    let snap = c.snapshot().await.map_err(|e| e.to_string())?;
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
                }
                // typing frames: the app shows nothing of them
                Some(Update::Live(_)) => {}
                Some(Update::Lagged(e)) => return Err(format!("resyncing: {}", e.message)),
                Some(Update::Closed) | None => return Err("sphered closed the connection".into()),
            },
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
                    continue;
                }
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
        Cmd::Shown => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sphere_proto::StepKind;

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
        event(seq, task, EventBody::TaskCreated { title: format!("Task {task}"), prompt: "p".into(), repo: "/r".into(), in_place: false, budget_usd: 1.0, mode: None, model: None, allow_rewrites: false, resume_session: None })
    }

    fn started(seq: u64, task: &str) -> Event {
        event(seq, task, EventBody::AgentStarted { child: "c".into(), pid: 7, resumed: false })
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
        Snapshot { seq: 0, tasks: vec![], requests: vec![], cost_today_usd: 0.0, daily_cap_usd: None, max_concurrent: Some(3) }
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
        let snap = Snapshot { seq: server.seq, tasks: server.tasks().into_iter().cloned().collect(), requests: server.open_requests().into_iter().cloned().collect(), cost_today_usd: 1.25, daily_cap_usd: None, max_concurrent: Some(2) };
        let mut s = Shared::default();
        let mut mirror = Mirror::connect(snap, &mut s);
        assert_eq!((s.tasks.len(), s.requests.len(), s.cost_today_usd, s.max_concurrent), (2, 1, 1.25, Some(2)));
        assert!(s.roster > 0 && s.states > 0 && s.spans > 0 && s.version > 0);
        for e in [answer(6, "t1", "r1"), step_done(7, "t1", "a"), turn(8, "t1", 0.5)] {
            server.apply(&e);
            mirror.fold(&e, &mut s).unwrap();
        }
        let tasks: Vec<TaskView> = server.tasks().into_iter().cloned().collect();
        assert_eq!(s.tasks, tasks);
        assert!(s.requests.is_empty());
        assert_eq!(s.cost_today_usd, 1.25, "a turn on another day leaves today's cost alone");
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

    // ---- the link, against a stand-in for sphered

    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    /// A sphered that serves one connection. It answers the calls the link makes (the snapshots are the ones given, one
    /// per call and the last one again after that; the history is empty), and once the link subscribes it sends each of
    /// `pushes` (whole messages) and, with `hang_up`, closes. `log` collects the method of every call it gets.
    struct Fake {
        log: Arc<Mutex<Vec<String>>>,
        path: PathBuf,
    }

    impl Fake {
        fn start(name: &str, snaps: Vec<Snapshot>, pushes: Vec<serde_json::Value>, hang_up: bool) -> Fake {
            let path = std::env::temp_dir().join(format!("sphere-link-{}-{name}.sock", std::process::id()));
            let _ = std::fs::remove_file(&path);
            let listener = tokio::net::UnixListener::bind(&path).unwrap();
            let log = Arc::new(Mutex::new(Vec::new()));
            let seen = log.clone();
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
                        "subscribe" => serde_json::json!({"seq": snaps[0].seq}),
                        "status" => serde_json::json!({"seq": snaps[0].seq, "version": "test", "active": 0, "queued": 0, "needs_you": 0, "cost_today_usd": 0.0, "tasks": [], "requests": [], "host_pid": null}),
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
            Fake { log, path }
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
        Snapshot { seq: v.seq, tasks: v.tasks().into_iter().cloned().collect(), requests: v.open_requests().into_iter().cloned().collect(), cost_today_usd: 0.0, daily_cap_usd: None, max_concurrent: Some(3) }
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
        (sh, tx, tokio::spawn(async move { session(&s, &client, &mut rx).await }))
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
        assert_eq!((fake.calls("hello"), fake.calls("snapshot"), fake.calls("subscribe"), fake.calls("status")), (1, 1, 1, 0));
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
        assert_eq!(fake.calls("status"), 0, "no ping at once");
        until(|| fake.calls("status") > 0).await;
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
        assert_eq!(fake.calls("status"), 0, "the window being shown again changes nothing while the timer is already on");
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
