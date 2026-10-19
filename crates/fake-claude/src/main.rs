//! A stand-in for Claude Code in tests. It speaks the same stream-json
//! protocol on stdin/stdout and replays a fixture: a JSONL script of ops (see
//! `fixtures/` and the README of the repo). It never talks to any network.

use serde_json::{json, Value};
use std::collections::VecDeque;
use std::io::{BufRead, Read, Write};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Args {
    raw: Vec<String>,
    fixture: Option<String>,
    resume: Option<String>,
    settings: Option<String>,
    permission_mode: Option<String>,
    data: Option<String>,
}

fn parse_args() -> Args {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut a = Args {
        raw: raw.clone(),
        ..Default::default()
    };
    let mut it = raw.iter();
    while let Some(arg) = it.next() {
        let mut take = || it.next().cloned();
        match arg.as_str() {
            "--fixture" => a.fixture = take(),
            "--resume" => a.resume = take(),
            "--settings" => a.settings = take(),
            "--permission-mode" => a.permission_mode = take(),
            "--data" => a.data = take(),
            _ => {}
        }
    }
    a
}

enum Input {
    Msg(Value),
    Eof,
}

enum Cls {
    Interrupt,
    Response(Value),
    Handled,
}

enum Flow {
    Next,
    Interrupted,
}

struct Fake {
    args: Args,
    rx: Receiver<Input>,
    pending_users: VecDeque<String>,
    last_user: String,
    session_id: String,
    cost_total: f64,
    last_response: Value,
    ctl_log: Vec<Value>,
    tool_n: u64,
    req_n: u64,
    started: Instant,
    ignore_eof: bool,
}

fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(a) => a
            .iter()
            .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn mode_matches(op: &Value, resumed: bool) -> bool {
    match op.get("mode").and_then(|m| m.as_str()) {
        Some("fresh") => !resumed,
        Some("resume") => resumed,
        _ => true,
    }
}

impl Fake {
    /// The input closed. A well-behaved agent exits; a stubborn one carries on.
    fn eof(&self) -> ! {
        if self.ignore_eof {
            loop {
                std::thread::sleep(Duration::from_secs(3600));
            }
        }
        std::process::exit(0)
    }

    fn emit(&self, v: Value) {
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{v}");
        let _ = out.flush();
    }

    fn assistant(&self, blocks: Vec<Value>) {
        self.emit(
            json!({"type":"assistant","message":{"role":"assistant","content":blocks},
            "parent_tool_use_id":null,"session_id":self.session_id}),
        );
    }

    fn say(&self, text: &str) {
        self.assistant(vec![json!({"type":"text","text":text})]);
    }

    fn control_ok(&self, request_id: &str) {
        self.emit(json!({"type":"control_response","response":{
            "subtype":"success","request_id":request_id,"response":{}}}));
    }

    fn next_input(&self, timeout: Option<Duration>) -> Option<Input> {
        match timeout {
            None => Some(self.rx.recv().unwrap_or(Input::Eof)),
            Some(t) => match self.rx.recv_timeout(t) {
                Ok(i) => Some(i),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => Some(Input::Eof),
            },
        }
    }

    fn classify(&mut self, m: Value) -> Cls {
        match m.get("type").and_then(|t| t.as_str()) {
            Some("user") => {
                self.pending_users.push_back(text_of(
                    m.pointer("/message/content").unwrap_or(&Value::Null),
                ));
                Cls::Handled
            }
            Some("control_request") => {
                let id = m
                    .get("request_id")
                    .and_then(|i| i.as_str())
                    .unwrap_or("")
                    .to_string();
                let req = m.get("request").cloned().unwrap_or(Value::Null);
                self.control_ok(&id);
                if req.get("subtype").and_then(|s| s.as_str()) == Some("interrupt") {
                    Cls::Interrupt
                } else {
                    self.ctl_log.push(req);
                    Cls::Handled
                }
            }
            Some("control_response") => Cls::Response(m),
            _ => Cls::Handled,
        }
    }

    fn block(&mut self, ops: &[Value]) -> Flow {
        let resumed = self.args.resume.is_some();
        for op in ops.iter().filter(|o| mode_matches(o, resumed)) {
            if let Flow::Interrupted = self.exec(op) {
                return Flow::Interrupted;
            }
        }
        Flow::Next
    }

    fn exec(&mut self, op: &Value) -> Flow {
        let s = |k: &str| op.get(k).and_then(|v| v.as_str()).map(str::to_string);
        match s("op").unwrap_or_default().as_str() {
            "init" => {
                let slash = op.get("slash_commands").cloned().unwrap_or(json!(["/compact", "/review", "/login"]));
                self.emit(json!({"type":"system","subtype":"init","session_id":self.session_id,
                    "cwd":std::env::current_dir().map(|p| p.to_string_lossy().to_string()).unwrap_or_default(),
                    "model":"test-model","permissionMode":self.args.permission_mode,
                    "slash_commands":slash,"tools":["Bash","Read","Edit","Write"],"mcp_servers":[],
                    "apiKeySource":"none"}));
            }
            "wait_user" => loop {
                if let Some(u) = self.pending_users.pop_front() {
                    self.last_user = u;
                    break;
                }
                match self.next_input(None) {
                    Some(Input::Eof) | None => self.eof(),
                    Some(Input::Msg(m)) => {
                        // An interrupt while idle has nothing to stop.
                        let _ = self.classify(m);
                    }
                }
            },
            "say" => self.say(&s("text").unwrap_or_default()),
            "say_user" => self.say(&format!("{}{}", s("prefix").unwrap_or_else(|| "got: ".into()), self.last_user)),
            "say_args" => self.say(&format!("args: {}", self.args.raw.join(" "))),
            "say_settings" => {
                let text = self
                    .args
                    .settings
                    .as_ref()
                    .and_then(|p| std::fs::read_to_string(p).ok())
                    .unwrap_or_else(|| "no settings".into());
                self.say(&format!("settings: {text}"));
            }
            "say_response" => self.say(&format!("response: {}", self.last_response)),
            "say_ctl" => self.say(&format!("ctl: {}", Value::Array(self.ctl_log.clone()))),
            "say_env" => {
                let name = s("name").unwrap_or_default();
                self.say(&format!("env {name}={}", std::env::var(&name).unwrap_or_default()));
            }
            "say_pid" => self.say(&format!("pid: {}", std::process::id())),
            "tool" | "tool_use" | "tool_result" => {
                let kind = s("op").unwrap_or_default();
                self.tool_n += 1;
                let id = s("id").unwrap_or_else(|| format!("toolu_{}", self.tool_n));
                if kind != "tool_result" {
                    self.assistant(vec![json!({"type":"tool_use","id":id,
                        "name":s("name").unwrap_or_else(|| "Bash".into()),
                        "input":op.get("input").cloned().unwrap_or(json!({}))})]);
                }
                let result = match (s("result"), op.get("result_bytes").and_then(|n| n.as_u64())) {
                    (Some(r), _) => Some(r),
                    (None, Some(n)) => Some((0..n).map(|i| char::from(b'a' + (i % 26) as u8)).collect()),
                    _ => None,
                };
                if kind != "tool_use" {
                    if let Some(r) = result {
                        self.emit(json!({"type":"user","message":{"role":"user","content":[
                            {"type":"tool_result","tool_use_id":id,"content":r,
                             "is_error":op.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false)}]},
                            "session_id":self.session_id}));
                    }
                }
            }
            "delta" => self.emit(json!({"type":"stream_event","event":{"type":"content_block_delta","index":0,
                "delta":{"type":"text_delta","text":s("text").unwrap_or_default()}},"session_id":self.session_id})),
            "emit" => self.emit(op.get("line").cloned().unwrap_or(Value::Null)),
            "touch_file" | "append_file" => {
                let path = s("path").unwrap_or_default();
                let content = s("content").unwrap_or_default();
                if let Some(dir) = std::path::Path::new(&path).parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                if s("op").as_deref() == Some("append_file") {
                    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
                        let _ = f.write_all(content.as_bytes());
                    }
                } else {
                    let _ = std::fs::write(&path, content);
                }
            }
            "spawn_sleeper" => {
                // A child in our process group, like a dev server or a test run.
                match std::process::Command::new("sleep").arg(s("secs").unwrap_or_else(|| "300".into())).spawn() {
                    Ok(c) => self.say(&format!("sleeper: {}", c.id())),
                    Err(e) => self.say(&format!("sleeper failed: {e}")),
                }
            }
            "ignore_eof" => self.ignore_eof = true,
            "ignore_sigterm" => unsafe {
                libc::signal(libc::SIGTERM, libc::SIG_IGN);
            },
            "stderr" => eprintln!("{}", s("text").unwrap_or_default()),
            "sleep" => {
                let deadline = Instant::now() + Duration::from_millis(op.get("ms").and_then(|m| m.as_u64()).unwrap_or(0));
                loop {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        break;
                    }
                    match self.next_input(Some(left)) {
                        None => break,
                        Some(Input::Eof) => self.eof(),
                        Some(Input::Msg(m)) => {
                            if let Cls::Interrupt = self.classify(m) {
                                return Flow::Interrupted;
                            }
                        }
                    }
                }
            }
            "ask" => {
                self.req_n += 1;
                let req_id = format!("req_{}_{}", std::process::id(), self.req_n);
                let mut request = json!({"subtype":"can_use_tool",
                    "tool_name":s("tool").unwrap_or_else(|| "Bash".into()),
                    "input":op.get("input").cloned().unwrap_or(json!({}))});
                for (key, field) in [
                    ("display_name", "display_name"),
                    ("tool_use_id", "tool_use_id"),
                    ("description", "description"),
                    ("blocked_path", "blocked_path"),
                ] {
                    if let Some(v) = op.get(field) {
                        request[key] = v.clone();
                    }
                }
                if let Some(v) = op.get("suggestions") {
                    request["permission_suggestions"] = v.clone();
                }
                self.emit(json!({"type":"control_request","request_id":req_id,"request":request}));
                let response = loop {
                    match self.next_input(None) {
                        Some(Input::Eof) | None => self.eof(),
                        Some(Input::Msg(m)) => match self.classify(m) {
                            Cls::Interrupt => return Flow::Interrupted,
                            Cls::Response(r) if r.pointer("/response/request_id").and_then(|i| i.as_str()) == Some(&req_id) => {
                                break r;
                            }
                            _ => {}
                        },
                    }
                };
                let inner = response.pointer("/response/response").cloned().unwrap_or(Value::Null);
                self.last_response = inner.clone();
                let branch = match inner.get("behavior").and_then(|b| b.as_str()) {
                    Some("allow") => "allow",
                    _ => "deny",
                };
                if let Some(ops) = op.get(branch).and_then(|b| b.as_array()).cloned() {
                    return self.block(&ops);
                }
            }
            "rpc" => {
                use std::os::unix::net::UnixStream;
                let env = s("socket_env").unwrap_or_else(|| "SPHERE_SOCKET".into());
                let method = s("method").unwrap_or_default();
                let reply = match std::env::var(&env).map(UnixStream::connect) {
                    Ok(Ok(mut st)) => {
                        let req = json!({"jsonrpc":"2.0","id":1,"method":method,"params":op.get("params").cloned().unwrap_or(json!({}))});
                        let _ = writeln!(st, "{req}");
                        let mut line = String::new();
                        let _ = std::io::BufReader::new(st).read_line(&mut line);
                        line.trim().to_string()
                    }
                    _ => "no socket".to_string(),
                };
                self.say(&format!("rpc {method}: {reply}"));
            }
            "result" => {
                self.cost_total += op.get("cost").and_then(|c| c.as_f64()).unwrap_or(0.01);
                let is_error = op.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false);
                let subtype = s("subtype").unwrap_or_else(|| if is_error { "error_during_execution".into() } else { "success".into() });
                self.emit(json!({"type":"result","subtype":subtype,"is_error":is_error,
                    "duration_ms":self.started.elapsed().as_millis() as u64,"num_turns":op.get("turns").and_then(|t| t.as_u64()).unwrap_or(1),
                    "result":s("text").unwrap_or_else(|| "done".into()),"session_id":self.session_id,
                    "total_cost_usd":self.cost_total}));
            }
            "exit" => std::process::exit(op.get("code").and_then(|c| c.as_i64()).unwrap_or(0) as i32),
            other => eprintln!("fake-claude: unknown op {other:?}"),
        }
        Flow::Next
    }

    fn run(&mut self, ops: Vec<Value>) {
        let resumed = self.args.resume.is_some();
        let ops: Vec<Value> = ops
            .into_iter()
            .filter(|o| mode_matches(o, resumed))
            .collect();
        let mut pc = 0;
        while pc < ops.len() {
            match self.exec(&ops[pc]) {
                Flow::Next => pc += 1,
                Flow::Interrupted => {
                    self.cost_total += 0.001;
                    self.emit(json!({"type":"result","subtype":"error_during_execution","is_error":true,
                        "duration_ms":0,"num_turns":1,"result":"interrupted","session_id":self.session_id,
                        "total_cost_usd":self.cost_total}));
                    // Carry on from the next time the script waits for the user.
                    pc = (pc + 1..ops.len())
                        .find(|i| ops[*i].get("op").and_then(|o| o.as_str()) == Some("wait_user"))
                        .unwrap_or(ops.len());
                }
            }
        }
        // Script over: stay alive between turns until the input closes.
        loop {
            match self.next_input(None) {
                Some(Input::Eof) | None => self.eof(),
                Some(Input::Msg(m)) => {
                    let _ = self.classify(m);
                }
            }
        }
    }
}

fn main() {
    let args = parse_args();
    // Stand-ins for the subcommands sphered calls.
    match args.raw.first().map(String::as_str) {
        Some("--version") => return println!("2.1.0 (fake-claude)"),
        Some("agents") => {
            let text = args
                .data
                .as_ref()
                .and_then(|p| std::fs::read_to_string(p).ok())
                .unwrap_or_else(|| "[]".into());
            return print!("{text}");
        }
        Some("auth") => {
            let text = args
                .data
                .as_ref()
                .and_then(|p| std::fs::read_to_string(p).ok())
                .unwrap_or_else(|| r#"{"loggedIn":true,"authMethod":"fake"}"#.into());
            return print!("{text}");
        }
        // Stands in for a person's terminal session: stays alive until its
        // folder is gone (a finished test) or ten minutes pass.
        _ if args.raw.iter().any(|a| a == "--idle-forever") => {
            let dir = std::env::current_dir().ok();
            let end = Instant::now() + Duration::from_secs(600);
            while Instant::now() < end && dir.as_ref().is_some_and(|d| d.exists()) {
                std::thread::sleep(Duration::from_millis(500));
            }
            return;
        }
        Some("stop") => return,
        _ => {}
    }
    let Some(path) = args.fixture.clone() else {
        eprintln!("fake-claude: --fixture <file> is required");
        std::process::exit(2);
    };
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        eprintln!("fake-claude: cannot read {path}: {e}");
        std::process::exit(2);
    });
    let ops: Vec<Value> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("//"))
        .map(|l| {
            serde_json::from_str(l).unwrap_or_else(|e| {
                eprintln!("fake-claude: bad fixture line {l:?}: {e}");
                std::process::exit(2);
            })
        })
        .collect();

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut buf = String::new();
        let mut lock = stdin.lock();
        loop {
            buf.clear();
            match lock.by_ref().take(16 * 1024 * 1024).read_line(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if let Ok(v) = serde_json::from_str::<Value>(buf.trim()) {
                        if tx.send(Input::Msg(v)).is_err() {
                            return;
                        }
                    }
                }
            }
        }
        let _ = tx.send(Input::Eof);
    });

    let session_id = args
        .resume
        .clone()
        .unwrap_or_else(|| format!("fake-session-{}", std::process::id()));
    let mut fake = Fake {
        args,
        rx,
        pending_users: VecDeque::new(),
        last_user: String::new(),
        session_id,
        cost_total: 0.0,
        last_response: Value::Null,
        ctl_log: vec![],
        tool_n: 0,
        req_n: 0,
        started: Instant::now(),
        ignore_eof: false,
    };
    fake.run(ops);
}
