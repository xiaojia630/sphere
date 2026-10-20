//! Protocols: agents on a schedule. A protocol is a line the user wrote (`every weekday 9:00 sphere: update the
//! dependencies`). It lives in the event log like everything else, so it outlives a restart, and while sphered runs a
//! loop looks at the clock every half minute and starts whatever has come due, through the same path as `task.start`.
//! The task it starts carries the protocol's id and waits for review like any other.
//!
//! A run is left out, with the reason saved on the protocol, when the last task it started is still going, when the
//! daily cap is reached, or when it was missed by more than twelve hours (the computer slept or sphered was down). A
//! run missed by less still happens, once, as soon as it is noticed. A one-shot turns itself off after it fires.

use crate::manager::{api_err, ApiError, ApiResult, Manager};
use crate::workspace;
use anyhow::anyhow;
use sphere_proto::api::{self, kind};
use sphere_proto::protocol::{self, LastRun, Protocol};
use sphere_proto::{EventBody, TaskState};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How often the clock is looked at. A run starts at most this long after it is due.
pub const TICK: Duration = Duration::from_secs(30);

/// A run missed by longer than this is dropped, not run late.
const CATCH_UP_SECS: i64 = 12 * 3600;

/// The time, as protocols read it.
pub trait Clock: Send + Sync {
    /// Unix seconds.
    fn now(&self) -> i64;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64)
    }
}

/// A protocol starting a task.
pub(crate) struct ProtocolRun {
    /// The protocol as it is to be saved once the task exists: its next run already moved on.
    pub save: Protocol,
    /// When the run happened.
    pub at: i64,
}

impl ProtocolRun {
    /// The event that saves the protocol with this run as its last, in the same step as the task's creation.
    pub(crate) fn saved_after_starting(self, task: &str) -> EventBody {
        let mut protocol = self.save;
        protocol.last_run = Some(LastRun {
            at: self.at,
            task: Some(task.to_string()),
            skipped: None,
            error: None,
        });
        EventBody::ProtocolSaved { protocol }
    }
}

/// The folder a repo in a protocol line means, as in the command bar: `~/x` is in the home folder, `/x` is a path,
/// and any other name is a folder in `~/Projects`.
fn repo_path(token: &str, home: &Path) -> PathBuf {
    if token == "~" {
        home.to_path_buf()
    } else if let Some(rest) = token.strip_prefix("~/") {
        home.join(rest)
    } else if token.starts_with('/') {
        PathBuf::from(token)
    } else {
        home.join("Projects").join(token)
    }
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// The first line of an error, short enough to show beside a protocol.
fn brief(message: &str) -> String {
    let line = message.lines().next().unwrap_or("").trim();
    match line.char_indices().nth(120) {
        Some((i, _)) => format!("{}…", &line[..i]),
        None => line.to_string(),
    }
}

impl Manager {
    /// Start the loop that runs protocols as they come due. It looks at once, so what came due while sphered was
    /// down runs as soon as it is back.
    pub fn start_protocols(self: &Arc<Self>, tick: Duration) {
        let this = self.clone();
        let task = tokio::spawn(async move {
            let mut every = tokio::time::interval(tick);
            // After a sleep one look is enough: the clock, not the count of ticks, says what is due.
            every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                every.tick().await;
                this.protocols_tick().await;
            }
        });
        *self.scheduler.lock().unwrap() = Some(task.abort_handle());
    }

    /// One look at every protocol: start the ones that are due, and keep the others' next run right.
    pub async fn protocols_tick(self: &Arc<Self>) {
        let _held = self.protocol_lock.lock().await;
        let now = self.clock.now();
        let all: Vec<Protocol> = self
            .hub
            .view(|v| v.protocols().into_iter().cloned().collect());
        for p in all.into_iter().filter(|p| p.enabled) {
            let id = p.id.clone();
            if let Err(e) = self.protocol_step(p, now).await {
                tracing::warn!(protocol = %id, "could not run: {}", e.0);
            }
        }
    }

    async fn protocol_step(self: &Arc<Self>, p: Protocol, now: i64) -> ApiResult<()> {
        let zone = &*self.zone;
        let Some(due) = p.next_run else {
            // On with nothing scheduled: only an edited log gets here. Schedule it.
            let next_run = p.schedule.next_after(now, zone);
            return self.save_protocol(Protocol { next_run, ..p });
        };
        if due > now {
            // Not yet. If the time zone changed since the time was worked out, the wall-clock time it meant has moved.
            let fresh = p.schedule.next_after(now, zone);
            if fresh != Some(due) {
                return self.save_protocol(Protocol {
                    next_run: fresh,
                    ..p
                });
            }
            return Ok(());
        }
        // A one-shot has no next run left, which turns it off.
        let next_run = p.schedule.next_after(now, zone);
        let mut after = Protocol {
            enabled: next_run.is_some(),
            next_run,
            ..p.clone()
        };
        let left_out = |why: &str| LastRun {
            at: now,
            task: None,
            skipped: Some(why.to_string()),
            error: None,
        };
        if now - due > CATCH_UP_SECS {
            after.last_run = Some(left_out("missed while off"));
            return self.save_protocol(after);
        }
        if self.protocol_task_going(&p.id).is_some() {
            after.last_run = Some(left_out("still running"));
            return self.save_protocol(after);
        }
        match self.start_protocol_task(&after, now).await {
            Ok(_) => Ok(()),
            Err(ApiError(e)) => {
                after.last_run = Some(if e.kind() == Some(kind::DAILY_CAP) {
                    left_out("daily cap")
                } else {
                    LastRun {
                        at: now,
                        task: None,
                        skipped: None,
                        error: Some(brief(&e.message)),
                    }
                });
                self.save_protocol(after)
            }
        }
    }

    /// The task the protocol started that is still going (queued, working or waiting on you), if any.
    fn protocol_task_going(&self, id: &str) -> Option<String> {
        self.hub.view(|v| {
            v.tasks()
                .into_iter()
                .find(|t| {
                    t.protocol.as_deref() == Some(id)
                        && matches!(
                            t.state,
                            TaskState::Queued | TaskState::Working | TaskState::NeedsYou
                        )
                })
                .map(|t| t.id.clone())
        })
    }

    /// Start the protocol's task like `task.start` does, and save `save` with it.
    async fn start_protocol_task(
        self: &Arc<Self>,
        save: &Protocol,
        now: i64,
    ) -> ApiResult<api::StartResult> {
        let params = api::StartParams {
            repo: save.repo.clone(),
            prompt: save.prompt.clone(),
            title: Some(save.name()),
            ..api::StartParams::default()
        };
        let run = ProtocolRun {
            save: save.clone(),
            at: now,
        };
        self.start_task_for(params, None, vec![], Some(run)).await
    }

    fn save_protocol(&self, protocol: Protocol) -> ApiResult<()> {
        self.hub
            .publish("", EventBody::ProtocolSaved { protocol })
            .map(drop)
            .map_err(ApiError::from)
    }

    fn protocol_get(&self, id: &str) -> ApiResult<Protocol> {
        self.hub
            .view(|v| v.protocol(id).cloned())
            .ok_or_else(|| api_err(kind::NOT_FOUND, format!("no such protocol: {id}")))
    }

    // ---- API ---------------------------------------------------------------------------------

    pub fn protocol_list(&self) -> api::ProtocolList {
        api::ProtocolList {
            protocols: self
                .hub
                .view(|v| v.protocols().into_iter().cloned().collect()),
        }
    }

    /// Write a protocol from the line the user typed.
    pub async fn protocol_add(self: &Arc<Self>, p: api::ProtocolAddParams) -> ApiResult<Protocol> {
        let _held = self.protocol_lock.lock().await;
        let now = self.clock.now();
        let parsed = protocol::parse(&p.text, now, &*self.zone)
            .map_err(|e| api_err(kind::BAD_SCHEDULE, e.0))?;
        let repo = self
            .protocol_repo(parsed.repo.as_deref(), p.repo.as_deref())
            .await?;
        let made = Protocol {
            id: self.hub.new_protocol_id(),
            text: p.text.trim().to_string(),
            next_run: parsed.schedule.next_after(now, &*self.zone),
            schedule: parsed.schedule,
            repo,
            prompt: parsed.prompt,
            enabled: true,
            created_at: now,
            last_run: None,
        };
        self.save_protocol(made.clone())?;
        Ok(made)
    }

    /// The top folder of the repo a protocol works in: the one the line names, else `default`.
    async fn protocol_repo(&self, named: Option<&str>, default: Option<&str>) -> ApiResult<String> {
        let path =
            match (named, default) {
                (Some(token), _) => repo_path(token, &home()),
                (None, Some(folder)) => PathBuf::from(folder),
                (None, None) => return Err(api_err(
                    kind::BAD_REPO,
                    "which repo? name it before the colon: 'every weekday 9:00 sphere: what to do'",
                )),
            };
        let looked = path.clone();
        let top = tokio::task::spawn_blocking(move || workspace::repo_toplevel(&looked))
            .await
            .map_err(|e| anyhow!(e))?
            .map_err(|_| api_err(kind::BAD_REPO, format!("no git repo at {}", path.display())))?;
        Ok(top.to_string_lossy().to_string())
    }

    pub async fn protocol_set_enabled(
        self: &Arc<Self>,
        id: &str,
        enabled: bool,
    ) -> ApiResult<Protocol> {
        let _held = self.protocol_lock.lock().await;
        let mut p = self.protocol_get(id)?;
        if p.enabled == enabled {
            return Ok(p);
        }
        p.enabled = enabled;
        p.next_run = None;
        if enabled {
            p.next_run = p.schedule.next_after(self.clock.now(), &*self.zone);
            if p.next_run.is_none() {
                return Err(api_err(
                    kind::INVALID_STATE,
                    "its time has passed; write a new one",
                ));
            }
        }
        self.save_protocol(p.clone())?;
        Ok(p)
    }

    pub async fn protocol_remove(self: &Arc<Self>, id: &str) -> ApiResult<()> {
        let _held = self.protocol_lock.lock().await;
        self.protocol_get(id)?;
        self.hub
            .publish("", EventBody::ProtocolRemoved { id: id.to_string() })
            .map(drop)
            .map_err(ApiError::from)
    }

    /// Run it now, in addition to its schedule, which stays as it is.
    pub async fn protocol_run_now(self: &Arc<Self>, id: &str) -> ApiResult<api::StartResult> {
        let _held = self.protocol_lock.lock().await;
        let p = self.protocol_get(id)?;
        if let Some(task) = self.protocol_task_going(id) {
            return Err(api_err(
                kind::INVALID_STATE,
                format!("still running ({task}); wait for it or stop it first"),
            ));
        }
        self.start_protocol_task(&p, self.clock.now()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::hub::Hub;
    use crate::store::Store;
    use sphere_proto::protocol::{Outcome, Zone};
    use sphere_proto::EventBody::{Failed, Turn};
    use std::sync::atomic::{AtomicI64, Ordering};

    /// 2026-03-04 12:00 UTC, a Wednesday.
    const START: i64 = 1_772_625_600;
    const HOUR: i64 = 3600;
    const DAY: i64 = 24 * HOUR;

    /// A clock that stands still until a test moves it.
    struct Manual(AtomicI64);

    impl Clock for Manual {
        fn now(&self) -> i64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    /// A time zone a test can move.
    struct Moving(AtomicI64);

    impl Zone for Moving {
        fn offset(&self, _utc: i64) -> i64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    struct Rig {
        mgr: Arc<Manager>,
        clock: Arc<Manual>,
        zone: Arc<Moving>,
        /// A git repo to work in.
        repo: tempfile::TempDir,
    }

    impl Rig {
        fn new() -> Rig {
            Self::with(
                None,
                Hub::with_store(Store::open_in_memory().unwrap(), 16).unwrap(),
            )
        }

        fn with(daily_cap_usd: Option<f64>, hub: Hub) -> Rig {
            let cfg = Config {
                daily_cap_usd,
                // Tasks stay queued: no agent is started.
                max_concurrent: 0,
                ..Config::default()
            };
            let clock = Arc::new(Manual(AtomicI64::new(START)));
            let zone = Arc::new(Moving(AtomicI64::new(0)));
            let mgr = Manager::with_time(Arc::new(cfg), Arc::new(hub), clock.clone(), zone.clone());
            let repo = tempfile::tempdir().unwrap();
            let git = std::process::Command::new("git")
                .args(["init", "-q"])
                .arg(repo.path())
                .status()
                .unwrap();
            assert!(git.success());
            Rig {
                mgr,
                clock,
                zone,
                repo,
            }
        }

        fn at(&self, t: i64) {
            self.clock.0.store(t, Ordering::SeqCst);
        }

        fn line(&self, schedule: &str) -> String {
            format!(
                "{schedule} {}: update the dependencies, run the tests",
                self.repo.path().display()
            )
        }

        async fn add(&self, schedule: &str) -> Protocol {
            let text = self.line(schedule);
            self.mgr
                .protocol_add(api::ProtocolAddParams { text, repo: None })
                .await
                .unwrap()
        }

        fn protocol(&self, id: &str) -> Protocol {
            self.mgr.hub.view(|v| v.protocol(id).cloned()).unwrap()
        }

        fn tasks(&self) -> Vec<sphere_proto::TaskView> {
            self.mgr
                .hub
                .view(|v| v.tasks().into_iter().cloned().collect())
        }

        fn finish(&self, task: &str, ok: bool) {
            let body = if ok {
                Turn {
                    ok: true,
                    interrupted: false,
                    summary: "all done".into(),
                    turns: Some(1),
                    duration_ms: None,
                    cost_usd: None,
                    cost_delta_usd: 0.0,
                }
            } else {
                Failed {
                    message: "it broke".into(),
                }
            };
            self.mgr.hub.publish(task, body).unwrap();
        }

        /// Spend today's whole cap of 0.5 dollars on a task that is not a protocol's.
        fn spend_the_cap(&self) {
            let made = EventBody::TaskCreated {
                title: "x".into(),
                prompt: "x".into(),
                repo: "/r".into(),
                in_place: false,
                budget_usd: 1.0,
                mode: None,
                model: None,
                allow_rewrites: false,
                resume_session: None,
                protocol: None,
            };
            let spent = Turn {
                ok: true,
                interrupted: false,
                summary: String::new(),
                turns: Some(1),
                duration_ms: None,
                cost_usd: Some(0.5),
                cost_delta_usd: 0.5,
            };
            self.mgr.hub.publish("t9", made).unwrap();
            self.mgr.hub.publish("t9", spent).unwrap();
        }
    }

    fn kind_of<T>(r: ApiResult<T>) -> String {
        match r {
            Ok(_) => panic!("expected an error"),
            Err(ApiError(e)) => e.kind().unwrap_or("none").to_string(),
        }
    }

    #[test]
    fn a_repo_in_a_line_is_found_like_the_command_bar_does() {
        let home = Path::new("/home/me");
        assert_eq!(
            repo_path("sphere", home),
            Path::new("/home/me/Projects/sphere")
        );
        assert_eq!(repo_path("~/code/x", home), Path::new("/home/me/code/x"));
        assert_eq!(repo_path("~", home), Path::new("/home/me"));
        assert_eq!(repo_path("/srv/x", home), Path::new("/srv/x"));
    }

    #[test]
    fn errors_are_cut_to_one_short_line() {
        assert_eq!(brief("short"), "short");
        assert_eq!(brief("first\nsecond"), "first");
        let long = "é".repeat(300);
        assert_eq!(brief(&long).chars().count(), 121);
        assert!(brief(&long).ends_with('…'));
    }

    #[tokio::test]
    async fn adding_a_protocol_saves_it_and_works_out_its_next_run() {
        let rig = Rig::new();
        let mut heard = rig.mgr.hub.subscribe(None);
        let p = rig.add("every weekday 9:00").await;
        assert_eq!(p.id, "p1");
        assert!(p.enabled && p.last_run.is_none());
        assert_eq!(p.created_at, START);
        assert_eq!(p.prompt, "update the dependencies, run the tests");
        assert_eq!(p.name(), "Update the dependencies");
        assert_eq!(Path::new(&p.repo), rig.repo.path().canonicalize().unwrap());
        // Thursday 09:00 UTC.
        assert_eq!(p.next_run, Some(START + 21 * HOUR));
        assert_eq!(rig.protocol("p1"), p);
        assert_eq!(rig.mgr.protocol_list().protocols, vec![p.clone()]);
        assert_eq!(rig.mgr.snapshot().protocols, vec![p.clone()]);
        // Clients hear of it at once, as an event of no task.
        match heard.rx.try_recv().unwrap() {
            crate::hub::Msg::Event(e) => {
                assert_eq!(e.task, "");
                assert_eq!(e.body, EventBody::ProtocolSaved { protocol: p });
            }
            other => panic!("{other:?}"),
        }
        assert!(rig.tasks().is_empty());
    }

    #[tokio::test]
    async fn a_line_that_is_not_a_schedule_is_refused_in_words() {
        let rig = Rig::new();
        let text = format!("evry weekday 9:00 {}: x", rig.repo.path().display());
        let e = rig
            .mgr
            .protocol_add(api::ProtocolAddParams { text, repo: None })
            .await
            .unwrap_err()
            .0;
        assert_eq!(e.kind(), Some(kind::BAD_SCHEDULE));
        assert_eq!(
            e.message,
            "didn't understand 'evry': try 'every weekday 9:00 repo: what to do'"
        );
        assert!(rig.mgr.protocol_list().protocols.is_empty());
    }

    #[tokio::test]
    async fn the_repo_is_the_one_named_else_the_default_else_an_error() {
        let rig = Rig::new();
        let add = |text: String, repo: Option<String>| {
            let mgr = rig.mgr.clone();
            async move {
                mgr.protocol_add(api::ProtocolAddParams { text, repo })
                    .await
            }
        };
        let here = rig.repo.path().canonicalize().unwrap();
        // Named only by the default.
        let p = add("every day 9:00: x".into(), Some(here.display().to_string()))
            .await
            .unwrap();
        assert_eq!(Path::new(&p.repo), here);
        // A subfolder of a repo is the repo.
        let sub = here.join("src");
        std::fs::create_dir_all(&sub).unwrap();
        let p = add("every day 9:00: x".into(), Some(sub.display().to_string()))
            .await
            .unwrap();
        assert_eq!(Path::new(&p.repo), here);
        // Neither.
        let e = add("every day 9:00: x".into(), None).await.unwrap_err().0;
        assert_eq!(e.kind(), Some(kind::BAD_REPO));
        assert!(e.message.starts_with("which repo?"), "{}", e.message);
        // Not a repo.
        let plain = tempfile::tempdir().unwrap();
        let line = format!("every day 9:00 {}: x", plain.path().display());
        let e = add(line, None).await.unwrap_err().0;
        assert_eq!(e.kind(), Some(kind::BAD_REPO));
        assert!(e.message.starts_with("no git repo at "), "{}", e.message);
        assert_eq!(rig.mgr.protocol_list().protocols.len(), 2);
    }

    #[tokio::test]
    async fn a_protocol_that_is_not_due_starts_nothing() {
        let rig = Rig::new();
        rig.add("every weekday 9:00").await;
        rig.mgr.protocols_tick().await;
        rig.at(START + 20 * HOUR);
        rig.mgr.protocols_tick().await;
        assert!(rig.tasks().is_empty());
        assert_eq!(rig.protocol("p1").last_run, None);
    }

    #[tokio::test]
    async fn a_due_protocol_starts_a_task_the_way_task_start_does() {
        let rig = Rig::new();
        let p = rig.add("every weekday 9:00").await;
        let mut heard = rig.mgr.hub.subscribe(None);
        let due = p.next_run.unwrap();
        rig.at(due + 5);
        rig.mgr.protocols_tick().await;

        let tasks = rig.tasks();
        assert_eq!(tasks.len(), 1);
        let t = &tasks[0];
        assert_eq!(t.id, "t1");
        assert_eq!(t.protocol.as_deref(), Some("p1"));
        assert_eq!(t.title, "Update the dependencies");
        assert_eq!(t.prompt, "update the dependencies, run the tests");
        assert_eq!(t.repo, p.repo);
        assert_eq!(t.state, TaskState::Queued);
        assert_eq!(t.budget_usd, Config::default().default_budget_usd);

        // The protocol knows the task and moved on, in the same step as the task's creation.
        let now = rig.protocol("p1");
        assert_eq!(now.next_run, Some(due + DAY));
        assert!(now.enabled);
        let last = now.last_run.unwrap();
        assert_eq!((last.at, last.task.as_deref()), (due + 5, Some("t1")));
        assert!(last.skipped.is_none() && last.error.is_none());
        let mut seen = vec![];
        while let Ok(crate::hub::Msg::Event(e)) = heard.rx.try_recv() {
            seen.push((e.seq, e.task, e.body.kind_name()));
        }
        assert_eq!(
            seen,
            vec![
                (2, "t1".to_string(), "task_created"),
                (3, String::new(), "protocol_saved")
            ]
        );
    }

    #[tokio::test]
    async fn a_run_waits_for_the_last_task_the_protocol_started() {
        let rig = Rig::new();
        let p = rig.add("every day 9:00").await;
        let first = p.next_run.unwrap();
        rig.at(first);
        rig.mgr.protocols_tick().await;
        assert_eq!(rig.tasks().len(), 1);

        // A day later the first task is still queued (no agent runs here): the run is left out, and says why.
        rig.at(first + DAY);
        rig.mgr.protocols_tick().await;
        assert_eq!(rig.tasks().len(), 1, "no second task");
        let now = rig.protocol("p1");
        let last = now.last_run.clone().unwrap();
        assert_eq!(last.skipped.as_deref(), Some("still running"));
        assert_eq!((last.task, last.at), (None, first + DAY));
        assert_eq!(now.next_run, Some(first + 2 * DAY));
        assert_eq!(
            now.outcome(|_| None),
            Outcome::Skipped("still running".into())
        );

        // Working counts as running.
        let started = EventBody::AgentStarted {
            child: "c1".into(),
            pid: 1,
            resumed: false,
        };
        rig.mgr.hub.publish("t1", started).unwrap();
        rig.at(first + 2 * DAY);
        rig.mgr.protocols_tick().await;
        assert_eq!(rig.tasks().len(), 1);

        // Once it is done the next run starts a new one.
        rig.finish("t1", true);
        rig.at(first + 3 * DAY);
        rig.mgr.protocols_tick().await;
        let tasks = rig.tasks();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[1].protocol.as_deref(), Some("p1"));
        let last = rig.protocol("p1").last_run.unwrap();
        assert_eq!((last.task.as_deref(), last.skipped), (Some("t2"), None));
    }

    #[tokio::test]
    async fn a_task_waiting_on_you_blocks_the_next_run_too() {
        let rig = Rig::new();
        let first = rig.add("every day 9:00").await.next_run.unwrap();
        rig.at(first);
        rig.mgr.protocols_tick().await;
        let t = rig.tasks()[0].id.clone();
        let started = EventBody::AgentStarted {
            child: "c1".into(),
            pid: 1,
            resumed: false,
        };
        rig.mgr.hub.publish(&t, started).unwrap();
        let ask = sphere_proto::RequestInfo {
            id: "r1".into(),
            task: t.clone(),
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
        rig.mgr
            .hub
            .publish(&t, EventBody::Request { request: ask })
            .unwrap();
        assert_eq!(rig.tasks()[0].state, TaskState::NeedsYou);
        rig.at(first + DAY);
        rig.mgr.protocols_tick().await;
        assert_eq!(rig.tasks().len(), 1);
        assert_eq!(
            rig.protocol("p1").last_run.unwrap().skipped.as_deref(),
            Some("still running")
        );
    }

    #[tokio::test]
    async fn a_run_missed_by_less_than_twelve_hours_still_happens_once() {
        let rig = Rig::new();
        let due = rig.add("every day 9:00").await.next_run.unwrap();
        // The Mac slept through 09:00 and woke at 20:00.
        rig.at(due + 11 * HOUR);
        rig.mgr.protocols_tick().await;
        assert_eq!(rig.tasks().len(), 1);
        // And the next one is tomorrow's, not another catch-up.
        assert_eq!(rig.protocol("p1").next_run, Some(due + DAY));
        rig.mgr.protocols_tick().await;
        assert_eq!(rig.tasks().len(), 1);
    }

    #[tokio::test]
    async fn an_older_miss_is_dropped_and_the_next_run_scheduled() {
        let rig = Rig::new();
        let due = rig.add("every day 9:00").await.next_run.unwrap();
        // Back from a long weekend: three 09:00s have passed. None of them runs.
        rig.at(due + 3 * DAY + 2 * HOUR);
        rig.mgr.protocols_tick().await;
        assert!(rig.tasks().is_empty());
        let p = rig.protocol("p1");
        assert_eq!(
            p.last_run.unwrap().skipped.as_deref(),
            Some("missed while off")
        );
        assert_eq!(p.next_run, Some(due + 4 * DAY));
        assert!(p.enabled);
        // Just past twelve hours is dropped as well; just under is not.
        let rig = Rig::new();
        let due = rig.add("every day 9:00").await.next_run.unwrap();
        rig.at(due + 12 * HOUR);
        rig.mgr.protocols_tick().await;
        assert_eq!(rig.tasks().len(), 1);
        let rig = Rig::new();
        let due = rig.add("every day 9:00").await.next_run.unwrap();
        rig.at(due + 12 * HOUR + 1);
        rig.mgr.protocols_tick().await;
        assert!(rig.tasks().is_empty());
    }

    #[tokio::test]
    async fn a_one_shot_turns_itself_off_after_it_fires() {
        let rig = Rig::new();
        let p = rig.add("in 30 minutes").await;
        assert_eq!(p.next_run, Some(START + 1800));
        rig.at(START + 1799);
        rig.mgr.protocols_tick().await;
        assert!(rig.tasks().is_empty());
        rig.at(START + 1830);
        rig.mgr.protocols_tick().await;
        assert_eq!(rig.tasks().len(), 1);
        let p = rig.protocol("p1");
        assert!(!p.enabled && p.next_run.is_none());
        assert_eq!(p.last_run.unwrap().task.as_deref(), Some("t1"));
        // It is still there to look at, and does not fire again.
        rig.at(START + 5 * DAY);
        rig.mgr.protocols_tick().await;
        assert_eq!(rig.tasks().len(), 1);
        // And its time has passed, so it cannot be turned back on.
        let e = rig.mgr.protocol_set_enabled("p1", true).await;
        assert_eq!(kind_of(e), kind::INVALID_STATE);
    }

    #[tokio::test]
    async fn a_one_shot_missed_by_half_a_day_is_dropped_and_off() {
        let rig = Rig::new();
        rig.add("tomorrow 9:00").await;
        rig.at(START + 3 * DAY);
        rig.mgr.protocols_tick().await;
        assert!(rig.tasks().is_empty());
        let p = rig.protocol("p1");
        assert!(!p.enabled && p.next_run.is_none());
        assert_eq!(
            p.last_run.unwrap().skipped.as_deref(),
            Some("missed while off")
        );
    }

    #[tokio::test]
    async fn a_run_is_left_out_while_the_daily_cap_is_reached() {
        let hub = Hub::with_store(Store::open_in_memory().unwrap(), 16).unwrap();
        let rig = Rig::with(Some(0.5), hub);
        let due = rig.add("every day 9:00").await.next_run.unwrap();
        // The cap counts what was spent on the real day, whatever the protocols' clock says.
        rig.spend_the_cap();
        rig.at(due);
        rig.mgr.protocols_tick().await;
        assert_eq!(rig.tasks().len(), 1, "only the task that spent the money");
        let p = rig.protocol("p1");
        assert_eq!(p.last_run.unwrap().skipped.as_deref(), Some("daily cap"));
        assert_eq!(p.next_run, Some(due + DAY), "tomorrow's run is still on");
    }

    #[tokio::test]
    async fn a_run_that_cannot_start_is_recorded_and_not_tried_every_half_minute() {
        let rig = Rig::new();
        let due = rig.add("every day 9:00").await.next_run.unwrap();
        // The repo is gone.
        let gone = rig.protocol("p1").repo;
        std::fs::remove_dir_all(&gone).unwrap();
        rig.at(due);
        rig.mgr.protocols_tick().await;
        assert!(rig.tasks().is_empty());
        let p = rig.protocol("p1");
        let last = p.last_run.clone().unwrap();
        assert!(last.error.is_some() && last.skipped.is_none() && last.task.is_none());
        assert!(matches!(p.outcome(|_| None), Outcome::Error(_)));
        assert_eq!(p.next_run, Some(due + DAY));
        assert!(p.enabled);
    }

    #[tokio::test]
    async fn an_interval_keeps_its_beat_whatever_the_ticks() {
        let rig = Rig::new();
        let p = rig.add("every 30 minutes").await;
        assert_eq!(p.next_run, Some(START + 1800));
        // Ticks every 30 seconds: the first one past the mark starts the run.
        rig.at(START + 1800 + 20);
        rig.mgr.protocols_tick().await;
        assert_eq!(rig.tasks().len(), 1);
        assert_eq!(rig.protocol("p1").next_run, Some(START + 3600));
        rig.finish("t1", true);
        // Woken after three hours: one catch-up run, and the beat goes on from where it was.
        rig.at(START + 3 * HOUR + 400);
        rig.mgr.protocols_tick().await;
        assert_eq!(rig.tasks().len(), 2);
        assert_eq!(rig.protocol("p1").next_run, Some(START + 3 * HOUR + 1800));
    }

    #[tokio::test]
    async fn an_off_protocol_runs_nothing_and_comes_back_with_a_fresh_next_run() {
        let rig = Rig::new();
        let due = rig.add("every day 9:00").await.next_run.unwrap();
        let p = rig.mgr.protocol_set_enabled("p1", false).await.unwrap();
        assert!(!p.enabled && p.next_run.is_none());
        assert_eq!(rig.protocol("p1"), p);
        rig.at(due + 2 * HOUR);
        rig.mgr.protocols_tick().await;
        assert!(rig.tasks().is_empty());
        // Turned on again two days later, it waits for the next 09:00 rather than running at once.
        rig.at(due + 2 * DAY + 2 * HOUR);
        let p = rig.mgr.protocol_set_enabled("p1", true).await.unwrap();
        assert!(p.enabled);
        assert_eq!(p.next_run, Some(due + 3 * DAY));
        rig.mgr.protocols_tick().await;
        assert!(rig.tasks().is_empty());
        // Setting it to what it is changes nothing.
        let seq = rig.mgr.hub.last_seq();
        rig.mgr.protocol_set_enabled("p1", true).await.unwrap();
        assert_eq!(rig.mgr.hub.last_seq(), seq);
        assert_eq!(
            kind_of(rig.mgr.protocol_set_enabled("p9", true).await),
            kind::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn a_removed_protocol_is_forgotten_and_its_tasks_stay() {
        let rig = Rig::new();
        let due = rig.add("every day 9:00").await.next_run.unwrap();
        rig.at(due);
        rig.mgr.protocols_tick().await;
        rig.mgr.protocol_remove("p1").await.unwrap();
        assert!(rig.mgr.protocol_list().protocols.is_empty());
        assert!(rig.mgr.snapshot().protocols.is_empty());
        assert_eq!(rig.tasks().len(), 1);
        assert_eq!(rig.tasks()[0].protocol.as_deref(), Some("p1"));
        rig.at(due + DAY);
        rig.mgr.protocols_tick().await;
        assert_eq!(rig.tasks().len(), 1);
        assert_eq!(
            kind_of(rig.mgr.protocol_remove("p1").await),
            kind::NOT_FOUND
        );
        // The number is not given out again.
        assert_eq!(rig.add("every day 9:00").await.id, "p2");
    }

    #[tokio::test]
    async fn running_one_now_leaves_its_schedule_alone() {
        let rig = Rig::new();
        let p = rig.add("every weekday 9:00").await;
        rig.at(START + HOUR);
        let started = rig.mgr.protocol_run_now("p1").await.unwrap();
        assert_eq!(started.task, "t1");
        let task = &rig.tasks()[0];
        assert_eq!(task.protocol.as_deref(), Some("p1"));
        let now = rig.protocol("p1");
        assert_eq!(now.next_run, p.next_run);
        assert!(now.enabled);
        let last = now.last_run.unwrap();
        assert_eq!((last.at, last.task.as_deref()), (START + HOUR, Some("t1")));
        // Not again while it goes; then fine.
        let e = rig.mgr.protocol_run_now("p1").await.unwrap_err().0;
        assert_eq!(e.kind(), Some(kind::INVALID_STATE));
        assert!(e.message.starts_with("still running (t1)"), "{}", e.message);
        rig.finish("t1", false);
        assert_eq!(rig.mgr.protocol_run_now("p1").await.unwrap().task, "t2");
        assert_eq!(
            kind_of(rig.mgr.protocol_run_now("p9").await),
            kind::NOT_FOUND
        );
        // Even when it is off.
        rig.finish("t2", true);
        rig.mgr.protocol_set_enabled("p1", false).await.unwrap();
        assert_eq!(rig.mgr.protocol_run_now("p1").await.unwrap().task, "t3");
        assert!(!rig.protocol("p1").enabled);
    }

    #[tokio::test]
    async fn running_one_now_obeys_the_daily_cap() {
        let hub = Hub::with_store(Store::open_in_memory().unwrap(), 16).unwrap();
        let rig = Rig::with(Some(0.5), hub);
        rig.add("every day 9:00").await;
        rig.spend_the_cap();
        let e = rig.mgr.protocol_run_now("p1").await;
        assert_eq!(kind_of(e), kind::DAILY_CAP);
    }

    #[tokio::test]
    async fn protocols_and_their_runs_survive_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("sphere.db");
        let first = Rig::with(None, Hub::open(&db, 16).unwrap());
        let p = first.add("every day 9:00").await;
        let due = p.next_run.unwrap();
        first.at(due);
        first.mgr.protocols_tick().await;
        assert_eq!(first.tasks().len(), 1);
        // sphered stops; the repo stays where it is.
        let Rig {
            mgr, repo: _repo, ..
        } = first;
        drop(mgr);

        let rig = Rig::with(None, Hub::open(&db, 16).unwrap());
        let now = rig.protocol("p1");
        assert_eq!(now.repo, p.repo);
        assert_eq!(now.next_run, Some(due + DAY));
        assert_eq!(now.last_run.unwrap().task.as_deref(), Some("t1"));
        assert_eq!(rig.tasks()[0].protocol.as_deref(), Some("p1"));
        // The run that was done is not done again, however soon the scheduler looks.
        rig.at(due + 60);
        rig.mgr.protocols_tick().await;
        assert_eq!(rig.tasks().len(), 1);
        // And what came due while it was down runs when it is back.
        rig.at(due + DAY + HOUR);
        rig.finish("t1", true);
        rig.mgr.protocols_tick().await;
        assert_eq!(rig.tasks().len(), 2);
        // New numbers carry on from the old.
        assert_eq!(rig.add("every day 10:00").await.id, "p2");
    }

    #[tokio::test]
    async fn the_next_run_follows_the_clock_when_the_time_zone_changes() {
        let rig = Rig::new();
        let p = rig.add("every day 9:00").await;
        // UTC: tomorrow 09:00.
        assert_eq!(p.next_run, Some(START + 21 * HOUR));
        // The laptop lands five hours behind: it is 07:00 there, and 09:00 is two hours away.
        rig.zone.0.store(-5 * HOUR, Ordering::SeqCst);
        rig.mgr.protocols_tick().await;
        assert_eq!(rig.protocol("p1").next_run, Some(START + 2 * HOUR));
        assert!(rig.tasks().is_empty(), "moved, not run");
        // Looking again changes nothing, and writes nothing.
        let seq = rig.mgr.hub.last_seq();
        rig.mgr.protocols_tick().await;
        assert_eq!(rig.mgr.hub.last_seq(), seq);
    }

    #[tokio::test]
    async fn the_scheduler_loop_runs_what_is_due_and_stops_with_the_manager() {
        let rig = Rig::new();
        let due = rig.add("every day 9:00").await.next_run.unwrap();
        rig.at(due);
        rig.mgr.start_protocols(Duration::from_millis(10));
        for _ in 0..200 {
            if !rig.tasks().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(rig.tasks().len(), 1, "the first look came at once");
        let handle = rig.mgr.scheduler.lock().unwrap().clone().unwrap();
        rig.mgr.abort_all();
        for _ in 0..200 {
            if handle.is_finished() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(handle.is_finished());
    }
}
