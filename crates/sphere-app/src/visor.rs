#![allow(deprecated)] // Interaction: Bevy 0.20 deprecates it in favour of ui_widgets, which this crate does not enable yet
//! The screen-pinned visor (prototype v2): the dock's controls (View, Theme, Fit, Reset, Sound), the theme menu,
//! corner brackets, edge labels, the event ticker along the bottom, and the toast stack above the dock.
use crate::core::{CoreCtl, Mode};
use crate::fonts::UiFonts;
use crate::home::{Act, Acts};
use crate::live::Toasts;
use crate::panel::{font, with_alpha};
use crate::theme::{Theme, ThemeId, ALL};
use crate::util::*;
use bevy::prelude::*;
use bevy::text::LineHeight;


#[derive(Component)]
pub struct Dock;

/// A dock button and what it does.
#[derive(Component, Clone, Copy, PartialEq)]
pub enum DockBtn {
    Mode(Mode),
    Collapse,
    ThemeMenu,
    Theme(ThemeId),
    Fit,
    Reset,
    Sound,
    Apps,
    /// the swarm dock: orbit tilt, and every agent's step track
    Tilt(crate::swarm::Tilt),
    Tracks,
}

/// The View group (hidden while the swarm is open) and the swarm's Tilt group (shown instead).
#[derive(Component)]
pub struct ViewGroup;
#[derive(Component)]
pub struct TiltGroup;

#[derive(Component)]
pub struct BtnLabel;
#[derive(Component)]
pub struct ThemePop;
#[derive(Component)]
pub struct TickerRow;
/// The left edge text ("SPHERE · CORE ONLINE", "... AGENT SWARM ONLINE" in the swarm).
#[derive(Component)]
pub struct EdgeLeft;
#[derive(Component)]
pub struct ToastStack;

#[derive(Resource, Default)]
pub struct Visor {
    pub theme_open: bool,
    pub ticker_x: f32,
    pub ticker_w: f32,
}

fn group_btn(c: &mut Commands, f: &UiFonts, label: &str, b: DockBtn) -> Entity {
    let line = role_color(Role::Line);
    let e = c
        .spawn((
            Interaction::None,
            bevy::ui::FocusPolicy::Block,
            b,
            Node { padding: UiRect { left: px(10), right: px(10), top: px(7), bottom: px(6) }, border: UiRect::all(px(1)), column_gap: px(7), align_items: AlignItems::Center, ..default() },
            BorderColor::all(with_alpha(line, 0.13)),
            BackgroundColor(Color::NONE),
        ))
        .id();
    let t = c.spawn((BtnLabel, Text::new(label.to_uppercase()), font(&f.ui700, 10.0, 2.6), TextColor(line), LineHeight::Px(10.0))).id();
    c.entity(e).add_child(t);
    e
}

fn swatch(c: &mut Commands, t: ThemeId, size: f32) -> Entity {
    let col = match t {
        ThemeId::Balanced => hex("#cfe8f4"),
        ThemeId::Full => hex("#a48bff"),
        ThemeId::Cyan => hex("#7ce2ff"),
        ThemeId::Black => hex("#0b0c0d"),
        ThemeId::Red => hex("#ff3b30"),
    };
    let e = c
        .spawn((Node { width: px(size), height: px(size), flex_shrink: 0.0, border: UiRect::all(px(if t == ThemeId::Black { 1.0 } else { 0.0 })), ..default() }, BackgroundColor(col), BorderColor::all(hex("#d3d8db"))))
        .id();
    if t == ThemeId::Full {
        c.entity(e).insert(BackgroundGradient(vec![Gradient::Linear(LinearGradient::to_bottom_right(vec![
            ColorStop::auto(hex("#a48bff")),
            ColorStop::auto(hex("#3ad7ff")),
            ColorStop::auto(hex("#5fe3a1")),
            ColorStop::auto(hex("#ffd166")),
            ColorStop::auto(hex("#ff6fb5")),
        ]))]));
    }
    if t == ThemeId::Balanced {
        let dot = c.spawn((Node { position_type: PositionType::Absolute, right: px(-2), bottom: px(-2), width: px(size / 2.0), height: px(size / 2.0), ..default() }, BackgroundColor(hex("#ffb347")))).id();
        c.entity(e).add_child(dot);
    }
    e
}

pub fn setup(mut commands: Commands, f: Res<UiFonts>, dock: Query<Entity, With<Dock>>, root: Res<crate::hud::OverlayRoot>, theme: Res<Theme>) {
    let c = &mut commands;
    let line = role_color(Role::Line);
    let dim = role_color(Role::Dim);
    let hot = role_color(Role::Hot);
    let Ok(dock) = dock.single() else { return };
    let ctrl = c.spawn(Node { flex_direction: FlexDirection::Row, flex_wrap: FlexWrap::Wrap, align_items: AlignItems::Center, justify_content: JustifyContent::Center, column_gap: px(18), row_gap: px(8), ..default() }).id();
    // View
    let view = c.spawn((ViewGroup, Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(4), ..default() })).id();
    let vl = c.spawn((Text::new("VIEW"), font(&f.ui600, 9.5, 3.8), TextColor(dim), LineHeight::Px(9.5), Node { margin: UiRect { right: px(6), ..default() }, ..default() })).id();
    c.entity(view).add_child(vl);
    for (l, b) in [("Flat", DockBtn::Mode(Mode::Flat)), ("Depth", DockBtn::Mode(Mode::Depth)), ("Orbit", DockBtn::Mode(Mode::Orbit)), ("Collapse", DockBtn::Collapse)] {
        let e = group_btn(c, &f, l, b);
        c.entity(view).add_child(e);
    }
    // Tilt (the swarm's dock group, in place of View while the swarm is open)
    let tilt = c.spawn((TiltGroup, Node { display: Display::None, flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(4), ..default() })).id();
    let tl = c.spawn((Text::new("TILT"), font(&f.ui600, 9.5, 3.8), TextColor(dim), LineHeight::Px(9.5), Node { margin: UiRect { right: px(6), ..default() }, ..default() })).id();
    c.entity(tilt).add_child(tl);
    for (l, b) in [("Low", DockBtn::Tilt(crate::swarm::Tilt::Low)), ("Mid", DockBtn::Tilt(crate::swarm::Tilt::Mid)), ("High", DockBtn::Tilt(crate::swarm::Tilt::High)), ("Tracks", DockBtn::Tracks)] {
        let e = group_btn(c, &f, l, b);
        c.entity(tilt).add_child(e);
    }
    // Theme: swatch, name, caret; the menu pops above
    let tw = c.spawn(Node { flex_direction: FlexDirection::Row, ..default() }).id();
    let tb = group_btn(c, &f, theme.id.label(), DockBtn::ThemeMenu);
    let sw = swatch(c, theme.id, 8.0);
    c.entity(tb).insert_children(0, &[sw]);
    let pop = c
        .spawn((
            ThemePop,
            Node {
                position_type: PositionType::Absolute,
                bottom: px(38),
                left: px(-120),
                width: px(322),
                padding: UiRect::all(px(8)),
                flex_direction: FlexDirection::Column,
                row_gap: px(2),
                border: UiRect::all(px(1)),
                ..default()
            },
            BackgroundGradient(vec![Gradient::Linear(LinearGradient::to_bottom(vec![
                ColorStop::percent(Color::srgba(0.09, 0.11, 0.12, 0.88), 0.0),
                ColorStop::percent(Color::srgba(0.03, 0.04, 0.05, 0.92), 100.0),
            ]))]),
            BorderColor::all(with_alpha(line, 0.34)),
            Visibility::Hidden,
            GlobalZIndex(5),
        ))
        .id();
    let desc = |t: ThemeId| match t {
        ThemeId::Balanced => "Calm white glass, color only for status",
        ThemeId::Full => "Every layer and panel in its own hue",
        ThemeId::Cyan => "The original holographic blue",
        ThemeId::Black => "Graphite and white, nothing else",
        ThemeId::Red => "White glass with a red signal",
    };
    for t in ALL {
        let row = c
            .spawn((Interaction::None, bevy::ui::FocusPolicy::Block, DockBtn::Theme(t), Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(13), padding: UiRect { left: px(11), right: px(30), top: px(9), bottom: px(9) }, ..default() }, BackgroundColor(Color::NONE)))
            .id();
        let sw = swatch(c, t, 14.0);
        let txt = c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(2), ..default() }).id();
        let n = c.spawn((Text::new(t.label().to_uppercase()), font(&f.ui700, 10.5, 2.73), TextColor(if t == theme.id { hot } else { line }), LineHeight::Px(13.0))).id();
        let d = c.spawn((Text::new(desc(t)), font(&f.ui500, 12.5, 0.5), TextColor(with_alpha(line, 0.64)), LineHeight::Px(16.0))).id();
        c.entity(txt).add_children(&[n, d]);
        c.entity(row).add_children(&[sw, txt]);
        c.entity(pop).add_child(row);
    }
    c.entity(tw).add_children(&[tb, pop]);
    // Tools
    let tools = c.spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(6), ..default() }).id();
    for (l, b) in [("Apps", DockBtn::Apps), ("Fit", DockBtn::Fit), ("Reset", DockBtn::Reset), ("Sound", DockBtn::Sound)] {
        let e = group_btn(c, &f, l, b);
        c.entity(tools).add_child(e);
    }
    c.entity(ctrl).add_children(&[view, tilt, tw, tools]);
    c.entity(dock).add_child(ctrl);

    // corner brackets: 56 px, top 14, bottom 38 (above the ticker), sides 14
    for (top, left) in [(true, true), (true, false), (false, true), (false, false)] {
        let mut n = Node { position_type: PositionType::Absolute, width: px(56), height: px(56), ..default() };
        if top {
            n.top = px(14);
        } else {
            n.bottom = px(38);
        }
        if left {
            n.left = px(14);
        } else {
            n.right = px(14);
        }
        n.border = UiRect { left: px(if left { 1.0 } else { 0.0 }), right: px(if left { 0.0 } else { 1.0 }), top: px(if top { 1.0 } else { 0.0 }), bottom: px(if top { 0.0 } else { 1.0 }) };
        let e = c.spawn((n, BorderColor::all(with_alpha(hot, 0.7)))).id();
        c.entity(root.0).add_child(e);
    }
    // edge labels, rotated, at mid height
    for (txt, left) in [("SPHERE · CORE CONNECTING…", true), (concat!("SPHERE ", env!("CARGO_PKG_VERSION"), " · CORE RINGS AFTER A.L.I.C.E. (MIT)"), false)] {
        let mut n = Node { position_type: PositionType::Absolute, top: percent(50), ..default() };
        if left {
            n.left = px(-60);
        } else {
            n.right = px(-160);
        }
        let e = c
            .spawn((Text::new(txt), font(&f.ui600, 9.5, 4.75), TextColor(dim), LineHeight::Px(9.5), n, UiTransform { rotation: Rot2::degrees(if left { -90.0 } else { 90.0 }), ..default() }))
            .id();
        if left {
            c.entity(e).insert(EdgeLeft);
        }
        c.entity(root.0).add_child(e);
    }
    // ticker strip
    let strip = c
        .spawn((
            Node { position_type: PositionType::Absolute, left: px(0), right: px(0), bottom: px(0), height: px(24), overflow: Overflow::clip(), border: UiRect { top: px(1), ..default() }, ..default() },
            BorderColor::all(with_alpha(line, 0.13)),
        ))
        .id();
    let row = c.spawn((TickerRow, Text::default(), font(&f.mono, 10.5, 0.21), TextColor(line), LineHeight::Px(23.0), TextLayout { linebreak: LineBreak::NoWrap, ..default() }, Node { position_type: PositionType::Absolute, left: px(0), top: px(0), ..default() })).id();
    c.entity(strip).add_child(row);
    // toasts, centred above the dock
    let stack = c
        .spawn((ToastStack, Node { position_type: PositionType::Absolute, left: px(0), right: px(0), bottom: px(TOASTS_BOTTOM), flex_direction: FlexDirection::Column, align_items: AlignItems::Center, row_gap: px(6), ..default() }))
        .id();
    c.entity(root.0).add_children(&[strip, stack]);
    c.init_resource::<Visor>();
}

/// Clicks and hovers on the dock's buttons.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn buttons(
    q: Query<(&Interaction, &DockBtn), Changed<Interaction>>,
    mut acts: ResMut<Acts>,
    mut v: ResMut<Visor>,
    mut sound: ResMut<crate::sound::Sound>,
    ctl: Res<CoreCtl>,
    mut pop: Query<&mut Visibility, With<ThemePop>>,
    mut orb: ResMut<crate::orbit::Orb>,
    mut swarm: ResMut<crate::swarm::Swarm>,
    mut toasts: ResMut<Toasts>,
) {
    for (i, b) in &q {
        match i {
            Interaction::Hovered => sound.play("hover"),
            Interaction::Pressed => {
                match *b {
                    DockBtn::Mode(m) => acts.0.push(Act::Mode(m)),
                    DockBtn::Collapse => acts.0.push(Act::Collapse(Some(!ctl.collapsed))),
                    DockBtn::ThemeMenu => {
                        v.theme_open = !v.theme_open;
                        sound.play_q("toggle", 150);
                    }
                    DockBtn::Theme(t) => {
                        v.theme_open = false;
                        acts.0.push(Act::Theme(Some(t)));
                    }
                    DockBtn::Fit => acts.0.push(Act::Fit),
                    DockBtn::Reset => acts.0.push(Act::Reset),
                    DockBtn::Sound => acts.0.push(Act::Mute),
                    DockBtn::Apps => orb.toggle(&mut sound),
                    DockBtn::Tilt(t) => crate::swarm::view::set_tilt(&mut swarm, t, &mut sound, &mut toasts),
                    DockBtn::Tracks => crate::swarm::view::toggle_tracks(&mut swarm, &mut sound, &mut toasts),
                }
                if !matches!(b, DockBtn::ThemeMenu | DockBtn::Theme(_)) {
                    sound.play_q("click", 90);
                }
            }
            Interaction::None => {}
        }
    }
    for mut vis in &mut pop {
        vis.set_if_neq(if v.theme_open { Visibility::Inherited } else { Visibility::Hidden });
    }
}

/// Pressed state for View (current mode) and labels that follow state (Collapse/Expand, the theme, Sound/Muted).
#[allow(clippy::type_complexity)]
pub fn draw_buttons(
    ctl: Res<CoreCtl>,
    theme: Res<Theme>,
    sound: Res<crate::sound::Sound>,
    swarm: Res<crate::swarm::Swarm>,
    mut btns: Query<(&DockBtn, &Interaction, &mut BackgroundColor, &mut BorderColor, &Children)>,
    mut labels: Query<(&mut Text, &mut TextColor), With<BtnLabel>>,
) {
    let line = role_color(Role::Line);
    let fill = crate::theme::now(Role::Fill);
    let hot = crate::theme::now(Role::Hot);
    for (b, i, mut bg, mut bd, kids) in &mut btns {
        let pressed = matches!(b, DockBtn::Mode(m) if *m == ctl.mode) || (*b == DockBtn::Apps && ctl.orb > 0.5) || matches!(b, DockBtn::Tilt(t) if *t == swarm.tilt) || (*b == DockBtn::Tracks && swarm.tracks);
        let hover = *i == Interaction::Hovered;
        bg.set_if_neq(BackgroundColor(match b {
            DockBtn::Theme(_) if hover => with_alpha(line, 0.1),
            _ if pressed => fill,
            _ => Color::NONE,
        }));
        let border = if pressed { fill } else if hover { with_alpha(line, 0.34) } else { with_alpha(line, 0.13) };
        if !matches!(b, DockBtn::Theme(_)) {
            bd.set_if_neq(BorderColor::all(border));
        }
        for k in kids.iter() {
            if let Ok((mut t, mut c)) = labels.get_mut(k) {
                let want = match b {
                    DockBtn::Collapse => if ctl.collapsed { "EXPAND" } else { "COLLAPSE" }.to_string(),
                    DockBtn::ThemeMenu => theme.id.label().to_uppercase(),
                    DockBtn::Sound => if sound.muted { "MUTED" } else { "SOUND" }.to_string(),
                    _ => t.0.clone(),
                };
                if t.0 != want {
                    t.0 = want;
                }
                c.set_if_neq(TextColor(if pressed { Color::BLACK } else if hover { hot } else { line }));
            }
        }
    }
}

/// The ticker: what Sphere knows right now, scrolling right to left (80 s a loop in the prototype).
#[allow(clippy::too_many_arguments)]
pub fn ticker(
    sim: Res<crate::core::Sim>,
    mut v: ResMut<Visor>,
    agents: Res<crate::core::Agents>,
    live: Res<crate::live::Live>,
    stats: Res<crate::sysstats::SysStats>,
    theme: Res<Theme>,
    args: Res<crate::Args>,
    swarm: Res<crate::swarm::Swarm>,
    sticker: Res<crate::swarm::feedback::SwarmTicker>,
    mut acc: Local<f32>,
    mut was: Local<(bool, u64)>,
    mut q: Query<(&mut Text, &mut UiTransform, &ComputedNode), With<TickerRow>>,
) {
    *acc += sim.dt;
    // the swarm's items replace the home's while it is open; rebuild at once when that, or its data, changes
    let key = (swarm.open, swarm.version);
    let swapped = *was != key || (swarm.open && sticker.is_changed());
    *was = key;
    let rebuild = swapped || *acc > 3.0 || q.iter().next().is_some_and(|(t, _, _)| t.0.is_empty());
    for (mut t, mut ut, cn) in &mut q {
        if rebuild {
            *acc = 0.0;
            let mut items: Vec<String> = Vec::new();
            if swarm.open {
                items = crate::swarm::feedback::ticker_items(&sticker.0, &swarm.model);
                // the game's events only: its agents have no slots or cost
                if swarm.game {
                    items.truncate(sticker.0.len());
                }
            }
            for a in agents.list.iter().take(if swarm.open { 0 } else { 4 }) {
                items.push(format!("AGENT {} · {}", a.name.to_lowercase(), a.step.to_lowercase()));
            }
            let up = matches!(live.link, Some(crate::service::Link::Connected));
            items.push(format!("SPHERED {}", if up { "online" } else if args.offline() { "demo data" } else { "offline" }));
            if !live.requests.is_empty() {
                items.push(format!("NEEDS YOU {} waiting", live.requests.len()));
            }
            let s = stats.get();
            if s.ready {
                items.push(format!("SYSTEM cpu {:.0}% · memory {:.0}% · gpu {:.0}%", s.cpu * 100.0, s.mem * 100.0, s.gpu * 100.0));
            }
            if s.docker != crate::sysstats::Docker::Unknown {
                items.push(format!("CONTAINERS {}", s.docker.label().to_lowercase()));
            }
            items.push(format!("THEME {}", theme.id.label().to_lowercase()));
            items.push("KEYS 1 2 3 views · G go to · C theme · X collapse · F fit · R reset · M sound".into());
            let line = items.join("        ");
            let s2 = format!("{line}        {line}        ");
            if t.0 != s2 {
                t.0 = s2;
            }
        }
        // the row holds the items twice; slide one copy's width, then wrap
        v.ticker_w = (cn.size().x * cn.inverse_scale_factor()) / 2.0;
        let speed = (v.ticker_w / 80.0).max(20.0);
        v.ticker_x -= speed * sim.dt;
        if v.ticker_w > 0.0 && v.ticker_x < -v.ticker_w {
            v.ticker_x += v.ticker_w;
        }
        // moved by its transform, not `left`, so scrolling doesn't relayout the UI every frame
        ut.translation = Val2::px(v.ticker_x, 0.0);
    }
}

/// How far the toasts sit above the bottom of the window, in px (the command bar's protocol preview lifts them).
pub const TOASTS_BOTTOM: f32 = 104.0;

#[derive(Component)]
pub struct ToastBox(pub usize);

/// Toast boxes (black glass, a fill bar on the left), easing in over .35 s and out over the last .4 s.
pub fn toasts(
    mut commands: Commands,
    t: Res<Toasts>,
    f: Res<UiFonts>,
    mut seen: Local<u64>,
    stack: Query<Entity, With<ToastStack>>,
    mut boxes: Query<(&ToastBox, &mut UiTransform, &mut BackgroundColor, &Children)>,
    mut texts: Query<&mut TextColor>,
) {
    let Ok(stack) = stack.single() else { return };
    let line = role_color(Role::Line);
    if *seen != t.version {
        *seen = t.version;
        commands.entity(stack).despawn_children();
        for (i, (s, _, kind)) in t.list.iter().enumerate() {
            let b = commands
                .spawn((
                    ToastBox(i),
                    Node { padding: UiRect { left: px(12), right: px(14), top: px(9), bottom: px(8) }, border: UiRect { left: px(3), right: px(1), top: px(1), bottom: px(1) }, ..default() },
                    BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.72)),
                    BorderColor { left: crate::theme::now(match kind { crate::live::ToastKind::Plain => Role::Fill, crate::live::ToastKind::Ok => Role::Ok, crate::live::ToastKind::Bad => Role::Bad }), right: with_alpha(line, 0.13), top: with_alpha(line, 0.13), bottom: with_alpha(line, 0.13) },
                    UiTransform::default(),
                ))
                .id();
            let tx = commands.spawn((Text::new(s.to_uppercase()), font(&f.ui600, 11.0, 2.64), TextColor(role_color(Role::Hot)), LineHeight::Px(11.0))).id();
            commands.entity(b).add_child(tx);
            commands.entity(stack).add_child(b);
        }
        return;
    }
    let life = 2.4 + 0.42;
    for (tb, mut tr, mut bg, kids) in &mut boxes {
        let Some((_, left, _)) = t.list.get(tb.0) else { continue };
        let age = life - left;
        let (o, y, sc) = if age < 0.35 {
            let e = out_cubic(age / 0.35);
            (e, 8.0 * (1.0 - e), lerp(0.96, 1.0, e))
        } else if *left < 0.42 {
            let e = 1.0 - left / 0.42;
            (1.0 - e, -6.0 * e, 1.0)
        } else {
            (1.0, 0.0, 1.0)
        };
        tr.translation = Val2::px(0.0, y);
        tr.scale = Vec2::splat(sc);
        bg.0 = Color::srgba(0.0, 0.0, 0.0, 0.72 * o);
        for k in kids.iter() {
            if let Ok(mut c) = texts.get_mut(k) {
                c.0 = crate::panel::with_alpha(role_color(Role::Hot), o);
            }
        }
    }
}

/// While the swarm is open the dock shows the Tilt group in place of View.
#[allow(clippy::type_complexity)]
pub fn dock_groups(swarm: Res<crate::swarm::Swarm>, mut groups: ParamSet<(Query<&mut Node, With<ViewGroup>>, Query<&mut Node, With<TiltGroup>>)>) {
    let (v, t) = if swarm.open { (Display::None, Display::Flex) } else { (Display::Flex, Display::None) };
    for mut n in &mut groups.p0() {
        n.set_if_neq(Node { display: v, ..n.clone() });
    }
    for mut n in &mut groups.p1() {
        n.set_if_neq(Node { display: t, ..n.clone() });
    }
}

/// The left edge text reads "AGENT SWARM ONLINE" while the swarm is open.
pub fn edge_text(swarm: Res<crate::swarm::Swarm>, live: Res<crate::live::Live>, args: Res<crate::Args>, mut q: Query<&mut Text, With<EdgeLeft>>) {
    let want = edge_line(swarm.open, crate::service::edge_word(live.link.as_ref(), args.offline()));
    for mut t in &mut q {
        if t.0 != want {
            t.0 = want.clone();
        }
    }
}

/// "SPHERE · CORE ONLINE" (or CONNECTING… / OFFLINE); "SPHERE · AGENT SWARM ONLINE" while the swarm is open.
pub fn edge_line(swarm: bool, word: &str) -> String {
    format!("SPHERE · {} {word}", if swarm { "AGENT SWARM" } else { "CORE" })
}

#[cfg(test)]
mod edge_tests {
    use super::*;

    #[test]
    fn edge_text_names_the_link_and_the_view() {
        assert_eq!(edge_line(false, "ONLINE"), "SPHERE · CORE ONLINE");
        assert_eq!(edge_line(false, "CONNECTING…"), "SPHERE · CORE CONNECTING…");
        assert_eq!(edge_line(true, "OFFLINE"), "SPHERE · AGENT SWARM OFFLINE");
    }
}
