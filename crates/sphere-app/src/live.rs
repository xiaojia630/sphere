//! Turns the service snapshot into what the scene shows: the Agents panel rows, the open requests behind the
//! Agent moment card, the link state and toasts.
use crate::core::{AState, Agent, Agents};
use crate::service::{Link, Service};
use bevy::prelude::*;
use sphere_proto::{RequestView, TaskState, TaskView};
use std::collections::HashSet;

#[derive(Resource, Default)]
pub struct Live {
    pub link: Option<Link>,
    /// Open requests, oldest first (the card shows them in this order).
    pub requests: Vec<RequestView>,
    /// Task titles by id, for the card header.
    pub titles: Vec<(String, String)>,
    pub version: u64,
    seen: u64,
}

impl Live {
    pub fn title<'a>(&'a self, task: &'a str) -> &'a str {
        self.titles.iter().find(|(id, _)| id == task).map(|(_, t)| t.as_str()).unwrap_or(task)
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

fn row(t: &TaskView, merged: bool) -> Agent {
    let (state, chip) = match t.state {
        TaskState::Queued | TaskState::Working => (AState::Working, None),
        TaskState::NeedsYou => (AState::Needs, None),
        TaskState::Done => (AState::Done, None),
        TaskState::Failed => (AState::Failed, Some("FAILED")),
        TaskState::Stopped => (AState::Failed, Some("STOPPED")),
    };
    let step = if merged && t.state == TaskState::Done {
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
    let mut step = step;
    if step.chars().count() > 44 {
        step = step.chars().take(43).collect::<String>() + "…";
    }
    Agent { name, state, p: p.max(0.04), step, rate: 0.0, figure, chip }
}

/// A task that failed or was stopped before its agent ever ran (no session, no step, no cost): a start
/// that didn't happen, not an agent.
fn never_ran(t: &TaskView) -> bool {
    matches!(t.state, TaskState::Failed | TaskState::Stopped) && t.session_id.is_none() && t.steps == 0 && t.cost_usd == 0.0
}

/// How long a finished, failed or stopped agent stays in the panel.
const RECENT_SECS: i64 = 24 * 3600;

/// The agents for the panel: what needs you first, then working, then the most recent finished (the panel
/// shows the first rows and counts the rest). Finished ones go after a day; starts that never ran are left
/// out but the latest, so its error still shows. `now` is unix seconds.
fn pick(tasks: &[TaskView], merged: &HashSet<String>, now: i64) -> Vec<Agent> {
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
    v.into_iter().map(|t| row(t, merged.contains(&t.id))).collect()
}

/// Finds the finished tasks whose branch is already merged (git, in the background, every few seconds) and
/// puts them in `Shared::merged`, so they read "Merged" rather than "Finished".
pub fn check_merged(time: Res<Time<Real>>, svc: Res<Service>, mut t: Local<f32>, mut busy: Local<Option<std::sync::Arc<std::sync::atomic::AtomicBool>>>) {
    use std::sync::atomic::{AtomicBool, Ordering};
    *t += time.delta_secs();
    // the first check runs at once (the swarm waits for it before it shows finished agents), then every 4 s
    let first = !svc.shared.lock().unwrap().merged_checked;
    if (*t < 4.0 && !first) || busy.as_ref().is_some_and(|b| b.load(Ordering::Relaxed)) {
        return;
    }
    *t = 0.0;
    let todo: Vec<(String, String, String)> = {
        let s = svc.shared.lock().unwrap();
        s.tasks.iter().filter(|t| t.state == TaskState::Done && !t.in_place && t.branch.is_some()).filter_map(|t| Some((t.id.clone(), t.workdir.clone()?, t.base.clone()?))).collect()
    };
    if todo.is_empty() && svc.shared.lock().unwrap().merged.is_empty() {
        // nothing finished yet: nothing can be merged (a task list that hasn't arrived waits for the next pass)
        let mut s = svc.shared.lock().unwrap();
        if !s.tasks.is_empty() && !s.merged_checked {
            s.merged_checked = true;
            s.version += 1;
        }
        return;
    }
    let flag = busy.get_or_insert_with(|| std::sync::Arc::new(AtomicBool::new(false))).clone();
    flag.store(true, Ordering::Relaxed);
    let shared = svc.shared.clone();
    std::thread::spawn(move || {
        let ids: HashSet<String> = todo.into_iter().filter(|(_, wd, base)| crate::review::is_merged(wd, base)).map(|(id, _, _)| id).collect();
        let mut s = shared.lock().unwrap();
        if s.merged != ids || !s.merged_checked {
            s.merged = ids;
            s.merged_checked = true;
            s.version += 1;
        }
        flag.store(false, Ordering::Relaxed);
    });
}

pub fn sync(svc: Res<Service>, mut live: ResMut<Live>, mut agents: ResMut<Agents>, mut toasts: ResMut<Toasts>, args: Res<crate::Args>) {
    if args.offline() {
        if args.moment && live.requests.is_empty() {
            demo_request(&mut live);
        }
        return;
    }
    let mut s = svc.shared.lock().unwrap();
    if s.version == live.seen {
        return;
    }
    live.seen = s.version;
    for n in s.notes.drain(..) {
        toasts.push(n);
    }
    if live.link != s.link {
        if let Some(Link::Down(e)) = &s.link {
            toasts.push(e.clone());
        }
        live.link = s.link.clone();
    }
    agents.set(pick(&s.tasks, &s.merged, crate::core::unix_now()));
    agents.set_runs(
        s.tasks
            .iter()
            .filter_map(|t| {
                let a = crate::working::parse_rfc3339(&t.started_at)?;
                let active = matches!(t.state, TaskState::Queued | TaskState::Working | TaskState::NeedsYou);
                Some((a, if active { i64::MAX } else { crate::working::parse_rfc3339(&t.updated_at).unwrap_or(a) }))
            })
            .collect(),
    );
    let mut reqs: Vec<RequestView> = s.requests.iter().filter(|r| r.status.is_open()).cloned().collect();
    reqs.sort_by(|a, b| a.opened_at.cmp(&b.opened_at));
    let titles: Vec<(String, String)> = s.tasks.iter().map(|t| (t.id.clone(), t.title.clone())).collect();
    if reqs != live.requests || titles != live.titles {
        live.requests = reqs;
        live.titles = titles;
        live.version += 1;
    }
}

/// The prototype's kind of moment, for `--moment` screenshots: an agent asking to run a command.
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
        suggestions: vec![],
        blocked_path: None,
        tool_use_id: None,
        agent_request: None,
        revises: None,
    };
    let mut r2 = info.clone();
    r2.id = "r2".into();
    r2.task = "t2".into();
    r2.tool = "Edit".into();
    r2.summary = "Squash and merge the login fix into main".into();
    let mut r3 = info.clone();
    r3.id = "r3".into();
    r3.task = "t3".into();
    r3.summary = "minio stopped because its disk is full; clear old caches and restart it".into();
    let rv = |i: sphere_proto::RequestInfo| RequestView { info: i, status: sphere_proto::RequestStatus::Open, opened_at: "2026-10-16T17:52:00Z".into() };
    live.requests = vec![rv(info), rv(r2), rv(r3)];
    live.titles = vec![("t1".into(), "Fix flaky login test".into()), ("t2".into(), "Merge the login fix".into()), ("t3".into(), "Restart minio".into())];
    live.version += 1;
}

/// When an agent finishes or fails: a toast, the sound, and (finished) Sphere says so, like the prototype's end.
pub fn transitions(
    svc: Res<Service>,
    args: Res<crate::Args>,
    mut seen: Local<Option<std::collections::HashMap<String, TaskState>>>,
    mut toasts: ResMut<Toasts>,
    mut sound: ResMut<crate::sound::Sound>,
    mut say: ResMut<crate::homepanels::Say>,
    mut fx: ResMut<crate::fx::Fx>,
) {
    if args.offline() {
        return;
    }
    let s = svc.shared.lock().unwrap();
    let now: std::collections::HashMap<String, TaskState> = s.tasks.iter().map(|t| (t.id.clone(), t.state)).collect();
    let Some(prev) = seen.as_ref() else {
        // what was already finished when the app started isn't news
        *seen = Some(now);
        return;
    };
    for t in &s.tasks {
        let before = prev.get(&t.id).copied();
        if before == Some(t.state) {
            continue;
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
    drop(s);
    *seen = Some(now);
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
        agents.set(pick(&tasks, &HashSet::new(), NOW));
        assert_eq!(agents.working(), 6, "agents past the fourth still count");
        assert_eq!(crate::panel::agent_rows(agents.list.len()), (3, 3));
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
        let names: Vec<String> = pick(&tasks, &HashSet::new(), NOW).into_iter().map(|a| a.name).collect();
        assert_eq!(names, vec!["Task f4", "Task f9", "Task d1"]);
    }

    #[test]
    fn finished_agents_leave_after_a_day_but_working_ones_stay() {
        let mut old = task("d0", TaskState::Done, "2026-10-17T10:00:00Z");
        old.steps = 5;
        let tasks = vec![old, task("w0", TaskState::Working, "2026-10-17T10:00:00Z"), task("x0", TaskState::Failed, "2026-10-17T11:00:00Z")];
        let names: Vec<String> = pick(&tasks, &HashSet::new(), NOW).into_iter().map(|a| a.name).collect();
        assert_eq!(names, vec!["Task w0"]);
    }

    #[test]
    fn a_merged_agent_reads_merged() {
        let tasks = vec![task("t1", TaskState::Done, "2026-10-19T00:00:00Z"), task("t2", TaskState::Done, "2026-10-19T00:01:00Z")];
        let merged: HashSet<String> = ["t1".to_string()].into();
        let rows = pick(&tasks, &merged, NOW);
        let step = |n: &str| rows.iter().find(|a| a.name == n).unwrap().step.clone();
        assert_eq!(step("Task t1"), "Merged · sphere/t1");
        assert_eq!(step("Task t2"), "Done: finished");
        assert_eq!(crate::appframe::finished(&tasks[0], &merged), "Merged");
        assert_eq!(crate::appframe::finished(&tasks[1], &merged), "Finished");
    }
}
