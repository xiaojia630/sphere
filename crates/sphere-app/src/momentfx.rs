//! The Agent moment's choreography (prototype: Agent moment): the core flares amber and draws back, an amber halo
//! rings the stack with tethers running to the card's corners, the other waiting requests orbit as satellites,
//! Send folds the card to a line and a comet streaks away, Later dims it into orbit, and when nothing is left the
//! halo turns green and Sphere says so. Sphere reads each request out as it rises.
use crate::core::{CoreCtl, CoreTilt};
use crate::fonts::UiFonts;
use crate::geo::*;
use crate::glow::{GlowMaterial, Kind, ATTR_HOLO};
use crate::live::{Live, Toasts};
use crate::moment::{waiting, Moment};
use crate::panel::*;
use crate::util::*;
use bevy::asset::RenderAssetUsages;
use bevy::mesh::PrimitiveTopology;
use bevy::prelude::*;
use bevy::text::LineHeight;
use sphere_proto::{RequestKind, RequestView};

const SATS: usize = 3;
pub const SAT_W: f32 = 240.0;
pub const SAT_H: f32 = 50.0;
const HALO_R: f32 = 440.0;

#[derive(Resource)]
pub struct MomentFx {
    pub amt: Spring,
    pub halo: Spring,
    /// the amber flare on the core, decaying e^-1.1t
    pub flare: f32,
    orb: f32,
    rise_seen: u64,
    spoke_at: Option<f32>,
    t: f32,
    /// the halo turns green when everything is answered
    ok: f32,
}

impl Default for MomentFx {
    fn default() -> Self {
        MomentFx { amt: Spring::new(0.0, 24.0, 9.5), halo: Spring::new(0.0, 26.0, 9.0), flare: 0.0, orb: 0.0, rise_seen: 0, spoke_at: None, t: 0.0, ok: 0.0 }
    }
}

#[derive(Component)]
pub struct Halo;
#[derive(Component)]
pub struct HaloSpin;
#[derive(Component)]
pub struct Tethers;
#[derive(Component)]
pub struct Streak;
#[derive(Component)]
pub struct Sat(pub usize);
#[derive(Resource)]
pub struct SatRoots(pub [Entity; SATS]);

fn lines(pos: Vec<[f32; 3]>, k: Vec<f32>) -> Mesh {
    let mut m = Mesh::new(PrimitiveTopology::LineList, RenderAssetUsages::default());
    let at: Vec<[f32; 4]> = k.into_iter().map(|k| [k, 0.0, 0.0, 0.0]).collect();
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
    m.insert_attribute(ATTR_HOLO, at);
    m
}

#[allow(clippy::too_many_arguments)]
pub fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut gmats: ResMut<Assets<GlowMaterial>>,
    mut atlas: ResMut<crate::panel::PanelAtlas>,
    mut pmats: ResMut<Assets<PanelMaterial>>,
    tilt: Query<Entity, With<CoreTilt>>,
) {
    let c = &mut commands;
    let Ok(tilt) = tilt.single() else { return };
    let mat = |gm: &mut Assets<GlowMaterial>, a: f32| {
        let mut m = GlowMaterial::new(Kind::Ring, crate::theme::now_lin(Role::Acc), a);
        m.u.a.y = 0.3;
        m.u.b.y = 6.0;
        gm.add(m)
    };
    // the halo, in core space just behind the time deck
    let halo = c.spawn((Halo, Transform::from_xyz(0.0, 0.0, -6.0), Visibility::Hidden)).id();
    c.entity(tilt).add_child(halo);
    let mut g = Geo::new();
    ring(&mut g, HALO_R, 1.7);
    ring_geo(&mut g, HALO_R, 20.0, RingOpt { k: 0.1, walls: 0, ..default() });
    for k in 0..4 {
        let a = (45.0 + 90.0 * k as f32) * DEG;
        ring_geo(&mut g, 426.0 + 13.0, 26.0, RingOpt { a0: a - 0.35 * DEG, a1: a + 0.35 * DEG, k: 0.85, ..default() });
    }
    let e = c.spawn((Mesh3d(meshes.add(g.into_mesh())), MeshMaterial3d(mat(&mut gmats, 0.0)), Transform::default())).id();
    let mut g = Geo::new();
    dash_geo(&mut g, 457.0, 1.0, 10.0, 9.0);
    let spin = c.spawn((HaloSpin, Mesh3d(meshes.add(g.into_mesh())), MeshMaterial3d(mat(&mut gmats, 0.0)), Transform::default())).id();
    c.entity(halo).add_children(&[e, spin]);
    // tethers: world space, rebuilt every frame
    let tm = meshes.add(lines(vec![[0.0; 3], [0.0; 3]], vec![0.0, 0.0]));
    let mut tmat = GlowMaterial::new(Kind::Line, crate::theme::now_lin(Role::Acc), 0.0);
    tmat.u.a.y = 0.0;
    c.spawn((Tethers, Mesh3d(tm), MeshMaterial3d(gmats.add(tmat)), Transform::default(), Visibility::Hidden, bevy::camera::visibility::NoFrustumCulling));
    // the streak: a comet tail (lines) and its head
    let sm = meshes.add(lines(vec![[0.0; 3], [0.0; 3]], vec![0.0, 0.0]));
    let mut smat = GlowMaterial::new(Kind::Line, crate::theme::now_lin(Role::Hot), 0.0);
    smat.u.a.y = 0.0;
    c.spawn((Streak, Mesh3d(sm), MeshMaterial3d(gmats.add(smat)), Transform::default(), Visibility::Hidden, bevy::camera::visibility::NoFrustumCulling));
    // satellites: small glass chips for the other waiting requests
    let mut roots = [Entity::PLACEHOLDER; SATS];
    for (i, r) in roots.iter_mut().enumerate() {
        let ui = spawn_ui_panel(c, &mut atlas, SAT_W, SAT_H);
        let e = spawn_panel_quad(c, &mut meshes, &mut pmats, &ui, &PanelSpec { id: PanelId::Sat, w: SAT_W, h: SAT_H, home: Vec3::ZERO, home_scale: 1.0, side: 0.0, phase: 0.0, show_at: 0.0, unframed: true });
        c.entity(e).remove::<Panel>().insert((Sat(i), Visibility::Hidden));
        *r = ui.root;
    }
    c.insert_resource(SatRoots(roots));
}

fn verb(r: &RequestView) -> &'static str {
    match r.info.kind {
        RequestKind::Question => "Answer it",
        RequestKind::Plan => "Approve it",
        _ => match r.info.tool.as_str() {
            "Bash" => "Run it",
            "Edit" | "MultiEdit" | "Write" => "Apply it",
            _ => "Allow it",
        },
    }
}

fn done_word(r: &RequestView) -> &'static str {
    match r.info.kind {
        RequestKind::Question => "Answered",
        RequestKind::Plan => "Approved",
        _ => match r.info.tool.as_str() {
            "Bash" => "Ran",
            "Edit" | "MultiEdit" | "Write" => "Applied",
            _ => "Allowed",
        },
    }
}

/// What Sphere says as a card rises: the task, the agent's own words, and the question.
fn intro(r: &RequestView, title: &str) -> String {
    let words = if r.info.summary.is_empty() { r.info.description.clone().unwrap_or_default() } else { r.info.summary.clone() };
    let words = words.trim().trim_end_matches('.').to_string();
    if words.is_empty() {
        format!("{title} needs you. {}?", verb(r))
    } else {
        format!("{title} needs you. {words}. {}?", verb(r))
    }
}

/// The flows (fold, streak, later, everything answered), the core's flare and draw-back, and the speech.
#[allow(clippy::too_many_arguments)]
pub fn flows(
    sim: Res<crate::core::Sim>,
    mut fx: ResMut<MomentFx>,
    mut m: ResMut<Moment>,
    live: Res<Live>,
    mut ctl: ResMut<CoreCtl>,
    mut sound: ResMut<crate::sound::Sound>,
    mut toasts: ResMut<Toasts>,
    mut say: ResMut<crate::homepanels::Say>,
    mut efx: ResMut<crate::fx::Fx>,
) {
    // the moment's whole sequence (rise, Send's fold and streak, Later, all done) runs on the Big clock
    let dt = crate::motion::dt(crate::motion::Pace::Big, sim.dt);
    fx.t += dt;
    let on = m.shown || m.holding.is_some() || m.all_done.is_some();
    fx.amt.t = if on { 1.0 } else { 0.0 };
    fx.amt.step(dt);
    fx.halo.t = if on { 1.0 } else { 0.0 };
    fx.halo.step(dt);
    fx.flare *= (-1.1 * dt).exp();
    fx.orb += TAU / 50.0 * sim.dt;
    ctl.moment = clamp(fx.amt.x, 0.0, 1.0);
    ctl.flare = fx.flare;
    // a card rose: the first one flares the core; each one is read out
    if m.rise_seq != fx.rise_seen {
        let first = fx.rise_seen == 0 || fx.amt.x < 0.2;
        fx.rise_seen = m.rise_seq;
        if first {
            fx.flare = 1.0;
            ctl.flash = 0.95;
            efx.shock(Role::Acc);
        }
        fx.spoke_at = Some(fx.t + if first { 1.35 } else { 0.4 });
        fx.ok = 0.0;
    }
    if let Some(at) = fx.spoke_at {
        if fx.t >= at {
            fx.spoke_at = None;
            if let Some((r, title)) = m.current.clone() {
                let words = intro(&r, &title);
                sound.speak(&words);
                say.say(words);
            }
        }
    }
    // Send: fold at .55 s, streak at 1.01, ok at 1.17, the next one (or the end) at 1.79
    let skip = std::mem::take(&mut m.skip);
    if let Some(t0) = m.fold {
        let t = if skip { t0.max(1.79) + dt } else { t0 + dt };
        m.fold = Some(t);
        if t0 < 0.55 && t >= 0.55 {
            sound.play_with("collapse", 0.45, 0.0);
        }
        if t0 < 1.01 && t >= 1.01 {
            sound.play_with("whoosh", 0.7, 5.0);
            ctl.flash = 0.5;
            m.streak = Some(0.0);
        }
        if t0 < 1.17 && t >= 1.17 {
            efx.shock(Role::Ok);
            ctl.flash = 0.4;
            if let Some((r, title)) = &m.holding {
                toasts.push(format!("{} · {}", done_word(r), title));
            }
        }
        if t >= 1.79 {
            m.fold = None;
            next(&mut m, &live, &mut sound, &mut fx);
        }
    }
    if let Some(s) = m.streak {
        let s = s + dt;
        m.streak = if s < 1.0 { Some(s) } else { None };
    }
    // Later: dims, then flies into orbit; the next rises
    if let Some(t0) = m.later {
        let t = if skip { t0.max(0.45) + dt } else { t0 + dt };
        m.later = Some(t);
        if t0 == 0.0 {
            sound.play_with("discard", 0.5, 0.0);
            if let Some((_, title)) = &m.holding {
                toasts.push(format!("Later · {title}"));
            }
        }
        if t >= 0.45 {
            m.later = None;
            next(&mut m, &live, &mut sound, &mut fx);
        }
    }
    // everything answered: green, said, then home
    if let Some(t0) = m.all_done {
        let t = t0 + dt;
        m.all_done = Some(t);
        fx.ok = (fx.ok + dt / 0.5).min(1.0);
        if t >= 2.9 {
            m.all_done = None;
            m.shown = false;
            m.version += 1;
            sound.play_with("collapse", 0.7, 0.0);
        }
    }
}

fn next(m: &mut Moment, live: &Live, sound: &mut crate::sound::Sound, fx: &mut MomentFx) {
    m.holding = None;
    m.version += 1;
    if crate::moment::pending_count(live, m) == 0 {
        m.all_done = Some(0.0);
        sound.play("done");
        let words = if waiting(live, m).is_empty() { "That's everything. Nothing needs you right now." } else { "That's everything for now. N brings back what you put off." };
        sound.speak(words);
        fx.ok = 0.0;
    } else {
        m.index = 0;
        m.rise_seq += 1;
        sound.play_with("open", 0.8, 0.0);
    }
}

/// N opens and closes the moment (with nothing waiting it says so). While the swarm is open N is its own (the
/// next agent that needs you, `swarm::input`).
#[allow(clippy::too_many_arguments)]
pub fn keys(
    mut ev: MessageReader<bevy::input::keyboard::KeyboardInput>,
    keys: Res<ButtonInput<KeyCode>>,
    focus: Res<Focus>,
    mut m: ResMut<Moment>,
    live: Res<Live>,
    mut sound: ResMut<crate::sound::Sound>,
    mut toasts: ResMut<Toasts>,
    swarm: Res<crate::swarm::Swarm>,
) {
    let mods = keys.any_pressed([KeyCode::SuperLeft, KeyCode::SuperRight, KeyCode::ControlLeft, KeyCode::ControlRight, KeyCode::AltLeft, KeyCode::AltRight]);
    for e in ev.read() {
        if e.state != bevy::input::ButtonState::Pressed || mods || focus.cmd || focus.terminal || e.repeat || m.editing || e.key_code != KeyCode::KeyN || swarm.open {
            continue;
        }
        if m.shown {
            m.shown = false;
            m.version += 1;
            sound.play_with("collapse", 0.7, 0.0);
        } else if waiting(&live, &m).is_empty() {
            toasts.push("Nothing needs you right now");
        } else {
            m.shown = true;
            m.rise_seq += 1;
            m.version += 1;
            sound.play_with("whoosh", 0.7, 0.0);
            sound.play_with("open", 0.8, 0.0);
        }
    }
}

/// The halo, the tethers, the streak and the satellites, every frame.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn draw(
    sim: Res<crate::core::Sim>,
    fx: Res<MomentFx>,
    m: Res<Moment>,
    live: Res<Live>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut gmats: ResMut<Assets<GlowMaterial>>,
    mut pmats: ResMut<Assets<PanelMaterial>>,
    card: Query<(&Panel, &GlobalTransform)>,
    root: Query<&GlobalTransform, With<crate::core::CoreRoot>>,
    tiltq: Query<&GlobalTransform, With<CoreTilt>>,
    mut sets: ParamSet<(
        Query<(&mut Visibility, &Children), With<Halo>>,
        Query<&mut Transform, With<HaloSpin>>,
        Query<(&Mesh3d, &MeshMaterial3d<GlowMaterial>, &mut Visibility), With<Tethers>>,
        Query<(&Mesh3d, &MeshMaterial3d<GlowMaterial>, &mut Visibility), With<Streak>>,
        Query<(&Sat, &mut Transform, &mut Visibility, &MeshMaterial3d<PanelMaterial>)>,
    )>,
    halo_mats: Query<&MeshMaterial3d<GlowMaterial>>,
    cam: Query<&GlobalTransform, With<crate::CamPanels>>,
) {
    let t = sim.t;
    let h = clamp(fx.halo.x * 1.25, 0.0, 1.0);
    // nothing to draw while the moment is away (keep the halo, tethers and satellites hidden, no mesh rebuilds)
    if h < 0.002 && m.streak.is_none() {
        for (mut v, _) in &mut sets.p0() {
            v.set_if_neq(Visibility::Hidden);
        }
        for (_, _, mut v) in &mut sets.p2() {
            v.set_if_neq(Visibility::Hidden);
        }
        for (_, _, mut v) in &mut sets.p3() {
            v.set_if_neq(Visibility::Hidden);
        }
        for (_, _, mut v, _) in &mut sets.p4() {
            v.set_if_neq(Visibility::Hidden);
        }
        return;
    }
    let acc = crate::theme::now_lin(Role::Acc);
    let ok = crate::theme::now_lin(Role::Ok);
    let col = acc.lerp(ok, fx.ok);
    let pulse = 0.5 + 0.5 * (3.1 * t).sin();
    let mut kids = Vec::new();
    for (mut v, ch) in &mut sets.p0() {
        v.set_if_neq(if h > 0.002 { Visibility::Inherited } else { Visibility::Hidden });
        kids.extend(ch.iter());
    }
    for k in kids {
        if let Ok(hm) = halo_mats.get(k) {
            if let Some(mut g) = gmats.get_mut(&hm.0) {
                g.u.color = col;
                g.u.a.x = h * (0.7 + 0.3 * pulse);
                g.u.a.z = h;
            }
        }
    }
    for mut tr in &mut sets.p1() {
        tr.rotation = Quat::from_rotation_z(-0.13 * t);
    }
    // the card's corners, from its panel (the Moment is the only Panel with PanelId::Moment)
    let Some((cp, cg)) = card.iter().find(|(p, _)| p.id == PanelId::Moment) else {
        return;
    };
    let (hw, hh) = (cp.w / 2.0, cp.h / 2.0);
    let corners = [Vec3::new(-hw, hh, 0.0), Vec3::new(hw, hh, 0.0), Vec3::new(hw, -hh, 0.0), Vec3::new(-hw, -hh, 0.0)].map(|v| cg.transform_point(v));
    let Ok(tg) = tiltq.single() else { return };
    // halo ticks at 315°, 45°, 135°, 225° (clockwise from 12) pair with the card's top-left, top-right, bottom-right, bottom-left
    let ticks = [315.0f32, 45.0, 135.0, 225.0].map(|a| tg.transform_point(Vec3::new(HALO_R * (a * DEG).sin(), HALO_R * (a * DEG).cos(), -6.0)));
    let card_op = clamp(cp.op.x, 0.0, 1.0);
    let rise = clamp((cp.sc.x - 0.1) / 0.85, 0.0, 1.0);
    let flow = lerp(3.2, 0.55, rise);
    let mut pos = Vec::new();
    let mut ks = Vec::new();
    for (a, b) in ticks.iter().zip(corners.iter()) {
        let d = *b - *a;
        let len = d.length().max(1.0);
        let dash = 11.0 / len;
        let off = (t * flow * 0.08) % (2.0 * dash);
        let mut u = -off;
        while u < 1.0 {
            let (u0, u1) = (u.max(0.0), (u + dash).min(1.0));
            if u1 > u0 {
                pos.push((*a + d * u0).to_array());
                pos.push((*a + d * u1).to_array());
                ks.push(lerp(0.66, 0.32, u0));
                ks.push(lerp(0.66, 0.32, u1));
            }
            u += 2.0 * dash;
        }
        // past the corner the tether carries on as a fading ray (≤ .34 of its length, ≤ 130)
        let ext = (0.34 * len).min(130.0);
        let dir = d / len;
        let n = 8;
        for k in 0..n {
            let (e0, e1) = (k as f32 / n as f32, (k + 1) as f32 / n as f32);
            pos.push((*b + dir * ext * e0).to_array());
            pos.push((*b + dir * ext * e1).to_array());
            ks.push(0.5 * (1.0 - e0));
            ks.push(0.5 * (1.0 - e1));
        }
    }
    for (mh, gm, mut v) in &mut sets.p2() {
        let a = h * card_op;
        v.set_if_neq(if a > 0.01 { Visibility::Inherited } else { Visibility::Hidden });
        if let Some(mut mesh) = meshes.get_mut(&mh.0) {
            *mesh = lines(pos.clone(), ks.clone());
        }
        if let Some(mut g) = gmats.get_mut(&gm.0) {
            g.u.color = col.lerp(crate::theme::now_lin(Role::Hot), 0.3);
            g.u.a.x = a;
        }
    }
    // the streak: from the card's centre off the top right, a tail behind a bright head
    let centre = cg.translation();
    for (mh, gm, mut v) in &mut sets.p3() {
        let Some(s) = m.streak else {
            v.set_if_neq(Visibility::Hidden);
            continue;
        };
        v.set_if_neq(Visibility::Inherited);
        let to = centre + Vec3::new(1400.0, 900.0, -60.0);
        let lift = Vec3::new(0.0, 90.0, 14.0);
        let at = |u: f32| {
            let u = clamp(u, 0.0, 1.0);
            let a = centre.lerp(to, u);
            a + lift * (4.0 * u * (1.0 - u))
        };
        let head = (s.powf(1.75) * 1.32).min(1.0);
        let tail = (head - 0.3).max(0.0);
        let mut pos = Vec::new();
        let mut ks = Vec::new();
        let n = 24;
        for i in 0..n {
            let (u0, u1) = (lerp(tail, head, i as f32 / n as f32), lerp(tail, head, (i + 1) as f32 / n as f32));
            let rel = (i + 1) as f32 / n as f32;
            pos.push(at(u0).to_array());
            pos.push(at(u1).to_array());
            ks.push(rel.powf(1.5));
            ks.push(rel.powf(1.5));
        }
        if let Some(mut mesh) = meshes.get_mut(&mh.0) {
            *mesh = lines(pos, ks);
        }
        if let Some(mut g) = gmats.get_mut(&gm.0) {
            g.u.color = ok.lerp(crate::theme::now_lin(Role::Hot), 0.5);
            g.u.a.x = 1.0 - clamp((s - 0.8) / 0.2, 0.0, 1.0);
        }
    }
    // satellites: the other waiting requests on a tilted ellipse around the core, brighter in front
    let c = root.single().map(|g| g.translation()).unwrap_or(Vec3::ZERO);
    let eye = cam.single().map(|g| g.translation()).unwrap_or(Vec3::new(0.0, 0.0, 1500.0));
    let w = waiting(&live, &m);
    let others: Vec<usize> = (0..w.len()).filter(|i| *i != m.index).collect();
    let amt = clamp(fx.amt.x, 0.0, 1.0);
    for (sat, mut tr, mut v, mh) in &mut sets.p4() {
        let i = sat.0;
        let show = i < others.len() && m.shown && m.all_done.is_none();
        let a = -0.4 + i as f32 * TAU / others.len().max(1) as f32 + fx.orb;
        let p = c + Vec3::new(560.0 * a.cos(), -330.0 * a.sin(), 210.0 * a.sin() + 40.0);
        tr.translation = p;
        tr.scale = Vec3::ONE;
        // a chip that would sit over the card on screen hides (in front of it or seen through the glass)
        let over = covers_card(eye, p, &corners);
        let op = if show && !over { amt * (0.42 + 0.58 * (a.sin() + 1.0) / 2.0) } else { 0.0 };
        v.set_if_neq(if op > 0.004 { Visibility::Inherited } else { Visibility::Hidden });
        if let Some(mm) = pmats.get(&mh.0) {
            if mm.u.size.w != op {
                if let Some(mut mm) = pmats.get_mut(&mh.0) {
                    mm.u.size.w = op;
                }
            }
        }
    }
}

/// Where `p` lands on screen, in world units on the plane z = 0, for the camera at `eye` looking down -z.
fn on_screen(eye: Vec3, p: Vec3) -> Vec2 {
    let k = eye.z / (eye.z - p.z).max(1.0);
    eye.truncate() + (p.truncate() - eye.truncate()) * k
}

/// Whether a satellite chip centred at `p` would cover any of the card (its four corners) on screen, with room
/// to spare so its label never touches the card's edge or the hint in its corner.
fn covers_card(eye: Vec3, p: Vec3, card: &[Vec3; 4]) -> bool {
    const MARGIN: f32 = 18.0;
    let pts = card.map(|c| on_screen(eye, c));
    let (lo, hi) = pts.iter().fold((Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)), |(lo, hi), q| (lo.min(*q), hi.max(*q)));
    let half = Vec2::new(SAT_W, SAT_H) / 2.0;
    let (a, b) = (on_screen(eye, p - half.extend(0.0)), on_screen(eye, p + half.extend(0.0)));
    a.x < hi.x + MARGIN && b.x > lo.x - MARGIN && a.y < hi.y + MARGIN && b.y > lo.y - MARGIN
}

/// The satellites' text: title and where it came from.
pub fn draw_sats(mut commands: Commands, m: Res<Moment>, live: Res<Live>, roots: Option<Res<SatRoots>>, f: Res<UiFonts>, mut seen: Local<u64>) {
    let Some(roots) = roots else { return };
    if *seen == m.version + live.version * 1000 {
        return;
    }
    *seen = m.version + live.version * 1000;
    let w = waiting(&live, &m);
    let others: Vec<&RequestView> = (0..w.len()).filter(|i| *i != m.index).map(|i| w[i]).collect();
    for (i, root) in roots.0.iter().enumerate() {
        commands.entity(*root).despawn_children();
        let Some(r) = others.get(i) else { continue };
        let line = role_color(Role::Line);
        let b = commands
            .spawn((
                Node { width: percent(100), height: percent(100), flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(10), padding: UiRect { left: px(10), right: px(10), ..default() }, ..default() },
                BackgroundGradient(vec![Gradient::Linear(LinearGradient::to_right(vec![ColorStop::percent(Color::srgba(0.0, 0.0, 0.0, 0.7), 0.0), ColorStop::percent(Color::NONE, 100.0)]))]),
            ))
            .id();
        let dot = commands.spawn((Node { width: px(7), height: px(7), border_radius: BorderRadius::MAX, flex_shrink: 0.0, ..default() }, BackgroundColor(role_color(Role::Acc)))).id();
        let col = commands.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(2), ..default() }).id();
        let mut title = live.title(&r.info.task).to_uppercase();
        if title.chars().count() > 24 {
            title = title.chars().take(23).collect::<String>() + "…";
        }
        let tt = commands.spawn((Text::new(title), font(&f.ui700, 11.0, 2.42), TextColor(role_color(Role::Hot)), LineHeight::Px(13.0))).id();
        let st = commands.spawn((Text::new(format!("{} · needs you", r.info.tool)), font(&f.ui500, 12.0, 0.5), TextColor(line), LineHeight::Px(14.0))).id();
        commands.entity(col).add_children(&[tt, st]);
        commands.entity(b).add_children(&[dot, col]);
        commands.entity(*root).add_child(b);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EYE: Vec3 = Vec3::new(0.0, 0.0, 1500.0);

    fn card() -> [Vec3; 4] {
        let (hw, hh, z) = (crate::moment::CARD_W / 2.0, crate::moment::CARD_H / 2.0, 230.0);
        [Vec3::new(-hw, hh, z), Vec3::new(hw, hh, z), Vec3::new(hw, -hh, z), Vec3::new(-hw, -hh, z)]
    }

    #[test]
    fn a_chip_behind_the_glass_still_counts_as_over_the_card() {
        // behind the card and near its bottom-right corner, where the "for the others" hint sits: the old check
        // let chips this far back show through the glass
        let hw = crate::moment::CARD_W / 2.0;
        let hh = crate::moment::CARD_H / 2.0;
        assert!(covers_card(EYE, Vec3::new(hw, -hh, -150.0), &card()));
        // just past the top edge, the label straddling it
        assert!(covers_card(EYE, Vec3::new(-180.0, hh + 30.0, -100.0), &card()));
    }

    #[test]
    fn a_chip_well_clear_of_the_card_shows() {
        let hw = crate::moment::CARD_W / 2.0;
        assert!(!covers_card(EYE, Vec3::new(hw + SAT_W, 0.0, 200.0), &card()));
        assert!(!covers_card(EYE, Vec3::new(-hw - SAT_W, 100.0, -150.0), &card()));
    }
}
