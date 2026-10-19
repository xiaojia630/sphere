//! The PATH sphered and its agents run with. Started by the app from Finder or the Dock, sphered gets macOS's
//! bare PATH (`/usr/bin:/bin:/usr/sbin:/sbin`): no `claude` (it lives in `~/.local/bin`), and the agents could not
//! find cargo, npm or brew either. So at start sphered takes the PATH from the user's login shell, keeps its own,
//! and adds the usual install folders, before anything else runs.

use std::path::Path;
use std::time::{Duration, Instant};

/// Marks the shell's answer, so whatever its startup files print around it is ignored.
const MARK: &str = "__SPHERE_PATH__";

/// Where Claude Code and the usual tools get installed, after the login shell's PATH and our own.
fn fallbacks(home: &Path) -> Vec<String> {
    let h = |p: &str| home.join(p).to_string_lossy().to_string();
    vec![
        h(".local/bin"),
        h(".claude/local"),
        "/opt/homebrew/bin".into(),
        "/opt/homebrew/sbin".into(),
        "/usr/local/bin".into(),
        h(".cargo/bin"),
        h(".bun/bin"),
        h(".npm-global/bin"),
    ]
}

/// The login shell's PATH out of its output (the line after the mark).
pub fn parse_shell_output(out: &str) -> Option<String> {
    let i = out.rfind(MARK)?;
    let p = out[i + MARK.len()..].lines().next()?.trim();
    (!p.is_empty()).then(|| p.to_string())
}

/// `login` first (what the user's terminal has), then `current`, then the install folders that exist; each
/// directory once, in that order.
pub fn merged(
    current: &str,
    login: Option<&str>,
    home: &Path,
    exists: impl Fn(&str) -> bool,
) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut add = |d: &str| {
        if !d.is_empty() && !out.iter().any(|o| o == d) {
            out.push(d.to_string());
        }
    };
    for d in login.unwrap_or("").split(':') {
        add(d);
    }
    for d in current.split(':') {
        add(d);
    }
    for d in fallbacks(home) {
        if exists(&d) {
            add(&d);
        }
    }
    out.join(":")
}

/// Asks the user's login shell for its PATH (interactive too, since many set it in .zshrc), giving up after
/// `timeout` so a shell waiting on a prompt can't hold sphered up.
pub fn login_shell_path(timeout: Duration) -> Option<String> {
    let shell = std::env::var("SHELL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/bin/zsh".into());
    let mut child = std::process::Command::new(&shell)
        .args(["-ilc", &format!("printf '\\n{MARK}%s\\n' \"$PATH\"")])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut out = String::new();
    std::io::Read::read_to_string(&mut child.stdout.take()?, &mut out).ok()?;
    parse_shell_output(&out)
}

/// A PATH of nothing but the system folders: what launchd gives an app opened from Finder or the Dock. Started
/// from a terminal (the CLI, the tests) sphered already has the user's PATH and skips asking the shell.
pub fn is_bare(path: &str) -> bool {
    path.split(':')
        .filter(|d| !d.is_empty())
        .all(|d| matches!(d, "/usr/bin" | "/bin" | "/usr/sbin" | "/sbin"))
}

/// Sets this process's PATH to the merged one. Call it first thing in `main`, before any thread starts.
pub fn fix_path() -> String {
    let current = std::env::var("PATH").unwrap_or_default();
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "/".into());
    let login = if is_bare(&current) {
        login_shell_path(Duration::from_secs(3))
    } else {
        None
    };
    let path = merged(&current, login.as_deref(), &home, |d| Path::new(d).is_dir());
    std::env::set_var("PATH", &path);
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_login_shell_answer_is_found_among_its_noise() {
        let out = format!("Last login: today\nwelcome!\n\n{MARK}/Users/me/.local/bin:/opt/homebrew/bin:/usr/bin\n");
        assert_eq!(
            parse_shell_output(&out).as_deref(),
            Some("/Users/me/.local/bin:/opt/homebrew/bin:/usr/bin")
        );
        assert_eq!(parse_shell_output("no mark here"), None);
        assert_eq!(parse_shell_output(&format!("{MARK}\n")), None);
    }

    #[test]
    fn a_finder_launch_gets_the_users_tools_back() {
        let home = Path::new("/Users/me");
        let bare = "/usr/bin:/bin:/usr/sbin:/sbin";
        // the login shell's PATH comes first, ours after, no duplicates
        let p = merged(
            bare,
            Some("/Users/me/.cargo/bin:/usr/bin:/opt/homebrew/bin"),
            home,
            |_| false,
        );
        assert_eq!(
            p,
            "/Users/me/.cargo/bin:/usr/bin:/opt/homebrew/bin:/bin:/usr/sbin:/sbin"
        );
        // no answer from the shell: the install folders that exist still make claude findable
        let p = merged(bare, None, home, |d| {
            d == "/Users/me/.local/bin" || d == "/opt/homebrew/bin"
        });
        assert_eq!(
            p,
            "/usr/bin:/bin:/usr/sbin:/sbin:/Users/me/.local/bin:/opt/homebrew/bin"
        );
    }

    #[test]
    fn only_a_bare_path_asks_the_login_shell() {
        assert!(is_bare("/usr/bin:/bin:/usr/sbin:/sbin"));
        assert!(is_bare(""));
        assert!(!is_bare("/Users/me/.local/bin:/usr/bin:/bin"));
        assert!(!is_bare("/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin"));
    }

    #[test]
    fn a_shell_that_never_answers_is_given_up_on() {
        // a real login shell answers with a PATH, or isn't there; either way this returns quickly
        let t = Instant::now();
        let _ = login_shell_path(Duration::from_secs(3));
        assert!(t.elapsed() < Duration::from_secs(5));
    }
}
