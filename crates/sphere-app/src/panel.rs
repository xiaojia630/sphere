//! Glass panels: Bevy UI rendered into an `Image` by its own UI camera, shown on a 3D quad whose shader
//! adds the smoked glass, the thin chamfered frame, corner ticks, LED and bar.
use crate::core::*;
use crate::fonts::UiFonts;
use crate::util::*;
use bevy::text::{LetterSpacing, LineHeight};
use bevy::{
    asset::{embedded_asset, RenderAssetUsages},
    camera::{visibility::RenderLayers, ClearColorConfig, ImageRenderTarget, RenderTarget},
    mesh::{MeshVertexBufferLayoutRef, PrimitiveTopology},
    pbr::{MaterialPipeline, MaterialPipelineKey},
    prelude::*,
    reflect::TypePath,
    render::render_resource::{
        AsBindGroup, BlendComponent, BlendFactor, BlendOperation, BlendState, CompareFunction, Extent3d,
        RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError, TextureDimension, TextureFormat,
    },
    shader::ShaderRef,
};

// ------------------------------------------------------------------ material

#[derive(Clone, Copy, Debug, ShaderType)]
pub struct PanelUniform {
    pub size: Vec4,
    pub line: Vec4,
    pub hot: Vec4,
    pub fill: Vec4,
    pub acc: Vec4,
    pub p: Vec4,
    /// theme swap (see `theme::panel_map`)
    pub from_c: [Vec4; 8],
    pub to_c: [Vec4; 8],
    /// x: how much of the outline is drawn (0..1)
    pub q: Vec4,
    /// where the panel's UI sits in the atlas: uv offset (xy) and size (zw); texture px until `finish_atlas`
    pub r: Vec4,
}

#[derive(Asset, TypePath, AsBindGroup, Clone, Debug)]
pub struct PanelMaterial {
    #[uniform(0)]
    pub u: PanelUniform,
    #[texture(1)]
    #[sampler(2)]
    pub content: Option<Handle<Image>>,
}

impl Material for PanelMaterial {
    fn vertex_shader() -> ShaderRef {
        "embedded://sphere_app/shaders/panel.wesl".into()
    }
    fn fragment_shader() -> ShaderRef {
        "embedded://sphere_app/shaders/panel.wesl".into()
    }
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Premultiplied
    }
    fn enable_prepass() -> bool {
        false
    }
    fn enable_shadows() -> bool {
        false
    }
    fn specialize(
        _pipeline: &MaterialPipeline,
        d: &mut RenderPipelineDescriptor,
        layout: &MeshVertexBufferLayoutRef,
        _key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        let vl = layout.0.get_layout(&[Mesh::ATTRIBUTE_POSITION.at_shader_location(0), Mesh::ATTRIBUTE_UV_0.at_shader_location(1)])?;
        d.vertex.buffers = vec![vl];
        d.vertex.entry_point = Some("vs_panel".into());
        let frag = d.fragment.as_mut().unwrap();
        frag.entry_point = Some("fs_panel".into());
        frag.targets[0].as_mut().unwrap().blend = Some(BlendState {
            color: BlendComponent { src_factor: BlendFactor::One, dst_factor: BlendFactor::OneMinusSrcAlpha, operation: BlendOperation::Add },
            alpha: BlendComponent { src_factor: BlendFactor::One, dst_factor: BlendFactor::OneMinusSrcAlpha, operation: BlendOperation::Add },
        });
        d.primitive.cull_mode = None;
        if let Some(ds) = d.depth_stencil.as_mut() {
            ds.depth_write_enabled = Some(false);
            ds.depth_compare = Some(CompareFunction::Always);
        }
        Ok(())
    }
}

pub struct PanelPlugin;
impl Plugin for PanelPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "shaders/panel.wesl");
        app.add_plugins(MaterialPlugin::<PanelMaterial>::default());
    }
}

// ------------------------------------------------------------------ panels

impl PanelId {
    /// Full color: the hue this panel blends toward.
    pub fn hue(self) -> Option<crate::theme::Hue> {
        use crate::theme::Hue;
        match self {
            PanelId::Agents => Some(Hue::Agents),
            PanelId::Clock => Some(Hue::Clock),
            PanelId::Readout | PanelId::Moment | PanelId::Say | PanelId::Sat => Some(Hue::Neutral),
            PanelId::System => Some(Hue::System),
            PanelId::Apps => Some(Hue::Apps),
            PanelId::Tele => Some(Hue::Tele),
            PanelId::Rail => Some(Hue::Rail),
            PanelId::Work | PanelId::Chip | PanelId::App | PanelId::AppBare | PanelId::AppFrame => Some(Hue::Code),
            PanelId::Terminal | PanelId::Brand | PanelId::Tile => None,
        }
    }
}

/// The panel's Full-color hue, on its quad (the readout has no `Panel`).
#[derive(Component, Clone, Copy)]
pub struct PanelHue(pub Option<crate::theme::Hue>);

/// Re-colours every panel's frame, glass and UI swap when the palette moves.
pub fn theme_panels(theme: Res<crate::theme::Theme>, mut seen: Local<u64>, mut mats: ResMut<Assets<PanelMaterial>>, q: Query<(&MeshMaterial3d<PanelMaterial>, &PanelHue)>) {
    if *seen == theme.version {
        return;
    }
    *seen = theme.version;
    for (h, hue) in &q {
        let (from, to) = crate::theme::panel_map(hue.0);
        if let Some(mut m) = mats.get_mut(&h.0) {
            let card = m.u.fill == m.u.acc;
            m.u.line = to[0];
            m.u.hot = to[2];
            m.u.acc = to[4];
            m.u.fill = if card { to[4] } else { to[3] };
            // the swap only runs when some role differs from Balanced
            let base = crate::theme::panel_map_base();
            m.u.q.y = if to.iter().zip(base.iter()).any(|(a, b)| (*a - *b).abs().max_element() > 1e-4) { 1.0 } else { 0.0 };
            m.u.from_c = from;
            m.u.to_c = to;
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PanelId {
    Agents,
    Terminal,
    Readout,
    Clock,
    Moment,
    Brand,
    System,
    Apps,
    Tele,
    Rail,
    Say,
    /// a core-open app tile
    Tile,
    /// the Working view's run, output and edit panels
    Work,
    /// a Working view step chip
    Chip,
    /// an Agent moment satellite (a waiting request)
    Sat,
    /// an app's pane (Code, ...)
    App,
    /// an app's bare text (its title bar, the label by the small core)
    AppBare,
    /// an app's workspace: the large glass the panes float over
    AppFrame,
}

#[derive(Component)]
pub struct Panel {
    pub id: PanelId,
    pub w: f32,
    pub h: f32,
    /// where you dragged it (x, y) and pushed it (z, scroll), on top of home
    pub user: Vec3,
    pub dragging: bool,
    /// tilt from drag velocity, rad
    pub tilt: Vec2,
    /// seconds since the frame started drawing in (boot, or the card rising)
    pub draw_t: f32,
    /// a Working view panel: (how open the view is, how far forward the output is)
    pub work: Option<(f32, f32)>,
    /// the Agent moment card: the last rise it played, and seconds since it rose
    pub rise_seen: u64,
    pub rise_t: f32,
    /// home position in world units (stage px mapped to world, z = 0 plane)
    pub home: Vec3,
    pub home_scale: f32,
    /// -1 right column (tilts toward the centre), +1 left column
    pub side: f32,
    pub px: Spring,
    pub py: Spring,
    pub pz: Spring,
    pub rx: Spring,
    pub ry: Spring,
    pub sc: Spring,
    pub op: Spring,
    pub phase: f32,
    pub show_at: f32,
    pub material: Handle<PanelMaterial>,
}

#[derive(Resource)]
#[allow(dead_code)]
pub struct PanelScale(pub f32);

pub fn stage_to_world(sx: f32, sy: f32) -> Vec3 {
    Vec3::new(sx - 800.0, 500.0 - sy, 0.0)
}

pub struct UiPanel {
    pub root: Entity,
    #[allow(dead_code)]
    pub camera: Entity,
    pub image: Handle<Image>,
    /// texture px in the atlas: x, y, w, h
    pub rect: Vec4,
}

/// Marks the camera that renders the panels' UI into the atlas. See `gate_panel_cameras`.
#[derive(Component)]
pub struct PanelUiCamera;

/// Every panel's UI is laid out side by side in one texture, drawn by one UI camera (rather than a camera and a
/// texture per panel). Rects are packed in shelves as panels spawn during Startup; `finish_atlas` then makes the
/// texture as tall as they need. It's a render target only: no CPU copy.
/// One atlas: a render-target texture and the UI camera that draws panels' UI into it, packed in shelves as panels
/// spawn during Startup. `finish` makes it as tall as its panels need. It's a render target only: no CPU copy.
pub struct Atlas {
    pub image: Handle<Image>,
    pub camera: Entity,
    pub scale: f32,
    /// texture width in px (`ATLAS_W`, or narrower for an app whose panels fit in less)
    pub w: u32,
    x: u32,
    y: u32,
    shelf: u32,
}

impl Atlas {
    pub fn new(commands: &mut Commands, images: &mut Assets<Image>, scale: f32, order: isize) -> Self {
        // a placeholder until `finish` knows the size (the camera needs its target to exist)
        let image = images.add(atlas_image(ATLAS_W, 1));
        let camera = commands
            .spawn((
                Camera2d,
                Camera { order, clear_color: ClearColorConfig::Custom(Color::NONE), ..default() },
                RenderTarget::Image(ImageRenderTarget { handle: image.clone(), scale_factor: scale }),
                Msaa::Off,
                PanelUiCamera,
                AtlasForce(0),
            ))
            .id();
        Atlas { image, camera, scale, w: ATLAS_W, x: 0, y: 0, shelf: 0 }
    }
    /// An atlas only `w` px wide (set before the panels spawn), for an app whose panels pack into less than the full width.
    pub fn with_width(mut self, w: u32) -> Self {
        self.w = w.min(ATLAS_W);
        self
    }
    pub fn height(&self) -> u32 {
        (self.y + self.shelf).max(1)
    }
    /// Points the panel materials drawn from this atlas at their rects (texture px until now).
    fn normalise(&self, mats: &mut Assets<PanelMaterial>) {
        let h = self.height() as f32;
        let size = Vec4::new(self.w as f32, h, self.w as f32, h);
        for (_, m) in mats.iter_mut() {
            if m.content.as_ref() == Some(&self.image) {
                m.u.r /= size;
            }
        }
    }
}

/// The home screen's panels (and the HUD's): always resident.
#[derive(Resource)]
pub struct PanelAtlas(pub Atlas);

impl std::ops::Deref for PanelAtlas {
    type Target = Atlas;
    fn deref(&self) -> &Atlas {
        &self.0
    }
}
impl std::ops::DerefMut for PanelAtlas {
    fn deref_mut(&mut self) -> &mut Atlas {
        &mut self.0
    }
}

/// Each app's own atlas. An app's texture only exists while the app is open (or folding away), so closed apps
/// cost no GPU memory; it's drawn at a higher scale than the home's, since app panes sit larger and closer.
#[derive(Resource, Default)]
pub struct AppAtlases {
    list: Vec<(u8, Atlas, bool, bool)>,
}

impl AppAtlases {
    pub fn add(&mut self, app: u8, atlas: Atlas) {
        self.list.push((app, atlas, false, false));
    }
    /// Asks for an app's texture (true while it's open or animating).
    pub fn want(&mut self, app: u8, want: bool) {
        if let Some(a) = self.list.iter_mut().find(|a| a.0 == app) {
            a.3 = want;
        }
    }
}

/// Forces an atlas camera to draw for this many more frames (its texture was just made, so it's empty).
#[derive(Component)]
pub struct AtlasForce(pub u8);

/// How much sharper than the screen an app's panes are drawn: they show about 1.1 × larger, and nearer.
pub const APP_SHARP: f32 = 1.3;

pub const ATLAS_W: u32 = 4096;
/// clear texels between panels, so sampling at a panel's edge never picks up its neighbour
const GUTTER: u32 = 2;

/// Texture px per stage px for the home's panels: the panel shows at about `k` on screen, so match the screen
/// pixels (a little over, since panels drift nearer the camera).
pub fn home_scale(args: &crate::Args, win: &Query<&Window, With<bevy::window::PrimaryWindow>>) -> f32 {
    let sv = StageView::compute(args.size.0 as f32, args.size.1 as f32);
    let dpr = win.single().map(|w| w.scale_factor()).unwrap_or(1.0);
    (sv.k * dpr * 1.1).max(crate::hud::SCALE_MIN)
}

pub fn setup_atlas(mut commands: Commands, mut images: ResMut<Assets<Image>>, args: Res<crate::Args>, win: Query<&Window, With<bevy::window::PrimaryWindow>>) {
    let scale = home_scale(&args, &win);
    commands.insert_resource(PanelScale(scale));
    let atlas = Atlas::new(&mut commands, &mut images, scale, -10);
    commands.insert_resource(PanelAtlas(atlas));
    commands.insert_resource(AppAtlases::default());
}

fn atlas_image(w: u32, h: u32) -> Image {
    let mut img = Image::new_uninit(Extent3d { width: w, height: h, depth_or_array_layers: 1 }, TextureDimension::D2, TextureFormat::Rgba8UnormSrgb, RenderAssetUsages::default());
    img.texture_descriptor.usage = bevy::render::render_resource::TextureUsages::TEXTURE_BINDING | bevy::render::render_resource::TextureUsages::RENDER_ATTACHMENT;
    img
}

/// Sizes the home atlas, and points every panel material at its rect in its atlas.
pub fn finish_atlas(atlas: Res<PanelAtlas>, apps: Res<AppAtlases>, mut images: ResMut<Assets<Image>>, mut mats: ResMut<Assets<PanelMaterial>>) {
    let h = atlas.height();
    let _ = images.insert(&atlas.image, atlas_image(atlas.w, h));
    atlas.normalise(&mut mats);
    for (_, a, _, _) in &apps.list {
        a.normalise(&mut mats);
    }
    if crate::stats::on() {
        eprintln!("stats: panel atlas {ATLAS_W}x{h} ({:.1} MB)", (ATLAS_W * h * 4) as f64 / 1e6);
        for (app, a, _, _) in &apps.list {
            eprintln!("stats: app {app} atlas {}x{} ({:.1} MB while open)", a.w, a.height(), (a.w * a.height() * 4) as f64 / 1e6);
        }
    }
}

/// Makes an app's texture when it opens and drops it once it has folded away.
pub fn app_atlas_residency(mut apps: ResMut<AppAtlases>, mut images: ResMut<Assets<Image>>, mut force: Query<&mut AtlasForce>) {
    for (_, a, alloc, want) in apps.list.iter_mut() {
        if *want != *alloc {
            *alloc = *want;
            let _ = images.insert(&a.image, atlas_image(a.w, if *want { a.height() } else { 1 }));
            if *want {
                if let Ok(mut f) = force.get_mut(a.camera) {
                    f.0 = 4;
                }
            }
        }
    }
}

/// A panel's UI root, placed in its own rect of the atlas (clipped to it).
pub fn spawn_ui_panel(commands: &mut Commands, atlas: &mut Atlas, w: f32, h: f32) -> UiPanel {
    let (pw, ph) = ((w * atlas.scale).ceil() as u32, (h * atlas.scale).ceil() as u32);
    if atlas.x + pw > atlas.w {
        atlas.x = 0;
        atlas.y += atlas.shelf;
        atlas.shelf = 0;
    }
    let (x, y) = (atlas.x, atlas.y);
    atlas.x += pw + GUTTER;
    atlas.shelf = atlas.shelf.max(ph + GUTTER);
    ui_panel_at(commands, atlas, w, h, x, y)
}

/// A panel's UI at a given texel position in its atlas (the caller packs; the atlas grows to hold it).
pub fn spawn_ui_panel_at(commands: &mut Commands, atlas: &mut Atlas, w: f32, h: f32, x: u32, y: u32) -> UiPanel {
    let ph = (h * atlas.scale).ceil() as u32;
    atlas.y = atlas.y.max(y + ph + GUTTER);
    atlas.shelf = 0;
    ui_panel_at(commands, atlas, w, h, x, y)
}

fn ui_panel_at(commands: &mut Commands, atlas: &Atlas, w: f32, h: f32, x: u32, y: u32) -> UiPanel {
    let root = commands
        .spawn((
            Node { width: px(w), height: px(h), position_type: PositionType::Absolute, left: px(x as f32 / atlas.scale), top: px(y as f32 / atlas.scale), overflow: Overflow::clip(), ..default() },
            UiTargetCamera(atlas.camera),
        ))
        .id();
    UiPanel { root, camera: atlas.camera, image: atlas.image.clone(), rect: Vec4::new(x as f32, y as f32, w * atlas.scale, h * atlas.scale) }
}

pub fn panel_mesh(w: f32, h: f32, pad: f32) -> Mesh {
    let (hw, hh) = (w / 2.0 + pad, h / 2.0 + pad);
    let mut m = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, vec![[-hw, -hh, 0.0], [hw, -hh, 0.0], [hw, hh, 0.0], [-hw, hh, 0.0]]);
    m.insert_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]]);
    m.insert_indices(bevy::mesh::Indices::U32(vec![0, 1, 2, 0, 2, 3]));
    m
}

fn lin(r: Role) -> Vec4 {
    role_lin(r)
}

pub const PAD: f32 = 24.0;

pub struct PanelSpec {
    pub id: PanelId,
    pub w: f32,
    pub h: f32,
    pub home: Vec3,
    pub home_scale: f32,
    pub side: f32,
    pub phase: f32,
    pub show_at: f32,
    pub unframed: bool,
}

/// Creates the quad + material for a panel whose UI was spawned with `spawn_ui_panel`.
pub fn spawn_panel_quad(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    mats: &mut Assets<PanelMaterial>,
    ui: &UiPanel,
    s: &PanelSpec,
) -> Entity {
    // The Agent moment card: darker glass (it sits over the core's flare) and an amber LED and bar.
    let card = s.id == PanelId::Moment;
    let (from_c, to_c) = crate::theme::panel_map(s.id.hue());
    let mat = mats.add(PanelMaterial {
        u: PanelUniform {
            size: Vec4::new(s.w, s.h, PAD, 0.0),
            line: lin(Role::Line),
            hot: lin(Role::Hot),
            fill: if card { lin(Role::Acc) } else { lin(Role::Fill) },
            acc: lin(Role::Acc),
            // 1 framed glass, 2 the core readout (dark backdrop), 3 bare text (the clock)
            p: Vec4::new(if matches!(s.id, PanelId::Clock | PanelId::Brand | PanelId::Apps | PanelId::Rail | PanelId::Say | PanelId::AppBare) { 3.0 } else if s.unframed { 2.0 } else { 1.0 }, if card { 0.86 } else { 0.36 }, 1.0, if matches!(s.id, PanelId::App | PanelId::AppBare) { 0.6 } else { 1.0 }),
            from_c,
            to_c,
            q: Vec4::new(0.0, 0.0, 0.0, 0.0),
            r: ui.rect,
        },
        content: Some(ui.image.clone()),
    });
    commands
        .spawn((
            Mesh3d(meshes.add(panel_mesh(s.w, s.h, PAD))),
            MeshMaterial3d(mat.clone()),
            Transform::from_translation(s.home).with_scale(Vec3::splat(0.001)),
            RenderLayers::layer(1),
            PanelHue(s.id.hue()),
            Panel {
                id: s.id,
                w: s.w,
                h: s.h,
                home: s.home,
                home_scale: s.home_scale,
                side: s.side,
                px: Spring::new(s.home.x, 120.0, 20.0),
                py: Spring::new(s.home.y, 120.0, 20.0),
                pz: Spring::new(-300.0, 60.0, 14.0),
                rx: Spring::new(0.0, 90.0, 15.0),
                ry: Spring::new(0.0, 90.0, 15.0),
                sc: Spring::new(1.0, 120.0, 19.0),
                op: Spring::new(0.0, 50.0, 13.0),
                phase: s.phase,
                show_at: s.show_at,
                material: mat,
                user: Vec3::ZERO,
                dragging: false,
                tilt: Vec2::ZERO,
                draw_t: 0.0,
                work: None,
                rise_seen: 0,
                rise_t: 9.0,
            },
        ))
        .id()
}

#[derive(Resource, Default)]
pub struct Focus {
    pub terminal: bool,
    /// The command bar has the keyboard.
    pub cmd: bool,
}

/// Moves each panel: gentle drift and tilt in the flat view, the prototype's side tilt in Depth, and the
/// "focus" fly-in for the terminal.
#[allow(clippy::too_many_arguments)]
pub fn update_panels(
    sim: Res<Sim>,
    mut ctl: ResMut<CoreCtl>,
    focus: Res<Focus>,
    moment: Res<crate::moment::Moment>,
    (swarm, live, scene): (Res<crate::swarm::Swarm>, Res<crate::live::Live>, Res<crate::swarm::scene::SceneOut>),
    args: Res<crate::Args>,
    mut mats: ResMut<Assets<PanelMaterial>>,
    mut q: Query<(&mut Panel, &mut Transform, &mut Visibility), Without<ReadoutLink>>,
) {
    let (dt, t) = (sim.dt, sim.t);
    let icon = args.icon;
    // the core fades back for the focused terminal, and partly for the Agent moment card so its text reads
    let ft = if focus.terminal { 1.0 } else if moment.shown { 0.5 } else { 0.0 };
    ctl.focus_amt += (ft - ctl.focus_amt) * (1.0 - (-6.0 * dt).exp());
    for (mut p, mut tr, mut vis) in &mut q {
        let depth = ctl.mode == Mode::Depth;
        let shown = sim.boot_t > p.show_at;
        let mut tx = p.home.x;
        let mut ty = p.home.y;
        let mut tz = p.home.z;
        let mut top = if shown { 1.0 } else { 0.0 };
        let mut tsc = p.home_scale;
        let mut trx = 0.0;
        let mut try_ = 0.0;
        // gentle drift, and a slight base tilt toward the core
        // SPHERE_STILL=1 holds panels still (recordings for GIFs, where drift costs megabytes)
        static STILL: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let still = *STILL.get_or_init(|| std::env::var_os("SPHERE_STILL").is_some());
        let drift = if still { 0.0 } else if sim.fixed_dt.is_some() { 0.6 } else { 1.0 };
        tx += (t * 0.31 + p.phase).sin() * 6.0 * drift;
        ty += (t * 0.43 + p.phase * 1.7).sin() * 5.0 * drift;
        tz += (t * 0.27 + p.phase * 0.6).sin() * 8.0 * drift;
        trx += (t * 0.37 + p.phase).sin() * 1.2 * DEG + 2.0 * DEG;
        try_ += p.side * (6.0 + (t * 0.29 + p.phase).sin() * 1.5) * DEG;
        if depth {
            try_ = p.side * 18.0 * DEG;
            tz -= 50.0;
            if p.side < 0.0 {
                tx += 110.0;
                tz -= 60.0;
                top *= 0.3;
            }
        }
        if ctl.mode == Mode::Orbit {
            tz -= 110.0;
            top *= 0.5;
        }
        // the Working view: its own panels come in; the home's draw back (the wordmark stays, Say moves under the core)
        if let Some((amt, big)) = p.work {
            top = amt;
            tz -= (1.0 - amt) * 260.0;
            tsc *= lerp(0.92, 1.0, amt);
            if big > 0.0 {
                tx = lerp(tx, 0.0, big);
                ty = lerp(ty, 20.0, big);
                tz = lerp(tz, 230.0, big);
                tsc *= lerp(1.0, 1.57, big);
                trx *= 1.0 - big;
                try_ *= 1.0 - big;
            }
        } else if ctl.work.max(ctl.app).max(if matches!(p.id, PanelId::Say | PanelId::Terminal) { 0.0 } else { ctl.swarm }) > 0.0 && p.id != PanelId::Brand && p.id != PanelId::Moment {
            // the Working view, an app or the Agent swarm: the home draws back (the swarm keeps Say and the brand)
            let wk = ctl.work.max(ctl.app);
            if p.id == PanelId::Say {
                tx -= 170.0 * wk;
                top *= 1.0 - ctl.app;
            } else {
                let sw = if p.id == PanelId::Terminal { 0.0 } else { ctl.swarm };
                let wk_only = wk;
                let wk = wk.max(sw);
                tx -= p.side * 200.0 * wk;
                tz -= 300.0 * wk;
                // the swarm shows none of the home's panels (not even as ghosts)
                top *= (1.0 - 0.93 * wk_only) * (1.0 - sw);
            }
        }
        // the swarm moves the wordmark to the top left (stage x 54, y 26: its panel is 560 x 52, the text left aligned)
        if p.id == PanelId::Brand && ctl.swarm > 0.0 {
            tx = lerp(tx, 54.0 + p.w / 2.0 - 800.0, ctl.swarm);
            ty = lerp(ty, 500.0 - (26.0 + p.h / 2.0), ctl.swarm);
        }
        // core open: the side panels step aside and back
        if ctl.orb > 0.0 && !matches!(p.id, PanelId::Brand | PanelId::Say | PanelId::Moment | PanelId::Terminal) {
            tx -= p.side * 230.0 * ctl.orb;
            tz -= 320.0 * ctl.orb;
            top *= 1.0 - 0.9 * ctl.orb;
        }
        let focused_me = p.id == PanelId::Terminal && focus.terminal;
        if p.id == PanelId::Terminal && !focus.terminal {
            // the terminal isn't on the home: T (or its app) brings it forward
            top = 0.0;
            tz -= 120.0;
        }
        if p.id == PanelId::Moment {
            // the card rises out of the core: it starts small, transparent and tipped at the core, on loose springs
            if moment.rise_seq != p.rise_seen {
                p.rise_seen = moment.rise_seq;
                p.rise_t = 0.0;
                p.sc.set(0.08);
                p.op.set(0.0);
                p.rx.set(-50.0 * DEG);
                // from the core, or from the satellite that asked (the swarm sets the origin)
                let o = crate::swarm::input::rise_origin(&swarm, &moment, &live, &scene).unwrap_or(Vec3::new(0.0, CORE_Y, -140.0));
                p.px.set(o.x);
                p.py.set(o.y);
                p.pz.set(o.z);
            }
            p.rise_t += crate::motion::dt(crate::motion::Pace::Big, dt);
            let loose = p.rise_t < 1.5;
            (p.sc.k, p.sc.c) = if loose { (105.0, 12.5) } else { (120.0, 19.0) };
            (p.pz.k, p.pz.c) = if loose { (88.0, 13.0) } else { (60.0, 14.0) };
            (p.op.k, p.op.c) = if loose { (80.0, 15.0) } else { (50.0, 13.0) };
            (p.rx.k, p.rx.c) = if loose { (80.0, 12.5) } else { (90.0, 15.0) };
            let up = (moment.shown || moment.holding.is_some()) && !focus.terminal;
            top = if up { 1.0 } else { 0.0 };
            // at rest it breathes
            tx = p.home.x;
            ty = p.home.y + 2.4 * (0.9 * t).sin();
            tz = p.home.z + 3.5 * (0.7 * t + 1.0).sin() + if up { 0.0 } else { -120.0 };
            trx = 0.5 * DEG * (0.6 * t).sin();
            try_ = 0.0;
            // Later dims it into orbit: faint, then small and gone
            if let Some(lt) = moment.later {
                top = if lt < 0.2 { 0.26 } else { 0.0 };
                tsc *= if lt < 0.2 { 1.0 } else { 0.09 };
            }
        } else if (moment.shown || moment.holding.is_some()) && !focus.terminal && !matches!(p.id, PanelId::Brand | PanelId::Say) {
            // the HUD draws back while the card is up
            let a = ctl.moment;
            tx -= p.side * 200.0 * a;
            tz -= 300.0 * a;
            top *= 1.0 - 0.93 * a;
        }
        if focused_me {
            tx = 0.0;
            ty = CORE_Y;
            tz = 260.0;
            tsc = p.home_scale * 1.15;
            trx = 0.0;
            try_ = 0.0;
            top = 1.0;
        } else if focus.terminal {
            top *= 0.26;
            tz -= 70.0;
        }
        if !focused_me && p.id != PanelId::Moment {
            tx += p.user.x;
            ty += p.user.y;
            tz += p.user.z;
            trx += p.tilt.y;
            try_ += p.tilt.x;
        }
        p.tilt *= 0.9;
        // while dragging the panel follows the pointer tightly
        let (k, c) = if p.dragging { (420.0, 40.0) } else { (120.0, 20.0) };
        p.px.k = k;
        p.px.c = c;
        p.py.k = k;
        p.py.c = c;
        p.px.t = tx;
        p.py.t = ty;
        p.pz.t = tz;
        p.rx.t = trx;
        p.ry.t = try_;
        p.sc.t = tsc;
        p.op.t = if icon { 0.0 } else { top };
        // an app's panes and the moment card move with the big transitions, the rest of the HUD on the UI clock
        let pace = if p.work.is_some() || p.id == PanelId::Moment { crate::motion::Pace::Big } else { crate::motion::Pace::Ui };
        let sdt = crate::motion::dt(pace, dt);
        p.px.step(sdt);
        p.py.step(sdt);
        p.pz.step(sdt);
        p.rx.step(sdt);
        p.ry.step(sdt);
        p.sc.step(sdt);
        p.op.step(sdt);
        tr.translation = Vec3::new(p.px.x, p.py.x, p.pz.x);
        tr.rotation = Quat::from_euler(EulerRot::XYZ, p.rx.x, p.ry.x, 0.0);
        tr.scale = Vec3::splat(p.sc.x.max(0.001));
        // Send folds the card to a line: height 1 → .014 (at 70 %) → .004 over .35 s from .55 s, then it's gone
        if p.id == PanelId::Moment {
            if let Some(ft) = moment.fold {
                let k = clamp((ft - 0.55) / 0.35, 0.0, 1.0);
                let e = bezier(0.6, 0.02, 0.72, 0.46, k);
                let fy = if e < 0.7 { lerp(1.0, 0.014, e / 0.7) } else { lerp(0.014, 0.004, (e - 0.7) / 0.3) };
                tr.scale.y *= fy;
                if ft > 0.9 {
                    p.op.set(0.0);
                }
            }
        }
        let op = clamp(p.op.x, 0.0, 1.0);
        *vis = if op > 0.004 { Visibility::Inherited } else { Visibility::Hidden };
        // the frame draws in over 1.15 s, cubic-bezier(.65,.05,.25,1), once the panel shows (and each time the card rises)
        if p.id == PanelId::Moment && !moment.shown {
            p.draw_t = 0.0;
        } else if shown {
            p.draw_t += sdt;
        }
        let draw = if sim.frozen { 1.0 } else { bezier(0.65, 0.05, 0.25, 1.0, p.draw_t / 1.15) };
        if mats.get(&p.material).is_some_and(|m| m.u.size.w != op || m.u.q.x != draw) {
            if let Some(mut m) = mats.get_mut(&p.material) {
                m.u.size.w = op;
                m.u.q.x = draw;
            }
        }
    }
}

/// Marks the core readout, which rides on the core's own transform instead of the panel springs.
#[derive(Component)]
pub struct ReadoutLink;

// ------------------------------------------------------------------ text helpers

pub fn font(h: &Handle<Font>, size: f32, spacing_px: f32) -> (TextFont, LetterSpacing) {
    (TextFont { font: FontSource::Handle(h.clone()), font_size: FontSize::Px(size), ..default() }, LetterSpacing::Px(spacing_px))
}

pub fn with_alpha(c: Color, a: f32) -> Color {
    let s = c.to_srgba();
    Color::srgba(s.red, s.green, s.blue, s.alpha * a)
}

#[derive(Component)]
pub struct AgentsRoot;
#[derive(Component)]
pub struct Blink;

pub fn build_agents_ui(commands: &mut Commands, root: Entity, f: &UiFonts, agents: &Agents, outside: &crate::takeover::Outside, sim_t: f32) {
    let line = role_color(Role::Line);
    let hot = role_color(Role::Hot);
    let dim = role_color(Role::Dim);
    let fill = role_color(Role::Fill);
    let acc = role_color(Role::Acc);
    let ok = role_color(Role::Ok);
    let bad = role_color(Role::Bad);
    let g1 = with_alpha(line, 0.72);
    let _ = sim_t;
    let body = commands
        .spawn((
            AgentsRoot,
            Node {
                width: percent(100),
                height: percent(100),
                flex_direction: FlexDirection::Column,
                padding: UiRect { left: px(18), right: px(18), top: px(14), bottom: px(16) },
                row_gap: px(4),
                ..default()
            },
        ))
        .id();
    commands.entity(root).add_child(body);
    // header
    let hdr = commands
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
            BorderColor::all(g1),
        ))
        .id();
    commands.entity(body).add_child(hdr);
    let sq = commands.spawn((Node { width: px(6), height: px(6), ..default() }, BackgroundColor(fill))).id();
    let t1 = commands
        .spawn((Text::new("AGENTS"), font(&f.ui600, 12.0, 3.84), TextColor(hot), LineHeight::Px(14.4)))
        .id();
    let t2 = commands
        .spawn((
            Text::new(agents.summary().to_uppercase()),
            font(&f.ui500, 12.0, 2.16),
            TextColor(dim),
            LineHeight::Px(14.4),
            Node { margin: UiRect { left: auto(), ..default() }, ..default() },
        ))
        .id();
    commands.entity(hdr).add_children(&[sq, t1, t2]);
    // rows
    let list = commands.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(2), ..default() }).id();
    commands.entity(body).add_child(list);
    if agents.list.is_empty() {
        let e = commands
            .spawn((
                Text::new("No agents running. Type a task in the bar below."),
                font(&f.ui500, 13.0, 1.04),
                TextColor(with_alpha(line, 0.7)),
                LineHeight::Px(16.9),
                Node { margin: UiRect { top: px(8), ..default() }, ..default() },
            ))
            .id();
        commands.entity(list).add_child(e);
    }
    let (ew_rows, ew_more) = elsewhere_rows_with(outside.list.len(), agents.list.len());
    let (rows, more) = agent_rows_in(agents.list.len(), elsewhere_height(ew_rows, ew_more));
    for a in agents.list.iter().take(rows) {
        let (name_c, op) = match a.state {
            AState::Needs => (acc, 1.0),
            AState::Done | AState::Failed => (hot, 0.6),
            _ => (hot, 1.0),
        };
        let row = commands
            .spawn(Node {
                flex_direction: FlexDirection::Column,
                row_gap: px(4),
                padding: UiRect { left: px(8), right: px(8), top: px(7), bottom: px(8) },
                margin: UiRect::horizontal(px(-8)),
                ..default()
            })
            .id();
        commands.entity(list).add_child(row);
        // top: name + chip
        let top = commands
            .spawn(Node { flex_direction: FlexDirection::Row, justify_content: JustifyContent::SpaceBetween, align_items: AlignItems::Center, column_gap: px(10), ..default() })
            .id();
        let name = commands
            .spawn((Text::new(a.name.to_uppercase()), font(&f.ui600, 13.5, 1.89), TextColor(with_alpha(name_c, op)), LineHeight::Px(17.55)))
            .id();
        let (chip_txt, chip_fg, chip_bg, chip_border) = match a.state {
            AState::Working => ("WORKING", line, Color::NONE, g1),
            AState::Needs => ("NEEDS YOU", Color::BLACK, acc, acc),
            AState::Done => ("DONE", ok, Color::NONE, with_alpha(ok, 0.7)),
            AState::Failed => (a.chip.unwrap_or("FAILED"), bad, Color::NONE, with_alpha(bad, 0.7)),
        };
        let chip = commands
            .spawn((
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: px(6),
                    padding: UiRect { left: px(6), right: px(6), top: px(4), bottom: px(3) },
                    border: UiRect::all(px(1)),
                    ..default()
                },
                BackgroundColor(with_alpha(chip_bg, op)),
                BorderColor::all(with_alpha(chip_border, op)),
            ))
            .id();
        if a.state == AState::Working {
            let led = commands.spawn((Node { width: px(5), height: px(5), ..default() }, BackgroundColor(fill), Blink)).id();
            commands.entity(chip).add_child(led);
        }
        let ct = commands.spawn((Text::new(chip_txt), font(&f.ui700, 10.0, 2.0), TextColor(with_alpha(chip_fg, op)), LineHeight::Px(10.0))).id();
        commands.entity(chip).add_child(ct);
        commands.entity(top).add_children(&[name, chip]);
        // bar: 18 segments + percent
        let bar = commands
            .spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(10), height: px(12), ..default() })
            .id();
        let track = commands.spawn(Node { flex_grow: 1.0, height: px(5), flex_direction: FlexDirection::Row, column_gap: px(2), ..default() }).id();
        let frac = if a.state == AState::Working { a.p } else { 1.0 };
        let on = (frac * 18.0).round() as usize;
        let on_c = if a.state == AState::Done { ok } else { fill };
        for i in 0..18 {
            let c = if i < on { with_alpha(on_c, op) } else { with_alpha(line, 0.13 * op) };
            let seg = commands.spawn((Node { flex_grow: 1.0, height: px(5), ..default() }, BackgroundColor(c))).id();
            commands.entity(track).add_child(seg);
        }
        let pct_txt = match (&a.figure, a.state) {
            (Some(f), _) => f.clone(),
            (None, AState::Working) => format!("{}%", (a.p * 100.0).round() as i32),
            (None, AState::Needs) => "WAIT".to_string(),
            (None, AState::Done) => "OK".to_string(),
            (None, AState::Failed) => "--".to_string(),
        };
        let pct_c = if a.state == AState::Done { ok } else { hot };
        let pct = commands
            .spawn((
                Node { min_width: px(36), justify_content: JustifyContent::FlexEnd, ..default() },
            ))
            .id();
        let pt = commands.spawn((Text::new(pct_txt), font(&f.mono, 11.5, 2.24), TextColor(with_alpha(pct_c, op)), LineHeight::Px(11.5))).id();
        commands.entity(pct).add_child(pt);
        commands.entity(bar).add_children(&[track, pct]);
        let step = commands
            .spawn((Text::new(a.step.clone()), font(&f.ui500, 13.0, 1.04), TextColor(with_alpha(line, op)), LineHeight::Px(16.9)))
            .id();
        commands.entity(row).add_children(&[top, bar, step]);
    }
    if more > 0 {
        let m = commands
            .spawn((
                Text::new(format!("+{more} MORE")),
                font(&f.ui600, 11.0, 2.2),
                TextColor(dim),
                LineHeight::Px(14.0),
                Node { margin: UiRect { top: px(10), ..default() }, ..default() },
            ))
            .id();
        commands.entity(list).add_child(m);
    }
    if ew_rows > 0 {
        build_elsewhere(commands, list, f, outside, ew_rows, ew_more);
    }
}

/// Room in the Agents panel for rows (its height less the padding and the header), the height of one agent's row,
/// and of the "elsewhere" part: its heading, each session's two lines, and the "+N more" line.
const ROWS_ROOM: f32 = 292.0;
const AGENT_ROW_H: f32 = 70.0;
const ELSE_HEAD_H: f32 = 24.0;
const ELSE_ROW_H: f32 = 36.0;
const ELSE_MORE_H: f32 = 18.0;

/// How many sessions running elsewhere the panel lists (three at most) and how many it leaves to a "+N more".
pub fn elsewhere_rows(n: usize) -> (usize, usize) {
    (n.min(3), n.saturating_sub(3))
}

/// The same, leaving room for at least two of Sphere's own agents (elsewhere keeps one row and a "+N more").
pub fn elsewhere_rows_with(n: usize, own: usize) -> (usize, usize) {
    let (max, _) = elsewhere_rows(n);
    let want = own.min(2);
    let r = (1..=max).rev().find(|&r| agent_rows_in(own, elsewhere_height(r, n - r)).0 >= want).unwrap_or(max.min(1));
    (r, n - r)
}

fn elsewhere_height(rows: usize, more: usize) -> f32 {
    if rows == 0 {
        0.0
    } else {
        ELSE_HEAD_H + rows as f32 * ELSE_ROW_H + if more > 0 { ELSE_MORE_H } else { 0.0 }
    }
}

/// Sphere's own agents first; then, dimmer, what runs elsewhere: title and state, then folder and where.
fn build_elsewhere(commands: &mut Commands, list: Entity, f: &UiFonts, outside: &crate::takeover::Outside, rows: usize, more: usize) {
    use crate::takeover::{repo_name, state_label, title_of, where_label, ElsewhereRow};
    let line = role_color(Role::Line);
    let dim = role_color(Role::Dim);
    let acc = role_color(Role::Acc);
    let head = commands
        .spawn((Node { margin: UiRect { top: px(8), ..default() }, padding: UiRect { top: px(7), bottom: px(3), ..default() }, border: UiRect { top: px(1), ..default() }, ..default() }, BorderColor::all(with_alpha(line, 0.13))))
        .id();
    let ht = commands.spawn((Text::new("ELSEWHERE"), font(&f.ui700, 9.5, 3.0), TextColor(dim), LineHeight::Px(12.0))).id();
    commands.entity(head).add_child(ht);
    commands.entity(list).add_child(head);
    for (i, s) in outside.list.iter().enumerate().take(rows) {
        let waiting = s.state == "waiting";
        let row = commands
            .spawn((
                ElsewhereRow(i),
                Node { flex_direction: FlexDirection::Column, row_gap: px(1), padding: UiRect { left: px(8), right: px(8), top: px(3), bottom: px(3) }, margin: UiRect::horizontal(px(-8)), ..default() },
            ))
            .id();
        let top = commands.spawn(Node { flex_direction: FlexDirection::Row, justify_content: JustifyContent::SpaceBetween, align_items: AlignItems::Center, column_gap: px(10), ..default() }).id();
        let name_c = if waiting { acc } else { line };
        let name = commands
            .spawn((
                Text::new(crate::appframe::clip(&title_of(s), 26).to_uppercase()),
                font(&f.ui600, 12.0, 1.7),
                TextColor(with_alpha(name_c, if waiting { 1.0 } else { 0.62 })),
                LineHeight::Px(14.0),
                TextLayout { linebreak: LineBreak::NoWrap, ..default() },
            ))
            .id();
        let st = commands
            .spawn((Text::new(state_label(&s.state).to_uppercase()), font(&f.ui700, 9.5, 1.9), TextColor(if waiting { acc } else { with_alpha(dim, 0.9) }), LineHeight::Px(10.0), TextLayout { linebreak: LineBreak::NoWrap, ..default() }))
            .id();
        commands.entity(top).add_children(&[name, st]);
        let folder = repo_name(s.cwd.as_deref());
        let meta = if folder.is_empty() { where_label(&s.where_) } else { format!("{folder} · {}", where_label(&s.where_)) };
        let m = commands.spawn((Text::new(meta), font(&f.ui500, 12.0, 0.96), TextColor(with_alpha(line, 0.5)), LineHeight::Px(14.0), TextLayout { linebreak: LineBreak::NoWrap, ..default() })).id();
        commands.entity(row).add_children(&[top, m]);
        commands.entity(list).add_child(row);
    }
    if more > 0 {
        let m = commands
            .spawn((Text::new(format!("+{more} MORE ELSEWHERE")), font(&f.ui600, 10.0, 2.0), TextColor(with_alpha(dim, 0.8)), LineHeight::Px(12.0), Node { margin: UiRect { top: px(4), ..default() }, ..default() }))
            .id();
        commands.entity(list).add_child(m);
    }
}

/// The Agents panel fits four rows: with more agents than that it shows three and a "+N more" line.
#[cfg(test)]
pub fn agent_rows(n: usize) -> (usize, usize) {
    agent_rows_in(n, 0.0)
}

/// The same with `extra` px of the panel taken by the sessions running elsewhere: fewer rows fit, and the rest are
/// counted in a "+N more" line that takes the place of one row.
pub fn agent_rows_in(n: usize, extra: f32) -> (usize, usize) {
    let fit = ((ROWS_ROOM - extra) / AGENT_ROW_H).floor().max(0.0) as usize;
    if n <= fit {
        (n, 0)
    } else {
        let show = fit.saturating_sub(1);
        (show, n - show)
    }
}

pub fn rebuild_agents_ui(
    mut commands: Commands,
    agents: Res<Agents>,
    outside: Res<crate::takeover::Outside>,
    f: Res<UiFonts>,
    roots: Query<(Entity, &ChildOf), With<AgentsRoot>>,
    sim: Res<Sim>,
    mut last: Local<(u32, u64)>,
) {
    if *last == (agents.version, outside.version) {
        return;
    }
    for (body, parent) in &roots {
        let root = parent.parent();
        commands.entity(body).despawn();
        build_agents_ui(&mut commands, root, &f, &agents, &outside, sim.t);
    }
    *last = (agents.version, outside.version);
}

pub fn blink_system(sim: Res<Sim>, mut q: Query<&mut BackgroundColor, With<Blink>>) {
    // `steps(2)` over 1.4s: on for the first half, 15% for the second
    let on = (sim.t % 1.4) < 0.7;
    let fill = role_color(Role::Fill);
    for mut b in &mut q {
        // `set_if_neq`: only a real flip should mark the UI changed and wake the panel camera
        b.set_if_neq(BackgroundColor(with_alpha(fill, if on { 1.0 } else { 0.15 })));
    }
}

/// Frames every panel camera renders after start-up, while fonts, glyph atlases and layout settle.
const PANEL_WARMUP_FRAMES: u32 = 30;

/// Panel UI only changes now and then (an agents rebuild, the LED blink, a terminal row), yet each panel camera
/// would redraw its texture every frame. Keep a panel camera active only in frames where something under its UI
/// root changed; otherwise its image keeps last frame's pixels. Runs in `Last`, after layout and text.
#[allow(clippy::type_complexity)]
pub fn gate_panel_cameras(
    changed: Query<
        Entity,
        Or<(
            Changed<ComputedNode>,
            Changed<UiGlobalTransform>,
            Changed<bevy::text::TextLayoutInfo>,
            Changed<Text>,
            Changed<TextSpan>,
            Changed<TextColor>,
            Changed<TextBackgroundColor>,
            Changed<BackgroundColor>,
            Changed<BorderColor>,
            Changed<Children>,
            Changed<Visibility>,
        )>,
    >,
    parents: Query<&ChildOf>,
    roots: Query<&UiTargetCamera>,
    mut cams: Query<(Entity, &mut Camera, Option<&mut AtlasForce>), With<PanelUiCamera>>,
    mut frames: Local<u32>,
    mut dirty: Local<Vec<Entity>>,
) {
    *frames = frames.saturating_add(1);
    dirty.clear();
    for e in &changed {
        let mut cur = e;
        loop {
            if let Ok(t) = roots.get(cur) {
                if !dirty.contains(&t.entity()) {
                    dirty.push(t.entity());
                }
                break;
            }
            match parents.get(cur) {
                Ok(p) => cur = p.parent(),
                Err(_) => break,
            }
        }
    }
    for (e, mut cam, force) in &mut cams {
        let mut forced = false;
        if let Some(mut f) = force {
            if f.0 > 0 {
                f.0 -= 1;
                forced = true;
            }
        }
        let on = *frames <= PANEL_WARMUP_FRAMES || dirty.contains(&e) || forced;
        if cam.is_active != on {
            cam.is_active = on;
        }
    }
}
