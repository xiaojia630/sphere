//! Bevy's per-frame machinery cut to what Sphere uses. Bevy hands every system of every schedule to a pool of
//! worker threads; Sphere's frame is a few hundred systems that each take microseconds, so the hand-offs cost
//! more than the work (most of the app's CPU went to workers waking each other, ~650 thread switches a frame).
//! Here each schedule runs its systems in order on the thread that runs it (main thread, render thread), the
//! pools keep one helper for the few engine loops that split work, and engine features Sphere never draws
//! (motion blur, depth of field, TAA, deferred lighting, last-frame transforms for motion vectors) are left out.
//! On the GPU side, passes that would only load and store a full-screen image are skipped, 2D views get no depth
//! image they never test against, and the (empty) light clusters are rebuilt only when their layout changes.
//! `SPHERE_LEAN=0` keeps Bevy's defaults, for measuring.
use bevy::core_pipeline::core_2d::{self, AlphaMask2d, Opaque2d, Transparent2d, CORE_2D_DEPTH_FORMAT};
use bevy::core_pipeline::core_3d::{self, AlphaMask3d, Opaque3d, Transparent3d};
use bevy::core_pipeline::oit::OrderIndependentTransparencySettings;
use bevy::core_pipeline::skybox::SkyboxPipelineId;
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::core_pipeline::{Core2d, Core2dSystems, Core3d, Core3dSystems};
use bevy::ecs::schedule::{ScheduleCleanupPolicy, Schedules, SingleThreadedExecutor};
use bevy::ecs::system::SystemParam;
use bevy::image::ToExtents;
use bevy::light::cluster::{ClusterableObjects, Clusters, GlobalClusterSettings};
use bevy::pbr::{prepare_clusters_for_cpu_clustering, ExtractedClusterableObjects, Transmissive3d, ViewClusterBindings};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::render::camera::ExtractedCamera;
use bevy::render::render_phase::{ViewBinnedRenderPhases, ViewSortedRenderPhases};
use bevy::render::render_resource::{TextureDescriptor, TextureDimension, TextureUsages};
use bevy::render::renderer::{RenderDevice, ViewQuery};
use bevy::render::texture::TextureCache;
use bevy::render::view::{ExtractedView, ViewDepthStencilTexture};
use bevy::render::{Extract, ExtractSchedule, Render, RenderApp, RenderSystems};
use bevy::ui_render::{TransparentUi, UiCameraView};

pub fn on() -> bool {
    !std::env::var("SPHERE_LEAN").is_ok_and(|v| v == "0")
}

/// Added last: its `finish` runs after every other plugin's build and finish, while the render world is still
/// reachable (pipelined rendering moves it to its own thread in `cleanup`).
pub struct LeanPlugin;

impl Plugin for LeanPlugin {
    fn build(&self, _app: &mut App) {}

    fn finish(&self, app: &mut App) {
        if !on() {
            return;
        }
        // No motion vectors: nothing reads last frame's transforms or view, yet they were copied every frame.
        let gone = ScheduleCleanupPolicy::RemoveSystemsOnly;
        let _ = app.remove_systems_in_set(PreUpdate, bevy::pbr::update_mesh_previous_global_transforms, gone);
        let _ = app.remove_systems_in_set(PreUpdate, bevy::pbr::update_previous_view_data, gone);
        single_threaded(app.world_mut());
        if let Some(r) = app.get_sub_app_mut(RenderApp) {
            // Creating and resizing the window's surface must happen on the main thread on macOS; Bevy gets it
            // there by having the multi-threaded executor send that one system over. With the render schedule
            // running on the render thread it moves to extraction, which runs on the main thread while the render
            // thread waits (right after the window sizes are extracted, so a resize still applies that frame).
            use bevy::render::view::window::{create_surfaces, need_surface_configuration};
            let _ = r.remove_systems_in_set(Render, create_surfaces, gone);
            r.add_systems(ExtractSchedule, create_surfaces.run_if(need_surface_configuration).after(bevy::render::camera::extract_cameras));

            // Nothing Sphere draws is opaque (glow is additive, panels premultiplied), so each camera's opaque pass
            // only loaded and stored its full-screen image. A view's first pass clears it, so when a later pass
            // draws, the clear moves there and the pixels stay the same.
            let _ = r.remove_systems_in_set(Core3d, core_3d::main_opaque_pass_3d, gone);
            r.add_systems(Core3d, core_3d::main_opaque_pass_3d.run_if(opaque_3d_runs).before(core_3d::main_transparent_pass_3d).in_set(Core3dSystems::MainPass));
            let _ = r.remove_systems_in_set(Core2d, core_2d::main_opaque_pass_2d, gone);
            r.add_systems(Core2d, core_2d::main_opaque_pass_2d.run_if(opaque_2d_runs).before(core_2d::main_transparent_pass_2d).in_set(Core2dSystems::MainPass));
            let _ = r.remove_systems_in_set(Render, core_2d::prepare_core_2d_depth_textures, gone);
            r.add_systems(Render, prepare_2d_depth.in_set(RenderSystems::PrepareResources));

            // Sphere has no lights, yet each 3D view got new light-cluster buffers every frame.
            let _ = r.remove_systems_in_set(Render, prepare_clusters_for_cpu_clustering, gone);
            r.init_resource::<ClusterLayout>().add_systems(ExtractSchedule, cluster_layout).add_systems(
                Render,
                prepare_clusters_for_cpu_clustering.in_set(RenderSystems::PrepareResources).run_if(clusters_stale),
            );
            single_threaded(r.world_mut());
        }
    }
}

fn single_threaded(world: &mut World) {
    if let Some(mut schedules) = world.get_resource_mut::<Schedules>() {
        for (_, s) in schedules.iter_mut() {
            s.set_executor(SingleThreadedExecutor::new());
        }
    }
}

/// Bloom and the anti-aliasing passes Sphere uses, without the other post effects their plugin groups add
/// (each costs per-camera graph systems every frame even when off).
pub fn trim(g: bevy::app::PluginGroupBuilder) -> bevy::app::PluginGroupBuilder {
    if !on() {
        return g;
    }
    g.set(bevy::pbr::PbrPlugin { prepass_enabled: false, add_default_deferred_lighting_plugin: false, use_gpu_instance_buffer_builder: false, ..default() })
        .disable::<bevy::post_process::PostProcessPlugin>()
        .add(bevy::post_process::msaa_writeback::MsaaWritebackPlugin)
        .add(bevy::post_process::bloom::BloomPlugin)
        .disable::<bevy::anti_alias::AntiAliasPlugin>()
        .add(bevy::anti_alias::smaa::SmaaPlugin)
        .add(bevy::anti_alias::fxaa::FxaaPlugin)
}

/// A 3D view's opaque pass runs when it draws something, or when nothing after it would clear the view.
#[allow(clippy::type_complexity)]
fn opaque_3d_runs(
    view: ViewQuery<(&ExtractedCamera, &ExtractedView, Has<SkyboxPipelineId>, Has<OrderIndependentTransparencySettings>)>,
    opaque: Res<ViewBinnedRenderPhases<Opaque3d>>,
    mask: Res<ViewBinnedRenderPhases<AlphaMask3d>>,
    transmissive: Option<Res<ViewSortedRenderPhases<Transmissive3d>>>,
    transparent: Res<ViewSortedRenderPhases<Transparent3d>>,
) -> bool {
    let (camera, view, sky, oit) = view.into_inner();
    let k = &view.retained_view_entity;
    let draws = sky
        || opaque.get(k).is_some_and(|p| !p.is_empty())
        || mask.get(k).is_some_and(|p| !p.is_empty())
        || transmissive.is_some_and(|t| t.get(k).is_some_and(|p| !p.items.is_empty()));
    // order-independent transparency can skip its pass for a frame, so it keeps its clear here
    let later = !oit && transparent.get(k).is_some_and(|p| !p.items.is_empty());
    draws || (!matches!(camera.clear_color, ClearColorConfig::None) && !later)
}

#[derive(SystemParam)]
struct Phases2d<'w, 's> {
    opaque: Res<'w, ViewBinnedRenderPhases<Opaque2d>>,
    mask: Res<'w, ViewBinnedRenderPhases<AlphaMask2d>>,
    transparent: Res<'w, ViewSortedRenderPhases<Transparent2d>>,
    ui: Option<Res<'w, ViewSortedRenderPhases<TransparentUi>>>,
    ui_views: Query<'w, 's, &'static ExtractedView>,
}

impl Phases2d<'_, '_> {
    fn transparent_draws(&self, view: &ExtractedView) -> bool {
        self.transparent.get(&view.retained_view_entity).is_some_and(|p| !p.items.is_empty())
    }

    /// Like the 3D rule; the UI pass counts as a later pass when no tonemapping runs between them (it reads the
    /// image before the UI pass would clear it). Sphere's 2D cameras don't tonemap.
    fn opaque_runs(&self, camera: &ExtractedCamera, view: &ExtractedView, ui: Option<&UiCameraView>, tonemapping: Option<&Tonemapping>) -> bool {
        let k = &view.retained_view_entity;
        if self.opaque.get(k).is_some_and(|p| !p.is_empty()) || self.mask.get(k).is_some_and(|p| !p.is_empty()) {
            return true;
        }
        let tonemaps = camera.hdr && tonemapping.is_some_and(|t| !matches!(t, Tonemapping::None));
        let ui_draws = !tonemaps
            && ui.and_then(|u| self.ui_views.get(u.0).ok()).zip(self.ui.as_deref()).is_some_and(|(v, phases)| phases.get(&v.retained_view_entity).is_some_and(|p| !p.items.is_empty()));
        !matches!(camera.clear_color, ClearColorConfig::None) && !self.transparent_draws(view) && !ui_draws
    }
}

fn opaque_2d_runs(view: ViewQuery<(&ExtractedCamera, &ExtractedView, Option<&UiCameraView>, Option<&Tonemapping>)>, phases: Phases2d) -> bool {
    let (camera, view, ui, tonemapping) = view.into_inner();
    phases.opaque_runs(camera, view, ui, tonemapping)
}

/// Bevy's `prepare_core_2d_depth_textures` (MIT/Apache-2.0), giving depth only to 2D views with a pass that uses
/// it. Sphere's 2D cameras draw only UI, which has no depth, yet each got a full-size depth image (25 MB for the
/// overlay, up to 48 MB per panel atlas).
#[allow(clippy::type_complexity)]
fn prepare_2d_depth(
    mut commands: Commands,
    mut texture_cache: ResMut<TextureCache>,
    render_device: Res<RenderDevice>,
    phases: Phases2d,
    views: Query<(Entity, &ExtractedCamera, &ExtractedView, &Msaa, Option<&UiCameraView>, Option<&Tonemapping>, Has<ViewDepthStencilTexture>), With<Camera2d>>,
) {
    let mut textures = <HashMap<_, _>>::default();
    for (entity, camera, view, msaa, ui, tonemapping, has_depth) in &views {
        let k = &view.retained_view_entity;
        if !phases.opaque.contains_key(k) || !phases.transparent.contains_key(k) {
            continue;
        }
        let Some(size) = camera.physical_target_size else { continue };
        if !phases.opaque_runs(camera, view, ui, tonemapping) && !phases.transparent_draws(view) {
            if has_depth {
                commands.entity(entity).remove::<ViewDepthStencilTexture>();
            }
            continue;
        }
        let texture = textures
            .entry(camera.target.clone())
            .or_insert_with(|| {
                texture_cache.get(
                    &render_device,
                    TextureDescriptor {
                        label: Some("view_depth_texture"),
                        size: size.to_extents(),
                        mip_level_count: 1,
                        sample_count: msaa.samples(),
                        dimension: TextureDimension::D2,
                        format: CORE_2D_DEPTH_FORMAT,
                        usage: TextureUsages::RENDER_ATTACHMENT,
                        view_formats: &[],
                    },
                )
            })
            .clone();
        commands.entity(entity).insert(ViewDepthStencilTexture::new(texture, Some(0.0), None));
    }
}

/// Each active 3D camera's cluster grid, and whether a light, probe or decal fell in any cell, at the last extraction.
#[derive(Resource, Default)]
struct ClusterLayout {
    grids: Vec<(Entity, UVec3)>,
    changed: bool,
}

fn cluster_layout(views: Extract<Query<(Entity, &Clusters, &Camera)>>, mut layout: ResMut<ClusterLayout>) {
    let mut grids = Vec::with_capacity(layout.grids.len());
    let mut objects = false;
    for (entity, clusters, camera) in &views {
        if !camera.is_active {
            continue;
        }
        grids.push((entity, clusters.dimensions));
        if let ClusterableObjects::Cpu(cells) = &clusters.clusterable_objects {
            objects |= cells.iter().any(|c| {
                let n = c.counts;
                n.point_lights + n.spot_lights + n.rect_lights + n.reflection_probes + n.irradiance_volumes + n.decals > 0
            });
        }
    }
    layout.changed = objects || grids != layout.grids;
    layout.grids = grids;
}

/// The cluster buffers only change with the grid or what's in it; a new view gets its first ones.
fn clusters_stale(
    layout: Res<ClusterLayout>,
    settings: Res<GlobalClusterSettings>,
    missing: Query<(), (With<ExtractedClusterableObjects>, Without<ViewClusterBindings>)>,
) -> bool {
    settings.gpu_clustering.is_none() && (layout.changed || !missing.is_empty())
}
