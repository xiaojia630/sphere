//! Everything a client shows is computed from the event log by `Views::apply`.
//! sphered rebuilds its views from the store with it; the CLI and the app run
//! the same code over a snapshot plus the events that follow.

use crate::api::Snapshot;
use crate::event::{Event, EventBody, FileStat, PlanItem, TaskState};
use crate::request::{AnswerKind, RequestInfo, RequestStatus};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// A tool call that has started and has no result yet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpenStep {
    pub id: String,
    pub tool: String,
    pub title: String,
    #[serde(default)]
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskView {
    pub id: String,
    pub title: String,
    pub prompt: String,
    pub state: TaskState,
    /// One line, kept up to date by the event handlers with no model call.
    pub status: String,
    pub repo: String,
    #[serde(default)]
    pub workdir: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub base: Option<String>,
    pub in_place: bool,
    pub budget_usd: f64,
    pub cost_usd: f64,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    /// The host child running the agent, when there is one.
    #[serde(default)]
    pub child: Option<String>,
    #[serde(default)]
    pub pid: Option<u32>,
    pub started_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub files: Vec<FileStat>,
    #[serde(default)]
    pub plan: Vec<PlanItem>,
    #[serde(default)]
    pub open_steps: Vec<OpenStep>,
    #[serde(default)]
    pub steps: u32,
    #[serde(default)]
    pub last_say: String,
    #[serde(default)]
    pub slash_commands: Vec<String>,
    #[serde(default)]
    pub allow_rewrites: bool,
    #[serde(default)]
    pub resume_session: Option<String>,
    /// Cumulative cost last reported by the current agent process.
    #[serde(default)]
    pub last_total_cost: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestView {
    #[serde(flatten)]
    pub info: RequestInfo,
    pub status: RequestStatus,
    pub opened_at: String,
}

#[derive(Debug, Clone, Default)]
pub struct Views {
    pub seq: u64,
    tasks: HashMap<String, TaskView>,
    task_order: Vec<String>,
    requests: HashMap<String, RequestView>,
    request_order: Vec<String>,
    cost_by_day: BTreeMap<String, f64>,
    max_task: u64,
    max_request: u64,
    /// `task + tool` -> the request that was answered `change`.
    revise_hints: HashMap<String, String>,
}

fn numeric_suffix(id: &str) -> u64 {
    id.trim_start_matches(|c: char| !c.is_ascii_digit())
        .parse()
        .unwrap_or(0)
}

fn hint_key(task: &str, tool: &str) -> String {
    format!("{task}\u{1f}{tool}")
}

fn first_line(s: &str, max: usize) -> String {
    let line = s
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    line.chars().take(max).collect()
}

impl Views {
    /// The views a client holds right after a snapshot: its tasks, its open requests and `cost_today_usd`, which
    /// goes under `today` (the UTC day, `YYYY-MM-DD`). The events after `snap.seq` then fold in with [`Views::apply`],
    /// the code sphered runs, so the client stays current without asking for another snapshot. It cannot see
    /// requests that were already closed, so `max_request` only counts the open ones.
    pub fn from_snapshot(snap: Snapshot, today: &str) -> Views {
        let mut v = Views {
            seq: snap.seq,
            ..Views::default()
        };
        for t in snap.tasks {
            v.max_task = v.max_task.max(numeric_suffix(&t.id));
            if !v.tasks.contains_key(&t.id) {
                v.task_order.push(t.id.clone());
            }
            v.tasks.insert(t.id.clone(), t);
        }
        for r in snap.requests {
            v.max_request = v.max_request.max(numeric_suffix(&r.info.id));
            if !v.requests.contains_key(&r.info.id) {
                v.request_order.push(r.info.id.clone());
            }
            v.requests.insert(r.info.id.clone(), r);
        }
        v.cost_by_day.insert(today.to_string(), snap.cost_today_usd);
        v
    }

    /// Forgets the requests that are answered or withdrawn. A client that only shows what is open never looks at
    /// them again, and `apply` scans every request it holds each time a step starts or ends.
    pub fn forget_closed_requests(&mut self) {
        self.requests.retain(|_, r| r.status.is_open());
        let requests = &self.requests;
        self.request_order.retain(|id| requests.contains_key(id));
    }

    pub fn task(&self, id: &str) -> Option<&TaskView> {
        self.tasks.get(id)
    }

    /// Tasks in the order they were created.
    pub fn tasks(&self) -> Vec<&TaskView> {
        self.task_order
            .iter()
            .filter_map(|id| self.tasks.get(id))
            .collect()
    }

    pub fn request(&self, id: &str) -> Option<&RequestView> {
        self.requests.get(id)
    }

    /// Requests still waiting on the human (open or snoozed), oldest first.
    pub fn open_requests(&self) -> Vec<&RequestView> {
        self.request_order
            .iter()
            .filter_map(|id| self.requests.get(id))
            .filter(|r| r.status.is_open())
            .collect()
    }

    pub fn open_requests_for(&self, task: &str) -> Vec<&RequestView> {
        self.open_requests()
            .into_iter()
            .filter(|r| r.info.task == task)
            .collect()
    }

    /// The request a new ask from `task` for `tool` revises, if the last one
    /// was answered `change`.
    pub fn revises(&self, task: &str, tool: &str) -> Option<String> {
        self.revise_hints.get(&hint_key(task, tool)).cloned()
    }

    pub fn max_task(&self) -> u64 {
        self.max_task
    }

    pub fn max_request(&self) -> u64 {
        self.max_request
    }

    /// Cost reported on a UTC day (`YYYY-MM-DD`).
    pub fn cost_on(&self, day: &str) -> f64 {
        self.cost_by_day.get(day).copied().unwrap_or(0.0)
    }

    pub fn active_count(&self) -> usize {
        self.tasks.values().filter(|t| t.state.is_active()).count()
    }

    pub fn queued(&self) -> Vec<&TaskView> {
        self.tasks()
            .into_iter()
            .filter(|t| t.state == TaskState::Queued)
            .collect()
    }

    /// A task that is waiting on a request is `needs_you`; with none left it is
    /// working again.
    fn settle(&mut self, task: &str) {
        let waiting = self
            .requests
            .values()
            .any(|r| r.info.task == task && r.status.is_open());
        if let Some(t) = self.tasks.get_mut(task) {
            match (t.state, waiting) {
                (TaskState::Working, true) => t.state = TaskState::NeedsYou,
                (TaskState::NeedsYou, false) => t.state = TaskState::Working,
                _ => {}
            }
            if t.state == TaskState::NeedsYou {
                if let Some(r) = self
                    .request_order
                    .iter()
                    .filter_map(|id| self.requests.get(id))
                    .find(|r| r.info.task == task && r.status.is_open())
                {
                    t.status = format!("Needs you: {}", request_line(&r.info));
                }
            } else if t.state == TaskState::Working {
                t.status = match t.open_steps.last() {
                    Some(s) => format!("{}, step {}", s.title, t.steps),
                    None => "Working".to_string(),
                };
            }
        }
    }

    pub fn apply(&mut self, ev: &Event) {
        self.seq = self.seq.max(ev.seq);
        let id = ev.task.as_str();
        match &ev.body {
            EventBody::TaskCreated {
                title,
                prompt,
                repo,
                in_place,
                budget_usd,
                mode,
                model,
                allow_rewrites,
                resume_session,
            } => {
                self.max_task = self.max_task.max(numeric_suffix(id));
                if !self.tasks.contains_key(id) {
                    self.task_order.push(id.to_string());
                }
                self.tasks.insert(
                    id.to_string(),
                    TaskView {
                        id: id.to_string(),
                        title: title.clone(),
                        prompt: prompt.clone(),
                        state: TaskState::Queued,
                        status: "Queued".into(),
                        repo: repo.clone(),
                        workdir: None,
                        branch: None,
                        base: None,
                        in_place: *in_place,
                        budget_usd: *budget_usd,
                        cost_usd: 0.0,
                        mode: mode.clone(),
                        model: model.clone(),
                        session_id: resume_session.clone(),
                        child: None,
                        pid: None,
                        started_at: ev.at.clone(),
                        updated_at: ev.at.clone(),
                        files: vec![],
                        plan: vec![],
                        open_steps: vec![],
                        steps: 0,
                        last_say: String::new(),
                        slash_commands: vec![],
                        allow_rewrites: *allow_rewrites,
                        resume_session: resume_session.clone(),
                        last_total_cost: 0.0,
                    },
                );
                return;
            }
            EventBody::Warning { .. } => return,
            _ => {}
        }

        // Request bookkeeping that is not about a single task's fields.
        match &ev.body {
            EventBody::Request { request } => {
                self.max_request = self.max_request.max(numeric_suffix(&request.id));
                if !self.requests.contains_key(&request.id) {
                    self.request_order.push(request.id.clone());
                }
                self.revise_hints
                    .remove(&hint_key(&request.task, &request.tool));
                self.requests.insert(
                    request.id.clone(),
                    RequestView {
                        info: request.clone(),
                        status: RequestStatus::Open,
                        opened_at: ev.at.clone(),
                    },
                );
            }
            EventBody::RequestSnoozed { request } => {
                if let Some(r) = self.requests.get_mut(request) {
                    if r.status.is_open() {
                        r.status = RequestStatus::Snoozed;
                    }
                }
            }
            EventBody::RequestAnswered {
                request, answer, ..
            } => {
                if let Some(r) = self.requests.get_mut(request) {
                    if *answer == AnswerKind::Later {
                        r.status = RequestStatus::Snoozed;
                    } else {
                        r.status = RequestStatus::Answered;
                        if *answer == AnswerKind::Change {
                            self.revise_hints
                                .insert(hint_key(&r.info.task, &r.info.tool), r.info.id.clone());
                        }
                    }
                }
            }
            EventBody::RequestWithdrawn { request, .. } => {
                if let Some(r) = self.requests.get_mut(request) {
                    r.status = RequestStatus::Withdrawn;
                }
            }
            _ => {}
        }

        let day = ev.at.get(..10).unwrap_or("").to_string();
        let mut extra_cost = 0.0;
        let Some(t) = self.tasks.get_mut(id) else {
            return;
        };
        t.updated_at = ev.at.clone();
        match &ev.body {
            EventBody::WorkspaceReady {
                workdir,
                branch,
                base,
            } => {
                t.workdir = Some(workdir.clone());
                t.branch = branch.clone();
                t.base = Some(base.clone());
            }
            EventBody::AgentStarted { child, pid, .. } => {
                t.child = Some(child.clone());
                t.pid = Some(*pid);
                t.state = TaskState::Working;
                t.last_total_cost = 0.0;
                t.status = "Working".into();
            }
            EventBody::AgentExited { .. } => {
                t.child = None;
                t.pid = None;
            }
            EventBody::SessionId { session_id } => t.session_id = Some(session_id.clone()),
            EventBody::Init {
                model,
                permission_mode,
                slash_commands,
            } => {
                if model.is_some() {
                    t.model = model.clone();
                }
                if permission_mode.is_some() {
                    t.mode = permission_mode.clone();
                }
                t.slash_commands = slash_commands.clone();
            }
            EventBody::UserMessage { .. } => {
                if matches!(t.state, TaskState::Done | TaskState::Failed) {
                    t.state = TaskState::Working;
                    t.status = "Working".into();
                }
            }
            EventBody::Say { text } => {
                t.last_say = text.clone();
            }
            EventBody::Plan { items } => t.plan = items.clone(),
            EventBody::Step {
                id,
                tool,
                title,
                detail,
                ..
            } => {
                t.steps += 1;
                t.open_steps.push(OpenStep {
                    id: id.clone(),
                    tool: tool.clone(),
                    title: title.clone(),
                    detail: detail.clone(),
                });
                if t.state == TaskState::Working {
                    t.status = format!("{title}, step {}", t.steps);
                }
            }
            EventBody::StepDone { id, .. } => t.open_steps.retain(|s| &s.id != id),
            EventBody::CutOff { step, .. } => t.open_steps.retain(|s| &s.id != step),
            EventBody::FilesTouched { files } => t.files = files.clone(),
            EventBody::ModeChanged { mode } => t.mode = Some(mode.clone()),
            EventBody::ModelChanged { model } => t.model = Some(model.clone()),
            EventBody::Turn {
                ok,
                interrupted,
                summary,
                cost_usd,
                cost_delta_usd,
                ..
            } => {
                t.open_steps.clear();
                t.cost_usd += cost_delta_usd;
                extra_cost = *cost_delta_usd;
                if let Some(c) = cost_usd {
                    t.last_total_cost = *c;
                }
                if *ok || *interrupted {
                    t.state = TaskState::Done;
                    let s = first_line(summary, 120);
                    t.status = if *interrupted {
                        "Interrupted".to_string()
                    } else if s.is_empty() {
                        "Done".to_string()
                    } else {
                        format!("Done: {s}")
                    };
                } else {
                    t.state = TaskState::Failed;
                    t.status = format!("Failed: {}", first_line(summary, 120));
                }
            }
            EventBody::Stopped {} => {
                t.state = TaskState::Stopped;
                t.status = "Stopped".into();
                t.child = None;
            }
            EventBody::Failed { message } => {
                t.state = TaskState::Failed;
                t.status = format!("Failed: {}", first_line(message, 120));
            }
            _ => {}
        }
        if extra_cost != 0.0 {
            *self.cost_by_day.entry(day).or_insert(0.0) += extra_cost;
        }
        if matches!(
            ev.body,
            EventBody::Request { .. }
                | EventBody::RequestAnswered { .. }
                | EventBody::RequestWithdrawn { .. }
                | EventBody::RequestSnoozed { .. }
                | EventBody::AgentStarted { .. }
                | EventBody::Step { .. }
                | EventBody::StepDone { .. }
        ) {
            let task = ev.task.clone();
            self.settle(&task);
        }
    }
}

fn request_line(info: &RequestInfo) -> String {
    let what = match info.input.get("command").and_then(|v| v.as_str()) {
        Some(cmd) => cmd.to_string(),
        None => info
            .input
            .get("file_path")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_default(),
    };
    if what.is_empty() {
        info.tool.clone()
    } else {
        format!("{} {}", info.tool, first_line(&what, 80))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::RequestKind;
    use serde_json::json;

    fn ev(seq: u64, task: &str, body: EventBody) -> Event {
        Event {
            seq,
            task: task.into(),
            at: "2026-03-04T05:06:07.000Z".into(),
            body,
        }
    }

    fn created(seq: u64, task: &str) -> Event {
        ev(
            seq,
            task,
            EventBody::TaskCreated {
                title: "t".into(),
                prompt: "p".into(),
                repo: "/r".into(),
                in_place: false,
                budget_usd: 1.0,
                mode: None,
                model: None,
                allow_rewrites: false,
                resume_session: None,
            },
        )
    }

    fn started(seq: u64, task: &str) -> Event {
        ev(
            seq,
            task,
            EventBody::AgentStarted {
                child: "c1".into(),
                pid: 5,
                resumed: false,
            },
        )
    }

    fn req(id: &str, task: &str, tool: &str, revises: Option<&str>) -> RequestInfo {
        RequestInfo {
            id: id.into(),
            task: task.into(),
            kind: RequestKind::Permission,
            tool: tool.into(),
            display_name: None,
            description: None,
            summary: String::new(),
            input: json!({"command": "git push origin x"}),
            suggestions: vec![],
            blocked_path: None,
            tool_use_id: None,
            agent_request: Some("a1".into()),
            revises: revises.map(str::to_string),
        }
    }

    #[test]
    fn task_lifecycle_and_request_states() {
        let mut v = Views::default();
        v.apply(&created(1, "t1"));
        assert_eq!(v.task("t1").unwrap().state, TaskState::Queued);
        assert_eq!(v.active_count(), 0);
        v.apply(&started(2, "t1"));
        assert_eq!(v.task("t1").unwrap().state, TaskState::Working);
        assert_eq!(v.active_count(), 1);

        v.apply(&ev(
            3,
            "t1",
            EventBody::Request {
                request: req("r1", "t1", "Bash", None),
            },
        ));
        assert_eq!(v.task("t1").unwrap().state, TaskState::NeedsYou);
        assert!(v
            .task("t1")
            .unwrap()
            .status
            .starts_with("Needs you: Bash git push"));
        assert_eq!(v.open_requests().len(), 1);

        // later: still open, still needs you.
        v.apply(&ev(
            4,
            "t1",
            EventBody::RequestAnswered {
                request: "r1".into(),
                answer: AnswerKind::Later,
                text: None,
                always: false,
            },
        ));
        assert_eq!(v.request("r1").unwrap().status, RequestStatus::Snoozed);
        assert_eq!(v.task("t1").unwrap().state, TaskState::NeedsYou);

        // change: closes it and leaves a hint for the next ask of the same tool.
        v.apply(&ev(
            5,
            "t1",
            EventBody::RequestAnswered {
                request: "r1".into(),
                answer: AnswerKind::Change,
                text: Some("no".into()),
                always: false,
            },
        ));
        assert_eq!(v.request("r1").unwrap().status, RequestStatus::Answered);
        assert_eq!(v.task("t1").unwrap().state, TaskState::Working);
        assert_eq!(v.revises("t1", "Bash").as_deref(), Some("r1"));
        assert_eq!(v.revises("t1", "Edit"), None);

        v.apply(&ev(
            6,
            "t1",
            EventBody::Request {
                request: req("r2", "t1", "Bash", Some("r1")),
            },
        ));
        assert_eq!(v.revises("t1", "Bash"), None, "the hint is used up");
        assert_eq!(v.max_request(), 2);

        v.apply(&ev(
            7,
            "t1",
            EventBody::RequestWithdrawn {
                request: "r2".into(),
                reason: "stop".into(),
            },
        ));
        assert!(v.open_requests().is_empty());
        assert_eq!(v.task("t1").unwrap().state, TaskState::Working);
    }

    #[test]
    fn steps_status_and_cost() {
        let mut v = Views::default();
        v.apply(&created(1, "t1"));
        v.apply(&started(2, "t1"));
        v.apply(&ev(
            3,
            "t1",
            EventBody::Step {
                id: "s1".into(),
                step_kind: crate::StepKind::Run,
                tool: "Bash".into(),
                title: "Running npx playwright test".into(),
                detail: String::new(),
                parent: None,
            },
        ));
        assert_eq!(
            v.task("t1").unwrap().status,
            "Running npx playwright test, step 1"
        );
        assert_eq!(v.task("t1").unwrap().open_steps.len(), 1);
        v.apply(&ev(
            4,
            "t1",
            EventBody::StepDone {
                id: "s1".into(),
                ok: true,
                output: None,
            },
        ));
        assert!(v.task("t1").unwrap().open_steps.is_empty());

        let turn = |seq, total, delta| {
            ev(
                seq,
                "t1",
                EventBody::Turn {
                    ok: true,
                    interrupted: false,
                    summary: "all good\nmore".into(),
                    turns: Some(1),
                    duration_ms: None,
                    cost_usd: Some(total),
                    cost_delta_usd: delta,
                },
            )
        };
        v.apply(&turn(5, 0.25, 0.25));
        v.apply(&ev(
            6,
            "t1",
            EventBody::UserMessage {
                text: "more".into(),
            },
        ));
        assert_eq!(v.task("t1").unwrap().state, TaskState::Working);
        v.apply(&turn(7, 0.75, 0.5));
        let t = v.task("t1").unwrap();
        assert_eq!(t.state, TaskState::Done);
        assert_eq!(t.status, "Done: all good");
        assert!((t.cost_usd - 0.75).abs() < 1e-9);
        assert!((v.cost_on("2026-03-04") - 0.75).abs() < 1e-9);
        assert_eq!(v.cost_on("2026-03-05"), 0.0);
    }

    #[test]
    fn failed_stopped_and_unknown_tasks() {
        let mut v = Views::default();
        v.apply(&created(1, "t12"));
        assert_eq!(v.max_task(), 12);
        v.apply(&ev(
            2,
            "t12",
            EventBody::Failed {
                message: "boom".into(),
            },
        ));
        assert_eq!(v.task("t12").unwrap().state, TaskState::Failed);
        v.apply(&ev(3, "t12", EventBody::Stopped {}));
        assert_eq!(v.task("t12").unwrap().state, TaskState::Stopped);
        // Events for tasks we never saw are ignored, not fatal.
        v.apply(&ev(4, "t99", EventBody::Say { text: "x".into() }));
        assert!(v.task("t99").is_none());
        assert_eq!(v.seq, 4);
    }

    const TODAY: &str = "2026-03-04";

    fn ev_at(seq: u64, task: &str, at: &str, body: EventBody) -> Event {
        Event {
            seq,
            task: task.into(),
            at: at.into(),
            body,
        }
    }

    fn step(id: &str, title: &str) -> EventBody {
        EventBody::Step {
            id: id.into(),
            step_kind: crate::StepKind::Run,
            tool: "Bash".into(),
            title: title.into(),
            detail: String::new(),
            parent: None,
        }
    }

    fn step_done(id: &str) -> EventBody {
        EventBody::StepDone {
            id: id.into(),
            ok: true,
            output: None,
        }
    }

    fn turn(ok: bool, delta: f64) -> EventBody {
        EventBody::Turn {
            ok,
            interrupted: false,
            summary: "all done\nmore".into(),
            turns: Some(1),
            duration_ms: None,
            cost_usd: Some(delta),
            cost_delta_usd: delta,
        }
    }

    /// What a client holds after `v`: sphered's snapshot of it.
    fn snapshot_of(v: &Views) -> Snapshot {
        Snapshot {
            seq: v.seq,
            tasks: v.tasks().into_iter().cloned().collect(),
            requests: v.open_requests().into_iter().cloned().collect(),
            cost_today_usd: v.cost_on(TODAY),
            daily_cap_usd: None,
            max_concurrent: None,
        }
    }

    /// Every way the app can see the views: the same afterwards, whatever the snapshot was taken at.
    fn assert_same_views(client: &Views, server: &Views, what: &str) {
        assert_eq!(client.seq, server.seq, "{what}: seq");
        assert_eq!(client.tasks(), server.tasks(), "{what}: tasks");
        assert_eq!(
            client.open_requests(),
            server.open_requests(),
            "{what}: open requests"
        );
        assert_eq!(client.cost_on(TODAY), server.cost_on(TODAY), "{what}: cost");
    }

    /// A client built from the snapshot taken after `events[..k]` and fed `events[k..]`, closed requests dropped as
    /// it goes, must end up exactly where the server is.
    fn check_every_split(events: &[Event], splits: impl Iterator<Item = usize>) {
        let mut server = Views::default();
        for e in events {
            server.apply(e);
        }
        for k in splits {
            let mut before = Views::default();
            for e in &events[..k] {
                before.apply(e);
            }
            let mut client = Views::from_snapshot(snapshot_of(&before), TODAY);
            for e in &events[k..] {
                client.apply(e);
                client.forget_closed_requests();
            }
            assert_same_views(&client, &server, &format!("snapshot after {k} events"));
        }
    }

    #[test]
    fn a_client_folds_events_into_a_snapshot_like_the_server() {
        let day = |d: u32| format!("2026-03-0{d}T05:06:07.000Z");
        let init = EventBody::Init {
            model: Some("m".into()),
            permission_mode: Some("default".into()),
            slash_commands: vec!["/a".into(), "/b".into()],
        };
        let bodies: Vec<(&str, u32, EventBody)> = vec![
            ("t1", 4, created(0, "t1").body),
            ("t2", 4, created(0, "t2").body),
            (
                "t1",
                4,
                EventBody::WorkspaceReady {
                    workdir: "/w/t1".into(),
                    branch: Some("sphere/t1".into()),
                    base: "abc".into(),
                },
            ),
            ("t1", 4, started(0, "t1").body),
            ("t1", 4, init),
            ("t1", 4, step("s1", "Running tests")),
            (
                "t1",
                4,
                EventBody::Say {
                    text: "looking".into(),
                },
            ),
            ("t2", 4, started(0, "t2").body),
            ("t2", 4, step("a", "Reading a.rs")),
            // t1 asks while its step runs: it needs you, and its status says what for
            (
                "t1",
                4,
                EventBody::Request {
                    request: req("r1", "t1", "Bash", None),
                },
            ),
            ("t1", 4, step("s2", "Editing")),
            ("t1", 4, step_done("s1")),
            (
                "t1",
                4,
                EventBody::RequestSnoozed {
                    request: "r1".into(),
                },
            ),
            (
                "t2",
                4,
                EventBody::Request {
                    request: req("r2", "t2", "Edit", None),
                },
            ),
            (
                "t1",
                4,
                EventBody::RequestAnswered {
                    request: "r1".into(),
                    answer: AnswerKind::Change,
                    text: Some("no".into()),
                    always: false,
                },
            ),
            ("t1", 4, step_done("s2")),
            (
                "t1",
                4,
                EventBody::FilesTouched {
                    files: vec![FileStat {
                        path: "a.rs".into(),
                        added: 3,
                        removed: 1,
                    }],
                },
            ),
            (
                "t1",
                4,
                EventBody::Plan {
                    items: vec![PlanItem {
                        text: "do it".into(),
                        status: "pending".into(),
                    }],
                },
            ),
            (
                "t2",
                4,
                EventBody::RequestWithdrawn {
                    request: "r2".into(),
                    reason: "stop".into(),
                },
            ),
            ("t2", 4, step_done("a")),
            ("t1", 4, turn(true, 0.25)),
            (
                "t1",
                4,
                EventBody::UserMessage {
                    text: "more".into(),
                },
            ),
            ("t1", 4, step("s3", "Again")),
            (
                "t1",
                4,
                EventBody::Warning {
                    message: "careful".into(),
                },
            ),
            // a turn on an earlier day costs nothing today
            ("t2", 3, turn(false, 0.5)),
            (
                "t2",
                4,
                EventBody::Failed {
                    message: "boom\nmore".into(),
                },
            ),
            (
                "t1",
                4,
                EventBody::ModeChanged {
                    mode: "plan".into(),
                },
            ),
            ("t1", 4, EventBody::ModelChanged { model: "m2".into() }),
            (
                "t1",
                4,
                EventBody::CutOff {
                    step: "s3".into(),
                    title: "Again".into(),
                    outbound: false,
                },
            ),
            (
                "t1",
                4,
                EventBody::AutoAnswered {
                    tool: "Read".into(),
                    allow: true,
                    rule: "r".into(),
                },
            ),
            (
                "t1",
                4,
                EventBody::AgentExited {
                    code: Some(0),
                    signal: None,
                    reason: "exit".into(),
                },
            ),
            (
                "t1",
                4,
                EventBody::SessionId {
                    session_id: "s".into(),
                },
            ),
            // a task nobody told us about, and one that starts late
            ("t9", 4, EventBody::Say { text: "who".into() }),
            ("t1", 4, started(0, "t1").body),
            (
                "t1",
                4,
                EventBody::Request {
                    request: req("r3", "t1", "Bash", Some("r1")),
                },
            ),
            ("t1", 4, turn(true, 0.125)),
            ("t3", 4, created(0, "t3").body),
            ("t3", 4, step("x", "Queued step")),
            ("t1", 4, EventBody::Stopped {}),
            (
                "t1",
                4,
                EventBody::RequestAnswered {
                    request: "r3".into(),
                    answer: AnswerKind::Yes,
                    text: None,
                    always: false,
                },
            ),
        ];
        let events: Vec<Event> = bodies
            .into_iter()
            .enumerate()
            .map(|(i, (task, d, body))| ev_at(i as u64 + 1, task, &day(d), body))
            .collect();
        check_every_split(&events, 0..=events.len());
    }

    /// A made-up but lawful history: requests are answered, snoozed or withdrawn only while they are open.
    fn made_up_history(seed: u64, n: u64) -> Vec<Event> {
        let mut x = seed;
        let mut next = move |m: u64| {
            x = x
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (x >> 33) % m
        };
        let (mut events, mut made, mut open, mut steps) = (vec![], 0u64, vec![], 0u64);
        for seq in 1..=n {
            let at = format!("2026-03-0{}T05:06:{:02}.000Z", 3 + next(2), next(60));
            let mut task = format!("t{}", 1 + next(4));
            let kind = next(14);
            let body = match kind {
                _ if made < 4 && (kind < 3 || task == format!("t{}", made + 1)) => {
                    made += 1;
                    task = format!("t{made}");
                    created(0, &task).body
                }
                0 | 1 => started(0, &task).body,
                2..=4 => {
                    steps += 1;
                    step(&format!("s{steps}"), &format!("Step {steps}"))
                }
                5 => step_done(&format!("s{}", 1 + next(steps.max(1)))),
                6 => EventBody::Say {
                    text: format!("said {seq}"),
                },
                7 => {
                    let id = format!("r{seq}");
                    open.push((id.clone(), task.clone()));
                    EventBody::Request {
                        request: req(&id, &task, "Bash", None),
                    }
                }
                8 | 9 if !open.is_empty() => {
                    let (id, t) = open.remove(next(open.len() as u64) as usize);
                    task = t;
                    if kind == 8 {
                        EventBody::RequestWithdrawn {
                            request: id,
                            reason: "gone".into(),
                        }
                    } else {
                        let answer = [AnswerKind::Yes, AnswerKind::Change][next(2) as usize];
                        EventBody::RequestAnswered {
                            request: id,
                            answer,
                            text: None,
                            always: false,
                        }
                    }
                }
                10 if !open.is_empty() => {
                    let (id, t) = open[next(open.len() as u64) as usize].clone();
                    task = t;
                    if next(2) == 0 {
                        EventBody::RequestSnoozed { request: id }
                    } else {
                        EventBody::RequestAnswered {
                            request: id,
                            answer: AnswerKind::Later,
                            text: None,
                            always: false,
                        }
                    }
                }
                11 => turn(next(3) != 0, 0.125 * next(5) as f64),
                12 => EventBody::UserMessage {
                    text: "again".into(),
                },
                _ => [
                    EventBody::Warning {
                        message: "w".into(),
                    },
                    EventBody::Stopped {},
                    EventBody::Failed {
                        message: "f".into(),
                    },
                ][next(3) as usize]
                    .clone(),
            };
            events.push(ev_at(seq, &task, &at, body));
        }
        events
    }

    #[test]
    fn a_client_stays_exact_over_a_long_made_up_history() {
        for seed in [1, 2, 3] {
            let events = made_up_history(seed, 400);
            check_every_split(&events, (0..=events.len()).step_by(7));
        }
    }

    #[test]
    fn a_snapshot_is_where_the_next_event_picks_up() {
        let mut server = Views::default();
        server.apply(&created(1, "t1"));
        server.apply(&started(2, "t1"));
        let mut client = Views::from_snapshot(snapshot_of(&server), TODAY);
        assert_eq!(client.seq, 2);
        assert_eq!(client.max_task(), 1);
        assert_eq!(client.tasks(), server.tasks());
        for e in [
            ev(3, "t1", step("s1", "Running")),
            ev(4, "t1", step_done("s1")),
        ] {
            server.apply(&e);
            client.apply(&e);
        }
        assert_eq!(client.task("t1"), server.task("t1"));
        assert_eq!(client.task("t1").unwrap().status, "Working");
        assert_eq!(client.task("t1").unwrap().steps, 1);
    }

    #[test]
    fn closed_requests_are_forgotten_and_open_ones_stay() {
        let mut v = Views::default();
        v.apply(&created(1, "t1"));
        v.apply(&started(2, "t1"));
        for (seq, id) in [(3, "r1"), (4, "r2"), (5, "r3")] {
            v.apply(&ev(
                seq,
                "t1",
                EventBody::Request {
                    request: req(id, "t1", "Bash", None),
                },
            ));
        }
        v.apply(&ev(
            6,
            "t1",
            EventBody::RequestAnswered {
                request: "r1".into(),
                answer: AnswerKind::Yes,
                text: None,
                always: false,
            },
        ));
        v.apply(&ev(
            7,
            "t1",
            EventBody::RequestWithdrawn {
                request: "r2".into(),
                reason: "stop".into(),
            },
        ));
        v.apply(&ev(
            8,
            "t1",
            EventBody::RequestSnoozed {
                request: "r3".into(),
            },
        ));
        v.forget_closed_requests();
        assert!(v.request("r1").is_none() && v.request("r2").is_none());
        assert_eq!(v.request("r3").unwrap().status, RequestStatus::Snoozed);
        assert_eq!(v.open_requests().len(), 1);
        // a snoozed request still holds the task
        assert_eq!(v.task("t1").unwrap().state, TaskState::NeedsYou);
        v.apply(&ev(
            9,
            "t1",
            EventBody::RequestAnswered {
                request: "r3".into(),
                answer: AnswerKind::Yes,
                text: None,
                always: false,
            },
        ));
        v.forget_closed_requests();
        assert!(v.open_requests().is_empty());
        assert_eq!(v.task("t1").unwrap().state, TaskState::Working);
    }
}
