use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::format_description::FormatItem;
use time::macros::format_description;
use time::OffsetDateTime;

/// One thing that happened in a task. `seq` is per task and monotonic, so a
/// subscriber can ask for everything after the last `seq` it saw.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub task: String,
    pub seq: u64,
    /// RFC 3339, UTC, millisecond precision (fixed width, so it sorts as text).
    pub at: String,
    #[serde(flatten)]
    pub body: EventBody,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventBody {
    TaskStarted {
        title: String,
        repo: String,
        workdir: String,
        #[serde(default)]
        branch: Option<String>,
    },
    Plan {
        items: Vec<PlanItem>,
    },
    Step {
        id: String,
        #[serde(rename = "step_kind")]
        kind: StepKind,
        title: String,
        #[serde(default)]
        detail: String,
    },
    Output {
        step: String,
        text: String,
        #[serde(default)]
        is_error: bool,
    },
    StepDone {
        id: String,
        ok: bool,
        #[serde(default)]
        summary: String,
    },
    FileTouched {
        path: String,
        added: u64,
        removed: u64,
    },
    Say {
        text: String,
    },
    NeedsYou {
        request: String,
        tool: String,
        #[serde(default)]
        input: Value,
        #[serde(default)]
        summary: String,
    },
    Resolved {
        request: String,
        decision: Decision,
        #[serde(default)]
        note: Option<String>,
    },
    Finished {
        ok: bool,
        #[serde(default)]
        summary: String,
        #[serde(default)]
        turns: Option<u64>,
        #[serde(default)]
        duration_ms: Option<u64>,
        #[serde(default)]
        cost_usd: Option<f64>,
        /// True when the task ended because it was stopped, not because it failed.
        #[serde(default, skip_serializing_if = "is_false")]
        stopped: bool,
    },
    Error {
        message: String,
    },
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl EventBody {
    pub fn kind_name(&self) -> &'static str {
        match self {
            EventBody::TaskStarted { .. } => "task_started",
            EventBody::Plan { .. } => "plan",
            EventBody::Step { .. } => "step",
            EventBody::Output { .. } => "output",
            EventBody::StepDone { .. } => "step_done",
            EventBody::FileTouched { .. } => "file_touched",
            EventBody::Say { .. } => "say",
            EventBody::NeedsYou { .. } => "needs_you",
            EventBody::Resolved { .. } => "resolved",
            EventBody::Finished { .. } => "finished",
            EventBody::Error { .. } => "error",
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Allow,
    Deny,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Working,
    NeedsYou,
    Done,
    Failed,
    Stopped,
}

impl TaskState {
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskState::Working => "working",
            TaskState::NeedsYou => "needs_you",
            TaskState::Done => "done",
            TaskState::Failed => "failed",
            TaskState::Stopped => "stopped",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskSummary {
    pub id: String,
    pub title: String,
    pub state: TaskState,
    pub status: String,
    pub started_at: String,
    #[serde(default)]
    pub repo: String,
    #[serde(default)]
    pub workdir: String,
    #[serde(default)]
    pub branch: Option<String>,
}

const AT_FORMAT: &[FormatItem<'static>] =
    format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z");

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
    fn event_wire_format() {
        let ev = Event {
            task: "t_1".into(),
            seq: 3,
            at: "2026-01-01T00:00:00.000Z".into(),
            body: EventBody::Step {
                id: "toolu_1".into(),
                kind: StepKind::Run,
                title: "Run ls".into(),
                detail: String::new(),
            },
        };
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["kind"], "step");
        assert_eq!(v["step_kind"], "run");
        assert_eq!(v["seq"], 3);
        let back: Event = serde_json::from_value(v).unwrap();
        assert_eq!(back, ev);
    }

    #[test]
    fn finished_roundtrip_and_defaults() {
        let raw = r#"{"task":"t_1","seq":9,"at":"x","kind":"finished","ok":true,"cost_usd":0.5}"#;
        let ev: Event = serde_json::from_str(raw).unwrap();
        match ev.body {
            EventBody::Finished {
                ok,
                cost_usd,
                turns,
                stopped,
                ..
            } => {
                assert!(ok);
                assert_eq!(cost_usd, Some(0.5));
                assert_eq!(turns, None);
                assert!(!stopped);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn timestamps_are_fixed_width() {
        let a = now_rfc3339();
        assert_eq!(a.len(), 24, "{a}");
        assert!(a.ends_with('Z'));
    }
}
