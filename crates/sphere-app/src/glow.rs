//! The additive "glow" material: WGSL/WESL port of the prototype's line, wall, sphere and label shaders.
use bevy::{
    asset::embedded_asset,
    mesh::{MeshVertexAttribute, MeshVertexBufferLayoutRef},
    pbr::{MaterialPipeline, MaterialPipelineKey},
    prelude::*,
    reflect::TypePath,
    render::render_resource::{
        AsBindGroup, BlendComponent, BlendFactor, BlendOperation, BlendState, CompareFunction,
        RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError, VertexFormat,
    },
    shader::ShaderRef,
};

/// (wall, back, ang, k) for rings; (k,..) for lines; (corner.xy, k) for dots; (u, v) for text.
pub const ATTR_HOLO: MeshVertexAttribute = MeshVertexAttribute::new("HoloAttr", 988540917, VertexFormat::Float32x4);

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
}

#[derive(Asset, TypePath, AsBindGroup, Clone, Debug)]
#[bind_group_data(GlowKey)]
pub struct GlowMaterial {
    #[uniform(0)]
    pub u: GlowUniform,
    #[texture(1)]
    #[sampler(2)]
    pub tex: Option<Handle<Image>>,
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
            },
            tex: None,
            kind,
        }
    }
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
    }
}
