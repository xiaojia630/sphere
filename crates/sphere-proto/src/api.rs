//! The sphered API: JSON-RPC 2.0 over newline-delimited JSON on a Unix socket.

use crate::event::{Event, FileStat, TaskState};
use crate::request::{AnswerKind, RequestStatus};
use crate::views::{RequestView, TaskView};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Method names.
pub mod method {
    pub const HELLO: &str = "hello";
    pub const SNAPSHOT: &str = "snapshot";
    pub const SUBSCRIBE: &str = "subscribe";
    pub const HISTORY: &str = "history";
    pub const BLOB_READ: &str = "blob.read";
    pub const STATUS: &str = "status";
    pub const TASK_START: &str = "task.start";
    pub const TASK_SEND: &str = "task.send";
    pub const TASK_INTERRUPT: &str = "task.interrupt";
    pub const TASK_SET_MODE: &str = "task.set_mode";
    pub const TASK_SET_MODEL: &str = "task.set_model";
    pub const TASK_REWIND: &str = "task.rewind";
    pub const TASK_STOP: &str = "task.stop";
    pub const SESSION_LIST: &str = "session.list";
    pub const SESSION_BRING_IN: &str = "session.bring_in";
    pub const REQUEST_ANSWER: &str = "request.answer";
}

/// Notification methods the server pushes on a subscribed connection.
pub mod notify {
    pub const EVENT: &str = "event";
    pub const LIVE: &str = "live";
}

/// JSON-RPC error codes. Application errors share `APP` and say what went
/// wrong in `data.kind`.
pub mod code {
    pub const PARSE: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL: i64 = -32603;
    pub const APP: i64 = -32000;
}

/// `data.kind` values of application errors.
pub mod kind {
    pub const NOT_FOUND: &str = "not_found";
    pub const ALREADY_ANSWERED: &str = "already_answered";
    pub const WITHDRAWN: &str = "withdrawn";
    pub const FORBIDDEN: &str = "forbidden";
    pub const NOT_IMPLEMENTED: &str = "not_implemented";
    pub const LAGGED: &str = "lagged";
    pub const INVALID_STATE: &str = "invalid_state";
    pub const SESSION_RUNNING: &str = "session_running";
    pub const DAILY_CAP: &str = "daily_cap";
    pub const BAD_REPO: &str = "bad_repo";
    pub const AGENT_ERROR: &str = "agent_error";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcRequest {
    pub jsonrpc: String,
    pub id: Value,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(default)]
    pub data: Option<Value>,
}

impl RpcError {
    pub fn app(kind: &str, message: impl Into<String>) -> Self {
        RpcError {
            code: code::APP,
            message: message.into(),
            data: Some(serde_json::json!({ "kind": kind })),
        }
    }

    pub fn kind(&self) -> Option<&str> {
        self.data.as_ref()?.get("kind")?.as_str()
    }
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.kind() {
            Some(k) => write!(f, "{} ({k})", self.message),
            None => write!(f, "{}", self.message),
        }
    }
}

/// Any message the server can send: a response (has `id`) or a notification
/// (has `method`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcMessage {
    pub jsonrpc: String,
    #[serde(default)]
    pub id: Option<Value>,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub params: Option<Value>,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<RpcError>,
}

// ---- params and results ----------------------------------------------------

/// Who is on the other end. `assistant` can never answer requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientKind {
    Cli,
    App,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelloParams {
    pub client: ClientKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub seq: u64,
    pub tasks: Vec<TaskView>,
    /// Requests waiting on the human (open or snoozed).
    pub requests: Vec<RequestView>,
    pub cost_today_usd: f64,
    #[serde(default)]
    pub daily_cap_usd: Option<f64>,
    /// How many agents run at once; the rest wait queued. `None` from an older sphered.
    #[serde(default)]
    pub max_concurrent: Option<usize>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SubscribeParams {
    /// Replay durable events with `seq` greater than this, then go live.
    #[serde(default)]
    pub since: u64,
    /// Only this task's events (and live frames).
    #[serde(default)]
    pub task: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscribeResult {
    /// The newest durable seq at subscription time.
    pub seq: u64,
}

/// A live frame: typing deltas and the like. Never stored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiveFrame {
    pub task: String,
    pub text: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HistoryParams {
    #[serde(default)]
    pub task: Option<String>,
    /// Events older than this seq (default: the newest).
    #[serde(default)]
    pub before: Option<u64>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryResult {
    /// Ascending by seq.
    pub events: Vec<Event>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlobReadParams {
    pub child: String,
    #[serde(default)]
    pub offset: u64,
    pub len: u64,
    /// The output reference is a stdout line; pull this tool call's result text
    /// out of it.
    #[serde(default)]
    pub tool_use_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlobReadResult {
    pub text: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StartParams {
    pub repo: String,
    pub prompt: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub in_place: bool,
    #[serde(default)]
    pub budget_usd: Option<f64>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    /// Do not deny force pushes and history rewrites for this task.
    #[serde(default)]
    pub allow_rewrites: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartResult {
    pub task: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendParams {
    pub task: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskParams {
    pub task: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetModeParams {
    pub task: String,
    /// `default`, `acceptEdits`, `plan`, ...
    pub mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetModelParams {
    pub task: String,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnswerParams {
    pub request: String,
    pub answer: AnswerKind,
    /// For `change`: what to tell the agent. For a question: the answer.
    #[serde(default)]
    pub text: Option<String>,
    /// For `yes`: the edited tool input.
    #[serde(default)]
    pub input: Option<Value>,
    /// For `yes`: also save the suggested rule.
    #[serde(default)]
    pub always: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ack {
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub session_id: String,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    /// Still running outside Sphere.
    pub running: bool,
    #[serde(default)]
    pub pid: Option<u32>,
    /// Already a Sphere task.
    #[serde(default)]
    pub task: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BringInParams {
    pub session_id: String,
    /// Where to run it; defaults to the session's own folder.
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub prompt: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskLine {
    pub id: String,
    pub title: String,
    pub state: TaskState,
    pub status: String,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Status {
    pub seq: u64,
    pub version: String,
    pub active: usize,
    pub queued: usize,
    pub needs_you: usize,
    pub cost_today_usd: f64,
    #[serde(default)]
    pub daily_cap_usd: Option<f64>,
    pub tasks: Vec<TaskLine>,
    pub requests: Vec<RequestLine>,
    pub host_pid: Option<u32>,
    /// `claude --version` output, when checked.
    #[serde(default)]
    pub claude_version: Option<String>,
    #[serde(default)]
    pub signed_in: Option<bool>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestLine {
    pub id: String,
    pub task: String,
    pub status: RequestStatus,
    pub summary: String,
}

/// Touched files, as `git diff --stat` reports them.
pub type Files = Vec<FileStat>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_carry_a_kind() {
        let e = RpcError::app(kind::ALREADY_ANSWERED, "request r1 was already answered");
        assert_eq!(e.kind(), Some("already_answered"));
        assert_eq!(e.code, code::APP);
        let back: RpcError = serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
        assert_eq!(back.kind(), Some("already_answered"));
    }

    #[test]
    fn answer_params_defaults() {
        let p: AnswerParams = serde_json::from_str(r#"{"request":"r1","answer":"yes"}"#).unwrap();
        assert_eq!(p.answer, AnswerKind::Yes);
        assert!(!p.always && p.text.is_none() && p.input.is_none());
    }

    #[test]
    fn rpc_message_parses_both_shapes() {
        let r: RpcMessage =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":1,"result":{"a":1}}"#).unwrap();
        assert!(r.method.is_none() && r.result.is_some());
        let n: RpcMessage = serde_json::from_str(
            r#"{"jsonrpc":"2.0","method":"live","params":{"task":"t1","text":"x"}}"#,
        )
        .unwrap();
        assert_eq!(n.method.as_deref(), Some("live"));
    }
}
