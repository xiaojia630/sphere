//! Lays out only the UI roots whose layout inputs changed.
//!
//! Bevy's `ui_layout_system` visits every UI root every frame: it syncs the root's children, asks taffy for a layout
//! (a cache hit when nothing changed, but taffy still rounds every node again) and rewrites the geometry of every
//! node. Sphere has some 700 UI nodes in a handful of roots (the overlay, one per panel atlas) and nearly all of them
//! sit idle, so that walk was mostly spent confirming that nothing moved.
//!
//! Here the system is split in two that run back to back as one system (`pipe`), so both see exactly the changes
//! Bevy's own would see:
//!  1. Bevy's `ui_layout_system` itself, its root queries narrowed to match nothing. It still does what comes before
//!     the per-root loop: syncing every changed node and its `ContentSize` into taffy (`ContentSize::measure` is
//!     private to bevy_ui, so that part cannot be copied) and the removals.
//!  2. `lay_out_ui_roots`, Bevy's per-root loop, run only for the roots `Gate` flags as changed.
//!
//! The per-root code is copied from Bevy: bevy_ui 0.20.0, `src/layout/mod.rs`, `ui_layout_system` (the `fixed_node_changes`
//! set and the loop over roots, lines 210-293, and `update_uinode_geometry_recursive`, lines 295-521).
//! Copyright (c) Bevy Contributors, https://bevy.org, MIT OR Apache-2.0. It differs only where bevy_ui's private items
//! are out of reach from here, and in which roots it visits:
//!  - the loop visits the roots the gate flagged (every root when `Dirty::audit` is on) instead of all of them;
//!  - `ui_surface.entity_to_taffy.contains_key(&entity)` is `ui_surface.is_node(entity)`, and the root's
//!    `ComputedUiRenderTargetInfo` is read through its accessors instead of its `pub(crate)` fields;
//!  - taffy is not a dependency of this crate, so `taffy_rect_to_border_rect` is written out where it was used;
//!  - the geometry function takes the query as the `GeometryQuery` alias and carries a clippy allow;
//!  - the code for bevy_ui's `ghost_nodes` feature is left out (it is not enabled, and needs more of `UiSurface` than is public).
//!
//! `SPHERE_LEAN=0` leaves Bevy's system in place. The gate is argued correct on `Gate`, and the tests below run a
//! stock and a gated app side by side through the same changes, asserting the same `ComputedNode` and
//! `UiGlobalTransform` on every node, every frame.
use bevy::{
    ecs::{
        entity::EntityHashSet,
        query::QueryBuilder,
        schedule::ScheduleCleanupPolicy,
        system::{ParamBuilder, QueryParamBuilder, SystemParam, SystemParamBuilder},
    },
    math::Affine2,
    prelude::*,
    text::{ComputedTextBlock, FontCx},
    ui::{
        experimental::{UiChildren, UiRootNodes},
        ui_layout_system,
        ui_surface::UiSurface,
        ContentSize, UiSystems,
    },
};

/// `SPHERE_LEAN=0` (for measuring) keeps Bevy's layout system.
fn on() -> bool {
    !std::env::var("SPHERE_LEAN").is_ok_and(|v| v == "0")
}

/// Swaps Bevy's `ui_layout_system` for the gated one. Add it last: it acts in `finish()`, once `UiPlugin` has added the system.
pub struct UiLayoutPlugin;

impl Plugin for UiLayoutPlugin {
    fn build(&self, _app: &mut App) {}

    fn finish(&self, app: &mut App) {
        if on() {
            // debug builds lay everything out anyway and complain when the gate was wrong
            install(app, cfg!(debug_assertions));
        }
    }
}

/// Replaces `ui_layout_system` in `PostUpdate` with Bevy's own system, narrowed to the node sync, piped into `lay_out_ui_roots`;
/// false (and nothing changed) when the schedule has no such system.
fn install(app: &mut App, audit: bool) -> bool {
    match app.remove_systems_in_set(PostUpdate, ui_layout_system, ScheduleCleanupPolicy::RemoveSystemsOnly) {
        Ok(n) if n > 0 => {}
        found => {
            warn!("ui layout: Bevy's ui_layout_system not found ({found:?}); leaving the UI layout as it is");
            return false;
        }
    }
    // One builder per parameter of `ui_layout_system`, in order: the two root queries get a filter nothing satisfies,
    // so its loop over roots has none to visit.
    let stock = (
        ParamBuilder,
        QueryParamBuilder::new(|b: &mut QueryBuilder<Entity, (With<Node>, Without<ChildOf>)>| {
            b.with::<NoRoots>();
        }),
        QueryParamBuilder::new(|b: &mut QueryBuilder<Entity, (With<FixedNode>, With<ChildOf>)>| {
            b.with::<NoRoots>();
        }),
        ParamBuilder,
        ParamBuilder,
        ParamBuilder,
        ParamBuilder,
        ParamBuilder,
        ParamBuilder,
        ParamBuilder,
        ParamBuilder,
        ParamBuilder,
    )
        .build_system(ui_layout_system);
    // Piped, not chained: nothing can run between the halves, so what the second finds changed is what the first has just
    // synced (an entity added in between would never reach taffy), as in Bevy's one system.
    app.insert_resource(Dirty { audit, ..default() }).add_systems(
        PostUpdate,
        stock
            .pipe(lay_out_ui_roots)
            .with_name("sphere_app::uilayout::ui_layout_system")
            .in_set(UiSystems::Layout)
            .ambiguous_with(bevy::sprite::update_text2d_layout),
    );
    true
}

/// Nothing has it: the filter that makes Bevy's own root loop empty.
#[derive(Component)]
struct NoRoots;

/// What the gate decided this frame.
#[derive(Resource, Default)]
struct Dirty {
    /// roots with a changed layout input: the whole pass (children, taffy, geometry)
    full: EntityHashSet,
    /// roots where only inputs of the geometry pass changed (the ticker and the toasts move by `UiTransform`): that pass alone
    geo: EntityHashSet,
    /// every root is laid out anyway, and a root the gate skipped panics if that changed anything
    audit: bool,
}

/// Finds the roots to lay out. Bevy 0.20.0's `ui_layout_system` reads these, and nothing else, from the world:
///  - synced into taffy as a node's style, measure or available space (Bevy's first loop over every node): `Node`,
///    `EmSize`, `ContentSize`, `ComputedUiRenderTargetInfo` (also the root's size and scale for the geometry), and
///    the resource `RemSize` (any change of it: every root, like Bevy);
///  - the tree: `Children` and `ChildOf`, and the removal of `Node`, `ChildOf` and `Children` (the root is found
///    from the entity if it still exists; if it was despawned its parent lost it, so that parent has `Changed<Children>`
///    or a removal of its own, and a root that was despawned has nothing left to lay out);
///    a new node has `Changed<Node>`, so `Added<Node>` needs no entry of its own;
///  - `ComputedUiTargetCamera` is not read by the layout; the target info that follows from it is. It is listed anyway:
///    a root that is retargeted is never skipped;
///  - only the geometry pass reads `UiTransform`, `LayoutConfig`, `Outline`, `ScrollPosition` and `IgnoreScroll`;
///  - `FixedNode` makes its own roots, laid out every frame as Bevy does; while any exists (or one was removed) every root is.
///
/// A root that none of these changed under needs nothing: its taffy tree is clean (taffy returns the cached layout and
/// rounds it to the same numbers), and every write of the geometry pass is guarded by `!=`, so the pass writes nothing.
/// Changes are found since this system last ran, the window Bevy's own system has.
#[derive(SystemParam)]
#[allow(clippy::type_complexity)]
struct Gate<'w, 's> {
    dirty: ResMut<'w, Dirty>,
    parents: Query<'w, 's, &'static ChildOf>,
    layout: Query<
        'w,
        's,
        Entity,
        (
            With<Node>,
            Or<(
                Changed<Node>,
                Changed<EmSize>,
                Changed<ContentSize>,
                Changed<ComputedUiRenderTargetInfo>,
                Changed<ComputedUiTargetCamera>,
                Changed<Children>,
                Changed<ChildOf>,
            )>,
        ),
    >,
    geometry: Query<
        'w,
        's,
        Entity,
        (
            With<Node>,
            Or<(
                Changed<UiTransform>,
                Changed<LayoutConfig>,
                Changed<Outline>,
                Changed<ScrollPosition>,
                Changed<IgnoreScroll>,
            )>,
        ),
    >,
    fixed: Query<'w, 's, (), With<FixedNode>>,
    removed_nodes: RemovedComponents<'w, 's, Node>,
    removed_parents: RemovedComponents<'w, 's, ChildOf>,
    removed_children: RemovedComponents<'w, 's, Children>,
    removed_fixed: RemovedComponents<'w, 's, FixedNode>,
}

impl Gate<'_, '_> {
    fn collect(&mut self, roots: &UiRootNodes, rem_size_changed: bool) {
        let Self { dirty, parents, layout, geometry, fixed, removed_nodes, removed_parents, removed_children, removed_fixed } = self;
        let Dirty { full, geo, .. } = &mut **dirty;
        full.clear();
        geo.clear();
        if removed_fixed.read().count() > 0 || !fixed.is_empty() || rem_size_changed {
            full.extend(roots.iter());
            removed_nodes.clear();
            removed_parents.clear();
            removed_children.clear();
            return;
        }
        // the top of the `ChildOf` chain, if it is a UI root (Bevy's roots are the `Node`s with no parent)
        let root_of = |entity: Entity| Some(parents.root_ancestor(entity)).filter(|top| roots.contains(*top));
        for entity in layout.iter().chain(removed_nodes.read()).chain(removed_parents.read()).chain(removed_children.read()) {
            full.extend(root_of(entity));
        }
        for entity in geometry.iter() {
            geo.extend(root_of(entity).filter(|root| !full.contains(root)));
        }
    }
}

type GeometryQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut ComputedNode,
        &'static UiTransform,
        &'static mut UiGlobalTransform,
        &'static Node,
        &'static EmSize,
        Option<&'static LayoutConfig>,
        Option<&'static Outline>,
        Option<&'static ScrollPosition>,
        Option<&'static IgnoreScroll>,
        Has<FixedNode>,
    ),
>;

/// The rest of Bevy's `ui_layout_system`, for the roots that changed: the second half of the pipe.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn lay_out_ui_roots(
    mut ui_surface: ResMut<UiSurface>,
    mut gate: Gate,
    ui_root_node_query: UiRootNodes,
    fixed_nodes_query: Query<Entity, (With<FixedNode>, With<ChildOf>)>,
    ui_children: UiChildren,
    target_query: Query<&ComputedUiRenderTargetInfo>,
    added_node_query: Query<(), Added<Node>>,
    added_fixed_node_query: Query<Entity, Added<FixedNode>>,
    mut node_update_query: GeometryQuery,
    mut buffer_query: Query<&mut ComputedTextBlock>,
    mut font_system: ResMut<FontCx>,
    mut removed_fixed_nodes: RemovedComponents<FixedNode>,
    rem_size: Res<RemSize>,
) {
    gate.collect(&ui_root_node_query, rem_size.is_changed());
    let dirty = &*gate.dirty;

    // The roots where only the geometry pass has anything to do: it alone, on the taffy layout they already have.
    for &ui_root_entity in &dirty.geo {
        let Ok(computed_target) = target_query.get(ui_root_entity) else {
            continue;
        };
        update_uinode_geometry_recursive(
            ui_root_entity,
            ui_root_entity,
            &mut ui_surface,
            true,
            computed_target.physical_size().as_vec2(),
            Affine2::IDENTITY,
            &mut node_update_query,
            &ui_children,
            computed_target.scale_factor().recip(),
            Vec2::ZERO,
            Vec2::ZERO,
            *rem_size,
        );
    }

    // The roots the loop below will lay out although the gate did not flag them (all of them in the audit).
    let audited: Vec<Entity> = if dirty.audit { ui_root_node_query.iter().filter(|root| !dirty.full.contains(root)).collect() } else { Vec::new() };
    let before = snapshot(&audited, &ui_children, &node_update_query);

    let fixed_node_changes = added_fixed_node_query
        .iter()
        .chain(removed_fixed_nodes.read())
        .collect::<EntityHashSet>();

    for ui_root_entity in ui_root_node_query.iter().filter(|root| dirty.audit || dirty.full.contains(root)).chain(fixed_nodes_query.iter()) {
        fn update_children_recursively(
            ui_surface: &mut UiSurface,
            ui_children: &UiChildren,
            added_node_query: &Query<(), Added<Node>>,
            fixed_nodes_query: &Query<Entity, (With<FixedNode>, With<ChildOf>)>,
            fixed_node_changes: &EntityHashSet,
            entity: Entity,
        ) {
            let children_changed = ui_children.is_changed(entity)
                || ui_children.iter_ui_children(entity).any(|child| {
                    added_node_query.contains(child) || fixed_node_changes.contains(&child)
                });

            if ui_surface.is_node(entity)
                && (added_node_query.contains(entity) || children_changed)
            {
                ui_surface.update_children(
                    entity,
                    ui_children
                        .iter_ui_children(entity)
                        .filter(|entity| !fixed_nodes_query.contains(*entity)),
                );
            }

            for child in ui_children.iter_ui_children(entity) {
                if fixed_nodes_query.contains(child) {
                    continue;
                }
                update_children_recursively(
                    ui_surface,
                    ui_children,
                    added_node_query,
                    fixed_nodes_query,
                    fixed_node_changes,
                    child,
                );
            }
        }

        update_children_recursively(
            &mut ui_surface,
            &ui_children,
            &added_node_query,
            &fixed_nodes_query,
            &fixed_node_changes,
            ui_root_entity,
        );

        let Ok(computed_target) = target_query.get(ui_root_entity) else {
            warn!("UI root {ui_root_entity} not found");
            continue;
        };

        ui_surface.compute_layout(
            ui_root_entity,
            computed_target.physical_size(),
            &mut buffer_query,
            &mut font_system,
        );

        update_uinode_geometry_recursive(
            ui_root_entity,
            ui_root_entity,
            &mut ui_surface,
            true,
            computed_target.physical_size().as_vec2(),
            Affine2::IDENTITY,
            &mut node_update_query,
            &ui_children,
            computed_target.scale_factor().recip(),
            Vec2::ZERO,
            Vec2::ZERO,
            *rem_size,
        );
    }

    check(before, &node_update_query);

    // Returns the combined bounding box of the node and any of its overflowing children.
    #[allow(clippy::too_many_arguments)]
    fn update_uinode_geometry_recursive(
        root: Entity,
        entity: Entity,
        ui_surface: &mut UiSurface,
        inherited_use_rounding: bool,
        target_size: Vec2,
        mut inherited_transform: Affine2,
        node_update_query: &mut GeometryQuery,
        ui_children: &UiChildren,
        inverse_target_scale_factor: f32,
        parent_size: Vec2,
        parent_scroll_position: Vec2,
        rem_size: RemSize,
    ) {
        if let Ok((
            mut node,
            transform,
            mut global_transform,
            style,
            em_size,
            maybe_layout_config,
            maybe_outline,
            maybe_scroll_position,
            maybe_scroll_sticky,
            is_fixed_node,
        )) = node_update_query.get_mut(entity)
        {
            if is_fixed_node && root != entity {
                return;
            }

            let use_rounding = maybe_layout_config
                .map(|layout_config| layout_config.use_rounding)
                .unwrap_or(inherited_use_rounding);

            let Ok((layout, unrounded_size)) = ui_surface.get_layout(entity, use_rounding) else {
                return;
            };

            let layout_size = Vec2::new(layout.size.width, layout.size.height);

            // Taffy layout position of the top-left corner of the node, relative to its parent.
            let layout_location = Vec2::new(layout.location.x, layout.location.y);

            // If IgnoreScroll is set, parent scroll position is ignored along the specified axes.
            let effective_parent_scroll = maybe_scroll_sticky
                .map(|scroll_sticky| parent_scroll_position * Vec2::from(!scroll_sticky.0))
                .unwrap_or(parent_scroll_position);

            // The position of the center of the node relative to its top-left corner.
            let local_center =
                layout_location - effective_parent_scroll + 0.5 * (layout_size - parent_size);

            // only trigger change detection when the new values are different
            if node.size != layout_size
                || node.unrounded_size != unrounded_size
                || node.inverse_scale_factor != inverse_target_scale_factor
            {
                node.size = layout_size;
                node.unrounded_size = unrounded_size;
                node.inverse_scale_factor = inverse_target_scale_factor;
            }

            let content_size = Vec2::new(
                layout.scrollable_overflow_rect.right,
                layout.scrollable_overflow_rect.bottom,
            );
            if node.content_size != content_size {
                node.content_size = content_size;
            }

            let new_border = BorderRect {
                min_inset: Vec2::new(layout.border.left, layout.border.top),
                max_inset: Vec2::new(layout.border.right, layout.border.bottom),
            };
            if node.border != new_border {
                node.border = new_border;
            }
            let new_padding = BorderRect {
                min_inset: Vec2::new(layout.padding.left, layout.padding.top),
                max_inset: Vec2::new(layout.padding.right, layout.padding.bottom),
            };
            if node.padding != new_padding {
                node.padding = new_padding;
            }

            if node.em_size != *em_size {
                node.em_size = *em_size;
            }
            if node.rem_size != rem_size {
                node.rem_size = rem_size;
            }

            // Compute the node's new global transform
            let mut local_transform = transform.compute_affine(
                inverse_target_scale_factor.recip(),
                layout_size,
                target_size,
                *em_size,
                rem_size,
            );
            local_transform.translation += local_center;
            inherited_transform *= local_transform;

            if inherited_transform != **global_transform {
                *global_transform = inherited_transform.into();
            }

            // We don't trigger change detection for changes to border radius
            // unless the border radius actually changed
            let new_border_radius = style.border_radius.resolve(
                inverse_target_scale_factor.recip(),
                node.size,
                target_size,
                *em_size,
                rem_size,
            );
            if node.border_radius != new_border_radius {
                node.border_radius = new_border_radius;
            }

            if let Some(outline) = maybe_outline {
                // don't trigger change detection unless the outline actually changed
                let new_outline_width = if style.display != Display::None {
                    outline
                        .width
                        .resolve(
                            inverse_target_scale_factor.recip(),
                            node.size().x,
                            target_size,
                            *em_size,
                            rem_size,
                        )
                        .unwrap_or(0.)
                        .max(0.)
                } else {
                    0.
                };

                if node.outline_width != new_outline_width {
                    node.outline_width = new_outline_width;
                }

                let new_outline_offset = outline
                    .offset
                    .resolve(
                        inverse_target_scale_factor.recip(),
                        node.size().x,
                        target_size,
                        *em_size,
                        rem_size,
                    )
                    .unwrap_or(0.)
                    // Clamp outline offsets to at least the length of the node's shorter side
                    // Negative offset outlines can be useful to create thing like in-set focus indicators
                    .max(-0.5 * node.size.min_element());
                if node.outline_offset != new_outline_offset {
                    node.outline_offset = new_outline_offset;
                }
            } else if node.outline_width != 0. || node.outline_offset != 0. {
                node.outline_width = 0.;
                node.outline_offset = 0.;
            }

            let new_scrollbar_size =
                Vec2::new(layout.scrollbar_size.width, layout.scrollbar_size.height);
            if node.scrollbar_size != new_scrollbar_size {
                node.scrollbar_size = new_scrollbar_size;
            }

            let scroll_position: Vec2 = maybe_scroll_position
                .map(|scroll_pos| {
                    Vec2::new(
                        if style.overflow.x == OverflowAxis::Scroll {
                            scroll_pos.x * inverse_target_scale_factor.recip()
                        } else {
                            0.0
                        },
                        if style.overflow.y == OverflowAxis::Scroll {
                            scroll_pos.y * inverse_target_scale_factor.recip()
                        } else {
                            0.0
                        },
                    )
                })
                .unwrap_or_default();

            let clamped_scroll_position = scroll_position.clamp(
                Vec2::ZERO,
                Vec2::new(layout.scroll_width(), layout.scroll_height()),
            );

            let physical_scroll_position = clamped_scroll_position.floor();

            if node.scroll_position != physical_scroll_position {
                node.scroll_position = physical_scroll_position;
            }

            for child_uinode in ui_children.iter_ui_children(entity) {
                update_uinode_geometry_recursive(
                    root,
                    child_uinode,
                    ui_surface,
                    use_rounding,
                    target_size,
                    inherited_transform,
                    node_update_query,
                    ui_children,
                    inverse_target_scale_factor,
                    layout_size,
                    physical_scroll_position,
                    rem_size,
                );
            }
        }
    }
}

/// What the layout of the nodes under `roots` is now.
fn snapshot(roots: &[Entity], ui_children: &UiChildren, nodes: &GeometryQuery) -> Vec<(Entity, Entity, ComputedNode, UiGlobalTransform)> {
    let mut out = Vec::new();
    for &root in roots {
        let mut stack = vec![root];
        while let Some(entity) = stack.pop() {
            if let Ok((node, _, global, ..)) = nodes.get(entity) {
                out.push((root, entity, *node, *global));
            }
            stack.extend(ui_children.iter_ui_children(entity));
        }
    }
    out
}

/// Panics if laying out a root that the gate skipped (or only gave the geometry pass) changed any of its nodes.
fn check(before: Vec<(Entity, Entity, ComputedNode, UiGlobalTransform)>, nodes: &GeometryQuery) {
    for (root, entity, node, global) in before {
        let Ok((now, _, now_global, ..)) = nodes.get(entity) else {
            continue;
        };
        assert!(
            *now == node && *now_global == global,
            "ui layout gate: root {root} was not laid out, but doing so changed {entity}: {node:?} {global:?} became {:?} {:?}; \
             find the input that changed and add it to `Gate` in uilayout.rs",
            *now,
            *now_global
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::{
        app::{HierarchyPropagatePlugin, PropagateSet, TaskPoolPlugin},
        camera::{Camera, Camera2d, ComputedCameraValues, RenderTargetInfo, Viewport},
        ui::{sync_font_size_to_em_size, update::propagate_ui_target_cameras, LayoutContext},
    };

    const W: u32 = 1000;
    const H: u32 = 600;

    /// The UI half of Bevy's `UiPlugin` (its sets, the target camera and size propagation, the layout system), stock or swapped for the gated one.
    fn app(gated: bool, audit: bool) -> App {
        let mut app = App::new();
        app.add_plugins(TaskPoolPlugin::default());
        app.add_plugins(HierarchyPropagatePlugin::<ComputedUiTargetCamera>::new(PostUpdate));
        app.add_plugins(HierarchyPropagatePlugin::<ComputedUiRenderTargetInfo>::new(PostUpdate));
        app.init_resource::<UiScale>().init_resource::<UiSurface>().init_resource::<FontCx>().init_resource::<RemSize>();
        // a resource is an entity: the stock app gets the one the gated app does, so that both worlds hand out the same ids
        app.init_resource::<Dirty>();
        app.configure_sets(PostUpdate, (UiSystems::Prepare, UiSystems::Propagate, UiSystems::Content, UiSystems::Layout).chain());
        app.configure_sets(PostUpdate, PropagateSet::<ComputedUiTargetCamera>::default().in_set(UiSystems::Propagate));
        app.configure_sets(PostUpdate, PropagateSet::<ComputedUiRenderTargetInfo>::default().in_set(UiSystems::Propagate));
        app.add_systems(
            PostUpdate,
            (
                propagate_ui_target_cameras.in_set(UiSystems::Prepare),
                sync_font_size_to_em_size.in_set(UiSystems::Content),
                ui_layout_system.in_set(UiSystems::Layout),
            ),
        );
        if gated {
            assert!(install(&mut app, audit));
        }
        app.world_mut().spawn((
            Camera2d,
            Camera {
                computed: ComputedCameraValues { target_info: Some(RenderTargetInfo { physical_size: UVec2::new(W, H), scale_factor: 1.0 }), ..default() },
                viewport: Some(Viewport { physical_size: UVec2::new(W, H), ..default() }),
                ..default()
            },
        ));
        app
    }

    /// A stock app and a gated one, driven through the same changes.
    struct Pair {
        stock: App,
        gated: App,
    }

    impl Pair {
        fn new(audit: bool) -> Self {
            Self::of(app(false, false), app(true, audit))
        }

        fn of(stock: App, gated: App) -> Self {
            Self { stock, gated }
        }

        /// Runs `f` on both worlds, which must do the same in each (spawns get the same ids).
        fn both<R: PartialEq + std::fmt::Debug>(&mut self, mut f: impl FnMut(&mut World) -> R) -> R {
            let stock = f(self.stock.world_mut());
            let gated = f(self.gated.world_mut());
            assert_eq!(stock, gated, "the two worlds diverged");
            stock
        }

        /// One frame in each; every node must come out the same.
        fn step(&mut self, what: &str) {
            self.stock.update();
            self.gated.update();
            let (stock, gated) = (layout(&mut self.stock), layout(&mut self.gated));
            assert_eq!(stock.len(), gated.len(), "{what}: how many nodes");
            for (s, g) in stock.iter().zip(&gated) {
                assert_eq!(s, g, "{what}: layout of {}", s.0);
            }
        }

        /// The roots the gated app laid out last frame: whole, and geometry only.
        fn laid(&self) -> (Vec<Entity>, Vec<Entity>) {
            let dirty = self.gated.world().resource::<Dirty>();
            (sorted(dirty.full.iter().copied()), sorted(dirty.geo.iter().copied()))
        }

        /// Steps and checks which roots were laid out.
        fn expect(&mut self, what: &str, full: &[Entity], geo: &[Entity]) {
            self.step(what);
            assert_eq!(self.laid(), (sorted(full.iter().copied()), sorted(geo.iter().copied())), "{what}: roots laid out (whole, geometry only)");
        }
    }

    fn sorted(entities: impl Iterator<Item = Entity>) -> Vec<Entity> {
        let mut v: Vec<Entity> = entities.collect();
        v.sort();
        v
    }

    fn layout(app: &mut App) -> Vec<(Entity, ComputedNode, UiGlobalTransform)> {
        let mut q = app.world_mut().query::<(Entity, &ComputedNode, &UiGlobalTransform)>();
        let mut v: Vec<_> = q.iter(app.world()).map(|(e, n, g)| (e, *n, *g)).collect();
        v.sort_by_key(|(e, ..)| *e);
        v
    }

    fn child(w: &mut World, parent: Entity, node: Node) -> Entity {
        w.spawn((node, ChildOf(parent))).id()
    }

    /// A node sized by its content, as text is.
    fn leaf(w: &mut World, parent: Entity, size: Vec2) -> Entity {
        w.spawn((Node::default(), ContentSize::fixed_size(size), ChildOf(parent))).id()
    }

    fn resize(w: &mut World, size: UVec2, scale_factor: f32) {
        let mut cameras = w.query::<&mut Camera>();
        for mut camera in cameras.iter_mut(w) {
            camera.computed.target_info = Some(RenderTargetInfo { physical_size: size, scale_factor });
            camera.viewport = Some(Viewport { physical_size: size, ..default() });
        }
    }

    /// Three roots: `a` with a nested row and measured leaves, `b` and `c` placed absolutely.
    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Tree {
        a: Entity,
        a_px: Entity,
        a_pct: Entity,
        a_leaf: Entity,
        a_row: Entity,
        a_row_px: Entity,
        b: Entity,
        b_px: Entity,
        b_col: Entity,
        b_col_px: Entity,
        c: Entity,
    }

    fn build(w: &mut World) -> Tree {
        let absolute = |left: f32, size: f32| Node { position_type: PositionType::Absolute, left: px(left), top: px(20.0), width: px(size), height: px(size), ..default() };
        let a = w.spawn(Node { width: px(400.0), height: px(300.0), flex_direction: FlexDirection::Column, ..default() }).id();
        let a_px = child(w, a, Node { width: px(100.0), height: px(50.0), ..default() });
        let a_pct = child(w, a, Node { width: percent(50.0), height: percent(20.0), ..default() });
        let a_leaf = leaf(w, a, Vec2::new(60.0, 20.0));
        let a_row = child(w, a, Node { flex_grow: 1.0, padding: UiRect::all(px(3.0)), ..default() });
        let a_row_px = child(w, a_row, Node { width: px(20.5), height: px(20.5), ..default() });
        leaf(w, a_row, Vec2::new(30.0, 10.0));
        let b = w.spawn(absolute(500.0, 200.0)).id();
        let b_px = child(w, b, Node { width: px(50.0), height: px(50.0), ..default() });
        let b_col = child(w, b, Node { flex_direction: FlexDirection::Column, ..default() });
        let b_col_px = child(w, b_col, Node { width: px(10.0), height: px(10.0), ..default() });
        let c = w.spawn(absolute(720.0, 100.0)).id();
        w.entity_mut(c).insert(UiTransform { rotation: Rot2::degrees(30.0), ..default() });
        Tree { a, a_px, a_pct, a_leaf, a_row, a_row_px, b, b_px, b_col, b_col_px, c }
    }

    fn set_width(w: &mut World, e: Entity, width: f32) {
        w.get_mut::<Node>(e).unwrap().width = px(width);
    }

    fn translate(w: &mut World, e: Entity, x: f32) {
        w.get_mut::<UiTransform>(e).unwrap().translation = Val2::px(x, 1.0);
    }

    #[test]
    fn every_kind_of_change_matches_stock_and_only_changed_roots_are_laid_out() {
        let mut p = Pair::new(false);
        let t = p.both(build);
        p.expect("first frame", &[t.a, t.b, t.c], &[]);
        for i in 0..3 {
            p.expect(&format!("idle {i}"), &[], &[]);
        }

        p.both(|w| set_width(w, t.a_px, 150.0));
        p.expect("node size", &[t.a], &[]);
        p.both(|w| *w.get_mut::<ContentSize>(t.a_leaf).unwrap() = ContentSize::fixed_size(Vec2::new(80.0, 44.0)));
        p.expect("measured size (text changed)", &[t.a], &[]);
        p.both(|w| w.get_mut::<ContentSize>(t.a_leaf).unwrap().clear());
        p.expect("measure cleared", &[t.a], &[]);
        p.both(|w| w.get_mut::<Node>(t.b_col).unwrap().display = Display::None);
        p.expect("hidden", &[t.b], &[]);
        p.both(|w| w.get_mut::<Node>(t.b_col).unwrap().display = Display::Flex);
        p.expect("shown again", &[t.b], &[]);
        p.expect("idle after", &[], &[]);

        // children added, removed, moved
        let added = p.both(|w| child(w, t.a_row, Node { width: px(7.0), height: px(7.0), ..default() }));
        p.expect("child added", &[t.a], &[]);
        p.both(|w| w.entity_mut(added).despawn());
        p.expect("child removed", &[t.a], &[]);
        p.both(|w| w.entity_mut(t.b_col_px).despawn());
        p.expect("last child removed (its parent loses Children)", &[t.b], &[]);
        p.both(|w| {
            w.entity_mut(t.a_row_px).insert(ChildOf(t.b_col));
        });
        p.expect("moved between roots", &[t.a, t.b], &[]);
        p.both(|w| {
            w.entity_mut(t.a_row_px).insert(ChildOf(t.a));
        });
        p.expect("moved back", &[t.a, t.b], &[]);
        p.both(|w| {
            w.entity_mut(t.a_row_px).remove::<ChildOf>();
        });
        p.expect("a node becomes a root", &[t.a, t.a_row_px], &[]);
        p.both(|w| {
            w.entity_mut(t.a_row_px).insert(ChildOf(t.a));
        });
        p.expect("and a child again", &[t.a], &[]);

        // a FixedNode is a root of its own, laid out every frame; while one exists every root is
        let fixed = p.both(|w| {
            let e = child(w, t.b, Node { width: px(40.0), height: px(40.0), ..default() });
            w.entity_mut(e).insert(FixedNode);
            e
        });
        p.expect("fixed node added", &[t.a, t.b, t.c], &[]);
        p.expect("fixed node idle", &[t.a, t.b, t.c], &[]);
        p.both(|w| translate(w, fixed, 4.0));
        p.expect("fixed node moved", &[t.a, t.b, t.c], &[]);
        p.both(|w| {
            w.entity_mut(fixed).remove::<FixedNode>();
        });
        p.expect("fixed node is a child again", &[t.a, t.b, t.c], &[]);
        p.both(|w| w.entity_mut(fixed).despawn());
        p.expect("and despawned", &[t.b], &[]);
        let fixed = p.both(|w| {
            let e = child(w, t.b, Node { width: px(40.0), height: px(40.0), ..default() });
            w.entity_mut(e).insert(FixedNode);
            e
        });
        p.expect("another fixed node", &[t.a, t.b, t.c], &[]);
        p.both(|w| w.entity_mut(fixed).despawn());
        p.expect("despawned while fixed (its removal counts)", &[t.a, t.b, t.c], &[]);
        p.expect("idle once more", &[], &[]);

        // a node that stays but is no longer one
        p.both(|w| {
            w.entity_mut(t.a_leaf).remove::<Node>();
        });
        p.expect("a node loses its Node", &[t.a], &[]);

        // roots spawned and despawned
        let d = p.both(|w| w.spawn(Node { width: px(30.0), height: px(30.0), ..default() }).id());
        p.expect("root spawned", &[d], &[]);
        p.both(|w| {
            w.entity_mut(d).remove::<Node>();
        });
        p.expect("a root loses its Node (nothing is left to lay out)", &[], &[]);
        p.both(|w| {
            w.entity_mut(d).insert(Node { width: px(31.0), height: px(31.0), ..default() });
        });
        p.expect("and gets one again", &[d], &[]);
        p.both(|w| w.entity_mut(d).despawn());
        p.expect("root despawned", &[], &[]);
        p.both(|w| w.entity_mut(t.c).despawn());
        p.expect("another root despawned", &[], &[]);

        // moving by UiTransform needs the geometry pass alone
        p.both(|w| translate(w, t.b_px, 12.0));
        p.expect("child moved", &[], &[t.b]);
        p.both(|w| translate(w, t.b, 3.0));
        p.expect("root moved", &[], &[t.b]);
        p.both(|w| {
            w.entity_mut(t.a_pct).insert(Outline::new(px(2.0), px(1.0), Color::WHITE));
        });
        p.expect("outline", &[], &[t.a]);
        p.both(|w| {
            w.entity_mut(t.a_pct).insert(LayoutConfig { use_rounding: false });
            translate(w, t.b_px, 99.0);
        });
        p.expect("rounding off, and a move elsewhere", &[], &[t.a, t.b]);
        p.both(|w| {
            set_width(w, t.a_px, 160.0);
            translate(w, t.a_px, 5.0);
        });
        p.expect("size and move in one root", &[t.a], &[]);
        p.both(|w| {
            w.get_mut::<Node>(t.a_row).unwrap().overflow = Overflow::scroll();
            w.entity_mut(t.a_row).insert(ScrollPosition(Vec2::new(0.0, 4.0)));
        });
        p.expect("scrolled", &[t.a], &[]);
        p.both(|w| w.get_mut::<ScrollPosition>(t.a_row).unwrap().0 = Vec2::new(0.0, 9.0));
        p.expect("scrolled further", &[], &[t.a]);
        p.expect("idle at the end", &[], &[]);

        // the target
        p.both(|w| resize(w, UVec2::new(W / 2, H), 1.0));
        p.expect("smaller target", &[t.a, t.b], &[]);
        p.both(|w| resize(w, UVec2::new(W, H), 2.0));
        p.expect("scale factor", &[t.a, t.b], &[]);
        p.both(|w| w.resource_mut::<UiScale>().0 = 1.5);
        p.expect("ui scale", &[t.a, t.b], &[]);
        p.both(|w| w.resource_mut::<RemSize>().0 = 20.0);
        p.expect("rem size", &[t.a, t.b], &[]);
        p.expect("idle at the very end", &[], &[]);
    }

    #[test]
    fn a_ticker_moving_every_frame_costs_the_geometry_pass_only() {
        let mut p = Pair::new(false);
        let t = p.both(build);
        p.step("first frame");
        for i in 0..40 {
            p.both(|w| translate(w, t.b_px, i as f32 * 1.75));
            p.expect(&format!("ticker {i}"), &[], &[t.b]);
        }
    }

    /// A tiny deterministic generator (xorshift), so a failing seed replays.
    struct Rng(u64);

    impl Rng {
        fn below(&mut self, n: usize) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 % n as u64) as usize
        }
    }

    fn random_val(rng: &mut Rng) -> Val {
        match rng.below(5) {
            0 | 1 => Val::Auto,
            2 => px(5.0 + rng.below(90) as f32 + rng.below(2) as f32 * 0.5),
            3 => percent(10.0 + rng.below(80) as f32),
            _ => Val::Vw(1.0 + rng.below(9) as f32),
        }
    }

    fn random_node(rng: &mut Rng) -> Node {
        Node {
            width: random_val(rng),
            height: random_val(rng),
            margin: UiRect::all(px(rng.below(5) as f32)),
            padding: UiRect::all(px(rng.below(4) as f32 * 0.5)),
            flex_grow: rng.below(3) as f32,
            flex_wrap: if rng.below(4) == 0 { FlexWrap::Wrap } else { FlexWrap::NoWrap },
            flex_direction: if rng.below(2) == 0 { FlexDirection::Row } else { FlexDirection::Column },
            position_type: if rng.below(8) == 0 { PositionType::Absolute } else { PositionType::Relative },
            left: if rng.below(3) == 0 { px(rng.below(50) as f32) } else { Val::Auto },
            display: if rng.below(9) == 0 { Display::None } else { Display::Flex },
            overflow: if rng.below(9) == 0 { Overflow::scroll() } else { Overflow::visible() },
            ..default()
        }
    }

    fn descendants(w: &World, e: Entity, out: &mut Vec<Entity>) {
        out.push(e);
        for &c in w.get::<Children>(e).map(|c| &c[..]).unwrap_or(&[]) {
            descendants(w, c, out);
        }
    }

    /// Changes things at random, frame after frame, and the gated app must stay identical to the stock one.
    /// The sum of what the gate laid out is checked to be far less than everything, or the gate could be doing nothing.
    fn fuzz(seed: u64, audit: bool) {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let mut p = Pair::new(audit);
        let mut nodes: Vec<Entity> = Vec::new();
        for _ in 0..3 {
            let root = p.both(|w| w.spawn(Node { width: px(300.0), height: px(200.0), ..default() }).id());
            nodes.push(root);
        }
        for _ in 0..30 {
            let parent = nodes[rng.below(nodes.len())];
            let n = random_node(&mut rng);
            let size = Vec2::new(4.0 + rng.below(40) as f32, 4.0 + rng.below(20) as f32);
            let measured = rng.below(4) == 0;
            nodes.push(p.both(|w| {
                let e = child(w, parent, n.clone());
                if measured {
                    w.entity_mut(e).insert(ContentSize::fixed_size(size));
                }
                e
            }));
        }
        p.step("start");
        let (mut laid, mut possible) = (0, 0);
        for frame in 0..400 {
            for _ in 0..rng.below(3) {
                let e = nodes[rng.below(nodes.len())];
                let is_root = p.stock.world().get::<ChildOf>(e).is_none();
                // the rarer changes only happen on some of their rolls
                let (roll, coin) = (rng.below(20), rng.below(4) == 0);
                match roll {
                    0..=3 => {
                        let n = random_node(&mut rng);
                        p.both(|w| *w.get_mut::<Node>(e).unwrap() = n.clone());
                    }
                    4 => {
                        let size = Vec2::new(4.0 + rng.below(40) as f32, 4.0 + rng.below(20) as f32);
                        let clear = rng.below(4) == 0;
                        p.both(|w| *w.get_mut::<ContentSize>(e).unwrap() = if clear { ContentSize::default() } else { ContentSize::fixed_size(size) });
                    }
                    5 | 6 => {
                        let n = random_node(&mut rng);
                        let size = Vec2::new(4.0 + rng.below(40) as f32, 4.0 + rng.below(20) as f32);
                        let measured = rng.below(3) == 0;
                        nodes.push(p.both(|w| {
                            let c = child(w, e, n.clone());
                            if measured {
                                w.entity_mut(c).insert(ContentSize::fixed_size(size));
                            }
                            c
                        }));
                    }
                    7 if !is_root => {
                        let mut gone = Vec::new();
                        descendants(p.stock.world(), e, &mut gone);
                        p.both(|w| w.entity_mut(e).despawn());
                        nodes.retain(|n| !gone.contains(n));
                    }
                    8 if !is_root => {
                        let target = nodes[rng.below(nodes.len())];
                        let mut below = Vec::new();
                        descendants(p.stock.world(), e, &mut below);
                        if !below.contains(&target) {
                            p.both(|w| {
                                w.entity_mut(e).insert(ChildOf(target));
                            });
                        }
                    }
                    9 | 10 => {
                        let (x, y, s, r) = (rng.below(30) as f32, rng.below(30) as f32, 0.5 + rng.below(4) as f32 * 0.5, rng.below(90) as f32);
                        p.both(|w| *w.get_mut::<UiTransform>(e).unwrap() = UiTransform { translation: Val2::px(x, y), scale: Vec2::splat(s), rotation: Rot2::degrees(r) });
                    }
                    11 if coin => {
                        let (size, scale) = (UVec2::new(300 + rng.below(900) as u32, 200 + rng.below(500) as u32), [1.0, 1.5, 2.0][rng.below(3)]);
                        p.both(|w| resize(w, size, scale));
                    }
                    12 if coin => {
                        let v = 1.0 + rng.below(3) as f32 * 0.5;
                        p.both(|w| w.resource_mut::<UiScale>().0 = v);
                    }
                    13 if coin => {
                        let v = 12.0 + rng.below(8) as f32;
                        p.both(|w| w.resource_mut::<RemSize>().0 = v);
                    }
                    14 => {
                        let rounding = rng.below(2) == 0;
                        p.both(|w| {
                            w.entity_mut(e).insert(LayoutConfig { use_rounding: rounding });
                        });
                    }
                    15 => {
                        let (width, offset) = (rng.below(5) as f32, rng.below(5) as f32 - 2.0);
                        p.both(|w| {
                            w.entity_mut(e).insert(Outline::new(px(width), px(offset), Color::WHITE));
                        });
                    }
                    16 => {
                        let at = Vec2::new(rng.below(30) as f32, rng.below(30) as f32);
                        p.both(|w| {
                            w.entity_mut(e).insert(ScrollPosition(at));
                        });
                    }
                    17 if is_root && nodes.len() > 10 => {
                        // a root and everything under it goes; another comes
                        let mut gone = Vec::new();
                        descendants(p.stock.world(), e, &mut gone);
                        p.both(|w| w.entity_mut(e).despawn());
                        nodes.retain(|n| !gone.contains(n));
                        nodes.push(p.both(|w| w.spawn(Node { width: px(250.0), height: px(250.0), ..default() }).id()));
                    }
                    _ => {}
                }
            }
            p.step(&format!("seed {seed} frame {frame}"));
            let (full, geo) = p.laid();
            laid += full.len() + geo.len();
            possible += nodes.iter().filter(|n| p.stock.world().get::<ChildOf>(**n).is_none()).count();
        }
        assert!(laid * 2 < possible, "seed {seed}: the gate laid out {laid} of {possible} roots; it should skip most");
    }

    #[test]
    fn random_changes_match_stock() {
        for seed in 1..=8 {
            fuzz(seed, false);
        }
    }

    #[test]
    fn random_changes_pass_the_audit() {
        for seed in 101..=104 {
            fuzz(seed, true);
        }
    }

    #[test]
    #[should_panic(expected = "ui layout gate")]
    fn the_audit_catches_a_change_the_gate_missed() {
        let mut app = app(true, true);
        let t = build(app.world_mut());
        app.update();
        app.update();
        // Restyle a node inside taffy, not through its `Node`: nothing the gate watches changes, yet the layout would.
        app.world_mut().resource_mut::<UiSurface>().upsert_node(&LayoutContext::DEFAULT, t.a_px, &Node { width: px(333.0), height: px(50.0), ..default() }, None);
        app.update();
    }

    #[test]
    fn without_bevys_system_nothing_is_installed() {
        assert!(!install(&mut App::new(), false));
    }

    /// Bevy's own `UiPlugin`, with the rest of `DefaultPlugins` (no window, no GPU), and `UiLayoutPlugin` added after all of it as `main` does, or not.
    fn real_app(gated: bool) -> App {
        use bevy::{
            camera::RenderTarget,
            log::LogPlugin,
            render::{settings::WgpuSettings, RenderPlugin},
            ui::IsDefaultUiCamera,
            window::ExitCondition,
            winit::WinitPlugin,
        };
        let mut app = App::new();
        app.add_plugins(
            DefaultPlugins
                .set(RenderPlugin { render_creation: WgpuSettings { backends: None, ..default() }.into(), ..default() })
                .set(WindowPlugin { primary_window: None, exit_condition: ExitCondition::DontExit, ..default() })
                .disable::<WinitPlugin>()
                .disable::<LogPlugin>(),
        );
        app.init_resource::<Dirty>();
        if gated {
            app.add_plugins(UiLayoutPlugin);
        }
        app.finish();
        app.cleanup();
        app.world_mut().spawn((Camera2d, RenderTarget::None { size: UVec2::new(W, H) }, IsDefaultUiCamera));
        app
    }

    /// The real apps, one update each, and then the same next entity id in both: the gated one set its schedule up in `finish`, the stock one on its first update, and that hands out ids.
    fn real_pair() -> Pair {
        let (mut stock, mut gated) = (real_app(false), real_app(true));
        stock.update();
        gated.update();
        let (mut s, mut g) = (stock.world_mut().spawn_empty().id(), gated.world_mut().spawn_empty().id());
        while s < g {
            s = stock.world_mut().spawn_empty().id();
        }
        while g < s {
            g = gated.world_mut().spawn_empty().id();
        }
        Pair::of(stock, gated)
    }

    #[test]
    fn with_bevys_own_ui_plugin_the_layout_matches_stock_too() {
        if !on() {
            return; // SPHERE_LEAN=0 leaves Bevy's system alone, so there is nothing to compare
        }
        let mut p = real_pair();
        let t = p.both(build);
        // text is what sets a ContentSize in a real UI
        let (title, line) = p.both(|w| (w.spawn((Text::new("SPHERE"), ChildOf(t.a))).id(), w.spawn((Text::new("online"), ChildOf(t.b_col))).id()));
        for i in 0..8 {
            p.step(&format!("settling {i}"));
        }
        for i in 0..3 {
            p.expect(&format!("idle {i}"), &[], &[]);
        }
        p.both(|w| w.get_mut::<Text>(title).unwrap().0 = "SPHERE ONLINE".into());
        p.expect("text edited", &[t.a], &[]);
        p.expect("idle after the text", &[], &[]);
        p.both(|w| w.get_mut::<TextFont>(line).unwrap().font_size = bevy::text::FontSize::Px(31.0));
        p.expect("font size", &[t.b], &[]);
        p.both(|w| translate(w, t.b_px, 6.0));
        p.expect("moved", &[], &[t.b]);
        p.both(|w| w.entity_mut(line).despawn());
        p.expect("text despawned", &[t.b], &[]);
        p.expect("idle at the end", &[], &[]);
    }
}
