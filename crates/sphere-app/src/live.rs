//! Turns the service snapshot into what the scene shows: the Agents panel rows, the open requests behind the
//! Agent moment card, the link state and toasts.
use crate::core::{AState, Agent, Agents};
use crate::service::{Link, Service};
use bevy::prelude::*;
use sphere_proto::{RequestView, TaskState, TaskView};

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

#[allow(dead_code)] // the Agent moment card (next) uses it
impl Live {
    pub fn title<'a>(&'a self, task: &'a str) -> &'a str {
        self.titles.iter().find(|(id, _)| id == task).map(|(_, t)| t.as_str()).unwrap_or(task)
    }
}

/// Short-lived lines shown above the command bar.
#[derive(Resource, Default)]
pub struct Toasts {
    pub list: Vec<(String, f32)>,
    pub version: u64,
}

impl Toasts {
    pub fn push(&mut self, s: impl Into<String>) {
        self.list.push((s.into(), 4.5));
        if self.list.len() > 3 {
            self.list.remove(0);
        }
        self.version += 1;
    }
}

fn row(t: &TaskView) -> Agent {
    let (state, chip) = match t.state {
        TaskState::Queued | TaskState::Working => (AState::Working, None),
        TaskState::NeedsYou => (AState::Needs, None),
        TaskState::Done => (AState::Done, None),
        TaskState::Failed => (AState::Failed, Some("FAILED")),
        TaskState::Stopped => (AState::Failed, Some("STOPPED")),
    };
    let step = if t.status.is_empty() { t.last_say.lines().next().unwrap_or("").to_string() } else { t.status.clone() };
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

/// The panel fits four rows: what needs you first, then working, then the most recent finished.
fn pick(tasks: &[TaskView]) -> Vec<Agent> {
    let rank = |t: &TaskView| match t.state {
        TaskState::NeedsYou => 0,
        TaskState::Working => 1,
        TaskState::Queued => 2,
        _ => 3,
    };
    let mut v: Vec<&TaskView> = tasks.iter().collect();
    v.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| b.updated_at.cmp(&a.updated_at)));
    v.into_iter().take(4).map(row).collect()
}

pub fn sync(svc: Res<Service>, mut live: ResMut<Live>, mut agents: ResMut<Agents>, mut toasts: ResMut<Toasts>, args: Res<crate::Args>) {
    if args.offline() {
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
    agents.set(pick(&s.tasks));
    let mut reqs: Vec<RequestView> = s.requests.iter().filter(|r| r.status.is_open()).cloned().collect();
    reqs.sort_by(|a, b| a.opened_at.cmp(&b.opened_at));
    let titles: Vec<(String, String)> = s.tasks.iter().map(|t| (t.id.clone(), t.title.clone())).collect();
    if reqs != live.requests || titles != live.titles {
        live.requests = reqs;
        live.titles = titles;
        live.version += 1;
    }
}

pub fn tick_toasts(time: Res<Time<Real>>, mut toasts: ResMut<Toasts>) {
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
