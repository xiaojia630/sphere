// Port of the prototype's HOLO / SPH / FRES / TEXT shaders (Three.js r160, "v2") to WESL.
// One material, five vertex/fragment pairs picked by pipeline specialisation.
//
// Vertex layout (every kind): location 0 position, location 1 attr (vec4)
//   ring : attr = (wall, back, ang, k)       lines/triangles extruded backwards by `depth`
//   line : attr = (k, 0, 0, 0)               wireframe sphere, icosahedron (face-lit by vFace)
//   dots : attr = (corner.x, corner.y, k, 0) camera-facing round points
//   text : attr = (u, v, 0, 0)               label quads sampling an alpha atlas
//   fres : attr unused                       fresnel shell, normal = normalize(position)
import bevy_pbr::render::mesh_functions::{get_world_from_local, mesh_position_local_to_clip};
import bevy_pbr::render::mesh_view_bindings::view;

struct Glow {
    color: vec4<f32>,   // rgb linear
    c2: vec4<f32>,      // second tone (two-tone belts)
    a: vec4<f32>,       // alpha, wall, reveal, hi
    b: vec4<f32>,       // fade, depth, two, pinch
    c: vec4<f32>,       // rg, ang, pt_px, round
    d: vec4<f32>,       // t, iri, 0, 0
    slot: u32,          // where color, a and d come from, see anim()
};
@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> m: Glow;
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var samp: sampler;

// The three vec4 that animate every frame. The CPU keeps them in one buffer shared by all materials (rewriting a
// material makes Bevy rebuild its uniform buffer and bind group); a material with a slot reads them there, one
// without keeps its own. Every entry point that needs color, a or d reads them through anim().
struct Anim {
    color: vec4<f32>,
    a: vec4<f32>,
    d: vec4<f32>,
};
const NO_SLOT: u32 = 0xffffffffu;
@group(constants::MATERIAL_BIND_GROUP) @binding(3) var<storage> slots: array<Anim>;
fn anim() -> Anim {
    if (m.slot == NO_SLOT) {
        return Anim(m.color, m.a, m.d);
    }
    return slots[m.slot];
}

struct VIn {
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
    @location(1) attr: vec4<f32>,
};
struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) attr: vec4<f32>,
    @location(1) extra: vec4<f32>,
};

const TAU: f32 = 6.28319;

fn view_face(world_from_local: mat4x4<f32>, dir: vec3<f32>) -> f32 {
    let w = (world_from_local * vec4<f32>(dir, 0.0)).xyz;
    let v = (view.view_from_world * vec4<f32>(w, 0.0)).xyz;
    return normalize(v).z;
}

// ---------------- ring decks ----------------
@vertex
fn vs_ring(v: VIn) -> VOut {
    var o: VOut;
    var p = v.position;
    p.z = p.z - v.attr.y * m.b.y;
    let len = max(length(p.xy), 1.0);
    let s = mix(1.0, m.c.x / len, m.b.w);
    p = vec3<f32>(p.x * s, p.y * s, p.z);
    o.clip = mesh_position_local_to_clip(get_world_from_local(v.instance_index), vec4<f32>(p, 1.0));
    o.attr = v.attr;
    o.extra = vec4<f32>(0.0);
    return o;
}
@fragment
fn fs_ring(i: VOut) -> @location(0) vec4<f32> {
    let e = anim();
    if (i.attr.z > e.a.z * TAU) { discard; }
    let wall = e.a.y * pow(clamp(1.0 - i.attr.y, 0.0, 1.0), 1.8);
    let t2 = m.b.z * (1.0 - smoothstep(0.3, 0.62, i.attr.w));
    let k = mix(i.attr.w, max(i.attr.w, 0.66), t2);
    let a = mix(1.0, wall, i.attr.x) * e.a.x * k * m.b.x * (1.0 + e.a.w * 0.8);
    return vec4<f32>(mix(e.color.rgb, m.c2.rgb, t2), clamp(a, 0.0, 1.0));
}

// ---------------- voice spokes ----------------
// A static mesh: per spoke a ribbon (x = side offset, y = 0 inner end / 1 outer end) and its wall, attr.w = the
// spoke's index. Each spoke's length follows the same slow sine the CPU used to rebuild the mesh with every frame.
// d = (t, energy, inner radius, length scale)
@vertex
fn vs_spokes(v: VIn) -> VOut {
    var o: VOut;
    let e = anim();
    let i = v.attr.w;
    let ang = v.attr.z;
    let ph = i * 0.37 + e.d.x * (1.1 + (i % 9.0) * 0.14);
    let lvl = (0.22 + (sin(ph * 0.7 + i * 0.28) * 0.5 + 0.5) * 0.3) * e.d.y;
    let r = e.d.z + v.position.y * (4.0 + lvl * e.d.w);
    let s = sin(ang);
    let c = cos(ang);
    let side = v.position.x;
    var p = vec3<f32>(s * r + c * side, c * r - s * side, -v.attr.y * m.b.y);
    let len = max(length(p.xy), 1.0);
    let sc = mix(1.0, m.c.x / len, m.b.w);
    p = vec3<f32>(p.x * sc, p.y * sc, p.z);
    o.clip = mesh_position_local_to_clip(get_world_from_local(v.instance_index), vec4<f32>(p, 1.0));
    o.attr = vec4<f32>(v.attr.x, v.attr.y, ang, 0.45 + lvl * 0.55);
    o.extra = vec4<f32>(0.0);
    return o;
}

// ---------------- sphere lines / faces ----------------
@vertex
fn vs_line(v: VIn) -> VOut {
    var o: VOut;
    let wl = get_world_from_local(v.instance_index);
    o.clip = mesh_position_local_to_clip(wl, vec4<f32>(v.position, 1.0));
    o.attr = v.attr;
    o.extra = vec4<f32>(view_face(wl, normalize(v.position)), 0.0, 0.0, 0.0);
    return o;
}
@fragment
fn fs_line(i: VOut) -> @location(0) vec4<f32> {
    let e = anim();
    let face = mix(0.16, 1.0, smoothstep(-0.25, 0.55, i.extra.x));
    return vec4<f32>(e.color.rgb, e.a.x * i.attr.x * m.b.x * face * e.a.z);
}

// ---------------- dots ----------------
@vertex
fn vs_dots(v: VIn) -> VOut {
    var o: VOut;
    let wl = get_world_from_local(v.instance_index);
    var c = mesh_position_local_to_clip(wl, vec4<f32>(v.position, 1.0));
    let corner = v.attr.xy;
    let vp = view.viewport.zw;
    c = vec4<f32>(c.xy + corner * (m.c.z / vp) * c.w, c.zw);
    o.clip = c;
    o.attr = v.attr;
    o.extra = vec4<f32>(view_face(wl, normalize(v.position)), 0.0, 0.0, 0.0);
    return o;
}
@fragment
fn fs_dots(i: VOut) -> @location(0) vec4<f32> {
    let e = anim();
    if (dot(i.attr.xy, i.attr.xy) > 1.0) { discard; }
    let face = mix(0.16, 1.0, smoothstep(-0.25, 0.55, i.extra.x));
    return vec4<f32>(e.color.rgb, e.a.x * i.attr.z * m.b.x * face * e.a.z);
}

// ---------------- fresnel shell ----------------
@vertex
fn vs_fres(v: VIn) -> VOut {
    var o: VOut;
    let wl = get_world_from_local(v.instance_index);
    o.clip = mesh_position_local_to_clip(wl, vec4<f32>(v.position, 1.0));
    let n = normalize(v.position);
    let wn = (wl * vec4<f32>(n, 0.0)).xyz;
    let vn = normalize((view.view_from_world * vec4<f32>(wn, 0.0)).xyz);
    let vp = (view.view_from_world * wl * vec4<f32>(v.position, 1.0)).xyz;
    o.attr = vec4<f32>(vn, 0.0);
    o.extra = vec4<f32>(normalize(-vp), 0.0);
    return o;
}
fn hue(h: f32) -> vec3<f32> {
    return clamp(abs(((h * 6.0 + vec3<f32>(0.0, 4.0, 2.0)) % 6.0) - 3.0) - 1.0, vec3<f32>(0.0), vec3<f32>(1.0));
}
@fragment
fn fs_fres(i: VOut) -> @location(0) vec4<f32> {
    let e = anim();
    let n = normalize(i.attr.xyz);
    let f = pow(1.0 - abs(dot(n, normalize(i.extra.xyz))), 2.4);
    let a = atan2(n.y, n.x) / TAU;
    let col = mix(e.color.rgb, mix(e.color.rgb, hue(fract(a + e.d.x * 0.03)), 0.72), e.d.y);
    return vec4<f32>(col, f * e.a.x * m.b.x * e.a.z);
}

// ---------------- text labels ----------------
@vertex
fn vs_text(v: VIn) -> VOut {
    var o: VOut;
    o.clip = mesh_position_local_to_clip(get_world_from_local(v.instance_index), vec4<f32>(v.position, 1.0));
    o.attr = v.attr;
    o.extra = vec4<f32>(0.0);
    return o;
}
@fragment
fn fs_text(i: VOut) -> @location(0) vec4<f32> {
    let e = anim();
    if (m.c.y > e.a.z * TAU) { discard; }
    let a = textureSample(tex, samp, i.attr.xy).r * e.a.x * m.b.x;
    return vec4<f32>(e.color.rgb, a);
}

// ---------------- screen backdrop: the prototype's #tint, #grid and #scan overlays ----------------
// color = tint, c2 = grid colour, c.xy = viewport size in css px
@fragment
fn fs_backdrop(i: VOut) -> @location(0) vec4<f32> {
    let e = anim();
    let size = m.c.xy;
    let px = i.attr.xy * size;
    // tint: radial-gradient(ellipse 58% 68% at 50% 46%, tint 0%, transparent 72%)
    let t = length((px - vec2<f32>(size.x * 0.5, size.y * 0.46)) / vec2<f32>(size.x * 0.58, size.y * 0.68));
    let ta = clamp(1.0 - t / 0.72, 0.0, 1.0);
    // grid: 1px lines every 55px, masked by a radial gradient (opaque to 8%, transparent at 70%)
    let yb = size.y - px.y;
    let lx = step(54.0, px.x % 55.0);
    let ly = step(54.0, yb % 55.0);
    let g = max(lx, ly);
    let rn = length((px - size * 0.5) / (size * 0.5 * 1.41421356));
    let mask = 1.0 - clamp((rn - 0.08) / 0.62, 0.0, 1.0);
    let grid = g * mask * 0.11 * 0.34;
    // scanlines: 2px dark every 4px at 5%
    let scan = step(2.0, yb % 4.0) * 0.05;
    let srgb = e.color.rgb * ta + m.c2.rgb * grid;
    let lin = select(srgb / 12.92, pow((srgb + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4)), srgb > vec3<f32>(0.04045));
    return vec4<f32>(lin, scan);
}
