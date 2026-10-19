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

/// The branch the repo's main checkout is on (the first worktree git lists), if `dir` is another worktree of it.
pub fn main_branch(dir: &Path) -> Option<String> {
    let out = git(dir, &["worktree", "list", "--porcelain"]).ok()?;
    let first = out.split("\n\n").next()?;
    // a checkout that is itself the main one has no other branch to be merged into
    let main = std::fs::canonicalize(first.lines().find_map(|l| l.strip_prefix("worktree "))?).ok()?;
    if std::fs::canonicalize(git_t(dir, &["rev-parse", "--show-toplevel"]).ok()?).ok()? == main {
        return None;
    }
    first.lines().find_map(|l| l.strip_prefix("branch refs/heads/")).map(str::to_string)
}

/// Whether HEAD in `dir` has commits since `fork` and they are all in `base`. The base is a commit when sphered
/// made the worktree: then "merged" means taken into the branch the main checkout is on.
fn merged_from(dir: &Path, fork: &str, base: &str) -> bool {
    let head = git_t(dir, &["rev-parse", "HEAD"]).unwrap_or_default();
    !head.is_empty()
        && head != fork
        && (git(dir, &["merge-base", "--is-ancestor", "HEAD", base]).is_ok()
            || main_branch(dir).is_some_and(|b| git(dir, &["merge-base", "--is-ancestor", "HEAD", &b]).is_ok()))
}

/// Whether an agent's worktree has work of its own that is already merged (a quick check, no diff read).
pub fn is_merged(workdir: &str, base: &str) -> bool {
    let dir = Path::new(workdir);
    dir.is_dir() && git_t(dir, &["merge-base", "HEAD", base]).is_ok_and(|fork| merged_from(dir, &fork, base))
}

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
        r.merged = merged_from(dir, &fork, base);
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
    /// (name, state) in the order cargo printed them (other runners only give counts)
    pub tests: Vec<(String, TestState)>,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub done: bool,
    /// it couldn't run, or the build failed (the first error line)
    pub error: Option<String>,
    /// the last line that looked like an error, and the last line at all
    pub note: Option<String>,
    pub last: Option<String>,
    /// stopped by the user (once `done`), or being stopped now
    pub cancelled: bool,
    pub cancelling: bool,
    pub timed_out: bool,
    /// how the process ended (None while it runs, or when it was killed)
    pub exit_ok: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunStatus {
    Running,
    Cancelling,
    Cancelled,
    TimedOut,
    /// no result came out: it didn't build, or didn't start
    CouldNotRun,
    Failed,
    Passed,
}

impl TestRun {
    pub fn total(&self) -> usize {
        self.tests.len().max(self.passed + self.failed + self.skipped)
    }
    pub fn status(&self) -> RunStatus {
        if !self.done {
            if self.cancelling { RunStatus::Cancelling } else { RunStatus::Running }
        } else if self.cancelled {
            RunStatus::Cancelled
        } else if self.timed_out {
            RunStatus::TimedOut
        } else if self.error.is_some() && self.total() == 0 {
            RunStatus::CouldNotRun
        } else if self.failed > 0 || self.exit_ok == Some(false) {
            RunStatus::Failed
        } else {
            RunStatus::Passed
        }
    }
    /// Cell `i` of `n` in the progress strip: a named test's state, or from the counts when the runner gives only those.
    pub fn cell(&self, i: usize, n: usize) -> Option<TestState> {
        let total = self.total();
        if total == 0 {
            return None;
        }
        let k = i * total / n.max(1);
        if !self.tests.is_empty() {
            return self.tests.get(k).map(|x| x.1);
        }
        Some(if k < self.passed { TestState::Pass } else if k < self.passed + self.failed { TestState::Fail } else { TestState::Ignored })
    }
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

/// How long a test run may take before it is stopped, and how long a stopped one gets to leave before it is killed.
pub const TEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15 * 60);
pub const KILL_GRACE: std::time::Duration = std::time::Duration::from_secs(3);

/// The test command for a repo, when we know one.
pub fn test_command(workdir: &str) -> Option<Vec<String>> {
    detect_tests(workdir).ok()
}

/// Whether `name` is an executable on PATH.
pub fn on_path(name: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(name).metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)))
}

pub fn detect_tests(workdir: &str) -> Result<Vec<String>, String> {
    detect_tests_with(workdir, &on_path)
}

/// Looks for a runner in the worktree (cargo, npm/pnpm/yarn/bun, go, pytest, rspec, mix, make) whose tool is there.
/// With none, the error says what was found but missing, or that nothing was recognised.
pub fn detect_tests_with(workdir: &str, has: &dyn Fn(&str) -> bool) -> Result<Vec<String>, String> {
    let d = PathBuf::from(workdir);
    let v = |a: &[&str]| a.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let mut missing: Option<String> = None;
    let mut pick = |marker: &str, tool: &str, argv: Vec<String>| -> Option<Vec<String>> {
        if has(tool) {
            return Some(argv);
        }
        missing.get_or_insert_with(|| format!("{marker} is here, but {tool} isn't installed or isn't on PATH"));
        None
    };
    if d.join("Cargo.toml").exists() {
        if let Some(c) = pick("Cargo.toml", "cargo", v(&["cargo", "test", "--color", "never", "--no-fail-fast"])) {
            return Ok(c);
        }
    }
    if let Ok(text) = std::fs::read_to_string(d.join("package.json")) {
        let script = serde_json::from_str::<serde_json::Value>(&text).ok().and_then(|j| j["scripts"]["test"].as_str().map(str::to_string));
        if script.is_some_and(|t| !t.trim().is_empty() && !t.contains("no test specified")) {
            let (tool, argv) = if d.join("bun.lockb").exists() || d.join("bun.lock").exists() {
                ("bun", v(&["bun", "run", "test"]))
            } else if d.join("pnpm-lock.yaml").exists() {
                ("pnpm", v(&["pnpm", "test"]))
            } else if d.join("yarn.lock").exists() {
                ("yarn", v(&["yarn", "test"]))
            } else {
                ("npm", v(&["npm", "test"]))
            };
            if let Some(c) = pick("package.json's test script", tool, argv) {
                return Ok(c);
            }
        }
    }
    if d.join("go.mod").exists() {
        if let Some(c) = pick("go.mod", "go", v(&["go", "test", "-v", "./..."])) {
            return Ok(c);
        }
    }
    if let Some(m) = ["pytest.ini", "pyproject.toml", "setup.cfg", "tox.ini"].into_iter().find(|m| d.join(m).exists()) {
        let py = if has("python3") { "python3" } else { "python" };
        if let Some(c) = pick(m, py, v(&[py, "-m", "pytest", "-q"])) {
            return Ok(c);
        }
    }
    if d.join("Gemfile").exists() && d.join("spec").is_dir() {
        if let Some(c) = pick("Gemfile with a spec directory", "bundle", v(&["bundle", "exec", "rspec"])) {
            return Ok(c);
        }
    }
    if d.join("mix.exs").exists() {
        if let Some(c) = pick("mix.exs", "mix", v(&["mix", "test"])) {
            return Ok(c);
        }
    }
    let make = ["Makefile", "makefile", "GNUmakefile"].into_iter().find_map(|m| std::fs::read_to_string(d.join(m)).ok());
    if make.is_some_and(|t| t.lines().any(|l| l.starts_with("test:") || l.starts_with("test :"))) {
        if let Some(c) = pick("A Makefile with a test target", "make", v(&["make", "test"])) {
            return Ok(c);
        }
    }
    Err(missing.unwrap_or_else(|| "No test command found (looked for Cargo, npm/pnpm/yarn/bun, go, pytest, rspec, mix and a make test target)".into()))
}

fn clip_line(l: &str) -> String {
    l.chars().take(240).collect()
}

/// (passed, failed, skipped) from the "N word" pairs in a summary line.
fn pairs(l: &str) -> (usize, usize, usize) {
    let toks: Vec<&str> = l.split([' ', ',', '|', '(', ')', ':', '=', ';']).filter(|t| !t.is_empty()).collect();
    let (mut p, mut f, mut s) = (0, 0, 0);
    for w in toks.windows(2) {
        let Ok(n) = w[0].parse::<usize>() else { continue };
        match w[1] {
            "passed" | "passing" | "pass" => p += n,
            "failed" | "failing" | "failure" | "failures" | "error" | "errors" | "fail" => f += n,
            "skipped" | "pending" | "todo" | "deselected" | "skip" => s += n,
            _ => {}
        }
    }
    (p, f, s)
}

/// Folds one line of a runner's output into the run: cargo per test, the others from their summary lines
/// (pytest, jest/vitest, mocha, TAP, go test, rspec/mix); every line keeps the last error-looking line and the last line.
pub fn test_line(run: &mut TestRun, line: &str) {
    let l = line.trim();
    if l.is_empty() {
        return;
    }
    fn add(run: &mut TestRun, (p, f, s): (usize, usize, usize)) {
        run.passed += p;
        run.failed += f;
        run.skipped += s;
    }
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
        return;
    }
    let first = l.chars().next().unwrap_or(' ');
    // go test -v: a top-level result (subtests are indented in the raw line)
    if let Some(r) = line.strip_prefix("--- ") {
        match r.split(':').next().unwrap_or("") {
            "PASS" => run.passed += 1,
            "FAIL" => {
                run.failed += 1;
                run.note = Some(clip_line(l));
            }
            "SKIP" => run.skipped += 1,
            _ => {}
        }
        return;
    }
    let bare = l.trim_matches(|c| c == '=' || c == ' ');
    if l.starts_with("Tests:") || l.starts_with("Tests ") || (l.starts_with("Tests") && l[5..].trim_start().starts_with(|c: char| c.is_ascii_digit())) {
        // jest "Tests: 1 failed, 4 passed, 5 total", vitest "Tests  1 failed | 4 passed (5)"
        let c = pairs(l);
        add(run, c);
    } else if (bare.contains(" in ") || bare.starts_with("no tests ran")) && bare.ends_with(['s', ')']) && (pairs(bare) != (0, 0, 0)) && !bare.starts_with("Time") {
        // pytest "2 failed, 5 passed, 1 skipped in 0.12s"
        let c = pairs(bare.split(" in ").next().unwrap_or(bare));
        add(run, c);
    } else if first.is_ascii_digit() && (l.contains(" examples, ") || l.contains(" example, ") || l.contains(" tests, ") || l.contains(" test, ")) && l.contains("failure") {
        // rspec "5 examples, 1 failure, 1 pending", mix "5 tests, 1 failure, 1 skipped"
        let n = l.split_whitespace().next().and_then(|n| n.parse::<usize>().ok()).unwrap_or(0);
        let (_, f, s) = pairs(l);
        run.passed += n.saturating_sub(f + s);
        run.failed += f;
        run.skipped += s;
    } else if first.is_ascii_digit() && (l.contains(" passing") || l.contains(" failing")) && l.split_whitespace().count() < 5 + 2 {
        // mocha "4 passing (12ms)", "1 failing"
        let c = pairs(l);
        add(run, c);
    } else if let Some(r) = l.strip_prefix("# ") {
        // TAP (node --test): "# pass 4", "# fail 1", "# skipped 0"
        let mut it = r.split_whitespace();
        if let (Some(w), Some(n)) = (it.next(), it.next().and_then(|n| n.parse::<usize>().ok())) {
            match w {
                "pass" => run.passed += n,
                "fail" => run.failed += n,
                "skipped" | "todo" => run.skipped += n,
                _ => {}
            }
        }
    }
    if (l.starts_with("error[") || l.starts_with("error:")) && run.error.is_none() && !l.starts_with("error: test failed") && !l.contains("target failed") && !l.contains("targets failed") {
        run.error = Some(l.to_string());
    }
    if ["FAILED", "FAIL", "E ", "error", "Error", "ERROR", "panic", "● ", "AssertionError", "npm ERR!", "Failure/Error", "** (", "# Failure"].iter().any(|p| l.starts_with(p)) || l.contains("panicked at") {
        run.note = Some(clip_line(l));
    }
    run.last = Some(clip_line(l));
}

fn signal_group(pgid: i32, sig: i32) {
    // the child leads its own process group (see `process_group(0)`), so the whole tree gets it
    unsafe {
        libc::kill(-pgid, sig);
    }
}

fn pump(r: impl std::io::Read, tx: std::sync::mpsc::Sender<String>) {
    use std::io::BufRead;
    let mut r = std::io::BufReader::new(r);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match r.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                if tx.send(String::from_utf8_lossy(&buf).into_owned()).is_err() {
                    break;
                }
            }
        }
    }
}

/// Runs `argv` in `wd` on this thread and sends the run as it goes (about 4 times a second), the last one with
/// `done`. `cancel` stops it: SIGTERM to the process group, SIGKILL after `grace`; `timeout` does the same.
pub fn run_tests(argv: &[String], wd: &str, tx: &std::sync::mpsc::Sender<TestRun>, cancel: &std::sync::atomic::AtomicBool, timeout: std::time::Duration, grace: std::time::Duration) {
    use std::os::unix::process::CommandExt;
    use std::sync::atomic::Ordering;
    use std::time::Instant;
    let mut run = TestRun { command: argv.join(" "), ..Default::default() };
    let child = std::process::Command::new(&argv[0]).args(&argv[1..]).current_dir(wd).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).process_group(0).spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            run.error = Some(format!("{}: {e}", argv[0]));
            run.done = true;
            let _ = tx.send(run);
            return;
        }
    };
    let pgid = child.id() as i32;
    let (ltx, lrx) = std::sync::mpsc::channel::<String>();
    if let Some(o) = child.stdout.take() {
        let t = ltx.clone();
        std::thread::spawn(move || pump(o, t));
    }
    if let Some(e) = child.stderr.take() {
        let t = ltx.clone();
        std::thread::spawn(move || pump(e, t));
    }
    drop(ltx);
    let (start, mut sent) = (Instant::now(), Instant::now());
    let (mut termed, mut exited): (Option<Instant>, Option<Instant>) = (None, None);
    let mut status = None;
    loop {
        match lrx.recv_timeout(std::time::Duration::from_millis(50)) {
            Ok(line) => {
                test_line(&mut run, &line);
                if sent.elapsed().as_millis() > 250 {
                    sent = Instant::now();
                    let _ = tx.send(run.clone());
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if termed.is_none() && (cancel.load(Ordering::Relaxed) || start.elapsed() > timeout) {
            run.cancelled = cancel.load(Ordering::Relaxed);
            run.timed_out = !run.cancelled;
            run.cancelling = true;
            termed = Some(Instant::now());
            signal_group(pgid, libc::SIGTERM);
            let _ = tx.send(run.clone());
        }
        if termed.is_some_and(|t| t.elapsed() > grace) {
            signal_group(pgid, libc::SIGKILL);
        }
        if status.is_none() {
            if let Ok(Some(s)) = child.try_wait() {
                status = Some(s);
                exited = Some(Instant::now());
                // anything it left running goes with it
                signal_group(pgid, libc::SIGKILL);
            }
        }
        // a stray process holding the pipes open must not hold the run
        if exited.is_some_and(|t| t.elapsed() > std::time::Duration::from_millis(500)) {
            break;
        }
    }
    let status = status.or_else(|| child.wait().ok());
    run.exit_ok = status.map(|s| s.success());
    run.cancelling = false;
    if termed.is_none() && run.exit_ok != Some(true) && run.error.is_none() && run.total() == 0 {
        run.error = run.note.clone().or_else(|| run.last.clone()).or_else(|| Some(format!("It exited with {}", status.and_then(|s| s.code()).map(|c| format!("code {c}")).unwrap_or_else(|| "a signal".into()))));
    }
    run.done = true;
    let _ = tx.send(run);
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

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("sphere-tests-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn detect(files: &[(&str, &str)], tools: &[&str]) -> Result<Vec<String>, String> {
        let d = tmp(&format!("d{}", files.iter().map(|f| f.0).collect::<Vec<_>>().join("-").replace('/', "_")));
        for (f, t) in files {
            if let Some(p) = Path::new(f).parent() {
                std::fs::create_dir_all(d.join(p)).unwrap();
            }
            std::fs::write(d.join(f), t).unwrap();
        }
        let r = detect_tests_with(d.to_str().unwrap(), &|t| tools.contains(&t));
        let _ = std::fs::remove_dir_all(&d);
        r
    }

    fn joined(r: Result<Vec<String>, String>) -> String {
        r.map(|v| v.join(" ")).unwrap_or_else(|e| format!("ERR {e}"))
    }

    #[test]
    fn detects_runners_from_the_worktree() {
        assert!(joined(detect(&[("Cargo.toml", "")], &["cargo"])).starts_with("cargo test"));
        let js = r#"{"scripts":{"test":"vitest run"}}"#;
        assert_eq!(joined(detect(&[("package.json", js)], &["npm"])), "npm test");
        assert_eq!(joined(detect(&[("package.json", js), ("pnpm-lock.yaml", "")], &["pnpm", "npm"])), "pnpm test");
        assert_eq!(joined(detect(&[("package.json", js), ("yarn.lock", "")], &["yarn"])), "yarn test");
        assert_eq!(joined(detect(&[("package.json", js), ("bun.lockb", ""), ("yarn.lock", "")], &["bun"])), "bun run test");
        assert_eq!(joined(detect(&[("go.mod", "module x")], &["go"])), "go test -v ./...");
        assert_eq!(joined(detect(&[("pyproject.toml", "")], &["python3"])), "python3 -m pytest -q");
        assert_eq!(joined(detect(&[("pytest.ini", "")], &["python"])), "python -m pytest -q");
        assert_eq!(joined(detect(&[("Gemfile", ""), ("spec/a_spec.rb", "")], &["bundle"])), "bundle exec rspec");
        assert_eq!(joined(detect(&[("mix.exs", "")], &["mix"])), "mix test");
        assert_eq!(joined(detect(&[("Makefile", "build:\n\tcc\ntest:\n\t./t\n")], &["make"])), "make test");
    }

    #[test]
    fn says_what_is_missing_or_unknown() {
        let js = r#"{"scripts":{"test":"jest"}}"#;
        let e = joined(detect(&[("package.json", js)], &[]));
        assert!(e.starts_with("ERR") && e.contains("npm isn't installed"), "{e}");
        // npm's placeholder script is not a test command; a Gemfile without specs neither; a Makefile without a test target neither
        let ph = r#"{"scripts":{"test":"echo \"Error: no test specified\" && exit 1"}}"#;
        assert!(joined(detect(&[("package.json", ph)], &["npm"])).contains("No test command found"));
        assert!(joined(detect(&[("Gemfile", "")], &["bundle"])).contains("No test command found"));
        assert!(joined(detect(&[("Makefile", "build:\n\tcc\n")], &["make"])).contains("No test command found"));
        // a missing tool does not hide a runner that works
        assert_eq!(joined(detect(&[("package.json", js), ("go.mod", "")], &["go"])), "go test -v ./...");
    }

    fn fold(lines: &str) -> TestRun {
        let mut r = TestRun::default();
        for l in lines.lines() {
            test_line(&mut r, l);
        }
        r
    }

    #[test]
    fn reads_other_runners_summaries() {
        let r = fold("..F.\n=========== short test summary info ===========\nFAILED tests/test_a.py::test_b - assert 1 == 2\n===== 1 failed, 5 passed, 2 skipped in 0.12s =====");
        assert_eq!((r.passed, r.failed, r.skipped, r.total()), (5, 1, 2, 8));
        assert_eq!(r.note.as_deref(), Some("FAILED tests/test_a.py::test_b - assert 1 == 2"));
        let r = fold("....\n4 passed in 0.01s");
        assert_eq!((r.passed, r.failed), (4, 0));
        let r = fold("Test Suites: 1 failed, 2 passed, 3 total\nTests:       1 failed, 4 passed, 5 total\nTime:        1.2 s");
        assert_eq!((r.passed, r.failed, r.total()), (4, 1, 5));
        let r = fold(" Test Files  1 failed | 1 passed (2)\n      Tests  1 failed | 7 passed | 1 skipped (9)\n   Duration  1.1s");
        assert_eq!((r.passed, r.failed, r.skipped), (7, 1, 1));
        let r = fold("=== RUN   TestA\n--- PASS: TestA (0.00s)\n    --- PASS: TestA/sub (0.00s)\n--- FAIL: TestB (0.00s)\n--- SKIP: TestC (0.00s)\nFAIL\nFAIL\tx/y\t0.01s");
        assert_eq!((r.passed, r.failed, r.skipped), (1, 1, 1));
        let r = fold("Finished in 0.1 seconds\n5 examples, 1 failure, 1 pending");
        assert_eq!((r.passed, r.failed, r.skipped), (3, 1, 1));
        let r = fold("3 tests, 0 failures");
        assert_eq!((r.passed, r.failed), (3, 0));
        let r = fold("  4 passing (12ms)\n  1 failing");
        assert_eq!((r.passed, r.failed), (4, 1));
        let r = fold("# tests 5\n# pass 4\n# fail 1");
        assert_eq!((r.passed, r.failed), (4, 1));
        // no counts at all: the last line is what there is to show
        let r = fold("compiling\nsegmentation fault");
        assert_eq!((r.total(), r.last.as_deref()), (0, Some("segmentation fault")));
        // progress cells follow the counts
        let r = fold("2 failed, 6 passed in 1s");
        assert_eq!((r.cell(0, 8), r.cell(6, 8), r.cell(7, 8)), (Some(TestState::Pass), Some(TestState::Fail), Some(TestState::Fail)));
    }

    fn run_sh(script: &str, cancel_after: Option<u64>, timeout_ms: u64) -> (TestRun, std::time::Duration) {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        let dir = tmp("sh");
        let cancel = Arc::new(AtomicBool::new(false));
        if let Some(ms) = cancel_after {
            let c = cancel.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(ms));
                c.store(true, Ordering::Relaxed);
            });
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let t0 = std::time::Instant::now();
        let argv: Vec<String> = ["sh", "-c", script].iter().map(|s| s.to_string()).collect();
        run_tests(&argv, dir.to_str().unwrap(), &tx, &cancel, std::time::Duration::from_millis(timeout_ms), std::time::Duration::from_millis(500));
        let took = t0.elapsed();
        let last = rx.try_iter().last().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        (last, took)
    }

    #[test]
    fn runs_reports_and_stops_a_run() {
        let (r, _) = run_sh("echo '2 passed in 0.1s'; exit 0", None, 60_000);
        assert!(r.done && !r.cancelled && r.status() == RunStatus::Passed && r.passed == 2, "{r:?}");
        let (r, _) = run_sh("echo '1 failed, 2 passed in 0.1s'; exit 1", None, 60_000);
        assert_eq!((r.status(), r.failed), (RunStatus::Failed, 1));
        let (r, _) = run_sh("echo boom >&2; exit 3", None, 60_000);
        assert_eq!((r.status(), r.error.as_deref()), (RunStatus::CouldNotRun, Some("boom")));
        // cancel: the whole group goes, grandchildren included, long before `sleep 30` would end
        let (r, took) = run_sh("echo started; sleep 30 & sleep 30; wait", Some(300), 60_000);
        assert!(r.done && r.cancelled && !r.timed_out && r.status() == RunStatus::Cancelled, "{r:?}");
        assert!(took.as_secs() < 5, "{took:?}");
        // a process that ignores SIGTERM is killed after the grace period
        let (r, took) = run_sh("trap '' TERM; while true; do sleep 1; done", Some(200), 60_000);
        assert_eq!(r.status(), RunStatus::Cancelled);
        assert!(took.as_secs() < 6, "{took:?}");
        // timeout
        let (r, took) = run_sh("sleep 30", None, 300);
        assert_eq!(r.status(), RunStatus::TimedOut);
        assert!(took.as_secs() < 5, "{took:?}");
        // not found
        let (tx, rx) = std::sync::mpsc::channel();
        run_tests(&["definitely-not-a-tool-xyz".to_string()], "/", &tx, &std::sync::atomic::AtomicBool::new(false), TEST_TIMEOUT, KILL_GRACE);
        let r = rx.try_iter().last().unwrap();
        assert!(r.done && r.error.is_some());
    }
}
