use serde::{Deserialize, Serialize};
use serde_json::Value;

/// What a request is asking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestKind {
    /// A tool permission (`can_use_tool`).
    Permission,
    /// `AskUserQuestion`.
    Question,
    /// `ExitPlanMode`.
    Plan,
    /// A cut-off outbound tool call: continue after a restart?
    ResumeCheck,
}

impl RequestKind {
    pub fn for_tool(tool: &str) -> Self {
        match tool {
            "AskUserQuestion" => RequestKind::Question,
            "ExitPlanMode" => RequestKind::Plan,
            _ => RequestKind::Permission,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnswerKind {
    Yes,
    Change,
    Later,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestStatus {
    Open,
    /// Answered `later`: still open, the agent stays paused.
    Snoozed,
    Answered,
    Withdrawn,
}

impl RequestStatus {
    pub fn is_open(self) -> bool {
        matches!(self, RequestStatus::Open | RequestStatus::Snoozed)
    }
}

/// Everything a client needs to draw a request card.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestInfo {
    pub id: String,
    pub task: String,
    pub kind: RequestKind,
    pub tool: String,
    #[serde(default)]
    pub display_name: Option<String>,
    /// The agent's own description of the action.
    #[serde(default)]
    pub description: Option<String>,
    /// The agent's own summary: its description, or what it last said.
    #[serde(default)]
    pub summary: String,
    /// The exact tool input. Cards show this, never only the description.
    #[serde(default)]
    pub input: Value,
    #[serde(default)]
    pub suggestions: Vec<Value>,
    #[serde(default)]
    pub blocked_path: Option<String>,
    #[serde(default)]
    pub tool_use_id: Option<String>,
    /// Claude Code's own id for the ask; the answer is sent back under it.
    #[serde(default)]
    pub agent_request: Option<String>,
    /// The earlier request this one replaces (after a `change` answer).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revises: Option<String>,
}
