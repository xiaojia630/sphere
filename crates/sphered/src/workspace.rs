//! Workspaces: a git worktree per task, outside the user's checkout, on its own
//! branch. Uses the `git` command line, like everything else on the machine.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use sphere_proto::FileStat;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    pub workdir: PathBuf,
    pub branch: Option<String>,
    /// The commit the task started from; "files touched" is measured against it.
    pub base: String,
}

/// A repo's `.sphere/config.toml`.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RepoConfig {
    /// Untracked files to copy into the worktree (`.env`, ...).
    pub copy: Vec<String>,
    /// Run in the worktree after it is created (`npm ci`, ...).
    pub setup: Option<String>,
}

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .context("run git")?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

/// `Fix the login bug!` -> `fix-the-login-bug`
pub fn slug(title: &str) -> String {
    let mut out = String::new();
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
        if out.len() >= 40 {
            break;
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() {
        "task".to_string()
    } else {
        out
    }
}

pub fn repo_toplevel(repo: &Path) -> Result<PathBuf> {
    let top = git(repo, &["rev-parse", "--show-toplevel"])
        .with_context(|| format!("{} is not a git repository", repo.display()))?;
    Ok(PathBuf::from(top))
}

fn branch_exists(repo: &Path, name: &str) -> bool {
    git(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{name}"),
        ],
    )
    .is_ok()
}

fn read_repo_config(top: &Path) -> Result<RepoConfig> {
    let path = top.join(".sphere").join("config.toml");
    match std::fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text).with_context(|| format!("parse {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(RepoConfig::default()),
        Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
    }
}

/// Only plain relative paths inside the repo may be copied.
fn safe_relative(p: &str) -> bool {
    let path = Path::new(p);
    path.is_relative()
        && !path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
}

/// Create the workspace for a task: a worktree on `sphere/<slug>`, or the repo
/// itself with `in_place`. Runs the repo's copy list and setup command.
pub fn prepare(
    worktrees: &Path,
    repo: &Path,
    task_id: &str,
    title: &str,
    in_place: bool,
) -> Result<Workspace> {
    let top = repo_toplevel(repo)?;
    let base = git(&top, &["rev-parse", "HEAD"]).context("the repository has no commits yet")?;
    if in_place {
        return Ok(Workspace {
            workdir: repo.canonicalize().unwrap_or_else(|_| repo.to_path_buf()),
            branch: None,
            base,
        });
    }
    let repo_name = top
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "repo".into());
    let workdir = worktrees.join(&repo_name).join(task_id);
    if let Some(parent) = workdir.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let base_name = format!("sphere/{}", slug(title));
    let mut branch = base_name.clone();
    let mut n = 2;
    while branch_exists(&top, &branch) {
        branch = format!("{base_name}-{n}");
        n += 1;
    }
    // forget worktrees whose folders are gone: task ids start again with a fresh database, and git refuses a
    // path that is still registered to a deleted worktree
    let _ = git(&top, &["worktree", "prune"]);
    git(
        &top,
        &[
            "worktree",
            "add",
            "-b",
            &branch,
            &workdir.to_string_lossy(),
            "HEAD",
        ],
    )?;

    let cfg = read_repo_config(&top)?;
    for rel in &cfg.copy {
        if !safe_relative(rel) {
            bail!("refusing to copy {rel:?}: only relative paths inside the repo are allowed");
        }
        let from = top.join(rel);
        if from.is_file() {
            let to = workdir.join(rel);
            if let Some(parent) = to.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&from, &to).with_context(|| format!("copy {rel}"))?;
        }
    }
    if let Some(setup) = &cfg.setup {
        let out = Command::new("sh")
            .arg("-c")
            .arg(setup)
            .current_dir(&workdir)
            .output()
            .context("run setup")?;
        if !out.status.success() {
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            let tail: String = text
                .chars()
                .rev()
                .take(800)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            bail!(
                "setup command `{setup}` failed ({}): {}",
                out.status,
                tail.trim()
            );
        }
    }
    Ok(Workspace {
        workdir,
        branch: Some(branch),
        base,
    })
}

/// Files changed since `base`, from `git diff --numstat` plus untracked files.
/// This also catches changes made by shell commands.
pub fn files_touched(workdir: &Path, base: &str) -> Result<Vec<FileStat>> {
    let mut files: Vec<FileStat> = vec![];
    let numstat = git(workdir, &["diff", "--numstat", "--no-renames", base])?;
    for line in numstat.lines() {
        let mut it = line.splitn(3, '\t');
        let (Some(a), Some(r), Some(path)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        files.push(FileStat {
            path: path.to_string(),
            added: a.parse().unwrap_or(0),
            removed: r.parse().unwrap_or(0),
        });
    }
    let untracked = git(workdir, &["ls-files", "--others", "--exclude-standard"])?;
    for path in untracked.lines().filter(|p| !p.is_empty()) {
        let full = workdir.join(path);
        let added = match std::fs::metadata(&full) {
            Ok(m) if m.is_file() && m.len() <= 1_000_000 => std::fs::read_to_string(&full)
                .map(|t| t.lines().count() as u64)
                .unwrap_or(0),
            _ => 0,
        };
        files.push(FileStat {
            path: path.to_string(),
            added,
            removed: 0,
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

#[cfg(test)]
pub mod testutil {
    use super::*;

    /// A repository with one commit.
    pub fn init_repo(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        let run = |args: &[&str]| {
            let st = Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(args)
                .output()
                .unwrap();
            assert!(
                st.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&st.stderr)
            );
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "T"]);
        std::fs::write(dir.join("README.md"), "hello\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "init"]);
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::init_repo;
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(slug("Fix the login bug!"), "fix-the-login-bug");
        assert_eq!(slug("  ---  "), "task");
        assert_eq!(slug("a".repeat(100).as_str()).len(), 40);
        assert_eq!(slug("Ünïcode & more"), "n-code-more");
    }

    #[test]
    fn creates_a_worktree_on_its_own_branch_and_avoids_branch_collisions() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("myrepo");
        init_repo(&repo);
        let wt = tmp.path().join("worktrees");

        let a = prepare(&wt, &repo, "t1", "Fix login", false).unwrap();
        assert_eq!(a.workdir, wt.join("myrepo").join("t1"));
        assert_eq!(a.branch.as_deref(), Some("sphere/fix-login"));
        assert!(a.workdir.join("README.md").exists());
        assert_eq!(
            git(&a.workdir, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap(),
            "sphere/fix-login"
        );
        assert_eq!(
            git(&repo, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap(),
            "main",
            "the checkout is untouched"
        );

        let b = prepare(&wt, &repo, "t2", "Fix login", false).unwrap();
        assert_eq!(b.branch.as_deref(), Some("sphere/fix-login-2"));
        assert_eq!(a.base, b.base);
    }

    #[test]
    fn reuses_the_path_of_a_worktree_whose_folder_was_deleted() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("myrepo");
        init_repo(&repo);
        let wt = tmp.path().join("worktrees");
        let a = prepare(&wt, &repo, "t1", "Fix login", false).unwrap();
        std::fs::remove_dir_all(&a.workdir).unwrap();
        // a fresh database hands out t1 again
        let b = prepare(&wt, &repo, "t1", "Fix login", false).unwrap();
        assert_eq!(b.workdir, a.workdir);
        assert_eq!(b.branch.as_deref(), Some("sphere/fix-login-2"));
        assert!(b.workdir.join("README.md").exists());
    }

    #[test]
    fn in_place_uses_the_repo_itself() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("r");
        init_repo(&repo);
        let w = prepare(&tmp.path().join("wt"), &repo, "t1", "x", true).unwrap();
        assert_eq!(w.workdir, repo.canonicalize().unwrap());
        assert_eq!(w.branch, None);
        assert!(!tmp.path().join("wt").exists());
    }

    #[test]
    fn copies_listed_files_and_runs_setup() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("r");
        init_repo(&repo);
        std::fs::write(repo.join(".env"), "SECRET=1\n").unwrap();
        std::fs::create_dir_all(repo.join(".sphere")).unwrap();
        std::fs::write(
            repo.join(".sphere/config.toml"),
            "copy = [\".env\", \"missing.txt\"]\nsetup = \"echo ready > setup-ran.txt\"\n",
        )
        .unwrap();
        let w = prepare(&tmp.path().join("wt"), &repo, "t1", "x", false).unwrap();
        assert_eq!(
            std::fs::read_to_string(w.workdir.join(".env")).unwrap(),
            "SECRET=1\n"
        );
        assert_eq!(
            std::fs::read_to_string(w.workdir.join("setup-ran.txt")).unwrap(),
            "ready\n"
        );
    }

    #[test]
    fn a_failing_setup_and_unsafe_copy_paths_are_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("r");
        init_repo(&repo);
        std::fs::create_dir_all(repo.join(".sphere")).unwrap();
        std::fs::write(
            repo.join(".sphere/config.toml"),
            "setup = \"echo nope >&2; exit 3\"\n",
        )
        .unwrap();
        let err = prepare(&tmp.path().join("wt"), &repo, "t1", "x", false).unwrap_err();
        assert!(format!("{err:#}").contains("nope"), "{err:#}");

        std::fs::write(repo.join(".sphere/config.toml"), "copy = [\"../escape\"]\n").unwrap();
        let err = prepare(&tmp.path().join("wt"), &repo, "t2", "y", false).unwrap_err();
        assert!(format!("{err:#}").contains("refusing"), "{err:#}");
    }

    #[test]
    fn not_a_repo_is_a_clear_error() {
        let tmp = tempfile::tempdir().unwrap();
        let err = prepare(&tmp.path().join("wt"), tmp.path(), "t1", "x", false).unwrap_err();
        assert!(
            format!("{err:#}").contains("not a git repository"),
            "{err:#}"
        );
    }

    #[test]
    fn files_touched_counts_edits_commits_and_new_files() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("r");
        init_repo(&repo);
        let w = prepare(&tmp.path().join("wt"), &repo, "t1", "x", false).unwrap();
        assert!(files_touched(&w.workdir, &w.base).unwrap().is_empty());
        std::fs::write(w.workdir.join("README.md"), "hello\nworld\n").unwrap();
        std::fs::write(w.workdir.join("new.txt"), "a\nb\nc\n").unwrap();
        let files = files_touched(&w.workdir, &w.base).unwrap();
        assert_eq!(
            files,
            vec![
                FileStat {
                    path: "README.md".into(),
                    added: 1,
                    removed: 0
                },
                FileStat {
                    path: "new.txt".into(),
                    added: 3,
                    removed: 0
                },
            ]
        );
        // Committed changes still count against the base.
        git(&w.workdir, &["add", "."]).unwrap();
        git(
            &w.workdir,
            &[
                "-c",
                "user.email=a@b",
                "-c",
                "user.name=n",
                "commit",
                "-q",
                "-m",
                "c",
            ],
        )
        .unwrap();
        assert_eq!(files_touched(&w.workdir, &w.base).unwrap().len(), 2);
    }
}
