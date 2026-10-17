//! Scene glue: the panels, the screen backdrop, the frame-time overlay, `--bench` and `--shot`.
use crate::core::*;
use crate::fonts::{self, UiFonts};
use crate::glow::*;
use crate::panel::*;
use crate::term::{self, Terminal, TERM_H, TERM_W};
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
    mut images: ResMut<Assets<Image>>,
    mut fonts_a: ResMut<Assets<Font>>,
    mut pmats: ResMut<Assets<PanelMaterial>>,
    agents: Res<Agents>,
    decks: Res<Decks>,
    args: Res<Args>,
    win: Query<&Window, With<bevy::window::PrimaryWindow>>,
) {
    let f = fonts::load_ui_fonts(&mut fonts_a);
    commands.insert_resource(f.clone());
    let sv = StageView::compute(args.size.0 as f32, args.size.1 as f32);
    let dpr = win.single().map(|w| w.scale_factor()).unwrap_or(1.0);
    // texture px per css px: the panel is shown at about `k` on screen, so match the screen pixels
    let scale = (sv.k * dpr).max(SCALE_MIN) * 1.0;
    commands.insert_resource(PanelScale(scale));
    commands.insert_resource(Focus { terminal: args.focus_term });

    // ---- Agents panel: right column, bottom aligned like the prototype (x 1546, y 962)
    let (aw, ah) = (356.0, 351.0);
    let ui = spawn_ui_panel(&mut commands, &mut images, aw, ah, scale, -10);
    build_agents_ui(&mut commands, ui.root, &f, &agents, 0.0);
    let home = stage_to_world(1546.0 - aw / 2.0, 962.0 - ah / 2.0);
    spawn_panel_quad(
        &mut commands,
        &mut meshes,
        &mut pmats,
        &ui,
        &PanelSpec { id: PanelId::Agents, w: aw, h: ah, home, home_scale: 1.0, side: -1.0, phase: 1.3, show_at: 0.9, unframed: false },
    );

    // ---- Terminal panel: left column
    let ui = spawn_ui_panel(&mut commands, &mut images, TERM_W, TERM_H, scale, -9);
    let tui = term::build_term_ui(&mut commands, ui.root, &f);
    commands.insert_resource(tui);
    let default_cmd = "git log --graph --color --oneline -20";
    let first = args.cmd.clone().unwrap_or_else(|| default_cmd.to_string());
    match Terminal::spawn(&first) {
        Ok(t) => {
            commands.insert_resource(t);
        }
        Err(e) => eprintln!("could not start a terminal: {e}"),
    }
    let home = stage_to_world(54.0 + TERM_W / 2.0, 70.0 + TERM_H / 2.0);
    spawn_panel_quad(
        &mut commands,
        &mut meshes,
        &mut pmats,
        &ui,
        &PanelSpec { id: PanelId::Terminal, w: TERM_W, h: TERM_H, home, home_scale: 1.0, side: 1.0, phase: 4.1, show_at: 1.1, unframed: false },
    );

    // ---- Core readout, riding on the sphere deck
    let (rw, rh) = (272.0, 96.0);
    let ui = spawn_ui_panel(&mut commands, &mut images, rw, rh, scale, -8);
    build_readout_ui(&mut commands, ui.root, &f, &agents);
    let spec = PanelSpec { id: PanelId::Readout, w: rw, h: rh, home: Vec3::ZERO, home_scale: 1.0, side: 0.0, phase: 0.0, show_at: 0.0, unframed: true };
    let e = spawn_panel_quad(&mut commands, &mut meshes, &mut pmats, &ui, &spec);
    commands.entity(e).remove::<Panel>().insert((ReadoutLink, Transform::from_xyz(0.0, 0.0, 112.0)));
    // pull the material handle back out through a marker query later; parent it under the core fold
    commands.entity(decks.list[CORE].fold).add_child(e);
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
            Text::new("STANDBY"),
            font(&f.display, 18.0, 5.4),
            TextColor(role_color(Role::Hot)),
            LineHeight::Px(21.6),
            Node { padding: UiRect { left: px(5.4), ..default() }, ..default() },
        ))
        .id();
    let sub = commands
        .spawn((Text::new(agents.summary().to_uppercase()), font(&f.ui600, 11.5, 2.76), TextColor(role_color(Role::Line)), LineHeight::Px(13.8)))
        .id();
    commands.entity(body).add_children(&[top, main, sub]);
}

pub fn update_readout(
    ctl: Res<CoreCtl>,
    decks: Res<Decks>,
    mut mats: ResMut<Assets<PanelMaterial>>,
    q: Query<&MeshMaterial3d<PanelMaterial>, With<ReadoutLink>>,
) {
    let d = &decks.list[CORE];
    let op = clamp(1.0 - ctl.focus_amt * 1.4, 0.0, 1.0) * clamp(d.u_reveal * 1.4, 0.0, 1.0);
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
    let mut m = Mesh::new(PrimitiveTopology::TriangleList, bevy::asset::RenderAssetUsages::default());
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
        if mats.get(&h.0).is_some_and(|m| m.u.c.x != sv.w || m.u.c.y != sv.h) {
            if let Some(mut m) = mats.get_mut(&h.0) {
                m.u.c.x = sv.w;
                m.u.c.y = sv.h;
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
}

#[derive(Component)]
pub struct StatsText;

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
            Text::new("FPS --"),
            font(&f.mono, 11.0, 0.8),
            TextColor(with_alpha(role_color(Role::Line), 0.8)),
            Node { position_type: PositionType::Absolute, left: px(18), top: px(14), ..default() },
        ))
        .id();
    let hint = commands
        .spawn((
            Text::new("D DEPTH   ·   T TERMINAL   ·   ESC RELEASE"),
            font(&f.ui600, 10.5, 3.2),
            TextColor(with_alpha(role_color(Role::Dim), 0.95)),
            Node { position_type: PositionType::Absolute, left: percent(50), margin: UiRect { left: px(-190), ..default() }, bottom: px(26), ..default() },
        ))
        .id();
    commands.entity(root).add_children(&[stats, hint]);
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
    if st.frames > 20 {
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

pub fn bench(args: Res<Args>, time: Res<Time<Real>>, st: Res<FrameStats>, mut exit: MessageWriter<AppExit>) {
    let Some(secs) = args.bench else { return };
    if time.elapsed_secs() < secs + 1.5 {
        return;
    }
    let mut v = st.all.clone();
    let mean: f32 = v.iter().sum::<f32>() / v.len().max(1) as f32;
    let (p50, p99, p999) = (pct(&mut v, 0.5), pct(&mut v, 0.99), pct(&mut v, 0.999));
    let max = v.last().copied().unwrap_or(0.0);
    println!(
        "bench: {} frames over {:.1}s | avg {:.1} fps | frame ms p50 {:.2}  p99 {:.2}  p99.9 {:.2}  max {:.2}",
        v.len(),
        v.iter().sum::<f32>() / 1000.0,
        1000.0 / mean.max(0.0001),
        p50,
        p99,
        p999,
        max
    );
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
            ev.data.clone(),
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
