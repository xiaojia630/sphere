//! Sphere's own tools for agents: start helper agents, wait for them, read
//! their results, follow up and stop them. Claude Code reaches them as an
//! in-process MCP server named `sphere` over the agent's own pipe.
//!
//! This module is the pure part: routing the JSON-RPC messages, validating
//! arguments, the limits, and the wait. The manager runs the calls.

use crate::agent_proto::SPHERE_TOOL_PREFIX;
use crate::hub::Hub;
use serde_json::{json, Value};
use sphere_proto::{TaskState, TaskView, Views};
use std::collections::HashSet;
use std::time::{Duration, Instant};

/// Helpers of one parent that may work at once.
pub const MAX_WORKING: usize = 4;
/// Helpers one parent may start in all.
pub const MAX_TOTAL: usize = 8;
pub const DEFAULT_WAIT_SECS: u64 = 300;
pub const MAX_WAIT_SECS: u64 = 900;

/// The longest a report quotes of a helper's summary.
const SUMMARY_CHARS: usize = 6000;

// ---- routing -----------------------------------------------------------------

/// A `tools/call` for the manager to run.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    /// The control request to answer.
    pub request_id: String,
    /// The JSON-RPC id to answer with.
    pub rpc_id: Value,
    pub name: String,
    pub args: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Routed {
    /// A line to write back at once.
    Write(String),
    Call(ToolCall),
}

fn rpc_result(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn rpc_error(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// The reply to a `tools/call`: text for the model, and whether it failed.
pub fn call_reply(call: &ToolCall, text: &str, is_error: bool) -> String {
    crate::agent_proto::mcp_response(
        &call.request_id,
        rpc_result(
            &call.rpc_id,
            json!({"content": [{"type": "text", "text": text}], "isError": is_error}),
        ),
    )
}

/// Decide what to do about one message for the `sphere` server. `task` is the
/// task whose agent sent it: what a helper is offered differs from a parent.
pub fn route(task: &TaskView, request_id: &str, message: &Value) -> Routed {
    let reply = |rpc: Value| Routed::Write(crate::agent_proto::mcp_response(request_id, rpc));
    let method = message.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let Some(id) = message.get("id").filter(|i| !i.is_null()) else {
        // A notification has no answer; the control request still wants one.
        return reply(rpc_result(&json!(0), json!({})));
    };
    match method {
        "initialize" => {
            let version = message
                .pointer("/params/protocolVersion")
                .and_then(|v| v.as_str())
                .unwrap_or("2024-11-05");
            reply(rpc_result(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "sphere", "version": env!("CARGO_PKG_VERSION")},
                }),
            ))
        }
        "ping" => reply(rpc_result(id, json!({}))),
        "tools/list" => reply(rpc_result(
            id,
            json!({"tools": tools(task.parent_task.is_some())}),
        )),
        "tools/call" => Routed::Call(ToolCall {
            request_id: request_id.to_string(),
            rpc_id: id.clone(),
            name: message
                .pointer("/params/name")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string(),
            args: message
                .pointer("/params/arguments")
                .cloned()
                .filter(|a| !a.is_null())
                .unwrap_or_else(|| json!({})),
        }),
        other => reply(rpc_error(id, -32601, &format!("method not found: {other}"))),
    }
}

/// The tools an agent is offered. A helper cannot start helpers.
pub fn tools(is_helper: bool) -> Vec<Value> {
    let mut v = vec![];
    if !is_helper {
        v.push(json!({
            "name": "start_helper",
            "description": "Start a helper agent for a separate piece of work you can do in parallel, such as research, a review or a trial of one approach. It works in its own git worktree of this repository, starting from the checked-out commit (not your uncommitted changes), and sees only the prompt, so make it self-contained. Returns its id. Its permission requests go to the human, not to you. Use wait_for_helpers to get its result. At most 4 helpers work at once and 8 in all; helpers cannot start helpers.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": {"type": "string", "description": "A few words naming the work."},
                    "prompt": {"type": "string", "description": "Everything the helper needs to do the work and report back."},
                },
                "required": ["title", "prompt"],
            },
        }));
    }
    v.push(json!({
        "name": "wait_for_helpers",
        "description": "Wait for your helpers to finish and get what each reported, with its branch and changed files. Waits for all of them unless `helpers` lists some. Returns early when a helper needs the human to answer a request (the human answers it; you cannot), and after `timeout_seconds` with whatever is still working. Call it again to keep waiting.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "helpers": {"type": "array", "items": {"type": "string"}, "description": "Helper ids. Default: all of yours."},
                "timeout_seconds": {"type": "number", "description": "Default 300, at most 900."},
            },
        },
    }));
    v.push(json!({
        "name": "message_helper",
        "description": "Send a follow-up to one of your helpers. If it has finished, this wakes it to continue. Use wait_for_helpers afterwards for its answer.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "helper": {"type": "string", "description": "The helper's id."},
                "text": {"type": "string", "description": "What to tell it."},
            },
            "required": ["helper", "text"],
        },
    }));
    v.push(json!({
        "name": "stop_helper",
        "description": "Stop one of your helpers that is no longer needed.",
        "inputSchema": {
            "type": "object",
            "properties": {"helper": {"type": "string", "description": "The helper's id."}},
            "required": ["helper"],
        },
    }));
    v
}

// ---- arguments -----------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum Tool {
    Start {
        title: String,
        prompt: String,
    },
    Wait {
        /// None: all of the parent's helpers.
        helpers: Option<Vec<String>>,
        timeout: Duration,
    },
    Message {
        helper: String,
        text: String,
    },
    Stop {
        helper: String,
    },
}

fn string_arg(args: &Value, key: &str) -> Result<String, String> {
    match args.get(key) {
        Some(Value::String(s)) if !s.trim().is_empty() => Ok(s.trim().to_string()),
        Some(Value::String(_)) | None | Some(Value::Null) => {
            Err(format!("`{key}` is required and must not be empty"))
        }
        Some(_) => Err(format!("`{key}` must be a string")),
    }
}

/// Check a call's name and arguments. The error is for the model to read.
pub fn parse_tool(name: &str, args: &Value, is_helper: bool) -> Result<Tool, String> {
    let short = name.strip_prefix(SPHERE_TOOL_PREFIX).unwrap_or(name);
    match short {
        "start_helper" if is_helper => {
            Err("Helpers cannot start helpers. Do the work yourself, or say what you need in your result.".into())
        }
        "start_helper" => {
            let title = string_arg(args, "title")?;
            Ok(Tool::Start {
                title: title.chars().take(100).collect(),
                prompt: string_arg(args, "prompt")?,
            })
        }
        "wait_for_helpers" => {
            let helpers = match args.get("helpers") {
                None | Some(Value::Null) => None,
                Some(Value::Array(a)) => {
                    let ids = a
                        .iter()
                        .map(|v| v.as_str().map(|s| s.trim().to_string()))
                        .collect::<Option<Vec<_>>>()
                        .filter(|ids| ids.iter().all(|i| !i.is_empty()))
                        .ok_or("`helpers` must be a list of helper ids")?;
                    // An empty list means all of them, as leaving it out does.
                    (!ids.is_empty()).then_some(ids)
                }
                Some(_) => return Err("`helpers` must be a list of helper ids".into()),
            };
            let secs = match args.get("timeout_seconds") {
                None | Some(Value::Null) => DEFAULT_WAIT_SECS as f64,
                Some(v) => v
                    .as_f64()
                    .filter(|s| s.is_finite())
                    .ok_or("`timeout_seconds` must be a number")?,
            };
            let secs = secs.clamp(1.0, MAX_WAIT_SECS as f64);
            Ok(Tool::Wait {
                helpers,
                timeout: Duration::from_secs_f64(secs),
            })
        }
        "message_helper" => Ok(Tool::Message {
            helper: string_arg(args, "helper")?,
            text: string_arg(args, "text")?,
        }),
        "stop_helper" => Ok(Tool::Stop {
            helper: string_arg(args, "helper")?,
        }),
        other => Err(format!("unknown tool `{other}`")),
    }
}

// ---- limits ----------------------------------------------------------------------

pub fn is_working(t: &TaskView) -> bool {
    matches!(
        t.state,
        TaskState::Queued | TaskState::Working | TaskState::NeedsYou
    )
}

/// May this task start another helper?
pub fn check_start(views: &Views, parent: &str) -> Result<(), String> {
    let Some(p) = views.task(parent) else {
        return Err("unknown task".into());
    };
    if p.parent_task.is_some() {
        return Err("Helpers cannot start helpers.".into());
    }
    let helpers = views.helpers_of(parent);
    if helpers.len() >= MAX_TOTAL {
        return Err(format!(
            "You have already started {MAX_TOTAL} helpers, which is the most for one task. Message a finished helper to give it more work."
        ));
    }
    if helpers.iter().filter(|h| is_working(h)).count() >= MAX_WORKING {
        return Err(format!(
            "{MAX_WORKING} helpers are already working, which is the most at once. Use wait_for_helpers, then start more."
        ));
    }
    Ok(())
}

/// A helper of this parent, by id. A parent may only touch its own.
pub fn own_helper<'a>(views: &'a Views, parent: &str, id: &str) -> Result<&'a TaskView, String> {
    match views.task(id) {
        Some(t) if t.parent_task.as_deref() == Some(parent) => Ok(t),
        _ => Err(format!("{id} is not one of your helpers.")),
    }
}

/// An identical helper (same title and prompt) of this parent that is still
/// working: what a call read again after a restart finds, so it starts nothing twice.
pub fn already_started<'a>(
    views: &'a Views,
    parent: &str,
    title: &str,
    prompt: &str,
) -> Option<&'a TaskView> {
    views
        .helpers_of(parent)
        .into_iter()
        .find(|h| is_working(h) && h.title == title && h.prompt == prompt)
}

// ---- waiting ---------------------------------------------------------------------

/// Where a wait stands.
#[derive(Debug, Clone, PartialEq)]
pub enum Wait {
    Pending,
    /// Every listed helper is finished.
    Finished,
    /// A helper waits on a request only the human can answer.
    NeedsHuman {
        helper: String,
        request: String,
        status: String,
    },
    /// The parent itself is no longer working: nobody wants the answer.
    ParentGone,
}

/// Look at the helpers. Requests in `reported` were already handed to the
/// agent once; waiting again does not return for them a second time, or an
/// agent that calls straight back would spin until the human answers.
pub fn wait_state(views: &Views, parent: &str, ids: &[String], reported: &HashSet<String>) -> Wait {
    let parent_working = views
        .task(parent)
        .is_some_and(|p| matches!(p.state, TaskState::Working | TaskState::NeedsYou));
    if !parent_working {
        return Wait::ParentGone;
    }
    let mut finished = true;
    for id in ids {
        let Some(h) = views.task(id) else {
            continue;
        };
        if h.state == TaskState::NeedsYou {
            if let Some(r) = views
                .open_requests_for(id)
                .into_iter()
                .find(|r| !reported.contains(&r.info.id))
            {
                return Wait::NeedsHuman {
                    helper: id.clone(),
                    request: r.info.id.clone(),
                    status: h.status.clone(),
                };
            }
        }
        if !matches!(
            h.state,
            TaskState::Done | TaskState::Failed | TaskState::Stopped
        ) {
            finished = false;
        }
    }
    if finished {
        Wait::Finished
    } else {
        Wait::Pending
    }
}

/// How a wait ended.
#[derive(Debug, Clone, PartialEq)]
pub enum WaitEnd {
    Done(Wait),
    TimedOut,
}

/// Wait on the hub's events (not on a timer) until `wait_state` settles or the
/// time is up. Subscribes before it looks, so nothing is missed.
pub async fn wait(
    hub: &Hub,
    parent: &str,
    ids: &[String],
    timeout: Duration,
    reported: &HashSet<String>,
) -> WaitEnd {
    let mut sub = hub.subscribe(None);
    let deadline = Instant::now() + timeout;
    let mut hearing = true;
    loop {
        match hub.view(|v| wait_state(v, parent, ids, reported)) {
            Wait::Pending => {}
            settled => return WaitEnd::Done(settled),
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return WaitEnd::TimedOut;
        }
        if hearing {
            // A day-long quiet is no reason to trust the subscription forever.
            let nap = left.min(Duration::from_secs(30));
            // `Ok(None)`: dropped for lagging, so poll from now on.
            if let Ok(None) = tokio::time::timeout(nap, sub.rx.recv()).await {
                hearing = false;
            }
        } else {
            tokio::time::sleep(left.min(Duration::from_millis(500))).await;
        }
    }
}

// ---- reports ---------------------------------------------------------------------

/// The final summary of a finished helper: its last turn's result, else the
/// last thing it said.
pub fn final_summary(hub: &Hub, t: &TaskView) -> String {
    let from_turn = hub
        .history(Some(&t.id), None, 200)
        .unwrap_or_default()
        .into_iter()
        .rev()
        .find_map(|e| match e.body {
            sphere_proto::EventBody::Turn { summary, .. } if !summary.trim().is_empty() => {
                Some(summary)
            }
            _ => None,
        });
    from_turn.unwrap_or_else(|| t.last_say.clone())
}

fn clip(s: &str, max: usize) -> String {
    let s = s.trim();
    let mut out: String = s.chars().take(max).collect();
    if s.chars().count() > max {
        out.push_str("\n[cut]");
    }
    out
}

/// One helper as the agent reads it. `summary` is its final summary.
pub fn describe(t: &TaskView, summary: Option<&str>) -> String {
    let state = match t.state {
        TaskState::Queued => "queued",
        TaskState::Working => "working",
        TaskState::NeedsYou => "waiting for the human",
        TaskState::Done => "done",
        TaskState::Failed => "failed",
        TaskState::Stopped => "stopped",
    };
    let mut out = format!("{} \"{}\": {state}", t.id, t.title);
    if matches!(t.state, TaskState::Failed) || summary.is_none() {
        out.push_str(&format!(" ({})", t.status));
    }
    if let Some(s) = summary.filter(|s| !s.trim().is_empty()) {
        out.push_str(&format!("\n{}", clip(s, SUMMARY_CHARS)));
    }
    if let Some(b) = &t.branch {
        let n = t.files.len();
        out.push_str(&format!(
            "\nbranch {b}, {n} file{} changed",
            if n == 1 { "" } else { "s" }
        ));
    } else if t.state == TaskState::Done {
        out.push_str("\nno files changed");
    }
    out
}

/// The text `wait_for_helpers` returns.
pub fn wait_report(hub: &Hub, ids: &[String], end: &WaitEnd, waited: Duration) -> String {
    let (head, details) = hub.view(|v| {
        let helpers: Vec<TaskView> = ids.iter().filter_map(|i| v.task(i).cloned()).collect();
        let working: Vec<&str> = helpers
            .iter()
            .filter(|h| !matches!(h.state, TaskState::Done | TaskState::Failed | TaskState::Stopped))
            .map(|h| h.id.as_str())
            .collect();
        let head = match end {
            WaitEnd::Done(Wait::Finished) => format!(
                "{} finished.",
                if helpers.len() == 1 {
                    "The helper has".to_string()
                } else {
                    format!("All {} helpers have", helpers.len())
                }
            ),
            WaitEnd::Done(Wait::NeedsHuman { helper, status, .. }) => format!(
                "Helper {helper} is waiting for the human: {status}. The human will answer it in Sphere; you cannot. Call wait_for_helpers again to keep waiting."
            ),
            WaitEnd::Done(Wait::ParentGone) | WaitEnd::Done(Wait::Pending) => {
                "You are no longer working.".to_string()
            }
            WaitEnd::TimedOut => format!(
                "Timed out after {} seconds. Still working: {}. Call wait_for_helpers again to keep waiting.",
                waited.as_secs(),
                working.join(", ")
            ),
        };
        (head, helpers)
    });
    let mut out = head;
    for h in &details {
        let finished = matches!(h.state, TaskState::Done | TaskState::Failed);
        let summary = finished.then(|| final_summary(hub, h));
        out.push_str("\n\n");
        out.push_str(&describe(h, summary.as_deref()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use sphere_proto::{EventBody, FileStat, RequestInfo, RequestKind};

    fn hub() -> Hub {
        Hub::with_store(Store::open_in_memory().unwrap(), 64).unwrap()
    }

    fn created(parent: Option<&str>, title: &str) -> EventBody {
        EventBody::TaskCreated {
            title: title.into(),
            prompt: format!("do {title}"),
            repo: "/r".into(),
            in_place: false,
            budget_usd: 1.0,
            mode: None,
            model: None,
            allow_rewrites: false,
            resume_session: None,
            parent_task: parent.map(str::to_string),
        }
    }

    fn started() -> EventBody {
        EventBody::AgentStarted {
            child: "c".into(),
            pid: 1,
            resumed: false,
        }
    }

    fn turn(summary: &str) -> EventBody {
        EventBody::Turn {
            ok: true,
            interrupted: false,
            summary: summary.into(),
            turns: Some(1),
            duration_ms: None,
            cost_usd: None,
            cost_delta_usd: 0.0,
        }
    }

    fn ask(id: &str, task: &str) -> EventBody {
        EventBody::Request {
            request: RequestInfo {
                id: id.into(),
                task: task.into(),
                kind: RequestKind::Permission,
                tool: "Bash".into(),
                display_name: None,
                description: None,
                summary: String::new(),
                input: json!({"command": "git push"}),
                suggestions: vec![],
                blocked_path: None,
                tool_use_id: None,
                agent_request: Some("a".into()),
                revises: None,
            },
        }
    }

    /// A parent `t1` working, with helpers `t2..` (each started and working).
    fn family(helpers: usize) -> Hub {
        let h = hub();
        h.publish("t1", created(None, "parent")).unwrap();
        h.publish("t1", started()).unwrap();
        for i in 0..helpers {
            let id = format!("t{}", i + 2);
            h.publish(&id, created(Some("t1"), &format!("h{i}")))
                .unwrap();
            h.publish(&id, started()).unwrap();
        }
        h
    }

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn written(r: Routed) -> Value {
        match r {
            Routed::Write(line) => {
                let v: Value = serde_json::from_str(&line).unwrap();
                v["response"]["response"]["mcp_response"].clone()
            }
            other => panic!("{other:?}"),
        }
    }

    fn view(h: &Hub, id: &str) -> TaskView {
        h.view(|v| v.task(id).cloned()).unwrap()
    }

    #[test]
    fn the_handshake_echoes_the_clients_version_and_lists_tools() {
        let h = family(1);
        let parent = view(&h, "t1");
        let init = written(route(
            &parent,
            "r1",
            &json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{}}}),
        ));
        assert_eq!(init["id"], 0);
        assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(init["result"]["capabilities"], json!({"tools": {}}));
        assert_eq!(init["result"]["serverInfo"]["name"], "sphere");
        // A notification has no id; it still gets an answer.
        let note = written(route(
            &parent,
            "r2",
            &json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        ));
        assert_eq!(note["result"], json!({}));
        let list = written(route(
            &parent,
            "r3",
            &json!({"jsonrpc":"2.0","id":"x","method":"tools/list"}),
        ));
        assert_eq!(list["id"], "x", "the id comes back as it came");
        let names: Vec<&str> = list["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "start_helper",
                "wait_for_helpers",
                "message_helper",
                "stop_helper"
            ]
        );
        for t in list["result"]["tools"].as_array().unwrap() {
            assert_eq!(t["inputSchema"]["type"], "object");
            assert!(!t["description"].as_str().unwrap().is_empty());
        }
        let unknown = written(route(
            &parent,
            "r4",
            &json!({"jsonrpc":"2.0","id":9,"method":"resources/list"}),
        ));
        assert_eq!(unknown["error"]["code"], -32601);
        assert_eq!(unknown["id"], 9);
    }

    #[test]
    fn a_helper_is_not_offered_start_helper() {
        let h = family(1);
        let list = written(route(
            &view(&h, "t2"),
            "r",
            &json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
        ));
        let names: Vec<&str> = list["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["wait_for_helpers", "message_helper", "stop_helper"]);
    }

    #[test]
    fn a_tool_call_is_routed_with_its_arguments_and_answered_with_the_same_ids() {
        let h = family(0);
        let msg = json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"stop_helper","arguments":{"helper":"t2"}}});
        let Routed::Call(call) = route(&view(&h, "t1"), "req_5", &msg) else {
            panic!("a call")
        };
        assert_eq!(call.name, "stop_helper");
        assert_eq!(call.args["helper"], "t2");
        let line: Value = serde_json::from_str(&call_reply(&call, "Stopped.", false)).unwrap();
        assert_eq!(line["response"]["request_id"], "req_5");
        let rpc = &line["response"]["response"]["mcp_response"];
        assert_eq!(rpc["id"], 4);
        assert_eq!(rpc["result"]["content"][0]["text"], "Stopped.");
        assert_eq!(rpc["result"]["isError"], false);
        // No arguments at all is an empty object.
        let bare = json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"wait_for_helpers"}});
        let Routed::Call(call) = route(&view(&h, "t1"), "r", &bare) else {
            panic!("a call")
        };
        assert_eq!(call.args, json!({}));
    }

    #[test]
    fn arguments_are_checked() {
        let p = |name: &str, args: Value| parse_tool(name, &args, false);
        assert_eq!(
            p("start_helper", json!({"title": " Docs ", "prompt": "Read"})).unwrap(),
            Tool::Start {
                title: "Docs".into(),
                prompt: "Read".into()
            }
        );
        // The prefix Claude Code adds is fine too.
        assert!(p("mcp__sphere__stop_helper", json!({"helper": "t2"})).is_ok());
        for bad in [
            json!({}),
            json!({"title": "x"}),
            json!({"title": "", "prompt": "p"}),
            json!({"title": "x", "prompt": "  "}),
            json!({"title": 3, "prompt": "p"}),
        ] {
            assert!(p("start_helper", bad.clone()).is_err(), "{bad}");
        }
        let long = "x".repeat(300);
        match p("start_helper", json!({"title": long, "prompt": "p"})).unwrap() {
            Tool::Start { title, .. } => assert_eq!(title.chars().count(), 100),
            other => panic!("{other:?}"),
        }
        assert!(p("message_helper", json!({"helper": "t2"})).is_err());
        assert!(p("stop_helper", json!({"helper": 2})).is_err());
        assert!(p("nope", json!({})).unwrap_err().contains("unknown tool"));

        let wait = |args| match p("wait_for_helpers", args).unwrap() {
            Tool::Wait { helpers, timeout } => (helpers, timeout.as_secs()),
            other => panic!("{other:?}"),
        };
        assert_eq!(wait(json!({})), (None, 300));
        assert_eq!(wait(json!({"helpers": []})), (None, 300));
        assert_eq!(
            wait(json!({"helpers": ["t2", " t3 "], "timeout_seconds": 20})),
            (Some(ids(&["t2", "t3"])), 20)
        );
        assert_eq!(wait(json!({"timeout_seconds": 5000})).1, 900, "capped");
        assert_eq!(
            wait(json!({"timeout_seconds": 0})).1,
            1,
            "at least a second"
        );
        assert!(p("wait_for_helpers", json!({"helpers": "t2"})).is_err());
        assert!(p("wait_for_helpers", json!({"helpers": [2]})).is_err());
        assert!(p("wait_for_helpers", json!({"timeout_seconds": "soon"})).is_err());
    }

    #[test]
    fn a_helper_cannot_start_helpers() {
        let args = json!({"title": "x", "prompt": "y"});
        let err = parse_tool("start_helper", &args, true).unwrap_err();
        assert!(err.contains("cannot start helpers"), "{err}");
        let h = family(1);
        let err = h.view(|v| check_start(v, "t2")).unwrap_err();
        assert!(err.contains("cannot start helpers"), "{err}");
    }

    #[test]
    fn the_limits_count_working_and_all_helpers() {
        let h = family(MAX_WORKING);
        let err = h.view(|v| check_start(v, "t1")).unwrap_err();
        assert!(err.contains("already working"), "{err}");
        // One finishes: room for another.
        h.publish("t2", turn("done")).unwrap();
        assert!(h.view(|v| check_start(v, "t1")).is_ok());
        // Queued ones count as working.
        h.publish("t9", created(Some("t1"), "queued")).unwrap();
        assert!(h.view(|v| check_start(v, "t1")).is_err());

        // Eight in all, even when every one has finished.
        let h = family(MAX_TOTAL);
        for i in 0..MAX_TOTAL {
            h.publish(&format!("t{}", i + 2), turn("ok")).unwrap();
        }
        let err = h.view(|v| check_start(v, "t1")).unwrap_err();
        assert!(err.contains("already started 8"), "{err}");
        // Another parent's helpers are not counted.
        h.publish("t20", created(None, "other")).unwrap();
        assert!(h.view(|v| check_start(v, "t20")).is_ok());
        assert!(h.view(|v| check_start(v, "nope")).is_err());
    }

    #[test]
    fn a_parent_only_reaches_its_own_helpers() {
        let h = family(1);
        h.publish("t5", created(None, "stranger")).unwrap();
        assert!(h
            .view(|v| own_helper(v, "t1", "t2").map(|t| t.id.clone()))
            .is_ok());
        let err = h
            .view(|v| own_helper(v, "t1", "t5").map(|_| ()))
            .unwrap_err();
        assert_eq!(err, "t5 is not one of your helpers.");
        assert!(h.view(|v| own_helper(v, "t1", "t1").map(|_| ())).is_err());
        assert!(h.view(|v| own_helper(v, "t1", "zz").map(|_| ())).is_err());
        // A helper of someone else is not yours.
        assert!(h.view(|v| own_helper(v, "t5", "t2").map(|_| ())).is_err());
    }

    #[test]
    fn a_start_read_again_finds_the_helper_it_already_started() {
        let h = family(1);
        let found = h.view(|v| already_started(v, "t1", "h0", "do h0").map(|t| t.id.clone()));
        assert_eq!(found.as_deref(), Some("t2"));
        assert!(h.view(|v| already_started(v, "t1", "h0", "something else").is_none()));
        // A finished one does not count: starting the same work again is allowed.
        h.publish("t2", turn("done")).unwrap();
        assert!(h.view(|v| already_started(v, "t1", "h0", "do h0").is_none()));
    }

    #[test]
    fn the_wait_state_follows_the_helpers() {
        let h = family(2);
        let all = ids(&["t2", "t3"]);
        let none = HashSet::new();
        let state =
            |h: &Hub, reported: &HashSet<String>| h.view(|v| wait_state(v, "t1", &all, reported));
        assert_eq!(state(&h, &none), Wait::Pending);
        h.publish("t2", turn("one")).unwrap();
        assert_eq!(state(&h, &none), Wait::Pending);
        h.publish(
            "t3",
            EventBody::Failed {
                message: "boom".into(),
            },
        )
        .unwrap();
        assert_eq!(state(&h, &none), Wait::Finished);
        // Messaging one wakes it: not finished again.
        h.publish(
            "t2",
            EventBody::UserMessage {
                text: "more".into(),
            },
        )
        .unwrap();
        assert_eq!(state(&h, &none), Wait::Pending);
    }

    #[test]
    fn a_helper_that_needs_the_human_ends_the_wait_once() {
        let h = family(2);
        let all = ids(&["t2", "t3"]);
        h.publish("t3", ask("r1", "t3")).unwrap();
        let first = h.view(|v| wait_state(v, "t1", &all, &HashSet::new()));
        assert_eq!(
            first,
            Wait::NeedsHuman {
                helper: "t3".into(),
                request: "r1".into(),
                status: "Needs you: Bash git push".into()
            }
        );
        // Told once; waiting again keeps waiting rather than returning at once.
        let reported: HashSet<String> = ["r1".to_string()].into();
        assert_eq!(
            h.view(|v| wait_state(v, "t1", &all, &reported)),
            Wait::Pending
        );
        // A new request is news again.
        h.publish("t2", ask("r2", "t2")).unwrap();
        assert!(matches!(
            h.view(|v| wait_state(v, "t1", &all, &reported)),
            Wait::NeedsHuman { request, .. } if request == "r2"
        ));
    }

    #[test]
    fn the_wait_ends_when_the_parent_stops() {
        let h = family(1);
        h.publish("t1", EventBody::Stopped {}).unwrap();
        assert_eq!(
            h.view(|v| wait_state(v, "t1", &ids(&["t2"]), &HashSet::new())),
            Wait::ParentGone
        );
    }

    #[tokio::test]
    async fn waiting_returns_when_the_last_helper_finishes() {
        let h = std::sync::Arc::new(family(2));
        let hub = h.clone();
        let later = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            hub.publish("t2", turn("a")).unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
            hub.publish("t3", turn("b")).unwrap();
        });
        let end = wait(
            &h,
            "t1",
            &ids(&["t2", "t3"]),
            Duration::from_secs(10),
            &HashSet::new(),
        )
        .await;
        assert_eq!(end, WaitEnd::Done(Wait::Finished));
        later.await.unwrap();
        let text = wait_report(&h, &ids(&["t2", "t3"]), &end, Duration::ZERO);
        assert!(text.starts_with("All 2 helpers have finished."), "{text}");
        assert!(text.contains("t2 \"h0\": done\na"), "{text}");
        assert!(text.contains("t3 \"h1\": done\nb"), "{text}");
    }

    #[tokio::test]
    async fn waiting_returns_early_for_the_human_and_names_the_request() {
        let h = std::sync::Arc::new(family(2));
        let hub = h.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            hub.publish("t3", ask("r1", "t3")).unwrap();
        });
        let started = Instant::now();
        let end = wait(
            &h,
            "t1",
            &ids(&["t2", "t3"]),
            Duration::from_secs(30),
            &HashSet::new(),
        )
        .await;
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(matches!(&end, WaitEnd::Done(Wait::NeedsHuman { helper, .. }) if helper == "t3"));
        let text = wait_report(&h, &ids(&["t2", "t3"]), &end, Duration::ZERO);
        assert!(
            text.contains("Helper t3 is waiting for the human: Needs you: Bash git push"),
            "{text}"
        );
        assert!(text.contains("you cannot"), "{text}");
        assert!(text.contains("t2 \"h0\": working"), "{text}");
        assert!(text.contains("t3 \"h1\": waiting for the human"), "{text}");
    }

    #[tokio::test]
    async fn waiting_times_out_and_says_what_is_still_working() {
        let h = family(2);
        h.publish("t2", turn("quick one")).unwrap();
        let started = Instant::now();
        let end = wait(
            &h,
            "t1",
            &ids(&["t2", "t3"]),
            Duration::from_millis(200),
            &HashSet::new(),
        )
        .await;
        assert_eq!(end, WaitEnd::TimedOut);
        assert!(started.elapsed() >= Duration::from_millis(200));
        let text = wait_report(&h, &ids(&["t2", "t3"]), &end, Duration::from_millis(200));
        assert!(text.contains("Still working: t3."), "{text}");
        assert!(text.contains("t2 \"h0\": done\nquick one"), "{text}");
        assert!(text.contains("t3 \"h1\": working (Working)"), "{text}");
    }

    #[tokio::test]
    async fn a_lagging_subscription_falls_back_to_looking_now_and_then() {
        // A queue of one overflows at once; the wait still sees the end.
        let h = std::sync::Arc::new(Hub::with_store(Store::open_in_memory().unwrap(), 1).unwrap());
        h.publish("t1", created(None, "parent")).unwrap();
        h.publish("t1", started()).unwrap();
        h.publish("t2", created(Some("t1"), "h")).unwrap();
        h.publish("t2", started()).unwrap();
        let hub = h.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            for _ in 0..5 {
                hub.publish("t2", EventBody::Say { text: "x".into() })
                    .unwrap();
            }
            hub.publish("t2", turn("late")).unwrap();
        });
        let end = wait(
            &h,
            "t1",
            &ids(&["t2"]),
            Duration::from_secs(10),
            &HashSet::new(),
        )
        .await;
        assert_eq!(end, WaitEnd::Done(Wait::Finished));
    }

    #[test]
    fn a_helper_reads_with_its_summary_branch_and_files() {
        let h = family(1);
        h.publish(
            "t2",
            EventBody::WorkspaceReady {
                workdir: "/w".into(),
                branch: Some("sphere/h0".into()),
                base: "main".into(),
            },
        )
        .unwrap();
        h.publish(
            "t2",
            EventBody::FilesTouched {
                files: vec![FileStat {
                    path: "a".into(),
                    added: 1,
                    removed: 0,
                }],
            },
        )
        .unwrap();
        h.publish(
            "t2",
            EventBody::Say {
                text: "last words".into(),
            },
        )
        .unwrap();
        h.publish("t2", turn("")).unwrap();
        let t = view(&h, "t2");
        // An empty result falls back to what it last said.
        assert_eq!(final_summary(&h, &t), "last words");
        assert_eq!(
            describe(&t, Some("last words")),
            "t2 \"h0\": done\nlast words\nbranch sphere/h0, 1 file changed"
        );
        h.publish(
            "t2",
            EventBody::UserMessage {
                text: "again".into(),
            },
        )
        .unwrap();
        h.publish("t2", turn("Fixed the thing")).unwrap();
        assert_eq!(final_summary(&h, &view(&h, "t2")), "Fixed the thing");
        // A helper that changed nothing says so.
        let h = family(1);
        h.publish("t2", turn("Looked around")).unwrap();
        assert_eq!(
            describe(&view(&h, "t2"), Some("Looked around")),
            "t2 \"h0\": done\nLooked around\nno files changed"
        );
        // A long summary is cut.
        let long = "y".repeat(SUMMARY_CHARS + 50);
        assert!(describe(&view(&h, "t2"), Some(&long)).contains("[cut]"));
    }

    #[test]
    fn a_failed_helper_shows_why() {
        let h = family(1);
        h.publish(
            "t2",
            EventBody::Failed {
                message: "the agent exited (exit code 1)".into(),
            },
        )
        .unwrap();
        let text = describe(&view(&h, "t2"), None);
        assert!(
            text.starts_with("t2 \"h0\": failed (Failed: the agent exited (exit code 1))"),
            "{text}"
        );
    }
}
