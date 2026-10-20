//! The swarm's panels and labels (SPEC sections 1.2, 1.3, 2.4, 4): the mission strip, Orbits, Run slots and the
//! focus panel (glass panels in the swarm's own atlas, `hud/panels.rs`), and the screen-space overlay: satellite
//! label plates, ring labels, conflict chips, step callouts and the tooltip, placed every frame from `SceneOut`.
//!
//! Clicks. The panels' buttons carry `Click::Key` and the strip's dots `Click::Pick(i)` (the i-th agent of
//! `Swarm.model.agents`). `panels::clicks` takes the swarm's clicks (`AppClicks::take(ATLAS)`): a dot selects its
//! agent itself (`Swarm.mode` = Focus, or Request for one that needs you; `version += 1`), and a button's key lands in
//! `HudKeys`, which the integration drains and handles as if the key were pressed (the same code as the keyboard):
//!   Open in Code = KeyO, Ask = KeyA, Stop = KeyX, Merge = Enter (the focus panel of a finished agent).
//!
//! Look tests: SPHERE_SWARM_FOCUS=<agent id> (e.g. `rate`, `json`) opens that agent's focus panel two seconds in;
//! SPHERE_SWARM_HOVER=<agent id> hovers it from 2.5 s (tooltip and step callouts, at its node when there is no pointer).
pub mod panels;

use super::scene::SceneOut;
use super::{Mode, Swarm};
use crate::fonts::UiFonts;
use crate::panel::{font, with_alpha, Panel};
use crate::swarm::model::{self, State};
use crate::util::*;
use crate::CamCore;
use bevy::prelude::*;
use bevy::text::LineHeight;

/// Where the HUD is on screen, for hit-testing (the integration uses it for clicks and hover on labels).
#[derive(Resource, Default)]
pub struct HudOut {
    /// every satellite label that is showing (opacity over .3): agent id and its plate's screen rect (px)
    pub labels: Vec<(String, Rect)>,
    /// the open focus panel and the request card (screen rects, px), for hiding what sits under them
    pub panels: Vec<Rect>,
}

/// Keys the panels' buttons press (see the top of this file); the integration drains it.
#[derive(Resource, Default)]
pub struct HudKeys(pub Vec<KeyCode>);

/// "Now" in unix seconds: the demo's fixed clock offline, the real one otherwise.
pub fn now_secs(args: &crate::Args, sim: &crate::core::Sim) -> i64 {
    if args.offline() {
        model::DEMO_NOW + sim.t as i64
    } else {
        web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
    }
}

pub fn build(app: &mut App) {
    app.init_resource::<HudOut>().init_resource::<HudKeys>();
    app.add_systems(Startup, (panels::setup, setup).chain().after(crate::mouse::setup_reticle));
    app.add_systems(Update, (panels::place, panels::test_focus, panels::draw, panels::pulse).chain().before(crate::panel::update_panels));
    app.add_systems(Update, panels::clicks.after(crate::appframe::route_clicks));
    app.add_systems(Update, (overlay, fade).chain().after(super::scene::update).after(crate::panel::update_panels));
}

// ------------------------------------------------------------------ pure layout math

/// A label's width on the stage (SPEC 2.4).
pub const LABEL_W: f32 = 268.0;

/// Which side of its node a label sits (+1 right, -1 left): right by default, left once the node is more than 80 px
/// left of the core, back to the right once it is more than 80 px right of it (hysteresis).
pub fn side_for(prev: f32, dx: f32) -> f32 {
    let s = if prev == 0.0 { 1.0 } else { prev };
    if s > 0.0 && dx < -80.0 {
        -1.0
    } else if s < 0.0 && dx > 80.0 {
        1.0
    } else {
        s
    }
}

/// Where a label's centre wants to be: beside its node (`k` px per world unit at its depth, `hv` hover 0..1).
pub fn label_target(node: Vec2, side: f32, w: f32, k: f32, hv: f32) -> Vec2 {
    let gap = 27.0 * k * (1.0 + hv * 0.1) + 10.0;
    Vec2::new(node.x + side * (w / 2.0 + gap), node.y - 2.0 * k)
}

/// Keeps a box of `size` centred at `c` inside `safe`.
pub fn clamp_box(c: Vec2, size: Vec2, safe: Rect) -> Vec2 {
    let h = size / 2.0;
    Vec2::new(c.x.clamp(safe.min.x + h.x, (safe.max.x - h.x).max(safe.min.x + h.x)), c.y.clamp(safe.min.y + h.y, (safe.max.y - h.y).max(safe.min.y + h.y)))
}

/// Pushes overlapping boxes apart (centre, size), `iters` rounds (SPEC 2.4: gaps 6 and 4 px; vertical when the
/// vertical overlap is the smaller, else horizontal), then clamps each into `safe`.
pub fn relax(boxes: &mut [(Vec2, Vec2)], safe: Rect, iters: usize) {
    for _ in 0..iters {
        for i in 0..boxes.len() {
            for j in i + 1..boxes.len() {
                let (p, q) = (boxes[i], boxes[j]);
                let (dx, dy) = (q.0.x - p.0.x, q.0.y - p.0.y);
                let ox = (p.1.x + q.1.x) / 2.0 + 6.0 - dx.abs();
                let oy = (p.1.y + q.1.y) / 2.0 + 4.0 - dy.abs();
                if ox > 0.0 && oy > 0.0 {
                    if oy <= ox * 1.4 {
                        let s = if dy < 0.0 { -1.0 } else { 1.0 };
                        boxes[i].0.y -= s * oy / 2.0;
                        boxes[j].0.y += s * oy / 2.0;
                    } else {
                        let s = if dx < 0.0 { -1.0 } else { 1.0 };
                        boxes[i].0.x -= s * ox / 2.0;
                        boxes[j].0.x += s * ox / 2.0;
                    }
                }
            }
        }
        for b in boxes.iter_mut() {
            b.0 = clamp_box(b.0, b.1, safe);
        }
    }
}

/// The tooltip's top-left: 22 px right and 18 px below the pointer, on the left of it when it would pass `w - 12`.
pub fn tip_pos(p: Vec2, size: Vec2, w: f32) -> Vec2 {
    let x = if p.x + 22.0 + size.x > w - 12.0 { p.x - 22.0 - size.x } else { p.x + 22.0 };
    Vec2::new(x.max(4.0), p.y + 18.0)
}

fn overlaps(a: Rect, b: Rect, pad: Vec2) -> bool {
    a.min.x - pad.x < b.max.x && a.max.x + pad.x > b.min.x && a.min.y - pad.y < b.max.y && a.max.y + pad.y > b.min.y
}

fn smooth(a: f32, b: f32, x: f32) -> f32 {
    let t = clamp((x - a) / (b - a), 0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn ease(rate: f32, dt: f32) -> f32 {
    1.0 - (-rate * dt).exp()
}

/// An eased "on" heading for 0 (rate 8), exactly 0 once it is under 1%, so a part that lost its subject fades out
/// in place and then hides.
fn fade_out(on: f32, dt: f32) -> f32 {
    let v = on - on * ease(8.0, dt);
    if v < 0.01 {
        0.0
    } else {
        v
    }
}

// ------------------------------------------------------------------ the overlay's parts

const N_LABELS: usize = 9;
const N_RINGS: usize = 9;
const N_CHIPS: usize = 6;
const N_CALLS: usize = 4;
/// owner ids: labels, ring labels (then the ready label), conflict chips, step callouts, the tooltip
const LABEL0: usize = 0;
const RING0: usize = LABEL0 + N_LABELS;
const READY: usize = RING0 + N_RINGS;
const CHIP0: usize = READY + 1;
const CALL0: usize = CHIP0 + N_CHIPS;
const TIP: usize = CALL0 + N_CALLS;
const N_OWNERS: usize = TIP + 1;

#[derive(Component)]
struct FadeText(usize, Color);
#[derive(Component)]
struct FadeBg(usize, Color);
#[derive(Component)]
struct FadeBorder(usize, Color);

/// What the overlay keeps between frames, per owner.
#[derive(Resource)]
struct Ov {
    roots: Vec<Entity>,
    /// what each root's children were built from
    key: Vec<String>,
    /// final opacity per owner (what `fade` applies)
    opac: Vec<f32>,
    /// the eased "should be on" (labels, chips)
    on: Vec<f32>,
    hide: Vec<f32>,
    hv: Vec<f32>,
    cur: Vec<Vec2>,
    side: Vec<f32>,
    /// which agent a label slot shows
    slot: Vec<Option<String>>,
    /// the hovered agent, eased in and out (tooltip, callouts)
    hov_id: Option<String>,
    hov_k: f32,
    theme: u64,
    shown: bool,
}

fn tc(r: Role) -> Color {
    crate::theme::now(r)
}

fn mix(a: Color, b: Color, t: f32) -> Color {
    let (a, b) = (a.to_srgba(), b.to_srgba());
    Color::srgba(lerp(a.red, b.red, t), lerp(a.green, b.green, t), lerp(a.blue, b.blue, t), lerp(a.alpha, b.alpha, t))
}

fn mute() -> Color {
    mix(tc(Role::Dim), tc(Role::Line), 0.48)
}

fn setup(mut commands: Commands, root: Res<crate::hud::OverlayRoot>) {
    let mut roots = Vec::new();
    for i in 0..N_OWNERS {
        let z = if i == TIP { 18 } else { 2 };
        let e = commands
            .spawn((Node { position_type: PositionType::Absolute, flex_direction: FlexDirection::Column, ..default() }, UiTransform::default(), Visibility::Hidden, GlobalZIndex(z)))
            .id();
        commands.entity(root.0).add_child(e);
        roots.push(e);
    }
    commands.insert_resource(Ov {
        roots,
        key: vec![String::new(); N_OWNERS],
        opac: vec![0.0; N_OWNERS],
        on: vec![0.0; N_OWNERS],
        hide: vec![0.0; N_OWNERS],
        hv: vec![0.0; N_OWNERS],
        cur: vec![Vec2::ZERO; N_OWNERS],
        side: vec![1.0; N_OWNERS],
        slot: vec![None; N_LABELS],
        hov_id: None,
        hov_k: 0.0,
        theme: 0,
        shown: false,
    });
}

fn text(c: &mut Commands, owner: usize, s: impl Into<String>, h: &Handle<Font>, size: f32, sp: f32, col: Color) -> Entity {
    c.spawn((Text::new(s.into()), font(h, size, sp), TextColor(with_alpha(col, 0.0)), FadeText(owner, col), LineHeight::Px(size * 1.25), TextLayout { linebreak: LineBreak::NoWrap, ..default() })).id()
}

fn boxed(c: &mut Commands, owner: usize, mut node: Node, bg: Option<Color>, border: Option<Color>) -> Entity {
    node.flex_direction = if node.flex_direction == FlexDirection::Row { FlexDirection::Row } else { node.flex_direction };
    let e = c.spawn(node).id();
    if let Some(b) = bg {
        c.entity(e).insert((BackgroundColor(with_alpha(b, 0.0)), FadeBg(owner, b)));
    }
    if let Some(b) = border {
        c.entity(e).insert((BorderColor::all(with_alpha(b, 0.0)), FadeBorder(owner, b)));
    }
    e
}

/// A satellite's label plate (SPEC 2.4): title, repo + chip + percent, step + cost, a 2 px bar; mirrored when it
/// sits left of its node.
#[allow(clippy::too_many_arguments)]
fn build_label(c: &mut Commands, f: &UiFonts, root: Entity, owner: usize, a: &model::Agent, repo: &str, left: bool, hover: bool) {
    let ink = Color::srgba(0.0, 0.0, 0.0, 0.62);
    let (line, hot, acc, ok, dim) = (tc(Role::Line), tc(Role::Hot), tc(Role::Acc), tc(Role::Ok), tc(Role::Dim));
    let needs = a.state == State::Needs;
    let done = matches!(a.state, State::Done | State::Merged);
    let queued = a.state == State::Queued;
    let rule = if needs { acc } else if done { ok } else { with_alpha(line, 0.72) };
    let rw = if needs { 2.0 } else { 1.0 };
    let bg = if hover { mix(ink, with_alpha(line, 0.2), 0.3) } else { ink };
    let pad = if left { UiRect { left: px(12), right: px(13), top: px(7), bottom: px(8) } } else { UiRect { left: px(13), right: px(12), top: px(7), bottom: px(8) } };
    c.entity(root).insert((
        Node { position_type: PositionType::Absolute, width: px(LABEL_W), flex_direction: FlexDirection::Column, row_gap: px(3), align_items: if left { AlignItems::FlexEnd } else { AlignItems::FlexStart }, padding: pad, border: if left { UiRect::right(px(rw)) } else { UiRect::left(px(rw)) }, ..default() },
        BackgroundColor(with_alpha(bg, 0.0)),
        FadeBg(owner, bg),
        BorderColor::all(with_alpha(rule, 0.0)),
        FadeBorder(owner, rule),
    ));
    let dir = if left { FlexDirection::RowReverse } else { FlexDirection::Row };
    let title_col = if needs { acc } else if queued { line } else { hot };
    let title = text(c, owner, a.title.to_uppercase(), &f.ui700, 10.5, 1.6, title_col);
    // an agent a protocol started carries a small PROTOCOL tag after its title
    let head = if a.protocol {
        c.entity(title).insert(Node { max_width: px(LABEL_W - 26.0 - 66.0), overflow: Overflow::clip(), ..default() });
        let tag = boxed(c, owner, Node { padding: UiRect { left: px(4), right: px(4), top: px(1), bottom: px(0) }, border: UiRect::all(px(1)), flex_shrink: 0.0, ..default() }, None, Some(with_alpha(line, 0.4)));
        let tt = text(c, owner, "PROTOCOL", &f.ui700, 8.0, 1.3, with_alpha(line, 0.85));
        c.entity(tag).add_child(tt);
        let row = c.spawn(Node { width: percent(100), flex_direction: dir, align_items: AlignItems::Center, column_gap: px(8), ..default() }).id();
        c.entity(row).add_children(&[title, tag]);
        row
    } else {
        c.entity(title).insert(Node { max_width: px(LABEL_W - 26.0), overflow: Overflow::clip(), ..default() });
        title
    };
    // repo, chip, percent
    let meta = c.spawn(Node { width: percent(100), flex_direction: dir, align_items: AlignItems::Center, column_gap: px(8), ..default() }).id();
    let rp = text(c, owner, repo.to_uppercase(), &f.ui600, 9.5, 1.6, mute());
    let (cb, cbg, ctx) = match a.state {
        State::Needs => (acc, Some(acc), Color::BLACK),
        State::Done | State::Merged => (ok, None, ok),
        State::Queued => (dim, None, dim),
        _ => (line, None, line),
    };
    let chip = boxed(c, owner, Node { padding: UiRect { left: px(6), right: px(6), top: px(3), bottom: px(2) }, border: UiRect::all(px(1)), flex_direction: dir, align_items: AlignItems::Center, column_gap: px(5), ..default() }, cbg, Some(cb));
    if a.state == State::Working {
        let sq = boxed(c, owner, Node { width: px(5), height: px(5), ..default() }, Some(tc(Role::Fill)), None);
        c.entity(chip).add_child(sq);
    }
    let ct = text(c, owner, model::chip(a.state).to_uppercase(), &f.ui600, 9.5, 1.5, ctx);
    c.entity(chip).add_child(ct);
    let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
    let pc = text(c, owner, model::pct(a), &f.mono, 10.5, 0.0, if done { ok } else { hot });
    c.entity(meta).add_children(&[rp, chip, sp, pc]);
    // step and cost
    let st = c.spawn(Node { width: percent(100), flex_direction: dir, align_items: AlignItems::Center, column_gap: px(7), ..default() }).id();
    if !queued {
        let b = boxed(c, owner, Node { width: px(4), height: px(4), flex_shrink: 0.0, ..default() }, Some(dim), None);
        c.entity(b).insert(UiTransform { rotation: Rot2::degrees(45.0), ..default() });
        c.entity(st).add_child(b);
    }
    let sw = c.spawn(Node { flex_grow: 1.0, flex_basis: px(0), overflow: Overflow::clip(), justify_content: if left { JustifyContent::FlexEnd } else { JustifyContent::FlexStart }, ..default() }).id();
    let step = text(c, owner, crate::appframe::clip(&a.step, 32), &f.ui500, 12.5, 0.2, if queued { mute() } else { line });
    c.entity(sw).add_child(step);
    let cost = text(c, owner, model::cost(a), &f.mono, 10.5, 0.0, mute());
    c.entity(st).add_children(&[sw, cost]);
    // bar
    let bar = boxed(c, owner, Node { width: percent(100), height: px(2), justify_content: if left { JustifyContent::FlexEnd } else { JustifyContent::FlexStart }, ..default() }, Some(with_alpha(line, 0.14)), None);
    let w = if queued { 0.0 } else { a.p.clamp(0.0, 1.0) * 100.0 };
    let fc = if needs { acc } else if done { ok } else { tc(Role::Fill) };
    let fl = boxed(c, owner, Node { width: percent(w), height: px(2), ..default() }, Some(fc), None);
    c.entity(bar).add_child(fl);
    c.entity(root).add_children(&[head, meta, st, bar]);
}

fn build_ring(c: &mut Commands, f: &UiFonts, root: Entity, owner: usize, name: &str, sub: &str) {
    let r = c.spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(8), ..default() }).id();
    let a = text(c, owner, name.to_uppercase(), &f.ui600, 9.5, 3.0, mute());
    let b = text(c, owner, sub, &f.mono, 10.5, 0.4, tc(Role::Dim));
    c.entity(r).add_children(&[a, b]);
    c.entity(root).add_child(r);
}

fn build_chip(c: &mut Commands, f: &UiFonts, root: Entity, owner: usize, file: &str) {
    let (acc, hot) = (tc(Role::Acc), tc(Role::Hot));
    c.entity(root).insert((
        Node { position_type: PositionType::Absolute, flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(8), padding: UiRect { left: px(8), right: px(9), top: px(5), bottom: px(4) }, border: UiRect::all(px(1)), ..default() },
        BackgroundColor(Color::NONE),
        FadeBg(owner, Color::srgba(0.0, 0.0, 0.0, 0.72)),
        BorderColor::all(Color::NONE),
        FadeBorder(owner, with_alpha(acc, 0.62)),
    ));
    let sq = boxed(c, owner, Node { width: px(6), height: px(6), ..default() }, Some(acc), None);
    let t1 = text(c, owner, "BOTH EDITING", &f.ui700, 10.5, 1.3, acc);
    let t2 = text(c, owner, file, &f.mono, 10.5, 0.0, hot);
    c.entity(root).add_children(&[sq, t1, t2]);
}

fn build_call(c: &mut Commands, f: &UiFonts, root: Entity, owner: usize, time: &str, s: &str) {
    c.entity(root).insert((
        Node { position_type: PositionType::Absolute, flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(8), padding: UiRect { left: px(8), right: px(9), top: px(3), bottom: px(3) }, border: UiRect::left(px(1)), ..default() },
        BackgroundColor(Color::NONE),
        FadeBg(owner, Color::srgba(0.0, 0.0, 0.0, 0.66)),
        BorderColor::all(Color::NONE),
        FadeBorder(owner, with_alpha(tc(Role::Line), 0.72)),
    ));
    let t = text(c, owner, time, &f.mono, 10.5, 0.0, tc(Role::Dim));
    let x = text(c, owner, s, &f.mono, 10.5, 0.0, tc(Role::Line));
    c.entity(root).add_children(&[t, x]);
}

fn build_tip(c: &mut Commands, f: &UiFonts, root: Entity, owner: usize, t1: &str, t2: &str) {
    c.entity(root).insert((
        Node { position_type: PositionType::Absolute, flex_direction: FlexDirection::Column, padding: UiRect { left: px(11), right: px(12), top: px(7), bottom: px(8) }, border: UiRect::left(px(2)), ..default() },
        BackgroundColor(Color::NONE),
        FadeBg(owner, Color::srgba(0.0, 0.0, 0.0, 0.76)),
        BorderColor::all(Color::NONE),
        FadeBorder(owner, tc(Role::Fill)),
    ));
    let a = text(c, owner, t1.to_uppercase(), &f.ui700, 10.5, 2.73, tc(Role::Hot));
    let b = text(c, owner, t2, &f.ui500, 12.5, 0.9, tc(Role::Line));
    c.entity(root).add_children(&[a, b]);
}

/// Sets the node's position (px, snapped to a quarter) only when it moved, so a still label doesn't relayout.
fn put(n: &mut Node, p: Vec2) {
    let (l, t) = (px((p.x * 4.0).round() / 4.0), px((p.y * 4.0).round() / 4.0));
    if n.left != l || n.top != t {
        n.left = l;
        n.top = t;
    }
}

/// Re-applies a part's left/top after its children were rebuilt: the `build_*` fns insert a fresh `Node` (auto
/// position = the screen's top-left corner) and the placement only runs next frame, so without this a plate rebuilt
/// by a theme or content change shows at (0,0) for a frame (every frame while the theme eases).
fn keep_pos(c: &mut Commands, root: Entity, at: (Val, Val)) {
    c.entity(root).queue(move |mut e: EntityWorldMut| {
        if let Some(mut n) = e.get_mut::<Node>() {
            n.left = at.0;
            n.top = at.1;
        }
    });
}

/// Opacity, quantised so easing doesn't touch every colour every frame.
fn quant(v: f32) -> f32 {
    (clamp(v, 0.0, 1.0) * 64.0).round() / 64.0
}

/// A rect on screen from a panel's quad: its corners through the camera.
fn panel_rect(cam: &Camera, gt: &GlobalTransform, p: &Panel, tr: &Transform) -> Option<Rect> {
    let mut r: Option<Rect> = None;
    for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
        let w = tr.transform_point(Vec3::new(sx * p.w / 2.0, sy * p.h / 2.0, 0.0));
        let v = cam.world_to_viewport(gt, w).ok()?;
        r = Some(match r {
            None => Rect { min: v, max: v },
            Some(a) => Rect { min: a.min.min(v), max: a.max.max(v) },
        });
    }
    r
}

/// Positions and shows everything, every frame (and rebuilds a part's children only when its content changed).
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn overlay(
    sim: Res<crate::core::Sim>,
    args: Res<crate::Args>,
    swarm: Res<Swarm>,
    scene: Res<SceneOut>,
    theme: Res<crate::theme::Theme>,
    f: Res<UiFonts>,
    ms: Res<crate::mouse::MouseState>,
    mut out: ResMut<HudOut>,
    mut ov: ResMut<Ov>,
    cams: Query<(&Camera, &Transform), With<CamCore>>,
    quads: Query<(&Panel, &Transform, Option<&panels::HudPanel>)>,
    mut nodes: Query<(&mut Node, &mut UiTransform, &mut Visibility, &ComputedNode)>,
    mut commands: Commands,
) {
    let amt = clamp(swarm.amt.x, 0.0, 1.0);
    if !(swarm.open || amt > 0.001) {
        if ov.shown {
            ov.shown = false;
            out.labels.clear();
            out.panels.clear();
            for e in ov.roots.clone() {
                if let Ok((_, _, mut v, _)) = nodes.get_mut(e) {
                    v.set_if_neq(Visibility::Hidden);
                }
            }
            ov.opac.iter_mut().for_each(|o| *o = 0.0);
            ov.on.iter_mut().for_each(|o| *o = 0.0);
            ov.slot.iter_mut().for_each(|s| *s = None);
        }
        return;
    }
    ov.shown = true;
    let Ok((cam, ctr)) = cams.single() else { return };
    let gt = GlobalTransform::from(*ctr);
    let dt = sim.dt;
    let size = cam.logical_viewport_size().unwrap_or(Vec2::new(args.size.0 as f32, args.size.1 as f32));
    let proj = |p: Vec3| cam.world_to_viewport(&gt, p).ok().filter(|v| v.is_finite());
    // the strip's bottom edge (stage y 58) + 10; above Sphere's spoken line (its panel's top, stage y 858) - 8, which
    // sits over the dock
    let y0 = proj(crate::panel::stage_to_world(800.0, 58.0)).map(|v| v.y).unwrap_or(68.0) + 10.0;
    let y1 = proj(crate::panel::stage_to_world(800.0, 858.0)).map(|v| v.y - 8.0).unwrap_or(size.y - 94.0).min(size.y - 94.0);
    let safe = Rect { min: Vec2::new(36.0, y0), max: Vec2::new(size.x - 36.0, y1.max(y0 + 60.0)) };
    let model = &swarm.model;
    let now = now_secs(&args, &sim);
    let dim = clamp(swarm.dim.x, 0.0, 1.0);
    ov.theme = theme.version;
    let right = gt.right();

    // the focus panel and the request card on screen
    out.panels.clear();
    for (p, tr, hp) in &quads {
        let focus = hp.is_some_and(|h| h.kind == panels::Kind::Focus);
        if (focus || p.id == crate::panel::PanelId::Moment) && p.op.x > 0.3 {
            if let Some(r) = panel_rect(cam, &gt, p, tr) {
                out.panels.push(r);
            }
        }
    }
    let pan = out.panels.clone();

    // ---- satellite labels
    let hover = swarm.hover.clone().or_else(|| if sim.t > 2.5 { std::env::var("SPHERE_SWARM_HOVER").ok().filter(|h| model.agent(h).is_some()) } else { None });
    for i in 0..N_LABELS {
        if let Some(id) = ov.slot[i].clone() {
            if !scene.nodes.iter().any(|n| n.0 == id) && ov.opac[LABEL0 + i] < 0.01 {
                ov.slot[i] = None;
            }
        }
    }
    for n in &scene.nodes {
        if !ov.slot.iter().any(|s| s.as_deref() == Some(n.0.as_str())) {
            if let Some(i) = ov.slot.iter().position(|s| s.is_none()) {
                ov.slot[i] = Some(n.0.clone());
                ov.opac[LABEL0 + i] = 0.0;
                ov.on[LABEL0 + i] = 0.0;
                ov.key[LABEL0 + i].clear();
                ov.side[LABEL0 + i] = 0.0;
            }
        }
    }
    struct L {
        i: usize,
        target: Vec2,
        size: Vec2,
        k: f32,
    }
    let mut ls: Vec<L> = Vec::new();
    for i in 0..N_LABELS {
        let o = LABEL0 + i;
        let Some(id) = ov.slot[i].clone() else { continue };
        let node = scene.nodes.iter().find(|n| n.0 == id);
        let agent = model.agent(&id);
        let proj_p = node.and_then(|n| proj(n.1).map(|p| (n, p)));
        let (Some(a), Some((n, p))) = (agent, proj_p) else {
            ov.on[o] += (0.0 - ov.on[o]) * ease(8.0, dt);
            ov.opac[o] = quant(ov.opac[o] * (1.0 - ease(8.0, dt)));
            if let Ok((_, _, mut v, _)) = nodes.get_mut(ov.roots[o]) {
                v.set_if_neq(if ov.opac[o] > 0.01 { Visibility::Inherited } else { Visibility::Hidden });
            }
            continue;
        };
        let k = proj(n.1 + *right * 10.0).map(|q| (q - p).length() / 10.0).unwrap_or(1.0).max(0.05);
        let focus_me = matches!(&swarm.mode, Mode::Focus(f) if *f == id);
        let want_on = n.3 > 0.02 && !focus_me;
        ov.on[o] += (if want_on { 1.0 } else { 0.0 } - ov.on[o]) * ease(8.0, dt);
        let hv_t = if hover.as_deref() == Some(id.as_str()) { 1.0 } else { 0.0 };
        ov.hv[o] += (hv_t - ov.hv[o]) * ease(12.0, dt);
        // which side: the scene's rule (80 px from the core), else ours from the node's offset to the core
        let side = if n.4 != 0.0 { n.4 } else { side_for(ov.side[o], p.x - proj(Vec3::new(0.0, 22.0, 0.0)).map(|c| c.x).unwrap_or(size.x / 2.0)) };
        let left = side < 0.0;
        let repo = model.repos.get(a.repo).map(|r| r.key.as_str()).unwrap_or("");
        let hov_on = hv_t > 0.5;
        let key = format!("{}|{}|{}|{:?}|{}|{}|{}|{}|{}", a.title, repo, model::pct(a), a.state, a.step, model::cost(a), left, hov_on, ov.theme);
        if ov.key[o] != key {
            ov.key[o] = key;
            let at = nodes.get(ov.roots[o]).map(|x| (x.0.left, x.0.top)).unwrap_or_default();
            commands.entity(ov.roots[o]).despawn_children();
            build_label(&mut commands, &f, ov.roots[o], o, a, repo, left, hov_on);
            keep_pos(&mut commands, ov.roots[o], at);
        }
        ov.side[o] = side;
        // measured plate size (stage px), the plate scaled by the depth factor k
        let (w, h) = match nodes.get(ov.roots[o]) {
            Ok((_, _, _, cn)) if cn.size().y > 1.0 => (LABEL_W, cn.size().y * cn.inverse_scale_factor()),
            _ => (LABEL_W, 78.0),
        };
        let sz = Vec2::new(w, h) * k;
        ls.push(L { i, target: label_target(p, side, w * k, k, ov.hv[o]), size: sz, k });
        // opacity (SPEC 2.4); the scene's fade already holds vis, queued, the dim and the merge
        let depth = 0.5 + 0.5 * smooth(-420.0, 260.0, n.1.z);
        let q = if a.state == State::Queued { 0.2 } else { 0.0 };
        ov.opac[o] = quant(ov.on[o] * clamp(n.3, 0.0, 1.0) * (0.46 + 0.54 * depth) * (1.0 - q) * (1.0 + ov.hv[o] * 0.4));
    }
    // relax the visible ones, then ease each toward its target
    let mut boxes: Vec<(Vec2, Vec2)> = ls.iter().map(|l| (l.target, l.size)).collect();
    relax(&mut boxes, safe, 5);
    out.labels.clear();
    for (l, b) in ls.iter().zip(&boxes) {
        let o = LABEL0 + l.i;
        let fresh = ov.opac[o] < 0.02 && ov.on[o] < 0.2;
        ov.cur[o] = if fresh { b.0 } else { ov.cur[o].lerp(b.0, ease(9.0, dt)) };
        let c = ov.cur[o];
        // hidden under the focus panel or the request card (ease 10)
        let rect = Rect::from_center_size(c, l.size);
        let under = pan.iter().any(|p| overlaps(rect, *p, Vec2::splat(4.0)));
        ov.hide[o] += (if under { 1.0 } else { 0.0 } - ov.hide[o]) * ease(10.0, dt);
        ov.opac[o] = quant(ov.opac[o] * (1.0 - ov.hide[o]));
        if let Ok((mut n, mut tr, mut v, cn)) = nodes.get_mut(ov.roots[o]) {
            let sz0 = cn.size() * cn.inverse_scale_factor();
            let sz0 = if sz0.y > 1.0 { sz0 } else { Vec2::new(LABEL_W, 78.0) };
            put(&mut n, c - sz0 / 2.0);
            let s = Vec2::splat(l.k);
            if tr.scale != s {
                tr.scale = s;
            }
            v.set_if_neq(if ov.opac[o] > 0.01 { Visibility::Inherited } else { Visibility::Hidden });
        }
        if ov.opac[o] > 0.3 {
            if let Some(id) = &ov.slot[l.i] {
                out.labels.push((id.clone(), rect));
            }
        }
    }

    // ---- ring labels and the ready label
    let rd = clamp(1.0 - 0.7 * dim, 0.0, 1.0);
    let ring = |o: usize, p: Option<Vec3>, name: String, sub: String, op: f32, ov: &mut Ov, commands: &mut Commands, nodes: &mut Query<(&mut Node, &mut UiTransform, &mut Visibility, &ComputedNode)>| {
        let key = format!("{name}|{sub}|{}", ov.theme);
        let Some(pp) = p.and_then(proj) else {
            ov.opac[o] = 0.0;
            if let Ok((_, _, mut v, _)) = nodes.get_mut(ov.roots[o]) {
                v.set_if_neq(Visibility::Hidden);
            }
            return;
        };
        if ov.key[o] != key {
            ov.key[o] = key;
            commands.entity(ov.roots[o]).despawn_children();
            build_ring(commands, &f, ov.roots[o], o, &name, &sub);
        }
        ov.opac[o] = quant(op * amt);
        if let Ok((mut n, _, mut v, cn)) = nodes.get_mut(ov.roots[o]) {
            let s = cn.size() * cn.inverse_scale_factor();
            put(&mut n, pp - Vec2::new(s.x / 2.0, s.y));
            v.set_if_neq(if ov.opac[o] > 0.01 { Visibility::Inherited } else { Visibility::Hidden });
        }
    };
    for r in 0..N_RINGS {
        let (name, sub) = if r < model.repos.len() { model::ring_label(model, r) } else { (String::new(), String::new()) };
        let p = scene.ring_label_pos.get(r).copied().filter(|_| r < model.repos.len());
        ring(RING0 + r, p, name, sub, 0.85 * rd, &mut ov, &mut commands, &mut nodes);
    }
    let (rn, rs) = model::ready_label(model);
    ring(READY, (!model.repos.is_empty()).then_some(scene.ready_label_pos), rn, rs, (if model.count(State::Done) > 0 { 0.85 } else { 0.5 }) * rd, &mut ov, &mut commands, &mut nodes);

    // ---- conflict chips at the arcs' crests
    for i in 0..N_CHIPS {
        let o = CHIP0 + i;
        let cr = scene.crests.get(i);
        let file = cr.and_then(|(a, b, _)| model.conflicts.iter().find(|k| (&k.a == a && &k.b == b) || (&k.a == b && &k.b == a))).map(|k| k.file.clone());
        let p = cr.and_then(|c| proj(c.2));
        let (Some(file), Some(p)) = (file, p) else {
            // the conflict cleared (or its crest is not on screen): fade out where it is, then hide
            ov.on[o] = fade_out(ov.on[o], dt);
            ov.opac[o] = quant(ov.opac[o].min(clamp(ov.on[o], 0.0, 1.0)));
            if let Ok((_, _, mut v, _)) = nodes.get_mut(ov.roots[o]) {
                v.set_if_neq(if ov.opac[o] > 0.01 { Visibility::Inherited } else { Visibility::Hidden });
            }
            continue;
        };
        let key = format!("{file}|{}", ov.theme);
        if ov.key[o] != key {
            ov.key[o] = key;
            let at = nodes.get(ov.roots[o]).map(|x| (x.0.left, x.0.top)).unwrap_or_default();
            commands.entity(ov.roots[o]).despawn_children();
            build_chip(&mut commands, &f, ov.roots[o], o, &file);
            keep_pos(&mut commands, ov.roots[o], at);
        }
        ov.on[o] += (1.0 - ov.on[o]) * ease(5.0, dt);
        if let Ok((mut n, _, mut v, cn)) = nodes.get_mut(ov.roots[o]) {
            let s = cn.size() * cn.inverse_scale_factor();
            let s = if s.y > 1.0 { s } else { Vec2::new(150.0, 22.0) };
            let rect = Rect::from_center_size(p, s);
            // hidden under the card or the focus panel (+40 px x, +14 px y)
            let under = pan.iter().any(|q| overlaps(rect, *q, Vec2::new(40.0, 14.0)));
            ov.hide[o] += (if under { 1.0 } else { 0.0 } - ov.hide[o]) * ease(10.0, dt);
            ov.opac[o] = quant(clamp(ov.on[o], 0.0, 1.0) * (1.0 - 0.55 * dim) * (1.0 - ov.hide[o]) * amt);
            put(&mut n, p - s / 2.0);
            v.set_if_neq(if ov.opac[o] > 0.01 { Visibility::Inherited } else { Visibility::Hidden });
        }
    }

    // ---- the hovered agent: step callouts and the tooltip
    let want = hover.clone().filter(|_| !matches!(swarm.mode, Mode::Request(_)));
    if let Some(h) = &want {
        if ov.hov_id.as_ref() != Some(h) {
            ov.hov_id = Some(h.clone());
            ov.hov_k = 0.0;
        }
    }
    ov.hov_k += (if want.is_some() { 1.0 } else { 0.0 } - ov.hov_k) * ease(10.0, dt);
    if want.is_none() && ov.hov_k < 0.01 {
        ov.hov_id = None;
    }
    let hid = ov.hov_id.clone();
    let hag = hid.as_deref().and_then(|id| model.agent(id));
    let hov_k = ov.hov_k;
    // callouts: the older steps of a working agent, at their marks, on the side opposite its label
    let mut ci = 0;
    if let (Some(a), Some(id)) = (hag, hid.as_deref()) {
        if a.state == State::Working {
            let side = ov.slot.iter().position(|s| s.as_deref() == Some(id)).map(|i| ov.side[LABEL0 + i]).unwrap_or(1.0);
            if let Some((_, marks)) = scene.marks.iter().find(|m| m.0 == id) {
                for j in 1..=4usize {
                    let (Some(&(wp, al)), Some(st)) = (marks.get(j), a.steps.get(j)) else { continue };
                    let Some(p) = proj(wp) else { continue };
                    if al < 0.08 || ci >= N_CALLS {
                        continue;
                    }
                    let o = CALL0 + ci;
                    ci += 1;
                    let time = model::ago(now - st.at, false);
                    let s = crate::appframe::clip(&st.text, 40);
                    let key = format!("{time}|{s}|{}", ov.theme);
                    if ov.key[o] != key {
                        ov.key[o] = key;
                        let at = nodes.get(ov.roots[o]).map(|x| (x.0.left, x.0.top)).unwrap_or_default();
                        commands.entity(ov.roots[o]).despawn_children();
                        build_call(&mut commands, &f, ov.roots[o], o, &time, &s);
                        keep_pos(&mut commands, ov.roots[o], at);
                    }
                    if let Ok((mut n, _, mut v, cn)) = nodes.get_mut(ov.roots[o]) {
                        let sz = cn.size() * cn.inverse_scale_factor();
                        let dir = -side;
                        let x = if dir > 0.0 { p.x + 12.0 } else { p.x - 12.0 - sz.x };
                        put(&mut n, Vec2::new(x, p.y - 10.0 - sz.y / 2.0));
                        ov.opac[o] = quant(clamp(al - 0.05, 0.0, 1.0) * hov_k * amt);
                        v.set_if_neq(if ov.opac[o] > 0.01 { Visibility::Inherited } else { Visibility::Hidden });
                    }
                }
            }
        }
    }
    for k in ci..N_CALLS {
        let o = CALL0 + k;
        ov.opac[o] = 0.0;
        if let Ok((_, _, mut v, _)) = nodes.get_mut(ov.roots[o]) {
            v.set_if_neq(Visibility::Hidden);
        }
    }
    // the tooltip near the pointer
    let o = TIP;
    if let Some(a) = hag {
        let (t1, t2) = model::tooltip(a, model, now);
        let key = format!("{t1}|{t2}|{}", ov.theme);
        if ov.key[o] != key {
            ov.key[o] = key;
            let at = nodes.get(ov.roots[o]).map(|x| (x.0.left, x.0.top)).unwrap_or_default();
            commands.entity(ov.roots[o]).despawn_children();
            build_tip(&mut commands, &f, ov.roots[o], o, &t1, &t2);
            keep_pos(&mut commands, ov.roots[o], at);
        }
        let at = ms.pos.or_else(|| hid.as_deref().and_then(|id| scene.nodes.iter().find(|n| n.0 == id)).and_then(|n| proj(n.1)));
        if let (Some(p), Ok((mut n, _, mut v, cn))) = (at, nodes.get_mut(ov.roots[o])) {
            let sz = cn.size() * cn.inverse_scale_factor();
            put(&mut n, tip_pos(p, sz, size.x));
            ov.opac[o] = quant(hov_k * amt);
            v.set_if_neq(if ov.opac[o] > 0.01 { Visibility::Inherited } else { Visibility::Hidden });
        }
    } else {
        ov.opac[o] = 0.0;
        if let Ok((_, _, mut v, _)) = nodes.get_mut(ov.roots[o]) {
            v.set_if_neq(Visibility::Hidden);
        }
    }
}

/// Applies each part's opacity to its colours (Bevy UI has no group opacity).
#[allow(clippy::type_complexity)]
fn fade(ov: Res<Ov>, mut sets: ParamSet<(Query<(&FadeText, &mut TextColor)>, Query<(&FadeBg, &mut BackgroundColor)>, Query<(&FadeBorder, &mut BorderColor)>)>) {
    if !ov.shown {
        return;
    }
    for (f, mut c) in &mut sets.p0() {
        let col = with_alpha(f.1, ov.opac[f.0]);
        if c.0 != col {
            c.0 = col;
        }
    }
    for (f, mut c) in &mut sets.p1() {
        let col = with_alpha(f.1, ov.opac[f.0]);
        if c.0 != col {
            c.0 = col;
        }
    }
    for (f, mut c) in &mut sets.p2() {
        let col = with_alpha(f.1, ov.opac[f.0]);
        if *c != BorderColor::all(col) {
            *c = BorderColor::all(col);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
        Rect { min: Vec2::new(x0, y0), max: Vec2::new(x1, y1) }
    }

    #[test]
    fn a_part_without_its_subject_fades_out_to_exactly_zero() {
        let mut v = 1.0;
        let mut last = v;
        for _ in 0..200 {
            v = fade_out(v, 1.0 / 60.0);
            assert!(v <= last);
            last = v;
        }
        assert_eq!(v, 0.0);
        assert_eq!(fade_out(0.0, 0.05), 0.0);
    }

    #[test]
    fn a_rebuilt_plate_keeps_its_position() {
        let mut world = World::new();
        let e = world.spawn(Node { position_type: PositionType::Absolute, left: px(300), top: px(120), ..default() }).id();
        // what build_* does: replace the Node (left/top back to auto), then keep_pos restores them
        let at = world.get::<Node>(e).map(|n| (n.left, n.top)).unwrap();
        let mut q = bevy::ecs::world::CommandQueue::default();
        let mut c = Commands::new(&mut q, &world);
        c.entity(e).insert(Node { position_type: PositionType::Absolute, width: px(268), ..default() });
        keep_pos(&mut c, e, at);
        q.apply(&mut world);
        let n = world.get::<Node>(e).unwrap();
        assert_eq!((n.left, n.top, n.width), (px(300), px(120), px(268)));
    }

    #[test]
    fn side_has_80px_hysteresis() {
        // right by default, left once 80 px left of the core
        assert_eq!(side_for(0.0, -50.0), 1.0);
        assert_eq!(side_for(1.0, -79.0), 1.0);
        assert_eq!(side_for(1.0, -81.0), -1.0);
        // stays left until 80 px right of it
        assert_eq!(side_for(-1.0, 79.0), -1.0);
        assert_eq!(side_for(-1.0, 0.0), -1.0);
        assert_eq!(side_for(-1.0, 81.0), 1.0);
    }

    #[test]
    fn label_sits_beside_its_node() {
        let t = label_target(Vec2::new(500.0, 300.0), 1.0, 268.0, 1.0, 0.0);
        assert_eq!(t, Vec2::new(500.0 + 134.0 + 37.0, 298.0));
        let l = label_target(Vec2::new(500.0, 300.0), -1.0, 268.0, 1.0, 0.0);
        assert_eq!(l.x, 500.0 - 171.0);
    }

    #[test]
    fn relaxing_separates_overlapping_labels() {
        let safe = rect(36.0, 80.0, 1564.0, 900.0);
        let size = Vec2::new(268.0, 78.0);
        let mut b = [(Vec2::new(600.0, 400.0), size), (Vec2::new(610.0, 420.0), size), (Vec2::new(1200.0, 500.0), size)];
        relax(&mut b, safe, 5);
        let (dx, dy) = ((b[1].0.x - b[0].0.x).abs(), (b[1].0.y - b[0].0.y).abs());
        assert!(dx >= size.x + 6.0 - 0.5 || dy >= size.y + 4.0 - 0.5, "still overlapping: {dx} {dy}");
        // the far one did not move
        assert_eq!(b[2].0, Vec2::new(1200.0, 500.0));
    }

    #[test]
    fn relaxing_pushes_vertically_when_that_is_the_smaller_overlap() {
        let safe = rect(0.0, 0.0, 2000.0, 2000.0);
        let size = Vec2::new(100.0, 50.0);
        let mut b = [(Vec2::new(500.0, 500.0), size), (Vec2::new(500.0, 510.0), size)];
        relax(&mut b, safe, 1);
        assert_eq!(b[0].0.x, 500.0);
        assert!(b[1].0.y - b[0].0.y > 50.0);
    }

    #[test]
    fn labels_clamp_into_the_safe_rect() {
        let safe = rect(36.0, 80.0, 1564.0, 900.0);
        let size = Vec2::new(268.0, 78.0);
        assert_eq!(clamp_box(Vec2::new(0.0, 0.0), size, safe), Vec2::new(36.0 + 134.0, 80.0 + 39.0));
        assert_eq!(clamp_box(Vec2::new(5000.0, 5000.0), size, safe), Vec2::new(1564.0 - 134.0, 900.0 - 39.0));
        let c = Vec2::new(800.0, 500.0);
        assert_eq!(clamp_box(c, size, safe), c);
        // relax clamps too
        let mut b = [(Vec2::new(5000.0, -50.0), size)];
        relax(&mut b, safe, 1);
        assert_eq!(b[0].0, Vec2::new(1564.0 - 134.0, 80.0 + 39.0));
    }

    #[test]
    fn tooltip_flips_left_near_the_right_edge() {
        let size = Vec2::new(200.0, 40.0);
        assert_eq!(tip_pos(Vec2::new(300.0, 200.0), size, 1600.0), Vec2::new(322.0, 218.0));
        let p = tip_pos(Vec2::new(1500.0, 200.0), size, 1600.0);
        assert_eq!(p, Vec2::new(1500.0 - 22.0 - 200.0, 218.0));
    }

    #[test]
    fn rect_overlap_with_padding() {
        let a = rect(0.0, 0.0, 100.0, 100.0);
        assert!(!overlaps(a, rect(110.0, 0.0, 200.0, 100.0), Vec2::splat(4.0)));
        assert!(overlaps(a, rect(110.0, 0.0, 200.0, 100.0), Vec2::new(40.0, 14.0)));
    }
}
