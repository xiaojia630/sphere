//! Terminal tabs held by sphere-host, so your shells survive the app closing (docs/sphered.md: the host owns every
//! PTY). A tab is a host child labelled `sphere-term`; the app follows its output into the VT state and writes
//! keystrokes in order. On start the app re-attaches the tabs still running, replaying their output.
use serde_json::json;
use sphere_host::{ChildInfo, FollowItem, HostClient, PtySize, SpawnParams, Stream};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{mpsc, OnceLock};
use std::time::Duration;
use tokio::runtime::Runtime;
use tokio::sync::mpsc as tmpsc;

pub const LABEL: &str = "sphere-term";
/// Replay at most this much of a re-attached tab's output (its screen is rebuilt from it).
const REPLAY_MAX: u64 = 1 << 20;

fn rt() -> &'static Runtime {
    static RT: OnceLock<Runtime> = OnceLock::new();
    RT.get_or_init(|| tokio::runtime::Builder::new_multi_thread().worker_threads(1).thread_name("sphere-terms").enable_all().build().expect("tokio runtime"))
}

fn host_bin() -> PathBuf {
    if let Ok(me) = std::env::current_exe() {
        if let Some(dir) = me.parent() {
            let p = dir.join("sphere-host");
            if p.exists() {
                return p;
            }
        }
    }
    PathBuf::from("sphere-host")
}

/// The host, started if nothing answers; `None` when it can't be reached (tabs then live in the app).
pub fn client() -> Option<HostClient> {
    static C: OnceLock<Option<HostClient>> = OnceLock::new();
    C.get_or_init(|| {
        let socket = sphere_proto::paths::default_host_socket();
        let dir = sphere_proto::paths::default_state_dir().join("host");
        let _ = std::fs::create_dir_all(&dir);
        rt().block_on(async { tokio::time::timeout(Duration::from_secs(4), sphere_host::ensure_running(&host_bin(), &socket, &dir)).await.ok()?.ok() })
    })
    .clone()
}

/// Running tabs from a previous run of the app, oldest first.
pub fn running() -> Vec<ChildInfo> {
    let Some(c) = client() else { return Vec::new() };
    let mut v: Vec<ChildInfo> = rt().block_on(c.list()).unwrap_or_default().into_iter().filter(|ch| ch.label.as_deref() == Some(LABEL) && ch.running && ch.pty).collect();
    v.sort_by(|a, b| a.id.cmp(&b.id));
    v
}

/// Keystrokes go to the host in order through one task per tab.
pub struct HostWriter {
    tx: tmpsc::UnboundedSender<Vec<u8>>,
}

impl Write for HostWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let _ = self.tx.send(buf.to_vec());
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub struct Handle {
    pub id: String,
    client: HostClient,
}

impl Handle {
    pub fn kill(&self) {
        let (c, id) = (self.client.clone(), self.id.clone());
        rt().spawn(async move {
            let _ = c.signal(&id, json!("HUP"), true).await;
            tokio::time::sleep(Duration::from_millis(300)).await;
            let _ = c.forget(&id).await;
        });
    }
}

/// Follows a host child's output into a channel (closed when it exits) and opens its ordered writer.
fn wire(c: HostClient, id: String, from: u64) -> (Handle, mpsc::Receiver<Vec<u8>>, HostWriter) {
    let (out_tx, out_rx) = mpsc::channel();
    let (fc, fid) = (c.clone(), id.clone());
    rt().spawn(async move {
        let Ok(mut f) = fc.follow(&fid, Stream::Out, from).await else { return };
        while let Some(item) = f.next().await {
            match item {
                Ok(FollowItem::Data { bytes, .. }) => {
                    if out_tx.send(bytes).is_err() {
                        break;
                    }
                }
                Ok(FollowItem::Exit(_)) | Err(_) => break,
            }
        }
    });
    let (w_tx, mut w_rx) = tmpsc::unbounded_channel::<Vec<u8>>();
    let (wc, wid) = (c.clone(), id.clone());
    rt().spawn(async move {
        while let Some(b) = w_rx.recv().await {
            if wc.write(&wid, &b).await.is_err() {
                break;
            }
        }
    });
    (Handle { id, client: c }, out_rx, HostWriter { tx: w_tx })
}

/// A new login shell in the host.
pub fn spawn(cols: u16, rows: u16, shell: &str) -> Option<(Handle, mpsc::Receiver<Vec<u8>>, HostWriter)> {
    let c = client()?;
    let mut env = BTreeMap::new();
    env.insert("TERM".to_string(), "xterm-256color".to_string());
    env.insert("COLORTERM".to_string(), "truecolor".to_string());
    let home = std::env::var("HOME").ok();
    // zsh and bash get the shell integration that Fix it reads; any other shell is a plain login shell
    let launch = crate::shellint::launch(shell);
    env.extend(launch.env);
    let p = SpawnParams { cmd: shell.to_string(), args: launch.args, cwd: home, env, pty: Some(PtySize { cols, rows }), label: Some(LABEL.into()) };
    let r = rt().block_on(c.spawn(p)).ok()?;
    Some(wire(c, r.id, 0))
}

/// How many bytes of its old output `attach` has the host replay for this tab.
pub fn replay_len(ch: &ChildInfo) -> u64 {
    ch.out_len.min(REPLAY_MAX)
}

/// Re-attach a tab from a previous run, replaying (the tail of) its output.
pub fn attach(ch: &ChildInfo) -> Option<(Handle, mpsc::Receiver<Vec<u8>>, HostWriter)> {
    let c = client()?;
    let from = ch.out_len - replay_len(ch);
    Some(wire(c, ch.id.clone(), from))
}
