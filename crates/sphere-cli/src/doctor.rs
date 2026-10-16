//! `sphere doctor`: is everything Sphere needs in place?

use sphere_client::Client;
use sphere_proto::version;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, PartialEq, Eq)]
pub enum Level {
    Ok,
    Warn,
    Fail,
}

pub struct Check {
    pub level: Level,
    pub name: &'static str,
    pub detail: String,
}

fn check(level: Level, name: &'static str, detail: impl Into<String>) -> Check {
    Check { level, name, detail: detail.into() }
}

/// Find `name` on `PATH` (or use it as a path).
pub fn which(name: &str) -> Option<PathBuf> {
    let p = Path::new(name);
    if p.components().count() > 1 {
        return p.exists().then(|| p.to_path_buf());
    }
    std::env::var_os("PATH")?
        .to_string_lossy()
        .split(':')
        .map(|d| Path::new(d).join(name))
        .find(|c| c.is_file())
}

fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).output().ok()?;
    out.status.success().then(|| format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

fn claude_checks(agent: &str) -> Vec<Check> {
    let mut out = vec![];
    let Some(path) = which(agent) else {
        out.push(check(Level::Fail, "claude", format!("`{agent}` not found on PATH; install Claude Code")));
        return out;
    };
    match run(&path.to_string_lossy(), &["--version"]) {
        Some(text) => match version::parse(&text) {
            Some(v) if version::in_tested_range(v) => {
                out.push(check(Level::Ok, "claude", format!("{} at {}", text.trim(), path.display())))
            }
            Some(v) => out.push(check(Level::Warn, "claude", version::out_of_range_message(v))),
            None => out.push(check(Level::Warn, "claude", format!("could not read a version from `{}`", text.trim()))),
        },
        None => out.push(check(Level::Fail, "claude", format!("`{} --version` failed", path.display()))),
    }
    match run(&path.to_string_lossy(), &["auth", "status"]) {
        Some(text) => {
            let signed = serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|v| v.get("loggedIn").and_then(|l| l.as_bool()));
            match signed {
                Some(true) => out.push(check(Level::Ok, "sign-in", "signed in")),
                Some(false) => out.push(check(Level::Warn, "sign-in", "signed out; run `claude auth login`")),
                None => out.push(check(Level::Warn, "sign-in", "could not tell from `claude auth status`")),
            }
        }
        None => out.push(check(Level::Warn, "sign-in", "`claude auth status` failed; run `claude auth login` if needed")),
    }
    out
}

#[cfg(target_os = "linux")]
fn sandbox_checks() -> Vec<Check> {
    let mut out = vec![];
    let bwrap = which("bwrap");
    out.push(match &bwrap {
        Some(p) => check(Level::Ok, "bubblewrap", p.display().to_string()),
        None => check(Level::Warn, "bubblewrap", "not installed; Claude Code's sandbox needs `bwrap` (apt install bubblewrap)"),
    });
    out.push(match which("socat") {
        Some(p) => check(Level::Ok, "socat", p.display().to_string()),
        None => check(Level::Warn, "socat", "not installed; the sandbox's network proxy needs it (apt install socat)"),
    });
    // The sandbox (and its seccomp helper, which ships inside Claude Code)
    // runs in unprivileged user namespaces.
    if let Some(bwrap) = bwrap {
        let ok = Command::new(bwrap)
            .args(["--unshare-user", "--ro-bind", "/", "/", "true"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        out.push(if ok {
            check(Level::Ok, "user namespaces", "the sandbox can start")
        } else {
            check(
                Level::Warn,
                "user namespaces",
                "bubblewrap cannot create a sandbox here (unprivileged user namespaces may be disabled)",
            )
        });
    }
    out
}

#[cfg(not(target_os = "linux"))]
fn sandbox_checks() -> Vec<Check> {
    vec![match which("/usr/bin/sandbox-exec") {
        Some(_) => check(Level::Ok, "sandbox", "Seatbelt is built in"),
        None => check(Level::Warn, "sandbox", "sandbox-exec not found"),
    }]
}

fn state_dir_check(dir: &Path) -> Check {
    let probe = dir.join(".sphere-doctor");
    let result = std::fs::create_dir_all(dir).and_then(|_| std::fs::write(&probe, b"ok"));
    let _ = std::fs::remove_file(&probe);
    match result {
        Ok(()) => check(Level::Ok, "state dir", format!("{} is writable", dir.display())),
        Err(e) => check(Level::Fail, "state dir", format!("{} is not writable: {e}", dir.display())),
    }
}

pub async fn run_all(agent: &str, state_dir: &Path, socket: &Path) -> Vec<Check> {
    let mut out = claude_checks(agent);
    out.extend(sandbox_checks());
    out.push(match which("git") {
        Some(_) => check(Level::Ok, "git", run("git", &["--version"]).unwrap_or_default().trim().to_string()),
        None => check(Level::Fail, "git", "not found; worktrees need git"),
    });
    out.push(state_dir_check(state_dir));
    match Client::connect(socket).await {
        Ok(c) => match c.status().await {
            Ok(s) => {
                out.push(check(
                    Level::Ok,
                    "sphered",
                    format!("answering on {} (v{}, {} active, {} queued)", socket.display(), s.version, s.active, s.queued),
                ));
                out.push(match s.host_pid {
                    Some(pid) => check(Level::Ok, "sphere-host", format!("running, pid {pid}")),
                    None => check(Level::Fail, "sphere-host", "sphered cannot reach the host"),
                });
                for w in s.warnings {
                    out.push(check(Level::Warn, "sphered", w));
                }
            }
            Err(e) => out.push(check(Level::Fail, "sphered", format!("connected but status failed: {e}"))),
        },
        Err(e) => out.push(check(
            Level::Fail,
            "sphered",
            format!("not reachable at {} ({e}); start it with `sphere daemon`", socket.display()),
        )),
    }
    out
}

pub fn render(checks: &[Check]) -> String {
    checks
        .iter()
        .map(|c| {
            let tag = match c.level {
                Level::Ok => "ok  ",
                Level::Warn => "warn",
                Level::Fail => "FAIL",
            };
            format!("{tag} {:<16} {}", c.name, c.detail)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn which_finds_things_on_path_and_by_path() {
        assert!(which("sh").is_some());
        assert!(which("definitely-not-a-command-xyz").is_none());
        assert!(which("/bin/sh").is_some());
        assert!(which("/nonexistent/sh").is_none());
    }

    #[test]
    fn a_missing_claude_is_a_failure() {
        let c = claude_checks("definitely-not-claude-xyz");
        assert_eq!(c[0].level, Level::Fail);
    }

    #[test]
    fn the_state_dir_check_writes_and_cleans_up() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("state");
        assert_eq!(state_dir_check(&dir).level, Level::Ok);
        assert!(std::fs::read_dir(&dir).unwrap().next().is_none());
        std::fs::write(tmp.path().join("file"), "x").unwrap();
        assert_eq!(state_dir_check(&tmp.path().join("file/sub")).level, Level::Fail);
    }

    #[test]
    fn rendering_tags_each_level() {
        let text = render(&[check(Level::Ok, "a", "fine"), check(Level::Warn, "b", "hm"), check(Level::Fail, "c", "bad")]);
        assert!(text.lines().next().unwrap().starts_with("ok   a"));
        assert!(text.contains("warn b") && text.contains("FAIL c"));
    }
}
