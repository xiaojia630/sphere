//! Wire types of the host socket. A request is `{"id":1,"method":"spawn",
//! "params":{..}}` and the reply `{"id":1,"result":..}` or `{"id":1,"error":".."}`.
//! `follow` is the exception: after its reply the connection carries
//! [`FollowEvent`] lines until the child exits.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub mod method {
    pub const PING: &str = "ping";
    pub const SPAWN: &str = "spawn";
    pub const WRITE: &str = "write";
    pub const CLOSE_STDIN: &str = "close_stdin";
    pub const RESIZE: &str = "resize";
    pub const SIGNAL: &str = "signal";
    pub const LIST: &str = "list";
    pub const READ: &str = "read";
    pub const FOLLOW: &str = "follow";
    pub const FORGET: &str = "forget";
    pub const SHUTDOWN: &str = "shutdown";
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtySize {
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SpawnParams {
    pub cmd: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    /// Added to the host's own environment.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Run on a PTY of this size instead of pipes.
    #[serde(default)]
    pub pty: Option<PtySize>,
    /// Free text the caller can use to find its children again.
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnResult {
    pub id: String,
    pub pid: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdParams {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteParams {
    pub id: String,
    /// Base64.
    pub bytes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResizeParams {
    pub id: String,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalParams {
    pub id: String,
    /// A signal number or name (`"TERM"`).
    pub sig: Value,
    /// Signal the whole process group.
    #[serde(default)]
    pub group: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stream {
    #[default]
    Out,
    Err,
}

impl Stream {
    pub fn ext(self) -> &'static str {
        match self {
            Stream::Out => "out",
            Stream::Err => "err",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadParams {
    pub id: String,
    #[serde(default)]
    pub stream: Stream,
    #[serde(default)]
    pub from_offset: u64,
    #[serde(default)]
    pub max: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadResult {
    /// Base64.
    pub data: String,
    /// Where the next read starts.
    pub offset: u64,
    /// The child has exited and everything it printed has been read.
    pub eof: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExitStatus {
    #[serde(default)]
    pub code: Option<i32>,
    #[serde(default)]
    pub signal: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChildInfo {
    pub id: String,
    pub pid: u32,
    #[serde(default)]
    pub label: Option<String>,
    pub cmd: String,
    #[serde(default)]
    pub cwd: Option<String>,
    pub pty: bool,
    pub running: bool,
    #[serde(default)]
    pub exit: Option<ExitStatus>,
    pub out_len: u64,
    pub err_len: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PingResult {
    pub pid: u32,
    pub version: String,
}

/// Lines on a `follow` connection after its reply.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum FollowEvent {
    Data {
        /// Offset of the first byte in `data`.
        offset: u64,
        /// Base64.
        data: String,
    },
    Exit {
        #[serde(flatten)]
        status: ExitStatus,
    },
}

/// Signal names accepted in [`SignalParams`].
pub fn signal_number(v: &Value) -> Option<i32> {
    if let Some(n) = v.as_i64() {
        return i32::try_from(n).ok();
    }
    let name = v.as_str()?.trim_start_matches("SIG").to_ascii_uppercase();
    Some(match name.as_str() {
        "HUP" => libc::SIGHUP,
        "INT" => libc::SIGINT,
        "QUIT" => libc::SIGQUIT,
        "KILL" => libc::SIGKILL,
        "TERM" => libc::SIGTERM,
        "USR1" => libc::SIGUSR1,
        "USR2" => libc::SIGUSR2,
        "STOP" => libc::SIGSTOP,
        "CONT" => libc::SIGCONT,
        "WINCH" => libc::SIGWINCH,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn signal_names_and_numbers() {
        assert_eq!(signal_number(&json!("TERM")), Some(libc::SIGTERM));
        assert_eq!(signal_number(&json!("SIGKILL")), Some(libc::SIGKILL));
        assert_eq!(signal_number(&json!(2)), Some(2));
        assert_eq!(signal_number(&json!("nope")), None);
    }

    #[test]
    fn follow_events_are_tagged() {
        let e = FollowEvent::Exit {
            status: ExitStatus {
                code: Some(3),
                signal: None,
            },
        };
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["event"], "exit");
        assert_eq!(v["code"], 3);
        let back: FollowEvent = serde_json::from_value(v).unwrap();
        assert!(matches!(back, FollowEvent::Exit { status } if status.code == Some(3)));
    }

    #[test]
    fn spawn_params_defaults() {
        let p: SpawnParams = serde_json::from_str(r#"{"cmd":"ls"}"#).unwrap();
        assert!(p.args.is_empty() && p.pty.is_none() && p.env.is_empty());
    }
}
