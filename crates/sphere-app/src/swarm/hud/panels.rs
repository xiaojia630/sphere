//! The swarm's glass panels, drawn in the swarm's own atlas (`ATLAS`, resident while the view is open): the
//! mission strip (SPEC 1.2), Orbits (4.7), Run slots (4.8) and the focus panel (4.3). Their UI is rebuilt when
//! `Swarm.version` changes (and once a second for the focus panel's running time); they follow `Swarm.amt`.
use super::{now_secs, HudKeys};
use crate::appframe::{clip, AppClicks, AppPart, Click, PaneRoot, Ui};
use crate::fonts::UiFonts;
use crate::panel::*;
use crate::swarm::model::{self, Agent, Model, State};
use crate::swarm::{Mode, Swarm, ATLAS};
use crate::util::*;
use bevy::prelude::*;

#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Strip,
    Orbits,
    Slots,
    Focus,
}

/// On a panel's quad: where it rests (the springs in `update_panels` follow `home`, which `place` moves).
#[derive(Component)]
pub struct HudPanel {
    pub kind: Kind,
    base: Vec3,
    side: f32,
}

#[derive(Resource)]
pub struct Roots {
    strip: Entity,
    orbits: Entity,
    slots: Entity,
    focus: Entity,
}

/// A glyph that breathes (a stepped fade, so the atlas camera only wakes a few times a cycle).
#[derive(Component)]
pub struct Pulse {
    period: f32,
    lo: f32,
    base: Color,
    phase: f32,
}

pub const STRIP: (f32, f32) = (690.0, 42.0);
pub const ORBITS: (f32, f32) = (316.0, 232.0);
pub const SLOTS: (f32, f32) = (316.0, 290.0);
pub const FOCUS: (f32, f32) = (478.0, 470.0);
const ROWS_MAX: usize = 4;

#[allow(clippy::too_many_arguments)]
pub fn setup(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut images: ResMut<Assets<Image>>, (home, mut atlases): (Res<PanelAtlas>, ResMut<AppAtlases>), mut pmats: ResMut<Assets<PanelMaterial>>) {
    let mut atlas = Atlas::new(&mut commands, &mut images, home.scale * APP_SHARP, -17);
    let c = &mut commands;
    let mut roots = Vec::new();
    // (kind, size, stage anchor, side, z)
    let list: [(Kind, (f32, f32), Vec3, f32); 4] = [
        (Kind::Strip, STRIP, crate::homepanels::anchor('c', 800.0, 't', 16.0, STRIP.0, STRIP.1), 0.0),
        (Kind::Orbits, ORBITS, crate::homepanels::anchor('l', 54.0, 't', 92.0, ORBITS.0, ORBITS.1), 1.0),
        (Kind::Slots, SLOTS, crate::homepanels::anchor('r', 1546.0, 't', 70.0, SLOTS.0, SLOTS.1), -1.0),
        (Kind::Focus, FOCUS, crate::homepanels::anchor('l', 96.0, 'm', 500.0, FOCUS.0, FOCUS.1), 1.0),
    ];
    for (i, (kind, (w, h), at, side)) in list.into_iter().enumerate() {
        let ui = spawn_ui_panel(c, &mut atlas, w, h);
        let mut home = at;
        home.z = if kind == Kind::Focus { 150.0 } else { 6.0 };
        let e = spawn_panel_quad(c, &mut meshes, &mut pmats, &ui, &PanelSpec { id: PanelId::App, w, h, home, home_scale: 1.0, side, phase: 0.9 + i as f32 * 1.3, show_at: 0.0, unframed: false });
        c.entity(e).insert((HudPanel { kind, base: home, side }, AppPart { app: ATLAS, pane: Some(i as u8) }, PaneRoot(ui.root)));
        roots.push(ui.root);
    }
    c.insert_resource(Roots { strip: roots[0], orbits: roots[1], slots: roots[2], focus: roots[3] });
    atlases.add(ATLAS, atlas);
}

/// The panels follow how open the swarm is; Orbits and Run slots step aside and back while a focus or request is
/// up (SPEC 3.2: x -/+ 120, back 200, opacity x .1), the focus panel slides in from the left (SPEC 4.3).
pub fn place(swarm: Res<Swarm>, mut atlases: ResMut<AppAtlases>, mut q: Query<(&mut Panel, &HudPanel)>) {
    let on = swarm.open || swarm.amt.x > 0.001;
    atlases.want(ATLAS, on);
    let amt = swarm.amt.x.clamp(0.0, 1.0);
    let dim = swarm.dim.x.clamp(0.0, 1.0);
    let foc = swarm.focus_amt.x.clamp(0.0, 1.0);
    for (mut p, h) in &mut q {
        let (work, home, sc) = match h.kind {
            Kind::Strip => (amt, h.base, 1.0),
            Kind::Orbits | Kind::Slots => (amt * (1.0 - 0.9 * dim), h.base + Vec3::new(-h.side * 120.0 * dim, 0.0, -200.0 * dim), 1.0),
            // closed: x - 90, z - 110 (the work fade adds the rest of the push back), scale .94
            Kind::Focus => (amt * foc, h.base + Vec3::new(-90.0 * (1.0 - foc), 0.0, 150.0 * (1.0 - foc)), lerp(0.94, 1.0, foc)),
        };
        p.work = Some((work, 0.0));
        p.home = home;
        p.home_scale = sc;
    }
}

/// What the strip's dot click does: a needs agent opens its request, any other (not leaving) one its focus panel.
pub fn select(swarm: &mut Swarm, i: usize) {
    let Some(a) = swarm.model.agents.get(i) else { return };
    let mode = match a.state {
        State::Needs => Mode::Request(a.id.clone()),
        State::Merged | State::Stopped => return,
        _ => Mode::Focus(a.id.clone()),
    };
    if swarm.mode != mode {
        swarm.mode = mode;
        swarm.version += 1;
    }
}

/// Clicks on the swarm's panes: a strip dot selects its agent; a button's key goes to `HudKeys` for the view.
pub fn clicks(mut clicks: ResMut<AppClicks>, mut swarm: ResMut<Swarm>, mut keys: ResMut<HudKeys>) {
    for c in clicks.take(ATLAS) {
        match c {
            Click::Pick(i) if swarm.open => select(&mut swarm, i),
            Click::Key(k) if swarm.open => keys.0.push(k),
            _ => {}
        }
    }
}

/// Look tests: SPHERE_SWARM_FOCUS=<agent id> opens that agent's focus panel two seconds in (e.g. `rate`, `json`).
pub fn test_focus(sim: Res<crate::core::Sim>, mut swarm: ResMut<Swarm>, mut done: Local<bool>) {
    if *done || sim.t < 2.0 || !swarm.open {
        return;
    }
    *done = true;
    if let Ok(id) = std::env::var("SPHERE_SWARM_FOCUS") {
        if swarm.model.agent(&id).is_some() {
            swarm.mode = Mode::Focus(id);
            swarm.version += 1;
        }
    }
}

/// Rebuilds the panels' UI when the model or mode changed; the focus panel also each second.
#[allow(clippy::too_many_arguments)]
pub fn draw(sim: Res<crate::core::Sim>, args: Res<crate::Args>, swarm: Res<Swarm>, roots: Res<Roots>, f: Res<UiFonts>, mut commands: Commands, mut seen: Local<(u64, i64, Option<String>)>) {
    if !(swarm.open || swarm.amt.x > 0.001) {
        return;
    }
    let now = now_secs(&args, &sim);
    let ver = swarm.version.wrapping_add(1);
    let all = seen.0 != ver;
    let focus_id = match &swarm.mode {
        Mode::Focus(id) => Some(id.clone()),
        _ => None,
    };
    let focus = focus_id.is_some() && (all || seen.1 != now || seen.2 != focus_id);
    if !all && !focus {
        return;
    }
    let c = &mut commands;
    let ui = Ui::new(&f);
    let m = &swarm.model;
    if all {
        c.entity(roots.strip).despawn_children();
        build_strip(c, &ui, roots.strip, m, swarm.current());
        c.entity(roots.orbits).despawn_children();
        build_orbits(c, &ui, roots.orbits, m);
        c.entity(roots.slots).despawn_children();
        build_slots(c, &ui, roots.slots, m);
    }
    if focus {
        if let Some(a) = focus_id.as_deref().and_then(|id| m.agent(id)) {
            c.entity(roots.focus).despawn_children();
            build_focus(c, &ui, roots.focus, m, a, now);
        }
        seen.1 = now;
        seen.2 = focus_id;
    }
    seen.0 = ver;
}

/// Breathing glyphs.
pub fn pulse(sim: Res<crate::core::Sim>, mut q: Query<(&Pulse, &mut BackgroundColor)>) {
    for (p, mut bg) in &mut q {
        let s = 0.5 + 0.5 * ((sim.t + p.phase) * std::f32::consts::TAU / p.period).sin();
        let a = ((p.lo + (1.0 - p.lo) * s) * 6.0).round() / 6.0;
        bg.set_if_neq(BackgroundColor(with_alpha(p.base, a)));
    }
}

// ------------------------------------------------------------------ pieces

fn fill() -> Color {
    role_color(Role::Fill)
}
fn acc() -> Color {
    role_color(Role::Acc)
}
fn ok() -> Color {
    role_color(Role::Ok)
}
fn bad() -> Color {
    role_color(Role::Bad)
}
fn line() -> Color {
    role_color(Role::Line)
}
fn dim() -> Color {
    role_color(Role::Dim)
}
fn hot() -> Color {
    role_color(Role::Hot)
}

fn state_col(s: State) -> Color {
    match s {
        State::Needs => acc(),
        State::Done | State::Merged => ok(),
        State::Queued => dim(),
        _ => fill(),
    }
}

fn row(c: &mut Commands, gap: f32) -> Entity {
    c.spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(gap), ..default() }).id()
}

fn col(c: &mut Commands, gap: f32) -> Entity {
    c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(gap), ..default() }).id()
}

fn spacer(c: &mut Commands) -> Entity {
    c.spawn(Node { flex_grow: 1.0, ..default() }).id()
}

fn mono(ui: &Ui, c: &mut Commands, s: impl Into<String>, size: f32, color: Color) -> Entity {
    ui.text(c, s, &ui.f.mono, size, 0.0, color)
}

/// A square of `s` px.
fn square(c: &mut Commands, s: f32, color: Color) -> Entity {
    c.spawn((Node { width: px(s), height: px(s), flex_shrink: 0.0, ..default() }, BackgroundColor(color))).id()
}

/// A breathing square of `s` px.
fn pulsing(c: &mut Commands, s: f32, color: Color, period: f32, phase: f32) -> Entity {
    let e = square(c, s, color);
    c.entity(e).insert(Pulse { period, lo: 0.3, base: color, phase });
    e
}

/// The state's small glyph: a working square, a needs-you amber one, a ready diamond, a queued outline.
fn glyph(c: &mut Commands, s: State, size: f32, phase: f32) -> Entity {
    match s {
        State::Needs => {
            let e = square(c, size, acc());
            c.entity(e).insert((Pulse { period: 1.5, lo: 0.3, base: acc(), phase }, Node { width: px(size), height: px(size), border_radius: BorderRadius::MAX, flex_shrink: 0.0, ..default() }));
            e
        }
        State::Working => {
            let e = square(c, size, fill());
            c.entity(e).insert((Pulse { period: 2.6, lo: 0.55, base: fill(), phase }, Node { width: px(size), height: px(size), border_radius: BorderRadius::MAX, flex_shrink: 0.0, ..default() }));
            e
        }
        State::Done | State::Merged => {
            let e = square(c, size, ok());
            c.entity(e).insert(UiTransform { rotation: Rot2::degrees(45.0), scale: Vec2::splat(0.85), ..default() });
            e
        }
        _ => c.spawn((Node { width: px(size), height: px(size), border: UiRect::all(px(1)), border_radius: BorderRadius::MAX, flex_shrink: 0.0, ..default() }, BorderColor::all(with_alpha(line(), 0.75)))).id(),
    }
}

/// A button: label in tracked caps, filled or ghost; a click presses `key`.
fn button(c: &mut Commands, ui: &Ui, label: &str, color: Color, filled: bool, key: KeyCode) -> Entity {
    let b = c
        .spawn((
            Node { height: px(28), padding: UiRect::horizontal(px(14)), align_items: AlignItems::Center, border: UiRect::all(px(1)), ..default() },
            BorderColor::all(with_alpha(color, if filled { 1.0 } else { 0.55 })),
            BackgroundColor(if filled { color } else { with_alpha(line(), 0.05) }),
            Click::Key(key),
        ))
        .id();
    let t = ui.text(c, label.to_uppercase(), &ui.f.ui700, 10.0, 2.0, if filled { Color::BLACK } else { color });
    c.entity(b).add_child(t);
    b
}

// ------------------------------------------------------------------ strip (SPEC 1.2)

fn build_strip(c: &mut Commands, ui: &Ui, root: Entity, m: &Model, cur: Option<&str>) {
    let r = c.spawn(Node { width: percent(100), height: percent(100), flex_direction: FlexDirection::Row, align_items: AlignItems::Center, justify_content: JustifyContent::Center, column_gap: px(16), padding: UiRect { left: px(18), right: px(16), ..default() }, ..default() }).id();
    c.entity(root).add_child(r);
    let segs = row(c, 9.0);
    for (i, (n, label, cls)) in model::strip(m).into_iter().enumerate() {
        if i > 0 {
            let s = ui.text(c, "·", &ui.f.ui500, 11.0, 0.0, dim());
            c.entity(segs).add_child(s);
        }
        let zero = cls.is_some() && n == "0";
        let (nc, lc) = match cls {
            _ if zero => (dim(), dim()),
            Some(State::Needs) => (acc(), acc()),
            Some(_) => (ok(), ok()),
            None => (hot(), line()),
        };
        let seg = row(c, 6.0);
        let nt = mono(ui, c, n, 12.0, nc);
        let lt = ui.text(c, label.to_uppercase(), &ui.f.ui600, 11.0, 2.2, lc);
        c.entity(seg).add_children(&[nt, lt]);
        c.entity(segs).add_child(seg);
    }
    let cost = row(c, 6.0);
    let ct = mono(ui, c, model::strip_cost(m), 12.0, hot());
    let td = ui.text(c, "TODAY", &ui.f.ui500, 10.5, 1.0, dim());
    c.entity(cost).add_children(&[ct, td]);
    c.entity(segs).add_child(cost);
    // the dots: one per agent, clickable
    let dots = c.spawn((Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, padding: UiRect::left(px(14)), border: UiRect::left(px(1)), ..default() }, BorderColor::all(with_alpha(line(), 0.34)))).id();
    for (i, a) in m.agents.iter().enumerate() {
        if matches!(a.state, State::Merged | State::Stopped) {
            continue;
        }
        let hit = c.spawn((Node { width: px(17), height: px(24), align_items: AlignItems::Center, justify_content: JustifyContent::Center, ..default() }, Click::Pick(i))).id();
        let on = cur == Some(a.id.as_str());
        let g = glyph(c, a.state, 7.0, i as f32 * 0.8);
        if a.state == State::Done {
            c.entity(g).insert(UiTransform { rotation: Rot2::degrees(45.0), scale: Vec2::splat(if on { 1.4 } else { 1.0 }), ..default() });
        } else if on {
            c.entity(g).insert(UiTransform { scale: Vec2::splat(1.5), ..default() });
        }
        if on {
            let ring = c.spawn((Node { width: px(13), height: px(13), align_items: AlignItems::Center, justify_content: JustifyContent::Center, border: UiRect::all(px(1)), border_radius: BorderRadius::MAX, ..default() }, BorderColor::all(with_alpha(hot(), 0.8)))).id();
            c.entity(ring).add_child(g);
            c.entity(hit).add_child(ring);
        } else {
            c.entity(hit).add_child(g);
        }
        c.entity(dots).add_child(hit);
    }
    c.entity(r).add_children(&[segs, dots]);
}

// ------------------------------------------------------------------ Orbits (SPEC 4.7)

fn build_orbits(c: &mut Commands, ui: &Ui, root: Entity, m: &Model) {
    let body = ui.body(c, root);
    let hd = ui.header(c, "Orbits", &model::orbits_header(m).to_uppercase(), dim());
    let list = c.spawn(Node { flex_direction: FlexDirection::Column, padding: UiRect { left: px(18), right: px(18), top: px(2), ..default() }, ..default() }).id();
    c.entity(body).add_children(&[hd, list]);
    for (i, (key, sub, states)) in model::orbits(m).into_iter().enumerate().take(ROWS_MAX) {
        let r = c.spawn((Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(10), padding: UiRect::vertical(px(7)), border: UiRect::bottom(px(1)), ..default() }, BorderColor::all(with_alpha(line(), 0.15)))).id();
        // the ring's icon: a tilted ellipse and a centre dot
        let icon = c.spawn(Node { width: px(16), height: px(16), align_items: AlignItems::Center, justify_content: JustifyContent::Center, flex_shrink: 0.0, ..default() }).id();
        let ell = c.spawn((Node { width: px(14), height: px(5.2 + i as f32 * 1.8), border: UiRect::all(px(1)), border_radius: BorderRadius::MAX, ..default() }, BorderColor::all(line()), UiTransform { rotation: Rot2::degrees((i as f32 - 1.0) * 14.0), ..default() })).id();
        let dot = c.spawn((Node { position_type: PositionType::Absolute, left: px(6.8), top: px(6.8), width: px(2.4), height: px(2.4), border_radius: BorderRadius::MAX, ..default() }, BackgroundColor(line()))).id();
        c.entity(icon).add_children(&[ell, dot]);
        let names = col(c, 2.0);
        let n = ui.text(c, clip(&key, 18).to_uppercase(), &ui.f.ui600, 13.0, 2.1, hot());
        let s = mono(ui, c, clip(&sub, 28), 10.5, dim());
        c.entity(names).add_children(&[n, s]);
        let chips = c.spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(4), margin: UiRect::left(auto()), ..default() }).id();
        for (j, st) in states.into_iter().enumerate().take(9) {
            let g = glyph(c, st, 7.0, j as f32);
            // squares, not circles, on this panel
            if matches!(st, State::Working | State::Needs) {
                c.entity(g).insert(Node { width: px(7), height: px(7), flex_shrink: 0.0, ..default() });
            }
            c.entity(chips).add_child(g);
        }
        c.entity(r).add_children(&[icon, names, chips]);
        c.entity(list).add_child(r);
    }
}

// ------------------------------------------------------------------ Run slots (SPEC 4.8)

fn build_slots(c: &mut Commands, ui: &Ui, root: Entity, m: &Model) {
    let body = ui.body(c, root);
    let hd = ui.header(c, "Run slots", &model::slots_header(m).to_uppercase(), dim());
    let list = c.spawn(Node { flex_direction: FlexDirection::Column, padding: UiRect { left: px(18), right: px(18), top: px(2), ..default() }, ..default() }).id();
    c.entity(body).add_children(&[hd, list]);
    let (rows, queue) = model::slots(m);
    for (i, slot) in rows.into_iter().enumerate().take(ROWS_MAX) {
        let r = row(c, 10.0);
        let idx = c.spawn(Node { width: px(14), flex_shrink: 0.0, ..default() }).id();
        let it = mono(ui, c, (i + 1).to_string(), 11.0, dim());
        c.entity(idx).add_child(it);
        let (title, right, p, needs) = slot.unwrap_or(("Free".to_string(), "—".to_string(), 0.0, false));
        let free = right == "—";
        let name = ui.text(c, clip(&title, 24), &ui.f.ui600, 13.5, 0.3, if free { dim() } else if needs { acc() } else { hot() });
        let sp = spacer(c);
        let rt = c.spawn(Node { width: px(40), justify_content: JustifyContent::FlexEnd, ..default() }).id();
        let rtt = mono(ui, c, right, 11.0, if free { dim() } else if needs { acc() } else { hot() });
        c.entity(rt).add_child(rtt);
        c.entity(r).add_children(&[idx, name, sp, rt]);
        let bar = c.spawn((Node { height: px(2), margin: UiRect { left: px(24), top: px(5), bottom: px(7), ..default() }, ..default() }, BackgroundColor(with_alpha(line(), 0.14)))).id();
        let fl = c.spawn((Node { width: percent(p.clamp(0.0, 1.0) * 100.0), height: px(2), ..default() }, BackgroundColor(if needs { acc() } else { fill() }))).id();
        c.entity(bar).add_child(fl);
        let wrap = c.spawn(Node { flex_direction: FlexDirection::Column, padding: UiRect::top(px(4)), ..default() }).id();
        c.entity(wrap).add_children(&[r, bar]);
        c.entity(list).add_child(wrap);
    }
    // the queue
    let q = c.spawn((Node { flex_direction: FlexDirection::Column, row_gap: px(4), margin: UiRect::top(px(4)), padding: UiRect::top(px(10)), border: UiRect::top(px(1)), ..default() }, BorderColor::all(with_alpha(line(), 0.15)))).id();
    let head = if queue.is_empty() { "Queue empty".to_string() } else { format!("Queued · {}", queue.len()) };
    let ht = ui.text(c, head.to_uppercase(), &ui.f.ui600, 10.0, 3.4, line());
    c.entity(q).add_child(ht);
    for t in queue.iter().take(3) {
        let r = row(c, 8.0);
        let b = c.spawn((Node { width: px(5), height: px(5), border: UiRect::all(px(1)), flex_shrink: 0.0, ..default() }, BorderColor::all(with_alpha(line(), 0.7)))).id();
        let tt = ui.text(c, clip(t, 34), &ui.f.ui500, 12.0, 0.2, line());
        c.entity(r).add_children(&[b, tt]);
        c.entity(q).add_child(r);
    }
    if queue.len() > 3 {
        let more = mono(ui, c, format!("+{} more", queue.len() - 3), 10.0, dim());
        c.entity(q).add_child(more);
    }
    c.entity(list).add_child(q);
}

// ------------------------------------------------------------------ focus panel (SPEC 4.3)

fn label_row(c: &mut Commands, ui: &Ui, left: &str, right: String) -> Entity {
    let r = c.spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, justify_content: JustifyContent::SpaceBetween, margin: UiRect { top: px(10), bottom: px(5), ..default() }, ..default() }).id();
    let l = ui.text(c, left.to_uppercase(), &ui.f.ui600, 9.0, 2.6, dim());
    let rt = ui.text(c, right.to_uppercase(), &ui.f.ui600, 9.0, 2.2, line());
    c.entity(r).add_children(&[l, rt]);
    r
}

fn build_focus(c: &mut Commands, ui: &Ui, root: Entity, m: &Model, a: &Agent, now: i64) {
    let body = c.spawn(Node { width: percent(100), height: percent(100), flex_direction: FlexDirection::Column, padding: UiRect { left: px(18), right: px(18), top: px(16), bottom: px(14) }, ..default() }).id();
    c.entity(root).add_child(body);
    let sc = state_col(a.state);
    // header: chip, title, repo
    let hd = c.spawn((Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(12), padding: UiRect::bottom(px(9)), border: UiRect::bottom(px(1)), ..default() }, BorderColor::all(with_alpha(line(), 0.34)))).id();
    let needs = a.state == State::Needs;
    let chip = c.spawn((Node { padding: UiRect { left: px(6), right: px(6), top: px(3), bottom: px(2) }, border: UiRect::all(px(1)), align_items: AlignItems::Center, column_gap: px(5), flex_shrink: 0.0, ..default() }, BorderColor::all(if a.state == State::Working { line() } else { sc }), BackgroundColor(if needs { acc() } else { Color::NONE }))).id();
    if a.state == State::Working {
        let sq = pulsing(c, 5.0, fill(), 1.4, 0.0);
        c.entity(chip).add_child(sq);
    }
    let ct = ui.text(c, model::chip(a.state).to_uppercase(), &ui.f.ui600, 9.0, 1.6, if needs { Color::BLACK } else if a.state == State::Working { line() } else { sc });
    c.entity(chip).add_child(ct);
    let title = ui.text(c, clip(&a.title, 30).to_uppercase(), &ui.f.ui600, 14.0, 1.6, hot());
    let sp = spacer(c);
    let repo = m.repos.get(a.repo).map(|r| r.key.to_uppercase()).unwrap_or_default();
    let rt = ui.text(c, clip(&repo, 14), &ui.f.ui500, 10.0, 2.0, dim());
    c.entity(hd).add_children(&[chip, title, sp, rt]);
    // meta: branch / running / cost
    let meta = c.spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(18), margin: UiRect::top(px(10)), ..default() }).id();
    let running = if a.state == State::Queued { "queued".to_string() } else { model::fmt_dur(now - a.started_at) };
    for (k, v) in [("Branch", clip(&a.branch, 26)), ("Running", running), ("Cost", format!("${:.2}", a.cost))] {
        let p = row(c, 8.0);
        let kt = ui.text(c, k.to_uppercase(), &ui.f.ui600, 9.0, 1.8, dim());
        let vt = mono(ui, c, v, 11.5, hot());
        c.entity(p).add_children(&[kt, vt]);
        c.entity(meta).add_child(p);
    }
    c.entity(body).add_children(&[hd, meta]);
    // progress: 30 segments
    let pr = label_row(c, ui, "Progress", model::pct(a));
    let meter = c.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(2), height: px(7), ..default() }).id();
    let lit = if a.state == State::Queued { 0 } else { (a.p * 30.0).round() as usize };
    let bar = if a.state == State::Done { ok() } else if needs { acc() } else { fill() };
    for i in 0..30 {
        let s = c.spawn((Node { flex_grow: 1.0, flex_basis: px(0), height: px(7), ..default() }, BackgroundColor(if i < lit { bar } else { with_alpha(line(), 0.16) }))).id();
        c.entity(meter).add_child(s);
    }
    c.entity(body).add_children(&[pr, meter]);
    // steps: the last five
    let st = label_row(c, ui, "Steps", model::focus_steps_label(a));
    c.entity(body).add_child(st);
    if a.steps.is_empty() {
        let e = ui.text(c, if a.state == State::Queued { "Not started" } else { "—" }, &ui.f.ui500, 12.5, 0.3, dim());
        c.entity(body).add_child(e);
    }
    for (i, s) in a.steps.iter().take(5).enumerate() {
        let r = c.spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(8), height: px(21), ..default() }).id();
        let tw = c.spawn(Node { width: px(34), flex_shrink: 0.0, ..default() }).id();
        let tt = mono(ui, c, model::ago(now - s.at, i == 0), 10.0, dim());
        c.entity(tw).add_child(tt);
        let mk = c.spawn(Node { width: px(5), height: px(5), flex_shrink: 0.0, ..default() }).id();
        if i == 0 && a.state == State::Working {
            let q = pulsing(c, 5.0, hot(), 1.4, 0.0);
            c.entity(mk).add_child(q);
        }
        let tx = ui.text(c, clip(&s.text, 44), &ui.f.ui500, 12.5, 0.3, if i == 0 { hot() } else { line() });
        c.entity(r).add_children(&[tw, mk, tx]);
        c.entity(body).add_child(r);
    }
    // files touched
    let fl = label_row(c, ui, "Files touched", format!("{} file{}", a.files.len(), if a.files.len() == 1 { "" } else { "s" }));
    c.entity(body).add_child(fl);
    if a.files.is_empty() {
        let e = ui.text(c, "No files yet", &ui.f.ui500, 12.5, 0.3, dim());
        c.entity(body).add_child(e);
    }
    for f in a.files.iter().take(4) {
        let conflict = m.conflicts.iter().any(|k| k.file == f.path && (k.a == a.id || k.b == a.id));
        let editing = a.state == State::Working && a.editing.as_deref() == Some(f.path.as_str());
        let r = c.spawn((Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(8), height: px(22), margin: UiRect::bottom(px(3)), padding: UiRect { left: px(10), right: px(8), ..default() }, ..default() }, BackgroundColor(with_alpha(line(), 0.03)))).id();
        let bar = c.spawn((Node { position_type: PositionType::Absolute, left: px(0), top: px(0), bottom: px(0), width: px(2), ..default() }, BackgroundColor(if conflict { acc() } else if editing { fill() } else { with_alpha(line(), 0.34) }))).id();
        if editing && !conflict {
            c.entity(bar).insert(Pulse { period: 1.4, lo: 0.3, base: fill(), phase: 0.0 });
        }
        let nm = ui.text(c, clip(&f.path, 34), &ui.f.ui500, 12.5, 0.3, line());
        let sp = spacer(c);
        c.entity(r).add_children(&[bar, nm, sp]);
        if conflict {
            let t = ui.text(c, "CONFLICT", &ui.f.ui700, 8.5, 1.2, acc());
            c.entity(r).add_child(t);
        }
        let ad = mono(ui, c, format!("+{}", f.added), 11.0, ok());
        let de = mono(ui, c, format!("\u{2212}{}", f.removed), 11.0, bad());
        c.entity(r).add_children(&[ad, de]);
        c.entity(body).add_child(r);
    }
    // diff
    let dr = row(c, 8.0);
    c.entity(dr).insert(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(8), margin: UiRect { top: px(10), bottom: px(5), ..default() }, ..default() });
    let dl = ui.text(c, "DIFF", &ui.f.ui600, 9.0, 2.6, dim());
    c.entity(dr).add_child(dl);
    for w in model::diff_line(a).split(' ') {
        let col = match w.chars().next() {
            Some('+') => ok(),
            Some('\u{2212}') => bad(),
            _ => hot(),
        };
        let t = mono(ui, c, w, 11.0, col);
        c.entity(dr).add_child(t);
    }
    c.entity(body).add_child(dr);
    let (add, del) = a.files.iter().fold((0u64, 0u64), |s, f| (s.0 + f.added, s.1 + f.removed));
    let db = c.spawn(Node { flex_direction: FlexDirection::Row, height: px(4), column_gap: px(2), ..default() }).id();
    if add + del > 0 {
        for (v, colr) in [(add, ok()), (del, bad())] {
            if v > 0 {
                let s = c.spawn((Node { flex_grow: v as f32, flex_basis: px(0), height: px(4), ..default() }, BackgroundColor(colr))).id();
                c.entity(db).add_child(s);
            }
        }
    } else {
        let s = c.spawn((Node { flex_grow: 1.0, height: px(4), ..default() }, BackgroundColor(with_alpha(line(), 0.16)))).id();
        c.entity(db).add_child(s);
    }
    c.entity(body).add_child(db);
    // actions: Open in Code / Ask / Stop, or Merge first for a finished agent
    let ac = c.spawn((Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(12), margin: UiRect::top(px(14)), padding: UiRect::top(px(12)), border: UiRect::top(px(1)), ..default() }, BorderColor::all(with_alpha(line(), 0.15)))).id();
    if a.state == State::Done {
        let b1 = button(c, ui, "Merge", ok(), true, KeyCode::Enter);
        let b2 = button(c, ui, "Open in Code", hot(), false, KeyCode::KeyO);
        let b3 = button(c, ui, "Ask", hot(), false, KeyCode::KeyA);
        c.entity(ac).add_children(&[b1, b2, b3]);
    } else {
        let b1 = button(c, ui, "Open in Code", fill(), true, KeyCode::KeyO);
        let b2 = button(c, ui, "Ask", hot(), false, KeyCode::KeyA);
        let b3 = button(c, ui, "Stop", line(), false, KeyCode::KeyX);
        c.entity(ac).add_children(&[b1, b2, b3]);
    }
    let sp = spacer(c);
    let hint = c.spawn((Node { padding: UiRect { left: px(6), right: px(6), top: px(4), bottom: px(3) }, border: UiRect::all(px(1)), ..default() }, BorderColor::all(with_alpha(line(), 0.4)))).id();
    let ht = ui.text(c, "ESC BACK", &ui.f.ui700, 8.5, 1.4, dim());
    c.entity(hint).add_child(ht);
    c.entity(ac).add_children(&[sp, hint]);
    c.entity(body).add_child(ac);
}
