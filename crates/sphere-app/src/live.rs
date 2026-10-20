//! Turns the service's shared slot into what the scene shows: the Agents panel rows, the open requests behind the
//! Agent moment card, the link state and toasts. Each system does its work only when the part of the slot it reads
//! has changed, and marks a resource changed only when its content did.
use crate::core::{AState, Agent, Agents, Roster};
use crate::service::{is_live, Cmd, Link, Service};
use bevy::prelude::*;
use sphere_proto::{RequestView, TaskState, TaskView};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Resource, Default)]
pub struct Live {
    pub link: Option<Link>,
    /// Open requests, oldest first (the card shows them in this order).
    pub requests: Vec<RequestView>,
    /// Task titles by id, for the card header.
    pub titles: Vec<(String, String)>,
    /// Helper tasks and the task that started each: (helper id, parent id).
    pub parents: Vec<(String, String)>,
    /// Seconds agents waited on you today over requests already closed (sphered's count, from the link's mirror).
    pub waited_closed_secs: u64,
    pub version: u64,
    seen: u64,
}

impl Live {
    pub fn title<'a>(&'a self, task: &'a str) -> &'a str {
        self.titles.iter().find(|(id, _)| id == task).map(|(_, t)| t.as_str()).unwrap_or(task)
    }

    /// The title of the task that started `task` as its helper, when it is one.
    pub fn parent_title(&self, task: &str) -> Option<&str> {
        let (_, p) = self.parents.iter().find(|(id, _)| id == task)?;
        Some(self.title(p))
    }

    /// Seconds agents have waited on you today, as of `now` (unix seconds): the closed requests' total plus what each
    /// request still open or snoozed has waited so far.
    pub fn waited_today_secs(&self, now: i64) -> u64 {
        waited_total(self.waited_closed_secs, &self.requests, now)
    }
}

/// `closed` plus how long each request still open or snoozed has waited as of `now` (unix seconds, from its `opened_at`).
pub fn waited_total(closed: u64, requests: &[RequestView], now: i64) -> u64 {
    let open = requests.iter().filter(|r| r.status.is_open()).filter_map(|r| crate::working::parse_rfc3339(&r.opened_at));
    open.fold(closed, |sum, at| sum.saturating_add((now - at).max(0) as u64))
}

/// A wait in whole minutes, as the panels say it: "1 min", "12 min", "1 h 5 min" (an exact hour is "2 h").
pub fn fmt_waited(secs: u64) -> String {
    let mins = secs / 60;
    match (mins / 60, mins % 60) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

/// A toast's left bar: the theme's fill, green (something went through) or red (it failed).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ToastKind {
    #[default]
    Plain,
    Ok,
    Bad,
}

/// Short-lived lines shown above the command bar: (text, seconds left, kind).
#[derive(Resource, Default)]
pub struct Toasts {
    pub list: Vec<(String, f32, ToastKind)>,
    pub version: u64,
    pending_sound: bool,
}

impl Toasts {
    /// A toast with its sound (skipped when another sound just played).
    pub fn push(&mut self, s: impl Into<String>) {
        self.pending_sound = true;
        self.push_quiet(s);
    }
    /// A toast whose event already made its own sound.
    pub fn push_quiet(&mut self, s: impl Into<String>) {
        self.push_kind(s, ToastKind::Plain);
    }
    /// A toast with a green bar, with its sound (something went through).
    pub fn push_ok(&mut self, s: impl Into<String>) {
        self.pending_sound = true;
        self.push_kind(s, ToastKind::Ok);
    }
    /// A toast with a red bar, with its sound (something failed).
    pub fn push_bad(&mut self, s: impl Into<String>) {
        self.pending_sound = true;
        self.push_kind(s, ToastKind::Bad);
    }
    /// A toast of a kind whose event already made its own sound.
    pub fn push_kind(&mut self, s: impl Into<String>, kind: ToastKind) {
        self.list.push((s.into(), 2.4 + 0.42, kind));
        if self.list.len() > 3 {
            self.list.remove(0);
        }
        self.version += 1;
    }
}

/// A finished task that changed no files: a question answered, nothing to merge.
pub fn answered(t: &TaskView) -> bool { t.state == TaskState::Done && t.files.is_empty() }

fn row(t: &TaskView, merged: bool, parent: Option<&str>) -> Agent {
    let (state, chip) = match t.state {
        TaskState::Queued | TaskState::Working => (AState::Working, None),
        TaskState::NeedsYou => (AState::Needs, None),
        TaskState::Done => (AState::Done, answered(t).then_some("ANSWERED")),
        TaskState::Failed => (AState::Failed, Some("FAILED")),
        TaskState::Stopped => (AState::Failed, Some("STOPPED")),
    };
    let step = if answered(t) {
        t.last_say.lines().next().unwrap_or("").to_string()
    } else if merged && t.state == TaskState::Done {
        format!("Merged · {}", t.branch.as_deref().unwrap_or(""))
    } else if t.status.is_empty() {
        t.last_say.lines().next().unwrap_or("").to_string()
    } else {
        t.status.clone()
    };
    // Real agents have no percent: the bar fills with the step count and the figure shows it.
    let p = 1.0 - (-(t.steps as f32) / 14.0).exp();
    let figure = match state {
        AState::Working if t.state == TaskState::Queued => Some("QUEUED".to_string()),
        AState::Working => Some(format!("{}", t.steps)),
        _ => None,
    };
    let mut name = t.title.clone();
    if name.chars().count() > 30 {
        name = name.chars().take(29).collect::<String>() + "…";
    }
    // a helper says whose it is
    let mut step = match parent {
        Some(p) => format!("Helper of {p} · {step}"),
        None => step,
    };
    if step.chars().count() > 44 {
        step = step.chars().take(43).collect::<String>() + "…";
    }
    Agent { name, state, p: p.max(0.04), step, rate: 0.0, figure, chip }
}

/// How a task ranks on the panel: what needs you first, then working, then queued, then the finished. Helpers go
/// behind every other agent (unless they need you), so they never push one out of the four rows.
fn rank(t: &TaskView) -> u8 {
    let r = match t.state {
        TaskState::NeedsYou => return 0,
        TaskState::Working => 1,
        TaskState::Queued => 2,
        _ => 3,
    };
    if t.parent_task.is_some() { r + 4 } else { r }
}

/// A task that failed or was stopped before its agent ever ran (no session, no step, no cost): a start
/// that didn't happen, not an agent.
pub(crate) fn never_ran(t: &TaskView) -> bool {
    matches!(t.state, TaskState::Failed | TaskState::Stopped) && t.session_id.is_none() && t.steps == 0 && t.cost_usd == 0.0
}

/// How long a finished, failed or stopped agent stays in the panel.
const RECENT_SECS: i64 = 24 * 3600;

/// The panel's view of the agents: the first rows (what needs you first, then working, then queued, then the most
/// recent finished) and the counts. Finished ones go after a day; starts that never ran are left out but the latest,
/// so its error still shows. `now` is unix seconds. The panel and the ring show four rows at most, so only those are
/// built: one pass over the tasks, no sort, and a row costs a few strings however many tasks there are.
fn pick(tasks: &[TaskView], merged: &HashSet<String>, now: i64) -> Roster {
    let recent = |t: &TaskView| crate::working::parse_rfc3339(&t.updated_at).is_none_or(|at| now - at < RECENT_SECS);
    let latest_dud = tasks.iter().filter(|t| never_ran(t)).max_by(|a, b| a.updated_at.cmp(&b.updated_at)).map(|t| t.id.as_str());
    let keep = |t: &TaskView| match t.state {
        TaskState::NeedsYou | TaskState::Working | TaskState::Queued => true,
        _ if never_ran(t) => latest_dud == Some(t.id.as_str()) && recent(t),
        _ => recent(t),
    };
    // a task goes ahead of an earlier one only if it ranks better or, at the same rank, changed more recently
    let ahead = |t: &TaskView, o: &TaskView| rank(t) < rank(o) || (rank(t) == rank(o) && t.updated_at > o.updated_at);
    let (mut top, mut n) = ([0usize; 4], 0);
    let (mut total, mut working, mut waiting) = (0, 0, 0);
    for (i, t) in tasks.iter().enumerate().filter(|(_, t)| keep(t)) {
        total += 1;
        match t.state {
            TaskState::Queued | TaskState::Working => working += 1,
            TaskState::NeedsYou => waiting += 1,
            _ => {}
        }
        let mut at = n;
        while at > 0 && ahead(t, &tasks[top[at - 1]]) {
            at -= 1;
        }
        if at < 4 {
            n = (n + 1).min(4);
            for k in (at + 1..n).rev() {
                top[k] = top[k - 1];
            }
            top[at] = i;
        }
    }
    let parent_of = |t: &TaskView| t.parent_task.as_deref().and_then(|p| tasks.iter().find(|o| o.id == p)).map(|o| o.title.as_str());
    let rows = top[..n].iter().map(|&i| row(&tasks[i], merged.contains(&tasks[i].id), parent_of(&tasks[i]))).collect();
    Roster { rows, total, working, waiting }
}

/// What the finished-task check found out about a task, and what it knew then: when the task last changed and where
/// the repo's main branch stood.
#[derive(Clone, Debug, PartialEq)]
struct Looked {
    updated_at: String,
    tip: Option<String>,
    merged: bool,
}

/// Whether a finished task needs `review::is_merged` (three to six git processes) run on it again. It does when it was
/// never looked at or has changed since (an agent that goes on works in its worktree). A task known merged stays so
/// until it changes. One that is not merged can become so only when the repo's main branch moves; if the branch can't be
/// read (`tip` is `None`) it is looked at every time.
fn needs_look(prev: Option<&Looked>, updated_at: &str, tip: Option<&str>) -> bool {
    match prev {
        None => true,
        Some(p) => p.updated_at != updated_at || (!p.merged && (tip.is_none() || p.tip.as_deref() != tip)),
    }
}

/// What the finished-task passes remember between them.
#[derive(Default)]
pub struct Memo {
    looked: HashMap<String, Looked>,
    /// the git directory a worktree shares with its repo (it never changes), by worktree
    common: HashMap<String, Option<PathBuf>>,
}

/// A finished task to look at.
struct Cand {
    id: String,
    workdir: String,
    base: String,
    updated_at: String,
}

/// The git directory a linked worktree shares with its repo's main checkout: the worktree's `.git` file names its own
/// directory, and that one names the shared one. `None` for anything else (a plain checkout has a `.git` directory).
fn common_dir(workdir: &Path) -> Option<PathBuf> {
    let file = std::fs::read_to_string(workdir.join(".git")).ok()?;
    let own = workdir.join(file.lines().find_map(|l| l.strip_prefix("gitdir:"))?.trim());
    let rel = std::fs::read_to_string(own.join("commondir")).ok()?;
    // written out (`..` resolved), so every worktree of a repo gives the same path and the repo is read once
    let mut dir = PathBuf::new();
    for part in own.join(rel.trim()).components() {
        match part {
            std::path::Component::ParentDir => {
                dir.pop();
            }
            std::path::Component::CurDir => {}
            other => dir.push(other),
        }
    }
    Some(dir)
}

/// Where the repo's main checkout stands, as `branch@commit`: the branch its `HEAD` names and that branch's commit
/// (its ref file, else its line in `packed-refs`). Read from the git directory, so no process starts. `None` when
/// anything is not the plain layout.
fn read_tip(common: &Path) -> Option<String> {
    let head = std::fs::read_to_string(common.join("HEAD")).ok()?;
    let head = head.trim();
    let Some(name) = head.strip_prefix("ref:").map(str::trim) else {
        // a detached HEAD is a commit already
        return (head.len() >= 40 && head.bytes().all(|b| b.is_ascii_hexdigit())).then(|| format!("@{head}"));
    };
    let loose = std::fs::read_to_string(common.join(name)).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let sha = loose.or_else(|| std::fs::read_to_string(common.join("packed-refs")).ok()?.lines().find_map(|l| l.split_once(' ').filter(|(_, n)| *n == name).map(|(sha, _)| sha.to_string())))?;
    Some(format!("{name}@{sha}"))
}

/// The main branch's tip for the repo of the worktree `workdir`, asking the disk once per repo in a pass.
fn tip_of(workdir: &str, common: &mut HashMap<String, Option<PathBuf>>, tips: &mut HashMap<PathBuf, Option<String>>) -> Option<String> {
    let dir = common.entry(workdir.to_string()).or_insert_with(|| common_dir(Path::new(workdir))).clone()?;
    tips.entry(dir.clone()).or_insert_with(|| read_tip(&dir)).clone()
}

/// Finds the finished tasks whose branch is already merged (git, in the background, every few seconds) and puts them
/// in `Shared::merged`, so they read "Merged" rather than "Finished". A task is looked at again only when it or the
/// repo's main branch changed (see [`needs_look`]), so a long history costs nothing.
pub fn check_merged(time: Res<Time<Real>>, svc: Res<Service>, mut t: Local<f32>, mut busy: Local<Option<Arc<std::sync::atomic::AtomicBool>>>, mut memo: Local<Option<Arc<Mutex<Memo>>>>) {
    use std::sync::atomic::{AtomicBool, Ordering};
    *t += time.delta_secs();
    // the first check runs at once (the swarm waits for it before it shows finished agents), then every 4 s
    let first = !svc.shared.lock().unwrap().merged_checked;
    if (*t < 4.0 && !first) || busy.as_ref().is_some_and(|b| b.load(Ordering::Relaxed)) {
        return;
    }
    *t = 0.0;
    let memo = memo.get_or_insert_with(Default::default).clone();
    let todo: Vec<Cand> = {
        let s = svc.shared.lock().unwrap();
        let m = memo.lock().unwrap();
        let known = |t: &TaskView| m.looked.get(&t.id).is_some_and(|l| l.merged && l.updated_at == t.updated_at);
        s.tasks.iter().filter(|t| t.state == TaskState::Done && !t.in_place && t.branch.is_some() && !known(t)).filter_map(|t| Some(Cand { id: t.id.clone(), workdir: t.workdir.clone()?, base: t.base.clone()?, updated_at: t.updated_at.clone() })).collect()
    };
    if todo.is_empty() {
        // nothing finished that needs a look (a task list that hasn't arrived waits for the next pass)
        let mut s = svc.shared.lock().unwrap();
        if !s.tasks.is_empty() && !s.merged_checked {
            s.merged_checked = true;
            s.version += 1;
        }
        return;
    }
    let flag = busy.get_or_insert_with(|| Arc::new(AtomicBool::new(false))).clone();
    flag.store(true, Ordering::Relaxed);
    let shared = svc.shared.clone();
    std::thread::spawn(move || {
        // the maps are this pass's alone (the next one waits for the flag), so git and the disk run off the lock
        let (mut looked, mut common) = {
            let mut m = memo.lock().unwrap();
            (std::mem::take(&mut m.looked), std::mem::take(&mut m.common))
        };
        let mut tips = HashMap::new();
        let mut found: Vec<(String, bool)> = Vec::new();
        for c in todo {
            let tip = tip_of(&c.workdir, &mut common, &mut tips);
            if !needs_look(looked.get(&c.id), &c.updated_at, tip.as_deref()) {
                continue;
            }
            let merged = crate::review::is_merged(&c.workdir, &c.base);
            found.push((c.id.clone(), merged));
            looked.insert(c.id, Looked { updated_at: c.updated_at, tip, merged });
        }
        {
            let mut m = memo.lock().unwrap();
            m.looked = looked;
            m.common = common;
        }
        let mut s = shared.lock().unwrap();
        let mut changed = !s.merged_checked;
        for (id, merged) in found {
            changed |= if merged { s.merged.insert(id) } else { s.merged.remove(&id) };
        }
        s.merged_checked = true;
        if changed {
            s.version += 1;
        }
        flag.store(false, Ordering::Relaxed);
    });
}

/// Brings the Agents panel, the Timeline, the open requests and the toasts up to date with the shared slot. Runs its
/// pass when the slot's version moved or a minute turned, and does each part only if its input changed: the rows and counts on every
/// pass (one scan, a few rows built), the Timeline arcs when one moved, the titles when a task joined; a resource is
/// marked changed only when its content differs.
pub fn sync(svc: Res<Service>, mut live: ResMut<Live>, mut agents: ResMut<Agents>, mut toasts: ResMut<Toasts>, args: Res<crate::Args>, mut seen: Local<(u64, u64, i64)>, mut hidden: Local<bool>) {
    if args.offline() {
        if args.moment && live.requests.is_empty() {
            demo_request(&mut live);
        }
        return;
    }
    // the link stops pinging while Sphere is hidden: tell it when the window is back
    let was_hidden = std::mem::replace(&mut *hidden, !crate::summon::on_screen());
    if was_hidden && !*hidden {
        svc.send(Cmd::Shown);
    }
    // a finished agent leaves the panel a day after it last changed: look again each minute, not only on news
    let now = crate::core::unix_now();
    let mut s = svc.shared.lock().unwrap();
    if s.version == live.seen && now / 60 == seen.2 {
        return;
    }
    seen.2 = now / 60;
    // the version is bookkeeping: writing it must not wake what follows `Live`
    live.bypass_change_detection().seen = s.version;
    if live.waited_closed_secs != s.waited_today_secs {
        live.waited_closed_secs = s.waited_today_secs;
    }
    for n in s.notes.drain(..) {
        toasts.push(n);
    }
    if live.link != s.link {
        if let Some(Link::Down(e)) = &s.link {
            toasts.push(e.clone());
        }
        live.link = s.link.clone();
    }
    let roster = pick(&s.tasks, &s.merged, now);
    if agents.differs(&roster) {
        agents.set_roster(roster);
    }
    if s.spans != seen.1 {
        seen.1 = s.spans;
        let spans: Vec<(i64, i64)> = s
            .tasks
            .iter()
            .filter_map(|t| {
                let a = crate::working::parse_rfc3339(&t.started_at)?;
                Some((a, if is_live(t) { i64::MAX } else { crate::working::parse_rfc3339(&t.updated_at).unwrap_or(a) }))
            })
            .collect();
        if agents.runs != spans {
            agents.set_runs(spans);
        }
    }
    let mut open: Vec<&RequestView> = s.requests.iter().filter(|r| r.status.is_open()).collect();
    open.sort_by(|a, b| a.opened_at.cmp(&b.opened_at));
    let new_requests = open.len() != live.requests.len() || open.iter().zip(&live.requests).any(|(a, b)| *a != b);
    let titles = (s.roster != seen.0).then(|| s.tasks.iter().map(|t| (t.id.clone(), t.title.clone())).collect::<Vec<_>>());
    let parents: Vec<(String, String)> = s.tasks.iter().filter_map(|t| Some((t.id.clone(), t.parent_task.clone()?))).collect();
    if live.parents != parents {
        live.parents = parents;
        live.version += 1;
    }
    seen.0 = s.roster;
    let new_titles = titles.as_ref().is_some_and(|t| *t != live.titles);
    if new_requests {
        live.requests = open.into_iter().cloned().collect();
    }
    if new_titles {
        live.titles = titles.unwrap_or_default();
    }
    if new_requests || new_titles {
        live.version += 1;
    }
}

/// The prototype's kind of moment, for `--moment` screenshots: an agent asking to run a command (with a rule to remember,
/// and another agent asking the very same) and an edit to approve.
fn demo_request(live: &mut Live) {
    let info = sphere_proto::RequestInfo {
        id: "r1".into(),
        task: "t1".into(),
        kind: sphere_proto::RequestKind::Permission,
        tool: "Bash".into(),
        display_name: None,
        description: Some("Run the login tests against staging".into()),
        summary: "The login test is flaky, not broken. I'd like to run it against staging to confirm the fix.".into(),
        input: serde_json::json!({"command": "npm test -- --grep login --env staging", "description": "Run the login tests against staging"}),
        suggestions: vec![serde_json::json!({"type": "addRules", "rules": [{"toolName": "Bash", "ruleContent": "npm test:*"}], "behavior": "allow", "destination": "localSettings"})],
        blocked_path: None,
        tool_use_id: None,
        agent_request: None,
        revises: None,
    };
    let mut r2 = info.clone();
    r2.id = "r2".into();
    r2.task = "t2".into();
    r2.tool = "Edit".into();
    r2.suggestions = vec![];
    r2.summary = "Squash and merge the login fix into main".into();
    let mut r3 = info.clone();
    r3.id = "r3".into();
    r3.task = "t3".into();
    r3.summary = "minio stopped because its disk is full; clear old caches and restart it".into();
    // opened a few minutes ago, the oldest first
    let rv = |i: sphere_proto::RequestInfo, mins: i64| RequestView {
        info: i,
        status: sphere_proto::RequestStatus::Open,
        opened_at: sphere_proto::rfc3339_from_unix_ms(((crate::core::unix_now() - mins * 60) * 1000) as u64),
    };
    live.requests = vec![rv(info, 4), rv(r2, 3), rv(r3, 2)];
    live.titles = vec![("t1".into(), "Fix flaky login test".into()), ("t2".into(), "Merge the login fix".into()), ("t3".into(), "Restart minio".into())];
    live.version += 1;
}

/// When an agent finishes or fails: a toast, the sound, and (finished) Sphere says so, like the prototype's end.
/// Only a task changing state can do that, so the tasks are looked at only when one did.
pub fn transitions(
    svc: Res<Service>,
    args: Res<crate::Args>,
    mut seen: Local<(u64, Option<HashMap<String, TaskState>>)>,
    mut toasts: ResMut<Toasts>,
    mut sound: ResMut<crate::sound::Sound>,
    mut say: ResMut<crate::homepanels::Say>,
    mut fx: ResMut<crate::fx::Fx>,
) {
    if args.offline() {
        return;
    }
    let s = svc.shared.lock().unwrap();
    let (states, known) = &mut *seen;
    if known.is_some() && s.states == *states {
        return;
    }
    *states = s.states;
    let Some(prev) = known.as_mut() else {
        // what was already finished when the app started isn't news
        *known = Some(s.tasks.iter().map(|t| (t.id.clone(), t.state)).collect());
        return;
    };
    for t in &s.tasks {
        let before = prev.get(t.id.as_str()).copied();
        if before == Some(t.state) {
            continue;
        }
        match prev.get_mut(t.id.as_str()) {
            Some(p) => *p = t.state,
            None => {
                prev.insert(t.id.clone(), t.state);
            }
        }
        match t.state {
            TaskState::Done if before.is_some() => {
                toasts.push_quiet(format!("{} is done", t.title));
                sound.play("done");
                fx.shock(crate::util::Role::Ok);
                let words = format!("Done. {} is finished.", t.title);
                sound.speak(&words);
                say.say(words);
            }
            TaskState::Failed if before.is_some() => {
                toasts.push_quiet(format!("{} failed", t.title));
                sound.play_with("error", 0.6, 0.0);
            }
            _ => {}
        }
    }
}

pub fn tick_toasts(time: Res<Time<Real>>, mut toasts: ResMut<Toasts>, mut sound: ResMut<crate::sound::Sound>) {
    if toasts.pending_sound {
        toasts.pending_sound = false;
        sound.play_q("toast", 150);
    }
    let dt = time.delta_secs();
    let before = toasts.list.len();
    for t in toasts.list.iter_mut() {
        t.1 -= dt;
    }
    toasts.list.retain(|t| t.1 > 0.0);
    if toasts.list.len() != before {
        toasts.version += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toasts_carry_their_kind() {
        let mut t = Toasts::default();
        t.push("a");
        t.push_quiet("b");
        t.push_ok("c");
        t.push_bad("d");
        // the list keeps the last three
        assert_eq!(t.list.iter().map(|x| (x.0.as_str(), x.2)).collect::<Vec<_>>(), vec![("b", ToastKind::Plain), ("c", ToastKind::Ok), ("d", ToastKind::Bad)]);
        t.push_kind("e", ToastKind::Ok);
        assert_eq!(t.list.last().map(|x| x.2), Some(ToastKind::Ok));
    }

    /// A timestamp `secs` before now, so a test's tasks stay inside RECENT_SECS whatever the date.
    fn ago(secs: u64) -> String {
        sphere_proto::rfc3339_from_unix_ms((crate::core::unix_now() as u64 - secs) * 1000)
    }

    fn task(id: &str, state: TaskState, at: &str) -> TaskView {
        serde_json::from_value(serde_json::json!({
            "id": id, "title": format!("Task {id}"), "prompt": "", "state": state, "status": "Done: finished",
            "repo": "/r", "branch": format!("sphere/{id}"), "in_place": false, "budget_usd": 0.0, "cost_usd": 0.0,
            "started_at": at, "updated_at": at,
        }))
        .unwrap()
    }

    #[test]
    fn every_agent_counts_and_the_panel_shows_three_and_more_past_four() {
        let tasks: Vec<TaskView> = (0..6).map(|i| task(&format!("t{i}"), TaskState::Working, &format!("2026-10-19T00:0{i}:00Z"))).collect();
        let mut agents = Agents::empty();
        agents.set_roster(pick(&tasks, &HashSet::new(), NOW));
        assert_eq!(agents.working(), 6, "agents past the fourth still count");
        assert_eq!((agents.list.len(), agents.total), (4, 6), "four rows are kept, six agents are counted");
        assert_eq!(crate::panel::agent_rows(agents.total), (3, 3));
        assert_eq!(crate::panel::agent_rows(4), (4, 0));
        assert_eq!(crate::panel::agent_rows(0), (0, 0));
    }

    /// 2026-10-19T12:00:00Z
    const NOW: i64 = 1_792_411_200;

    #[test]
    fn starts_that_never_ran_are_not_agents_but_the_latest_shows() {
        let mut tasks: Vec<TaskView> = (0..5).map(|i| task(&format!("f{i}"), TaskState::Failed, &format!("2026-10-19T00:0{i}:00Z"))).collect();
        let mut done = task("d1", TaskState::Done, "2026-10-18T13:00:00Z");
        done.steps = 3;
        tasks.push(done);
        let mut ran = task("f9", TaskState::Failed, "2026-10-18T14:00:00Z");
        ran.session_id = Some("s".into());
        tasks.push(ran);
        let names: Vec<String> = pick(&tasks, &HashSet::new(), NOW).rows.into_iter().map(|a| a.name).collect();
        assert_eq!(names, vec!["Task f4", "Task f9", "Task d1"]);
    }

    #[test]
    fn finished_agents_leave_after_a_day_but_working_ones_stay() {
        let mut old = task("d0", TaskState::Done, "2026-10-17T10:00:00Z");
        old.steps = 5;
        let tasks = vec![old, task("w0", TaskState::Working, "2026-10-17T10:00:00Z"), task("x0", TaskState::Failed, "2026-10-17T11:00:00Z")];
        let names: Vec<String> = pick(&tasks, &HashSet::new(), NOW).rows.into_iter().map(|a| a.name).collect();
        assert_eq!(names, vec!["Task w0"]);
    }

    #[test]
    fn a_merged_agent_reads_merged() {
        let mut tasks = vec![task("t1", TaskState::Done, "2026-10-19T00:00:00Z"), task("t2", TaskState::Done, "2026-10-19T00:01:00Z")];
        for t in tasks.iter_mut() {
            t.files = vec![sphere_proto::FileStat { path: "a.rs".into(), added: 1, removed: 0 }];
        }
        let merged: HashSet<String> = ["t1".to_string()].into();
        let rows = pick(&tasks, &merged, NOW).rows;
        let step = |n: &str| rows.iter().find(|a| a.name == n).unwrap().step.clone();
        assert_eq!(step("Task t1"), "Merged · sphere/t1");
        assert_eq!(step("Task t2"), "Done: finished");
        assert_eq!(crate::appframe::finished(&tasks[0], &merged), "Merged");
        assert_eq!(crate::appframe::finished(&tasks[1], &merged), "Finished");
    }

    /// The rows as they were built before only the first were: every agent kept, sorted whole. The oracle for `pick`.
    fn every_row(tasks: &[TaskView], merged: &HashSet<String>, now: i64) -> Vec<Agent> {
        let recent = |t: &TaskView| crate::working::parse_rfc3339(&t.updated_at).is_none_or(|at| now - at < RECENT_SECS);
        let latest_dud = tasks.iter().filter(|t| never_ran(t)).max_by(|a, b| a.updated_at.cmp(&b.updated_at)).map(|t| t.id.clone());
        let keep = |t: &TaskView| match t.state {
            TaskState::NeedsYou | TaskState::Working | TaskState::Queued => true,
            _ if never_ran(t) => latest_dud.as_ref() == Some(&t.id) && recent(t),
            _ => recent(t),
        };
        let rank = |t: &TaskView| match t.state {
            TaskState::NeedsYou => 0,
            TaskState::Working => 1,
            TaskState::Queued => 2,
            _ => 3,
        };
        let mut v: Vec<&TaskView> = tasks.iter().filter(|t| keep(t)).collect();
        v.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| b.updated_at.cmp(&a.updated_at)));
        v.into_iter().map(|t| row(t, merged.contains(&t.id), None)).collect()
    }

    #[test]
    fn the_rows_kept_are_the_first_of_the_full_sort_and_the_counts_cover_everything() {
        let states = [TaskState::Queued, TaskState::Working, TaskState::NeedsYou, TaskState::Done, TaskState::Failed, TaskState::Stopped];
        let mut x: u64 = 99;
        let mut next = move |m: u64| {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (x >> 33) % m
        };
        for round in 0..400 {
            let n = next(14) as usize;
            // few distinct times, so many tasks tie and the order of equals is tested; some are days old
            let tasks: Vec<TaskView> = (0..n)
                .map(|i| {
                    let mut t = task(&format!("t{i}"), states[next(6) as usize], &format!("2026-10-1{}T00:0{}:00Z", 7 + next(3), next(3)));
                    t.steps = next(2) as u32;
                    t
                })
                .collect();
            let merged: HashSet<String> = tasks.iter().filter(|_| next(3) == 0).map(|t| t.id.clone()).collect();
            let all = every_row(&tasks, &merged, NOW);
            let r = pick(&tasks, &merged, NOW);
            assert_eq!(r.rows, all[..all.len().min(4)], "round {round}: {} tasks", n);
            assert_eq!(r.total, all.len());
            assert_eq!(r.working, all.iter().filter(|a| a.state == AState::Working).count());
            assert_eq!(r.waiting, all.iter().filter(|a| a.state == AState::Needs).count());
            assert_eq!(r, Roster::of(&all), "the same roster as the whole list gives");
        }
    }

    fn looked(updated_at: &str, tip: Option<&str>, merged: bool) -> Looked {
        Looked { updated_at: updated_at.into(), tip: tip.map(str::to_string), merged }
    }

    #[test]
    fn a_finished_task_is_looked_at_again_only_when_something_could_have_changed() {
        // never looked at: look
        assert!(needs_look(None, "t1", Some("main@a")));
        assert!(needs_look(None, "t1", None));
        // known merged and unchanged: never again, whatever the repo does
        let merged = looked("t1", Some("main@a"), true);
        assert!(!needs_look(Some(&merged), "t1", Some("main@a")));
        assert!(!needs_look(Some(&merged), "t1", Some("main@b")));
        assert!(!needs_look(Some(&merged), "t1", None));
        // it went on working and finished again: look
        assert!(needs_look(Some(&merged), "t2", Some("main@a")));
        // not merged, nothing new anywhere: no look
        let open = looked("t1", Some("main@a"), false);
        assert!(!needs_look(Some(&open), "t1", Some("main@a")));
        // the main branch moved, the task changed: look
        assert!(needs_look(Some(&open), "t1", Some("main@b")));
        assert!(needs_look(Some(&open), "t2", Some("main@a")));
        // the branch can't be read: look every time, as before
        assert!(needs_look(Some(&open), "t1", None));
        assert!(needs_look(Some(&looked("t1", None, false)), "t1", None));
        assert!(needs_look(Some(&looked("t1", None, false)), "t1", Some("main@a")));
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git").arg("-C").arg(dir).args(["-c", "user.email=t@t", "-c", "user.name=t", "-c", "commit.gpgsign=false"]).args(args).output().unwrap();
        assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[test]
    fn the_main_branchs_tip_is_read_from_the_disk_and_follows_it() {
        let base = std::env::temp_dir().join(format!("sphere-live-tip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (main, wt) = (base.join("main"), base.join("wt"));
        std::fs::create_dir_all(&main).unwrap();
        git(&main, &["init", "-q", "-b", "main"]);
        std::fs::write(main.join("a.txt"), "one\n").unwrap();
        git(&main, &["add", "."]);
        git(&main, &["commit", "-q", "-m", "base"]);
        git(&main, &["worktree", "add", "-q", "-b", "agent", wt.to_str().unwrap()]);
        let wt2 = base.join("wt2");
        git(&main, &["worktree", "add", "-q", "-b", "agent2", wt2.to_str().unwrap()]);
        // the worktree leads to the repo's own git directory (the same one from every worktree, with no `..` in it), and
        // the tip is the branch the main checkout is on
        let common = common_dir(&wt).expect("a linked worktree has a shared git directory");
        assert!(common.join("HEAD").is_file() && !common.to_string_lossy().contains(".."), "{common:?}");
        assert_eq!(common_dir(&wt2), Some(common.clone()));
        let sha = git(&main, &["rev-parse", "main"]);
        assert_eq!(read_tip(&common), Some(format!("refs/heads/main@{sha}")));
        // a commit on main moves it; so does packing the refs, without changing the answer
        std::fs::write(main.join("a.txt"), "two\n").unwrap();
        git(&main, &["commit", "-q", "-am", "next"]);
        let next = git(&main, &["rev-parse", "main"]);
        assert_ne!(sha, next);
        assert_eq!(read_tip(&common), Some(format!("refs/heads/main@{next}")));
        git(&main, &["pack-refs", "--all"]);
        assert_eq!(read_tip(&common), Some(format!("refs/heads/main@{next}")));
        // the cache asks the disk once per repo and per pass
        let (mut commons, mut tips) = (HashMap::new(), HashMap::new());
        assert_eq!(tip_of(wt.to_str().unwrap(), &mut commons, &mut tips), Some(format!("refs/heads/main@{next}")));
        assert_eq!(tip_of(wt2.to_str().unwrap(), &mut commons, &mut tips), Some(format!("refs/heads/main@{next}")));
        assert_eq!((commons.len(), tips.len()), (2, 1), "two worktrees, one repo read");
        // a plain checkout and a folder that isn't a repo have no shared directory: those are looked at every time
        assert_eq!(common_dir(&main), None);
        assert_eq!(common_dir(&base.join("nowhere")), None);
        assert_eq!(tip_of(main.to_str().unwrap(), &mut commons, &mut tips), None);
        // a detached main checkout is its commit
        git(&main, &["checkout", "-q", "--detach"]);
        assert_eq!(read_tip(&common), Some(format!("@{next}")));
        let _ = std::fs::remove_dir_all(&base);
    }

    // ---- the systems, on a bare App

    fn args() -> crate::Args {
        crate::Args { cmd: None, bench: None, shot: None, record: None, record_frames: 180, frames: 120, depth: false, orbit: false, apps: false, work: false, swarm: false, code: false, git: false, open_app: None, icon: false, aa: "smaa".into(), offscreen: false, collapsed: false, focus_term: false, msaa: true, size: (1600, 1000), dt: 0.05, demo: false, live: false, moment: false, elsewhere: false, calendar: false, takeover: None, theme: None, motion: None, goto: None, tasks: false }
    }

    fn app() -> App {
        let mut app = App::new();
        app.insert_resource(crate::service::start(true))
            .init_resource::<Live>()
            .insert_resource(Agents::empty())
            .init_resource::<Toasts>()
            .init_resource::<crate::sound::Sound>()
            .init_resource::<crate::homepanels::Say>()
            .init_resource::<crate::fx::Fx>()
            .insert_resource(args())
            .add_systems(Update, (sync, transitions).chain());
        app
    }

    /// Changes the shared slot the way the link does, then bumps its version.
    fn put(app: &App, f: impl FnOnce(&mut crate::service::Shared)) {
        let svc = app.world().resource::<Service>().clone();
        let mut s = svc.shared.lock().unwrap();
        f(&mut s);
        s.version += 1;
    }

    /// When a resource was last marked changed.
    fn stamp<R: Resource>(app: &App) -> u32 {
        app.world().resource_ref::<R>().last_changed().get()
    }

    fn ask(task: &str) -> RequestView {
        let info = sphere_proto::RequestInfo { id: format!("r-{task}"), task: task.into(), kind: sphere_proto::RequestKind::Permission, tool: "Bash".into(), display_name: None, description: None, summary: "run it".into(), input: serde_json::json!({"command": "ls"}), suggestions: vec![], blocked_path: None, tool_use_id: None, agent_request: None, revises: None };
        RequestView { info, status: sphere_proto::RequestStatus::Open, opened_at: "2026-10-19T00:00:00.000Z".into() }
    }

    #[test]
    fn a_resource_is_marked_changed_only_when_its_content_changed() {
        let mut app = app();
        put(&app, |s| {
            s.tasks = vec![task("t1", TaskState::Working, &ago(120)), task("t2", TaskState::Done, &ago(60))];
            (s.roster, s.states, s.spans) = (1, 1, 1);
            s.link = Some(Link::Connected);
        });
        app.update();
        let agents = app.world().resource::<Agents>();
        assert_eq!((agents.total, agents.list.len(), agents.working()), (2, 2, 1));
        assert_eq!(agents.runs.len(), 2, "the Timeline got both arcs");
        let live = app.world().resource::<Live>();
        assert_eq!((live.titles.len(), live.link.clone()), (2, Some(Link::Connected)));
        let (a0, l0, ring0) = (stamp::<Agents>(&app), stamp::<Live>(&app), agents.ring);

        // a new version with nothing new in it: nothing is marked, nothing rebuilds
        put(&app, |_| {});
        app.update();
        assert_eq!((stamp::<Agents>(&app), stamp::<Live>(&app)), (a0, l0));

        // the words of a step change: the panel's rows are new, the ring is not, `Live` is not
        put(&app, |s| s.tasks[0].status = "Running tests, step 2".into());
        app.update();
        assert_ne!(stamp::<Agents>(&app), a0);
        assert_eq!(stamp::<Live>(&app), l0);
        assert_eq!(app.world().resource::<Agents>().ring, ring0);
        assert_eq!(app.world().resource::<Agents>().list[0].step, "Running tests, step 2");
        let a1 = stamp::<Agents>(&app);

        // a step counted: the working agent's arc moves, so the ring does too
        put(&app, |s| s.tasks[0].steps = 3);
        app.update();
        assert_ne!(stamp::<Agents>(&app), a1);
        assert_eq!(app.world().resource::<Agents>().ring, ring0 + 1);
        let (a2, v0) = (stamp::<Agents>(&app), app.world().resource::<Live>().version);

        // a request arrives: `Live` is marked and its version moves; the agents are not
        put(&app, |s| s.requests = vec![ask("t1")]);
        app.update();
        assert_eq!(stamp::<Agents>(&app), a2);
        assert_eq!(app.world().resource::<Live>().requests.len(), 1);
        assert_eq!(app.world().resource::<Live>().version, v0 + 1);
        let l1 = stamp::<Live>(&app);

        // the link drops: `Live` is marked, with a toast for the person
        put(&app, |s| s.link = Some(Link::Down("sphered is gone".into())));
        app.update();
        assert_ne!(stamp::<Live>(&app), l1);
        assert_eq!(app.world().resource::<Toasts>().list.last().map(|t| t.0.as_str()), Some("sphered is gone"));
    }

    #[test]
    fn titles_and_arcs_are_worked_out_only_when_a_task_joins_or_an_arc_moves() {
        let mut app = app();
        put(&app, |s| {
            s.tasks = vec![task("t1", TaskState::Working, "2026-10-19T00:01:00Z")];
            (s.roster, s.states, s.spans) = (1, 1, 1);
        });
        app.update();
        assert_eq!(app.world().resource::<Live>().titles, vec![("t1".to_string(), "Task t1".to_string())]);
        let runs0 = app.world().resource::<Agents>().runs_ver;
        // an edit the counters do not announce is not picked up: the passes skip the work (the link bumps them with every real change)
        put(&app, |s| {
            s.tasks[0].title = "Renamed".into();
            s.tasks[0].state = TaskState::Done;
        });
        app.update();
        assert_eq!(app.world().resource::<Live>().titles[0].1, "Task t1");
        assert_eq!(app.world().resource::<Agents>().runs_ver, runs0);
        // announced: picked up
        put(&app, |s| (s.roster, s.spans) = (2, 2));
        app.update();
        assert_eq!(app.world().resource::<Live>().titles[0].1, "Renamed");
        assert_eq!(app.world().resource::<Agents>().runs_ver, runs0 + 1, "the arc closed");
    }

    #[test]
    fn a_finished_agent_toasts_once_and_what_was_finished_at_start_is_not_news() {
        let mut app = app();
        put(&app, |s| {
            s.tasks = vec![task("t1", TaskState::Working, "2026-10-19T00:01:00Z"), task("t2", TaskState::Done, "2026-10-19T00:02:00Z")];
            (s.roster, s.states, s.spans) = (1, 1, 1);
        });
        app.update();
        assert!(app.world().resource::<Toasts>().list.is_empty(), "t2 was done before we looked");
        put(&app, |s| {
            s.tasks[0].state = TaskState::Done;
            s.states += 1;
        });
        app.update();
        let toasts = |app: &App| app.world().resource::<Toasts>().list.iter().map(|t| t.0.clone()).collect::<Vec<_>>();
        assert_eq!(toasts(&app), ["Task t1 is done"]);
        for _ in 0..3 {
            put(&app, |_| {});
            app.update();
        }
        assert_eq!(toasts(&app), ["Task t1 is done"], "later passes with no state change say nothing more");
        put(&app, |s| {
            s.tasks.push(task("t3", TaskState::Working, "2026-10-19T00:03:00Z"));
            (s.roster, s.states, s.spans) = (s.roster + 1, s.states + 1, s.spans + 1);
        });
        app.update();
        assert_eq!(toasts(&app).len(), 1, "a new working task is not news either");
        put(&app, |s| {
            s.tasks[2].state = TaskState::Failed;
            s.states += 1;
        });
        app.update();
        assert_eq!(toasts(&app), ["Task t1 is done", "Task t3 failed"]);
    }

    fn waiting_request(id: &str, status: sphere_proto::RequestStatus, opened_at: &str) -> RequestView {
        serde_json::from_value(serde_json::json!({
            "id": id, "task": format!("t-{id}"), "kind": "permission", "tool": "Bash", "summary": "", "input": {},
            "status": status, "opened_at": opened_at,
        }))
        .unwrap()
    }

    #[test]
    fn the_waited_total_adds_what_open_and_snoozed_requests_have_waited_so_far() {
        use sphere_proto::RequestStatus::*;
        let reqs = vec![
            // 5 min and 30 min before NOW
            waiting_request("r1", Open, "2026-10-19T11:55:00Z"),
            waiting_request("r2", Snoozed, "2026-10-19T11:30:00Z"),
            // closed ones are in the closed total already
            waiting_request("r3", Answered, "2026-10-19T08:00:00Z"),
            waiting_request("r4", Withdrawn, "2026-10-19T08:00:00Z"),
            // a clock a little behind the service's, and a time that can't be read, add nothing
            waiting_request("r5", Open, "2026-10-19T12:00:30Z"),
            waiting_request("r6", Open, "not a time"),
        ];
        assert_eq!(waited_total(120, &reqs, NOW), 120 + 300 + 1800);
        assert_eq!(waited_total(0, &[], NOW), 0);
        assert_eq!(waited_total(90, &reqs[2..4], NOW), 90);
        // the same through Live, which keeps the snapshot's figure beside the open requests
        let live = Live { waited_closed_secs: 120, requests: reqs, ..default() };
        assert_eq!(live.waited_today_secs(NOW), 2220);
        // it grows with the clock while requests wait (r5 has waited 30 s by then)
        assert_eq!(live.waited_today_secs(NOW + 60), 2220 + 60 + 60 + 30);
    }

    #[test]
    fn a_wait_reads_in_whole_minutes_then_hours() {
        assert_eq!(fmt_waited(59), "0 min");
        assert_eq!(fmt_waited(60), "1 min");
        assert_eq!(fmt_waited(119), "1 min");
        assert_eq!(fmt_waited(12 * 60 + 30), "12 min");
        assert_eq!(fmt_waited(59 * 60 + 59), "59 min");
        assert_eq!(fmt_waited(3600), "1 h");
        assert_eq!(fmt_waited(65 * 60), "1 h 5 min");
        assert_eq!(fmt_waited(3 * 3600 + 120), "3 h 2 min");
    }

    #[test]
    fn a_done_task_without_files_reads_answered_with_the_answer_as_its_step() {
        let mut t = task("q1", TaskState::Done, "2026-10-19T00:00:00Z");
        t.last_say = "It is slow because of the N+1 query.\nMore detail.".into();
        assert!(answered(&t));
        let a = row(&t, false, None);
        assert_eq!((a.chip, a.step.as_str()), (Some("ANSWERED"), "It is slow because of the N+1 query."));
        t.files = vec![sphere_proto::FileStat { path: "a.rs".into(), added: 1, removed: 0 }];
        assert!(!answered(&t));
        assert_eq!(row(&t, false, None).chip, None);
        assert!(!answered(&task("q2", TaskState::Working, "2026-10-19T00:00:00Z")));
    }
}
