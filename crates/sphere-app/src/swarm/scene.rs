//! The swarm's 3D scene (SPEC section 2, 3, 4.1, 5): the repo rings and the ready ring, the satellites, the step
//! tracks and the conflict arcs. Rings hang under one root that follows the core; satellites, tracks, marks and
//! arcs live in world space (a node can fly toward the camera). Everything draws on the core's layer (bloom).
mod ghosts;
mod mesh;

use super::model::State;
use super::{Mode, Swarm};
use crate::core::{CoreCtl, Decks, Sim, StageView};
use crate::glow::{GlowMaterial, GlowSlots, Kind};
use crate::motion::{self, Pace};
use crate::util::*;
use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::NoFrustumCulling;
use bevy::prelude::*;

const NSAT: usize = 9;
const NRING: usize = 4;
const NARC: usize = 8;
/// the world radius of a node's halo (the prototype's 78 px point at the default distance)
const HALO_R: f32 = 42.0;

type H = Handle<GlowMaterial>;

/// One orbit: radius, tilt about x and y, angular speed of its agents (rad/s).
#[derive(Clone, Copy)]
pub struct RingDef {
    pub rad: f32,
    pub tx: f32,
    pub ty: f32,
    pub w: f32,
}

impl RingDef {
    pub fn q(&self) -> Quat {
        Quat::from_euler(EulerRot::XYZ, self.tx, self.ty, 0.0)
    }
}

/// SPEC 2.2 REPOS (a 4th ring for a 4th repo; more repos share the rings round).
pub const REPOS: [RingDef; NRING] = [
    RingDef { rad: 292.0, tx: 0.0, ty: 0.0, w: 0.074 },
    RingDef { rad: 372.0, tx: 8.0 * DEG, ty: 7.0 * DEG, w: -0.061 },
    RingDef { rad: 452.0, tx: -7.0 * DEG, ty: -9.0 * DEG, w: 0.052 },
    RingDef { rad: 500.0, tx: 5.0 * DEG, ty: -4.0 * DEG, w: -0.044 },
];
/// The shared "Ready to merge" ring.
pub const READY: RingDef = RingDef { rad: 540.0, tx: 4.0 * DEG, ty: -5.0 * DEG, w: 0.027 };

/// What the HUD needs from the scene each frame (world positions; project them with the core camera).
#[derive(Resource, Default)]
pub struct SceneOut {
    /// every satellite on screen: (agent id, world position, node scale, fade 0..1 (incl. the view's own), side hint
    /// +1 label to the right of the node / -1 to the left, with the SPEC's 80 px hysteresis)
    pub nodes: Vec<(String, Vec3, f32, f32, f32)>,
    /// each conflict's arc crest (the "BOTH EDITING" chip's spot, already 20 up and 2 forward): (a id, b id, pos)
    pub crests: Vec<(String, String, Vec3)>,
    /// per repo ring (as many as are shown): where its label goes (ring angle 100 degrees, 14 up)
    pub ring_label_pos: Vec<Vec3>,
    /// the READY ring's label (ring angle 78 degrees, 14 up)
    pub ready_label_pos: Vec3,
    /// per agent: where each of its last steps landed, index i = `steps[i]`: (world pos on the track, alpha 0..1;
    /// 0 when that step is farther behind than the track reaches)
    pub marks: Vec<(String, Vec<(Vec3, f32)>)>,
    /// the swarm's frame this frame (the rings' centre, rotation and scale), for the ghosts of sessions elsewhere
    pub sw: Option<(Vec3, Quat, f32)>,
}

// ---------------------------------------------------------------- pure math

/// A point on ring `d` at ring angle `a` (ccw from its +x) and `dz` out of its plane, in the swarm's own space.
pub fn ring_local(d: &RingDef, a: f32, dz: f32) -> Vec3 {
    d.q() * Vec3::new(a.cos() * d.rad, a.sin() * d.rad, dz)
}

/// Shortest angular distance between two angles.
pub fn ang_dist(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(TAU);
    d.min(TAU - d)
}

/// The candidate angle farthest from every `taken` one (ties: the first candidate).
fn farthest(cands: impl Iterator<Item = f32>, taken: &[f32]) -> f32 {
    let mut best = (f32::MIN, 0.0);
    for c in cands {
        let m = taken.iter().map(|t| ang_dist(c, *t)).fold(f32::MAX, f32::min);
        if m > best.0 + 1e-4 {
            best = (m, c);
        }
    }
    best.1
}

/// SPEC 3.3 readySlot: candidates `k * TAU / 12 + .4`, the farthest from the nodes already there; the first is 160 deg.
pub fn ready_slot(taken: &[f32]) -> f32 {
    if taken.is_empty() {
        return 160.0 * DEG;
    }
    farthest((0..12).map(|k| k as f32 * TAU / 12.0 + 0.4), taken)
}

fn hash(s: &str, salt: u64) -> f32 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ salt;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    ((h >> 11) as f64 / (1u64 << 53) as f64) as f32
}

/// SPEC 9's starting angle (degrees) for the prototype's own agents.
fn demo_angle(id: &str) -> Option<f32> {
    Some(match id {
        "rate" => 22.0,
        "redis" => 205.0,
        "checkout" => 312.0,
        "react" => 118.0,
        "json" => 150.0,
        "cold" => 64.0,
        _ => return None,
    } * DEG)
}

/// Where a new agent starts on its ring: the prototype's angle for its own agents, else the id's hash picks a
/// base and the 12 positions round it are tried for the one farthest from the agents already on the ring.
pub fn pick_angle(id: &str, taken: &[f32]) -> f32 {
    if let Some(a) = demo_angle(id) {
        return a;
    }
    let base = hash(id, 0) * TAU;
    if taken.is_empty() {
        return base;
    }
    farthest((0..12).map(|k| base + k as f32 * TAU / 12.0), taken)
}

/// The conflict arc's control point (SPEC 5): the midpoint pushed away from the core and toward the viewer.
pub fn arc_ctrl(a: Vec3, b: Vec3, core: Vec3, age: f32) -> Vec3 {
    let mid = (a + b) / 2.0;
    let mut d = mid - core;
    d.z = 0.0;
    let dir = if d.length_squared() < 1.0 { Vec3::Y } else { d.normalize() };
    let bulge = 38.0 + a.distance(b) * 0.16;
    let mut m = mid + dir * (bulge * 2.0 * age);
    m.z += 44.0 * age;
    m
}

pub fn bez(a: Vec3, m: Vec3, b: Vec3, t: f32) -> Vec3 {
    let u = 1.0 - t;
    a * (u * u) + m * (2.0 * u * t) + b * (t * t)
}

/// SPEC 4.3: the front-left spot of a focused node: the window point `(max(96, W*.10), max(safe + 74, H*.17))`
/// projected onto the plane z = 235 from the camera (compact: centred). Falls back to (-600, 250, 235).
pub fn focus_target(cam: Vec3, w: f32, h: f32) -> Vec3 {
    let (sx, sy) = if w < 820.0 || w / h < 1.05 { (w * 0.5, 124.0f32.max(h * 0.2)) } else { (96.0f32.max(w * 0.10), 142.0f32.max(h * 0.17)) };
    let th = (20.0 * DEG).tan();
    let (nx, ny) = (sx / w * 2.0 - 1.0, 1.0 - sy / h * 2.0);
    let t = cam.z - 235.0;
    if !(w > 1.0 && h > 1.0) || t < 1.0 {
        return Vec3::new(-600.0, 250.0, 235.0);
    }
    Vec3::new(cam.x + nx * th * (w / h) * t, cam.y + ny * th * t, 235.0)
}

fn smooth(a: f32, b: f32, x: f32) -> f32 {
    let t = clamp((x - a) / (b - a), 0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

// ---------------------------------------------------------------- scene state

struct RingGfx {
    group: Entity,
    thin: H,
    band: H,
    ticks: H,
    arcs: H,
    ticks_e: Entity,
    arcs_e: Entity,
    ticks_a: f32,
    arcs_a: f32,
    show: f32,
    hi: f32,
}

struct ReadyGfx {
    group: Entity,
    dash: H,
    ring: H,
    ticks: H,
    ticks_e: Entity,
    ticks_a: f32,
    fade: f32,
}

struct SatGfx {
    root: Entity,
    shell: H,
    ico: Entity,
    edges: H,
    fill: H,
    gy1: Entity,
    gy1_m: H,
    gy2: Entity,
    gy2_m: H,
    track: H,
    prog: H,
    pulse: Entity,
    pulse_m: H,
    hov: Entity,
    hov_m: H,
    halo: H,
    trail: Entity,
    trail_mesh: Handle<Mesh>,
    trail_m: H,
    /// the ring (and its direction) the trail mesh was built for
    built: Option<(usize, f32)>,
    marks: Entity,
    marks_mesh: Handle<Mesh>,
    marks_m: H,
    /// the discs the marks mesh was built from
    marks_built: Vec<(Vec3, f32, f32)>,
    /// a helper's tether to its parent
    tether: Entity,
    tether_mesh: Handle<Mesh>,
    tether_m: H,
    tether_built: Option<(Vec3, Vec3, f32, f32)>,
}

struct ArcGfx {
    e: Entity,
    mesh: Handle<Mesh>,
    mat: H,
    /// what the ribbon was built from: its ends and bend, its width, its pulse and its age
    built: Option<(Vec3, Vec3, Vec3, f32, f32, f32)>,
}

/// One agent's satellite: the prototype's per-agent state (angle, springs, colour).
struct Slot {
    id: String,
    alive: bool,
    gone: bool,
    ring: usize,
    state: State,
    prev: Option<State>,
    p: f32,
    ang: f32,
    ang_r: f32,
    phase: f32,
    fly: Spring,
    foc: Spring,
    mg: Spring,
    rb: Spring,
    q: Spring,
    vis: Spring,
    hv: Spring,
    trk: Spring,
    kick: Spring,
    pr: Spring,
    col: Vec4,
    /// the pulse ring's progress, 1 = idle
    pl: f32,
    seq: u64,
    /// the ring angle where each of the last steps landed, newest first
    marks: Vec<f32>,
    ico: (f32, f32),
    gy1z: f32,
    gy2z: f32,
    hov_a: f32,
    side: f32,
    pos: Vec3,
    sc: f32,
    fade: f32,
    /// the agent this one is a helper of
    parent: Option<String>,
}

impl Slot {
    fn blank() -> Slot {
        Slot {
            id: String::new(),
            alive: false,
            gone: false,
            ring: 0,
            state: State::Working,
            prev: None,
            p: 0.0,
            ang: 0.0,
            ang_r: 0.0,
            phase: 0.0,
            fly: Spring::new(0.0, 34.0, 9.5),
            foc: Spring::new(0.0, 40.0, 10.5),
            mg: Spring::new(0.0, 30.0, 12.0),
            rb: Spring::new(0.0, 22.0, 7.5),
            q: Spring::new(0.0, 36.0, 12.0),
            vis: Spring::new(0.0, 60.0, 13.0),
            hv: Spring::new(0.0, 90.0, 15.0),
            trk: Spring::new(0.0, 60.0, 14.0),
            kick: Spring::new(0.0, 220.0, 10.0),
            pr: Spring::new(0.0, 50.0, 12.0),
            col: Vec4::ONE,
            pl: 1.0,
            seq: 0,
            marks: Vec::new(),
            ico: (0.0, 0.0),
            gy1z: 0.0,
            gy2z: 0.0,
            hov_a: 0.0,
            side: 1.0,
            pos: Vec3::ZERO,
            sc: 1.0,
            fade: 0.0,
            parent: None,
        }
    }
}

#[derive(Resource)]
pub struct Scene {
    root: Entity,
    rings: Vec<RingGfx>,
    ready: ReadyGfx,
    sats: Vec<SatGfx>,
    arcs: Vec<ArcGfx>,
    slots: Vec<Slot>,
    /// conflicts on screen with how far their arc has grown (0..1 over .6 s)
    conf: Vec<(String, String, f32)>,
    spin_mul: f32,
    hidden: bool,
}

// ---------------------------------------------------------------- build

pub fn build(app: &mut App) {
    app.init_resource::<SceneOut>().add_systems(Startup, spawn).add_systems(Update, update.after(crate::core::update_core));
    ghosts::build(app);
}

struct Gfx<'a, 'w, 's> {
    c: &'a mut Commands<'w, 's>,
    meshes: &'a mut Assets<Mesh>,
    mats: &'a mut Assets<GlowMaterial>,
    gslots: &'a mut GlowSlots,
}

impl Gfx<'_, '_, '_> {
    /// One glow part under `parent`: its mesh, a fresh material (colour from the theme role, alpha, no walls). Every
    /// part has a slot: its colour and alpha move with the satellite's colour and fade, which settle but never quite
    /// stop (the colour's lerp is not exact), so keeping them in the material would rebuild it again and again.
    fn part(&mut self, parent: Entity, mut m: Mesh, kind: Kind, role: Role, alpha: f32, tf: Transform) -> (Entity, H) {
        // the handle isn't kept, so nothing reads or rebuilds this mesh: no CPU copy once it's uploaded
        m.asset_usage = RenderAssetUsages::RENDER_WORLD;
        let mut mat = GlowMaterial::new(kind, crate::theme::now_lin(role), alpha);
        mat.u.a.y = 0.0;
        self.gslots.attach(&mut mat);
        let h = self.mats.add(mat);
        let e = self.c.spawn((Mesh3d(self.meshes.add(m)), MeshMaterial3d(h.clone()), tf, Visibility::Inherited)).id();
        self.c.entity(parent).add_child(e);
        (e, h)
    }
    /// A part with no parent (world space), hidden until used.
    fn loose(&mut self, m: Mesh, role: Role) -> (Entity, Handle<Mesh>, H) {
        let mut mat = GlowMaterial::new(Kind::Ring, crate::theme::now_lin(role), 1.0);
        mat.u.a.y = 0.0;
        mat.u.b.x = 0.0;
        self.gslots.attach(&mut mat);
        let h = self.mats.add(mat);
        let mh = self.meshes.add(m);
        let e = self.c.spawn((Mesh3d(mh.clone()), MeshMaterial3d(h.clone()), Transform::default(), Visibility::Hidden, NoFrustumCulling)).id();
        (e, mh, h)
    }
    fn group(&mut self, parent: Option<Entity>, tf: Transform) -> Entity {
        let e = self.c.spawn((tf, Visibility::Inherited)).id();
        if let Some(p) = parent {
            self.c.entity(p).add_child(e);
        }
        e
    }
}

fn spawn(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut mats: ResMut<Assets<GlowMaterial>>, mut gslots: ResMut<GlowSlots>) {
    let mut g = Gfx { c: &mut commands, meshes: &mut meshes, mats: &mut mats, gslots: &mut gslots };
    let root = g.c.spawn((Transform::default(), Visibility::Hidden)).id();
    let tf = Transform::default();
    let mut rings = Vec::new();
    for d in REPOS {
        let group = g.group(Some(root), Transform::from_rotation(d.q()));
        let (_, thin) = g.part(group, mesh::geo(|g| crate::geo::ring_geo(g, d.rad, 0.8, crate::geo::RingOpt { walls: 0, ..default() })), Kind::Ring, Role::Line, 0.26, tf);
        let (_, band) = g.part(group, mesh::geo(|g| crate::geo::ring_geo(g, d.rad, 7.0, crate::geo::RingOpt { walls: 0, ..default() })), Kind::Ring, Role::Line, 0.02, tf);
        let (ticks_e, ticks) = g.part(group, mesh::geo(|g| crate::geo::ticks_geo(g, d.rad - 1.0, 72, 4.5, 0.9, 6, 8.0)), Kind::Ring, Role::Line, 0.22, tf);
        let (arcs_e, arcs) = g.part(group, mesh::geo(|g| crate::geo::arcs_geo(g, d.rad, 3, 1.5, 74.0)), Kind::Ring, Role::Fill, 0.32, tf);
        rings.push(RingGfx { group, thin, band, ticks, arcs, ticks_e, arcs_e, ticks_a: 0.0, arcs_a: 0.0, show: 0.0, hi: 0.0 });
    }
    let rg = g.group(Some(root), Transform::from_rotation(READY.q()));
    let (_, dash) = g.part(rg, mesh::geo(|g| crate::geo::dash_geo(g, READY.rad, 0.9, 7.0, 9.0)), Kind::Ring, Role::Ok, 0.42, tf);
    let (_, ring) = g.part(rg, mesh::geo(|g| crate::geo::ring_geo(g, READY.rad + 8.0, 0.7, crate::geo::RingOpt { walls: 0, ..default() })), Kind::Ring, Role::Ok, 0.12, tf);
    let (ticks_e, ticks) = g.part(rg, mesh::geo(|g| crate::geo::ticks_geo(g, READY.rad - 1.0, 48, 4.5, 0.9, 0, 0.0)), Kind::Ring, Role::Ok, 0.24, tf);
    let ready = ReadyGfx { group: rg, dash, ring, ticks, ticks_e, ticks_a: 0.0, fade: 0.4 };

    let mut sats = Vec::new();
    for _ in 0..NSAT {
        let sroot = g.c.spawn((Transform::default(), Visibility::Hidden)).id();
        let (_, shell) = g.part(sroot, mesh::shell_mesh(10.5), Kind::Fres, Role::Line, 0.62, tf);
        let ico = g.group(Some(sroot), tf);
        let (em, fm) = mesh::ico_meshes(6.2);
        let (_, edges) = g.part(ico, em, Kind::Line, Role::Hot, 0.85, tf);
        let (_, fill) = g.part(ico, fm, Kind::Line, Role::Fill, 0.07, tf);
        let (gy1, gy1_m) = g.part(sroot, mesh::geo(|g| crate::geo::ring_geo(g, 16.5, 0.9, crate::geo::RingOpt { walls: 0, ..default() })), Kind::Ring, Role::Line, 0.75, tf);
        let (gy2, gy2_m) = g.part(sroot, mesh::geo(|g| crate::geo::dash_geo(g, 21.0, 0.9, 3.2, 4.4)), Kind::Ring, Role::Line, 0.6, tf);
        let (_, track) = g.part(sroot, mesh::geo(|g| crate::geo::ring_geo(g, 26.0, 0.8, crate::geo::RingOpt { walls: 0, ..default() })), Kind::Ring, Role::Line, 0.26, tf);
        let (_, prog) = g.part(sroot, mesh::geo(|g| crate::geo::ring_geo(g, 26.0, 2.3, crate::geo::RingOpt { n: 72, walls: 0, ..default() })), Kind::Ring, Role::Fill, 0.95, tf);
        let (pulse, pulse_m) = g.part(sroot, mesh::geo(|g| crate::geo::ring_geo(g, 13.0, 1.3, crate::geo::RingOpt { walls: 0, ..default() })), Kind::Ring, Role::Hot, 0.9, tf);
        let (hov, hov_m) = g.part(sroot, mesh::geo(|g| crate::geo::arcs_geo(g, 35.0, 4, 1.1, 58.0)), Kind::Ring, Role::Hot, 0.9, tf);
        let (_, halo) = g.part(sroot, mesh::halo_mesh(HALO_R), Kind::Ring, Role::Line, 0.26, tf);
        let (trail, trail_mesh, trail_m) = g.loose(mesh::trail_mesh(REPOS[0].rad, 1.0), Role::Fill);
        let (marks, marks_mesh, marks_m) = g.loose(mesh::discs_mesh(&[]), Role::Hot);
        let (tether, tether_mesh, tether_m) = g.loose(mesh::ribbon_mesh(&[Vec3::ZERO, Vec3::X], 0.5, &[0.0, 0.0]), Role::Line);
        sats.push(SatGfx { root: sroot, shell, ico, edges, fill, gy1, gy1_m, gy2, gy2_m, track, prog, pulse, pulse_m, hov, hov_m, halo, trail, trail_mesh, trail_m, built: None, marks, marks_mesh, marks_m, marks_built: Vec::new(), tether, tether_mesh, tether_m, tether_built: None });
    }
    let mut arcs = Vec::new();
    for _ in 0..NARC {
        let (e, mesh, mat) = g.loose(mesh::ribbon_mesh(&[Vec3::ZERO, Vec3::X], 0.5, &[0.0, 0.0]), Role::Acc);
        arcs.push(ArcGfx { e, mesh, mat, built: None });
    }
    commands.insert_resource(Scene { root, rings, ready, sats, arcs, slots: (0..NSAT).map(|_| Slot::blank()).collect(), conf: Vec::new(), spin_mul: 1.0, hidden: true });
}

// ---------------------------------------------------------------- per frame

/// The swarm's frame: where its group sits (the core's position and tilt, the orbit tipped by `orbit_tilt`).
#[derive(Clone, Copy)]
struct Sw {
    pos: Vec3,
    rot: Quat,
    s: f32,
}

impl Sw {
    fn point(&self, d: &RingDef, a: f32, dz: f32) -> Vec3 {
        self.pos + self.rot * (ring_local(d, a, dz) * self.s)
    }
}

fn put(tfs: &mut Query<&mut Transform>, e: Entity, f: impl FnOnce(&mut Transform)) {
    if let Ok(mut t) = tfs.get_mut(e) {
        let mut n = *t;
        f(&mut n);
        if n != *t {
            *t = n;
        }
    }
}

fn show(vis: &mut Query<&mut Visibility>, e: Entity, on: bool) {
    if let Ok(mut v) = vis.get_mut(e) {
        v.set_if_neq(if on { Visibility::Inherited } else { Visibility::Hidden });
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity, clippy::needless_range_loop)]
pub fn update(
    sim: Res<Sim>,
    swarm: Res<Swarm>,
    decks: Res<Decks>,
    ctl: Res<CoreCtl>,
    sv: Res<StageView>,
    view: Res<crate::home::View>,
    mut scene: ResMut<Scene>,
    mut out: ResMut<SceneOut>,
    mut tfs: Query<&mut Transform>,
    mut vis: Query<&mut Visibility>,
    mut mats: ResMut<Assets<GlowMaterial>>,
    mut gslots: ResMut<GlowSlots>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let amt = clamp(swarm.amt.x, 0.0, 1.0);
    let sc = &mut *scene;
    if amt < 0.001 {
        if !sc.hidden {
            sc.hidden = true;
            for e in [sc.root].into_iter().chain(sc.sats.iter().flat_map(|s| [s.root, s.trail, s.marks, s.tether])).chain(sc.arcs.iter().map(|a| a.e)) {
                show(&mut vis, e, false);
            }
            for s in &mut sc.slots {
                *s = Slot::blank();
            }
            sc.conf.clear();
            *out = SceneOut::default();
        }
        return;
    }
    sc.hidden = false;
    show(&mut vis, sc.root, true);

    let (dt, t) = (sim.dt, sim.t);
    let (big, ui) = (motion::dt(Pace::Big, dt), motion::dt(Pace::Ui, dt));
    let cam = Vec3::new(view.px.x, sv.base_y + view.py.x, sv.d * ctl.zoom.x);
    let kpx = |z: f32| sv.h / (2.0 * (20.0 * DEG).tan() * (cam.z - z).max(60.0));
    let core = tfs.get(decks.root).map(|t| t.translation).unwrap_or(Vec3::ZERO);
    let tilt = tfs.get(decks.tilt).map(|t| t.rotation).unwrap_or(Quat::IDENTITY);
    let compact = sv.w < 820.0 || sv.w / sv.h.max(1.0) < 1.05;
    let focus_amt = clamp(swarm.focus_amt.x, 0.0, 1.0);
    let sw = Sw { pos: core, rot: tilt * (Quat::from_rotation_y((t * 0.13).sin() * 0.05) * Quat::from_rotation_x(-swarm.orbit_tilt.x * DEG)), s: if compact { 0.3 } else { 1.0 } * (1.0 - 0.08 * focus_amt) };
    out.sw = Some((sw.pos, sw.rot, sw.s));
    put(&mut tfs, sc.root, |tr| {
        tr.translation = sw.pos;
        tr.rotation = sw.rot;
        tr.scale = Vec3::splat(sw.s);
    });

    let model = &swarm.model;
    let req = match &swarm.mode {
        Mode::Request(id) => Some(id.as_str()),
        _ => None,
    };
    let foc = match &swarm.mode {
        Mode::Focus(id) => Some(id.as_str()),
        _ => None,
    };
    let cur = swarm.current();
    let hover = swarm.hover.as_deref();
    let dim = clamp(swarm.dim.x, 0.0, 1.0);
    let front = Vec3::new(core.x - 20.0, (core.y + 330.0).min(268.0), core.z + 330.0);
    let ftar = focus_target(cam, sv.w, sv.h);

    // ---- rings
    let k_spin = 1.0 - (-3.0 * dt).exp();
    sc.spin_mul = lerp(sc.spin_mul, if req.is_some() { 1.7 } else { 1.0 }, k_spin);
    let n_rings = model.repos.len().min(NRING);
    let hov_ring = hover.and_then(|h| sc.slots.iter().find(|s| s.alive && s.id == h)).filter(|s| s.state != State::Done).map(|s| s.ring);
    let (line, fillc, okc, acc, hot, dimc) = (crate::theme::now_lin(Role::Line), crate::theme::now_lin(Role::Fill), crate::theme::now_lin(Role::Ok), crate::theme::now_lin(Role::Acc), crate::theme::now_lin(Role::Hot), crate::theme::now_lin(Role::Dim));
    for (k, r) in sc.rings.iter_mut().enumerate() {
        r.show = lerp(r.show, if k < n_rings { 1.0 } else { 0.0 }, 1.0 - (-4.0 * dt).exp());
        r.hi = lerp(r.hi, if hov_ring == Some(k) { 0.9 } else { 0.0 }, 1.0 - (-8.0 * dt).exp());
        r.ticks_a += REPOS[k].w * 0.6 * dt * sc.spin_mul;
        r.arcs_a += REPOS[k].w * 2.4 * dt * sc.spin_mul;
        let (ta, aa) = (r.ticks_a, r.arcs_a);
        put(&mut tfs, r.ticks_e, |tr| tr.rotation = Quat::from_rotation_z(ta));
        put(&mut tfs, r.arcs_e, |tr| tr.rotation = Quat::from_rotation_z(aa));
        show(&mut vis, r.group, r.show > 0.003);
        let (f, hi) = (amt * r.show, r.hi);
        for (h, col, a) in [(&r.thin, line, 0.26), (&r.band, line, 0.02), (&r.ticks, line, 0.22), (&r.arcs, fillc, 0.32)] {
            gslots.update(&mut mats, h, |u| {
                u.color = col;
                u.a.x = a;
                u.a.w = hi;
                u.b.x = f;
            });
        }
    }
    let any_done = model.agents.iter().any(|a| a.ready());
    let rd = &mut sc.ready;
    rd.fade = lerp(rd.fade, if any_done { 1.0 } else { 0.4 }, 1.0 - (-3.0 * dt).exp());
    rd.ticks_a += TAU / 160.0 * dt * sc.spin_mul;
    let ra = rd.ticks_a;
    put(&mut tfs, rd.ticks_e, |tr| tr.rotation = Quat::from_rotation_z(ra));
    show(&mut vis, rd.group, true);
    let f = amt * rd.fade;
    for (h, a) in [(&rd.dash, 0.42), (&rd.ring, 0.12), (&rd.ticks, 0.24)] {
        gslots.update(&mut mats, h, |u| {
            u.color = okc;
            u.a.x = a;
            u.b.x = f;
        });
    }

    // ---- which slot draws which agent
    let mut assigned: Vec<Option<usize>> = vec![None; NSAT];
    for s in sc.slots.iter_mut() {
        if s.alive && !model.agents.iter().take(NSAT).any(|a| a.id == s.id) {
            s.gone = true;
        }
    }
    for (ai, a) in model.agents.iter().enumerate().take(NSAT) {
        let si = match sc.slots.iter().position(|s| s.alive && s.id == a.id) {
            Some(i) => i,
            None => {
                let Some(i) = sc.slots.iter().position(|s| !s.alive) else { continue };
                let ring = a.repo % NRING;
                let taken: Vec<f32> = sc.slots.iter().filter(|o| o.alive && o.ring == ring).map(|o| o.ang).collect();
                let mut s = Slot::blank();
                s.alive = true;
                s.id = a.id.clone();
                s.parent = a.parent.clone();
                s.ring = ring;
                s.ang = pick_angle(&a.id, &taken);
                s.phase = hash(&a.id, 7) * TAU;
                s.seq = a.step_seq;
                s.p = a.p;
                s.pr.set(if a.state == State::Queued { 0.0 } else { a.p });
                s.q.set(if a.state == State::Queued { 1.0 } else { 0.0 });
                s.col = state_col(a.state, line, hot, acc, okc, dimc);
                // the last steps landed behind it, a step every ~3 s
                let sgn = REPOS[ring].w.signum();
                s.marks = (0..a.steps.len().min(9)).map(|i| s.ang - sgn * i as f32 * REPOS[ring].w.abs() * 3.0).collect();
                sc.slots[i] = s;
                i
            }
        };
        sc.slots[si].gone = false;
        assigned[si] = Some(ai);
    }

    // ---- every satellite
    out.nodes.clear();
    out.marks.clear();
    out.crests.clear();
    let tgt_state = |s: &Slot| if s.gone { State::Stopped } else { s.state };
    // a helper follows its parent, so the parents move first
    let mut order: Vec<usize> = (0..NSAT).collect();
    order.sort_by_key(|&i| sc.slots[i].parent.is_some());
    for si in order {
        if !sc.slots[si].alive {
            show(&mut vis, sc.sats[si].tether, false);
            show(&mut vis, sc.sats[si].root, false);
            show(&mut vis, sc.sats[si].trail, false);
            show(&mut vis, sc.sats[si].marks, false);
            continue;
        }
        let agent = assigned[si].map(|ai| &model.agents[ai]);
        // the ready slot of an agent that just finished: the farthest from those already there
        let taken: Vec<f32> = sc.slots.iter().enumerate().filter(|(j, o)| *j != si && o.alive && matches!(o.prev, Some(State::Done | State::Merged))).map(|(_, o)| o.ang_r).collect();
        let anchor = sc.slots[si].parent.as_ref().and_then(|p| sc.slots.iter().find(|o| o.alive && !o.gone && &o.id == p)).map(|o| (o.pos, o.sc));
        let s = &mut sc.slots[si];
        if let Some(a) = agent {
            s.parent = a.parent.clone();
            s.state = a.state;
            s.p = a.p;
            s.ring = a.repo % NRING;
        }
        let state = tgt_state(s);
        if s.prev != Some(state) {
            if state == State::Done {
                s.ang_r = ready_slot(&taken);
                if s.prev.is_some() {
                    s.kick.x += 7.0;
                } else {
                    s.rb.set(1.0);
                }
            }
            if state == State::Merged && s.prev.is_none() {
                s.rb.set(1.0);
            }
            s.prev = Some(state);
        }
        let def = REPOS[s.ring];
        match state {
            State::Working => s.ang += def.w * dt,
            State::Needs => s.ang += def.w * dt * 0.7,
            State::Done | State::Merged => {
                s.ang += def.w * dt * 0.5;
                s.ang_r += READY.w * dt;
            }
            State::Queued | State::Stopped => {}
        }
        // a step landed: pulse, kick, and remember where on the ring
        if let Some(a) = agent {
            if a.step_seq != s.seq {
                if a.step_seq > s.seq {
                    s.pl = 0.0;
                    s.kick.x += 4.0;
                    s.marks.insert(0, s.ang);
                    s.marks.truncate(9);
                }
                s.seq = a.step_seq;
            }
        }
        if s.pl < 1.0 {
            s.pl = (s.pl + dt / 0.95).min(1.0);
        }
        let is = |x: Option<&str>| x == Some(s.id.as_str());
        s.fly.t = is(req) as u8 as f32;
        s.foc.t = is(foc) as u8 as f32;
        s.mg.t = (state == State::Merged) as u8 as f32;
        s.rb.t = matches!(state, State::Done | State::Merged) as u8 as f32;
        s.q.t = (state == State::Queued) as u8 as f32;
        s.vis.t = if state == State::Stopped { 0.0 } else { 1.0 };
        s.vis.k = if state == State::Stopped { 40.0 } else { 60.0 };
        s.hv.t = is(hover) as u8 as f32;
        s.trk.t = clamp((if swarm.tracks { 0.6 } else { 0.0 }) + (match state { State::Working => 0.24, State::Needs => 0.2, _ => 0.0 }) + clamp(s.hv.x, 0.0, 1.0) * 0.8, 0.0, 1.0);
        s.pr.t = if state == State::Queued { 0.0 } else { s.p };
        s.kick.t = 0.0;
        for sp in [&mut s.fly, &mut s.foc, &mut s.mg, &mut s.rb, &mut s.q, &mut s.vis] {
            sp.step(big);
        }
        for sp in [&mut s.hv, &mut s.trk, &mut s.kick, &mut s.pr] {
            sp.step(ui);
        }
        s.col = s.col.lerp(state_col(state, line, hot, acc, okc, dimc), 1.0 - (-6.0 * dt).exp());
        let lazy = match state {
            State::Working => 1.0,
            State::Queued => 0.25,
            _ => 0.55,
        };
        s.ico.0 += dt * 0.37 * lazy;
        s.ico.1 += dt * 0.8 * lazy;
        s.gy1z += dt * 0.7 * lazy;
        s.gy2z -= dt * 0.45 * lazy;
        s.hov_a += dt * 0.55;
        // where it is
        let wob = (t * 0.45 + s.phase * 1.7).sin() * 0.012;
        let bob = (t * 0.9 + s.phase).sin() * 5.0;
        let qorder = agent.and_then(|a| a.queue_pos).unwrap_or(si) as f32;
        let sway = if state == State::Queued { (t * 0.35 + qorder * 2.0).sin() * 0.07 } else { 0.0 };
        let on_ring = sw.point(&def, s.ang + wob + sway, bob);
        let on_ready = sw.point(&READY, s.ang_r + wob, bob);
        let orb = on_ring.lerp(on_ready, s.rb.x);
        let orb = match anchor {
            Some((pp, _)) => {
                let a = t * 0.55 + s.phase;
                pp + Vec3::new(a.cos() * 44.0, a.sin() * 44.0 + 4.0, 10.0) * sw.s.max(0.5)
            }
            None => orb,
        };
        s.pos = orb.lerp(front, s.fly.x).lerp(ftar, s.foc.x).lerp(core, clamp(s.mg.x, 0.0, 1.05));
        let (hv, fly, foc_x, mg, q) = (clamp(s.hv.x, 0.0, 1.0), s.fly.x, s.foc.x, s.mg.x, clamp(s.q.x, 0.0, 1.0));
        s.sc = (1.0 + hv * 0.22 + s.kick.x.max(0.0) * 0.1 + fly * 0.35 + foc_x * 0.25) * (1.0 - 0.84 * smooth(0.0, 1.0, mg)) * (1.0 - 0.12 * q) * if anchor.is_some() { 0.66 } else { 1.0 };
        let dim_o = if cur.is_some() && !is(cur) { 1.0 - 0.48 * dim } else { 1.0 };
        s.fade = clamp(s.vis.x, 0.0, 1.0) * (1.0 - 0.3 * q) * dim_o * (1.0 - smooth(0.5, 1.0, mg));
        if s.gone && s.vis.x < 0.01 {
            s.alive = false;
            continue;
        }
        let shown = s.vis.x > 0.01 && mg < 0.99 && s.fade > 0.002;
        // the label side, with the SPEC's 80 px hysteresis from the core
        let dx = (s.pos.x - core.x) * kpx(s.pos.z);
        if s.side > 0.0 && dx < -80.0 {
            s.side = -1.0;
        } else if s.side < 0.0 && dx > 80.0 {
            s.side = 1.0;
        }
        let ft = s.fade * amt;
        // marks for the HUD (index = steps[i]) and the mark discs
        let sgn = def.w.signum();
        let zfac = 0.2 + 0.8 * hv;
        let mut hud_marks = Vec::new();
        let mut discs = Vec::new();
        let on_ring_k = (1.0 - clamp(fly.max(foc_x).max(s.rb.x).max(mg), 0.0, 1.0)) * if matches!(state, State::Queued | State::Stopped) { 0.0 } else { 1.0 };
        let mk = clamp((if swarm.tracks { 0.85 } else { 0.0 }) + hv, 0.0, 1.0) * on_ring_k;
        let marks_on = if anchor.is_some() { &[][..] } else { &s.marks[..] };
        for (i, m) in marks_on.iter().enumerate() {
            let back = (s.ang - m) * sgn;
            let inside = (0.0..=mesh::TRAIL_SPAN).contains(&back);
            let f = clamp(back / mesh::TRAIL_SPAN, 0.0, 1.0);
            let p = sw.point(&def, *m, mesh::fan(f) * zfac);
            let al = if inside { (1.0 - f) * 0.95 + 0.05 } else { 0.0 };
            hud_marks.push((p, al));
            if inside && mk * ft > 0.004 {
                discs.push((p, 5.5 / kpx(p.z) * if i == 0 { 1.35 } else { 1.0 }, al));
            }
        }
        out.marks.push((s.id.clone(), hud_marks));
        if shown {
            out.nodes.push((s.id.clone(), s.pos, s.sc, ft, s.side));
        }

        // ---- the satellite's parts
        let g = &sc.sats[si];
        show(&mut vis, g.root, shown);
        if shown {
            let (c, sp) = (s.col, s.pos);
            let needs = state == State::Needs;
            put(&mut tfs, g.root, |tr| {
                tr.translation = sp;
                tr.scale = Vec3::splat(s.sc);
            });
            put(&mut tfs, g.ico, |tr| tr.rotation = Quat::from_euler(EulerRot::XYZ, s.ico.0, s.ico.1, 0.0));
            put(&mut tfs, g.gy1, |tr| tr.rotation = Quat::from_euler(EulerRot::XYZ, 1.12 + (t * 0.3 + s.phase).sin() * 0.14, 0.0, s.gy1z));
            put(&mut tfs, g.gy2, |tr| tr.rotation = Quat::from_euler(EulerRot::XYZ, 1.1, 0.5 + (t * 0.22 + s.phase).sin() * 0.2, s.gy2z));
            put(&mut tfs, g.pulse, |tr| tr.scale = Vec3::splat(1.0 + s.pl * 2.5));
            put(&mut tfs, g.hov, |tr| tr.rotation = Quat::from_rotation_z(s.hov_a));
            let pulse_a = if s.pl < 1.0 { 0.9 * (1.0 - s.pl).powf(1.5) } else { 0.0 };
            let halo_f = if needs { 1.5 } else if state == State::Queued { 0.3 } else { 0.7 + hv * 0.5 };
            let reveal = clamp(s.pr.x, 0.0, 1.0);
            let sets: [(&H, Vec4, f32, f32); 10] = [
                (&g.shell, c, (0.62 + if needs { 0.25 } else { 0.0 }) * (1.0 - 0.4 * q), ft),
                (&g.edges, hot, 0.85, ft),
                (&g.fill, fillc, 0.07, ft),
                (&g.gy1_m, c, 0.75, ft),
                (&g.gy2_m, c, lerp(0.6, 1.0, q), ft),
                (&g.track, c, 0.26 * (1.0 - q), ft),
                (&g.prog, c, 0.95 * (1.0 - q), ft),
                (&g.pulse_m, hot, pulse_a, ft),
                (&g.hov_m, hot, 0.9, hv * 0.85 * ft),
                (&g.halo, c, (0.26 + hv * 0.1 + fly * 0.2) * halo_f, ft),
            ];
            for (h, col, a, f) in sets {
                gslots.update(&mut mats, h, |u| {
                    u.color = col;
                    u.a.x = a;
                    u.b.x = f;
                });
            }
            gslots.update(&mut mats, &g.prog, |u| u.a.z = reveal);
        }

        // ---- the step trail (rebuilt only when its ring changes) and marks
        let g = &mut sc.sats[si];
        let trail_f = if anchor.is_some() { 0.0 } else { s.trk.x * on_ring_k * ft };
        if shown && trail_f > 0.004 {
            let want = (s.ring, sgn);
            if g.built != Some(want) {
                g.built = Some(want);
                if let Some(mut m) = meshes.get_mut(&g.trail_mesh) {
                    *m = mesh::trail_mesh(def.rad, sgn);
                }
            }
            let ang = s.ang + wob;
            put(&mut tfs, g.trail, |tr| {
                tr.translation = sw.pos;
                tr.rotation = sw.rot * def.q() * Quat::from_rotation_z(ang);
                tr.scale = Vec3::new(sw.s, sw.s, sw.s * zfac);
            });
            gslots.update(&mut mats, &g.trail_m, |u| {
                u.color = fillc;
                u.a.x = 0.9;
                u.b.x = trail_f;
            });
            show(&mut vis, g.trail, true);
        } else {
            show(&mut vis, g.trail, false);
        }
        if shown && !discs.is_empty() {
            // a rebuilt mesh makes the renderer queue its entity again, which reorders the draws it ties with (so a
            // still frame keeps changing): rebuilt only when the discs moved
            if g.marks_built != discs {
                if let Some(mut m) = meshes.get_mut(&g.marks_mesh) {
                    *m = mesh::discs_mesh(&discs);
                }
                g.marks_built = discs;
            }
            gslots.update(&mut mats, &g.marks_m, |u| {
                u.color = hot;
                u.a.x = 1.0;
                u.b.x = mk * ft;
            });
            show(&mut vis, g.marks, true);
        } else {
            show(&mut vis, g.marks, false);
        }

        // ---- a helper's tether back to its parent
        match anchor {
            Some((pp, psc)) if shown => {
                let (a, b) = (pp, s.pos);
                let d = (b - a).normalize_or_zero();
                let (a, b) = (a + d * 17.0 * psc, b - d * 11.0 * s.sc);
                let w = 0.9 / kpx(a.lerp(b, 0.5).z);
                let want = (a, b, w, t);
                if g.tether_built.map(|o| (o.0, o.1, o.2)) != Some((a, b, w)) {
                    g.tether_built = Some(want);
                    let n = 8;
                    let pts: Vec<Vec3> = (0..=n).map(|i| a.lerp(b, i as f32 / n as f32)).collect();
                    let ks: Vec<f32> = (0..=n).map(|i| 0.35 + 0.65 * (std::f32::consts::PI * i as f32 / n as f32).sin()).collect();
                    if let Some(mut m) = meshes.get_mut(&g.tether_mesh) {
                        *m = mesh::ribbon_mesh(&pts, w, &ks);
                    }
                }
                gslots.update(&mut mats, &g.tether_m, |u| {
                    u.color = line;
                    u.a.x = 1.0;
                    u.b.x = ft;
                });
                show(&mut vis, g.tether, true);
            }
            _ => show(&mut vis, g.tether, false),
        }
    }

    // ---- conflict arcs (SPEC 5)
    sc.conf.retain(|(a, b, _)| model.conflicts.iter().any(|c| &c.a == a && &c.b == b));
    for c in model.conflicts.iter().take(NARC) {
        if !sc.conf.iter().any(|(a, b, _)| *a == c.a && *b == c.b) {
            sc.conf.push((c.a.clone(), c.b.clone(), 0.0));
        }
    }
    let pulse = 0.72 + 0.28 * (t * 3.1).sin();
    let beam = acc.lerp(hot, 0.4);
    let mut used = 0;
    for (a, b, age) in sc.conf.iter_mut() {
        *age = (*age + dt / 0.6).min(1.0);
        let find = |id: &str| sc.slots.iter().find(|s| s.alive && s.id == id && s.vis.x > 0.01);
        let (Some(sa), Some(sb)) = (find(a), find(b)) else { continue };
        if used >= NARC {
            break;
        }
        let (pa, pb) = (sa.pos, sb.pos);
        let m = arc_ctrl(pa, pb, core, *age);
        let g = &mut sc.arcs[used];
        used += 1;
        let mid = bez(pa, m, pb, 0.5);
        // rebuilt only when it moved or pulsed, like the marks
        let want = (pa, m, pb, 1.0 / kpx(mid.z), pulse, *age);
        if g.built != Some(want) {
            g.built = Some(want);
            let n = 20;
            let pts: Vec<Vec3> = (0..=n).map(|i| bez(pa, m, pb, i as f32 / n as f32)).collect();
            let ks: Vec<f32> = (0..=n).map(|i| (0.18 + 0.82 * (std::f32::consts::PI * i as f32 / n as f32).sin()) * pulse * *age).collect();
            if let Some(mut mm) = meshes.get_mut(&g.mesh) {
                *mm = mesh::ribbon_mesh(&pts, want.3, &ks);
            }
        }
        let f = amt * sa.fade.min(sb.fade).max(0.0) * (1.0 - 0.55 * dim);
        gslots.update(&mut mats, &g.mat, |u| {
            u.color = beam;
            u.a.x = 1.0;
            u.b.x = f;
        });
        show(&mut vis, g.e, true);
        out.crests.push((a.clone(), b.clone(), mid + Vec3::new(0.0, 20.0, 2.0)));
    }
    for g in sc.arcs.iter().skip(used) {
        show(&mut vis, g.e, false);
    }

    // ---- ring labels
    out.ring_label_pos.clear();
    for d in REPOS.iter().take(n_rings) {
        out.ring_label_pos.push(sw.point(d, 100.0 * DEG, 0.0) + Vec3::Y * 14.0);
    }
    out.ready_label_pos = sw.point(&READY, 78.0 * DEG, 0.0) + Vec3::Y * 14.0;
}

fn state_col(s: State, line: Vec4, hot: Vec4, acc: Vec4, ok: Vec4, dim: Vec4) -> Vec4 {
    match s {
        State::Needs => acc,
        State::Done | State::Merged => ok,
        State::Queued => dim,
        State::Working | State::Stopped => line.lerp(hot, 0.3),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ring_point_sits_on_its_ring() {
        for d in REPOS.iter().chain([&READY]) {
            for k in 0..12 {
                let p = ring_local(d, k as f32 * 0.7, 0.0);
                assert!((p.length() - d.rad).abs() < 1e-2, "{p:?}");
            }
        }
        // out of plane by dz, along the ring's normal
        let p = ring_local(&REPOS[1], 1.0, 5.0);
        assert!((p.length() - (372.0f32 * 372.0 + 25.0).sqrt()).abs() < 1e-2);
        // an untilted ring is the plain circle
        assert!((ring_local(&REPOS[0], 0.0, 0.0) - Vec3::new(292.0, 0.0, 0.0)).length() < 1e-3);
    }

    #[test]
    fn the_first_ready_slot_is_160_degrees_and_the_next_is_far_from_it() {
        assert!((ready_slot(&[]) - 160.0 * DEG).abs() < 1e-6);
        let a = ready_slot(&[160.0 * DEG]);
        assert!(ang_dist(a, 160.0 * DEG) > 150.0 * DEG, "{}", a / DEG);
        let b = ready_slot(&[160.0 * DEG, a]);
        assert!(ang_dist(b, a) > 60.0 * DEG && ang_dist(b, 160.0 * DEG) > 60.0 * DEG);
    }

    #[test]
    fn new_agents_spread_round_their_ring() {
        let ids = ["t-aa11", "t-bb22", "t-cc33", "t-dd44", "t-ee55", "t-ff66"];
        let mut taken: Vec<f32> = Vec::new();
        for id in ids {
            let a = pick_angle(id, &taken);
            assert_eq!(a, pick_angle(id, &taken), "deterministic");
            for t in &taken {
                assert!(ang_dist(a, *t) > 29.0 * DEG, "{id} landed {} deg from another", ang_dist(a, *t) / DEG);
            }
            taken.push(a);
        }
        // the prototype's agents start where SPEC 9 says
        assert!((pick_angle("redis", &[]) - 205.0 * DEG).abs() < 1e-5);
        assert!((pick_angle("cold", &[1.0]) - 64.0 * DEG).abs() < 1e-5);
    }

    #[test]
    fn the_conflict_arc_bulges_away_from_the_core_and_ends_on_its_nodes() {
        let (a, b, core) = (Vec3::new(300.0, 100.0, 0.0), Vec3::new(280.0, -150.0, 0.0), Vec3::ZERO);
        // not grown yet: a straight line between the nodes
        let m0 = arc_ctrl(a, b, core, 0.0);
        assert!((bez(a, m0, b, 0.5) - (a + b) / 2.0).length() < 1e-3);
        let m = arc_ctrl(a, b, core, 1.0);
        assert!(m.x > (a.x + b.x) / 2.0 + 60.0 && m.z > 40.0);
        assert!((bez(a, m, b, 0.0) - a).length() < 1e-4 && (bez(a, m, b, 1.0) - b).length() < 1e-4);
        // the crest is half-way to the control point
        let crest = bez(a, m, b, 0.5);
        assert!((crest - (a + b) / 2.0).length() > 30.0);
        // a degenerate midpoint on the core still bulges somewhere (up)
        let m = arc_ctrl(Vec3::new(10.0, 0.0, 0.0), Vec3::new(-10.0, 0.0, 0.0), Vec3::ZERO, 1.0);
        assert!(m.y > 30.0);
    }

    #[test]
    fn the_focus_target_is_front_left_on_the_z235_plane() {
        let p = focus_target(Vec3::new(0.0, 0.0, 1700.0), 1600.0, 1000.0);
        assert!((p.z - 235.0).abs() < 1e-3);
        assert!(p.x < -400.0 && p.y > 100.0, "{p:?}");
        // projecting it back lands on (160, 170) px
        let k = 1000.0 / (2.0 * (20.0 * DEG).tan() * (1700.0 - 235.0));
        assert!((800.0 + p.x * k - 160.0).abs() < 0.5 && (500.0 - p.y * k - 170.0).abs() < 0.5);
    }

    #[test]
    fn the_trail_fans_out_of_the_plane_and_fades() {
        assert_eq!(mesh::fan(0.0), 0.0);
        assert!((mesh::fan(1.0) - 22.0).abs() < 1e-4);
        assert!(mesh::fan(0.5) < 22.0 * 0.6 + 1.0);
    }
}
