//! Mesh builders for the swarm scene: everything is a `Kind::Ring` triangle mesh (the glow material's flat line
//! work, `attr = (wall, back, ang, k)`, k the per-vertex alpha) except the icosahedron and the fresnel shell.
use crate::geo::Geo;
use crate::glow::ATTR_HOLO;
use crate::util::TAU;
use bevy::{asset::RenderAssetUsages, mesh::PrimitiveTopology, prelude::*};

fn tris(pos: Vec<[f32; 3]>, at: Vec<[f32; 4]>, topo: PrimitiveTopology) -> Mesh {
    let mut m = Mesh::new(topo, RenderAssetUsages::default());
    if pos.is_empty() {
        // an empty mesh still needs its attributes so the pipeline layout matches
        m.insert_attribute(Mesh::ATTRIBUTE_POSITION, vec![[0.0f32; 3]; 3]);
        m.insert_attribute(ATTR_HOLO, vec![[0.0f32; 4]; 3]);
        return m;
    }
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
    m.insert_attribute(ATTR_HOLO, at);
    m
}

/// Triangles with a per-vertex alpha `k` (the ring shader's `attr.w`), flat (no walls).
pub fn raw(pos: Vec<[f32; 3]>, k: Vec<f32>) -> Mesh {
    let at = k.into_iter().map(|k| [0.0, 0.0, 0.0, k]).collect();
    tris(pos, at, PrimitiveTopology::TriangleList)
}

/// A ring-line mesh from the shared `Geo` builders.
pub fn geo(f: impl FnOnce(&mut Geo)) -> Mesh {
    let mut g = Geo::new();
    f(&mut g);
    g.into_mesh()
}

/// The soft halo behind a node: a disc whose alpha falls off like the prototype's `pow(1 - r, 1.8)`.
pub fn halo_mesh(r: f32) -> Mesh {
    const N: usize = 24;
    const STEPS: [f32; 5] = [0.0, 0.25, 0.5, 0.75, 1.0];
    let mut pos = Vec::new();
    let mut k = Vec::new();
    for w in STEPS.windows(2) {
        let (r0, r1) = (w[0], w[1]);
        let (k0, k1) = ((1.0 - r0).powf(1.8), (1.0 - r1).powf(1.8));
        for i in 0..N {
            let (a0, a1) = (TAU * i as f32 / N as f32, TAU * (i + 1) as f32 / N as f32);
            let p = |rr: f32, a: f32| [a.cos() * rr * r, a.sin() * rr * r, 0.0];
            for (q, kk) in [(p(r0, a0), k0), (p(r1, a0), k1), (p(r1, a1), k1), (p(r0, a0), k0), (p(r1, a1), k1), (p(r0, a1), k0)] {
                pos.push(q);
                k.push(kk);
            }
        }
    }
    raw(pos, k)
}

/// The icosahedron's edges (a line list) and its faces (triangles), radius `r`, for `Kind::Line`.
pub fn ico_meshes(r: f32) -> (Mesh, Mesh) {
    let t = (1.0 + 5.0f32.sqrt()) / 2.0;
    let v: Vec<Vec3> = [
        [-1.0, t, 0.0], [1.0, t, 0.0], [-1.0, -t, 0.0], [1.0, -t, 0.0], [0.0, -1.0, t], [0.0, 1.0, t],
        [0.0, -1.0, -t], [0.0, 1.0, -t], [t, 0.0, -1.0], [t, 0.0, 1.0], [-t, 0.0, -1.0], [-t, 0.0, 1.0],
    ]
    .iter()
    .map(|p| Vec3::from(*p).normalize() * r)
    .collect();
    let faces: [[usize; 3]; 20] = [
        [0, 11, 5], [0, 5, 1], [0, 1, 7], [0, 7, 10], [0, 10, 11], [1, 5, 9], [5, 11, 4], [11, 10, 2], [10, 7, 6], [7, 1, 8],
        [3, 9, 4], [3, 4, 2], [3, 2, 6], [3, 6, 8], [3, 8, 9], [4, 9, 5], [2, 4, 11], [6, 2, 10], [8, 6, 7], [9, 8, 1],
    ];
    let (mut epos, mut fpos) = (Vec::new(), Vec::new());
    for f in faces {
        for i in 0..3 {
            let (a, c) = (v[f[i]], v[f[(i + 1) % 3]]);
            fpos.push(a.to_array());
            // a shared edge is walked low -> high by exactly one of its two faces
            if f[i] < f[(i + 1) % 3] {
                epos.push(a.to_array());
                epos.push(c.to_array());
            }
        }
    }
    let one = |n: usize| vec![[1.0, 0.0, 0.0, 0.0]; n];
    let (en, fnn) = (epos.len(), fpos.len());
    (tris(epos, one(en), PrimitiveTopology::LineList), tris(fpos, one(fnn), PrimitiveTopology::TriangleList))
}

/// The fresnel shell.
pub fn shell_mesh(r: f32) -> Mesh {
    let mut s = Sphere::new(r).mesh().uv(28, 18);
    let n = s.count_vertices();
    s.insert_attribute(ATTR_HOLO, vec![[0.0f32; 4]; n]);
    s
}

pub const TRAIL_SPAN: f32 = 104.0 * crate::util::DEG;
pub const TRAIL_N: usize = 46;
pub const TRAIL_FAN: f32 = 22.0;

/// The fan out of the ring plane at `f` (0 at the node, 1 at the tail), before the hover factor.
pub fn fan(f: f32) -> f32 {
    TRAIL_FAN * f.clamp(0.0, 1.0).powf(0.85)
}

/// The step trail: 46 quads over 104 degrees behind a node on a ring of radius `rad` that moves with sign
/// `sgn`, local to the ring group rotated by the node's angle (so the node is at +x).
pub fn trail_mesh(rad: f32, sgn: f32) -> Mesh {
    let mut pos = Vec::new();
    let mut k = Vec::new();
    let pt = |f: f32, side: f32| {
        let hw = 0.8 + 1.5 * (1.0 - f);
        let a = -sgn * TRAIL_SPAN * f;
        let r = rad + side * hw;
        [a.cos() * r, a.sin() * r, fan(f)]
    };
    for i in 0..TRAIL_N {
        let (f0, f1) = (i as f32 / TRAIL_N as f32, (i + 1) as f32 / TRAIL_N as f32);
        let (k0, k1) = ((1.0 - f0).powf(1.7), (1.0 - f1).powf(1.7));
        let (i0, o0, i1, o1) = (pt(f0, -1.0), pt(f0, 1.0), pt(f1, -1.0), pt(f1, 1.0));
        for (q, kk) in [(i0, k0), (o0, k0), (o1, k1), (i0, k0), (o1, k1), (i1, k1)] {
            pos.push(q);
            k.push(kk);
        }
    }
    raw(pos, k)
}

/// Camera-facing discs (the camera looks straight down -z) for the step marks: (centre, radius, alpha).
pub fn discs_mesh(d: &[(Vec3, f32, f32)]) -> Mesh {
    const N: usize = 10;
    let mut pos = Vec::new();
    let mut k = Vec::new();
    for (c, r, a) in d {
        for i in 0..N {
            let (a0, a1) = (TAU * i as f32 / N as f32, TAU * (i + 1) as f32 / N as f32);
            for q in [*c, *c + Vec3::new(a0.cos() * r, a0.sin() * r, 0.0), *c + Vec3::new(a1.cos() * r, a1.sin() * r, 0.0)] {
                pos.push(q.to_array());
                k.push(*a);
            }
        }
    }
    raw(pos, k)
}

/// A ribbon along `pts` (world space, facing the camera), `hw` half-width, alpha `k[i]` per point.
pub fn ribbon_mesh(pts: &[Vec3], hw: f32, k: &[f32]) -> Mesh {
    let mut pos = Vec::new();
    let mut kk = Vec::new();
    let n = pts.len();
    let side = |i: usize| {
        let t = (pts[(i + 1).min(n - 1)] - pts[i.saturating_sub(1)]).truncate();
        let t = if t.length_squared() < 1e-6 { Vec2::X } else { t.normalize() };
        Vec3::new(-t.y, t.x, 0.0) * hw
    };
    for i in 0..n.saturating_sub(1) {
        let (s0, s1) = (side(i), side(i + 1));
        let (a, b, c, d) = (pts[i] - s0, pts[i] + s0, pts[i + 1] + s1, pts[i + 1] - s1);
        for (q, w) in [(a, k[i]), (b, k[i]), (c, k[i + 1]), (a, k[i]), (c, k[i + 1]), (d, k[i + 1])] {
            pos.push(q.to_array());
            kk.push(w);
        }
    }
    raw(pos, kk)
}
