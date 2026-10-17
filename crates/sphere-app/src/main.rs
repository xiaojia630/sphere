//! Sphere OS look test (M0): proves the approved HUD look can be matched natively in Rust.
mod cmdbar;
mod core;
mod fonts;
mod geo;
mod glow;
mod hud;
mod live;
mod moment;
mod panel;
mod service;
mod term;
mod util;

use bevy::{
    camera::{visibility::RenderLayers, CameraOutputMode, ClearColorConfig},
    core_pipeline::tonemapping::{DebandDither, Tonemapping},
    post_process::bloom::{Bloom, BloomCompositeMode},
    prelude::*,
    window::{ExitCondition, PresentMode, PrimaryWindow},
    winit::WinitPlugin,
    camera::{ImageRenderTarget, RenderTarget},
    render::render_resource::{BlendState, TextureFormat, TextureUsages},
    app::ScheduleRunnerPlugin,
};
use core::*;
use util::*;

#[derive(Resource, Clone, Debug)]
pub struct Args {
    pub cmd: Option<String>,
    pub bench: Option<f32>,
    pub shot: Option<String>,
    pub frames: u32,
    pub depth: bool,
    pub focus_term: bool,
    pub msaa: bool,
    pub size: (u32, u32),
    pub dt: f32,
    /// The look test's demo data instead of sphered (always on for `--shot` and `--bench`).
    pub demo: bool,
    /// `--live`: screenshots and benches use sphered too.
    pub live: bool,
}

impl Args {
    /// Screenshots, benches and `--demo` never touch the service.
    pub fn offline(&self) -> bool {
        !self.live && (self.demo || self.shot.is_some() || self.bench.is_some())
    }
}

fn parse_args() -> Args {
    let mut a = Args { cmd: None, bench: None, shot: None, frames: 120, depth: false, focus_term: false, msaa: true, size: (1600, 1000), dt: 1.0 / 20.0, demo: false, live: false };
    let mut it = std::env::args().skip(1);
    while let Some(s) = it.next() {
        match s.as_str() {
            "--cmd" => a.cmd = it.next(),
            "--bench" => a.bench = it.next().and_then(|v| v.parse().ok()),
            "--shot" => a.shot = it.next(),
            "--frames" => a.frames = it.next().and_then(|v| v.parse().ok()).unwrap_or(120),
            "--depth" => a.depth = true,
            "--term" => a.focus_term = true,
            "--no-msaa" => a.msaa = false,
            "--demo" => a.demo = true,
            "--live" => a.live = true,
            "--dt" => a.dt = it.next().and_then(|v| v.parse().ok()).unwrap_or(0.05),
            "--size" => {
                if let Some(v) = it.next() {
                    let p: Vec<u32> = v.split('x').filter_map(|x| x.parse().ok()).collect();
                    if p.len() == 2 {
                        a.size = (p[0], p[1]);
                    }
                }
            }
            "-h" | "--help" => {
                println!("sphere-app [--demo] [--live] [--cmd \"<command>\"] [--bench <seconds>] [--shot <png> [--frames N] [--depth] [--term]] [--size WxH] [--no-msaa]");
                std::process::exit(0);
            }
            other => eprintln!("ignoring unknown argument {other}"),
        }
    }
    a
}

#[derive(Component)]
pub struct CamCore;
#[derive(Component)]
pub struct CamPanels;

fn main() {
    let args = parse_args();
    let mut app = App::new();
    app.insert_resource(args.clone())
        .insert_resource(ClearColor(Color::BLACK))
        .add_plugins(default_plugins(&args))
        .add_plugins((glow::GlowPlugin, panel::PanelPlugin))
        .add_systems(PreStartup, setup_offscreen)
        .insert_resource(Sim { dt: 1.0 / 60.0, t: 0.0, boot_t: 0.0, fixed_dt: args.shot.as_ref().map(|_| args.dt), frozen: false })
        .insert_resource(CoreCtl::new(true))
        .insert_resource(StageView::compute(args.size.0 as f32, args.size.1 as f32))
        .insert_resource(if args.offline() { Agents::demo() } else { Agents::empty() })
        .insert_resource(service::start(args.offline()))
        .init_resource::<live::Live>()
        .init_resource::<live::Toasts>()
        .init_resource::<cmdbar::CmdBar>()
        .init_resource::<moment::Moment>()
        .init_resource::<hud::FrameStats>()
        .add_systems(Startup, (setup_core, setup_cameras, hud::setup_panels, hud::setup_backdrop, hud::setup_overlay, cmdbar::setup).chain())
        .add_systems(First, tick_sim)
        .add_systems(
            Update,
            (
                (keys, stage_view, camera_rig, update_core, spin_system, update_materials, panel::update_panels, hud::update_readout, hud::update_backdrop)
                    .chain(),
                (live::sync, live::tick_toasts, panel::rebuild_agents_ui, hud::update_readout_text, hud::update_clock, panel::blink_system).chain(),
                (term::pump, cmdbar::keys, term::read_keys, term::draw, cmdbar::draw).chain(),
                (hud::frame_stats, hud::bench, hud::shot).chain(),
            )
                .chain(),
        )
        .add_systems(Last, panel::gate_panel_cameras);
    if args.shot.is_some() {
        app.add_plugins(ScheduleRunnerPlugin::run_loop(std::time::Duration::from_millis(1)));
        // Offscreen there is no swap chain to throttle submission, and a software rasteriser would let queued
        // frames pile up in memory. Wait for the GPU after each frame instead.
        if let Some(r) = app.get_sub_app_mut(bevy::render::RenderApp) {
            r.add_systems(bevy::render::Render, wait_for_gpu.after(bevy::render::RenderSystems::Render));
        }
    }
    app.run();
}

/// `--shot` renders offscreen into an image: no window, no display server, no surface needed.
fn default_plugins(args: &Args) -> impl PluginGroup {
    if args.shot.is_some() {
        DefaultPlugins
            .set(WindowPlugin { primary_window: None, exit_condition: ExitCondition::DontExit, ..default() })
            .disable::<WinitPlugin>()
    } else {
        DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Sphere".into(),
                resolution: (args.size.0, args.size.1).into(),
                present_mode: if args.bench.is_some() { PresentMode::AutoNoVsync } else { PresentMode::AutoVsync },
                ..default()
            }),
            ..default()
        })
    }
}

/// Where cameras draw: the window, or the offscreen image in `--shot` mode.
#[derive(Resource, Clone)]
pub struct Offscreen(pub Option<Handle<Image>>);

pub fn target_for(off: &Offscreen) -> RenderTarget {
    match &off.0 {
        Some(h) => RenderTarget::Image(ImageRenderTarget { handle: h.clone(), scale_factor: 1.0 }),
        None => RenderTarget::default(),
    }
}

fn setup_offscreen(mut commands: Commands, mut images: ResMut<Assets<Image>>, args: Res<Args>) {
    if args.shot.is_none() {
        commands.insert_resource(Offscreen(None));
        return;
    }
    let mut img = Image::new_target_texture(args.size.0, args.size.1, TextureFormat::Rgba8UnormSrgb, None);
    img.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    commands.insert_resource(Offscreen(Some(images.add(img))));
}

fn wait_for_gpu(device: Res<bevy::render::renderer::RenderDevice>) {
    let _ = device.poll(bevy::render::render_resource::PollType::wait_indefinitely());
}

fn tick_sim(time: Res<Time<Real>>, mut sim: ResMut<Sim>) {
    let dt = match sim.fixed_dt {
        Some(d) => d,
        None => time.delta_secs().min(1.0 / 30.0),
    };
    let dt = if sim.frozen { 0.0 } else { dt };
    sim.dt = dt;
    sim.t += dt;
    sim.boot_t += dt;
}

/// How the three window cameras (core, panels, overlay) share textures and reach the window.
///
/// Bevy gives cameras on one target a shared main texture only when their MSAA matches, and a camera that draws
/// over a shared MSAA texture first copies the resolved image back into it (a full-screen 4x "writeback"). With
/// MSAA on, only the core's lines need it: the panels are textured quads whose edges are transparent padding, and
/// the overlay is UI. So the core renders with MSAA and writes the window; the panels and the overlay share one
/// non-MSAA texture, cleared to transparent, and the overlay blends it over the window (premultiplied, as both
/// draw it). Without MSAA all three share one texture and only the last camera writes the window.
pub fn camera_output(args: &Args, cam: WindowCam) -> (Msaa, ClearColorConfig, CameraOutputMode) {
    let skip = CameraOutputMode::Skip;
    let write = |blend| CameraOutputMode::Write { blend_state: Some(blend), clear_color: ClearColorConfig::None };
    match (args.msaa, cam) {
        (true, WindowCam::Core) => (Msaa::Sample4, ClearColorConfig::Custom(Color::BLACK), CameraOutputMode::default()),
        (true, WindowCam::Panels) => (Msaa::Off, ClearColorConfig::Custom(Color::NONE), skip),
        (true, WindowCam::Overlay) => (Msaa::Off, ClearColorConfig::None, write(BlendState::PREMULTIPLIED_ALPHA_BLENDING)),
        (false, WindowCam::Core) => (Msaa::Off, ClearColorConfig::Custom(Color::BLACK), skip),
        (false, WindowCam::Panels) => (Msaa::Off, ClearColorConfig::None, skip),
        (false, WindowCam::Overlay) => (Msaa::Off, ClearColorConfig::None, write(BlendState::REPLACE)),
    }
}

#[derive(Clone, Copy)]
pub enum WindowCam {
    Core,
    Panels,
    Overlay,
}

fn setup_cameras(mut commands: Commands, args: Res<Args>, off: Res<Offscreen>) {
    let proj = Projection::Perspective(PerspectiveProjection { fov: 40.0 * DEG, near: 10.0, far: 40000.0, ..default() });
    let (msaa, clear_color, output_mode) = camera_output(&args, WindowCam::Core);
    commands.spawn((
        Camera3d::default(),
        Camera { order: 0, clear_color, output_mode, ..default() },
        bevy::camera::Hdr,
        Tonemapping::None,
        DebandDither::Disabled,
        Bloom { intensity: 0.22, low_frequency_boost: 0.7, composite_mode: BloomCompositeMode::Additive, ..Bloom::NATURAL },
        proj.clone(),
        msaa,
        target_for(&off),
        Transform::from_xyz(0.0, 0.0, 1500.0),
        CamCore,
    ));
    let (msaa, clear_color, output_mode) = camera_output(&args, WindowCam::Panels);
    commands.spawn((
        Camera3d::default(),
        Camera { order: 1, clear_color, output_mode, ..default() },
        bevy::camera::Hdr,
        Tonemapping::None,
        DebandDither::Disabled,
        proj,
        msaa,
        target_for(&off),
        RenderLayers::layer(1),
        Transform::from_xyz(0.0, 0.0, 1500.0),
        CamPanels,
    ));
}

fn stage_view(win: Query<&Window, With<PrimaryWindow>>, mut sv: ResMut<StageView>) {
    if let Ok(w) = win.single() {
        *sv = StageView::compute(w.width(), w.height());
    }
}

#[allow(clippy::type_complexity)]
fn camera_rig(sv: Res<StageView>, ctl: Res<CoreCtl>, mut cams: Query<&mut Transform, Or<(With<CamCore>, With<CamPanels>)>>) {
    for mut t in &mut cams {
        t.translation = Vec3::new(0.0, sv.base_y, sv.d * ctl.zoom.x);
    }
}

fn keys(mut ctl: ResMut<CoreCtl>, args: Res<Args>, mut once: Local<bool>, win: Query<&Window, With<PrimaryWindow>>) {
    if !*once {
        *once = true;
        if args.depth {
            ctl.set_mode(Mode::Depth);
        }
    }
    if let Ok(w) = win.single() {
        ctl.pointer = w.cursor_position().map(|p| Vec2::new(p.x / w.width(), p.y / w.height()));
    }
}
