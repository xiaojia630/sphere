//! A typed async client for the sphered API. The CLI and the app use the same
//! calls. One [`Client`] is one connection; it can be shared between tasks.

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use sphere_proto::api::Snapshot;
use sphere_proto::api::{self, kind, method, notify, RpcError, RpcMessage};
use sphere_proto::{Event, Note, NoteSource};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::OwnedWriteHalf;
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot};

pub use sphere_proto::api::{
    AnswerParams, BringInParams, ClientKind, HistoryParams, LiveFrame, SessionInfo, StartParams,
    Status,
};
pub use sphere_proto::Protocol;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("cannot reach sphered: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Rpc(RpcError),
    #[error("the connection to sphered closed")]
    Closed,
    #[error("unexpected reply: {0}")]
    Protocol(String),
}

impl Error {
    /// The application error kind (`already_answered`, `not_found`, ...).
    pub fn kind(&self) -> Option<&str> {
        match self {
            Error::Rpc(e) => e.kind(),
            _ => None,
        }
    }

    pub fn is_already_answered(&self) -> bool {
        self.kind() == Some(kind::ALREADY_ANSWERED)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// What a subscription delivers.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)] // events dominate; boxing them would only add an allocation per message
pub enum Update {
    Event(Event),
    /// A typing delta; never stored, may be missing from history.
    Live(LiveFrame),
    /// The server dropped us for falling behind. Reconnect and resync.
    Lagged(RpcError),
    Closed,
}

pub struct Subscription {
    rx: mpsc::UnboundedReceiver<Update>,
}

impl Subscription {
    /// The next update, or `None` after `Closed` was delivered.
    pub async fn next(&mut self) -> Option<Update> {
        self.rx.recv().await
    }
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<std::result::Result<Value, RpcError>>>>>;

struct Inner {
    writer: tokio::sync::Mutex<OwnedWriteHalf>,
    pending: Pending,
    next_id: AtomicU64,
    updates: Mutex<Option<mpsc::UnboundedReceiver<Update>>>,
}

#[derive(Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

impl Client {
    pub async fn connect(socket: &Path) -> Result<Client> {
        let stream = UnixStream::connect(socket).await?;
        let (r, w) = stream.into_split();
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let (tx, rx) = mpsc::unbounded_channel();
        let p = pending.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(r).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(msg) = serde_json::from_str::<RpcMessage>(&line) else {
                    continue;
                };
                match (&msg.id, &msg.method) {
                    (Some(id), _) if !id.is_null() => {
                        let Some(id) = id.as_u64() else { continue };
                        if let Some(waiter) = p.lock().unwrap().remove(&id) {
                            let _ = waiter.send(match msg.error {
                                Some(e) => Err(e),
                                None => Ok(msg.result.unwrap_or(Value::Null)),
                            });
                        }
                    }
                    (_, Some(m)) => {
                        let params = msg.params.unwrap_or(Value::Null);
                        let update = match m.as_str() {
                            notify::EVENT => serde_json::from_value(params).ok().map(Update::Event),
                            notify::LIVE => serde_json::from_value(params).ok().map(Update::Live),
                            _ => None,
                        };
                        if let Some(u) = update {
                            let _ = tx.send(u);
                        }
                    }
                    // An error with no id is the server telling us it dropped us.
                    _ => {
                        if let Some(e) = msg.error {
                            let _ = tx.send(Update::Lagged(e));
                        }
                    }
                }
            }
            // Wake every caller still waiting.
            p.lock().unwrap().clear();
            let _ = tx.send(Update::Closed);
        });
        Ok(Client {
            inner: Arc::new(Inner {
                writer: tokio::sync::Mutex::new(w),
                pending,
                next_id: AtomicU64::new(1),
                updates: Mutex::new(Some(rx)),
            }),
        })
    }

    pub async fn connect_default() -> Result<Client> {
        Self::connect(&sphere_proto::paths::default_socket()).await
    }

    pub async fn call<T: DeserializeOwned>(
        &self,
        method: &str,
        params: impl Serialize,
    ) -> Result<T> {
        let id = self.inner.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.inner.pending.lock().unwrap().insert(id, tx);
        let req =
            serde_json::json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let mut line = req.to_string();
        line.push('\n');
        {
            let mut w = self.inner.writer.lock().await;
            if let Err(e) = w.write_all(line.as_bytes()).await {
                self.inner.pending.lock().unwrap().remove(&id);
                return Err(e.into());
            }
        }
        match rx.await {
            Ok(Ok(v)) => serde_json::from_value(v).map_err(|e| Error::Protocol(e.to_string())),
            Ok(Err(e)) => Err(Error::Rpc(e)),
            Err(_) => Err(Error::Closed),
        }
    }

    // ---- sync ------------------------------------------------------------------

    pub async fn hello(&self, client: ClientKind) -> Result<()> {
        let _: api::Ack = self
            .call(method::HELLO, api::HelloParams { client })
            .await?;
        Ok(())
    }

    pub async fn snapshot(&self) -> Result<Snapshot> {
        self.call(method::SNAPSHOT, serde_json::json!({})).await
    }

    pub async fn status(&self) -> Result<Status> {
        self.call(method::STATUS, serde_json::json!({})).await
    }

    pub async fn history(&self, p: HistoryParams) -> Result<Vec<Event>> {
        Ok(self
            .call::<api::HistoryResult>(method::HISTORY, p)
            .await?
            .events)
    }

    pub async fn blob_read(&self, p: api::BlobReadParams) -> Result<String> {
        Ok(self
            .call::<api::BlobReadResult>(method::BLOB_READ, p)
            .await?
            .text)
    }

    /// Replay durable events after `since`, then stream live. Once per client.
    /// Returns the seq at subscription time.
    pub async fn subscribe(&self, since: u64, task: Option<String>) -> Result<(u64, Subscription)> {
        let rx = self
            .inner
            .updates
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| Error::Protocol("this client is already subscribed".into()))?;
        let r: api::SubscribeResult = self
            .call(method::SUBSCRIBE, api::SubscribeParams { since, task })
            .await?;
        Ok((r.seq, Subscription { rx }))
    }

    // ---- tasks -------------------------------------------------------------------

    pub async fn start_task(&self, p: StartParams) -> Result<String> {
        Ok(self
            .call::<api::StartResult>(method::TASK_START, p)
            .await?
            .task)
    }

    pub async fn send(&self, task: &str, text: &str) -> Result<()> {
        let _: api::Ack = self
            .call(
                method::TASK_SEND,
                api::SendParams {
                    task: task.into(),
                    text: text.into(),
                },
            )
            .await?;
        Ok(())
    }

    pub async fn interrupt(&self, task: &str) -> Result<()> {
        let _: api::Ack = self
            .call(
                method::TASK_INTERRUPT,
                api::TaskParams { task: task.into() },
            )
            .await?;
        Ok(())
    }

    pub async fn stop(&self, task: &str) -> Result<()> {
        let _: api::Ack = self
            .call(method::TASK_STOP, api::TaskParams { task: task.into() })
            .await?;
        Ok(())
    }

    pub async fn set_mode(&self, task: &str, mode: &str) -> Result<()> {
        let _: api::Ack = self
            .call(
                method::TASK_SET_MODE,
                api::SetModeParams {
                    task: task.into(),
                    mode: mode.into(),
                },
            )
            .await?;
        Ok(())
    }

    pub async fn set_model(&self, task: &str, model: &str) -> Result<()> {
        let _: api::Ack = self
            .call(
                method::TASK_SET_MODEL,
                api::SetModelParams {
                    task: task.into(),
                    model: model.into(),
                },
            )
            .await?;
        Ok(())
    }

    /// Not implemented by sphered yet; always an error of kind `not_implemented`.
    pub async fn rewind(&self, task: &str) -> Result<()> {
        let _: api::Ack = self
            .call(method::TASK_REWIND, api::TaskParams { task: task.into() })
            .await?;
        Ok(())
    }

    pub async fn session_list(&self) -> Result<Vec<SessionInfo>> {
        self.call(method::SESSION_LIST, serde_json::json!({})).await
    }

    pub async fn session_outside(&self) -> Result<Vec<api::OutsideSession>> {
        self.call(method::SESSION_OUTSIDE, serde_json::json!({}))
            .await
    }

    pub async fn take_over(&self, session_id: &str) -> Result<api::TakeOverResult> {
        self.call(
            method::SESSION_TAKE_OVER,
            api::TakeOverParams {
                session_id: session_id.into(),
            },
        )
        .await
    }

    pub async fn bring_in(&self, p: BringInParams) -> Result<String> {
        Ok(self
            .call::<api::StartResult>(method::SESSION_BRING_IN, p)
            .await?
            .task)
    }

    // ---- notes ---------------------------------------------------------------------

    /// The user's notes, newest first. With `repo`, only the ones an agent working there is told.
    pub async fn memory_list(&self, repo: Option<&str>) -> Result<Vec<Note>> {
        let list: api::MemoryList = self
            .call(
                method::MEMORY_LIST,
                api::MemoryListParams {
                    repo: repo.map(str::to_string),
                },
            )
            .await?;
        Ok(list.notes)
    }

    /// Keep a note for `repo` (a folder inside it will do), or for every repo with `None`.
    pub async fn memory_add(
        &self,
        repo: Option<&str>,
        text: &str,
        source: NoteSource,
    ) -> Result<api::MemoryAdded> {
        self.call(
            method::MEMORY_ADD,
            api::MemoryAddParams {
                repo: repo.map(str::to_string),
                all: repo.is_none(),
                text: text.into(),
                source,
            },
        )
        .await
    }

    pub async fn memory_remove(&self, id: u64) -> Result<()> {
        let _: api::Ack = self
            .call(method::MEMORY_REMOVE, api::MemoryRemoveParams { id })
            .await?;
        Ok(())
    }

    // ---- protocols -----------------------------------------------------------------

    /// Write a protocol from a line such as `every weekday 9:00 sphere: update the dependencies`.
    /// `repo` is the folder for a line that names none. Returns the protocol with its next run.
    pub async fn protocol_add(&self, text: &str, repo: Option<&str>) -> Result<Protocol> {
        self.call(
            method::PROTOCOL_ADD,
            api::ProtocolAddParams {
                text: text.into(),
                repo: repo.map(Into::into),
            },
        )
        .await
    }

    /// The saved protocols, oldest first.
    pub async fn protocol_list(&self) -> Result<Vec<Protocol>> {
        Ok(self
            .call::<api::ProtocolList>(method::PROTOCOL_LIST, serde_json::json!({}))
            .await?
            .protocols)
    }

    pub async fn protocol_set_enabled(&self, id: &str, enabled: bool) -> Result<Protocol> {
        self.call(
            method::PROTOCOL_SET_ENABLED,
            api::ProtocolEnabledParams {
                id: id.into(),
                enabled,
            },
        )
        .await
    }

    pub async fn protocol_remove(&self, id: &str) -> Result<()> {
        let _: api::Ack = self
            .call(
                method::PROTOCOL_REMOVE,
                api::ProtocolParams { id: id.into() },
            )
            .await?;
        Ok(())
    }

    /// Run it now, besides its schedule. Returns the task.
    pub async fn protocol_run_now(&self, id: &str) -> Result<String> {
        Ok(self
            .call::<api::StartResult>(
                method::PROTOCOL_RUN_NOW,
                api::ProtocolParams { id: id.into() },
            )
            .await?
            .task)
    }

    // ---- requests ------------------------------------------------------------------

    pub async fn answer(&self, p: AnswerParams) -> Result<()> {
        let _: api::Ack = self.call(method::REQUEST_ANSWER, p).await?;
        Ok(())
    }

    pub async fn yes(&self, request: &str) -> Result<()> {
        self.answer(AnswerParams {
            request: request.into(),
            answer: sphere_proto::AnswerKind::Yes,
            text: None,
            input: None,
            always: false,
        })
        .await
    }

    pub async fn change(&self, request: &str, text: &str) -> Result<()> {
        self.answer(AnswerParams {
            request: request.into(),
            answer: sphere_proto::AnswerKind::Change,
            text: Some(text.into()),
            input: None,
            always: false,
        })
        .await
    }

    pub async fn later(&self, request: &str) -> Result<()> {
        self.answer(AnswerParams {
            request: request.into(),
            answer: sphere_proto::AnswerKind::Later,
            text: None,
            input: None,
            always: false,
        })
        .await
    }
}
