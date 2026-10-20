//! The web playground's pretend machine. The apps there run their own code on what the tools they read would
//! print on a developer's Mac (a compose project, a few dev servers, two repos with agents' branches), so the
//! Containers, Ports & logs, System, Git and Code apps look and work as they do in the app. Commands that change
//! something answer as if they had worked. Compiled for the playground and for the tests that keep these outputs
//! readable by the apps' parsers.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
use std::path::Path;

mod fixtures;
mod gitrepo;
mod ops;

#[allow(unused_imports)]
pub use gitrepo::{file, files, is_dir, is_repo_top, lang, repos};
#[allow(unused_imports)]
pub use ops::core_ticks;

/// The pretend machine's home folder.
pub const HOME: &str = "/Users/you";
/// Its git repos, in ~/Projects.
pub const REPOS: &[&str] = &["/Users/you/Projects/shop-api", "/Users/you/Projects/sphere"];

/// Whether `p` is the top of one of its repos, or a worktree an agent works in.
pub fn is_repo(p: &Path) -> bool {
    REPOS.iter().any(|r| p == Path::new(r)) || p.starts_with("/Users/you/.sphere/worktrees")
}

/// What `cmd args` prints: docker, lsof, ps, vm_stat, sysctl, netstat, df, pmset, ioreg (`opsdata`).
pub fn run(cmd: &str, args: &[&str]) -> Result<String, String> {
    ops::run(cmd, args)
}

/// What `git -C dir args` prints (`gitdata::git`).
pub fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    gitrepo::git(dir, args)
}

/// What `gh args` prints in `dir` (`prs::run_gh`).
pub fn gh(dir: &Path, args: &[&str]) -> Result<crate::prs::Out, crate::prs::GhError> {
    gitrepo::gh(dir, args)
}
