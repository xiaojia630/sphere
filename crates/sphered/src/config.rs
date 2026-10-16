//! sphered's configuration: a TOML file in the state dir, overridden by flags.
//! Nothing here is read from the environment, so tests point the service at
//! the fake agent through the config alone.

use anyhow::{Context, Result};
use serde::Deserialize;
use sphere_proto::paths;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub state_dir: PathBuf,
    pub socket: Option<PathBuf>,
    pub host_socket: Option<PathBuf>,
    /// The `sphere-host` binary. Default: next to this executable, else `PATH`.
    pub host_command: Option<PathBuf>,
    /// The agent binary (Claude Code).
    pub agent_command: String,
    /// Arguments placed before the ones sphered generates.
    pub agent_args: Vec<String>,
    pub max_concurrent: usize,
    pub default_budget_usd: f64,
    pub daily_cap_usd: Option<f64>,
    /// How long a stop waits between terminate and kill.
    pub stop_grace_ms: u64,
    /// How long a stop waits after the interrupt before terminating.
    pub interrupt_wait_ms: u64,
    /// How long an idle agent stays warm after a result.
    pub idle_ttl_ms: u64,
    /// Check `claude --version` against the tested range (warn only).
    pub check_version: bool,
    /// Lists Claude sessions as JSON. Default: `<agent> agents --json`.
    pub session_list_command: Option<Vec<String>>,
    /// Reports sign-in as JSON. Default: `<agent> auth status`.
    pub auth_status_command: Option<Vec<String>>,
    /// Where Claude Code keeps transcripts. Default: `~/.claude/projects`.
    pub claude_projects_dir: Option<PathBuf>,
    /// Per-subscriber queue length before a slow client is dropped.
    pub subscriber_queue: usize,
    /// Output bigger than this is cut to a head and tail in the event.
    pub excerpt_bytes: usize,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            state_dir: paths::default_state_dir(),
            socket: None,
            host_socket: None,
            host_command: None,
            agent_command: "claude".into(),
            agent_args: vec![],
            max_concurrent: 3,
            default_budget_usd: 5.0,
            daily_cap_usd: None,
            stop_grace_ms: 5000,
            interrupt_wait_ms: 1000,
            idle_ttl_ms: 10 * 60 * 1000,
            check_version: true,
            session_list_command: None,
            auth_status_command: None,
            claude_projects_dir: None,
            subscriber_queue: 1024,
            excerpt_bytes: 4096,
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Config> {
        let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parse {}", path.display()))
    }

    pub fn socket_path(&self) -> PathBuf {
        self.socket.clone().unwrap_or_else(paths::default_socket)
    }

    pub fn host_socket_path(&self) -> PathBuf {
        self.host_socket.clone().unwrap_or_else(paths::default_host_socket)
    }

    pub fn host_dir(&self) -> PathBuf {
        self.state_dir.join("host")
    }

    pub fn db_path(&self) -> PathBuf {
        self.state_dir.join("sphere.db")
    }

    pub fn policy_path(&self) -> PathBuf {
        self.state_dir.join("policy.toml")
    }

    pub fn worktrees_dir(&self) -> PathBuf {
        self.state_dir.join("worktrees")
    }

    pub fn settings_dir(&self) -> PathBuf {
        self.state_dir.join("settings")
    }

    pub fn projects_dir(&self) -> PathBuf {
        self.claude_projects_dir
            .clone()
            .unwrap_or_else(paths::default_claude_projects_dir)
    }

    pub fn host_binary(&self) -> PathBuf {
        if let Some(p) = &self.host_command {
            return p.clone();
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                let sibling = dir.join("sphere-host");
                if sibling.exists() {
                    return sibling;
                }
            }
        }
        PathBuf::from("sphere-host")
    }

    pub fn session_list_cmd(&self) -> Vec<String> {
        self.session_list_command
            .clone()
            .unwrap_or_else(|| vec![self.agent_command.clone(), "agents".into(), "--json".into()])
    }

    pub fn auth_status_cmd(&self) -> Vec<String> {
        self.auth_status_command
            .clone()
            .unwrap_or_else(|| vec![self.agent_command.clone(), "auth".into(), "status".into()])
    }

    /// The fake agent has no version worth checking.
    pub fn version_check_applies(&self) -> bool {
        self.check_version
            && !Path::new(&self.agent_command)
                .file_name()
                .map(|n| n.to_string_lossy().starts_with("fake-claude"))
                .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_config_file_and_rejects_typos() {
        let c: Config = toml::from_str("max_concurrent = 1\nagent_args = [\"--fixture\", \"f\"]\n").unwrap();
        assert_eq!(c.max_concurrent, 1);
        assert_eq!(c.agent_args, vec!["--fixture", "f"]);
        assert_eq!(c.stop_grace_ms, 5000);
        assert!(toml::from_str::<Config>("max_concurent = 1").is_err());
    }

    #[test]
    fn the_fake_agent_skips_the_version_check() {
        let c = Config { agent_command: "/x/target/debug/fake-claude".into(), ..Config::default() };
        assert!(!c.version_check_applies());
        assert!(Config::default().version_check_applies());
    }
}
