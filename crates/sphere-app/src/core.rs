//! The core: six ring decks (time, load, agents, signal, voice, sphere) ported from the prototype.
use crate::fonts::{raster_label, RAJDHANI_600};
use crate::geo::*;
use crate::glow::*;
use crate::util::*;
use bevy::{asset::RenderAssetUsages, mesh::PrimitiveTopology, prelude::*};

// ---------------------------------------------------------------- resources

#[derive(Resource)]
pub struct Sim {
    /// simulated seconds since the previous frame (clamped, or fixed in --shot mode)
    pub dt: f32,
    pub t: f32,
    pub boot_t: f32,
    pub fixed_dt: Option<f32>,
    /// `--shot` freezes the simulation once its frames are rendered, so readbacks can settle
    pub frozen: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Flat,
    Depth,
    /// the rings fold around the core like a gyroscope
    Orbit,
}

#[derive(Resource)]
pub struct CoreCtl {
    pub mode: Mode,
    pub q_user: Quat,
    pub q_target: Quat,
    pub q_par: Quat,
    pub spacing: Spring,
    pub depth: Spring,
    pub zoom: Spring,
    pub spin_mul: f32,
    pub pointer: Option<Vec2>, // 0..1 in window
    pub focus_amt: f32,
    pub depth_amt: f32,
    /// the core eases back to its pose; off while you turn it by hand, until a mode change or reset
    pub auto_orient: bool,
    /// drag inertia, rad/s about world Y (x) and X (y)
    pub omega: Vec2,
    /// the whole stack folded onto the sphere (X)
    pub collapsed: bool,
    /// seconds since the last collapse/expand (for the stagger)
    pub collapse_t: f32,
    /// added highlight on every deck, decays e^-3.2dt
    pub flash: f32,
    /// every collapsed ring has landed (flash and the drop sound once)
    pub landed: bool,
    /// how open the core is (the app orbit), 0..1
    pub orb: f32,
    /// the Working view: the core slides aside, 0..1
    pub work: f32,
    /// the core spins faster while an agent works
    pub work_spin: f32,
}

impl CoreCtl {
    pub fn new(intro: bool) -> Self {
        let mut c = Self {
            mode: Mode::Flat,
            q_user: Quat::IDENTITY,
            q_target: Quat::IDENTITY,
            q_par: Quat::IDENTITY,
            spacing: Spring::new(8.0, 46.0, 11.5),
            depth: Spring::new(3.0, 60.0, 13.0),
            zoom: Spring::new(1.0, 80.0, 18.0),
            spin_mul: 1.0,
            pointer: None,
            focus_amt: 0.0,
            depth_amt: 0.0,
            auto_orient: true,
            omega: Vec2::ZERO,
            collapsed: false,
            collapse_t: 10.0,
            flash: 0.0,
            landed: false,
            orb: 0.0,
            work: 0.0,
            work_spin: 1.0,
        };
        if intro {
            c.spacing.set(165.0);
            c.depth.set(80.0);
            c.zoom.set(1.22);
            c.zoom.t = 1.0;
            c.q_user = Quat::from_euler(EulerRot::YXZ, -26.0 * DEG, 30.0 * DEG, 0.0);
        }
        c
    }
    pub fn set_mode(&mut self, m: Mode) {
        self.mode = m;
        self.q_target = match m {
            Mode::Flat => Quat::IDENTITY,
            Mode::Depth => Quat::from_euler(EulerRot::YXZ, -36.0 * DEG, 14.0 * DEG, 0.0),
            Mode::Orbit => Quat::from_euler(EulerRot::YXZ, -12.0 * DEG, 16.0 * DEG, 0.0),
        };
        self.auto_orient = true;
        self.omega = Vec2::ZERO;
    }
}

/// Window to stage mapping. The prototype lays a 1600x1000 stage out above its 95px dock.
#[derive(Resource, Default, Clone, Copy)]
pub struct StageView {
    pub k: f32,
    pub d: f32,
    pub base_y: f32,
    pub w: f32,
    pub h: f32,
}
impl StageView {
    pub fn compute(w: f32, h: f32) -> Self {
        let (sw, sh) = (1600.0, 1000.0);
        let reserved = 95.0;
        let top_pad = 6.0;
        let avail_h = (h - reserved - top_pad).max(160.0);
        let avail_w = (w - 24.0).max(160.0);
        let k = (avail_w / sw).min(avail_h / sh);
        let d = (h / k) / (2.0 * (40.0f32 * DEG / 2.0).tan());
        let base_y = ((top_pad + avail_h / 2.0) - h / 2.0) / k;
        Self { k, d, base_y, w, h }
    }
}

pub const CORE_Y: f32 = 22.0; // stage (800, 478) -> world (0, 22)

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AState {
    Working,
    Needs,
    Done,
    /// Failed or stopped; `Agent::chip` says which.
    Failed,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Agent {
    pub name: String,
    pub state: AState,
    pub p: f32,
    pub step: String,
    pub rate: f32,
    /// The right-hand figure in the panel row (a percent in the demo, a step count for real agents).
    pub figure: Option<String>,
    pub chip: Option<&'static str>,
}
#[derive(Resource)]
pub struct Agents {
    pub list: Vec<Agent>,
    pub version: u32,
    acc: f32,
    seed: u32,
    /// The look test's scripted progress; real agents only change through `set`.
    demo: bool,
}
impl Agents {
    pub fn demo() -> Self {
        let a = |name: &str, state, p, step: &str, rate| Agent { name: name.to_string(), state, p, step: step.to_string(), rate, figure: None, chip: None };
        Self {
            list: vec![
                a("Fix flaky login test", AState::Working, 0.64, "Running tests · 41 / 64", 0.0042),
                a("Draft release notes", AState::Working, 0.28, "Reading 12 merged PRs", 0.0016),
                a("Reply to Tolu", AState::Needs, 1.0, "Draft ready for your review", 0.0),
                a("Bump Postgres to 16.4", AState::Done, 1.0, "Merged · #214", 0.0),
            ],
            version: 1,
            acc: 0.0,
            seed: 12345,
            demo: true,
        }
    }
    /// No agents yet: the panel and the ring deck start empty and fill from sphered.
    pub fn empty() -> Self {
        Self { list: Vec::new(), version: 1, acc: 0.0, seed: 12345, demo: false }
    }
    /// Replaces the list (only when it differs, so the panel rebuilds only on real changes).
    pub fn set(&mut self, list: Vec<Agent>) {
        if self.list != list {
            self.list = list;
            self.version += 1;
        }
    }
    pub fn working(&self) -> usize {
        self.list.iter().filter(|a| a.state == AState::Working).count()
    }
    pub fn waiting(&self) -> usize {
        self.list.iter().filter(|a| a.state == AState::Needs).count()
    }
    pub fn summary(&self) -> String {
        if self.waiting() > 0 {
            format!("{} working · {} needs you", self.working(), self.waiting())
        } else {
            format!("{} working · all clear", self.working())
        }
    }
    fn rnd(&mut self) -> f32 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        (self.seed % 10000) as f32 / 10000.0
    }
    /// The prototype ticks agent progress once a second.
    pub fn tick(&mut self, dt: f32) {
        if !self.demo {
            return;
        }
        self.acc += dt;
        while self.acc >= 1.0 {
            self.acc -= 1.0;
            let mut changed = false;
            for i in 0..self.list.len() {
                if self.list[i].state != AState::Working {
                    continue;
                }
                let r = self.rnd();
                let a = &mut self.list[i];
                a.p = (a.p + a.rate * (0.6 + r * 0.8)).min(1.0);
                if i == 0 {
                    a.step = format!("Running tests · {} / 64", ((a.p * 64.0).round() as i32).min(64));
                }
                if a.p >= 1.0 {
                    a.state = AState::Done;
                    a.step = if i == 0 { "Fixed · PR #219 opened".into() } else { "Drafted · ready to publish".into() };
                }
                changed = true;
            }
            if changed {
                self.version += 1;
            }
        }
    }
}

// ---------------------------------------------------------------- components

#[derive(Component)]
pub struct CoreRoot;
#[derive(Component)]
pub struct CoreTilt;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tag {
    None,
    Need,
    Spine,
    Spokes,
    SphLines,
    SphEdges,
    SphFill,
    Shell,
    Dots,
    /// the core-open track (fades with the orbit, no reveal)
    Orb,
}

#[derive(Component)]
pub struct GlowInfo {
    pub deck: usize,
    /// the colour role, resolved against the live theme (and the deck's own hue in Full color)
    pub role: Role,
    pub alpha: f32,
    pub wall: f32,
    pub tag: Tag,
}

#[derive(Component)]
pub struct Spin {
    pub speed: f32,
    pub rx: f32,
    pub rz: f32,
}

#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dyn {
    Track,
    Work,
    Need,
    Done,
    TimeElapsed,
    TimeEvents,
    TimeNow,
    Spokes,
}

/// Sphere sub-groups that spin each frame.
#[derive(Component)]
pub struct SphereSpin {
    pub kind: u8, // 0 sphereG, 1 ico
}

pub struct Deck {
    pub root: Entity,
    pub fold: Entity,
    pub zi: f32,
    pub reveal_at: f32,
    pub focus_fade: f32,
    pub hi: Spring,
    pub u_depth: f32,
    pub u_reveal: f32,
    pub u_hi: f32,
    pub u_fade: f32,
    /// Orbit fold (gyroscope) for the outer four rings
    pub f: Spring,
    pub fold_e: Option<Vec3>,
    pub prec: Vec3,
    pub prec_a: f32,
    /// collapse onto the sphere (whole stack, X)
    pub c: Spring,
    pub c_dir: f32,
    pub c_ax: f32,
    /// this ring alone folded onto a ghost circle (double-click)
    pub g: Spring,
    pub r_min: f32,
    pub r_max: f32,
    pub u_pinch: f32,
}

#[derive(Resource)]
pub struct Decks {
    pub list: Vec<Deck>,
    pub tilt: Entity,
    pub root: Entity,
    pub voice_lvl: f32,
}

// indexes
pub const TIME: usize = 0;
pub const LOAD: usize = 1;
pub const AGENTS: usize = 2;
pub const SIGNAL: usize = 3;
pub const VOICE: usize = 4;
pub const CORE: usize = 5;

const NSP: usize = 156;

#[derive(Resource)]
pub struct CoreAssets {
    #[allow(dead_code)]
    pub spokes_mesh: Handle<Mesh>,
    spokes: Vec<(f32, f32, f32)>, // angle, level, phase
    energy: f32,
}

// ---------------------------------------------------------------- build

pub struct B<'a, 'w, 's> {
    pub commands: &'a mut Commands<'w, 's>,
    pub meshes: &'a mut Assets<Mesh>,
    pub mats: &'a mut Assets<GlowMaterial>,
    pub images: &'a mut Assets<Image>,
}

fn glow_mat(kind: Kind, role: Role, alpha: f32) -> GlowMaterial {
    GlowMaterial::new(kind, role_lin(role), alpha)
}

#[allow(clippy::too_many_arguments)]
fn add_geo(
    b: &mut B,
    deck: usize,
    parent: Entity,
    geo: Geo,
    role: Role,
    alpha: f32,
    wall: f32,
    spin: f32,
    tag: Tag,
    dynk: Option<Dyn>,
) -> (Entity, Handle<Mesh>) {
    let mh = b.meshes.add(geo.into_mesh());
    let mut m = glow_mat(Kind::Ring, role, alpha);
    m.u.a.y = wall;
    let mut e = b.commands.spawn((
        Mesh3d(mh.clone()),
        MeshMaterial3d(b.mats.add(m)),
        Transform::default(),
        GlowInfo { deck, role, alpha, wall, tag },
    ));
    if spin != 0.0 {
        e.insert(Spin { speed: spin, rx: 0.0, rz: 0.0 });
    }
    if let Some(d) = dynk {
        e.insert(d);
    }
    let id = e.id();
    b.commands.entity(parent).add_child(id);
    (id, mh)
}

#[allow(clippy::too_many_arguments)]
fn quad_label(b: &mut B, deck: usize, parent: Entity, text: &str, size: f32, role: Role, alpha: f32, ang: f32, r: f32) {
    let l = raster_label(RAJDHANI_600, text, size, 0.16);
    let img = b.images.add(l.image);
    let (w, h) = (l.size.x / 2.0, l.size.y / 2.0);
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_POSITION,
        vec![[-w, -h, 0.0], [w, -h, 0.0], [w, h, 0.0], [-w, -h, 0.0], [w, h, 0.0], [-w, h, 0.0]],
    );
    mesh.insert_attribute(
        ATTR_HOLO,
        vec![
            [0.0, 1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0, 0.0],
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 0.0, 0.0, 0.0],
        ],
    );
    let mut m = glow_mat(Kind::Text, role, alpha);
    m.u.c.y = ang;
    m.tex = Some(img);
    let e = b
        .commands
        .spawn((
            Mesh3d(b.meshes.add(mesh)),
            MeshMaterial3d(b.mats.add(m)),
            Transform::from_xyz(ang.sin() * r, ang.cos() * r, 0.0),
            GlowInfo { deck, role, alpha, wall: 0.0, tag: Tag::None },
        ))
        .id();
    b.commands.entity(parent).add_child(e);
}

fn attr_mesh(topology: PrimitiveTopology, pos: Vec<[f32; 3]>, attr: Vec<[f32; 4]>) -> Mesh {
    let mut m = Mesh::new(topology, RenderAssetUsages::default());
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
    m.insert_attribute(ATTR_HOLO, attr);
    m
}

pub fn now_minutes() -> f32 {
    let s = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
    ((s % 86400.0) / 60.0) as f32
}

pub fn build_time_dyn(now: f32) -> (Geo, Geo, Geo) {
    let a = now / 1440.0 * TAU;
    let mut elapsed = Geo::new();
    ring_geo(&mut elapsed, 350.0, 1.3, RingOpt { a0: 0.0, a1: a.max(0.002), ..Default::default() });
    let first = ((now + 20.0) / 30.0).ceil() * 30.0;
    let su = if now < 600.0 { 600.0 } else { 2040.0 };
    let mut evs = vec![(first, first + 30.0), (first + 60.0, first + 150.0), (first + 180.0, first + 240.0), (first + 330.0, first + 420.0), (su, su + 15.0)];
    evs.retain(|e| e.0 - now < 1440.0);
    let mut events = Geo::new();
    for (s, e) in evs {
        if s >= 1440.0 {
            continue;
        }
        ring_geo(&mut events, 372.0, 4.4, RingOpt { a0: s / 1440.0 * TAU, a1: e.min(1440.0) / 1440.0 * TAU, walls: 2, ..Default::default() });
    }
    let mut nowg = Geo::new();
    let (s, c) = a.sin_cos();
    let (r0, r1, hw) = (337.0, 351.0, 5.5);
    let aa: C = [s * r0 - c * hw, c * r0 + s * hw, a];
    let bb: C = [s * r0 + c * hw, c * r0 - s * hw, a];
    let tt: C = [s * r1, c * r1, a];
    nowg.tri(aa, bb, tt, 1.0);
    nowg.wall(aa, bb, 1.0);
    nowg.wall(bb, tt, 1.0);
    nowg.wall(tt, aa, 1.0);
    (elapsed, events, nowg)
}

fn sector(i: usize) -> (f32, f32) {
    let c = i as f32 * 90.0 * DEG;
    let s = 80.0 * DEG;
    (c - s / 2.0, c + s / 2.0)
}

pub fn build_agents_dyn(list: &[Agent]) -> [Geo; 4] {
    let mut track = Geo::new();
    let mut work = Geo::new();
    let mut need = Geo::new();
    let mut done = Geo::new();
    for (i, a) in list.iter().enumerate() {
        let (a0, a1) = sector(i);
        ring_geo(&mut track, 247.0, 1.0, RingOpt { a0, a1, ..Default::default() });
        for t in [a0, a1] {
            let (s, c) = t.sin_cos();
            track.quad(
                [s * 240.0 - c * 0.6, c * 240.0 + s * 0.6, 0.0],
                [s * 240.0 + c * 0.6, c * 240.0 - s * 0.6, 0.0],
                [s * 254.0 + c * 0.6, c * 254.0 - s * 0.6, 0.0],
                [s * 254.0 - c * 0.6, c * 254.0 + s * 0.6, 0.0],
                1.0,
            );
            track.wall([s * 240.0, c * 240.0, 0.0], [s * 254.0, c * 254.0, 0.0], 1.0);
        }
        match a.state {
            AState::Working => ring_geo(&mut work, 247.0, 4.6, RingOpt { a0, a1: a0 + (a1 - a0) * clamp(a.p, 0.02, 1.0), walls: 2, ..Default::default() }),
            AState::Needs => ring_geo(&mut need, 247.0, 4.6, RingOpt { a0, a1, walls: 2, ..Default::default() }),
            AState::Done | AState::Failed => ring_geo(&mut done, 247.0, 2.4, RingOpt { a0, a1, ..Default::default() }),
        }
    }
    [track, work, need, done]
}

pub fn setup_core(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<GlowMaterial>>,
    mut images: ResMut<Assets<Image>>,
    agents: Res<Agents>,
) {
    let mut b = B { commands: &mut commands, meshes: &mut meshes, mats: &mut mats, images: &mut images };
    let root = b.commands.spawn((CoreRoot, Transform::from_xyz(0.0, CORE_Y, 0.0), Visibility::default())).id();
    let tilt = b.commands.spawn((CoreTilt, Transform::default(), Visibility::default())).id();
    b.commands.entity(root).add_child(tilt);

    let mk = |zi: f32, reveal_at: f32, focus_fade: f32| -> (f32, f32, f32) { (zi, reveal_at, focus_fade) };
    let defs = [mk(-3.0, 0.55, 0.0), mk(-2.0, 0.44, 0.0), mk(-1.0, 0.33, 0.6), mk(0.0, 0.22, 0.75), mk(1.0, 0.11, 0.8), mk(2.0, 0.0, 0.8)];
    // Orbit: spring, fold angles (Euler XYZ, deg) and precession (rad/s) per ring; radii per deck
    let orbit: [(f32, f32, Option<Vec3>, Vec3); 6] = [
        (36.0, 10.0, Some(Vec3::new(72.0, 0.0, 0.0)), Vec3::new(0.0, 0.11, 0.0)),
        (44.0, 11.0, Some(Vec3::new(0.0, 70.0, 0.0)), Vec3::new(0.14, 0.0, 0.0)),
        (52.0, 12.0, Some(Vec3::new(-44.0, 48.0, 0.0)), Vec3::new(0.0, -0.17, 0.0)),
        (60.0, 13.0, Some(Vec3::new(52.0, -40.0, 0.0)), Vec3::new(0.0, 0.23, 0.0)),
        (60.0, 13.0, None, Vec3::ZERO),
        (60.0, 13.0, None, Vec3::ZERO),
    ];
    let radii = [(336.0, 412.0), (270.0, 336.0), (210.0, 270.0), (176.0, 210.0), (118.0, 176.0), (0.0, 118.0)];
    let mut decks = Vec::new();
    for (i, (zi, reveal_at, focus_fade)) in defs.into_iter().enumerate() {
        let (fk, fc, fold_e, prec) = orbit[i];
        let droot = b.commands.spawn((Transform::default(), Visibility::default())).id();
        let fold = b.commands.spawn((Transform::default(), Visibility::default())).id();
        b.commands.entity(droot).add_child(fold);
        b.commands.entity(tilt).add_child(droot);
        decks.push(Deck {
            root: droot,
            fold,
            zi,
            reveal_at,
            focus_fade,
            hi: Spring::new(0.0, 170.0, 24.0),
            u_depth: 0.0,
            u_reveal: 0.0,
            u_hi: 0.0,
            u_fade: 1.0,
            f: Spring::new(0.0, fk, fc),
            fold_e: fold_e.map(|v| v * DEG),
            prec,
            prec_a: 0.0,
            c: Spring::new(0.0, 74.0, 2.0 * 0.86 * 74f32.sqrt()),
            c_dir: if i % 2 == 0 { 1.0 } else { -1.0 },
            c_ax: i as f32 * 72.0 * DEG,
            g: Spring::new(0.0, 80.0, 15.0),
            r_min: radii[i].0,
            r_max: radii[i].1,
            u_pinch: 0.0,
        });
    }
    let fold = |i: usize| decks[i].fold;

    // Layer 1 · today's 24-hour dial
    let mut g = Geo::new();
    ticks_geo(&mut g, 356.0, 96, 6.0, 1.1, 4, 14.0);
    add_geo(&mut b, TIME, fold(TIME), g, Role::Line, 0.55, 0.3, 0.0, Tag::None, None);
    let mut g = Geo::new();
    ring(&mut g, 363.0, 1.0);
    add_geo(&mut b, TIME, fold(TIME), g, Role::Line, 0.45, 0.3, 0.0, Tag::None, None);
    for h in (0..24).step_by(3) {
        let major = h % 6 == 0;
        let a = h as f32 / 24.0 * TAU;
        quad_label(
            &mut b,
            TIME,
            fold(TIME),
            &format!("{:02}", h),
            if major { 12.5 } else { 11.0 },
            if major { Role::Hot } else { Role::Line },
            if major { 0.95 } else { 0.6 },
            a,
            393.0,
        );
    }
    let (el, ev, nw) = build_time_dyn(now_minutes());
    add_geo(&mut b, TIME, fold(TIME), ev, Role::Fill, 0.9, 0.5, 0.0, Tag::None, Some(Dyn::TimeEvents));
    add_geo(&mut b, TIME, fold(TIME), el, Role::Hot, 0.6, 0.3, 0.0, Tag::None, Some(Dyn::TimeElapsed));
    add_geo(&mut b, TIME, fold(TIME), nw, Role::Fill, 1.0, 0.55, 0.0, Tag::None, Some(Dyn::TimeNow));

    // Layer 2 · structure and system load
    let mut g = Geo::new();
    ring(&mut g, 337.0, 1.0);
    add_geo(&mut b, LOAD, fold(LOAD), g, Role::Dim, 0.65, 0.3, 0.0, Tag::None, None);
    let mut g = Geo::new();
    arcs_geo(&mut g, 324.0, 3, 6.5, 44.0);
    add_geo(&mut b, LOAD, fold(LOAD), g, Role::Deep, 0.9, 0.4, -TAU / 210.0, Tag::None, None);
    let mut g = Geo::new();
    blocks_geo(&mut g, 307.0, 56, 9.3, 3.7, 3, 0.24);
    add_geo(&mut b, LOAD, fold(LOAD), g, Role::Line, 0.75, 0.5, TAU / 170.0, Tag::None, None);
    let mut g = Geo::new();
    dash_geo(&mut g, 294.0, 1.0, 2.0, 9.0);
    add_geo(&mut b, LOAD, fold(LOAD), g, Role::Dim, 0.75, 0.3, 0.0, Tag::None, None);
    let mut g = Geo::new();
    arrows_geo(&mut g, 279.0, 8, 9.3);
    add_geo(&mut b, LOAD, fold(LOAD), g, Role::Hot, 0.9, 0.5, -TAU / 120.0, Tag::None, None);

    // Layer 3 · agents (one 80 degree sector per agent)
    let mut g = Geo::new();
    ticks_geo(&mut g, 260.0, 180, 5.6, 1.0, 0, 0.0);
    add_geo(&mut b, AGENTS, fold(AGENTS), g, Role::Line, 0.4, 0.22, TAU / 95.0, Tag::None, None);
    let [tr, wk, nd, dn] = build_agents_dyn(&agents.list);
    add_geo(&mut b, AGENTS, fold(AGENTS), tr, Role::Dim, 0.7, 0.3, 0.0, Tag::None, Some(Dyn::Track));
    add_geo(&mut b, AGENTS, fold(AGENTS), wk, Role::Fill, 0.95, 0.5, 0.0, Tag::None, Some(Dyn::Work));
    add_geo(&mut b, AGENTS, fold(AGENTS), nd, Role::Acc, 1.0, 0.6, 0.0, Tag::Need, Some(Dyn::Need));
    add_geo(&mut b, AGENTS, fold(AGENTS), dn, Role::Ok, 0.5, 0.3, 0.0, Tag::None, Some(Dyn::Done));
    let mut g = Geo::new();
    blocks_geo(&mut g, 231.0, 40, 13.0, 5.6, 2, 0.2);
    add_geo(&mut b, AGENTS, fold(AGENTS), g, Role::Hot, 0.78, 0.45, TAU / 130.0, Tag::None, None);
    let mut g = Geo::new();
    ring(&mut g, 219.0, 1.0);
    add_geo(&mut b, AGENTS, fold(AGENTS), g, Role::Line, 0.6, 0.3, 0.0, Tag::None, None);

    // Layer 4 · signal
    let mut g = Geo::new();
    ticks_geo(&mut g, 199.0, 72, 10.0, 1.0, 6, 20.0);
    add_geo(&mut b, SIGNAL, fold(SIGNAL), g, Role::Line, 0.55, 0.28, -TAU / 88.0, Tag::None, None);
    let mut g = Geo::new();
    arcs_geo(&mut g, 186.0, 2, 2.3, 70.0);
    add_geo(&mut b, SIGNAL, fold(SIGNAL), g, Role::Line, 0.9, 0.35, TAU / 44.0, Tag::None, None);

    // Layer 5 · voice: rings plus a burst of spokes driven by the voice level
    let mut g = Geo::new();
    ring(&mut g, 140.0, 1.3);
    add_geo(&mut b, VOICE, fold(VOICE), g, Role::Hot, 0.7, 0.3, 0.0, Tag::None, None);
    let mut g = Geo::new();
    ring(&mut g, 132.0, 1.0);
    add_geo(&mut b, VOICE, fold(VOICE), g, Role::Line, 0.5, 0.3, 0.0, Tag::None, None);
    let mut g = Geo::new();
    ticks_geo(&mut g, 125.0, 60, 4.6, 1.0, 0, 0.0);
    add_geo(&mut b, VOICE, fold(VOICE), g, Role::Line, 0.5, 0.3, -TAU / 30.0, Tag::None, None);
    let mut sp = Geo::new();
    write_spokes_geo(&mut sp, &vec![(0.0, 0.3, 0.0); NSP], 145.0, 112.0);
    let spokes: Vec<(f32, f32, f32)> = (0..NSP).map(|i| (i as f32 / NSP as f32 * TAU, 0.0, i as f32 * 0.37)).collect();
    let (_, spokes_mesh) = add_geo(&mut b, VOICE, fold(VOICE), sp, Role::Fill, 0.9, 0.5, 0.0, Tag::Spokes, Some(Dyn::Spokes));

    // Layer 6 · the glass sphere itself
    build_sphere(&mut b, fold(CORE));
    // a dashed equator ring, tilted, so the sphere reads as 3D even face-on
    let mut g = Geo::new();
    dash_geo(&mut g, 122.0, 1.2, 6.0, 9.0);
    let (eq, _) = add_geo(&mut b, CORE, fold(CORE), g, Role::Line, 0.7, 0.3, 0.0, Tag::None, None);
    b.commands.entity(eq).insert(Spin { speed: 0.12, rx: 74.0 * DEG, rz: 0.0 });

    // depth spine
    let mut pos = Vec::new();
    let mut at = Vec::new();
    let mut z = -330.0;
    while z < 250.0 {
        pos.push([0.0, 0.0, z]);
        pos.push([0.0, 0.0, z + 6.0]);
        at.push([0.0, 0.0, 0.0, 1.0]);
        at.push([0.0, 0.0, 0.0, 1.0]);
        z += 14.0;
    }
    let spine = b.meshes.add(attr_mesh(PrimitiveTopology::LineList, pos, at));
    let mut m = glow_mat(Kind::Ring, Role::Line, 0.0);
    m.u.a.y = 0.0;
    let se = b
        .commands
        .spawn((Mesh3d(spine), MeshMaterial3d(b.mats.add(m)), Transform::default(), GlowInfo { deck: CORE, role: Role::Line, alpha: 0.0, wall: 0.0, tag: Tag::Spine }))
        .id();
    b.commands.entity(tilt).add_child(se);

    b.commands.entity(root).insert(Visibility::default());
    b.commands.insert_resource(Decks { list: decks, tilt, root, voice_lvl: 0.0 });
    b.commands.insert_resource(CoreAssets { spokes_mesh, spokes, energy: 0.0 });
}

fn build_sphere(b: &mut B, fold: Entity) {
    let r = 104.0f32;
    let sphere_g = b.commands.spawn((Transform::from_rotation(Quat::from_rotation_z(14.0 * DEG)), Visibility::default(), SphereSpin { kind: 0 })).id();
    b.commands.entity(fold).add_child(sphere_g);
    // wireframe: 12 meridians + 7 latitudes
    let mut pos: Vec<[f32; 3]> = Vec::new();
    let mut at: Vec<[f32; 4]> = Vec::new();
    let seg = 96;
    for m in 0..12 {
        let phi = m as f32 / 12.0 * std::f32::consts::PI;
        let kk = if m % 3 == 0 { 1.0 } else { 0.5 };
        for i in 0..seg {
            for a in [i as f32 / seg as f32 * TAU, (i + 1) as f32 / seg as f32 * TAU] {
                pos.push([a.cos() * phi.cos() * r, a.sin() * r, a.cos() * phi.sin() * r]);
                at.push([kk, 0.0, 0.0, 0.0]);
            }
        }
    }
    for j in 1..8 {
        let lat = -std::f32::consts::FRAC_PI_2 + j as f32 / 8.0 * std::f32::consts::PI;
        let (y, rr) = (lat.sin() * r, lat.cos() * r);
        let kk = if j == 4 { 1.0 } else { 0.45 };
        for i in 0..seg {
            for a in [i as f32 / seg as f32 * TAU, (i + 1) as f32 / seg as f32 * TAU] {
                pos.push([a.cos() * rr, y, a.sin() * rr]);
                at.push([kk, 0.0, 0.0, 0.0]);
            }
        }
    }
    let lines = b.meshes.add(attr_mesh(PrimitiveTopology::LineList, pos, at));
    spawn_sph(b, sphere_g, lines, Kind::Line, Role::Line, 0.62, Tag::SphLines);
    // dots
    let mut dpos = Vec::new();
    let mut dat = Vec::new();
    let mut seed: u64 = 7;
    let mut rnd = || {
        seed = (seed * 16807) % 2147483647;
        seed as f32 / 2147483647.0
    };
    for _ in 0..70 {
        let u = rnd() * 2.0 - 1.0;
        let th = rnd() * TAU;
        let rr = (1.0 - u * u).sqrt() * (r + 0.5);
        let p = [th.cos() * rr, u * (r + 0.5), th.sin() * rr];
        let k = 0.4 + rnd() * 0.6;
        for c in [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]] {
            dpos.push(p);
            dat.push([c[0], c[1], k, 0.0]);
        }
    }
    let dots = b.meshes.add(attr_mesh(PrimitiveTopology::TriangleList, dpos, dat));
    spawn_sph(b, sphere_g, dots, Kind::Dots, Role::Fill, 0.95, Tag::Dots);
    // fresnel shell
    let shell = Sphere::new(r + 1.0).mesh().uv(64, 40);
    let n = shell.count_vertices();
    let mut shell = shell;
    shell.insert_attribute(ATTR_HOLO, vec![[0.0f32; 4]; n]);
    let h = b.meshes.add(shell);
    spawn_sph(b, fold, h, Kind::Fres, Role::Line, 0.3, Tag::Shell);
    // icosahedron: edges + very faint faces
    let t = (1.0 + 5.0f32.sqrt()) / 2.0;
    let v: Vec<Vec3> = [
        [-1.0, t, 0.0], [1.0, t, 0.0], [-1.0, -t, 0.0], [1.0, -t, 0.0], [0.0, -1.0, t], [0.0, 1.0, t],
        [0.0, -1.0, -t], [0.0, 1.0, -t], [t, 0.0, -1.0], [t, 0.0, 1.0], [-t, 0.0, -1.0], [-t, 0.0, 1.0],
    ]
    .iter()
    .map(|p| Vec3::from(*p).normalize() * 40.0)
    .collect();
    let faces: [[usize; 3]; 20] = [
        [0, 11, 5], [0, 5, 1], [0, 1, 7], [0, 7, 10], [0, 10, 11], [1, 5, 9], [5, 11, 4], [11, 10, 2], [10, 7, 6], [7, 1, 8],
        [3, 9, 4], [3, 4, 2], [3, 2, 6], [3, 6, 8], [3, 8, 9], [4, 9, 5], [2, 4, 11], [6, 2, 10], [8, 6, 7], [9, 8, 1],
    ];
    let ico = b.commands.spawn((Transform::default(), Visibility::default(), SphereSpin { kind: 1 })).id();
    b.commands.entity(fold).add_child(ico);
    let mut epos = Vec::new();
    let mut fpos = Vec::new();
    for f in faces {
        for i in 0..3 {
            let (a, c) = (v[f[i]], v[f[(i + 1) % 3]]);
            fpos.push(a.to_array());
            // each shared edge drawn once
            if f[i] < f[(i + 1) % 3] {
                epos.push(a.to_array());
                epos.push(c.to_array());
            }
        }
    }
    // an edge is shared by two faces; the lower->higher test keeps one traversal per face pair direction
    let eat = vec![[1.0, 0.0, 0.0, 0.0]; epos.len()];
    let fat = vec![[1.0, 0.0, 0.0, 0.0]; fpos.len()];
    let em = b.meshes.add(attr_mesh(PrimitiveTopology::LineList, epos, eat));
    spawn_sph(b, ico, em, Kind::Line, Role::Hot, 0.6, Tag::SphEdges);
    let fm = b.meshes.add(attr_mesh(PrimitiveTopology::TriangleList, fpos, fat));
    spawn_sph(b, ico, fm, Kind::Line, Role::Fill, 0.045, Tag::SphFill);
}

fn spawn_sph(b: &mut B, parent: Entity, mesh: Handle<Mesh>, kind: Kind, role: Role, alpha: f32, tag: Tag) {
    let mut m = glow_mat(kind, role, alpha);
    if kind == Kind::Dots {
        m.u.c.z = 3.0;
    }
    let e = b.commands.spawn((Mesh3d(mesh), MeshMaterial3d(b.mats.add(m)), Transform::default(), GlowInfo { deck: CORE, role, alpha, wall: 0.0, tag })).id();
    b.commands.entity(parent).add_child(e);
}

/// Voice spokes: 12 vertices each (a flat ribbon plus its wall), levels drive their length.
fn write_spokes_geo(g: &mut Geo, spokes: &[(f32, f32, f32)], rw: f32, wl: f32) {
    for (a, v, _) in spokes {
        let (s, c) = a.sin_cos();
        let hw = 1.1;
        let (r0, r1) = (rw, rw + 4.0 + v * wl);
        let k = 0.45 + v * 0.55;
        let aa: C = [s * r0 - c * hw, c * r0 + s * hw, *a];
        let bb: C = [s * r0 + c * hw, c * r0 - s * hw, *a];
        let cc: C = [s * r1 + c * hw, c * r1 - s * hw, *a];
        let dd: C = [s * r1 - c * hw, c * r1 + s * hw, *a];
        g.quad(aa, bb, cc, dd, k);
        g.wall([s * r0, c * r0, *a], [s * r1, c * r1, *a], k);
    }
}

// ---------------------------------------------------------------- per-frame update

#[allow(clippy::too_many_arguments)]
pub fn update_core(
    sim: Res<Sim>,
    mut ctl: ResMut<CoreCtl>,
    mut decks: ResMut<Decks>,
    mut assets: ResMut<CoreAssets>,
    mut agents: ResMut<Agents>,
    mut tfs: Query<&mut Transform>,
    mut meshes: ResMut<Assets<Mesh>>,
    dyns: Query<(&Dyn, &Mesh3d)>,
    mut last_ver: Local<u32>,
    mut last_min: Local<f32>,
    mut last_agents: Local<(u32, f32)>,
) {
    let dt = sim.dt;
    let t = sim.t;
    let boot = sim.boot_t;
    // core orientation
    // core open: tipped back 62° like a holo-table
    let mut tgt = if ctl.orb > 0.001 { ctl.q_target.slerp(Quat::from_rotation_x(-62.0 * DEG), ctl.orb) } else { ctl.q_target };
    // the Working view turns the core toward the work (yaw 24°, pitch -4°)
    if ctl.work > 0.001 {
        tgt = tgt.slerp(Quat::from_euler(EulerRot::YXZ, 24.0 * DEG, -4.0 * DEG, 0.0), ctl.work);
    }
    // where the core sits: up a little when open, aside and smaller in the Working view
    if let Ok(mut tr) = tfs.get_mut(decks.root) {
        let w = ctl.work;
        tr.translation = Vec3::new(-260.0 * w, CORE_Y - 26.0 * ctl.orb - 192.0 * w, 0.0);
        tr.scale = Vec3::splat(lerp(1.0, 0.52, w));
    }
    let k = 1.0 - (-3.2 * dt).exp();
    if ctl.auto_orient {
        ctl.q_user = ctl.q_user.slerp(tgt, k);
    } else if ctl.omega.length() > 1e-4 {
        // coasting after a drag: premultiplied about world Y, then X
        let om = ctl.omega;
        ctl.q_user = (Quat::from_rotation_x(om.y * dt) * Quat::from_rotation_y(om.x * dt) * ctl.q_user).normalize();
        ctl.omega *= (-2.4 * dt).exp();
    }
    ctl.flash *= (-3.2 * dt).exp();
    ctl.collapse_t += dt;
    let (px, py) = ctl.pointer.map(|p| (p.x - 0.5, p.y - 0.5)).unwrap_or((0.0, 0.0));
    let qp = Quat::from_euler(EulerRot::YXZ, px * 7.0 * DEG, py * 5.0 * DEG, 0.0);
    let kp = 1.0 - (-3.0 * dt).exp();
    ctl.q_par = ctl.q_par.slerp(qp, kp);
    let q = (ctl.q_par * ctl.q_user).normalize();
    if let Ok(mut tr) = tfs.get_mut(decks.tilt) {
        tr.rotation = q;
    }
    let nz = (q * Vec3::Z).z;
    let tilt_k = clamp(clamp(nz, -1.0, 1.0).acos() / (55.0 * DEG), 0.0, 1.0);
    match ctl.mode {
        Mode::Depth => {
            ctl.spacing.t = 92.0;
            ctl.depth.t = 40.0;
        }
        Mode::Flat => {
            ctl.spacing.t = 8.0 + 72.0 * tilt_k;
            ctl.depth.t = 3.0 + 40.0 * tilt_k;
        }
        Mode::Orbit => {
            ctl.spacing.t = 0.0;
            ctl.depth.t = 16.0;
        }
    }
    // core open: a low dome with short walls
    let o = ctl.orb;
    ctl.spacing.t = lerp(ctl.spacing.t, 30.0, o);
    ctl.depth.t = lerp(ctl.depth.t, 12.0, o);
    ctl.spacing.step(dt);
    ctl.depth.step(dt);
    ctl.zoom.step(dt);
    let spin_target = ctl.work_spin;
    ctl.spin_mul = lerp(ctl.spin_mul, spin_target, 1.0 - (-3.0 * dt).exp());
    let spin_mul = ctl.spin_mul;
    ctl.depth_amt = if ctl.mode == Mode::Depth { clamp((ctl.spacing.x - 10.0) / 80.0, 0.0, 1.0) } else { 0.0 };

    // spokes
    let a = &mut *assets;
    let idle_energy = 1.0;
    a.energy += (idle_energy - a.energy) * (dt * 4.0).min(1.0);
    let mut sum = 0.0;
    for (i, s) in a.spokes.iter_mut().enumerate() {
        s.2 += dt * (1.1 + (i % 9) as f32 * 0.14);
        let amp = 0.22 + ((s.2 * 0.7 + i as f32 * 0.28).sin() * 0.5 + 0.5) * 0.3;
        s.1 += (amp * a.energy - s.1) * (dt * 15.0).min(1.0);
        sum += s.1;
    }
    decks.voice_lvl = sum / NSP as f32;
    let voice_lvl = decks.voice_lvl;

    // decks
    let orbit = ctl.mode == Mode::Orbit;
    // collapse stagger: outermost first when folding in, innermost first when expanding
    const STAGGER: [f32; 5] = [0.0, 0.07, 0.155, 0.235, 0.32];
    let (collapsed, ct, flash, spacing) = (ctl.collapsed, ctl.collapse_t, ctl.flash, ctl.spacing.x);
    let mut orbit_scale = 0.0;
    let mut all_landed = true;
    for (i, d) in decks.list.iter_mut().enumerate() {
        d.hi.t = 0.0;
        d.hi.step(dt);
        // Orbit: the outer rings fold around the core and precess
        d.f.t = if orbit && d.fold_e.is_some() { 1.0 } else { 0.0 };
        d.f.step(dt);
        d.prec_a += dt * d.f.x;
        let qf = match d.fold_e {
            Some(e) => {
                let p = d.prec * d.prec_a;
                let full = Quat::from_euler(EulerRot::XYZ, p.x, p.y, p.z) * Quat::from_euler(EulerRot::XYZ, e.x, e.y, e.z);
                Quat::IDENTITY.slerp(full, clamp(d.f.x, 0.0, 1.0))
            }
            None => Quat::IDENTITY,
        };
        if i == TIME {
            orbit_scale = clamp(d.f.x, 0.0, 1.0);
        }
        // collapse: rings 0..5 (not the core) fold onto the sphere
        let mut c = 0.0;
        if i < CORE {
            let order = if collapsed { i } else { 4 - i };
            if ct >= STAGGER[order] {
                let tgt = if collapsed { 1.0 } else { 0.0 };
                if d.c.t != tgt {
                    d.c.t = tgt;
                    // in: k 74, damping .86; out: k 62, damping .64
                    let (k, z) = if collapsed { (74.0, 0.86) } else { (62.0, 0.64) };
                    d.c.k = k;
                    d.c.c = 2.0 * z * f32::sqrt(k);
                }
            }
            d.c.step(dt);
            c = d.c.x.max(0.0);
            if collapsed && c < 0.9 {
                all_landed = false;
            }
        }
        let rg = (d.r_min + d.r_max) * 0.5;
        let land = 0.9 * 104.0;
        let scale = if i < CORE { lerp(1.0, land / rg.max(1.0), c) } else { 1.0 };
        let ax = d.c_ax + d.c_dir * c * 40.0 * DEG;
        let rot = Quat::from_axis_angle(Vec3::new(ax.cos(), ax.sin(), 0.0), d.c_dir * c * 80.0 * DEG);
        if let Ok(mut tr) = tfs.get_mut(d.fold) {
            tr.rotation = rot * qf;
            tr.scale = Vec3::splat(scale);
        }
        if let Ok(mut tr) = tfs.get_mut(d.root) {
            let z = d.zi * spacing + d.hi.x * 14.0;
            tr.translation.z = lerp(z, 2.0 * spacing, clamp(c, 0.0, 1.0));
        }
        // a single ring folded onto its own radius
        d.g.step(dt);
        d.u_pinch = clamp(d.g.x, 0.0, 1.0);
        let smooth = |a: f32, b: f32, x: f32| {
            let t = clamp((x - a) / (b - a), 0.0, 1.0);
            t * t * (3.0 - 2.0 * t)
        };
        d.u_depth = (ctl.depth.x + d.hi.x * 8.0 + if i == VOICE { voice_lvl * 30.0 } else { 0.0 }).max(0.0);
        d.u_hi = d.hi.x * 0.9 + flash;
        d.u_fade = (1.0 - ctl.focus_amt * d.focus_fade) * (1.0 - if i == CORE { 0.1 } else { 0.38 } * ctl.orb) * (1.0 - smooth(0.3, 0.93, c)) * (1.0 - 0.35 * clamp(c, 0.0, 1.0)) * (1.0 - smooth(0.06, 0.7, d.u_pinch));
        d.u_reveal = out_cubic((boot - 0.15 - d.reveal_at) / 0.9);
    }
    // the sphere grows a little in Orbit, and breathes while the stack is collapsed
    let breathe = if collapsed { 1.0 + 0.03 * (1.25 * t).sin() } else { 1.0 };
    if let Ok(mut tr) = tfs.get_mut(decks.list[CORE].fold) {
        tr.scale = Vec3::splat((1.0 + 0.14 * orbit_scale) * breathe);
    }
    if collapsed && all_landed && !ctl.landed {
        ctl.landed = true;
        ctl.flash = 0.8;
    }
    if !collapsed {
        ctl.landed = false;
    }

    // dynamic geometry
    let now = now_minutes();
    let rebuild_time = (now - *last_min).abs() > 0.2;
    let rebuild_agents = agents.version != *last_ver;
    agents.tick(dt);
    // spokes mesh is rebuilt every frame
    let mut sg = Geo::new();
    write_spokes_geo(&mut sg, &a.spokes, 145.0 + ctl.focus_amt * 165.0, 112.0 * (1.0 - ctl.focus_amt * 0.35));
    let (g_spokes, g_time, g_ag) = (sg, if rebuild_time { Some(build_time_dyn(now)) } else { None }, if rebuild_agents { Some(build_agents_dyn(&agents.list)) } else { None });
    let mut g_time = g_time;
    let mut g_ag = g_ag.map(|a| a.into_iter().map(Some).collect::<Vec<_>>());
    let mut g_spokes = Some(g_spokes);
    for (d, mh) in &dyns {
        let geo = match d {
            Dyn::Spokes => g_spokes.take(),
            Dyn::TimeElapsed => g_time.as_mut().map(|g| std::mem::take(&mut g.0)),
            Dyn::TimeEvents => g_time.as_mut().map(|g| std::mem::take(&mut g.1)),
            Dyn::TimeNow => g_time.as_mut().map(|g| std::mem::take(&mut g.2)),
            Dyn::Track => g_ag.as_mut().and_then(|g| g[0].take()),
            Dyn::Work => g_ag.as_mut().and_then(|g| g[1].take()),
            Dyn::Need => g_ag.as_mut().and_then(|g| g[2].take()),
            Dyn::Done => g_ag.as_mut().and_then(|g| g[3].take()),
        };
        if let Some(g) = geo {
            if let Some(mut m) = meshes.get_mut(&mh.0) {
                *m = g.into_mesh();
            }
        }
    }
    if rebuild_time {
        *last_min = now;
    }
    if rebuild_agents {
        *last_ver = agents.version;
    }
    let _ = &mut *last_agents;

    // spinners and sphere parts
    let sp: Vec<(Entity, f32, f32)> = vec![];
    let _ = sp;
    let _ = t;
    let _ = spin_mul;
}

pub fn spin_system(sim: Res<Sim>, ctl: Res<CoreCtl>, mut q: Query<(&mut Transform, &mut Spin)>, mut sph: Query<(&mut Transform, &SphereSpin), Without<Spin>>, mut acc: Local<(f32, f32, f32)>) {
    let dt = sim.dt * ctl.spin_mul;
    for (mut tr, mut s) in &mut q {
        s.rz += s.speed * dt;
        tr.rotation = Quat::from_euler(EulerRot::XYZ, s.rx, 0.0, s.rz);
    }
    acc.0 += 0.22 * dt; // sphereG y
    acc.1 -= 0.5 * dt; // ico y
    acc.2 += 0.21 * dt; // ico x
    for (mut tr, s) in &mut sph {
        if s.kind == 0 {
            tr.rotation = Quat::from_euler(EulerRot::XYZ, 0.0, acc.0, 14.0 * DEG);
        } else {
            tr.rotation = Quat::from_euler(EulerRot::XYZ, acc.2, acc.1, 0.0);
        }
    }
}

pub fn update_materials(
    sim: Res<Sim>,
    ctl: Res<CoreCtl>,
    decks: Res<Decks>,
    win: Query<&Window, With<bevy::window::PrimaryWindow>>,
    mut mats: ResMut<Assets<GlowMaterial>>,
    q: Query<(&MeshMaterial3d<GlowMaterial>, &GlowInfo)>,
) {
    let pr = win.single().map(|w| w.scale_factor()).unwrap_or(1.0);
    for (h, info) in &q {
        // `get_mut` marks the asset changed and makes the renderer prepare it again, so only take it when a value
        // actually differs.
        let Some(cur) = mats.get(&h.0) else { continue };
        let d = &decks.list[info.deck];
        let mut alpha = info.alpha;
        match info.tag {
            Tag::Need => alpha = 0.62 + 0.38 * (sim.t * 4.2).sin(),
            Tag::Spine => alpha = ctl.depth_amt * 0.55,
            _ => {}
        }
        let mut u = cur.u;
        u.color = crate::theme::deck_lin(info.deck, info.role);
        u.c2 = crate::theme::deck_b2_lin(info.deck);
        u.a = Vec4::new(alpha, info.wall, d.u_reveal, d.u_hi);
        u.b = Vec4::new(d.u_fade, d.u_depth, 0.0, d.u_pinch);
        if cur.kind == Kind::Ring && info.deck != CORE {
            u.c.x = (d.r_min + d.r_max) * 0.5;
        }
        if info.tag == Tag::Dots {
            u.c.z = 3.0 * pr;
        }
        if info.tag == Tag::Shell {
            u.d = Vec4::new(sim.t, 0.0, 0.0, 0.0);
        }
        if info.tag == Tag::Orb {
            let o = ctl.orb;
            let rev = clamp(o / 0.85, 0.0, 1.0);
            u.a = Vec4::new(info.alpha * o, info.wall, rev * rev * (3.0 - 2.0 * rev), ctl.flash * 0.6);
            u.b = Vec4::new(1.0, 14.0, 0.0, 0.0);
            u.c.x = 0.0;
        }
        if u != cur.u {
            if let Some(mut m) = mats.get_mut(&h.0) {
                m.u = u;
            }
        }
    }
}
