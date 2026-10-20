//! The Agent swarm (S from home, "swarm" in the command bar, Esc back): every agent at once, as satellites on
//! one tilted orbit per repo around the core, with a shared outer ring for work that is ready to merge. Hover
//! shows an agent's recent steps as a track behind it, a click (or 1-9, Tab) brings it front-left with a focus
//! panel, N goes to the next one that needs you, and a request opens the Agent moment card from its satellite.
//! The design is `look/swarm/SPEC.md` (from the approved prototype); the data is sphered's (`model`).
pub mod feedback;
pub mod hud;
pub mod input;
pub mod model;
pub mod scene;
pub mod view;

use crate::util::Spring;
use bevy::prelude::*;

/// The swarm's own atlas in `AppAtlases` (apps use 0-5).
pub const ATLAS: u8 = 6;
/// The focus panel's own atlas (the tall panel, over half the swarm's texels): only resident while a focus is open.
pub const ATLAS_FOCUS: u8 = 7;

/// While Hold the Swarm is on, the core rises this far (stage px) and it and the orbits shrink by this much, so the
/// orbits clear the game's card at the bottom.
pub const GAME_LIFT: f32 = 130.0;
pub const GAME_SHRINK: f32 = 0.18;

/// How far the orbit plane tips from face-on, in degrees (SPEC 2.1): Low 71, Mid 60, High 46.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tilt {
    Low,
    #[default]
    Mid,
    High,
}

impl Tilt {
    pub fn deg(self) -> f32 {
        match self {
            Tilt::Low => 71.0,
            Tilt::Mid => 60.0,
            Tilt::High => 46.0,
        }
    }
}

/// What the swarm is showing: every agent, one agent front-left with its focus panel, or one agent's request
/// on the Agent moment card.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Overview,
    Focus(String),
    Request(String),
}

#[derive(Resource)]
pub struct Swarm {
    pub open: bool,
    /// how open the swarm view is (Big pace)
    pub amt: Spring,
    pub mode: Mode,
    /// the agent under the pointer (a satellite or its label)
    pub hover: Option<String>,
    /// the rest of the scene draws back while a request or a focus panel is up (SPEC `ui.dim`, 40/12)
    pub dim: Spring,
    /// a focus panel is open: the core slides right and the orbits shrink (SPEC `focAmt`, 40/12)
    pub focus_amt: Spring,
    /// Hold the Swarm is on: the core and the orbits rise and shrink to leave the bottom to its card (40/12)
    pub game_amt: Spring,
    pub tilt: Tilt,
    /// degrees from face-on, springing to `tilt` (40/11)
    pub orbit_tilt: Spring,
    /// T: every agent's step track, not only the hovered one's
    pub tracks: bool,
    /// the agents, repos, conflicts and counts on screen, rebuilt when sphered's data changes
    pub model: model::Model,
    /// bumps when the model or the mode changes (panels and labels redraw on it)
    pub version: u64,
    /// Hold the Swarm has the view (`game`): it writes `model`, and the swarm's own sync, keys, pointer, panels and
    /// feedback stand aside
    pub game: bool,
    /// the game's title, pause or results panel is over the core: the scene draws back as for a request
    pub game_panel: bool,
}

impl Default for Swarm {
    fn default() -> Self {
        Swarm {
            open: false,
            amt: Spring::new(0.0, 26.0, 9.0),
            mode: Mode::Overview,
            hover: None,
            dim: Spring::new(0.0, 40.0, 12.0),
            focus_amt: Spring::new(0.0, 40.0, 12.0),
            game_amt: Spring::new(0.0, 40.0, 12.0),
            tilt: Tilt::Mid,
            orbit_tilt: Spring::new(Tilt::Mid.deg(), 40.0, 11.0),
            tracks: false,
            model: model::Model::default(),
            version: 0,
            game: false,
            game_panel: false,
        }
    }
}

impl Swarm {
    pub fn open(&mut self) {
        self.open = true;
        self.mode = Mode::Overview;
        self.version += 1;
    }
    pub fn close(&mut self) {
        self.open = false;
        self.mode = Mode::Overview;
        self.hover = None;
        self.version += 1;
    }
    /// The agent in focus or on the request card.
    pub fn current(&self) -> Option<&str> {
        match &self.mode {
            Mode::Focus(id) | Mode::Request(id) => Some(id),
            Mode::Overview => None,
        }
    }
}

pub struct SwarmPlugin;

impl Plugin for SwarmPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Swarm>();
        model::build(app);
        scene::build(app);
        hud::build(app);
        view::build(app);
        input::build(app);
        feedback::build(app);
    }
}
