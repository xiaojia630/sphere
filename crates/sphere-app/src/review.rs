//! What the Code app reviews: an agent's changes in its worktree against the branch it started from, read with
//! `git` (argv only, never a shell), plus the repo's tests. Everything here blocks, so the app calls it from a
//! background thread.
use crate::gitdata::{git, git_t};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Add,
    Del,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DiffLine {
    pub kind: LineKind,
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Hunk {
    /// the `@@ -a,b +c,d @@` line
    pub header: String,
    /// what git names the enclosing function or block, after the `@@`
    pub context: String,
    pub new_start: u32,
    pub lines: Vec<DiffLine>,
}

impl Hunk {
    pub fn added(&self) -> usize {
        self.lines.iter().filter(|l| l.kind == LineKind::Add).count()
    }
    pub fn removed(&self) -> usize {
        self.lines.iter().filter(|l| l.kind == LineKind::Del).count()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Added,
    Modified,
    Deleted,
    Renamed,
}

impl Status {
    pub fn letter(self) -> &'static str {
        match self {
            Status::Added => "A",
            Status::Modified => "M",
            Status::Deleted => "D",
            Status::Renamed => "R",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FileDiff {
    pub path: String,
    /// a renamed file's old path
    pub from: Option<String>,
    pub status: Status,
    pub added: usize,
    pub removed: usize,
    pub hunks: Vec<Hunk>,
    /// binary, or too big to show
    pub note: Option<String>,
}

/// One task's changes: committed on its branch since it forked, plus what's uncommitted in the worktree.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Review {
    pub workdir: String,
    pub base: String,
    pub files: Vec<FileDiff>,
    /// commits on the branch since the base
    pub commits: Vec<(String, String)>,
    pub dirty: bool,
    /// the branch is already part of the base
    pub merged: bool,
    /// new files left out of the list (past `MAX_NEW`)
    pub more: usize,
    pub error: Option<String>,
}

impl Review {
    pub fn added(&self) -> usize {
        self.files.iter().map(|f| f.added).sum()
    }
    pub fn removed(&self) -> usize {
        self.files.iter().map(|f| f.removed).sum()
    }
}

/// Files bigger than this many diff lines show a note instead.
const MAX_LINES: usize = 4000;

/// The empty tree: the "fork point" of a repo with no commits.
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
/// Untracked files listed (a repo with an unignored `node_modules` would otherwise list thousands).
const MAX_NEW: usize = 300;
/// Untracked files bigger than this many bytes aren't read.
const MAX_BYTES: u64 = 1_000_000;
/// Longest diff line kept; the rest is cut.
const MAX_LINE: usize = 400;

pub fn load(workdir: &str, base: &str) -> Review {
    let dir = Path::new(workdir);
    let mut r = Review { workdir: workdir.to_string(), base: base.to_string(), ..Default::default() };
    if !dir.is_dir() {
        r.error = Some("The worktree is gone (discarded or removed)".into());
        return r;
    }
    let has_head = git(dir, &["rev-parse", "--verify", "-q", "HEAD"]).is_ok();
    let fork = if !has_head {
        EMPTY_TREE.to_string()
    } else {
        // the base may only exist on the remote, or not at all (then the uncommitted work is what there is)
        let found = [base.to_string(), format!("origin/{base}")].into_iter().find_map(|b| git_t(dir, &["merge-base", "HEAD", &b]).ok());
        match found {
            Some(f) => f,
            None => {
                if git(dir, &["rev-parse", "--is-inside-work-tree"]).is_err() {
                    r.error = Some("Not a git worktree".into());
                    return r;
                }
                r.error = Some(format!("No base branch {base} to compare with"));
                return r;
            }
        }
    };
    if has_head {
        r.commits = git(dir, &["log", "--format=%h%x09%s", &format!("{fork}..HEAD")])
            .unwrap_or_default()
            .lines()
            .filter_map(|l| l.split_once('\t').map(|(h, s)| (h.to_string(), s.to_string())))
            .collect();
        let head = git_t(dir, &["rev-parse", "HEAD"]).unwrap_or_default();
        r.merged = !head.is_empty() && head != fork && git(dir, &["merge-base", "--is-ancestor", "HEAD", base]).is_ok();
    }
    // the fork point against the working tree: commits and uncommitted edits together
    let raw = git(dir, &["diff", "--no-color", "--no-ext-diff", "-M", "-U3", &fork]).unwrap_or_default();
    r.files = parse_diff(&raw);
    // new files git doesn't track yet
    let untracked = git(dir, &["ls-files", "--others", "--exclude-standard", "-z"]).unwrap_or_default();
    let new: Vec<&str> = untracked.split('\0').filter(|p| !p.is_empty()).collect();
    r.more = new.len().saturating_sub(MAX_NEW);
    for p in new.into_iter().take(MAX_NEW) {
        r.files.push(new_file(dir, p));
    }
    r.dirty = !git(dir, &["status", "--porcelain"]).unwrap_or_default().trim().is_empty();
    r.files.sort_by(|a, b| a.path.cmp(&b.path));
    r
}

fn new_file(dir: &Path, p: &str) -> FileDiff {
    let mut f = FileDiff { path: p.to_string(), from: None, status: Status::Added, added: 0, removed: 0, hunks: Vec::new(), note: None };
    let full = dir.join(p);
    let meta = std::fs::symlink_metadata(&full);
    match meta {
        Ok(m) if m.file_type().is_symlink() => f.note = Some("a symbolic link".into()),
        Ok(m) if m.is_dir() => f.note = Some("a folder (another repo or a submodule)".into()),
        Ok(m) if m.len() > MAX_BYTES => f.note = Some(format!("{} KB, too big to show", m.len() / 1000)),
        Ok(_) => match std::fs::read(&full) {
            Ok(b) if b.contains(&0) => f.note = Some("binary file".into()),
            Ok(b) => {
                let text = String::from_utf8_lossy(&b);
                let lines: Vec<DiffLine> = text.lines().enumerate().map(|(i, l)| DiffLine { kind: LineKind::Add, old: None, new: Some(i as u32 + 1), text: cut(l) }).collect();
                f.added = lines.len();
                if lines.len() > MAX_LINES {
                    f.note = Some(format!("{} lines, too long to show", lines.len()));
                } else if !lines.is_empty() {
                    f.hunks.push(Hunk { header: format!("@@ -0,0 +1,{} @@", lines.len()), context: String::new(), new_start: 1, lines });
                }
            }
            Err(e) => f.note = Some(e.to_string()),
        },
        Err(e) => f.note = Some(e.to_string()),
    }
    f
}

/// A diff line cut to `MAX_LINE` characters, tabs as spaces.
fn cut(s: &str) -> String {
    let s = s.replace('\t', "    ");
    if s.chars().count() > MAX_LINE {
        s.chars().take(MAX_LINE).collect::<String>() + "…"
    } else {
        s
    }
}

/// The path out of a `--- a/x` or `+++ b/x` line (`None` for /dev/null).
fn header_path(rest: &str) -> Option<String> {
    let p = rest.trim_end_matches('\t');
    if p == "/dev/null" {
        return None;
    }
    Some(p.strip_prefix("a/").or_else(|| p.strip_prefix("b/")).unwrap_or(p).to_string())
}

/// Parses `git diff` output (unified, with git's headers).
pub fn parse_diff(raw: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    let (mut old_no, mut new_no) = (0u32, 0u32);
    // file headers (---, +++) only come before a file's first hunk: inside one, a line starting with ---
    // is a removed line that starts with --
    let mut in_hunk = false;
    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            // "a/x b/x": the b side is the path now (the +++ and rename lines correct it for odd names)
            let path = rest.rsplit_once(" b/").map(|(_, b)| b.to_string()).unwrap_or_else(|| rest.to_string());
            files.push(FileDiff { path, from: None, status: Status::Modified, added: 0, removed: 0, hunks: Vec::new(), note: None });
            in_hunk = false;
            continue;
        }
        let Some(f) = files.last_mut() else { continue };
        if !in_hunk {
            if line.starts_with("new file mode") {
                f.status = Status::Added;
            } else if line.starts_with("deleted file mode") {
                f.status = Status::Deleted;
            } else if let Some(p) = line.strip_prefix("rename from ") {
                f.from = Some(p.to_string());
            } else if let Some(p) = line.strip_prefix("rename to ") {
                f.status = Status::Renamed;
                f.path = p.to_string();
            } else if line.starts_with("Binary files ") || line.starts_with("GIT binary patch") {
                f.note = Some("binary file".into());
            } else if let Some(p) = line.strip_prefix("+++ ") {
                if let Some(p) = header_path(p) {
                    f.path = p;
                }
            } else if let Some(p) = line.strip_prefix("--- ") {
                if f.status == Status::Deleted {
                    if let Some(p) = header_path(p) {
                        f.path = p;
                    }
                }
            }
        }
        if let Some(h) = line.strip_prefix("@@ ") {
            in_hunk = true;
            let (range, ctx) = h.split_once(" @@").unwrap_or((h, ""));
            let mut it = range.split(' ');
            let start = |s: Option<&str>| s.and_then(|s| s.get(1..)).and_then(|s| s.split(',').next()).and_then(|n| n.parse::<u32>().ok()).unwrap_or(0);
            old_no = start(it.next());
            new_no = start(it.next());
            f.hunks.push(Hunk { header: format!("@@ {range} @@"), context: ctx.trim().to_string(), new_start: new_no, lines: Vec::new() });
        } else if in_hunk {
            let Some(h) = f.hunks.last_mut() else { continue };
            let (kind, text) = match line.chars().next() {
                Some('+') => (LineKind::Add, &line[1..]),
                Some('-') => (LineKind::Del, &line[1..]),
                Some(' ') => (LineKind::Context, &line[1..]),
                // an empty context line some tools strip the space from
                None => (LineKind::Context, ""),
                _ => continue,
            };
            let (o, n) = match kind {
                LineKind::Add => {
                    new_no += 1;
                    (None, Some(new_no - 1))
                }
                LineKind::Del => {
                    old_no += 1;
                    (Some(old_no - 1), None)
                }
                LineKind::Context => {
                    old_no += 1;
                    new_no += 1;
                    (Some(old_no - 1), Some(new_no - 1))
                }
            };
            match kind {
                LineKind::Add => f.added += 1,
                LineKind::Del => f.removed += 1,
                LineKind::Context => {}
            }
            h.lines.push(DiffLine { kind, old: o, new: n, text: cut(text) });
        }
    }
    for f in &mut files {
        if f.hunks.iter().map(|h| h.lines.len()).sum::<usize>() > MAX_LINES {
            f.note = Some(format!("{} changed lines, too long to show", f.added + f.removed));
            f.hunks.clear();
        }
    }
    files
}

// ------------------------------------------------------------------ tests

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TestState {
    Pass,
    Fail,
    Ignored,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TestRun {
    pub command: String,
    /// (name, state) in the order cargo printed them
    pub tests: Vec<(String, TestState)>,
    pub passed: usize,
    pub failed: usize,
    pub done: bool,
    /// it couldn't run, or the build failed (the first error line)
    pub error: Option<String>,
}

impl TestRun {
    /// Tests grouped by module (`core::sched::x` → `core::sched`): (group, passed, total, any failed).
    pub fn groups(&self) -> Vec<(String, usize, usize, bool)> {
        let mut out: Vec<(String, usize, usize, bool)> = Vec::new();
        for (name, st) in &self.tests {
            let g = name.rsplit_once("::").map(|(m, _)| m.trim_start_matches("tests::").trim_end_matches("::tests").to_string()).unwrap_or_else(|| name.clone());
            // a crate's own `mod tests` reads as the crate root
            let g = if g == "tests" { "lib".to_string() } else { g };
            let i = match out.iter().position(|o| o.0 == g) {
                Some(i) => i,
                None => {
                    out.push((g, 0, 0, false));
                    out.len() - 1
                }
            };
            out[i].2 += 1;
            match st {
                TestState::Pass => out[i].1 += 1,
                TestState::Fail => out[i].3 = true,
                TestState::Ignored => {}
            }
        }
        out
    }
}

/// The test command for a repo, when we know one.
pub fn test_command(workdir: &str) -> Option<Vec<String>> {
    let d = PathBuf::from(workdir);
    if d.join("Cargo.toml").exists() {
        return Some(vec!["cargo".into(), "test".into(), "--color".into(), "never".into(), "--no-fail-fast".into()]);
    }
    None
}

/// Folds one line of `cargo test` output into the run.
pub fn test_line(run: &mut TestRun, line: &str) {
    let l = line.trim();
    if let Some(rest) = l.strip_prefix("test ") {
        if let Some((name, res)) = rest.rsplit_once(" ... ") {
            let st = match res.trim() {
                "ok" => TestState::Pass,
                r if r.starts_with("ignored") => TestState::Ignored,
                r if r.starts_with("FAILED") => TestState::Fail,
                _ => return,
            };
            match st {
                TestState::Pass => run.passed += 1,
                TestState::Fail => run.failed += 1,
                TestState::Ignored => {}
            }
            run.tests.push((name.to_string(), st));
        }
    } else if (l.starts_with("error[") || l.starts_with("error:")) && run.error.is_none() && !l.starts_with("error: test failed") && !l.starts_with("error: 1 target failed") {
        run.error = Some(l.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    const DIFF: &str = "diff --git a/src/core.rs b/src/core.rs
index 1..2 100644
--- a/src/core.rs
+++ b/src/core.rs
@@ -139,7 +139,11 @@ impl Scheduler
     pub fn tick(&mut self) {
-        self.frames += 1;
+        self.frames = self.frames.wrapping_add(1);
+        if budget.is_spent() {
+            return;
+        }
         let _guard = self.lock.lock();
diff --git a/src/budget.rs b/src/budget.rs
new file mode 100644
--- /dev/null
+++ b/src/budget.rs
@@ -0,0 +1,2 @@
+pub struct FrameBudget;
+impl FrameBudget {}
";

    #[test]
    fn parses_files_hunks_and_line_numbers() {
        let f = parse_diff(DIFF);
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].path, "src/core.rs");
        assert_eq!((f[0].status, f[0].added, f[0].removed), (Status::Modified, 4, 1));
        let h = &f[0].hunks[0];
        assert_eq!(h.context, "impl Scheduler");
        assert_eq!(h.new_start, 139);
        assert_eq!((h.lines[0].old, h.lines[0].new), (Some(139), Some(139)));
        assert_eq!((h.lines[1].kind, h.lines[1].old), (LineKind::Del, Some(140)));
        assert_eq!((h.lines[2].kind, h.lines[2].new), (LineKind::Add, Some(140)));
        assert_eq!(h.lines.last().map(|l| (l.old, l.new)), Some((Some(141), Some(144))));
        assert_eq!((f[1].status, f[1].added), (Status::Added, 2));
    }

    #[test]
    fn reads_cargo_test_output() {
        let mut r = TestRun::default();
        for l in ["test core::sched::tests::ticks ... ok", "test core::budget::tests::spent ... FAILED", "test net::sync ... ignored", "test result: FAILED. 1 passed; 1 failed"] {
            test_line(&mut r, l);
        }
        assert_eq!((r.passed, r.failed, r.tests.len()), (1, 1, 3));
        let g = r.groups();
        assert_eq!(g[0], ("core::sched".to_string(), 1, 1, false));
        assert_eq!(g[1], ("core::budget".to_string(), 0, 1, true));
    }

    #[test]
    fn reviews_a_worktree_against_its_base() {
        let dir = std::env::temp_dir().join(format!("sphere-review-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let g = |args: &[&str]| assert!(Command::new("git").arg("-C").arg(&dir).args(args).output().unwrap().status.success(), "{args:?}");
        g(&["init", "-q", "-b", "main"]);
        g(&["config", "user.email", "t@t"]);
        g(&["config", "user.name", "t"]);
        std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
        g(&["add", "."]);
        g(&["commit", "-q", "-m", "base"]);
        g(&["checkout", "-q", "-b", "agent"]);
        std::fs::write(dir.join("a.txt"), "one\n2\n").unwrap();
        g(&["commit", "-q", "-am", "edit"]);
        std::fs::write(dir.join("new.txt"), "fresh\n").unwrap();
        let r = load(dir.to_str().unwrap(), "main");
        assert_eq!(r.error, None);
        assert_eq!(r.commits.len(), 1);
        assert!(r.dirty);
        let paths: Vec<_> = r.files.iter().map(|f| (f.path.as_str(), f.status, f.added, f.removed)).collect();
        assert_eq!(paths, vec![("a.txt", Status::Modified, 1, 1), ("new.txt", Status::Added, 1, 0)]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sphere-review-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn g(dir: &Path, args: &[&str]) {
        let out = Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
        assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    fn init(dir: &Path) {
        g(dir, &["init", "-q", "-b", "main"]);
        g(dir, &["config", "user.email", "t@t"]);
        g(dir, &["config", "user.name", "t"]);
        g(dir, &["config", "commit.gpgsign", "false"]);
    }

    #[test]
    fn removed_lines_that_start_with_dashes_stay_in_the_hunk() {
        let raw = "diff --git a/q.sql b/q.sql
index 1..2 100644
--- a/q.sql
+++ b/q.sql
@@ -1,3 +1,3 @@
 select 1;
--- a comment
+++ another comment
 select 2;
\\ No newline at end of file
";
        let f = parse_diff(raw);
        assert_eq!(f.len(), 1);
        assert_eq!((f[0].added, f[0].removed), (1, 1));
        assert_eq!(f[0].hunks[0].lines[1].text, "-- a comment");
        assert_eq!(f[0].hunks[0].lines[1].kind, LineKind::Del);
        assert_eq!(f[0].hunks[0].lines[2].text, "++ another comment");
        assert_eq!(f[0].hunks[0].lines.len(), 4);
    }

    #[test]
    fn reads_deletes_renames_binaries_and_odd_names() {
        let raw = "diff --git a/gone.rs b/gone.rs
deleted file mode 100644
index 1..0
--- a/gone.rs
+++ /dev/null
@@ -1,2 +0,0 @@
-a
-b
diff --git a/old name.rs b/new b/name.rs
similarity index 90%
rename from old name.rs
rename to new b/name.rs
index 1..2 100644
--- a/old name.rs
+++ b/new b/name.rs\t
@@ -1 +1 @@
-x
+y
diff --git a/img.png b/img.png
index 1..2 100644
Binary files a/img.png and b/img.png differ
";
        let f = parse_diff(raw);
        assert_eq!(f.len(), 3);
        assert_eq!((f[0].path.as_str(), f[0].status, f[0].removed, f[0].hunks[0].new_start), ("gone.rs", Status::Deleted, 2, 0));
        assert_eq!((f[1].path.as_str(), f[1].status, f[1].from.as_deref()), ("new b/name.rs", Status::Renamed, Some("old name.rs")));
        assert_eq!((f[1].added, f[1].removed), (1, 1));
        assert_eq!((f[2].path.as_str(), f[2].note.as_deref()), ("img.png", Some("binary file")));
    }

    #[test]
    fn very_long_lines_and_huge_diffs_are_cut() {
        let long = "x".repeat(5000);
        let raw = format!("diff --git a/m.js b/m.js\n--- a/m.js\n+++ b/m.js\n@@ -1 +1 @@\n-a\n+{long}\n");
        let f = parse_diff(&raw);
        assert!(f[0].hunks[0].lines[1].text.chars().count() <= MAX_LINE + 1);
        let mut big = String::from("diff --git a/b.txt b/b.txt\n--- a/b.txt\n+++ b/b.txt\n@@ -0,0 +1,5000 @@\n");
        for i in 0..5000 {
            big.push_str(&format!("+{i}\n"));
        }
        let f = parse_diff(&big);
        assert!(f[0].hunks.is_empty());
        assert!(f[0].note.as_deref().unwrap().contains("too long"));
    }

    #[test]
    fn a_repo_with_no_commits_shows_its_new_files() {
        let dir = temp("nocommit");
        init(&dir);
        std::fs::write(dir.join("a.txt"), "hello\n").unwrap();
        let r = load(dir.to_str().unwrap(), "main");
        assert_eq!(r.error, None);
        assert_eq!(r.files.iter().map(|f| (f.path.as_str(), f.added)).collect::<Vec<_>>(), vec![("a.txt", 1)]);
        assert!(r.commits.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_base_or_worktree_is_a_readable_error() {
        let dir = temp("nobase");
        init(&dir);
        std::fs::write(dir.join("a.txt"), "1\n").unwrap();
        g(&dir, &["add", "."]);
        g(&dir, &["commit", "-q", "-m", "one"]);
        let r = load(dir.to_str().unwrap(), "develop");
        assert_eq!(r.error.as_deref(), Some("No base branch develop to compare with"));
        let r = load(dir.join("gone").to_str().unwrap(), "main");
        assert!(r.error.unwrap().contains("gone"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn notices_a_branch_that_is_already_merged() {
        let dir = temp("merged");
        init(&dir);
        std::fs::write(dir.join("a.txt"), "1\n").unwrap();
        g(&dir, &["add", "."]);
        g(&dir, &["commit", "-q", "-m", "one"]);
        g(&dir, &["checkout", "-q", "-b", "agent"]);
        std::fs::write(dir.join("a.txt"), "2\n").unwrap();
        g(&dir, &["commit", "-q", "-am", "two"]);
        let r = load(dir.to_str().unwrap(), "main");
        assert!(!r.merged);
        assert_eq!(r.files.len(), 1);
        g(&dir, &["checkout", "-q", "main"]);
        g(&dir, &["merge", "-q", "--ff-only", "agent"]);
        g(&dir, &["checkout", "-q", "agent"]);
        let r = load(dir.to_str().unwrap(), "main");
        assert!(r.merged || r.files.is_empty());
        assert!(r.files.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn untracked_files_are_bounded_and_odd_ones_get_a_note() {
        let dir = temp("untracked");
        init(&dir);
        std::fs::write(dir.join("a.txt"), "1\n").unwrap();
        g(&dir, &["add", "."]);
        g(&dir, &["commit", "-q", "-m", "one"]);
        std::fs::write(dir.join("blob.bin"), [0u8, 1, 2, 3]).unwrap();
        std::fs::write(dir.join("big.txt"), "x".repeat(1_500_000)).unwrap();
        std::fs::write(dir.join("empty.txt"), "").unwrap();
        for i in 0..MAX_NEW + 20 {
            std::fs::write(dir.join(format!("f{i:04}.txt")), "x\n").unwrap();
        }
        let r = load(dir.to_str().unwrap(), "main");
        assert_eq!(r.files.len(), MAX_NEW);
        assert!(r.more > 0);
        let _ = std::fs::remove_dir_all(&dir);
        let f = new_file(&temp("odd"), "missing.txt");
        assert!(f.note.is_some());
        let d = temp("odd2");
        std::fs::write(d.join("blob.bin"), [0u8, 1]).unwrap();
        std::fs::write(d.join("big.txt"), "x".repeat(1_500_000)).unwrap();
        std::fs::write(d.join("empty.txt"), "").unwrap();
        assert_eq!(new_file(&d, "blob.bin").note.as_deref(), Some("binary file"));
        assert!(new_file(&d, "big.txt").note.as_deref().unwrap().contains("too big"));
        let e = new_file(&d, "empty.txt");
        assert!(e.note.is_none() && e.hunks.is_empty());
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(temp("odd"));
    }

    #[test]
    fn header_paths_lose_prefixes_and_tabs() {
        assert_eq!(header_path("b/src/a b.rs\t").as_deref(), Some("src/a b.rs"));
        assert_eq!(header_path("/dev/null"), None);
    }
}
