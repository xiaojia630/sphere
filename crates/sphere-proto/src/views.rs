//! Everything a client shows is computed from the event log by `Views::apply`.
//! sphered rebuilds its views from the store with it; the CLI and the app run
//! the same code over a snapshot plus the events that follow.

use crate::api::Snapshot;
use crate::event::{Event, EventBody, FileStat, PlanItem, TaskState};
use crate::protocol::Protocol;
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
    /// How many of the user's notes the latest agent process was told when it started.
    #[serde(default)]
    pub briefed: u32,
    /// The protocol (`p1`) that started the task on its schedule.
    #[serde(default)]
    pub protocol: Option<String>,
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
    protocols: HashMap<String, Protocol>,
    protocol_order: Vec<String>,
    max_task: u64,
    max_request: u64,
    /// The highest protocol number ever saved: ids are not reused when a protocol is removed.
    max_protocol: u64,
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
pub(crate) fn seconds_between(from: &str, to: &str) -> u64 {
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
    /// The views a client holds right after a snapshot: its tasks, its open requests, `cost_today_usd` and
    /// `waited_today_secs`, which go under `today` (the UTC day, `YYYY-MM-DD`). The events after `snap.seq` then fold
    /// in with [`Views::apply`], the code sphered runs, so the client stays current without asking for another
    /// snapshot. It cannot see requests that were already closed, so `max_request` only counts the open ones.
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
        for p in snap.protocols {
            v.save_protocol(p);
        }
        v.cost_by_day.insert(today.to_string(), snap.cost_today_usd);
        v.waited_by_day
            .insert(today.to_string(), snap.waited_today_secs);
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

    pub fn protocol(&self, id: &str) -> Option<&Protocol> {
        self.protocols.get(id)
    }

    /// Protocols in the order they were written.
    pub fn protocols(&self) -> Vec<&Protocol> {
        self.protocol_order
            .iter()
            .filter_map(|id| self.protocols.get(id))
            .collect()
    }

    /// The highest protocol number ever saved, for numbering the next one.
    pub fn max_protocol(&self) -> u64 {
        self.max_protocol
    }

    fn save_protocol(&mut self, p: Protocol) {
        self.max_protocol = self.max_protocol.max(numeric_suffix(&p.id));
        if !self.protocols.contains_key(&p.id) {
            self.protocol_order.push(p.id.clone());
        }
        self.protocols.insert(p.id.clone(), p);
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
                protocol,
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
                        briefed: 0,
                        protocol: protocol.clone(),
                    },
                );
                return;
            }
            EventBody::ProtocolSaved { protocol } => {
                self.save_protocol(protocol.clone());
                return;
            }
            EventBody::ProtocolRemoved { id: removed } => {
                self.protocols.remove(removed);
                self.protocol_order.retain(|p| p != removed);
                return;
            }
            EventBody::Warning { .. } | EventBody::MemoryChanged { .. } => return,
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
            EventBody::AgentStarted {
                child,
                pid,
                briefed,
                ..
            } => {
                t.child = Some(child.clone());
                t.pid = Some(*pid);
                t.briefed = *briefed;
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
    use crate::protocol::{Schedule, Weekday};
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
                protocol: None,
            },
        )
    }

    fn created_by(seq: u64, task: &str, protocol: &str) -> Event {
        let mut e = created(seq, task);
        if let EventBody::TaskCreated { protocol: p, .. } = &mut e.body {
            *p = Some(protocol.to_string());
        }
        e
    }

    fn saved(id: &str, enabled: bool) -> EventBody {
        EventBody::ProtocolSaved {
            protocol: Protocol {
                id: id.into(),
                text: format!("every day 9:00 sphere: {id}"),
                schedule: Schedule::Weekly {
                    days: Weekday::ALL.to_vec(),
                    hour: 9,
                    minute: 0,
                },
                repo: "/r".into(),
                prompt: id.into(),
                enabled,
                created_at: 1,
                last_run: None,
                next_run: enabled.then_some(3600),
            },
        }
    }

    fn started(seq: u64, task: &str) -> Event {
        ev(
            seq,
            task,
            EventBody::AgentStarted {
                child: "c1".into(),
                pid: 5,
                resumed: false,
                briefed: 0,
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
                protocol: None,
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
    fn a_task_keeps_how_many_notes_its_latest_agent_was_told() {
        let mut v = Views::default();
        v.apply(&created(1, "t1"));
        assert_eq!(v.task("t1").unwrap().briefed, 0);
        let start = |seq, briefed| {
            ev(
                seq,
                "t1",
                EventBody::AgentStarted {
                    child: "c1".into(),
                    pid: 5,
                    resumed: seq > 2,
                    briefed,
                },
            )
        };
        v.apply(&start(2, 3));
        assert_eq!(v.task("t1").unwrap().briefed, 3);
        // a resumed process is briefed again, with the notes as they are by then
        v.apply(&start(3, 1));
        assert_eq!(v.task("t1").unwrap().briefed, 1);
    }

    #[test]
    fn a_memory_change_moves_the_seq_and_touches_no_task() {
        let mut v = Views::default();
        v.apply(&created(1, "t1"));
        let before = v.task("t1").unwrap().clone();
        v.apply(&ev(2, "", EventBody::MemoryChanged { notes: 4 }));
        assert_eq!(v.seq, 2);
        assert_eq!(v.task("t1"), Some(&before));
        assert_eq!(v.tasks().len(), 1);
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
            waited_today_secs: v.waited_on(TODAY),
            memory: vec![],
            protocols: v.protocols().into_iter().cloned().collect(),
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
        assert_eq!(client.protocols(), server.protocols(), "{what}: protocols");
        assert_eq!(client.cost_on(TODAY), server.cost_on(TODAY), "{what}: cost");
        assert_eq!(
            client.waited_on(TODAY),
            server.waited_on(TODAY),
            "{what}: waited"
        );
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
            let kind = next(16);
            let body = match kind {
                _ if made < 4 && (kind < 3 || task == format!("t{}", made + 1)) => {
                    made += 1;
                    task = format!("t{made}");
                    // Every other task is one a protocol started.
                    if made % 2 == 0 {
                        created_by(0, &task, &format!("p{}", 1 + next(3))).body
                    } else {
                        created(0, &task).body
                    }
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
                14 => {
                    task = String::new();
                    saved(&format!("p{}", 1 + next(3)), next(2) == 0)
                }
                15 => {
                    task = String::new();
                    EventBody::ProtocolRemoved {
                        id: format!("p{}", 1 + next(3)),
                    }
                }
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

    #[test]
    fn protocols_are_saved_replaced_and_removed_in_order() {
        let mut v = Views::default();
        v.apply(&ev(1, "", saved("p1", true)));
        v.apply(&ev(2, "", saved("p2", true)));
        assert_eq!(v.seq, 2);
        let ids = |v: &Views| {
            v.protocols()
                .iter()
                .map(|p| p.id.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(&v), ["p1", "p2"]);
        // Saving again replaces what was there and keeps its place.
        v.apply(&ev(3, "", saved("p1", false)));
        assert_eq!(ids(&v), ["p1", "p2"]);
        assert!(!v.protocol("p1").unwrap().enabled);
        assert_eq!(v.protocol("p1").unwrap().next_run, None);
        v.apply(&ev(4, "", EventBody::ProtocolRemoved { id: "p1".into() }));
        assert_eq!(ids(&v), ["p2"]);
        assert!(v.protocol("p1").is_none());
        // A number that was used is not handed out again, even once its protocol is gone.
        assert_eq!(v.max_protocol(), 2);
        v.apply(&ev(5, "", EventBody::ProtocolRemoved { id: "p2".into() }));
        assert_eq!(v.max_protocol(), 2);
        // Removing what isn't there is nothing.
        v.apply(&ev(6, "", EventBody::ProtocolRemoved { id: "p9".into() }));
        assert!(v.protocols().is_empty() && v.seq == 6);
        assert!(v.tasks().is_empty());
    }

    #[test]
    fn a_task_remembers_the_protocol_that_started_it() {
        let mut v = Views::default();
        v.apply(&created(1, "t1"));
        v.apply(&created_by(2, "t2", "p3"));
        assert_eq!(v.task("t1").unwrap().protocol, None);
        assert_eq!(v.task("t2").unwrap().protocol.as_deref(), Some("p3"));
        // The task view travels in a snapshot with it, and one from an older sphered has none.
        let json = serde_json::to_value(v.task("t2").unwrap()).unwrap();
        assert_eq!(json["protocol"], "p3");
        let mut old = json;
        old.as_object_mut().unwrap().remove("protocol");
        let t: TaskView = serde_json::from_value(old).unwrap();
        assert_eq!(t.protocol, None);
    }

    #[test]
    fn a_client_keeps_the_protocols_current_from_a_snapshot() {
        let mut server = Views::default();
        server.apply(&ev(1, "", saved("p1", true)));
        server.apply(&created_by(2, "t1", "p1"));
        let mut client = Views::from_snapshot(snapshot_of(&server), TODAY);
        assert_eq!(client.protocols(), server.protocols());
        assert_eq!(client.max_protocol(), 1);
        for e in [
            ev(3, "", saved("p2", true)),
            ev(4, "", saved("p1", false)),
            ev(5, "", EventBody::ProtocolRemoved { id: "p2".into() }),
        ] {
            server.apply(&e);
            client.apply(&e);
        }
        assert_eq!(client.protocols(), server.protocols());
        assert_eq!(client.protocols().len(), 1);
    }
}
