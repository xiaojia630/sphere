//! The game's glass panels, in the swarm's own style and places: the score strip where the mission strip sits, Today
//! (your best, the streak, the Always rules you kept) where Orbits sits, Asking (every request waiting on you, with its
//! patience) where Run slots sits, the request card at the bottom, and the title, pause and results panel over the
//! core in an atlas of its own. Content is rebuilt when what it shows changes; the numbers that move every frame (the
//! score, the clocks, the patience bars) are set in place by `live`.
use super::content::Tool;
use super::engine::{self, Mark, Req, SHIELDS};
use super::{Game, Hold, Screen, ATLAS, ATLAS_BIG};
use crate::appframe::{clip, AppPart, Click, PaneRoot, Ui};
use crate::fonts::UiFonts;
use crate::panel::*;
use crate::swarm::hud::Keepout;
use crate::swarm::Swarm;
use crate::util::*;
use bevy::prelude::*;
use bevy::text::LineHeight;

#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Top,
    Today,
    Asking,
    Card,
    Big,
}

/// On a panel's quad: where it rests.
#[derive(Component)]
pub struct GamePanel {
    pub kind: Kind,
    base: Vec3,
}

#[derive(Resource)]
pub struct Roots {
    top: Entity,
    today: Entity,
    asking: Entity,
    card: Entity,
    big: Entity,
}

/// A node `live` keeps up to date every frame.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Live {
    Score,
    Shipped,
    Combo,
    Waited,
    Time,
    /// the i-th shield's diamond
    Shield(u32),
    /// the card's "BASH · 2.1 S" (the request's tool and how long it has waited)
    CardAge,
    /// the card's patience bar
    CardBar,
    /// an Asking row's seconds left, and its bar
    RowLeft(usize),
    RowBar(usize),
}

pub const TOP: (f32, f32) = (790.0, 42.0);
pub const TODAY: (f32, f32) = (316.0, 232.0);
pub const ASKING: (f32, f32) = (316.0, 290.0);
pub const CARD: (f32, f32) = (700.0, 200.0);
pub const BIG: (f32, f32) = (640.0, 440.0);
/// Asking shows this many requests.
const ROWS: usize = 5;

/// The game atlas at `scale` texels per stage px: its width and the strip's, Today's, Asking's and the card's
/// top-left texels (the strip on top, the side panels under it, the card under them).
pub fn pack(scale: f32) -> (u32, [(u32, u32); 4]) {
    let px = |v: f32| (v * scale).ceil() as u32;
    let g = 2;
    let (tw, aw) = (px(TODAY.0), px(ASKING.0));
    let y1 = px(TOP.1) + g;
    let y2 = y1 + px(TODAY.1).max(px(ASKING.1)) + g;
    let w = px(TOP.0).max(tw + g + aw).max(px(CARD.0)) + g;
    (w, [(0, 0), (0, y1), (tw + g, y1), (0, y2)])
}

pub fn big_width(scale: f32) -> u32 {
    (BIG.0 * scale).ceil() as u32 + 2
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn setup(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut images: ResMut<Assets<Image>>, (home, mut atlases): (Res<PanelAtlas>, ResMut<AppAtlases>), mut pmats: ResMut<Assets<PanelMaterial>>) {
    // the home atlas's scale, as the swarm's panels: they sit at the same depth, and share its glyph atlases
    let (aw, spots) = pack(home.scale);
    let mut atlas = Atlas::new(&mut commands, &mut images, home.scale, -18).with_width(aw);
    let mut batlas = Atlas::new(&mut commands, &mut images, home.scale, -19).with_width(big_width(home.scale));
    let c = &mut commands;
    let mut roots = Vec::new();
    // (kind, size, stage anchor, side, z, labels keep clear of it or fade under it)
    let list: [(Kind, (f32, f32), Vec3, f32, f32, Keepout); 5] = [
        (Kind::Top, TOP, crate::homepanels::anchor('c', 800.0, 't', 16.0, TOP.0, TOP.1), 0.0, 6.0, Keepout::Clear),
        (Kind::Today, TODAY, crate::homepanels::anchor('l', 54.0, 't', 92.0, TODAY.0, TODAY.1), 1.0, 6.0, Keepout::Clear),
        (Kind::Asking, ASKING, crate::homepanels::anchor('r', 1546.0, 't', 70.0, ASKING.0, ASKING.1), -1.0, 6.0, Keepout::Clear),
        (Kind::Card, CARD, crate::homepanels::anchor('c', 800.0, 'b', 836.0, CARD.0, CARD.1), 0.0, 120.0, Keepout::Clear),
        (Kind::Big, BIG, crate::homepanels::anchor('c', 800.0, 'm', 468.0, BIG.0, BIG.1), 0.0, 200.0, Keepout::Hide),
    ];
    for (i, (kind, (w, h), at, side, z, keep)) in list.into_iter().enumerate() {
        let ui = if kind == Kind::Big { spawn_ui_panel_at(c, &mut batlas, w, h, 0, 0) } else { spawn_ui_panel_at(c, &mut atlas, w, h, spots[i].0, spots[i].1) };
        let mut home = at;
        home.z = z;
        let e = spawn_panel_quad(c, &mut meshes, &mut pmats, &ui, &PanelSpec { id: PanelId::App, w, h, home, home_scale: 1.0, side, phase: 0.7 + i as f32 * 1.1, show_at: 0.0, unframed: false });
        c.entity(e).insert((GamePanel { kind, base: home }, AppPart { app: if kind == Kind::Big { ATLAS_BIG } else { ATLAS }, pane: Some(i as u8) }, PaneRoot(ui.root), keep));
        // the card and the big panel are the Agent moment card's glass: darker (they sit over the core's light; the
        // big one, all text, darker still), with an amber LED and bar, and its neutral hue
        if matches!(kind, Kind::Card | Kind::Big) {
            c.entity(e).insert(PanelHue(Some(crate::theme::Hue::Neutral)));
            let id = pmats.iter().find(|(_, m)| m.content.as_ref() == Some(&ui.image) && m.u.r == ui.rect).map(|(id, _)| id);
            if let Some(mut m) = id.and_then(|id| pmats.get_mut(id)) {
                let (from_c, to_c) = crate::theme::panel_map(Some(crate::theme::Hue::Neutral));
                m.u.from_c = from_c;
                m.u.to_c = to_c;
                m.u.fill = m.u.acc;
                m.u.p.y = if kind == Kind::Big { 0.94 } else { 0.86 };
                m.u.p.w = 1.0;
            }
        }
        roots.push(ui.root);
    }
    c.insert_resource(Roots { top: roots[0], today: roots[1], asking: roots[2], card: roots[3], big: roots[4] });
    atlases.add(ATLAS, atlas);
    atlases.add(ATLAS_BIG, batlas);
}

/// Which panels a screen shows.
pub fn shown(s: Screen, k: Kind) -> bool {
    match k {
        Kind::Top | Kind::Today => matches!(s, Screen::Play | Screen::Paused),
        Kind::Asking | Kind::Card => s == Screen::Play,
        Kind::Big => matches!(s, Screen::Title | Screen::Paused | Screen::Over),
    }
}

/// The panels follow the swarm's opening and the screen; a panel that comes in draws its frame in again, and a key
/// that does nothing shakes the card.
pub fn place(mut game: ResMut<Game>, swarm: Res<Swarm>, mut atlases: ResMut<AppAtlases>, mut q: Query<(&mut Panel, &GamePanel)>, mut was: Local<[bool; 5]>) {
    let on = game.on() && (swarm.open || swarm.amt.x > 0.001);
    let amt = swarm.amt.x.clamp(0.0, 1.0);
    let mut big_up = false;
    let mut any = false;
    let nudge = std::mem::take(&mut game.nudge);
    for (mut p, g) in &mut q {
        let show = on && shown(game.screen, g.kind);
        let i = g.kind as usize;
        if show && !was[i] {
            p.draw_t = 0.0;
        }
        was[i] = show;
        let visible = show || p.op.x > 0.005;
        if g.kind == Kind::Big {
            big_up = on && visible;
        } else {
            any |= on && visible;
        }
        if g.kind == Kind::Card && nudge {
            p.tilt.x += 0.06;
        }
        p.work = Some((if show { amt } else { 0.0 }, 0.0));
        p.home = g.base;
        p.home_scale = 1.0;
    }
    atlases.want(ATLAS, any || (on && game.screen != Screen::Off));
    atlases.want(ATLAS_BIG, big_up);
}

// ------------------------------------------------------------------ colours and small pieces

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

fn row(c: &mut Commands, gap: f32) -> Entity {
    c.spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(gap), ..default() }).id()
}

fn spacer(c: &mut Commands) -> Entity {
    c.spawn(Node { flex_grow: 1.0, ..default() }).id()
}

/// Mono at the home panels' one size (they share its glyph texture).
fn mono(ui: &Ui, c: &mut Commands, s: impl Into<String>, color: Color) -> Entity {
    ui.text(c, s, &ui.f.mono, 11.5, 0.0, color)
}

/// A paragraph that wraps at `w` px.
fn para(c: &mut Commands, f: &UiFonts, s: &str, size: f32, color: Color, w: f32) -> Entity {
    c.spawn((Text::new(s), font(&f.ui500, size, size * 0.01), TextColor(color), LineHeight::Px(size * 1.45), Node { max_width: px(w), ..default() })).id()
}

/// A small caps caption: "SCORE", "BEST TODAY".
fn cap(ui: &Ui, c: &mut Commands, s: &str, color: Color) -> Entity {
    ui.text(c, s.to_uppercase(), &ui.f.ui600, 9.5, 2.4, color)
}

/// A key cap: "⏎", "E", "⇧ ⏎".
fn kbd(c: &mut Commands, f: &UiFonts, k: &str, color: Color) -> Entity {
    let b = c.spawn((Node { padding: UiRect { left: px(5), right: px(5), top: px(2), bottom: px(1) }, border: UiRect::all(px(1)), flex_shrink: 0.0, ..default() }, BorderColor::all(with_alpha(color, 0.5)))).id();
    let t = c.spawn((Text::new(k), font(&f.mono_bold, 9.5, 0.0), TextColor(with_alpha(color, 0.8)), LineHeight::Px(11.0))).id();
    c.entity(b).add_child(t);
    b
}

/// The moment card's buttons: filled amber, framed, or a ghost; a key cap after the label.
fn button(c: &mut Commands, f: &UiFonts, label: &str, key: &str, look: u8, click: Click) -> Entity {
    let (fill, ghost) = (look == 0, look == 2);
    let col = if fill { acc() } else { with_alpha(line(), 0.34) };
    let b = c
        .spawn((
            Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(10), padding: if fill { UiRect { left: px(20), right: px(14), top: px(10), bottom: px(9) } } else { UiRect { left: px(15), right: px(11), top: px(9), bottom: px(8) } }, border: UiRect::all(px(1)), flex_shrink: 0.0, ..default() },
            BackgroundColor(if fill { acc() } else { Color::NONE }),
            BorderColor::all(if ghost { Color::NONE } else { col }),
            click,
        ))
        .id();
    let fg = if fill { Color::BLACK } else if ghost { line() } else { hot() };
    let t = c.spawn((Text::new(label.to_uppercase()), font(&f.ui700, if fill { 12.0 } else { 11.0 }, if fill { 3.2 } else { 2.6 }), TextColor(fg), LineHeight::Px(12.5))).id();
    let k = kbd(c, f, key, fg);
    c.entity(b).add_children(&[t, k]);
    b
}

/// A chip for the other ways to say yes (Always, Allow all).
fn chip(c: &mut Commands, f: &UiFonts, label: &str, code: Option<&str>, key: &str, color: Color, click: Click) -> Entity {
    let b = c
        .spawn((
            Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(7), padding: UiRect { left: px(11), right: px(9), top: px(7), bottom: px(6) }, border: UiRect::all(px(1)), flex_shrink: 1.0, overflow: Overflow::clip(), ..default() },
            BorderColor::all(color),
            click,
        ))
        .id();
    let t = c.spawn((Text::new(label.to_uppercase()), font(&f.ui700, 10.5, 2.2), TextColor(if color == acc() { acc() } else { hot() }), LineHeight::Px(12.0), TextLayout { linebreak: LineBreak::NoWrap, ..default() })).id();
    c.entity(b).add_child(t);
    if let Some(code) = code {
        let ct = c.spawn((Text::new(code), font(&f.mono, 11.5, 0.0), TextColor(hot()), LineHeight::Px(13.0), TextLayout { linebreak: LineBreak::NoWrap, ..default() })).id();
        c.entity(b).add_child(ct);
    }
    let k = kbd(c, f, key, line());
    c.entity(b).add_child(k);
    b
}

/// A shield: a diamond, lit while it holds.
fn shield(c: &mut Commands, i: u32) -> Entity {
    c.spawn((Node { width: px(8), height: px(8), border: UiRect::all(px(1)), flex_shrink: 0.0, ..default() }, BackgroundColor(ok()), BorderColor::all(ok()), UiTransform { rotation: Rot2::degrees(45.0), ..default() }, Live::Shield(i))).id()
}

/// What the tool's line is called on the card.
fn tool_label(t: Tool) -> &'static str {
    match t {
        Tool::Bash => "Command",
        Tool::Edit => "Edit",
        Tool::Write => "New file",
        Tool::WebFetch => "Fetch",
    }
}

fn mark_col(m: Mark) -> Color {
    match m {
        Mark::Yes => ok(),
        Mark::Caught => fill(),
        Mark::Incident => bad(),
        Mark::Slip => acc(),
    }
}

// ------------------------------------------------------------------ drawing

/// What each panel last drew, so it only rebuilds when that changes.
#[derive(Default)]
pub struct Seen {
    top: String,
    today: String,
    asking: String,
    card: String,
    big: String,
}

pub fn draw(game: Res<Game>, swarm: Res<Swarm>, roots: Res<Roots>, f: Res<UiFonts>, theme: Res<crate::theme::Theme>, mut commands: Commands, mut seen: Local<Seen>) {
    if !game.on() || !(swarm.open || swarm.amt.x > 0.001) {
        if !seen.top.is_empty() {
            *seen = Seen::default();
        }
        return;
    }
    let c = &mut commands;
    let ui = Ui::new(&f);
    let g = &*game;
    let r = &g.run;
    let th = theme.version;
    let top = format!("{:?}|{}|{th}", g.screen, g.day.key);
    if seen.top != top {
        seen.top = top;
        c.entity(roots.top).despawn_children();
        build_top(c, &ui, roots.top, g);
    }
    let today = format!("{:?}|{}|{}|{}|{}|{:?}|{th}", g.screen, g.record.best_today(&g.day), g.record.streak_now(&g.day), g.record.top, g.day.key, r.rules);
    if seen.today != today {
        seen.today = today;
        c.entity(roots.today).despawn_children();
        build_today(c, &ui, roots.today, g);
    }
    let ids: Vec<u32> = r.queue.iter().take(ROWS).map(|q| q.id).collect();
    let asking = format!("{:?}|{ids:?}|{}|{}|{th}", g.screen, r.queue.len(), r.cur);
    if seen.asking != asking {
        seen.asking = asking;
        c.entity(roots.asking).despawn_children();
        build_asking(c, &ui, roots.asking, g);
    }
    let cur = r.current();
    let card = format!("{:?}|{:?}|{}|{}|{}|{th}", g.screen, cur.map(|q| q.id), cur.map(|q| r.same_count(q)).unwrap_or(0), r.queue.len(), r.cur);
    if seen.card != card {
        seen.card = card;
        c.entity(roots.card).despawn_children();
        build_card(c, &ui, roots.card, g);
    }
    let big = format!("{:?}|{:?}|{}|{}|{}|{th}", g.screen, g.hold, g.best, g.copied > 0.0, r.marks.len());
    if seen.big != big {
        seen.big = big;
        c.entity(roots.big).despawn_children();
        match g.screen {
            Screen::Title => build_title(c, &ui, roots.big, g),
            Screen::Paused => build_paused(c, &ui, roots.big, g),
            Screen::Over => build_over(c, &ui, roots.big, g),
            _ => {}
        }
    }
}

/// m:ss
fn clock(t: f64) -> String {
    let s = t.max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// Sets what moves every frame: the strip's numbers, the shields, the card's wait and patience, the Asking rows'.
#[allow(clippy::type_complexity)]
pub fn live(game: Res<Game>, mut texts: Query<(&Live, &mut Text, &mut TextColor)>, mut nodes: Query<(&Live, &mut Node, &mut BackgroundColor), Without<Text>>) {
    if !game.on() {
        return;
    }
    let r = &game.run;
    let cur = r.current();
    let left = |i: usize| r.queue.get(i).map(|q| q.left.max(0.0) / q.patience.max(0.1));
    for (l, mut t, mut col) in &mut texts {
        let (s, color) = match *l {
            Live::Score => (engine::thousands(r.score), hot()),
            Live::Shipped => (r.shipped.to_string(), hot()),
            Live::Combo => (format!("×{}", r.mult()), if r.mult() > 1 { acc() } else { hot() }),
            Live::Waited => (format!("{:.1}s", r.live_waited()), hot()),
            Live::Time => (clock(r.t), hot()),
            Live::CardAge => match cur {
                Some(q) => (format!("{} · {:.1} S", q.tool.name().to_uppercase(), (r.t - q.born).max(0.0)), with_alpha(line(), 0.62)),
                None => (String::new(), line()),
            },
            Live::RowLeft(i) => match r.queue.get(i) {
                Some(q) => (format!("{:.1}s", q.left.max(0.0)), if q.left < 2.0 { bad() } else { acc() }),
                None => (String::new(), line()),
            },
            _ => continue,
        };
        if t.0 != s {
            t.0 = s;
        }
        col.set_if_neq(TextColor(color));
    }
    for (l, mut n, mut bg) in &mut nodes {
        match *l {
            Live::Shield(i) => {
                bg.set_if_neq(BackgroundColor(if i < r.shields { ok() } else { Color::NONE }));
            }
            Live::CardBar | Live::RowBar(_) => {
                let k = match *l {
                    Live::CardBar => cur.map(|q| q.left.max(0.0) / q.patience.max(0.1)),
                    Live::RowBar(i) => left(i),
                    _ => None,
                }
                .unwrap_or(0.0) as f32;
                // whole percents: the atlas redraws only when a bar visibly moves
                let w = percent((k * 100.0).round());
                if n.width != w {
                    n.width = w;
                }
                bg.set_if_neq(BackgroundColor(if k < 0.3 { bad() } else { acc() }));
            }
            _ => {}
        }
    }
}

// ------------------------------------------------------------------ the strip

fn build_top(c: &mut Commands, ui: &Ui, root: Entity, g: &Game) {
    let r = c.spawn(Node { width: percent(100), height: percent(100), flex_direction: FlexDirection::Row, align_items: AlignItems::Center, justify_content: JustifyContent::Center, column_gap: px(14), padding: UiRect { left: px(18), right: px(16), ..default() }, ..default() }).id();
    c.entity(root).add_child(r);
    let run = ui.text(c, g.day.run_label().to_uppercase(), &ui.f.ui700, 11.5, 2.4, acc());
    c.entity(r).add_child(run);
    let dot = |c: &mut Commands| ui.text(c, "·", &ui.f.ui500, 10.5, 0.0, dim());
    for (i, (live, label)) in [(Live::Score, "Score"), (Live::Shipped, "Shipped"), (Live::Combo, "Combo")].into_iter().enumerate() {
        let d = dot(c);
        let seg = row(c, 6.0);
        let n = mono(ui, c, "", hot());
        c.entity(n).insert(live);
        let l = ui.text(c, label.to_uppercase(), &ui.f.ui600, 11.5, 2.0, line());
        c.entity(seg).add_children(&[n, l]);
        if i == 0 {
            c.entity(n).insert(Node { min_width: px(42), justify_content: JustifyContent::FlexEnd, ..default() });
        }
        c.entity(r).add_children(&[d, seg]);
    }
    // shields, the wait and the clock after a rule
    let rest = c.spawn((Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(14), padding: UiRect::left(px(14)), border: UiRect::left(px(1)), ..default() }, BorderColor::all(with_alpha(line(), 0.34)))).id();
    let sh = row(c, 7.0);
    for i in 0..SHIELDS {
        let s = shield(c, i);
        c.entity(sh).add_child(s);
    }
    let sl = ui.text(c, "SHIELDS", &ui.f.ui600, 11.5, 2.0, line());
    c.entity(sh).add_child(sl);
    let w = row(c, 6.0);
    let wn = mono(ui, c, "", hot());
    c.entity(wn).insert(Live::Waited);
    let wl = ui.text(c, "WAITED", &ui.f.ui600, 11.5, 2.0, line());
    c.entity(w).add_children(&[wn, wl]);
    let tm = mono(ui, c, "", hot());
    c.entity(tm).insert(Live::Time);
    c.entity(rest).add_children(&[sh, w, tm]);
    c.entity(r).add_child(rest);
}

// ------------------------------------------------------------------ Today

fn build_today(c: &mut Commands, ui: &Ui, root: Entity, g: &Game) {
    let body = ui.body(c, root);
    let hd = ui.header(c, "Today", &format!("{} · {}", g.day.run_label(), g.day.label).to_uppercase(), dim());
    let list = c.spawn(Node { flex_direction: FlexDirection::Column, padding: UiRect { left: px(18), right: px(18), top: px(2), ..default() }, ..default() }).id();
    c.entity(body).add_children(&[hd, list]);
    let best = g.record.best_today(&g.day);
    let streak = g.record.streak_now(&g.day);
    let rows = [
        ("Best today", if best > 0 { engine::thousands(best) } else { "—".to_string() }),
        ("Streak", format!("{streak} day{}", if streak == 1 { "" } else { "s" })),
        ("Best ever", if g.record.top > 0 { engine::thousands(g.record.top) } else { "—".to_string() }),
    ];
    for (k, v) in rows {
        let r = c.spawn((Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, padding: UiRect::vertical(px(7)), border: UiRect::bottom(px(1)), ..default() }, BorderColor::all(with_alpha(line(), 0.15)))).id();
        let kt = ui.text(c, k.to_uppercase(), &ui.f.ui600, 11.0, 2.2, line());
        let sp = spacer(c);
        let vt = mono(ui, c, v.to_uppercase(), hot());
        c.entity(r).add_children(&[kt, sp, vt]);
        c.entity(list).add_child(r);
    }
    // the Always rules kept this run: what answers for you
    let q = c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(4), margin: UiRect::top(px(10)), ..default() }).id();
    let n = g.run.rules.len();
    let ht = ui.text(c, if n == 0 { "ALWAYS RULES".to_string() } else { format!("ALWAYS RULES · {n}") }, &ui.f.ui600, 10.0, 3.4, line());
    c.entity(q).add_child(ht);
    if n == 0 {
        let t = ui.text(c, "None yet · A keeps one", &ui.f.ui500, 12.0, 0.2, dim());
        c.entity(q).add_child(t);
    }
    for rule in g.run.rules.iter().rev().take(3) {
        let r = row(c, 8.0);
        let b = c.spawn((Node { width: px(5), height: px(5), flex_shrink: 0.0, ..default() }, BackgroundColor(fill()))).id();
        let tt = mono(ui, c, clip(rule, 36), hot());
        c.entity(r).add_children(&[b, tt]);
        c.entity(q).add_child(r);
    }
    if n > 3 {
        let more = mono(ui, c, format!("+{} more", n - 3), dim());
        c.entity(q).add_child(more);
    }
    c.entity(list).add_child(q);
}

// ------------------------------------------------------------------ Asking

fn build_asking(c: &mut Commands, ui: &Ui, root: Entity, g: &Game) {
    let r = &g.run;
    let body = ui.body(c, root);
    let n = r.queue.len();
    let (right, col) = if n == 0 { ("ALL CLEAR".to_string(), dim()) } else { (format!("{n} WAITING"), acc()) };
    let hd = ui.header(c, "Asking", &right, col);
    let list = c.spawn(Node { flex_direction: FlexDirection::Column, padding: UiRect { left: px(18), right: px(18), top: px(4), ..default() }, ..default() }).id();
    c.entity(body).add_children(&[hd, list]);
    if n == 0 {
        let t = para(c, ui.f, "Agents are working. What they ask lands here, and on the card.", 12.5, dim(), 270.0);
        c.entity(t).insert(Node { margin: UiRect::top(px(8)), max_width: px(270), ..default() });
        c.entity(list).add_child(t);
        return;
    }
    for (i, q) in r.queue.iter().enumerate().take(ROWS) {
        let on = i == r.cur;
        let a = &r.agents[q.agent];
        let wrap = c.spawn((Node { flex_direction: FlexDirection::Column, row_gap: px(3), padding: UiRect { left: px(10), top: px(5), bottom: px(4), ..default() }, ..default() }, Click::Pick(i))).id();
        if on {
            let bar = c.spawn((Node { position_type: PositionType::Absolute, left: px(0), top: px(5), bottom: px(6), width: px(2), ..default() }, BackgroundColor(acc()))).id();
            c.entity(wrap).add_child(bar);
        }
        let top = row(c, 8.0);
        let title = format!("{}{}", a.task[..1].to_uppercase(), &a.task[1..]);
        let tt = ui.text(c, clip(&title, 24), &ui.f.ui600, 12.5, 0.3, if on { acc() } else { hot() });
        let sp = spacer(c);
        let lt = mono(ui, c, "", acc());
        c.entity(lt).insert(Live::RowLeft(i));
        c.entity(top).add_children(&[tt, sp, lt]);
        let cmd = mono(ui, c, clip(&format!("{} {}", q.tool.name(), q.text), 40), if on { hot() } else { line() });
        let track = c.spawn((Node { height: px(2), margin: UiRect::top(px(2)), ..default() }, BackgroundColor(with_alpha(line(), 0.14)))).id();
        let fl = c.spawn((Node { width: percent(100), height: px(2), ..default() }, BackgroundColor(acc()), Live::RowBar(i))).id();
        c.entity(track).add_child(fl);
        c.entity(wrap).add_children(&[top, cmd, track]);
        c.entity(list).add_child(wrap);
    }
    if n > ROWS {
        let more = mono(ui, c, format!("+{} more · ← → to reach them", n - ROWS), dim());
        c.entity(more).insert(Node { margin: UiRect::top(px(4)), ..default() });
        c.entity(list).add_child(more);
    }
}

// ------------------------------------------------------------------ the card

fn build_card(c: &mut Commands, ui: &Ui, root: Entity, g: &Game) {
    let r = &g.run;
    let f = ui.f;
    let g3 = with_alpha(line(), 0.13);
    let body = c.spawn(Node { width: percent(100), height: percent(100), flex_direction: FlexDirection::Column, padding: UiRect { left: px(24), right: px(24), top: px(14), bottom: px(14) }, ..default() }).id();
    c.entity(root).add_child(body);
    let req: Option<&Req> = r.current();
    // header: chip, title, where and how long
    let hdr = c.spawn((Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(12), padding: UiRect { bottom: px(10), ..default() }, border: UiRect { bottom: px(1), ..default() }, ..default() }, BorderColor::all(with_alpha(line(), 0.4)))).id();
    let nowrap = TextLayout { linebreak: LineBreak::NoWrap, ..default() };
    let (chip_t, title) = match req {
        Some(q) => {
            let a = &r.agents[q.agent];
            let same = r.same_count(q);
            (if same > 1 { format!("{same} ASK THE SAME") } else { "NEEDS YOU".to_string() }, format!("{} · {}", a.repo, a.task))
        }
        None => ("WORKING".to_string(), "All clear".to_string()),
    };
    let busy = req.is_some();
    let chip_n = c.spawn((Node { padding: UiRect { left: px(8), right: px(9), top: px(5), bottom: px(4) }, column_gap: px(6), align_items: AlignItems::Center, flex_shrink: 0.0, border: UiRect::all(px(1)), ..default() }, BackgroundColor(if busy { acc() } else { Color::NONE }), BorderColor::all(if busy { acc() } else { with_alpha(line(), 0.5) }))).id();
    let dot = c.spawn((Node { width: px(5), height: px(5), ..default() }, BackgroundColor(if busy { Color::BLACK } else { fill() }))).id();
    let ct = c.spawn((Text::new(chip_t), font(&f.ui700, 10.5, 2.31), TextColor(if busy { Color::BLACK } else { line() }), LineHeight::Px(10.5), nowrap)).id();
    c.entity(chip_n).add_children(&[dot, ct]);
    let tt = c.spawn((Text::new(clip(&title.to_uppercase(), 34)), font(&f.ui600, 14.0, 2.6), TextColor(hot()), LineHeight::Px(17.0), nowrap, Node { flex_grow: 1.0, flex_shrink: 1.0, overflow: Overflow::clip(), ..default() })).id();
    let pos = c.spawn((Text::new(if r.queue.len() > 1 { format!("{} OF {}", r.cur + 1, r.queue.len()) } else { String::new() }), font(&f.mono, 11.0, 0.66), TextColor(hot()), LineHeight::Px(13.0), nowrap, Node { flex_shrink: 0.0, ..default() })).id();
    let age = c.spawn((Text::new(""), font(&f.ui500, 11.0, 1.76), TextColor(with_alpha(line(), 0.62)), LineHeight::Px(13.0), nowrap, Node { flex_shrink: 0.0, ..default() }, Live::CardAge)).id();
    c.entity(hdr).add_children(&[chip_n, tt, pos, age]);
    let tick = c.spawn((Node { position_type: PositionType::Absolute, left: px(0), bottom: px(-1), width: px(44), height: px(2), ..default() }, BackgroundColor(if busy { acc() } else { with_alpha(line(), 0.5) }))).id();
    c.entity(hdr).add_child(tick);
    c.entity(body).add_child(hdr);
    // the exact action, or the idle line
    let draft = c.spawn((Node { flex_direction: FlexDirection::Column, row_gap: px(7), margin: UiRect { top: px(12), ..default() }, padding: UiRect { left: px(16), top: px(6), bottom: px(2), ..default() }, border: UiRect { left: px(1), ..default() }, ..default() }, BorderColor::all(with_alpha(line(), 0.34)))).id();
    let bar = c.spawn((Node { position_type: PositionType::Absolute, left: px(-2), top: px(0), width: px(3), height: px(20), ..default() }, BackgroundColor(if busy { acc() } else { with_alpha(line(), 0.4) }))).id();
    c.entity(draft).add_child(bar);
    match req {
        Some(q) => {
            let dl = c.spawn((Text::new(tool_label(q.tool).to_uppercase()), font(&f.ui700, 9.5, 3.04), TextColor(line()), LineHeight::Px(9.5))).id();
            let dt = c.spawn((Text::new(clip(&q.text, 70)), font(&f.mono, 15.0, 0.0), TextColor(hot()), LineHeight::Px(20.0), nowrap)).id();
            c.entity(draft).add_children(&[dl, dt]);
            if !q.detail.is_empty() {
                let d = c.spawn((Text::new(clip(q.detail, 84)), font(&f.ui500, 12.0, 0.12), TextColor(with_alpha(line(), 0.8)), LineHeight::Px(15.0), nowrap)).id();
                c.entity(draft).add_child(d);
            }
        }
        None => {
            let dl = c.spawn((Text::new("NOTHING NEEDS YOU"), font(&f.ui700, 9.5, 3.04), TextColor(line()), LineHeight::Px(9.5))).id();
            let dt = c.spawn((Text::new("Agents are working. The next ask rises here."), font(&f.ui500, 16.0, 0.16), TextColor(line()), LineHeight::Px(20.0), nowrap)).id();
            c.entity(draft).add_children(&[dl, dt]);
        }
    }
    c.entity(body).add_child(draft);
    // patience
    let track = c.spawn((Node { height: px(2), margin: UiRect::top(px(11)), ..default() }, BackgroundColor(g3))).id();
    let fl = c.spawn((Node { width: percent(if busy { 100.0 } else { 0.0 }), height: px(2), ..default() }, BackgroundColor(acc()), Live::CardBar)).id();
    c.entity(track).add_child(fl);
    c.entity(body).add_child(track);
    let sp = spacer(c);
    c.entity(body).add_child(sp);
    // actions: yes, change, not now; the other ways to say yes on the right
    let acts = c.spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(10), ..default() }).id();
    let yes = req.map(|q| q.tool.verb()).unwrap_or("Run");
    let b1 = button(c, f, yes, "⏎", 0, Click::Key(KeyCode::Enter));
    let b2 = button(c, f, "Change", "E", 1, Click::Key(KeyCode::KeyE));
    let b3 = button(c, f, "Not now", "L", 2, Click::Key(KeyCode::KeyL));
    c.entity(acts).add_children(&[b1, b2, b3]);
    let gap = spacer(c);
    c.entity(acts).add_child(gap);
    if let Some(q) = req {
        if let Some(rule) = &q.rule {
            let rc = clip(rule, 22);
            let ch = chip(c, f, "Always", Some(rc.as_str()), "A", with_alpha(line(), 0.34), Click::Key(KeyCode::KeyA));
            c.entity(acts).add_child(ch);
        }
        let same = r.same_count(q);
        if same > 1 {
            let ch = chip(c, f, &format!("Allow all {same}"), None, "⇧⏎", acc(), Click::Alt(0));
            c.entity(acts).add_child(ch);
        }
    }
    if !busy {
        // nothing to answer: the buttons rest
        for b in [b1, b2, b3] {
            c.entity(b).insert(BorderColor::all(with_alpha(line(), 0.18)));
        }
        c.entity(b1).insert(BackgroundColor(with_alpha(acc(), 0.18)));
    }
    c.entity(body).add_child(acts);
}

// ------------------------------------------------------------------ title, pause, results

/// The big panel's body: padding and a column.
fn big_body(c: &mut Commands, root: Entity) -> Entity {
    let b = c.spawn(Node { width: percent(100), height: percent(100), flex_direction: FlexDirection::Column, padding: UiRect { left: px(34), right: px(34), top: px(26), bottom: px(24) }, ..default() }).id();
    c.entity(root).add_child(b);
    b
}

fn wordmark(c: &mut Commands, f: &UiFonts, s: &str, size: f32) -> Entity {
    c.spawn((Text::new(s), font(&f.display, size, size * 0.28), TextColor(hot()), LineHeight::Px(size * 1.2), TextLayout { linebreak: LineBreak::NoWrap, ..default() })).id()
}

fn stat(ui: &Ui, c: &mut Commands, k: &str, v: String, col: Color) -> Entity {
    let s = c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(5), ..default() }).id();
    let kt = cap(ui, c, k, line());
    let vt = ui.text(c, v, &ui.f.mono, 15.0, 0.0, col);
    c.entity(s).add_children(&[kt, vt]);
    s
}

fn actions(c: &mut Commands, list: &[Entity]) -> Entity {
    let a = c.spawn((Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(12), padding: UiRect::top(px(16)), border: UiRect::top(px(1)), ..default() }, BorderColor::all(with_alpha(line(), 0.13)))).id();
    c.entity(a).add_children(list);
    a
}

/// The answer keys, two columns of three.
fn keys(c: &mut Commands, ui: &Ui) -> Entity {
    let f = ui.f;
    let grid = c.spawn(Node { flex_direction: FlexDirection::Row, flex_wrap: FlexWrap::Wrap, row_gap: px(9), margin: UiRect { top: px(18), bottom: px(18), ..default() }, ..default() }).id();
    for (k, what) in [("⏎", "Run or apply it"), ("E", "Change it: block it"), ("A", "Always allow it"), ("⇧ ⏎", "Allow it for every agent asking"), ("L", "Not now: the next one"), ("← →", "Step through the asks")] {
        let r = row(c, 10.0);
        c.entity(r).insert(Node { width: percent(50), flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(10), ..default() });
        let kc = c.spawn(Node { width: px(44), flex_shrink: 0.0, ..default() }).id();
        let kb = kbd(c, f, k, hot());
        c.entity(kc).add_child(kb);
        let t = ui.text(c, what, &f.ui500, 13.0, 0.2, hot());
        c.entity(r).add_children(&[kc, t]);
        c.entity(grid).add_child(r);
    }
    grid
}

fn build_title(c: &mut Commands, ui: &Ui, root: Entity, g: &Game) {
    let f = ui.f;
    let body = big_body(c, root);
    let meta = cap(ui, c, &format!("A Sphere game · {} · {}", g.day.run_label(), g.day.label), acc());
    let wm = wordmark(c, f, "HOLD THE SWARM", 30.0);
    c.entity(wm).insert(Node { margin: UiRect { top: px(10), bottom: px(12), ..default() }, ..default() });
    let lede = para(c, f, "Agents circle the core and ask before they act. Answer with Sphere's own keys and keep them moving, but don't let an rm -rf ~ through. Everyone plays the same run today.", 15.0, line(), 560.0);
    let grid = keys(c, ui);
    c.entity(body).add_children(&[meta, wm, lede, grid]);
    let stats = c.spawn((Node { flex_direction: FlexDirection::Row, column_gap: px(34), padding: UiRect::vertical(px(12)), border: UiRect::top(px(1)), ..default() }, BorderColor::all(with_alpha(line(), 0.13)))).id();
    let best = g.record.best_today(&g.day);
    let streak = g.record.streak_now(&g.day);
    let s1 = stat(ui, c, "Your best today", if best > 0 { engine::thousands(best) } else { "—".into() }, hot());
    let s2 = stat(ui, c, "Streak", format!("{streak} day{}", if streak == 1 { "" } else { "s" }), hot());
    let s3 = stat(ui, c, "Best ever", if g.record.top > 0 { engine::thousands(g.record.top) } else { "—".into() }, hot());
    c.entity(stats).add_children(&[s1, s2, s3]);
    let note = para(c, f, "Three shields. Letting a dangerous ask through, or leaving an agent waiting too long, costs one.", 12.5, dim(), 560.0);
    c.entity(note).insert(Node { margin: UiRect::bottom(px(4)), max_width: px(560), ..default() });
    let sp = spacer(c);
    let b1 = button(c, f, "Start today's run", "⏎", 0, Click::Key(KeyCode::Enter));
    let b2 = button(c, f, "Back", "ESC", 2, Click::Key(KeyCode::Escape));
    let sp2 = spacer(c);
    let hint = cap(ui, c, "Real requests pause it", dim());
    let acts = actions(c, &[b1, b2, sp2, hint]);
    c.entity(body).add_children(&[stats, note, sp, acts]);
}

fn build_paused(c: &mut Commands, ui: &Ui, root: Entity, g: &Game) {
    let f = ui.f;
    let body = big_body(c, root);
    let meta = cap(ui, c, &format!("{} · {} into the shift", g.day.run_label(), clock(g.run.t)), acc());
    let wm = wordmark(c, f, "PAUSED", 30.0);
    c.entity(wm).insert(Node { margin: UiRect { top: px(10), bottom: px(12), ..default() }, ..default() });
    let why = match g.hold {
        Some(Hold::Request) => "A real agent needs you. Answer it on its card, then come back: nothing here moved.",
        Some(Hold::Typing) => "You're typing. The agents hold until you're back.",
        Some(Hold::Away) => "Sphere went to the background. The agents hold until you're back.",
        None => "The agents are holding. Nothing ticks while you're away.",
    };
    let lede = para(c, f, why, 15.0, line(), 560.0);
    c.entity(body).add_children(&[meta, wm, lede]);
    let stats = c.spawn((Node { flex_direction: FlexDirection::Row, column_gap: px(34), margin: UiRect::top(px(22)), padding: UiRect::vertical(px(12)), border: UiRect::top(px(1)), ..default() }, BorderColor::all(with_alpha(line(), 0.13)))).id();
    let r = &g.run;
    let s1 = stat(ui, c, "Score", engine::thousands(r.score), hot());
    let s2 = stat(ui, c, "Shipped", r.shipped.to_string(), hot());
    let s3 = stat(ui, c, "Shields", format!("{} of {SHIELDS}", r.shields), if r.shields == 1 { bad() } else { hot() });
    let s4 = stat(ui, c, "Waiting on you", r.queue.len().to_string(), if r.queue.is_empty() { hot() } else { acc() });
    c.entity(stats).add_children(&[s1, s2, s3, s4]);
    // the keys again, for the way back in
    let grid = keys(c, ui);
    let sp = spacer(c);
    let b1 = button(c, f, "Resume", "⏎", 0, Click::Key(KeyCode::Enter));
    let b2 = button(c, f, "Leave the run", "ESC", 2, Click::Key(KeyCode::Escape));
    let acts = actions(c, &[b1, b2]);
    c.entity(body).add_children(&[stats, grid, sp, acts]);
}

fn build_over(c: &mut Commands, ui: &Ui, root: Entity, g: &Game) {
    let f = ui.f;
    let r = &g.run;
    let body = big_body(c, root);
    let end = match r.end {
        Some(engine::End::Stall) => "an agent waited too long",
        _ => "three incidents",
    };
    let meta = cap(ui, c, &format!("{} · Shift over · {end}", g.day.run_label()), acc());
    let head = wordmark(c, f, if g.best { "TODAY'S BEST" } else { "SHIFT OVER" }, 22.0);
    c.entity(head).insert(Node { margin: UiRect { top: px(10), ..default() }, ..default() });
    let score = c.spawn((Text::new(engine::thousands(r.score)), font(&f.display, 46.0, 4.0), TextColor(if g.best { acc() } else { hot() }), LineHeight::Px(56.0))).id();
    let pts = cap(ui, c, "points", line());
    c.entity(body).add_children(&[meta, head, score, pts]);
    let grid = c.spawn(Node { flex_direction: FlexDirection::Row, flex_wrap: FlexWrap::Wrap, row_gap: px(12), margin: UiRect { top: px(16), bottom: px(14), ..default() }, ..default() }).id();
    for (k, v, col) in [
        ("Shipped", r.shipped.to_string(), hot()),
        ("Caught", r.caught.to_string(), if r.caught > 0 { fill() } else { hot() }),
        ("Incidents", r.incidents.to_string(), if r.incidents > 0 { bad() } else { hot() }),
        ("Stalls", r.stalls.to_string(), if r.stalls > 0 { acc() } else { hot() }),
        ("Best combo", r.max_combo.to_string(), hot()),
        ("Agents waited", format!("{:.1} s", r.waited), hot()),
    ] {
        let s = stat(ui, c, k, v, col);
        c.entity(s).insert(Node { width: percent(33.3), flex_direction: FlexDirection::Column, row_gap: px(5), ..default() });
        c.entity(grid).add_child(s);
    }
    c.entity(body).add_child(grid);
    // the last 24 answers, as the copied result has them
    let n = r.marks.len();
    let marks = c.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(3), ..default() }).id();
    for m in &r.marks[n.saturating_sub(24)..] {
        let s = c.spawn((Node { width: px(12), height: px(12), flex_shrink: 0.0, ..default() }, BackgroundColor(mark_col(*m)))).id();
        c.entity(marks).add_child(s);
    }
    c.entity(body).add_child(marks);
    let note = if !r.lesson.is_empty() {
        format!("Lesson · {}", r.lesson)
    } else {
        let best = g.record.best_today(&g.day);
        format!("Today's best {} · streak {} day{}", engine::thousands(best), g.record.streak, if g.record.streak == 1 { "" } else { "s" })
    };
    let nt = para(c, f, &clip(&note, 150), 12.5, if r.lesson.is_empty() { line() } else { acc() }, 570.0);
    c.entity(nt).insert(Node { margin: UiRect::top(px(12)), max_width: px(570), ..default() });
    c.entity(body).add_child(nt);
    let sp = spacer(c);
    let b1 = button(c, f, "Play again", "⏎", 0, Click::Key(KeyCode::Enter));
    let b2 = button(c, f, if g.copied > 0.0 { "Copied" } else { "Copy result" }, "C", 1, Click::Key(KeyCode::KeyC));
    let b3 = button(c, f, "Back", "ESC", 2, Click::Key(KeyCode::Escape));
    let acts = actions(c, &[b1, b2, b3]);
    c.entity(body).add_children(&[sp, acts]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_atlas_packs_without_overlap() {
        for scale in [1.0f32, 1.65, 2.2, 3.0] {
            let (w, spots) = pack(scale);
            let sizes = [TOP, TODAY, ASKING, CARD];
            let rects: Vec<(u32, u32, u32, u32)> = spots.iter().zip(sizes).map(|(&(x, y), (pw, ph))| (x, y, x + (pw * scale).ceil() as u32, y + (ph * scale).ceil() as u32)).collect();
            for r in &rects {
                assert!(r.2 <= w, "{r:?} passes the width {w}");
            }
            for i in 0..rects.len() {
                for j in i + 1..rects.len() {
                    let (a, b) = (rects[i], rects[j]);
                    assert!(a.2 <= b.0 || b.2 <= a.0 || a.3 <= b.1 || b.3 <= a.1, "{a:?} overlaps {b:?}");
                }
            }
            assert!(big_width(scale) as f32 >= BIG.0 * scale);
        }
    }

    #[test]
    fn each_screen_shows_its_panels() {
        assert!(shown(Screen::Play, Kind::Card) && shown(Screen::Play, Kind::Asking) && !shown(Screen::Play, Kind::Big));
        assert!(shown(Screen::Paused, Kind::Big) && shown(Screen::Paused, Kind::Top) && !shown(Screen::Paused, Kind::Card));
        assert!(shown(Screen::Title, Kind::Big) && !shown(Screen::Title, Kind::Top));
        assert!(shown(Screen::Over, Kind::Big) && !shown(Screen::Over, Kind::Card));
        assert!(!shown(Screen::Off, Kind::Big));
    }
}
