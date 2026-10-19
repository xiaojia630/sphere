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
    /// CPU use is measured from 8 s to 18 s after start: the snapshot at 8 s and its frame
    usage: Option<(Usage, u32)>,
    usage_done: bool,
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
    if !s.usage_done {
        let t = ms();
        if s.usage.is_none() && t > 8000.0 {
            s.usage = Some((Usage::now(), s.frame));
        } else if t > 18000.0 {
            if let Some((a, f0)) = s.usage.take() {
                s.usage_done = true;
                eprintln!("stats: usage, {} | {}", window_size(world), usage_line(&a, &Usage::now(), (s.frame - f0) as u64));
            }
        }
    }
}

/// The window's size in pixels and its scale (2 on a Retina screen), or "offscreen".
pub fn window_size(world: &mut World) -> String {
    let mut q = world.query_filtered::<&Window, With<bevy::window::PrimaryWindow>>();
    match q.iter(world).next() {
        Some(w) => format!("window {}x{} px at scale {}", w.physical_width(), w.physical_height(), w.scale_factor()),
        None => "offscreen".into(),
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

/// CPU time all of the process's threads have used, in ns.
pub fn process_cpu_ns() -> u64 {
    // SAFETY: getrusage only writes the zeroed struct we give it.
    let u = unsafe {
        let mut u: libc::rusage = std::mem::zeroed();
        libc::getrusage(libc::RUSAGE_SELF, &mut u);
        u
    };
    let ns = |t: libc::timeval| t.tv_sec as u64 * 1_000_000_000 + t.tv_usec as u64 * 1000;
    ns(u.ru_utime) + ns(u.ru_stime)
}

/// What the process has used up to a moment, for `--bench` and `SPHERE_STATS=1`: CPU time in all and per thread,
/// and on macOS the CPU's energy, its time on performance cores, context switches and wake-ups (zero where the OS
/// doesn't say). Two of these make a `usage_line`.
#[derive(Clone)]
pub struct Usage {
    at: Instant,
    cpu_ns: u64,
    /// (thread id, name, CPU ns); the first is the main thread
    threads: Vec<(u64, String, u64)>,
    energy_nj: u64,
    /// CPU time in all and on performance cores, in the OS's own clock (only their ratio is used)
    time_abs: u64,
    ptime_abs: u64,
    csw: u64,
    wakeups: u64,
}

impl Usage {
    pub fn now() -> Usage {
        #[allow(unused_mut)]
        let mut u = Usage { at: Instant::now(), cpu_ns: process_cpu_ns(), threads: Vec::new(), energy_nj: 0, time_abs: 0, ptime_abs: 0, csw: 0, wakeups: 0 };
        #[cfg(target_os = "macos")]
        mac::fill(&mut u);
        u
    }

    /// CPU ms per frame from `a` to `self`: the main thread's, the render thread's (0 where threads can't be
    /// read), and all threads'.
    pub fn per_frame_ms(&self, a: &Usage, frames: u64) -> (f64, f64, f64) {
        let d = deltas(a, self);
        let n = frames.max(1) as f64;
        let ms = |want: &str| d.iter().filter(|(name, _)| name == want).map(|(_, t)| *t).sum::<u64>() as f64 / 1e6 / n;
        (ms("main"), ms("Render thread"), self.cpu_ns.saturating_sub(a.cpu_ns) as f64 / 1e6 / n)
    }
}

/// Each thread's CPU ns from `a` to `b`, by name (threads that started in between count from zero).
fn deltas(a: &Usage, b: &Usage) -> Vec<(String, u64)> {
    b.threads
        .iter()
        .map(|(id, name, t)| {
            let t0 = a.threads.iter().find(|(i, _, _)| i == id).map(|(_, _, t)| *t).unwrap_or(0);
            (name.clone(), t.saturating_sub(t0))
        })
        .collect()
}

/// `a` to `b` over `frames` frames in one line: CPU per frame, by thread (pools summed: "Compute Task Pool x6"),
/// and on macOS the share on performance cores, the CPU's power, context switches and wake-ups per second.
pub fn usage_line(a: &Usage, b: &Usage, frames: u64) -> String {
    let secs = b.at.duration_since(a.at).as_secs_f64().max(1e-6);
    let n = frames.max(1) as f64;
    let ms = |ns: u64| ns as f64 / 1e6 / n;
    let cpu = b.cpu_ns.saturating_sub(a.cpu_ns);
    let mut groups: Vec<(String, u64, usize)> = Vec::new();
    for (name, t) in deltas(a, b) {
        // "Compute Task Pool (3)" -> "Compute Task Pool"
        let key = match name.rfind(" (") {
            Some(i) if name.ends_with(')') => name[..i].to_string(),
            _ if name.is_empty() => "unnamed".to_string(),
            _ => name,
        };
        match groups.iter_mut().find(|g| g.0 == key) {
            Some(g) => {
                g.1 += t;
                g.2 += 1;
            }
            None => groups.push((key, t, 1)),
        }
    }
    groups.sort_by_key(|g| std::cmp::Reverse(g.1));
    let listed: u64 = groups.iter().map(|g| g.1).sum();
    let mut per: Vec<String> = groups
        .iter()
        .filter(|g| g.1 as f64 / 1e6 / n >= 0.005)
        .map(|(k, t, c)| if *c > 1 { format!("{k} x{c} {:.2}", ms(*t)) } else { format!("{k} {:.2}", ms(*t)) })
        .collect();
    if !b.threads.is_empty() && cpu > listed {
        per.push(format!("exited threads {:.2}", ms(cpu - listed)));
    }
    let mut line = format!(
        "{:.1} s, {} frames ({:.1} fps) | CPU {:.2} cores, {:.2} ms/frame",
        secs,
        frames,
        frames as f64 / secs,
        cpu as f64 / 1e9 / secs,
        ms(cpu)
    );
    if !per.is_empty() {
        line += &format!(": {}", per.join(", "));
    }
    let t = b.time_abs.saturating_sub(a.time_abs);
    let p = b.ptime_abs.saturating_sub(a.ptime_abs);
    if t > 0 && p <= t {
        line += &format!(" | on performance cores {:.0}%", p as f64 * 100.0 / t as f64);
    }
    let e = b.energy_nj.saturating_sub(a.energy_nj);
    if e > 0 {
        line += &format!(" | CPU power {:.2} W, {:.2} mJ/frame", e as f64 / 1e9 / secs, e as f64 / 1e6 / n);
    }
    if b.csw > 0 {
        line += &format!(" | {:.0} context switches/s, {:.0} wake-ups/s", b.csw.saturating_sub(a.csw) as f64 / secs, b.wakeups.saturating_sub(a.wakeups) as f64 / secs);
    }
    line
}

#[cfg(target_os = "macos")]
mod mac {
    use super::Usage;
    use std::ffi::CStr;

    extern "C" {
        static mach_task_self_: libc::mach_port_t;
        fn mach_port_deallocate(task: libc::mach_port_t, name: libc::mach_port_t) -> libc::kern_return_t;
    }

    /// `proc_pid_rusage` flavour 6 (macOS 12+): adds time on performance cores and CPU energy to v4.
    const RUSAGE_INFO_V6: libc::c_int = 6;

    pub fn fill(u: &mut Usage) {
        // SAFETY: Mach and libproc queries about our own task, each writing only into the buffer it is given (sized
        // as the call expects; the rusage buffer is larger than any version's struct); the thread list and its
        // ports are released after use.
        unsafe {
            let task = mach_task_self_;
            let mut list: libc::thread_act_array_t = std::ptr::null_mut();
            let mut count: libc::mach_msg_type_number_t = 0;
            if libc::task_threads(task, &mut list, &mut count) == libc::KERN_SUCCESS {
                for i in 0..count as usize {
                    let th = *list.add(i);
                    let mut bi: libc::thread_basic_info = std::mem::zeroed();
                    let mut bn = libc::THREAD_BASIC_INFO_COUNT;
                    let mut ii: libc::thread_identifier_info = std::mem::zeroed();
                    let mut iin = libc::THREAD_IDENTIFIER_INFO_COUNT;
                    if libc::thread_info(th, libc::THREAD_BASIC_INFO as libc::thread_flavor_t, &mut bi as *mut _ as libc::thread_info_t, &mut bn) == libc::KERN_SUCCESS
                        && libc::thread_info(th, libc::THREAD_IDENTIFIER_INFO as libc::thread_flavor_t, &mut ii as *mut _ as libc::thread_info_t, &mut iin) == libc::KERN_SUCCESS
                    {
                        let ns = |v: libc::time_value_t| v.seconds as u64 * 1_000_000_000 + v.microseconds as u64 * 1000;
                        let mut buf = [0 as libc::c_char; 64];
                        let pt = libc::pthread_from_mach_thread_np(th);
                        let mut name = if pt != 0 && libc::pthread_getname_np(pt, buf.as_mut_ptr(), buf.len()) == 0 { CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned() } else { String::new() };
                        if i == 0 && name.is_empty() {
                            name = "main".into();
                        }
                        u.threads.push((ii.thread_id, name, ns(bi.user_time) + ns(bi.system_time)));
                    }
                    mach_port_deallocate(task, th);
                }
                libc::vm_deallocate(task, list as libc::vm_address_t, count as libc::vm_size_t * std::mem::size_of::<libc::thread_act_t>() as libc::vm_size_t);
            }
            let pid = libc::getpid();
            let mut ri = [0u64; 96];
            if libc::proc_pid_rusage(pid, RUSAGE_INFO_V6, ri.as_mut_ptr() as *mut libc::rusage_info_t) == 0 {
                // rusage_info_v6, as u64s after the 16-byte uuid: user/system time 2-3, idle and interrupt wake-ups
                // 4-5, user/system time on performance cores 38-39, energy (nJ) 42
                u.time_abs = ri[2] + ri[3];
                u.wakeups = ri[4] + ri[5];
                u.ptime_abs = ri[38] + ri[39];
                u.energy_nj = ri[42];
            }
            let mut ti: libc::proc_taskinfo = std::mem::zeroed();
            let sz = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
            if libc::proc_pidinfo(pid, libc::PROC_PIDTASKINFO, 0, &mut ti as *mut _ as *mut libc::c_void, sz) == sz {
                u.csw = ti.pti_csw as u32 as u64;
            }
        }
    }
}
