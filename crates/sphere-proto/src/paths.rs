//! Where Sphere keeps things. Everything is overridable by config; these are
//! only the defaults.

use std::path::PathBuf;

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

/// `~/.sphere`
pub fn default_state_dir() -> PathBuf {
    home().join(".sphere")
}

/// Directory for sockets. Linux prefers `$XDG_RUNTIME_DIR/sphere`; everything
/// else (and Linux without it) uses `<state dir>/run`.
pub fn default_run_dir() -> PathBuf {
    #[cfg(target_os = "linux")]
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir).join("sphere");
        }
    }
    default_state_dir().join("run")
}

pub fn default_socket() -> PathBuf {
    default_run_dir().join("sphered.sock")
}

pub fn default_host_socket() -> PathBuf {
    default_run_dir().join("sphere-host.sock")
}

/// `~/.claude/projects`, where Claude Code keeps session transcripts.
pub fn default_claude_projects_dir() -> PathBuf {
    home().join(".claude").join("projects")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sockets_live_in_a_private_looking_dir() {
        let s = default_socket();
        assert!(s.ends_with("sphered.sock"));
        assert!(default_host_socket().ends_with("sphere-host.sock"));
    }
}
