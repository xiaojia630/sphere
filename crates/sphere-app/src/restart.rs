//! sphered outlives the app, so after an update the app can find an older sphered still running. When its version
//! differs from the app's, sphered is restarted from the binary next to the app, but only while no agent is running,
//! and only once per mismatch. sphere-host is never restarted: it holds the terminal shells and the agents, and its
//! shutdown kills every child.
use std::path::Path;
use std::time::Duration;

/// What to do about the running sphered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    /// stop it and start the bundled one
    Restart,
    /// "Updated background service"
    SayUpdated,
    /// "out of date: restarts when your agents finish"
    SayWaiting,
    /// a restart didn't help: say so and stop trying
    SayGaveUp,
}

/// What the app has done about a version mismatch, across reconnects.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Upgrade {
    /// restarts tried
    pub attempts: u32,
    told_waiting: bool,
    done: bool,
}

/// The decision, pure: the versions, how many agents are running (queued, working or needing you) and what was
/// already done. `None` when there is nothing to do.
pub fn decide(app: &str, daemon: &str, running: usize, u: &Upgrade) -> Option<Act> {
    if u.done {
        return None;
    }
    if app == daemon {
        return (u.attempts > 0).then_some(Act::SayUpdated);
    }
    if u.attempts > 0 {
        return Some(Act::SayGaveUp);
    }
    if running > 0 {
        return (!u.told_waiting).then_some(Act::SayWaiting);
    }
    Some(Act::Restart)
}

impl Upgrade {
    /// Decides and records it, so each toast shows once and a restart is tried once.
    pub fn step(&mut self, app: &str, daemon: &str, running: usize) -> Option<Act> {
        let a = decide(app, daemon, running, self)?;
        match a {
            Act::Restart => self.attempts += 1,
            Act::SayWaiting => self.told_waiting = true,
            Act::SayUpdated | Act::SayGaveUp => self.done = true,
        }
        Some(a)
    }

    /// A restart failed outright (already said): leave it at that.
    pub fn give_up(&mut self) { self.done = true; }
}

/// The toast for an act (none for the restart itself).
pub fn words(a: Act, app: &str, daemon: &str) -> Option<String> {
    match a {
        Act::Restart => None,
        Act::SayUpdated => Some("Updated background service".into()),
        Act::SayWaiting => Some("Background service is out of date: restarts when your agents finish".into()),
        Act::SayGaveUp => Some(format!("Background service is still out of date (sphered {daemon}, app {app})")),
    }
}

/// The process side of a restart, behind a trait so tests never touch a real process.
pub trait Procs {
    /// the pid of the process serving `socket`
    fn server_pid(&self, socket: &Path) -> Option<u32>;
    /// its executable's file name
    fn name(&self, pid: u32) -> Option<String>;
    fn terminate(&self, pid: u32) -> bool;
    fn alive(&self, pid: u32) -> bool;
}

/// Stops the sphered serving `socket` (SIGTERM, which it handles: it stops serving and leaves sphere-host alone) and
/// waits up to `wait` for it to go. Refuses anything not named `sphered`. The caller's reconnect then starts the
/// bundled one.
pub fn stop_sphered(p: &dyn Procs, socket: &Path, wait: Duration) -> Result<(), String> {
    let pid = p.server_pid(socket).ok_or("could not tell which process serves the sphered socket")?;
    let name = p.name(pid).unwrap_or_default();
    if name != "sphered" {
        return Err(format!("the sphered socket is served by {name:?} (pid {pid}), not sphered"));
    }
    if !p.terminate(pid) {
        return Err(format!("could not stop sphered (pid {pid})"));
    }
    let t0 = std::time::Instant::now();
    while p.alive(pid) {
        if t0.elapsed() >= wait {
            return Err(format!("sphered (pid {pid}) did not stop"));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

/// The real processes.
pub struct Real;

impl Procs for Real {
    fn server_pid(&self, socket: &Path) -> Option<u32> {
        use std::os::fd::AsRawFd;
        let s = std::os::unix::net::UnixStream::connect(socket).ok()?;
        peer_pid(s.as_raw_fd())
    }
    fn name(&self, pid: u32) -> Option<String> {
        let out = std::process::Command::new("ps").args(["-p", &pid.to_string(), "-o", "comm="]).output().ok()?;
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        Path::new(&s).file_name().map(|n| n.to_string_lossy().to_string())
    }
    fn terminate(&self, pid: u32) -> bool {
        // SAFETY: kill(2) with a pid we checked is sphered's; no memory is involved.
        pid > 1 && unsafe { libc::kill(pid as i32, libc::SIGTERM) } == 0
    }
    fn alive(&self, pid: u32) -> bool {
        // SAFETY: signal 0 only checks that the process exists.
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }
}

#[cfg(target_os = "macos")]
fn peer_pid(fd: i32) -> Option<u32> {
    let mut pid: libc::pid_t = 0;
    let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    // SAFETY: LOCAL_PEERPID writes one pid_t into `pid`, whose size is passed in `len`.
    let r = unsafe { libc::getsockopt(fd, libc::SOL_LOCAL, libc::LOCAL_PEERPID, &mut pid as *mut _ as *mut libc::c_void, &mut len) };
    (r == 0 && pid > 0).then_some(pid as u32)
}

#[cfg(target_os = "linux")]
fn peer_pid(fd: i32) -> Option<u32> {
    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: SO_PEERCRED writes one ucred into `cred`, whose size is passed in `len`.
    let r = unsafe { libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_PEERCRED, &mut cred as *mut _ as *mut libc::c_void, &mut len) };
    (r == 0 && cred.pid > 0).then_some(cred.pid as u32)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn peer_pid(_fd: i32) -> Option<u32> { None }

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    const APP: &str = "0.2.0";

    #[test]
    fn the_same_version_does_nothing() {
        let mut u = Upgrade::default();
        assert_eq!(u.step(APP, APP, 0), None);
        assert_eq!(u.step(APP, APP, 3), None);
        assert_eq!(u, Upgrade::default());
    }

    #[test]
    fn an_old_sphered_with_no_agents_restarts_once_and_says_so() {
        let mut u = Upgrade::default();
        assert_eq!(u.step(APP, "0.1.0", 0), Some(Act::Restart));
        // the new one answers with the app's version
        assert_eq!(u.step(APP, APP, 0), Some(Act::SayUpdated));
        assert_eq!(u.step(APP, APP, 0), None);
        assert_eq!(u.attempts, 1);
    }

    #[test]
    fn running_agents_hold_the_restart_and_the_toast_shows_once() {
        let mut u = Upgrade::default();
        assert_eq!(u.step(APP, "0.1.0", 2), Some(Act::SayWaiting));
        assert_eq!(u.step(APP, "0.1.0", 1), None);
        assert_eq!(u.step(APP, "0.1.0", 1), None);
        // all finished
        assert_eq!(u.step(APP, "0.1.0", 0), Some(Act::Restart));
        assert_eq!(u.step(APP, APP, 0), Some(Act::SayUpdated));
    }

    #[test]
    fn a_restart_that_does_not_help_is_not_tried_again() {
        let mut u = Upgrade::default();
        assert_eq!(u.step(APP, "0.1.0", 0), Some(Act::Restart));
        // still old (say the bundled sphered wasn't found and PATH has the old one)
        assert_eq!(u.step(APP, "0.1.0", 0), Some(Act::SayGaveUp));
        for _ in 0..5 {
            assert_eq!(u.step(APP, "0.1.0", 0), None);
        }
        assert_eq!(u.attempts, 1);
        assert!(words(Act::SayGaveUp, APP, "0.1.0").unwrap().contains("0.1.0"));
        assert_eq!(words(Act::Restart, APP, "0.1.0"), None);
    }

    #[test]
    fn a_newer_sphered_than_the_app_counts_as_a_mismatch_too() {
        assert_eq!(decide(APP, "0.3.0", 0, &Upgrade::default()), Some(Act::Restart));
    }

    /// A process table in memory.
    struct Fake {
        pid: Option<u32>,
        name: &'static str,
        dies: bool,
        killed: RefCell<Vec<u32>>,
    }

    impl Procs for Fake {
        fn server_pid(&self, _: &Path) -> Option<u32> { self.pid }
        fn name(&self, _: u32) -> Option<String> { Some(self.name.into()) }
        fn terminate(&self, pid: u32) -> bool {
            self.killed.borrow_mut().push(pid);
            true
        }
        fn alive(&self, pid: u32) -> bool { !(self.dies && self.killed.borrow().contains(&pid)) }
    }

    fn fake(pid: Option<u32>, name: &'static str, dies: bool) -> Fake { Fake { pid, name, dies, killed: RefCell::new(vec![]) } }

    #[test]
    fn stopping_terminates_the_sphered_serving_the_socket() {
        let f = fake(Some(4242), "sphered", true);
        assert_eq!(stop_sphered(&f, Path::new("/x/sphered.sock"), Duration::from_secs(1)), Ok(()));
        assert_eq!(*f.killed.borrow(), vec![4242]);
    }

    #[test]
    fn stopping_refuses_anything_that_is_not_sphered() {
        let f = fake(Some(4242), "sphere-host", true);
        assert!(stop_sphered(&f, Path::new("/x"), Duration::from_secs(1)).is_err());
        assert!(f.killed.borrow().is_empty());
        let f = fake(None, "sphered", true);
        assert!(stop_sphered(&f, Path::new("/x"), Duration::from_secs(1)).is_err());
        assert!(f.killed.borrow().is_empty());
    }

    #[test]
    fn the_server_pid_comes_from_the_socket_and_a_non_sphered_server_is_left_alone() {
        // the test serves the socket itself: the pid found is its own, and its name isn't sphered, so nothing is sent
        let dir = std::env::temp_dir().join(format!("sphere-restart-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let sock = dir.join("s.sock");
        let _ = std::fs::remove_file(&sock);
        let _l = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        assert_eq!(Real.server_pid(&sock), Some(std::process::id()));
        assert!(Real.alive(std::process::id()));
        assert!(stop_sphered(&Real, &sock, Duration::from_millis(10)).unwrap_err().contains("not sphered"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stopping_gives_up_when_sphered_hangs_on() {
        let f = fake(Some(7), "sphered", false);
        assert!(stop_sphered(&f, Path::new("/x"), Duration::from_millis(120)).unwrap_err().contains("did not stop"));
    }
}
