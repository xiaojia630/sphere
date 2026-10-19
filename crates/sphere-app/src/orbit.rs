//! Core open (prototype: Core Open): the core tips back 62° like a holo-table and the app tiles (seven while Music hides)
//! rise on light beams onto a spinnable orbit. A, the dock's Apps button, typing "apps", or holding the sphere
//! opens it; Esc, A, or a click on the sphere closes it. ← → (or the wheel, or a drag) spin it; Enter or a
//! click opens the front app. Terminal and Agents open their real panels; the rest come later.
use crate::core::{CoreCtl, CoreTilt};
use crate::fonts::UiFonts;
use crate::geo::*;
use crate::glow::{GlowMaterial, GlowSlots, Kind, ATTR_HOLO};
use crate::homepanels::APPS;
use crate::panel::*;
use crate::util::*;
use bevy::asset::RenderAssetUsages;
use bevy::mesh::PrimitiveTopology;
use bevy::prelude::*;
use bevy::text::LineHeight;

const R: f32 = 560.0;
const Z: f32 = -160.0;
const H: f32 = 110.0;
/// How many tiles ride the orbit, evenly spaced (45° apart for eight).
const N: usize = APPS.len();
const STEP: f32 = TAU / N as f32;
/// The fan-out order, as offsets from the front: the front, then alternating neighbours, the back last.
const FAN: [usize; N] = fan();

const fn fan() -> [usize; N] {
    let mut f = [0; N];
    let mut k = 1;
    while k < N {
        f[k] = if k % 2 == 1 { k.div_ceil(2) } else { N - k / 2 };
        k += 1;
    }
    f
}
pub const TILE_W: f32 = 232.0;
pub const TILE_H: f32 = 190.0;

#[derive(Resource)]
pub struct Orb {
    pub open: bool,
    pub amt: Spring,
    pub a: Spring,
    pub(crate) em: [Spring; N],
    kick: [Spring; N],
    /// seconds since the last open/close (for the fan-out stagger)
    since: f32,
    /// spin the front tile to here, then open it
    launch: Option<usize>,
    front: usize,
    /// drag spin
    pub dragging: bool,
    pub version: u64,
}

impl Default for Orb {
    fn default() -> Self {
        Orb {
            open: false,
            amt: Spring::new(0.0, 30.0, 10.5),
            a: Spring::new(0.0, 60.0, 15.0),
            em: [Spring::new(0.0, 62.0, 11.5); N],
            kick: [Spring::new(0.0, 220.0, 8.0); N],
            since: 9.0,
            launch: None,
            front: 0,
            dragging: false,
            version: 0,
        }
    }
}

impl Orb {
    pub fn amount(&self) -> f32 {
        clamp(self.amt.x, 0.0, 1.0)
    }
    pub fn toggle(&mut self, sound: &mut crate::sound::Sound) {
        self.set(!self.open, sound);
    }
    pub fn set(&mut self, open: bool, sound: &mut crate::sound::Sound) {
        if open == self.open {
            return;
        }
        self.open = open;
        self.since = 0.0;
        self.launch = None;
        self.version += 1;
        sound.play(if open { "expand" } else { "collapse" });
    }
    /// One slot left or right (→ brings the next app to the front).
    pub fn step(&mut self, dir: f32) {
        let slot = (self.a.t / STEP).round();
        self.a.t = (slot - dir) * STEP;
    }
    /// Spin app `i` to the front, then open it.
    pub fn launch(&mut self, i: usize) {
        // the nearest target angle that puts i at the front: φ = i·STEP + a ≡ 0
        let base = -(i as f32) * STEP;
        let k = ((self.a.t - base) / TAU).round();
        self.a.t = base + k * TAU;
        self.launch = Some(i);
    }
    pub fn launch_front(&mut self) {
        self.launch(self.front);
    }
    /// The drag spin: radians per pointer px like the prototype's `dx·2.4/max(320, .9W)`.
    pub fn drag_by(&mut self, dx: f32, w: f32) {
        let da = dx * 2.4 / (0.9 * w).max(320.0);
        self.a.x += da;
        self.a.t = self.a.x;
        self.launch = None;
    }
    pub fn release(&mut self, v: f32) {
        let tgt = self.a.x + v.clamp(-9.0, 9.0) * 0.28;
        self.a.t = (tgt / STEP).round() * STEP;
    }
}

#[derive(Component)]
pub struct OrbTrack;
#[derive(Component)]
pub struct OrbMarker;
#[derive(Component)]
pub struct Tile(pub usize);
#[derive(Component)]
pub struct TileFoot(pub usize);
/// A tile's right label (0), headline (1) or sub line (2), written from live state each frame.
#[derive(Component)]
pub struct TileText(pub usize, pub u8);
#[derive(Component)]
pub struct Beam(pub usize);

fn beam_mesh() -> Mesh {
    // a unit-high strip, bright at the pad (v = 0) fading up: k = .16 + .84(1 - v)^1.5
    let n = 12;
    let mut pos = Vec::new();
    let mut at = Vec::new();
    for i in 0..n {
        let (v0, v1) = (i as f32 / n as f32, (i + 1) as f32 / n as f32);
        let k = |v: f32| 0.16 + 0.84 * (1.0 - v).powf(1.5);
        let quad = [[-0.5, v0, k(v0)], [0.5, v0, k(v0)], [0.5, v1, k(v1)], [-0.5, v0, k(v0)], [0.5, v1, k(v1)], [-0.5, v1, k(v1)]];
        for [x, y, kk] in quad {
            pos.push([x, y, 0.0]);
            at.push([0.0, 0.0, 0.0, kk]);
        }
    }
    // built once and never read back: no CPU copy once it's uploaded
    let mut m = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
    m.insert_attribute(ATTR_HOLO, at);
    m
}

#[allow(clippy::too_many_arguments)]
pub fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut gmats: ResMut<Assets<GlowMaterial>>,
    mut gslots: ResMut<GlowSlots>,
    mut atlas: ResMut<crate::panel::PanelAtlas>,
    mut pmats: ResMut<Assets<PanelMaterial>>,
    f: Res<UiFonts>,
    tilt: Query<Entity, With<CoreTilt>>,
    args: Res<crate::Args>,
) {
    let Ok(tilt) = tilt.single() else { return };
    let c = &mut commands;
    let gm = |role: Role, alpha: f32, wall: f32| {
        let mut m = GlowMaterial::new(Kind::Ring, crate::theme::now_lin(role), alpha);
        m.u.a.y = wall;
        m.u.b.y = 14.0;
        m
    };
    // the track (spins with the orbit)
    let track = c.spawn((OrbTrack, Transform::from_xyz(0.0, 0.0, Z), Visibility::Hidden)).id();
    c.entity(tilt).add_child(track);
    let mut parts: Vec<(Geo, Role, f32, f32)> = Vec::new();
    let mut g = Geo::new();
    ring(&mut g, R, 1.5);
    parts.push((g, Role::Line, 0.9, 0.3));
    let mut g = Geo::new();
    ring(&mut g, R - 20.0, 36.0);
    parts.push((g, Role::Deep, 0.08, 0.0));
    let mut g = Geo::new();
    ticks_geo(&mut g, R + 9.0, 180, 4.0, 1.0, 15, 10.0);
    parts.push((g, Role::Dim, 0.85, 0.3));
    let mut g = Geo::new();
    dash_geo(&mut g, R - 44.0, 1.0, 3.0, 10.0);
    parts.push((g, Role::Dim, 0.55, 0.3));
    let mut g = Geo::new();
    for i in 0..N {
        // pads at π − i·STEP, measured clockwise from 12 o'clock, so i = 0 is at the front
        let mid = std::f32::consts::PI - i as f32 * STEP;
        ring_geo(&mut g, R, 7.0, RingOpt { a0: mid - 4.0 * DEG, a1: mid + 4.0 * DEG, walls: 2, ..default() });
    }
    parts.push((g, Role::Fill, 1.0, 0.5));
    for (g, role, a, wall) in parts {
        let e = c.spawn((Mesh3d(meshes.add(g.into_gpu_mesh())), MeshMaterial3d(gmats.add(gm(role, a, wall))), Transform::default(), crate::core::GlowInfo { deck: crate::core::CORE, role, alpha: a, wall, tag: crate::core::Tag::Orb })).id();
        c.entity(track).add_child(e);
    }
    // the fixed front marker: an arc ±9° and two arrowheads at ±15°
    let marker = c.spawn((OrbMarker, Transform::from_xyz(0.0, 0.0, Z), Visibility::Hidden)).id();
    c.entity(tilt).add_child(marker);
    let mut g = Geo::new();
    let front = std::f32::consts::PI;
    ring_geo(&mut g, R, 3.0, RingOpt { a0: front - 9.0 * DEG, a1: front + 9.0 * DEG, ..default() });
    for s in [-1.0f32, 1.0] {
        let tip = pt(R, front + s * 11.0 * DEG);
        let b0 = pt(R - 7.0, front + s * 15.0 * DEG);
        let b1 = pt(R + 7.0, front + s * 15.0 * DEG);
        g.tri(tip, b0, b1, 1.0);
    }
    let e = c.spawn((Mesh3d(meshes.add(g.into_gpu_mesh())), MeshMaterial3d(gmats.add(gm(Role::Hot, 1.0, 0.3))), Transform::default(), crate::core::GlowInfo { deck: crate::core::CORE, role: Role::Hot, alpha: 1.0, wall: 0.3, tag: crate::core::Tag::Orb })).id();
    c.entity(marker).add_child(e);
    // the tiles and their beams
    let bm = meshes.add(beam_mesh());
    for (i, (name, live)) in APPS.iter().enumerate() {
        let ui = spawn_ui_panel(c, &mut atlas, TILE_W, TILE_H);
        build_tile(c, ui.root, &f, i, name, *live);
        let e = spawn_panel_quad(c, &mut meshes, &mut pmats, &ui, &PanelSpec { id: PanelId::Tile, w: TILE_W, h: TILE_H, home: Vec3::ZERO, home_scale: 1.0, side: 0.0, phase: 0.0, show_at: 0.0, unframed: false });
        c.entity(e).remove::<Panel>().insert((Tile(i), Visibility::Hidden));
        let mut m = GlowMaterial::new(Kind::Ring, crate::theme::now_lin(Role::Fill), 0.0);
        m.u.a.y = 0.0;
        // a beam's alpha follows its tile every frame while the tiles move (see `update`)
        gslots.attach(&mut m);
        c.spawn((Beam(i), Mesh3d(bm.clone()), MeshMaterial3d(gmats.add(m)), Transform::default(), Visibility::Hidden));
    }
    c.insert_resource(Orb { open: args.apps, since: 0.0, ..default() });
}

/// What a tile says (right label, headline, sub line), from what the machine and sphered report right now.
/// `None` is the moment before the first look: nothing claimed yet.
fn tile_text(i: usize, now: Option<&TileState>) -> (String, String, String) {
    let tasks = now.map(|n| n.tasks);
    match i {
        0 => ("live".into(), now.map(|n| format!("{} tab{}", n.tabs, if n.tabs == 1 { "" } else { "s" })).unwrap_or_else(|| "Your shell".into()), "Cmd T new · Cmd 1-9 switch".into()),
        1 => match now {
            Some(n) if n.agents == 0 => ("live".into(), "No agents yet".into(), "Start one with /".into()),
            Some(n) => ("live".into(), format!("{} working", n.working), if n.needs > 0 { format!("{} need{} you · Enter to answer", n.needs, if n.needs == 1 { "s" } else { "" }) } else { "Start one with / · answer with Enter".into() }),
            None => ("live".into(), "Agents".into(), "Start one with /".into()),
        },
        2 => ("live".into(), "Zed and diffs".into(), match tasks {
            Some(0) => "No tasks yet · review, merge, PR".into(),
            Some(n) => format!("{n} task{} · review, merge, PR", if n == 1 { "" } else { "s" }),
            None => "Review, checkpoints, merge, PR".into(),
        }),
        3 => match now.and_then(|n| n.prs.as_ref()) {
            Some((open, line)) => (format!("{open} open"), "Branches and PRs".into(), line.clone()),
            None => ("live".into(), "Branches and PRs".into(), now.and_then(|n| n.git.clone()).unwrap_or_else(|| "Status, branches and pull requests".into())),
        },
        4 => ("live".into(), now.map(|n| n.docker.clone()).unwrap_or_else(|| "Docker".into()), now.map(|n| n.docker_detail.clone()).unwrap_or_else(|| "Start, stop, restart, logs".into())),
        5 => ("live".into(), "Ports and logs".into(), now.and_then(|n| n.ports).map(|p| format!("{p} listening · and their output")).unwrap_or_else(|| "What's listening, and its output".into())),
        6 => ("live".into(), "Files".into(), now.and_then(|n| n.files.clone()).unwrap_or_else(|| "Browse, preview, quick edit".into())),
        // Music: only while `homepanels::SHOW_MUSIC` brings its tile back
        _ => ("soon".into(), "Music".into(), "Not built yet".into()),
    }
}

struct TileState {
    tabs: usize,
    agents: usize,
    working: usize,
    needs: usize,
    tasks: usize,
    docker: String,
    docker_detail: String,
    /// "main · 3 changed", ports listening and "Projects · 24 items", once those apps have read them
    git: Option<String>,
    ports: Option<usize>,
    files: Option<String>,
    prs: Option<(usize, String)>,
}

fn build_tile(c: &mut Commands, root: Entity, f: &UiFonts, i: usize, name: &str, live: bool) {
    let line = role_color(Role::Line);
    let hot = role_color(Role::Hot);
    let dim = role_color(Role::Dim);
    let b = c
        .spawn(Node { width: percent(100), height: percent(100), flex_direction: FlexDirection::Column, row_gap: px(4), padding: UiRect { left: px(18), right: px(18), top: px(13), bottom: px(12) }, ..default() })
        .id();
    c.entity(root).add_child(b);
    let hdr = c
        .spawn((
            Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(8), padding: UiRect { bottom: px(7), ..default() }, margin: UiRect { bottom: px(3), ..default() }, border: UiRect { bottom: px(1), ..default() }, ..default() },
            BorderColor::all(with_alpha(line, 0.72)),
        ))
        .id();
    let sq = c.spawn((Node { width: px(6), height: px(6), ..default() }, BackgroundColor(role_color(Role::Fill)))).id();
    let t = c.spawn((Text::new(name.to_uppercase()), font(&f.ui600, 11.5, 2.76), TextColor(hot), LineHeight::Px(13.8))).id();
    let (right, main, sub) = tile_text(i, None);
    let r = c.spawn((TileText(i, 0), Text::new(right.to_uppercase()), font(&f.ui500, 10.5, 1.47), TextColor(dim), LineHeight::Px(13.0), Node { margin: UiRect { left: auto(), ..default() }, ..default() })).id();
    c.entity(hdr).add_children(&[sq, t, r]);
    let m = c.spawn((TileText(i, 1), Text::new(main.to_uppercase()), font(&f.ui600, 14.0, 1.4), TextColor(hot), LineHeight::Px(18.0))).id();
    let s = c.spawn((TileText(i, 2), Text::new(sub), font(&f.ui500, 12.5, 0.5), TextColor(line), LineHeight::Px(16.0))).id();
    let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
    let foot = c
        .spawn((Node { height: px(25), flex_direction: FlexDirection::Row, align_items: AlignItems::Center, border: UiRect { top: px(1), ..default() }, ..default() }, BorderColor::all(with_alpha(line, 0.13))))
        .id();
    let hint = c.spawn((TileFoot(i), Text::new("OPEN"), font(&f.ui700, 10.0, 2.2), TextColor(line), LineHeight::Px(10.0))).id();
    c.entity(foot).add_child(hint);
    if !live {
        let chip = c.spawn((Node { margin: UiRect { left: auto(), ..default() }, padding: UiRect { left: px(6), right: px(6), top: px(3), bottom: px(2) }, border: UiRect::all(px(1)), ..default() }, BorderColor::all(with_alpha(dim, 0.7)))).id();
        let ct = c.spawn((Text::new("SOON"), font(&f.ui700, 9.5, 1.9), TextColor(dim), LineHeight::Px(9.5))).id();
        c.entity(chip).add_child(ct);
        c.entity(foot).add_child(chip);
    }
    c.entity(b).add_children(&[hdr, m, s, sp, foot]);
}

/// Springs, the track, the tiles and their beams, every frame.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn update(
    sim: Res<crate::core::Sim>,
    mut orb: ResMut<Orb>,
    mut ctl: ResMut<CoreCtl>,
    mut sound: ResMut<crate::sound::Sound>,
    mut open: ResMut<crate::apps::Open>,
    mut toasts: ResMut<crate::live::Toasts>,
    mut sets: ParamSet<(
        Query<(&mut Transform, &mut Visibility), With<OrbTrack>>,
        Query<&mut Visibility, With<OrbMarker>>,
        Query<(&Tile, &mut Transform, &mut Visibility, &MeshMaterial3d<PanelMaterial>)>,
        Query<(&Beam, &mut Transform, &mut Visibility, &MeshMaterial3d<GlowMaterial>)>,
    )>,
    tilt: Query<&GlobalTransform, With<CoreTilt>>,
    cam: Query<&GlobalTransform, With<crate::CamCore>>,
    mut pmats: ResMut<Assets<PanelMaterial>>,
    mut gmats: ResMut<Assets<GlowMaterial>>,
    mut gslots: ResMut<GlowSlots>,
    mut feet: Query<(&TileFoot, &mut Text, &mut TextColor)>,
    (mut words, agents, live, stats, terms, git, ports, files): (Query<(&TileText, &mut Text), Without<TileFoot>>, Res<crate::core::Agents>, Res<crate::live::Live>, Res<crate::sysstats::SysStats>, Res<crate::term::Terms>, Res<crate::gitapp::GitApp>, Res<crate::ports::PortsApp>, Res<crate::filesapp::FilesApp>),
) {
    let dt = sim.dt;
    if orb.amount() > 0.01 {
        let st = TileState { tabs: terms.tabs.len(), agents: agents.total, working: agents.working(), needs: live.requests.len().max(agents.waiting()), tasks: live.titles.len(), docker: stats.get().docker.label(), docker_detail: stats.get().docker.detail(), git: git.caption(), ports: ports.listening(), files: files.caption(), prs: crate::prs::SUMMARY.lock().ok().and_then(|g| g.clone()) };
        for (t, mut tx) in &mut words {
            let (r, m, s) = tile_text(t.0, Some(&st));
            let want = match t.1 {
                0 => r.to_uppercase(),
                1 => m.to_uppercase(),
                _ => s,
            };
            if tx.0 != want {
                tx.0 = want;
            }
        }
    }
    let dt = crate::motion::dt(crate::motion::Pace::Big, dt);
    orb.since += sim.dt;
    orb.amt.t = if orb.open { 1.0 } else { 0.0 };
    orb.amt.step(dt);
    if !orb.dragging {
        orb.a.step(dt);
    }
    let amt = orb.amount();
    ctl.orb = amt;
    // fan-out: front first, alternating neighbours, back last (.16 + k·.075 s); fold-in reversed (k·.035 s)
    let front = (((-orb.a.x / STEP).round() as i32).rem_euclid(N as i32)) as usize;
    for (k, off) in FAN.iter().enumerate() {
        let i = (front + off) % N;
        let (tgt, delay) = if orb.open { (1.0, 0.16 + k as f32 * 0.075) } else { (0.0, (N - 1 - k) as f32 * 0.035) };
        let delay = crate::motion::stagger(delay);
        if orb.since >= delay && orb.em[i].t != tgt {
            orb.em[i].t = tgt;
            if tgt > 0.5 {
                sound.play_with("tick", 0.3, 0.0);
            }
        }
    }
    for i in 0..N {
        orb.em[i].step(dt);
        orb.kick[i].t = 0.0;
        orb.kick[i].step(dt);
    }
    if front != orb.front {
        orb.front = front;
        if amt > 0.5 {
            sound.play_with("tick", 0.45, 0.0);
        }
    }
    // a launch waits for its tile to land at the front
    if let Some(i) = orb.launch {
        if (orb.a.x - orb.a.t).abs() < 0.06 && front == i {
            orb.launch = None;
            if APPS[i].1 {
                open.app(i, &mut toasts);
                orb.set(false, &mut sound);
            } else {
                orb.kick[i].v += 7.0;
                open.app(i, &mut toasts);
            }
        }
    }
    let vis = if amt > 0.002 { Visibility::Inherited } else { Visibility::Hidden };
    let a = orb.a.x;
    for (mut tr, mut v) in &mut sets.p0() {
        tr.rotation = Quat::from_rotation_z(-a);
        v.set_if_neq(vis);
    }
    for mut v in &mut sets.p1() {
        v.set_if_neq(vis);
    }
    let Ok(tg) = tilt.single() else { return };
    let cam_z = cam.single().map(|g| g.translation().z).unwrap_or(1500.0);
    let t = sim.t;
    let mut beams: Vec<(usize, Vec3, Vec3, f32, f32)> = Vec::new();
    for (tile, mut tr, mut v, h) in &mut sets.p2() {
        let i = tile.0;
        let em = orb.em[i].x.max(0.0);
        let ec = clamp(em, 0.0, 1.0);
        let phi = i as f32 * STEP + a - (1.0 - ec) * 1.25;
        let bob = 4.0 * (1.25 * t + 1.7 * i as f32).sin();
        // beam top in core space; φ = 0 is the front (toward you, at the bottom of the tipped ring)
        let top_local = Vec3::new(R * phi.sin(), -R * phi.cos(), Z + H + bob);
        let pad_local = Vec3::new(R * phi.sin(), -R * phi.cos(), Z);
        let centre = tg.translation();
        let top = tg.transform_point(top_local);
        let pad = tg.transform_point(pad_local);
        let f = (phi.cos() + 1.0) / 2.0;
        let smooth = |a: f32, b: f32, x: f32| {
            let t = clamp((x - a) / (b - a), 0.0, 1.0);
            t * t * (3.0 - 2.0 * t)
        };
        // perspective, softened: nearer tiles grow less than they would (mag^-.42)
        let mag = cam_z / (cam_z - top.z).max(1.0);
        let sc = mag.powf(-0.42) * lerp(0.1, 1.0, smooth(0.0, 1.0, ec));
        let pos = centre.lerp(top + Vec3::Y * (TILE_H * sc / 2.0 + 4.0), ec);
        tr.translation = pos;
        tr.rotation = Quat::from_rotation_y(-phi.sin() * 22.0 * DEG + orb.kick[i].x * 0.05);
        tr.scale = Vec3::splat(sc);
        let op = (0.24 + 0.76 * smooth(0.12, 0.9, f)) * smooth(0.0, 0.4, em) * amt.max(ec);
        v.set_if_neq(if op > 0.004 { Visibility::Inherited } else { Visibility::Hidden });
        if let Some(m) = pmats.get(&h.0) {
            let draw = clamp(em, 0.0, 1.0);
            if m.u.size.w != op || m.u.q.x != draw {
                if let Some(mut m) = pmats.get_mut(&h.0) {
                    m.u.size.w = op;
                    m.u.q.x = draw;
                }
            }
        }
        beams.push((i, pad, top, op * smooth(0.55, 1.0, em) * 0.3, f));
    }
    for (beam, mut tr, mut v, h) in &mut sets.p3() {
        let Some(&(_, pad, top, alpha, _)) = beams.iter().find(|b| b.0 == beam.0) else { continue };
        let d = top - pad;
        let len = d.length().max(0.001);
        v.set_if_neq(if alpha > 0.004 { Visibility::Inherited } else { Visibility::Hidden });
        // a strip from the pad up to the tile, facing the camera (+Z) as much as it can
        let up = d / len;
        let side = up.cross(Vec3::Z).normalize_or(Vec3::X);
        let fwd = side.cross(up);
        tr.translation = pad;
        tr.rotation = Quat::from_mat3(&Mat3::from_cols(side, up, fwd));
        tr.scale = Vec3::new(TILE_W * 0.5 * 0.62, len, 1.0);
        let color = crate::theme::now_lin(Role::Fill);
        gslots.update(&mut gmats, &h.0, |u| {
            u.a.x = alpha;
            u.color = color;
        });
    }
    // the footer: "■ Enter to open" on the front tile
    for (ft, mut tx, mut c) in &mut feet {
        let (s, col) = if ft.0 == front { ("ENTER TO OPEN", role_color(Role::Hot)) } else { ("OPEN", role_color(Role::Line)) };
        if tx.0 != s {
            tx.0 = s.into();
        }
        c.set_if_neq(TextColor(col));
    }
}

/// A opens and closes; while open ← → spin, Enter opens the front app, Esc closes.
pub fn keys(
    mut ev: MessageReader<bevy::input::keyboard::KeyboardInput>,
    keys: Res<ButtonInput<KeyCode>>,
    focus: Res<Focus>,
    moment: Res<crate::moment::Moment>,
    mut orb: ResMut<Orb>,
    mut sound: ResMut<crate::sound::Sound>,
) {
    let mods = keys.any_pressed([KeyCode::SuperLeft, KeyCode::SuperRight, KeyCode::ControlLeft, KeyCode::ControlRight, KeyCode::AltLeft, KeyCode::AltRight]);
    for e in ev.read() {
        if e.state != bevy::input::ButtonState::Pressed || mods || focus.cmd || focus.terminal || moment.shown {
            continue;
        }
        match e.key_code {
            KeyCode::KeyA if !e.repeat => orb.toggle(&mut sound),
            KeyCode::ArrowRight if orb.open => orb.step(1.0),
            KeyCode::ArrowLeft if orb.open => orb.step(-1.0),
            KeyCode::Enter if orb.open => orb.launch_front(),
            KeyCode::Escape if orb.open => orb.set(false, &mut sound),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fan_reaches_every_tile_once() {
        let mut f = FAN.to_vec();
        assert_eq!(&f[..3], &[0, 1, N - 1]);
        f.sort();
        assert_eq!(f, (0..N).collect::<Vec<_>>());
        assert!((STEP * N as f32 - TAU).abs() < 1e-5);
    }
}
