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
            PanelId::Readout | PanelId::Moment | PanelId::Say => Some(Hue::Neutral),
            PanelId::System => Some(Hue::System),
            PanelId::Apps => Some(Hue::Apps),
            PanelId::Tele => Some(Hue::Tele),
            PanelId::Rail => Some(Hue::Rail),
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
}

/// Marks a camera that renders a panel's UI into its image. See `gate_panel_cameras`.
#[derive(Component)]
pub struct PanelUiCamera;

/// A UI camera that renders into an image, plus the full-size root node whose UI lives there.
pub fn spawn_ui_panel(commands: &mut Commands, images: &mut Assets<Image>, w: f32, h: f32, scale: f32, order: isize) -> UiPanel {
    let (pw, ph) = ((w * scale).ceil() as u32, (h * scale).ceil() as u32);
    let mut img = Image::new_fill(
        Extent3d { width: pw, height: ph, depth_or_array_layers: 1 },
        TextureDimension::D2,
        &[0, 0, 0, 0],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    img.texture_descriptor.usage = bevy::render::render_resource::TextureUsages::TEXTURE_BINDING
        | bevy::render::render_resource::TextureUsages::COPY_DST
        | bevy::render::render_resource::TextureUsages::RENDER_ATTACHMENT;
    let image = images.add(img);
    let camera = commands
        .spawn((
            Camera2d,
            Camera { order, clear_color: ClearColorConfig::Custom(Color::NONE), ..default() },
            RenderTarget::Image(ImageRenderTarget { handle: image.clone(), scale_factor: scale }),
            Msaa::Off,
            PanelUiCamera,
        ))
        .id();
    let root = commands
        .spawn((
            Node { width: px(w), height: px(h), position_type: PositionType::Absolute, left: px(0), top: px(0), ..default() },
            UiTargetCamera(camera),
        ))
        .id();
    UiPanel { root, camera, image }
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
            p: Vec4::new(if matches!(s.id, PanelId::Clock | PanelId::Brand | PanelId::Apps | PanelId::Rail | PanelId::Say) { 3.0 } else if s.unframed { 2.0 } else { 1.0 }, if card { 0.86 } else { 0.36 }, 1.0, 1.0),
            from_c,
            to_c,
            q: Vec4::new(0.0, 0.0, 0.0, 0.0),
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
pub fn update_panels(
    sim: Res<Sim>,
    mut ctl: ResMut<CoreCtl>,
    focus: Res<Focus>,
    moment: Res<crate::moment::Moment>,
    mut mats: ResMut<Assets<PanelMaterial>>,
    mut q: Query<(&mut Panel, &mut Transform, &mut Visibility), Without<ReadoutLink>>,
) {
    let (dt, t) = (sim.dt, sim.t);
    // the core fades back for the focused terminal, and partly for the Agent moment card so its text reads
    let ft = if focus.terminal { 1.0 } else if moment.shown { 0.75 } else { 0.0 };
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
        let drift = if sim.fixed_dt.is_some() { 0.6 } else { 1.0 };
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
            // the card: up while a request waits, flat and still so its text reads cleanly
            top = if moment.shown && !focus.terminal { 1.0 } else { 0.0 };
            tx = p.home.x;
            ty = p.home.y;
            tz = p.home.z + if moment.shown { 0.0 } else { -120.0 };
            trx = 0.0;
            try_ = 0.0;
        } else if moment.shown && !focus.terminal {
            // other panels step back while the card is up
            top *= 0.45;
            tz -= 40.0;
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
        p.op.t = top;
        p.px.step(dt);
        p.py.step(dt);
        p.pz.step(dt);
        p.rx.step(dt);
        p.ry.step(dt);
        p.sc.step(dt);
        p.op.step(dt);
        tr.translation = Vec3::new(p.px.x, p.py.x, p.pz.x);
        tr.rotation = Quat::from_euler(EulerRot::XYZ, p.rx.x, p.ry.x, 0.0);
        tr.scale = Vec3::splat(p.sc.x.max(0.001));
        let op = clamp(p.op.x, 0.0, 1.0);
        *vis = if op > 0.004 { Visibility::Inherited } else { Visibility::Hidden };
        // the frame draws in over 1.15 s, cubic-bezier(.65,.05,.25,1), once the panel shows (and each time the card rises)
        if p.id == PanelId::Moment && !moment.shown {
            p.draw_t = 0.0;
        } else if shown {
            p.draw_t += dt;
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

pub fn build_agents_ui(commands: &mut Commands, root: Entity, f: &UiFonts, agents: &Agents, sim_t: f32) {
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
                Text::new("No agents yet. Type a task in the bar below."),
                font(&f.ui500, 13.0, 1.04),
                TextColor(with_alpha(line, 0.7)),
                LineHeight::Px(16.9),
                Node { margin: UiRect { top: px(8), ..default() }, ..default() },
            ))
            .id();
        commands.entity(list).add_child(e);
    }
    for a in &agents.list {
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
}

pub fn rebuild_agents_ui(
    mut commands: Commands,
    agents: Res<Agents>,
    f: Res<UiFonts>,
    roots: Query<(Entity, &ChildOf), With<AgentsRoot>>,
    sim: Res<Sim>,
    mut last: Local<u32>,
) {
    if *last == agents.version {
        return;
    }
    for (body, parent) in &roots {
        let root = parent.parent();
        commands.entity(body).despawn();
        build_agents_ui(&mut commands, root, &f, &agents, sim.t);
    }
    *last = agents.version;
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
    mut cams: Query<(Entity, &mut Camera), With<PanelUiCamera>>,
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
    for (e, mut cam) in &mut cams {
        let on = *frames <= PANEL_WARMUP_FRAMES || dirty.contains(&e);
        if cam.is_active != on {
            cam.is_active = on;
        }
    }
}
