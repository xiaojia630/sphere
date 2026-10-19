//! The mouse on the home (prototype v2): drag the core to turn it (it coasts), drag panels anywhere, scroll to zoom
//! (or push a panel back, or spin the Apps rolodex), double-click the sphere to collapse, a ring to fold it, empty
//! space to fit. A reticle replaces the system cursor and tooltips name the rings.
use crate::core::{CoreCtl, Decks, CORE};
use crate::home::{Act, Acts};
use crate::panel::{Panel, PanelId, PAD};
use crate::util::*;
use crate::CamCore;
use bevy::input::mouse::{MouseButton, MouseScrollUnit, MouseWheel};
use bevy::prelude::*;
use bevy::window::{CursorOptions, PrimaryWindow};

/// What is under the pointer.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Hit {
    None,
    /// over the dock or the ticker: the canvas leaves it alone
    Visor,
    Panel(Entity, PanelId),
    /// a deck (ring) of the core, by index
    Ring(usize),
    /// a core-open app tile
    Tile(usize),
}

#[derive(Default, PartialEq)]
enum Drag {
    #[default]
    None,
    Core,
    Panel(Entity),
    Orbit,
}

#[derive(Resource, Default)]
pub struct MouseState {
    pub hit: Option<Hit>,
    pub pos: Option<Vec2>,
    down_at: Option<(Vec2, f32)>,
    drag: Drag,
    last_pos: Vec2,
    last_move: f32,
    rate: Vec2,
    last_click: Option<(Vec2, f32)>,
    pending_click: Option<(Hit, f32)>,
    wheel_acc: f32,
    pane_wheel: crate::appframe::WheelAcc,
    t: f32,
    /// for the reticle
    pub dragging: bool,
    /// the Agent swarm is open: it takes the clicks and the hover (the core still turns when dragged)
    pub swarm: bool,
    /// the take-over card's box on screen: the canvas leaves the pointer over it alone
    pub block: Option<Rect>,
}

pub fn setup(mut cursor: Query<&mut CursorOptions, With<PrimaryWindow>>, args: Res<crate::Args>) {
    if args.offline() {
        return;
    }
    // the reticle replaces the system cursor over Sphere
    for mut c in &mut cursor {
        c.visible = false;
    }
}

/// Whether the system cursor shows: over the dock (where the reticle hides), or with the pointer outside the
/// window (where the reticle can't follow).
fn system_cursor(inside: bool, over_dock: bool) -> bool {
    !inside || over_dock
}

type PanelPick<'w, 's> = Query<'w, 's, (Entity, &'static Panel, &'static GlobalTransform, &'static Visibility)>;

type TilePick<'w, 's> = Query<'w, 's, (&'static crate::orbit::Tile, &'static GlobalTransform, &'static Visibility)>;

fn pick(win: &Window, pos: Vec2, cam: (&Camera, &GlobalTransform), panels: &PanelPick, tiles: &TilePick, decks: &Decks, gts: &Query<&GlobalTransform>) -> Hit {
    // the dock and the ticker sit in the bottom ~100 px
    if pos.y > win.height() - 100.0 {
        return Hit::Visor;
    }
    let Ok(ray) = cam.0.viewport_to_world(cam.1, pos) else { return Hit::None };
    let mut best: Option<(f32, Hit)> = None;
    for (t, gt, vis) in tiles {
        if *vis == Visibility::Hidden {
            continue;
        }
        let Some(d) = ray.intersect_plane(gt.translation(), InfinitePlane3d::new(gt.back())) else { continue };
        let local = gt.affine().inverse().transform_point3(ray.get_point(d));
        if local.x.abs() <= crate::orbit::TILE_W / 2.0 && local.y.abs() <= crate::orbit::TILE_H / 2.0 && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, Hit::Tile(t.0)));
        }
    }
    for (e, p, gt, vis) in panels {
        if *vis == Visibility::Hidden || p.op.x < 0.06 {
            continue;
        }
        let n = gt.back();
        let Some(d) = ray.intersect_plane(gt.translation(), InfinitePlane3d::new(n)) else { continue };
        let local = gt.affine().inverse().transform_point3(ray.get_point(d));
        let inside = local.x.abs() <= p.w / 2.0 + PAD * 0.5 && local.y.abs() <= p.h / 2.0 + PAD * 0.5;
        if inside && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, Hit::Panel(e, p.id)));
        }
    }
    if let Some((_, h)) = best {
        return h;
    }
    // rings: each deck's plane in its fold space, nearest to the camera wins
    let mut ring: Option<(f32, usize)> = None;
    for (i, d) in decks.list.iter().enumerate() {
        let Ok(gt) = gts.get(d.fold) else { continue };
        let Some(t) = ray.intersect_plane(gt.translation(), InfinitePlane3d::new(gt.back())) else { continue };
        let local = gt.affine().inverse().transform_point3(ray.get_point(t));
        let r = local.truncate().length();
        if r >= d.r_min && r <= d.r_max && d.c.x < 0.5 && ring.is_none_or(|(bt, _)| t < bt) {
            ring = Some((t, i));
        }
    }
    match ring {
        Some((_, i)) => Hit::Ring(i),
        None => Hit::None,
    }
}

pub const DECK_NAMES: [&str; 6] = ["Timeline", "System load", "Agents", "Signal", "Voice", "Sphere core"];

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn update(
    time: Res<Time<Real>>,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut wheel: MessageReader<MouseWheel>,
    win: Query<&Window, With<PrimaryWindow>>,
    cams: Query<(&Camera, &GlobalTransform), With<CamCore>>,
    mut panels: ParamSet<(PanelPick, Query<&mut Panel>)>,
    tiles: TilePick,
    mut orb: ResMut<crate::orbit::Orb>,
    mut decks: ResMut<Decks>,
    gts: Query<&GlobalTransform>,
    mut ctl: ResMut<CoreCtl>,
    mut ms: ResMut<MouseState>,
    (mut acts, mut sound, mut toasts, swarm): (ResMut<Acts>, ResMut<crate::sound::Sound>, ResMut<crate::live::Toasts>, Res<crate::swarm::Swarm>),
    (mut rolo, mut fx, mut open, sv, mut pclicks, mut scrolls, parts): (ResMut<crate::homepanels::Rolodex>, ResMut<crate::fx::Fx>, ResMut<crate::apps::Open>, Res<crate::core::StageView>, ResMut<crate::appframe::PanelClicks>, ResMut<crate::appframe::AppScrolls>, Query<&crate::appframe::AppPart>),
) {
    let dt = time.delta_secs();
    ms.t += dt;
    let now = ms.t;
    if fx.reset_panels {
        fx.reset_panels = false;
        for mut p in &mut panels.p1() {
            p.user = Vec3::ZERO;
        }
    }
    if fx.unfold_all {
        fx.unfold_all = false;
        for d in decks.list.iter_mut() {
            d.g.t = 0.0;
        }
    }
    let Ok(w) = win.single() else { return };
    let Ok(cam) = cams.single() else { return };
    let Some(pos) = w.cursor_position() else {
        ms.pos = None;
        return;
    };
    ms.pos = Some(pos);
    ms.swarm = swarm.open;
    let hit = if ms.drag == Drag::None {
        if ms.block.is_some_and(|r| r.contains(pos)) {
            Hit::Visor
        } else {
            pick(w, pos, cam, &panels.p0(), &tiles, &decks, &gts)
        }
    } else {
        ms.hit.unwrap_or(Hit::None)
    };
    if ms.hit != Some(hit) && matches!(hit, Hit::Ring(_)) && !swarm.open {
        sound.play("hover");
    }
    ms.hit = Some(hit);

    // ---- wheel
    let mut dy = 0.0;
    for ev in wheel.read() {
        dy += match ev.unit {
            MouseScrollUnit::Line => ev.y * 40.0,
            MouseScrollUnit::Pixel => ev.y,
        };
    }
    if dy != 0.0 {
        let ctrl = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
        if orb.open && !ctrl && hit != Hit::Visor {
            // while the core is open, the wheel spins the orbit: every 60 px one slot
            ms.wheel_acc += dy;
            while ms.wheel_acc.abs() >= 60.0 {
                orb.step(if ms.wheel_acc > 0.0 { -1.0 } else { 1.0 });
                ms.wheel_acc -= ms.wheel_acc.signum() * 60.0;
            }
            dy = 0.0;
        }
        match hit {
            _ if dy == 0.0 => {}
            Hit::Panel(_, PanelId::Apps) if !ctrl => {
                // accumulate; every 40 px is one step, with a tick
                ms.wheel_acc += dy;
                while ms.wheel_acc.abs() >= 40.0 {
                    let step = if ms.wheel_acc > 0.0 { -1i32 } else { 1 };
                    ms.wheel_acc -= ms.wheel_acc.signum() * 40.0;
                    let n = crate::homepanels::APPS.len() as i32;
                    rolo.sel = ((rolo.sel as i32 + step).rem_euclid(n)) as usize;
                    rolo.version += 1;
                    sound.play("tick");
                }
            }
            // an app pane: the wheel scrolls what is under the pointer (Ctrl keeps the depth push)
            Hit::Panel(e, PanelId::App | PanelId::AppBare | PanelId::AppFrame) if !ctrl => {
                let n = ms.pane_wheel.feed(e, dy);
                if let (n, Ok(part)) = (n, parts.get(e)) {
                    if n != 0 && part.pane.is_some() {
                        scrolls.push(part.app, part.pane, n);
                    }
                }
            }
            Hit::Panel(e, _) if !ctrl => {
                if let Ok(mut p) = panels.p1().get_mut(e) {
                    p.user.z = (p.user.z + dy * 0.7).clamp(-700.0, 420.0);
                }
            }
            Hit::Visor => {}
            _ => {
                // zoom; scrolling down (negative y) zooms out like the prototype's deltaY > 0
                let f = (-dy * 0.0011 * if ctrl { 9.0 } else { 1.0 }).exp();
                ctl.zoom.t = (ctl.zoom.t * f).clamp(0.35, 2.6);
            }
        }
    }

    // ---- buttons
    if buttons.just_pressed(MouseButton::Left) && hit != Hit::Visor {
        ms.down_at = Some((pos, now));
        ms.last_pos = pos;
    }
    if let Some((p0, _)) = ms.down_at {
        let d = pos - ms.last_pos;
        if ms.drag == Drag::None && (pos - p0).length() > 4.0 && buttons.pressed(MouseButton::Left) {
            ms.drag = match hit {
                _ if orb.open => Drag::Orbit,
                Hit::Panel(e, id) if id != PanelId::Readout && id != PanelId::Moment => Drag::Panel(e),
                _ => Drag::Core,
            };
            orb.dragging = ms.drag == Drag::Orbit;
            if let Drag::Panel(e) = ms.drag {
                if let Ok(mut p) = panels.p1().get_mut(e) {
                    p.dragging = true;
                }
            }
            sound.play_with("grab", 0.8, 0.0);
        }
        match ms.drag {
            Drag::Core => {
                // turn the core: premultiplied about world Y, then X
                let (ay, ax) = (d.x * 0.0062, d.y * 0.0062);
                ctl.auto_orient = false;
                ctl.q_user = (Quat::from_rotation_x(ax) * Quat::from_rotation_y(ay) * ctl.q_user).normalize();
                if dt > 0.0 && d.length() > 0.0 {
                    let r = Vec2::new(ay, ax) / dt;
                    ms.rate = ms.rate.lerp(r, 0.4);
                    ms.last_move = now;
                }
            }
            Drag::Orbit => {
                orb.drag_by(d.x, w.width());
                if dt > 0.0 && d.x != 0.0 {
                    let da = d.x * 2.4 / (0.9 * w.width()).max(320.0) / dt;
                    ms.rate.x = lerp(ms.rate.x, da, 0.35);
                    ms.last_move = now;
                }
            }
            Drag::Panel(e) => {
                // move on the panel's z-plane: screen px to world units at the camera's scale
                let k = sv.k / ctl.zoom.x.max(0.01);
                if let Ok(mut p) = panels.p1().get_mut(e) {
                    p.user.x += d.x / k;
                    p.user.y -= d.y / k;
                    p.tilt.x = (p.tilt.x + d.x * 0.00045 / dt.max(1e-3) * dt).clamp(-0.45, 0.45);
                    p.tilt.y = (p.tilt.y + d.y * 0.00045 / dt.max(1e-3) * dt).clamp(-0.45, 0.45);
                }
            }
            Drag::None => {}
        }
        ms.last_pos = pos;
    }
    ms.dragging = ms.drag != Drag::None;
    if buttons.just_released(MouseButton::Left) {
        match std::mem::take(&mut ms.drag) {
            Drag::Core => {
                // release keeps the spin unless the pointer had stopped
                ctl.omega = if now - ms.last_move < 0.09 { ms.rate } else { Vec2::ZERO };
                ms.rate = Vec2::ZERO;
                sound.play_with("drop", 0.8, 0.0);
            }
            Drag::Panel(e) => {
                if let Ok(mut p) = panels.p1().get_mut(e) {
                    p.dragging = false;
                }
                sound.play_with("drop", 0.8, 0.0);
            }
            Drag::Orbit => {
                orb.dragging = false;
                orb.release(if now - ms.last_move < 0.09 { ms.rate.x } else { 0.0 });
                ms.rate = Vec2::ZERO;
                sound.play_with("drop", 0.8, 0.0);
            }
            Drag::None => {
                // a click: double within 300 ms, else a single after 260 ms
                if let Some((p0, t0)) = ms.down_at {
                    let app_pane = match hit {
                        Hit::Panel(e, PanelId::App | PanelId::AppBare | PanelId::AppFrame | PanelId::Moment) => Some(e),
                        _ => None,
                    };
                    if let (Some(e), true) = (app_pane, (pos - p0).length() <= 4.0) {
                        // an app's pane: the click goes to what's under it, now (apps have no double-click)
                        if let Some(at) = panel_point(cam, &panels.p0(), e, pos) {
                            pclicks.0.push((e, at));
                        }
                    } else if swarm.open {
                        // the swarm takes the clicks (`swarm::input`): none of the home's single and double clicks
                        ms.last_click = None;
                        ms.pending_click = None;
                    } else if (pos - p0).length() <= 4.0 && now - t0 < 0.45 {
                        let dbl = !orb.open && ms.last_click.is_some_and(|(lp, lt)| now - lt < 0.3 && (pos - lp).length() < 8.0);
                        if orb.open {
                            // open: a tile opens its app, the sphere closes the orbit
                            match hit {
                                Hit::Tile(i) => orb.launch(i),
                                Hit::Ring(CORE) => orb.set(false, &mut sound),
                                _ => {}
                            }
                        } else if matches!(hit, Hit::Panel(_, PanelId::Apps | PanelId::Terminal | PanelId::Agents)) {
                            // these panels have no double-click: open now instead of waiting to rule one out
                            ms.last_click = None;
                            ms.pending_click = None;
                            let at = match hit {
                                Hit::Panel(e, _) => panel_point(cam, &panels.p0(), e, pos),
                                _ => None,
                            };
                            single_click(hit, at, &mut rolo, &mut open, &mut sound, &mut toasts);
                        } else if dbl {
                            ms.last_click = None;
                            ms.pending_click = None;
                            double_click(hit, &mut decks, &mut acts, &mut sound, &mut toasts);
                        } else {
                            ms.last_click = Some((pos, now));
                            ms.pending_click = Some((hit, now));
                        }
                    }
                }
            }
        }
        ms.down_at = None;
    }
    // hold the sphere half a second (without moving) to open the core
    if let Some((p0, t0)) = ms.down_at {
        if !orb.open && !swarm.open && ms.drag == Drag::None && hit == Hit::Ring(CORE) && (pos - p0).length() <= 4.0 && now - t0 > 0.5 {
            orb.set(true, &mut sound);
            ms.down_at = None;
            ms.pending_click = None;
        }
    }
    if let Some((h, t0)) = ms.pending_click {
        if now - t0 > 0.26 {
            ms.pending_click = None;
            single_click(h, None, &mut rolo, &mut open, &mut sound, &mut toasts);
        }
    }
}

fn double_click(hit: Hit, decks: &mut Decks, acts: &mut Acts, sound: &mut crate::sound::Sound, toasts: &mut crate::live::Toasts) {
    match hit {
        Hit::Ring(CORE) => acts.0.push(Act::Collapse(None)),
        Hit::Ring(i) => {
            let d = &mut decks.list[i];
            let fold = d.g.t < 0.5;
            d.g.t = if fold { 1.0 } else { 0.0 };
            sound.play_with(if fold { "collapse" } else { "expand" }, 0.6, 0.0);
            toasts.push_quiet(if fold { format!("{} folded · double-click to unfold", DECK_NAMES[i]) } else { format!("{} unfolded", DECK_NAMES[i]) });
        }
        Hit::None => acts.0.push(Act::Fit),
        _ => {}
    }
}

/// Where the pointer at `pos` meets panel `e`, in panel px from its top-left.
fn panel_point(cam: (&Camera, &GlobalTransform), panels: &PanelPick, e: Entity, pos: Vec2) -> Option<Vec2> {
    let ray = cam.0.viewport_to_world(cam.1, pos).ok()?;
    let (_, p, gt, _) = panels.get(e).ok()?;
    let d = ray.intersect_plane(gt.translation(), InfinitePlane3d::new(gt.back()))?;
    let local = gt.affine().inverse().transform_point3(ray.get_point(d));
    Some(Vec2::new(local.x + p.w / 2.0, p.h / 2.0 - local.y))
}

/// `at` is where on the panel it was clicked (panel px), when known.
fn single_click(hit: Hit, at: Option<Vec2>, rolo: &mut crate::homepanels::Rolodex, open: &mut crate::apps::Open, sound: &mut crate::sound::Sound, toasts: &mut crate::live::Toasts) {
    match hit {
        Hit::Panel(_, PanelId::Apps) => {
            // the row under the pointer turns to the band and opens; the heading and the hint open the selected one
            if let Some(i) = at.and_then(|a| crate::homepanels::rolo_row_at(a.y, rolo.sel)) {
                if i != rolo.sel {
                    rolo.sel = i;
                    rolo.version += 1;
                }
            }
            open.app(rolo.sel, toasts);
            sound.play_q("click", 90);
        }
        Hit::Panel(_, PanelId::Terminal) => open.app(0, toasts),
        // a row of "elsewhere" opens its card (`takeover::commands` finds which), anywhere else the Working view
        Hit::Panel(_, PanelId::Agents) => match at {
            Some(a) => open.agents_click = Some(a),
            None => open.app(1, toasts),
        },
        Hit::Ring(crate::core::AGENTS) => open.app(1, toasts),
        _ => {}
    }
}

#[derive(Component)]
pub struct Reticle;
#[derive(Component)]
pub struct ReticleRing;
#[derive(Component)]
pub struct Tip;
#[derive(Component)]
pub struct TipText(pub u8);

pub fn setup_reticle(mut commands: Commands, f: Res<crate::fonts::UiFonts>, root: Res<crate::hud::OverlayRoot>, args: Res<crate::Args>) {
    if args.offline() {
        return;
    }
    let c = &mut commands;
    let line = role_color(Role::Line);
    let hot = role_color(Role::Hot);
    let r = c.spawn((Reticle, Node { position_type: PositionType::Absolute, width: px(44), height: px(44), ..default() }, UiTransform::default(), GlobalZIndex(20))).id();
    let outer = c
        .spawn((ReticleRing, Node { position_type: PositionType::Absolute, left: px(7), top: px(7), width: px(30), height: px(30), border: UiRect::all(px(1)), border_radius: BorderRadius::MAX, ..default() }, BorderColor::all(crate::panel::with_alpha(line, 0.7))))
        .id();
    let inner = c.spawn((Node { position_type: PositionType::Absolute, left: px(15), top: px(15), width: px(14), height: px(14), border: UiRect::all(px(1)), border_radius: BorderRadius::MAX, ..default() }, BorderColor::all(hot))).id();
    let dot = c.spawn((Node { position_type: PositionType::Absolute, left: px(20.6), top: px(20.6), width: px(2.8), height: px(2.8), border_radius: BorderRadius::MAX, ..default() }, BackgroundColor(role_color(Role::Fill)))).id();
    let mut ticks = vec![];
    for (l, t, w, h) in [(21.5, 1.0, 1.0, 4.0), (21.5, 39.0, 1.0, 4.0), (1.0, 21.5, 4.0, 1.0), (39.0, 21.5, 4.0, 1.0)] {
        ticks.push(c.spawn((Node { position_type: PositionType::Absolute, left: px(l), top: px(t), width: px(w), height: px(h), ..default() }, BackgroundColor(hot))).id());
    }
    c.entity(r).add_children(&[outer, inner, dot]);
    c.entity(r).add_children(&ticks);
    // the tooltip: black glass, a fill bar on the left, two lines
    let tip = c
        .spawn((
            Tip,
            Node { position_type: PositionType::Absolute, flex_direction: FlexDirection::Column, padding: UiRect { left: px(11), right: px(12), top: px(7), bottom: px(8) }, border: UiRect { left: px(2), ..default() }, ..default() },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.76)),
            BorderColor::all(role_color(Role::Fill)),
            Visibility::Hidden,
            GlobalZIndex(19),
        ))
        .id();
    let t1 = c.spawn((TipText(1), Text::new(""), crate::panel::font(&f.ui700, 10.5, 2.73), TextColor(hot), bevy::text::LineHeight::Px(15.2))).id();
    let t2 = c.spawn((TipText(2), Text::new(""), crate::panel::font(&f.ui500, 12.0, 0.96), TextColor(line), bevy::text::LineHeight::Px(16.8))).id();
    c.entity(tip).add_children(&[t1, t2]);
    c.entity(root.0).add_children(&[r, tip]);
}

/// The reticle follows the pointer (ease 40); it turns 45° and grows over something you can use, and grows more
/// while dragging. Over a ring, the tooltip names it.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn draw_reticle(
    time: Res<Time<Real>>,
    ms: Res<MouseState>,
    win: Query<&Window, With<PrimaryWindow>>,
    agents: Res<crate::core::Agents>,
    stats: Res<crate::sysstats::SysStats>,
    swarm: Res<crate::swarm::Swarm>,
    args: Res<crate::Args>,
    mut cursor: Query<&mut CursorOptions, With<PrimaryWindow>>,
    mut at: Local<Vec2>,
    mut q: ParamSet<(Query<(&mut Node, &mut UiTransform, &mut Visibility), With<Reticle>>, Query<(&mut Node, &mut Visibility), With<Tip>>, Query<(&TipText, &mut Text)>)>,
) {
    let dt = time.delta_secs();
    // over the dock and the command bar the reticle steps aside for the system cursor (a text caret over the
    // bar): one of the two always shows
    if !args.offline() {
        let want = system_cursor(ms.pos.is_some(), matches!(ms.hit, Some(Hit::Visor)));
        for mut c in &mut cursor {
            if c.visible != want {
                c.visible = want;
            }
        }
    }
    let Some(p) = ms.pos else {
        for (_, _, mut v) in &mut q.p0() {
            v.set_if_neq(Visibility::Hidden);
        }
        return;
    };
    let k = 1.0 - (-40.0 * dt).exp();
    *at = at.lerp(p, k);
    let hov = matches!(ms.hit, Some(Hit::Ring(_)) | Some(Hit::Panel(..))) || swarm.hover.is_some();
    let typing_area = matches!(ms.hit, Some(Hit::Visor));
    for (mut n, mut tr, mut v) in &mut q.p0() {
        v.set_if_neq(if typing_area { Visibility::Hidden } else { Visibility::Inherited });
        // writes only on change, so a still cursor doesn't relayout the UI every frame
        let (l, t) = (px(at.x - 22.0), px(at.y - 22.0));
        if n.left != l || n.top != t {
            n.left = l;
            n.top = t;
        }
        let (rot, sc) = if ms.dragging { (0.0, 1.55) } else if hov { (45.0, 1.25) } else { (0.0, 1.0) };
        let cur = tr.rotation.as_degrees();
        let r = lerp(cur, rot, (12.0 * dt).min(1.0));
        let s = lerp(tr.scale.x, sc, (12.0 * dt).min(1.0));
        if (r - cur).abs() > 1e-3 || (s - tr.scale.x).abs() > 1e-4 {
            tr.rotation = Rot2::degrees(r);
            tr.scale = Vec2::splat(s);
        }
    }
    // the swarm has its own tooltip (for the agent under the pointer)
    let tip = match ms.hit {
        Some(Hit::Ring(i)) if !ms.swarm => Some(i),
        _ => None,
    };
    let w = win.single().map(|w| w.width()).unwrap_or(1600.0);
    for (mut n, mut v) in &mut q.p1() {
        v.set_if_neq(if tip.is_some() && !ms.dragging { Visibility::Inherited } else { Visibility::Hidden });
        let x = if p.x + 22.0 + 260.0 > w { p.x - 22.0 - 260.0 } else { p.x + 22.0 };
        let (l, t) = (px(x), px(p.y + 18.0));
        if n.left != l || n.top != t {
            n.left = l;
            n.top = t;
        }
    }
    if let Some(i) = tip {
        let s = stats.get();
        let (t1, t2) = match i {
            crate::core::TIME => ("TIMELINE · TODAY".to_string(), "Now on the ring at 12 o'clock is midnight".to_string()),
            crate::core::LOAD => ("SYSTEM LOAD".to_string(), format!("CPU {:.0}% · memory {:.0}% · GPU {:.0}%", s.cpu * 100.0, s.mem * 100.0, s.gpu * 100.0)),
            crate::core::AGENTS => ("AGENTS".to_string(), format!("{} · click to open", agents.summary())),
            crate::core::SIGNAL => ("SIGNAL".to_string(), "sphered link and services".to_string()),
            crate::core::VOICE => ("VOICE".to_string(), "Voice arrives with M5 · / to type".to_string()),
            _ => ("SPHERE CORE".to_string(), "Drag rotate · double-click to collapse · double-click empty space to fit".to_string()),
        };
        for (tt, mut t) in &mut q.p2() {
            let want = if tt.0 == 1 { &t1 } else { &t2 };
            if &t.0 != want {
                t.0 = want.clone();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cursor_always_shows() {
        // over the scene the reticle is the cursor; over the dock (the reticle hides there) and outside the
        // window the system cursor shows
        assert!(!system_cursor(true, false));
        assert!(system_cursor(true, true));
        assert!(system_cursor(false, false));
    }
}
