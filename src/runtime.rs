//! The agent runtime: tasks agents are working on and the state each is in.
//!
//! States follow herdr's model (https://github.com/herdrdev/herdr): an agent
//! is working, blocked on a person, or idle, and the states roll up so one
//! glance at the core says whether anything needs you. Agents are simulated
//! for now; the next step backs these tasks with real agent sessions.

/// What an agent task is doing, in herdr's vocabulary.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AgentState {
    Working,
    /// Waiting on a person: here, a result to approve.
    Blocked,
    Done,
}

impl AgentState {
    pub fn label(self) -> &'static str {
        match self {
            AgentState::Working => "Working",
            AgentState::Blocked => "Needs you",
            AgentState::Done => "Done",
        }
    }
}

/// The rolled-up state the core shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CoreState {
    Idle,
    Thinking,
    NeedsYou(usize),
}

pub struct Task {
    pub id: u64,
    pub text: String,
    pub state: AgentState,
    pub progress: f32,
    pub result: Option<String>,
    /// Slow tasks advance on their own clock, like long research jobs.
    pub slow: bool,
}

pub const PLAN: [&str; 3] = [
    "Understand what you asked",
    "Gather what it needs",
    "Prepare a result for you to check",
];

pub struct Runtime {
    pub tasks: Vec<Task>,
    next_id: u64,
    rng: Rng,
}

impl Default for Runtime {
    fn default() -> Self {
        let tasks = vec![
            Task {
                id: 1,
                text: "Draft a reply to Tolu about invoice #0423".into(),
                state: AgentState::Blocked,
                progress: 1.0,
                result: Some("Reply drafted. It confirms payment by the 24th.".into()),
                slow: false,
            },
            Task {
                id: 2,
                text: "Q4 market research for the board deck".into(),
                state: AgentState::Working,
                progress: 0.62,
                result: None,
                slow: true,
            },
            Task {
                id: 3,
                text: "Move tomorrow's dentist to a free slot".into(),
                state: AgentState::Done,
                progress: 1.0,
                result: Some("Moved to Tue 10:30.".into()),
                slow: false,
            },
        ];
        Self {
            tasks,
            next_id: 4,
            rng: Rng(0x9e3779b97f4a7c15),
        }
    }
}

impl Runtime {
    pub fn start(&mut self, text: impl Into<String>) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.tasks.insert(
            0,
            Task {
                id,
                text: text.into(),
                state: AgentState::Working,
                progress: 0.0,
                result: None,
                slow: false,
            },
        );
        id
    }

    pub fn get(&self, id: u64) -> Option<&Task> {
        self.tasks.iter().find(|t| t.id == id)
    }

    /// Advances working tasks. Returns the texts of tasks that just became
    /// ready for review.
    pub fn tick(&mut self, dt: f32) -> Vec<String> {
        let mut ready = Vec::new();
        for t in self
            .tasks
            .iter_mut()
            .filter(|t| t.state == AgentState::Working)
        {
            let speed = if t.slow {
                0.004
            } else {
                0.22 + 0.25 * self.rng.next()
            };
            t.progress = (t.progress + dt * speed).min(1.0);
            if t.progress >= 1.0 {
                t.state = AgentState::Blocked;
                t.result = Some(result_for(&t.text));
                ready.push(t.text.clone());
            }
        }
        ready
    }

    pub fn approve(&mut self, id: u64) {
        if let Some(t) = self.tasks.iter_mut().find(|t| t.id == id) {
            t.state = AgentState::Done;
        }
    }

    pub fn discard(&mut self, id: u64) {
        self.tasks.retain(|t| t.id != id);
    }

    pub fn working(&self) -> usize {
        self.tasks
            .iter()
            .filter(|t| t.state == AgentState::Working)
            .count()
    }

    pub fn blocked(&self) -> usize {
        self.tasks
            .iter()
            .filter(|t| t.state == AgentState::Blocked)
            .count()
    }

    pub fn core_state(&self) -> CoreState {
        match (self.blocked(), self.working()) {
            (n, _) if n > 0 => CoreState::NeedsYou(n),
            (_, n) if n > 0 => CoreState::Thinking,
            _ => CoreState::Idle,
        }
    }
}

pub fn result_for(text: &str) -> String {
    let s = text.to_lowercase();
    if s.contains("inbox") {
        "12 new emails. 3 need you, 2 replies drafted, 7 archived.".into()
    } else if s.contains("afternoon") || s.contains("plan") {
        "14:00 Ada 1:1 (brief ready), 15:00 focus block, 16:30 gym.".into()
    } else {
        "Done. A summary is ready for you to check.".into()
    }
}

/// Small deterministic random source for the simulations.
#[derive(Clone)]
pub struct Rng(pub u64);

impl Rng {
    /// A value in [0, 1).
    pub fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }

    /// A value in [-1, 1).
    pub fn signed(&mut self) -> f32 {
        self.next() * 2.0 - 1.0
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[(self.next() * items.len() as f32) as usize % items.len()]
    }
}
