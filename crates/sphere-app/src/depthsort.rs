//! Bevy 0.20 keeps each view's transparent draws from frame to frame, and an entry's sort position (its mesh's
//! centre in the world) is taken when it is queued, which happens only when its mesh or material changes. Sphere's
//! panels and glow keep their materials while they move, so a part that moved after its last change went on
//! sorting from where it was then, and overlapping parts could draw in the wrong order (in Files, a pane's tick
//! marks went under the glass of the pane below). Before the sort, every entry's position is set to where its mesh
//! is now, which is how each part sorted when every material changed every frame.
use bevy::core_pipeline::core_3d::{Transparent3d, TransparentSortingInfo3d};
use bevy::pbr::{get_mesh_instance_world_from_local, MeshInputUniform, MeshUniform, RenderMeshInstances};
use bevy::prelude::*;
use bevy::render::batching::gpu_preprocessing::BatchedInstanceBuffers;
use bevy::render::mesh::RenderMesh;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_phase::{sort_phase_system, ViewSortedRenderPhases};
use bevy::render::{Render, RenderApp, RenderSystems};

pub struct DepthSortPlugin;

impl Plugin for DepthSortPlugin {
    fn build(&self, app: &mut App) {
        if let Some(r) = app.get_sub_app_mut(RenderApp) {
            r.add_systems(Render, follow.in_set(RenderSystems::PhaseSort).before(sort_phase_system::<Transparent3d>));
        }
    }
}

fn follow(
    mut phases: ResMut<ViewSortedRenderPhases<Transparent3d>>,
    instances: Res<RenderMeshInstances>,
    meshes: Res<RenderAssets<RenderMesh>>,
    batched: Option<Res<BatchedInstanceBuffers<MeshUniform, MeshInputUniform>>>,
) {
    for phase in phases.values_mut() {
        for item in phase.items.values_mut() {
            let TransparentSortingInfo3d::Sorted { mesh_center, .. } = &mut item.sorting_info else { continue };
            let e = item.entity.1;
            let Some(q) = instances.render_mesh_queue_data(e) else { continue };
            let Some(mesh) = meshes.get(q.mesh_asset_id()) else { continue };
            *mesh_center = get_mesh_instance_world_from_local(e, q.current_uniform_index, &instances, batched.as_deref()).transform_point3(mesh.aabb_center);
        }
    }
}
