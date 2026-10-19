//! The additive "glow" material: WGSL/WESL port of the prototype's line, wall, sphere and label shaders.
use bevy::{
    asset::{embedded_asset, uuid_handle, AssetEventSystems},
    mesh::{MeshVertexAttribute, MeshVertexBufferLayoutRef},
    pbr::{MaterialPipeline, MaterialPipelineKey},
    platform::collections::AlignedVec,
    prelude::*,
    reflect::TypePath,
    render::{
        render_resource::{
            AsBindGroup, BlendComponent, BlendFactor, BlendOperation, BlendState, BufferUsages, CompareFunction,
            RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError, VertexFormat,
        },
        storage::{ShaderBuffer, ShaderBufferData},
    },
    shader::ShaderRef,
};

/// (wall, back, ang, k) for rings; (k,..) for lines; (corner.xy, k) for dots; (u, v) for text.
pub const ATTR_HOLO: MeshVertexAttribute = MeshVertexAttribute::new("HoloAttr", 988540917, VertexFormat::Float32x4);

/// How many materials can animate through the shared buffer. Its size never changes: a resized buffer is a new GPU
/// buffer, and then every material's bind group would have to be built again.
pub const NSLOT: usize = 1024;
/// One slot in the buffer: an `Anim`, three vec4.
const SLOT_BYTES: usize = 48;
/// `GlowUniform::slot` of a material that keeps its own `color`, `a` and `d`.
pub const NO_SLOT: u32 = u32::MAX;
/// The shared buffer of every animated value; a fixed handle, so `GlowMaterial::new` can bind it without a resource.
pub const GLOW_SLOTS: Handle<ShaderBuffer> = uuid_handle!("7f701baa-e405-4b3b-82a5-322c645aca7c");

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Kind {
    Ring,
    Line,
    Dots,
    Fres,
    Text,
    Backdrop,
    /// the voice spokes: a ring whose lengths the vertex shader animates (see `vs_spokes`)
    Spokes,
}

#[derive(Clone, Copy, Debug, PartialEq, ShaderType)]
pub struct GlowUniform {
    pub color: Vec4,
    pub c2: Vec4,
    pub a: Vec4, // alpha, wall, reveal, hi
    pub b: Vec4, // fade, depth, two, pinch
    pub c: Vec4, // rg, ang, pt_px, round
    pub d: Vec4, // t, iri
    /// where the shader takes `color`, `a` and `d` from: that slot of the shared buffer (see `GlowSlots`), and these
    /// fields are then ignored; `NO_SLOT`: these fields
    pub slot: u32,
}

#[derive(Asset, TypePath, AsBindGroup, Clone, Debug)]
#[bind_group_data(GlowKey)]
pub struct GlowMaterial {
    #[uniform(0)]
    pub u: GlowUniform,
    #[texture(1)]
    #[sampler(2)]
    pub tex: Option<Handle<Image>>,
    /// always the one shared buffer, `GLOW_SLOTS`
    #[storage(3, read_only)]
    pub slots: Handle<ShaderBuffer>,
    pub kind: Kind,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct GlowKey(Kind);
impl From<&GlowMaterial> for GlowKey {
    fn from(m: &GlowMaterial) -> Self {
        GlowKey(m.kind)
    }
}

impl GlowMaterial {
    pub fn new(kind: Kind, color: Vec4, alpha: f32) -> Self {
        Self {
            u: GlowUniform {
                color,
                c2: color,
                a: Vec4::new(alpha, 0.3, 1.0, 0.0),
                b: Vec4::new(1.0, 0.0, 0.0, 0.0),
                c: Vec4::new(1.0, 0.0, 3.0, 1.0),
                d: Vec4::ZERO,
                slot: NO_SLOT,
            },
            tex: None,
            slots: GLOW_SLOTS,
            kind,
        }
    }
}

// ---------------------------------------------------------------- animated values

/// The three vec4 of a material that animate: what a slot holds, and what the shader reads in place of the
/// material's own `color`, `a` and `d`.
#[derive(Clone, Copy)]
struct Anim {
    color: Vec4,
    a: Vec4,
    d: Vec4,
}

impl Anim {
    /// Whether the GPU would see no difference (by bits, so a NaN doesn't upload every frame).
    fn same(&self, o: &Anim) -> bool {
        let bits = |v: Vec4| v.to_array().map(f32::to_bits);
        bits(self.color) == bits(o.color) && bits(self.a) == bits(o.a) && bits(self.d) == bits(o.d)
    }
}

/// The values that change every frame, kept out of the materials. Writing a material makes Bevy prepare it again,
/// a new uniform buffer and bind group for each material every frame. So a material that animates has a slot in one
/// storage buffer shared by all of them, where the shader reads its `color`, `a` and `d`; this holds the values, and
/// `upload` sends them to the GPU (one write in place, only when one changed).
///
/// A slotted material's own `color`, `a` and `d` are never read again, so everything that animates one goes through
/// `update`. Nothing that animates is ever despawned, so a slot is never given back.
#[derive(Resource, Default)]
pub struct GlowSlots {
    vals: Vec<Anim>,
    /// a value changed since the last upload
    dirty: bool,
}

impl GlowSlots {
    /// Gives `m` the next slot, holding the `color`, `a` and `d` it has now, so it looks as it did. With every slot
    /// taken it keeps its own fields, and `update` writes the material itself.
    pub fn attach(&mut self, m: &mut GlowMaterial) {
        if self.vals.len() == NSLOT {
            return;
        }
        m.u.slot = self.vals.len() as u32;
        self.vals.push(Anim { color: m.u.color, a: m.u.a, d: m.u.d });
        self.dirty = true;
    }

    /// Runs `f` on a material's uniform and keeps the result: `color`, `a` and `d` of a slotted material in its
    /// slot, the rest in the material, and only when it differs (`get_mut` makes the renderer prepare the asset
    /// again). `f` sees the values as they are now, wherever they live.
    pub fn update(&mut self, mats: &mut Assets<GlowMaterial>, h: &Handle<GlowMaterial>, f: impl FnOnce(&mut GlowUniform)) {
        let Some(cur) = mats.get(h) else { return };
        let mut u = cur.u;
        let slot = u.slot as usize;
        let slotted = slot < self.vals.len();
        if slotted {
            let v = self.vals[slot];
            (u.color, u.a, u.d) = (v.color, v.a, v.d);
        }
        f(&mut u);
        if slotted {
            self.set(slot, Anim { color: u.color, a: u.a, d: u.d });
            // the material's own copies stay as they are, so they can't make it differ
            (u.color, u.a, u.d) = (cur.u.color, cur.u.a, cur.u.d);
        }
        if u != cur.u {
            if let Some(mut m) = mats.get_mut(h) {
                m.u = u;
            }
        }
    }

    fn set(&mut self, slot: usize, v: Anim) {
        if !self.vals[slot].same(&v) {
            self.vals[slot] = v;
            self.dirty = true;
        }
    }

    /// The buffer's contents: per slot its color, a and d, in order.
    fn words(&self) -> Vec<[f32; 4]> {
        let mut w = Vec::with_capacity(self.vals.len() * 3);
        for v in &self.vals {
            w.extend([v.color.to_array(), v.a.to_array(), v.d.to_array()]);
        }
        w
    }
}

/// The shared buffer, at its one size, empty.
fn slots_buffer() -> ShaderBuffer {
    ShaderBuffer {
        data: ShaderBufferData::Uninitialized((NSLOT * SLOT_BYTES) as u64),
        label: "glow slots".into(),
        buffer_usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
        ..default()
    }
}

/// Sends the slots to the GPU when a value changed. Only the slots in use go in the write, at the front of the full
/// size buffer, which Bevy writes in place (a same size buffer is the same GPU buffer, so no bind group notices).
/// Runs before the asset events are sent, so the renderer takes it in this very frame, like the material writes it
/// replaces.
fn upload(mut slots: ResMut<GlowSlots>, mut bufs: ResMut<Assets<ShaderBuffer>>) {
    if !slots.dirty {
        return;
    }
    let Some(mut buf) = bufs.get_mut(&GLOW_SLOTS) else { return };
    slots.dirty = false;
    buf.data = ShaderBufferData::Initialized { data: AlignedVec::from(slots.words()), buffer_size: (NSLOT * SLOT_BYTES) as u64 };
}

impl Material for GlowMaterial {
    fn vertex_shader() -> ShaderRef {
        "embedded://sphere_app/shaders/glow.wesl".into()
    }
    fn fragment_shader() -> ShaderRef {
        "embedded://sphere_app/shaders/glow.wesl".into()
    }
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
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
        key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        let vl = layout.0.get_layout(&[
            Mesh::ATTRIBUTE_POSITION.at_shader_location(0),
            ATTR_HOLO.at_shader_location(1),
        ])?;
        d.vertex.buffers = vec![vl];
        let (vs, fs) = match key.bind_group_data.0 {
            Kind::Ring => ("vs_ring", "fs_ring"),
            Kind::Line => ("vs_line", "fs_line"),
            Kind::Dots => ("vs_dots", "fs_dots"),
            Kind::Fres => ("vs_fres", "fs_fres"),
            Kind::Text => ("vs_text", "fs_text"),
            Kind::Backdrop => ("vs_text", "fs_backdrop"),
            Kind::Spokes => ("vs_spokes", "fs_ring"),
        };
        d.vertex.entry_point = Some(vs.into());
        let frag = d.fragment.as_mut().unwrap();
        frag.entry_point = Some(fs.into());
        // Three.js: AdditiveBlending, depthTest off, depthWrite off, DoubleSide.
        let additive = key.bind_group_data.0 != Kind::Backdrop;
        frag.targets[0].as_mut().unwrap().blend = Some(if additive {
            BlendState {
                color: BlendComponent { src_factor: BlendFactor::SrcAlpha, dst_factor: BlendFactor::One, operation: BlendOperation::Add },
                alpha: BlendComponent { src_factor: BlendFactor::Zero, dst_factor: BlendFactor::One, operation: BlendOperation::Add },
            }
        } else {
            // premultiplied: rgb is added light, alpha is the scanline darkening
            BlendState {
                color: BlendComponent { src_factor: BlendFactor::One, dst_factor: BlendFactor::OneMinusSrcAlpha, operation: BlendOperation::Add },
                alpha: BlendComponent { src_factor: BlendFactor::Zero, dst_factor: BlendFactor::One, operation: BlendOperation::Add },
            }
        });
        d.primitive.cull_mode = None;
        if let Some(ds) = d.depth_stencil.as_mut() {
            ds.depth_write_enabled = Some(false);
            ds.depth_compare = Some(CompareFunction::Always);
        }
        Ok(())
    }
}

pub struct GlowPlugin;
impl Plugin for GlowPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "shaders/glow.wesl");
        app.add_plugins(MaterialPlugin::<GlowMaterial>::default());
        add_slots(app);
    }
}

/// The slots, their shared buffer and the system that sends one to the other.
fn add_slots(app: &mut App) {
    app.init_resource::<GlowSlots>().add_systems(PostUpdate, upload.before(AssetEventSystems));
    app.world_mut().resource_mut::<Assets<ShaderBuffer>>().insert(&GLOW_SLOTS, slots_buffer()).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::{app::TaskPoolPlugin, asset::AssetPlugin, ecs::message::Messages};

    fn mat() -> GlowMaterial {
        GlowMaterial::new(Kind::Ring, Vec4::new(0.1, 0.2, 0.3, 1.0), 0.5)
    }

    /// The assets and systems `GlowPlugin` adds, without a renderer.
    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((TaskPoolPlugin::default(), AssetPlugin::default())).init_asset::<GlowMaterial>().init_asset::<ShaderBuffer>();
        add_slots(&mut app);
        app
    }

    fn slotted(app: &mut App) -> Handle<GlowMaterial> {
        let mut m = mat();
        app.world_mut().resource_mut::<GlowSlots>().attach(&mut m);
        app.world_mut().resource_mut::<Assets<GlowMaterial>>().add(m)
    }

    fn animate(app: &mut App, h: &Handle<GlowMaterial>, f: impl FnOnce(&mut GlowUniform)) {
        app.world_mut().resource_scope(|w, mut slots: Mut<GlowSlots>| slots.update(&mut w.resource_mut::<Assets<GlowMaterial>>(), h, f));
    }

    fn uniform(app: &App, h: &Handle<GlowMaterial>) -> GlowUniform {
        app.world().resource::<Assets<GlowMaterial>>().get(h).unwrap().u
    }

    /// The assets the renderer was told have changed since this was last asked (each once, if they changed in a row).
    fn modified<A: Asset>(app: &mut App) -> Vec<AssetId<A>> {
        let mut ids: Vec<_> = app.world_mut().resource_mut::<Messages<AssetEvent<A>>>().drain().filter_map(|e| if let AssetEvent::Modified { id } = e { Some(id) } else { None }).collect();
        ids.dedup();
        ids
    }

    #[test]
    fn slots_go_out_in_order_and_start_with_what_the_material_had() {
        let mut slots = GlowSlots::default();
        let (mut a, mut b) = (mat(), mat());
        b.u.d = Vec4::new(1.0, 2.0, 3.0, 4.0);
        slots.attach(&mut a);
        slots.attach(&mut b);
        assert_eq!((a.u.slot, b.u.slot), (0, 1));
        // the shader reads a slot in place of the material's color, a and d, so a new slot must hold them as they are
        let want: Vec<[f32; 4]> = [&a, &b].iter().flat_map(|m| [m.u.color, m.u.a, m.u.d].map(|v| v.to_array())).collect();
        assert_eq!(slots.words(), want);
        assert!(slots.dirty);
    }

    #[test]
    fn with_every_slot_taken_a_material_keeps_its_own_values() {
        let mut slots = GlowSlots::default();
        for _ in 0..NSLOT {
            slots.attach(&mut mat());
        }
        let mut m = mat();
        slots.attach(&mut m);
        assert_eq!((m.u.slot, slots.vals.len()), (NO_SLOT, NSLOT));
        // animating it writes the material, as before
        let mut mats = Assets::<GlowMaterial>::default();
        let h = mats.add(m);
        slots.update(&mut mats, &h, |u| u.a.x = 0.25);
        assert_eq!(mats.get(&h).unwrap().u.a.x, 0.25);
    }

    #[test]
    fn only_a_value_that_differs_in_bits_marks_the_buffer_to_be_sent() {
        let mut slots = GlowSlots::default();
        slots.attach(&mut mat());
        let v = slots.vals[0];
        slots.dirty = false;
        slots.set(0, v);
        assert!(!slots.dirty);
        slots.set(0, Anim { a: Vec4::new(0.75, 0.3, 1.0, 0.0), ..v });
        assert!(slots.dirty);
        // NaN is never equal to itself, but it is the same bits: nothing new to send
        let nan = Anim { d: Vec4::NAN, ..v };
        slots.set(0, nan);
        slots.dirty = false;
        slots.set(0, nan);
        assert!(!slots.dirty);
    }

    #[test]
    fn animating_a_slotted_material_leaves_the_material_alone() {
        let mut app = app();
        let h = slotted(&mut app);
        app.update();
        modified::<GlowMaterial>(&mut app);
        let own = uniform(&app, &h);
        for k in 1..4 {
            animate(&mut app, &h, |u| (u.a.x, u.color, u.d.x) = (k as f32 * 0.25, Vec4::splat(k as f32), k as f32));
            app.update();
            assert!(modified::<GlowMaterial>(&mut app).is_empty(), "frame {k}");
        }
        assert_eq!(uniform(&app, &h), own);
        let v = app.world().resource::<GlowSlots>().vals[own.slot as usize];
        assert_eq!((v.a.x, v.color, v.d.x), (0.75, Vec4::splat(3.0), 3.0));
        // what isn't in the slot still goes in the material, which the renderer then prepares again
        animate(&mut app, &h, |u| u.b.x = 0.5);
        app.update();
        assert_eq!(modified::<GlowMaterial>(&mut app), [h.id()]);
        assert_eq!(uniform(&app, &h).b.x, 0.5);
    }

    #[test]
    fn update_hands_over_the_values_wherever_they_live() {
        let mut app = app();
        let s = slotted(&mut app);
        let p = app.world_mut().resource_mut::<Assets<GlowMaterial>>().add(mat());
        app.update();
        modified::<GlowMaterial>(&mut app);
        for h in [&s, &p] {
            // the second sees what the first left
            animate(&mut app, h, |u| u.a.x += 1.0);
            animate(&mut app, h, |u| u.a.x += 1.0);
        }
        app.update();
        assert_eq!(app.world().resource::<GlowSlots>().vals[0].a.x, 2.5);
        assert_eq!(uniform(&app, &s).a.x, 0.5);
        assert_eq!(uniform(&app, &p).a.x, 2.5);
        assert_eq!(modified::<GlowMaterial>(&mut app), [p.id()]);
        // a material that is written the value it has is not written
        animate(&mut app, &p, |u| u.a.x = 2.5);
        app.update();
        assert!(modified::<GlowMaterial>(&mut app).is_empty());
    }

    #[test]
    fn the_buffer_keeps_its_size_and_is_sent_only_when_a_value_changed() {
        let mut app = app();
        let (h, _other) = (slotted(&mut app), slotted(&mut app));
        app.update();
        // the same frame the slots changed, so the renderer has them as soon as it did with the material writes
        assert_eq!(modified::<ShaderBuffer>(&mut app), [GLOW_SLOTS.id()]);
        let bufs = app.world().resource::<Assets<ShaderBuffer>>();
        let buf = bufs.get(&GLOW_SLOTS).unwrap();
        // a new GPU buffer would break every bind group, so it is always the full size, the used slots at its front
        assert_eq!(buf.buffer_size(), (NSLOT * SLOT_BYTES) as u64);
        let w = buf.cast_slice::<[f32; 4]>().unwrap();
        assert_eq!(w.len(), 6);
        assert_eq!(w[1], mat().u.a.to_array());
        app.update();
        assert!(modified::<ShaderBuffer>(&mut app).is_empty());
        animate(&mut app, &h, |u| u.a.x = 0.75);
        app.update();
        assert_eq!(modified::<ShaderBuffer>(&mut app), [GLOW_SLOTS.id()]);
        let buf = app.world().resource::<Assets<ShaderBuffer>>().get(&GLOW_SLOTS).unwrap();
        assert_eq!(buf.buffer_size(), (NSLOT * SLOT_BYTES) as u64);
        assert_eq!(buf.cast_slice::<[f32; 4]>().unwrap()[1][0], 0.75);
    }

    #[test]
    fn the_rust_side_has_the_layout_the_shader_reads() {
        // WGSL: Anim is three vec4, so an array of them strides 48; Glow is six vec4 and a u32, 112 with padding
        assert_eq!(size_of::<Anim>(), SLOT_BYTES);
        assert_eq!(GlowUniform::min_size().get(), 112);
    }
}
