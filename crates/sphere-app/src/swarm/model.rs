//! What the swarm shows, from sphered's data (or the prototype's example agents for `--demo` and the look
//! tests): the agents in a stable order, the repos they orbit, conflicts, run slots and the strip's counts.
//! Everything here is plain data; `scene` and `hud` draw it.
use bevy::prelude::*;

/// An agent's state on the swarm (SPEC section 3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum State {
    #[default]
    Working,
    /// waiting on you: a request is open
    Needs,
    /// finished, its branch not merged yet: on the outer ring, "Ready to merge"
    Done,
    /// waiting for a run slot
    Queued,
    /// its branch was just merged: it collapses into the core, then leaves the model
    Merged,
    /// stopped or failed: it fades out, then leaves the model
    Stopped,
}

/// One of the agent's recent steps, newest first in `Agent::steps`.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Step {
    pub text: String,
    /// unix seconds when it started
    pub at: i64,
}

/// A file the agent touched, with its diff stats.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct File {
    pub path: String,
    pub added: u64,
    pub removed: u64,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Agent {
    /// sphered's task id
    pub id: String,
    pub title: String,
    /// index into `Model::repos` (its ring)
    pub repo: usize,
    pub branch: String,
    pub state: State,
    /// 0..1 (real agents have no percent: it follows the step count, like the Agents panel; done is 1)
    pub p: f32,
    /// what it is doing now (the label's step line)
    pub step: String,
    pub cost: f64,
    /// unix seconds
    pub started_at: i64,
    /// newest first, at most 10
    pub steps: Vec<Step>,
    pub files: Vec<File>,
    /// the file its current step edits, if any
    pub editing: Option<String>,
    /// queued agents keep their place in the queue
    pub queue_pos: Option<usize>,
    /// bumps each time a new step lands (the satellite pulses)
    pub step_seq: u64,
}

/// A repo with agents on it: one orbit ring.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Repo {
    /// the folder name, shown as the ring's key (e.g. "api-server")
    pub key: String,
    pub path: String,
    /// the branch its checkout is on
    pub branch: String,
}

/// Two agents in one repo editing the same file.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Conflict {
    pub a: String,
    pub b: String,
    pub file: String,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Model {
    /// stable order (the strip's dots, 1-9, Tab): oldest first; at most 9
    pub agents: Vec<Agent>,
    pub repos: Vec<Repo>,
    pub conflicts: Vec<Conflict>,
    /// how many agents run at once (sphered's max_concurrent; 3 when unknown)
    pub limit: usize,
    pub cost_today: f64,
}

impl Model {
    pub fn agent(&self, id: &str) -> Option<&Agent> {
        self.agents.iter().find(|a| a.id == id)
    }
    pub fn count(&self, s: State) -> usize {
        self.agents.iter().filter(|a| a.state == s).count()
    }
    /// The agents in a run slot (working or waiting on you), in model order.
    pub fn running(&self) -> Vec<&Agent> {
        self.agents.iter().filter(|a| matches!(a.state, State::Working | State::Needs)).collect()
    }
}


// ---------------------------------------------------------------- from sphered's data

use crate::service::Shared;
use crate::working::parse_rfc3339;
use sphere_proto::{RequestStatus, StepKind, TaskState};

const DAY: i64 = 86_400;
/// Most steps kept per agent.
const MAX_STEPS: usize = 10;
/// Most agents on the swarm.
pub const MAX_AGENTS: usize = 9;

/// The branch `path`'s checkout is on (`git symbolic-ref --short -q HEAD`, "main" when detached or unknown).
/// This runs git and blocks: call it from a background thread (the `sync` system does), never from a frame.
pub fn repo_branch(path: &str) -> String {
    crate::gitdata::git_t(std::path::Path::new(path), &["symbolic-ref", "--short", "-q", "HEAD"]).ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "main".to_string())
}

fn first_line(s: &str) -> String {
    s.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").to_string()
}

/// The path an Edit step works on: a touched file named in its title, else the title's last path-like word.
fn edit_path(title: &str, files: &[File]) -> Option<String> {
    if let Some(f) = files.iter().find(|f| title.contains(&f.path)) {
        return Some(f.path.clone());
    }
    let w = title.split_whitespace().last()?;
    (w.contains('/') || w.contains('.')).then(|| w.trim_matches(|c| c == '`' || c == '"' || c == '\'').to_string())
}

fn progress(steps: u32) -> f32 {
    (1.0 - (-(steps as f32) / 14.0).exp()).clamp(0.03, 0.985)
}

/// Two running agents in one repo on the same file. Both rules: the file each is editing right now (the
/// prototype's rule, the only one for the demo), and, for real data, any file both have touched. Pairs in agent
/// order, so the list is deterministic. `touched`: also use the files each agent has touched.
pub fn find_conflicts(agents: &[Agent], touched: bool) -> Vec<Conflict> {
    let live = |s: State| if touched { matches!(s, State::Working | State::Needs) } else { s == State::Working };
    let mut out = Vec::new();
    for (i, a) in agents.iter().enumerate() {
        for b in &agents[i + 1..] {
            if a.repo != b.repo || !live(a.state) || !live(b.state) {
                continue;
            }
            let mut file = match (&a.editing, &b.editing) {
                (Some(x), Some(y)) if x == y => Some(x.clone()),
                _ => None,
            };
            if file.is_none() && touched {
                let has = |ag: &Agent, p: &str| ag.files.iter().any(|f| f.path == p);
                file = a.editing.clone().filter(|p| has(b, p)).or_else(|| b.editing.clone().filter(|p| has(a, p))).or_else(|| a.files.iter().find(|f| has(b, &f.path)).map(|f| f.path.clone()));
            }
            if let Some(file) = file {
                out.push(Conflict { a: a.id.clone(), b: b.id.clone(), file });
            }
        }
    }
    out
}

/// The swarm from sphered's data. `prev` is the model on screen now: it keeps the agents' order and decides the
/// one-shot Merged and Stopped states; `now` is unix seconds.
///
/// Tasks shown: Queued, Working and NeedsYou; Done ones whose branch is not merged, that have a workdir, are not
/// in place and were updated in the last 24 h; a merged task that was Done in `prev` once as `State::Merged`
/// (the scene plays the collapse), then it drops; a Stopped or Failed task that was on screen once as
/// `State::Stopped`, then it drops. At most 9 (what needs you, works or waits first, then the newest done).
/// `queue_pos` is 0 for the next queued agent to start. A repo's branch is `prev`'s, else "main": the `sync`
/// system fills it from `repo_branch` in the background.
pub fn from_shared(s: &Shared, prev: &Model, now: i64) -> Model {
    // (rank, tie-break, agent, repo path)
    let mut picks: Vec<(u8, i64, Agent, String)> = Vec::new();
    for t in &s.tasks {
        let before = prev.agent(&t.id).map(|a| a.state);
        let started = parse_rfc3339(&t.started_at).unwrap_or(0);
        let updated = parse_rfc3339(&t.updated_at).unwrap_or(started);
        let state = match t.state {
            TaskState::Queued => State::Queued,
            TaskState::Working => State::Working,
            TaskState::NeedsYou => State::Needs,
            // until the app knows which finished branches are merged, finished agents wait off screen (otherwise
            // a launch shows old merged work as ready, then plays its merge again)
            TaskState::Done if !s.merged_checked => continue,
            TaskState::Done => {
                if s.merged.contains(&t.id) {
                    if before == Some(State::Done) {
                        State::Merged
                    } else {
                        continue;
                    }
                } else if t.workdir.is_some() && !t.in_place && now - updated <= DAY {
                    State::Done
                } else {
                    continue;
                }
            }
            TaskState::Failed | TaskState::Stopped => match before {
                Some(b) if !matches!(b, State::Stopped | State::Merged) => State::Stopped,
                _ => continue,
            },
        };
        let run = s.runs.get(&t.id);
        let steps: Vec<Step> = run.map(|r| r.steps.iter().rev().take(MAX_STEPS).map(|x| Step { text: x.title.clone(), at: parse_rfc3339(&x.at).unwrap_or(0) }).collect()).unwrap_or_default();
        let files: Vec<File> = t.files.iter().map(|f| File { path: f.path.clone(), added: f.added, removed: f.removed }).collect();
        let editing = run.and_then(|r| r.steps.last()).filter(|x| x.kind == StepKind::Edit).and_then(|x| edit_path(&x.title, &files));
        let step_seq = run.and_then(|r| r.steps.last()).map(|x| x.seq + 1).unwrap_or(t.steps as u64);
        let req = s.requests.iter().rev().find(|r| r.info.task == t.id && matches!(r.status, RequestStatus::Open | RequestStatus::Snoozed));
        let status_or_say = || if t.status.trim().is_empty() { first_line(&t.last_say) } else { t.status.clone() };
        let step = match state {
            State::Queued => "Queued · starts when a slot frees".to_string(),
            State::Needs => req
                .map(|r| {
                    let l = first_line(&r.info.summary);
                    if !l.is_empty() {
                        l
                    } else if let Some(d) = r.info.description.as_deref().map(first_line).filter(|d| !d.is_empty()) {
                        d
                    } else {
                        format!("waiting · {}", r.info.display_name.as_deref().filter(|n| !n.is_empty()).unwrap_or(&r.info.tool))
                    }
                })
                .unwrap_or_else(status_or_say),
            // a finished agent shows what it came to (sphered's status reads "Done: <its result>"), not its last command
            State::Done | State::Merged => {
                let st = status_or_say();
                let r = st.strip_prefix("Done: ").unwrap_or(&st).trim().to_string();
                if r.is_empty() { steps.first().map(|x| x.text.clone()).unwrap_or_default() } else { r }
            }
            _ => steps.first().map(|x| x.text.clone()).filter(|x| !x.is_empty()).unwrap_or_else(status_or_say),
        };
        let p = match state {
            State::Queued => 0.0,
            State::Done | State::Merged => 1.0,
            _ => progress(t.steps),
        };
        let tie = if state == State::Done { -updated } else { 0 };
        let rank = match state {
            State::Needs | State::Merged | State::Stopped => 0,
            State::Working => 1,
            State::Queued => 2,
            State::Done => 3,
        };
        let agent = Agent { id: t.id.clone(), title: t.title.clone(), repo: 0, branch: t.branch.clone().unwrap_or_default(), state, p, step, cost: t.cost_usd, started_at: started, steps, files, editing, queue_pos: None, step_seq };
        picks.push((rank, tie, agent, t.repo.clone()));
    }
    picks.sort_by(|x, y| (x.0, x.1, &x.2.id).cmp(&(y.0, y.1, &y.2.id)));
    picks.truncate(MAX_AGENTS);
    // stable order: the agents already on screen keep their places, new ones follow, oldest first
    let place = |id: &str| prev.agents.iter().position(|a| a.id == id);
    picks.sort_by_key(|p| match place(&p.2.id) {
        Some(i) => (0u8, i, 0i64, p.2.id.clone()),
        None => (1, 0, p.2.started_at, p.2.id.clone()),
    });
    let mut repos: Vec<Repo> = Vec::new();
    for p in &mut picks {
        let i = match repos.iter().position(|r| r.path == p.3) {
            Some(i) => i,
            None => {
                let key = std::path::Path::new(&p.3).file_name().map(|n| n.to_string_lossy().to_string()).filter(|n| !n.is_empty()).unwrap_or_else(|| p.3.clone());
                let branch = prev.repos.iter().find(|r| r.path == p.3).map(|r| r.branch.clone()).unwrap_or_else(|| "main".to_string());
                repos.push(Repo { key, path: p.3.clone(), branch });
                repos.len() - 1
            }
        };
        p.2.repo = i;
    }
    let mut agents: Vec<Agent> = picks.into_iter().map(|p| p.2).collect();
    let mut queued: Vec<(i64, String)> = agents.iter().filter(|a| a.state == State::Queued).map(|a| (a.started_at, a.id.clone())).collect();
    queued.sort();
    for a in &mut agents {
        a.queue_pos = queued.iter().position(|q| q.1 == a.id);
    }
    let conflicts = find_conflicts(&agents, true);
    Model { agents, repos, conflicts, limit: s.max_concurrent.unwrap_or(3), cost_today: s.cost_today_usd }
}

// ---------------------------------------------------------------- the prototype's example agents

/// The fixed "now" the demo's ages are relative to (unix seconds).
pub const DEMO_NOW: i64 = 1_800_000_000;
/// The base of the strip's cost in the demo (SPEC 9 `sim.today0`).
const DEMO_TODAY0: f64 = 0.57;

struct Def {
    id: &'static str,
    repo: usize,
    title: &'static str,
    branch: &'static str,
    state: State,
    p: f32,
    cost: f64,
    age: i64,
    iv: f32,
    files: &'static [(&'static str, u64, u64)],
    first: (&'static str, Option<&'static str>),
    script: &'static [(&'static str, Option<&'static str>)],
    done: &'static str,
    hist: &'static [&'static str],
}

const DEMO_REPOS: [&str; 3] = ["api-server", "web-app", "sphere-cli"];

const DEFS: [Def; 6] = [
    Def {
        id: "rate",
        repo: 0,
        title: "Add rate limiting to /login",
        branch: "feat/login-rate-limit",
        state: State::Working,
        p: 0.34,
        cost: 0.12,
        age: 192,
        iv: 3.0,
        files: &[("src/auth.rs", 42, 3), ("src/middleware/limit.rs", 31, 0)],
        first: ("editing src/auth.rs", Some("src/auth.rs")),
        script: &[
            ("editing src/routes.rs", Some("src/routes.rs")),
            ("wiring the limiter into src/routes.rs", Some("src/routes.rs")),
            ("editing src/routes.rs", Some("src/routes.rs")),
            ("running cargo test · 18 / 31", None),
            ("fixing a clippy warning in src/auth.rs", Some("src/auth.rs")),
            ("running cargo test · 31 / 31", None),
            ("writing tests/login_rate.rs", Some("tests/login_rate.rs")),
            ("committing 4 files", None),
            ("opening PR #482", None),
        ],
        done: "PR #482 · CI green",
        hist: &["planning the limiter", "reading src/auth.rs", "creating branch feat/login-rate-limit"],
    },
    Def {
        id: "redis",
        repo: 0,
        title: "Move sessions to Redis",
        branch: "feat/redis-sessions",
        state: State::Working,
        p: 0.58,
        cost: 0.31,
        age: 410,
        iv: 3.4,
        files: &[("src/session.rs", 64, 21), ("src/routes.rs", 12, 4)],
        first: ("editing src/routes.rs", Some("src/routes.rs")),
        script: &[
            ("editing src/routes.rs", Some("src/routes.rs")),
            ("editing src/routes.rs", Some("src/routes.rs")),
            ("reading src/session.rs", None),
            ("editing src/session.rs", Some("src/session.rs")),
            ("adding redis to Cargo.toml", Some("Cargo.toml")),
            ("running cargo check", None),
            ("editing src/store/redis.rs", Some("src/store/redis.rs")),
            ("running migration 0007", None),
            ("running cargo test · 12 / 31", None),
            ("editing src/session.rs", Some("src/session.rs")),
            ("running cargo test · 31 / 31", None),
            ("opening PR #483", None),
        ],
        done: "PR #483 · CI green",
        hist: &["migrating the session type", "reading src/session.rs", "creating branch feat/redis-sessions"],
    },
    Def {
        id: "checkout",
        repo: 1,
        title: "Fix flaky checkout test",
        branch: "fix/checkout-flake",
        state: State::Working,
        p: 0.71,
        cost: 0.18,
        age: 275,
        iv: 2.9,
        files: &[("e2e/checkout.spec.ts", 18, 9)],
        first: ("running checkout.spec · repeat 4 / 20", None),
        script: &[
            ("reading e2e/checkout.spec.ts", None),
            ("editing e2e/checkout.spec.ts", Some("e2e/checkout.spec.ts")),
            ("running pnpm test:e2e · repeat 6 / 20", None),
            ("replacing a fixed wait in e2e/helpers.ts", Some("e2e/helpers.ts")),
            ("running pnpm test:e2e · repeat 13 / 20", None),
            ("running pnpm test:e2e · repeat 20 / 20", None),
            ("committing 2 files", None),
            ("opening PR #232", None),
        ],
        done: "20 / 20 pass · PR #232",
        hist: &["reproduced the flake 3 / 20", "reading e2e/helpers.ts", "creating branch fix/checkout-flake"],
    },
    Def {
        id: "react",
        repo: 1,
        title: "Upgrade React to 19",
        branch: "chore/react-19",
        state: State::Queued,
        p: 0.0,
        cost: 0.0,
        age: 0,
        iv: 3.1,
        files: &[],
        first: ("creating the worktree", None),
        script: &[
            ("bumping react and react-dom to 19", Some("package.json")),
            ("running the codemod", None),
            ("editing src/components/Cart.tsx", Some("src/components/Cart.tsx")),
            ("fixing 3 type errors", None),
            ("running pnpm test · 112 / 240", None),
            ("editing src/hooks/useCheckout.ts", Some("src/hooks/useCheckout.ts")),
            ("running pnpm test · 240 / 240", None),
            ("opening PR #233", None),
        ],
        done: "PR #233 · CI green",
        hist: &[],
    },
    Def {
        id: "json",
        repo: 2,
        title: "Add --json flag to sphere run",
        branch: "feat/run-json",
        state: State::Done,
        p: 1.0,
        cost: 0.66,
        age: 1210,
        iv: 3.2,
        files: &[("src/cmd/run.rs", 48, 6), ("src/output.rs", 22, 2), ("tests/run_json.rs", 31, 0)],
        first: ("PR #77 · CI green", None),
        script: &[],
        done: "PR #77 · CI green",
        hist: &["opening PR #77", "committing 3 files", "running cargo test · 24 / 24"],
    },
    Def {
        id: "cold",
        repo: 2,
        title: "Speed up cold start",
        branch: "perf/cold-start",
        state: State::Queued,
        p: 0.0,
        cost: 0.0,
        age: 0,
        iv: 3.3,
        files: &[],
        first: ("profiling the first run", None),
        script: &[
            ("reading src/boot.rs", None),
            ("editing src/boot.rs", Some("src/boot.rs")),
            ("lazy-loading the plugin index", Some("src/plugins.rs")),
            ("running the benchmark · 410 ms", None),
            ("editing src/plugins.rs", Some("src/plugins.rs")),
            ("running the benchmark · 210 ms", None),
            ("opening PR #78", None),
        ],
        done: "PR #78 · 410 → 210 ms",
        hist: &[],
    },
];

/// The prototype's request step text per agent (SPEC 3.2) and when it asks (sim seconds).
const DEMO_REQS: [(&str, f32, &str); 2] = [("checkout", 6.0, "waiting to run pnpm test:e2e"), ("redis", 47.0, "waiting · migrate live sessions?")];

/// The prototype's LCG: `seed = seed * 16807 % 2147483647`.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> f32 {
        self.0 = self.0 * 16807 % 2_147_483_647;
        (self.0 as f64 / 2_147_483_647.0) as f32
    }
}

fn demo_agent(d: &Def) -> Agent {
    let at = |ago: i64| DEMO_NOW - ago;
    let steps: Vec<Step> = if d.state == State::Queued {
        Vec::new()
    } else {
        std::iter::once(d.first.0).chain(d.hist.iter().copied()).enumerate().map(|(i, t)| Step { text: t.to_string(), at: at(16 * i as i64 + 5) }).collect()
    };
    Agent {
        id: d.id.to_string(),
        title: d.title.to_string(),
        repo: d.repo,
        branch: d.branch.to_string(),
        state: d.state,
        p: d.p,
        step: if d.state == State::Queued { "Queued · starts when a slot frees".to_string() } else { d.first.0.to_string() },
        cost: d.cost,
        started_at: at(d.age),
        step_seq: steps.len() as u64,
        steps,
        files: d.files.iter().map(|f| File { path: f.0.to_string(), added: f.1, removed: f.2 }).collect(),
        editing: if d.state == State::Working { d.first.1.map(str::to_string) } else { None },
        queue_pos: None,
    }
}

fn demo_repos() -> Vec<Repo> {
    DEMO_REPOS.iter().map(|k| Repo { key: k.to_string(), path: format!("/demo/{k}"), branch: "main".to_string() }).collect()
}

fn finish_demo(agents: Vec<Agent>) -> Model {
    let mut agents = agents;
    let mut next = 0;
    for a in &mut agents {
        a.queue_pos = (a.state == State::Queued).then(|| {
            next += 1;
            next - 1
        });
    }
    let conflicts = find_conflicts(&agents, false);
    let cost_today = DEMO_TODAY0 + agents.iter().map(|a| a.cost).sum::<f64>();
    Model { agents, repos: demo_repos(), conflicts, limit: 3, cost_today }
}

/// The prototype's six example agents at the start (SPEC 9).
#[cfg(test)]
pub fn demo() -> Model {
    finish_demo(DEFS.iter().map(demo_agent).collect())
}

/// The demo `t` seconds in: steps land per agent from their scripts, checkout asks at 6 s and redis at 47 s
/// (nothing answers), agents that run out of script become Done and queued ones start while fewer than 3 run.
/// Deterministic for a given `t` (a fixed 1/20 s step, the SPEC's LCG with seed 11); `DEMO_NOW + t` is "now".
pub fn demo_at(t: f32) -> Model {
    struct Sa {
        i: usize,
        t_next: f32,
        dpv: f32,
    }
    let mut rng = Lcg(11);
    let mut agents: Vec<Agent> = DEFS.iter().map(demo_agent).collect();
    let mut sa: Vec<Sa> = DEFS
        .iter()
        .map(|d| {
            let t_next = if d.state == State::Working { d.iv * (0.7 + 0.5 * rng.next()) } else { 0.0 };
            Sa { i: 0, t_next, dpv: (0.008f32).max((1.0 - d.p - 0.03) / d.script.len().max(1) as f32) }
        })
        .collect();
    let mut due = [DEMO_REQS[0].1, DEMO_REQS[1].1];
    let n = (t.clamp(0.0, 600.0) * 20.0 + 1e-3).floor() as usize;
    for k in 1..=n {
        let now = k as f32 / 20.0;
        let stamp = DEMO_NOW + now as i64;
        // requests: an agent that is not working asks 3 s later
        for (r, (id, _, text)) in DEMO_REQS.iter().enumerate() {
            let j = DEFS.iter().position(|d| d.id == *id).unwrap();
            if now >= due[r] {
                if agents[j].state == State::Working {
                    agents[j].state = State::Needs;
                    agents[j].step = text.to_string();
                    agents[j].editing = None;
                    due[r] = f32::MAX;
                } else if agents[j].state != State::Needs {
                    due[r] += 3.0;
                }
            }
        }
        // queued agents start while fewer than 3 run
        let mut running = agents.iter().filter(|a| matches!(a.state, State::Working | State::Needs)).count();
        for j in 0..agents.len() {
            if running >= 3 {
                break;
            }
            if agents[j].state == State::Queued {
                let d = &DEFS[j];
                let a = &mut agents[j];
                a.state = State::Working;
                a.p = 0.03;
                a.step = d.first.0.to_string();
                a.editing = d.first.1.map(str::to_string);
                a.started_at = stamp;
                a.steps = vec![Step { text: d.first.0.to_string(), at: stamp }];
                a.step_seq = 1;
                sa[j].t_next = now + 1.6;
                sa[j].dpv = (0.008f32).max((1.0 - 0.03 - 0.03) / d.script.len() as f32);
                running += 1;
            }
        }
        // steps
        for j in 0..agents.len() {
            if agents[j].state != State::Working || now < sa[j].t_next {
                continue;
            }
            let d = &DEFS[j];
            let a = &mut agents[j];
            if sa[j].i >= d.script.len() {
                a.state = State::Done;
                a.p = 1.0;
                a.step = d.done.to_string();
                a.editing = None;
                a.steps.insert(0, Step { text: d.done.to_string(), at: stamp });
                a.steps.truncate(MAX_STEPS);
                a.step_seq += 1;
                continue;
            }
            let (text, file) = d.script[sa[j].i];
            sa[j].i += 1;
            a.step = text.to_string();
            a.editing = file.map(str::to_string);
            if let Some(f) = file {
                let add = 3 + (rng.next() * 11.0).floor() as u64;
                let del = if rng.next() < 0.5 { (rng.next() * 3.0).floor() as u64 } else { 0 };
                match a.files.iter_mut().find(|x| x.path == f) {
                    Some(x) => {
                        x.added += add;
                        x.removed += del;
                    }
                    None => a.files.push(File { path: f.to_string(), added: add, removed: del }),
                }
            }
            a.p = (a.p + sa[j].dpv).min(0.985);
            a.cost += 0.006 + rng.next() as f64 * 0.016;
            a.steps.insert(0, Step { text: text.to_string(), at: stamp });
            a.steps.truncate(MAX_STEPS);
            a.step_seq += 1;
            sa[j].t_next = now + d.iv * (0.88 + rng.next() * 0.24);
        }
    }
    finish_demo(agents)
}

// ---------------------------------------------------------------- text the HUD shows

/// The state chip's words.
pub fn chip(s: State) -> &'static str {
    match s {
        State::Working => "Working",
        State::Needs => "Needs you",
        State::Done | State::Merged => "Ready to merge",
        State::Queued => "Queued",
        State::Stopped => "Stopped",
    }
}

/// "48%", queued "—".
pub fn pct(a: &Agent) -> String {
    if a.state == State::Queued {
        "—".to_string()
    } else {
        format!("{}%", (a.p * 100.0).round() as i32)
    }
}

/// "$0.12", queued "".
/// An agent's cost as Claude Code reports it: an estimate at API prices (a subscription isn't billed it).
pub fn cost(a: &Agent) -> String {
    if a.state == State::Queued {
        String::new()
    } else {
        format!("≈ ${:.2}", a.cost)
    }
}

/// m:ss
pub fn fmt_dur(secs: i64) -> String {
    let s = secs.max(0);
    format!("{}:{:02}", s / 60, s % 60)
}

/// A step's age: "now" (only for the first row, under 2 s), "12s", "3m".
pub fn ago(secs: i64, first: bool) -> String {
    if secs < 2 && first {
        "now".to_string()
    } else if secs < 60 {
        format!("{}s", secs.max(0))
    } else {
        format!("{}m", (secs as f64 / 60.0).round() as i64)
    }
}

/// The hover tooltip: the title, then "{repo} · {status}" (SPEC 4.1).
pub fn tooltip(a: &Agent, m: &Model, now: i64) -> (String, String) {
    let repo = m.repos.get(a.repo).map(|r| r.key.as_str()).unwrap_or("");
    let status = match a.state {
        State::Needs => "Needs you · click to answer".to_string(),
        State::Done => "Ready to merge · click to open".to_string(),
        State::Queued => "Queued · starts when a slot frees".to_string(),
        State::Working => format!("Step {} · {} · {} running", a.step_seq, a.step, fmt_dur(now - a.started_at)),
        State::Merged => "Merged".to_string(),
        State::Stopped => "Stopped".to_string(),
    };
    (a.title.clone(), format!("{repo} · {status}"))
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Agents that count on a repo's ring and in the orbits panel (not the ones leaving).
fn on_repo(m: &Model, repo: usize) -> Vec<&Agent> {
    m.agents.iter().filter(|a| a.repo == repo && !matches!(a.state, State::Merged | State::Stopped)).collect()
}

/// A repo ring's label: ("api-server", "2 agents").
pub fn ring_label(m: &Model, repo: usize) -> (String, String) {
    (m.repos.get(repo).map(|r| r.key.clone()).unwrap_or_default(), plural(on_repo(m, repo).len(), "agent", "agents"))
}

/// The outer ring's label: ("Ready to merge", "1 ready" or "none yet").
pub fn ready_label(m: &Model) -> (String, String) {
    let n = m.count(State::Done);
    ("Ready to merge".to_string(), if n == 0 { "none yet".to_string() } else { format!("{n} ready") })
}

/// The mission strip's segments as (number, label, class): "6" "agents", "3" "working", "1" "needs you" (Needs),
/// "1" "ready" (Done), "2" "queued". The class is only set where the strip colours it.
pub fn strip(m: &Model) -> Vec<(String, String, Option<State>)> {
    let n = m.agents.iter().filter(|a| !matches!(a.state, State::Merged | State::Stopped)).count();
    vec![
        (n.to_string(), if n == 1 { "agent" } else { "agents" }.to_string(), None),
        (m.count(State::Working).to_string(), "working".to_string(), None),
        (m.count(State::Needs).to_string(), "needs you".to_string(), Some(State::Needs)),
        (m.count(State::Done).to_string(), "ready".to_string(), Some(State::Done)),
        (m.count(State::Queued).to_string(), "queued".to_string(), None),
    ]
}

/// "$1.89"
/// Today's total, an estimate at API prices like each agent's (the strip follows it with "today · API est.").
pub fn strip_cost(m: &Model) -> String {
    format!("≈ ${:.2}", m.cost_today)
}

/// The run slots panel: one row per slot (title, "48%" or "WAIT", progress 0..1, needs you) or None for Free,
/// then the queued titles in queue order.
#[allow(clippy::type_complexity)]
pub fn slots(m: &Model) -> (Vec<Option<(String, String, f32, bool)>>, Vec<String>) {
    let mut rows: Vec<Option<(String, String, f32, bool)>> = m
        .running()
        .into_iter()
        .take(m.limit)
        .map(|a| {
            let needs = a.state == State::Needs;
            Some((a.title.clone(), if needs { "WAIT".to_string() } else { pct(a) }, a.p, needs))
        })
        .collect();
    rows.resize(m.limit, None);
    let mut q: Vec<&Agent> = m.agents.iter().filter(|a| a.state == State::Queued).collect();
    q.sort_by_key(|a| a.queue_pos.unwrap_or(usize::MAX));
    (rows, q.into_iter().map(|a| a.title.clone()).collect())
}

/// The slots panel's header: "3 of 3".
pub fn slots_header(m: &Model) -> String {
    format!("{} of {}", m.running().len(), m.limit)
}

/// The orbits panel: per repo (key, "main · 2 agents", its agents' states in model order).
pub fn orbits(m: &Model) -> Vec<(String, String, Vec<State>)> {
    m.repos
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let on = on_repo(m, i);
            (r.key.clone(), format!("{} · {}", r.branch, plural(on.len(), "agent", "agents")), on.iter().map(|a| a.state).collect())
        })
        .collect()
}

/// The orbits panel's header: "3 repos".
pub fn orbits_header(m: &Model) -> String {
    plural(m.repos.len(), "repo", "repos")
}

/// The focus panel's Steps label.
pub fn focus_steps_label(a: &Agent) -> String {
    match a.state {
        State::Queued => "waiting for a slot".to_string(),
        State::Done | State::Merged => format!("{} recent", a.steps.len()),
        _ => format!("step {}", a.step_seq),
    }
}

/// The focus panel's Diff line: "+84 −6 · 3 files" (U+2212) or "no changes yet".
pub fn diff_line(a: &Agent) -> String {
    if a.files.is_empty() {
        return "no changes yet".to_string();
    }
    let (add, del) = a.files.iter().fold((0, 0), |s, f| (s.0 + f.added, s.1 + f.removed));
    format!("+{add} \u{2212}{del} · {}", plural(a.files.len(), "file", "files"))
}

// ---------------------------------------------------------------- events

/// What changed between two models.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// finished, ready to merge
    Ready(String),
    /// left the queue
    Started(String),
    NeedsYou(String),
    Merged(String),
    Stopped(String),
    ConflictNew(Conflict),
    ConflictCleared(Conflict),
    /// a new step landed
    StepLanded(String),
}

pub use crate::live::ToastKind;

/// The events from `prev` to `next`: only for agents in both (a new agent has no event), and conflicts that are
/// new, or gone while both agents are still there.
pub fn diff(prev: &Model, next: &Model) -> Vec<Event> {
    let mut out = Vec::new();
    for n in &next.agents {
        let Some(p) = prev.agent(&n.id) else { continue };
        if p.state != n.state {
            match n.state {
                State::Working if p.state == State::Queued => out.push(Event::Started(n.id.clone())),
                State::Needs => out.push(Event::NeedsYou(n.id.clone())),
                State::Done if matches!(p.state, State::Working | State::Needs) => out.push(Event::Ready(n.id.clone())),
                State::Merged => out.push(Event::Merged(n.id.clone())),
                State::Stopped => out.push(Event::Stopped(n.id.clone())),
                _ => {}
            }
        }
        if n.step_seq > p.step_seq {
            out.push(Event::StepLanded(n.id.clone()));
        }
    }
    let same = |x: &Conflict, y: &Conflict| x.a == y.a && x.b == y.b;
    for c in &next.conflicts {
        if !prev.conflicts.iter().any(|p| same(p, c)) {
            out.push(Event::ConflictNew(c.clone()));
        }
    }
    for c in &prev.conflicts {
        if !next.conflicts.iter().any(|n| same(n, c)) && next.agent(&c.a).is_some() && next.agent(&c.b).is_some() {
            out.push(Event::ConflictCleared(c.clone()));
        }
    }
    out
}

fn repo_of<'a>(m: &'a Model, a: &Agent) -> Option<&'a Repo> {
    m.repos.get(a.repo)
}

/// The agent an event is about, for the strings below.
fn subject<'a>(e: &Event, m: &'a Model) -> Option<&'a Agent> {
    match e {
        Event::Ready(id) | Event::Started(id) | Event::NeedsYou(id) | Event::Merged(id) | Event::Stopped(id) | Event::StepLanded(id) => m.agent(id),
        Event::ConflictNew(c) | Event::ConflictCleared(c) => m.agent(&c.a),
    }
}

/// The toast for an event (SPEC 6.4).
pub fn toast(e: &Event, m: &Model) -> Option<(String, ToastKind)> {
    let a = subject(e, m)?;
    Some(match e {
        Event::Ready(_) => (format!("{} · ready to merge", a.title), ToastKind::Ok),
        Event::Started(_) => (format!("{} · started", a.title), ToastKind::Ok),
        Event::NeedsYou(_) => (format!("{} · needs you", a.title), ToastKind::Plain),
        Event::ConflictNew(c) => (format!("Conflict · both editing {}", c.file), ToastKind::Plain),
        Event::ConflictCleared(c) => (format!("Conflict cleared · {}", c.file), ToastKind::Ok),
        Event::Merged(_) => (format!("Merged into {}", repo_of(m, a).map(|r| r.branch.as_str()).unwrap_or("main")), ToastKind::Ok),
        Event::Stopped(_) => (format!("Stopped · {} · worktree kept", a.title), ToastKind::Plain),
        Event::StepLanded(_) => return None,
    })
}

/// The ticker item for an event (SPEC 6.3): (key, value, warn).
pub fn ticker(e: &Event, m: &Model) -> Option<(String, String, bool)> {
    let a = subject(e, m)?;
    let key = repo_of(m, a).map(|r| r.key.to_uppercase()).unwrap_or_default();
    let t = a.title.to_lowercase();
    Some(match e {
        Event::Ready(_) => (key, format!("{t} · {} · ready to merge", a.step), false),
        Event::Started(_) => (key, format!("{t} · started in a free slot"), false),
        Event::NeedsYou(_) => (key, format!("{t} · {}", a.step), true),
        Event::ConflictNew(c) => {
            let b = m.agent(&c.b)?;
            (key, format!("conflict · {t} and {} both editing {}", b.title.to_lowercase(), c.file), true)
        }
        Event::Merged(_) => ("GIT".to_string(), format!("{} ← {t} · {}", repo_of(m, a).map(|r| r.branch.as_str()).unwrap_or("main"), a.step), false),
        Event::Stopped(_) => (key, format!("{t} · stopped, worktree kept"), true),
        Event::ConflictCleared(_) | Event::StepLanded(_) => return None,
    })
}

/// The line Sphere says for an event (SPEC 6.2).
pub fn say(e: &Event, m: &Model) -> Option<String> {
    let a = subject(e, m)?;
    match e {
        Event::Ready(_) => Some(format!("{} is ready to merge.", a.title)),
        Event::ConflictNew(c) => Some(format!("Heads up: two agents in {} are both editing {}.", repo_of(m, a).map(|r| r.key.as_str()).unwrap_or(""), c.file)),
        Event::Merged(_) => Some(format!("Merged into {}.", repo_of(m, a).map(|r| r.branch.as_str()).unwrap_or("main"))),
        _ => None,
    }
}

// ---------------------------------------------------------------- the system

/// What `sync` drains into sounds, toasts, the ticker and speech; consumers drain it.
#[derive(Resource, Default)]
pub struct Events(pub Vec<Event>);

/// Repo branches read in the background, by repo path.
#[derive(Default)]
struct Branches {
    known: std::sync::Mutex<std::collections::HashMap<String, String>>,
    busy: std::sync::Mutex<std::collections::HashSet<String>>,
}

#[derive(Default)]
struct SyncState {
    ver: Option<u64>,
    /// real seconds at the last build
    at: f32,
    /// sim seconds at the last demo build
    demo_t: Option<f32>,
    was_open: bool,
    /// real seconds when each repo's branch was last asked for
    asked: std::collections::HashMap<String, f32>,
    branches: std::sync::Arc<Branches>,
}

fn unix_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Rebuilds `Swarm.model` while the swarm is open (or opening): from sphered's data when its version changes
/// (and every second, for the 24 h window), or from `demo_at(sim.t)` offline. Pushes the changes as `Events`.
fn sync(svc: Res<crate::service::Service>, args: Res<crate::Args>, sim: Res<crate::core::Sim>, time: Res<Time<Real>>, mut swarm: ResMut<crate::swarm::Swarm>, mut events: ResMut<Events>, mut st: Local<SyncState>) {
    let open = swarm.open || swarm.amt.x > 0.01;
    if !open {
        st.was_open = false;
        return;
    }
    let real = time.elapsed_secs();
    let fresh = !st.was_open;
    let next = if args.offline() {
        if !fresh && st.demo_t.is_some_and(|d| (sim.t - d).abs() < 0.2 && sim.t >= d) {
            st.was_open = true;
            return;
        }
        st.demo_t = Some(sim.t);
        demo_at(sim.t)
    } else {
        let s = svc.shared.lock().unwrap();
        if !fresh && st.ver == Some(s.version) && real - st.at < 1.0 {
            return;
        }
        st.ver = Some(s.version);
        let mut m = from_shared(&s, &swarm.model, unix_now());
        drop(s);
        // the branches read so far; ask for the ones not read yet (and refresh the rest now and then)
        let known = st.branches.known.lock().unwrap().clone();
        for r in &mut m.repos {
            if let Some(b) = known.get(&r.path) {
                r.branch = b.clone();
            }
            let due = st.asked.get(&r.path).map(|t| real - t > 30.0).unwrap_or(true);
            if due && st.branches.busy.lock().unwrap().insert(r.path.clone()) {
                st.asked.insert(r.path.clone(), real);
                let (path, br) = (r.path.clone(), st.branches.clone());
                std::thread::spawn(move || {
                    let b = repo_branch(&path);
                    br.known.lock().unwrap().insert(path.clone(), b);
                    br.busy.lock().unwrap().remove(&path);
                });
            }
        }
        m
    };
    st.at = real;
    st.was_open = true;
    if next != swarm.model {
        if !fresh {
            let ev = diff(&swarm.model, &next);
            events.0.extend(ev);
        }
        swarm.model = next;
        swarm.version += 1;
    }
}

pub fn build(app: &mut App) {
    app.init_resource::<Events>().add_systems(Update, sync);
}

#[cfg(test)]
mod tests {
    use super::*;
    use sphere_proto::TaskView;

    fn task(id: &str, state: TaskState, started: &str, extra: serde_json::Value) -> TaskView {
        let mut v = serde_json::json!({
            "id": id, "title": format!("Task {id}"), "prompt": "", "state": state, "status": format!("status {id}"),
            "repo": "/work/api-server", "workdir": format!("/wt/{id}"), "branch": format!("sphere/{id}"), "in_place": false,
            "budget_usd": 0.0, "cost_usd": 0.5, "started_at": started, "updated_at": started,
        });
        if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
            for (k, x) in e {
                o.insert(k.clone(), x.clone());
            }
        }
        serde_json::from_value(v).unwrap()
    }

    /// A time shortly before `NOW` as the tasks' own timestamp.
    fn t_ago(secs: i64) -> String {
        let base = parse_rfc3339("2027-01-15T08:00:00Z").unwrap();
        let s = base - secs;
        // civil from days (the tests only need dates after 2027-01-01)
        let (days, rem) = (s.div_euclid(86_400), s.rem_euclid(86_400));
        let jan1 = parse_rfc3339("2027-01-01T00:00:00Z").unwrap() / 86_400;
        let day = 1 + (days - jan1);
        format!("2027-01-{day:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
    }

    fn now() -> i64 {
        parse_rfc3339("2027-01-15T08:00:00Z").unwrap()
    }

    fn shared(tasks: Vec<TaskView>) -> Shared {
        Shared { tasks, merged_checked: true, ..Default::default() }
    }

    fn state_of(m: &Model, id: &str) -> Option<State> {
        m.agent(id).map(|a| a.state)
    }

    #[test]
    fn states_map_and_progress_follows_steps() {
        let s = shared(vec![
            task("q", TaskState::Queued, &t_ago(500), serde_json::json!({})),
            task("w", TaskState::Working, &t_ago(400), serde_json::json!({"steps": 14})),
            task("n", TaskState::NeedsYou, &t_ago(300), serde_json::json!({"steps": 0})),
            task("d", TaskState::Done, &t_ago(200), serde_json::json!({})),
        ]);
        let m = from_shared(&s, &Model::default(), now());
        assert_eq!(state_of(&m, "q"), Some(State::Queued));
        assert_eq!(state_of(&m, "w"), Some(State::Working));
        assert_eq!(state_of(&m, "n"), Some(State::Needs));
        assert_eq!(state_of(&m, "d"), Some(State::Done));
        assert_eq!(m.agent("q").unwrap().p, 0.0);
        assert!((m.agent("w").unwrap().p - (1.0 - (-1.0f32).exp())).abs() < 1e-4);
        assert_eq!(m.agent("n").unwrap().p, 0.03);
        assert_eq!(m.agent("d").unwrap().p, 1.0);
        assert_eq!(m.agent("q").unwrap().step, "Queued · starts when a slot frees");
        assert_eq!(m.agent("w").unwrap().step, "status w");
        assert_eq!(m.limit, 3);
        assert_eq!(m.repos.len(), 1);
        assert_eq!(m.repos[0].key, "api-server");
        assert_eq!(m.repos[0].branch, "main");
    }

    #[test]
    fn done_tasks_need_a_workdir_not_in_place_and_to_be_recent() {
        let s = shared(vec![
            task("old", TaskState::Done, &t_ago(90_000), serde_json::json!({})),
            task("inplace", TaskState::Done, &t_ago(100), serde_json::json!({"in_place": true})),
            task("nowd", TaskState::Done, &t_ago(100), serde_json::json!({"workdir": null})),
            task("ok", TaskState::Done, &t_ago(100), serde_json::json!({})),
            task("dead", TaskState::Failed, &t_ago(100), serde_json::json!({})),
        ]);
        let m = from_shared(&s, &Model::default(), now());
        assert_eq!(m.agents.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), vec!["ok"]);
    }

    #[test]
    fn finished_agents_wait_until_the_merged_check_is_in() {
        let mut s = shared(vec![task("ok", TaskState::Done, &t_ago(100), serde_json::json!({})), task("w", TaskState::Working, &t_ago(100), serde_json::json!({}))]);
        s.merged_checked = false;
        let m = from_shared(&s, &Model::default(), now());
        assert_eq!(m.agents.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), vec!["w"]);
        // an old merge found by the first check never shows (no "Merged" replayed at launch)
        s.merged_checked = true;
        s.merged.insert("ok".into());
        let m2 = from_shared(&s, &m, now());
        assert_eq!(m2.agents.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), vec!["w"]);
        assert!(diff(&m, &m2).iter().all(|e| !matches!(e, Event::Merged(_))));
    }

    #[test]
    fn a_finished_agent_shows_its_result_not_its_last_command() {
        let s = shared(vec![task("ok", TaskState::Done, &t_ago(100), serde_json::json!({"status": "Done: Flake fixed"}))]);
        assert_eq!(from_shared(&s, &Model::default(), now()).agents[0].step, "Flake fixed");
    }

    #[test]
    fn order_is_oldest_first_and_stable_and_capped_at_nine() {
        let mut tasks: Vec<TaskView> = (0..12).map(|i| task(&format!("t{i:02}"), TaskState::Working, &t_ago(1000 - i * 10), serde_json::json!({}))).collect();
        let m = from_shared(&shared(tasks.clone()), &Model::default(), now());
        assert_eq!(m.agents.len(), 9);
        assert_eq!(m.agents[0].id, "t00");
        assert_eq!(m.agents[8].id, "t08");
        // an older task arriving later does not push the others around
        tasks.truncate(3);
        let m1 = from_shared(&shared(tasks.clone()), &Model::default(), now());
        tasks.push(task("old", TaskState::Working, &t_ago(5000), serde_json::json!({})));
        let m2 = from_shared(&shared(tasks), &m1, now());
        assert_eq!(m2.agents.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), vec!["t00", "t01", "t02", "old"]);
    }

    #[test]
    fn the_cap_keeps_what_runs_before_the_newest_done() {
        let mut tasks: Vec<TaskView> = (0..8).map(|i| task(&format!("w{i}"), TaskState::Working, &t_ago(1000 - i), serde_json::json!({}))).collect();
        tasks.push(task("d1", TaskState::Done, &t_ago(900), serde_json::json!({})));
        tasks.push(task("d2", TaskState::Done, &t_ago(100), serde_json::json!({})));
        let m = from_shared(&shared(tasks), &Model::default(), now());
        assert_eq!(m.agents.len(), 9);
        assert!(m.agent("d2").is_some() && m.agent("d1").is_none());
    }

    #[test]
    fn merged_plays_once_then_leaves() {
        let tasks = vec![task("a", TaskState::Done, &t_ago(100), serde_json::json!({}))];
        let mut s = shared(tasks);
        let m0 = from_shared(&s, &Model::default(), now());
        assert_eq!(state_of(&m0, "a"), Some(State::Done));
        s.merged.insert("a".to_string());
        let m1 = from_shared(&s, &m0, now());
        assert_eq!(state_of(&m1, "a"), Some(State::Merged));
        assert_eq!(diff(&m0, &m1), vec![Event::Merged("a".to_string())]);
        let m2 = from_shared(&s, &m1, now());
        assert!(m2.agents.is_empty());
        // a merged task never seen as done does not show at all
        assert!(from_shared(&s, &Model::default(), now()).agents.is_empty());
    }

    #[test]
    fn stopped_shows_once() {
        let mut s = shared(vec![task("a", TaskState::Working, &t_ago(100), serde_json::json!({}))]);
        let m0 = from_shared(&s, &Model::default(), now());
        s.tasks[0].state = TaskState::Stopped;
        let m1 = from_shared(&s, &m0, now());
        assert_eq!(state_of(&m1, "a"), Some(State::Stopped));
        assert_eq!(diff(&m0, &m1), vec![Event::Stopped("a".to_string())]);
        assert!(from_shared(&s, &m1, now()).agents.is_empty());
        assert!(from_shared(&s, &Model::default(), now()).agents.is_empty());
    }

    #[test]
    fn queue_positions_follow_start_order() {
        let s = shared(vec![
            task("b", TaskState::Queued, &t_ago(100), serde_json::json!({})),
            task("a", TaskState::Queued, &t_ago(300), serde_json::json!({})),
            task("w", TaskState::Working, &t_ago(900), serde_json::json!({})),
        ]);
        let m = from_shared(&s, &Model::default(), now());
        assert_eq!(m.agent("a").unwrap().queue_pos, Some(0));
        assert_eq!(m.agent("b").unwrap().queue_pos, Some(1));
        assert_eq!(m.agent("w").unwrap().queue_pos, None);
        let (rows, queued) = slots(&m);
        assert_eq!(queued, vec!["Task a", "Task b"]);
        assert_eq!(rows.len(), 3);
        assert!(rows[0].is_some() && rows[1].is_none());
        assert_eq!(slots_header(&m), "1 of 3");
    }

    fn files(paths: &[&str]) -> serde_json::Value {
        serde_json::json!({ "files": paths.iter().map(|p| serde_json::json!({"path": p, "added": 3, "removed": 1})).collect::<Vec<_>>() })
    }

    #[test]
    fn conflicts_come_from_touched_files_or_the_file_being_edited() {
        let s = shared(vec![
            task("a", TaskState::Working, &t_ago(900), files(&["src/x.rs", "src/shared.rs"])),
            task("b", TaskState::NeedsYou, &t_ago(800), files(&["src/shared.rs", "src/y.rs"])),
            task("c", TaskState::Working, &t_ago(700), files(&["src/z.rs"])),
            task("d", TaskState::Done, &t_ago(600), files(&["src/shared.rs"])),
        ]);
        let m = from_shared(&s, &Model::default(), now());
        assert_eq!(m.conflicts, vec![Conflict { a: "a".into(), b: "b".into(), file: "src/shared.rs".into() }]);
        // editing wins when it is shared
        let mut agents = m.agents.clone();
        agents[0].editing = Some("src/q.rs".into());
        agents[1].editing = Some("src/q.rs".into());
        assert_eq!(find_conflicts(&agents, true)[0].file, "src/q.rs");
        // demo rule: working only, editing only
        assert_eq!(find_conflicts(&agents, false).len(), 0);
        agents[1].state = State::Working;
        assert_eq!(find_conflicts(&agents, false)[0].file, "src/q.rs");
        // another repo never conflicts
        agents[1].repo = 1;
        assert!(find_conflicts(&agents, true).is_empty());
    }

    #[test]
    fn steps_files_editing_and_the_request_line() {
        let mut s = shared(vec![task("a", TaskState::NeedsYou, &t_ago(100), serde_json::json!({"steps": 3, "files": [{"path": "src/core.rs", "added": 4, "removed": 1}]}))]);
        let mut run = crate::service::Run::default();
        for (i, (kind, title)) in [(StepKind::Read, "src/lib.rs"), (StepKind::Edit, "Editing src/core.rs")].into_iter().enumerate() {
            run.steps.push(crate::service::RunStep { id: i.to_string(), kind, tool: String::new(), title: title.into(), detail: String::new(), ok: None, output: String::new(), at: t_ago(50 - i as i64), seq: i as u64 });
        }
        s.runs.insert("a".into(), run);
        let m = from_shared(&s, &Model::default(), now());
        let a = m.agent("a").unwrap();
        assert_eq!(a.steps.iter().map(|x| x.text.as_str()).collect::<Vec<_>>(), vec!["Editing src/core.rs", "src/lib.rs"]);
        assert_eq!(a.steps[0].at - a.steps[1].at, 1);
        assert_eq!(a.editing.as_deref(), Some("src/core.rs"));
        assert_eq!(a.step_seq, 2);
        assert_eq!(a.files.len(), 1);
        // needs, no request: the status
        assert_eq!(a.step, "status a");
        s.tasks[0].state = TaskState::Working;
        assert_eq!(from_shared(&s, &Model::default(), now()).agent("a").unwrap().step, "Editing src/core.rs");
    }

    #[test]
    fn text_helpers() {
        let m = demo();
        let get = |id: &str| m.agent(id).unwrap();
        assert_eq!(chip(State::Working), "Working");
        assert_eq!(chip(State::Needs), "Needs you");
        assert_eq!(chip(State::Done), "Ready to merge");
        assert_eq!(chip(State::Merged), "Ready to merge");
        assert_eq!(chip(State::Queued), "Queued");
        assert_eq!(chip(State::Stopped), "Stopped");
        assert_eq!(pct(get("rate")), "34%");
        assert_eq!(pct(get("react")), "—");
        assert_eq!(pct(get("json")), "100%");
        assert_eq!(cost(get("rate")), "≈ $0.12");
        assert_eq!(cost(get("cold")), "");
        assert_eq!(fmt_dur(198), "3:18");
        assert_eq!(fmt_dur(5), "0:05");
        assert_eq!(ago(1, true), "now");
        assert_eq!(ago(1, false), "1s");
        assert_eq!(ago(59, true), "59s");
        assert_eq!(ago(60, false), "1m");
        assert_eq!(ago(200, false), "3m");
        let now = DEMO_NOW;
        assert_eq!(tooltip(get("rate"), &m, now), ("Add rate limiting to /login".to_string(), "api-server · Step 4 · editing src/auth.rs · 3:12 running".to_string()));
        assert_eq!(tooltip(get("react"), &m, now).1, "web-app · Queued · starts when a slot frees");
        assert_eq!(tooltip(get("json"), &m, now).1, "sphere-cli · Ready to merge · click to open");
        let mut n = get("rate").clone();
        n.state = State::Needs;
        assert_eq!(tooltip(&n, &m, now).1, "api-server · Needs you · click to answer");
        assert_eq!(ring_label(&m, 0), ("api-server".to_string(), "2 agents".to_string()));
        let mut one = m.clone();
        one.agents.retain(|a| a.id != "react");
        assert_eq!(ring_label(&one, 1), ("web-app".to_string(), "1 agent".to_string()));
        assert_eq!(ready_label(&m), ("Ready to merge".to_string(), "1 ready".to_string()));
        assert_eq!(ready_label(&Model::default()).1, "none yet");
        let sg = strip(&m);
        assert_eq!(sg[0], ("6".to_string(), "agents".to_string(), None));
        assert_eq!(sg[1], ("3".to_string(), "working".to_string(), None));
        assert_eq!(sg[2], ("0".to_string(), "needs you".to_string(), Some(State::Needs)));
        assert_eq!(sg[3], ("1".to_string(), "ready".to_string(), Some(State::Done)));
        assert_eq!(sg[4], ("2".to_string(), "queued".to_string(), None));
        assert_eq!(strip_cost(&m), "≈ $1.84");
        assert_eq!(strip(&one)[0].1, "agents");
        let (rows, q) = slots(&m);
        assert_eq!(rows[0], Some(("Add rate limiting to /login".to_string(), "34%".to_string(), 0.34, false)));
        assert_eq!(rows.len(), 3);
        assert_eq!(q, vec!["Upgrade React to 19", "Speed up cold start"]);
        assert_eq!(slots_header(&m), "3 of 3");
        let o = orbits(&m);
        assert_eq!(o[0], ("api-server".to_string(), "main · 2 agents".to_string(), vec![State::Working, State::Working]));
        assert_eq!(o[2].2, vec![State::Done, State::Queued]);
        assert_eq!(orbits_header(&m), "3 repos");
        assert_eq!(orbits_header(&Model { repos: vec![Repo::default()], ..Default::default() }), "1 repo");
        assert_eq!(focus_steps_label(get("react")), "waiting for a slot");
        assert_eq!(focus_steps_label(get("json")), "4 recent");
        assert_eq!(focus_steps_label(get("rate")), "step 4");
        assert_eq!(diff_line(get("json")), "+101 \u{2212}8 · 3 files");
        assert_eq!(diff_line(get("react")), "no changes yet");
    }

    #[test]
    fn demo_matches_the_spec() {
        let m = demo();
        let ids: Vec<&str> = m.agents.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, vec!["rate", "redis", "checkout", "react", "json", "cold"]);
        assert_eq!(m.repos.iter().map(|r| (r.key.as_str(), r.branch.as_str())).collect::<Vec<_>>(), vec![("api-server", "main"), ("web-app", "main"), ("sphere-cli", "main")]);
        let states: Vec<State> = m.agents.iter().map(|a| a.state).collect();
        assert_eq!(states, vec![State::Working, State::Working, State::Working, State::Queued, State::Done, State::Queued]);
        assert_eq!(m.agents.iter().map(|a| a.repo).collect::<Vec<_>>(), vec![0, 0, 1, 1, 2, 2]);
        assert_eq!(m.agent("checkout").unwrap().p, 0.71);
        assert_eq!(m.agent("redis").unwrap().cost, 0.31);
        assert_eq!(m.agent("rate").unwrap().steps.len(), 4);
        assert_eq!(m.agent("rate").unwrap().steps[3].text, "creating branch feat/login-rate-limit");
        assert_eq!(DEMO_NOW - m.agent("rate").unwrap().steps[3].at, 53);
        assert_eq!(m.agent("rate").unwrap().editing.as_deref(), Some("src/auth.rs"));
        assert_eq!(m.agent("redis").unwrap().editing.as_deref(), Some("src/routes.rs"));
        assert_eq!(m.agent("checkout").unwrap().editing, None);
        assert_eq!(m.agent("json").unwrap().step, "PR #77 · CI green");
        assert_eq!(DEMO_NOW - m.agent("rate").unwrap().started_at, 192);
        assert_eq!(m.agent("react").unwrap().queue_pos, Some(0));
        assert_eq!(m.agent("cold").unwrap().queue_pos, Some(1));
        assert_eq!(m.limit, 3);
        // the conflict is live at the start, like the prototype
        assert_eq!(m.conflicts.len(), 0);
    }

    #[test]
    fn demo_runs_like_the_prototype() {
        assert_eq!(demo_at(0.0), demo());
        let m = demo_at(5.0);
        assert_eq!(m.conflicts, vec![Conflict { a: "rate".into(), b: "redis".into(), file: "src/routes.rs".into() }]);
        assert_eq!(state_of(&m, "checkout"), Some(State::Working));
        let m = demo_at(6.5);
        assert_eq!(state_of(&m, "checkout"), Some(State::Needs));
        assert_eq!(m.agent("checkout").unwrap().step, "waiting to run pnpm test:e2e");
        assert_eq!(demo_at(6.5), m, "deterministic");
        // steps landed and cost grew
        assert!(m.agent("rate").unwrap().step_seq > 4);
        assert!(m.agent("rate").unwrap().cost > 0.12);
        // a minute in: agents finished, the queue started, never more than 3 run
        let late = demo_at(70.0);
        assert!(late.running().len() <= 3);
        assert_eq!(state_of(&late, "rate"), Some(State::Done));
        assert!(matches!(state_of(&late, "react"), Some(State::Working | State::Done)));
        assert!(late.agent("react").unwrap().queue_pos.is_none());
    }

    #[test]
    fn diff_finds_the_events() {
        let prev = demo_at(5.0);
        let next = demo_at(6.5);
        let ev = diff(&prev, &next);
        assert!(ev.contains(&Event::NeedsYou("checkout".to_string())));
        assert!(ev.iter().any(|e| matches!(e, Event::StepLanded(_))));
        // the rate/redis conflict clears once one moves off the file, in some later model
        let mut seen_clear = false;
        let mut p = prev.clone();
        for i in 0..400 {
            let n = demo_at(5.0 + i as f32 * 0.25);
            if diff(&p, &n).iter().any(|e| matches!(e, Event::ConflictCleared(_))) {
                seen_clear = true;
            }
            p = n;
        }
        assert!(seen_clear);
        // queued -> working, working -> done
        let mut a = demo();
        let mut b = demo();
        b.agents[3].state = State::Working;
        b.agents[0].state = State::Done;
        a.agents[2].state = State::Needs;
        let e = diff(&a, &b);
        assert!(e.contains(&Event::Started("react".to_string())));
        assert!(e.contains(&Event::Ready("rate".to_string())));
        assert!(!diff(&Model::default(), &b).iter().any(|e| matches!(e, Event::Ready(_) | Event::Started(_))));
    }

    #[test]
    fn event_strings() {
        let mut m = demo_at(5.0);
        let conf = m.conflicts[0].clone();
        let e = Event::ConflictNew(conf.clone());
        assert_eq!(toast(&e, &m), Some(("Conflict · both editing src/routes.rs".to_string(), ToastKind::Plain)));
        assert_eq!(ticker(&e, &m), Some(("API-SERVER".to_string(), "conflict · add rate limiting to /login and move sessions to redis both editing src/routes.rs".to_string(), true)));
        assert_eq!(say(&e, &m).as_deref(), Some("Heads up: two agents in api-server are both editing src/routes.rs."));
        assert_eq!(toast(&Event::ConflictCleared(conf), &m), Some(("Conflict cleared · src/routes.rs".to_string(), ToastKind::Ok)));
        let r = Event::Ready("json".to_string());
        assert_eq!(toast(&r, &m), Some(("Add --json flag to sphere run · ready to merge".to_string(), ToastKind::Ok)));
        assert_eq!(ticker(&r, &m), Some(("SPHERE-CLI".to_string(), "add --json flag to sphere run · PR #77 · CI green · ready to merge".to_string(), false)));
        assert_eq!(say(&r, &m).as_deref(), Some("Add --json flag to sphere run is ready to merge."));
        let s = Event::Started("react".to_string());
        assert_eq!(toast(&s, &m), Some(("Upgrade React to 19 · started".to_string(), ToastKind::Ok)));
        assert_eq!(ticker(&s, &m).unwrap().1, "upgrade react to 19 · started in a free slot");
        let n = Event::NeedsYou("checkout".to_string());
        assert_eq!(toast(&n, &m), Some(("Fix flaky checkout test · needs you".to_string(), ToastKind::Plain)));
        m.repos[2].branch = "develop".to_string();
        let g = Event::Merged("json".to_string());
        assert_eq!(toast(&g, &m), Some(("Merged into develop".to_string(), ToastKind::Ok)));
        assert_eq!(ticker(&g, &m), Some(("GIT".to_string(), "develop ← add --json flag to sphere run · PR #77 · CI green".to_string(), false)));
        assert_eq!(say(&g, &m).as_deref(), Some("Merged into develop."));
        let st = Event::Stopped("cold".to_string());
        assert_eq!(toast(&st, &m), Some(("Stopped · Speed up cold start · worktree kept".to_string(), ToastKind::Plain)));
        assert_eq!(ticker(&st, &m), Some(("SPHERE-CLI".to_string(), "speed up cold start · stopped, worktree kept".to_string(), true)));
        assert_eq!(toast(&Event::StepLanded("rate".to_string()), &m), None);
        assert_eq!(say(&n, &m), None);
    }
}
