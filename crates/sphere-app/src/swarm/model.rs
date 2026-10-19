//! What the swarm shows, from sphered's data (or the prototype's example agents for `--demo` and the look
//! tests): the agents in a stable order, the repos they orbit, conflicts, run slots and the strip's counts.
//! Everything here is plain data; `scene` and `hud` draw it.
use bevy::prelude::*;

/// An agent's state on the swarm (SPEC section 3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum State {
    #[default]
    Working,
    /// waiting on you: a request is open
    Needs,
    /// finished, its branch not merged yet: on the outer ring, "Ready to merge"
    Done,
    /// waiting for a run slot
    Queued,
    /// its branch was just merged: it collapses into the core, then leaves the model
    Merged,
    /// stopped or failed: it fades out, then leaves the model
    Stopped,
}

/// One of the agent's recent steps, newest first in `Agent::steps`.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Step {
    pub text: String,
    /// unix seconds when it started
    pub at: i64,
}

/// A file the agent touched, with its diff stats.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct File {
    pub path: String,
    pub added: u64,
    pub removed: u64,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Agent {
    /// sphered's task id
    pub id: String,
    pub title: String,
    /// index into `Model::repos` (its ring)
    pub repo: usize,
    pub branch: String,
    pub state: State,
    /// 0..1 (real agents have no percent: it follows the step count, like the Agents panel; done is 1)
    pub p: f32,
    /// what it is doing now (the label's step line)
    pub step: String,
    pub cost: f64,
    /// unix seconds
    pub started_at: i64,
    /// newest first, at most 10
    pub steps: Vec<Step>,
    pub files: Vec<File>,
    /// the file its current step edits, if any
    pub editing: Option<String>,
    /// queued agents keep their place in the queue
    pub queue_pos: Option<usize>,
    /// bumps each time a new step lands (the satellite pulses)
    pub step_seq: u64,
}

/// A repo with agents on it: one orbit ring.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Repo {
    /// the folder name, shown as the ring's key (e.g. "api-server")
    pub key: String,
    pub path: String,
    /// the branch its checkout is on
    pub branch: String,
}

/// Two agents in one repo editing the same file.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Conflict {
    pub a: String,
    pub b: String,
    pub file: String,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Model {
    /// stable order (the strip's dots, 1-9, Tab): oldest first; at most 9
    pub agents: Vec<Agent>,
    pub repos: Vec<Repo>,
    pub conflicts: Vec<Conflict>,
    /// how many agents run at once (sphered's max_concurrent; 3 when unknown)
    pub limit: usize,
    pub cost_today: f64,
}

impl Model {
    pub fn agent(&self, id: &str) -> Option<&Agent> {
        self.agents.iter().find(|a| a.id == id)
    }
    pub fn count(&self, s: State) -> usize {
        self.agents.iter().filter(|a| a.state == s).count()
    }
    /// The agents in a run slot (working or waiting on you), in model order.
    pub fn running(&self) -> Vec<&Agent> {
        self.agents.iter().filter(|a| matches!(a.state, State::Working | State::Needs)).collect()
    }
}

pub fn build(_app: &mut App) {}
