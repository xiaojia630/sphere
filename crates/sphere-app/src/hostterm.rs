//! Terminal tabs held by sphere-host, so your shells survive the app closing (docs/sphered.md: the host owns every
//! PTY). A tab is a host child labelled `sphere-term`; the app follows its output into the VT state and writes
//! keystrokes in order. On start the app re-attaches the tabs still running, replaying their output.
//!
//! A tab whose shell has the integration that Fix it reads (shellint.rs) is labelled `sphere-term:<nonce>`, the
//! nonce that its shell's marks carry. The host keeps the label and hands it back with the tab when the app starts
//! again, so a re-attached tab is read with the nonce it was started with, and nothing else is kept anywhere.
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

/// The label of a new tab: `LABEL`, then `:` and the nonce of its shell's marks when it has the integration.
fn label_of(nonce: Option<&str>) -> String {
    match nonce {
        Some(n) => format!("{LABEL}:{n}"),
        None => LABEL.to_string(),
    }
}

/// Whether a host child's label is a tab's.
fn is_tab(label: &str) -> bool {
    label == LABEL || label.strip_prefix(LABEL).is_some_and(|rest| rest.starts_with(':'))
}

/// The nonce in a tab's label; none for a tab whose shell has no integration.
fn nonce_of(label: &str) -> Option<&str> {
    label.strip_prefix(LABEL)?.strip_prefix(':').filter(|n| !n.is_empty())
}

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
    running_on(&c)
}

fn running_on(c: &HostClient) -> Vec<ChildInfo> {
    let mut v: Vec<ChildInfo> = rt().block_on(c.list()).unwrap_or_default().into_iter().filter(|ch| ch.label.as_deref().is_some_and(is_tab) && ch.running && ch.pty).collect();
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
    /// The nonce that the tab's shell prints in its marks; none when the shell has no integration.
    pub nonce: Option<String>,
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
fn wire(c: HostClient, id: String, from: u64, nonce: Option<String>) -> (Handle, mpsc::Receiver<Vec<u8>>, HostWriter) {
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
    (Handle { id, client: c, nonce }, out_rx, HostWriter { tx: w_tx })
}

/// A new login shell in the host.
pub fn spawn(cols: u16, rows: u16, shell: &str) -> Option<(Handle, mpsc::Receiver<Vec<u8>>, HostWriter)> {
    let c = client()?;
    // zsh and bash get the shell integration that Fix it reads; any other shell is a plain login shell
    spawn_on(c, cols, rows, shell, crate::shellint::launch(shell), std::env::var("HOME").ok())
}

/// `spawn` on the host `c`, with the shell started the way `launch` says and in `cwd`.
fn spawn_on(c: HostClient, cols: u16, rows: u16, shell: &str, launch: crate::shellint::Launch, cwd: Option<String>) -> Option<(Handle, mpsc::Receiver<Vec<u8>>, HostWriter)> {
    let mut env = BTreeMap::new();
    env.insert("TERM".to_string(), "xterm-256color".to_string());
    env.insert("COLORTERM".to_string(), "truecolor".to_string());
    env.extend(launch.env);
    let p = SpawnParams { cmd: shell.to_string(), args: launch.args, cwd, env, pty: Some(PtySize { cols, rows }), label: Some(label_of(launch.nonce.as_deref())) };
    let r = rt().block_on(c.spawn(p)).ok()?;
    Some(wire(c, r.id, 0, launch.nonce))
}

/// How many bytes of its old output `attach` has the host replay for this tab.
pub fn replay_len(ch: &ChildInfo) -> u64 {
    ch.out_len.min(REPLAY_MAX)
}

/// Re-attach a tab from a previous run, replaying (the tail of) its output. The handle has the nonce that the
/// tab was started with.
pub fn attach(ch: &ChildInfo) -> Option<(Handle, mpsc::Receiver<Vec<u8>>, HostWriter)> {
    Some(attach_on(client()?, ch))
}

fn attach_on(c: HostClient, ch: &ChildInfo) -> (Handle, mpsc::Receiver<Vec<u8>>, HostWriter) {
    let from = ch.out_len - replay_len(ch);
    wire(c, ch.id.clone(), from, ch.label.as_deref().and_then(nonce_of).map(str::to_string))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::{Event, Finished, Scanner};
    use crate::shellint::Launch;
    use std::time::Instant;

    const NONCE: &str = "d41d8cd98f00b204e9800998ecf8427e";

    /// A scratch directory that goes away again.
    struct Scratch(PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A host in this process on a scratch socket (the path stays short: a socket's can't be long), and a client
    /// of it. `shutdown` ends it and the tabs in it.
    fn host(name: &str) -> (Scratch, HostClient) {
        let dir = std::env::temp_dir().join(format!("sphere-ht-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let scratch = Scratch(dir.clone());
        let opts = sphere_host::ServeOptions { socket: dir.join("h.sock"), dir: dir.join("out"), reap_orphans: false };
        rt().spawn(async move {
            let _ = sphere_host::serve(opts).await;
        });
        for _ in 0..200 {
            if let Ok(c) = rt().block_on(HostClient::connect(&dir.join("h.sock"))) {
                if rt().block_on(c.ping()).is_ok() {
                    return (scratch, c);
                }
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("the host did not start");
    }

    fn shutdown(c: &HostClient) {
        let _ = rt().block_on(c.shutdown());
    }

    fn on_path(prog: &str) -> Option<PathBuf> {
        std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(prog)).find(|f| f.is_file())
    }

    #[test]
    fn a_tab_is_labelled_with_its_nonce_and_the_label_gives_it_back() {
        assert_eq!(label_of(Some(NONCE)), format!("sphere-term:{NONCE}"));
        assert_eq!(label_of(None), "sphere-term");
        for (label, tab, nonce) in [
            (format!("sphere-term:{NONCE}"), true, Some(NONCE)),
            ("sphere-term".to_string(), true, None),
            ("sphere-term:".to_string(), true, None),
            ("task:t1".to_string(), false, None),
            ("sphere-terminal".to_string(), false, None),
            ("sphere-term-2:abc".to_string(), false, None),
            (String::new(), false, None),
        ] {
            assert_eq!((is_tab(&label), nonce_of(&label)), (tab, nonce), "{label:?}");
        }
    }

    #[test]
    fn a_re_attached_tab_has_the_nonce_it_was_started_with() {
        let (_dir, c) = host("nonce");
        let sleep = |nonce: Option<&str>| Launch { args: vec!["-c".into(), "sleep 30".into()], env: Vec::new(), nonce: nonce.map(String::from) };
        // two tabs with the integration, each with a nonce of its own, and one without
        let tabs: Vec<_> = [Some("1111aaaa"), Some("2222bbbb"), None].into_iter().map(|n| spawn_on(c.clone(), 80, 24, "/bin/sh", sleep(n), None).unwrap()).collect();
        let started: Vec<_> = tabs.iter().map(|t| (t.0.id.clone(), t.0.nonce.clone())).collect();
        // and a child of the host's that is no tab
        let other = SpawnParams { cmd: "/bin/sh".into(), args: vec!["-c".into(), "sleep 30".into()], pty: Some(PtySize { cols: 80, rows: 24 }), label: Some("task:t1".into()), ..Default::default() };
        rt().block_on(c.spawn(other)).unwrap();
        // the app goes away, the host and its tabs stay
        drop(tabs);
        let found = running_on(&c);
        assert_eq!(found.iter().map(|ch| ch.id.clone()).collect::<Vec<_>>(), started.iter().map(|t| t.0.clone()).collect::<Vec<_>>());
        // the app comes back and attaches: each tab has the nonce it was started with
        let attached: Vec<_> = found.iter().map(|ch| (attach_on(c.clone(), ch).0.nonce, ch.label.clone())).collect();
        assert_eq!(attached.iter().map(|a| a.0.clone()).collect::<Vec<_>>(), started.iter().map(|t| t.1.clone()).collect::<Vec<_>>());
        assert_eq!(attached[0].1.as_deref(), Some("sphere-term:1111aaaa"));
        assert_eq!(attached[2].1.as_deref(), Some("sphere-term"));
        shutdown(&c);
    }

    /// What the app reads of a tab: its output, through a scanner that knows the nonce.
    struct Reader {
        rx: mpsc::Receiver<Vec<u8>>,
        scanner: Scanner,
        raw: Vec<u8>,
        done: Vec<Finished>,
    }

    impl Reader {
        fn new(rx: mpsc::Receiver<Vec<u8>>, nonce: Option<&str>) -> Self {
            Reader { rx, scanner: Scanner::new(nonce), raw: Vec::new(), done: Vec::new() }
        }

        fn read_until(&mut self, what: &str, done: impl Fn(&Reader) -> bool) {
            let deadline = Instant::now() + Duration::from_secs(20);
            while !done(self) {
                assert!(Instant::now() < deadline, "no {what} in 20 s; the tab printed {:?}", String::from_utf8_lossy(&self.raw));
                if let Ok(chunk) = self.rx.recv_timeout(Duration::from_millis(100)) {
                    for e in self.scanner.feed(&chunk, Instant::now()) {
                        if let Event::Finished(f) = e {
                            self.done.push(f);
                        }
                    }
                    self.raw.extend_from_slice(&chunk);
                }
            }
        }

        fn prompts(&self) -> usize {
            let mark = format!("\x1b]133;A;sphere={NONCE}\x07");
            self.raw.windows(mark.len()).filter(|w| *w == mark.as_bytes()).count()
        }

        /// Types a line and waits for the next prompt.
        fn run(&mut self, w: &mut HostWriter, line: &str) {
            let n = self.prompts() + 1;
            w.write_all(format!("{line}\n").as_bytes()).unwrap();
            self.read_until(&format!("prompt number {n}"), |r| r.prompts() >= n);
        }

        fn commands(&self) -> Vec<(&str, i32)> {
            self.done.iter().map(|f| (f.command.as_str(), f.exit)).collect()
        }
    }

    #[test]
    fn a_tab_found_again_after_an_app_restart_still_reports_its_commands() {
        let Some(bash) = on_path("bash") else { return eprintln!("skipped: no bash on PATH") };
        let (dir, c) = host("restart");
        let (scripts, home) = (dir.0.join("shell"), dir.0.join("home"));
        std::fs::create_dir_all(&home).unwrap();
        crate::shellint::install(&scripts).unwrap();
        let mut launch = crate::shellint::launch_in(&scripts, bash.to_str().unwrap(), None, None, NONCE);
        launch.env.push(("HOME".into(), home.to_string_lossy().into_owned()));
        let (h, rx, mut w) = spawn_on(c.clone(), 100, 30, bash.to_str().unwrap(), launch, Some(home.to_string_lossy().into_owned())).unwrap();
        assert_eq!(h.nonce.as_deref(), Some(NONCE));
        let mut tab = Reader::new(rx, h.nonce.as_deref());
        tab.read_until("first prompt", |r| r.prompts() >= 1);
        for line in ["false", "sh -c 'exit 3'"] {
            tab.run(&mut w, line);
        }
        assert_eq!(tab.commands(), [("false", 1), ("sh -c 'exit 3'", 3)]);

        // the app quits and starts again: it finds the tab in the host, with the nonce of its shell
        drop((h, w, tab));
        let found = running_on(&c);
        assert_eq!(found.len(), 1);
        let (h, rx, mut w) = attach_on(c.clone(), &found[0]);
        assert_eq!(h.nonce.as_deref(), Some(NONCE));
        let mut tab = Reader::new(rx, h.nonce.as_deref());
        // what the tab printed before is replayed, and that is read with the nonce too
        let replayed = found[0].out_len;
        tab.read_until("the replay", |r| r.raw.len() as u64 >= replayed);
        assert_eq!(tab.commands(), [("false", 1), ("sh -c 'exit 3'", 3)]);
        // and a command that fails now is reported as it was before the restart
        tab.run(&mut w, "sh -c 'exit 7'");
        assert_eq!(tab.commands().last(), Some(&("sh -c 'exit 7'", 7)));
        // a scanner that does not have the nonce reads none of it
        let mut stranger = Scanner::new(Some("not-the-nonce"));
        assert!(stranger.feed(&tab.raw, Instant::now()).is_empty());
        shutdown(&c);
    }
}
