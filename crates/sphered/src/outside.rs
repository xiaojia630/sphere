//! Claude sessions running outside Sphere: interactive `claude` in a terminal
//! tab (found in the process table, tied to its transcript) and background
//! sessions (from `claude agents --json`). Nothing runs unless a client asks.

use crate::config::Config;
use crate::sessions;
use anyhow::{bail, Result};
use serde_json::Value;
use sphere_proto::api::OutsideSession;
use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// How a session is described when `claude agents` runs it.
pub const BACKGROUND: &str = "Claude background";
/// How much of a transcript's end is read to tell what the session is doing.
const TAIL_BYTES: u64 = 64 * 1024;

/// One session found outside Sphere, with what is needed to stop it.
#[derive(Debug, Clone)]
pub struct Found {
    pub info: OutsideSession,
    /// Process start time (unix seconds), to tell a pid that was reused.
    pub start: Option<u64>,
    pub background: bool,
}

// ---- the process table ------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Proc {
    pub pid: u32,
    pub ppid: u32,
    /// Unix seconds.
    pub start: Option<u64>,
    pub comm: String,
    pub args: String,
}

fn next_token(s: &str) -> Option<(&str, &str)> {
    let s = s.trim_start();
    if s.is_empty() {
        return None;
    }
    let end = s.find(char::is_whitespace).unwrap_or(s.len());
    Some((&s[..end], &s[end..]))
}

/// `Mon Oct 19 14:23:55 2026` (local time, as `ps -o lstart` prints it).
fn parse_lstart(t: &[&str]) -> Option<u64> {
    if t.len() != 5 {
        return None;
    }
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let mon = MONTHS.iter().position(|m| *m == t[1])? as i32;
    let day: i32 = t[2].parse().ok()?;
    let mut hms = t[3].split(':').map(|x| x.parse::<i32>());
    let (h, m, s) = (hms.next()?.ok()?, hms.next()?.ok()?, hms.next()?.ok()?);
    let year: i32 = t[4].parse().ok()?;
    // SAFETY: tm is plain data; mktime only reads and normalises it.
    let secs = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        tm.tm_year = year - 1900;
        tm.tm_mon = mon;
        tm.tm_mday = day;
        tm.tm_hour = h;
        tm.tm_min = m;
        tm.tm_sec = s;
        tm.tm_isdst = -1;
        libc::mktime(&mut tm)
    };
    (secs >= 0).then_some(secs as u64)
}

/// `comm` and `args` are printed one after the other and either may hold
/// spaces. `args` normally starts with `comm`, which tells where one ends.
fn split_comm_args(rest: &str) -> (String, String) {
    let rest = rest.trim();
    for (i, c) in rest.char_indices() {
        if c.is_whitespace() {
            let tail = rest[i..].trim_start();
            if tail.starts_with(&rest[..i]) {
                return (rest[..i].to_string(), tail.to_string());
            }
        }
    }
    match rest.split_once(char::is_whitespace) {
        Some((c, a)) => (c.to_string(), a.trim().to_string()),
        None => (rest.to_string(), rest.to_string()),
    }
}

/// The output of `ps -axo pid=,ppid=,lstart=,comm=,args=`.
pub fn parse_ps(text: &str) -> Vec<Proc> {
    text.lines()
        .filter_map(|line| {
            let (pid, rest) = next_token(line)?;
            let (ppid, mut rest) = next_token(rest)?;
            let mut lstart = vec![];
            for _ in 0..5 {
                let (t, r) = next_token(rest)?;
                lstart.push(t);
                rest = r;
            }
            let (comm, args) = split_comm_args(rest);
            Some(Proc {
                pid: pid.parse().ok()?,
                ppid: ppid.parse().ok()?,
                start: parse_lstart(&lstart),
                comm,
                args,
            })
        })
        .collect()
}

fn basename(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

fn ps_command(extra: &[&str]) -> std::process::Command {
    let mut c = std::process::Command::new("ps");
    c.env("LC_ALL", "C")
        .args(["-o", "pid=,ppid=,lstart=,comm=,args="])
        .args(extra);
    c
}

// ---- what is a Claude process -------------------------------------------------

/// The words of a command line after the program itself.
fn program_args(p: &Proc) -> Vec<&str> {
    let t: Vec<&str> = p.args.split_whitespace().collect();
    match t
        .iter()
        .position(|w| w.contains("claude-code") && w.contains("cli.js"))
    {
        Some(j) => t[j + 1..].to_vec(),
        None => t.get(1..).unwrap_or_default().to_vec(),
    }
}

/// Native `claude`, or node running claude-code's `cli.js`.
pub fn is_claude(p: &Proc) -> bool {
    let arg0 = p.args.split_whitespace().next().map(basename).unwrap_or("");
    if basename(&p.comm) == "claude" || arg0 == "claude" {
        return true;
    }
    matches!(arg0, "node" | "nodejs" | "bun")
        && (p.args.contains("@anthropic-ai/claude-code") || p.args.contains("claude-code/cli.js"))
}

const SUBCOMMANDS: &[&str] = &[
    "agents",
    "mcp",
    "daemon",
    "attach",
    "update",
    "doctor",
    "auth",
    "config",
    "install",
    "stop",
    "setup-token",
    "migrate-installer",
];

/// `claude agents`, `claude mcp` and the like: tools, never a session.
fn is_subcommand(p: &Proc) -> bool {
    program_args(p)
        .first()
        .is_some_and(|w| SUBCOMMANDS.contains(w))
}

/// One-shot and helper forms are not sessions a person sits in: print mode,
/// the SDK/stream-json mode other tools drive, subcommands, version and help.
pub fn is_helper(p: &Proc) -> bool {
    if is_subcommand(p) {
        return true;
    }
    let a = program_args(p);
    a.iter().any(|w| {
        matches!(
            *w,
            "-p" | "--print"
                | "--version"
                | "-v"
                | "-V"
                | "--help"
                | "-h"
                | "--output-format"
                | "--input-format"
                | "--sdk-url"
        ) || w.starts_with("--output-format=")
            || w.starts_with("--input-format=")
    })
}

fn is_uuid(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(&parts)
            .all(|(n, p)| p.len() == *n && p.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// `--session-id <id>`, `-r <id>`, `--resume <id>` or `--resume=<id>`.
pub fn explicit_session(p: &Proc) -> Option<String> {
    let a = program_args(p);
    for (i, w) in a.iter().enumerate() {
        let val = match *w {
            "--session-id" | "-r" | "--resume" => a.get(i + 1).copied(),
            _ => w
                .strip_prefix("--resume=")
                .or_else(|| w.strip_prefix("--session-id=")),
        };
        if let Some(v) = val.filter(|v| is_uuid(v)) {
            return Some(v.to_string());
        }
    }
    None
}

/// The terminal, multiplexer or editor a name belongs to.
fn host_name(comm: &str) -> Option<&'static str> {
    let path = comm.to_ascii_lowercase();
    let b = basename(&path);
    if path.contains("visual studio code") {
        return Some("VS Code");
    }
    if path.contains("cursor.app") {
        return Some("Cursor");
    }
    if path.contains("warp") && b == "stable" {
        return Some("Warp");
    }
    Some(match b {
        "herdr" => "herdr",
        _ if b.starts_with("tmux") => "tmux",
        "zellij" => "zellij",
        "screen" => "screen",
        "terminal" => "Terminal",
        "iterm2" | "iterm" => "iTerm2",
        "ghostty" => "Ghostty",
        "wezterm" | "wezterm-gui" => "WezTerm",
        "kitty" => "kitty",
        "alacritty" => "Alacritty",
        "warp" | "warp-terminal" => "Warp",
        "code" => "VS Code",
        _ if b.starts_with("code helper") => "VS Code",
        "cursor" => "Cursor",
        _ if b.starts_with("cursor helper") => "Cursor",
        "zed" | "zed-editor" => "Zed",
        _ if b.starts_with("sshd") => "over SSH",
        _ => return None,
    })
}

/// Where `pid` runs: the first known host among its ancestors.
pub fn host_of(table: &HashMap<u32, &Proc>, pid: u32) -> String {
    let mut cur = table.get(&pid).map(|p| p.ppid);
    for _ in 0..64 {
        let Some(p) = cur.and_then(|c| table.get(&c)) else {
            break;
        };
        let arg0 = p.args.split_whitespace().next().unwrap_or("");
        if let Some(h) = host_name(&p.comm).or_else(|| host_name(arg0)) {
            return h.to_string();
        }
        cur = (p.ppid != p.pid).then_some(p.ppid);
    }
    "a terminal".to_string()
}

/// Is `pid` or one of its ancestors in `roots`?
fn descends(table: &HashMap<u32, &Proc>, pid: u32, roots: &HashSet<u32>) -> bool {
    let mut cur = pid;
    for _ in 0..64 {
        if roots.contains(&cur) {
            return true;
        }
        match table.get(&cur) {
            Some(p) if p.ppid != cur && p.ppid > 0 => cur = p.ppid,
            _ => return false,
        }
    }
    false
}

// ---- working directories ------------------------------------------------------

/// `lsof -a -d cwd -p ... -Fn`: `p<pid>` lines, each followed by `n<path>`.
pub fn parse_lsof_cwd(text: &str) -> HashMap<u32, String> {
    let mut out = HashMap::new();
    let mut cur = None;
    for line in text.lines() {
        if let Some(p) = line.strip_prefix('p') {
            cur = p.parse::<u32>().ok();
        } else if let (Some(path), Some(pid)) = (line.strip_prefix('n'), cur) {
            out.entry(pid).or_insert_with(|| path.to_string());
        }
    }
    out
}

#[cfg(target_os = "linux")]
fn cwds(pids: &[u32]) -> HashMap<u32, String> {
    pids.iter()
        .filter_map(|p| {
            let l = std::fs::read_link(format!("/proc/{p}/cwd")).ok()?;
            Some((*p, l.to_string_lossy().to_string()))
        })
        .collect()
}

#[cfg(not(target_os = "linux"))]
fn cwds(pids: &[u32]) -> HashMap<u32, String> {
    if pids.is_empty() {
        return HashMap::new();
    }
    let list: Vec<String> = pids.iter().map(u32::to_string).collect();
    let out = std::process::Command::new("lsof")
        .args(["-a", "-d", "cwd", "-Fn", "-p", &list.join(",")])
        .stdin(std::process::Stdio::null())
        .output();
    // lsof exits 1 when one of the pids is gone; the rest is still printed
    out.map(|o| parse_lsof_cwd(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default()
}

// ---- transcripts --------------------------------------------------------------

/// Where Claude Code keeps the transcripts of a folder: every character that
/// is not a letter or digit becomes `-`.
fn encode_cwd(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn mtime_secs(path: &Path) -> Option<u64> {
    let t = std::fs::metadata(path).and_then(|m| m.modified()).ok()?;
    Some(t.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_secs())
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tx {
    pub id: String,
    pub cwd: String,
    /// Unix seconds.
    pub mtime: u64,
}

fn tx_in(dir: &Path, since: u64, out: &mut Vec<Tx>) {
    let Ok(files) = std::fs::read_dir(dir) else {
        return;
    };
    for f in files.flatten() {
        let p = f.path();
        if p.extension().is_none_or(|e| e != "jsonl") {
            continue;
        }
        let Some(mtime) = mtime_secs(&p).filter(|m| *m >= since) else {
            continue;
        };
        if let Some(s) = sessions::read_transcript(&p) {
            if let Some(cwd) = s.cwd {
                out.push(Tx {
                    id: s.session_id,
                    cwd,
                    mtime,
                });
            }
        }
    }
}

/// Transcripts of `cwd` written since `since`.
fn transcripts_for(projects: &Path, cwd: &str, since: u64) -> Vec<Tx> {
    let mut out = vec![];
    let dir = projects.join(encode_cwd(cwd));
    if dir.is_dir() {
        tx_in(&dir, since, &mut out);
    } else if let Ok(dirs) = std::fs::read_dir(projects) {
        for d in dirs.flatten() {
            tx_in(&d.path(), since, &mut out);
        }
    }
    out.retain(|t| t.cwd == cwd);
    out
}

/// The transcript file of a session.
pub fn find_transcript(projects: &Path, id: &str, cwd: Option<&str>) -> Option<PathBuf> {
    let name = format!("{id}.jsonl");
    if let Some(c) = cwd {
        let p = projects.join(encode_cwd(c)).join(&name);
        if p.is_file() {
            return Some(p);
        }
    }
    std::fs::read_dir(projects)
        .ok()?
        .flatten()
        .map(|d| d.path().join(&name))
        .find(|p| p.is_file())
}

/// A process to find the transcript of.
#[derive(Debug, Clone)]
pub struct Cand {
    pub pid: u32,
    pub cwd: Option<String>,
    pub start: Option<u64>,
    pub explicit: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Assigned {
    pub id: Option<String>,
    /// Shares its folder with another process and cannot be told apart.
    pub ambiguous: bool,
}

/// Tie each process to a session: the id on its command line, else the newest
/// transcript of its folder written since it started. Processes in one folder
/// with no id between them are all left ambiguous, and a transcript goes to
/// one process only.
pub fn assign(cands: &[Cand], txs: &[Tx]) -> Vec<Assigned> {
    let mut out = vec![Assigned::default(); cands.len()];
    let mut used: HashSet<String> = HashSet::new();
    let mut groups: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, c) in cands.iter().enumerate() {
        if let Some(id) = &c.explicit {
            out[i].id = Some(id.clone());
            used.insert(id.clone());
        } else if let Some(cwd) = &c.cwd {
            groups.entry(cwd.as_str()).or_default().push(i);
        }
    }
    for (cwd, idxs) in groups {
        if idxs.len() > 1 {
            for i in idxs {
                out[i].ambiguous = true;
            }
            continue;
        }
        let i = idxs[0];
        let since = cands[i].start.map(|s| s.saturating_sub(2)).unwrap_or(0);
        let pick = txs
            .iter()
            .filter(|t| t.cwd == cwd && t.mtime >= since && !used.contains(&t.id))
            .max_by(|a, b| a.mtime.cmp(&b.mtime).then(b.id.cmp(&a.id)));
        if let Some(t) = pick {
            used.insert(t.id.clone());
            out[i].id = Some(t.id.clone());
        }
    }
    out
}

// ---- what the session is doing ------------------------------------------------

fn read_tail(path: &Path) -> Option<String> {
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let from = len.saturating_sub(TAIL_BYTES);
    f.seek(SeekFrom::Start(from)).ok()?;
    let mut buf = vec![];
    f.take(TAIL_BYTES).read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf).to_string();
    if from == 0 {
        return Some(text);
    }
    // the cut may fall mid-line
    Some(text.split_once('\n').map(|x| x.1).unwrap_or("").to_string())
}

fn blocks(v: &Value) -> Vec<&Value> {
    match v.pointer("/message/content") {
        Some(Value::Array(a)) => a.iter().collect(),
        _ => vec![],
    }
}

fn first_text(v: &Value) -> String {
    match v.pointer("/message/content") {
        Some(Value::String(s)) => s.clone(),
        _ => blocks(v)
            .iter()
            .find_map(|b| b.get("text").and_then(|t| t.as_str()))
            .unwrap_or("")
            .to_string(),
    }
}

/// `(state, last assistant text)` from the end of a transcript: a user
/// message or a tool result last means it is `working`; a tool call with no
/// result yet means `waiting` (a permission prompt or a running tool); an
/// assistant turn that ended means `idle`.
pub fn tail_summary(tail: &str) -> (&'static str, Option<String>) {
    let recs: Vec<Value> = tail
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|v| {
            matches!(
                v.get("type").and_then(|t| t.as_str()),
                Some("user" | "assistant")
            ) && v.get("isSidechain").and_then(|s| s.as_bool()) != Some(true)
        })
        .collect();
    let is_assistant = |v: &Value| v.get("type").and_then(|t| t.as_str()) == Some("assistant");
    let mut open: HashSet<&str> = HashSet::new();
    for v in &recs {
        if is_assistant(v) {
            for b in blocks(v) {
                if b.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                    if let Some(id) = b.get("id").and_then(|i| i.as_str()) {
                        open.insert(id);
                    }
                }
            }
            continue;
        }
        let results: Vec<&str> = blocks(v)
            .iter()
            .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_result"))
            .filter_map(|b| b.get("tool_use_id").and_then(|i| i.as_str()))
            .collect();
        if results.is_empty() {
            open.clear();
        }
        for r in results {
            open.remove(r);
        }
    }
    let last_message = recs.iter().rev().filter(|v| is_assistant(v)).find_map(|v| {
        let t = blocks(v)
            .iter()
            .rev()
            .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
            .find_map(|b| b.get("text").and_then(|t| t.as_str()))?
            .trim();
        let line = t.lines().next().unwrap_or("");
        (!line.is_empty()).then(|| {
            if line.chars().count() > 140 {
                format!("{}…", line.chars().take(139).collect::<String>())
            } else {
                line.to_string()
            }
        })
    });
    let state = match recs.last() {
        None => "unknown",
        Some(v) if !is_assistant(v) => {
            if first_text(v).starts_with("[Request interrupted by user") {
                "idle"
            } else {
                "working"
            }
        }
        Some(_) if !open.is_empty() => "waiting",
        Some(v)
            if v.pointer("/message/stop_reason").and_then(|s| s.as_str()) == Some("end_turn")
                || blocks(v)
                    .iter()
                    .any(|b| b.get("type").and_then(|t| t.as_str()) == Some("text")) =>
        {
            "idle"
        }
        Some(_) => "unknown",
    };
    (state, last_message)
}

/// State and last message of a session's transcript.
pub fn transcript_state(path: &Path) -> (&'static str, Option<String>) {
    read_tail(path)
        .map(|t| tail_summary(&t))
        .unwrap_or(("unknown", None))
}

// ---- scanning -----------------------------------------------------------------

/// The longest a [`Probe`] stands while no Claude process starts or stops.
const PROBE_MAX_AGE: Duration = Duration::from_secs(60);

/// What the slow part of a scan found: the agents list (`claude agents --json`
/// starts a Node program) and the folder of each Claude process (`lsof` on
/// macOS). It stands while the same Claude processes run and their
/// transcripts can tell their status, so a scan of a quiet machine runs `ps`
/// and reads transcripts.
#[derive(Debug)]
pub struct Probe {
    /// When the list was asked for.
    at: SystemTime,
    since: Instant,
    /// The Claude processes running then (see [`claude_procs`]).
    claudes: Vec<(u32, Option<u64>)>,
    listed: Option<String>,
    /// Folders looked up so far, by pid (`None`: it could not be read).
    cwds: HashMap<u32, Option<String>>,
}

impl Probe {
    /// Asks for the agents list now, on a machine running `claudes`, keeping
    /// the folders `old` found of the processes still running (one it could
    /// not read is asked about again).
    async fn take(cfg: &Config, claudes: Vec<(u32, Option<u64>)>, old: Option<Probe>) -> Probe {
        let cwds = match old {
            Some(Probe {
                claudes: was, cwds, ..
            }) => cwds
                .into_iter()
                .filter(|(pid, cwd)| {
                    cwd.is_some() && was.iter().any(|w| w.0 == *pid && claudes.contains(w))
                })
                .collect(),
            None => HashMap::new(),
        };
        let at = SystemTime::now();
        let listed = match sessions::run_json(&cfg.session_list_cmd()).await {
            Ok(t) => Some(t),
            Err(e) => {
                tracing::debug!("could not list sessions: {e:#}");
                None
            }
        };
        Probe {
            at,
            since: Instant::now(),
            claudes,
            listed,
            cwds,
        }
    }

    /// Whether it still stands for a machine running `claudes`.
    fn stands(&self, claudes: &[(u32, Option<u64>)]) -> bool {
        self.claudes == claudes && self.since.elapsed() < PROBE_MAX_AGE
    }
}

/// The process table, empty when `ps` fails.
fn read_procs() -> Vec<Proc> {
    ps_command(&["-ax"])
        .stdin(std::process::Stdio::null())
        .output()
        .map(|out| parse_ps(&String::from_utf8_lossy(&out.stdout)))
        .unwrap_or_default()
}

/// Every Claude process but the subcommands (`claude agents` itself is one),
/// with its start, sorted: helpers count, since a background session may run
/// as one. While this stays the same, no session started or stopped.
fn claude_procs(procs: &[Proc]) -> Vec<(u32, Option<u64>)> {
    let mut v: Vec<(u32, Option<u64>)> = procs
        .iter()
        .filter(|p| is_claude(p) && !is_subcommand(p))
        .map(|p| (p.pid, p.start))
        .collect();
    v.sort_unstable();
    v
}

/// The status the agents list gave, unless the transcript changed after the
/// list was taken (or the list said nothing): then the transcript says. But
/// an open tool call is either running or waiting for an answer, which only
/// the list can tell: `None` asks for a new list, unless it was just taken.
fn pick_state<'a>(
    listed: Option<&'a str>,
    tail: &'a str,
    changed: bool,
    just_listed: bool,
) -> Option<&'a str> {
    match listed {
        Some(s) if !changed || tail == "unknown" => Some(s),
        Some(s) if tail == "waiting" => just_listed.then_some(s),
        _ => Some(tail),
    }
}

fn modified(path: Option<&Path>) -> Option<SystemTime> {
    std::fs::metadata(path?).and_then(|m| m.modified()).ok()
}

/// Interactive Claude processes in `procs` outside `roots` (Sphere's own
/// agents) and `me`. A process's folder is looked up once and kept in
/// `cwd_of`; `listed_at` is when `listed` was taken. True with them when a
/// status needs a new list (see [`pick_state`]).
#[allow(clippy::too_many_arguments)]
fn scan_procs(
    projects: &Path,
    procs: &[Proc],
    roots: &[i32],
    me: u32,
    listed: &[Listed],
    listed_at: SystemTime,
    just_listed: bool,
    cwd_of: &mut HashMap<u32, Option<String>>,
) -> (Vec<Found>, bool) {
    let table: HashMap<u32, &Proc> = procs.iter().map(|p| (p.pid, p)).collect();
    let mut skip: HashSet<u32> = roots
        .iter()
        .filter(|r| **r > 0)
        .map(|r| *r as u32)
        .collect();
    skip.insert(me);
    let claudes: Vec<&Proc> = procs
        .iter()
        .filter(|p| is_claude(p) && !is_helper(p) && !descends(&table, p.pid, &skip))
        .collect();
    // a wrapper and the process it runs are one session
    let pids: HashSet<u32> = claudes.iter().map(|p| p.pid).collect();
    let claudes: Vec<&Proc> = claudes
        .into_iter()
        .filter(|p| !pids.contains(&p.ppid))
        .collect();
    // a process keeps its folder: ask only about the new ones
    let new: Vec<u32> = claudes
        .iter()
        .map(|p| p.pid)
        .filter(|pid| !cwd_of.contains_key(pid))
        .collect();
    if !new.is_empty() {
        let mut got = cwds(&new);
        for pid in new {
            cwd_of.insert(pid, got.remove(&pid));
        }
    }
    let cands: Vec<Cand> = claudes
        .iter()
        .map(|p| Cand {
            pid: p.pid,
            cwd: cwd_of.get(&p.pid).cloned().flatten(),
            start: p.start,
            // the agents list knows the session of an interactive process
            explicit: explicit_session(p)
                .or_else(|| listed_for(listed, p.pid).map(|l| l.id.clone())),
        })
        .collect();
    let mut txs = vec![];
    let mut seen = HashSet::new();
    for c in cands.iter().filter(|c| c.explicit.is_none()) {
        if let Some(cwd) = &c.cwd {
            if seen.insert(cwd.clone()) {
                let since = cands
                    .iter()
                    .filter(|o| o.cwd.as_ref() == Some(cwd))
                    .filter_map(|o| o.start)
                    .min()
                    .unwrap_or(0);
                txs.extend(transcripts_for(projects, cwd, since.saturating_sub(2)));
            }
        }
    }
    let assigned = assign(&cands, &txs);
    let mut again = false;
    let found = claudes
        .iter()
        .zip(cands.iter().zip(assigned))
        .map(|(p, (c, a))| {
            let path =
                a.id.as_deref()
                    .and_then(|id| find_transcript(projects, id, c.cwd.as_deref()));
            let (tail_state, last_message) = path
                .as_deref()
                .map(transcript_state)
                .unwrap_or(("unknown", None));
            let changed_at = modified(path.as_deref());
            let state = pick_state(
                listed_for(listed, p.pid).and_then(|l| l.state),
                tail_state,
                changed_at.is_some_and(|m| m > listed_at),
                just_listed,
            )
            .unwrap_or_else(|| {
                again = true;
                tail_state
            });
            let head = path.as_deref().and_then(sessions::read_transcript);
            Found {
                start: p.start,
                background: false,
                info: OutsideSession {
                    session_id: a.id,
                    pid: Some(p.pid),
                    cwd: c.cwd.clone(),
                    title: head.as_ref().and_then(|h| h.title.clone()),
                    where_: host_of(&table, p.pid),
                    state: state.to_string(),
                    ambiguous: a.ambiguous,
                    last_message,
                    updated_at: changed_at.and_then(sessions::rfc3339),
                },
            }
        })
        .collect();
    (found, again)
}

/// An entry of `claude agents --json`: newer versions list interactive
/// sessions too (`"kind": "interactive"`), with their pid and status.
#[derive(Debug, Clone, PartialEq)]
pub struct Listed {
    pub id: String,
    pub pid: Option<u32>,
    pub interactive: bool,
    /// `working`, `waiting` or `idle`, when the status says.
    pub state: Option<&'static str>,
}

fn listed_state(status: &str) -> Option<&'static str> {
    match status.to_ascii_lowercase().as_str() {
        "busy" | "running" | "working" | "active" | "starting" => Some("working"),
        "waiting" | "needs_input" | "blocked" => Some("waiting"),
        "idle" => Some("idle"),
        _ => None,
    }
}

pub fn parse_listed(text: &str) -> Vec<Listed> {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return vec![];
    };
    let list = match &v {
        Value::Array(a) => a.clone(),
        Value::Object(o) => o
            .get("sessions")
            .or_else(|| o.get("agents"))
            .and_then(|a| a.as_array())
            .cloned()
            .unwrap_or_default(),
        _ => vec![],
    };
    let get = |e: &Value, keys: &[&str]| -> Option<String> {
        keys.iter()
            .find_map(|k| e.get(k).and_then(|x| x.as_str()).map(str::to_string))
    };
    list.iter()
        .filter_map(|e| {
            Some(Listed {
                id: get(e, &["sessionId", "session_id", "id"])?,
                pid: e
                    .get("pid")
                    .and_then(|p| p.as_u64())
                    .map(|p| p as u32)
                    .filter(|p| *p > 0),
                interactive: get(e, &["kind"]).as_deref() == Some("interactive"),
                state: get(e, &["status", "state"]).and_then(|s| listed_state(&s)),
            })
        })
        .collect()
}

fn listed_for(listed: &[Listed], pid: u32) -> Option<&Listed> {
    listed.iter().find(|l| l.pid == Some(pid))
}

/// Running background sessions of `claude agents --json`, taken at
/// `listed_at`. Interactive ones are left to the process scan, which knows
/// where they run and skips the ones driven by a program (an SDK, the desktop
/// app, Sphere itself). True with them when a status needs a new list.
async fn scan_background(
    cfg: &Config,
    text: Option<&str>,
    listed_at: SystemTime,
    just_listed: bool,
) -> (Vec<Found>, bool) {
    let Some(text) = text else {
        return (vec![], false);
    };
    let listed = match sessions::parse_agents_json(text) {
        Ok(l) => l,
        Err(e) => {
            tracing::debug!("could not list background sessions: {e:#}");
            return (vec![], false);
        }
    };
    let interactive: HashSet<String> = parse_listed(text)
        .into_iter()
        .filter(|l| l.interactive)
        .map(|l| l.id)
        .collect();
    let projects = cfg.projects_dir();
    let mut again = false;
    let found = listed
        .into_iter()
        .filter(|s| s.running && !interactive.contains(&s.session_id))
        .map(|s| {
            let path = find_transcript(&projects, &s.session_id, s.cwd.as_deref());
            let (tail_state, last_message) = path
                .as_deref()
                .map(transcript_state)
                .unwrap_or(("unknown", None));
            let state = pick_state(
                s.state.as_deref().filter(|st| *st != "unknown"),
                tail_state,
                modified(path.as_deref()).is_some_and(|m| m > listed_at),
                just_listed,
            )
            .unwrap_or_else(|| {
                again = true;
                tail_state
            })
            .to_string();
            let head = path.as_deref().and_then(sessions::read_transcript);
            Found {
                start: None,
                background: true,
                info: OutsideSession {
                    session_id: Some(s.session_id),
                    pid: s.pid,
                    cwd: s.cwd.or_else(|| head.as_ref().and_then(|h| h.cwd.clone())),
                    title: s.title.or_else(|| head.and_then(|h| h.title)),
                    where_: BACKGROUND.into(),
                    state,
                    ambiguous: false,
                    last_message,
                    updated_at: s.updated_at,
                },
            }
        })
        .collect();
    (found, again)
}

/// Every session running outside Sphere, newest first. `roots` are the pids
/// of Sphere's own agents, which are skipped along with their children.
/// `probe` keeps the slow part: it is taken again when a Claude process
/// started or stopped, when a transcript shows a tool call made after the
/// list (running, or waiting for an answer?), after a minute, or when `fresh`.
pub async fn scan(
    cfg: &Config,
    roots: Vec<i32>,
    probe: &mut Option<Probe>,
    fresh: bool,
) -> Vec<Found> {
    let procs = tokio::task::spawn_blocking(read_procs)
        .await
        .unwrap_or_default();
    scan_with(cfg, procs, roots, probe, fresh).await
}

async fn scan_with(
    cfg: &Config,
    procs: Vec<Proc>,
    roots: Vec<i32>,
    probe: &mut Option<Probe>,
    fresh: bool,
) -> Vec<Found> {
    let claudes = claude_procs(&procs);
    let mut renew = fresh || !probe.as_ref().is_some_and(|p| p.stands(&claudes));
    let (background, found) = loop {
        if renew {
            *probe = Some(Probe::take(cfg, claudes.clone(), probe.take()).await);
        }
        let Some(p) = probe.as_mut() else {
            return vec![];
        };
        let listed = p.listed.as_deref().map(parse_listed).unwrap_or_default();
        let (projects, me, at) = (cfg.projects_dir(), std::process::id(), p.at);
        let (procs, roots) = (procs.clone(), roots.clone());
        let mut cwd_of = std::mem::take(&mut p.cwds);
        let ((found, again), cwd_of) = tokio::task::spawn_blocking(move || {
            let found = scan_procs(
                &projects,
                &procs,
                &roots,
                me,
                &listed,
                at,
                renew,
                &mut cwd_of,
            );
            (found, cwd_of)
        })
        .await
        .unwrap_or_default();
        p.cwds = cwd_of;
        let (background, again_bg) = scan_background(cfg, p.listed.as_deref(), at, renew).await;
        if renew || !(again || again_bg) {
            break (background, found);
        }
        renew = true;
    };
    let mut out = background;
    for f in found {
        let dup = out.iter().any(|b| {
            (f.info.pid.is_some() && b.info.pid == f.info.pid)
                || (f.info.session_id.is_some() && b.info.session_id == f.info.session_id)
        });
        if !dup {
            out.push(f);
        }
    }
    out.sort_by(|a, b| {
        b.info
            .updated_at
            .cmp(&a.info.updated_at)
            .then(a.info.pid.cmp(&b.info.pid))
    });
    out
}

// ---- stopping -------------------------------------------------------------------

/// Gone, or a zombie nobody has reaped yet.
fn pid_gone(pid: u32) -> bool {
    if unsafe { libc::kill(pid as i32, 0) } != 0 {
        return true;
    }
    std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .map(|o| {
            let s = String::from_utf8_lossy(&o.stdout);
            s.trim().is_empty() || s.trim().starts_with('Z')
        })
        .unwrap_or(false)
}

fn wait_gone(pid: u32, secs: f64) -> bool {
    let end = Instant::now() + Duration::from_secs_f64(secs);
    while Instant::now() < end {
        if pid_gone(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    pid_gone(pid)
}

/// Terminate one claude process: SIGTERM, then SIGKILL after five seconds.
/// Refuses if the pid now belongs to something else.
fn kill_claude(pid: u32, start: Option<u64>) -> Result<()> {
    let out = ps_command(&["-p", &pid.to_string()])
        .stdin(std::process::Stdio::null())
        .output()?;
    // `-p` with no `-ax` still prints the same columns
    let Some(p) = parse_ps(&String::from_utf8_lossy(&out.stdout))
        .into_iter()
        .next()
    else {
        return Ok(()); // already gone
    };
    if p.pid != pid || !is_claude(&p) || (start.is_some() && p.start != start) {
        bail!("pid {pid} is no longer the Claude session found; nothing was stopped");
    }
    unsafe { libc::kill(pid as i32, libc::SIGTERM) };
    if !wait_gone(pid, 5.0) {
        unsafe { libc::kill(pid as i32, libc::SIGKILL) };
        if !wait_gone(pid, 3.0) {
            bail!("could not stop pid {pid}");
        }
    }
    Ok(())
}

/// Stop a session found outside Sphere. Only its own process, never its shell
/// or its terminal.
pub async fn stop(cfg: &Config, f: &Found) -> Result<()> {
    let Some(id) = f.info.session_id.clone() else {
        bail!("the session cannot be told apart from another one");
    };
    if f.background {
        sessions::run_json(&cfg.session_stop_cmd(&id)).await?;
        if let Some(pid) = f.info.pid {
            tokio::task::spawn_blocking(move || wait_gone(pid, 5.0)).await?;
        }
        return Ok(());
    }
    let Some(pid) = f.info.pid else {
        bail!("no process to stop");
    };
    let start = f.start;
    tokio::task::spawn_blocking(move || kill_claude(pid, start)).await?
}

/// Wait until the file has had no writes for `quiet`, at most `cap`.
pub async fn wait_quiet(path: &Path, quiet: Duration, cap: Duration) {
    let stamp = || {
        std::fs::metadata(path)
            .ok()
            .map(|m| (m.modified().ok(), m.len()))
    };
    let end = Instant::now() + cap;
    let (mut last, mut since) = (stamp(), Instant::now());
    while Instant::now() < end && since.elapsed() < quiet {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let now = stamp();
        if now != last {
            last = now;
            since = Instant::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_agents_list_marks_interactive_sessions_and_their_status() {
        let l = parse_listed(
            r#"[{"sessionId":"a","pid":39402,"status":"idle","kind":"interactive"},
                {"sessionId":"b","pid":7,"status":"busy","kind":"interactive"},
                {"sessionId":"c","status":"waiting","kind":"background"},
                {"sessionId":"d","state":"running"},{"nope":1}]"#,
        );
        assert_eq!(l.len(), 4);
        assert!(l[0].interactive && l[0].state == Some("idle") && l[0].pid == Some(39402));
        assert_eq!(l[1].state, Some("working"));
        assert!(!l[2].interactive && l[2].state == Some("waiting") && l[2].pid.is_none());
        assert!(!l[3].interactive && l[3].state == Some("working"));
        assert_eq!(listed_for(&l, 7).map(|x| x.id.as_str()), Some("b"));
        assert!(parse_listed("garbage").is_empty());
    }

    const PS: &str = "\
    1     0 Mon Oct 19 08:00:00 2026 /sbin/launchd                 /sbin/launchd
  200     1 Mon Oct 19 08:01:00 2026 /Applications/Utilities/Terminal.app/Contents/MacOS/Terminal /Applications/Utilities/Terminal.app/Contents/MacOS/Terminal
  210   200 Mon Oct 19 08:01:01 2026 login                       login -pf mac
  211   210 Mon Oct 19 08:01:01 2026 -zsh                        -zsh
  300   211 Mon Oct 19 14:23:55 2026 /Users/mac/.local/bin/claude claude --resume 11111111-2222-3333-4444-555555555555
  310     1 Mon Oct 19 09:00:00 2026 tmux: server                tmux new -s w
  311   310 Mon Oct 19 09:00:01 2026 zsh                         -zsh
  320   311 Mon Oct 19 09:00:05 2026 node                        node /usr/lib/node_modules/@anthropic-ai/claude-code/cli.js
  330   311 Mon Oct 19 09:00:06 2026 claude                      claude -p fix it
  340   311 Mon Oct 19 09:00:07 2026 claude                      claude mcp list
  350   311 Mon Oct 19 09:00:08 2026 claude                      claude --version
  360   311 Mon Oct 19 09:00:09 2026 claude                      claude --output-format stream-json --input-format stream-json
  370   311 Mon Oct 19 09:00:10 2026 vim                         vim claude-code/cli.js
";

    fn procs() -> Vec<Proc> {
        parse_ps(PS)
    }

    #[test]
    fn parses_ps_lines() {
        let p = procs();
        assert_eq!(p.len(), 13);
        assert_eq!(p[0].pid, 1);
        assert_eq!(p[0].ppid, 0);
        assert!(p[0].start.is_some());
        assert_eq!(
            p[1].comm,
            "/Applications/Utilities/Terminal.app/Contents/MacOS/Terminal"
        );
        assert_eq!(p[4].comm, "/Users/mac/.local/bin/claude");
        assert_eq!(
            p[4].args,
            "claude --resume 11111111-2222-3333-4444-555555555555"
        );
        // 14:23:55 is later than 08:00:00 on the same day
        assert!(p[4].start.unwrap() > p[0].start.unwrap());
        assert_eq!(
            p[4].start.unwrap() - p[0].start.unwrap(),
            6 * 3600 + 23 * 60 + 55
        );
        assert!(parse_ps("garbage\n\n12 x").is_empty());
    }

    #[test]
    fn splits_a_comm_with_spaces() {
        let (c, a) = split_comm_args(
            "/Applications/Visual Studio Code.app/Contents/MacOS/Electron /Applications/Visual Studio Code.app/Contents/MacOS/Electron --x",
        );
        assert_eq!(
            c,
            "/Applications/Visual Studio Code.app/Contents/MacOS/Electron"
        );
        assert!(a.ends_with("--x"));
        // Linux truncates comm and may differ from args
        let (c, a) = split_comm_args("tmux: server tmux new");
        assert_eq!((c.as_str(), a.as_str()), ("tmux:", "server tmux new"));
    }

    #[test]
    fn finds_claude_and_skips_helper_forms() {
        let p = procs();
        let by = |pid: u32| p.iter().find(|x| x.pid == pid).unwrap();
        let keep = |pid: u32| is_claude(by(pid)) && !is_helper(by(pid));
        assert!(keep(300));
        assert!(keep(320), "node running cli.js");
        assert!(is_claude(by(330)) && is_helper(by(330)), "print mode");
        assert!(is_helper(by(340)), "subcommand");
        assert!(is_helper(by(350)), "--version");
        assert!(is_helper(by(360)), "stream-json is another tool driving it");
        assert!(!is_claude(by(370)), "vim is not claude");
        assert!(!is_claude(by(211)));
    }

    #[test]
    fn reads_an_explicit_session_id() {
        let mk = |args: &str| Proc {
            pid: 1,
            ppid: 0,
            start: None,
            comm: "claude".into(),
            args: args.into(),
        };
        let id = "11111111-2222-3333-4444-555555555555";
        for a in [
            format!("claude --resume {id}"),
            format!("claude -r {id} --model x"),
            format!("claude --resume={id}"),
            format!("claude --dangerously-skip-permissions --session-id {id}"),
        ] {
            assert_eq!(explicit_session(&mk(&a)).as_deref(), Some(id), "{a}");
        }
        assert_eq!(explicit_session(&mk("claude --resume")), None);
        assert_eq!(explicit_session(&mk("claude --resume fix-the-bug")), None);
        assert_eq!(explicit_session(&mk("claude")), None);
    }

    #[test]
    fn names_the_host_from_the_parent_chain() {
        let p = procs();
        let table: HashMap<u32, &Proc> = p.iter().map(|x| (x.pid, x)).collect();
        assert_eq!(host_of(&table, 300), "Terminal");
        assert_eq!(host_of(&table, 320), "tmux");
        // nothing known above launchd
        assert_eq!(host_of(&table, 210), "Terminal");
        assert_eq!(host_of(&table, 200), "a terminal");
        for (comm, want) in [
            ("/usr/bin/herdr", "herdr"),
            ("/Applications/iTerm.app/Contents/MacOS/iTerm2", "iTerm2"),
            ("/Applications/Ghostty.app/Contents/MacOS/ghostty", "Ghostty"),
            ("wezterm-gui", "WezTerm"),
            ("/Applications/Warp.app/Contents/MacOS/stable", "Warp"),
            ("/Applications/Visual Studio Code.app/Contents/MacOS/Electron", "VS Code"),
            ("/Applications/Visual Studio Code.app/Contents/Frameworks/Code Helper (Plugin).app/Contents/MacOS/Code Helper (Plugin)", "VS Code"),
            ("/Applications/Cursor.app/Contents/MacOS/Cursor", "Cursor"),
            ("sshd-session", "over SSH"),
            ("/usr/local/bin/zellij", "zellij"),
            ("SCREEN", "screen"),
            ("kitty", "kitty"),
            ("alacritty", "Alacritty"),
            ("zed", "Zed"),
        ] {
            assert_eq!(host_name(comm), Some(want), "{comm}");
        }
        assert_eq!(host_name("/bin/zsh"), None);
        assert_eq!(host_name("/usr/bin/stable-thing"), None);
    }

    #[test]
    fn skips_what_descends_from_sphere() {
        let p = procs();
        let table: HashMap<u32, &Proc> = p.iter().map(|x| (x.pid, x)).collect();
        let roots: HashSet<u32> = [310].into();
        assert!(descends(&table, 320, &roots));
        assert!(descends(&table, 310, &roots));
        assert!(!descends(&table, 300, &roots));
    }

    #[test]
    fn parses_lsof_cwd_output() {
        let m = parse_lsof_cwd(
            "p300\nfcwd\nn/Users/mac/code/app\np320\nfcwd\nn/tmp/with space\nn/ignored\nbad\n",
        );
        assert_eq!(m.len(), 2);
        assert_eq!(m[&300], "/Users/mac/code/app");
        assert_eq!(m[&320], "/tmp/with space");
    }

    #[test]
    fn encodes_a_folder_like_claude_does() {
        assert_eq!(encode_cwd("/Users/mac/my.app_x"), "-Users-mac-my-app-x");
    }

    fn cand(pid: u32, cwd: &str, start: u64, explicit: Option<&str>) -> Cand {
        Cand {
            pid,
            cwd: Some(cwd.into()),
            start: Some(start),
            explicit: explicit.map(str::to_string),
        }
    }

    fn tx(id: &str, cwd: &str, mtime: u64) -> Tx {
        Tx {
            id: id.into(),
            cwd: cwd.into(),
            mtime,
        }
    }

    #[test]
    fn maps_a_process_to_the_newest_transcript_since_it_started() {
        let txs = [
            tx("old", "/r", 50),
            tx("mine", "/r", 150),
            tx("newer-elsewhere", "/other", 160),
        ];
        let a = assign(&[cand(1, "/r", 100, None)], &txs);
        assert_eq!(a[0].id.as_deref(), Some("mine"));
        // nothing written since it started
        let a = assign(&[cand(1, "/r", 200, None)], &txs);
        assert_eq!(a[0], Assigned::default());
    }

    #[test]
    fn two_processes_in_one_folder_are_ambiguous() {
        let txs = [tx("a", "/r", 150), tx("b", "/r", 160)];
        let a = assign(&[cand(1, "/r", 100, None), cand(2, "/r", 110, None)], &txs);
        assert!(a.iter().all(|x| x.ambiguous && x.id.is_none()));
    }

    #[test]
    fn an_explicit_id_settles_it_and_is_not_handed_out_twice() {
        let txs = [tx("a", "/r", 150), tx("b", "/r", 160)];
        let a = assign(
            &[cand(1, "/r", 100, Some("b")), cand(2, "/r", 110, None)],
            &txs,
        );
        assert_eq!(a[0].id.as_deref(), Some("b"));
        assert_eq!(a[1].id.as_deref(), Some("a"));
        assert!(!a[0].ambiguous && !a[1].ambiguous);
        // two explicit ones are not ambiguous either
        let a = assign(
            &[cand(1, "/r", 100, Some("a")), cand(2, "/r", 110, Some("b"))],
            &txs,
        );
        assert_eq!(a[1].id.as_deref(), Some("b"));
        // a lone process may not take the transcript an explicit one owns
        let a = assign(
            &[cand(1, "/r", 100, Some("b")), cand(2, "/q", 110, None)],
            &txs,
        );
        assert_eq!(a[1].id, None);
    }

    fn line(kind: &str, content: Value, stop: Option<&str>) -> String {
        serde_json::json!({"type": kind, "cwd": "/r",
            "message": {"role": kind, "content": content, "stop_reason": stop}})
        .to_string()
            + "\n"
    }

    fn text(t: &str) -> Value {
        serde_json::json!([{"type": "text", "text": t}])
    }

    #[test]
    fn tells_working_waiting_and_idle_from_the_tail() {
        let noise = "{\"type\":\"summary\",\"summary\":\"s\"}\n{\"type\":\"file-history-snapshot\"}\nnot json\n";
        let user = line("user", serde_json::json!("fix the bug"), None);
        let say = line("assistant", text("Looking now.\nMore"), None);
        let tool = line(
            "assistant",
            serde_json::json!([{"type":"tool_use","id":"t1","name":"Bash","input":{}}]),
            Some("tool_use"),
        );
        let result = line(
            "user",
            serde_json::json!([{"type":"tool_result","tool_use_id":"t1","content":"ok"}]),
            None,
        );
        let done = line("assistant", text("All fixed."), Some("end_turn"));

        let (s, m) = tail_summary(&format!("{user}{say}{noise}"));
        assert_eq!(s, "idle");
        assert_eq!(m.as_deref(), Some("Looking now."));
        assert_eq!(tail_summary(&user).0, "working");
        assert_eq!(tail_summary(&format!("{user}{say}{tool}")).0, "waiting");
        let (s, m) = tail_summary(&format!("{user}{say}{tool}{result}{noise}"));
        assert_eq!(s, "working");
        assert_eq!(m.as_deref(), Some("Looking now."));
        let (s, m) = tail_summary(&format!("{user}{tool}{result}{done}{noise}"));
        assert_eq!(s, "idle");
        assert_eq!(m.as_deref(), Some("All fixed."));
        // an old tool call that never got a result does not matter after a new prompt
        let again = line("user", serde_json::json!("next"), None);
        assert_eq!(tail_summary(&format!("{tool}{again}{done}")).0, "idle");
        let stopped = line("user", text("[Request interrupted by user]"), None);
        assert_eq!(tail_summary(&format!("{user}{say}{stopped}")).0, "idle");
        assert_eq!(tail_summary(noise).0, "unknown");
        assert_eq!(tail_summary("").0, "unknown");
    }

    #[test]
    fn the_last_message_is_one_short_line() {
        let long = "x".repeat(300);
        let (_, m) = tail_summary(&line("assistant", text(&long), Some("end_turn")));
        let m = m.unwrap();
        assert_eq!(m.chars().count(), 140);
        assert!(m.ends_with('…'));
    }

    #[test]
    fn reads_only_the_tail_of_a_big_transcript() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("s.jsonl");
        let filler = line("assistant", text(&"y".repeat(1000)), Some("end_turn"));
        let mut body = filler.repeat(200);
        body.push_str(&line("user", serde_json::json!("go"), None));
        std::fs::write(&p, body).unwrap();
        assert!(read_tail(&p).unwrap().len() <= TAIL_BYTES as usize);
        assert_eq!(transcript_state(&p).0, "working");
        assert_eq!(transcript_state(&tmp.path().join("none")).0, "unknown");
    }

    #[test]
    fn the_real_process_table_parses() {
        let out = ps_command(&["-ax"]).output().unwrap();
        let procs = parse_ps(&String::from_utf8_lossy(&out.stdout));
        let me = std::process::id();
        let mine = procs.iter().find(|p| p.pid == me).expect("this process");
        assert!(mine.start.is_some(), "{mine:?}");
        assert!(mine.ppid > 0);
    }

    fn proc_at(pid: u32, start: u64, args: &str) -> Proc {
        Proc {
            pid,
            ppid: 1,
            start: Some(start),
            comm: args.split_whitespace().next().unwrap().into(),
            args: args.into(),
        }
    }

    #[test]
    fn the_claude_processes_are_sessions_and_helpers_but_not_subcommands() {
        let procs = vec![
            proc_at(30, 100, "claude --resume abc"),
            proc_at(10, 100, "/usr/local/bin/claude"),
            proc_at(20, 100, "claude -p hello"),
            proc_at(40, 100, "claude agents --json"),
            proc_at(
                50,
                100,
                "node /x/@anthropic-ai/claude-code/cli.js mcp serve",
            ),
            proc_at(60, 100, "bash"),
        ];
        let some = |pid| (pid, Some(100));
        assert_eq!(claude_procs(&procs), vec![some(10), some(20), some(30)]);
        // the same pid started again is another process
        let mut again = procs.clone();
        again[1].start = Some(200);
        assert_ne!(claude_procs(&again), claude_procs(&procs));
    }

    #[test]
    fn a_listed_status_stands_until_the_transcript_changes_after_the_list() {
        assert_eq!(
            pick_state(Some("working"), "idle", false, false),
            Some("working")
        );
        assert_eq!(
            pick_state(Some("waiting"), "working", true, false),
            Some("working")
        );
        assert_eq!(
            pick_state(Some("working"), "idle", true, false),
            Some("idle")
        );
        // a transcript that can't be read says nothing new
        assert_eq!(
            pick_state(Some("idle"), "unknown", true, false),
            Some("idle")
        );
        assert_eq!(pick_state(None, "idle", false, false), Some("idle"));
        // an open tool call may be running or waiting for an answer: ask again
        assert_eq!(pick_state(Some("working"), "waiting", true, false), None);
        assert_eq!(
            pick_state(Some("working"), "waiting", true, true),
            Some("working")
        );
        assert_eq!(pick_state(None, "waiting", true, false), Some("waiting"));
    }

    #[tokio::test]
    async fn the_agents_list_runs_only_when_a_claude_process_starts_or_stops() {
        let tmp = tempfile::tempdir().unwrap();
        let count = tmp.path().join("runs");
        // a stand-in for `claude agents --json` that counts its runs
        let cfg = Config {
            session_list_command: Some(vec![
                "sh".into(),
                "-c".into(),
                "echo run >> \"$0\"; echo '[]'".into(),
                count.to_string_lossy().into(),
            ]),
            claude_projects_dir: Some(tmp.path().join("projects")),
            ..Config::default()
        };
        let runs = || {
            std::fs::read_to_string(&count)
                .map(|s| s.lines().count())
                .unwrap_or(0)
        };
        // pids no process has, so their folders can't be read
        let one = vec![proc_at(4_000_001, 100, "claude")];
        let mut probe = None;
        let found = scan_with(&cfg, one.clone(), vec![], &mut probe, false).await;
        assert_eq!((runs(), found.len()), (1, 1));
        for _ in 0..3 {
            scan_with(&cfg, one.clone(), vec![], &mut probe, false).await;
        }
        assert_eq!(runs(), 1, "nothing started or stopped: ps alone");
        let mut two = one.clone();
        two.push(proc_at(4_000_002, 150, "claude --resume x"));
        let found = scan_with(&cfg, two, vec![], &mut probe, false).await;
        assert_eq!((runs(), found.len()), (2, 2), "a session started");
        scan_with(&cfg, one.clone(), vec![], &mut probe, false).await;
        assert_eq!(runs(), 3, "and stopped");
        // the list running (a subcommand) is not a session starting
        let mut listing = one.clone();
        listing.push(proc_at(4_000_003, 160, "claude agents --json"));
        scan_with(&cfg, listing, vec![], &mut probe, false).await;
        assert_eq!(runs(), 3);
        // a take-over asks afresh
        scan_with(&cfg, one, vec![], &mut probe, true).await;
        assert_eq!(runs(), 4);
    }

    #[tokio::test]
    async fn an_open_tool_call_written_after_the_list_asks_for_it_again() {
        let tmp = tempfile::tempdir().unwrap();
        let count = tmp.path().join("runs");
        // the list knows the session of pid 4000001 and says it is busy
        let cfg = Config {
            session_list_command: Some(vec![
                "sh".into(),
                "-c".into(),
                r#"echo run >> "$0"; echo '[{"sessionId":"s1","pid":4000001,"status":"busy","kind":"interactive"}]'"#.into(),
                count.to_string_lossy().into(),
            ]),
            claude_projects_dir: Some(tmp.path().join("projects")),
            ..Config::default()
        };
        let runs = || {
            std::fs::read_to_string(&count)
                .map(|s| s.lines().count())
                .unwrap_or(0)
        };
        let dir = tmp.path().join("projects").join("-x");
        std::fs::create_dir_all(&dir).unwrap();
        let write =
            |lines: &[&str]| std::fs::write(dir.join("s1.jsonl"), lines.join("\n") + "\n").unwrap();
        let prompt = r#"{"type":"user","message":{"role":"user","content":"add a flag"}}"#;
        let call = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"cargo test"}}]}}"#;
        let result = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}}"#;
        let done = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Done."}],"stop_reason":"end_turn"}}"#;
        write(&[prompt]);
        let one = vec![proc_at(4_000_001, 100, "claude")];
        let mut probe = None;
        let state = |f: &[Found]| f[0].info.state.clone();
        let found = scan_with(&cfg, one.clone(), vec![], &mut probe, false).await;
        assert_eq!((runs(), state(&found).as_str()), (1, "working"));
        // a tool call: running or waiting for an answer, which only the list knows
        tokio::time::sleep(Duration::from_millis(20)).await;
        write(&[prompt, call]);
        let found = scan_with(&cfg, one.clone(), vec![], &mut probe, false).await;
        assert_eq!((runs(), state(&found).as_str()), (2, "working"));
        scan_with(&cfg, one.clone(), vec![], &mut probe, false).await;
        assert_eq!(runs(), 2, "asked once");
        // its result and the end of the turn: the transcript says
        tokio::time::sleep(Duration::from_millis(20)).await;
        write(&[prompt, call, result, done]);
        let found = scan_with(&cfg, one, vec![], &mut probe, false).await;
        assert_eq!((runs(), state(&found).as_str()), (2, "idle"));
    }
}
