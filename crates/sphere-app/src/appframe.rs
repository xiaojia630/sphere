//! What every app shares: the UI helpers its panes are drawn with (a pane header, a body, text).
use crate::fonts::UiFonts;
use crate::panel::*;
use bevy::prelude::*;
use bevy::text::LineHeight;

impl<'a> Ui<'a> {
    pub fn new(f: &'a UiFonts) -> Self {
        Ui { f, line: crate::util::role_color(crate::util::Role::Line), hot: crate::util::role_color(crate::util::Role::Hot), dim: crate::util::role_color(crate::util::Role::Dim) }
    }
}

pub struct Ui<'a> {
    pub f: &'a UiFonts,
    pub line: Color,
    pub hot: Color,
    pub dim: Color,
}

impl Ui<'_> {
    pub fn text(&self, c: &mut Commands, s: impl Into<String>, h: &Handle<Font>, size: f32, sp: f32, col: Color) -> Entity {
        c.spawn((Text::new(s.into()), font(h, size, sp), TextColor(col), LineHeight::Px(size * 1.25), TextLayout { linebreak: LineBreak::NoWrap, ..default() })).id()
    }
    pub fn row(&self, c: &mut Commands, gap: f32) -> Entity {
        c.spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(gap), ..default() }).id()
    }
    /// "■ TITLE ............ right"
    pub fn header(&self, c: &mut Commands, title: &str, right: &str, right_col: Color) -> Entity {
        let h = c
            .spawn((
                Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(8), padding: UiRect { left: px(14), right: px(14), top: px(11), bottom: px(8) }, margin: UiRect { left: px(4), right: px(4), ..default() }, border: UiRect { bottom: px(1), ..default() }, flex_shrink: 0.0, ..default() },
                BorderColor::all(with_alpha(self.line, 0.34)),
            ))
            .id();
        let sq = c.spawn((Node { width: px(6), height: px(6), ..default() }, BackgroundColor(self.hot))).id();
        let t = self.text(c, title.to_uppercase(), &self.f.ui600, 11.0, 3.3, self.hot);
        let r = c.spawn((Text::new(right.to_string()), font(&self.f.ui500, 11.0, 1.4), TextColor(right_col), LineHeight::Px(13.0), Node { margin: UiRect { left: auto(), ..default() }, ..default() })).id();
        c.entity(h).add_children(&[sq, t, r]);
        h
    }
    pub fn body(&self, c: &mut Commands, root: Entity) -> Entity {
        let b = c.spawn(Node { width: percent(100), height: percent(100), flex_direction: FlexDirection::Column, overflow: Overflow::clip(), ..default() }).id();
        c.entity(root).add_child(b);
        b
    }
}

pub fn clip(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        s.chars().take(n.saturating_sub(1)).collect::<String>() + "…"
    } else {
        s.to_string()
    }
}


// ------------------------------------------------------------------ the workspace every app opens into

use crate::util::{role_color, Role, Spring};

/// The workspace on the stage (its centre) and how much larger than stage px its panes draw.
pub const WS_W: f32 = 1280.0;
pub const WS_H: f32 = 700.0;
const WS_CX: f32 = 803.0;
const WS_CY: f32 = 552.0;
const K: f32 = 1.1;

/// Part of an app's workspace: `pane` is its index in the app's pane list (`None` for the glass and the label).
#[derive(Component)]
pub struct AppPart {
    pub app: u8,
    pub pane: Option<u8>,
}

pub struct Workspace {
    pub panes: Vec<Entity>,
    pub corelab: Entity,
}

/// Spawns an app's glass, its panes (x, y, w, h, z in workspace px; `bare` ones get no frame) and the label beside
/// the small core.
/// Spawns an app's glass, its panes and its label in the app's own atlas (made and dropped as the app opens and
/// closes; see `AppAtlases`).
#[allow(clippy::too_many_arguments)]
pub fn spawn_workspace(c: &mut Commands, meshes: &mut Assets<Mesh>, images: &mut Assets<Image>, home_atlas: &PanelAtlas, apps: &mut AppAtlases, pmats: &mut Assets<PanelMaterial>, app: u8, rects: &[([f32; 5], bool)]) -> Workspace {
    let mut own = Atlas::new(c, images, home_atlas.scale * APP_SHARP, -11 - app as isize);
    let atlas = &mut own;
    let at = |x: f32, y: f32, w: f32, h: f32, z: f32| {
        let mut p = stage_to_world(WS_CX + K * (x + w / 2.0 - WS_W / 2.0), WS_CY + K * (y + h / 2.0 - WS_H / 2.0));
        p.z = z;
        p
    };
    let ui = spawn_ui_panel(c, atlas, 4.0, 4.0);
    let e = spawn_panel_quad(c, meshes, pmats, &ui, &PanelSpec { id: PanelId::AppFrame, w: WS_W, h: WS_H, home: at(0.0, 0.0, WS_W, WS_H, -10.0), home_scale: K, side: 0.0, phase: 0.5 + app as f32, show_at: 0.0, unframed: false });
    c.entity(e).insert(AppPart { app, pane: None });
    let mut panes = Vec::new();
    for (i, ([x, y, w, h, z], bare)) in rects.iter().enumerate() {
        let ui = spawn_ui_panel(c, atlas, *w, *h);
        let id = if *bare { PanelId::AppBare } else { PanelId::App };
        let e = spawn_panel_quad(c, meshes, pmats, &ui, &PanelSpec { id, w: *w, h: *h, home: at(*x, *y, *w, *h, *z), home_scale: K, side: 0.0, phase: 0.4 + i as f32 * 1.1 + app as f32, show_at: 0.0, unframed: false });
        c.entity(e).insert((AppPart { app, pane: Some(i as u8) }, PaneRoot(ui.root)));
        panes.push(ui.root);
    }
    let (lw, lh) = (300.0, 40.0);
    let ui = spawn_ui_panel(c, atlas, lw, lh);
    let mut home = stage_to_world(132.0 + lw / 2.0, 52.0 + lh / 2.0);
    home.z = 20.0;
    let e = spawn_panel_quad(c, meshes, pmats, &ui, &PanelSpec { id: PanelId::AppBare, w: lw, h: lh, home, home_scale: 1.0, side: 0.0, phase: 2.6, show_at: 0.0, unframed: false });
    c.entity(e).insert(AppPart { app, pane: None });
    let corelab = ui.root;
    apps.add(app, own);
    Workspace { panes, corelab }
}

/// The parts follow how open their app is (a pane `hide` says no to stays away).
pub fn place_app(app: u8, amt: f32, hide: impl Fn(u8) -> bool, q: &mut Query<(&mut Panel, &AppPart)>) {
    let amt = amt.clamp(0.0, 1.0);
    for (mut p, part) in q.iter_mut() {
        if part.app != app {
            continue;
        }
        let a = if part.pane.is_some_and(&hide) { 0.0 } else { amt };
        if p.work.map(|w| w.0) != Some(a) {
            p.work = Some((a, 0.0));
        }
    }
}

/// Opening and closing: the spring and the core's corner.
pub fn step_open(open: bool, amt: &mut Spring, dt: f32, ctl: &mut crate::core::CoreCtl) {
    amt.t = if open { 1.0 } else { 0.0 };
    amt.step(crate::motion::dt(crate::motion::Pace::Big, dt));
    ctl.app = ctl.app.max(amt.x.clamp(0.0, 1.0));
}

impl Ui<'_> {
    /// "NAME / line two" beside the small core.
    pub fn corelab(&self, c: &mut Commands, root: Entity, name: &str, line2: String, col: Color) {
        let lab = c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(5), padding: UiRect::top(px(4)), ..default() }).id();
        let l1 = self.text(c, name.to_uppercase(), &self.f.ui600, 11.0, 3.1, self.hot);
        let l2 = self.text(c, line2, &self.f.ui500, 11.5, 1.2, col);
        c.entity(lab).add_children(&[l1, l2]);
        c.entity(root).add_child(lab);
    }
    /// The workspace's title bar: name, a sub line, the credit and ESC.
    pub fn top(&self, c: &mut Commands, root: Entity, name: &str, sub: String, credit: &str) {
        let r = c.spawn(Node { width: percent(100), height: percent(100), flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(12), ..default() }).id();
        c.entity(root).add_child(r);
        let n = self.text(c, name.to_uppercase(), &self.f.ui600, 11.0, 3.5, self.hot);
        let s = self.text(c, sub, &self.f.ui500, 11.0, 2.0, self.dim);
        let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
        let cr = self.text(c, credit, &self.f.ui500, 10.5, 1.0, self.dim);
        let esc = c.spawn((Node { padding: UiRect { left: px(6), right: px(6), top: px(3), bottom: px(2) }, border: UiRect::all(px(1)), ..default() }, BorderColor::all(with_alpha(self.line, 0.55)), Click::Key(KeyCode::Escape))).id();
        let et = self.text(c, "ESC", &self.f.ui700, 10.0, 1.6, self.hot);
        c.entity(esc).add_child(et);
        c.entity(r).add_children(&[n, s, sp, cr, esc]);
    }
    /// The agents strip with the key legend on the right.
    pub fn strip(&self, c: &mut Commands, root: Entity, agents: &[sphere_proto::TaskView], merged: &std::collections::HashSet<String>, legend: [&str; 2]) {
        use sphere_proto::TaskState;
        let r = c.spawn(Node { width: percent(100), height: percent(100), flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(12), padding: UiRect { left: px(16), right: px(14), ..default() }, ..default() }).id();
        c.entity(root).add_child(r);
        let lab = self.text(c, "AGENTS", &self.f.ui600, 10.5, 3.2, self.dim);
        c.entity(r).add_child(lab);
        for t in agents.iter().take(3) {
            let (col, sub) = match t.state {
                TaskState::Done => (role_color(Role::Ok), finished(t, merged).to_string()),
                TaskState::NeedsYou => (role_color(Role::Acc), format!("Needs you · {}", clip(&t.status, 24))),
                TaskState::Failed => (role_color(Role::Bad), "Failed".to_string()),
                TaskState::Stopped => (self.dim, "Stopped".to_string()),
                _ => (self.line, clip(&t.status, 30)),
            };
            let chip = c.spawn((Node { height: px(36), flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(10), padding: UiRect { left: px(10), right: px(12), ..default() }, border: UiRect::all(px(1)), ..default() }, BorderColor::all(with_alpha(col, 0.5)))).id();
            let dot = c.spawn((Node { width: px(8), height: px(8), ..default() }, BackgroundColor(col), UiTransform { rotation: Rot2::degrees(45.0), ..default() })).id();
            let col2 = c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(1), ..default() }).id();
            let tt = self.text(c, clip(&t.title, 26), &self.f.ui600, 12.5, 0.4, self.hot);
            let st = self.text(c, sub, &self.f.ui500, 11.0, 0.4, col);
            c.entity(col2).add_children(&[tt, st]);
            c.entity(chip).add_children(&[dot, col2]);
            c.entity(r).add_child(chip);
        }
        if agents.is_empty() {
            let t = self.text(c, "No agents running", &self.f.ui500, 12.0, 0.4, self.dim);
            c.entity(r).add_child(t);
        }
        let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
        let lg = c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(4), align_items: AlignItems::FlexEnd, ..default() }).id();
        let l1 = self.text(c, legend[0], &self.f.ui600, 10.0, 1.6, self.dim);
        let l2 = self.text(c, legend[1], &self.f.ui600, 10.0, 1.6, self.dim);
        c.entity(lg).add_children(&[l1, l2]);
        c.entity(r).add_children(&[sp, lg]);
    }
    /// A key-value row: dim label left, value right.
    pub fn kv(&self, c: &mut Commands, k: &str, v: String, vcol: Color) -> Entity {
        let row = c.spawn(Node { flex_direction: FlexDirection::Row, justify_content: JustifyContent::SpaceBetween, align_items: AlignItems::Center, height: px(20), ..default() }).id();
        let kt = self.text(c, k.to_uppercase(), &self.f.ui600, 10.0, 2.2, self.dim);
        let vt = self.text(c, v, &self.f.mono, 12.0, 0.0, vcol);
        c.entity(row).add_children(&[kt, vt]);
        row
    }
    /// A thin bar 0..1.
    pub fn bar(&self, c: &mut Commands, v: f32, col: Color, w: f32) -> Entity {
        let track = c.spawn((Node { width: px(w), height: px(4), ..default() }, BackgroundColor(with_alpha(self.line, 0.14)))).id();
        let fill = c.spawn((Node { width: percent(v.clamp(0.0, 1.0) * 100.0), height: px(4), ..default() }, BackgroundColor(col))).id();
        c.entity(track).add_child(fill);
        track
    }
    /// A sparkline of bars from the newest 40 samples (0..1).
    pub fn spark(&self, c: &mut Commands, vals: &[f32], h: f32, col: Color) -> Entity {
        let row = c.spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::FlexEnd, height: px(h), column_gap: px(2), width: percent(100), ..default() }).id();
        // always 40 slots, newest on the right, so a short history doesn't draw fat bars
        let pad = 40usize.saturating_sub(vals.len());
        for v in std::iter::repeat_n(&0.0f32, pad).chain(vals.iter().rev().take(40).collect::<Vec<_>>().into_iter().rev()) {
            let b = c.spawn((Node { flex_grow: 1.0, height: px((v.clamp(0.0, 1.0) * h).max(1.0)), ..default() }, BackgroundColor(with_alpha(col, 0.75)))).id();
            c.entity(row).add_child(b);
        }
        row
    }
    /// A small framed key-labelled button.
    pub fn button(&self, c: &mut Commands, label: &str, key: &str, col: Color, filled: bool) -> Entity {
        let btn = c
            .spawn((
                Node { height: px(28), padding: UiRect::horizontal(px(10)), align_items: AlignItems::Center, column_gap: px(8), border: UiRect::all(px(1)), ..default() },
                BorderColor::all(with_alpha(col, if filled { 1.0 } else { 0.6 })),
                BackgroundColor(if filled { col } else { with_alpha(self.line, 0.05) }),
            ))
            .id();
        let fg = if filled { Color::BLACK } else { col };
        let t = self.text(c, label.to_uppercase(), &self.f.ui600, 10.5, 2.0, fg);
        let k = self.text(c, key, &self.f.ui700, 9.5, 0.5, with_alpha(fg, 0.7));
        c.entity(btn).add_children(&[t, k]);
        btn
    }
}

// ------------------------------------------------------------------ clicks

/// What clicking a node in an app's pane does: press a key, or pick the nth item (and `Alt`, a second kind of
/// item in the same app: a repo card, a filter tab, a pinned place).
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub enum Click {
    Key(KeyCode),
    Pick(usize),
    Alt(usize),
}

/// On an app pane's quad: the root of its UI, to find what a click landed on.
#[derive(Component)]
pub struct PaneRoot(pub Entity);

/// Clicks on app panes this frame: the quad and where on it (pane px from the top-left).
#[derive(Resource, Default)]
pub struct PanelClicks(pub Vec<(Entity, Vec2)>);

/// Clicks resolved to what they do, per app, until the app takes them.
#[derive(Resource, Default)]
pub struct AppClicks(Vec<(u8, Click)>);

impl AppClicks {
    pub fn take(&mut self, app: u8) -> Vec<Click> {
        let (mine, rest): (Vec<_>, Vec<_>) = self.0.drain(..).partition(|c| c.0 == app);
        self.0 = rest;
        mine.into_iter().map(|c| c.1).collect()
    }

    /// A click on an app's pane, as `route_clicks` makes them (tests).
    #[cfg(test)]
    pub fn push(&mut self, app: u8, c: Click) {
        self.0.push((app, c));
    }
}

/// Finds the clickable node under each click: the smallest one whose rect holds the point.
#[allow(clippy::type_complexity)]
pub fn route_clicks(
    mut clicks: ResMut<PanelClicks>,
    mut out: ResMut<AppClicks>,
    quads: Query<(&PaneRoot, &Panel, &AppPart)>,
    nodes: Query<(&ComputedNode, &UiGlobalTransform)>,
    clickable: Query<(Entity, &Click)>,
    children: Query<&Children>,
    mut sound: ResMut<crate::sound::Sound>,
) {
    for (quad, at) in clicks.0.drain(..) {
        let Ok((root, panel, part)) = quads.get(quad) else { continue };
        let Ok((rn, rt)) = nodes.get(root.0) else { continue };
        let size = rn.size();
        let scale = size.x / panel.w.max(1.0);
        let p = rt.translation - size / 2.0 + at * scale;
        let mut best: Option<(f32, Click)> = None;
        for e in children.iter_descendants(root.0) {
            let Ok((_, c)) = clickable.get(e) else { continue };
            let Ok((n, t)) = nodes.get(e) else { continue };
            let (half, centre) = (n.size() / 2.0, t.translation);
            if (p - centre).abs().cmple(half).all() {
                let area = n.size().x * n.size().y;
                if best.is_none_or(|b| area < b.0) {
                    best = Some((area, *c));
                }
            }
        }
        if let Some((_, c)) = best {
            out.0.push((part.app, c));
            sound.play_q("click", 40);
        }
    }
}

/// Wheel px that make one scroll step in an app pane.
pub const WHEEL_STEP: f32 = 40.0;
/// Steps one frame of wheel may make, so a fling doesn't flood an app.
const WHEEL_MAX: i32 = 6;

/// The wheel over one pane, accumulated into steps (a different pane starts over).
#[derive(Default)]
pub struct WheelAcc {
    acc: f32,
    at: Option<Entity>,
}

impl WheelAcc {
    /// Adds a wheel delta (positive = wheel up) over `at`; returns whole steps, positive = down.
    pub fn feed(&mut self, at: Entity, dy: f32) -> i32 {
        if self.at != Some(at) {
            self.at = Some(at);
            self.acc = 0.0;
        }
        self.acc += dy;
        let n = (self.acc / WHEEL_STEP).abs().floor() as i32;
        self.acc -= self.acc.signum() * n as f32 * WHEEL_STEP;
        if n == 0 {
            return 0;
        }
        let down = if dy > 0.0 { -1 } else { 1 };
        down * n.min(WHEEL_MAX)
    }
}

/// Wheel steps on app panes, per app: (app, pane index in `AppPart`, steps; positive = down).
#[derive(Resource, Default)]
pub struct AppScrolls(Vec<(u8, Option<u8>, i32)>);

impl AppScrolls {
    pub fn push(&mut self, app: u8, pane: Option<u8>, n: i32) {
        self.0.push((app, pane, n));
    }
    pub fn take(&mut self, app: u8) -> Vec<(Option<u8>, i32)> {
        let (mine, rest): (Vec<_>, Vec<_>) = self.0.drain(..).partition(|c| c.0 == app);
        self.0 = rest;
        mine.into_iter().map(|c| (c.1, c.2)).collect()
    }
}

/// Scroll steps as the keys the pane answers to (`key(pane, down)`), one press per step.
pub fn scroll_keys(steps: Vec<(Option<u8>, i32)>, key: impl Fn(Option<u8>, bool) -> Option<KeyCode>) -> Vec<KeyCode> {
    let mut out = Vec::new();
    for (pane, n) in steps {
        if let Some(k) = key(pane, n > 0) {
            out.extend(std::iter::repeat_n(k, n.unsigned_abs() as usize));
        }
    }
    out
}

/// Look tests: SPHERE_TEST_WHEEL="app,pane,steps" scrolls a pane (steps > 0 = down) once, five seconds in.
pub fn test_wheel(sim: Res<crate::core::Sim>, mut scrolls: ResMut<AppScrolls>, mut done: Local<bool>) {
    if *done || sim.t < 5.0 {
        return;
    }
    *done = true;
    let Ok(v) = std::env::var("SPHERE_TEST_WHEEL") else { return };
    let f: Vec<i32> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
    if f.len() == 3 {
        scrolls.push(f[0] as u8, Some(f[1] as u8), f[2]);
    }
}

/// Look tests: SPHERE_TEST_CLICK="app,pane,x,y" clicks there (pane px; pane -1 is a quad with no pane, the moment card is app 200) once, four seconds in.
pub fn test_click(sim: Res<crate::core::Sim>, mut clicks: ResMut<PanelClicks>, quads: Query<(Entity, &AppPart), With<PaneRoot>>, mut done: Local<bool>) {
    if *done || sim.t < 4.0 {
        return;
    }
    *done = true;
    let Ok(v) = std::env::var("SPHERE_TEST_CLICK") else { return };
    let f: Vec<f32> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
    if f.len() != 4 {
        return;
    }
    if let Some((e, _)) = quads.iter().find(|(_, p)| p.app == f[0] as u8 && p.pane == (f[1] >= 0.0).then_some(f[1] as u8)) {
        clicks.0.push((e, Vec2::new(f[2], f[3])));
    }
}

impl Ui<'_> {
    /// A straight line from `a` to `b` (px in `parent`, which lays its children out absolutely).
    pub fn line(&self, c: &mut Commands, parent: Entity, a: Vec2, b: Vec2, w: f32, col: Color) {
        let d = b - a;
        let len = d.length();
        let mid = (a + b) / 2.0;
        let e = c
            .spawn((
                Node { position_type: PositionType::Absolute, left: px(mid.x - len / 2.0), top: px(mid.y - w / 2.0), width: px(len), height: px(w), ..default() },
                BackgroundColor(col),
                UiTransform { rotation: Rot2::radians(d.y.atan2(d.x)), ..default() },
            ))
            .id();
        c.entity(parent).add_child(e);
    }
    /// A dashed line: `dash` px on, `gap` px off.
    #[allow(clippy::too_many_arguments)]
    pub fn dashed(&self, c: &mut Commands, parent: Entity, a: Vec2, b: Vec2, w: f32, col: Color, dash: f32, gap: f32) {
        let len = (b - a).length();
        let dir = (b - a) / len.max(1e-3);
        let mut s = 0.0;
        while s < len {
            let e = (s + dash).min(len);
            self.line(c, parent, a + dir * s, a + dir * e, w, col);
            s += dash + gap;
        }
    }
}

/// What a finished agent's chip says: "Merged" once its branch is in the repo, "Finished" before.
pub fn finished(t: &sphere_proto::TaskView, merged: &std::collections::HashSet<String>) -> &'static str {
    if merged.contains(&t.id) {
        "Merged"
    } else {
        "Finished"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wheel_accumulates_into_steps() {
        let mut world = World::new();
        let (a, b) = (world.spawn_empty().id(), world.spawn_empty().id());
        let mut w = WheelAcc::default();
        assert_eq!(w.feed(a, -15.0), 0);
        assert_eq!(w.feed(a, -15.0), 0);
        assert_eq!(w.feed(a, -15.0), 1);
        assert_eq!(w.feed(a, 85.0), -2);
        // 5 left over; the other pane starts over
        assert_eq!(w.feed(b, -30.0), 0);
        assert_eq!(w.feed(b, -30.0), 1);
        // a fling is capped
        assert_eq!(w.feed(b, -4000.0), WHEEL_MAX);
    }

    #[test]
    fn scrolls_go_to_their_app_and_become_keys() {
        let mut s = AppScrolls::default();
        s.push(1, Some(4), 2);
        s.push(2, Some(0), -1);
        assert_eq!(s.take(1), vec![(Some(4), 2)]);
        assert!(s.take(1).is_empty());
        assert_eq!(s.take(2), vec![(Some(0), -1)]);
        let keys = scroll_keys(vec![(Some(4), 2), (Some(9), 1), (Some(4), -1)], |p, down| (p == Some(4)).then_some(if down { KeyCode::KeyJ } else { KeyCode::KeyK }));
        assert_eq!(keys, vec![KeyCode::KeyJ, KeyCode::KeyJ, KeyCode::KeyK]);
    }
}

