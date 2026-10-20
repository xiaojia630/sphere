//! The web playground's pretend sphered. It speaks the real one's language: every change is a `sphere_proto` event
//! folded into the app's mirror with the code the app runs against sphered, so the Agents panel, the Working view,
//! the swarm, the Agent moment card and the Code app all show it as they would a real agent. At start it has four
//! agents (one finished and merged, one waiting on you, two working). Agents you start from the command bar or Fix
//! it get a short scripted run: they read, search and edit, ask to run the tests, and finish with a change to review.
use crate::service::{Cmd, Link, Mirror, Service};
use bevy::prelude::*;
use sphere_proto::api::Snapshot;
use sphere_proto::{AnswerKind, Event, EventBody, FileStat, PlanItem, RequestInfo, RequestKind, StepKind};
use std::collections::VecDeque;
use std::sync::mpsc::Receiver;
use std::sync::Mutex;

static COMMANDS: Mutex<Option<Receiver<Cmd>>> = Mutex::new(None);

/// The app's command channel (`service::start`): what the visitor asks for comes here.
pub fn take_commands(rx: Receiver<Cmd>) {
    *COMMANDS.lock().unwrap() = Some(rx);
}

pub struct SimPlugin;

impl Plugin for SimPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, seed).add_systems(Update, tick.before(crate::live::sync));
    }
}

/// One beat of an agent's run.
enum Beat {
    Say(String),
    Plan(Vec<String>),
    /// a tool call, and how long it runs (s)
    Step { kind: StepKind, tool: &'static str, title: String, detail: String, secs: f64, output: String },
    /// the tool call `id` ends, with what it printed
    StepEnd { id: String, output: String },
    /// asks to run a command, and waits for the answer
    Ask { command: String, description: String, summary: String },
    Files(Vec<FileStat>),
    /// the turn ends: its summary and what it cost
    Done { summary: String, cost: f64 },
}

struct Run {
    task: String,
    beats: VecDeque<Beat>,
    /// when the next beat plays (seconds of `Time<Real>`)
    next: f64,
    /// the request it waits on, and the beat it asked with (asked again when snoozed)
    waiting: Option<(String, RequestInfo)>,
    /// a snoozed request comes back at this time
    again_at: Option<f64>,
    step_n: u32,
}

#[derive(Resource)]
struct Sim {
    mirror: Mirror,
    seq: u64,
    runs: Vec<Run>,
    tasks: u32,
    requests: u32,
}

fn now_ms() -> u64 {
    web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn slug(title: &str) -> String {
    let s: String = title.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let s = s.split('-').filter(|w| !w.is_empty()).collect::<Vec<_>>().join("-");
    s.chars().take(40).collect::<String>().trim_end_matches('-').to_string()
}

fn repo_name(repo: &str) -> &str {
    repo.rsplit('/').next().unwrap_or(repo)
}

fn files(list: &[(&str, u64, u64)]) -> Vec<FileStat> {
    list.iter().map(|(p, a, r)| FileStat { path: p.to_string(), added: *a, removed: *r }).collect()
}

impl Sim {
    /// Folds one event, stamped `ms` (unix ms), into the app's mirror.
    fn emit(&mut self, svc: &Service, task: &str, ms: u64, body: EventBody) {
        self.seq += 1;
        let ev = Event { seq: self.seq, task: task.to_string(), at: sphere_proto::rfc3339_from_unix_ms(ms), body };
        let mut s = svc.shared.lock().unwrap();
        let _ = self.mirror.fold(&ev, &mut s);
    }

    /// A new agent: created, its worktree made, started. `ago` seconds in the past for the ones there at start.
    fn create(&mut self, svc: &Service, title: &str, prompt: &str, repo: &str, ago: u64) -> String {
        self.tasks += 1;
        let id = format!("t{}", self.tasks);
        let t0 = now_ms() - ago * 1000;
        let sl = slug(title);
        self.emit(svc, &id, t0, EventBody::TaskCreated { title: title.into(), prompt: prompt.into(), repo: repo.into(), parent_task: None, in_place: false, budget_usd: 5.0, mode: None, model: Some("claude-opus-5-5".into()), allow_rewrites: false, resume_session: None, protocol: None });
        let workdir = format!("{}/.sphere/worktrees/{}/{sl}", crate::machine::HOME, repo_name(repo));
        self.emit(svc, &id, t0 + 400, EventBody::WorkspaceReady { workdir, branch: Some(format!("sphere/{sl}")), base: "main".into() });
        let briefed = svc.shared.lock().unwrap().memory.iter().filter(|n| n.repo.as_deref().is_none_or(|r| r == repo)).count() as u32;
        self.emit(svc, &id, t0 + 900, EventBody::AgentStarted { child: format!("c{}", self.tasks), pid: 40100 + self.tasks * 7, resumed: false, briefed });
        self.emit(svc, &id, t0 + 1300, EventBody::Init { model: Some("claude-opus-5-5".into()), permission_mode: Some("default".into()), slash_commands: vec![] });
        id
    }

    /// Plays `beat` for `task` at `ms`. Returns how long until the next beat, or None when it now waits on you.
    /// A step's end goes to the front of `rest`, to play when the step has run.
    fn play(&mut self, svc: &Service, task: &str, ms: u64, beat: Beat, n: &mut u32, rest: &mut VecDeque<Beat>) -> Option<(f64, Option<(String, RequestInfo)>)> {
        match beat {
            Beat::Say(text) => self.emit(svc, task, ms, EventBody::Say { text }),
            Beat::Plan(items) => {
                let items = items.into_iter().enumerate().map(|(i, text)| PlanItem { text, status: if i == 0 { "in_progress".into() } else { "pending".into() } }).collect();
                self.emit(svc, task, ms, EventBody::Plan { items });
            }
            Beat::Step { kind, tool, title, detail, secs, output } => {
                *n += 1;
                let id = format!("{task}-s{}", self.seq);
                self.emit(svc, task, ms, EventBody::Step { id: id.clone(), step_kind: kind, tool: tool.into(), title, detail, parent: None });
                rest.push_front(Beat::StepEnd { id, output });
                return Some((secs, None));
            }
            Beat::StepEnd { id, output } => {
                let out = (!output.is_empty()).then(|| sphere_proto::Excerpt { head: output.clone(), tail: None, bytes: output.len() as u64, source: None });
                self.emit(svc, task, ms, EventBody::StepDone { id, ok: true, output: out });
                return Some((0.5, None));
            }
            Beat::Ask { command, description, summary } => {
                self.requests += 1;
                let info = RequestInfo {
                    id: format!("r{}", self.requests),
                    task: task.into(),
                    kind: RequestKind::Permission,
                    tool: "Bash".into(),
                    display_name: None,
                    description: Some(description.clone()),
                    summary,
                    input: serde_json::json!({ "command": command, "description": description }),
                    suggestions: vec![serde_json::json!({"type": "addRules", "rules": [{"toolName": "Bash", "ruleContent": format!("{}:*", command.split_whitespace().take(2).collect::<Vec<_>>().join(" "))}], "behavior": "allow", "destination": "localSettings"})],
                    blocked_path: None,
                    tool_use_id: None,
                    agent_request: None,
                    revises: None,
                };
                self.emit(svc, task, ms, EventBody::Request { request: info.clone() });
                return Some((0.0, Some((info.id.clone(), info))));
            }
            Beat::Files(list) => self.emit(svc, task, ms, EventBody::FilesTouched { files: list }),
            Beat::Done { summary, cost } => {
                self.emit(svc, task, ms, EventBody::Turn { ok: true, interrupted: false, summary, turns: Some(*n as u64 + 2), duration_ms: Some(48_000), cost_usd: Some(cost), cost_delta_usd: cost });
                self.emit(svc, task, ms + 200, EventBody::AgentExited { code: Some(0), signal: None, reason: "idle".into() });
                return None;
            }
        }
        Some((1.2, None))
    }

    /// Plays a whole run at once, in the past (the agents already there at start).
    fn replay(&mut self, svc: &Service, task: &str, beats: Vec<Beat>, from_ms: u64) -> Option<(String, RequestInfo)> {
        let (mut ms, mut n) = (from_ms, 0);
        let mut beats: VecDeque<Beat> = beats.into();
        while let Some(b) = beats.pop_front() {
            match self.play(svc, task, ms, b, &mut n, &mut beats) {
                Some((gap, None)) => ms += (gap * 1000.0) as u64,
                Some((_, Some(w))) => return Some(w),
                None => return None,
            }
        }
        None
    }

    fn start(&mut self, svc: &Service, task: String, beats: Vec<Beat>, at: f64) {
        self.runs.push(Run { task, beats: beats.into(), next: at + 1.0, waiting: None, again_at: None, step_n: 0 });
    }
}

fn step(kind: StepKind, tool: &'static str, title: &str, detail: &str, secs: f64) -> Beat {
    Beat::Step { kind, tool, title: title.into(), detail: detail.into(), secs, output: String::new() }
}

fn step_out(kind: StepKind, tool: &'static str, title: &str, detail: &str, secs: f64, output: &str) -> Beat {
    Beat::Step { kind, tool, title: title.into(), detail: detail.into(), secs, output: output.into() }
}

/// What an agent the visitor starts does: a believable small change in the repo it was given, tests asked for first.
fn script_for(title: &str, repo: &str) -> Vec<Beat> {
    let (a, b) = if repo.ends_with("sphere") { ("crates/sphered/src/manager.rs", "crates/sphered/tests/tasks.rs") } else { ("src/routes/health.rs", "tests/health.rs") };
    vec![
        Beat::Say(format!("I'll start by reading how this part of {} is put together.", repo_name(repo))),
        Beat::Plan(vec!["Read the code involved".into(), "Make the change".into(), "Add a test".into(), "Run the tests".into()]),
        step(StepKind::Search, "Grep", "Searching the code", &format!("{}", title.split_whitespace().last().unwrap_or("fn")), 1.4),
        step(StepKind::Read, "Read", &format!("Reading {a}"), a, 1.2),
        step(StepKind::Read, "Read", "Reading src/main.rs", "src/main.rs", 0.9),
        Beat::Say("Found it. The change is small; I'll keep it to two files.".into()),
        step(StepKind::Edit, "Edit", &format!("Editing {a}"), a, 1.6),
        step(StepKind::Edit, "Write", &format!("Writing {b}"), b, 1.4),
        Beat::Files(files(&[(a, 24, 3), (b, 31, 0)])),
        Beat::Ask { command: "cargo test".into(), description: "Run the test suite".into(), summary: "The change and its test are in. I'd like to run the whole test suite to make sure nothing else broke.".into() },
        step_out(StepKind::Run, "Bash", "Running cargo test", "cargo test", 3.2, "running 65 tests\n.................................................................\ntest result: ok. 65 passed; 0 failed; 0 ignored; finished in 2.81s"),
        Beat::Say("All 65 tests pass, including the new one.".into()),
        Beat::Done { summary: format!("{title}: done in two files, with a test. All 65 tests pass."), cost: 0.37 },
    ]
}

/// The four agents the playground opens with, and the two that keep working.
fn seed(mut commands: Commands, svc: Res<Service>, time: Res<Time<Real>>) {
    let snap = Snapshot { seq: 0, tasks: vec![], requests: vec![], cost_today_usd: 0.0, daily_cap_usd: None, max_concurrent: Some(4), waited_today_secs: 0, memory: vec![], protocols: vec![] };
    let mirror = {
        let mut s = svc.shared.lock().unwrap();
        s.link = Some(Link::Connected);
        s.rtt_ms = Some(0.4);
        Mirror::connect(snap, &mut s)
    };
    let mut sim = Sim { mirror, seq: 0, runs: Vec::new(), tasks: 0, requests: 0 };
    let shop = crate::machine::REPOS[0];
    let sphere = crate::machine::REPOS[1];
    let now = now_ms();

    // finished three hours ago, and merged
    let t = sim.create(&svc, "Bump Postgres to 16.4", "Bump Postgres to 16.4 in compose and CI", shop, 3 * 3600);
    sim.replay(&svc, &t, vec![
        step(StepKind::Read, "Read", "Reading docker-compose.yml", "docker-compose.yml", 2.0),
        step(StepKind::Edit, "Edit", "Editing docker-compose.yml", "docker-compose.yml", 3.0),
        step(StepKind::Edit, "Edit", "Editing .github/workflows/ci.yml", ".github/workflows/ci.yml", 3.0),
        Beat::Files(files(&[("docker-compose.yml", 1, 1), (".github/workflows/ci.yml", 2, 2)])),
        step_out(StepKind::Run, "Bash", "Running cargo test", "cargo test", 40.0, "test result: ok. 64 passed; 0 failed"),
        Beat::Done { summary: "Postgres is 16.4 in compose and CI; all 64 tests pass against it.".into(), cost: 0.41 },
    ], now - 3 * 3600 * 1000 + 2000);

    // waiting on you: it wants to push
    let t = sim.create(&svc, "Reply to the API review", "Address the review on the orders API: rename the handler and add pagination", shop, 25 * 60);
    sim.replay(&svc, &t, vec![
        Beat::Say("Two asks in the review: rename list_orders to orders_page, and paginate GET /orders.".into()),
        step(StepKind::Read, "Read", "Reading src/routes/orders.rs", "src/routes/orders.rs", 20.0),
        step(StepKind::Edit, "Edit", "Editing src/routes/orders.rs", "src/routes/orders.rs", 30.0),
        step(StepKind::Edit, "Edit", "Editing docs/api.md", "docs/api.md", 20.0),
        Beat::Files(files(&[("src/routes/orders.rs", 38, 11), ("docs/api.md", 14, 2), ("tests/orders_api.rs", 22, 0)])),
        step_out(StepKind::Run, "Bash", "Running cargo test --test orders_api", "cargo test --test orders_api", 60.0, "test result: ok. 9 passed; 0 failed"),
    ], now - 24 * 60 * 1000);
    // it asks a few seconds in, so a visitor sees the core first and then an agent stop to ask
    sim.start(&svc, t, vec![
        Beat::Ask { command: "git push -u origin sphere/reply-to-the-api-review".into(), description: "Push the branch for the reviewer".into(), summary: "The rename and pagination are in and the API tests pass. May I push the branch so the reviewer can see it?".into() },
        step_out(StepKind::Run, "Bash", "Running git push", "git push -u origin sphere/reply-to-the-api-review", 2.0, "To github.com:you/shop-api.git\n * [new branch]      sphere/reply-to-the-api-review -> sphere/reply-to-the-api-review"),
        Beat::Done { summary: "Review addressed and pushed: orders_page, pagination with limit and cursor, docs and tests.".into(), cost: 0.52 },
    ], time.elapsed_secs_f64() + 12.0);

    // working: it finishes about a minute in
    let t = sim.create(&svc, "Fix flaky login test", "The login integration test fails one run in ten on CI. Find out why and fix it.", shop, 6 * 60);
    sim.replay(&svc, &t, vec![
        Beat::Say("The test sleeps 200 ms and hopes the server is up. I'll look at how it starts the server.".into()),
        step(StepKind::Read, "Read", "Reading tests/login.rs", "tests/login.rs", 30.0),
        step(StepKind::Search, "Grep", "Searching for the token expiry", "expires_at", 20.0),
        step(StepKind::Read, "Read", "Reading src/auth/token.rs", "src/auth/token.rs", 25.0),
    ], now - 5 * 60 * 1000);
    let at = time.elapsed_secs_f64();
    sim.start(&svc, t, vec![
        Beat::Say("Two causes: a fixed sleep instead of waiting for the server, and a token that expires one second early.".into()),
        step(StepKind::Edit, "Edit", "Editing tests/login.rs", "tests/login.rs", 7.0),
        step(StepKind::Edit, "Edit", "Editing src/auth/token.rs", "src/auth/token.rs", 5.0),
        Beat::Files(files(&[("tests/login.rs", 18, 6), ("src/auth/token.rs", 3, 3), ("tests/support/mod.rs", 21, 0)])),
        step_out(StepKind::Run, "Bash", "Running the login test 50 times", "for i in $(seq 50); do cargo test --test login -q || break; done", 16.0, "test result: ok. 4 passed (x50)"),
        step_out(StepKind::Run, "Bash", "Running cargo test", "cargo test", 12.0, "test result: ok. 64 passed; 0 failed"),
        Beat::Say("50 runs in a row pass now.".into()),
        Beat::Done { summary: "Fixed: the test waits for the server to be ready, and tokens no longer expire a second early. 50 runs in a row pass.".into(), cost: 0.29 },
    ], at + 4.0);

    // working: release notes, slower
    let t = sim.create(&svc, "Draft release notes", "Draft the release notes for 0.1.5 from the PRs merged since 0.1.4", sphere, 3 * 60);
    sim.replay(&svc, &t, vec![
        Beat::Plan(vec!["List PRs merged since v0.1.4".into(), "Group them".into(), "Write RELEASE_NOTES.md".into()]),
        step(StepKind::Run, "Bash", "Running git log v0.1.4..main", "git log --oneline v0.1.4..main", 4.0),
    ], now - 150 * 1000);
    sim.start(&svc, t, vec![
        step(StepKind::Read, "Read", "Reading 12 merged PRs", "gh pr list --state merged", 12.0),
        Beat::Say("Twelve PRs: three features, five fixes, four speed-ups. I'll lead with Protocols.".into()),
        step(StepKind::Edit, "Write", "Writing RELEASE_NOTES.md", "RELEASE_NOTES.md", 14.0),
        Beat::Files(files(&[("RELEASE_NOTES.md", 46, 0)])),
        Beat::Done { summary: "Release notes for 0.1.5 drafted in RELEASE_NOTES.md: features, fixes and speed-ups, each with its PR.".into(), cost: 0.18 },
    ], at + 70.0);

    // a Protocol, so Next 24h shows one
    {
        use sphere_proto::protocol::{self, LocalZone};
        let text = "every weekday 9:00 shop-api: update dependencies and run the tests";
        let t = crate::core::unix_now();
        if let Ok(p) = protocol::parse(text, t, &LocalZone) {
            let made = sphere_proto::Protocol { id: "p1".into(), text: text.into(), next_run: p.schedule.next_after(t, &LocalZone), schedule: p.schedule, repo: shop.into(), prompt: p.prompt, enabled: true, created_at: t - 6 * 86_400, last_run: None };
            sim.emit(&svc, "", now - 6 * 86_400 * 1000, EventBody::ProtocolSaved { protocol: made });
        }
    }
    commands.insert_resource(sim);
}

/// Plays the beats that are due, and takes the visitor's commands.
fn tick(mut sim: ResMut<Sim>, svc: Res<Service>, time: Res<Time<Real>>) {
    let now = time.elapsed_secs_f64();
    let cmds: Vec<Cmd> = COMMANDS.lock().unwrap().as_ref().map(|rx| rx.try_iter().collect()).unwrap_or_default();
    for c in cmds {
        command(&mut sim, &svc, c, now);
    }
    let sim = &mut *sim;
    let mut i = 0;
    while i < sim.runs.len() {
        // a snoozed request comes back
        if sim.runs[i].again_at.is_some_and(|t| now >= t) {
            sim.runs[i].again_at = None;
            if let Some((_, info)) = sim.runs[i].waiting.clone() {
                sim.requests += 1;
                let mut info = info;
                info.id = format!("r{}", sim.requests);
                let task = sim.runs[i].task.clone();
                sim.emit(&svc, &task, now_ms(), EventBody::Request { request: info.clone() });
                sim.runs[i].waiting = Some((info.id.clone(), info));
            }
        }
        if sim.runs[i].waiting.is_some() || now < sim.runs[i].next {
            i += 1;
            continue;
        }
        let Some(beat) = sim.runs[i].beats.pop_front() else {
            sim.runs.remove(i);
            continue;
        };
        let task = sim.runs[i].task.clone();
        let mut n = sim.runs[i].step_n;
        let mut rest = std::mem::take(&mut sim.runs[i].beats);
        let r = sim.play(&svc, &task, now_ms(), beat, &mut n, &mut rest);
        sim.runs[i].beats = rest;
        sim.runs[i].step_n = n;
        match r {
            Some((gap, w)) => {
                sim.runs[i].next = now + gap;
                sim.runs[i].waiting = w;
                i += 1;
            }
            None => {
                sim.runs.remove(i);
            }
        }
    }
}

fn command(sim: &mut Sim, svc: &Service, c: Cmd, now: f64) {
    match c {
        Cmd::Start { repo, prompt, title, .. } => {
            let title = title.unwrap_or_else(|| sphere_proto::protocol::short_name(&prompt, 40));
            let id = sim.create(svc, &title, &prompt, &repo, 0);
            let beats = script_for(&title, &repo);
            sim.start(svc, id, beats, now);
        }
        Cmd::Answer { request, answer, text, always } => {
            let Some(i) = sim.runs.iter().position(|r| r.waiting.as_ref().is_some_and(|w| w.0 == request)) else { return };
            let task = sim.runs[i].task.clone();
            sim.emit(svc, &task, now_ms(), EventBody::RequestAnswered { request: request.clone(), answer, text: text.clone(), always });
            match answer {
                AnswerKind::Yes => {
                    sim.runs[i].waiting = None;
                    sim.runs[i].next = now + 0.4;
                }
                AnswerKind::Change => {
                    // it takes the change on board and carries on without running the command
                    let said = text.unwrap_or_default();
                    sim.runs[i].waiting = None;
                    sim.runs[i].next = now + 0.8;
                    let rest: Vec<Beat> = sim.runs[i].beats.drain(..).filter(|b| !matches!(b, Beat::Step { kind: StepKind::Run, .. })).collect();
                    sim.runs[i].beats = rest.into();
                    sim.runs[i].beats.push_front(Beat::Say(format!("Understood: {}. I'll do it that way instead.", said.trim().trim_end_matches('.'))));
                }
                AnswerKind::Later => {
                    sim.emit(svc, &task, now_ms(), EventBody::RequestSnoozed { request });
                    sim.runs[i].again_at = Some(now + 45.0);
                }
            }
        }
        Cmd::Send { task, text } => {
            sim.emit(svc, &task, now_ms(), EventBody::UserMessage { text: text.clone() });
            let beats = vec![
                Beat::Say("Let me check.".into()),
                step(StepKind::Read, "Read", "Reading the change again", "git diff main", 1.4),
                Beat::Say("Yes: the change covers that, and the tests exercise it. Nothing else needed.".into()),
                Beat::Done { summary: "Answered your follow-up: the change already covers it.".into(), cost: 0.04 },
            ];
            sim.runs.retain(|r| r.task != task);
            sim.start(svc, task, beats, now - 0.6);
        }
        Cmd::Interrupt { task } => {
            sim.runs.retain(|r| r.task != task);
            sim.emit(svc, &task, now_ms(), EventBody::Turn { ok: false, interrupted: true, summary: String::new(), turns: None, duration_ms: None, cost_usd: None, cost_delta_usd: 0.0 });
        }
        Cmd::Stop { task } => {
            sim.runs.retain(|r| r.task != task);
            sim.emit(svc, &task, now_ms(), EventBody::Stopped {});
        }
        Cmd::Seen { task } => sim.emit(svc, &task, now_ms(), EventBody::Seen {}),
        Cmd::Remember { repo, text, source } => {
            let mut s = svc.shared.lock().unwrap();
            match crate::memory::keep_here(&mut s, repo.as_deref(), &text, source) {
                Ok(added) => s.good.push(crate::memory::remembered(&added)),
                Err(e) => s.notes.push(e),
            }
            s.version += 1;
        }
        Cmd::Forget { id } => {
            let mut s = svc.shared.lock().unwrap();
            s.memory.retain(|n| n.id != id);
            s.memory_rev += 1;
            s.version += 1;
        }
        Cmd::ProtocolAdd { text, repo } => {
            use sphere_proto::protocol::{self, LocalZone};
            let t = crate::core::unix_now();
            match protocol::parse(&text, t, &LocalZone) {
                Ok(p) => {
                    let n = svc.shared.lock().unwrap().protocols.len() + 1;
                    let repo = p.repo.map(|r| format!("{}/Projects/{}", crate::machine::HOME, r.trim_start_matches("~/"))).or(repo).unwrap_or_else(|| crate::machine::REPOS[0].into());
                    let made = sphere_proto::Protocol { id: format!("p{n}"), text: text.trim().into(), next_run: p.schedule.next_after(t, &LocalZone), schedule: p.schedule, repo, prompt: p.prompt, enabled: true, created_at: t, last_run: None };
                    sim.emit(svc, "", now_ms(), EventBody::ProtocolSaved { protocol: made.clone() });
                    svc.shared.lock().unwrap().good.push(crate::service::saved_words(&made, t));
                }
                Err(e) => svc.shared.lock().unwrap().notes.push(e.0),
            }
        }
        Cmd::ProtocolEnable { id, enabled } => {
            let p = svc.shared.lock().unwrap().protocols.iter().find(|p| p.id == id).cloned();
            if let Some(mut p) = p {
                let t = crate::core::unix_now();
                p.enabled = enabled;
                p.next_run = if enabled { p.schedule.next_after(t, &sphere_proto::protocol::LocalZone) } else { None };
                sim.emit(svc, "", now_ms(), EventBody::ProtocolSaved { protocol: p.clone() });
                svc.shared.lock().unwrap().good.push(if enabled { format!("Protocol on · {}", crate::service::next_words(&p, t)) } else { "Protocol off".into() });
            }
        }
        Cmd::ProtocolRunNow { id } => {
            let p = svc.shared.lock().unwrap().protocols.iter().find(|p| p.id == id).cloned();
            if let Some(p) = p {
                let title = sphere_proto::protocol::short_name(&p.prompt, 40);
                let t = sim.create(svc, &title, &p.prompt, &p.repo, 0);
                let beats = script_for(&title, &p.repo);
                sim.start(svc, t, beats, now);
                svc.shared.lock().unwrap().good.push("Protocol running".into());
            }
        }
        Cmd::ProtocolRemove { id } => {
            sim.emit(svc, "", now_ms(), EventBody::ProtocolRemoved { id });
            svc.shared.lock().unwrap().good.push("Protocol removed".into());
        }
        Cmd::TakeOver { session_id } => {
            let mut s = svc.shared.lock().unwrap();
            s.taken.push((session_id, Err("The playground has no other sessions to take over".into())));
            s.version += 1;
        }
        Cmd::Shown => {}
    }
}
