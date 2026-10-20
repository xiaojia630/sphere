//! git and gh on the pretend machine: two repos (shop-api and sphere) with their history, branches, pull requests
//! and the worktrees agents work in. Reads answer from the fixtures; the things the apps do (stage, commit, switch,
//! push, merge, discard a worktree) are remembered, so the apps show what they did.
#![allow(dead_code)]
use super::fixtures::{self, Ch, Line, Rp};
use super::ops::{iso, now};
use super::HOME;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

// ------------------------------------------------------------------ what has been done

#[derive(Clone)]
enum Ev {
    Commit { repo: String, branch: String, subject: String, at: i64, merge: Option<String> },
    Branch { repo: String, name: String },
    Switch { repo: String, name: String },
    Push { repo: String, branch: String },
}

#[derive(Clone)]
struct State {
    events: Vec<Ev>,
    /// (repo, slug, when it was first looked at): agents started in the playground
    seen: Vec<(String, String, i64)>,
    removed: Vec<String>,
    /// (checkout, path)
    staged: Vec<(String, String)>,
    committed: Vec<(String, String)>,
    merged_prs: Vec<(String, u64)>,
}

static STATE: Mutex<State> = Mutex::new(State { events: Vec::new(), seen: Vec::new(), removed: Vec::new(), staged: Vec::new(), committed: Vec::new(), merged_prs: Vec::new() });

fn state() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

// ------------------------------------------------------------------ where a command runs

/// A checkout: a repo's own, or an agent's worktree.
struct Cx {
    rp: &'static Rp,
    /// the worktree's slug
    wt: Option<String>,
    top: String,
    /// where in the checkout `dir` is, without a leading or trailing slash
    prefix: String,
}

fn within(s: &str, top: &str) -> Option<String> {
    if s == top {
        Some(String::new())
    } else {
        s.strip_prefix(top).and_then(|r| r.strip_prefix('/')).map(str::to_string)
    }
}

fn ctx(dir: &Path) -> Option<Cx> {
    let s = dir.to_string_lossy().trim_end_matches('/').to_string();
    for rp in fixtures::REPOS.iter() {
        let top = format!("{HOME}/Projects/{}", rp.name);
        if let Some(prefix) = within(&s, &top) {
            return Some(Cx { rp, wt: None, top, prefix });
        }
    }
    let root = format!("{HOME}/.sphere/worktrees/");
    let rest = s.strip_prefix(&root)?;
    let mut it = rest.splitn(3, '/');
    let (name, slug, sub) = (it.next()?, it.next()?, it.next().unwrap_or(""));
    let rp = fixtures::REPOS.iter().find(|r| r.name == name)?;
    let top = format!("{root}{name}/{slug}");
    let mut st = state();
    if slug.is_empty() || st.removed.contains(&top) {
        return None;
    }
    if !st.seen.iter().any(|x| x.0 == name && x.1 == slug) {
        st.seen.push((name.to_string(), slug.to_string(), now()));
    }
    Some(Cx { rp, wt: Some(slug.to_string()), top, prefix: sub.trim_end_matches('/').to_string() })
}

// ------------------------------------------------------------------ the history

struct Cm {
    id: String,
    key: String,
    parents: Vec<usize>,
    who: u8,
    born: i64,
    subject: String,
}

/// A repo's commits and refs as they stand now.
struct World {
    rp: &'static Rp,
    cms: Vec<Cm>,
    locals: Vec<(String, usize)>,
    remotes: Vec<(String, usize)>,
    cur: String,
    /// every line of work: branch, its fixture, and when it began (for an agent started here)
    lines: Vec<(String, &'static Line, i64)>,
    /// branches merged into main by the apps
    merged: Vec<String>,
}

/// 40 hex digits from a name, the same every time.
fn sha(seed: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in seed.bytes() {
        h = (h ^ b as u64).wrapping_mul(0x0100_0000_01b3);
    }
    let mut out = String::new();
    for i in 0..3u64 {
        h = h.wrapping_mul(0x9E37_79B9_7F4A_7C15).rotate_left(23) ^ i;
        out += &format!("{h:016x}");
    }
    out.truncate(40);
    out
}

fn world(rp: &'static Rp) -> World {
    let t = now();
    let st = state().clone();
    let mut lines: Vec<(String, &'static Line, i64)> = rp.lines.iter().map(|l| (l.branch.to_string(), l, t)).collect();
    for (repo, slug, at) in &st.seen {
        let branch = format!("sphere/{slug}");
        if repo == rp.name && !lines.iter().any(|l| l.0 == branch) {
            lines.push((branch, &rp.generic, *at));
        }
    }
    let mut cms: Vec<Cm> = Vec::new();
    let mut want: Vec<Vec<String>> = Vec::new();
    let mut add = |key: String, parents: Vec<String>, who: u8, born: i64, subject: String, cms: &mut Vec<Cm>| {
        cms.push(Cm { id: sha(&format!("{}:{key}", rp.name)), key, parents: Vec::new(), who, born, subject });
        want.push(parents);
    };
    for h in rp.history {
        add(h.key.to_string(), h.parents.iter().map(|p| p.to_string()).collect(), h.who, t - (h.age * 3600.0) as i64, h.subject.to_string(), &mut cms);
    }
    for (branch, l, at) in &lines {
        for (i, (subject, age)) in l.commits.iter().enumerate() {
            let parent = if i == 0 { l.fork.to_string() } else { format!("{branch}#{}", i - 1) };
            add(format!("{branch}#{i}"), vec![parent], 0, at - (age * 3600.0) as i64, subject.to_string(), &mut cms);
        }
    }
    let index: HashMap<String, usize> = cms.iter().enumerate().map(|(i, c)| (c.key.clone(), i)).collect();
    for (c, p) in cms.iter_mut().zip(want) {
        c.parents = p.iter().filter_map(|k| index.get(k).copied()).collect();
    }
    let last = |branch: &str, l: &Line| index[&format!("{branch}#{}", l.commits.len() - 1)];
    let main = index[rp.history.last().map(|h| h.key).unwrap_or("")];
    let mut locals = vec![("main".to_string(), main)];
    let mut remotes = vec![("origin/main".to_string(), index[rp.origin])];
    for (branch, l, _) in &lines {
        if l.local || l.worktree {
            locals.push((branch.clone(), last(branch, l)));
        }
        if l.remote {
            remotes.push((format!("origin/{branch}"), last(branch, l)));
        }
    }
    let mut w = World { rp, cms, locals, remotes, cur: "main".into(), lines, merged: Vec::new() };
    for ev in &st.events {
        match ev {
            Ev::Commit { repo, branch, subject, at, merge } if repo == rp.name => {
                let Some(tip) = w.tip(branch) else { continue };
                let mut parents = vec![tip];
                if let Some(other) = merge.as_deref().and_then(|b| w.tip(b)) {
                    parents.push(other);
                    w.merged.push(merge.clone().unwrap_or_default());
                }
                let n = w.cms.len();
                w.cms.push(Cm { id: sha(&format!("{}:visitor:{n}", rp.name)), key: format!("v{n}"), parents, who: 0, born: *at, subject: subject.clone() });
                w.set(branch, n);
            }
            Ev::Branch { repo, name } if repo == rp.name => {
                if let Some(tip) = w.tip(&w.cur.clone()) {
                    w.set(name, tip);
                }
            }
            Ev::Switch { repo, name } if repo == rp.name => w.cur = name.clone(),
            Ev::Push { repo, branch } if repo == rp.name => {
                if let Some(tip) = w.tip(branch) {
                    let r = format!("origin/{branch}");
                    match w.remotes.iter_mut().find(|x| x.0 == r) {
                        Some(x) => x.1 = tip,
                        None => w.remotes.push((r, tip)),
                    }
                }
            }
            _ => {}
        }
    }
    w
}

impl World {
    fn tip(&self, branch: &str) -> Option<usize> {
        self.locals.iter().find(|l| l.0 == branch).map(|l| l.1)
    }
    fn set(&mut self, branch: &str, tip: usize) {
        match self.locals.iter_mut().find(|l| l.0 == branch) {
            Some(l) => l.1 = tip,
            None => self.locals.push((branch.to_string(), tip)),
        }
    }
    fn short(&self, i: usize) -> &str {
        &self.cms[i].id[..7]
    }
    fn upstream(&self, branch: &str) -> Option<(String, usize)> {
        let n = format!("origin/{branch}");
        self.remotes.iter().find(|r| r.0 == n).map(|r| (n, r.1))
    }
    /// Every commit reachable from `i`, itself included.
    fn anc(&self, i: usize) -> Vec<bool> {
        let mut seen = vec![false; self.cms.len()];
        let mut todo = vec![i];
        while let Some(c) = todo.pop() {
            if !std::mem::replace(&mut seen[c], true) {
                todo.extend(&self.cms[c].parents);
            }
        }
        seen
    }
    /// A commit named the way git is told: HEAD, a branch, `origin/x`, `x^{commit}`, `HEAD~2`, a hash.
    fn resolve(&self, name: &str, head: usize) -> Option<usize> {
        let n = name.trim_end_matches("^{commit}");
        let (base, back) = match n.split_once('~') {
            Some((b, k)) => (b, k.parse::<usize>().unwrap_or(1)),
            None => match n.strip_suffix('^') {
                Some(b) => (b, 1),
                None => (n, 0),
            },
        };
        let base = base.trim_start_matches("refs/heads/").trim_start_matches("refs/remotes/");
        let mut i = if base == "HEAD" {
            head
        } else if let Some(t) = self.tip(base) {
            t
        } else if let Some(r) = self.remotes.iter().find(|r| r.0 == base) {
            r.1
        } else if base.len() >= 4 && base.bytes().all(|b| b.is_ascii_hexdigit()) {
            self.cms.iter().position(|c| c.id.starts_with(base))?
        } else {
            return None;
        };
        for _ in 0..back {
            i = *self.cms[i].parents.first()?;
        }
        Some(i)
    }
    /// Newest first (a child always before its parents).
    fn order(&self, set: &[bool]) -> Vec<usize> {
        let mut v: Vec<usize> = (0..self.cms.len()).filter(|i| set[*i]).collect();
        v.sort_by_key(|i| std::cmp::Reverse((self.cms[*i].born, *i)));
        v
    }
    fn decorations(&self, i: usize, head: Option<(usize, &str)>) -> String {
        let mut v: Vec<String> = Vec::new();
        if let Some((_, b)) = head.filter(|h| h.0 == i) {
            v.push(format!("HEAD -> {b}"));
        }
        let mut names: Vec<&str> = self.locals.iter().filter(|l| l.1 == i && head.is_none_or(|h| h.1 != l.0)).map(|l| l.0.as_str()).collect();
        names.sort();
        v.extend(names.into_iter().map(str::to_string));
        let mut rem: Vec<&str> = self.remotes.iter().filter(|l| l.1 == i).map(|l| l.0.as_str()).collect();
        rem.sort();
        v.extend(rem.into_iter().map(str::to_string));
        v.join(", ")
    }
    fn line(&self, branch: &str) -> Option<&(String, &'static Line, i64)> {
        self.lines.iter().find(|l| l.0 == branch)
    }
}

/// "3 hours ago", the way git words an age.
fn rel(secs: i64) -> String {
    let (n, unit) = match secs.max(0) {
        s @ 0..=59 => (s, "second"),
        s @ 60..=3599 => (s / 60, "minute"),
        s @ 3600..=86_399 => (s / 3600, "hour"),
        s if s < 14 * 86_400 => (s / 86_400, "day"),
        s => (s / (7 * 86_400), "week"),
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

// ------------------------------------------------------------------ files and diffs

fn main_text(rp: &Rp, path: &str) -> Option<String> {
    rp.files.iter().find(|f| f.0 == path).map(|f| f.1.to_string())
}

fn apply(base: &str, edits: &[(&str, &str)]) -> String {
    edits.iter().fold(base.to_string(), |s, (old, new)| s.replacen(old, new, 1))
}

/// A file after a change.
fn after(rp: &Rp, l: &Line, c: &Ch) -> String {
    match c.new {
        Some(t) => t.to_string(),
        None if l.in_main => main_text(rp, c.path).unwrap_or_default(),
        None => apply(&main_text(rp, c.path).unwrap_or_default(), c.edits),
    }
}

/// A file before a change (`None` for a new one).
fn before(rp: &Rp, l: &Line, c: &Ch) -> Option<String> {
    if c.new.is_some() {
        return None;
    }
    let m = main_text(rp, c.path).unwrap_or_default();
    Some(if l.in_main { c.edits.iter().rev().fold(m, |s, (old, new)| s.replacen(new, old, 1)) } else { m })
}

fn ops(a: &[&str], b: &[&str]) -> Vec<(char, usize, usize)> {
    let (n, m) = (a.len(), b.len());
    let at = |i: usize, j: usize| i * (m + 1) + j;
    let mut dp = vec![0u32; (n + 1) * (m + 1)];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[at(i, j)] = if a[i] == b[j] { dp[at(i + 1, j + 1)] + 1 } else { dp[at(i + 1, j)].max(dp[at(i, j + 1)]) };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    while i < n || j < m {
        if i < n && j < m && a[i] == b[j] {
            out.push((' ', i, j));
            i += 1;
            j += 1;
        } else if i < n && (j == m || dp[at(i + 1, j)] >= dp[at(i, j + 1)]) {
            out.push(('-', i, j));
            i += 1;
        } else {
            out.push(('+', i, j));
            j += 1;
        }
    }
    out
}

fn range(start: usize, len: usize) -> String {
    match len {
        1 => start.to_string(),
        _ => format!("{start},{len}"),
    }
}

/// `git diff` for one file with `ctx` lines around each change: the text, lines added, lines removed.
fn udiff(path: &str, old: Option<&str>, new: &str, ctx: usize) -> (String, usize, usize) {
    let a: Vec<&str> = old.map(|o| o.lines().collect()).unwrap_or_default();
    let b: Vec<&str> = new.lines().collect();
    let all = ops(&a, &b);
    let mut out = format!("diff --git a/{path} b/{path}\n");
    if old.is_none() {
        out += "new file mode 100644\n";
    }
    let h = |s: &str| sha(s)[..7].to_string();
    out += &format!("index {}..{}{}\n", old.map(h).unwrap_or_else(|| "0000000".into()), h(new), if old.is_some() { " 100644" } else { "" });
    out += &format!("--- {}\n+++ b/{path}\n", if old.is_some() { format!("a/{path}") } else { "/dev/null".into() });
    let changes: Vec<usize> = all.iter().enumerate().filter(|(_, o)| o.0 != ' ').map(|(i, _)| i).collect();
    let (added, removed) = (all.iter().filter(|o| o.0 == '+').count(), all.iter().filter(|o| o.0 == '-').count());
    let mut k = 0;
    while k < changes.len() {
        let mut end = k;
        while end + 1 < changes.len() && changes[end + 1] - changes[end] - 1 <= 2 * ctx {
            end += 1;
        }
        let from = changes[k].saturating_sub(ctx);
        let to = (changes[end] + ctx).min(all.len() - 1);
        let body = &all[from..=to];
        let (ol, nl) = (body.iter().filter(|o| o.0 != '+').count(), body.iter().filter(|o| o.0 != '-').count());
        let (o0, n0) = (body[0].1, body[0].2);
        let (os, ns) = (if ol == 0 { o0 } else { o0 + 1 }, if nl == 0 { n0 } else { n0 + 1 });
        // what git names after the @@: the nearest earlier line of the old file that starts a name
        let funcname = a[..o0.min(a.len())].iter().rev().find(|l| l.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_' || c == '$')).map(|l| format!(" {}", l.chars().take(80).collect::<String>())).unwrap_or_default();
        out += &format!("@@ -{} +{} @@{funcname}\n", range(os, ol), range(ns, nl));
        for (c, i, j) in body {
            out += &format!("{c}{}\n", if *c == '+' { b[*j] } else { a[*i] });
        }
        k = end + 1;
    }
    (out, added, removed)
}

/// What a line of work changes, sorted by path: (path, before, after).
fn changes(rp: &Rp, l: &Line) -> Vec<(&'static str, Option<String>, String)> {
    let mut v: Vec<_> = l.changes.iter().map(|c| (c.path, before(rp, l, c), after(rp, l, c))).collect();
    v.sort_by_key(|x| x.0);
    v
}

fn branch_diff(rp: &Rp, l: &Line, only: Option<&str>, ctx: usize) -> String {
    changes(rp, l).iter().filter(|c| only.is_none_or(|p| p == c.0)).map(|c| udiff(c.0, c.1.as_deref(), &c.2, ctx).0).collect()
}

/// Edits sitting in a checkout, not committed: (path, before, after).
fn dirty(cx: &Cx, w: &World) -> Vec<(String, String, String)> {
    let st = state();
    let done = |p: &str| st.committed.iter().any(|c| c.0 == cx.top && c.1 == p);
    match &cx.wt {
        None => cx.rp.dirty.iter().filter(|c| !done(c.path)).map(|c| (c.path.to_string(), main_text(cx.rp, c.path).unwrap_or_default(), apply(&main_text(cx.rp, c.path).unwrap_or_default(), c.edits))).collect(),
        Some(slug) => match w.line(&format!("sphere/{slug}")) {
            Some((_, l, _)) => changes(cx.rp, l).into_iter().filter(|c| l.dirty.contains(&c.0) && !done(c.0)).map(|c| (c.0.to_string(), c.1.unwrap_or_default(), c.2)).collect(),
            None => Vec::new(),
        },
    }
}

fn is_staged(cx: &Cx, path: &str) -> bool {
    state().staged.iter().any(|s| s.0 == cx.top && s.1 == path)
}

/// The paths in the checkout.
fn paths(cx: &Cx, w: &World) -> Vec<String> {
    let mut v: Vec<String> = cx.rp.files.iter().map(|f| f.0.to_string()).collect();
    let mut news = |l: &Line| v.extend(l.changes.iter().filter(|c| c.new.is_some()).map(|c| c.path.to_string()));
    match &cx.wt {
        Some(slug) => {
            if let Some((_, l, _)) = w.line(&format!("sphere/{slug}")) {
                news(l);
            }
        }
        None => {
            for b in &w.merged {
                if let Some((_, l, _)) = w.line(b) {
                    news(l);
                }
            }
        }
    }
    v.sort();
    v.dedup();
    v
}

fn text_in(cx: &Cx, w: &World, path: &str) -> Option<String> {
    let line_of = |branch: &str| w.line(branch).map(|l| l.1);
    match &cx.wt {
        Some(slug) => {
            let l = line_of(&format!("sphere/{slug}"))?;
            match l.changes.iter().find(|c| c.path == path) {
                Some(c) => Some(after(cx.rp, l, c)),
                None => main_text(cx.rp, path),
            }
        }
        None => {
            for b in &w.merged {
                if let Some(c) = line_of(b).and_then(|l| l.changes.iter().find(|c| c.path == path).map(|c| (l, c))) {
                    return Some(after(cx.rp, c.0, c.1));
                }
            }
            match cx.rp.dirty.iter().find(|c| c.path == path) {
                Some(c) => Some(apply(&main_text(cx.rp, path).unwrap_or_default(), c.edits)),
                None => main_text(cx.rp, path),
            }
        }
    }
}

// ------------------------------------------------------------------ what the apps ask of the file system

/// The repos the Git app lists: the ones agents work in first, then the rest.
pub fn repos(agent_repos: &[String], max: usize) -> Vec<PathBuf> {
    let known: Vec<PathBuf> = super::REPOS.iter().map(PathBuf::from).collect();
    let mut out: Vec<PathBuf> = Vec::new();
    for p in agent_repos.iter().map(PathBuf::from).filter(|p| known.contains(p)).chain(known.clone()) {
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out.truncate(max);
    out
}

/// The language the Git app names for a repo.
pub fn lang(dir: &Path) -> &'static str {
    if ctx(dir).is_some() {
        "RUST"
    } else {
        ""
    }
}

/// Whether `p` is a folder on the pretend machine: a repo, a worktree, a folder inside one, or one of their parents.
pub fn is_dir(p: &Path) -> bool {
    let s = p.to_string_lossy().trim_end_matches('/').to_string();
    let wts = format!("{HOME}/.sphere/worktrees");
    if [HOME.to_string(), format!("{HOME}/Projects"), format!("{HOME}/.sphere"), wts.clone()].contains(&s) || fixtures::REPOS.iter().any(|r| s == format!("{wts}/{}", r.name)) {
        return true;
    }
    match ctx(p) {
        Some(cx) if cx.prefix.is_empty() => true,
        Some(cx) => paths(&cx, &world(cx.rp)).iter().any(|f| f.starts_with(&format!("{}/", cx.prefix))),
        None => false,
    }
}

/// Whether `p` is the top of a checkout.
pub fn is_repo_top(p: &Path) -> bool {
    ctx(p).is_some_and(|c| c.prefix.is_empty())
}

/// The files of the checkout `root` (the tracked ones).
pub fn files(root: &Path) -> Vec<String> {
    ctx(root).map(|cx| paths(&cx, &world(cx.rp))).unwrap_or_default()
}

/// A file's text, if there is such a file.
pub fn file(p: &Path) -> Option<String> {
    let cx = ctx(p)?;
    if cx.prefix.is_empty() {
        return None;
    }
    text_in(&cx, &world(cx.rp), &cx.prefix)
}

// ------------------------------------------------------------------ git

const NOT_REPO: &str = "not a git repository (or any of the parent directories): .git";

fn fmt_commit(w: &World, i: usize, f: &str, head: Option<(usize, &str)>) -> String {
    let c = &w.cms[i];
    let (name, email) = w.rp.people[c.who as usize];
    let mut out = String::new();
    let mut it = f.chars().peekable();
    while let Some(ch) = it.next() {
        if ch != '%' {
            out.push(ch);
            continue;
        }
        match it.next() {
            Some('h') => out += w.short(i),
            Some('H') => out += &c.id,
            Some('p') => out += &c.parents.iter().map(|p| w.short(*p)).collect::<Vec<_>>().join(" "),
            Some('s') | Some('B') | Some('b') => out += &c.subject,
            Some('D') => out += &w.decorations(i, head),
            Some('n') => out.push('\n'),
            Some('%') => out.push('%'),
            Some('a') => match it.next() {
                Some('n') => out += name,
                Some('e') => out += email,
                Some('r') => out += &rel(now() - c.born),
                _ => {}
            },
            Some('c') => match it.next() {
                Some('t') => out += &c.born.to_string(),
                Some('r') => out += &rel(now() - c.born),
                _ => {}
            },
            Some('x') => {
                let hex: String = it.by_ref().take(2).collect();
                if let Some(ch) = u8::from_str_radix(&hex, 16).ok().map(|b| b as char) {
                    out.push(ch);
                }
            }
            Some(o) => {
                out.push('%');
                out.push(o);
            }
            None => out.push('%'),
        }
    }
    out
}

/// `git log --graph` drawing for the commits in `order`: lanes, then each commit's own text.
fn graph(w: &World, order: &[usize], f: &str, head: Option<(usize, &str)>) -> String {
    let mut lanes: Vec<Option<usize>> = Vec::new();
    let mut out = String::new();
    for &c in order {
        let mine: Vec<usize> = lanes.iter().enumerate().filter(|(_, l)| **l == Some(c)).map(|(i, _)| i).collect();
        let lane = match mine.first() {
            Some(i) => *i,
            None => match lanes.iter().position(|l| l.is_none()) {
                Some(i) => i,
                None => {
                    lanes.push(None);
                    lanes.len() - 1
                }
            },
        };
        let row: String = (0..lanes.len()).map(|j| if j == lane { "* " } else if lanes[j].is_some() { "| " } else { "  " }).collect();
        out += &format!("{row}{}\n", fmt_commit(w, c, f, head));
        for j in mine.iter().skip(1) {
            lanes[*j] = None;
        }
        let parents = &w.cms[c].parents;
        lanes[lane] = parents.first().copied();
        for p in parents.iter().skip(1) {
            if !lanes.contains(&Some(*p)) {
                match lanes.iter().position(|l| l.is_none()) {
                    Some(i) => lanes[i] = Some(*p),
                    None => lanes.push(Some(*p)),
                }
            }
        }
        if parents.len() > 1 {
            out += &format!("{}|\\\n", "| ".repeat(lane));
        } else if mine.len() > 1 {
            out += &format!("{}|/\n", "| ".repeat(lane));
        }
        while lanes.last() == Some(&None) {
            lanes.pop();
        }
    }
    out
}

fn head_of(w: &World, cx: &Cx) -> (usize, String) {
    let branch = match &cx.wt {
        Some(s) => format!("sphere/{s}"),
        None => w.cur.clone(),
    };
    (w.tip(&branch).unwrap_or(0), branch)
}

/// What `git -C dir args` prints.
pub fn git(dir: &Path, a: &[&str]) -> Result<String, String> {
    let cx = ctx(dir).ok_or_else(|| NOT_REPO.to_string())?;
    let w = world(cx.rp);
    let (head, branch) = head_of(&w, &cx);
    let has = |s: &str| a.contains(&s);
    let after_dashes = || a.iter().skip_while(|s| **s != "--").skip(1).map(|s| s.to_string()).collect::<Vec<_>>();
    let fail = || Err("git failed".to_string());
    let ok = |s: &str| Ok(s.to_string());
    let top_dir = || cx.top.clone();
    match a.first().copied().unwrap_or("") {
        "rev-parse" => {
            if has("--git-dir") {
                return Ok(match &cx.wt {
                    Some(s) => format!("{HOME}/Projects/{}/.git/worktrees/{s}\n", cx.rp.name),
                    None => ".git\n".into(),
                });
            }
            if has("--show-toplevel") {
                let prefix = if has("--show-prefix") { format!("{}\n", if cx.prefix.is_empty() { String::new() } else { format!("{}/", cx.prefix) }) } else { String::new() };
                return Ok(format!("{}\n{prefix}", cx.top));
            }
            if has("--is-inside-work-tree") {
                return ok("true\n");
            }
            if has("--abbrev-ref") {
                return match w.upstream(&branch) {
                    Some((n, _)) => Ok(format!("{n}\n")),
                    None => Err(format!("no upstream configured for branch '{branch}'")),
                };
            }
            let name = a.iter().skip(1).find(|s| !s.starts_with('-')).copied().unwrap_or("HEAD");
            match w.resolve(name, head) {
                Some(i) if has("--short") => Ok(format!("{}\n", w.short(i))),
                Some(i) => Ok(format!("{}\n", w.cms[i].id)),
                None if has("-q") => fail(),
                None => Err("Needed a single revision".into()),
            }
        }
        "symbolic-ref" => ok(&format!("{branch}\n")),
        "status" => {
            let z = has("-z");
            let st = state().clone();
            let mut rows: Vec<(String, char, char)> = dirty(&cx, &w).into_iter().map(|d| {
                let staged = st.staged.iter().any(|s| s.0 == cx.top && s.1 == d.0);
                if staged { (d.0, 'M', ' ') } else { (d.0, ' ', 'M') }
            }).collect();
            rows.sort();
            Ok(rows.iter().map(|(p, x, y)| format!("{x}{y} {p}{}", if z { '\0' } else { '\n' })).collect())
        }
        "stash" | "tag" | "gc" | "prune" | "clean" | "cherry-pick" | "revert" | "rebase" | "mv" | "fetch" | "pull" | "init" | "apply" => ok(""),
        "for-each-ref" => {
            if !a.iter().any(|s| s.contains("refname:short")) {
                return ok("");
            }
            let mut v: Vec<&(String, usize)> = w.locals.iter().collect();
            v.sort_by_key(|l| std::cmp::Reverse((w.cms[l.1].born, l.1)));
            Ok(v.iter().map(|(n, tip)| {
                let track = match w.upstream(n) {
                    Some((_, up)) => {
                        let (x, y) = (w.anc(*tip), w.anc(up));
                        let (ah, be) = ((0..x.len()).filter(|i| x[*i] && !y[*i]).count(), (0..x.len()).filter(|i| y[*i] && !x[*i]).count());
                        match (ah, be) {
                            (0, 0) => String::new(),
                            (a, 0) => format!("ahead {a}"),
                            (0, b) => format!("behind {b}"),
                            (a, b) => format!("ahead {a}, behind {b}"),
                        }
                    }
                    None => String::new(),
                };
                format!("{n}\t{}\t{track}\n", rel(now() - w.cms[*tip].born))
            }).collect())
        }
        "log" => {
            let n = a.iter().position(|s| *s == "-n").and_then(|i| a.get(i + 1)).and_then(|s| s.parse::<usize>().ok()).or_else(|| a.iter().find_map(|s| s.strip_prefix('-').and_then(|d| d.parse::<usize>().ok()))).unwrap_or(usize::MAX);
            let f = a.iter().find_map(|s| s.strip_prefix("--format=").or_else(|| s.strip_prefix("--pretty=format:"))).unwrap_or("%h %s");
            let mut set = if has("--all") {
                let mut s = w.anc(head);
                for t in w.locals.iter().map(|l| l.1).chain(w.remotes.iter().map(|r| r.1)) {
                    for (k, v) in w.anc(t).into_iter().enumerate() {
                        s[k] |= v;
                    }
                }
                s
            } else {
                w.anc(head)
            };
            if let Some(r) = a.iter().find(|s| s.contains("..") && !s.starts_with('-')) {
                let (x, y) = r.split_once("..").unwrap_or(("", ""));
                let from = w.resolve(x, head).ok_or_else(|| format!("bad revision '{r}'"))?;
                let to = w.resolve(if y.is_empty() { "HEAD" } else { y }, head).ok_or_else(|| format!("bad revision '{r}'"))?;
                let (ax, ay) = (w.anc(from), w.anc(to));
                set = (0..ax.len()).map(|i| ay[i] && !ax[i]).collect();
            }
            let order: Vec<usize> = w.order(&set).into_iter().take(n).collect();
            let hd = Some((head, branch.as_str()));
            if has("--graph") {
                return Ok(graph(&w, &order, f, hd));
            }
            Ok(order.iter().map(|i| format!("{}\n", fmt_commit(&w, *i, f, hd))).collect())
        }
        "rev-list" => {
            if has("--left-right") {
                let Some((_, up)) = w.upstream(&branch) else { return Err(format!("no upstream configured for branch '{branch}'")) };
                let (x, y) = (w.anc(head), w.anc(up));
                let (ah, be) = ((0..x.len()).filter(|i| x[*i] && !y[*i]).count(), (0..x.len()).filter(|i| y[*i] && !x[*i]).count());
                return Ok(format!("{ah}\t{be}\n"));
            }
            let spec = a.last().copied().unwrap_or("HEAD");
            let set = match spec.split_once("..") {
                Some((x, y)) => {
                    let from = w.resolve(x, head).ok_or("Invalid symmetric difference expression")?;
                    let to = w.resolve(if y.is_empty() { "HEAD" } else { y }, head).ok_or("Invalid symmetric difference expression")?;
                    let (ax, ay) = (w.anc(from), w.anc(to));
                    (0..ax.len()).map(|i| ay[i] && !ax[i]).collect()
                }
                None => w.anc(w.resolve(spec, head).ok_or("bad revision")?),
            };
            Ok(format!("{}\n", set.iter().filter(|b| **b).count()))
        }
        "remote" => match a.get(1) {
            Some(&"get-url") => ok(&format!("{}\n", cx.rp.remote)),
            _ => ok("origin\n"),
        },
        "branch" => {
            if let Some(i) = a.iter().position(|s| *s == "--merged") {
                let into = a.get(i + 1).and_then(|b| w.resolve(b, head)).ok_or("malformed object name")?;
                let set = w.anc(into);
                return Ok(w.locals.iter().filter(|l| set[l.1]).map(|l| format!("{} {}\n", if l.0 == branch { "*" } else { " " }, l.0)).collect());
            }
            Ok(w.locals.iter().map(|l| format!("{} {}\n", if l.0 == branch { "*" } else { " " }, l.0)).collect())
        }
        "merge-base" => {
            if has("--is-ancestor") {
                let ids: Vec<&&str> = a.iter().skip(1).filter(|s| !s.starts_with('-')).collect();
                let (x, y) = (w.resolve(ids.first().map(|s| **s).unwrap_or(""), head), w.resolve(ids.get(1).map(|s| **s).unwrap_or(""), head));
                return match (x, y) {
                    (Some(x), Some(y)) if w.anc(y)[x] => ok(""),
                    _ => fail(),
                };
            }
            let ids: Vec<&&str> = a.iter().skip(1).filter(|s| !s.starts_with('-')).collect();
            let (n0, n1) = (ids.first().map(|s| **s).unwrap_or("HEAD"), ids.get(1).map(|s| **s).unwrap_or("HEAD"));
            let (x, y) = (w.resolve(n0, head).ok_or(format!("Not a valid object name {n0}"))?, w.resolve(n1, head).ok_or(format!("Not a valid object name {n1}"))?);
            // an agent's branch forked from where its fixture says, whatever main has done since
            if let Some((_, l, _)) = cx.wt.as_ref().and_then(|s| w.line(&format!("sphere/{s}"))) {
                if n0 == "HEAD" {
                    let k = w.cms.iter().position(|c| c.key == l.fork).unwrap_or(0);
                    return Ok(format!("{}\n", w.cms[k].id));
                }
            }
            let (ax, ay) = (w.anc(x), w.anc(y));
            let best = (0..ax.len()).filter(|i| ax[*i] && ay[*i]).max_by_key(|i| (w.cms[*i].born, *i));
            match best {
                Some(i) => Ok(format!("{}\n", w.cms[i].id)),
                None => fail(),
            }
        }
        "diff" => {
            if has("--name-only") {
                return ok("");
            }
            let cached = has("--cached");
            let unstaged: Vec<_> = dirty(&cx, &w).into_iter().filter(|d| is_staged(&cx, &d.0) == cached).collect();
            if has("--numstat") {
                return Ok(unstaged.iter().map(|d| {
                    let (_, ad, rm) = udiff(&d.0, Some(&d.1), &d.2, 3);
                    format!("{ad}\t{rm}\t{}\0", d.0)
                }).collect());
            }
            let ctx_n = if has("-U1") { 1 } else { 3 };
            if let Some(path) = after_dashes().first() {
                return Ok(unstaged.iter().filter(|d| &d.0 == path).map(|d| udiff(&d.0, Some(&d.1), &d.2, ctx_n).0).collect());
            }
            match &cx.wt {
                Some(s) => match w.line(&format!("sphere/{s}")) {
                    Some((_, l, _)) => Ok(branch_diff(cx.rp, l, None, ctx_n)),
                    None => ok(""),
                },
                None => Ok(dirty(&cx, &w).iter().map(|d| udiff(&d.0, Some(&d.1), &d.2, ctx_n).0).collect()),
            }
        }
        "ls-files" => {
            if has("--others") && !has("--cached") {
                return ok("");
            }
            Ok(paths(&cx, &w).iter().map(|p| format!("{p}\0")).collect())
        }
        "show" => {
            let spec = a.last().copied().unwrap_or("");
            match spec.split_once(':') {
                Some((_, p)) => text_in(&cx, &w, p).ok_or_else(|| format!("path '{p}' does not exist in 'HEAD'")),
                None => Ok(format!("commit {}\n    {}\n", w.cms[head].id, w.cms[head].subject)),
            }
        }
        "worktree" => match a.get(1).copied() {
            Some("list") => {
                let st = state().clone();
                let mut out = format!("worktree {HOME}/Projects/{}\nHEAD {}\nbranch refs/heads/{}\n\n", cx.rp.name, w.cms[w.tip(&w.cur).unwrap_or(0)].id, w.cur);
                for (b, l, _) in &w.lines {
                    let slug = b.strip_prefix("sphere/").unwrap_or(b);
                    let path = format!("{HOME}/.sphere/worktrees/{}/{slug}", cx.rp.name);
                    if l.worktree && !st.removed.contains(&path) {
                        out += &format!("worktree {path}\nHEAD {}\nbranch refs/heads/{b}\n\n", w.cms[w.tip(b).unwrap_or(0)].id);
                    }
                }
                Ok(out)
            }
            Some("remove") => {
                let p = a.iter().skip(2).find(|s| !s.starts_with('-')).copied().unwrap_or("");
                state().removed.push(p.trim_end_matches('/').to_string());
                ok("")
            }
            _ => ok(""),
        },
        "add" => {
            let all = has("-A") || has("--all") || has(".");
            let want = after_dashes();
            let cand = dirty_for_add(&cx, &w);
            let mut st = state();
            for d in cand {
                if (all || want.contains(&d)) && !st.staged.iter().any(|s| s.0 == cx.top && s.1 == d) {
                    st.staged.push((top_dir(), d));
                }
            }
            ok("")
        }
        "restore" | "reset" | "rm" => {
            let want = after_dashes();
            let mut st = state();
            st.staged.retain(|s| s.0 != cx.top || (!want.is_empty() && !want.contains(&s.1)));
            ok("")
        }
        "commit" => {
            let amend = has("--amend");
            let msg = a.iter().position(|s| *s == "-m").and_then(|i| a.get(i + 1)).map(|s| s.to_string()).unwrap_or_default();
            let subject = msg.lines().next().unwrap_or("").to_string();
            let staged: Vec<String> = state().staged.iter().filter(|s| s.0 == cx.top).map(|s| s.1.clone()).collect();
            if staged.is_empty() && !amend && !has("--allow-empty") {
                return Err("nothing to commit, working tree clean".into());
            }
            let mut st = state();
            st.staged.retain(|s| s.0 != cx.top);
            st.committed.extend(staged.into_iter().map(|p| (top_dir(), p)));
            let subject = if subject.is_empty() { w.cms[head].subject.clone() } else { subject };
            st.events.push(Ev::Commit { repo: cx.rp.name.into(), branch, subject, at: now(), merge: None });
            ok("")
        }
        "push" => {
            state().events.push(Ev::Push { repo: cx.rp.name.into(), branch });
            ok("")
        }
        "switch" | "checkout" => {
            let mut st = state();
            let repo = cx.rp.name.to_string();
            if let Some(i) = a.iter().position(|s| *s == "-c" || *s == "-b") {
                if let Some(n) = a.get(i + 1) {
                    st.events.push(Ev::Branch { repo: repo.clone(), name: n.to_string() });
                    st.events.push(Ev::Switch { repo, name: n.to_string() });
                }
            } else if let Some(n) = a.iter().skip(1).rev().find(|s| !s.starts_with('-')) {
                if w.tip(n).is_some() {
                    st.events.push(Ev::Switch { repo, name: n.to_string() });
                }
            }
            ok("")
        }
        "merge" => {
            if has("--abort") {
                return ok("");
            }
            let other = a.last().copied().unwrap_or("");
            let Some(tip) = w.tip(other) else { return Err(format!("{other} - not something we can merge")) };
            if w.anc(head)[tip] {
                return ok("Already up to date.\n");
            }
            let subject = format!("Merge branch '{other}'");
            state().events.push(Ev::Commit { repo: cx.rp.name.into(), branch, subject, at: now(), merge: Some(other.to_string()) });
            ok("")
        }
        "config" => fail(),
        other => Err(format!("git {other}: not available in the playground")),
    }
}

/// Paths `git add` can stage in a checkout: the edited files that are not staged.
fn dirty_for_add(cx: &Cx, w: &World) -> Vec<String> {
    dirty(cx, w).into_iter().map(|d| d.0).collect()
}

// ------------------------------------------------------------------ gh

fn out(ok: bool, stdout: String, stderr: &str) -> Result<crate::prs::Out, crate::prs::GhError> {
    Ok(crate::prs::Out { ok, stdout, stderr: stderr.to_string() })
}

fn state_word(c: &str) -> &'static str {
    match c {
        "SUCCESS" => "pass",
        "FAILURE" => "fail",
        _ => "pending",
    }
}

fn pr_json(w: &World, branch: &str, l: &Line) -> String {
    let p = l.pr.as_ref().unwrap();
    let (mut ad, mut rm) = (0, 0);
    for c in changes(w.rp, l) {
        let (_, a, r) = udiff(c.0, c.1.as_deref(), &c.2, 3);
        ad += a;
        rm += r;
    }
    let roll: Vec<String> = p.checks.iter().map(|(n, c)| if *c == "IN_PROGRESS" || *c == "PENDING" { format!("{{\"__typename\":\"CheckRun\",\"name\":\"{n}\",\"status\":\"{c}\",\"conclusion\":\"\"}}") } else { format!("{{\"__typename\":\"CheckRun\",\"name\":\"{n}\",\"status\":\"COMPLETED\",\"conclusion\":\"{c}\"}}") }).collect();
    let updated = iso(now() - (p.updated * 3600.0) as i64, 0);
    format!(
        "{{\"number\":{},\"title\":\"{}\",\"headRefName\":\"{branch}\",\"baseRefName\":\"main\",\"author\":{{\"login\":\"{}\",\"is_bot\":false}},\"isDraft\":{},\"reviewDecision\":\"{}\",\"statusCheckRollup\":[{}],\"updatedAt\":\"{}Z\",\"url\":\"https://github.com/{}/pull/{}\",\"additions\":{ad},\"deletions\":{rm},\"state\":\"OPEN\"}}",
        p.number,
        p.title,
        p.author,
        p.draft,
        p.review,
        roll.join(","),
        &updated[..19],
        w.rp.slug,
        p.number
    )
}

/// What `gh args` prints in `dir`.
pub fn gh(dir: &Path, a: &[&str]) -> Result<crate::prs::Out, crate::prs::GhError> {
    let Some(cx) = ctx(dir) else { return out(false, String::new(), "fatal: not a git repository (or any of the parent directories): .git") };
    let w = world(cx.rp);
    let merged = state().merged_prs.clone();
    let open: Vec<(&String, &'static Line)> = w.lines.iter().filter(|l| l.1.pr.is_some() && !merged.iter().any(|m| m.0 == cx.rp.name && Some(m.1) == l.1.pr.as_ref().map(|p| p.number))).map(|l| (&l.0, l.1)).collect();
    let number = a.get(2).and_then(|n| n.parse::<u64>().ok());
    let pr = number.and_then(|n| open.iter().find(|l| l.1.pr.as_ref().is_some_and(|p| p.number == n)));
    match (a.first().copied(), a.get(1).copied()) {
        (Some("pr"), Some("list")) => out(true, format!("[{}]\n", open.iter().map(|(b, l)| pr_json(&w, b, l)).collect::<Vec<_>>().join(",")), ""),
        (Some("pr"), Some(cmd)) => {
            let Some((b, l)) = pr else { return out(false, String::new(), &format!("no pull requests found for {}", a.get(2).copied().unwrap_or("this branch"))) };
            match cmd {
                "diff" => out(true, branch_diff(cx.rp, l, None, 3), ""),
                "checks" => {
                    let p = l.pr.as_ref().unwrap();
                    let lines: String = p.checks.iter().enumerate().map(|(i, (n, c))| format!("{n}\t{}\t{}\thttps://github.com/{}/actions/runs/{}/job/{}\n", state_word(c), if *c == "SUCCESS" || *c == "FAILURE" { format!("{}m{:02}s", 1 + i, 12 + i * 17) } else { "0".into() }, w.rp.slug, 1_182_000 + p.number, 3_400_000 + i)).collect();
                    out(p.checks.iter().all(|c| c.1 == "SUCCESS"), lines, "")
                }
                "view" if a.contains(&"--json") => out(true, pr_json(&w, b, l) + "\n", ""),
                "view" => out(true, format!("{}\n", l.pr.as_ref().map(|p| p.title).unwrap_or("")), ""),
                "merge" => {
                    let p = l.pr.as_ref().unwrap();
                    let mut st = state();
                    st.merged_prs.push((cx.rp.name.to_string(), p.number));
                    st.events.push(Ev::Commit { repo: cx.rp.name.into(), branch: "main".into(), subject: format!("{} (#{})", p.title, p.number), at: now(), merge: None });
                    out(true, String::new(), "")
                }
                _ => out(true, String::new(), ""),
            }
        }
        (Some("auth"), _) => out(true, "github.com\n  Logged in to github.com account you (keyring)\n".into(), ""),
        _ => out(false, String::new(), "unknown command for the playground"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gitdata;
    use crate::review::{parse_diff, Status};

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }
    const SHOP: &str = "/Users/you/Projects/shop-api";
    const SPHERE: &str = "/Users/you/Projects/sphere";
    const WT: &str = "/Users/you/.sphere/worktrees";

    fn g(dir: &str, a: &[&str]) -> String {
        git(Path::new(dir), a).unwrap_or_else(|e| panic!("git {a:?} in {dir}: {e}"))
    }

    /// What `review::load` makes of a worktree, from the same git calls.
    fn review(dir: &str, base: &str) -> (Vec<crate::review::FileDiff>, Vec<String>, bool, bool) {
        let fork = g(dir, &["merge-base", "HEAD", base]).trim().to_string();
        let commits = g(dir, &["log", "--format=%h%x09%s", &format!("{fork}..HEAD")]).lines().map(str::to_string).collect();
        let files = parse_diff(&g(dir, &["diff", "--no-color", "--no-ext-diff", "-M", "-U3", &fork]));
        let head = g(dir, &["rev-parse", "HEAD"]).trim().to_string();
        let merged = head != fork && git(Path::new(dir), &["merge-base", "--is-ancestor", "HEAD", base]).is_ok();
        let dirty = !g(dir, &["status", "--porcelain"]).trim().is_empty();
        (files, commits, merged, dirty)
    }

    fn worktrees_review_like_the_leads_numbers() {
        let count = |f: &[crate::review::FileDiff]| f.iter().map(|x| (x.path.clone(), x.added, x.removed)).collect::<Vec<_>>();
        let (f, c, merged, dirty) = review(&format!("{WT}/shop-api/fix-flaky-login-test"), "main");
        println!("fix-flaky {:?}", count(&f));
        assert_eq!(f.iter().map(|x| x.path.as_str()).collect::<Vec<_>>(), ["src/auth/token.rs", "tests/login.rs", "tests/support/mod.rs"]);
        assert_eq!((c.len(), merged, dirty), (1, false, true));
        assert_eq!((f[0].added, f[0].removed), (3, 3));
        assert_eq!(f[2].status, Status::Added);
        assert!(f.iter().all(|x| !x.hunks.is_empty() && x.hunks.iter().all(|h| !h.lines.is_empty())));
        let (f, c, merged, dirty) = review(&format!("{WT}/shop-api/reply-to-the-api-review"), "main");
        println!("reply {:?}", count(&f));
        assert_eq!(f.iter().map(|x| x.path.as_str()).collect::<Vec<_>>(), ["docs/api.md", "src/routes/orders.rs", "tests/orders_api.rs"]);
        assert_eq!((c.len(), merged, dirty), (2, false, false));
        let (f, c, merged, _) = review(&format!("{WT}/shop-api/bump-postgres-to-16-4"), "main");
        println!("bump {:?}", count(&f));
        assert_eq!(count(&f), vec![(".github/workflows/ci.yml".to_string(), 1, 1), ("docker-compose.yml".to_string(), 1, 1)]);
        assert_eq!((c.len(), merged), (1, true));
        // the same with the base a commit, as sphered records it
        let fork = g(SHOP, &["rev-parse", "main~1"]).trim().to_string();
        assert!(review(&format!("{WT}/shop-api/bump-postgres-to-16-4"), &fork).2);
        let (f, c, merged, _) = review(&format!("{WT}/sphere/draft-release-notes"), "main");
        println!("notes {:?}", count(&f));
        assert_eq!((f.len(), f[0].path.as_str(), f[0].status, c.len(), merged), (1, "RELEASE_NOTES.md", Status::Added, 1, false));
        // an agent the visitor starts
        let (f, c, merged, dirty) = review(&format!("{WT}/shop-api/make-the-api-faster"), "main");
        println!("generic shop {:?}", count(&f));
        assert_eq!(f.iter().map(|x| x.path.as_str()).collect::<Vec<_>>(), ["src/routes/health.rs", "tests/health.rs"]);
        assert_eq!((c.len(), merged, dirty), (2, false, false));
        let (f, ..) = review(&format!("{WT}/sphere/tidy-the-daemon"), "main");
        println!("generic sphere {:?}", count(&f));
        assert_eq!(f.iter().map(|x| x.path.as_str()).collect::<Vec<_>>(), ["crates/sphered/src/manager.rs", "crates/sphered/tests/tasks.rs"]);
        // a worktree that was never made is not a repo until it is asked for... but any slug is accepted, a missing repo is not
        assert!(git(Path::new(&format!("{WT}/nope/x")), &["status"]).is_err());
    }

    fn diffs_are_what_git_prints() {
        let (d, a, r) = udiff("f.txt", Some("a\nb\nc\nd\ne\nf\ng\nh\ni\nj\n"), "a\nB\nc\nd\ne\nf\ng\nh\nI\nj\nk\n", 3);
        assert_eq!((a, r), (3, 2));
        assert!(d.contains("@@ -1,10 +1,11 @@\n"), "{d}");
        let (d, ..) = udiff("n.txt", None, "x\ny\n", 3);
        assert!(d.contains("new file mode 100644\n") && d.contains("--- /dev/null\n+++ b/n.txt\n@@ -0,0 +1,2 @@\n+x\n+y\n"), "{d}");
        let (d, ..) = udiff("f.rs", Some("fn a() {\n    1\n}\n"), "fn a() {\n    2\n}\n", 1);
        assert!(d.contains("@@ -1,3 +1,3 @@\n fn a() {\n-    1\n+    2\n }\n"), "{d}");
        // far-apart changes make two hunks, and the second names the function above it
        let old: String = (0..40).map(|i| if i == 10 { "fn mid() {\n".to_string() } else { format!("    line {i}\n") }).collect();
        let new = old.replace("line 2\n", "line two\n").replace("line 30\n", "line thirty\n");
        let (d, ..) = udiff("g.rs", Some(&old), &new, 3);
        assert_eq!(d.matches("\n@@ ").count(), 2, "{d}");
        assert!(d.contains("@@ fn mid() {\n") || d.contains("fn mid() {\n"), "{d}");
    }

    fn the_git_app_reads_the_repos() {
        assert_eq!(repos(&[SHOP.to_string()], 8), vec![p(SHOP), p(SPHERE)]);
        assert_eq!(repos(&[SPHERE.to_string(), "/elsewhere".into()], 1), vec![p(SPHERE)]);
        for (repo, files) in [(SHOP, 21), (SPHERE, 16)] {
            assert!(is_dir(Path::new(repo)) && is_repo_top(Path::new(repo)) && lang(Path::new(repo)) == "RUST");
            assert_eq!(g(repo, &["rev-parse", "--git-dir"]), ".git\n");
            assert_eq!(g(repo, &["symbolic-ref", "--short", "-q", "HEAD"]).trim(), "main");
            assert_eq!(g(repo, &["ls-files", "-z", "--cached", "--others", "--exclude-standard"]).split('\0').filter(|s| !s.is_empty()).count(), files);
            assert_eq!(g(repo, &["rev-parse", "--short", "HEAD"]).trim().len(), 7);
            assert_eq!(g(repo, &["remote"]).trim(), "origin");
            assert_eq!(g(repo, &["stash", "list"]), "");
            // one commit ahead of origin/main in shop-api, level in sphere
            let t = g(repo, &["rev-list", "--left-right", "--count", "HEAD...@{upstream}"]);
            assert_eq!(t.trim(), if repo == SHOP { "1\t0" } else { "0\t0" });
            let last = g(repo, &["log", "-1", "--format=%h%x09%s%x09%an%x09%ar"]);
            assert_eq!(last.trim().split('\t').count(), 4);
            assert!(!is_dir(&p(&format!("{repo}/nothing"))));
        }
        // branches, the way gitdata::load reads them
        let refs = g(SHOP, &["for-each-ref", "--sort=-committerdate", "--format=%(refname:short)%09%(committerdate:relative)%09%(upstream:track,nobracket)", "refs/heads"]);
        let names: Vec<&str> = refs.lines().map(|l| l.split('\t').next().unwrap()).collect();
        assert!(names.contains(&"main"), "{refs}");
        for n in ["sphere/fix-flaky-login-test", "sphere/reply-to-the-api-review", "sphere/bump-postgres-to-16-4", "feature/refunds"] {
            assert!(names.contains(&n), "{n} in {names:?}");
        }
        assert!(refs.lines().find(|l| l.starts_with("main\t")).unwrap().ends_with("ahead 1"), "{refs}");
        assert!(!names.contains(&"amara/order-export-csv"));
        // the graph and the lane layout
        let dag = g(SHOP, &["log", "--all", "--date-order", "-n", "26", "--format=%h%x02%p%x02%D%x02%s"]);
        let commits: Vec<_> = dag
            .lines()
            .map(|l| {
                let f: Vec<&str> = l.split('\u{2}').collect();
                (f[0].to_string(), f[1].split_whitespace().map(str::to_string).collect::<Vec<_>>(), f[2].split(", ").filter(|s| !s.is_empty() && *s != "HEAD").map(|s| s.trim_start_matches("HEAD -> ").to_string()).collect::<Vec<_>>(), f[3].to_string())
            })
            .collect();
        assert!((24..=26).contains(&commits.len()), "{}", commits.len());
        assert!(commits.iter().any(|c| c.1.len() == 2 && c.3.starts_with("Merge branch")));
        assert!(commits.iter().any(|c| c.2.contains(&"main".to_string())));
        let nodes = gitdata::lay_out(commits);
        assert!(nodes.iter().map(|n| n.lane).max().unwrap() >= 2, "branches should sit in lanes");
        let gr = gitdata::parse_graph(&g(SHOP, &["log", "--graph", "--all", "--date-order", "-n", "200", "--format=%x01%h%x02%D%x02%s%x02%ar"]));
        assert!(gr.iter().filter(|r| !r.hash.is_empty()).count() >= 24 && gr.iter().any(|r| r.hash.is_empty()));
        assert!(gr.iter().any(|r| r.refs.contains(&"origin/main".to_string())));
        assert!(gr.iter().all(|r| r.hash.is_empty() || r.age.ends_with("ago")));
    }

    fn staging_committing_switching_and_merging_are_remembered() {
        let dirty = |d: &str| gitdata::parse_status(&g(d, &["status", "--porcelain=v1", "-z"]));
        let (st, un) = dirty(SPHERE);
        assert!(st.is_empty());
        assert_eq!(un.iter().map(|c| (c.path.as_str(), c.code)).collect::<Vec<_>>(), vec![("crates/sphered/src/workspace.rs", 'M')]);
        assert_eq!(gitdata::changed_count(&g(SPHERE, &["status", "--porcelain=v1", "-z"])), 1);
        let ns = g(SPHERE, &["diff", "--numstat", "-z"]);
        assert_eq!(ns.split('\0').next().unwrap(), "6\t1\tcrates/sphered/src/workspace.rs");
        let pv = g(SPHERE, &["diff", "--no-color", "--no-ext-diff", "-U1", "--", "crates/sphered/src/workspace.rs"]);
        assert!(pv.lines().any(|l| l.starts_with("@@")));
        g(SPHERE, &["add", "--", "crates/sphered/src/workspace.rs"]);
        let (st, un) = dirty(SPHERE);
        assert_eq!((st.len(), un.len()), (1, 0));
        assert!(g(SPHERE, &["diff", "--cached", "--numstat", "-z"]).starts_with("6\t1\t"));
        g(SPHERE, &["restore", "--staged", "--", "crates/sphered/src/workspace.rs"]);
        assert_eq!(dirty(SPHERE).0.len(), 0);
        g(SPHERE, &["add", "-A"]);
        let before = g(SPHERE, &["rev-parse", "HEAD"]);
        g(SPHERE, &["commit", "-q", "-m", "Trim slugs without a trailing dash"]);
        let after = g(SPHERE, &["rev-parse", "HEAD"]);
        assert_ne!(before, after);
        assert_eq!(g(SPHERE, &["log", "-1", "--format=%s"]).trim(), "Trim slugs without a trailing dash");
        assert_eq!(dirty(SPHERE), (vec![], vec![]));
        assert!(git(Path::new(SPHERE), &["commit", "-q", "-m", "again"]).unwrap_err().contains("nothing to commit"));
        assert_eq!(g(SPHERE, &["rev-list", "--left-right", "--count", "HEAD...@{upstream}"]).trim(), "1\t0");
        g(SPHERE, &["push", "-q"]);
        assert_eq!(g(SPHERE, &["rev-list", "--left-right", "--count", "HEAD...@{upstream}"]).trim(), "0\t0");
        assert_eq!(g(SPHERE, &["show", "HEAD:crates/sphered/src/workspace.rs"]).matches("trim_end_matches('-')").count(), 1);
        // a new branch, and back
        g(SPHERE, &["switch", "-c", "try-it"]);
        assert_eq!(g(SPHERE, &["symbolic-ref", "--short", "-q", "HEAD"]).trim(), "try-it");
        assert!(g(SPHERE, &["rev-parse", "--verify", "-q", "refs/heads/try-it"]).len() > 40);
        g(SPHERE, &["switch", "main"]);
        assert!(git(Path::new(SPHERE), &["rev-parse", "--verify", "-q", "refs/heads/nope"]).is_err());
        // merging an agent's branch: not an ancestor before, one after, and the worktree reads as merged
        let wt = format!("{WT}/sphere/draft-release-notes");
        assert!(git(Path::new(SPHERE), &["merge-base", "--is-ancestor", "sphere/draft-release-notes", "main"]).is_err());
        g(SPHERE, &["merge", "--no-ff", "--no-edit", "sphere/draft-release-notes"]);
        assert!(git(Path::new(SPHERE), &["merge-base", "--is-ancestor", "sphere/draft-release-notes", "main"]).is_ok());
        assert!(review(&wt, "main").2);
        assert!(g(SPHERE, &["show", "HEAD:RELEASE_NOTES.md"]).contains("0.9.0"));
        assert_eq!(g(SPHERE, &["log", "-1", "--format=%s"]).trim(), "Merge branch 'sphere/draft-release-notes'");
        // discarding a worktree
        assert!(is_dir(Path::new(&wt)));
        g(SPHERE, &["worktree", "remove", "--force", &wt]);
        assert!(!is_dir(Path::new(&wt)) && git(Path::new(&wt), &["status"]).is_err());
        assert!(!g(SPHERE, &["worktree", "list", "--porcelain"]).contains("draft-release-notes"));
    }

    fn files_read_as_the_branch_has_them() {
        let wt = Path::new("/Users/you/.sphere/worktrees/shop-api/reply-to-the-api-review");
        let orders = file(&wt.join("src/routes/orders.rs")).unwrap();
        assert!(orders.contains("list_orders") && orders.contains("struct Page"));
        assert!(file(Path::new("/Users/you/Projects/shop-api/src/routes/orders.rs")).unwrap().contains("async fn list("));
        assert!(file(&wt.join("tests/orders_api.rs")).is_some() && file(Path::new("/Users/you/Projects/shop-api/tests/orders_api.rs")).is_none());
        assert!(file(Path::new("/Users/you/Projects/sphere/crates/sphered/src/workspace.rs")).is_some_and(|t| t.contains("path.exists()")));
        assert!(file(Path::new("/Users/you/Projects/shop-api/Cargo.toml")).unwrap().contains("axum"));
        assert_eq!(file(Path::new("/Users/you/Projects/shop-api")), None);
        assert!(is_dir(Path::new("/Users/you/Projects/shop-api/src/routes")) && !is_dir(Path::new("/Users/you/Projects/shop-api/src/nope")));
        assert!(is_dir(Path::new("/Users/you/Projects")) && is_dir(Path::new("/Users/you/.sphere/worktrees/shop-api")));
        let top = g("/Users/you/Projects/shop-api/src/routes", &["rev-parse", "--show-toplevel", "--show-prefix"]);
        assert_eq!(top, "/Users/you/Projects/shop-api\nsrc/routes/\n");
        assert!(!is_repo_top(Path::new("/Users/you/Projects/shop-api/src")));
    }

    fn pull_requests_list_and_open() {
        let run = |d: &Path, a: &[&str]| gh(d, a);
        let shop = crate::prs::list(Path::new(SHOP), &run);
        let crate::prs::PrList::Prs(v) = shop else { panic!("{shop:?}") };
        assert_eq!(v.len(), 3);
        let by = |n: u64| v.iter().find(|p| p.number == n).unwrap();
        assert_eq!((by(31).head.as_str(), by(31).draft, by(31).checks.label().0.as_str()), ("sphere/reply-to-the-api-review", false, "PASSING"));
        assert!(by(29).draft && by(27).checks.fail == 1 && by(27).author == "amara-okafor");
        assert_eq!(by(27).review, crate::prs::Review::Changes);
        assert!(by(31).additions > 20 && by(31).deletions > 3 && by(31).updated.ends_with('Z'));
        assert!(crate::prs::iso_secs(&by(31).updated).is_some());
        let crate::prs::PrList::Prs(s) = crate::prs::list(Path::new(SPHERE), &run) else { panic!() };
        assert_eq!(s.len(), 2);
        assert_eq!(crate::prs::summarize(&v).0, 3);
        let d = crate::prs::detail(Path::new(SHOP), 27, &run);
        assert_eq!(d.error, None);
        assert_eq!(d.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), ["Cargo.toml", "src/routes/orders.rs"]);
        assert_eq!(d.checks.iter().map(|c| (c.0.as_str(), c.1.as_str())).collect::<Vec<_>>(), vec![("fmt", "pass"), ("clippy", "pass"), ("test", "fail")]);
        // from a worktree the same repo answers
        assert!(matches!(crate::prs::list(Path::new("/Users/you/.sphere/worktrees/shop-api/fix-flaky-login-test"), &run), crate::prs::PrList::Prs(_)));
    }

    fn pull_requests_act() {
        let run = |d: &Path, a: &[&str]| gh(d, a);
        assert_eq!(crate::prs::merge(Path::new(SHOP), 31, &run), Ok("Merged #31 (squash)".to_string()));
        let crate::prs::PrList::Prs(after) = crate::prs::list(Path::new(SHOP), &run) else { panic!() };
        assert_eq!(after.len(), 2);
        assert!(g(SHOP, &["log", "-1", "--format=%s"]).contains("(#31)"));
        assert_eq!(crate::prs::checkout(Path::new(SHOP), 29, &run), Ok("Checked out #29".to_string()));
        assert!(crate::prs::open_web(Path::new(SHOP), 29, &run).is_ok());
        assert!(crate::prs::merge(Path::new(SHOP), 999, &run).is_err());
    }

    /// One test, in this order: the state the apps change is shared, so reads come before the changes.
    #[test]
    fn the_pretend_repos_work() {
        diffs_are_what_git_prints();
        the_git_app_reads_the_repos();
        files_read_as_the_branch_has_them();
        pull_requests_list_and_open();
        worktrees_review_like_the_leads_numbers();
        staging_committing_switching_and_merging_are_remembered();
        pull_requests_act();
    }
}
