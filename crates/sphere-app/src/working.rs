//! The Working view (prototype: Working): an agent's run, live.
use bevy::prelude::*;

#[derive(Resource, Default)]
pub struct Working {
    pub open: bool,
    /// the task on screen; `None` follows the latest
    pub task: Option<String>,
    pub version: u64,
}

impl Working {
    /// Opens the view on the most recent agent.
    pub fn open_latest(&mut self) {
        self.open = true;
        self.task = None;
        self.version += 1;
    }
}
