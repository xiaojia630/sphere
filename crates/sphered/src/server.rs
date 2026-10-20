//! The API socket: JSON-RPC 2.0, newline-delimited, one task per connection.

use crate::hub::Msg;
use crate::manager::{ApiError, Manager};
use crate::peer;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use sphere_proto::api::{self, code, kind, method, notify, ClientKind, RpcError};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, watch, Notify};

const REPLAY_PAGE: usize = 500;

struct Conn {
    kind: ClientKind,
    pid: Option<i32>,
    subscribed: bool,
    out: mpsc::Sender<String>,
    close: Arc<Notify>,
    /// A subscription to start streaming once its reply has been queued, so
    /// no event can overtake the reply.
    start_stream: Option<(crate::hub::Subscription, api::SubscribeParams)>,
}

pub async fn serve(mgr: Arc<Manager>, listener: UnixListener, mut shutdown: watch::Receiver<bool>) {
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else { continue };
                let mgr = mgr.clone();
                tokio::spawn(async move { handle(mgr, stream).await });
            }
            _ = shutdown.changed() => return,
        }
    }
}

fn parse<T: DeserializeOwned>(v: Value) -> Result<T, RpcError> {
    serde_json::from_value(v).map_err(|e| RpcError {
        code: code::INVALID_PARAMS,
        message: format!("invalid params: {e}"),
        data: None,
    })
}

fn ok<T: serde::Serialize>(v: T) -> Result<Value, RpcError> {
    serde_json::to_value(v).map_err(|e| RpcError {
        code: code::INTERNAL,
        message: e.to_string(),
        data: None,
    })
}

fn response(id: &Value, r: Result<Value, RpcError>) -> String {
    match r {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err(error) => json!({"jsonrpc": "2.0", "id": id, "error": error}),
    }
    .to_string()
}

async fn handle(mgr: Arc<Manager>, stream: UnixStream) {
    let cred = stream.peer_cred();
    match &cred {
        Ok(c) if c.uid() == peer::my_uid() => {}
        _ => {
            tracing::warn!("refusing a connection from another user");
            return;
        }
    }
    let pid = cred.ok().and_then(|c| c.pid());
    let (r, mut w) = stream.into_split();
    let (out_tx, mut out_rx) = mpsc::channel::<String>(256);
    let close = Arc::new(Notify::new());
    let writer = tokio::spawn(async move {
        while let Some(mut line) = out_rx.recv().await {
            line.push('\n');
            if w.write_all(line.as_bytes()).await.is_err() {
                break;
            }
        }
    });
    let mut conn = Conn {
        kind: ClientKind::Cli,
        pid,
        subscribed: false,
        out: out_tx,
        close: close.clone(),
        start_stream: None,
    };
    let mut lines = BufReader::new(r).lines();
    loop {
        let line = tokio::select! {
            l = lines.next_line() => match l { Ok(Some(l)) => l, _ => break },
            _ = close.notified() => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<api::RpcRequest>(&line) {
            Err(e) => response(
                &Value::Null,
                Err(RpcError {
                    code: code::PARSE,
                    message: format!("parse error: {e}"),
                    data: None,
                }),
            ),
            Ok(req) => {
                let result = dispatch(&mgr, &mut conn, &req.method, req.params).await;
                response(&req.id, result)
            }
        };
        if conn.out.send(reply).await.is_err() {
            break;
        }
        if let Some((sub, p)) = conn.start_stream.take() {
            tokio::spawn(forward(
                mgr.clone(),
                sub,
                p,
                conn.out.clone(),
                conn.close.clone(),
            ));
        }
    }
    drop(conn);
    let _ = writer.await;
}

async fn dispatch(
    mgr: &Arc<Manager>,
    conn: &mut Conn,
    m: &str,
    params: Value,
) -> Result<Value, RpcError> {
    let r: Result<Value, ApiError> = async {
        match m {
            method::HELLO => {
                conn.kind = parse::<api::HelloParams>(params)?.client;
                Ok(ok(api::Ack { ok: true })?)
            }
            method::SNAPSHOT => Ok(ok(mgr.snapshot())?),
            method::STATUS => Ok(ok(mgr.status().await)?),
            method::HISTORY => {
                let p: api::HistoryParams = parse(params)?;
                let events = mgr
                    .hub
                    .history(
                        p.task.as_deref(),
                        p.before,
                        p.limit.unwrap_or(100).clamp(1, 1000),
                    )
                    .map_err(ApiError::from)?;
                Ok(ok(api::HistoryResult { events })?)
            }
            method::BLOB_READ => Ok(ok(mgr.blob_read(parse(params)?).await?)?),
            method::SUBSCRIBE => {
                if conn.subscribed {
                    return Err(ApiError(RpcError::app(
                        kind::INVALID_STATE,
                        "already subscribed on this connection",
                    )));
                }
                let p: api::SubscribeParams = parse(params)?;
                conn.subscribed = true;
                let sub = mgr.hub.subscribe(p.task.clone());
                let upto = sub.replay_upto;
                conn.start_stream = Some((sub, p));
                Ok(ok(api::SubscribeResult { seq: upto })?)
            }
            method::TASK_START => Ok(ok(mgr.start_task(parse(params)?).await?)?),
            method::TASK_SEND => {
                let p: api::SendParams = parse(params)?;
                mgr.send(&p.task, p.text).await?;
                Ok(ok(api::Ack { ok: true })?)
            }
            method::TASK_INTERRUPT => {
                mgr.interrupt(&parse::<api::TaskParams>(params)?.task)
                    .await?;
                Ok(ok(api::Ack { ok: true })?)
            }
            method::TASK_STOP => {
                mgr.stop(&parse::<api::TaskParams>(params)?.task).await?;
                Ok(ok(api::Ack { ok: true })?)
            }
            method::TASK_SET_MODE => {
                let p: api::SetModeParams = parse(params)?;
                mgr.set_mode(&p.task, p.mode).await?;
                Ok(ok(api::Ack { ok: true })?)
            }
            method::TASK_SET_MODEL => {
                let p: api::SetModelParams = parse(params)?;
                mgr.set_model(&p.task, p.model).await?;
                Ok(ok(api::Ack { ok: true })?)
            }
            method::TASK_REWIND => Err(ApiError(RpcError::app(
                kind::NOT_IMPLEMENTED,
                "rewind is not implemented yet",
            ))),
            method::SESSION_LIST => Ok(ok(mgr.session_list().await?)?),
            method::SESSION_OUTSIDE => Ok(ok(mgr.session_outside().await?)?),
            method::SESSION_TAKE_OVER => Ok(ok(mgr.take_over(parse(params)?).await?)?),
            method::SESSION_BRING_IN => Ok(ok(mgr.bring_in(parse(params)?).await?)?),
            method::REQUEST_ANSWER => {
                let p: api::AnswerParams = parse(params)?;
                authorize_user(mgr, conn, "answer requests").await?;
                mgr.answer(p).await?;
                Ok(ok(api::Ack { ok: true })?)
            }
            method::MEMORY_LIST => Ok(ok(mgr.memory_list(parse(params)?).await?)?),
            method::MEMORY_ADD => {
                let p: api::MemoryAddParams = parse(params)?;
                authorize_user(mgr, conn, "change your notes").await?;
                Ok(ok(mgr.memory_add(p).await?)?)
            }
            method::MEMORY_REMOVE => {
                let p: api::MemoryRemoveParams = parse(params)?;
                authorize_user(mgr, conn, "change your notes").await?;
                mgr.memory_remove(p.id)?;
                Ok(ok(api::Ack { ok: true })?)
            }
            method::PROTOCOL_ADD => {
                let p: api::ProtocolAddParams = parse(params)?;
                authorize_user(mgr, conn, "change your protocols").await?;
                Ok(ok(mgr.protocol_add(p).await?)?)
            }
            method::PROTOCOL_LIST => Ok(ok(mgr.protocol_list())?),
            method::PROTOCOL_SET_ENABLED => {
                let p: api::ProtocolEnabledParams = parse(params)?;
                authorize_user(mgr, conn, "change your protocols").await?;
                Ok(ok(mgr.protocol_set_enabled(&p.id, p.enabled).await?)?)
            }
            method::PROTOCOL_REMOVE => {
                let p: api::ProtocolParams = parse(params)?;
                authorize_user(mgr, conn, "change your protocols").await?;
                mgr.protocol_remove(&p.id).await?;
                Ok(ok(api::Ack { ok: true })?)
            }
            method::PROTOCOL_RUN_NOW => {
                let p: api::ProtocolParams = parse(params)?;
                Ok(ok(mgr.protocol_run_now(&p.id).await?)?)
            }
            other => Err(ApiError(RpcError {
                code: code::METHOD_NOT_FOUND,
                message: format!("unknown method `{other}`"),
                data: None,
            })),
        }
    }
    .await;
    r.map_err(|e| e.0)
}

impl From<RpcError> for ApiError {
    fn from(e: RpcError) -> Self {
        ApiError(e)
    }
}

/// Only you decide what `what` names (answer requests, change your notes or your protocols): not the assistant, and
/// not anything an agent started. A note or a schedule an agent wrote would outlive its task.
async fn authorize_user(mgr: &Arc<Manager>, conn: &Conn, what: &str) -> Result<(), ApiError> {
    let forbidden = |why: String| ApiError(RpcError::app(kind::FORBIDDEN, why));
    if conn.kind == ClientKind::Assistant {
        return Err(forbidden(format!(
            "the assistant cannot {what}; only you can"
        )));
    }
    match conn.pid {
        Some(pid) => {
            let roots = mgr.agent_roots().await;
            if peer::descends_from(pid, &roots) {
                return Err(forbidden(format!(
                    "a process an agent started cannot {what}"
                )));
            }
        }
        None => tracing::warn!("could not identify the caller's process; allowing it to {what}"),
    }
    Ok(())
}

/// Replay stored events, then stream live ones. Ends when the client goes
/// away or falls too far behind.
async fn forward(
    mgr: Arc<Manager>,
    mut sub: crate::hub::Subscription,
    p: api::SubscribeParams,
    out: mpsc::Sender<String>,
    close: Arc<Notify>,
) {
    let note = |method: &str, params: Value| {
        json!({"jsonrpc": "2.0", "method": method, "params": params}).to_string()
    };
    let mut since = p.since;
    loop {
        let page = match mgr
            .hub
            .replay_page(since, sub.replay_upto, p.task.as_deref(), REPLAY_PAGE)
        {
            Ok(p) => p,
            Err(e) => {
                tracing::error!("replay failed: {e:#}");
                close.notify_one();
                return;
            }
        };
        let Some(last) = page.last().map(|e| e.seq) else {
            break;
        };
        for ev in page {
            if out
                .send(note(
                    notify::EVENT,
                    serde_json::to_value(&ev).unwrap_or_default(),
                ))
                .await
                .is_err()
            {
                return;
            }
        }
        since = last;
    }
    while let Some(msg) = sub.rx.recv().await {
        let line = match msg {
            Msg::Event(ev) => note(notify::EVENT, serde_json::to_value(&ev).unwrap_or_default()),
            Msg::Live(f) => note(notify::LIVE, serde_json::to_value(&f).unwrap_or_default()),
        };
        if out.send(line).await.is_err() {
            return;
        }
    }
    if sub.lagged.load(Ordering::SeqCst) {
        let err = response(
            &Value::Null,
            Err(RpcError::app(
                kind::LAGGED,
                "the client fell too far behind; reconnect and resync from a snapshot",
            )),
        );
        let _ = out.send(err).await;
    }
    close.notify_one();
}
