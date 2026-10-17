//! What the Code app reviews: an agent's changes in its worktree against the branch it started from, read with
//! `git` (argv only, never a shell), plus the repo's tests. Everything here blocks, so the app calls it from a
//! background thread.
use std::path::{Path, PathBuf};
use std::process::Command;

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

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output().map_err(|e| format!("git: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(err.lines().next().unwrap_or("git failed").to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Files bigger than this many diff lines show a note instead.
const MAX_LINES: usize = 4000;

pub fn load(workdir: &str, base: &str) -> Review {
    let dir = Path::new(workdir);
    let mut r = Review { workdir: workdir.to_string(), base: base.to_string(), ..Default::default() };
    let fork = match git(dir, &["merge-base", "HEAD", base]) {
        Ok(s) => s.trim().to_string(),
        Err(e) => {
            r.error = Some(e);
            return r;
        }
    };
    r.commits = git(dir, &["log", "--format=%h%x09%s", &format!("{fork}..HEAD")])
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_once('\t').map(|(h, s)| (h.to_string(), s.to_string())))
        .collect();
    // the fork point against the working tree: commits and uncommitted edits together
    let raw = git(dir, &["diff", "--no-color", "--no-ext-diff", "-M", "-U3", &fork]).unwrap_or_default();
    r.files = parse_diff(&raw);
    // new files git doesn't track yet
    let untracked = git(dir, &["ls-files", "--others", "--exclude-standard", "-z"]).unwrap_or_default();
    for p in untracked.split('\0').filter(|p| !p.is_empty()) {
        r.files.push(new_file(dir, p));
    }
    r.dirty = !git(dir, &["status", "--porcelain"]).unwrap_or_default().trim().is_empty();
    r.files.sort_by(|a, b| a.path.cmp(&b.path));
    r
}

fn new_file(dir: &Path, p: &str) -> FileDiff {
    let mut f = FileDiff { path: p.to_string(), status: Status::Added, added: 0, removed: 0, hunks: Vec::new(), note: None };
    match std::fs::read(dir.join(p)) {
        Ok(b) if b.contains(&0) => f.note = Some("binary file".into()),
        Ok(b) => {
            let text = String::from_utf8_lossy(&b);
            let lines: Vec<DiffLine> = text.lines().enumerate().map(|(i, l)| DiffLine { kind: LineKind::Add, old: None, new: Some(i as u32 + 1), text: l.to_string() }).collect();
            f.added = lines.len();
            if lines.len() > MAX_LINES {
                f.note = Some(format!("{} lines, too long to show", lines.len()));
            } else {
                f.hunks.push(Hunk { header: format!("@@ -0,0 +1,{} @@", lines.len()), context: String::new(), new_start: 1, lines });
            }
        }
        Err(e) => f.note = Some(e.to_string()),
    }
    f
}

/// Parses `git diff` output (unified, with git's headers).
pub fn parse_diff(raw: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    let (mut old_no, mut new_no) = (0u32, 0u32);
    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            // "a/x b/x": the b side is the path now
            let path = rest.rsplit_once(" b/").map(|(_, b)| b.to_string()).unwrap_or_else(|| rest.to_string());
            files.push(FileDiff { path, status: Status::Modified, added: 0, removed: 0, hunks: Vec::new(), note: None });
            continue;
        }
        let Some(f) = files.last_mut() else { continue };
        if line.starts_with("new file mode") {
            f.status = Status::Added;
        } else if line.starts_with("deleted file mode") {
            f.status = Status::Deleted;
        } else if line.starts_with("rename to ") {
            f.status = Status::Renamed;
        } else if line.starts_with("Binary files ") {
            f.note = Some("binary file".into());
        } else if let Some(h) = line.strip_prefix("@@ ") {
            let (range, ctx) = h.split_once(" @@").unwrap_or((h, ""));
            let mut it = range.split(' ');
            let start = |s: Option<&str>| s.and_then(|s| s[1..].split(',').next()).and_then(|n| n.parse::<u32>().ok()).unwrap_or(0);
            old_no = start(it.next());
            new_no = start(it.next());
            f.hunks.push(Hunk { header: format!("@@ {range} @@"), context: ctx.trim().to_string(), new_start: new_no, lines: Vec::new() });
        } else if let Some(h) = f.hunks.last_mut() {
            let (kind, text) = match line.chars().next() {
                Some('+') if !line.starts_with("+++") => (LineKind::Add, &line[1..]),
                Some('-') if !line.starts_with("---") => (LineKind::Del, &line[1..]),
                Some(' ') => (LineKind::Context, &line[1..]),
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
            h.lines.push(DiffLine { kind, old: o, new: n, text: text.replace('\t', "    ") });
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
}
