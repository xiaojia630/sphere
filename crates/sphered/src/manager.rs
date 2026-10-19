//! The supervisor: starts agents through the host, reads their output into the
//! event log, turns permission asks into requests, and carries answers back.
//!
//! sphered keeps no agent state that the log and the host's files cannot
//! rebuild, so restarting it interrupts nothing: it reconnects to the host and
//! reads on from the offsets it saved in the same transaction as the events.

use crate::agent_proto::{self as proto, Block, CanUseTool, Incoming};
use crate::config::Config;
use crate::host_link::HostLink;
use crate::hub::Hub;
use crate::policy::{self, Ctx, Policy, Verdict};
use crate::store::OffsetUpdate;
use crate::{sessions, workspace};
use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use sphere_host::{ExitStatus, FollowItem, SpawnParams, Stream};
use sphere_proto::api::{self, kind, RpcError};
use sphere_proto::version;
use sphere_proto::{
    now_rfc3339, AnswerKind, EventBody, Excerpt, OutputRef, RequestInfo, RequestKind,
    RequestStatus, TaskState, TaskView,
};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{oneshot, watch};
use tokio::task::AbortHandle;

/// An error an API call reports to its client.
#[derive(Debug)]
pub struct ApiError(pub RpcError);

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        ApiError(RpcError {
            code: api::code::INTERNAL,
            message: format!("{e:#}"),
            data: None,
        })
    }
}

fn api_err(kind: &str, message: impl Into<String>) -> ApiError {
    ApiError(RpcError::app(kind, message))
}

type ApiResult<T> = Result<T, ApiError>;

const MODES: &[&str] = &["default", "acceptEdits", "plan", "dontAsk"];

/// The longest a wait for the end of the day sleeps before it looks at the
/// clock again: timers stand still while a Mac sleeps, and the day may have
/// ended meanwhile.
const CAP_RECHECK: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
struct CtlReply {
    ok: bool,
    error: Option<String>,
}

struct TaskRt {
    stopping: bool,
    interrupted: bool,
    idle_exit: bool,
    idle_timer: Option<AbortHandle>,
    exit_tx: watch::Sender<bool>,
}

#[derive(Default)]
struct Rt {
    tasks: HashMap<String, TaskRt>,
    readers: HashMap<String, AbortHandle>,
    /// Tasks whose agent is being started right now.
    launching: HashSet<String>,
    /// The wait for the daily cap to end, while queued tasks sit it out.
    cap_wait: Option<AbortHandle>,
}

#[derive(Default)]
struct Info {
    claude_version: Option<String>,
    signed_in: Option<bool>,
    warnings: Vec<String>,
}

pub struct Manager {
    pub cfg: Arc<Config>,
    pub hub: Arc<Hub>,
    pub host: HostLink,
    rt: Mutex<Rt>,
    answer_lock: tokio::sync::Mutex<()>,
    ctl: Mutex<HashMap<String, oneshot::Sender<CtlReply>>>,
    ctl_seq: AtomicU64,
    info: Mutex<Info>,
}

/// What one line of agent output turns into.
#[derive(Debug, Default)]
pub struct Plan {
    pub events: Vec<EventBody>,
    /// Lines to write back to the agent.
    pub writes: Vec<String>,
    pub live: Vec<String>,
    pub refresh_files: bool,
    pub turn_ended: bool,
    pub ctl: Option<(String, bool, Option<String>)>,
}

/// Everything `plan_line` may look at.
pub struct LineCtx<'a> {
    pub view: &'a TaskView,
    pub policy: &'a Policy,
    pub open_requests: &'a [RequestInfo],
    pub revises: Option<String>,
    pub interrupted: bool,
    pub child: &'a str,
    pub offset: u64,
    pub len: u64,
    pub excerpt_bytes: usize,
    pub new_request_id: &'a dyn Fn() -> String,
}

fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// Decide what to do about one parsed line. Pure apart from allocating ids.
pub fn plan_line(cx: &LineCtx, inc: Incoming) -> Plan {
    let mut plan = Plan::default();
    let task = &cx.view.id;
    match inc {
        Incoming::Init {
            session_id,
            model,
            permission_mode,
            slash_commands,
        } => {
            if let Some(id) = session_id {
                if cx.view.session_id.as_deref() != Some(id.as_str()) {
                    plan.events.push(EventBody::SessionId { session_id: id });
                }
            }
            plan.events.push(EventBody::Init {
                model,
                permission_mode,
                slash_commands,
            });
        }
        Incoming::Assistant { blocks, parent } => {
            for b in blocks {
                match b {
                    Block::Text(t) if !t.trim().is_empty() => {
                        plan.events.push(EventBody::Say { text: t })
                    }
                    Block::Text(_) => {}
                    Block::ToolUse { id, name, input } => {
                        let (step_kind, title, detail) = proto::step_info(&name, &input);
                        if name == "TodoWrite" {
                            plan.events.push(EventBody::Plan {
                                items: proto::plan_items(&input),
                            });
                        }
                        plan.events.push(EventBody::Step {
                            id,
                            step_kind,
                            tool: name,
                            title,
                            detail,
                            parent: parent.clone(),
                        });
                    }
                }
            }
        }
        Incoming::ToolResults(results) => {
            for r in results {
                let tool = cx
                    .view
                    .open_steps
                    .iter()
                    .find(|s| s.id == r.tool_use_id)
                    .map(|s| s.tool.clone());
                if tool
                    .as_deref()
                    .map(proto::may_change_files)
                    .unwrap_or(false)
                {
                    plan.refresh_files = true;
                }
                let output = (!r.text.is_empty()).then(|| {
                    Excerpt::new(
                        &r.text,
                        cx.excerpt_bytes,
                        Some(OutputRef {
                            child: cx.child.to_string(),
                            offset: cx.offset,
                            len: cx.len,
                        }),
                    )
                });
                plan.events.push(EventBody::StepDone {
                    id: r.tool_use_id,
                    ok: !r.is_error,
                    output,
                });
            }
        }
        Incoming::Delta(t) => plan.live.push(t),
        Incoming::Result {
            ok,
            text,
            turns,
            duration_ms,
            total_cost_usd,
            ..
        } => {
            // An agent that finished its turn is not waiting on anything.
            for r in cx
                .open_requests
                .iter()
                .filter(|r| r.kind != RequestKind::ResumeCheck)
            {
                plan.events.push(EventBody::RequestWithdrawn {
                    request: r.id.clone(),
                    reason: "turn ended".into(),
                });
            }
            let delta = total_cost_usd
                .map(|c| (c - cx.view.last_total_cost).max(0.0))
                .unwrap_or(0.0);
            plan.events.push(EventBody::Turn {
                ok,
                interrupted: cx.interrupted,
                summary: text,
                turns,
                duration_ms,
                cost_usd: total_cost_usd,
                cost_delta_usd: delta,
            });
            plan.turn_ended = true;
        }
        Incoming::CanUseTool(c) => plan_permission(cx, c, &mut plan),
        Incoming::OtherControl {
            request_id,
            subtype,
        } => {
            tracing::debug!(%subtype, "unsupported control request from the agent");
            plan.writes.push(proto::control_error(
                &request_id,
                "not supported by this host",
            ));
        }
        Incoming::ControlResponse {
            request_id,
            ok,
            error,
            ..
        } => plan.ctl = Some((request_id, ok, error)),
        Incoming::ControlCancel { request_id } => {
            for r in cx
                .open_requests
                .iter()
                .filter(|r| r.agent_request.as_deref() == Some(request_id.as_str()))
            {
                plan.events.push(EventBody::RequestWithdrawn {
                    request: r.id.clone(),
                    reason: "cancelled by the agent".into(),
                });
            }
        }
        Incoming::Other(ty) => tracing::debug!(%task, %ty, "ignoring agent message"),
    }
    plan
}

fn plan_permission(cx: &LineCtx, c: CanUseTool, plan: &mut Plan) {
    let ctx = Ctx {
        branch: cx.view.branch.as_deref(),
    };
    match cx.policy.decide(&c.tool_name, &c.input, &ctx) {
        Verdict::Allow(rule) => {
            plan.writes
                .push(proto::allow_response(&c.request_id, &c.input, None));
            plan.events.push(EventBody::AutoAnswered {
                tool: c.tool_name,
                allow: true,
                rule,
            });
        }
        Verdict::Deny(rule) => {
            plan.writes.push(proto::deny_response(
                &c.request_id,
                &format!("Denied by Sphere policy ({rule})."),
            ));
            plan.events.push(EventBody::AutoAnswered {
                tool: c.tool_name,
                allow: false,
                rule,
            });
        }
        Verdict::Ask => {
            let summary = c
                .description
                .clone()
                .filter(|d| !d.trim().is_empty())
                .unwrap_or_else(|| truncate(&cx.view.last_say, 300));
            plan.events.push(EventBody::Request {
                request: RequestInfo {
                    id: (cx.new_request_id)(),
                    task: cx.view.id.clone(),
                    kind: RequestKind::for_tool(&c.tool_name),
                    tool: c.tool_name,
                    display_name: c.display_name,
                    description: c.description,
                    summary,
                    input: c.input,
                    suggestions: c.suggestions,
                    blocked_path: c.blocked_path,
                    tool_use_id: c.tool_use_id,
                    agent_request: Some(c.request_id),
                    revises: cx.revises.clone(),
                },
            });
        }
    }
}

fn describe_exit(s: &ExitStatus) -> String {
    match (s.code, s.signal) {
        (Some(c), _) => format!("exit code {c}"),
        (None, Some(sig)) => format!("signal {sig}"),
        _ => "unknown status".into(),
    }
}

fn today() -> String {
    now_rfc3339()[..10].to_string()
}

/// How long `today()` stays the same: the time left in the UTC day.
fn until_next_day() -> Duration {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    Duration::from_secs(86_400 - now.as_secs() % 86_400)
}

async fn wait_exit(rx: &mut watch::Receiver<bool>, dur: Duration) -> bool {
    tokio::time::timeout(dur, async {
        while !*rx.borrow() {
            if rx.changed().await.is_err() {
                return;
            }
        }
    })
    .await
    .is_ok()
}

impl Manager {
    pub fn new(cfg: Arc<Config>, hub: Arc<Hub>) -> Arc<Manager> {
        Arc::new(Manager {
            host: HostLink::new(cfg.clone()),
            cfg,
            hub,
            rt: Mutex::new(Rt::default()),
            answer_lock: tokio::sync::Mutex::new(()),
            ctl: Mutex::new(HashMap::new()),
            ctl_seq: AtomicU64::new(0),
            info: Mutex::new(Info::default()),
        })
    }

    // ---- start-up ------------------------------------------------------------

    /// Check the install, reconnect to the host, pick up whatever is running
    /// and resume whatever is not.
    pub async fn start(self: &Arc<Self>) -> Result<()> {
        let this = self.clone();
        tokio::spawn(async move { this.check_install().await });
        self.host.get().await.context("start sphere-host")?;
        self.recover_all().await?;
        self.pump();
        Ok(())
    }

    fn warn(&self, message: String) {
        tracing::warn!("{message}");
        self.info.lock().unwrap().warnings.push(message.clone());
        let _ = self.hub.publish("", EventBody::Warning { message });
    }

    async fn check_install(self: &Arc<Self>) {
        if self.cfg.version_check_applies() {
            match run_capture(&[self.cfg.agent_command.clone(), "--version".into()]).await {
                Ok(text) => {
                    self.info.lock().unwrap().claude_version = Some(text.trim().to_string());
                    match version::parse(&text) {
                        Some(v) if version::in_tested_range(v) => {}
                        Some(v) => self.warn(version::out_of_range_message(v)),
                        None => {
                            self.warn(format!("could not read a version from `{}`", text.trim()))
                        }
                    }
                }
                Err(e) => self.warn(format!(
                    "could not run `{} --version`: {e:#}",
                    self.cfg.agent_command
                )),
            }
        }
        match run_capture(&self.cfg.auth_status_cmd()).await {
            Ok(text) => {
                let signed = serde_json::from_str::<Value>(&text)
                    .ok()
                    .and_then(|v| v.get("loggedIn").and_then(|l| l.as_bool()));
                self.info.lock().unwrap().signed_in = signed;
                if signed == Some(false) {
                    self.warn("Claude Code is signed out; run `claude auth login`".into());
                }
            }
            Err(e) => tracing::debug!("auth status unavailable: {e:#}"),
        }
    }

    async fn recover_all(self: &Arc<Self>) -> Result<()> {
        let children = self.host.list().await?;
        let tasks: Vec<TaskView> = self.hub.view(|v| v.tasks().into_iter().cloned().collect());
        let referenced: HashSet<String> = tasks.iter().filter_map(|t| t.child.clone()).collect();
        // A child that was spawned but never recorded would run unwatched.
        for c in children
            .iter()
            .filter(|c| c.running && !referenced.contains(&c.id))
        {
            if c.label.as_deref().is_some_and(|l| l.starts_with("task:")) {
                tracing::warn!(child = %c.id, "killing an agent that was never recorded");
                let _ = self.host.signal(&c.id, json!("KILL"), true).await;
            }
        }
        for t in tasks {
            if let Some(child) = &t.child {
                if children.iter().any(|c| &c.id == child) {
                    let from = self.hub.offset(child, "out")?;
                    tracing::info!(task = %t.id, %child, from, "reattaching to a running agent");
                    self.attach(&t.id, child, from);
                    if matches!(t.state, TaskState::Done | TaskState::Failed) {
                        self.start_idle_timer(&t.id);
                    }
                    continue;
                }
                self.clone().resume_lost(t.id.clone()).await;
            } else if t.state.is_active()
                && self.hub.view(|v| v.open_requests_for(&t.id).is_empty())
            {
                // Crashed between steps with nothing recorded: start it again.
                self.clone().resume_lost(t.id.clone()).await;
            }
        }
        Ok(())
    }

    pub fn abort_all(&self) {
        let rt = self.rt.lock().unwrap();
        for h in rt.readers.values() {
            h.abort();
        }
        for t in rt.tasks.values() {
            if let Some(h) = &t.idle_timer {
                h.abort();
            }
        }
        if let Some(h) = &rt.cap_wait {
            h.abort();
        }
    }

    // ---- scheduling ----------------------------------------------------------

    fn capped(&self) -> bool {
        self.cfg
            .daily_cap_usd
            .map(|cap| self.hub.view(|v| v.cost_on(&today())) >= cap)
            .unwrap_or(false)
    }

    fn has_queued(&self) -> bool {
        self.hub.view(|v| !v.queued().is_empty())
    }

    /// Start queued tasks while there are free slots. Idle agents hold none.
    ///
    /// Whatever can make room (a task arriving, a turn ending, an agent
    /// exiting or failing to start) calls this itself; nothing polls. Only the
    /// daily cap ends without an event.
    pub fn pump(self: &Arc<Self>) {
        if self.capped() {
            self.wait_out_cap();
            return;
        }
        let start: Vec<String> = {
            let mut rt = self.rt.lock().unwrap();
            let launching = rt.launching.clone();
            let (active, queued) = self.hub.view(|v| {
                let queued_launching = launching
                    .iter()
                    .filter(|t| v.task(t).is_some_and(|t| t.state == TaskState::Queued))
                    .count();
                let queued: Vec<String> = v
                    .queued()
                    .into_iter()
                    .filter(|t| !launching.contains(&t.id))
                    .map(|t| t.id.clone())
                    .collect();
                (v.active_count() + queued_launching, queued)
            });
            let free = self.cfg.max_concurrent.saturating_sub(active);
            let pick: Vec<String> = queued.into_iter().take(free).collect();
            rt.launching.extend(pick.iter().cloned());
            pick
        };
        for task in start {
            let this = self.clone();
            tokio::spawn(async move { this.launch(task, None).await });
        }
    }

    /// The cap ends with the UTC day, which nothing announces: queued tasks it
    /// holds back sleep until then and pump again. With none queued, nothing
    /// waits.
    fn wait_out_cap(self: &Arc<Self>) {
        if !self.has_queued() {
            return;
        }
        let mut rt = self.rt.lock().unwrap();
        if rt.cap_wait.as_ref().is_some_and(|h| !h.is_finished()) {
            return;
        }
        let this = self.clone();
        let wait = tokio::spawn(async move {
            while this.capped() && this.has_queued() {
                tokio::time::sleep(until_next_day().min(CAP_RECHECK)).await;
            }
            this.rt.lock().unwrap().cap_wait = None;
            this.pump();
        });
        rt.cap_wait = Some(wait.abort_handle());
    }

    async fn launch(self: Arc<Self>, task: String, message: Option<String>) {
        let result = self.launch_inner(&task, message).await;
        self.rt.lock().unwrap().launching.remove(&task);
        if let Err(e) = result {
            tracing::warn!(%task, "could not start the agent: {e:#}");
            self.fail(&task, format!("could not start the agent: {e:#}"));
        }
        self.pump();
    }

    fn fail(&self, task: &str, message: String) {
        let stopped = self
            .hub
            .view(|v| v.task(task).map(|t| t.state == TaskState::Stopped));
        if stopped == Some(false) {
            let _ = self.hub.publish(task, EventBody::Failed { message });
        }
    }

    fn policy_for(&self, repo: &str) -> Policy {
        let repo_file = Path::new(repo).join(".sphere").join("policy.toml");
        Policy::load(&self.cfg.policy_path(), Some(&repo_file)).unwrap_or_else(|e| {
            tracing::warn!("policy files unreadable, using the defaults: {e:#}");
            Policy::default()
        })
    }

    async fn launch_inner(self: &Arc<Self>, task: &str, message: Option<String>) -> Result<()> {
        let view = self
            .hub
            .view(|v| v.task(task).cloned())
            .context("unknown task")?;
        if view.state == TaskState::Stopped {
            return Ok(());
        }
        let workdir: PathBuf = match &view.workdir {
            Some(w) => PathBuf::from(w),
            None => {
                let (wt, repo, id, title, in_place) = (
                    self.cfg.worktrees_dir(),
                    PathBuf::from(&view.repo),
                    task.to_string(),
                    view.title.clone(),
                    view.in_place,
                );
                let ws = tokio::task::spawn_blocking(move || {
                    workspace::prepare(&wt, &repo, &id, &title, in_place)
                })
                .await??;
                self.hub.publish(
                    task,
                    EventBody::WorkspaceReady {
                        workdir: ws.workdir.to_string_lossy().to_string(),
                        branch: ws.branch.clone(),
                        base: ws.base.clone(),
                    },
                )?;
                ws.workdir
            }
        };

        let settings = self
            .policy_for(&view.repo)
            .settings_json(view.allow_rewrites);
        let dir = self.cfg.settings_dir();
        crate::private_dir(&dir)?;
        let settings_path = dir.join(format!("{task}.json"));
        std::fs::write(&settings_path, serde_json::to_vec_pretty(&settings)?)?;

        let resume = view.session_id.clone();
        let text = match (&message, &resume) {
            (Some(m), _) => m.clone(),
            (None, Some(_)) => "continue".to_string(),
            (None, None) => view.prompt.clone(),
        };
        let gen = proto::build_args(&proto::AgentArgs {
            settings: &settings_path.to_string_lossy(),
            budget_usd: view.budget_usd,
            mode: view.mode.as_deref().unwrap_or("acceptEdits"),
            model: view.model.as_deref(),
            resume: resume.as_deref(),
        });
        let mut args = self.cfg.agent_args.clone();
        args.extend(gen);
        let mut env = std::collections::BTreeMap::new();
        env.insert(
            "SPHERE_SOCKET".to_string(),
            self.cfg.socket_path().to_string_lossy().to_string(),
        );
        env.insert("SPHERE_TASK".to_string(), task.to_string());
        // sphere-host may have been started with a bare PATH (it outlives sphered): the agent gets ours, which
        // has the user's login-shell PATH (see `envpath`), to find `claude` and the tools it runs
        if let Ok(path) = std::env::var("PATH") {
            env.insert("PATH".to_string(), path);
        }
        let spawned = self
            .host
            .spawn(SpawnParams {
                cmd: self.cfg.agent_command.clone(),
                args,
                cwd: Some(workdir.to_string_lossy().to_string()),
                env,
                pty: None,
                label: Some(format!("task:{task}")),
            })
            .await?;

        // Stopped while we were starting it?
        if self.hub.view(|v| v.task(task).map(|t| t.state)) == Some(TaskState::Stopped) {
            let _ = self.host.signal(&spawned.id, json!("KILL"), true).await;
            return Ok(());
        }
        self.hub.commit(
            task,
            vec![EventBody::AgentStarted {
                child: spawned.id.clone(),
                pid: spawned.pid,
                resumed: resume.is_some(),
            }],
            Some(OffsetUpdate {
                child: spawned.id.clone(),
                stream: "out",
                offset: 0,
            }),
        )?;
        self.attach(task, &spawned.id, 0);
        self.host
            .write(&spawned.id, proto::user_message(&text).as_bytes())
            .await?;
        self.hub.publish(task, EventBody::UserMessage { text })?;
        Ok(())
    }

    // ---- reading agent output --------------------------------------------------

    fn attach(self: &Arc<Self>, task: &str, child: &str, from: u64) {
        let (exit_tx, _) = watch::channel(false);
        let this = self.clone();
        let (t, c, tx) = (task.to_string(), child.to_string(), exit_tx.clone());
        let handle = tokio::spawn(async move { this.reader(t, c, from, tx).await });
        let mut rt = self.rt.lock().unwrap();
        if let Some(old) = rt.tasks.insert(
            task.to_string(),
            TaskRt {
                stopping: false,
                interrupted: false,
                idle_exit: false,
                idle_timer: None,
                exit_tx,
            },
        ) {
            if let Some(h) = old.idle_timer {
                h.abort();
            }
        }
        if let Some(old) = rt.readers.insert(task.to_string(), handle.abort_handle()) {
            old.abort();
        }
    }

    async fn reader(
        self: Arc<Self>,
        task: String,
        child: String,
        mut offset: u64,
        exit_tx: watch::Sender<bool>,
    ) {
        let err_tail = Arc::new(Mutex::new(String::new()));
        let err_reader = {
            let (this, child, tail) = (self.clone(), child.clone(), err_tail.clone());
            tokio::spawn(async move {
                let Ok(mut f) = this.host.follow(&child, Stream::Err, 0).await else {
                    return;
                };
                while let Some(Ok(item)) = f.next().await {
                    if let FollowItem::Data { bytes, .. } = item {
                        let mut t = tail.lock().unwrap();
                        t.push_str(&String::from_utf8_lossy(&bytes));
                        let keep = t.len().saturating_sub(2000);
                        if keep > 0 {
                            let cut = (keep..t.len())
                                .find(|i| t.is_char_boundary(*i))
                                .unwrap_or(t.len());
                            t.drain(..cut);
                        }
                    }
                }
            })
        };
        loop {
            let follow = self.host.follow(&child, Stream::Out, offset).await;
            let mut buf: Vec<u8> = vec![];
            let mut buf_start = offset;
            if let Ok(mut f) = follow {
                loop {
                    match f.next().await {
                        Some(Ok(FollowItem::Data { offset: o, bytes })) => {
                            if buf.is_empty() {
                                buf_start = o;
                            }
                            buf.extend(bytes);
                            while let Some(nl) = buf.iter().position(|b| *b == b'\n') {
                                let line: Vec<u8> = buf.drain(..=nl).collect();
                                let end = buf_start + line.len() as u64;
                                self.handle_line(&task, &child, buf_start, end, &line).await;
                                buf_start = end;
                                offset = end;
                            }
                        }
                        Some(Ok(FollowItem::Exit(status))) => {
                            if !buf.is_empty() {
                                let end = buf_start + buf.len() as u64;
                                self.handle_line(&task, &child, buf_start, end, &buf).await;
                            }
                            // Let the stderr reader catch up before we quote it.
                            let _ =
                                tokio::time::timeout(Duration::from_millis(300), err_reader).await;
                            let tail = err_tail.lock().unwrap().clone();
                            self.on_exit(&task, status, tail, &exit_tx).await;
                            return;
                        }
                        Some(Err(_)) | None => break,
                    }
                }
            }
            // The connection to the host dropped. Find out whether the host
            // (and the child) are still there.
            tokio::time::sleep(Duration::from_millis(300)).await;
            match self.host.recover().await {
                Ok(_) => match self.host.list().await {
                    Ok(list) if list.iter().any(|c| c.id == child) => continue,
                    Ok(_) => {
                        self.clone().resume_lost(task.clone()).await;
                        return;
                    }
                    Err(e) => tracing::warn!("host list failed: {e:#}"),
                },
                Err(e) => tracing::warn!("host unreachable: {e:#}"),
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    async fn handle_line(
        self: &Arc<Self>,
        task: &str,
        child: &str,
        start: u64,
        end: u64,
        raw: &[u8],
    ) {
        let text = String::from_utf8_lossy(raw);
        let line = text.trim();
        if line.is_empty() {
            return;
        }
        let inc = match proto::parse_line(line) {
            Ok(i) => i,
            Err(e) => {
                tracing::debug!(%task, "unreadable agent line: {e}");
                return;
            }
        };
        let Some(view) = self.hub.view(|v| v.task(task).cloned()) else {
            return;
        };
        let open: Vec<RequestInfo> = self.hub.view(|v| {
            v.open_requests_for(task)
                .iter()
                .map(|r| r.info.clone())
                .collect()
        });
        let revises = match &inc {
            Incoming::CanUseTool(c) => self.hub.view(|v| v.revises(task, &c.tool_name)),
            _ => None,
        };
        let interrupted = matches!(inc, Incoming::Result { .. })
            && self
                .rt
                .lock()
                .unwrap()
                .tasks
                .get_mut(task)
                .map(|t| std::mem::take(&mut t.interrupted))
                .unwrap_or(false);
        let policy = if matches!(inc, Incoming::CanUseTool(_)) {
            self.policy_for(&view.repo)
        } else {
            Policy::default()
        };
        let hub = self.hub.clone();
        let new_id = move || hub.new_request_id();
        let plan = plan_line(
            &LineCtx {
                view: &view,
                policy: &policy,
                open_requests: &open,
                revises,
                interrupted,
                child,
                offset: start,
                len: end - start,
                excerpt_bytes: self.cfg.excerpt_bytes,
                new_request_id: &new_id,
            },
            inc,
        );

        // Answers go out before the commit: a crash in between replays the line
        // and answers twice, which the agent shrugs off; the other order could
        // leave it waiting for good.
        for w in &plan.writes {
            if let Err(e) = self.host.write(child, w.as_bytes()).await {
                tracing::warn!(%task, "could not write to the agent: {e:#}");
            }
        }
        for l in plan.live {
            self.hub.live(task, l);
        }
        if !plan.events.is_empty() {
            let upd = OffsetUpdate {
                child: child.to_string(),
                stream: "out",
                offset: end,
            };
            if let Err(e) = self.hub.commit(task, plan.events, Some(upd)) {
                tracing::error!(%task, "could not store events: {e:#}");
            }
        }
        if let Some((id, ok, error)) = plan.ctl {
            if let Some(tx) = self.ctl.lock().unwrap().remove(&id) {
                let _ = tx.send(CtlReply { ok, error });
            }
        }
        if plan.refresh_files {
            self.refresh_files(task);
        }
        if plan.turn_ended {
            self.start_idle_timer(task);
            self.pump();
        }
    }

    fn refresh_files(self: &Arc<Self>, task: &str) {
        let Some((Some(workdir), Some(base))) = self
            .hub
            .view(|v| v.task(task).map(|t| (t.workdir.clone(), t.base.clone())))
        else {
            return;
        };
        let (this, task) = (self.clone(), task.to_string());
        tokio::spawn(async move {
            let files = tokio::task::spawn_blocking(move || {
                workspace::files_touched(Path::new(&workdir), &base)
            })
            .await;
            if let Ok(Ok(files)) = files {
                let same = this
                    .hub
                    .view(|v| v.task(&task).map(|t| t.files == files))
                    .unwrap_or(true);
                if !same {
                    let _ = this.hub.publish(&task, EventBody::FilesTouched { files });
                }
            }
        });
    }

    async fn on_exit(
        self: &Arc<Self>,
        task: &str,
        status: ExitStatus,
        stderr_tail: String,
        exit_tx: &watch::Sender<bool>,
    ) {
        let (stopping, idle) = {
            let rt = self.rt.lock().unwrap();
            rt.tasks
                .get(task)
                .map(|t| (t.stopping, t.idle_exit))
                .unwrap_or((false, false))
        };
        let view = self.hub.view(|v| v.task(task).cloned());
        let mut events: Vec<EventBody> = self.hub.view(|v| {
            v.open_requests_for(task)
                .iter()
                .filter(|r| r.info.kind != RequestKind::ResumeCheck)
                .map(|r| EventBody::RequestWithdrawn {
                    request: r.info.id.clone(),
                    reason: "the agent exited".into(),
                })
                .collect()
        });
        let reason = if stopping {
            "stopped"
        } else if idle {
            "idle"
        } else {
            "exit"
        };
        events.push(EventBody::AgentExited {
            code: status.code,
            signal: status.signal,
            reason: reason.into(),
        });
        if let Some(view) = &view {
            if stopping {
                events.push(EventBody::Stopped {});
            } else if !idle && view.state.is_active() {
                let tail = stderr_tail.trim();
                let mut message = format!(
                    "the agent exited ({}) before finishing",
                    describe_exit(&status)
                );
                if !tail.is_empty() {
                    message.push_str(&format!(": {tail}"));
                }
                events.push(EventBody::Failed { message });
            }
        }
        if let Err(e) = self.hub.commit(task, events, None) {
            tracing::error!(%task, "could not store the exit: {e:#}");
        }
        {
            let mut rt = self.rt.lock().unwrap();
            if let Some(t) = rt.tasks.remove(task) {
                if let Some(h) = t.idle_timer {
                    h.abort();
                }
            }
            rt.readers.remove(task);
        }
        let _ = exit_tx.send(true);
        self.pump();
    }

    /// The agent's process is gone and nobody saw it exit (the host restarted,
    /// or it was lost). Resume it from its saved session.
    async fn resume_lost(self: Arc<Self>, task: String) {
        let Some(view) = self.hub.view(|v| v.task(&task).cloned()) else {
            return;
        };
        if !view.state.is_active() && view.child.is_none() {
            return;
        }
        if self.rt.lock().unwrap().launching.contains(&task) {
            return;
        }
        let mut events: Vec<EventBody> = self.hub.view(|v| {
            v.open_requests_for(&task)
                .iter()
                .filter(|r| r.info.kind != RequestKind::ResumeCheck)
                .map(|r| EventBody::RequestWithdrawn {
                    request: r.info.id.clone(),
                    reason: "the agent was restarted".into(),
                })
                .collect()
        });
        let mut cut: Vec<(String, String, bool)> = vec![];
        for s in &view.open_steps {
            let outbound = proto::is_outbound(&s.tool, &Value::String(s.detail.clone()));
            events.push(EventBody::CutOff {
                step: s.id.clone(),
                title: s.title.clone(),
                outbound,
            });
            cut.push((s.id.clone(), s.title.clone(), outbound));
        }
        if view.child.is_some() {
            events.push(EventBody::AgentExited {
                code: None,
                signal: None,
                reason: "lost".into(),
            });
        }
        let outbound: Vec<&(String, String, bool)> = cut.iter().filter(|c| c.2).collect();
        let pending_check = self.hub.view(|v| {
            v.open_requests_for(&task)
                .iter()
                .any(|r| r.info.kind == RequestKind::ResumeCheck)
        });
        if !outbound.is_empty() && !pending_check {
            let list: Vec<Value> = view
                .open_steps
                .iter()
                .filter(|s| outbound.iter().any(|o| o.0 == s.id))
                .map(|s| json!({"id": s.id, "tool": s.tool, "title": s.title, "detail": s.detail}))
                .collect();
            events.push(EventBody::Request {
                request: RequestInfo {
                    id: self.hub.new_request_id(),
                    task: task.clone(),
                    kind: RequestKind::ResumeCheck,
                    tool: "resume".into(),
                    display_name: Some("Continue after a restart?".into()),
                    description: None,
                    summary: "These calls leave your machine and were cut off by a restart. They may or may not have happened."
                        .into(),
                    input: json!({ "cut_off": list }),
                    suggestions: vec![],
                    blocked_path: None,
                    tool_use_id: None,
                    agent_request: None,
                    revises: None,
                },
            });
            if let Err(e) = self.hub.commit(&task, events, None) {
                tracing::error!(%task, "could not store the restart: {e:#}");
            }
            return;
        }
        if let Err(e) = self.hub.commit(&task, events, None) {
            tracing::error!(%task, "could not store the restart: {e:#}");
        }
        let message = continue_message(&view, None);
        self.rt.lock().unwrap().launching.insert(task.clone());
        self.launch(task, Some(message)).await;
    }

    // ---- idle agents -------------------------------------------------------------

    fn start_idle_timer(self: &Arc<Self>, task: &str) {
        let ttl = Duration::from_millis(self.cfg.idle_ttl_ms);
        let (this, t) = (self.clone(), task.to_string());
        let handle = tokio::spawn(async move {
            tokio::time::sleep(ttl).await;
            this.idle_exit(&t).await;
        });
        let mut rt = self.rt.lock().unwrap();
        match rt.tasks.get_mut(task) {
            Some(t) => {
                if let Some(old) = t.idle_timer.replace(handle.abort_handle()) {
                    old.abort();
                }
            }
            None => handle.abort(),
        }
    }

    fn cancel_idle_timer(&self, task: &str) {
        if let Some(t) = self.rt.lock().unwrap().tasks.get_mut(task) {
            if let Some(h) = t.idle_timer.take() {
                h.abort();
            }
        }
    }

    async fn idle_exit(self: &Arc<Self>, task: &str) {
        let Some(view) = self.hub.view(|v| v.task(task).cloned()) else {
            return;
        };
        let Some(child) = view.child.clone() else {
            return;
        };
        if !matches!(view.state, TaskState::Done | TaskState::Failed) {
            return;
        }
        let rx = {
            let mut rt = self.rt.lock().unwrap();
            let Some(t) = rt.tasks.get_mut(task) else {
                return;
            };
            t.idle_exit = true;
            t.idle_timer = None;
            t.exit_tx.subscribe()
        };
        self.terminate(&child, rx, false).await;
    }

    /// Interrupt (optionally), close the input, terminate the group, kill it
    /// after the grace period. Returns once the process is gone.
    async fn terminate(&self, child: &str, mut exit_rx: watch::Receiver<bool>, interrupt: bool) {
        let wait = Duration::from_millis(self.cfg.interrupt_wait_ms);
        let grace = Duration::from_millis(self.cfg.stop_grace_ms);
        if interrupt {
            let _ = self
                .host
                .write(child, proto::interrupt_request(&self.ctl_id()).as_bytes())
                .await;
        }
        let _ = self.host.close_stdin(child).await;
        if !wait_exit(&mut exit_rx, wait).await {
            let _ = self.host.signal(child, json!("TERM"), true).await;
            if !wait_exit(&mut exit_rx, grace).await {
                let _ = self.host.signal(child, json!("KILL"), true).await;
                wait_exit(&mut exit_rx, Duration::from_secs(5)).await;
            }
        }
        // Whatever the agent started and left behind goes too.
        let _ = self.host.signal(child, json!("KILL"), true).await;
    }

    fn ctl_id(&self) -> String {
        format!("sphere-{}", self.ctl_seq.fetch_add(1, Ordering::SeqCst) + 1)
    }

    // ---- API ---------------------------------------------------------------------

    pub async fn start_task(self: &Arc<Self>, p: api::StartParams) -> ApiResult<api::StartResult> {
        self.start_task_inner(p, None).await
    }

    async fn start_task_inner(
        self: &Arc<Self>,
        p: api::StartParams,
        resume_session: Option<String>,
    ) -> ApiResult<api::StartResult> {
        if p.prompt.trim().is_empty() {
            return Err(ApiError(RpcError {
                code: api::code::INVALID_PARAMS,
                message: "the prompt is empty".into(),
                data: None,
            }));
        }
        if self.capped() {
            return Err(api_err(
                kind::DAILY_CAP,
                "the daily cost cap is reached; raise it or wait for tomorrow",
            ));
        }
        if let Some(m) = &p.mode {
            if !MODES.contains(&m.as_str()) {
                return Err(ApiError(RpcError {
                    code: api::code::INVALID_PARAMS,
                    message: format!("unknown mode `{m}` (one of {})", MODES.join(", ")),
                    data: None,
                }));
            }
        }
        let budget = p.budget_usd.unwrap_or(self.cfg.default_budget_usd);
        if budget <= 0.0 || !budget.is_finite() {
            return Err(ApiError(RpcError {
                code: api::code::INVALID_PARAMS,
                message: "the budget must be above zero".into(),
                data: None,
            }));
        }
        let repo_in = PathBuf::from(&p.repo);
        let top = tokio::task::spawn_blocking(move || workspace::repo_toplevel(&repo_in))
            .await
            .map_err(|e| anyhow!(e))?
            .map_err(|e| api_err(kind::BAD_REPO, format!("{e:#}")))?;
        let title = p
            .title
            .clone()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| truncate(p.prompt.lines().next().unwrap_or("task"), 60));
        let id = self.hub.new_task_id();
        self.hub
            .publish(
                &id,
                EventBody::TaskCreated {
                    title,
                    prompt: p.prompt,
                    repo: top.to_string_lossy().to_string(),
                    in_place: p.in_place,
                    budget_usd: budget,
                    mode: p.mode,
                    model: p.model,
                    allow_rewrites: p.allow_rewrites,
                    resume_session,
                },
            )
            .map_err(ApiError::from)?;
        self.pump();
        Ok(api::StartResult { task: id })
    }

    fn task_view(&self, task: &str) -> ApiResult<TaskView> {
        self.hub
            .view(|v| v.task(task).cloned())
            .ok_or_else(|| api_err(kind::NOT_FOUND, format!("no such task: {task}")))
    }

    pub async fn send(self: &Arc<Self>, task: &str, text: String) -> ApiResult<()> {
        let view = self.task_view(task)?;
        match view.state {
            TaskState::Queued => {
                return Err(api_err(kind::INVALID_STATE, "the task has not started yet"))
            }
            TaskState::Stopped => return Err(api_err(kind::INVALID_STATE, "the task was stopped")),
            _ => {}
        }
        if self.capped() {
            return Err(api_err(kind::DAILY_CAP, "the daily cost cap is reached"));
        }
        if let Some(child) = &view.child {
            self.cancel_idle_timer(task);
            self.host
                .write(child, proto::user_message(&text).as_bytes())
                .await
                .map_err(|e| api_err(kind::AGENT_ERROR, format!("{e:#}")))?;
            self.hub
                .publish(task, EventBody::UserMessage { text })
                .map_err(ApiError::from)?;
            return Ok(());
        }
        if view.session_id.is_none() {
            return Err(api_err(
                kind::INVALID_STATE,
                "the task has no session to continue",
            ));
        }
        if !self.rt.lock().unwrap().launching.insert(task.to_string()) {
            return Err(api_err(
                kind::INVALID_STATE,
                "the agent is starting; try again in a moment",
            ));
        }
        let this = self.clone();
        let t = task.to_string();
        tokio::spawn(async move { this.launch(t, Some(text)).await });
        Ok(())
    }

    fn withdraw_events(
        &self,
        task: &str,
        reason: &str,
        include_resume_checks: bool,
    ) -> Vec<EventBody> {
        self.hub.view(|v| {
            v.open_requests_for(task)
                .iter()
                .filter(|r| include_resume_checks || r.info.kind != RequestKind::ResumeCheck)
                .map(|r| EventBody::RequestWithdrawn {
                    request: r.info.id.clone(),
                    reason: reason.into(),
                })
                .collect()
        })
    }

    pub async fn interrupt(self: &Arc<Self>, task: &str) -> ApiResult<()> {
        let view = self.task_view(task)?;
        let Some(child) = view.child else {
            return Err(api_err(kind::INVALID_STATE, "the agent is not running"));
        };
        let withdraw = self.withdraw_events(task, "interrupted", false);
        if !withdraw.is_empty() {
            self.hub
                .commit(task, withdraw, None)
                .map_err(ApiError::from)?;
        }
        if let Some(t) = self.rt.lock().unwrap().tasks.get_mut(task) {
            t.interrupted = true;
        }
        self.host
            .write(&child, proto::interrupt_request(&self.ctl_id()).as_bytes())
            .await
            .map_err(|e| api_err(kind::AGENT_ERROR, format!("{e:#}")))?;
        Ok(())
    }

    async fn control(
        &self,
        task: &str,
        build: impl FnOnce(&str) -> String,
    ) -> ApiResult<Option<CtlReply>> {
        let view = self.task_view(task)?;
        let Some(child) = view.child else {
            return Ok(None);
        };
        let id = self.ctl_id();
        let (tx, rx) = oneshot::channel();
        self.ctl.lock().unwrap().insert(id.clone(), tx);
        let sent = self.host.write(&child, build(&id).as_bytes()).await;
        if let Err(e) = sent {
            self.ctl.lock().unwrap().remove(&id);
            return Err(api_err(kind::AGENT_ERROR, format!("{e:#}")));
        }
        match tokio::time::timeout(Duration::from_secs(10), rx).await {
            Ok(Ok(r)) => Ok(Some(r)),
            _ => {
                self.ctl.lock().unwrap().remove(&id);
                Err(api_err(
                    kind::AGENT_ERROR,
                    "the agent did not answer in time",
                ))
            }
        }
    }

    pub async fn set_mode(self: &Arc<Self>, task: &str, mode: String) -> ApiResult<()> {
        if !MODES.contains(&mode.as_str()) {
            return Err(ApiError(RpcError {
                code: api::code::INVALID_PARAMS,
                message: format!("unknown mode `{mode}` (one of {})", MODES.join(", ")),
                data: None,
            }));
        }
        // With no agent running the mode applies when it next starts.
        if let Some(r) = self
            .control(task, |id| proto::set_mode_request(id, &mode))
            .await?
        {
            if !r.ok {
                return Err(api_err(
                    kind::AGENT_ERROR,
                    r.error.unwrap_or_else(|| "the agent refused".into()),
                ));
            }
        }
        self.hub
            .publish(task, EventBody::ModeChanged { mode })
            .map_err(ApiError::from)?;
        Ok(())
    }

    pub async fn set_model(self: &Arc<Self>, task: &str, model: String) -> ApiResult<()> {
        if let Some(r) = self
            .control(task, |id| proto::set_model_request(id, &model))
            .await?
        {
            if !r.ok {
                return Err(api_err(
                    kind::AGENT_ERROR,
                    r.error.unwrap_or_else(|| "the agent refused".into()),
                ));
            }
        }
        self.hub
            .publish(task, EventBody::ModelChanged { model })
            .map_err(ApiError::from)?;
        Ok(())
    }

    pub async fn stop(self: &Arc<Self>, task: &str) -> ApiResult<()> {
        let view = self.task_view(task)?;
        if view.child.is_none() && !view.state.is_active() && view.state != TaskState::Queued {
            return Ok(()); // nothing is running
        }
        let withdraw = self.withdraw_events(task, "the task was stopped", true);
        let rx = {
            let mut rt = self.rt.lock().unwrap();
            rt.tasks.get_mut(task).map(|t| {
                t.stopping = true;
                t.interrupted = true;
                if let Some(h) = t.idle_timer.take() {
                    h.abort();
                }
                t.exit_tx.subscribe()
            })
        };
        match (view.child.clone(), rx) {
            (Some(child), Some(rx)) => {
                if !withdraw.is_empty() {
                    self.hub
                        .commit(task, withdraw, None)
                        .map_err(ApiError::from)?;
                }
                // The reader publishes `Stopped` when the exit arrives.
                self.terminate(&child, rx.clone(), true).await;
                let mut rx = rx;
                wait_exit(&mut rx, Duration::from_secs(10)).await;
            }
            _ => {
                let mut events = withdraw;
                events.push(EventBody::Stopped {});
                self.hub
                    .commit(task, events, None)
                    .map_err(ApiError::from)?;
                self.pump();
            }
        }
        Ok(())
    }

    pub async fn answer(self: &Arc<Self>, p: api::AnswerParams) -> ApiResult<()> {
        let _guard = self.answer_lock.lock().await;
        let req = self
            .hub
            .view(|v| v.request(&p.request).cloned())
            .ok_or_else(|| api_err(kind::NOT_FOUND, format!("no such request: {}", p.request)))?;
        match req.status {
            RequestStatus::Answered => {
                return Err(api_err(
                    kind::ALREADY_ANSWERED,
                    format!("request {} was already answered", p.request),
                ))
            }
            RequestStatus::Withdrawn => {
                return Err(api_err(
                    kind::WITHDRAWN,
                    format!("request {} was withdrawn", p.request),
                ))
            }
            RequestStatus::Open | RequestStatus::Snoozed => {}
        }
        let info = req.info;
        if p.answer == AnswerKind::Later {
            self.hub
                .publish(
                    &info.task,
                    EventBody::RequestSnoozed {
                        request: info.id.clone(),
                    },
                )
                .map_err(ApiError::from)?;
            return Ok(());
        }
        let view = self.task_view(&info.task)?;
        let mut always_saved = false;
        let note = p.text.clone().filter(|t| !t.trim().is_empty());

        if info.kind == RequestKind::ResumeCheck {
            self.hub
                .publish(
                    &info.task,
                    EventBody::RequestAnswered {
                        request: info.id.clone(),
                        answer: p.answer,
                        text: p.text.clone(),
                        always: false,
                    },
                )
                .map_err(ApiError::from)?;
            let mut message = continue_message(&view, None);
            if let Some(list) = info.input["cut_off"].as_array() {
                if !list.is_empty() && view.open_steps.is_empty() {
                    message.push_str(
                        "\n\nThese tool calls were cut off before they finished, so they may or may not have completed. \
                         Check their state before repeating anything with side effects:",
                    );
                    for c in list {
                        message.push_str(&format!(
                            "\n- {}",
                            c["title"].as_str().unwrap_or("a tool call")
                        ));
                    }
                }
            }
            if p.answer == AnswerKind::Change {
                if let Some(n) = note {
                    message.push_str(&format!("\n\nThe user says: {n}"));
                }
            }
            if self.rt.lock().unwrap().launching.insert(info.task.clone()) {
                let (this, task) = (self.clone(), info.task.clone());
                tokio::spawn(async move { this.launch(task, Some(message)).await });
            }
            return Ok(());
        }

        let (Some(child), Some(agent_req)) = (view.child.clone(), info.agent_request.clone())
        else {
            return Err(api_err(kind::INVALID_STATE, "the agent is not running"));
        };
        let line = match p.answer {
            AnswerKind::Yes => {
                let input = match (&p.input, &note, info.kind) {
                    (Some(i), _, _) => i.clone(),
                    (None, Some(t), RequestKind::Question) => {
                        proto::question_answer_input(&info.input, t)
                    }
                    _ => info.input.clone(),
                };
                let mut perms = None;
                if p.always {
                    if let Some(s) = info.suggestions.first() {
                        if let Some(rule) = policy::rule_from_suggestion(s) {
                            match policy::append_allow_rule(&self.cfg.policy_path(), &rule) {
                                Ok(()) => always_saved = true,
                                Err(e) => tracing::warn!("could not save the rule: {e:#}"),
                            }
                        }
                        perms = Some(vec![policy::session_scoped(s)]);
                    }
                }
                proto::allow_response(&agent_req, &input, perms)
            }
            AnswerKind::Change => proto::deny_response(
                &agent_req,
                &note.clone().unwrap_or_else(|| {
                    "The user wants this changed. Ask what they have in mind.".into()
                }),
            ),
            AnswerKind::Later => unreachable!("handled above"),
        };
        self.host
            .write(&child, line.as_bytes())
            .await
            .map_err(|e| {
                api_err(
                    kind::AGENT_ERROR,
                    format!("could not reach the agent: {e:#}"),
                )
            })?;
        self.hub
            .publish(
                &info.task,
                EventBody::RequestAnswered {
                    request: info.id.clone(),
                    answer: p.answer,
                    text: p.text,
                    always: always_saved,
                },
            )
            .map_err(ApiError::from)?;
        Ok(())
    }

    // ---- reads ---------------------------------------------------------------------

    pub fn snapshot(&self) -> api::Snapshot {
        let day = today();
        self.hub.view(|v| api::Snapshot {
            seq: v.seq,
            tasks: v.tasks().into_iter().cloned().collect(),
            requests: v.open_requests().into_iter().cloned().collect(),
            cost_today_usd: v.cost_on(&day),
            daily_cap_usd: self.cfg.daily_cap_usd,
            max_concurrent: Some(self.cfg.max_concurrent),
        })
    }

    pub async fn status(&self) -> api::Status {
        let host_pid = self.host.pid().await.ok();
        let day = today();
        let info = self.info.lock().unwrap();
        self.hub.view(|v| api::Status {
            seq: v.seq,
            version: env!("CARGO_PKG_VERSION").to_string(),
            active: v.active_count(),
            queued: v.queued().len(),
            needs_you: v
                .tasks()
                .iter()
                .filter(|t| t.state == TaskState::NeedsYou)
                .count(),
            cost_today_usd: v.cost_on(&day),
            daily_cap_usd: self.cfg.daily_cap_usd,
            tasks: v
                .tasks()
                .into_iter()
                .map(|t| api::TaskLine {
                    id: t.id.clone(),
                    title: t.title.clone(),
                    state: t.state,
                    status: t.status.clone(),
                    cost_usd: t.cost_usd,
                })
                .collect(),
            requests: v
                .open_requests()
                .into_iter()
                .map(|r| api::RequestLine {
                    id: r.info.id.clone(),
                    task: r.info.task.clone(),
                    status: r.status,
                    summary: r.info.summary.clone(),
                })
                .collect(),
            host_pid,
            claude_version: info.claude_version.clone(),
            signed_in: info.signed_in,
            warnings: info.warnings.clone(),
        })
    }

    pub async fn blob_read(&self, p: api::BlobReadParams) -> ApiResult<api::BlobReadResult> {
        let want = p.len.min(8 * 1024 * 1024);
        let mut bytes = vec![];
        let mut off = p.offset;
        while (bytes.len() as u64) < want {
            let r = self
                .host
                .read(
                    &p.child,
                    Stream::Out,
                    off,
                    Some((want - bytes.len() as u64) as usize),
                )
                .await
                .map_err(|e| api_err(kind::NOT_FOUND, format!("{e:#}")))?;
            if r.bytes.is_empty() {
                break;
            }
            off = r.offset;
            bytes.extend(r.bytes);
        }
        let text = String::from_utf8_lossy(&bytes).to_string();
        let text = match &p.tool_use_id {
            Some(id) => match proto::parse_line(text.trim()) {
                Ok(Incoming::ToolResults(rs)) => rs
                    .into_iter()
                    .find(|r| &r.tool_use_id == id)
                    .map(|r| r.text)
                    .ok_or_else(|| api_err(kind::NOT_FOUND, "no such tool result in that line"))?,
                _ => return Err(api_err(kind::NOT_FOUND, "that range is not a tool result")),
            },
            None => text,
        };
        Ok(api::BlobReadResult { text })
    }

    pub async fn session_list(&self) -> ApiResult<Vec<api::SessionInfo>> {
        let known: HashMap<String, String> = self.hub.view(|v| {
            v.tasks()
                .into_iter()
                .filter_map(|t| t.session_id.clone().map(|s| (s, t.id.clone())))
                .collect()
        });
        Ok(sessions::list(&self.cfg, &known).await?)
    }

    pub async fn bring_in(self: &Arc<Self>, p: api::BringInParams) -> ApiResult<api::StartResult> {
        let sessions = self.session_list().await?;
        let found = sessions.iter().find(|s| s.session_id == p.session_id);
        if let Some(s) = found {
            if s.running {
                return Err(api_err(
                    kind::SESSION_RUNNING,
                    format!(
                        "session {} is still running outside Sphere{}; stop it there first",
                        s.session_id,
                        s.pid.map(|p| format!(" (pid {p})")).unwrap_or_default()
                    ),
                ));
            }
            if let Some(t) = &s.task {
                if self
                    .hub
                    .view(|v| v.task(t).is_some_and(|t| t.state.is_active()))
                {
                    return Err(api_err(
                        kind::INVALID_STATE,
                        format!("session {} is already running as task {t}", s.session_id),
                    ));
                }
            }
        }
        let cwd = p
            .cwd
            .clone()
            .or_else(|| found.and_then(|s| s.cwd.clone()))
            .ok_or_else(|| {
                api_err(
                    kind::NOT_FOUND,
                    format!("no folder known for session {}; pass one", p.session_id),
                )
            })?;
        self.start_task_inner(
            api::StartParams {
                repo: cwd,
                prompt: p.prompt.unwrap_or_else(|| "continue".into()),
                title: found
                    .and_then(|s| s.title.clone())
                    .map(|t| truncate(&t, 60)),
                in_place: true,
                ..Default::default()
            },
            Some(p.session_id),
        )
        .await
    }

    /// Pids a caller must not descend from to answer a request: the host (which
    /// adopts orphans) and everything it is running.
    pub async fn agent_roots(&self) -> Vec<i32> {
        let mut roots = vec![];
        if let Ok(pid) = self.host.pid().await {
            roots.push(pid as i32);
        }
        if let Ok(list) = self.host.list().await {
            roots.extend(list.iter().filter(|c| c.running).map(|c| c.pid as i32));
        }
        roots
    }
}

/// The message an agent gets when it is resumed after a restart.
fn continue_message(view: &TaskView, user_note: Option<String>) -> String {
    let mut m = String::from("continue");
    if !view.open_steps.is_empty() {
        m.push_str(
            "\n\nSphere was restarted while these tool calls were running, so they may or may not have completed. \
             Check their state before repeating anything with side effects:",
        );
        for s in &view.open_steps {
            m.push_str(&format!("\n- {}", s.title));
        }
    }
    if let Some(n) = user_note {
        m.push_str(&format!("\n\nThe user says: {n}"));
    }
    m
}

async fn run_capture(cmd: &[String]) -> Result<String> {
    let (prog, args) = cmd.split_first().context("empty command")?;
    let fut = tokio::process::Command::new(prog)
        .args(args)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(Duration::from_secs(15), fut)
        .await
        .context("timed out")??;
    if !out.status.success() {
        anyhow::bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{AutoRule, Decision};
    use crate::store::Store;
    use sphere_proto::views::OpenStep;

    fn view() -> TaskView {
        let mut v = sphere_proto::Views::default();
        v.apply(&sphere_proto::Event {
            seq: 1,
            task: "t1".into(),
            at: "2026-01-01T00:00:00.000Z".into(),
            body: EventBody::TaskCreated {
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
        });
        v.task("t1").unwrap().clone()
    }

    fn cx<'a>(
        view: &'a TaskView,
        policy: &'a Policy,
        open: &'a [RequestInfo],
        new_id: &'a dyn Fn() -> String,
    ) -> LineCtx<'a> {
        LineCtx {
            view,
            policy,
            open_requests: open,
            revises: None,
            interrupted: false,
            child: "c1",
            offset: 10,
            len: 20,
            excerpt_bytes: 100,
            new_request_id: new_id,
        }
    }

    fn ask(tool: &str, input: Value) -> Incoming {
        Incoming::CanUseTool(CanUseTool {
            request_id: "req_1".into(),
            tool_name: tool.into(),
            display_name: None,
            input,
            tool_use_id: Some("toolu_1".into()),
            description: Some("Push it".into()),
            suggestions: vec![json!({"type": "addRules"})],
            blocked_path: None,
        })
    }

    #[test]
    fn init_records_the_session_once() {
        let mut v = view();
        let p = Policy::default();
        let id = || "r1".to_string();
        let init = || Incoming::Init {
            session_id: Some("s1".into()),
            model: Some("m".into()),
            permission_mode: None,
            slash_commands: vec![],
        };
        let plan = plan_line(&cx(&v, &p, &[], &id), init());
        assert!(
            matches!(&plan.events[0], EventBody::SessionId { session_id } if session_id == "s1")
        );
        assert!(matches!(&plan.events[1], EventBody::Init { .. }));
        v.session_id = Some("s1".into());
        let plan = plan_line(&cx(&v, &p, &[], &id), init());
        assert_eq!(plan.events.len(), 1, "same session: no new SessionId");
    }

    #[test]
    fn assistant_blocks_become_say_plan_and_steps() {
        let v = view();
        let p = Policy::default();
        let id = || "r1".to_string();
        let plan = plan_line(
            &cx(&v, &p, &[], &id),
            Incoming::Assistant {
                parent: Some("toolu_parent".into()),
                blocks: vec![
                    Block::Text("  ".into()),
                    Block::Text("Hello".into()),
                    Block::ToolUse {
                        id: "a".into(),
                        name: "TodoWrite".into(),
                        input: json!({"todos":[{"content":"x","status":"pending"}]}),
                    },
                    Block::ToolUse {
                        id: "b".into(),
                        name: "Bash".into(),
                        input: json!({"command":"cargo test"}),
                    },
                ],
            },
        );
        let kinds: Vec<&str> = plan.events.iter().map(|e| e.kind_name()).collect();
        assert_eq!(kinds, vec!["say", "plan", "step", "step"]);
        assert!(
            matches!(&plan.events[3], EventBody::Step { parent: Some(p), tool, .. } if p == "toolu_parent" && tool == "Bash")
        );
    }

    #[test]
    fn tool_results_cut_big_output_and_refresh_files_for_edits() {
        let mut v = view();
        v.open_steps.push(OpenStep {
            id: "e1".into(),
            tool: "Edit".into(),
            title: "Editing a".into(),
            detail: String::new(),
        });
        v.open_steps.push(OpenStep {
            id: "r1".into(),
            tool: "Read".into(),
            title: "Reading".into(),
            detail: String::new(),
        });
        let p = Policy::default();
        let id = || "r1".to_string();
        let big = "x".repeat(500);
        let plan = plan_line(
            &cx(&v, &p, &[], &id),
            Incoming::ToolResults(vec![
                proto::ToolResult {
                    tool_use_id: "r1".into(),
                    text: "small".into(),
                    is_error: false,
                },
                proto::ToolResult {
                    tool_use_id: "e1".into(),
                    text: big,
                    is_error: true,
                },
            ]),
        );
        assert!(plan.refresh_files, "an edit finished");
        match (&plan.events[0], &plan.events[1]) {
            (
                EventBody::StepDone {
                    ok: true,
                    output: Some(a),
                    ..
                },
                EventBody::StepDone {
                    ok: false,
                    output: Some(b),
                    id,
                },
            ) => {
                assert_eq!(a.head, "small");
                assert!(a.source.is_none());
                assert_eq!(id, "e1");
                assert!(b.tail.is_some());
                assert_eq!(
                    b.source,
                    Some(OutputRef {
                        child: "c1".into(),
                        offset: 10,
                        len: 20
                    })
                );
            }
            other => panic!("{other:?}"),
        }
        let only_read = plan_line(
            &cx(&v, &p, &[], &id),
            Incoming::ToolResults(vec![proto::ToolResult {
                tool_use_id: "r1".into(),
                text: "t".into(),
                is_error: false,
            }]),
        );
        assert!(!only_read.refresh_files);
    }

    #[test]
    fn a_result_computes_the_cost_delta_and_withdraws_stale_asks() {
        let mut v = view();
        v.last_total_cost = 0.25;
        let p = Policy::default();
        let id = || "r9".to_string();
        let open = vec![RequestInfo {
            id: "r1".into(),
            task: "t1".into(),
            kind: RequestKind::Permission,
            tool: "Bash".into(),
            display_name: None,
            description: None,
            summary: String::new(),
            input: Value::Null,
            suggestions: vec![],
            blocked_path: None,
            tool_use_id: None,
            agent_request: Some("a".into()),
            revises: None,
        }];
        let mut c = cx(&v, &p, &open, &id);
        c.interrupted = true;
        let plan = plan_line(
            &c,
            Incoming::Result {
                ok: false,
                subtype: "error_during_execution".into(),
                text: "x".into(),
                turns: Some(2),
                duration_ms: Some(5),
                total_cost_usd: Some(0.75),
            },
        );
        assert!(plan.turn_ended);
        assert!(
            matches!(&plan.events[0], EventBody::RequestWithdrawn { request, .. } if request == "r1")
        );
        match &plan.events[1] {
            EventBody::Turn {
                interrupted: true,
                cost_usd: Some(c),
                cost_delta_usd,
                ok: false,
                ..
            } => {
                assert_eq!(*c, 0.75);
                assert!((cost_delta_usd - 0.5).abs() < 1e-9);
            }
            other => panic!("{other:?}"),
        }
        // A restarted process reports a lower total: never a negative delta.
        let mut v2 = view();
        v2.last_total_cost = 5.0;
        let plan = plan_line(
            &cx(&v2, &p, &[], &id),
            Incoming::Result {
                ok: true,
                subtype: "success".into(),
                text: "ok".into(),
                turns: None,
                duration_ms: None,
                total_cost_usd: Some(0.1),
            },
        );
        assert!(
            matches!(&plan.events[0], EventBody::Turn { cost_delta_usd, .. } if *cost_delta_usd == 0.0)
        );
    }

    #[test]
    fn permission_asks_become_requests_with_the_exact_input() {
        let mut v = view();
        v.last_say = "I will push now".into();
        let p = Policy::default();
        let id = || "r7".to_string();
        let mut c = cx(&v, &p, &[], &id);
        c.revises = Some("r3".into());
        let plan = plan_line(
            &c,
            ask("Bash", json!({"command": "git push origin sphere/x"})),
        );
        assert!(plan.writes.is_empty(), "nothing is answered yet");
        match &plan.events[0] {
            EventBody::Request { request } => {
                assert_eq!(request.id, "r7");
                assert_eq!(request.kind, RequestKind::Permission);
                assert_eq!(request.input["command"], "git push origin sphere/x");
                assert_eq!(request.agent_request.as_deref(), Some("req_1"));
                assert_eq!(request.revises.as_deref(), Some("r3"));
                assert_eq!(request.summary, "Push it");
                assert_eq!(request.suggestions.len(), 1);
            }
            other => panic!("{other:?}"),
        }
        // No description: the agent's last words stand in.
        let plan = plan_line(
            &cx(&v, &p, &[], &id),
            Incoming::CanUseTool(CanUseTool {
                request_id: "q".into(),
                tool_name: "AskUserQuestion".into(),
                display_name: None,
                input: json!({"questions": []}),
                tool_use_id: None,
                description: None,
                suggestions: vec![],
                blocked_path: None,
            }),
        );
        match &plan.events[0] {
            EventBody::Request { request } => {
                assert_eq!(request.kind, RequestKind::Question);
                assert_eq!(request.summary, "I will push now");
            }
            other => panic!("{other:?}"),
        }
        let plan = plan_line(
            &cx(&v, &p, &[], &id),
            ask("ExitPlanMode", json!({"plan": "do it"})),
        );
        assert!(
            matches!(&plan.events[0], EventBody::Request { request } if request.kind == RequestKind::Plan)
        );
    }

    #[test]
    fn tier2_rules_answer_without_a_request() {
        let mut v = view();
        v.branch = Some("sphere/x".into());
        let mut p = Policy::default();
        p.auto.push(AutoRule {
            tool: "Bash".into(),
            pattern: Some("git push * {branch}".into()),
            decision: Decision::Allow,
            name: Some("push-own-branch".into()),
        });
        p.auto.push(AutoRule {
            tool: "Bash".into(),
            pattern: Some("*prod*".into()),
            decision: Decision::Deny,
            name: None,
        });
        let id = || "r1".to_string();
        let plan = plan_line(
            &cx(&v, &p, &[], &id),
            ask("Bash", json!({"command": "git push origin sphere/x"})),
        );
        let w: Value = serde_json::from_str(&plan.writes[0]).unwrap();
        assert_eq!(w["response"]["request_id"], "req_1");
        assert_eq!(w["response"]["response"]["behavior"], "allow");
        assert_eq!(
            w["response"]["response"]["updatedInput"]["command"],
            "git push origin sphere/x"
        );
        assert!(
            matches!(&plan.events[0], EventBody::AutoAnswered { allow: true, rule, .. } if rule == "push-own-branch")
        );

        let plan = plan_line(
            &cx(&v, &p, &[], &id),
            ask("Bash", json!({"command": "deploy prod"})),
        );
        let w: Value = serde_json::from_str(&plan.writes[0]).unwrap();
        assert_eq!(w["response"]["response"]["behavior"], "deny");
        assert!(w["response"]["response"]["message"]
            .as_str()
            .unwrap()
            .contains("policy"));

        let plan = plan_line(
            &cx(&v, &p, &[], &id),
            ask("Bash", json!({"command": "git push origin main"})),
        );
        assert!(plan.writes.is_empty());
        assert!(matches!(&plan.events[0], EventBody::Request { .. }));
    }

    #[test]
    fn controls_deltas_cancels_and_unknowns() {
        let v = view();
        let p = Policy::default();
        let id = || "r1".to_string();
        let open = vec![RequestInfo {
            id: "r4".into(),
            task: "t1".into(),
            kind: RequestKind::Permission,
            tool: "Bash".into(),
            display_name: None,
            description: None,
            summary: String::new(),
            input: Value::Null,
            suggestions: vec![],
            blocked_path: None,
            tool_use_id: None,
            agent_request: Some("req_4".into()),
            revises: None,
        }];
        let c = cx(&v, &p, &open, &id);
        assert_eq!(plan_line(&c, Incoming::Delta("hi".into())).live, vec!["hi"]);
        let plan = plan_line(
            &c,
            Incoming::ControlCancel {
                request_id: "req_4".into(),
            },
        );
        assert!(
            matches!(&plan.events[0], EventBody::RequestWithdrawn { request, .. } if request == "r4")
        );
        let plan = plan_line(
            &c,
            Incoming::ControlResponse {
                request_id: "sphere-1".into(),
                ok: true,
                error: None,
                response: Value::Null,
            },
        );
        assert_eq!(plan.ctl, Some(("sphere-1".into(), true, None)));
        let plan = plan_line(
            &c,
            Incoming::OtherControl {
                request_id: "x".into(),
                subtype: "hook_callback".into(),
            },
        );
        assert!(plan.writes[0].contains("\"error\""));
        assert!(plan_line(&c, Incoming::Other("weird".into()))
            .events
            .is_empty());
    }

    #[test]
    fn the_continue_message_lists_cut_off_calls_and_the_users_note() {
        let mut v = view();
        assert_eq!(continue_message(&v, None), "continue");
        v.open_steps.push(OpenStep {
            id: "s".into(),
            tool: "Bash".into(),
            title: "Running cargo test".into(),
            detail: String::new(),
        });
        let m = continue_message(&v, Some("skip the push".into()));
        assert!(
            m.starts_with("continue")
                && m.contains("Running cargo test")
                && m.contains("skip the push")
        );
    }

    fn manager(daily_cap_usd: Option<f64>) -> Arc<Manager> {
        let cfg = Config {
            daily_cap_usd,
            ..Config::default()
        };
        let hub = Hub::with_store(Store::open_in_memory().unwrap(), 16).unwrap();
        Manager::new(Arc::new(cfg), Arc::new(hub))
    }

    fn queue(m: &Manager, task: &str) {
        m.hub
            .publish(
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
            .unwrap();
    }

    fn spend(m: &Manager, task: &str, usd: f64) {
        m.hub
            .publish(
                task,
                EventBody::Turn {
                    ok: true,
                    interrupted: false,
                    summary: String::new(),
                    turns: None,
                    duration_ms: None,
                    cost_usd: Some(usd),
                    cost_delta_usd: usd,
                },
            )
            .unwrap();
    }

    #[test]
    fn the_day_has_one_to_86400_seconds_left() {
        let left = until_next_day();
        assert!((Duration::from_secs(1)..=Duration::from_secs(86_400)).contains(&left));
    }

    #[tokio::test]
    async fn pumping_with_nothing_queued_waits_for_nothing() {
        let m = manager(None);
        m.pump();
        assert!(m.rt.lock().unwrap().cap_wait.is_none());
        // Spent to the cap with nothing queued is the same: no clock to watch.
        let m = manager(Some(0.5));
        queue(&m, "t1");
        spend(&m, "t1", 0.5);
        assert!(m.capped());
        m.pump();
        assert!(m.rt.lock().unwrap().cap_wait.is_none());
    }

    #[tokio::test]
    async fn tasks_held_back_by_the_daily_cap_wait_once_for_the_end_of_the_day() {
        let m = manager(Some(0.5));
        queue(&m, "t1");
        spend(&m, "t1", 0.5);
        queue(&m, "t2");
        m.pump();
        let wait = m.rt.lock().unwrap().cap_wait.clone().expect("a wait");
        assert!(!wait.is_finished());
        assert!(m.rt.lock().unwrap().launching.is_empty(), "nothing starts");
        // More events while it waits do not pile up more waits.
        m.pump();
        m.pump();
        let again =
            m.rt.lock()
                .unwrap()
                .cap_wait
                .clone()
                .expect("the same wait");
        assert_eq!(again.id(), wait.id());
        m.abort_all();
        while !wait.is_finished() {
            tokio::task::yield_now().await;
        }
    }
}
