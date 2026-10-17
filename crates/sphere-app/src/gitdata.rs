//! What the Git app shows and does, read with `git` (argv only, never a shell). Everything here blocks, so the
//! app calls it from background threads.
use std::path::{Path, PathBuf};
use std::process::Command;

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output().map_err(|e| format!("git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let e = String::from_utf8_lossy(&out.stderr);
        Err(e.lines().find(|l| !l.trim().is_empty()).unwrap_or("git failed").trim().to_string())
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RepoCard {
    pub name: String,
    pub path: String,
    pub lang: String,
    pub branch: String,
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

#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub path: String,
    /// M, A, D, R or ? (untracked)
    pub code: char,
    pub added: Option<u64>,
    pub removed: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Repo {
    pub card: RepoCard,
    pub branches: Vec<Branch>,
    pub graph: Vec<GraphRow>,
    pub staged: Vec<Change>,
    pub unstaged: Vec<Change>,
    pub upstream: Option<String>,
    pub remote_url: String,
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

pub fn card(dir: &Path) -> RepoCard {
    let (ahead, behind) = track(dir);
    RepoCard {
        name: dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
        path: dir.to_string_lossy().to_string(),
        lang: lang(dir).to_string(),
        branch: git(dir, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_default().trim().to_string(),
        ahead,
        behind,
        changed: git(dir, &["status", "--porcelain"]).unwrap_or_default().lines().count(),
        stashes: git(dir, &["stash", "list"]).unwrap_or_default().lines().count(),
    }
}

/// The repos to show: the ones agents work in first, then git repos under ~/Projects, most recently committed
/// first.
pub fn discover(agent_repos: &[String], max: usize) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut push = |p: PathBuf| {
        if p.join(".git").exists() && !out.contains(&p) {
            out.push(p);
        }
    };
    for r in agent_repos {
        push(PathBuf::from(r));
    }
    let projects = sphere_proto::paths::default_state_dir().parent().map(|h| h.join("Projects"));
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

pub fn load(dir: &Path) -> Repo {
    let mut r = Repo { card: card(dir), ..Default::default() };
    if let Err(e) = git(dir, &["rev-parse", "--git-dir"]) {
        r.error = Some(e);
        return r;
    }
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
            Some(Branch { current: name == cur, agent: name.starts_with("sphere/") || name.starts_with("claude/"), ahead: num("ahead "), behind: num("behind "), name, age })
        })
        .collect();
    r.graph = parse_graph(&git(dir, &["log", "--graph", "--all", "--date-order", "-n", "40", "--format=%x01%h%x02%D%x02%s%x02%ar"]).unwrap_or_default());
    let (staged, unstaged) = parse_status(&git(dir, &["status", "--porcelain=v1", "-z"]).unwrap_or_default());
    r.staged = with_counts(staged, &git(dir, &["diff", "--cached", "--numstat", "-z"]).unwrap_or_default());
    r.unstaged = with_counts(unstaged, &git(dir, &["diff", "--numstat", "-z"]).unwrap_or_default());
    r.upstream = git(dir, &["rev-parse", "--abbrev-ref", "@{upstream}"]).ok().map(|s| s.trim().to_string());
    r.remote_url = git(dir, &["remote", "get-url", "origin"]).unwrap_or_default().trim().to_string();
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

/// `git status --porcelain=v1 -z`: (staged, unstaged), untracked files counting as unstaged.
pub fn parse_status(raw: &str) -> (Vec<Change>, Vec<Change>) {
    let (mut st, mut un) = (Vec::new(), Vec::new());
    let mut it = raw.split('\0').filter(|s| s.len() > 3);
    while let Some(e) = it.next() {
        let (x, y) = (e.as_bytes()[0] as char, e.as_bytes()[1] as char);
        let path = e[3..].to_string();
        if x == 'R' || x == 'C' {
            it.next(); // the old name
        }
        let ch = |code: char| Change { path: path.clone(), code, added: None, removed: None };
        if x == '?' {
            un.push(ch('?'));
            continue;
        }
        if x != ' ' {
            st.push(ch(x));
        }
        if y != ' ' {
            un.push(ch(y));
        }
    }
    (st, un)
}

/// Adds `git diff --numstat -z` counts to the changes they belong to.
fn with_counts(mut v: Vec<Change>, numstat: &str) -> Vec<Change> {
    for rec in numstat.split('\0') {
        let f: Vec<&str> = rec.splitn(3, '\t').collect();
        if f.len() == 3 {
            if let Some(c) = v.iter_mut().find(|c| c.path == f[2]) {
                c.added = f[0].parse().ok();
                c.removed = f[1].parse().ok();
            }
        }
    }
    v
}

/// The first lines of a file's change, for the preview under the working tree.
pub fn preview(dir: &Path, path: &str, staged: bool, untracked: bool) -> Vec<(char, String)> {
    if untracked {
        return std::fs::read_to_string(dir.join(path)).unwrap_or_default().lines().take(8).map(|l| ('+', l.to_string())).collect();
    }
    let args: Vec<&str> = if staged { vec!["diff", "--cached", "--no-color", "-U1", "--", path] } else { vec!["diff", "--no-color", "-U1", "--", path] };
    let raw = git(dir, &args).unwrap_or_default();
    raw.lines()
        .skip_while(|l| !l.starts_with("@@"))
        .take(9)
        .map(|l| match l.chars().next() {
            Some(c @ ('+' | '-')) => (c, l[1..].to_string()),
            Some('@') => ('@', l.to_string()),
            _ => (' ', l.get(1..).unwrap_or("").to_string()),
        })
        .collect()
}

pub enum Op {
    Stage(String),
    Unstage(String),
    StageAll,
    UnstageAll,
    Commit { msg: String, amend: bool },
    Switch(String),
    Pull,
    Push,
}

/// Runs one action and says what happened.
pub fn run(dir: &Path, op: Op) -> Result<String, String> {
    match op {
        Op::Stage(p) => git(dir, &["add", "--", &p]).map(|_| format!("Staged {p}")),
        Op::Unstage(p) => git(dir, &["restore", "--staged", "--", &p]).or_else(|_| git(dir, &["rm", "--cached", "-q", "--", &p])).map(|_| format!("Unstaged {p}")),
        Op::StageAll => git(dir, &["add", "-A"]).map(|_| "Staged everything".into()),
        Op::UnstageAll => git(dir, &["reset", "-q"]).map(|_| "Unstaged everything".into()),
        Op::Commit { msg, amend } => {
            let mut a = vec!["commit", "-q", "-m", msg.as_str()];
            if amend {
                a.push("--amend");
            }
            git(dir, &a)?;
            let h = git(dir, &["log", "-1", "--format=%h"]).unwrap_or_default();
            Ok(format!("Committed {} · {}", h.trim(), msg))
        }
        Op::Switch(b) => git(dir, &["switch", &b]).map(|_| format!("On {b}")),
        Op::Pull => git(dir, &["pull", "--ff-only", "-q"]).map(|_| "Pulled".into()).map_err(|e| format!("Pull: {e}")),
        Op::Push => {
            let has_up = git(dir, &["rev-parse", "--abbrev-ref", "@{upstream}"]).is_ok();
            let r = if has_up { git(dir, &["push", "-q"]) } else { git(dir, &["push", "-q", "-u", "origin", "HEAD"]) };
            r.map(|_| "Pushed".into()).map_err(|e| format!("Push: {e}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        run(&dir, Op::Unstage("a.txt".into())).unwrap();
        assert!(load(&dir).staged.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
