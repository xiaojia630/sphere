//! Claude sessions running outside Sphere, on the Agent swarm: ghost satellites on their repo's orbit. A ghost is an
//! outline only (two dashed rings, no body, no progress), with a small "in herdr" tag beside it. Hover shows its
//! title and state like any satellite; a click opens the take-over card. A session in a folder with no orbit of its
//! own sits on the next free orbit (drawn as a faint dashed ring), or the last orbit when all four are in use.
use super::{hash, mesh, pick_angle, ring_local, Gfx, Scene, SceneOut, NRING, NSAT, REPOS, H};
use crate::core::Sim;
use crate::fonts::UiFonts;
use crate::glow::{GlowMaterial, Kind};
use crate::panel::{font, with_alpha};
use crate::swarm::Swarm;
use crate::takeover::{key_of, match_repo, repo_name, state_label, title_of, where_label, Outside, TakeOver};
use crate::util::*;
use bevy::prelude::*;
use bevy::text::LineHeight;
use std::collections::HashMap;

/// Ghosts on screen at once.
pub const NG: usize = 6;

struct G {
    root: Entity,
    outer_e: Entity,
    inner: H,
    outer: H,
    /// the session it draws (its key)
    key: Option<String>,
    vis: Spring,
    hv: Spring,
    spin: f32,
}

#[derive(Resource)]
pub struct Ghosts {
    slots: Vec<G>,
    /// the faint orbit for sessions in a folder no agent works in
    ring_e: Entity,
    ring_mesh: Handle<Mesh>,
    ring_m: H,
    ring_for: Option<usize>,
    ring_vis: f32,
    /// key -> (orbit, angle)
    at: HashMap<String, (usize, f32)>,
    /// on screen now: (index in `Outside::list`, world position, fade)
    pub nodes: Vec<(usize, Vec3, f32)>,
    /// the ghost under the pointer (index in `Outside::list`)
    pub hover: Option<usize>,
}

#[derive(Component)]
pub struct GhostTag(usize);
#[derive(Component)]
pub struct GhostTip(usize);
#[derive(Component)]
pub struct GhostTipRoot;

pub fn build(app: &mut App) {
    app.add_systems(Startup, setup.after(super::spawn))
        .add_systems(Startup, setup_ui.after(crate::cmdbar::setup))
        .add_systems(Update, (update, hover, click, ui).chain().after(super::update));
}

fn setup(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut mats: ResMut<Assets<GlowMaterial>>, scene: Res<Scene>) {
    let mut g = Gfx { c: &mut commands, meshes: &mut meshes, mats: &mut mats };
    let mut slots = Vec::new();
    for _ in 0..NG {
        let root = g.c.spawn((Transform::default(), Visibility::Hidden)).id();
        let tf = Transform::default();
        let (_, inner) = g.part(root, mesh::geo(|g| crate::geo::dash_geo(g, 15.0, 0.9, 3.2, 3.4)), Kind::Ring, Role::Line, 0.8, tf);
        let (outer_e, outer) = g.part(root, mesh::geo(|g| crate::geo::dash_geo(g, 23.0, 0.8, 1.6, 4.6)), Kind::Ring, Role::Line, 0.4, tf);
        slots.push(G { root, outer_e, inner, outer, key: None, vis: Spring::new(0.0, 60.0, 13.0), hv: Spring::new(0.0, 90.0, 15.0), spin: 0.0 });
    }
    let ring_e = g.group(Some(scene.root), Transform::default());
    let (ring_part, ring_mesh, ring_m) = g.loose(mesh::geo(|g| crate::geo::dash_geo(g, REPOS[NRING - 1].rad, 0.8, 6.0, 9.0)), Role::Line);
    g.c.entity(ring_part).insert(Visibility::Inherited);
    g.c.entity(ring_e).add_child(ring_part);
    commands.insert_resource(Ghosts {
        slots,
        ring_e,
        ring_mesh,
        ring_m,
        ring_for: None,
        ring_vis: 0.0,
        at: HashMap::new(),
        nodes: Vec::new(),
        hover: None,
    });
}

fn setup_ui(mut commands: Commands, f: Res<UiFonts>, root: Res<crate::hud::OverlayRoot>) {
    for i in 0..NG {
        let t = commands
            .spawn((
                GhostTag(i),
                Text::new(""),
                font(&f.ui700, 9.5, 1.9),
                TextColor(Color::NONE),
                LineHeight::Px(10.0),
                TextLayout { linebreak: LineBreak::NoWrap, ..default() },
                Node { position_type: PositionType::Absolute, ..default() },
                Visibility::Hidden,
            ))
            .id();
        commands.entity(root.0).add_child(t);
    }
    let tip = commands
        .spawn((
            Node { position_type: PositionType::Absolute, flex_direction: FlexDirection::Column, padding: UiRect { left: px(11), right: px(12), top: px(7), bottom: px(8) }, border: UiRect::left(px(2)), ..default() },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.76)),
            BorderColor::all(role_color(Role::Fill)),
            Visibility::Hidden,
            GhostTipRoot,
        ))
        .id();
    let a = commands.spawn((GhostTip(0), Text::new(""), font(&f.ui700, 10.5, 2.73), TextColor(role_color(Role::Hot)), LineHeight::Px(13.0))).id();
    let b = commands.spawn((GhostTip(1), Text::new(""), font(&f.ui500, 12.5, 0.9), TextColor(role_color(Role::Line)), LineHeight::Px(16.0))).id();
    commands.entity(tip).add_children(&[a, b]);
    commands.entity(root.0).add_child(tip);
}

/// Places the ghosts on their orbits and fades them with the swarm.
#[allow(clippy::too_many_arguments)]
fn update(
    sim: Res<Sim>,
    swarm: Res<Swarm>,
    outside: Res<Outside>,
    scene: Res<Scene>,
    out: Res<SceneOut>,
    mut gh: ResMut<Ghosts>,
    mut tfs: Query<&mut Transform>,
    mut vis: Query<&mut Visibility>,
    mut mats: ResMut<Assets<GlowMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let amt = clamp(swarm.amt.x, 0.0, 1.0);
    let gh = &mut *gh;
    let hide_all = |gh: &mut Ghosts, vis: &mut Query<&mut Visibility>| {
        for s in &gh.slots {
            super::show(vis, s.root, false);
        }
        super::show(vis, gh.ring_e, false);
        gh.nodes.clear();
        gh.hover = None;
    };
    let Some((spos, srot, ss)) = out.sw else {
        hide_all(gh, &mut vis);
        return;
    };
    if amt < 0.001 || (outside.list.is_empty() && gh.slots.iter().all(|s| s.key.is_none())) {
        hide_all(gh, &mut vis);
        for s in &mut gh.slots {
            s.key = None;
            s.vis.set(0.0);
        }
        return;
    }
    let (dt, t) = (sim.dt, sim.t);
    let big = crate::motion::dt(crate::motion::Pace::Big, dt);
    let ui = crate::motion::dt(crate::motion::Pace::Ui, dt);
    let model = &swarm.model;
    let n_rings = model.repos.len().min(NRING);
    let free_ring = n_rings.min(NRING - 1);
    let paths: Vec<&str> = model.repos.iter().map(|r| r.path.as_str()).collect();
    // which slot draws which session: keep a session in its slot, give new ones a free slot
    let wanted: Vec<(usize, String)> = outside.list.iter().enumerate().take(NG).map(|(i, s)| (i, key_of(s))).collect();
    for s in gh.slots.iter_mut() {
        if s.key.as_ref().is_some_and(|k| !wanted.iter().any(|(_, w)| w == k)) {
            s.vis.t = 0.0;
            if s.vis.x < 0.01 {
                s.key = None;
            }
        }
    }
    for (_, k) in &wanted {
        if gh.slots.iter().any(|s| s.key.as_ref() == Some(k)) {
            continue;
        }
        if let Some(s) = gh.slots.iter_mut().find(|s| s.key.is_none()) {
            s.key = Some(k.clone());
            s.vis.set(0.0);
        }
    }
    // their orbits and angles
    let mut nodes = Vec::new();
    let mut need_free_ring = false;
    let (line, acc, dimc) = (crate::theme::now_lin(Role::Line), crate::theme::now_lin(Role::Acc), crate::theme::now_lin(Role::Dim));
    let dimk = if swarm.current().is_some() { 1.0 - 0.48 * clamp(swarm.dim.x, 0.0, 1.0) } else { 1.0 };
    for si in 0..NG {
        let Some(key) = gh.slots[si].key.clone() else {
            super::show(&mut vis, gh.slots[si].root, false);
            continue;
        };
        let found = wanted.iter().find(|(_, k)| *k == key).map(|(i, _)| *i);
        let sess = found.and_then(|i| outside.list.get(i));
        let want_ring = sess
            .map(|s| match s.cwd.as_deref().and_then(|c| match_repo(paths.iter().copied(), c)) {
                Some(r) => r % NRING,
                None => {
                    need_free_ring |= free_ring == n_rings && n_rings < NRING;
                    free_ring
                }
            })
            .or_else(|| gh.at.get(&key).map(|a| a.0))
            .unwrap_or(free_ring);
        let cur = gh.at.get(&key).copied();
        let (ring, ang) = match cur {
            Some((r, a)) if r == want_ring => (r, a),
            _ => {
                // the angles already taken on that orbit, by agents and by other ghosts
                let mut taken: Vec<f32> = scene.slots.iter().take(NSAT).filter(|o| o.alive && o.ring == want_ring).map(|o| o.ang).collect();
                taken.extend(gh.at.iter().filter(|(k, a)| **k != key && a.0 == want_ring).map(|(_, a)| a.1));
                (want_ring, pick_angle(&key, &taken))
            }
        };
        let ang = ang + REPOS[ring].w * 0.8 * dt;
        gh.at.insert(key.clone(), (ring, ang));
        let ph = hash(&key, 7) * TAU;
        let wob = (t * 0.45 + ph * 1.7).sin() * 0.012;
        let bob = (t * 0.9 + ph).sin() * 4.0;
        let pos = spos + srot * (ring_local(&REPOS[ring], ang + wob, bob) * ss);
        let hov = found.is_some() && gh.hover == found;
        let s = &mut gh.slots[si];
        if sess.is_some() {
            s.vis.t = 1.0;
        }
        s.vis.step(big);
        s.hv.t = hov as u8 as f32;
        s.hv.step(ui);
        s.spin += dt * 0.35;
        let fade = clamp(s.vis.x, 0.0, 1.0) * amt * dimk;
        let shown = fade > 0.004;
        super::show(&mut vis, s.root, shown);
        let hv = clamp(s.hv.x, 0.0, 1.0);
        let spin = s.spin;
        super::put(&mut tfs, s.root, |tr| {
            tr.translation = pos;
            tr.scale = Vec3::splat(1.25 + hv * 0.25);
        });
        super::put(&mut tfs, s.outer_e, |tr| tr.rotation = Quat::from_rotation_z(-spin));
        if shown {
            let waiting = sess.is_some_and(|x| x.state == "waiting");
            let col = if waiting { acc } else { line };
            let (inner, outer) = (s.inner.clone(), s.outer.clone());
            super::upd(&mut mats, &inner, |u| {
                u.color = col;
                u.a.x = if waiting { 1.0 } else { 0.8 } + hv * 0.2;
                u.b.x = fade;
            });
            super::upd(&mut mats, &outer, |u| {
                u.color = if waiting { acc } else { dimc };
                u.a.x = 0.42 + hv * 0.3;
                u.b.x = fade;
            });
            if let Some(i) = found {
                nodes.push((i, pos, fade));
            }
        }
    }
    gh.nodes = nodes;
    // the faint orbit for the folders no agent works in
    gh.ring_vis = lerp(gh.ring_vis, if need_free_ring { 1.0 } else { 0.0 }, 1.0 - (-4.0 * dt).exp());
    if need_free_ring && gh.ring_for != Some(free_ring) {
        gh.ring_for = Some(free_ring);
        if let Some(mut m) = meshes.get_mut(&gh.ring_mesh) {
            *m = mesh::geo(|g| crate::geo::dash_geo(g, REPOS[free_ring].rad, 0.8, 6.0, 9.0));
        }
        super::put(&mut tfs, gh.ring_e, |tr| tr.rotation = REPOS[free_ring].q());
    }
    let ring_on = gh.ring_vis > 0.01 && gh.ring_for.is_some();
    super::show(&mut vis, gh.ring_e, ring_on);
    let (rm, rv) = (gh.ring_m.clone(), gh.ring_vis * amt);
    super::upd(&mut mats, &rm, |u| {
        u.color = line;
        u.a.x = 0.16;
        u.b.x = rv;
    });
}

/// The ghost under the pointer.
fn hover(
    mut gh: ResMut<Ghosts>,
    swarm: Res<Swarm>,
    ms: Res<crate::mouse::MouseState>,
    take: Res<TakeOver>,
    cams: Query<(&Camera, &GlobalTransform), With<crate::CamCore>>,
) {
    let mut best: Option<(f32, usize)> = None;
    if let (true, Some(pos), Ok((cam, ctr)), true) = (swarm.open && swarm.hover.is_none(), ms.pos, cams.single(), take.card.is_none()) {
        let blocked = ms.dragging || matches!(ms.hit, Some(crate::mouse::Hit::Visor) | Some(crate::mouse::Hit::Panel(..)));
        if !blocked {
            for (i, p, fade) in &gh.nodes {
                if *fade < 0.3 {
                    continue;
                }
                let Ok(c) = cam.world_to_viewport(ctr, *p) else { continue };
                let r = 30.0;
                let d = c.distance(pos);
                if d <= r && best.is_none_or(|b| d < b.0) {
                    best = Some((d, *i));
                }
            }
        }
    }
    let h = best.map(|b| b.1);
    if gh.hover != h {
        gh.hover = h;
    }
}

/// A click on a ghost opens its card.
#[allow(clippy::too_many_arguments)]
fn click(
    buttons: Res<ButtonInput<MouseButton>>,
    time: Res<Time<Real>>,
    ms: Res<crate::mouse::MouseState>,
    gh: Res<Ghosts>,
    outside: Res<Outside>,
    mut take: ResMut<TakeOver>,
    bar: Res<crate::cmdbar::CmdBar>,
    mut sound: ResMut<crate::sound::Sound>,
    mut down: Local<Option<(Vec2, f32)>>,
) {
    let now = time.elapsed_secs();
    if buttons.just_pressed(MouseButton::Left) {
        *down = ms.pos.map(|p| (p, now));
    }
    if buttons.just_released(MouseButton::Left) {
        let (Some((p0, t0)), Some(p)) = (down.take(), ms.pos) else { return };
        if (p - p0).length() < 4.0 && now - t0 < 0.45 && !ms.dragging && !bar.open && take.card.is_none() {
            if let Some(i) = gh.hover {
                take.open_one(&outside.list, i);
                sound.play_with("open", 0.7, 0.0);
            }
        }
    }
}

/// The tags beside the ghosts, and the tooltip.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn ui(
    gh: Res<Ghosts>,
    swarm: Res<Swarm>,
    outside: Res<Outside>,
    ms: Res<crate::mouse::MouseState>,
    cams: Query<(&Camera, &GlobalTransform), With<crate::CamCore>>,
    win: Query<&Window, With<bevy::window::PrimaryWindow>>,
    mut tags: Query<(&GhostTag, &mut Node, &mut Visibility, &mut Text, &mut TextColor), (Without<GhostTipRoot>, Without<GhostTip>)>,
    mut tip: ParamSet<(Query<(&mut Node, &mut Visibility, &ComputedNode), With<GhostTipRoot>>, Query<(&GhostTip, &mut Text), Without<GhostTag>>)>,
) {
    let Ok((cam, ctr)) = cams.single() else { return };
    let line = role_color(Role::Line);
    let acc = role_color(Role::Acc);
    for (GhostTag(k), mut n, mut v, mut text, mut col) in &mut tags {
        let node = gh.nodes.get(*k).filter(|_| swarm.open);
        let Some((i, p, fade)) = node else {
            v.set_if_neq(Visibility::Hidden);
            continue;
        };
        let (Some(s), Ok(sp)) = (outside.list.get(*i), cam.world_to_viewport(ctr, *p)) else {
            v.set_if_neq(Visibility::Hidden);
            continue;
        };
        let want = where_label(&s.where_).to_uppercase();
        if text.0 != want {
            text.0 = want;
        }
        let base = if s.state == "waiting" { acc } else { line };
        col.set_if_neq(TextColor(with_alpha(base, 0.8 * fade)));
        let (l, t) = (px((sp.x + 26.0).round()), px((sp.y - 5.0).round()));
        if n.left != l || n.top != t {
            n.left = l;
            n.top = t;
        }
        v.set_if_neq(if *fade > 0.05 { Visibility::Inherited } else { Visibility::Hidden });
    }
    // the tooltip
    let h = gh.hover.and_then(|i| outside.list.get(i).map(|s| (s, i)));
    if let Some((s, _)) = h {
        let folder = repo_name(s.cwd.as_deref());
        let t2 = format!("{}{} · {}", if folder.is_empty() { String::new() } else { format!("{folder} · ") }, where_label(&s.where_), {
            let st = state_label(&s.state);
            if crate::takeover::takeable(s) {
                format!("{st} · click to take over")
            } else {
                format!("{st} · can't be told apart")
            }
        });
        for (k, mut t) in &mut tip.p1() {
            let want = if k.0 == 0 { title_of(s).to_uppercase() } else { t2.clone() };
            if t.0 != want {
                t.0 = want;
            }
        }
    }
    let size = win.single().map(|w| Vec2::new(w.width(), w.height())).unwrap_or(Vec2::new(1600.0, 1000.0));
    for (mut n, mut v, cn) in &mut tip.p0() {
        match (h, ms.pos) {
            (Some(_), Some(p)) => {
                let sz = cn.size() * cn.inverse_scale_factor();
                let at = crate::swarm::hud::tip_pos(p, sz, size.x);
                let (l, t) = (px(at.x.round()), px(at.y.round()));
                if n.left != l || n.top != t {
                    n.left = l;
                    n.top = t;
                }
                v.set_if_neq(Visibility::Inherited);
            }
            _ => {
                v.set_if_neq(Visibility::Hidden);
            }
        }
    }
}
