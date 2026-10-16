use crate::proto::*;
use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UnixStream;
use tokio::sync::Mutex;

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

struct Conn {
    r: BufReader<OwnedReadHalf>,
    w: OwnedWriteHalf,
}

/// A connection to the host. Calls are sequential; clone to share. Following
/// output opens its own connection.
#[derive(Clone)]
pub struct HostClient {
    path: PathBuf,
    conn: Arc<Mutex<Option<Conn>>>,
    next: Arc<AtomicU64>,
}

async fn open(path: &Path) -> Result<Conn> {
    let s = UnixStream::connect(path)
        .await
        .with_context(|| format!("connect {}", path.display()))?;
    let (r, w) = s.into_split();
    Ok(Conn {
        r: BufReader::new(r),
        w,
    })
}

impl HostClient {
    pub async fn connect(path: &Path) -> Result<HostClient> {
        let conn = open(path).await?;
        Ok(HostClient {
            path: path.to_path_buf(),
            conn: Arc::new(Mutex::new(Some(conn))),
            next: Arc::new(AtomicU64::new(1)),
        })
    }

    async fn roundtrip(conn: &mut Conn, req: &Request) -> Result<Response> {
        let mut line = serde_json::to_string(req)?;
        line.push('\n');
        conn.w.write_all(line.as_bytes()).await?;
        let mut resp = String::new();
        if conn.r.read_line(&mut resp).await? == 0 {
            bail!("host closed the connection");
        }
        Ok(serde_json::from_str(&resp)?)
    }

    /// Call a method. A dropped connection is re-opened once.
    pub async fn call<T: DeserializeOwned>(
        &self,
        method: &str,
        params: impl Serialize,
    ) -> Result<T> {
        let req = Request {
            id: self.next.fetch_add(1, Ordering::SeqCst),
            method: method.to_string(),
            params: serde_json::to_value(params)?,
        };
        let mut guard = self.conn.lock().await;
        let mut last_err = None;
        for _ in 0..2 {
            if guard.is_none() {
                match open(&self.path).await {
                    Ok(c) => *guard = Some(c),
                    Err(e) => {
                        last_err = Some(e);
                        break;
                    }
                }
            }
            match Self::roundtrip(guard.as_mut().unwrap(), &req).await {
                Ok(resp) => {
                    if let Some(e) = resp.error {
                        return Err(anyhow!("host: {e}"));
                    }
                    return Ok(serde_json::from_value(resp.result.unwrap_or(Value::Null))?);
                }
                Err(e) => {
                    *guard = None;
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow!("host unreachable")))
    }

    pub async fn ping(&self) -> Result<PingResult> {
        self.call(method::PING, json!({})).await
    }

    pub async fn spawn(&self, p: SpawnParams) -> Result<SpawnResult> {
        self.call(method::SPAWN, p).await
    }

    pub async fn write(&self, id: &str, bytes: &[u8]) -> Result<()> {
        let _: Value = self
            .call(
                method::WRITE,
                WriteParams {
                    id: id.into(),
                    bytes: B64.encode(bytes),
                },
            )
            .await?;
        Ok(())
    }

    pub async fn close_stdin(&self, id: &str) -> Result<()> {
        let _: Value = self
            .call(method::CLOSE_STDIN, IdParams { id: id.into() })
            .await?;
        Ok(())
    }

    pub async fn resize(&self, id: &str, cols: u16, rows: u16) -> Result<()> {
        let _: Value = self
            .call(
                method::RESIZE,
                ResizeParams {
                    id: id.into(),
                    cols,
                    rows,
                },
            )
            .await?;
        Ok(())
    }

    pub async fn signal(&self, id: &str, sig: Value, group: bool) -> Result<()> {
        let _: Value = self
            .call(
                method::SIGNAL,
                SignalParams {
                    id: id.into(),
                    sig,
                    group,
                },
            )
            .await?;
        Ok(())
    }

    pub async fn list(&self) -> Result<Vec<ChildInfo>> {
        self.call(method::LIST, json!({})).await
    }

    /// One read, decoded.
    pub async fn read(
        &self,
        id: &str,
        stream: Stream,
        from_offset: u64,
        max: Option<usize>,
    ) -> Result<DecodedRead> {
        let r: ReadResult = self
            .call(
                method::READ,
                ReadParams {
                    id: id.into(),
                    stream,
                    from_offset,
                    max,
                },
            )
            .await?;
        let bytes = B64.decode(&r.data)?;
        Ok(DecodedRead {
            data: String::from_utf8_lossy(&bytes).to_string(),
            bytes,
            offset: r.offset,
            eof: r.eof,
        })
    }

    pub async fn forget(&self, id: &str) -> Result<()> {
        let _: Value = self
            .call(method::FORGET, IdParams { id: id.into() })
            .await?;
        Ok(())
    }

    pub async fn shutdown(&self) -> Result<()> {
        let _: Value = self.call(method::SHUTDOWN, json!({})).await?;
        Ok(())
    }

    /// Stream a child's output from `from_offset`, then its exit.
    pub async fn follow(&self, id: &str, stream: Stream, from_offset: u64) -> Result<Follow> {
        let mut conn = open(&self.path).await?;
        let req = Request {
            id: self.next.fetch_add(1, Ordering::SeqCst),
            method: method::FOLLOW.into(),
            params: serde_json::to_value(ReadParams {
                id: id.into(),
                stream,
                from_offset,
                max: None,
            })?,
        };
        let resp = Self::roundtrip(&mut conn, &req).await?;
        if let Some(e) = resp.error {
            bail!("host: {e}");
        }
        Ok(Follow { conn })
    }
}

pub struct DecodedRead {
    pub bytes: Vec<u8>,
    pub data: String,
    pub offset: u64,
    pub eof: bool,
}

#[derive(Debug)]
pub enum FollowItem {
    Data { offset: u64, bytes: Vec<u8> },
    Exit(ExitStatus),
}

pub struct Follow {
    conn: Conn,
}

impl Follow {
    /// The next item, or `None` when the host closed the connection without
    /// reporting an exit (the host is gone).
    pub async fn next(&mut self) -> Option<Result<FollowItem>> {
        let mut line = String::new();
        match self.conn.r.read_line(&mut line).await {
            Ok(0) => None,
            Err(e) => Some(Err(e.into())),
            Ok(_) => Some(
                serde_json::from_str::<FollowEvent>(&line)
                    .map_err(anyhow::Error::from)
                    .and_then(|ev| match ev {
                        FollowEvent::Data { offset, data } => Ok(FollowItem::Data {
                            offset,
                            bytes: B64.decode(data)?,
                        }),
                        FollowEvent::Exit { status } => Ok(FollowItem::Exit(status)),
                    }),
            ),
        }
    }
}

/// Connect to the host on `socket`, starting it detached (it double-forks, so
/// nothing that kills the caller can kill it) when nothing answers.
pub async fn ensure_running(host_bin: &Path, socket: &Path, dir: &Path) -> Result<HostClient> {
    if let Ok(c) = HostClient::connect(socket).await {
        if c.ping().await.is_ok() {
            return Ok(c);
        }
    }
    let bin = host_bin.to_path_buf();
    let (socket_s, dir_s) = (socket.to_path_buf(), dir.to_path_buf());
    let status = tokio::task::spawn_blocking(move || {
        std::process::Command::new(&bin)
            .arg("--daemonize")
            .arg("--socket")
            .arg(&socket_s)
            .arg("--dir")
            .arg(&dir_s)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .with_context(|| format!("start {}", bin.display()))
    })
    .await??;
    if !status.success() {
        bail!("sphere-host failed to start ({status})");
    }
    for _ in 0..100 {
        if let Ok(c) = HostClient::connect(socket).await {
            if c.ping().await.is_ok() {
                return Ok(c);
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    bail!("sphere-host did not come up on {}", socket.display())
}
