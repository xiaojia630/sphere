use crate::proto::*;
use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::{DirBuilderExt, FileExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, watch, Notify};

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;
/// How long after the child exits we stop waiting for its output pipes to
/// close (a grandchild can hold them open).
const EXIT_DRAIN: Duration = Duration::from_millis(300);

pub struct ServeOptions {
    pub socket: PathBuf,
    /// Where the append-only output files go.
    pub dir: PathBuf,
    /// Become a child subreaper and reap every orphan. The real host does;
    /// hosts embedded in a test process must not touch other children.
    pub reap_orphans: bool,
}

/// Create `path` (and parents) readable by the owner only.
pub fn ensure_private_dir(path: &Path) -> Result<()> {
    if !path.exists() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)
            .with_context(|| format!("create {}", path.display()))?;
    }
    Ok(())
}

struct Chan {
    path: PathBuf,
    len: AtomicU64,
    done: AtomicBool,
}

struct Child {
    id: String,
    pid: u32,
    label: Option<String>,
    cmd: String,
    cwd: Option<String>,
    pty: bool,
    out: Chan,
    err: Chan,
    stdin: Mutex<Option<mpsc::UnboundedSender<Vec<u8>>>>,
    master: Option<OwnedFd>,
    exit: Mutex<Option<(ExitStatus, Instant)>>,
    /// Bumped on new output, output closing and exit.
    changed: watch::Sender<u64>,
}

impl Child {
    fn chan(&self, s: Stream) -> &Chan {
        match s {
            Stream::Out => &self.out,
            Stream::Err => &self.err,
        }
    }

    fn bump(&self) {
        self.changed.send_modify(|n| *n += 1);
    }

    fn exit(&self) -> Option<(ExitStatus, Instant)> {
        self.exit.lock().unwrap().clone()
    }

    fn info(&self) -> ChildInfo {
        let exit = self.exit().map(|(s, _)| s);
        ChildInfo {
            id: self.id.clone(),
            pid: self.pid,
            label: self.label.clone(),
            cmd: self.cmd.clone(),
            cwd: self.cwd.clone(),
            pty: self.pty,
            running: exit.is_none(),
            exit,
            out_len: self.out.len.load(Ordering::SeqCst),
            err_len: self.err.len.load(Ordering::SeqCst),
        }
    }

    fn set_exit(&self, status: ExitStatus) {
        *self.exit.lock().unwrap() = Some((status, Instant::now()));
        if !self.pty {
            // Nothing can read it any more.
            self.stdin.lock().unwrap().take();
        }
        self.bump();
    }
}

/// Copy everything `src` yields into `file`, publishing the length as it grows.
fn pump(mut src: impl Read + Send + 'static, mut file: File, child: Arc<Child>, which: Stream) {
    std::thread::spawn(move || {
        let chan = child.chan(which);
        let mut buf = [0u8; 16 * 1024];
        loop {
            match src.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if file.write_all(&buf[..n]).is_err() {
                        break;
                    }
                    chan.len.fetch_add(n as u64, Ordering::SeqCst);
                    child.bump();
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        chan.done.store(true, Ordering::SeqCst);
        child.bump();
    });
}

fn writer_thread(mut sink: impl Write + Send + 'static, mut rx: mpsc::UnboundedReceiver<Vec<u8>>) {
    std::thread::spawn(move || {
        while let Some(bytes) = rx.blocking_recv() {
            if sink.write_all(&bytes).and_then(|_| sink.flush()).is_err() {
                break;
            }
        }
    });
}

struct Host {
    dir: PathBuf,
    reap_orphans: bool,
    children: Mutex<HashMap<String, Arc<Child>>>,
    next: AtomicU64,
    shutdown: Notify,
    /// Set once a shutdown starts: the children are being killed, and
    /// followers must see the connection drop, not a clean exit, so their
    /// owner knows the host went away rather than the child finishing.
    closing: AtomicBool,
}

fn exit_status(raw: i32) -> ExitStatus {
    if libc::WIFEXITED(raw) {
        ExitStatus { code: Some(libc::WEXITSTATUS(raw)), signal: None }
    } else if libc::WIFSIGNALED(raw) {
        ExitStatus { code: None, signal: Some(libc::WTERMSIG(raw)) }
    } else {
        ExitStatus { code: None, signal: None }
    }
}

impl Host {
    fn new(dir: PathBuf, reap_orphans: bool) -> Host {
        // Output files of earlier hosts stay; never reuse their ids.
        let mut next = 1;
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if let Some(n) = name
                    .strip_prefix('c')
                    .and_then(|r| r.split('.').next())
                    .and_then(|n| n.parse::<u64>().ok())
                {
                    next = next.max(n + 1);
                }
            }
        }
        Host {
            dir,
            reap_orphans,
            children: Mutex::new(HashMap::new()),
            next: AtomicU64::new(next),
            shutdown: Notify::new(),
            closing: AtomicBool::new(false),
        }
    }

    fn get(&self, id: &str) -> Result<Arc<Child>> {
        self.children
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow!("no such child: {id}"))
    }

    /// Collect the exit status of every child that has exited. Runs under the
    /// children lock so a child that exits during `spawn` is never missed.
    fn reap(&self) {
        let children = self.children.lock().unwrap();
        if !self.reap_orphans {
            for c in children.values().filter(|c| c.exit().is_none()) {
                let mut raw = 0;
                if unsafe { libc::waitpid(c.pid as i32, &mut raw, libc::WNOHANG) } == c.pid as i32 {
                    c.set_exit(exit_status(raw));
                }
            }
            return;
        }
        loop {
            let mut raw = 0;
            let pid = unsafe { libc::waitpid(-1, &mut raw, libc::WNOHANG) };
            if pid <= 0 {
                break;
            }
            if let Some(c) = children.values().find(|c| c.pid as i32 == pid) {
                c.set_exit(exit_status(raw));
            }
            // Anything else is an orphan adopted as subreaper; reaping is all it needs.
        }
    }

    fn spawn(&self, p: SpawnParams) -> Result<SpawnResult> {
        let mut children = self.children.lock().unwrap();
        let id = format!("c{}", self.next.fetch_add(1, Ordering::SeqCst));
        let out_path = self.dir.join(format!("{id}.out"));
        let err_path = self.dir.join(format!("{id}.err"));
        let open = |path: &Path| {
            OpenOptions::new().create(true).append(true).open(path).with_context(|| path.display().to_string())
        };
        let out_file = open(&out_path)?;

        let mut cmd = Command::new(&p.cmd);
        cmd.args(&p.args).envs(&p.env);
        if let Some(cwd) = &p.cwd {
            cmd.current_dir(cwd);
        }

        let (pid, io) = if let Some(size) = p.pty {
            if !p.env.contains_key("TERM") {
                cmd.env("TERM", "xterm-256color");
            }
            let (master, slave) = open_pty(size)?;
            cmd.stdin(Stdio::from(slave.try_clone()?))
                .stdout(Stdio::from(slave.try_clone()?))
                .stderr(Stdio::from(slave));
            unsafe {
                cmd.pre_exec(|| {
                    if libc::setsid() < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    // The slave on stdin becomes the controlling terminal.
                    libc::ioctl(0, libc::TIOCSCTTY as _, 0);
                    Ok(())
                });
            }
            let child = cmd.spawn().with_context(|| format!("spawn {}", p.cmd))?;
            let pid = child.id();
            drop(cmd);
            (pid, Io::Pty(master))
        } else {
            cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
            unsafe {
                cmd.pre_exec(|| {
                    if libc::setsid() < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            let mut child = cmd.spawn().with_context(|| format!("spawn {}", p.cmd))?;
            let pid = child.id();
            (
                pid,
                Io::Pipes {
                    stdin: child.stdin.take(),
                    stdout: child.stdout.take(),
                    stderr: child.stderr.take(),
                },
            )
        };

        let (stdin_tx, stdin_rx) = mpsc::unbounded_channel();
        let (changed, _) = watch::channel(0u64);
        let is_pty = matches!(io, Io::Pty(_));
        let master_keep = match &io {
            Io::Pty(m) => Some(m.try_clone()?),
            _ => None,
        };
        let child = Arc::new(Child {
            id: id.clone(),
            pid,
            label: p.label.clone(),
            cmd: p.cmd.clone(),
            cwd: p.cwd.clone(),
            pty: is_pty,
            out: Chan { path: out_path, len: AtomicU64::new(0), done: AtomicBool::new(false) },
            err: Chan { path: err_path, len: AtomicU64::new(0), done: AtomicBool::new(false) },
            stdin: Mutex::new(Some(stdin_tx)),
            master: master_keep,
            exit: Mutex::new(None),
            changed,
        });
        match io {
            Io::Pty(master) => {
                child.err.done.store(true, Ordering::SeqCst);
                let reader = File::from(master.try_clone()?);
                writer_thread(File::from(master), stdin_rx);
                pump(reader, out_file, child.clone(), Stream::Out);
            }
            Io::Pipes { stdin, stdout, stderr } => {
                if let Some(stdin) = stdin {
                    writer_thread(stdin, stdin_rx);
                }
                if let Some(stdout) = stdout {
                    pump(stdout, out_file, child.clone(), Stream::Out);
                }
                match stderr {
                    Some(stderr) => pump(stderr, open(&child.err.path)?, child.clone(), Stream::Err),
                    None => child.err.done.store(true, Ordering::SeqCst),
                }
            }
        }
        children.insert(id.clone(), child);
        Ok(SpawnResult { id, pid })
    }

    fn kill_all(&self) {
        self.closing.store(true, Ordering::SeqCst);
        for c in self.children.lock().unwrap().values() {
            if c.exit().is_none() {
                unsafe { libc::kill(-(c.pid as i32), libc::SIGKILL) };
            }
        }
    }

    fn call(&self, method: &str, params: Value) -> Result<Value> {
        fn parse<T: serde::de::DeserializeOwned>(v: Value) -> Result<T> {
            serde_json::from_value(v).context("bad params")
        }
        match method {
            method::PING => Ok(serde_json::to_value(PingResult {
                pid: std::process::id(),
                version: VERSION.to_string(),
            })?),
            method::SPAWN => Ok(serde_json::to_value(self.spawn(parse(params)?)?)?),
            method::WRITE => {
                let p: WriteParams = parse(params)?;
                let bytes = B64.decode(&p.bytes).context("bytes is not base64")?;
                let c = self.get(&p.id)?;
                let tx = c.stdin.lock().unwrap();
                match tx.as_ref() {
                    Some(tx) => tx.send(bytes).map_err(|_| anyhow!("input is closed"))?,
                    None => bail!("input is closed"),
                }
                Ok(json!({}))
            }
            method::CLOSE_STDIN => {
                let c = self.get(&parse::<IdParams>(params)?.id)?;
                if c.pty {
                    // End of file on a terminal is ^D.
                    if let Some(tx) = c.stdin.lock().unwrap().as_ref() {
                        let _ = tx.send(vec![4]);
                    }
                } else {
                    c.stdin.lock().unwrap().take();
                }
                Ok(json!({}))
            }
            method::RESIZE => {
                let p: ResizeParams = parse(params)?;
                let c = self.get(&p.id)?;
                let Some(m) = &c.master else { bail!("not a terminal") };
                let ws = libc::winsize { ws_row: p.rows, ws_col: p.cols, ws_xpixel: 0, ws_ypixel: 0 };
                if unsafe { libc::ioctl(m.as_raw_fd(), libc::TIOCSWINSZ as _, &ws) } != 0 {
                    bail!("resize: {}", std::io::Error::last_os_error());
                }
                // The kernel signals the foreground group on its own; be sure.
                unsafe { libc::kill(-(c.pid as i32), libc::SIGWINCH) };
                Ok(json!({}))
            }
            method::SIGNAL => {
                let p: SignalParams = parse(params)?;
                let sig = signal_number(&p.sig).ok_or_else(|| anyhow!("unknown signal {}", p.sig))?;
                let c = self.get(&p.id)?;
                let target = if p.group { -(c.pid as i32) } else { c.pid as i32 };
                if c.exit().is_some() && !p.group {
                    bail!("child has exited");
                }
                let rc = unsafe { libc::kill(target, sig) };
                if rc != 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH) {
                    bail!("kill: {}", std::io::Error::last_os_error());
                }
                Ok(json!({}))
            }
            method::LIST => {
                let mut infos: Vec<ChildInfo> =
                    self.children.lock().unwrap().values().map(|c| c.info()).collect();
                infos.sort_by_key(|i| i.id[1..].parse::<u64>().unwrap_or(0));
                Ok(serde_json::to_value(infos)?)
            }
            method::READ => {
                let p: ReadParams = parse(params)?;
                let c = self.get(&p.id)?;
                let chan = c.chan(p.stream);
                let len = chan.len.load(Ordering::SeqCst);
                let max = p.max.unwrap_or(64 * 1024).min(4 * 1024 * 1024) as u64;
                let n = len.saturating_sub(p.from_offset).min(max);
                let data = read_at(&chan.path, p.from_offset, n as usize)?;
                let offset = p.from_offset + data.len() as u64;
                let eof = c.exit().is_some() && chan.done.load(Ordering::SeqCst) && offset >= len;
                Ok(serde_json::to_value(ReadResult { data: B64.encode(&data), offset, eof })?)
            }
            method::FORGET => {
                let id = parse::<IdParams>(params)?.id;
                let c = self.get(&id)?;
                if c.exit().is_none() {
                    bail!("child is still running");
                }
                self.children.lock().unwrap().remove(&id);
                let _ = std::fs::remove_file(&c.out.path);
                let _ = std::fs::remove_file(&c.err.path);
                Ok(json!({}))
            }
            method::SHUTDOWN => {
                self.kill_all();
                Ok(json!({}))
            }
            other => bail!("unknown method {other}"),
        }
    }
}

enum Io {
    Pty(OwnedFd),
    Pipes {
        stdin: Option<std::process::ChildStdin>,
        stdout: Option<std::process::ChildStdout>,
        stderr: Option<std::process::ChildStderr>,
    },
}

fn open_pty(size: PtySize) -> Result<(OwnedFd, OwnedFd)> {
    let (mut master, mut slave) = (0, 0);
    let ws = libc::winsize { ws_row: size.rows, ws_col: size.cols, ws_xpixel: 0, ws_ypixel: 0 };
    let rc = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &ws as *const libc::winsize as *mut libc::winsize,
        )
    };
    if rc != 0 {
        bail!("openpty: {}", std::io::Error::last_os_error());
    }
    let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
    for fd in [&master, &slave] {
        unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) };
    }
    Ok((master, slave))
}

fn read_at(path: &Path, offset: u64, n: usize) -> Result<Vec<u8>> {
    if n == 0 {
        return Ok(vec![]);
    }
    let f = File::open(path).with_context(|| path.display().to_string())?;
    let mut buf = vec![0u8; n];
    let mut got = 0;
    while got < n {
        let r = f.read_at(&mut buf[got..], offset + got as u64)?;
        if r == 0 {
            break;
        }
        got += r;
    }
    buf.truncate(got);
    Ok(buf)
}

async fn send_line(w: &mut tokio::net::unix::OwnedWriteHalf, v: &impl serde::Serialize) -> std::io::Result<()> {
    let mut s = serde_json::to_string(v).unwrap_or_default();
    s.push('\n');
    w.write_all(s.as_bytes()).await
}

async fn follow(host: &Host, p: ReadParams, id: u64, w: &mut tokio::net::unix::OwnedWriteHalf) -> std::io::Result<()> {
    let child = match host.get(&p.id) {
        Ok(c) => c,
        Err(e) => {
            return send_line(w, &Response { id, result: None, error: Some(e.to_string()) }).await;
        }
    };
    send_line(w, &Response { id, result: Some(json!({"following": true})), error: None }).await?;
    let chan = child.chan(p.stream);
    let mut off = p.from_offset;
    let mut rx = child.changed.subscribe();
    loop {
        rx.borrow_and_update();
        let len = chan.len.load(Ordering::SeqCst);
        if off < len {
            let n = (len - off).min(64 * 1024) as usize;
            let data = match read_at(&chan.path, off, n) {
                Ok(d) if !d.is_empty() => d,
                _ => break,
            };
            let ev = FollowEvent::Data { offset: off, data: B64.encode(&data) };
            send_line(w, &ev).await?;
            off += data.len() as u64;
            continue;
        }
        if host.closing.load(Ordering::SeqCst) {
            return Ok(());
        }
        if let Some((status, at)) = child.exit() {
            if chan.done.load(Ordering::SeqCst) || at.elapsed() > EXIT_DRAIN {
                if off < chan.len.load(Ordering::SeqCst) {
                    continue;
                }
                send_line(w, &FollowEvent::Exit { status }).await?;
                return Ok(());
            }
        }
        tokio::select! {
            _ = rx.changed() => {}
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
    }
    Ok(())
}

async fn handle(host: Arc<Host>, stream: UnixStream) {
    let (r, mut w) = stream.into_split();
    let mut lines = BufReader::new(r).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        let req: Request = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                let _ = send_line(&mut w, &Response { id: 0, result: None, error: Some(format!("bad request: {e}")) }).await;
                continue;
            }
        };
        if req.method == method::FOLLOW {
            match serde_json::from_value::<ReadParams>(req.params) {
                Ok(p) => {
                    let _ = follow(&host, p, req.id, &mut w).await;
                }
                Err(e) => {
                    let _ = send_line(&mut w, &Response { id: req.id, result: None, error: Some(e.to_string()) }).await;
                }
            }
            return;
        }
        let is_shutdown = req.method == method::SHUTDOWN;
        let resp = match host.call(&req.method, req.params) {
            Ok(v) => Response { id: req.id, result: Some(v), error: None },
            Err(e) => Response { id: req.id, result: None, error: Some(format!("{e:#}")) },
        };
        if send_line(&mut w, &resp).await.is_err() {
            return;
        }
        if is_shutdown {
            tokio::time::sleep(Duration::from_millis(50)).await;
            host.shutdown.notify_one();
        }
    }
}

/// Run the host until it is told to shut down. Fails if another host already
/// answers on the socket.
pub async fn serve(opts: ServeOptions) -> Result<()> {
    if let Some(parent) = opts.socket.parent() {
        ensure_private_dir(parent)?;
    }
    ensure_private_dir(&opts.dir)?;
    if opts.socket.exists() {
        if UnixStream::connect(&opts.socket).await.is_ok() {
            bail!("a host is already running on {}", opts.socket.display());
        }
        std::fs::remove_file(&opts.socket)?;
    }
    let listener = UnixListener::bind(&opts.socket).context("bind host socket")?;
    std::fs::set_permissions(&opts.socket, std::fs::Permissions::from_mode(0o600))?;

    #[cfg(target_os = "linux")]
    if opts.reap_orphans {
        // Orphaned grandchildren come to us, so they stay traceable and get reaped.
        unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) };
    }

    let host = Arc::new(Host::new(opts.dir.clone(), opts.reap_orphans));
    tracing::info!(socket = %opts.socket.display(), "sphere-host listening");

    let reaper = host.clone();
    tokio::spawn(async move {
        let mut sigchld = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::child()).ok();
        loop {
            match sigchld.as_mut() {
                Some(s) => {
                    tokio::select! {
                        _ = s.recv() => {}
                        _ = tokio::time::sleep(Duration::from_millis(500)) => {}
                    }
                }
                None => tokio::time::sleep(Duration::from_millis(200)).await,
            }
            reaper.reap();
        }
    });

    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let my_uid = unsafe { libc::geteuid() };
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else { continue };
                match stream.peer_cred() {
                    Ok(c) if c.uid() == my_uid => {}
                    _ => continue,
                }
                tokio::spawn(handle(host.clone(), stream));
            }
            _ = host.shutdown.notified() => break,
            _ = term.recv() => { host.kill_all(); break }
        }
    }
    let _ = std::fs::remove_file(&opts.socket);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::{FollowItem, HostClient};

    async fn start() -> (tempfile::TempDir, HostClient) {
        let tmp = tempfile::tempdir().unwrap();
        let socket = tmp.path().join("run/host.sock");
        let dir = tmp.path().join("out");
        let opts = ServeOptions { socket: socket.clone(), dir, reap_orphans: false };
        tokio::spawn(async move {
            let _ = serve(opts).await;
        });
        for _ in 0..100 {
            if let Ok(c) = HostClient::connect(&socket).await {
                if c.ping().await.is_ok() {
                    return (tmp, c);
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("host did not start");
    }

    fn sh(script: &str) -> SpawnParams {
        SpawnParams {
            cmd: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
            ..Default::default()
        }
    }

    async fn read_all(c: &HostClient, id: &str, stream: Stream) -> String {
        let mut f = c.follow(id, stream, 0).await.unwrap();
        let mut out = Vec::new();
        while let Some(item) = f.next().await {
            match item.unwrap() {
                FollowItem::Data { bytes, .. } => out.extend(bytes),
                FollowItem::Exit(_) => break,
            }
        }
        String::from_utf8_lossy(&out).to_string()
    }

    #[tokio::test]
    async fn pipes_capture_both_streams_and_the_exit_status() {
        let (_tmp, c) = start().await;
        let s = c.spawn(sh("echo out; echo err >&2; exit 3")).await.unwrap();
        assert_eq!(read_all(&c, &s.id, Stream::Out).await, "out\n");
        assert_eq!(read_all(&c, &s.id, Stream::Err).await, "err\n");
        tokio::time::sleep(Duration::from_millis(200)).await;
        let list = c.list().await.unwrap();
        assert_eq!(list.len(), 1);
        assert!(!list[0].running);
        assert_eq!(list[0].exit, Some(ExitStatus { code: Some(3), signal: None }));
        assert_eq!(list[0].out_len, 4);
        // Reading from an offset picks up where a previous reader stopped.
        let r = c.read(&s.id, Stream::Out, 2, None).await.unwrap();
        assert_eq!(r.data, "t\n");
        assert!(r.eof);
    }

    #[tokio::test]
    async fn stdin_stays_open_until_closed() {
        let (_tmp, c) = start().await;
        let s = c.spawn(sh("while read l; do echo got:$l; done")).await.unwrap();
        c.write(&s.id, b"a\nb\n").await.unwrap();
        c.close_stdin(&s.id).await.unwrap();
        assert_eq!(read_all(&c, &s.id, Stream::Out).await, "got:a\ngot:b\n");
    }

    #[tokio::test]
    async fn each_child_is_its_own_group_and_signals_reach_the_group() {
        let (_tmp, c) = start().await;
        // A grandchild that would outlive its parent.
        let s = c.spawn(sh("sleep 300 & echo $!; wait")).await.unwrap();
        let first = {
            let mut f = c.follow(&s.id, Stream::Out, 0).await.unwrap();
            match f.next().await.unwrap().unwrap() {
                FollowItem::Data { bytes, .. } => String::from_utf8(bytes).unwrap(),
                other => panic!("{other:?}"),
            }
        };
        let grandchild: i32 = first.trim().parse().unwrap();
        assert_ne!(unsafe { libc::getpgid(s.pid as i32) }, unsafe { libc::getpgid(0) });
        assert_eq!(unsafe { libc::getpgid(grandchild) }, s.pid as i32);
        c.signal(&s.id, serde_json::json!("TERM"), true).await.unwrap();
        for _ in 0..100 {
            let info = &c.list().await.unwrap()[0];
            if !info.running && unsafe { libc::kill(grandchild, 0) } != 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("group was not terminated");
    }

    #[tokio::test]
    async fn a_pty_child_gets_a_terminal_of_the_asked_size_and_can_be_resized() {
        let (_tmp, c) = start().await;
        let mut p = sh("stty size; read x; echo got:$x; stty size");
        p.pty = Some(PtySize { cols: 100, rows: 30 });
        let s = c.spawn(p).await.unwrap();
        let mut f = c.follow(&s.id, Stream::Out, 0).await.unwrap();
        let mut text = String::new();
        let mut resized = false;
        let mut sent = false;
        while let Some(item) = f.next().await {
            match item.unwrap() {
                FollowItem::Data { bytes, .. } => {
                    text.push_str(&String::from_utf8_lossy(&bytes));
                    if !resized && text.contains("30 100") {
                        resized = true;
                        c.resize(&s.id, 120, 40).await.unwrap();
                    }
                    if resized && !sent {
                        sent = true;
                        c.write(&s.id, b"abc\n").await.unwrap();
                    }
                }
                FollowItem::Exit(st) => {
                    assert_eq!(st.code, Some(0));
                    break;
                }
            }
        }
        assert!(text.contains("got:abc"), "{text:?}");
        assert!(text.contains("40 120"), "{text:?}");
    }

    #[tokio::test]
    async fn bad_requests_are_errors_not_crashes() {
        let (_tmp, c) = start().await;
        assert!(c.close_stdin("c99").await.is_err());
        assert!(c.spawn(SpawnParams { cmd: "/nonexistent/x".into(), ..Default::default() }).await.is_err());
        assert!(c.resize("c99", 1, 1).await.is_err());
        c.ping().await.unwrap();
    }

    #[tokio::test]
    async fn shutdown_kills_children_and_stops_the_host() {
        let (tmp, c) = start().await;
        let s = c.spawn(sh("sleep 300")).await.unwrap();
        c.shutdown().await.unwrap();
        for _ in 0..100 {
            let gone = unsafe { libc::kill(s.pid as i32, 0) } != 0;
            if gone && !tmp.path().join("run/host.sock").exists() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("child or socket still there");
    }
}
