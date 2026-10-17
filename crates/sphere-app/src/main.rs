//! Sphere OS look test (M0): proves the approved HUD look can be matched natively in Rust.
mod appframe;
mod apps;
mod callouts;
mod codeapp;
mod containers;
mod cmdbar;
mod core;
mod filesapp;
mod fonts;
mod fx;
mod home;
mod hostterm;
mod homepanels;
mod geo;
mod gitapp;
mod gitdata;
mod glow;
mod hud;
mod live;
mod moment;
mod momentfx;
mod mouse;
mod orbit;
mod opsdata;
mod ports;
mod panel;
mod service;
mod sound;
mod summon;
mod sysapp;
mod sysstats;
mod theme;
mod review;
mod stats;
mod term;
mod util;
mod visor;
mod working;

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
    pub orbit: bool,
    /// open the core (the app orbit) at start, for screenshots
    pub apps: bool,
    /// open the Working view at start, for screenshots
    pub work: bool,
    /// open the Code app at start, for screenshots
    pub code: bool,
    /// open the Git app at start
    pub git: bool,
    /// open this app at start: containers, ports, system
    pub open_app: Option<String>,
    /// only the core (the app icon render)
    pub icon: bool,
    /// anti-aliasing on the core: smaa (default; a post pass, so the three window cameras share one non-MSAA
    /// texture, at about a third of MSAA's memory), msaa4, msaa2, fxaa, none
    pub aa: String,
    /// `--offscreen` with `--bench`: render into an image and wait for the GPU each frame, so the numbers are the
    /// whole frame's cost, not the display's refresh
    pub offscreen: bool,
    pub collapsed: bool,
    pub focus_term: bool,
    pub msaa: bool,
    pub size: (u32, u32),
    pub dt: f32,
    /// The look test's demo data instead of sphered (always on for `--shot` and `--bench`).
    pub demo: bool,
    /// `--live`: screenshots and benches use sphered too.
    pub live: bool,
    /// `--moment`: the demo shows the Agent moment card with a sample request.
    pub moment: bool,
    /// `--theme <name>`: balanced, full, cyan, black, red
    pub theme: Option<theme::ThemeId>,
}

impl Args {
    /// Screenshots, benches and `--demo` never touch the service.
    pub fn offline(&self) -> bool {
        !self.live && (self.demo || self.shot.is_some() || self.bench.is_some())
    }
    /// Rendering into an image with no window: screenshots, and the offscreen bench.
    pub fn headless(&self) -> bool {
        self.shot.is_some() || (self.offscreen && self.bench.is_some())
    }
}

fn parse_args() -> Args {
    let mut a = Args { cmd: None, bench: None, shot: None, frames: 120, depth: false, orbit: false, apps: false, work: false, code: false, git: false, open_app: None, icon: false, aa: std::env::var("SPHERE_AA").unwrap_or_else(|_| "smaa".into()), offscreen: false, collapsed: false, focus_term: false, msaa: true, size: (1600, 1000), dt: 1.0 / 20.0, demo: false, live: false, moment: false, theme: None };
    let mut it = std::env::args().skip(1);
    while let Some(s) = it.next() {
        match s.as_str() {
            "--cmd" => a.cmd = it.next(),
            "--bench" => a.bench = it.next().and_then(|v| v.parse().ok()),
            "--shot" => a.shot = it.next(),
            "--frames" => a.frames = it.next().and_then(|v| v.parse().ok()).unwrap_or(120),
            "--depth" => a.depth = true,
            "--orbit" => a.orbit = true,
            "--apps" => a.apps = true,
            "--work" => a.work = true,
            "--code" => a.code = true,
            "--git" => a.git = true,
            "--app" => a.open_app = it.next(),
            "--icon" => a.icon = true,
            "--offscreen" => a.offscreen = true,
            "--aa" => a.aa = it.next().unwrap_or_else(|| "smaa".into()),
            "--collapsed" => a.collapsed = true,
            "--term" => a.focus_term = true,
            "--no-msaa" => {
                a.msaa = false;
                a.aa = "none".into();
            }
            "--demo" => a.demo = true,
            "--live" => a.live = true,
            "--moment" => a.moment = true,
            "--theme" => a.theme = it.next().and_then(|v| theme::ThemeId::from_key(&v)),
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
                println!("sphere-app [--demo] [--live] [--moment] [--theme name] [--cmd \"<command>\"] [--bench <seconds>] [--shot <png> [--frames N] [--depth] [--term]] [--size WxH] [--no-msaa]");
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
    stats::mark_start();
    let args = parse_args();
    interactive_qos();
    let mut app = App::new();
    app.insert_resource(args.clone())
        .insert_resource(ClearColor(Color::BLACK))
        .add_plugins(default_plugins(&args))
        .add_plugins((glow::GlowPlugin, panel::PanelPlugin))
        .add_systems(PreStartup, (setup_offscreen, panel::setup_atlas, no_gpu_clustering))
        .add_systems(PostStartup, panel::finish_atlas)
        .insert_resource(Sim { dt: 1.0 / 60.0, t: 0.0, boot_t: 0.0, fixed_dt: args.shot.as_ref().map(|_| args.dt), frozen: false })
        .insert_resource(CoreCtl::new(true))
        .insert_resource(StageView::compute(args.size.0 as f32, args.size.1 as f32))
        .insert_resource(if args.offline() { Agents::demo() } else { Agents::empty() })
        .insert_resource(service::start(args.offline()))
        .insert_resource(sysstats::start(args.offline()))
        .insert_resource(theme::Theme::new(args.theme.unwrap_or_else(|| if args.offline() { theme::ThemeId::Balanced } else { theme::saved() })))
        .init_resource::<live::Live>()
        .init_resource::<live::Toasts>()
        .init_resource::<cmdbar::CmdBar>()
        .init_resource::<moment::Moment>()
        .init_resource::<home::Acts>()
        .init_resource::<home::View>()
        .init_resource::<fx::Fx>()
        .init_resource::<mouse::MouseState>()
        .init_resource::<apps::Open>()
        .init_resource::<codeapp::CodeApp>()
        .init_resource::<gitapp::GitApp>()
        .init_resource::<containers::ContainersApp>()
        .init_resource::<ports::PortsApp>()
        .init_resource::<sysapp::SysApp>()
        .init_resource::<filesapp::FilesApp>()
        .init_resource::<appframe::PanelClicks>()
        .init_resource::<appframe::AppClicks>()
        .init_resource::<working::Working>()
        .init_resource::<momentfx::MomentFx>()
        .init_resource::<hud::FrameStats>()
        .add_systems(Startup, (sound::setup, setup_core, setup_cameras, hud::setup_panels, hud::setup_backdrop, hud::setup_overlay, cmdbar::setup, visor::setup, fx::setup, homepanels::setup, orbit::setup, working::setup, (codeapp::setup, gitapp::setup, containers::setup, ports::setup, sysapp::setup, filesapp::setup).chain(), momentfx::setup, callouts::setup, mouse::setup, mouse::setup_reticle).chain())
        .add_systems(First, (tick_sim, theme::animate))
        .add_systems(
            Update,
            (
                (keys, stage_view, camera_rig, update_core, spin_system, update_materials, panel::update_panels, hud::update_readout, hud::update_backdrop)
                    .chain(),
                (live::sync, live::transitions, moment::update, summon::notify, live::tick_toasts, panel::rebuild_agents_ui, hud::update_readout_text, hud::update_clock, panel::blink_system, homepanels::update, homepanels::draw_rolodex, homepanels::draw_say).chain(),
                (term::pump, cmdbar::keys, moment::keys, term::read_keys, home::keys, orbit::keys, working::keys, (codeapp::keys, gitapp::keys, containers::keys, ports::keys, sysapp::keys, filesapp::keys).chain(), momentfx::keys, visor::buttons, mouse::update, appframe::test_click, appframe::route_clicks, home::apply, apps::apply).chain(),
                (working::update, working::place_panels, working::draw, (codeapp::update, codeapp::place, codeapp::draw, gitapp::update, gitapp::place, gitapp::draw, containers::update, containers::place, containers::draw, ports::update, ports::place, ports::draw, sysapp::update, sysapp::place, sysapp::draw, filesapp::update, filesapp::place, filesapp::draw).chain(), momentfx::flows, momentfx::draw, momentfx::draw_sats, callouts::update).chain(),
                (orbit::update, mouse::draw_reticle, visor::draw_buttons, visor::ticker, visor::toasts, home::landed_sound, home::step_view, fx::update, term::draw, cmdbar::draw, moment::draw).chain(),
                (hud::frame_stats, hud::bench, hud::shot).chain(),
            )
                .chain(),
        )
        .add_systems(Last, panel::gate_panel_cameras)
        .add_systems(PostUpdate, (panel::theme_panels, theme_bloom, sound::flush));
    if !args.offline() {
        summon::install(&mut app);
    }
    if args.headless() {
        app.add_plugins(ScheduleRunnerPlugin::run_loop(std::time::Duration::from_millis(1)));
        // Offscreen there is no swap chain to throttle submission, and a software rasteriser would let queued
        // frames pile up in memory. Wait for the GPU after each frame instead.
        if let Some(r) = app.get_sub_app_mut(bevy::render::RenderApp) {
            r.add_systems(bevy::render::Render, wait_for_gpu.after(bevy::render::RenderSystems::Render));
        }
    }
    app.add_systems(Last, stats::report);
    if !args.headless() {
        app.insert_resource(summon::visible_settings()).add_systems(Last, summon::residency.after(panel::gate_panel_cameras));
    }
    if let Some(r) = app.get_sub_app_mut(bevy::render::RenderApp) {
        r.add_systems(bevy::render::Render, render_thread_qos);
    }
    app.run();
}

/// `--shot` renders offscreen into an image: no window, no display server, no surface needed.
fn default_plugins(args: &Args) -> impl PluginGroup {
    if args.headless() {
        DefaultPlugins
            .set(task_pools())
            .set(WindowPlugin { primary_window: None, exit_condition: ExitCondition::DontExit, ..default() })
            .disable::<WinitPlugin>()
    } else {
        DefaultPlugins.set(task_pools()).set(WindowPlugin {
            primary_window: Some(Window {
                title: "Sphere".into(),
                resolution: (args.size.0, args.size.1).into(),
                present_mode: if args.bench.is_some() { PresentMode::AutoNoVsync } else { PresentMode::AutoVsync },
                desired_maximum_frame_latency: std::env::var("SPHERE_LAT").ok().and_then(|v| v.parse().ok()).and_then(std::num::NonZeroU32::new),
                mode: if std::env::var("SPHERE_FS").is_ok() { bevy::window::WindowMode::BorderlessFullscreen(bevy::window::MonitorSelection::Primary) } else { bevy::window::WindowMode::Windowed },
                ..default()
            }),
            close_when_requested: false,
            ..default()
        })
    }
}

/// Worker threads run at user-interactive QoS, so macOS keeps them on performance cores and doesn't park them
/// mid-frame (at the default QoS frames hitched by ~14 ms a few times a second).
fn task_pools() -> TaskPoolPlugin {
    use bevy::app::TaskPoolThreadAssignmentPolicy as P;
    let hook: Option<std::sync::Arc<dyn Fn() + Send + Sync>> = Some(std::sync::Arc::new(interactive_qos));
    let pol = |min: usize, max: usize, percent: f32| P { min_threads: min, max_threads: max, percent, on_thread_spawn: hook.clone(), on_thread_destroy: None };
    TaskPoolPlugin {
        task_pool_options: TaskPoolOptions {
            io: pol(1, 4, 0.25),
            async_compute: pol(1, 4, 0.25),
            compute: pol(1, usize::MAX, 1.0),
            ..default()
        },
    }
}

/// Marks the calling thread user-interactive (macOS); a no-op elsewhere.
pub fn interactive_qos() {
    #[cfg(target_os = "macos")]
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0);
    }
}

/// We have no lights, so the GPU light clustering Bevy runs for every 3D view (compute passes, buffers, bind
/// groups) is pure cost; the CPU path with nothing to cluster is free.
fn no_gpu_clustering(mut s: ResMut<bevy::light::cluster::GlobalClusterSettings>) {
    s.gpu_clustering = None;
}

/// The render thread too: this runs in the render world, on that thread, and marks it once.
fn render_thread_qos(_w: &mut World, mut done: Local<bool>) {
    if !*done {
        *done = true;
        interactive_qos();
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
    if !args.headless() {
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
    let msaa_on = args.msaa && args.aa.starts_with("msaa");
    match (msaa_on, cam) {
        (true, WindowCam::Core) => (if args.aa == "msaa2" { Msaa::Sample2 } else { Msaa::Sample4 }, ClearColorConfig::Custom(Color::BLACK), CameraOutputMode::default()),
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
    let core = commands.spawn((
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
    )).id();
    // post-process AA instead of MSAA (cheaper at Retina full screen)
    match args.aa.as_str() {
        "smaa" => {
            commands.entity(core).insert(bevy::anti_alias::smaa::Smaa { preset: bevy::anti_alias::smaa::SmaaPreset::High });
        }
        "fxaa" => {
            commands.entity(core).insert(bevy::anti_alias::fxaa::Fxaa::default());
        }
        _ => {}
    }
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

/// Bloom follows the theme (the prototype's bloom strength .6 is our 0.22).
fn theme_bloom(theme: Res<theme::Theme>, ctl: Res<CoreCtl>, mut q: Query<&mut bevy::post_process::bloom::Bloom, With<CamCore>>) {
    if !theme.is_changed() && !ctl.is_changed() {
        return;
    }
    // core open dims the bloom by a third
    let i = 0.22 * (theme::bloom() + 0.5 * ctl.flare) / 0.6 * (1.0 - 0.32 * ctl.orb);
    for mut b in &mut q {
        if (b.intensity - i).abs() > 1e-4 {
            b.intensity = i;
        }
    }
}

#[allow(clippy::type_complexity)]
fn camera_rig(sv: Res<StageView>, ctl: Res<CoreCtl>, view: Res<home::View>, mut cams: Query<&mut Transform, Or<(With<CamCore>, With<CamPanels>)>>) {
    for mut t in &mut cams {
        t.translation = Vec3::new(view.px.x, sv.base_y + view.py.x, sv.d * ctl.zoom.x);
    }
}

fn keys(mut ctl: ResMut<CoreCtl>, args: Res<Args>, mut once: Local<bool>, win: Query<&Window, With<PrimaryWindow>>) {
    if !*once {
        *once = true;
        if args.depth {
            ctl.set_mode(Mode::Depth);
        }
        if args.orbit {
            ctl.set_mode(Mode::Orbit);
        }
        if args.collapsed {
            ctl.collapsed = true;
            ctl.collapse_t = 0.0;
        }
    }
    if let Ok(w) = win.single() {
        ctl.pointer = w.cursor_position().map(|p| Vec2::new(p.x / w.width(), p.y / w.height()));
    }
}
