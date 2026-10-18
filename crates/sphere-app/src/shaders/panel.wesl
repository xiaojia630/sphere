// Glass panel: smoked glass, thin chamfered frame with glow, corner ticks, LED and bar, plus the
// Bevy-UI-rendered content texture. Everything is drawn in panel pixels, so lines stay crisp at any tilt.
// Output is premultiplied alpha.
import bevy_pbr::render::mesh_functions::{get_world_from_local, mesh_position_local_to_clip};

struct Panel {
    size: vec4<f32>,   // w, h, pad, opacity
    line: vec4<f32>,
    hot: vec4<f32>,
    fill: vec4<f32>,
    acc: vec4<f32>,
    p: vec4<f32>,      // framed (1) / readout (2), smoke alpha, glass scale, glow
    // theme swap: the Balanced role colours the UI is drawn in, and what each becomes now (linear)
    from_c: array<vec4<f32>, 8>,
    to_c: array<vec4<f32>, 8>,
    q: vec4<f32>,      // x: how much of the outline is drawn (the boot draw-in), 0..1
    r: vec4<f32>,      // the panel's rect in the shared UI atlas: uv offset (xy), size (zw)
};
@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> u: Panel;
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var content: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var content_samp: sampler;

struct VIn {
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
};
struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_panel(v: VIn) -> VOut {
    var o: VOut;
    o.clip = mesh_position_local_to_clip(get_world_from_local(v.instance_index), vec4<f32>(v.position, 1.0));
    o.uv = v.uv;
    return o;
}

// how much a content texel glows: only bright text does (dim labels and small figures stay crisp, as with the
// prototype's text-shadow on the hot text)
fn glow_tap(uv: vec2<f32>) -> f32 {
    let c = textureSample(content, content_samp, uv);
    return c.a * smoothstep(0.38, 0.85, max(c.r, max(c.g, c.b)));
}

// panel uv (0..1) to the atlas
fn atl(t: vec2<f32>) -> vec2<f32> {
    return u.r.xy + t * u.r.zw;
}

fn seg_d(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let pa = p - a;
    let ba = b - a;
    let h = clamp(dot(pa, ba) / dot(ba, ba), 0.0, 1.0);
    return length(pa - ba * h);
}
fn cover(d: f32, hw: f32, aa: f32) -> f32 {
    return 1.0 - smoothstep(hw - aa, hw + aa, d);
}
// premultiplied "over"
fn over(dst: vec4<f32>, src: vec4<f32>) -> vec4<f32> {
    return src + dst * (1.0 - src.a);
}

// Panel opacity, applied the way CSS does (in sRGB, over the dark scene): blending here is linear, so the
// prototype's `opacity: .07` is .07^2.2 of the light, not .07.
fn fade() -> f32 {
    return pow(clamp(u.size.w, 0.0, 1.0), 2.2);
}

// UI is drawn in the Balanced colours; swap each pixel that is one of them for the current theme's colour.
// Texels are premultiplied, so compare the straight colour (tolerant for faint edge pixels).
fn retheme(c: vec4<f32>) -> vec4<f32> {
    // nothing to swap in the Balanced theme (q.y is 0)
    if (c.a < 0.01 || u.q.y < 0.5) { return c; }
    let s = pow(clamp(c.rgb / c.a, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(1.0 / 2.2));
    var best = 1e9;
    var bi = -1;
    for (var i = 0; i < 8; i = i + 1) {
        // from_c is stored gamma-encoded, so it compares with `s` directly
        let d = distance(s, u.from_c[i].rgb);
        if (d < best) { best = d; bi = i; }
    }
    let tol = 0.05 + (1.0 - c.a) * 0.2;
    if (bi < 0 || best > tol) { return c; }
    return vec4<f32>(u.to_c[bi].rgb * c.a, c.a);
}

@fragment
fn fs_panel(i: VOut) -> @location(0) vec4<f32> {
    let w = u.size.x;
    let h = u.size.y;
    let pad = u.size.z;
    let q = vec2<f32>(i.uv.x * (w + 2.0 * pad) - pad, i.uv.y * (h + 2.0 * pad) - pad);
    let fw = max(max(length(vec2<f32>(dpdx(q.x), dpdy(q.x))), length(vec2<f32>(dpdx(q.y), dpdy(q.y)))), 0.0001);
    let c = 14.0;
    var col = vec4<f32>(0.0);

    // content texture (+ a soft text glow from a few taps, standing in for the CSS text-shadow)
    let tuv = q / vec2<f32>(w, h);
    let inb = f32(tuv.x >= 0.0 && tuv.x <= 1.0 && tuv.y >= 0.0 && tuv.y <= 1.0);
    let cuv = clamp(tuv, vec2<f32>(0.0), vec2<f32>(1.0));
    let tex = retheme(textureSample(content, content_samp, atl(cuv))) * inb;

    if (u.p.x > 2.5) {
        // bare text, no glass or frame
        return tex * fade();
    }
    if (u.p.x > 1.5) {
        // core readout: black radial backdrop, then the text
        let rr = length((q - vec2<f32>(w, h) * 0.5) / (vec2<f32>(w, h) * 0.5));
        let ba = mix(0.7, 0.42, clamp(rr / 0.58, 0.0, 1.0)) * (1.0 - smoothstep(0.58, 1.0, rr)) * u.p.y;
        col = vec4<f32>(0.0, 0.0, 0.0, ba * inb);
        col = over(col, tex);
        return col * fade();
    }

    // glass: smoke + the prototype's line-tinted gradient, clipped to the chamfered outline
    let gy = clamp(q.y / h, 0.0, 1.0);
    var ga = 0.0;
    if (gy < 0.34) { ga = mix(0.09, 0.025, gy / 0.34); } else { ga = mix(0.025, 0.04, (gy - 0.34) / 0.66); }
    ga = ga * u.p.z;
    let sd = min(min(min(q.x, w - q.x), min(q.y, h - q.y)),
                 min((q.x + q.y - c) * 0.70710678, ((w - q.x) + (h - q.y) - c) * 0.70710678));
    let mask = clamp(sd / fw + 0.5, 0.0, 1.0);
    let smoke = u.p.y;
    let ls = pow(u.line.rgb, vec3<f32>(1.0 / 2.2));
    let gcol = pow(ls * ga, vec3<f32>(2.2));
    let glass = vec4<f32>(gcol, ga + smoke * (1.0 - ga));
    col = glass * mask;

    // content over glass, with glow
    let r = 2.2;
    let tw = vec2<f32>(r / w, r / h);
    var gl = 0.0;
    gl += glow_tap(atl(clamp(cuv + vec2<f32>(tw.x, 0.0), vec2<f32>(0.0), vec2<f32>(1.0))));
    gl += glow_tap(atl(clamp(cuv - vec2<f32>(tw.x, 0.0), vec2<f32>(0.0), vec2<f32>(1.0))));
    gl += glow_tap(atl(clamp(cuv + vec2<f32>(0.0, tw.y), vec2<f32>(0.0), vec2<f32>(1.0))));
    gl += glow_tap(atl(clamp(cuv - vec2<f32>(0.0, tw.y), vec2<f32>(0.0), vec2<f32>(1.0))));
    gl = gl * 0.25 * inb * u.p.w;
    col = over(col, vec4<f32>(u.line.rgb * gl * 0.55, gl * 0.35) * mask);
    col = over(col, tex * mask);

    // frame
    let hw = max(0.5, 0.5 * fw);
    let aa = max(0.5 * fw, 0.35);
    let P0 = vec2<f32>(c, 0.5);
    let P1 = vec2<f32>(w - 0.5, 0.5);
    let P2 = vec2<f32>(w - 0.5, h - c);
    let P3 = vec2<f32>(w - c, h - 0.5);
    let P4 = vec2<f32>(0.5, h - 0.5);
    let P5 = vec2<f32>(0.5, c);
    // the outline, and where along it (in path length) this pixel is: it draws in from the top-left chamfer
    let pts = array<vec2<f32>, 7>(P0, P1, P2, P3, P4, P5, P0);
    var d = 1e9;
    var s = 0.0;
    var acc = 0.0;
    for (var k = 0; k < 6; k = k + 1) {
        let a = pts[k];
        let b = pts[k + 1];
        let ba = b - a;
        let len = length(ba);
        let h = clamp(dot(q - a, ba) / dot(ba, ba), 0.0, 1.0);
        let dk = length(q - a - ba * h);
        if (dk < d) { d = dk; s = acc + h * len; }
        acc = acc + len;
    }
    let drawn = clamp((u.q.x * acc - s) / 2.0 + 0.5, 0.0, 1.0);
    let glow_a = (0.72 * exp(-d * d / 2.0) * 0.45 + 0.13 * exp(-d * d / 32.0)) * u.p.w * drawn;
    col = over(col, vec4<f32>(u.line.rgb * glow_a, glow_a));
    let la = cover(d, hw, aa) * 0.92 * drawn;
    col = over(col, vec4<f32>(u.hot.rgb * la, la));

    // ticks: six along the top right, five down the left edge
    var td = 1000.0;
    for (var k = 0; k < 6; k = k + 1) {
        let x = w - 58.0 + 8.0 * f32(k);
        td = min(td, seg_d(q, vec2<f32>(x, 0.5), vec2<f32>(x, 5.0)));
    }
    for (var k = 0; k < 5; k = k + 1) {
        let y = c + 16.0 + 6.0 * f32(k);
        td = min(td, seg_d(q, vec2<f32>(0.5, y), vec2<f32>(4.5, y)));
    }
    let ta = cover(td, hw, aa) * 0.7 * u.q.x;
    col = over(col, vec4<f32>(u.line.rgb * ta, ta));

    // LED triangle (bottom right) and bar (bottom left)
    let A = vec2<f32>(w - 2.5, h - c + 4.5);
    let B = vec2<f32>(w - c + 4.5, h - 2.5);
    let C = vec2<f32>(w - 2.5, h - 2.5);
    let e0 = (dot(q - A, vec2<f32>(B.y - A.y, A.x - B.x)) / length(B - A));
    let tri_in = min(min(w - 2.5 - q.x, h - 2.5 - q.y), -e0 * -1.0);
    // inside when q.x <= C.x, q.y <= C.y and on the C side of edge A-B
    let side = (q.x - A.x) * (B.y - A.y) - (q.y - A.y) * (B.x - A.x);
    let sC = (C.x - A.x) * (B.y - A.y) - (C.y - A.y) * (B.x - A.x);
    let on_c = select(0.0, 1.0, side * sC >= 0.0);
    let led = clamp(min(min(C.x - q.x, C.y - q.y) / fw + 0.5, 1.0), 0.0, 1.0) * on_c;
    col = over(col, vec4<f32>(u.fill.rgb * led * u.q.x, led * u.q.x));
    let bx = clamp(min(q.x - 18.0, 48.0 - q.x) / fw + 0.5, 0.0, 1.0);
    let by = clamp(min(q.y - (h - 3.0), (h - 0.5) - q.y) / fw + 0.5, 0.0, 1.0);
    let bar = bx * by * u.q.x;
    col = over(col, vec4<f32>(u.fill.rgb * bar, bar));

    return col * fade();
}
