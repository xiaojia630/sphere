//! Everything a client shows is computed from the event log by `Views::apply`.
//! sphered rebuilds its views from the store with it; the CLI and the app run
//! the same code over a snapshot plus the events that follow.

use crate::event::{Event, EventBody, FileStat, PlanItem, TaskState};
use crate::request::{AnswerKind, RequestInfo, RequestStatus};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

/// A tool call that has started and has no result yet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpenStep {
    pub id: String,
    pub tool: String,
    pub title: String,
    #[serde(default)]
    pub detail: String,
}

/// The status of a resumed session that waits for the user's first message.
pub const WAITING_FOR_YOU: &str = "Waiting for your message";

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
    /// How many agent processes this task has started.
    #[serde(default)]
    pub starts: u32,
    /// Where the session ran before it was taken over into Sphere.
    #[serde(default)]
    pub taken_over_from: Option<String>,
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
    /// Seconds agents waited on the human, by the UTC day the request closed.
    waited_by_day: BTreeMap<String, u64>,
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

/// Whole seconds from one RFC 3339 time to a later one. 0 when either won't parse or the span
/// runs backwards.
fn seconds_between(from: &str, to: &str) -> u64 {
    let parse = |s: &str| OffsetDateTime::parse(s, &Rfc3339).ok();
    match (parse(from), parse(to)) {
        (Some(a), Some(b)) => u64::try_from((b - a).whole_seconds()).unwrap_or(0),
        _ => 0,
    }
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

    /// Seconds agents spent waiting on the human, over the requests that closed on a UTC day
    /// (`YYYY-MM-DD`). A request still open isn't counted.
    pub fn waited_on(&self, day: &str) -> u64 {
        self.waited_by_day.get(day).copied().unwrap_or(0)
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
                        starts: 0,
                        taken_over_from: None,
                    },
                );
                return;
            }
            EventBody::Warning { .. } => return,
            _ => {}
        }

        let day = ev.at.get(..10).unwrap_or("").to_string();

        // A request that was open or snoozed closes for good: what it waited counts toward the
        // day it closed. This runs before the status changes, so a second close adds nothing.
        let closing = match &ev.body {
            EventBody::RequestAnswered {
                request, answer, ..
            } if *answer != AnswerKind::Later => Some(request),
            EventBody::RequestWithdrawn { request, .. } => Some(request),
            _ => None,
        };
        if let Some(r) = closing
            .and_then(|id| self.requests.get(id))
            .filter(|r| r.status.is_open())
        {
            let secs = seconds_between(&r.opened_at, &ev.at);
            if secs > 0 {
                *self.waited_by_day.entry(day.clone()).or_insert(0) += secs;
            }
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
                if t.starts == 0 && t.prompt.trim().is_empty() {
                    // Resumed with no message: it waits for the user's.
                    t.state = TaskState::Done;
                    t.status = WAITING_FOR_YOU.into();
                } else {
                    t.status = "Working".into();
                }
                t.starts += 1;
            }
            EventBody::TakenOver { from } => t.taken_over_from = Some(from.clone()),
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
                turns,
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
                // a resumed session reporting in with no turn still waits for the user
                if *ok && *turns == Some(0) && t.status == WAITING_FOR_YOU {
                    t.state = TaskState::Done;
                } else if *ok || *interrupted {
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
    use crate::request::AnswerKind::{Change, Later, Yes};
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

    fn at(seq: u64, when: &str, body: EventBody) -> Event {
        Event {
            at: when.into(),
            ..ev(seq, "t1", body)
        }
    }

    fn ask(seq: u64, when: &str, id: &str) -> Event {
        let request = req(id, "t1", "Bash", None);
        at(seq, when, EventBody::Request { request })
    }

    fn answer(seq: u64, when: &str, id: &str, answer: AnswerKind) -> Event {
        let body = EventBody::RequestAnswered {
            request: id.into(),
            answer,
            text: None,
            always: false,
        };
        at(seq, when, body)
    }

    fn snooze(seq: u64, when: &str, id: &str) -> Event {
        let request = id.into();
        at(seq, when, EventBody::RequestSnoozed { request })
    }

    fn withdraw(seq: u64, when: &str, id: &str) -> Event {
        let body = EventBody::RequestWithdrawn {
            request: id.into(),
            reason: "stop".into(),
        };
        at(seq, when, body)
    }

    /// Views with one running task, `t1`.
    fn with_task() -> Views {
        let mut v = Views::default();
        v.apply(&created(1, "t1"));
        v.apply(&started(2, "t1"));
        v
    }

    #[test]
    fn a_session_resumed_with_no_message_waits_for_the_user() {
        let mut v = Views::default();
        v.apply(&ev(
            1,
            "t1",
            EventBody::TaskCreated {
                title: "t".into(),
                prompt: String::new(),
                repo: "/r".into(),
                in_place: true,
                budget_usd: 1.0,
                mode: None,
                model: None,
                allow_rewrites: false,
                resume_session: Some("s1".into()),
            },
        ));
        v.apply(&ev(
            2,
            "t1",
            EventBody::TakenOver {
                from: "herdr".into(),
            },
        ));
        v.apply(&started(3, "t1"));
        let t = v.task("t1").unwrap();
        assert_eq!(
            (t.state, t.status.as_str()),
            (TaskState::Done, WAITING_FOR_YOU)
        );
        assert_eq!(t.taken_over_from.as_deref(), Some("herdr"));
        // the resumed agent reports in with no turn: still waiting, nothing spent
        let turn = |seq, turns| {
            ev(
                seq,
                "t1",
                EventBody::Turn {
                    ok: true,
                    interrupted: false,
                    summary: String::new(),
                    turns,
                    duration_ms: None,
                    cost_usd: Some(0.26),
                    cost_delta_usd: 0.0,
                },
            )
        };
        v.apply(&turn(4, Some(0)));
        let t = v.task("t1").unwrap();
        assert_eq!(
            (t.state, t.status.as_str()),
            (TaskState::Done, WAITING_FOR_YOU)
        );
        assert_eq!((t.cost_usd, t.last_total_cost), (0.0, 0.26));
        v.apply(&turn(5, Some(1)));
        assert_eq!(v.task("t1").unwrap().status, "Done");
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

    #[test]
    fn answering_counts_the_whole_seconds_the_agent_waited() {
        let mut v = with_task();
        v.apply(&ask(3, "2026-03-04T10:00:00.000Z", "r1"));
        assert_eq!(v.waited_on("2026-03-04"), 0, "still waiting");
        v.apply(&answer(4, "2026-03-04T10:03:00.000Z", "r1", Yes));
        assert_eq!(v.waited_on("2026-03-04"), 180);
        assert_eq!(v.waited_on("2026-03-05"), 0);

        // Change closes it too, and a part of a second doesn't count.
        v.apply(&ask(5, "2026-03-04T10:10:00.800Z", "r2"));
        v.apply(&answer(6, "2026-03-04T10:10:02.700Z", "r2", Change));
        assert_eq!(v.waited_on("2026-03-04"), 181);
    }

    #[test]
    fn later_keeps_the_agent_waiting_until_the_request_closes_for_good() {
        let mut v = with_task();
        v.apply(&ask(3, "2026-03-04T10:00:00.000Z", "r1"));
        v.apply(&answer(4, "2026-03-04T10:01:00.000Z", "r1", Later));
        v.apply(&snooze(5, "2026-03-04T10:02:00.000Z", "r1"));
        assert_eq!(v.request("r1").unwrap().status, RequestStatus::Snoozed);
        assert_eq!(v.waited_on("2026-03-04"), 0, "snoozed is still waiting");

        v.apply(&answer(6, "2026-03-04T10:10:00.000Z", "r1", Yes));
        assert_eq!(v.waited_on("2026-03-04"), 600, "counted from opening");
    }

    #[test]
    fn a_withdrawn_request_counts_what_it_waited() {
        let mut v = with_task();
        v.apply(&ask(3, "2026-03-04T10:00:00.000Z", "r1"));
        v.apply(&withdraw(4, "2026-03-04T10:02:30.000Z", "r1"));
        assert_eq!(v.waited_on("2026-03-04"), 150);

        // One that was snoozed first counts all the same.
        v.apply(&ask(5, "2026-03-04T11:00:00.000Z", "r2"));
        v.apply(&snooze(6, "2026-03-04T11:00:10.000Z", "r2"));
        v.apply(&withdraw(7, "2026-03-04T11:01:00.000Z", "r2"));
        assert_eq!(v.waited_on("2026-03-04"), 210);
    }

    #[test]
    fn a_request_closed_twice_counts_once() {
        let mut v = with_task();
        v.apply(&ask(3, "2026-03-04T10:00:00.000Z", "r1"));
        v.apply(&answer(4, "2026-03-04T10:03:00.000Z", "r1", Yes));
        v.apply(&answer(5, "2026-03-04T10:09:00.000Z", "r1", Yes));
        v.apply(&answer(6, "2026-03-04T10:10:00.000Z", "r1", Change));
        v.apply(&withdraw(7, "2026-03-04T10:20:00.000Z", "r1"));
        assert_eq!(v.waited_on("2026-03-04"), 180);

        // Withdrawn first, then answered anyway.
        v.apply(&ask(8, "2026-03-04T11:00:00.000Z", "r2"));
        v.apply(&withdraw(9, "2026-03-04T11:01:00.000Z", "r2"));
        v.apply(&answer(10, "2026-03-04T11:05:00.000Z", "r2", Yes));
        assert_eq!(v.waited_on("2026-03-04"), 240);

        // A request nobody opened has nothing to count.
        v.apply(&answer(11, "2026-03-04T12:00:00.000Z", "r9", Yes));
        assert_eq!(v.waited_on("2026-03-04"), 240);
    }

    #[test]
    fn each_day_keeps_its_own_waited_total() {
        let mut v = with_task();
        v.apply(&ask(3, "2026-03-04T08:00:00.000Z", "r1"));
        v.apply(&answer(4, "2026-03-04T08:00:45.000Z", "r1", Yes));
        // Opened late on the 4th, answered on the 5th: it counts in full on the 5th.
        v.apply(&ask(5, "2026-03-04T23:59:00.000Z", "r2"));
        v.apply(&answer(6, "2026-03-05T00:01:00.000Z", "r2", Yes));
        v.apply(&ask(7, "2026-03-05T09:00:00.000Z", "r3"));
        v.apply(&withdraw(8, "2026-03-05T09:00:30.000Z", "r3"));
        assert_eq!(v.waited_on("2026-03-04"), 45);
        assert_eq!(v.waited_on("2026-03-05"), 150);
        assert_eq!(v.waited_on("2026-03-06"), 0);
    }

    #[test]
    fn spans_that_cannot_be_measured_count_nothing() {
        let mut v = with_task();
        // Opened at a time that is not a time.
        v.apply(&ask(3, "sometime", "r1"));
        v.apply(&answer(4, "2026-03-04T10:00:00.000Z", "r1", Yes));
        // Closed at a time that is not a time.
        v.apply(&ask(5, "2026-03-04T10:00:00.000Z", "r2"));
        v.apply(&answer(6, "later", "r2", Yes));
        // Closed before it opened.
        v.apply(&ask(7, "2026-03-04T10:05:00.000Z", "r3"));
        v.apply(&answer(8, "2026-03-04T10:00:00.000Z", "r3", Yes));
        assert_eq!(v.waited_on("2026-03-04"), 0);
        assert_eq!(v.waited_on(""), 0);
        assert!(v.open_requests().is_empty(), "they still closed");
    }
}
