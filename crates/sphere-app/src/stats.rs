//! `SPHERE_STATS=1`: startup timings and what the app holds (entities, cameras, meshes, materials, images), for
//! measuring the app. Off by default; costs nothing then.
use bevy::prelude::*;
use std::sync::OnceLock;
use std::time::Instant;

static START: OnceLock<Instant> = OnceLock::new();

pub fn mark_start() {
    START.get_or_init(Instant::now);
}

pub fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("SPHERE_STATS").is_some())
}

pub fn ms() -> f64 {
    START
        .get()
        .map(|s| s.elapsed().as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

/// Prints how long a step took, when stats are on.
pub fn timed<T>(what: &str, f: impl FnOnce() -> T) -> T {
    if !on() {
        return f();
    }
    let t = Instant::now();
    let r = f();
    eprintln!(
        "stats: {what} took {:.1} ms",
        t.elapsed().as_secs_f64() * 1000.0
    );
    r
}

#[derive(Default)]
pub struct Seen {
    frame: u32,
    visible: bool,
    counted: bool,
}

pub fn report(world: &mut World, mut s: Local<Seen>) {
    if !on() {
        return;
    }
    s.frame += 1;
    if s.frame == 1 {
        eprintln!("stats: first frame (main world) at {:.0} ms", ms());
    }
    if s.frame == 3 {
        eprintln!(
            "stats: third frame (first one presented, pipelined) at {:.0} ms",
            ms()
        );
    }
    let boot = world.resource::<crate::core::Sim>().boot_t;
    if !s.visible && boot > 3.2 {
        s.visible = true;
        eprintln!(
            "stats: boot animation done (everything visible) at {:.0} ms",
            ms()
        );
    }
    if !s.counted && boot > 6.0 {
        s.counted = true;
        counts(world);
    }
}

fn counts(world: &mut World) {
    let ents = world.entities().count_spawned();
    let mut q = world.query::<&Camera>();
    let cams: Vec<bool> = q.iter(world).map(|c| c.is_active).collect();
    let meshes = world.resource::<Assets<Mesh>>();
    let (mut mv, mut mbytes) = (0usize, 0usize);
    for (_, m) in meshes.iter() {
        mv += m.count_vertices();
        mbytes += m.count_vertices() * m.get_vertex_size() as usize
            + m.indices().map(|i| i.len() * 4).unwrap_or(0);
    }
    let nm = meshes.len();
    let images = world.resource::<Assets<Image>>();
    let (mut ib, mut cpu) = (0usize, 0usize);
    for (_, i) in images.iter() {
        let d = i.texture_descriptor.size;
        let bpp = i
            .texture_descriptor
            .format
            .block_copy_size(None)
            .unwrap_or(4) as usize;
        ib += d.width as usize * d.height as usize * d.depth_or_array_layers as usize * bpp;
        cpu += i.data.as_ref().map(|v| v.len()).unwrap_or(0);
    }
    let ni = images.len();
    let glow = world.resource::<Assets<crate::glow::GlowMaterial>>().len();
    let panel = world
        .resource::<Assets<crate::panel::PanelMaterial>>()
        .len();
    let mut tq = world.query::<&Text>();
    let texts = tq.iter(world).count();
    let mut nq = world.query::<&Node>();
    let nodes = nq.iter(world).count();
    eprintln!(
        "stats: entities {ents} | ui nodes {nodes}, texts {texts} | cameras {} ({} active) | meshes {nm} ({mv} verts, {:.1} MB) | materials glow {glow}, panel {panel} | images {ni}: GPU-size {:.1} MB, CPU copies {:.1} MB",
        cams.len(),
        cams.iter().filter(|a| **a).count(),
        mbytes as f64 / 1e6,
        ib as f64 / 1e6,
        cpu as f64 / 1e6
    );
}
