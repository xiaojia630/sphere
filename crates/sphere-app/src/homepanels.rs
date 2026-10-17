//! The rest of the home (prototype v2): the SPHERE OS wordmark, System, the Apps rolodex, Telemetry (this
//! machine's real load), Next 24h and the Say line. Positions are the prototype's wide-stage anchors.
use crate::fonts::UiFonts;
use crate::panel::*;
use crate::util::*;
use bevy::prelude::*;
use bevy::text::LineHeight;

/// The eight apps, in rolodex and orbit order. Terminal and Agents open real panels; the rest come later.
pub const APPS: [(&str, bool); 8] = [
    ("Terminal", true),
    ("Agents", true),
    ("Code", false),
    ("Git & PRs", false),
    ("Containers", false),
    ("Ports & logs", false),
    ("Files", false),
    ("Music", false),
];

/// Where panel `i` of the boot order appears (0.42 s + i · 0.09 s): brand, system, apps, telemetry, clock,
/// rail, agents, say.
pub fn show_at(i: usize) -> f32 {
    0.42 + i as f32 * 0.09
}

/// Stage anchors to the panel's centre in world units.
pub fn anchor(ax: char, x: f32, ay: char, y: f32, w: f32, h: f32) -> Vec3 {
    let cx = match ax {
        'l' => x + w / 2.0,
        'r' => x - w / 2.0,
        _ => x,
    };
    let cy = match ay {
        't' => y + h / 2.0,
        'b' => y - h / 2.0,
        _ => y,
    };
    stage_to_world(cx, cy)
}

#[derive(Component)]
pub struct SysRow(pub &'static str);
#[derive(Component)]
pub struct Meter(pub usize);
#[derive(Component)]
pub struct MeterValue(pub usize);
#[derive(Component)]
pub struct RoloRow;
#[derive(Component)]
pub struct SayText;

#[derive(Resource, Default)]
pub struct Rolodex {
    pub sel: usize,
    pub version: u64,
}

/// What the Say line shows: Sphere's last words, typed out at 30 ms a character, back to the idle hint after 9 s.
#[derive(Resource, Default)]
pub struct Say {
    pub text: String,
    pub shown: usize,
    pub age: f32,
    acc: f32,
}

impl Say {
    pub fn say(&mut self, t: impl Into<String>) {
        self.text = t.into();
        self.shown = 0;
        self.age = 0.0;
    }
}

const IDLE: &str = "/ TO TYPE · DRAG TO ROTATE · F TO FIT · R TO RESET";

fn hdr(c: &mut Commands, f: &UiFonts, title: &str, right: &str) -> Entity {
    let line = role_color(Role::Line);
    let h = c
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: px(9),
                padding: UiRect { bottom: px(8), ..default() },
                margin: UiRect { bottom: px(6), ..default() },
                border: UiRect { bottom: px(1), ..default() },
                ..default()
            },
            BorderColor::all(with_alpha(line, 0.72)),
        ))
        .id();
    let sq = c.spawn((Node { width: px(6), height: px(6), ..default() }, BackgroundColor(role_color(Role::Fill)))).id();
    let t = c.spawn((Text::new(title.to_uppercase()), font(&f.ui600, 12.0, 3.84), TextColor(role_color(Role::Hot)), LineHeight::Px(14.4))).id();
    let r = c
        .spawn((
            Text::new(right.to_uppercase()),
            font(&f.ui500, 12.0, 2.16),
            TextColor(role_color(Role::Dim)),
            LineHeight::Px(14.4),
            Node { margin: UiRect { left: auto(), ..default() }, ..default() },
        ))
        .id();
    c.entity(h).add_children(&[sq, t, r]);
    h
}

fn body(c: &mut Commands, root: Entity, framed: bool) -> Entity {
    let b = c
        .spawn(Node {
            width: percent(100),
            height: percent(100),
            flex_direction: FlexDirection::Column,
            padding: if framed { UiRect { left: px(18), right: px(18), top: px(14), bottom: px(16) } } else { UiRect::ZERO },
            ..default()
        })
        .id();
    c.entity(root).add_child(b);
    b
}

#[allow(clippy::too_many_arguments)]
pub fn setup(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut atlas: ResMut<crate::panel::PanelAtlas>, mut pmats: ResMut<Assets<PanelMaterial>>, f: Res<UiFonts>) {
    let c = &mut commands;
    let line = role_color(Role::Line);
    let hot = role_color(Role::Hot);
    let dim = role_color(Role::Dim);
    let fill = role_color(Role::Fill);

    // ---- brand: centre 800, top 20, 560 wide
    let (w, h) = (560.0, 52.0);
    let ui = spawn_ui_panel(c, &mut atlas, w, h);
    let b = c.spawn(Node { width: percent(100), height: percent(100), flex_direction: FlexDirection::Column, align_items: AlignItems::Center, row_gap: px(9), ..default() }).id();
    c.entity(ui.root).add_child(b);
    let wm = c.spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(14), padding: UiRect { left: px(14.3), ..default() }, ..default() }).id();
    let t = c.spawn((Text::new("SPHERE"), font(&f.display, 23.0, 14.26), TextColor(hot), LineHeight::Px(23.0))).id();
    let tag = c.spawn((Node { padding: UiRect { left: px(7), right: px(5), top: px(4), bottom: px(3) }, ..default() }, BackgroundColor(fill))).id();
    let tt = c.spawn((Text::new("OS"), font(&f.ui700, 10.5, 2.52), TextColor(Color::BLACK), LineHeight::Px(10.5))).id();
    c.entity(tag).add_child(tt);
    c.entity(wm).add_children(&[t, tag]);
    let sub = c.spawn((Text::new("AGENT OPERATING SYSTEM"), font(&f.ui600, 11.0, 5.72), TextColor(line), LineHeight::Px(11.0))).id();
    c.entity(b).add_children(&[wm, sub]);
    spawn_panel_quad(c, &mut meshes, &mut pmats, &ui, &PanelSpec { id: PanelId::Brand, w, h, home: anchor('c', 800.0, 't', 20.0, w, h), home_scale: 1.0, side: 0.0, phase: 0.7, show_at: show_at(0), unframed: true });

    // ---- system: left 54, top 54, framed
    let (w, h) = (316.0, 212.0);
    let ui = spawn_ui_panel(c, &mut atlas, w, h);
    let b = body(c, ui.root, true);
    let hd = hdr(c, &f, "Sphere · System", &format!("v{}", env!("CARGO_PKG_VERSION")));
    c.entity(b).add_child(hd);
    for (k, v, acc) in [("Core", "Online", false), ("Agents", "0 working", false), ("Needs you", "None", false), ("Containers", "Not set up", false), ("Voice", "Off", false), ("Response", "—", false)] {
        let row = c.spawn(Node { flex_direction: FlexDirection::Row, justify_content: JustifyContent::SpaceBetween, column_gap: px(22), ..default() }).id();
        let dt = c.spawn((Text::new(k.to_uppercase()), font(&f.ui500, 14.0, 2.24), TextColor(line), LineHeight::Px(25.2))).id();
        let dd = c.spawn((SysRow(k), Text::new(v.to_uppercase()), font(&f.ui600, 14.0, 2.24), TextColor(if acc { role_color(Role::Acc) } else { hot }), LineHeight::Px(25.2))).id();
        c.entity(row).add_children(&[dt, dd]);
        c.entity(b).add_child(row);
    }
    spawn_panel_quad(c, &mut meshes, &mut pmats, &ui, &PanelSpec { id: PanelId::System, w, h, home: anchor('l', 54.0, 't', 54.0, w, h), home_scale: 1.0, side: 1.0, phase: 2.9, show_at: show_at(1), unframed: false });

    // ---- apps rolodex: left 60, middle 488, 226 wide
    let (w, h) = (226.0, 352.0);
    let ui = spawn_ui_panel(c, &mut atlas, w, h);
    let b = c.spawn(Node { width: percent(100), height: percent(100), flex_direction: FlexDirection::Column, row_gap: px(8), ..default() }).id();
    c.entity(ui.root).add_child(b);
    let hh = c.spawn((Text::new("APPS"), font(&f.ui600, 11.0, 4.62), TextColor(dim), LineHeight::Px(13.2), Node { margin: UiRect { left: px(14), ..default() }, ..default() })).id();
    let win = c.spawn(Node { height: px(306), flex_direction: FlexDirection::Column, overflow: Overflow::clip(), ..default() }).id();
    // the selection band at y 136 (rows are 34 px; the selected row sits in it)
    let band = c
        .spawn((
            Node { position_type: PositionType::Absolute, left: px(0), right: px(0), top: px(136), height: px(34), border: UiRect::vertical(px(1)), ..default() },
            BorderColor::all(with_alpha(line, 0.13)),
        ))
        .id();
    let bl = c.spawn((Node { position_type: PositionType::Absolute, left: px(0), top: px(-1), bottom: px(-1), width: px(9), border: UiRect { left: px(1), top: px(1), bottom: px(1), ..default() }, ..default() }, BorderColor::all(hot))).id();
    let br = c.spawn((Node { position_type: PositionType::Absolute, right: px(0), top: px(-1), bottom: px(-1), width: px(9), border: UiRect { right: px(1), top: px(1), bottom: px(1), ..default() }, ..default() }, BorderColor::all(hot))).id();
    c.entity(band).add_children(&[bl, br]);
    let list = c.spawn((Node { position_type: PositionType::Absolute, left: px(0), right: px(0), top: px(136.0 - 8.0 * 34.0), flex_direction: FlexDirection::Column, ..default() }, RoloList)).id();
    // three copies, so the drum wraps: the selected row is always in the middle copy
    for k in 0..APPS.len() * 3 {
        let i = k % APPS.len();
        let (name, live) = APPS[i];
        let row = c.spawn((RoloRow, Node { height: px(34), flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(10), padding: UiRect { left: px(14), ..default() }, ..default() })).id();
        let dot = c.spawn((Node { width: px(4), height: px(4), ..default() }, BackgroundColor(with_alpha(line, 0.6)), RoloDot(k))).id();
        let t = c.spawn((Text::new(name.to_uppercase()), font(&f.ui600, 13.0, 3.38), TextColor(with_alpha(line, 0.5)), LineHeight::Px(16.0), RoloText(k))).id();
        c.entity(row).add_children(&[dot, t]);
        if !live {
            let soon = c.spawn((Text::new("SOON"), font(&f.ui600, 10.0, 1.8), TextColor(with_alpha(dim, 0.5)), LineHeight::Px(10.0), Node { margin: UiRect { left: auto(), right: px(14), ..default() }, ..default() })).id();
            c.entity(row).add_child(soon);
        }
        c.entity(list).add_child(row);
    }
    c.entity(win).add_children(&[list, band]);
    let foot = c.spawn((Text::new("Scroll to spin, click to open"), font(&f.ui500, 12.0, 1.68), TextColor(dim), LineHeight::Px(15.6), Node { margin: UiRect { left: px(14), ..default() }, ..default() })).id();
    c.entity(b).add_children(&[hh, win, foot]);
    spawn_panel_quad(c, &mut meshes, &mut pmats, &ui, &PanelSpec { id: PanelId::Apps, w, h, home: anchor('l', 60.0, 'm', 488.0, w, h), home_scale: 1.0, side: 1.0, phase: 3.7, show_at: show_at(2), unframed: true });

    // ---- telemetry: left 54, bottom 962, framed
    let (w, h) = (316.0, 206.0);
    let ui = spawn_ui_panel(c, &mut atlas, w, h);
    let b = body(c, ui.root, true);
    let hd = hdr(c, &f, "Telemetry", "This machine");
    c.entity(b).add_child(hd);
    let meters = c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(10), padding: UiRect { top: px(4), ..default() }, ..default() }).id();
    for (i, name) in ["Processor", "Memory", "GPU", "Network"].iter().enumerate() {
        let m = c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(5), ..default() }).id();
        let lab = c.spawn(Node { flex_direction: FlexDirection::Row, justify_content: JustifyContent::SpaceBetween, ..default() }).id();
        let l = c.spawn((Text::new(name.to_uppercase()), font(&f.ui500, 12.0, 1.92), TextColor(line), LineHeight::Px(14.4))).id();
        let v = c.spawn((MeterValue(i), Text::new("--"), font(&f.mono, 11.5, 0.46), TextColor(hot), LineHeight::Px(14.4))).id();
        c.entity(lab).add_children(&[l, v]);
        let track = c.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(2), height: px(8), ..default() }).id();
        for j in 0..22 {
            let seg = c.spawn((Node { flex_grow: 1.0, height: px(8), ..default() }, BackgroundColor(with_alpha(line, 0.13)), Meter(i * 100 + j))).id();
            c.entity(track).add_child(seg);
        }
        c.entity(m).add_children(&[lab, track]);
        c.entity(meters).add_child(m);
    }
    c.entity(b).add_child(meters);
    spawn_panel_quad(c, &mut meshes, &mut pmats, &ui, &PanelSpec { id: PanelId::Tele, w, h, home: anchor('l', 54.0, 'b', 962.0, w, h), home_scale: 1.0, side: 1.0, phase: 5.3, show_at: show_at(3), unframed: false });

    // ---- rail: right 1546, top 236, "Next 24h" (no calendar source yet: an empty state)
    let (w, h) = (336.0, 120.0);
    let ui = spawn_ui_panel(c, &mut atlas, w, h);
    let b = c.spawn(Node { width: percent(100), height: percent(100), flex_direction: FlexDirection::Column, row_gap: px(8), align_items: AlignItems::FlexEnd, ..default() }).id();
    c.entity(ui.root).add_child(b);
    let rh = c.spawn((Text::new("NEXT 24H"), font(&f.ui600, 11.5, 4.6), TextColor(dim), LineHeight::Px(13.8), Node { margin: UiRect { right: px(52), ..default() }, ..default() })).id();
    let empty = c.spawn((Text::new("NOTHING SCHEDULED"), font(&f.ui500, 12.5, 2.5), TextColor(with_alpha(line, 0.55)), LineHeight::Px(16.0), Node { margin: UiRect { right: px(52), top: px(6), ..default() }, ..default() })).id();
    let hint = c.spawn((Text::new("Calendar arrives with a later app"), font(&f.ui500, 12.0, 1.0), TextColor(with_alpha(dim, 0.8)), LineHeight::Px(15.0), Node { margin: UiRect { right: px(52), ..default() }, ..default() })).id();
    c.entity(b).add_children(&[rh, empty, hint]);
    spawn_panel_quad(c, &mut meshes, &mut pmats, &ui, &PanelSpec { id: PanelId::Rail, w, h, home: anchor('r', 1546.0, 't', 236.0, w, h), home_scale: 1.0, side: -1.0, phase: 0.2, show_at: show_at(5), unframed: true });

    // ---- say: centre 800, top 886, 780 wide
    let (w, h) = (780.0, 96.0);
    let ui = spawn_ui_panel(c, &mut atlas, w, h);
    let b = c.spawn(Node { width: percent(100), height: percent(100), flex_direction: FlexDirection::Column, align_items: AlignItems::Center, justify_content: JustifyContent::Center, ..default() }).id();
    c.entity(ui.root).add_child(b);
    let t = c.spawn((SayText, Text::new(IDLE), font(&f.ui600, 12.0, 3.36), TextColor(dim), LineHeight::Px(16.0), TextLayout { justify: Justify::Center, ..default() })).id();
    c.entity(b).add_child(t);
    spawn_panel_quad(c, &mut meshes, &mut pmats, &ui, &PanelSpec { id: PanelId::Say, w, h, home: anchor('c', 800.0, 't', 858.0, w, h), home_scale: 1.0, side: 0.0, phase: 6.1, show_at: show_at(7), unframed: true });

    c.init_resource::<Rolodex>();
    c.init_resource::<Say>();
}

#[derive(Component)]
pub struct RoloList;
#[derive(Component)]
pub struct RoloDot(pub usize);
#[derive(Component)]
pub struct RoloText(pub usize);

/// System rows from the service; Telemetry from this machine (every 1.2 s, like the prototype).
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn update(
    time: Res<Time<Real>>,
    stats: Res<crate::sysstats::SysStats>,
    agents: Res<crate::core::Agents>,
    live: Res<crate::live::Live>,
    svc: Res<crate::service::Service>,
    args: Res<crate::Args>,
    mut acc: Local<f32>,
    mut texts: ParamSet<(Query<(&SysRow, &mut Text, &mut TextColor)>, Query<(&MeterValue, &mut Text)>)>,
    mut segs: Query<(&Meter, &mut BackgroundColor)>,
) {
    *acc += time.delta_secs();
    if *acc < 1.2 {
        return;
    }
    *acc = 0.0;
    let line = role_color(Role::Line);
    let rtt = svc.shared.lock().unwrap().rtt_ms;
    let up = matches!(live.link, Some(crate::service::Link::Connected)) || args.offline();
    let needs = live.requests.len();
    for (r, mut t, mut c) in &mut texts.p0() {
        let (v, col) = match r.0 {
            "Core" => (if up { "ONLINE".to_string() } else { "OFFLINE".to_string() }, if up { role_color(Role::Ok) } else { role_color(Role::Bad) }),
            "Agents" => (format!("{} WORKING", agents.working()), role_color(Role::Hot)),
            "Needs you" => (if needs == 0 { "NONE".to_string() } else { format!("{needs} WAITING") }, if needs == 0 { role_color(Role::Hot) } else { role_color(Role::Acc) }),
            "Response" => (rtt.map(|ms| format!("{:.2} S", ms / 1000.0)).unwrap_or_else(|| "—".into()), role_color(Role::Hot)),
            _ => continue,
        };
        if t.0 != v {
            t.0 = v;
        }
        c.set_if_neq(TextColor(col));
    }
    let s = stats.get();
    if !s.ready {
        return;
    }
    let vals = [s.cpu, s.mem, s.gpu, (s.net / 150.0e6).min(1.0)];
    for (m, mut t) in &mut texts.p1() {
        let v = if m.0 == 3 { crate::sysstats::rate(s.net) } else { format!("{:.0}%", vals[m.0] * 100.0) };
        if t.0 != v {
            t.0 = v;
        }
    }
    let fill = role_color(Role::Fill);
    for (m, mut bg) in &mut segs {
        let (i, j) = (m.0 / 100, m.0 % 100);
        let on = (j as f32) < (vals[i] * 22.0).round();
        bg.set_if_neq(BackgroundColor(if on { fill } else { with_alpha(line, 0.13) }));
    }
}

/// The rolodex: the selected row bright with a lit dot, the list slid so it sits in the band.
#[allow(clippy::type_complexity)]
pub fn draw_rolodex(
    r: Res<Rolodex>,
    mut seen: Local<u64>,
    mut list: Query<&mut Node, With<RoloList>>,
    mut texts: Query<(&RoloText, &mut TextColor)>,
    mut dots: Query<(&RoloDot, &mut BackgroundColor)>,
) {
    if *seen == r.version + 1 {
        return;
    }
    *seen = r.version + 1;
    let n = APPS.len();
    let mid = n + r.sel;
    for mut node in &mut list {
        node.top = px(136.0 - mid as f32 * 34.0);
    }
    let line = role_color(Role::Line);
    // the window fades its top and bottom quarter
    let fade = |k: usize| -> f32 {
        let d = (k as f32 - mid as f32).abs();
        if d <= 2.0 { 1.0 } else { (1.0 - (d - 2.0) / 2.5).max(0.0) }
    };
    for (t, mut c) in &mut texts {
        c.set_if_neq(TextColor(if t.0 == mid { role_color(Role::Hot) } else { with_alpha(line, 0.5 * fade(t.0)) }));
    }
    for (d, mut c) in &mut dots {
        c.set_if_neq(BackgroundColor(if d.0 == mid { role_color(Role::Fill) } else { with_alpha(line, 0.6 * fade(d.0)) }));
    }
}

/// Types Sphere's words into the Say line at 30 ms a character; back to the idle hint after 9 s.
pub fn draw_say(time: Res<Time<Real>>, mut say: ResMut<Say>, mut q: Query<(&mut Text, &mut TextColor, &mut TextFont, &mut LineHeight, &mut bevy::text::LetterSpacing), With<SayText>>) {
    let dt = time.delta_secs();
    let mut changed = false;
    if !say.text.is_empty() {
        say.age += dt;
        say.acc += dt;
        let n = say.text.chars().count();
        while say.acc >= 0.03 && say.shown < n {
            say.acc -= 0.03;
            say.shown += 1;
            changed = true;
        }
        if say.age > 9.0 + n as f32 * 0.03 {
            say.text.clear();
            changed = true;
        }
    }
    if !changed && !say.is_changed() {
        return;
    }
    for (mut t, mut c, mut f, mut lh, mut ls) in &mut q {
        if say.text.is_empty() {
            t.0 = IDLE.into();
            c.0 = role_color(Role::Dim);
            f.font_size = FontSize::Px(12.0);
            *lh = LineHeight::Px(16.0);
            *ls = bevy::text::LetterSpacing::Px(3.36);
        } else {
            t.0 = say.text.chars().take(say.shown).collect();
            c.0 = role_color(Role::Hot);
            // long lines read smaller
            let size = if say.text.chars().count() > 70 { 19.0 } else { 23.0 };
            f.font_size = FontSize::Px(size);
            *lh = LineHeight::Px(size * 1.35);
            *ls = bevy::text::LetterSpacing::Px(size * 0.02);
        }
    }
}
