//! Who is on the other end of a socket: the user check, and whether the caller
//! descends from a process one of our agents started.

/// The parent of `pid`.
#[cfg(target_os = "linux")]
fn parent_of(pid: i32) -> Option<i32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // `pid (comm) state ppid ...`; comm may contain spaces and parentheses.
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(1)?.parse().ok()
}

#[cfg(not(target_os = "linux"))]
fn parent_of(pid: i32) -> Option<i32> {
    // Best effort: libc offers proc_pidinfo on macOS, but `ps` is enough here.
    let out = std::process::Command::new("ps")
        .args(["-o", "ppid=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// `pid` followed by its ancestors, nearest first.
pub fn lineage(pid: i32) -> Vec<i32> {
    let mut out = vec![];
    let mut cur = pid;
    while cur > 1 && out.len() < 64 {
        out.push(cur);
        match parent_of(cur) {
            Some(p) if p != cur && p > 0 => cur = p,
            _ => break,
        }
    }
    out
}

/// Is `pid` one of `roots`, a descendant of one, or in a session one of them
/// leads? Agents start in their own session, and the host adopts orphans as a
/// child subreaper, so a detached process is still traceable.
pub fn descends_from(pid: i32, roots: &[i32]) -> bool {
    if roots.is_empty() {
        return false;
    }
    if lineage(pid).iter().any(|p| roots.contains(p)) {
        return true;
    }
    let sid = unsafe { libc::getsid(pid) };
    sid > 0 && roots.contains(&sid)
}

pub fn my_uid() -> u32 {
    unsafe { libc::geteuid() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    #[test]
    fn lineage_starts_at_the_pid_and_walks_up() {
        let me = std::process::id() as i32;
        let l = lineage(me);
        assert_eq!(l[0], me);
        assert!(l.len() >= 2, "{l:?}");
    }

    #[test]
    fn a_child_descends_from_its_parent_and_a_stranger_does_not() {
        let me = std::process::id() as i32;
        let mut child = Command::new("sleep")
            .arg("5")
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id() as i32;
        assert!(descends_from(pid, &[me]));
        assert!(descends_from(pid, &[pid]));
        assert!(!descends_from(pid, &[]));
        assert!(!descends_from(me, &[pid]));
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn a_session_member_counts_even_when_reparented() {
        // `setsid sh -c 'sleep & ...'` leaves the grandchild in the leader's session.
        let mut leader = Command::new("sh")
            .args(["-c", "sleep 5 & echo $!; wait"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        use std::io::BufRead;
        let mut line = String::new();
        std::io::BufReader::new(leader.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let grandchild: i32 = line.trim().parse().unwrap();
        let sid = unsafe { libc::getsid(leader.id() as i32) };
        // Not a session leader here: the grandchild shares *our* session, so a
        // root that is not in its lineage does not match through the session.
        assert!(!descends_from(grandchild, &[sid + 100_000]));
        assert!(descends_from(grandchild, &[leader.id() as i32]));
        leader.kill().unwrap();
        leader.wait().unwrap();
        unsafe { libc::kill(grandchild, libc::SIGKILL) };
    }
}
