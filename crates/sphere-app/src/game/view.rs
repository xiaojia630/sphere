//! The run as the swarm draws it: each agent a satellite on its repo's ring (amber while it asks, dim while it is
//! stalled), each merged task going out to the ready ring and down into the core, and the points floating up from
//! the satellite that earned them.
use super::engine::{AState, Agent, Run};
use crate::swarm::model::{self, Model, State, Step};
use crate::util::*;
use bevy::prelude::*;
use bevy::text::LineHeight;

/// A merged task's satellite on its way out: on the ready ring, then into the core.
#[derive(Clone, Debug, PartialEq)]
pub struct Leaving {
    /// the roster place of the agent that merged it
    pub idx: usize,
    pub id: String,
    pub title: String,
    pub repo: &'static str,
    /// run seconds when it merged
    pub at: f64,
}

/// How long a merged task sits on the ready ring, and when it is gone into the core.
const READY_SECS: f64 = 0.45;
const GONE_SECS: f64 = 1.3;

/// Points (and words) floating up from a satellite.
#[derive(Clone, Debug, PartialEq)]
pub struct Pop {
    pub id: String,
    pub text: String,
    pub role: Role,
    /// seconds since it appeared
    pub t: f32,
    /// where its satellite was last seen on screen (it keeps rising there once the satellite is gone)
    pub at: Option<Vec2>,
}

pub const POP_SECS: f32 = 1.1;

#[derive(Default)]
pub struct View {
    pub leaving: Vec<Leaving>,
    pub pops: Vec<Pop>,
    /// the repos in the order their first agent joined: a repo keeps its ring
    repos: Vec<&'static str>,
}

/// A task's satellite: the agent's place in the roster and how many tasks it merged before this one.
pub fn agent_id(a: &Agent) -> String {
    format!("game-{}-{}", a.idx, a.merged)
}

fn title(task: &str) -> String {
    let mut c = task.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

impl View {
    pub fn reset(&mut self) {
        *self = View::default();
    }

    /// A task merged: its satellite goes out the way a real merge does.
    pub fn merged(&mut self, run: &Run, agent: usize, task: &str) {
        let a = &run.agents[agent];
        self.leaving.push(Leaving { idx: a.idx, id: format!("game-{}-{}", a.idx, a.merged.saturating_sub(1)), title: title(task), repo: a.repo, at: run.t });
    }

    pub fn pop(&mut self, run: &Run, agent: usize, text: impl Into<String>, role: Role) {
        let id = self.leaving.iter().rev().find(|l| l.idx == run.agents[agent].idx && run.t - l.at < GONE_SECS).map(|l| l.id.clone()).unwrap_or_else(|| agent_id(&run.agents[agent]));
        self.pops.push(Pop { id, text: text.into(), role, t: 0.0, at: None });
        if self.pops.len() > POPS {
            self.pops.remove(0);
        }
    }

    fn repo(&mut self, key: &'static str) -> usize {
        match self.repos.iter().position(|r| *r == key) {
            Some(i) => i,
            None => {
                self.repos.push(key);
                self.repos.len() - 1
            }
        }
    }

    /// The swarm's model for the run at `now` (unix seconds, for the step times).
    pub fn model(&mut self, run: &Run, now: i64) -> Model {
        // a successor that asks cuts its predecessor's way out short
        self.leaving.retain(|l| {
            let asking = run.agents.iter().position(|a| a.idx == l.idx).is_some_and(|ai| run.queue.iter().any(|q| q.agent == ai));
            run.t - l.at < GONE_SECS && !asking
        });
        let mut agents = Vec::new();
        for (ai, a) in run.agents.iter().enumerate() {
            let repo = self.repo(a.repo);
            if let Some(l) = self.leaving.iter().find(|l| l.idx == a.idx) {
                let age = run.t - l.at;
                agents.push(model::Agent {
                    id: l.id.clone(),
                    title: l.title.clone(),
                    repo,
                    branch: format!("feat/{}", super::engine::slug(&l.title)),
                    state: if age < READY_SECS { State::Done } else { State::Merged },
                    p: 1.0,
                    step: "merged into main".to_string(),
                    step_seq: u64::from(a.k + a.done) + 1,
                    steps: vec![Step { text: "merged into main".to_string(), at: now }],
                    game: true,
                    ..default()
                });
                continue;
            }
            let req = run.queue.iter().find(|q| q.agent == ai);
            let (state, step) = match (a.state, req) {
                (AState::Wait, Some(q)) => (State::Needs, format!("waiting to {} {}", q.tool.verb().to_lowercase(), q.text)),
                (AState::Stall, _) => (State::Queued, "stalled · waited too long on you".to_string()),
                _ => (State::Working, a.last.clone()),
            };
            agents.push(model::Agent {
                id: agent_id(a),
                title: title(a.task),
                repo,
                branch: format!("feat/{}", super::engine::slug(a.task)),
                state,
                p: (a.done as f32 / a.steps.max(1) as f32).clamp(0.04, 0.97),
                step: crate::appframe::clip(&step, 52),
                step_seq: u64::from(a.k + a.done),
                steps: vec![Step { text: a.last.clone(), at: now }],
                queue_pos: (state == State::Queued).then_some(0),
                game: true,
                ..default()
            });
        }
        let repos = self.repos.iter().map(|k| model::Repo { key: k.to_string(), path: format!("~/{k}"), branch: "main".to_string() }).collect();
        Model { agents, repos, conflicts: Vec::new(), limit: 9, cost_today: 0.0 }
    }
}

// ---------------------------------------------------------------- the floating points

/// At most this many float at once.
pub const POPS: usize = 8;

#[derive(Component)]
pub struct PopText(usize);

pub fn setup_pops(mut commands: Commands, root: Res<crate::hud::OverlayRoot>, f: Res<crate::fonts::UiFonts>) {
    for i in 0..POPS {
        let e = commands
            .spawn((
                PopText(i),
                Text::new(""),
                crate::panel::font(&f.ui700, 13.0, 2.6),
                TextColor(Color::NONE),
                LineHeight::Px(16.0),
                TextLayout { linebreak: LineBreak::NoWrap, ..default() },
                Node { position_type: PositionType::Absolute, ..default() },
                UiTransform::default(),
                Visibility::Hidden,
            ))
            .id();
        commands.entity(root.0).add_child(e);
    }
}

/// Places each pop over its satellite, rising and fading.
#[allow(clippy::type_complexity)]
pub fn draw_pops(
    sim: Res<crate::core::Sim>,
    mut game: ResMut<super::Game>,
    scene: Res<crate::swarm::scene::SceneOut>,
    cams: Query<(&Camera, &Transform), With<crate::CamCore>>,
    mut q: Query<(&PopText, &mut Text, &mut TextColor, &mut Node, &mut Visibility, &ComputedNode)>,
) {
    let dt = sim.dt;
    let v = &mut game.view;
    if v.pops.is_empty() && q.iter().all(|(_, _, _, _, vis, _)| *vis == Visibility::Hidden) {
        return;
    }
    let cam = cams.single().ok();
    for p in &mut v.pops {
        p.t += dt;
        let seen = scene.nodes.iter().find(|n| n.0 == p.id).and_then(|n| cam.and_then(|(c, t)| c.world_to_viewport(&GlobalTransform::from(*t), n.1).ok())).filter(|v| v.is_finite());
        p.at = seen.or(p.at);
    }
    v.pops.retain(|p| p.t < POP_SECS);
    for (pt, mut text, mut col, mut node, mut vis, cn) in &mut q {
        let Some((p, base)) = v.pops.get(pt.0).and_then(|p| p.at.map(|a| (p, a))) else {
            vis.set_if_neq(Visibility::Hidden);
            continue;
        };
        let k = (p.t / POP_SECS).clamp(0.0, 1.0);
        if text.0 != p.text {
            text.0 = p.text.clone();
        }
        let w = cn.size().x.max(1.0) * cn.inverse_scale_factor();
        node.left = px(base.x - w / 2.0);
        node.top = px(base.y - 34.0 - k * 24.0);
        col.0 = crate::panel::with_alpha(role_color(p.role), 1.0 - k * k);
        vis.set_if_neq(Visibility::Inherited);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::engine::{Answer, Ev};

    #[test]
    fn the_run_draws_as_the_swarm_with_asks_in_amber() {
        let mut run = Run::new("2026-10-20");
        let mut v = View::default();
        let m = v.model(&run, 0);
        assert_eq!(m.agents.len(), 3);
        assert_eq!(m.repos.iter().map(|r| r.key.as_str()).collect::<Vec<_>>(), vec!["web-app", "api", "sphere"]);
        assert!(m.agents.iter().all(|a| a.state == State::Working));
        while run.queue.is_empty() {
            run.update(0.05);
        }
        let m = v.model(&run, 0);
        let asking = run.queue[0].agent;
        let a = m.agents.iter().find(|a| a.id == agent_id(&run.agents[asking])).unwrap();
        assert_eq!(a.state, State::Needs);
        assert!(a.step.starts_with("waiting to"), "{}", a.step);
    }

    #[test]
    fn a_merged_task_goes_to_the_ready_ring_then_into_the_core_and_its_agent_comes_back() {
        let mut run = Run::new("2026-10-20");
        let mut v = View::default();
        // answer agent 0 until it merges
        loop {
            run.update(0.05);
            let mine: Vec<usize> = (0..run.queue.len()).filter(|&i| run.queue[i].agent == 0).collect();
            for i in (0..run.queue.len()).rev() {
                if !mine.contains(&i) {
                    run.queue.remove(i);
                }
            }
            run.cur = 0;
            if let Some(q) = run.queue.first_mut() {
                q.danger = false;
                run.answer(Answer::Yes);
            }
            if let Some(task) = run.events.iter().find_map(|e| if let Ev::Merged { agent: 0, task, .. } = e { Some(*task) } else { None }) {
                v.merged(&run, 0, task);
                break;
            }
            run.events.clear();
        }
        let m = v.model(&run, 0);
        assert_eq!(m.agents[0].id, "game-0-0");
        assert_eq!(m.agents[0].state, State::Done);
        run.update(0.6);
        assert_eq!(v.model(&run, 0).agents[0].state, State::Merged);
        run.update(0.8);
        let m = v.model(&run, 0);
        assert_eq!(m.agents[0].id, "game-0-1");
        assert!(m.agents.len() <= model::MAX_AGENTS);
    }
}
