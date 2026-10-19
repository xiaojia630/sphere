//! Line-work geometry builders, ported from the prototype's `Geo` class.
//! Every ring, tick and block is a flat shape plus "walls" that extrude backwards by `uDepth`.
use crate::glow::ATTR_HOLO;
use crate::util::{DEG, TAU};
use bevy::{asset::RenderAssetUsages, mesh::PrimitiveTopology, prelude::*};

pub type C = [f32; 3]; // x, y, angle (clockwise from 12 o'clock, used for the boot reveal)

#[derive(Default)]
pub struct Geo {
    pub p: Vec<[f32; 3]>,
    pub a: Vec<[f32; 4]>,
}

impl Geo {
    pub fn new() -> Self {
        Self::default()
    }
    fn vtx(&mut self, c: C, wall: f32, back: f32, k: f32) {
        self.p.push([c[0], c[1], 0.0]);
        self.a.push([wall, back, c[2], k]);
    }
    pub fn quad(&mut self, c0: C, c1: C, c2: C, c3: C, k: f32) {
        for c in [c0, c1, c2, c0, c2, c3] {
            self.vtx(c, 0.0, 0.0, k);
        }
    }
    pub fn tri(&mut self, c0: C, c1: C, c2: C, k: f32) {
        for c in [c0, c1, c2] {
            self.vtx(c, 0.0, 0.0, k);
        }
    }
    pub fn wall(&mut self, c0: C, c1: C, k: f32) {
        self.vtx(c0, 1.0, 0.0, k);
        self.vtx(c1, 1.0, 0.0, k);
        self.vtx(c1, 1.0, 1.0, k);
        self.vtx(c0, 1.0, 0.0, k);
        self.vtx(c1, 1.0, 1.0, k);
        self.vtx(c0, 1.0, 1.0, k);
    }
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.p.is_empty()
    }
    /// Keeps a CPU copy after upload: for a mesh that is rebuilt later (`*mesh = ...`).
    pub fn into_mesh(self) -> Mesh {
        self.mesh(RenderAssetUsages::default())
    }
    /// For a mesh nothing reads or rewrites once it has spawned: the CPU copy is freed as soon as the renderer has it.
    pub fn into_gpu_mesh(self) -> Mesh {
        self.mesh(RenderAssetUsages::RENDER_WORLD)
    }
    fn mesh(self, usage: RenderAssetUsages) -> Mesh {
        let mut m = Mesh::new(PrimitiveTopology::TriangleList, usage);
        if self.p.is_empty() {
            // an empty mesh still needs its attributes so the pipeline layout matches
            m.insert_attribute(Mesh::ATTRIBUTE_POSITION, vec![[0.0f32; 3]; 3]);
            m.insert_attribute(ATTR_HOLO, vec![[0.0f32; 4]; 3]);
            return m;
        }
        m.insert_attribute(Mesh::ATTRIBUTE_POSITION, self.p);
        m.insert_attribute(ATTR_HOLO, self.a);
        m
    }
}

/// Angle measured clockwise from 12 o'clock.
pub fn pt(r: f32, t: f32) -> C {
    let mut a = t;
    if !(0.0..=TAU).contains(&a) {
        a = ((a % TAU) + TAU) % TAU;
    }
    [t.sin() * r, t.cos() * r, a]
}

pub struct RingOpt {
    pub a0: f32,
    pub a1: f32,
    pub n: usize,
    pub k: f32,
    pub walls: u8,
}
impl Default for RingOpt {
    fn default() -> Self {
        Self { a0: 0.0, a1: TAU, n: 0, k: 1.0, walls: 1 }
    }
}

pub fn ring_geo(g: &mut Geo, r: f32, w: f32, o: RingOpt) {
    let span = o.a1 - o.a0;
    let n = if o.n > 0 { o.n } else { ((span.abs() * r / 6.0).ceil() as usize).max(4) };
    let (r0, r1) = (r - w / 2.0, r + w / 2.0);
    for i in 0..n {
        let t0 = o.a0 + span * i as f32 / n as f32;
        let t1 = o.a0 + span * (i + 1) as f32 / n as f32;
        g.quad(pt(r0, t0), pt(r1, t0), pt(r1, t1), pt(r0, t1), o.k);
        if o.walls == 1 {
            g.wall(pt(r, t0), pt(r, t1), o.k);
        } else if o.walls == 2 {
            g.wall(pt(r0, t0), pt(r0, t1), o.k);
            g.wall(pt(r1, t0), pt(r1, t1), o.k);
        }
    }
    if o.walls == 2 && span.abs() < TAU - 1e-4 {
        g.wall(pt(r0, o.a0), pt(r1, o.a0), o.k);
        g.wall(pt(r0, o.a1), pt(r1, o.a1), o.k);
    }
}

pub fn ring(g: &mut Geo, r: f32, w: f32) {
    ring_geo(g, r, w, RingOpt::default());
}

pub fn ticks_geo(g: &mut Geo, r: f32, n: usize, len: f32, w: f32, every: usize, long_len: f32) {
    for i in 0..n {
        let t = TAU * i as f32 / n as f32;
        let l = if every > 0 && i % every == 0 { long_len } else { len };
        let (s, c) = t.sin_cos();
        let (nx, ny) = (c * w / 2.0, -s * w / 2.0);
        g.quad(
            [s * r - nx, c * r - ny, t],
            [s * r + nx, c * r + ny, t],
            [s * (r + l) + nx, c * (r + l) + ny, t],
            [s * (r + l) - nx, c * (r + l) - ny, t],
            1.0,
        );
        g.wall([s * r, c * r, t], [s * (r + l), c * (r + l), t], 1.0);
    }
}

pub fn blocks_geo(g: &mut Geo, r: f32, n: usize, h: f32, bw: f32, alt: usize, k_low: f32) {
    for i in 0..n {
        let t = TAU * i as f32 / n as f32;
        let (s, c) = t.sin_cos();
        let kk = if alt > 0 && i % alt != 0 { k_low } else { 1.0 };
        let p = |rr: f32, side: f32| -> C { [s * rr + c * side, c * rr - s * side, t] };
        let a = p(r - h / 2.0, -bw / 2.0);
        let b = p(r - h / 2.0, bw / 2.0);
        let cc = p(r + h / 2.0, bw / 2.0);
        let d = p(r + h / 2.0, -bw / 2.0);
        g.quad(a, b, cc, d, kk);
        g.wall(a, b, kk);
        g.wall(b, cc, kk);
        g.wall(cc, d, kk);
        g.wall(d, a, kk);
    }
}

pub fn arrows_geo(g: &mut Geo, r: f32, n: usize, sz: f32) {
    for i in 0..n {
        let t = TAU * i as f32 / n as f32;
        let (s, c) = t.sin_cos();
        let a: C = [s * r - c * sz, c * r + s * sz, t];
        let b: C = [s * r + c * sz, c * r - s * sz, t];
        let tt: C = [s * (r - sz * 1.5), c * (r - sz * 1.5), t];
        g.tri(a, b, tt, 1.0);
        g.wall(a, b, 1.0);
        g.wall(b, tt, 1.0);
        g.wall(tt, a, 1.0);
    }
}

pub fn dash_geo(g: &mut Geo, r: f32, w: f32, dash: f32, gap: f32) {
    let n = (TAU * r / (dash + gap)).floor() as usize;
    let da = dash / r;
    for i in 0..n {
        let t0 = TAU * i as f32 / n as f32;
        ring_geo(g, r, w, RingOpt { a0: t0, a1: t0 + da, n: 1, ..Default::default() });
    }
}

pub fn arcs_geo(g: &mut Geo, r: f32, segs: usize, w: f32, gap_deg: f32) {
    let st = TAU / segs as f32;
    let gp = gap_deg * DEG;
    for i in 0..segs {
        ring_geo(
            g,
            r,
            w,
            RingOpt {
                a0: i as f32 * st + gp / 2.0,
                a1: (i + 1) as f32 * st - gp / 2.0,
                walls: if w > 3.0 { 2 } else { 1 },
                ..Default::default()
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn some_ring() -> Geo {
        let mut g = Geo::new();
        ring(&mut g, 100.0, 2.0);
        g
    }

    #[test]
    fn only_the_gpu_mesh_gives_up_its_cpu_copy() {
        // `into_mesh` is for meshes rebuilt later with `*mesh = ...`: it has to keep its vertices in the main world
        let kept = some_ring().into_mesh();
        assert_eq!(kept.asset_usage, RenderAssetUsages::default());
        let gpu = some_ring().into_gpu_mesh();
        assert_eq!(gpu.asset_usage, RenderAssetUsages::RENDER_WORLD);
        // the same vertices either way, and an empty one still gets its placeholder triangle
        assert!(kept.count_vertices() > 0 && kept.count_vertices() == gpu.count_vertices());
        let empty = Geo::new().into_gpu_mesh();
        assert_eq!((empty.asset_usage, empty.count_vertices()), (RenderAssetUsages::RENDER_WORLD, 3));
    }
}
