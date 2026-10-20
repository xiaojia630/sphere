use crate::protocol::Protocol;
use crate::request::{AnswerKind, RequestInfo};
use serde::{Deserialize, Serialize};
use time::format_description::FormatItem;
use time::macros::format_description;
use time::OffsetDateTime;

/// One durable thing that happened. `seq` is global and monotonic, so a client
/// can ask for everything after the last `seq` it saw. `task` is empty for
/// events that belong to no task.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub seq: u64,
    pub task: String,
    /// RFC 3339, UTC, millisecond precision (fixed width, so it sorts as text).
    pub at: String,
    #[serde(flatten)]
    pub body: EventBody,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventBody {
    TaskCreated {
        title: String,
        prompt: String,
        repo: String,
        #[serde(default)]
        in_place: bool,
        budget_usd: f64,
        /// Initial permission mode.
        #[serde(default)]
        mode: Option<String>,
        #[serde(default)]
        model: Option<String>,
        /// Force pushes and history rewrites are not denied for this task.
        #[serde(default)]
        allow_rewrites: bool,
        /// Set when the task brings in an existing Claude session.
        #[serde(default)]
        resume_session: Option<String>,
        /// The protocol that started the task, when one did.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        protocol: Option<String>,
    },
    WorkspaceReady {
        workdir: String,
        #[serde(default)]
        branch: Option<String>,
        base: String,
    },
    AgentStarted {
        child: String,
        pid: u32,
        resumed: bool,
        /// How many of the user's notes this process was told when it started.
        #[serde(default)]
        briefed: u32,
    },
    AgentExited {
        #[serde(default)]
        code: Option<i32>,
        #[serde(default)]
        signal: Option<i32>,
        /// `exit`, `stopped`, `idle` or `lost` (the host lost it).
        reason: String,
    },
    SessionId {
        session_id: String,
    },
    /// Claude Code's first message: what it can do in this session.
    Init {
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        permission_mode: Option<String>,
        #[serde(default)]
        slash_commands: Vec<String>,
    },
    UserMessage {
        text: String,
    },
    Say {
        text: String,
    },
    Plan {
        items: Vec<PlanItem>,
    },
    Step {
        id: String,
        step_kind: StepKind,
        tool: String,
        title: String,
        #[serde(default)]
        detail: String,
        /// The step (a subagent call) this one nests under.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
    },
    StepDone {
        id: String,
        ok: bool,
        #[serde(default)]
        output: Option<Excerpt>,
    },
    /// A tool call that never got a result because the agent was cut off.
    CutOff {
        step: String,
        title: String,
        outbound: bool,
    },
    FilesTouched {
        files: Vec<FileStat>,
    },
    Request {
        request: RequestInfo,
    },
    RequestSnoozed {
        request: String,
    },
    RequestAnswered {
        request: String,
        answer: AnswerKind,
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        always: bool,
    },
    RequestWithdrawn {
        request: String,
        reason: String,
    },
    /// Settled by Sphere's own rules, without asking.
    AutoAnswered {
        tool: String,
        allow: bool,
        rule: String,
    },
    ModeChanged {
        mode: String,
    },
    ModelChanged {
        model: String,
    },
    /// One turn finished (Claude Code's `result`).
    Turn {
        ok: bool,
        #[serde(default)]
        interrupted: bool,
        #[serde(default)]
        summary: String,
        #[serde(default)]
        turns: Option<u64>,
        #[serde(default)]
        duration_ms: Option<u64>,
        /// Cumulative cost reported by this agent process.
        #[serde(default)]
        cost_usd: Option<f64>,
        /// Cost added by this turn (the change in the cumulative figure).
        #[serde(default)]
        cost_delta_usd: f64,
    },
    Stopped {},
    Failed {
        message: String,
    },
    Warning {
        message: String,
    },
    /// The session was running outside Sphere and was stopped there and moved here.
    TakenOver {
        /// Where it ran: `herdr`, `Terminal`, `Claude background`, ...
        from: String,
    },
    /// A note was added or removed. It belongs to no task and holds no text: the notes are in the
    /// snapshot and `memory.list`, and this tells a client to read them again.
    MemoryChanged {
        /// How many notes there are now, in all.
        #[serde(default)]
        notes: u32,
    },
    /// A protocol was written, changed or ran (task is empty). It replaces what was saved under its id.
    ProtocolSaved {
        protocol: Protocol,
    },
    /// A protocol was deleted (task is empty).
    ProtocolRemoved {
        id: String,
    },
}

impl EventBody {
    pub fn kind_name(&self) -> &'static str {
        match self {
            EventBody::TaskCreated { .. } => "task_created",
            EventBody::WorkspaceReady { .. } => "workspace_ready",
            EventBody::AgentStarted { .. } => "agent_started",
            EventBody::AgentExited { .. } => "agent_exited",
            EventBody::SessionId { .. } => "session_id",
            EventBody::Init { .. } => "init",
            EventBody::UserMessage { .. } => "user_message",
            EventBody::Say { .. } => "say",
            EventBody::Plan { .. } => "plan",
            EventBody::Step { .. } => "step",
            EventBody::StepDone { .. } => "step_done",
            EventBody::CutOff { .. } => "cut_off",
            EventBody::FilesTouched { .. } => "files_touched",
            EventBody::Request { .. } => "request",
            EventBody::RequestSnoozed { .. } => "request_snoozed",
            EventBody::RequestAnswered { .. } => "request_answered",
            EventBody::RequestWithdrawn { .. } => "request_withdrawn",
            EventBody::AutoAnswered { .. } => "auto_answered",
            EventBody::ModeChanged { .. } => "mode_changed",
            EventBody::ModelChanged { .. } => "model_changed",
            EventBody::Turn { .. } => "turn",
            EventBody::Stopped {} => "stopped",
            EventBody::Failed { .. } => "failed",
            EventBody::Warning { .. } => "warning",
            EventBody::TakenOver { .. } => "taken_over",
            EventBody::MemoryChanged { .. } => "memory_changed",
            EventBody::ProtocolSaved { .. } => "protocol_saved",
            EventBody::ProtocolRemoved { .. } => "protocol_removed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanItem {
    pub text: String,
    /// `pending`, `in_progress` or `completed`.
    pub status: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    Read,
    Search,
    Edit,
    Run,
    Web,
    Delegate,
    Plan,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Queued,
    Working,
    NeedsYou,
    Done,
    Failed,
    Stopped,
}

impl TaskState {
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskState::Queued => "queued",
            TaskState::Working => "working",
            TaskState::NeedsYou => "needs_you",
            TaskState::Done => "done",
            TaskState::Failed => "failed",
            TaskState::Stopped => "stopped",
        }
    }

    /// Tasks in these states hold a concurrency slot.
    pub fn is_active(&self) -> bool {
        matches!(self, TaskState::Working | TaskState::NeedsYou)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileStat {
    pub path: String,
    pub added: u64,
    pub removed: u64,
}

/// Where the full text of a big output lives: a line in the host's output file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputRef {
    pub child: String,
    pub offset: u64,
    pub len: u64,
}

/// A short view of a tool's output: all of it when small, else head and tail.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Excerpt {
    pub head: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tail: Option<String>,
    /// Size of the full output in bytes.
    pub bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<OutputRef>,
}

impl Excerpt {
    /// Keep `text` whole when it fits in `limit` bytes, else its first and
    /// last `limit / 2` bytes (cut on character boundaries).
    pub fn new(text: &str, limit: usize, source: Option<OutputRef>) -> Excerpt {
        let bytes = text.len() as u64;
        if text.len() <= limit {
            return Excerpt {
                head: text.to_string(),
                tail: None,
                bytes,
                source: None,
            };
        }
        let half = limit / 2;
        let mut h = half;
        while !text.is_char_boundary(h) {
            h -= 1;
        }
        let mut t = text.len() - half;
        while !text.is_char_boundary(t) {
            t += 1;
        }
        Excerpt {
            head: text[..h].to_string(),
            tail: Some(text[t..].to_string()),
            bytes,
            source,
        }
    }
}

const AT_FORMAT: &[FormatItem<'static>] =
    format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z");

/// A time given as milliseconds since the Unix epoch, in the `at` format.
pub fn rfc3339_from_unix_ms(ms: u64) -> String {
    OffsetDateTime::from_unix_timestamp_nanos(ms as i128 * 1_000_000)
        .ok()
        .and_then(|t| t.format(AT_FORMAT).ok())
        .unwrap_or_else(|| "1970-01-01T00:00:00.000Z".to_string())
}

/// The current time in the `at` format.
pub fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(AT_FORMAT)
        .unwrap_or_else(|_| "1970-01-01T00:00:00.000Z".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_wire_format_is_flat() {
        let ev = Event {
            seq: 3,
            task: "t1".into(),
            at: "2026-01-01T00:00:00.000Z".into(),
            body: EventBody::Step {
                id: "toolu_1".into(),
                step_kind: StepKind::Run,
                tool: "Bash".into(),
                title: "Run ls".into(),
                detail: String::new(),
                parent: None,
            },
        };
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["kind"], "step");
        assert_eq!(v["step_kind"], "run");
        assert_eq!(v["seq"], 3);
        assert!(v.get("parent").is_none());
        let back: Event = serde_json::from_value(v).unwrap();
        assert_eq!(back, ev);
    }

    #[test]
    fn turn_defaults() {
        let raw = r#"{"seq":9,"task":"t1","at":"x","kind":"turn","ok":true,"cost_usd":0.5}"#;
        let ev: Event = serde_json::from_str(raw).unwrap();
        match ev.body {
            EventBody::Turn {
                ok,
                cost_usd,
                turns,
                interrupted,
                cost_delta_usd,
                ..
            } => {
                assert!(ok);
                assert_eq!(cost_usd, Some(0.5));
                assert_eq!(turns, None);
                assert!(!interrupted);
                assert_eq!(cost_delta_usd, 0.0);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn an_agent_started_from_before_notes_was_briefed_with_none() {
        let raw = r#"{"seq":4,"task":"t1","at":"x","kind":"agent_started","child":"c1","pid":9,"resumed":true}"#;
        let ev: Event = serde_json::from_str(raw).unwrap();
        assert!(matches!(
            ev.body,
            EventBody::AgentStarted { briefed: 0, .. }
        ));
    }

    #[test]
    fn unix_ms_formats() {
        assert_eq!(rfc3339_from_unix_ms(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(
            rfc3339_from_unix_ms(1_700_000_000_123),
            "2023-11-14T22:13:20.123Z"
        );
    }

    #[test]
    fn timestamps_are_fixed_width() {
        let a = now_rfc3339();
        assert_eq!(a.len(), 24, "{a}");
        assert!(a.ends_with('Z'));
    }

    #[test]
    fn excerpt_keeps_small_text_and_cuts_big_text() {
        let e = Excerpt::new("hello", 100, None);
        assert_eq!(e.head, "hello");
        assert!(e.tail.is_none() && e.source.is_none());

        let big = "é".repeat(500);
        let src = OutputRef {
            child: "c1".into(),
            offset: 0,
            len: 1,
        };
        let e = Excerpt::new(&big, 101, Some(src.clone()));
        assert_eq!(e.bytes, 1000);
        assert!(e.head.len() <= 50 && e.tail.as_ref().unwrap().len() <= 50);
        assert_eq!(e.source, Some(src));
        assert!(e.head.chars().all(|c| c == 'é'));
    }

    #[test]
    fn every_body_has_a_kind_name_matching_its_tag() {
        let bodies = vec![
            EventBody::Stopped {},
            EventBody::Warning {
                message: "w".into(),
            },
            EventBody::SessionId {
                session_id: "s".into(),
            },
            EventBody::MemoryChanged { notes: 3 },
            EventBody::AgentStarted {
                child: "c".into(),
                pid: 1,
                resumed: false,
                briefed: 2,
            },
            EventBody::ProtocolRemoved { id: "p1".into() },
            EventBody::ProtocolSaved {
                protocol: crate::protocol::Protocol {
                    id: "p1".into(),
                    text: "in 2 hours sphere: x".into(),
                    schedule: crate::protocol::Schedule::Once { at: 9 },
                    repo: "/r".into(),
                    prompt: "x".into(),
                    enabled: true,
                    created_at: 1,
                    last_run: None,
                    next_run: Some(9),
                },
            },
        ];
        for b in bodies {
            let v = serde_json::to_value(&b).unwrap();
            assert_eq!(v["kind"], b.kind_name());
            let back: EventBody = serde_json::from_value(v).unwrap();
            assert_eq!(back, b);
        }
    }

    #[test]
    fn a_task_made_before_protocols_has_none() {
        let raw = r#"{"seq":1,"task":"t1","at":"x","kind":"task_created","title":"t","prompt":"p","repo":"/r","budget_usd":1.0}"#;
        let ev: Event = serde_json::from_str(raw).unwrap();
        assert!(matches!(
            ev.body,
            EventBody::TaskCreated { protocol: None, .. }
        ));
        // And a task that no protocol started doesn't carry the field.
        let v = serde_json::to_value(&ev).unwrap();
        assert!(v.get("protocol").is_none());
    }
}
