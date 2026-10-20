//! The pull requests of the Git app's repo, read and acted on with the GitHub CLI (`gh`, argv only, never a shell).
//! Everything here blocks, so the app calls it from background threads. The runner is injected so tests never
//! reach the network.
use crate::review::{parse_diff, FileDiff};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;
use web_time::Instant;

/// `gh pr list` fields asked for.
pub const LIST_FIELDS: &str = "number,title,headRefName,baseRefName,author,isDraft,reviewDecision,statusCheckRollup,updatedAt,url,additions,deletions";
pub const TIMEOUT: Duration = Duration::from_secs(10);
/// A merge press waits this long for its second press.
pub const ARM_SECS: f32 = 3.0;

/// What a `gh` run gave back.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Out {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

/// `gh` could not be run at all, or ran and failed (with its output).
#[derive(Clone, Debug, PartialEq)]
pub enum GhError {
    Missing,
    Run(String),
}

/// Runs `gh args` in a repo directory.
pub type Runner<'a> = &'a dyn Fn(&Path, &[&str]) -> Result<Out, GhError>;

/// Seconds since the epoch of an ISO `2026-10-18T10:00:00Z` stamp (UTC).
pub fn iso_secs(s: &str) -> Option<i64> {
    let n = |a: usize, b: usize| s.get(a..b)?.parse::<i64>().ok();
    let (y, m, d, hh, mm, ss) = (n(0, 4)?, n(5, 7)?, n(8, 10)?, n(11, 13)?, n(14, 16)?, n(17, 19)?);
    // days from civil
    let (y, m) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * m + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some((era * 146_097 + doe - 719_468) * 86_400 + hh * 3600 + mm * 60 + ss)
}

/// The real runner: `gh` with no prompts, killed after `TIMEOUT`.
pub fn run_gh(dir: &Path, args: &[&str]) -> Result<Out, GhError> {
    #[cfg(target_arch = "wasm32")]
    return crate::machine::gh(dir, args);
    let mut cmd = Command::new("gh");
    cmd.args(args).current_dir(dir).env("GH_PROMPT_DISABLED", "1").env("GH_NO_UPDATE_NOTIFIER", "1").env("NO_COLOR", "1").env("GH_PAGER", "cat").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { GhError::Missing } else { GhError::Run(format!("gh: {e}")) })?;
    let drain = |mut r: Box<dyn Read + Send>| std::thread::spawn(move || {
        let mut s = Vec::new();
        let _ = r.read_to_end(&mut s);
        String::from_utf8_lossy(&s).into_owned()
    });
    let so = drain(Box::new(child.stdout.take().unwrap()));
    let se = drain(Box::new(child.stderr.take().unwrap()));
    let t0 = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if t0.elapsed() > TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(GhError::Run("gh timed out after 10 s".into()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(e) => return Err(GhError::Run(format!("gh: {e}"))),
        }
    };
    Ok(Out { ok: status.is_some_and(|s| s.success()), stdout: so.join().unwrap_or_default(), stderr: se.join().unwrap_or_default() })
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Checks {
    pub pass: usize,
    pub fail: usize,
    pub pending: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Ok,
    Bad,
    Acc,
    Dim,
}

impl Checks {
    #[cfg(test)]
    pub fn total(&self) -> usize {
        self.pass + self.fail + self.pending
    }
    /// The single chip for the row: failing wins over pending, pending over passing.
    pub fn label(&self) -> (String, Tone) {
        if self.fail > 0 {
            (format!("{} FAILING", self.fail), Tone::Bad)
        } else if self.pending > 0 {
            (format!("{} PENDING", self.pending), Tone::Acc)
        } else if self.pass > 0 {
            ("PASSING".into(), Tone::Ok)
        } else {
            ("NO CHECKS".into(), Tone::Dim)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Review {
    None,
    Approved,
    Changes,
    Required,
}

impl Review {
    pub fn parse(s: &str) -> Review {
        match s {
            "APPROVED" => Review::Approved,
            "CHANGES_REQUESTED" => Review::Changes,
            "REVIEW_REQUIRED" => Review::Required,
            _ => Review::None,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Review::Approved => "APPROVED",
            Review::Changes => "CHANGES REQUESTED",
            Review::Required => "REVIEW REQUIRED",
            Review::None => "NO REVIEW",
        }
    }
    pub fn tone(self) -> Tone {
        match self {
            Review::Approved => Tone::Ok,
            Review::Changes => Tone::Bad,
            Review::Required => Tone::Acc,
            Review::None => Tone::Dim,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pr {
    pub number: u64,
    pub title: String,
    pub head: String,
    pub base: String,
    pub author: String,
    pub draft: bool,
    pub review: Review,
    pub checks: Checks,
    pub updated: String,
    pub url: String,
    pub additions: u64,
    pub deletions: u64,
}

/// What the pane shows.
#[derive(Clone, Debug, PartialEq)]
pub enum PrList {
    Prs(Vec<Pr>),
    /// nothing to list, and why (already worded for the pane)
    Note(String),
}

pub const NO_REMOTE: &str = "No GitHub remote";
pub const NO_GH: &str = "Install the GitHub CLI: brew install gh";
pub const NOT_SIGNED_IN: &str = "Sign in: gh auth login";
pub const NO_PRS: &str = "No open pull requests";

/// Words a failed `gh` run for the pane: the cases people can fix get their fix, else the first line of stderr.
pub fn map_error(e: &GhError) -> String {
    match e {
        GhError::Missing => NO_GH.into(),
        GhError::Run(s) => first_line(s),
    }
}

pub fn map_stderr(stderr: &str) -> String {
    let low = stderr.to_lowercase();
    if low.contains("gh auth login") || low.contains("not logged in") || low.contains("authentication required") || low.contains("bad credentials") {
        NOT_SIGNED_IN.into()
    } else if low.contains("no git remotes") || low.contains("none of the git remotes") || low.contains("could not resolve to a repository") || low.contains("no github remote") || low.contains("unable to determine") {
        NO_REMOTE.into()
    } else if low.contains("not a git repository") {
        "Not a git repository".into()
    } else {
        first_line(stderr)
    }
}

fn first_line(s: &str) -> String {
    s.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("gh failed").to_string()
}

/// Counts the checks of one `statusCheckRollup`: check runs by status and conclusion, commit statuses by state.
pub fn count_checks(rollup: &[serde_json::Value]) -> Checks {
    let mut c = Checks::default();
    for r in rollup {
        let s = |k: &str| r.get(k).and_then(|v| v.as_str()).unwrap_or("").to_uppercase();
        let (status, concl, state) = (s("status"), s("conclusion"), s("state"));
        if !state.is_empty() {
            match state.as_str() {
                "SUCCESS" => c.pass += 1,
                "FAILURE" | "ERROR" => c.fail += 1,
                _ => c.pending += 1,
            }
        } else if status != "COMPLETED" {
            c.pending += 1;
        } else {
            match concl.as_str() {
                "SUCCESS" | "NEUTRAL" | "SKIPPED" => c.pass += 1,
                "" => c.pending += 1,
                _ => c.fail += 1,
            }
        }
    }
    c
}

/// Parses `gh pr list --json ...` output.
pub fn parse_list(json: &str) -> Result<Vec<Pr>, String> {
    let v: serde_json::Value = serde_json::from_str(json.trim()).map_err(|e| format!("gh gave unreadable output: {e}"))?;
    let arr = v.as_array().ok_or("gh gave unexpected output")?;
    Ok(arr
        .iter()
        .filter_map(|p| {
            let st = |k: &str| p.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
            let n = |k: &str| p.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
            Some(Pr {
                number: p.get("number")?.as_u64()?,
                title: st("title"),
                head: st("headRefName"),
                base: st("baseRefName"),
                author: p.get("author").and_then(|a| a.get("login")).and_then(|v| v.as_str()).unwrap_or("").to_string(),
                draft: p.get("isDraft").and_then(|v| v.as_bool()).unwrap_or(false),
                review: Review::parse(&st("reviewDecision")),
                checks: count_checks(p.get("statusCheckRollup").and_then(|v| v.as_array()).map(|a| a.as_slice()).unwrap_or(&[])),
                updated: st("updatedAt"),
                url: st("url"),
                additions: n("additions"),
                deletions: n("deletions"),
            })
        })
        .collect())
}

/// The open PRs of the repo in `dir`.
pub fn list(dir: &Path, run: Runner) -> PrList {
    match run(dir, &["pr", "list", "--json", LIST_FIELDS, "--limit", "30"]) {
        Err(e) => PrList::Note(map_error(&e)),
        Ok(o) if !o.ok => PrList::Note(map_stderr(&o.stderr)),
        Ok(o) => match parse_list(&o.stdout) {
            Ok(v) if v.is_empty() => PrList::Note(NO_PRS.into()),
            Ok(v) => PrList::Prs(v),
            Err(e) => PrList::Note(e),
        },
    }
}

/// A PR's files and checks.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Detail {
    pub number: u64,
    pub files: Vec<FileDiff>,
    /// (name, state word, tone)
    pub checks: Vec<(String, String, Tone)>,
    pub error: Option<String>,
}

/// Parses `gh pr checks` lines (tab separated: name, state, elapsed, link).
pub fn parse_checks(raw: &str) -> Vec<(String, String, Tone)> {
    raw.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            if f.len() < 2 || f[0].trim().is_empty() {
                return None;
            }
            let state = f[1].trim().to_lowercase();
            let tone = match state.as_str() {
                "pass" | "success" => Tone::Ok,
                "fail" | "failure" | "error" | "cancel" | "cancelled" => Tone::Bad,
                "pending" | "queued" | "in_progress" => Tone::Acc,
                _ => Tone::Dim,
            };
            Some((f[0].trim().to_string(), state, tone))
        })
        .collect()
}

pub fn detail(dir: &Path, number: u64, run: Runner) -> Detail {
    let n = number.to_string();
    let mut d = Detail { number, ..Default::default() };
    match run(dir, &["pr", "diff", &n]) {
        Err(e) => d.error = Some(map_error(&e)),
        Ok(o) if !o.ok => d.error = Some(map_stderr(&o.stderr)),
        Ok(o) => d.files = parse_diff(&o.stdout),
    }
    // checks exit non-zero while some fail or wait, with their lines on stdout; "no checks reported" is not an error
    if let Ok(o) = run(dir, &["pr", "checks", &n]) {
        d.checks = parse_checks(&o.stdout);
    }
    d
}

/// What a press of M does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MergePress {
    /// first press: wait for the second
    Armed,
    /// second press within `ARM_SECS`: merge
    Merge,
}

/// Merging takes two presses of M on the same PR within `ARM_SECS`.
pub fn press_merge(arm: &mut Option<(u64, f32)>, number: u64, now: f32) -> MergePress {
    match *arm {
        Some((n, at)) if n == number && now - at <= ARM_SECS => {
            *arm = None;
            MergePress::Merge
        }
        _ => {
            *arm = Some((number, now));
            MergePress::Armed
        }
    }
}

/// True once an armed merge has waited too long.
pub fn arm_expired(arm: &Option<(u64, f32)>, now: f32) -> bool {
    arm.is_some_and(|(_, at)| now - at > ARM_SECS)
}

/// The toast words for the outcome of an action.
pub fn outcome(res: Result<Out, GhError>, ok_msg: String) -> Result<String, String> {
    match res {
        Err(e) => Err(map_error(&e)),
        Ok(o) if o.ok => Ok(ok_msg),
        Ok(o) => Err(map_stderr(&o.stderr)),
    }
}

/// Checkout, merge and open, in the background (the caller sends the result as a toast).
pub fn checkout(dir: &Path, n: u64, run: Runner) -> Result<String, String> {
    outcome(run(dir, &["pr", "checkout", &n.to_string()]), format!("Checked out #{n}"))
}

/// Squash merge: no `--admin`, no `--auto`.
pub fn merge(dir: &Path, n: u64, run: Runner) -> Result<String, String> {
    outcome(run(dir, &["pr", "merge", &n.to_string(), "--squash"]), format!("Merged #{n} (squash)"))
}

pub fn open_web(dir: &Path, n: u64, run: Runner) -> Result<String, String> {
    outcome(run(dir, &["pr", "view", &n.to_string(), "--web"]), format!("Opened #{n} on GitHub"))
}

/// What the orbit tile says: (open count, one line of reviews and CI), from the last read of the current repo.
pub static SUMMARY: std::sync::Mutex<Option<(usize, String)>> = std::sync::Mutex::new(None);

pub fn summarize(prs: &[Pr]) -> (usize, String) {
    let reviews = prs.iter().filter(|p| p.review == Review::Required).count();
    let c = prs.iter().fold(Checks::default(), |a, p| Checks { pass: a.pass + p.checks.pass, fail: a.fail + p.checks.fail, pending: a.pending + p.checks.pending });
    let ci = match c.label() {
        (_, Tone::Bad) => "CI failing",
        (_, Tone::Acc) => "CI running",
        (_, Tone::Ok) => "CI passing",
        _ => "no CI",
    };
    (prs.len(), format!("{reviews} review{} · {ci}", if reviews == 1 { "" } else { "s" }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::path::PathBuf;

    const LIST: &str = r#"[
 {"number":12,"title":"Add the thing","headRefName":"feat/thing","baseRefName":"main","author":{"login":"otilor","is_bot":false},"isDraft":false,"reviewDecision":"APPROVED","updatedAt":"2026-10-18T10:00:00Z","url":"https://github.com/o/r/pull/12","additions":40,"deletions":3,
  "statusCheckRollup":[{"__typename":"CheckRun","name":"fmt","status":"COMPLETED","conclusion":"SUCCESS"},{"__typename":"CheckRun","name":"clippy","status":"COMPLETED","conclusion":"SKIPPED"},{"__typename":"StatusContext","context":"ci","state":"SUCCESS"}]},
 {"number":13,"title":"Break the build","headRefName":"fix/b","baseRefName":"main","author":{"login":"ana"},"isDraft":false,"reviewDecision":"CHANGES_REQUESTED","updatedAt":"2026-10-17T10:00:00Z","url":"https://github.com/o/r/pull/13","additions":1,"deletions":1,
  "statusCheckRollup":[{"__typename":"CheckRun","status":"COMPLETED","conclusion":"FAILURE"},{"__typename":"CheckRun","status":"IN_PROGRESS","conclusion":""},{"__typename":"CheckRun","status":"COMPLETED","conclusion":"SUCCESS"}]},
 {"number":14,"title":"WIP","headRefName":"wip","baseRefName":"dev","author":{"login":"bo"},"isDraft":true,"reviewDecision":"REVIEW_REQUIRED","updatedAt":"2026-10-16T10:00:00Z","url":"https://github.com/o/r/pull/14","additions":0,"deletions":0,
  "statusCheckRollup":[{"__typename":"CheckRun","status":"QUEUED","conclusion":""},{"__typename":"StatusContext","state":"PENDING"}]},
 {"number":15,"title":"Docs","headRefName":"docs","baseRefName":"main","author":{"login":"cy"},"isDraft":false,"reviewDecision":"","updatedAt":"2026-10-15T10:00:00Z","url":"https://github.com/o/r/pull/15","additions":5,"deletions":0,"statusCheckRollup":[]}
]"#;

    const DIFF: &str = "diff --git a/src/a.rs b/src/a.rs
index 1..2 100644
--- a/src/a.rs
+++ b/src/a.rs
@@ -1,2 +1,3 @@
 fn a() {}
+fn b() {}
-fn c() {}
";

    fn out(ok: bool, so: &str, se: &str) -> Result<Out, GhError> {
        Ok(Out { ok, stdout: so.into(), stderr: se.into() })
    }

    #[test]
    fn parses_every_state_of_a_pr_list() {
        let v = parse_list(LIST).unwrap();
        assert_eq!(v.len(), 4);
        assert_eq!((v[0].number, v[0].author.as_str(), v[0].head.as_str(), v[0].base.as_str()), (12, "otilor", "feat/thing", "main"));
        assert_eq!((v[0].review, v[0].checks), (Review::Approved, Checks { pass: 3, fail: 0, pending: 0 }));
        assert_eq!((v[0].additions, v[0].deletions), (40, 3));
        assert_eq!((v[1].review, v[1].checks), (Review::Changes, Checks { pass: 1, fail: 1, pending: 1 }));
        assert!(v[2].draft && v[2].review == Review::Required);
        assert_eq!(v[2].checks, Checks { pass: 0, fail: 0, pending: 2 });
        assert_eq!((v[3].review, v[3].checks.total()), (Review::None, 0));
        assert!(parse_list("not json").is_err());
        assert!(parse_list("{}").is_err());
    }

    #[test]
    fn the_checks_chip_says_the_worst_state() {
        let l = |pass, fail, pending| Checks { pass, fail, pending }.label();
        assert_eq!(l(3, 0, 0), ("PASSING".to_string(), Tone::Ok));
        assert_eq!(l(3, 1, 2), ("1 FAILING".to_string(), Tone::Bad));
        assert_eq!(l(3, 0, 2), ("2 PENDING".to_string(), Tone::Acc));
        assert_eq!(l(0, 0, 0), ("NO CHECKS".to_string(), Tone::Dim));
    }

    #[test]
    fn review_states_have_words_and_tones() {
        assert_eq!(Review::parse("CHANGES_REQUESTED").label(), "CHANGES REQUESTED");
        assert_eq!(Review::parse("").label(), "NO REVIEW");
        assert_eq!(Review::parse("APPROVED").tone(), Tone::Ok);
    }

    #[test]
    fn lists_through_an_injected_runner() {
        let seen = RefCell::new(Vec::<String>::new());
        let run = |_: &Path, a: &[&str]| {
            seen.borrow_mut().push(a.join(" "));
            out(true, LIST, "")
        };
        let PrList::Prs(v) = list(&PathBuf::from("/r"), &run) else { panic!("prs") };
        assert_eq!(v.len(), 4);
        assert!(seen.borrow()[0].starts_with("pr list --json number,title,headRefName"));
        assert!(seen.borrow()[0].ends_with("--limit 30"));
        assert_eq!(list(&PathBuf::from("/r"), &|_: &Path, _: &[&str]| out(true, "[]", "")), PrList::Note(NO_PRS.into()));
    }

    #[test]
    fn maps_errors_to_what_to_do() {
        let note = |r: Result<Out, GhError>| list(&PathBuf::from("/r"), &move |_: &Path, _: &[&str]| r.clone());
        assert_eq!(note(Err(GhError::Missing)), PrList::Note(NO_GH.into()));
        assert_eq!(note(out(false, "", "none of the git remotes configured for this repository point to a known GitHub host.\n")), PrList::Note(NO_REMOTE.into()));
        assert_eq!(note(out(false, "", "no git remotes found")), PrList::Note(NO_REMOTE.into()));
        assert_eq!(note(out(false, "", "To get started with GitHub CLI, please run:  gh auth login\n")), PrList::Note(NOT_SIGNED_IN.into()));
        assert_eq!(note(out(false, "", "\nGraphQL: something odd\nmore\n")), PrList::Note("GraphQL: something odd".into()));
        assert_eq!(note(Err(GhError::Run("gh timed out after 10 s".into()))), PrList::Note("gh timed out after 10 s".into()));
        assert_eq!(note(out(true, "oops", "")), PrList::Note("gh gave unreadable output: expected value at line 1 column 1".into()));
    }

    #[test]
    fn a_detail_has_the_parsed_diff_and_the_checks() {
        let run = |_: &Path, a: &[&str]| match a[1] {
            "diff" => out(true, DIFF, ""),
            // gh checks exits 8 while some are pending, with the lines still on stdout
            _ => out(false, "fmt\tpass\t12s\thttps://x/1\nclippy\tfail\t1m\thttps://x/2\nrelease\tpending\t0\thttps://x/3\n", ""),
        };
        let d = detail(&PathBuf::from("/r"), 12, &run);
        assert_eq!(d.number, 12);
        assert_eq!(d.files.len(), 1);
        assert_eq!((d.files[0].path.as_str(), d.files[0].added, d.files[0].removed), ("src/a.rs", 1, 1));
        assert_eq!(d.checks.iter().map(|c| c.2).collect::<Vec<_>>(), vec![Tone::Ok, Tone::Bad, Tone::Acc]);
        assert!(d.error.is_none());
        let bad = detail(&PathBuf::from("/r"), 1, &|_: &Path, _: &[&str]| out(false, "", "no pull requests found for 1"));
        assert_eq!(bad.error.as_deref(), Some("no pull requests found for 1"));
    }

    #[test]
    fn merging_takes_two_presses_within_three_seconds() {
        let mut arm = None;
        assert_eq!(press_merge(&mut arm, 7, 10.0), MergePress::Armed);
        assert!(!arm_expired(&arm, 12.9) && arm_expired(&arm, 13.1));
        assert_eq!(press_merge(&mut arm, 7, 12.0), MergePress::Merge);
        assert!(arm.is_none());
        // too late: it arms again instead of merging
        assert_eq!(press_merge(&mut arm, 7, 20.0), MergePress::Armed);
        assert_eq!(press_merge(&mut arm, 7, 24.0), MergePress::Armed);
        // another PR never inherits the arming
        assert_eq!(press_merge(&mut arm, 8, 24.5), MergePress::Armed);
        assert_eq!(press_merge(&mut arm, 8, 25.0), MergePress::Merge);
    }

    #[test]
    fn actions_run_the_safe_commands() {
        let seen = RefCell::new(Vec::<String>::new());
        let run = |_: &Path, a: &[&str]| {
            seen.borrow_mut().push(a.join(" "));
            out(true, "", "")
        };
        let d = PathBuf::from("/r");
        assert_eq!(merge(&d, 5, &run), Ok("Merged #5 (squash)".into()));
        assert_eq!(checkout(&d, 5, &run), Ok("Checked out #5".into()));
        assert!(open_web(&d, 5, &run).is_ok());
        assert_eq!(*seen.borrow(), vec!["pr merge 5 --squash", "pr checkout 5", "pr view 5 --web"]);
        assert!(!seen.borrow()[0].contains("--admin") && !seen.borrow()[0].contains("--auto"));
        assert_eq!(merge(&d, 5, &|_: &Path, _: &[&str]| out(false, "", "Pull request is not mergeable\n")), Err("Pull request is not mergeable".into()));
    }

    #[test]
    fn reads_iso_stamps() {
        assert_eq!(iso_secs("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(iso_secs("2026-10-18T10:00:00Z"), Some(1_792_317_600));
        assert_eq!(iso_secs("junk"), None);
    }

    #[test]
    fn the_tile_summary_counts_reviews_and_ci() {
        let v = parse_list(LIST).unwrap();
        assert_eq!(summarize(&v), (4, "1 review · CI failing".to_string()));
        assert_eq!(summarize(&v[..1]), (1, "0 reviews · CI passing".to_string()));
    }
}
