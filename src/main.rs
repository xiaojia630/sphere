//! Sphere OS: an agent-first operating surface.
//!
//! The whole desktop is a single glowing sphere wrapped in a HUD. Click it to
//! expand into the spaces you work in, click a space to fly into it and see
//! what's inside, drag to rotate, scroll or pinch to zoom, and press Esc (or
//! right-click) to step back out.

use std::f32::consts::{PI, TAU};

use bevy::asset::RenderAssetUsages;
use bevy::core_pipeline::{bloom::Bloom, tonemapping::Tonemapping};
use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit};
use bevy::picking::pointer::PointerButton;
use bevy::prelude::*;
use bevy::render::mesh::{Indices, PrimitiveTopology};

/// The spaces the core expands into, with their colour and what's inside.
const SPACES: [(&str, [f32; 3], &[&str]); 6] = [
    (
        "AGENTS",
        [0.3, 0.85, 1.0],
        &[
            "Inbox triage",
            "Q4 market research",
            "Weekly update draft",
            "Flights to Lagos",
        ],
    ),
    (
        "MAIL",
        [1.0, 0.55, 0.25],
        &["3 need a reply", "Invoices", "Follow-ups", "Newsletters"],
    ),
    (
        "CALENDAR",
        [0.45, 1.0, 0.6],
        &["Today", "Prep for 2pm", "Free slots", "Travel"],
    ),
    (
        "FILES",
        [0.75, 0.5, 1.0],
        &["Recent", "Shared with me", "Contracts", "Screenshots"],
    ),
    (
        "NOTES",
        [1.0, 0.85, 0.3],
        &["Ideas", "Meeting notes", "Journal"],
    ),
    (
        "WEB",
        [1.0, 0.4, 0.6],
        &["Open research", "Saved pages", "Watchlist"],
    ),
];

const CYAN: LinearRgba = LinearRgba::new(0.25, 0.75, 1.0, 1.0);
const ICE: LinearRgba = LinearRgba::new(0.75, 0.92, 1.0, 1.0);
const DEEP: LinearRgba = LinearRgba::new(0.05, 0.3, 1.0, 1.0);
const SPACE_RING_RADIUS: f32 = 3.6;
const SPACE_RING_TILT: f32 = 0.35;
const OVERVIEW_DISTANCE: f32 = 7.0;
const EXPANDED_DISTANCE: f32 = 11.5;
const FOCUS_DISTANCE: f32 = 3.6;
/// Pointer travel (px) after which a press counts as a drag, not a click.
const DRAG_THRESHOLD: f32 = 6.0;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Sphere OS".into(),
                canvas: Some("#sphere".into()),
                fit_canvas_to_parent: true,
                prevent_default_event_handling: true,
                ..default()
            }),
            ..default()
        }))
        .add_plugins(MeshPickingPlugin)
        .insert_resource(ClearColor(Color::srgb(0.004, 0.008, 0.02)))
        .insert_resource(AmbientLight {
            color: Color::srgb(0.6, 0.7, 1.0),
            brightness: 60.0,
            ..default()
        })
        .init_resource::<OrbitCamera>()
        .init_resource::<Gesture>()
        .init_resource::<Nav>()
        .init_resource::<Actions>()
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (
                camera_input,
                apply_actions,
                animate_core,
                animate_orbs,
                update_camera,
                billboard_hud,
                update_labels,
                update_hud,
                update_clock,
            )
                .chain(),
        )
        .run();
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// Smoothed orbit camera. Input moves the `*_goal` fields; the camera eases
/// towards them every frame.
#[derive(Resource)]
struct OrbitCamera {
    yaw: f32,
    pitch: f32,
    distance: f32,
    target: Vec3,
    yaw_goal: f32,
    pitch_goal: f32,
    distance_goal: f32,
    target_goal: Vec3,
    idle_secs: f32,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            yaw: 0.8,
            pitch: -0.15,
            distance: 30.0,
            target: Vec3::ZERO,
            yaw_goal: 0.0,
            pitch_goal: -0.15,
            distance_goal: OVERVIEW_DISTANCE,
            target_goal: Vec3::ZERO,
            idle_secs: 0.0,
        }
    }
}

/// How far the pointer has moved since the last press, so a drag that ends
/// over a sphere doesn't also count as a click on it.
#[derive(Resource, Default)]
struct Gesture {
    travelled: f32,
}

/// Where the user is in the sphere.
#[derive(Resource, Default)]
struct Nav {
    expanded: bool,
    focus: Option<Entity>,
    selected: Option<(usize, usize)>,
}

#[derive(Clone, Copy)]
enum Action {
    ClickCore,
    ClickOrb(Entity),
    Back,
}

/// Clicks collected by picking observers, applied once per frame.
#[derive(Resource, Default)]
struct Actions(Vec<Action>);

#[derive(Component)]
struct Core;

/// A flat HUD element that always faces the camera and turns in its own plane.
#[derive(Component)]
struct Billboard {
    speed: f32,
    angle: f32,
}

/// A piece of the core's HUD; dims while you're inside a space.
#[derive(Component)]
struct HudRing(LinearRgba);

/// The track the spaces sit on; fades in when the core is expanded.
#[derive(Component)]
struct Track;

#[derive(Component)]
struct HudPath;

#[derive(Component)]
struct HudDetail;

#[derive(Component)]
struct HudClock;

#[derive(Component)]
struct HudDate;

#[derive(Clone, Copy, PartialEq)]
enum OrbKind {
    Space(usize),
    Item(usize, usize),
}

#[derive(Component)]
struct Orb {
    kind: OrbKind,
    parent: Option<Entity>,
    label: Entity,
    origin: Vec3,
    home: Vec3,
    size: f32,
    color: LinearRgba,
    /// 0 = folded into its parent, 1 = fully out.
    anim: f32,
    alive: bool,
    hovered: bool,
    glow: f32,
    seed: f32,
}

#[derive(Resource)]
struct OrbAssets {
    sphere: Handle<Mesh>,
    ring: Handle<Mesh>,
}

// ---------------------------------------------------------------------------
// Setup
// ---------------------------------------------------------------------------

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn((
        Camera3d::default(),
        Camera {
            hdr: true,
            ..default()
        },
        Tonemapping::TonyMcMapface,
        Bloom {
            intensity: 0.32,
            ..Bloom::NATURAL
        },
        Transform::from_xyz(0.0, 0.0, 30.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    // Light from the core so the orbs are lit from the centre.
    commands.spawn((
        PointLight {
            color: Color::srgb(0.6, 0.85, 1.0),
            intensity: 400_000.0,
            range: 40.0,
            shadows_enabled: false,
            ..default()
        },
        Transform::default(),
    ));

    let sphere = meshes.add(Sphere::new(1.0).mesh().ico(5).unwrap());
    let orb_ring = meshes.add(arc_mesh(&orb_ring_arcs()));
    commands.insert_resource(OrbAssets {
        sphere: sphere.clone(),
        ring: orb_ring,
    });

    // The core.
    commands
        .spawn((
            Core,
            Mesh3d(sphere.clone()),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: Color::srgb(0.02, 0.05, 0.1),
                emissive: CYAN * 1.2,
                ..default()
            })),
            Transform::from_scale(Vec3::splat(0.9)),
        ))
        .observe(
            |trigger: Trigger<Pointer<Click>>,
             gesture: Res<Gesture>,
             mut actions: ResMut<Actions>| {
                if trigger.event().button == PointerButton::Primary
                    && gesture.travelled < DRAG_THRESHOLD
                {
                    actions.0.push(Action::ClickCore);
                }
            },
        );

    // HUD rings around the core, in the style of a JARVIS display.
    for (arcs, color, speed) in hud_rings() {
        commands.spawn((
            Mesh3d(meshes.add(arc_mesh(&arcs))),
            MeshMaterial3d(materials.add(glow_material(color))),
            Transform::default(),
            Billboard { speed, angle: 0.0 },
            HudRing(color),
            Pickable::IGNORE,
        ));
    }

    // The track the spaces orbit on.
    let mut track_arcs = vec![(
        SPACE_RING_RADIUS - 0.012,
        SPACE_RING_RADIUS + 0.012,
        0.0,
        TAU,
    )];
    for i in 0..120 {
        let a = i as f32 / 120.0 * TAU;
        let len = if i % 10 == 0 { 0.14 } else { 0.05 };
        track_arcs.push((
            SPACE_RING_RADIUS + 0.04,
            SPACE_RING_RADIUS + 0.04 + len,
            a,
            a + 0.006,
        ));
    }
    commands.spawn((
        Track,
        Mesh3d(meshes.add(arc_mesh(&track_arcs))),
        MeshMaterial3d(materials.add(glow_material(CYAN * 0.0))),
        Transform::from_rotation(
            Quat::from_rotation_x(SPACE_RING_TILT) * Quat::from_rotation_x(-PI / 2.0),
        ),
        Pickable::IGNORE,
    ));

    // Distant stars.
    let dot = meshes.add(Sphere::new(1.0).mesh().ico(0).unwrap());
    let star_material = materials.add(glow_material(LinearRgba::new(0.5, 0.65, 1.0, 1.0) * 1.5));
    let n = 500;
    for i in 0..n {
        let r = 45.0 + 25.0 * hash(i as f32 * 7.3);
        commands.spawn((
            Mesh3d(dot.clone()),
            MeshMaterial3d(star_material.clone()),
            Transform::from_translation(fibonacci(i, n) * r)
                .with_scale(Vec3::splat(0.03 + 0.05 * hash(i as f32))),
            Pickable::IGNORE,
        ));
    }

    spawn_hud_text(&mut commands);
}

fn spawn_hud_text(commands: &mut Commands) {
    let dim = Color::srgba(0.55, 0.8, 1.0, 0.55);
    let font = |size: f32| TextFont {
        font_size: size,
        ..default()
    };

    // Top left: name and where you are.
    commands
        .spawn(Node {
            position_type: PositionType::Absolute,
            left: Val::Px(20.0),
            top: Val::Px(16.0),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(4.0),
            ..default()
        })
        .with_children(|hud| {
            hud.spawn((
                Text::new("SPHERE OS"),
                font(18.0),
                TextColor(Color::srgb(0.55, 0.85, 1.0)),
            ));
            hud.spawn((HudPath, Text::new("CORE"), font(13.0), TextColor(dim)));
        });

    // Top centre: clock.
    commands
        .spawn(Node {
            position_type: PositionType::Absolute,
            top: Val::Px(14.0),
            left: Val::Px(0.0),
            right: Val::Px(0.0),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            ..default()
        })
        .with_children(|hud| {
            hud.spawn((
                HudClock,
                Text::new("--:--"),
                font(30.0),
                TextColor(Color::srgb(0.85, 0.95, 1.0)),
            ));
            hud.spawn((HudDate, Text::new(""), font(12.0), TextColor(dim)));
        });

    // Top right: system status. These come online in later steps.
    commands.spawn((
        Text::new("AGENT LINK  OFFLINE\nVOICE       OFFLINE\nTASKS             0"),
        font(12.0),
        TextColor(dim),
        TextLayout::new_with_justify(JustifyText::Right),
        Node {
            position_type: PositionType::Absolute,
            right: Val::Px(20.0),
            top: Val::Px(18.0),
            ..default()
        },
    ));

    // Bottom: what's selected, and how to drive it.
    commands.spawn((
        HudDetail,
        Text::new(""),
        font(15.0),
        TextColor(Color::srgb(0.85, 0.95, 1.0)),
        TextLayout::new_with_justify(JustifyText::Center),
        Node {
            position_type: PositionType::Absolute,
            bottom: Val::Px(50.0),
            left: Val::Px(16.0),
            right: Val::Px(16.0),
            justify_content: JustifyContent::Center,
            ..default()
        },
    ));
    commands.spawn((
        Text::new("DRAG  rotate   /   SCROLL  zoom   /   CLICK  expand   /   ESC  back"),
        font(11.0),
        TextColor(Color::srgba(0.55, 0.8, 1.0, 0.4)),
        TextLayout::new_with_justify(JustifyText::Center),
        Node {
            position_type: PositionType::Absolute,
            bottom: Val::Px(18.0),
            left: Val::Px(16.0),
            right: Val::Px(16.0),
            justify_content: JustifyContent::Center,
            ..default()
        },
    ));
}

/// An unlit, additive material: on a black background it reads as light.
fn glow_material(color: LinearRgba) -> StandardMaterial {
    StandardMaterial {
        base_color: Color::LinearRgba(color),
        unlit: true,
        alpha_mode: AlphaMode::Add,
        cull_mode: None,
        double_sided: true,
        ..default()
    }
}

/// An annular arc: inner radius, outer radius, start angle, end angle.
type Arc = (f32, f32, f32, f32);

/// The core's HUD: concentric rings of ticks, arcs and bars, each turning at
/// its own speed.
fn hud_rings() -> Vec<(Vec<Arc>, LinearRgba, f32)> {
    let mut rings = Vec::new();

    // Tight bright rim.
    rings.push((vec![(1.08, 1.095, 0.0, TAU)], ICE * 3.0, 0.0));

    // Fine tick ring with a long tick every 6.
    let ticks: Vec<Arc> = (0..144)
        .map(|i| {
            let a = i as f32 / 144.0 * TAU;
            let outer = if i % 6 == 0 { 1.33 } else { 1.27 };
            (1.21, outer, a, a + 0.007)
        })
        .collect();
    rings.push((ticks, CYAN * 1.6, 0.04));

    // Segmented band, alternating bright and dim segments.
    let segment = |i: usize| {
        let a = i as f32 / 10.0 * TAU;
        (1.42, 1.5, a, a + TAU / 10.0 * 0.82)
    };
    rings.push(((0..10).step_by(2).map(segment).collect(), CYAN * 1.4, -0.11));
    rings.push(((1..10).step_by(2).map(segment).collect(), DEEP * 0.9, -0.11));

    // Thin circle with compass marks.
    let mut compass = vec![(1.62, 1.627, 0.0, TAU)];
    for i in 0..4 {
        let a = i as f32 / 4.0 * TAU - 0.13;
        compass.push((1.59, 1.66, a, a + 0.26));
    }
    rings.push((compass, ICE * 1.4, 0.025));

    // Equaliser bars, like the readouts on the reference HUD.
    let bars = |range: std::ops::Range<usize>, keep: fn(usize) -> bool| -> Vec<Arc> {
        range
            .filter(|&i| keep(i))
            .map(|i| {
                let a = i as f32 / 96.0 * TAU;
                let h = hash(i as f32 * 3.7);
                (1.78, 1.84 + 0.32 * h * h, a, a + 0.035)
            })
            .collect()
    };
    rings.push((bars(0..96, |i| i % 5 != 0), DEEP * 1.6, -0.035));
    rings.push((bars(0..96, |i| i % 5 == 0), CYAN * 2.2, -0.035));

    // Outer sweeping arcs.
    rings.push((
        vec![
            (2.3, 2.33, 0.4, 4.4),
            (2.3, 2.4, 4.65, 5.5),
            (2.45, 2.47, 5.7, 6.1),
        ],
        ICE * 1.1,
        0.018,
    ));
    rings.push((vec![(2.52, 2.53, 0.0, TAU)], DEEP * 0.6, 0.0));

    rings
}

/// The small tick ring worn by each orb (in units of the orb's radius).
fn orb_ring_arcs() -> Vec<Arc> {
    let mut arcs: Vec<Arc> = (0..24)
        .map(|i| {
            let a = i as f32 / 24.0 * TAU;
            (1.3, 1.45, a, a + 0.06)
        })
        .collect();
    arcs.push((1.6, 1.65, 0.3, 2.6));
    arcs.push((1.6, 1.65, 3.4, 5.7));
    arcs
}

/// Builds a flat mesh in the XY plane out of annular arcs.
fn arc_mesh(arcs: &[Arc]) -> Mesh {
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    for &(inner, outer, start, end) in arcs {
        let steps = ((end - start).abs() / TAU * 160.0).ceil().max(1.0) as u32;
        let base = positions.len() as u32;
        for s in 0..=steps {
            let a = start + (end - start) * s as f32 / steps as f32;
            let (sin, cos) = a.sin_cos();
            positions.push([cos * inner, sin * inner, 0.0]);
            positions.push([cos * outer, sin * outer, 0.0]);
        }
        for s in 0..steps {
            let i = base + s * 2;
            indices.extend([i, i + 1, i + 2, i + 1, i + 3, i + 2]);
        }
    }
    let n = positions.len();
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 0.0, 1.0]; n])
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0, 0.0]; n])
    .with_inserted_indices(Indices::U32(indices))
}

/// Evenly spread point `i` of `n` on the unit sphere.
fn fibonacci(i: usize, n: usize) -> Vec3 {
    let y = 1.0 - 2.0 * (i as f32 + 0.5) / n as f32;
    let r = (1.0 - y * y).sqrt();
    let theta = i as f32 * PI * (3.0 - 5f32.sqrt());
    Vec3::new(r * theta.cos(), y, r * theta.sin())
}

/// Cheap deterministic noise in [0, 1).
fn hash(x: f32) -> f32 {
    ((x * 12.9898).sin() * 43758.547).fract().abs()
}

// ---------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------

fn camera_input(
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    mouse_motion: Res<AccumulatedMouseMotion>,
    mouse_scroll: Res<AccumulatedMouseScroll>,
    touches: Res<Touches>,
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    mut cam: ResMut<OrbitCamera>,
    mut gesture: ResMut<Gesture>,
    mut actions: ResMut<Actions>,
) {
    let mut drag = Vec2::ZERO;
    let mut zoom = 0.0;

    // Motion in the frame of the press itself happened before the press.
    if mouse_buttons.just_pressed(MouseButton::Left) || touches.any_just_pressed() {
        gesture.travelled = 0.0;
    } else if mouse_buttons.pressed(MouseButton::Left) {
        drag += mouse_motion.delta;
    }

    let active: Vec<_> = touches.iter().collect();
    match active.as_slice() {
        [one] if !touches.any_just_pressed() => drag += one.delta(),
        [a, b] => {
            let now = a.position().distance(b.position());
            let before = a.previous_position().distance(b.previous_position());
            zoom += (now - before) * 0.01;
            gesture.travelled += DRAG_THRESHOLD;
        }
        _ => {}
    }

    gesture.travelled += drag.length();

    zoom += match mouse_scroll.unit {
        MouseScrollUnit::Line => mouse_scroll.delta.y * 0.12,
        MouseScrollUnit::Pixel => mouse_scroll.delta.y * 0.0015,
    };

    if drag != Vec2::ZERO {
        cam.yaw_goal -= drag.x * 0.005;
        cam.pitch_goal = (cam.pitch_goal - drag.y * 0.005).clamp(-1.45, 1.45);
    }
    if zoom != 0.0 {
        cam.distance_goal = (cam.distance_goal * (1.0 - zoom).clamp(0.5, 1.5)).clamp(1.8, 40.0);
    }

    if drag != Vec2::ZERO || zoom != 0.0 || mouse_buttons.get_just_pressed().next().is_some() {
        cam.idle_secs = 0.0;
    } else {
        cam.idle_secs += time.delta_secs();
    }

    let right_click = mouse_buttons.just_released(MouseButton::Right);
    if keys.just_pressed(KeyCode::Escape) || keys.just_pressed(KeyCode::Backspace) || right_click {
        actions.0.push(Action::Back);
    }
}

fn on_orb_click(
    trigger: Trigger<Pointer<Click>>,
    gesture: Res<Gesture>,
    mut actions: ResMut<Actions>,
) {
    if trigger.event().button == PointerButton::Primary && gesture.travelled < DRAG_THRESHOLD {
        actions.0.push(Action::ClickOrb(trigger.target()));
    }
}

fn on_orb_over(trigger: Trigger<Pointer<Over>>, mut orbs: Query<&mut Orb>) {
    if let Ok(mut orb) = orbs.get_mut(trigger.target()) {
        orb.hovered = true;
    }
}

fn on_orb_out(trigger: Trigger<Pointer<Out>>, mut orbs: Query<&mut Orb>) {
    if let Ok(mut orb) = orbs.get_mut(trigger.target()) {
        orb.hovered = false;
    }
}

// ---------------------------------------------------------------------------
// Navigation
// ---------------------------------------------------------------------------

fn apply_actions(
    mut commands: Commands,
    mut actions: ResMut<Actions>,
    mut nav: ResMut<Nav>,
    mut cam: ResMut<OrbitCamera>,
    mut orbs: Query<(Entity, &mut Orb)>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    assets: Res<OrbAssets>,
) {
    for action in std::mem::take(&mut actions.0) {
        match action {
            Action::ClickCore => {
                if !nav.expanded {
                    nav.expanded = true;
                    for (i, (_, color, _)) in SPACES.iter().enumerate() {
                        let angle = i as f32 / SPACES.len() as f32 * TAU;
                        let home = Quat::from_rotation_x(SPACE_RING_TILT)
                            * Vec3::new(angle.cos(), 0.0, angle.sin())
                            * SPACE_RING_RADIUS;
                        spawn_orb(
                            &mut commands,
                            &mut materials,
                            &assets,
                            OrbKind::Space(i),
                            None,
                            Vec3::ZERO,
                            home,
                            0.36,
                            LinearRgba::from_f32_array_no_alpha(*color),
                        );
                    }
                    focus_overview(&mut cam, EXPANDED_DISTANCE);
                } else if nav.focus.is_some() || nav.selected.is_some() {
                    unfocus(&mut nav, &mut orbs, &mut cam);
                } else {
                    collapse_all(&mut nav, &mut orbs, &mut cam);
                }
            }
            Action::ClickOrb(entity) => {
                let Ok((_, orb)) = orbs.get(entity) else {
                    continue;
                };
                if !orb.alive {
                    continue;
                }
                match orb.kind {
                    OrbKind::Space(space) => {
                        if nav.focus == Some(entity) {
                            unfocus(&mut nav, &mut orbs, &mut cam);
                            continue;
                        }
                        let home = orb.home;
                        if nav.focus.is_some() {
                            unfocus(&mut nav, &mut orbs, &mut cam);
                        }
                        nav.focus = Some(entity);
                        let items = SPACES[space].2;
                        let out = home.normalize();
                        let tangent = out.cross(Vec3::Y).normalize_or(Vec3::X);
                        let bitangent = out.cross(tangent);
                        for (j, _) in items.iter().enumerate() {
                            let angle = j as f32 / items.len() as f32 * TAU + 0.4;
                            let offset = (tangent * angle.cos() + bitangent * angle.sin()) * 1.05
                                + out * 0.35;
                            let color = LinearRgba::from_f32_array_no_alpha(SPACES[space].1);
                            spawn_orb(
                                &mut commands,
                                &mut materials,
                                &assets,
                                OrbKind::Item(space, j),
                                Some(entity),
                                home,
                                home + offset,
                                0.15,
                                color,
                            );
                        }
                        // Fly in from outside the ring so the space sits in front of the core.
                        let view = (out + Vec3::Y * 0.35).normalize();
                        let yaw = view.x.atan2(view.z);
                        cam.yaw_goal += wrap_angle(yaw - cam.yaw_goal);
                        cam.pitch_goal = -view.y.asin();
                        cam.target_goal = home;
                        cam.distance_goal = FOCUS_DISTANCE;
                    }
                    OrbKind::Item(space, item) => {
                        nav.selected = if nav.selected == Some((space, item)) {
                            None
                        } else {
                            Some((space, item))
                        };
                    }
                }
            }
            Action::Back => {
                if nav.selected.is_some() {
                    nav.selected = None;
                } else if nav.focus.is_some() {
                    unfocus(&mut nav, &mut orbs, &mut cam);
                } else if nav.expanded {
                    collapse_all(&mut nav, &mut orbs, &mut cam);
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_orb(
    commands: &mut Commands,
    materials: &mut Assets<StandardMaterial>,
    assets: &OrbAssets,
    kind: OrbKind,
    parent: Option<Entity>,
    origin: Vec3,
    home: Vec3,
    size: f32,
    color: LinearRgba,
) {
    let (name, font_size) = match kind {
        OrbKind::Space(i) => (SPACES[i].0, 14.0),
        OrbKind::Item(i, j) => (SPACES[i].2[j], 12.0),
    };
    let label = commands
        .spawn((
            Text::new(name),
            TextFont {
                font_size,
                ..default()
            },
            TextColor(Color::NONE),
            TextLayout::new_with_justify(JustifyText::Center),
            Node {
                position_type: PositionType::Absolute,
                width: Val::Px(180.0),
                justify_content: JustifyContent::Center,
                ..default()
            },
            Visibility::Hidden,
        ))
        .id();

    commands
        .spawn((
            Orb {
                kind,
                parent,
                label,
                origin,
                home,
                size,
                color,
                anim: 0.0,
                alive: true,
                hovered: false,
                glow: 0.0,
                seed: home.x * 3.1 + home.y * 1.7 + home.z * 2.3,
            },
            Mesh3d(assets.sphere.clone()),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: Color::LinearRgba(color * 0.15),
                emissive: color * 0.6,
                perceptual_roughness: 0.3,
                ..default()
            })),
            Transform::from_translation(origin).with_scale(Vec3::ZERO),
        ))
        .with_child((
            Mesh3d(assets.ring.clone()),
            MeshMaterial3d(materials.add(glow_material(color * 1.6))),
            Transform::default(),
            Billboard {
                speed: if matches!(kind, OrbKind::Space(_)) {
                    0.3
                } else {
                    -0.5
                },
                angle: home.x,
            },
            Pickable::IGNORE,
        ))
        .observe(on_orb_click)
        .observe(on_orb_over)
        .observe(on_orb_out);
}

fn unfocus(nav: &mut Nav, orbs: &mut Query<(Entity, &mut Orb)>, cam: &mut OrbitCamera) {
    if let Some(focus) = nav.focus.take() {
        for (_, mut orb) in orbs.iter_mut() {
            if orb.parent == Some(focus) {
                orb.alive = false;
            }
        }
    }
    nav.selected = None;
    focus_overview(cam, EXPANDED_DISTANCE);
}

fn collapse_all(nav: &mut Nav, orbs: &mut Query<(Entity, &mut Orb)>, cam: &mut OrbitCamera) {
    for (_, mut orb) in orbs.iter_mut() {
        orb.alive = false;
    }
    *nav = Nav::default();
    focus_overview(cam, OVERVIEW_DISTANCE);
}

fn focus_overview(cam: &mut OrbitCamera, distance: f32) {
    cam.target_goal = Vec3::ZERO;
    cam.distance_goal = distance;
    cam.pitch_goal = cam.pitch_goal.clamp(-0.6, 0.2);
}

fn wrap_angle(a: f32) -> f32 {
    (a + PI).rem_euclid(TAU) - PI
}

// ---------------------------------------------------------------------------
// Animation
// ---------------------------------------------------------------------------

fn animate_core(
    time: Res<Time>,
    nav: Res<Nav>,
    mut core: Query<(&mut Transform, &MeshMaterial3d<StandardMaterial>), With<Core>>,
    track: Query<&MeshMaterial3d<StandardMaterial>, With<Track>>,
    rings: Query<(&HudRing, &MeshMaterial3d<StandardMaterial>)>,
    mut track_fade: Local<f32>,
    mut ring_fade: Local<Option<f32>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let t = time.elapsed_secs();
    let breathe = (t * 1.6).sin();

    if let Ok((mut transform, material)) = core.single_mut() {
        transform.scale = Vec3::splat(0.9 * (1.0 + 0.02 * breathe));
        if let Some(material) = materials.get_mut(&material.0) {
            let energy = if nav.expanded { 1.5 } else { 1.1 };
            material.emissive = CYAN * (energy + 0.3 * breathe);
        }
    }

    let dt = time.delta_secs();
    let ring_goal = if nav.focus.is_some() { 0.15 } else { 1.0 };
    let fade = ring_fade.get_or_insert(1.0);
    if *fade != ring_goal {
        *fade = move_towards(*fade, ring_goal, dt * 2.0);
        for (ring, material) in rings.iter() {
            if let Some(material) = materials.get_mut(&material.0) {
                material.base_color = Color::LinearRgba(ring.0 * *fade);
            }
        }
    }

    let goal = if nav.expanded { 1.0 } else { 0.0 };
    *track_fade = move_towards(*track_fade, goal, dt * 1.5);
    if let Ok(material) = track.single() {
        if let Some(material) = materials.get_mut(&material.0) {
            material.base_color = Color::LinearRgba(CYAN * 0.6 * *track_fade);
        }
    }
}

fn animate_orbs(
    mut commands: Commands,
    time: Res<Time>,
    mut orbs: Query<(
        Entity,
        &mut Orb,
        &mut Transform,
        &MeshMaterial3d<StandardMaterial>,
    )>,
    nav: Res<Nav>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let dt = time.delta_secs();
    let t = time.elapsed_secs();
    for (entity, mut orb, mut transform, material) in orbs.iter_mut() {
        let goal = if orb.alive { 1.0 } else { 0.0 };
        let rate = if orb.alive { 2.2 } else { 4.0 };
        orb.anim = move_towards(orb.anim, goal, dt * rate);
        let selected = matches!(orb.kind, OrbKind::Item(i, j) if nav.selected == Some((i, j)));
        let focused = nav.focus == Some(entity);
        let glow_goal = if orb.hovered || selected || focused {
            1.0
        } else {
            0.0
        };
        orb.glow = move_towards(orb.glow, glow_goal, dt * 6.0);

        if !orb.alive && orb.anim <= 0.0 {
            commands.entity(orb.label).despawn();
            commands.entity(entity).despawn();
            continue;
        }

        let e = ease_out_back(orb.anim);
        let bob = Vec3::Y * (t * 1.3 + orb.seed).sin() * 0.04 * orb.anim;
        transform.translation = orb.origin.lerp(orb.home, ease_out_cubic(orb.anim)) + bob;
        transform.scale = Vec3::splat(orb.size * e.max(0.0) * (1.0 + 0.18 * orb.glow));

        if let Some(material) = materials.get_mut(&material.0) {
            let pulse = if selected {
                1.0 + 0.4 * (t * 6.0).sin()
            } else {
                1.0
            };
            material.emissive = orb.color * (0.55 + 1.4 * orb.glow) * pulse;
        }
    }
}

fn move_towards(value: f32, goal: f32, step: f32) -> f32 {
    if value < goal {
        (value + step).min(goal)
    } else {
        (value - step).max(goal)
    }
}

fn ease_out_cubic(x: f32) -> f32 {
    1.0 - (1.0 - x).powi(3)
}

fn ease_out_back(x: f32) -> f32 {
    let c1 = 1.70158;
    let c3 = c1 + 1.0;
    1.0 + c3 * (x - 1.0).powi(3) + c1 * (x - 1.0).powi(2)
}

fn update_camera(
    time: Res<Time>,
    nav: Res<Nav>,
    mut cam: ResMut<OrbitCamera>,
    mut camera: Query<&mut Transform, With<Camera3d>>,
) {
    let dt = time.delta_secs();
    // Drift slowly when nobody is touching it.
    if cam.idle_secs > 4.0 && nav.focus.is_none() {
        cam.yaw_goal += dt * 0.08;
    }
    let k = 1.0 - (-dt * 6.0).exp();
    let kd = 1.0 - (-dt * 3.0).exp();
    cam.yaw += (cam.yaw_goal - cam.yaw) * k;
    cam.pitch += (cam.pitch_goal - cam.pitch) * k;
    cam.distance += (cam.distance_goal - cam.distance) * kd;
    let target_goal = cam.target_goal;
    cam.target = cam.target.lerp(target_goal, kd);

    if let Ok(mut transform) = camera.single_mut() {
        let rotation = Quat::from_euler(EulerRot::YXZ, cam.yaw, cam.pitch, 0.0);
        transform.translation = cam.target + rotation * Vec3::Z * cam.distance;
        transform.look_at(cam.target, Vec3::Y);
    }
}

/// Keeps flat HUD pieces facing the camera. Orbs never rotate, so the camera
/// rotation works as a local rotation for their rings too.
fn billboard_hud(
    time: Res<Time>,
    camera: Query<&Transform, (With<Camera3d>, Without<Billboard>)>,
    mut huds: Query<(&mut Transform, &mut Billboard)>,
) {
    let Ok(camera) = camera.single() else { return };
    for (mut transform, mut hud) in huds.iter_mut() {
        hud.angle += hud.speed * time.delta_secs();
        transform.rotation = camera.rotation * Quat::from_rotation_z(hud.angle);
    }
}

fn update_labels(
    camera: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    orbs: Query<(&Orb, &Transform)>,
    mut labels: Query<(&mut Node, &mut Visibility, &mut TextColor)>,
) {
    let Ok((camera, camera_transform)) = camera.single() else {
        return;
    };
    let view_dir = camera_transform.forward();
    for (orb, transform) in orbs.iter() {
        let Ok((mut node, mut visibility, mut color)) = labels.get_mut(orb.label) else {
            continue;
        };
        let anchor = transform.translation - Vec3::Y * (transform.scale.y * 1.7 + 0.05);
        let in_front = (anchor - camera_transform.translation()).dot(*view_dir) > 0.0;
        match camera.world_to_viewport(camera_transform, anchor) {
            Ok(pos) if in_front && orb.anim > 0.2 => {
                node.left = Val::Px(pos.x - 90.0);
                node.top = Val::Px(pos.y + 2.0);
                *visibility = Visibility::Visible;
                let alpha = ((orb.anim - 0.2) / 0.8).clamp(0.0, 1.0) * (0.75 + 0.25 * orb.glow);
                color.0 = Color::srgba(0.88, 0.95, 1.0, alpha);
            }
            _ => *visibility = Visibility::Hidden,
        }
    }
}

fn update_hud(
    nav: Res<Nav>,
    orbs: Query<&Orb>,
    mut path: Query<&mut Text, (With<HudPath>, Without<HudDetail>)>,
    mut detail: Query<&mut Text, (With<HudDetail>, Without<HudPath>)>,
) {
    if !nav.is_changed() {
        return;
    }
    let mut crumbs = vec!["CORE".to_string()];
    let space = nav
        .focus
        .and_then(|e| orbs.get(e).ok())
        .and_then(|orb| match orb.kind {
            OrbKind::Space(i) => Some(i),
            OrbKind::Item(..) => None,
        });
    if let Some(i) = space {
        crumbs.push(SPACES[i].0.to_string());
    }
    if let Some((i, j)) = nav.selected {
        crumbs.push(SPACES[i].2[j].to_uppercase());
    }
    if let Ok(mut text) = path.single_mut() {
        text.0 = crumbs.join("  >  ");
    }
    if let Ok(mut text) = detail.single_mut() {
        text.0 = match nav.selected {
            Some((i, j)) => format!(
                "{}  /  {}\nAn agent will pick this up for you once the agent link is online.",
                SPACES[i].0, SPACES[i].2[j]
            ),
            None if !nav.expanded => "Click the sphere".to_string(),
            None => String::new(),
        };
    }
}

fn update_clock(
    mut last: Local<String>,
    mut clock: Query<&mut Text, (With<HudClock>, Without<HudDate>)>,
    mut date: Query<&mut Text, (With<HudDate>, Without<HudClock>)>,
) {
    let now = chrono::Local::now();
    let time = now.format("%H:%M").to_string();
    if *last == time {
        return;
    }
    if let Ok(mut text) = clock.single_mut() {
        text.0 = time.clone();
    }
    if let Ok(mut text) = date.single_mut() {
        text.0 = now.format("%a %d %b %Y").to_string().to_uppercase();
    }
    *last = time;
}
