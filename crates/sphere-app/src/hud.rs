//! Scene glue: the panels, the screen backdrop, the frame-time overlay, `--bench` and `--shot`.
use crate::core::*;
use crate::fonts::{self, UiFonts};
use crate::glow::*;
use crate::panel::*;
use crate::term::{self, Terminal, Terms, TERM_H, TERM_W};
use crate::util::*;
use crate::{camera_output, target_for, Args, CamCore, Offscreen, WindowCam};
use bevy::text::LineHeight;
use bevy::{
    camera::visibility::RenderLayers,
    mesh::PrimitiveTopology,
    prelude::*,
    asset::RenderAssetUsages,
    render::{
        gpu_readback::{ReadbackComplete, ReadbackOnce},
        render_resource::{Extent3d, TextureDimension, TextureFormat},
    },
};

pub const SCALE_MIN: f32 = 1.0;

#[allow(clippy::too_many_arguments)]
pub fn setup_panels(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut atlas: ResMut<crate::panel::PanelAtlas>,
    mut fonts_a: ResMut<Assets<Font>>,
    mut pmats: ResMut<Assets<PanelMaterial>>,
    agents: Res<Agents>,
    outside: Res<crate::takeover::Outside>,
    decks: Res<Decks>,
    args: Res<Args>,
) {
    let f = fonts::load_ui_fonts(&mut fonts_a);
    commands.insert_resource(f.clone());
    commands.insert_resource(Focus { terminal: args.focus_term, cmd: false });

    // ---- Agents panel: right column, bottom aligned like the prototype (x 1546, y 962)
    let (aw, ah) = (356.0, 351.0);
    let ui = spawn_ui_panel(&mut commands, &mut atlas, aw, ah);
    build_agents_ui(&mut commands, ui.root, &f, &agents, &outside, 0.0);
    let home = stage_to_world(1546.0 - aw / 2.0, 962.0 - ah / 2.0);
    spawn_panel_quad(
        &mut commands,
        &mut meshes,
        &mut pmats,
        &ui,
        &PanelSpec { id: PanelId::Agents, w: aw, h: ah, home, home_scale: 1.0, side: -1.0, phase: 1.3, show_at: crate::homepanels::show_at(6), unframed: false },
    );

    // ---- Terminal panel: left column
    let ui = spawn_ui_panel(&mut commands, &mut atlas, TERM_W, TERM_H);
    let tui = term::build_term_ui(&mut commands, ui.root, &f);
    commands.insert_resource(tui);
    // The look test's first tab (bash, the Sphere prompt, a git log) for screenshots and benches; your own
    // shell otherwise.
    let demo = args.offline();
    let default_cmd = "git log --graph --color --oneline -20";
    let first = if demo { Some(args.cmd.clone().unwrap_or_else(|| default_cmd.to_string())) } else { args.cmd.clone() };
    let mut terms = Terms { demo, ..default() };
    if !demo {
        terms.connect(first);
    } else if args.fixit.is_some() {
        // `--fixit`: a tab that has just printed a failed cargo test, read by the same scanner as any other
        terms.tabs.push(crate::fixit::demo_tab());
    } else {
        match Terminal::spawn(first.as_deref(), demo) {
            Ok(t) => terms.tabs.push(t),
            Err(e) => eprintln!("could not start a terminal: {e}"),
        }
    }
    commands.insert_resource(terms);
    let home = stage_to_world(54.0 + TERM_W / 2.0, 70.0 + TERM_H / 2.0);
    spawn_panel_quad(
        &mut commands,
        &mut meshes,
        &mut pmats,
        &ui,
        &PanelSpec { id: PanelId::Terminal, w: TERM_W, h: TERM_H, home, home_scale: 1.0, side: 1.0, phase: 4.1, show_at: 1.1, unframed: false },
    );

    // ---- Clock: right column, top, right aligned (prototype x 1546, y 46), unframed
    let (cw, ch) = (356.0, 92.0);
    let ui = spawn_ui_panel(&mut commands, &mut atlas, cw, ch);
    build_clock_ui(&mut commands, ui.root, &f);
    let home = stage_to_world(1546.0 - cw / 2.0, 46.0 + ch / 2.0);
    spawn_panel_quad(
        &mut commands,
        &mut meshes,
        &mut pmats,
        &ui,
        &PanelSpec { id: PanelId::Clock, w: cw, h: ch, home, home_scale: 1.0, side: -1.0, phase: 2.2, show_at: crate::homepanels::show_at(4), unframed: true },
    );

    // ---- Agent moment card: centred over the core (prototype x 800, y 500, middle), in front of it
    let ui = spawn_ui_panel(&mut commands, &mut atlas, crate::moment::CARD_W, crate::moment::CARD_H);
    commands.insert_resource(crate::moment::CardRoot(ui.root));
    let mut home = stage_to_world(800.0, 500.0);
    home.z = 230.0;
    let e = spawn_panel_quad(
        &mut commands,
        &mut meshes,
        &mut pmats,
        &ui,
        &PanelSpec { id: PanelId::Moment, w: crate::moment::CARD_W, h: crate::moment::CARD_H, home, home_scale: 1.0, side: 0.0, phase: 0.4, show_at: 0.0, unframed: false },
    );
    c_moment_click(&mut commands, e, ui.root);

    // ---- Core readout, riding on the sphere deck
    let (rw, rh) = (272.0, 96.0);
    let ui = spawn_ui_panel(&mut commands, &mut atlas, rw, rh);
    build_readout_ui(&mut commands, ui.root, &f, &agents);
    let spec = PanelSpec { id: PanelId::Readout, w: rw, h: rh, home: Vec3::ZERO, home_scale: 1.0, side: 0.0, phase: 0.0, show_at: 0.0, unframed: true };
    let e = spawn_panel_quad(&mut commands, &mut meshes, &mut pmats, &ui, &spec);
    commands.entity(e).remove::<Panel>().insert((ReadoutLink, Transform::from_xyz(0.0, 0.0, 112.0)));
    // pull the material handle back out through a marker query later; parent it under the core fold
    commands.entity(decks.list[CORE].fold).add_child(e);
}

fn c_moment_click(commands: &mut Commands, quad: Entity, root: Entity) {
    commands.entity(quad).insert((crate::appframe::AppPart { app: crate::moment::CLICK_APP, pane: None }, crate::appframe::PaneRoot(root)));
}

#[derive(Component)]
pub struct ClockText;
#[derive(Component)]
pub struct DateText;

fn build_clock_ui(commands: &mut Commands, root: Entity, f: &UiFonts) {
    let body = commands
        .spawn(Node { width: percent(100), height: percent(100), flex_direction: FlexDirection::Column, align_items: AlignItems::FlexEnd, row_gap: px(6), ..default() })
        .id();
    commands.entity(root).add_child(body);
    let (time, date) = clock_strings();
    let t = commands
        .spawn((ClockText, Text::new(time), font(&f.display, 50.0, 1.0), TextColor(role_color(Role::Hot)), LineHeight::Px(50.0)))
        .id();
    let d = commands
        .spawn((DateText, Text::new(date), font(&f.ui600, 12.5, 3.75), TextColor(role_color(Role::Line)), LineHeight::Px(17.5)))
        .id();
    commands.entity(body).add_children(&[t, d]);
}

/// Local time as "13:34" and "FRIDAY, OCTOBER 16, 2026", from the system clock and zone (read once a minute, in-process: it used to
/// start `date` on the main thread, which could hitch a frame).
#[cfg(not(target_arch = "wasm32"))]
fn clock_strings() -> (String, String) {
    extern "C" {
        fn tzset();
    }
    // SAFETY: tzset, time and localtime_r only write into the zeroed `tm` we give them (tzset first, so a changed zone is noticed, as each run of `date` did).
    let tm = unsafe {
        tzset();
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        (!libc::localtime_r(&now, &mut tm).is_null()).then_some(tm)
    };
    tm.map_or_else(|| ("--:--".into(), String::new()), |tm| format_clock(&tm))
}

/// The two strings for a broken-down time, as `date "+%H:%M|%A, %B %-d, %Y"` printed them (the names are English: the app never sets a locale).
#[cfg(not(target_arch = "wasm32"))]
fn format_clock(tm: &libc::tm) -> (String, String) {
    let fmt = |f: &std::ffi::CStr| {
        let mut buf = [0u8; 96];
        // SAFETY: strftime writes at most `buf.len()` bytes and returns how many.
        let n = unsafe { libc::strftime(buf.as_mut_ptr().cast(), buf.len(), f.as_ptr(), tm) };
        String::from_utf8_lossy(&buf[..n]).into_owned()
    };
    (fmt(c"%H:%M"), fmt(c"%A, %B %-d, %Y").to_uppercase())
}

#[cfg(target_arch = "wasm32")]
use crate::web::clock_strings;

#[allow(clippy::type_complexity)]
/// Updates the clock when the minute changes (the panel camera then redraws once).
pub fn update_clock(mut last: Local<i64>, mut q: ParamSet<(Query<&mut Text, With<ClockText>>, Query<&mut Text, With<DateText>>)>, args: Res<Args>) {
    if args.shot.is_some() {
        return;
    }
    let min = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_secs() as i64 / 60).unwrap_or(0);
    if *last == min {
        return;
    }
    *last = min;
    let (t, d) = clock_strings();
    for mut x in &mut q.p0() {
        x.0 = t.clone();
    }
    for mut x in &mut q.p1() {
        x.0 = d.clone();
    }
}

#[derive(Component)]
pub struct ReadoutMain;
#[derive(Component)]
pub struct ReadoutSub;

#[allow(clippy::type_complexity)]
/// The readout on the core says what the service is doing: connecting, standby, working, needs you.
pub fn update_readout_text(
    agents: Res<Agents>,
    live: Res<crate::live::Live>,
    args: Res<Args>,
    mut q: ParamSet<(Query<(&mut Text, &mut TextColor), With<ReadoutMain>>, Query<&mut Text, With<ReadoutSub>>)>,
    sim: Res<crate::core::Sim>,
    mut booted: Local<bool>,
) {
    // boot: "Starting" with a progress line, then "Standby" at 2.1 s
    let mut just_booted = false;
    if !*booted {
        let t = sim.boot_t / 1.75;
        if t < 1.2 {
            let p = 1.0 - (1.0 - t.min(1.0)).powf(1.7);
            for (mut x, _) in &mut q.p0() {
                if x.0 != "STARTING" {
                    x.0 = "STARTING".into();
                }
            }
            let segs = (p * 10.0).round() as usize;
            let bar: String = (0..10).map(|i| if i < segs { '■' } else { '·' }).collect();
            let s = format!("BOOT  {bar}  {:>3.0}%", p * 100.0);
            for mut x in &mut q.p1() {
                if x.0 != s {
                    x.0 = s.clone();
                }
            }
            return;
        }
        *booted = true;
        just_booted = true;
        for (mut x, _) in &mut q.p0() {
            x.0 = "STANDBY".into();
        }
        for mut x in &mut q.p1() {
            x.0 = if args.offline() { agents.summary().to_uppercase() } else { "CONNECTING TO SPHERED".into() };
        }
    }
    // (it follows what changed; at the end of the boot it also says where things stand now)
    if args.offline() || !(just_booted || agents.is_changed() || live.is_changed()) {
        return;
    }
    use crate::service::Link;
    let needs = live.requests.len();
    let (main, hot, sub) = match &live.link {
        Some(Link::Connected) if needs > 0 => ("NEEDS YOU", false, agents.summary()),
        Some(Link::Connected) if agents.working() > 0 => ("WORKING", true, agents.summary()),
        Some(Link::Connected) => ("STANDBY", true, if agents.list.is_empty() { "no agents yet".to_string() } else { agents.summary() }),
        Some(Link::Down(_)) => ("OFFLINE", true, "sphered is not running".to_string()),
        _ => ("STANDBY", true, "connecting to sphered".to_string()),
    };
    for (mut t, mut c) in &mut q.p0() {
        if t.0 != main {
            t.0 = main.to_string();
            c.0 = role_color(if hot { Role::Hot } else { Role::Acc });
        }
    }
    let sub = sub.to_uppercase();
    for mut t in &mut q.p1() {
        if t.0 != sub {
            t.0 = sub.clone();
        }
    }
}

fn build_readout_ui(commands: &mut Commands, root: Entity, f: &UiFonts, agents: &Agents) {
    let body = commands
        .spawn(Node {
            width: percent(100),
            height: percent(100),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            row_gap: px(6),
            padding: UiRect { top: px(18), bottom: px(20), ..default() },
            ..default()
        })
        .id();
    commands.entity(root).add_child(body);
    let top = commands
        .spawn((
            Text::new("SPHERE CORE"),
            font(&f.ui600, 10.5, 4.83),
            TextColor(role_color(Role::Dim)),
            LineHeight::Px(10.5),
            Node { padding: UiRect { left: px(4.83), ..default() }, ..default() },
        ))
        .id();
    let main = commands
        .spawn((
            ReadoutMain,
            Text::new("STANDBY"),
            font(&f.display, 18.0, 5.4),
            TextColor(role_color(Role::Hot)),
            LineHeight::Px(21.6),
            Node { padding: UiRect { left: px(5.4), ..default() }, ..default() },
        ))
        .id();
    let sub = commands
        .spawn((ReadoutSub, Text::new(agents.summary().to_uppercase()), font(&f.ui600, 11.5, 2.76), TextColor(role_color(Role::Line)), LineHeight::Px(13.8)))
        .id();
    commands.entity(body).add_children(&[top, main, sub]);
}

pub fn update_readout(
    args: Res<Args>,
    ctl: Res<CoreCtl>,
    decks: Res<Decks>,
    mut mats: ResMut<Assets<PanelMaterial>>,
    q: Query<&MeshMaterial3d<PanelMaterial>, With<ReadoutLink>>,
) {
    let d = &decks.list[CORE];
    let op = clamp(1.0 - ctl.focus_amt * 1.4, 0.0, 1.0) * clamp(d.u_reveal * 1.4, 0.0, 1.0) * (1.0 - ctl.orb) * (1.0 - ctl.moment) * if args.icon { 0.0 } else { 1.0 };
    for h in &q {
        if mats.get(&h.0).is_some_and(|m| m.u.size.w != op) {
            if let Some(mut m) = mats.get_mut(&h.0) {
                m.u.size.w = op;
            }
        }
    }
}

// ---------------------------------------------------------------- backdrop

pub fn setup_backdrop(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<GlowMaterial>>,
    cam: Query<Entity, With<CamCore>>,
) {
    // only its transform and material change after this: no CPU copy once it's uploaded
    let mut m = Mesh::new(PrimitiveTopology::TriangleList, bevy::asset::RenderAssetUsages::RENDER_WORLD);
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, vec![[-0.5f32, -0.5, 0.0], [0.5, -0.5, 0.0], [0.5, 0.5, 0.0], [-0.5, -0.5, 0.0], [0.5, 0.5, 0.0], [-0.5, 0.5, 0.0]]);
    m.insert_attribute(
        ATTR_HOLO,
        vec![[0.0f32, 1.0, 0.0, 0.0], [1.0, 1.0, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0], [0.0, 0.0, 0.0, 0.0]],
    );
    let tint = hex("#03141a").to_srgba();
    let line = role_color(Role::Line).to_srgba();
    let mut gm = GlowMaterial::new(Kind::Backdrop, Vec4::new(tint.red, tint.green, tint.blue, 1.0), 1.0);
    gm.u.c2 = Vec4::new(line.red, line.green, line.blue, 1.0);
    let e = commands
        .spawn((Mesh3d(meshes.add(m)), MeshMaterial3d(mats.add(gm)), Transform::from_xyz(0.0, 0.0, -3000.0), Backdrop))
        .id();
    if let Ok(c) = cam.single() {
        commands.entity(c).add_child(e);
    }
}

#[derive(Component)]
pub struct Backdrop;

pub fn update_backdrop(
    sv: Res<StageView>,
    mut mats: ResMut<Assets<GlowMaterial>>,
    mut q: Query<(&mut Transform, &MeshMaterial3d<GlowMaterial>), With<Backdrop>>,
) {
    let half_h = (20.0f32 * DEG).tan() * 3000.0;
    for (mut t, h) in &mut q {
        t.scale = Vec3::new(2.0 * half_h * sv.w / sv.h, 2.0 * half_h, 1.0);
        let t = crate::theme::tint();
        let tint = Color::srgb(t.x, t.y, t.z).to_srgba();
        let line = crate::theme::now(Role::Line).to_srgba();
        let (c, c2) = (Vec4::new(tint.red, tint.green, tint.blue, 1.0), Vec4::new(line.red, line.green, line.blue, 1.0));
        if mats.get(&h.0).is_some_and(|m| m.u.c.x != sv.w || m.u.c.y != sv.h || m.u.color != c || m.u.c2 != c2) {
            if let Some(mut m) = mats.get_mut(&h.0) {
                m.u.c.x = sv.w;
                m.u.c.y = sv.h;
                m.u.color = c;
                m.u.c2 = c2;
            }
        }
    }
}

// ---------------------------------------------------------------- overlay, bench, shot

#[derive(Resource, Default)]
pub struct FrameStats {
    pub recent: Vec<f32>,
    pub all: Vec<f32>,
    pub frames: u64,
    pub since_label: f32,
    /// what the process had used when the bench started counting
    pub usage0: Option<crate::stats::Usage>,
}

#[derive(Component)]
pub struct StatsText;

/// The screen-pinned UI root (frame stats, hints, the command bar).
#[derive(Resource)]
pub struct OverlayRoot(pub Entity);

pub fn setup_overlay(mut commands: Commands, f: Res<UiFonts>, off: Res<Offscreen>, args: Res<Args>) {
    let (msaa, clear_color, output_mode) = camera_output(&args, WindowCam::Overlay);
    let cam = commands
        .spawn((
            Camera2d,
            Camera { order: 100, clear_color, output_mode, ..default() },
            bevy::camera::Hdr,
            msaa,
            bevy::core_pipeline::tonemapping::Tonemapping::None,
            bevy::core_pipeline::tonemapping::DebandDither::Disabled,
            target_for(&off),
            RenderLayers::layer(7),
        ))
        .id();
    let root = commands
        .spawn((
            Node { width: percent(100), height: percent(100), position_type: PositionType::Absolute, ..default() },
            UiTargetCamera(cam),
        ))
        .id();
    let stats = commands
        .spawn((
            StatsText,
            // not in recordings (or shots with SPHERE_CLEAN)
            if args.record.is_some() || std::env::var_os("SPHERE_CLEAN").is_some() { Visibility::Hidden } else { Visibility::Inherited },
            Text::new("FPS --"),
            font(&f.mono, 11.0, 0.8),
            TextColor(with_alpha(role_color(Role::Line), 0.8)),
            Node { position_type: PositionType::Absolute, left: px(18), top: px(14), ..default() },
        ))
        .id();
commands.entity(root).add_child(stats);
    if args.icon {
        commands.entity(root).insert(Visibility::Hidden);
    }
    commands.insert_resource(OverlayRoot(root));
}

fn pct(v: &mut [f32], p: f32) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() - 1) as f32 * p).round() as usize]
}

pub fn frame_stats(time: Res<Time<Real>>, sim: Res<Sim>, mut st: ResMut<FrameStats>, mut text: Query<&mut Text, With<StatsText>>) {
    if sim.frozen {
        return;
    }
    let ms = time.delta_secs() * 1000.0;
    st.frames += 1;
    if ms > 50.0 && crate::stats::on() {
        eprintln!("stats: long frame {ms:.0} ms (frame {}) at {:.0} ms", st.frames, crate::stats::ms());
    }
    if st.frames > 20 {
        if st.usage0.is_none() {
            st.usage0 = Some(crate::stats::Usage::now());
        }
        st.recent.push(ms);
        if st.recent.len() > 300 {
            st.recent.remove(0);
        }
        st.all.push(ms);
    }
    st.since_label += time.delta_secs();
    if st.since_label > 0.25 && !st.recent.is_empty() {
        st.since_label = 0.0;
        let mean: f32 = st.recent.iter().sum::<f32>() / st.recent.len() as f32;
        let mut v = st.recent.clone();
        let (p50, p99) = (pct(&mut v, 0.5), pct(&mut v, 0.99));
        for mut t in &mut text {
            t.0 = format!("FPS {:>4.0}   P50 {:>5.1} MS   P99 {:>5.1} MS", 1000.0 / mean, p50, p99);
        }
    }
}

pub fn bench(args: Res<Args>, time: Res<Time<Real>>, st: Res<FrameStats>, window: Query<&Window, With<bevy::window::PrimaryWindow>>, mut exit: MessageWriter<AppExit>) {
    let Some(secs) = args.bench else { return };
    if time.elapsed_secs() < secs + 1.5 {
        return;
    }
    let mut v = st.all.clone();
    let mean: f32 = v.iter().sum::<f32>() / v.len().max(1) as f32;
    let (p50, p99, p999) = (pct(&mut v, 0.5), pct(&mut v, 0.99), pct(&mut v, 0.999));
    let max = v.last().copied().unwrap_or(0.0);
    // CPU time per frame: the main thread, the render thread, and every thread of the process together
    let now = crate::stats::Usage::now();
    let n = v.len() as u64;
    let (main_ms, render_ms, all_ms) = st.usage0.as_ref().map_or((0.0, 0.0, 0.0), |a| now.per_frame_ms(a, n));
    println!(
        "bench: {} frames over {:.1}s | avg {:.1} fps | frame ms p50 {:.2}  p99 {:.2}  p99.9 {:.2}  max {:.2} | cpu ms/frame main {:.2}  render {:.2}  all threads {:.2}",
        v.len(),
        v.iter().sum::<f32>() / 1000.0,
        1000.0 / mean.max(0.0001),
        p50,
        p99,
        p999,
        max,
        main_ms,
        render_ms,
        all_ms
    );
    if let Some(a) = &st.usage0 {
        let w = window.iter().next().map(|w| format!("window {}x{} px at scale {}", w.physical_width(), w.physical_height(), w.scale_factor())).unwrap_or_else(|| "offscreen".into());
        println!("usage: {w} | {}", crate::stats::usage_line(a, &now, n));
    }
    exit.write(AppExit::Success);
}

#[derive(Default)]
pub struct ShotState {
    frame: u32,
    last_ask: f32,
    attempts: u32,
    prev: std::sync::Arc<std::sync::Mutex<Option<Vec<u8>>>>,
}

/// `--shot`: render N frames, freeze the simulation, then read the offscreen render target back from the GPU
/// until two successive readbacks are identical (the render world can lag far behind the main world on a software
/// rasteriser while its pipelines compile), and write that as a PNG.
pub fn shot(args: Res<Args>, off: Res<Offscreen>, mut sim: ResMut<Sim>, time: Res<Time<Real>>, mut commands: Commands, mut s: Local<ShotState>, mut exit: MessageWriter<AppExit>) {
    let Some(path) = args.shot.clone() else { return };
    if s.frame == 0 {
        let _ = std::fs::remove_file(&path);
    }
    s.frame += 1;
    if s.frame < args.frames {
        return;
    }
    sim.frozen = true;
    let now = time.elapsed_secs();
    if now - s.last_ask < 2.0 {
        return;
    }
    s.last_ask = now;
    s.attempts += 1;
    if s.attempts > 200 {
        eprintln!("screenshot never settled");
        exit.write(AppExit::error());
        return;
    }
    let Some(img) = &off.0 else { return };
    let (w, h) = args.size;
    let p = path.clone();
    let prev = s.prev.clone();
    let attempt = s.attempts;
    commands.spawn(ReadbackOnce::texture(img.clone())).observe(move |ev: On<ReadbackComplete>, mut exit: MessageWriter<AppExit>| {
        let nonzero = ev.data.iter().filter(|b| **b != 0).count();
        let same = prev.lock().unwrap().as_ref().map(|p| *p == ev.data).unwrap_or(false);
        eprintln!("shot attempt {attempt}: {nonzero} non-zero bytes, same as previous: {same}");
        *prev.lock().unwrap() = Some(ev.data.clone());
        if nonzero == 0 || !same {
            return;
        }
        let image = Image::new(
            Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            TextureDimension::D2,
            unpad(&ev.data, w, h),
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::default(),
        );
        match image.try_into_dynamic() {
            Ok(d) => match d.to_rgb8().save(&p) {
                Ok(_) => {
                    println!("screenshot saved to {p}");
                    exit.write(AppExit::Success);
                }
                Err(e) => eprintln!("could not save {p}: {e}"),
            },
            Err(e) => eprintln!("could not convert screenshot: {e}"),
        }
    });
}

#[derive(Default)]
pub struct RecordState {
    frame: u32,
    asked: u32,
    saved: std::sync::Arc<std::sync::atomic::AtomicU32>,
}

/// `--record <dir>`: after `--frames` of warm-up, reads back every frame of the offscreen image and writes it as
/// `<dir>/frame-00000.png` (the simulation steps a fixed `--dt` per frame, so it plays back at 1 / dt fps).
/// Quits once `--record-frames` are on disk.
pub fn record(args: Res<Args>, off: Res<Offscreen>, mut commands: Commands, mut s: Local<RecordState>, mut exit: MessageWriter<AppExit>) {
    let Some(dir) = args.record.clone() else { return };
    s.frame += 1;
    if s.frame == 1 {
        let _ = std::fs::create_dir_all(&dir);
    }
    let saved = s.saved.load(std::sync::atomic::Ordering::SeqCst);
    if saved >= args.record_frames {
        println!("recorded {saved} frames to {dir}");
        exit.write(AppExit::Success);
        return;
    }
    if s.frame <= args.frames || s.asked >= args.record_frames {
        return;
    }
    let Some(img) = &off.0 else { return };
    let i = s.asked;
    s.asked += 1;
    let (w, h) = args.size;
    let counter = s.saved.clone();
    let path = format!("{dir}/frame-{i:05}.png");
    commands.spawn(ReadbackOnce::texture(img.clone())).observe(move |ev: On<ReadbackComplete>| {
        let image = Image::new(Extent3d { width: w, height: h, depth_or_array_layers: 1 }, TextureDimension::D2, unpad(&ev.data, w, h), TextureFormat::Rgba8UnormSrgb, RenderAssetUsages::default());
        if let Ok(d) = image.try_into_dynamic() {
            if d.to_rgb8().save(&path).is_ok() {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        }
    });
}

/// `SPHERE_SCRIPT="1.5:a 3:right 4.2:enter"`: presses keys at those simulation times (seconds), for recordings.
/// Names: a letter or digit, or left right up down enter esc space tab slash.
pub fn script(sim: Res<Sim>, mut out: MessageWriter<bevy::input::keyboard::KeyboardInput>, mut plan: Local<Option<Vec<(f32, String)>>>) {
    use bevy::input::keyboard::{Key, KeyboardInput};
    use bevy::input::ButtonState;
    let plan = plan.get_or_insert_with(|| {
        std::env::var("SPHERE_SCRIPT").unwrap_or_default().split_whitespace().filter_map(|p| p.split_once(':')).filter_map(|(t, k)| Some((t.parse().ok()?, k.to_lowercase()))).collect()
    });
    while let Some(pos) = plan.iter().position(|(t, _)| sim.t >= *t) {
        let (_, k) = plan.remove(pos);
        let (code, logical) = match k.as_str() {
            "left" => (KeyCode::ArrowLeft, Key::ArrowLeft),
            "right" => (KeyCode::ArrowRight, Key::ArrowRight),
            "up" => (KeyCode::ArrowUp, Key::ArrowUp),
            "down" => (KeyCode::ArrowDown, Key::ArrowDown),
            "enter" => (KeyCode::Enter, Key::Enter),
            "esc" => (KeyCode::Escape, Key::Escape),
            "space" => (KeyCode::Space, Key::Space),
            "tab" => (KeyCode::Tab, Key::Tab),
            "slash" => (KeyCode::Slash, Key::Character("/".into())),
            s if s.len() == 1 => {
                let c = s.chars().next().unwrap();
                let code = match c {
                    'a'..='z' => [KeyCode::KeyA, KeyCode::KeyB, KeyCode::KeyC, KeyCode::KeyD, KeyCode::KeyE, KeyCode::KeyF, KeyCode::KeyG, KeyCode::KeyH, KeyCode::KeyI, KeyCode::KeyJ, KeyCode::KeyK, KeyCode::KeyL, KeyCode::KeyM, KeyCode::KeyN, KeyCode::KeyO, KeyCode::KeyP, KeyCode::KeyQ, KeyCode::KeyR, KeyCode::KeyS, KeyCode::KeyT, KeyCode::KeyU, KeyCode::KeyV, KeyCode::KeyW, KeyCode::KeyX, KeyCode::KeyY, KeyCode::KeyZ][(c as u8 - b'a') as usize],
                    '0'..='9' => [KeyCode::Digit0, KeyCode::Digit1, KeyCode::Digit2, KeyCode::Digit3, KeyCode::Digit4, KeyCode::Digit5, KeyCode::Digit6, KeyCode::Digit7, KeyCode::Digit8, KeyCode::Digit9][(c as u8 - b'0') as usize],
                    _ => continue,
                };
                (code, Key::Character(s.into()))
            }
            _ => continue,
        };
        for state in [ButtonState::Pressed, ButtonState::Released] {
            out.write(KeyboardInput { key_code: code, logical_key: logical.clone(), state, text: None, repeat: false, window: Entity::PLACEHOLDER });
        }
    }
}

/// GPU readbacks pad each row to 256 bytes; drops the padding (sizes whose rows aren't a multiple of 64 px).
fn unpad(data: &[u8], w: u32, h: u32) -> Vec<u8> {
    let row = w as usize * 4;
    let padded = row.div_ceil(256) * 256;
    if padded == row || data.len() < padded * h as usize {
        return data.to_vec();
    }
    (0..h as usize).flat_map(|y| data[y * padded..y * padded + row].iter().copied()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::Live;
    use crate::service::Link;

    /// A fixed UTC moment, broken down the way `clock_strings` formats the local one.
    fn at(t: libc::time_t) -> (String, String) {
        // SAFETY: gmtime_r only writes into the zeroed `tm` it is given.
        let tm = unsafe {
            let mut tm: libc::tm = std::mem::zeroed();
            libc::gmtime_r(&t, &mut tm);
            tm
        };
        format_clock(&tm)
    }

    #[test]
    fn the_clock_reads_hours_and_minutes_and_an_upper_case_date() {
        assert_eq!(at(1792416840), ("13:34".to_string(), "MONDAY, OCTOBER 19, 2026".to_string()));
        // hours and minutes keep their zero, the day of the month doesn't
        assert_eq!(at(1772600760), ("05:06".to_string(), "WEDNESDAY, MARCH 4, 2026".to_string()));
    }

    #[test]
    fn the_clock_agrees_with_the_date_command_it_replaced() {
        for _ in 0..3 {
            let out = std::process::Command::new("date").env("LC_ALL", "C").arg("+%H:%M|%A, %B %-d, %Y").output().unwrap();
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let (t, d) = s.split_once('|').unwrap();
            let now = clock_strings();
            // the minute may have ticked between the two: look again
            if now.0 == t {
                assert_eq!(now.1, d.to_uppercase());
                return;
            }
        }
        panic!("the minute kept changing");
    }

    fn args() -> Args {
        Args { cmd: None, bench: None, shot: None, record: None, record_frames: 180, frames: 120, depth: false, orbit: false, apps: false, work: false, swarm: false, code: false, git: false, open_app: None, icon: false, aa: "smaa".into(), offscreen: false, collapsed: false, focus_term: false, msaa: true, size: (1600, 1000), dt: 0.05, demo: false, live: false, moment: false, elsewhere: false, calendar: false, takeover: None, memory: None, protocols: None, fixit: None, theme: None, motion: None }
    }

    /// The readout only follows what changed (`Agents` and `Live` are no longer marked changed every frame), so the
    /// frame the boot ends in has to say where things stand: they may have settled while the boot ran.
    #[test]
    fn the_readout_says_where_things_stand_when_the_boot_ends() {
        let mut app = App::new();
        app.insert_resource(Agents::empty())
            .insert_resource(Live::default())
            .insert_resource(args())
            .insert_resource(Sim { dt: 0.05, t: 0.0, boot_t: 0.5, fixed_dt: None, frozen: false })
            .add_systems(Update, update_readout_text);
        let main = app.world_mut().spawn((ReadoutMain, Text::new("STANDBY"), TextColor(Color::WHITE))).id();
        let sub = app.world_mut().spawn((ReadoutSub, Text::new(""))).id();
        let words = |app: &App| (app.world().get::<Text>(main).unwrap().0.clone(), app.world().get::<Text>(sub).unwrap().0.clone());
        app.update();
        assert_eq!(words(&app).0, "STARTING");
        // the link comes up and an agent starts while the boot still runs
        app.world_mut().resource_mut::<Live>().link = Some(Link::Connected);
        let working = Agent { name: "a".into(), state: AState::Working, p: 0.5, step: "x".into(), rate: 0.0, figure: None, chip: None, protocol: false };
        app.world_mut().resource_mut::<Agents>().set_roster(Roster::of(&[working]));
        app.update();
        assert_eq!(words(&app).0, "STARTING");
        // the boot ends in a frame where nothing changed
        app.world_mut().resource_mut::<Sim>().boot_t = 99.0;
        app.update();
        assert_eq!(words(&app), ("WORKING".to_string(), "1 WORKING · ALL CLEAR".to_string()));
        // and from then on the readout follows the changes
        app.world_mut().resource_mut::<Live>().link = Some(Link::Down("gone".into()));
        app.update();
        assert_eq!(words(&app).0, "OFFLINE");
    }
}
