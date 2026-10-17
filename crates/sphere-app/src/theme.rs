//! The five themes (prototype v2): Balanced (default), Full color, Cyan, Black, Red. C cycles them.
//!
//! UI inside the panels is always drawn in the Balanced colours; the panel shader swaps each Balanced role colour
//! for the current theme's (and, in Full color, the panel's own hue), so a theme change cross-fades smoothly with
//! no UI rebuilt. The core's materials, bloom and the screen overlay read the live palette directly.
use crate::util::*;
use bevy::prelude::*;
use std::sync::RwLock;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ThemeId {
    #[default]
    Balanced,
    Full,
    Cyan,
    Black,
    Red,
}

pub const ALL: [ThemeId; 5] = [ThemeId::Balanced, ThemeId::Full, ThemeId::Cyan, ThemeId::Black, ThemeId::Red];
pub const ROLES: [Role; 8] = [Role::Line, Role::Dim, Role::Hot, Role::Fill, Role::Acc, Role::Deep, Role::Ok, Role::Bad];

impl ThemeId {
    pub fn label(self) -> &'static str {
        match self {
            ThemeId::Balanced => "Balanced",
            ThemeId::Full => "Full color",
            ThemeId::Cyan => "Cyan",
            ThemeId::Black => "Black",
            ThemeId::Red => "Red",
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            ThemeId::Balanced => "balanced",
            ThemeId::Full => "full",
            ThemeId::Cyan => "cyan",
            ThemeId::Black => "black",
            ThemeId::Red => "red",
        }
    }
    pub fn from_key(s: &str) -> Option<ThemeId> {
        ALL.iter().copied().find(|t| t.key() == s)
    }
    pub fn next(self) -> ThemeId {
        let i = ALL.iter().position(|t| *t == self).unwrap_or(0);
        ALL[(i + 1) % ALL.len()]
    }
    /// line, dim, hot, fill, acc, deep, ok, bad (the prototype's PAL table)
    fn hexes(self) -> [&'static str; 8] {
        match self {
            ThemeId::Balanced => ["#a6d3e6", "#3e6678", "#f1f9fc", "#dff4ff", "#ffb347", "#2f7493", "#5fe3a1", "#ff5a4f"],
            ThemeId::Full => ["#c4e2f1", "#4e7b90", "#ffffff", "#e9f7ff", "#ffb347", "#3f7f9b", "#5fe3a1", "#ff5a4f"],
            ThemeId::Cyan => ["#7ce2ff", "#2c88a8", "#e6fbff", "#bff3ff", "#ffb347", "#1f6f8f", "#7ce2ff", "#ffb347"],
            ThemeId::Black => ["#d3d8db", "#5a6167", "#ffffff", "#eef1f2", "#f0bd6a", "#3e4449", "#d3d8db", "#f0bd6a"],
            ThemeId::Red => ["#e8e3e1", "#6e6260", "#ffffff", "#ff3b30", "#ffb347", "#a3231a", "#e8e3e1", "#ffb347"],
        }
    }
    pub fn bloom(self) -> f32 {
        match self {
            ThemeId::Balanced => 0.6,
            ThemeId::Full => 0.72,
            ThemeId::Cyan => 0.68,
            ThemeId::Black => 0.34,
            ThemeId::Red => 0.58,
        }
    }
    /// The screen tint behind the core (`--tint`).
    pub fn tint(self) -> &'static str {
        match self {
            ThemeId::Balanced => "#03141a",
            ThemeId::Full => "#04141f",
            ThemeId::Cyan => "#031a22",
            ThemeId::Black => "#0d0f10",
            ThemeId::Red => "#230605",
        }
    }
}

fn v3(hex_s: &str) -> Vec3 {
    let c = hex(hex_s).to_srgba();
    Vec3::new(c.red, c.green, c.blue)
}

/// Full color: each layer of the core carries its own hue. (line, dim, hot, fill, deep, b2) per deck, in the
/// core's deck order: time, load, agents, signal, voice, core.
pub const FULL_DECKS: [[&str; 6]; 6] = [
    ["#a48bff", "#5b4aa8", "#efe9ff", "#c3b2ff", "#6d4fd6", "#7de3ff"],
    ["#3ad7ff", "#1f7e9c", "#e2faff", "#8ee9ff", "#1c93bd", "#a48bff"],
    ["#5fe3a1", "#2c8e63", "#e6fff3", "#9ff2c8", "#2ba06c", "#ffd166"],
    ["#ffd166", "#9a7730", "#fff6e0", "#ffe3a0", "#c79a3a", "#ff6fb5"],
    ["#ff6fb5", "#a3447a", "#ffe8f4", "#ffa8d2", "#d14a8e", "#ffd166"],
    ["#dff3ff", "#6f99ab", "#ffffff", "#ffffff", "#7fc9e8", "#9fe4ff"],
];

/// Full color: each panel blends toward its own hue (line, dim, hot, fill). `None` keeps the global palette.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hue {
    System,
    Apps,
    Tele,
    Clock,
    Rail,
    Agents,
    Neutral,
    /// The Working view's run panel and step chips (the Code tile's violet).
    Code,
}

impl Hue {
    fn hexes(self) -> [&'static str; 4] {
        match self {
            Hue::System => ["#62d8ff", "#2b86a6", "#e4f9ff", "#9aeaff"],
            Hue::Apps => ["#b8a4ff", "#6856b3", "#f1ecff", "#cdbeff"],
            Hue::Tele => ["#5de5cc", "#28907f", "#e3fff8", "#8ff2df"],
            Hue::Clock => ["#ffd98f", "#9b7c3a", "#ffffff", "#ffd166"],
            Hue::Rail => ["#a9afff", "#5b61b2", "#eceeff", "#c3c7ff"],
            Hue::Agents => ["#62e8a6", "#2d9a67", "#e4fff1", "#98f5c6"],
            Hue::Neutral => ["#e6f1f7", "#8aa1ad", "#ffffff", "#ffffff"],
            Hue::Code => ["#b8a4ff", "#6856b3", "#f1ecff", "#cdbeff"],
        }
    }
}

/// The animated palette everything reads. sRGB components.
#[derive(Clone, Copy, Debug)]
pub struct Live {
    pub roles: [Vec3; 8],
    pub decks: [[Vec3; 6]; 6],
    pub tint: Vec3,
    pub bloom: f32,
    /// 1 in Full color, cross-fading
    pub fx: f32,
}

impl Live {
    fn target(id: ThemeId) -> Live {
        let h = id.hexes();
        let roles = [v3(h[0]), v3(h[1]), v3(h[2]), v3(h[3]), v3(h[4]), v3(h[5]), v3(h[6]), v3(h[7])];
        let mut decks = [[Vec3::ZERO; 6]; 6];
        for (i, d) in decks.iter_mut().enumerate() {
            if id == ThemeId::Full {
                for (j, c) in d.iter_mut().enumerate() {
                    *c = v3(FULL_DECKS[i][j]);
                }
            } else {
                // line, dim, hot, fill, deep from the palette; b2 is the line colour
                *d = [roles[0], roles[1], roles[2], roles[3], roles[5], roles[0]];
            }
        }
        Live { roles, decks, tint: v3(id.tint()), bloom: id.bloom(), fx: if id == ThemeId::Full { 1.0 } else { 0.0 } }
    }
}

static LIVE: RwLock<Option<Live>> = RwLock::new(None);

fn live() -> Live {
    LIVE.read().unwrap().unwrap_or_else(|| Live::target(ThemeId::Balanced))
}

fn idx(r: Role) -> usize {
    ROLES.iter().position(|x| *x == r).unwrap_or(0)
}

/// The current (animated) colour of a role.
pub fn now(r: Role) -> Color {
    let c = live().roles[idx(r)];
    Color::srgb(c.x, c.y, c.z)
}

pub fn now_lin(r: Role) -> Vec4 {
    let c = now(r).to_linear();
    Vec4::new(c.red, c.green, c.blue, 1.0)
}

/// A deck's colour for a role (Full color gives each deck its own hue). `b2` is index 5.
pub fn deck_lin(deck: usize, r: Role) -> Vec4 {
    let l = live();
    let j = match r {
        Role::Line => 0,
        Role::Dim => 1,
        Role::Hot => 2,
        Role::Fill => 3,
        Role::Deep => 4,
        _ => return now_lin(r),
    };
    let c = l.decks[deck.min(5)][j];
    let c = Color::srgb(c.x, c.y, c.z).to_linear();
    Vec4::new(c.red, c.green, c.blue, 1.0)
}

pub fn deck_b2_lin(deck: usize) -> Vec4 {
    let c = live().decks[deck.min(5)][5];
    let c = Color::srgb(c.x, c.y, c.z).to_linear();
    Vec4::new(c.red, c.green, c.blue, 1.0)
}

pub fn bloom() -> f32 {
    live().bloom
}
pub fn tint() -> Vec3 {
    live().tint
}

/// The colour swap for a panel's shader: the Balanced role colours (what UI is drawn in) and what each becomes now,
/// both linear. Roles 0..8 in `ROLES` order.
pub fn panel_map(hue: Option<Hue>) -> ([Vec4; 8], [Vec4; 8]) {
    let base = Live::target(ThemeId::Balanced);
    let l = live();
    let lin = |c: Vec3| {
        let c = Color::srgb(c.x, c.y, c.z).to_linear();
        Vec4::new(c.red, c.green, c.blue, 1.0)
    };
    let mut from = [Vec4::ZERO; 8];
    let mut to = [Vec4::ZERO; 8];
    for i in 0..8 {
        from[i] = lin(base.roles[i]);
        let mut c = l.roles[i];
        // Full color: line, dim, hot, fill blend toward the panel's hue
        if let (Some(h), true) = (hue, i < 4) {
            c = c.lerp(v3(h.hexes()[i]), l.fx);
        }
        to[i] = lin(c);
    }
    (from, to)
}

#[derive(Resource)]
pub struct Theme {
    pub id: ThemeId,
    /// Bumps every frame the palette moves (systems that copy colours watch it).
    pub version: u64,
    target: Live,
}

impl Theme {
    pub fn new(id: ThemeId) -> Self {
        let t = Live::target(id);
        *LIVE.write().unwrap() = Some(t);
        Theme { id, version: 1, target: t }
    }
    pub fn set(&mut self, id: ThemeId) {
        self.id = id;
        self.target = Live::target(id);
        save(id);
    }
}

fn pref_file() -> std::path::PathBuf {
    sphere_proto::paths::default_state_dir().join("app").join("theme")
}

pub fn saved() -> ThemeId {
    std::fs::read_to_string(pref_file()).ok().and_then(|s| ThemeId::from_key(s.trim())).unwrap_or_default()
}

fn save(id: ThemeId) {
    let f = pref_file();
    if let Some(d) = f.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(f, id.key());
}

/// Eases the live palette toward the theme (the prototype's 1 - e^(-4.5 dt) per frame).
pub fn animate(time: Res<Time<Real>>, mut theme: ResMut<Theme>) {
    let kk = 1.0 - (-4.5 * time.delta_secs().min(1.0 / 30.0)).exp();
    let mut l = live();
    let t = theme.target;
    let mut moved = 0.0f32;
    let mut step = |a: &mut Vec3, b: Vec3| {
        let n = a.lerp(b, kk);
        moved = moved.max((n - *a).abs().max_element());
        *a = if (b - n).abs().max_element() < 0.002 { b } else { n };
    };
    for i in 0..8 {
        step(&mut l.roles[i], t.roles[i]);
    }
    for i in 0..6 {
        for j in 0..6 {
            step(&mut l.decks[i][j], t.decks[i][j]);
        }
    }
    step(&mut l.tint, t.tint);
    let (b, f) = (lerp(l.bloom, t.bloom, kk), lerp(l.fx, t.fx, kk));
    moved = moved.max((b - l.bloom).abs()).max((f - l.fx).abs());
    l.bloom = if (t.bloom - b).abs() < 0.002 { t.bloom } else { b };
    l.fx = if (t.fx - f).abs() < 0.002 { t.fx } else { f };
    if moved > 0.0 {
        *LIVE.write().unwrap() = Some(l);
        theme.version += 1;
    }
}
