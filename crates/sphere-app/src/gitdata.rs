//! What the Git app shows and does, read with `git` (argv only, never a shell). Everything here blocks, so the
//! app calls it from background threads.
use std::path::{Path, PathBuf};
use std::process::Command;

/// Runs git in `dir` (argv only). Never asks for a password or a pager, and read-only commands don't take the
/// index lock, so polling doesn't make the user's own `git commit` fail.
pub(crate) fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir).args(["-c", "core.quotepath=off", "-c", "http.lowSpeedLimit=1000", "-c", "http.lowSpeedTime=30"]).args(args);
    cmd.env("GIT_TERMINAL_PROMPT", "0").env("GIT_OPTIONAL_LOCKS", "0").env("GIT_PAGER", "cat").env("LC_ALL", "C");
    if std::env::var_os("GIT_SSH_COMMAND").is_none() {
        cmd.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes -o ConnectTimeout=15");
    }
    let out = cmd.stdin(std::process::Stdio::null()).output().map_err(|e| format!("git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(err_line(&String::from_utf8_lossy(&out.stderr)))
    }
}

/// Like `git`, trimmed.
pub(crate) fn git_t(dir: &Path, args: &[&str]) -> Result<String, String> {
    git(dir, args).map(|s| s.trim().to_string())
}

/// The line of git's complaint worth showing: a rejection or a fatal/error line, else the first one.
pub(crate) fn err_line(stderr: &str) -> String {
    let lines: Vec<&str> = stderr.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let pick = lines
        .iter()
        .find(|l| l.contains("[rejected]") || l.contains("[remote rejected]"))
        .or_else(|| lines.iter().find(|l| l.starts_with("fatal:") && !l.contains("failed to push")))
        .or_else(|| lines.iter().find(|l| l.starts_with("error:") && !l.contains("failed to push")))
        .or_else(|| lines.first());
    match pick {
        Some(l) => l.trim_start_matches("fatal: ").trim_start_matches("error: ").trim_start_matches("! ").to_string(),
        None => "git failed".to_string(),
    }
}

/// A path cut to `n` characters from the front, so the file name survives.
pub fn clip_start(s: &str, n: usize) -> String {
    let len = s.chars().count();
    if len > n {
        format!("…{}", s.chars().skip(len - n.saturating_sub(1)).collect::<String>())
    } else {
        s.to_string()
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RepoCard {
    pub name: String,
    pub path: String,
    pub lang: String,
    /// the branch, or `detached abc1234`
    pub branch: String,
    pub detached: bool,
    /// no commits yet
    pub empty: bool,
    pub ahead: u32,
    pub behind: u32,
    pub changed: usize,
    pub stashes: usize,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Branch {
    pub name: String,
    pub age: String,
    pub ahead: u32,
    pub behind: u32,
    pub current: bool,
    /// an agent's branch (sphere/…, claude/…)
    pub agent: bool,
}

/// One row of `git log --graph`: the lane drawing, then the commit (empty for connector rows).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GraphRow {
    pub lanes: String,
    pub hash: String,
    pub refs: Vec<String>,
    pub subject: String,
    pub age: String,
}

/// A commit in the graph, placed: its lane (0 = the first branch's line) and row (0 = newest).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Node {
    pub hash: String,
    pub parents: Vec<String>,
    pub refs: Vec<String>,
    pub subject: String,
    pub lane: usize,
    pub row: usize,
}

/// Lays commits (newest first, each with its parents) out in lanes: a commit takes the lane that was waiting
/// for it (or a new one), hands that lane to its first parent and opens lanes for the others.
pub fn lay_out(commits: Vec<(String, Vec<String>, Vec<String>, String)>) -> Vec<Node> {
    let mut lanes: Vec<Option<String>> = Vec::new();
    let mut out = Vec::with_capacity(commits.len());
    for (row, (hash, parents, refs, subject)) in commits.into_iter().enumerate() {
        let lane = match lanes.iter().position(|l| l.as_deref() == Some(hash.as_str())) {
            Some(i) => i,
            None => match lanes.iter().position(|l| l.is_none()) {
                Some(i) => i,
                None => {
                    lanes.push(None);
                    lanes.len() - 1
                }
            },
        };
        // any other lane waiting for this commit merges here
        for l in lanes.iter_mut() {
            if l.as_deref() == Some(hash.as_str()) {
                *l = None;
            }
        }
        lanes[lane] = parents.first().cloned();
        for p in parents.iter().skip(1) {
            if !lanes.iter().any(|l| l.as_deref() == Some(p.as_str())) {
                match lanes.iter().position(|l| l.is_none()) {
                    Some(i) => lanes[i] = Some(p.clone()),
                    None => lanes.push(Some(p.clone())),
                }
            }
        }
        out.push(Node { hash, parents, refs, subject, lane, row });
    }
    out
}

pub fn dag(dir: &Path, n: usize) -> Vec<Node> {
    let raw = git(dir, &["log", "--all", "--date-order", "-n", &n.to_string(), "--format=%h%x02%p%x02%D%x02%s"]).unwrap_or_default();
    let commits = raw
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\u{2}').collect();
            (f.len() >= 4).then(|| (f[0].to_string(), f[1].split_whitespace().map(|s| s.to_string()).collect(), f[2].split(", ").filter(|s| !s.is_empty() && *s != "HEAD").map(|s| s.trim_start_matches("HEAD -> ").to_string()).collect(), f[3].to_string()))
        })
        .collect();
    lay_out(commits)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub path: String,
    /// M, A, D, R, U (unmerged) or ? (untracked)
    pub code: char,
    /// a renamed file's old path
    pub from: Option<String>,
    pub added: Option<u64>,
    pub removed: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Repo {
    pub card: RepoCard,
    pub branches: Vec<Branch>,
    pub graph: Vec<GraphRow>,
    pub dag: Vec<Node>,
    pub staged: Vec<Change>,
    pub unstaged: Vec<Change>,
    pub upstream: Option<String>,
    pub remote_url: String,
    pub has_remote: bool,
    /// a merge, rebase or cherry-pick is half done
    pub in_progress: Option<&'static str>,
    /// (hash, subject, author, age) of HEAD
    pub last: Option<(String, String, String, String)>,
    pub error: Option<String>,
}

fn lang(dir: &Path) -> &'static str {
    if dir.join("Cargo.toml").exists() {
        "RUST"
    } else if dir.join("tsconfig.json").exists() {
        "TYPESCRIPT"
    } else if dir.join("package.json").exists() {
        "JAVASCRIPT"
    } else if dir.join("go.mod").exists() {
        "GO"
    } else if dir.join("pyproject.toml").exists() || dir.join("setup.py").exists() {
        "PYTHON"
    } else {
        ""
    }
}

/// ahead, behind against the upstream (0, 0 without one)
fn track(dir: &Path) -> (u32, u32) {
    let s = git(dir, &["rev-list", "--left-right", "--count", "HEAD...@{upstream}"]).unwrap_or_default();
    let mut it = s.split_whitespace().map(|n| n.parse().unwrap_or(0));
    (it.next().unwrap_or(0), it.next().unwrap_or(0))
}

/// The branch name, or `None` on a detached HEAD.
fn branch_name(dir: &Path) -> Option<String> {
    git_t(dir, &["symbolic-ref", "--short", "-q", "HEAD"]).ok().filter(|s| !s.is_empty())
}

fn short_head(dir: &Path) -> Option<String> {
    git_t(dir, &["rev-parse", "--short", "HEAD"]).ok().filter(|s| !s.is_empty())
}

/// How many files `git status` lists (a file staged and edited again counts once).
pub fn changed_count(porcelain_z: &str) -> usize {
    let (s, u) = parse_status(porcelain_z);
    let mut paths: Vec<&str> = s.iter().chain(u.iter()).map(|c| c.path.as_str()).collect();
    paths.sort_unstable();
    paths.dedup();
    paths.len()
}

fn card_with(dir: &Path, status: Option<&str>) -> RepoCard {
    let (ahead, behind) = track(dir);
    let name = branch_name(dir);
    let head = short_head(dir);
    let status = status.map(str::to_string).unwrap_or_else(|| git(dir, &["status", "--porcelain=v1", "-z"]).unwrap_or_default());
    RepoCard {
        name: dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
        path: dir.to_string_lossy().to_string(),
        lang: lang(dir).to_string(),
        detached: name.is_none() && head.is_some(),
        empty: head.is_none(),
        branch: match (&name, &head) {
            (Some(n), _) => n.clone(),
            (None, Some(h)) => format!("detached {h}"),
            (None, None) => String::new(),
        },
        ahead,
        behind,
        changed: changed_count(&status),
        stashes: git(dir, &["stash", "list"]).unwrap_or_default().lines().count(),
    }
}

pub fn card(dir: &Path) -> RepoCard {
    card_with(dir, None)
}

/// A card without any git calls (for repos not in view).
pub fn bare_card(dir: &Path) -> RepoCard {
    RepoCard { name: dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(), path: dir.to_string_lossy().to_string(), ..Default::default() }
}

/// The repos to show: the ones agents work in first, then git repos under ~/Projects, most recently committed
/// first.
pub fn discover(agent_repos: &[String], max: usize) -> Vec<PathBuf> {
    let projects = sphere_proto::paths::default_state_dir().parent().map(|h| h.join("Projects"));
    discover_in(agent_repos, projects.as_deref(), max)
}

pub fn discover_in(agent_repos: &[String], projects: Option<&Path>, max: usize) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut push = |p: PathBuf| {
        if p.join(".git").exists() && !out.contains(&p) {
            out.push(p);
        }
    };
    for r in agent_repos {
        push(PathBuf::from(r));
    }
    let mut found: Vec<(i64, PathBuf)> = Vec::new();
    if let Some(Ok(rd)) = projects.map(std::fs::read_dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.join(".git").is_dir() {
                let t = git(&p, &["log", "-1", "--format=%ct"]).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
                found.push((t, p));
            }
        }
    }
    found.sort_by_key(|f| std::cmp::Reverse(f.0));
    for (_, p) in found {
        push(p);
    }
    out.truncate(max);
    out
}

fn in_progress(dir: &Path) -> Option<&'static str> {
    let g = git_t(dir, &["rev-parse", "--git-dir"]).ok()?;
    let g = if Path::new(&g).is_absolute() { PathBuf::from(g) } else { dir.join(g) };
    [("MERGE_HEAD", "merging"), ("rebase-merge", "rebasing"), ("rebase-apply", "rebasing"), ("CHERRY_PICK_HEAD", "cherry-picking"), ("REVERT_HEAD", "reverting")].into_iter().find(|(f, _)| g.join(f).exists()).map(|x| x.1)
}

pub fn load(dir: &Path) -> Repo {
    let mut r = Repo::default();
    if !dir.exists() {
        r.card = bare_card(dir);
        r.error = Some("The folder is gone".into());
        return r;
    }
    if let Err(e) = git(dir, &["rev-parse", "--git-dir"]) {
        r.card = bare_card(dir);
        r.error = Some(e);
        return r;
    }
    let status = git(dir, &["status", "--porcelain=v1", "-z"]).unwrap_or_default();
    r.card = card_with(dir, Some(&status));
    let cur = r.card.branch.clone();
    r.branches = git(dir, &["for-each-ref", "--sort=-committerdate", "--format=%(refname:short)%09%(committerdate:relative)%09%(upstream:track,nobracket)", "refs/heads"])
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let mut f = l.split('\t');
            let name = f.next()?.to_string();
            let age = f.next().unwrap_or("").to_string();
            let tr = f.next().unwrap_or("");
            let num = |k: &str| tr.split(", ").find_map(|p| p.strip_prefix(k)).and_then(|n| n.trim().parse().ok()).unwrap_or(0);
            Some(Branch { current: !r.card.detached && name == cur, agent: name.starts_with("sphere/") || name.starts_with("claude/"), ahead: num("ahead "), behind: num("behind "), name, age })
        })
        .collect();
    if r.card.empty && !cur.is_empty() {
        // a branch with no commits isn't a ref yet
        r.branches.push(Branch { name: cur, age: "no commits yet".into(), current: true, ..Default::default() });
    }
    if !r.card.empty {
        r.dag = dag(dir, 26);
        r.graph = parse_graph(&git(dir, &["log", "--graph", "--all", "--date-order", "-n", "200", "--format=%x01%h%x02%D%x02%s%x02%ar"]).unwrap_or_default());
    }
    let (staged, unstaged) = parse_status(&status);
    r.staged = with_counts(staged, &git(dir, &["diff", "--cached", "--numstat", "-z"]).unwrap_or_default());
    r.unstaged = with_counts(unstaged, &git(dir, &["diff", "--numstat", "-z"]).unwrap_or_default());
    r.upstream = git_t(dir, &["rev-parse", "--abbrev-ref", "@{upstream}"]).ok().filter(|s| !s.is_empty());
    r.remote_url = git_t(dir, &["remote", "get-url", "origin"]).unwrap_or_default();
    r.has_remote = !git_t(dir, &["remote"]).unwrap_or_default().is_empty();
    r.in_progress = in_progress(dir);
    r.last = git(dir, &["log", "-1", "--format=%h%x09%s%x09%an%x09%ar"]).ok().and_then(|s| {
        let v: Vec<&str> = s.trim().splitn(4, '\t').collect();
        (v.len() == 4).then(|| (v[0].into(), v[1].into(), v[2].into(), v[3].into()))
    });
    r
}

pub fn parse_graph(raw: &str) -> Vec<GraphRow> {
    raw.lines()
        .map(|l| match l.split_once('\u{1}') {
            Some((lanes, rest)) => {
                let f: Vec<&str> = rest.split('\u{2}').collect();
                let refs = f.get(1).map(|d| d.split(", ").filter(|s| !s.is_empty()).map(|s| s.trim_start_matches("HEAD -> ").to_string()).filter(|s| s != "HEAD").collect()).unwrap_or_default();
                GraphRow { lanes: lanes.to_string(), hash: f.first().unwrap_or(&"").to_string(), refs, subject: f.get(2).unwrap_or(&"").to_string(), age: f.get(3).unwrap_or(&"").to_string() }
            }
            None => GraphRow { lanes: l.to_string(), ..Default::default() },
        })
        .collect()
}

/// `git status --porcelain=v1 -z`: (staged, unstaged), untracked files counting as unstaged and merge conflicts
/// as unstaged `U`.
pub fn parse_status(raw: &str) -> (Vec<Change>, Vec<Change>) {
    let (mut st, mut un) = (Vec::new(), Vec::new());
    let mut it = raw.split('\0').filter(|s| s.len() > 3);
    while let Some(e) = it.next() {
        let (x, y) = (e.as_bytes()[0] as char, e.as_bytes()[1] as char);
        let path = e[3..].to_string();
        let from = if x == 'R' || x == 'C' || y == 'R' || y == 'C' { it.next().map(|s| s.to_string()) } else { None };
        let ch = |code: char, from: Option<String>| Change { path: path.clone(), code, from, added: None, removed: None };
        if x == '?' {
            un.push(ch('?', None));
            continue;
        }
        if x == '!' {
            continue;
        }
        if matches!((x, y), ('U', _) | (_, 'U') | ('A', 'A') | ('D', 'D')) {
            un.push(ch('U', None));
            continue;
        }
        if x != ' ' {
            st.push(ch(x, from.clone().filter(|_| x == 'R' || x == 'C')));
        }
        if y != ' ' {
            un.push(ch(y, from.filter(|_| y == 'R' || y == 'C')));
        }
    }
    (st, un)
}

/// Adds `git diff --numstat -z` counts to the changes they belong to (a rename's record is `a\tr\t` then the old
/// and new names as their own records).
fn with_counts(mut v: Vec<Change>, numstat: &str) -> Vec<Change> {
    let index: std::collections::HashMap<String, usize> = v.iter().enumerate().map(|(i, c)| (c.path.clone(), i)).collect();
    let mut it = numstat.split('\0');
    while let Some(rec) = it.next() {
        let f: Vec<&str> = rec.splitn(3, '\t').collect();
        if f.len() < 3 {
            continue;
        }
        let path = if f[2].is_empty() {
            it.next();
            it.next().unwrap_or("")
        } else {
            f[2]
        };
        if let Some(&i) = index.get(path) {
            v[i].added = f[0].parse().ok();
            v[i].removed = f[1].parse().ok();
        }
    }
    v
}

/// What the preview of a change shows.
pub type Preview = Vec<(char, String)>;

/// The first lines of a file's change, for the preview under the working tree.
pub fn preview(dir: &Path, path: &str, staged: bool, untracked: bool) -> Preview {
    use std::io::Read;
    if untracked {
        let p = dir.join(path);
        let Ok(meta) = std::fs::symlink_metadata(&p) else { return vec![('@', "gone".into())] };
        if meta.is_dir() {
            return vec![('@', "a new folder".into())];
        }
        if meta.file_type().is_symlink() {
            return vec![('@', "a new link".into())];
        }
        let mut buf = Vec::new();
        let Ok(f) = std::fs::File::open(&p) else { return vec![('@', "can't read it".into())] };
        let _ = f.take(64 * 1024).read_to_end(&mut buf);
        if buf.contains(&0) {
            return vec![('@', format!("binary file · {} bytes", meta.len()))];
        }
        return String::from_utf8_lossy(&buf).lines().take(8).map(|l| ('+', l.replace('\t', "    "))).collect();
    }
    let args: Vec<&str> = if staged { vec!["diff", "--cached", "--no-color", "--no-ext-diff", "-U1", "--", path] } else { vec!["diff", "--no-color", "--no-ext-diff", "-U1", "--", path] };
    let raw = git(dir, &args).unwrap_or_default();
    if raw.lines().any(|l| l.starts_with("Binary files ")) {
        return vec![('@', "binary file".into())];
    }
    raw.lines()
        .skip_while(|l| !l.starts_with("@@"))
        .take(9)
        .map(|l| match l.chars().next() {
            Some(c @ ('+' | '-')) => (c, l[1..].replace('\t', "    ")),
            Some('@') => ('@', l.to_string()),
            _ => (' ', l.get(1..).unwrap_or("").replace('\t', "    ")),
        })
        .collect()
}

pub enum Op {
    Stage(String),
    Unstage(String, Option<String>),
    StageAll,
    UnstageAll,
    Commit { msg: String, amend: bool },
    Switch(String),
    Fetch,
    Pull,
    Push,
}

fn commit_count(dir: &Path, range: &str) -> usize {
    git_t(dir, &["rev-list", "--count", range]).ok().and_then(|s| s.parse().ok()).unwrap_or(0)
}

/// Runs one action and says what happened.
pub fn run(dir: &Path, op: Op) -> Result<String, String> {
    match op {
        Op::Stage(p) => git(dir, &["add", "--", &p]).map(|_| format!("Staged {p}")),
        Op::Unstage(p, from) => {
            let mut paths = vec![p.as_str()];
            paths.extend(from.as_deref());
            let mut a = vec!["restore", "--staged", "--"];
            a.extend(&paths);
            let mut b = vec!["rm", "--cached", "-q", "-r", "--"];
            b.extend(&paths);
            git(dir, &a).or_else(|_| git(dir, &b)).map(|_| format!("Unstaged {p}"))
        }
        Op::StageAll => git(dir, &["add", "-A"]).map(|_| "Staged everything".into()),
        Op::UnstageAll => git(dir, &["reset", "-q"]).map(|_| "Unstaged everything".into()),
        Op::Commit { msg, amend } => {
            let msg = msg.trim().to_string();
            if msg.is_empty() && !amend {
                return Err("A commit needs a message".into());
            }
            let mut a = vec!["commit", "-q"];
            if msg.is_empty() {
                a.push("--no-edit");
            } else {
                a.extend(["-m", msg.as_str()]);
            }
            if amend {
                a.push("--amend");
            }
            git(dir, &a)?;
            let h = git_t(dir, &["log", "-1", "--format=%h"]).unwrap_or_default();
            Ok(format!("{} {h} · {}", if amend { "Amended" } else { "Committed" }, msg))
        }
        Op::Switch(b) => {
            let b = b.trim();
            if b.is_empty() || b.starts_with('-') {
                return Err("That isn't a branch name".into());
            }
            if let Some(new) = b.strip_prefix('+') {
                let new = new.trim();
                if new.is_empty() || new.starts_with('-') {
                    return Err("That isn't a branch name".into());
                }
                return git(dir, &["switch", "-c", new]).map(|_| format!("Created and switched to {new}"));
            }
            let exists = git(dir, &["rev-parse", "--verify", "-q", &format!("refs/heads/{b}")]).is_ok() || git(dir, &["rev-parse", "--verify", "-q", &format!("refs/remotes/origin/{b}")]).is_ok();
            if !exists {
                return Err(format!("No branch {b} (type +{b} to create it)"));
            }
            git(dir, &["switch", b]).map(|_| format!("On {b}"))
        }
        Op::Fetch => {
            if git_t(dir, &["remote"])?.is_empty() {
                return Err("This repo has no remote".into());
            }
            git(dir, &["fetch", "--all", "--prune", "-q"]).map(|_| "Fetched".into()).map_err(|e| format!("Fetch: {e}"))
        }
        Op::Pull => {
            if branch_name(dir).is_none() {
                return Err("Detached HEAD: switch to a branch to pull".into());
            }
            if git(dir, &["rev-parse", "--abbrev-ref", "@{upstream}"]).is_err() {
                return Err("No upstream for this branch: push it first (P)".into());
            }
            let before = git_t(dir, &["rev-parse", "HEAD"]).unwrap_or_default();
            git(dir, &["pull", "--ff-only", "-q"]).map_err(|e| format!("Pull: {e}"))?;
            let n = if before.is_empty() { 0 } else { commit_count(dir, &format!("{before}..HEAD")) };
            Ok(if n == 0 { "Already up to date".into() } else { format!("Pulled {n} commit{}", if n == 1 { "" } else { "s" }) })
        }
        Op::Push => {
            let Some(branch) = branch_name(dir) else { return Err("Detached HEAD: switch to a branch to push".into()) };
            let remotes = git_t(dir, &["remote"]).unwrap_or_default();
            if remotes.is_empty() {
                return Err("This repo has no remote to push to".into());
            }
            if git(dir, &["rev-parse", "--abbrev-ref", "@{upstream}"]).is_ok() {
                if track(dir).0 == 0 {
                    return Ok("Nothing to push".into());
                }
                git(dir, &["push", "-q"]).map(|_| format!("Pushed {branch}")).map_err(|e| format!("Push: {e}"))
            } else {
                let remote = if remotes.lines().any(|r| r == "origin") { "origin" } else { remotes.lines().next().unwrap_or("origin") };
                git(dir, &["push", "-q", "-u", remote, "HEAD"]).map(|_| format!("Pushed {branch} to {remote}")).map_err(|e| format!("Push: {e}"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn reads_status_and_graph() {
        let (st, un) = parse_status(" M a.rs\0M  b.rs\0MM c.rs\0?? n.md\0R  new.rs\0old.rs\0");
        assert_eq!(st.iter().map(|c| (c.path.as_str(), c.code)).collect::<Vec<_>>(), vec![("b.rs", 'M'), ("c.rs", 'M'), ("new.rs", 'R')]);
        assert_eq!(un.iter().map(|c| (c.path.as_str(), c.code)).collect::<Vec<_>>(), vec![("a.rs", 'M'), ("c.rs", 'M'), ("n.md", '?')]);
        let g = parse_graph("* \u{1}abc1234\u{2}HEAD -> main, origin/main\u{2}Fix it\u{2}2 hours ago\n|\\\n");
        assert_eq!(g[0].hash, "abc1234");
        assert_eq!(g[0].refs, vec!["main", "origin/main"]);
        assert_eq!(g[1].lanes, "|\\");
        assert!(g[1].hash.is_empty());
    }

    #[test]
    fn lays_branches_out_in_lanes() {
        let c = |h: &str, p: &[&str]| (h.to_string(), p.iter().map(|s| s.to_string()).collect(), Vec::new(), String::new());
        // m merges a (on main) and f (a feature off b): f gets its own lane, and both meet at b
        let n = lay_out(vec![c("m", &["a", "f"]), c("f", &["b"]), c("a", &["b"]), c("b", &[])]);
        let lane = |h: &str| n.iter().find(|x| x.hash == h).unwrap().lane;
        assert_eq!((lane("m"), lane("a"), lane("b")), (0, 0, 0));
        assert_eq!(lane("f"), 1);
    }

    #[test]
    fn stages_commits_and_lists_branches() {
        let dir = std::env::temp_dir().join(format!("sphere-gitdata-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for a in [&["init", "-q", "-b", "main"][..], &["config", "user.email", "t@t"], &["config", "user.name", "t"]] {
            git(&dir, a).unwrap();
        }
        std::fs::write(dir.join("a.txt"), "1\n").unwrap();
        run(&dir, Op::StageAll).unwrap();
        run(&dir, Op::Commit { msg: "first".into(), amend: false }).unwrap();
        git(&dir, &["branch", "sphere/agent-work"]).unwrap();
        std::fs::write(dir.join("a.txt"), "2\n").unwrap();
        std::fs::write(dir.join("b.txt"), "x\n").unwrap();
        let r = load(&dir);
        assert_eq!(r.error, None);
        assert_eq!((r.card.branch.as_str(), r.card.changed), ("main", 2));
        assert_eq!(r.unstaged.iter().map(|c| (c.path.as_str(), c.code, c.added)).collect::<Vec<_>>(), vec![("a.txt", 'M', Some(1)), ("b.txt", '?', None)]);
        assert!(r.branches.iter().any(|b| b.agent && b.name == "sphere/agent-work"));
        run(&dir, Op::Stage("a.txt".into())).unwrap();
        assert_eq!(load(&dir).staged.len(), 1);
        run(&dir, Op::Unstage("a.txt".into(), None)).unwrap();
        assert!(load(&dir).staged.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sphere-gitdata-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn init(dir: &Path) {
        for a in [&["init", "-q", "-b", "main"][..], &["config", "user.email", "t@t"], &["config", "user.name", "t"], &["config", "commit.gpgsign", "false"]] {
            git(dir, a).unwrap();
        }
    }

    fn commit_file(dir: &Path, name: &str, text: &str) {
        std::fs::write(dir.join(name), text).unwrap();
        run(dir, Op::StageAll).unwrap();
        run(dir, Op::Commit { msg: format!("add {name}"), amend: false }).unwrap();
    }

    #[test]
    fn reads_renames_conflicts_and_untracked_folders() {
        let (st, un) = parse_status("R  new name.rs\0old name.rs\0UU both.rs\0AA also.rs\0?? dir/\0!! target\0 D gone.rs\0");
        assert_eq!(st.len(), 1);
        assert_eq!((st[0].path.as_str(), st[0].code, st[0].from.as_deref()), ("new name.rs", 'R', Some("old name.rs")));
        assert_eq!(un.iter().map(|c| (c.path.as_str(), c.code)).collect::<Vec<_>>(), vec![("both.rs", 'U'), ("also.rs", 'U'), ("dir/", '?'), ("gone.rs", 'D')]);
        assert_eq!(changed_count("MM a.rs\0 M b.rs\0"), 2);
    }

    #[test]
    fn numstat_counts_follow_renames_and_binaries() {
        let v = vec![
            Change { path: "b.rs".into(), code: 'R', from: Some("a.rs".into()), added: None, removed: None },
            Change { path: "img.png".into(), code: 'M', from: None, added: None, removed: None },
            Change { path: "c.rs".into(), code: 'M', from: None, added: None, removed: None },
        ];
        let v = with_counts(v, "2\t1\t\0a.rs\0b.rs\0-\t-\timg.png\x005\t0\tc.rs\0");
        assert_eq!((v[0].added, v[0].removed), (Some(2), Some(1)));
        assert_eq!((v[1].added, v[1].removed), (None, None));
        assert_eq!((v[2].added, v[2].removed), (Some(5), Some(0)));
    }

    #[test]
    fn picks_the_useful_line_of_git_errors() {
        assert_eq!(err_line("To x\n ! [rejected]        main -> main (fetch first)\nerror: failed to push some refs to 'x'\n"), "[rejected]        main -> main (fetch first)");
        assert_eq!(err_line("fatal: not a git repository\n"), "not a git repository");
        assert_eq!(err_line("\n\n"), "git failed");
        assert_eq!(clip_start("crates/sphere-app/src/gitapp.rs", 12), "…c/gitapp.rs");
        assert_eq!(clip_start("short", 12), "short");
    }

    #[test]
    fn a_repo_with_no_commits_loads() {
        let dir = temp("empty");
        init(&dir);
        std::fs::write(dir.join("a.txt"), "x\n").unwrap();
        let r = load(&dir);
        assert_eq!(r.error, None);
        assert!(r.card.empty);
        assert_eq!(r.card.branch, "main");
        assert_eq!(r.branches.len(), 1);
        assert!(r.dag.is_empty() && r.graph.is_empty() && r.last.is_none());
        assert_eq!(r.unstaged.len(), 1);
        // staging and unstaging work before the first commit
        run(&dir, Op::Stage("a.txt".into())).unwrap();
        assert_eq!(load(&dir).staged.len(), 1);
        run(&dir, Op::Unstage("a.txt".into(), None)).unwrap();
        assert!(load(&dir).staged.is_empty());
        assert!(run(&dir, Op::Commit { msg: "  ".into(), amend: false }).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_detached_head_is_named_and_cant_push_or_pull() {
        let dir = temp("detached");
        init(&dir);
        commit_file(&dir, "a.txt", "1\n");
        commit_file(&dir, "b.txt", "2\n");
        git(&dir, &["checkout", "-q", "--detach", "HEAD~1"]).unwrap();
        let r = load(&dir);
        assert!(r.card.detached);
        assert!(r.card.branch.starts_with("detached "));
        assert!(r.branches.iter().all(|b| !b.current));
        assert!(run(&dir, Op::Push).unwrap_err().contains("Detached"));
        assert!(run(&dir, Op::Pull).unwrap_err().contains("Detached"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn renames_and_unstaging_them() {
        let dir = temp("rename");
        init(&dir);
        commit_file(&dir, "old.txt", "one\ntwo\nthree\nfour\n");
        git(&dir, &["mv", "old.txt", "new.txt"]).unwrap();
        let r = load(&dir);
        assert_eq!(r.staged.len(), 1);
        assert_eq!((r.staged[0].path.as_str(), r.staged[0].code, r.staged[0].from.as_deref()), ("new.txt", 'R', Some("old.txt")));
        assert_eq!(r.staged[0].added, Some(0));
        run(&dir, Op::Unstage("new.txt".into(), Some("old.txt".into()))).unwrap();
        let r = load(&dir);
        assert!(r.staged.is_empty());
        assert_eq!(r.card.changed, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn previews_text_binary_and_folders() {
        let dir = temp("preview");
        init(&dir);
        commit_file(&dir, "a.txt", "1\n2\n");
        std::fs::write(dir.join("a.txt"), "1\nchanged\n").unwrap();
        let p = preview(&dir, "a.txt", false, false);
        assert!(p[0].1.starts_with("@@"));
        assert!(p.contains(&('+', "changed".into())) && p.contains(&('-', "2".into())));
        std::fs::write(dir.join("bin.dat"), [0u8, 1, 2]).unwrap();
        assert!(preview(&dir, "bin.dat", false, true)[0].1.contains("binary"));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        assert_eq!(preview(&dir, "sub/", false, true)[0].1, "a new folder");
        // a huge untracked file is read only at its start
        std::fs::write(dir.join("big.txt"), "line\n".repeat(200_000)).unwrap();
        assert_eq!(preview(&dir, "big.txt", false, true).len(), 8);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn commits_amends_and_switches_branches() {
        let dir = temp("commit");
        init(&dir);
        commit_file(&dir, "a.txt", "1\n");
        std::fs::write(dir.join("a.txt"), "2\n").unwrap();
        run(&dir, Op::StageAll).unwrap();
        let note = run(&dir, Op::Commit { msg: "second".into(), amend: true }).unwrap();
        assert!(note.starts_with("Amended"), "{note}");
        assert_eq!(git_t(&dir, &["rev-list", "--count", "HEAD"]).unwrap(), "1");
        assert_eq!(git_t(&dir, &["log", "-1", "--format=%s"]).unwrap(), "second");
        // amending with no new message keeps the old one
        run(&dir, Op::Commit { msg: String::new(), amend: true }).unwrap();
        assert_eq!(git_t(&dir, &["log", "-1", "--format=%s"]).unwrap(), "second");
        // switching: unknown, flag-like, created, existing
        assert!(run(&dir, Op::Switch("nope".into())).unwrap_err().contains("+nope"));
        assert!(run(&dir, Op::Switch("--detach".into())).is_err());
        assert_eq!(run(&dir, Op::Switch("+feature".into())).unwrap(), "Created and switched to feature");
        assert_eq!(load(&dir).card.branch, "feature");
        assert_eq!(run(&dir, Op::Switch("main".into())).unwrap(), "On main");
        // a dirty file the other branch changes blocks the switch with git's own words
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pushes_pulls_and_fetches_against_a_local_remote() {
        let base = temp("remote");
        let bare = base.join("origin.git");
        let a = base.join("a");
        let b = base.join("b");
        std::fs::create_dir_all(&bare).unwrap();
        git(&bare, &["init", "-q", "--bare", "-b", "main"]).unwrap();
        std::fs::create_dir_all(&a).unwrap();
        init(&a);
        assert!(run(&a, Op::Push).unwrap_err().contains("no remote"));
        assert!(run(&a, Op::Fetch).unwrap_err().contains("no remote"));
        commit_file(&a, "a.txt", "1\n");
        git(&a, &["remote", "add", "origin", bare.to_str().unwrap()]).unwrap();
        // no upstream yet: pull says so, push sets it
        assert!(run(&a, Op::Pull).unwrap_err().contains("No upstream"));
        assert_eq!(run(&a, Op::Push).unwrap(), "Pushed main to origin");
        assert_eq!(run(&a, Op::Push).unwrap(), "Nothing to push");
        assert_eq!(load(&a).upstream.as_deref(), Some("origin/main"));
        // a second clone commits; the first fetches, sees it behind, pulls
        git(&base, &["clone", "-q", bare.to_str().unwrap(), "b"]).unwrap();
        git(&b, &["config", "user.email", "t@t"]).unwrap();
        git(&b, &["config", "user.name", "t"]).unwrap();
        commit_file(&b, "b.txt", "2\n");
        run(&b, Op::Push).unwrap();
        assert_eq!(card(&a).behind, 0);
        run(&a, Op::Fetch).unwrap();
        assert_eq!(card(&a).behind, 1);
        assert_eq!(run(&a, Op::Pull).unwrap(), "Pulled 1 commit");
        assert_eq!(run(&a, Op::Pull).unwrap(), "Already up to date");
        // diverged: pull refuses, push is rejected with a readable line
        commit_file(&a, "c.txt", "3\n");
        commit_file(&b, "d.txt", "4\n");
        run(&b, Op::Push).unwrap();
        let e = run(&a, Op::Push).unwrap_err();
        assert!(e.starts_with("Push:") && e.contains("rejected"), "{e}");
        run(&a, Op::Fetch).unwrap();
        assert!(run(&a, Op::Pull).unwrap_err().starts_with("Pull:"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn finds_repos_agents_first_then_projects_by_recency() {
        let base = temp("discover");
        let projects = base.join("Projects");
        let (old, new, plain) = (projects.join("old"), projects.join("new"), projects.join("plain"));
        for d in [&old, &new, &plain] {
            std::fs::create_dir_all(d).unwrap();
        }
        for (d, date) in [(&old, "2020-01-01T00:00:00"), (&new, "2024-01-01T00:00:00")] {
            init(d);
            std::fs::write(d.join("f"), "x").unwrap();
            git(d, &["add", "."]).unwrap();
            let out = Command::new("git").arg("-C").arg(d).args(["commit", "-q", "-m", "x"]).env("GIT_AUTHOR_DATE", date).env("GIT_COMMITTER_DATE", date).output().unwrap();
            assert!(out.status.success());
        }
        let agent = base.join("agent-repo");
        std::fs::create_dir_all(&agent).unwrap();
        init(&agent);
        let found = discover_in(&[agent.to_string_lossy().to_string(), "/nonexistent/repo".into()], Some(&projects), 10);
        assert_eq!(found, vec![agent.clone(), new.clone(), old.clone()]);
        assert_eq!(discover_in(&[], Some(&projects), 1), vec![new]);
        assert!(discover_in(&[], None, 5).is_empty());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_missing_or_plain_folder_is_an_error_not_a_crash() {
        let dir = temp("plain");
        let r = load(&dir);
        assert!(r.error.is_some());
        let r = load(&dir.join("gone"));
        assert_eq!(r.error.as_deref(), Some("The folder is gone"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
