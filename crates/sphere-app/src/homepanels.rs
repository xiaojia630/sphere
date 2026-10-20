//! The rest of the home (prototype v2): the SPHERE OS wordmark, System, the Apps rolodex, Telemetry (this
//! machine's real load), Next 24h and the Say line. Positions are the prototype's wide-stage anchors.
use crate::fonts::UiFonts;
use crate::panel::*;
use crate::util::*;
use bevy::prelude::*;
use bevy::text::LineHeight;

/// Music isn't built yet, so it is hidden from the rolodex, the orbit and the bar; true brings its "soon" tile back.
pub const SHOW_MUSIC: bool = false;

/// Every app, in rolodex and orbit order (`false`: not built yet, it says it comes later). Music is last, so hiding
/// it leaves the others' indices as they are.
const ALL_APPS: [(&str, bool); 8] = [
    ("Terminal", true),
    ("Agents", true),
    ("Code", true),
    ("Git & PRs", true),
    ("Containers", true),
    ("Ports & logs", true),
    ("Files", true),
    ("Music", false),
];

/// The apps that show, in rolodex and orbit order.
pub const APPS: &[(&str, bool)] = if SHOW_MUSIC { &ALL_APPS } else { ALL_APPS.split_at(7).0 };

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
/// The line under System's Needs you row: how long agents have waited on you today (hidden until it is a minute).
#[derive(Component)]
pub struct WaitedLine;
/// The brand panel's sub line (it reads "Agent swarm" while the swarm is open).
#[derive(Component)]
pub struct BrandSub;
#[derive(Component)]
pub struct Meter(pub usize);
#[derive(Component)]
pub struct MeterValue(pub usize);
#[derive(Component)]
pub struct RoloRow;
#[derive(Component)]
pub struct SayText;
#[derive(Component)]
pub struct RailEmpty;
#[derive(Component)]
pub struct RailHint;
#[derive(Component)]
pub struct RailRow(pub usize);
#[derive(Component)]
pub struct RailTime(pub usize);
#[derive(Component)]
pub struct RailTitle(pub usize);
#[derive(Component)]
pub struct RailDot(pub usize);
/// The PROTOCOL tag of a rail row that is a protocol's run: it follows the time, and the colour square goes.
#[derive(Component)]
pub struct RailTag(pub usize);

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

const IDLE: &str = "/ TO TYPE · G TO GO TO · DRAG TO ROTATE · F TO FIT · R TO RESET";

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
    let sub = c.spawn((BrandSub, Text::new("AGENT OPERATING SYSTEM"), font(&f.ui600, 11.0, 5.72), TextColor(line), LineHeight::Px(11.0))).id();
    c.entity(b).add_children(&[wm, sub]);
    spawn_panel_quad(c, &mut meshes, &mut pmats, &ui, &PanelSpec { id: PanelId::Brand, w, h, home: anchor('c', 800.0, 't', 20.0, w, h), home_scale: 1.0, side: 0.0, phase: 0.7, show_at: show_at(0), unframed: true });

    // ---- system: left 54, top 54, framed (a little taller than its six rows, for the waited line under Needs you)
    let (w, h) = (316.0, 220.0);
    let ui = spawn_ui_panel(c, &mut atlas, w, h);
    let b = body(c, ui.root, true);
    let hd = hdr(c, &f, "Sphere · System", &format!("v{}", env!("CARGO_PKG_VERSION")));
    c.entity(b).add_child(hd);
    for (k, v, acc) in [("Core", "Connecting", false), ("Agents", "—", false), ("Needs you", "—", false), ("Containers", "Checking", false), ("Voice", "—", false), ("Response", "—", false)] {
        let row = c.spawn(Node { flex_direction: FlexDirection::Row, justify_content: JustifyContent::SpaceBetween, column_gap: px(22), ..default() }).id();
        let dt = c.spawn((Text::new(k.to_uppercase()), font(&f.ui500, 14.0, 2.24), TextColor(line), LineHeight::Px(25.2))).id();
        let dd = c.spawn((SysRow(k), Text::new(v.to_uppercase()), font(&f.ui600, 14.0, 2.24), TextColor(if acc { role_color(Role::Acc) } else { hot }), LineHeight::Px(25.2))).id();
        c.entity(row).add_children(&[dt, dd]);
        c.entity(b).add_child(row);
        if k == "Needs you" {
            let wl = c
                .spawn((WaitedLine, Text::new(""), font(&f.ui500, 11.0, 1.76), TextColor(dim), LineHeight::Px(13.0), Node { display: Display::None, align_self: AlignSelf::FlexEnd, margin: UiRect { top: px(-6), ..default() }, ..default() }))
                .id();
            c.entity(b).add_child(wl);
        }
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
    let list = c.spawn((Node { position_type: PositionType::Absolute, left: px(0), right: px(0), top: px(136.0 - APPS.len() as f32 * 34.0), flex_direction: FlexDirection::Column, ..default() }, RoloList)).id();
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

    // ---- rail: right 1546, top 236, "Next 24h": Calendar events (calendar.rs), else an empty state
    let (w, h) = (336.0, 120.0);
    let ui = spawn_ui_panel(c, &mut atlas, w, h);
    let b = c.spawn(Node { width: percent(100), height: percent(100), flex_direction: FlexDirection::Column, row_gap: px(8), align_items: AlignItems::FlexEnd, ..default() }).id();
    c.entity(ui.root).add_child(b);
    let rh = c.spawn((Text::new("NEXT 24H"), font(&f.ui600, 11.5, 4.6), TextColor(dim), LineHeight::Px(13.8), Node { margin: UiRect { right: px(52), ..default() }, ..default() })).id();
    let empty = c.spawn((RailEmpty, Text::new("NOTHING SCHEDULED"), font(&f.ui500, 12.5, 2.5), TextColor(with_alpha(line, 0.55)), LineHeight::Px(16.0), Node { margin: UiRect { right: px(52), top: px(6), ..default() }, ..default() })).id();
    let hint = c.spawn((RailHint, Text::new("No calendar connected"), font(&f.ui500, 12.0, 1.0), TextColor(with_alpha(dim, 0.8)), LineHeight::Px(15.0), Node { margin: UiRect { right: px(52), ..default() }, ..default() })).id();
    c.entity(b).add_children(&[rh, empty, hint]);
    // event rows, hidden until there are events: the time, the title, the calendar's colour as a square
    for i in 0..crate::calendar::SLOTS {
        // a fixed row and time slot (wide enough for "TMRW 09:00"), so times and titles line up in columns
        let row = c.spawn((RailRow(i), Node { display: Display::None, flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(9), width: px(RAIL_ROW_W), margin: UiRect { right: px(52), ..default() }, ..default() })).id();
        let tm = c.spawn((RailTime(i), Text::new(""), font(&f.mono, 11.5, 0.46), TextColor(dim), LineHeight::Px(16.0), TextLayout { justify: Justify::Right, linebreak: LineBreak::NoWrap }, Node { width: px(RAIL_TIME_W), flex_shrink: 0.0, ..default() })).id();
        let tt = c.spawn((RailTitle(i), Text::new(""), font(&f.ui500, 12.5, 0.6), TextColor(with_alpha(line, 0.85)), LineHeight::Px(16.0), TextLayout { linebreak: LineBreak::NoWrap, ..default() }, Node { flex_grow: 1.0, flex_shrink: 1.0, overflow: Overflow::clip(), ..default() })).id();
        let sq = c.spawn((RailDot(i), Node { width: px(6), height: px(6), ..default() }, BackgroundColor(fill))).id();
        // a protocol's run has this tag after its time, and no colour square
        let tag = c.spawn((RailTag(i), Node { display: Display::None, flex_shrink: 0.0, ..default() })).id();
        let tag_text = c.spawn((Text::new("PROTOCOL"), font(&f.ui700, 9.0, 0.8), TextColor(with_alpha(line, 0.7)), LineHeight::Px(16.0))).id();
        c.entity(tag).add_child(tag_text);
        c.entity(row).add_children(&[tm, tag, tt, sq]);
        c.entity(b).add_child(row);
    }
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
    sound: Res<crate::sound::Sound>,
    mut acc: Local<f32>,
    mut last_voice: Local<Option<crate::sound::VoiceState>>,
    mut texts: ParamSet<(Query<(&SysRow, &mut Text, &mut TextColor)>, Query<(&MeterValue, &mut Text)>, Query<(&mut Text, &mut Node), With<WaitedLine>>)>,
    mut segs: Query<(&Meter, &mut BackgroundColor)>,
) {
    *acc += time.delta_secs();
    // the Voice row changes the moment the voice does, the rest every 1.2 s
    let vs = sound.voice_state();
    let voice_changed = *last_voice != Some(vs);
    *last_voice = Some(vs);
    if *acc < 1.2 && !voice_changed {
        return;
    }
    *acc = 0.0;
    let line = role_color(Role::Line);
    let rtt = svc.shared.lock().unwrap().rtt_ms;
    let up = matches!(live.link, Some(crate::service::Link::Connected)) || args.offline();
    let needs = live.requests.len();
    let s = stats.get();
    for (r, mut t, mut c) in &mut texts.p0() {
        let (v, col) = match r.0 {
            "Core" => (if up { "ONLINE".to_string() } else { "OFFLINE".to_string() }, if up { role_color(Role::Ok) } else { role_color(Role::Bad) }),
            "Agents" => (format!("{} WORKING", agents.working()), role_color(Role::Hot)),
            "Needs you" => (if needs == 0 { "NONE".to_string() } else { format!("{needs} WAITING") }, if needs == 0 { role_color(Role::Hot) } else { role_color(Role::Acc) }),
            "Containers" => (s.docker.label(), if matches!(s.docker, crate::sysstats::Docker::Off) { role_color(Role::Dim) } else { role_color(Role::Hot) }),
            "Voice" => {
                (vs.row().to_string(), if vs == crate::sound::VoiceState::Muted { role_color(Role::Dim) } else { role_color(Role::Hot) })
            }
            "Response" => (rtt.map(|ms| crate::service::fmt_rtt(ms).to_uppercase()).unwrap_or_else(|| "—".into()), role_color(Role::Hot)),
            _ => continue,
        };
        if t.0 != v {
            t.0 = v;
        }
        c.set_if_neq(TextColor(col));
    }
    let waited = waited_line(live.waited_today_secs(crate::core::unix_now()));
    for (mut t, mut n) in &mut texts.p2() {
        let shown = if waited.is_some() { Display::Flex } else { Display::None };
        if n.display != shown {
            n.display = shown;
        }
        if let Some(w) = &waited {
            if &t.0 != w {
                t.0 = w.clone();
            }
        }
    }
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

/// The Next 24h rail's row width and time slot: "TMRW 09:00" in the 11.5px mono face, with a little room.
const RAIL_ROW_W: f32 = 270.0;
const RAIL_TIME_W: f32 = 82.0;

/// The Next 24h rail: Calendar events and the protocols that run in the next day, when either changes and as the minute
/// turns (times read NOW, then pass), else "Nothing scheduled" and why.
#[allow(clippy::type_complexity)]
pub fn draw_rail(
    cal: Res<crate::calendar::Calendar>,
    svc: Res<crate::service::Service>,
    mut seen: Local<(u64, u64, i64)>,
    mut nodes: ParamSet<(Query<(&RailRow, &mut Node)>, Query<&mut Node, With<RailEmpty>>, Query<&mut Node, With<RailHint>>, Query<(&RailTag, &mut Node)>, Query<(&RailDot, &mut Node)>)>,
    mut texts: ParamSet<(Query<(&RailTime, &mut Text)>, Query<(&RailTitle, &mut Text)>, Query<&mut Text, With<RailEmpty>>, Query<&mut Text, With<RailHint>>)>,
    mut dots: Query<(&RailDot, &mut BackgroundColor)>,
) {
    let now = crate::core::unix_now();
    let (ver, feed) = cal.get();
    let (protocols_rev, runs) = {
        let s = svc.shared.lock().unwrap();
        // the protocols are read only when they or the minute changed
        if *seen == (ver + 1, s.protocols_rev + 1, now / 60) {
            return;
        }
        (s.protocols_rev, crate::calendar::runs(&s.protocols, now))
    };
    *seen = (ver + 1, protocols_rev + 1, now / 60);
    let r = crate::calendar::rail(&feed, &runs, now, crate::core::local_offset, crate::calendar::SLOTS);
    let show = |on: bool| if on { Display::Flex } else { Display::None };
    for (i, mut n) in &mut nodes.p0() {
        let d = show(i.0 < r.rows.len());
        if n.display != d {
            n.display = d;
        }
    }
    for (i, mut t) in &mut texts.p0() {
        let v = r.rows.get(i.0).map(|x| x.time.clone()).unwrap_or_default();
        if t.0 != v {
            t.0 = v;
        }
    }
    for (i, mut t) in &mut texts.p1() {
        let v = r.rows.get(i.0).map(|x| x.title.clone()).unwrap_or_default();
        if t.0 != v {
            t.0 = v;
        }
    }
    for mut t in &mut texts.p2() {
        let v = r.empty.unwrap_or("");
        if t.0 != v {
            t.0 = v.into();
        }
    }
    for mut t in &mut texts.p3() {
        let v = r.hint.unwrap_or("");
        if t.0 != v {
            t.0 = v.into();
        }
    }
    for (i, mut n) in &mut nodes.p3() {
        let d = show(r.rows.get(i.0).is_some_and(|x| x.protocol));
        if n.display != d {
            n.display = d;
        }
    }
    let fill = role_color(Role::Fill);
    for (i, mut bg) in &mut dots {
        // the calendar's own colour; the theme's fill when it has none; no square on the "+N more" row, nor on a
        // protocol's (it has its tag, and the title takes the room)
        let c = match r.rows.get(i.0) {
            Some(x) if x.time.is_empty() || x.protocol => Color::NONE,
            Some(x) => x.color.map(|[r, g, b]| Color::srgb(r, g, b)).unwrap_or(fill),
            None => Color::NONE,
        };
        bg.set_if_neq(BackgroundColor(c));
    }
    for (i, mut n) in &mut nodes.p4() {
        let d = show(!r.rows.get(i.0).is_some_and(|x| x.protocol));
        if n.display != d {
            n.display = d;
        }
    }
    for mut n in &mut nodes.p1() {
        let d = show(r.empty.is_some());
        if n.display != d {
            n.display = d;
        }
    }
    for mut n in &mut nodes.p2() {
        let d = show(r.hint.is_some());
        if n.display != d {
            n.display = d;
        }
    }
}

/// The line under Needs you once agents have waited a minute or more on you today: "3 MIN WAITED TODAY".
fn waited_line(secs: u64) -> Option<String> {
    (secs >= 60).then(|| format!("{} waited today", crate::live::fmt_waited(secs)).to_uppercase())
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
    let fade = |k: usize| rolo_fade(k, mid, n);
    for (t, mut c) in &mut texts {
        c.set_if_neq(TextColor(if t.0 == mid { role_color(Role::Hot) } else { with_alpha(line, 0.5 * fade(t.0)) }));
    }
    for (d, mut c) in &mut dots {
        c.set_if_neq(BackgroundColor(if d.0 == mid { role_color(Role::Fill) } else { with_alpha(line, 0.6 * fade(d.0)) }));
    }
}

/// Where the drum's window starts in the Apps panel (below the "APPS" heading and its gap), its height, where the
/// selection band sits in it and how tall a row is, in panel px.
const ROLO_WIN_TOP: f32 = 13.2 + 8.0;
const ROLO_WIN_H: f32 = 306.0;
const ROLO_BAND: f32 = 136.0;
const ROLO_ROW: f32 = 34.0;

/// The app whose row is at `y` (panel px, from the panel's top) with `sel` selected: the band's row is `sel`,
/// the rows above and below are its neighbours round the drum. None outside the window or on a hidden row.
pub fn rolo_row_at(y: f32, sel: usize) -> Option<usize> {
    let n = APPS.len();
    let wy = y - ROLO_WIN_TOP;
    if !(0.0..ROLO_WIN_H).contains(&wy) {
        return None;
    }
    let d = ((wy - ROLO_BAND) / ROLO_ROW).floor() as i32;
    let mid = n + sel;
    let k = mid as i32 + d;
    if k < 0 || rolo_fade(k as usize, mid, n) <= 0.0 {
        return None;
    }
    Some((sel as i32 + d).rem_euclid(n as i32) as usize)
}

/// How bright row `k` of the drum is with row `mid` selected: the window fades its top and bottom quarter, and
/// the bottom row as far from the selection as half the drum is hidden (it is the same app as the top row,
/// which would otherwise show twice); with an odd number of apps the top row past half the drum is hidden too.
fn rolo_fade(k: usize, mid: usize, n: usize) -> f32 {
    let d = k.abs_diff(mid);
    if (k > mid && d * 2 >= n) || (k < mid && d * 2 > n) {
        0.0
    } else if d <= 2 {
        1.0
    } else {
        (1.0 - (d as f32 - 2.0) / 2.5).max(0.0)
    }
}

/// The idle hint's letter spacing in px at 12 px type: the spec's .28em (3.36) for a short hint, tighter for a long one
/// so it stays on one line in the 780 px say panel (an average advance of about .55em plus the spacing).
pub fn idle_spacing(chars: usize) -> f32 {
    (760.0 / chars.max(1) as f32 - 12.0 * 0.55).clamp(0.6, 3.36)
}

/// Types Sphere's words into the Say line at 30 ms a character; back to the idle hint after 9 s.
pub fn draw_say(time: Res<Time<Real>>, mut say: ResMut<Say>, swarm: Res<crate::swarm::Swarm>, mut idle_seen: Local<&'static str>, mut q: Query<(&mut Text, &mut TextColor, &mut TextFont, &mut LineHeight, &mut bevy::text::LetterSpacing), With<SayText>>) {
    let dt = time.delta_secs();
    let mut changed = false;
    if !say.text.is_empty() {
        say.age += dt;
        say.acc += dt;
        let n = say.text.chars().count();
        let per = crate::motion::delay(crate::motion::Pace::Big, 0.03);
        while say.acc >= per && say.shown < n {
            say.acc -= per;
            say.shown += 1;
            changed = true;
        }
        if say.age > 9.0 + n as f32 * 0.03 {
            say.text.clear();
            changed = true;
        }
    }
    // the idle hint: the home's, the game's keys (none while its title, pause or results panel lists them), or the
    // swarm's for its mode
    let idle = if swarm.open && swarm.game { if swarm.game_panel { "" } else { crate::game::HINT } } else if swarm.open { crate::swarm::feedback::idle_hint(&swarm.mode) } else { IDLE };
    if *idle_seen != idle {
        *idle_seen = idle;
        changed = true;
    }
    if !changed && !say.is_changed() {
        return;
    }
    for (mut t, mut c, mut f, mut lh, mut ls) in &mut q {
        if say.text.is_empty() {
            t.0 = idle.into();
            c.0 = role_color(Role::Dim);
            f.font_size = FontSize::Px(12.0);
            *lh = LineHeight::Px(16.0);
            *ls = bevy::text::LetterSpacing::Px(idle_spacing(idle.chars().count()));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_click_picks_the_row_under_it() {
        let band = ROLO_WIN_TOP + ROLO_BAND + ROLO_ROW / 2.0;
        // the band is the selection; a row above is the one before it round the drum, a row below the next
        for sel in 0..APPS.len() {
            assert_eq!(rolo_row_at(band, sel), Some(sel));
            assert_eq!(rolo_row_at(band - ROLO_ROW, sel), Some((sel + APPS.len() - 1) % APPS.len()));
            assert_eq!(rolo_row_at(band + 2.0 * ROLO_ROW, sel), Some((sel + 2) % APPS.len()));
        }
        // with Terminal selected (as the home starts), Containers is the top row shown, Git & PRs the bottom one
        let up = (APPS.len() / 2) as f32;
        assert_eq!(APPS[rolo_row_at(band - up * ROLO_ROW, 0).unwrap()].0, "Containers");
        assert_eq!(APPS[rolo_row_at(band + 3.0 * ROLO_ROW, 0).unwrap()].0, "Git & PRs");
        // the heading, the hint under the window, and the hidden rows past half the drum aren't rows
        assert_eq!(rolo_row_at(4.0, 0), None);
        assert_eq!(rolo_row_at(ROLO_WIN_TOP + ROLO_WIN_H + 4.0, 0), None);
        assert_eq!(rolo_row_at(band + 4.0 * ROLO_ROW, 0), None);
        if APPS.len() % 2 == 1 {
            assert_eq!(rolo_row_at(band - (up + 1.0) * ROLO_ROW, 0), None);
        }
    }

    #[test]
    fn the_waited_line_shows_from_a_minute() {
        assert_eq!(waited_line(0), None);
        assert_eq!(waited_line(59), None);
        assert_eq!(waited_line(60).as_deref(), Some("1 MIN WAITED TODAY"));
        assert_eq!(waited_line(12 * 60 + 40).as_deref(), Some("12 MIN WAITED TODAY"));
        assert_eq!(waited_line(65 * 60).as_deref(), Some("1 H 5 MIN WAITED TODAY"));
    }

    #[test]
    fn the_rolodex_shows_each_app_once() {
        let n = APPS.len();
        for (sel, app) in APPS.iter().enumerate() {
            let mid = n + sel;
            // the window is 9 rows: 4 above the band and 4 below
            let shown: Vec<usize> = (mid - 4..=mid + 4).filter(|&k| rolo_fade(k, mid, n) > 0.0).map(|k| k % n).collect();
            let mut uniq = shown.clone();
            uniq.sort();
            uniq.dedup();
            assert_eq!(uniq.len(), shown.len(), "an app shows twice with {} selected: {shown:?}", app.0);
            assert_eq!(shown.len(), n, "every app shows with {} selected: {shown:?}", app.0);
            assert_eq!(rolo_fade(mid, mid, n), 1.0);
        }
    }

    #[test]
    fn music_hides_behind_its_switch() {
        assert_eq!(APPS.iter().any(|a| a.0 == "Music"), SHOW_MUSIC);
        // the others keep their places: the app indices (Code 2, Files 6, ...) don't move
        assert_eq!(APPS[2].0, "Code");
        assert_eq!(APPS[3].0, "Git & PRs");
        assert_eq!(APPS[6].0, "Files");
        assert_eq!(&APPS[..7], &ALL_APPS[..7]);
    }
}
