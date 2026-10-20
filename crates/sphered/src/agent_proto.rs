//! The stream-json protocol Claude Code speaks when driven as an SDK host:
//! parsing what it prints and building what we write. Parsing is tolerant:
//! unknown types and fields are ignored, never fatal.

use serde_json::{json, Value};
use sphere_proto::{PlanItem, StepKind};

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Text(String),
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolResult {
    pub tool_use_id: String,
    pub text: String,
    pub is_error: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CanUseTool {
    pub request_id: String,
    pub tool_name: String,
    pub display_name: Option<String>,
    pub input: Value,
    pub tool_use_id: Option<String>,
    pub description: Option<String>,
    pub suggestions: Vec<Value>,
    pub blocked_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    Init {
        session_id: Option<String>,
        model: Option<String>,
        permission_mode: Option<String>,
        slash_commands: Vec<String>,
    },
    Assistant {
        blocks: Vec<Block>,
        parent: Option<String>,
    },
    ToolResults(Vec<ToolResult>),
    /// A typing delta; a live frame, never stored.
    Delta(String),
    Result {
        ok: bool,
        subtype: String,
        text: String,
        turns: Option<u64>,
        duration_ms: Option<u64>,
        total_cost_usd: Option<f64>,
    },
    CanUseTool(CanUseTool),
    /// A control request we do not handle; it gets an error reply.
    OtherControl {
        request_id: String,
        subtype: String,
    },
    ControlResponse {
        request_id: String,
        ok: bool,
        error: Option<String>,
        response: Value,
    },
    ControlCancel {
        request_id: String,
    },
    /// Anything else, by its `type`.
    Other(String),
}

fn s(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(|x| x.as_str()).map(str::to_string)
}

/// Flatten a tool result's `content` (a string, or blocks with text).
fn content_text(c: &Value) -> String {
    match c {
        Value::String(t) => t.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

pub fn parse_line(line: &str) -> Result<Incoming, serde_json::Error> {
    let v: Value = serde_json::from_str(line)?;
    let ty = s(&v, "type").unwrap_or_default();
    Ok(match ty.as_str() {
        "system" if s(&v, "subtype").as_deref() == Some("init") => Incoming::Init {
            session_id: s(&v, "session_id"),
            model: s(&v, "model"),
            permission_mode: s(&v, "permissionMode").or_else(|| s(&v, "permission_mode")),
            slash_commands: v
                .get("slash_commands")
                .and_then(|a| a.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|c| c.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
        },
        "assistant" => {
            let blocks = v
                .pointer("/message/content")
                .and_then(|c| c.as_array())
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|b| match b.get("type").and_then(|t| t.as_str())? {
                            "text" => Some(Block::Text(s(b, "text")?)),
                            "tool_use" => Some(Block::ToolUse {
                                id: s(b, "id")?,
                                name: s(b, "name")?,
                                input: b.get("input").cloned().unwrap_or(Value::Null),
                            }),
                            _ => None,
                        })
                        .collect()
                })
                .unwrap_or_default();
            Incoming::Assistant {
                blocks,
                parent: s(&v, "parent_tool_use_id"),
            }
        }
        "user" => {
            let results: Vec<ToolResult> = v
                .pointer("/message/content")
                .and_then(|c| c.as_array())
                .map(|items| {
                    items
                        .iter()
                        .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_result"))
                        .filter_map(|b| {
                            Some(ToolResult {
                                tool_use_id: s(b, "tool_use_id")?,
                                text: content_text(b.get("content").unwrap_or(&Value::Null)),
                                is_error: b
                                    .get("is_error")
                                    .and_then(|e| e.as_bool())
                                    .unwrap_or(false),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            if results.is_empty() {
                Incoming::Other("user".into())
            } else {
                Incoming::ToolResults(results)
            }
        }
        "stream_event" => {
            let delta = v.pointer("/event/delta");
            match (
                v.pointer("/event/type").and_then(|t| t.as_str()),
                delta.and_then(|d| d.get("type")).and_then(|t| t.as_str()),
            ) {
                (Some("content_block_delta"), Some("text_delta")) => {
                    Incoming::Delta(delta.and_then(|d| s(d, "text")).unwrap_or_default())
                }
                _ => Incoming::Other("stream_event".into()),
            }
        }
        "result" => {
            let subtype = s(&v, "subtype").unwrap_or_default();
            let is_error = v.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false);
            let text = s(&v, "result").unwrap_or_else(|| {
                v.get("errors")
                    .and_then(|e| e.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str())
                            .collect::<Vec<_>>()
                            .join("; ")
                    })
                    .filter(|t| !t.is_empty())
                    .unwrap_or_else(|| subtype.clone())
            });
            Incoming::Result {
                ok: !is_error && (subtype.is_empty() || subtype == "success"),
                subtype,
                text,
                turns: v.get("num_turns").and_then(|n| n.as_u64()),
                duration_ms: v.get("duration_ms").and_then(|n| n.as_u64()),
                total_cost_usd: v.get("total_cost_usd").and_then(|n| n.as_f64()),
            }
        }
        "control_request" => {
            let request_id = s(&v, "request_id").unwrap_or_default();
            let req = v.get("request").cloned().unwrap_or(Value::Null);
            match s(&req, "subtype").as_deref() {
                Some("can_use_tool") => Incoming::CanUseTool(CanUseTool {
                    request_id,
                    tool_name: s(&req, "tool_name").unwrap_or_default(),
                    display_name: s(&req, "display_name"),
                    input: req.get("input").cloned().unwrap_or(Value::Null),
                    tool_use_id: s(&req, "tool_use_id"),
                    description: s(&req, "description"),
                    suggestions: req
                        .get("permission_suggestions")
                        .and_then(|a| a.as_array())
                        .cloned()
                        .unwrap_or_default(),
                    blocked_path: s(&req, "blocked_path"),
                }),
                other => Incoming::OtherControl {
                    request_id,
                    subtype: other.unwrap_or("").to_string(),
                },
            }
        }
        "control_response" => {
            let r = v.get("response").cloned().unwrap_or(Value::Null);
            Incoming::ControlResponse {
                request_id: s(&r, "request_id").unwrap_or_default(),
                ok: s(&r, "subtype").as_deref() == Some("success"),
                error: s(&r, "error"),
                response: r.get("response").cloned().unwrap_or(Value::Null),
            }
        }
        "control_cancel_request" => Incoming::ControlCancel {
            request_id: s(&v, "request_id").unwrap_or_default(),
        },
        other => Incoming::Other(other.to_string()),
    })
}

// ---- what we write ----------------------------------------------------------

fn line(v: Value) -> String {
    let mut s = v.to_string();
    s.push('\n');
    s
}

pub fn user_message(text: &str) -> String {
    line(json!({"type": "user", "message": {"role": "user", "content": text}}))
}

pub fn allow_response(
    request_id: &str,
    updated_input: &Value,
    updated_permissions: Option<Vec<Value>>,
) -> String {
    let mut resp = json!({"behavior": "allow", "updatedInput": updated_input});
    if let Some(p) = updated_permissions {
        resp["updatedPermissions"] = Value::Array(p);
    }
    line(json!({"type": "control_response", "response": {
        "subtype": "success", "request_id": request_id, "response": resp}}))
}

pub fn deny_response(request_id: &str, message: &str) -> String {
    line(json!({"type": "control_response", "response": {
        "subtype": "success", "request_id": request_id,
        "response": {"behavior": "deny", "message": message}}}))
}

pub fn control_error(request_id: &str, message: &str) -> String {
    line(json!({"type": "control_response", "response": {
        "subtype": "error", "request_id": request_id, "error": message}}))
}

fn control_request(request_id: &str, request: Value) -> String {
    line(json!({"type": "control_request", "request_id": request_id, "request": request}))
}

pub fn interrupt_request(request_id: &str) -> String {
    control_request(request_id, json!({"subtype": "interrupt"}))
}

pub fn set_mode_request(request_id: &str, mode: &str) -> String {
    control_request(
        request_id,
        json!({"subtype": "set_permission_mode", "mode": mode}),
    )
}

pub fn set_model_request(request_id: &str, model: &str) -> String {
    control_request(request_id, json!({"subtype": "set_model", "model": model}))
}

/// The command line for an agent, after the configured leading arguments.
pub struct AgentArgs<'a> {
    pub settings: &'a str,
    pub budget_usd: f64,
    pub mode: &'a str,
    pub model: Option<&'a str>,
    pub resume: Option<&'a str>,
    /// The user's notes for this repo (see `briefing`), added to the agent's system prompt.
    pub briefing: Option<&'a str>,
}

pub fn build_args(a: &AgentArgs) -> Vec<String> {
    let mut args: Vec<String> = [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        "--permission-prompt-tool",
        "stdio",
        "--permission-mode",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    args.push(a.mode.to_string());
    args.push("--settings".into());
    args.push(a.settings.to_string());
    args.push("--max-budget-usd".into());
    args.push(a.budget_usd.to_string());
    if let Some(b) = a.briefing {
        args.push("--append-system-prompt".into());
        args.push(b.to_string());
    }
    if let Some(m) = a.model {
        args.push("--model".into());
        args.push(m.to_string());
    }
    if let Some(r) = a.resume {
        args.push("--resume".into());
        args.push(r.to_string());
    }
    args
}

/// An answer to `AskUserQuestion`: the input with `answers` filled in for the
/// first question.
pub fn question_answer_input(input: &Value, answer: &str) -> Value {
    let mut out = input.clone();
    if let Some(q) = input
        .pointer("/questions/0/question")
        .and_then(|q| q.as_str())
    {
        out["answers"] = json!({ q: answer });
    }
    out
}

// ---- reading steps ----------------------------------------------------------

fn first_line(s: &str, max: usize) -> String {
    let l = s
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    let mut out: String = l.chars().take(max).collect();
    if l.chars().count() > max {
        out.push('…');
    }
    out
}

/// How a tool call reads in the step track: kind, title, and the detail (the
/// exact command or path).
pub fn step_info(tool: &str, input: &Value) -> (StepKind, String, String) {
    let f = |k: &str| input.get(k).and_then(|v| v.as_str()).unwrap_or("");
    match tool {
        "Read" => (
            StepKind::Read,
            format!("Reading {}", f("file_path")),
            f("file_path").into(),
        ),
        "Edit" | "MultiEdit" | "Write" => (
            StepKind::Edit,
            format!("Editing {}", f("file_path")),
            f("file_path").into(),
        ),
        "NotebookEdit" => (
            StepKind::Edit,
            format!("Editing {}", f("notebook_path")),
            f("notebook_path").into(),
        ),
        "Bash" => (
            StepKind::Run,
            format!("Running {}", first_line(f("command"), 80)),
            f("command").into(),
        ),
        "Grep" | "Glob" => (
            StepKind::Search,
            format!("Searching {}", f("pattern")),
            f("pattern").into(),
        ),
        "WebFetch" => (
            StepKind::Web,
            format!("Fetching {}", f("url")),
            f("url").into(),
        ),
        "WebSearch" => (
            StepKind::Web,
            format!("Searching the web for {}", f("query")),
            f("query").into(),
        ),
        "Task" | "Agent" => (
            StepKind::Delegate,
            format!("Delegating: {}", first_line(f("description"), 80)),
            f("prompt").into(),
        ),
        "TodoWrite" => (StepKind::Plan, "Updating the plan".into(), String::new()),
        "ExitPlanMode" => (StepKind::Plan, "Presenting a plan".into(), f("plan").into()),
        "AskUserQuestion" => (
            StepKind::Other,
            format!(
                "Asking: {}",
                first_line(
                    input
                        .pointer("/questions/0/question")
                        .and_then(|q| q.as_str())
                        .unwrap_or(""),
                    80
                )
            ),
            String::new(),
        ),
        other => (StepKind::Other, format!("Using {other}"), String::new()),
    }
}

/// Plan items from a `TodoWrite` input.
pub fn plan_items(input: &Value) -> Vec<PlanItem> {
    input
        .get("todos")
        .and_then(|t| t.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|t| {
                    Some(PlanItem {
                        text: t.get("content").and_then(|c| c.as_str())?.to_string(),
                        status: t
                            .get("status")
                            .and_then(|c| c.as_str())
                            .unwrap_or("pending")
                            .to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Tools that can change files, which makes "files touched" worth refreshing.
pub fn may_change_files(tool: &str) -> bool {
    matches!(
        tool,
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit" | "Bash"
    )
}

const OUTBOUND_COMMANDS: &[&str] = &[
    "curl", "wget", "ssh", "scp", "rsync", "gh", "nc", "ncat", "sftp", "ftp", "telnet", "sendmail",
    "mail",
];
const OUTBOUND_PREFIXES: &[&str] = &[
    "git push",
    "npm publish",
    "yarn publish",
    "pnpm publish",
    "cargo publish",
    "docker push",
    "twine upload",
];
const OUTBOUND_MCP_WORDS: &[&str] = &[
    "send",
    "post",
    "message",
    "email",
    "mail",
    "slack",
    "publish",
    "tweet",
    "notify",
    "comment",
    "merge",
    "pull_request",
];

/// Does this tool call send something off the machine? Used to decide whether
/// a call cut off by a restart may simply be run again.
pub fn is_outbound(tool: &str, detail_or_input: &Value) -> bool {
    match tool {
        "WebFetch" | "WebSearch" => true,
        "Bash" => {
            let cmd = detail_or_input
                .get("command")
                .and_then(|c| c.as_str())
                .or_else(|| detail_or_input.as_str())
                .unwrap_or("");
            cmd.split([';', '&', '|', '\n', '(', ')', '`'])
                .map(|seg| {
                    let mut words = seg.split_whitespace().peekable();
                    // Skip `VAR=x`, `sudo`, `time`, `then` ...
                    while let Some(w) = words.peek() {
                        if w.contains('=')
                            || matches!(*w, "sudo" | "time" | "then" | "do" | "exec" | "command")
                        {
                            words.next();
                        } else {
                            break;
                        }
                    }
                    words.collect::<Vec<_>>().join(" ")
                })
                .any(|seg| {
                    let first = seg.split(' ').next().unwrap_or("");
                    OUTBOUND_COMMANDS.contains(&first)
                        || OUTBOUND_PREFIXES.iter().any(|p| seg.starts_with(p))
                })
        }
        t if t.starts_with("mcp__") => {
            let lower = t.to_ascii_lowercase();
            OUTBOUND_MCP_WORDS.iter().any(|w| lower.contains(w))
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_init() {
        let l = r#"{"type":"system","subtype":"init","session_id":"s1","model":"m","permissionMode":"acceptEdits","slash_commands":["/compact","/review"],"tools":["Bash"],"extra":1}"#;
        assert_eq!(
            parse_line(l).unwrap(),
            Incoming::Init {
                session_id: Some("s1".into()),
                model: Some("m".into()),
                permission_mode: Some("acceptEdits".into()),
                slash_commands: vec!["/compact".into(), "/review".into()],
            }
        );
        // Other system messages are not an init.
        assert_eq!(
            parse_line(r#"{"type":"system","subtype":"status"}"#).unwrap(),
            Incoming::Other("system".into())
        );
    }

    #[test]
    fn parses_assistant_blocks_and_ignores_unknown_ones() {
        let l = r#"{"type":"assistant","parent_tool_use_id":"toolu_p","message":{"role":"assistant","content":[
            {"type":"thinking","thinking":"hm"},
            {"type":"text","text":"Looking."},
            {"type":"tool_use","id":"toolu_1","name":"Read","input":{"file_path":"/a"}}]}}"#;
        match parse_line(l).unwrap() {
            Incoming::Assistant { blocks, parent } => {
                assert_eq!(parent.as_deref(), Some("toolu_p"));
                assert_eq!(blocks.len(), 2);
                assert_eq!(blocks[0], Block::Text("Looking.".into()));
                assert!(matches!(&blocks[1], Block::ToolUse { name, .. } if name == "Read"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parses_tool_results_with_string_or_block_content() {
        let l = r#"{"type":"user","message":{"role":"user","content":[
            {"type":"tool_result","tool_use_id":"a","content":"plain"},
            {"type":"tool_result","tool_use_id":"b","is_error":true,"content":[{"type":"text","text":"l1"},{"type":"text","text":"l2"}]}]}}"#;
        match parse_line(l).unwrap() {
            Incoming::ToolResults(r) => {
                assert_eq!(
                    r[0],
                    ToolResult {
                        tool_use_id: "a".into(),
                        text: "plain".into(),
                        is_error: false
                    }
                );
                assert_eq!(r[1].text, "l1\nl2");
                assert!(r[1].is_error);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            parse_line(r#"{"type":"user","message":{"role":"user","content":"hi"}}"#).unwrap(),
            Incoming::Other("user".into())
        );
    }

    #[test]
    fn parses_deltas_results_and_unknowns() {
        let d = r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"He"}}}"#;
        assert_eq!(parse_line(d).unwrap(), Incoming::Delta("He".into()));
        let other = r#"{"type":"stream_event","event":{"type":"message_start"}}"#;
        assert_eq!(
            parse_line(other).unwrap(),
            Incoming::Other("stream_event".into())
        );

        let r = r#"{"type":"result","subtype":"success","is_error":false,"result":"All done","num_turns":3,"duration_ms":1200,"total_cost_usd":0.0123,"session_id":"s"}"#;
        assert_eq!(
            parse_line(r).unwrap(),
            Incoming::Result {
                ok: true,
                subtype: "success".into(),
                text: "All done".into(),
                turns: Some(3),
                duration_ms: Some(1200),
                total_cost_usd: Some(0.0123),
            }
        );
        let e = r#"{"type":"result","subtype":"error_max_budget_usd","is_error":true,"errors":["budget"]}"#;
        match parse_line(e).unwrap() {
            Incoming::Result { ok, text, .. } => {
                assert!(!ok);
                assert_eq!(text, "budget");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            parse_line(r#"{"type":"brand_new","x":1}"#).unwrap(),
            Incoming::Other("brand_new".into())
        );
        assert!(parse_line("not json").is_err());
    }

    #[test]
    fn parses_permission_asks_with_and_without_optional_fields() {
        let full = r#"{"type":"control_request","request_id":"req_1","request":{"subtype":"can_use_tool","tool_name":"Bash","display_name":"Run command","input":{"command":"git push"},"tool_use_id":"toolu_9","description":"Push it","permission_suggestions":[{"type":"addRules"}],"blocked_path":"/etc"}}"#;
        match parse_line(full).unwrap() {
            Incoming::CanUseTool(c) => {
                assert_eq!(c.request_id, "req_1");
                assert_eq!(c.tool_name, "Bash");
                assert_eq!(c.display_name.as_deref(), Some("Run command"));
                assert_eq!(c.input["command"], "git push");
                assert_eq!(c.tool_use_id.as_deref(), Some("toolu_9"));
                assert_eq!(c.suggestions.len(), 1);
                assert_eq!(c.blocked_path.as_deref(), Some("/etc"));
            }
            other => panic!("{other:?}"),
        }
        let bare = r#"{"type":"control_request","request_id":"r","request":{"subtype":"can_use_tool","tool_name":"Edit","input":{}}}"#;
        match parse_line(bare).unwrap() {
            Incoming::CanUseTool(c) => {
                assert!(
                    c.display_name.is_none() && c.description.is_none() && c.suggestions.is_empty()
                );
            }
            other => panic!("{other:?}"),
        }
        let other =
            r#"{"type":"control_request","request_id":"r","request":{"subtype":"hook_callback"}}"#;
        assert_eq!(
            parse_line(other).unwrap(),
            Incoming::OtherControl {
                request_id: "r".into(),
                subtype: "hook_callback".into()
            }
        );
    }

    #[test]
    fn parses_control_responses_and_cancels() {
        let ok = r#"{"type":"control_response","response":{"subtype":"success","request_id":"sphere-1","response":{"a":1}}}"#;
        assert_eq!(
            parse_line(ok).unwrap(),
            Incoming::ControlResponse {
                request_id: "sphere-1".into(),
                ok: true,
                error: None,
                response: json!({"a": 1})
            }
        );
        let err = r#"{"type":"control_response","response":{"subtype":"error","request_id":"x","error":"nope"}}"#;
        assert!(
            matches!(parse_line(err).unwrap(), Incoming::ControlResponse { ok: false, error: Some(e), .. } if e == "nope")
        );
        assert_eq!(
            parse_line(r#"{"type":"control_cancel_request","request_id":"req_1"}"#).unwrap(),
            Incoming::ControlCancel {
                request_id: "req_1".into()
            }
        );
    }

    #[test]
    fn builds_the_lines_we_write() {
        let u: Value = serde_json::from_str(&user_message("hi")).unwrap();
        assert_eq!(
            u,
            json!({"type":"user","message":{"role":"user","content":"hi"}})
        );
        assert!(
            user_message("a\nb").trim_end().lines().count() == 1,
            "one JSON per line"
        );

        let a: Value = serde_json::from_str(&allow_response(
            "req_1",
            &json!({"command": "ls"}),
            Some(vec![json!({"type":"addRules"})]),
        ))
        .unwrap();
        assert_eq!(a["type"], "control_response");
        assert_eq!(a["response"]["subtype"], "success");
        assert_eq!(a["response"]["request_id"], "req_1");
        assert_eq!(a["response"]["response"]["behavior"], "allow");
        assert_eq!(a["response"]["response"]["updatedInput"]["command"], "ls");
        assert_eq!(
            a["response"]["response"]["updatedPermissions"][0]["type"],
            "addRules"
        );

        let plain: Value = serde_json::from_str(&allow_response("r", &json!({}), None)).unwrap();
        assert!(plain["response"]["response"]
            .get("updatedPermissions")
            .is_none());

        let d: Value = serde_json::from_str(&deny_response("req_2", "no thanks")).unwrap();
        assert_eq!(
            d["response"]["response"],
            json!({"behavior":"deny","message":"no thanks"})
        );

        let i: Value = serde_json::from_str(&interrupt_request("sphere-1")).unwrap();
        assert_eq!(
            i,
            json!({"type":"control_request","request_id":"sphere-1","request":{"subtype":"interrupt"}})
        );
        let m: Value = serde_json::from_str(&set_mode_request("x", "plan")).unwrap();
        assert_eq!(
            m["request"],
            json!({"subtype":"set_permission_mode","mode":"plan"})
        );
        let m: Value = serde_json::from_str(&set_model_request("x", "some-model")).unwrap();
        assert_eq!(
            m["request"],
            json!({"subtype":"set_model","model":"some-model"})
        );
    }

    #[test]
    fn builds_the_command_line() {
        let args = build_args(&AgentArgs {
            settings: "/s.json",
            budget_usd: 2.5,
            mode: "acceptEdits",
            model: None,
            resume: Some("sess"),
            briefing: None,
        });
        let joined = args.join(" ");
        assert!(joined.starts_with("-p --input-format stream-json --output-format stream-json --verbose --include-partial-messages --permission-prompt-tool stdio --permission-mode acceptEdits"));
        assert!(joined.contains("--settings /s.json --max-budget-usd 2.5"));
        assert!(joined.ends_with("--resume sess"));
        assert!(!joined.contains("--model"));
        let with_model = build_args(&AgentArgs {
            settings: "s",
            budget_usd: 1.0,
            mode: "plan",
            model: Some("m"),
            resume: None,
            briefing: None,
        });
        assert!(
            with_model.join(" ").contains("--model m")
                && !with_model.join(" ").contains("--resume")
        );
    }

    fn args_with(briefing: Option<&str>, model: Option<&str>, resume: Option<&str>) -> Vec<String> {
        build_args(&AgentArgs {
            settings: "/s.json",
            budget_usd: 1.0,
            mode: "acceptEdits",
            model,
            resume,
            briefing,
        })
    }

    #[test]
    fn no_notes_add_nothing_to_the_system_prompt() {
        let args = args_with(None, Some("m"), Some("sess"));
        assert!(!args.iter().any(|a| a.contains("system-prompt")));
        assert_eq!(
            args.join(" "),
            args_with(Some("x"), Some("m"), Some("sess"))
                .join(" ")
                .replace(" --append-system-prompt x", "")
        );
    }

    #[test]
    fn the_briefing_is_one_argument_after_the_system_prompt_flag() {
        let text = "Notes from the user for this repository. Follow them unless the task says otherwise:\n- use pnpm, never npm\n- keep \"quotes\" and --flags as they are";
        let args = args_with(Some(text), None, None);
        let at = args
            .iter()
            .position(|a| a == "--append-system-prompt")
            .unwrap();
        assert_eq!(args[at + 1], text);
        assert_eq!(
            args.iter()
                .filter(|a| *a == "--append-system-prompt")
                .count(),
            1
        );
        // a resumed process is briefed again, and the flags that end the line stay at its end
        let resumed = args_with(Some(text), Some("m"), Some("sess"));
        let flags = resumed.iter().map(String::as_str).collect::<Vec<_>>();
        assert!(
            flags.ends_with(&["--model", "m", "--resume", "sess"]),
            "{flags:?}"
        );
        assert!(flags.contains(&text));
    }

    #[test]
    fn question_answers_fill_the_first_question() {
        let input = json!({"questions":[{"question":"Which db?","options":[{"label":"pg"}]}]});
        let out = question_answer_input(&input, "pg");
        assert_eq!(out["answers"]["Which db?"], "pg");
        assert_eq!(out["questions"], input["questions"]);
        assert_eq!(question_answer_input(&json!({}), "x"), json!({}));
    }

    #[test]
    fn step_titles() {
        assert_eq!(
            step_info("Bash", &json!({"command": "npx playwright test\nmore"})).1,
            "Running npx playwright test"
        );
        assert_eq!(
            step_info("Edit", &json!({"file_path": "/a.rs"})),
            (StepKind::Edit, "Editing /a.rs".into(), "/a.rs".into())
        );
        assert_eq!(step_info("mcp__x__y", &json!({})).0, StepKind::Other);
        let plan =
            plan_items(&json!({"todos":[{"content":"a","status":"completed"},{"content":"b"}]}));
        assert_eq!(plan.len(), 2);
        assert_eq!(plan[1].status, "pending");
        assert!(may_change_files("Bash") && !may_change_files("Read"));
    }

    #[test]
    fn outbound_detection() {
        let bash = |c: &str| is_outbound("Bash", &json!({"command": c}));
        assert!(bash("git push origin x"));
        assert!(bash("cargo test && git push"));
        assert!(bash("FOO=1 curl https://x"));
        assert!(bash("sudo scp a b:c"));
        assert!(bash("echo hi; gh pr create"));
        assert!(bash("npm publish"));
        assert!(!bash("cargo test"));
        assert!(!bash("git status"));
        assert!(!bash("echo curl"));
        assert!(!bash("ls | grep push"));
        assert!(is_outbound("WebFetch", &json!({})));
        assert!(is_outbound("mcp__slack__post_message", &json!({})));
        assert!(!is_outbound("mcp__files__read", &json!({})));
        assert!(!is_outbound("Edit", &json!({})));
        // A step's stored detail (the bare command) works too.
        assert!(is_outbound(
            "Bash",
            &Value::String("git push -u origin x".into())
        ));
    }
}
