//! This machine's load for the Telemetry panel and the core's load deck: processor, memory, GPU and network,
//! sampled every two seconds on a background thread from the system's own tools (`ps`, `vm_stat`, `ioreg`,
//! `netstat`; macOS). Nothing here runs on the frame path.
use bevy::prelude::*;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stats {
    /// 0..1
    pub cpu: f32,
    pub mem: f32,
    pub gpu: f32,
    /// bytes per second, in and out
    pub net: f32,
    /// false until the first sample lands
    pub ready: bool,
    pub docker: Docker,
}

/// What `docker ps -a` said, last time it was asked.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Docker {
    /// not asked yet
    #[default]
    Unknown,
    /// no docker CLI, or its engine isn't running
    Off,
    Count { running: u32, total: u32 },
}

impl Docker {
    /// For the System panel, the ticker and the Containers tile.
    pub fn label(&self) -> String {
        match self {
            Docker::Unknown => "CHECKING".into(),
            Docker::Off => "DOCKER OFF".into(),
            Docker::Count { total: 0, .. } => "NO CONTAINERS".into(),
            Docker::Count { running: 0, .. } => "NONE RUNNING".into(),
            Docker::Count { running, .. } => format!("{running} RUNNING"),
        }
    }
    /// The Containers tile's second line.
    pub fn detail(&self) -> String {
        match self {
            Docker::Count { total, .. } if *total > 0 => format!("{total} container{} · start, stop, logs", if *total == 1 { "" } else { "s" }),
            _ => "Start, stop, restart, logs".into(),
        }
    }
}

#[derive(Resource, Clone, Default)]
pub struct SysStats(pub Arc<Mutex<Stats>>);

impl SysStats {
    pub fn get(&self) -> Stats {
        *self.0.lock().unwrap()
    }
}

pub fn start(offline: bool) -> SysStats {
    let s = SysStats::default();
    if offline {
        // screenshots and benches: the prototype's example numbers, so the look is comparable
        *s.0.lock().unwrap() = Stats { cpu: 0.23, mem: 0.61, gpu: 0.12, net: 84.0e6, ready: true, docker: Docker::Count { running: 3, total: 5 } };
        return s;
    }
    // containers: `docker ps` on its own thread every few seconds, so a stuck engine never holds the load figures
    let dk = s.0.clone();
    let _ = std::thread::Builder::new().name("sphere-docker".into()).spawn(move || loop {
        let d = docker();
        if let Ok(mut st) = dk.lock() {
            st.docker = d;
        }
        std::thread::sleep(Duration::from_secs(6));
    });
    let out = s.0.clone();
    let _ = std::thread::Builder::new().name("sphere-stats".into()).spawn(move || {
        let ncpu = sh("sysctl", &["-n", "hw.ncpu"]).trim().parse::<f32>().unwrap_or(1.0).max(1.0);
        let memsize = sh("sysctl", &["-n", "hw.memsize"]).trim().parse::<f64>().unwrap_or(0.0);
        let mut last_net: Option<(u64, Instant)> = None;
        loop {
            let cpu = cpu(ncpu);
            let mem = mem(memsize);
            let gpu = gpu();
            let now = Instant::now();
            let bytes = net_bytes();
            let net = match (last_net, bytes) {
                (Some((b0, t0)), Some(b1)) if b1 >= b0 => (b1 - b0) as f32 / now.duration_since(t0).as_secs_f32().max(0.1),
                _ => 0.0,
            };
            if let Some(b) = bytes {
                last_net = Some((b, now));
            }
            let mut st = out.lock().unwrap();
            let docker = st.docker;
            *st = Stats { cpu, mem, gpu, net, ready: true, docker };
            drop(st);
            std::thread::sleep(Duration::from_secs(2));
        }
    });
    s
}

/// Counts containers with `docker ps -a`, giving up after a few seconds (an engine that is starting can hang).
fn docker() -> Docker {
    let bin = ["/usr/local/bin/docker", "/opt/homebrew/bin/docker", "/Applications/Docker.app/Contents/Resources/bin/docker"].into_iter().find(|p| std::path::Path::new(p).exists()).unwrap_or("docker");
    let filter = std::env::var("SPHERE_DOCKER_FILTER").ok();
    let mut args = vec!["ps", "-a", "--format", "{{.State}}"];
    if let Some(f) = filter.as_deref() {
        args.extend(["--filter", f]);
    }
    let Ok(mut child) = Command::new(bin).args(&args).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).stdin(std::process::Stdio::null()).spawn() else { return Docker::Off };
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() < Duration::from_secs(5) => std::thread::sleep(Duration::from_millis(40)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Docker::Off;
            }
        }
    }
    let Ok(out) = child.wait_with_output() else { return Docker::Off };
    if !out.status.success() {
        return Docker::Off;
    }
    count_containers(&String::from_utf8_lossy(&out.stdout))
}

/// One state per line, as `docker ps -a --format {{.State}}` prints them.
fn count_containers(out: &str) -> Docker {
    let states: Vec<&str> = out.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    Docker::Count { running: states.iter().filter(|s| **s == "running").count() as u32, total: states.len() as u32 }
}

fn sh(cmd: &str, args: &[&str]) -> String {
    Command::new(cmd).args(args).output().map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default()
}

/// Sum of every process's CPU share over the number of cores.
fn cpu(ncpu: f32) -> f32 {
    let total: f32 = sh("ps", &["-A", "-o", "%cpu="]).lines().filter_map(|l| l.trim().parse::<f32>().ok()).sum();
    (total / (100.0 * ncpu)).clamp(0.0, 1.0)
}

/// Active, wired and compressed pages over physical memory (what Activity Monitor calls memory used, roughly).
fn mem(memsize: f64) -> f32 {
    if memsize <= 0.0 {
        return 0.0;
    }
    let out = sh("vm_stat", &[]);
    let page = out
        .lines()
        .next()
        .and_then(|l| l.split("page size of ").nth(1))
        .and_then(|s| s.split_whitespace().next())
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(16384.0);
    let pages = |key: &str| -> f64 {
        out.lines()
            .find(|l| l.starts_with(key))
            .and_then(|l| l.rsplit(':').next())
            .map(|v| v.trim().trim_end_matches('.').parse::<f64>().unwrap_or(0.0))
            .unwrap_or(0.0)
    };
    let used = pages("Pages active") + pages("Pages wired down") + pages("Pages occupied by compressor");
    ((used * page) / memsize).clamp(0.0, 1.0) as f32
}

/// The GPU's own utilisation figure from its performance statistics.
pub fn gpu() -> f32 {
    let out = sh("ioreg", &["-r", "-d", "1", "-w", "0", "-c", "IOAccelerator"]);
    out.split("\"Device Utilization %\"=")
        .nth(1)
        .and_then(|s| s.split(|c: char| !c.is_ascii_digit()).next())
        .and_then(|s| s.parse::<f32>().ok())
        .map(|v| (v / 100.0).clamp(0.0, 1.0))
        .unwrap_or(0.0)
}

/// Bytes in and out across the physical interfaces (en*), from the link-level rows of `netstat -ib`.
fn net_bytes() -> Option<u64> {
    // -n: no DNS lookups (with them this took seconds)
    let out = sh("netstat", &["-inb"]);
    let mut lines = out.lines();
    let header: Vec<&str> = lines.next()?.split_whitespace().collect();
    let ib = header.iter().position(|h| *h == "Ibytes")?;
    let ob = header.iter().position(|h| *h == "Obytes")?;
    let mut total = 0u64;
    for l in lines {
        let f: Vec<&str> = l.split_whitespace().collect();
        // link rows carry "<Link#n>" in the network column; address rows would double count
        if f.len() > ob && f[0].starts_with("en") && f.get(2).is_some_and(|n| n.starts_with("<Link")) {
            // rows with an empty address column are one field shorter
            let shift = if f.len() == header.len() { 0 } else { 1 };
            let i = f.get(ib - shift).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
            let o = f.get(ob - shift).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
            total += i + o;
        }
    }
    Some(total)
}

/// "84 MB/s", "512 KB/s"
pub fn rate(bps: f32) -> String {
    if bps >= 1.0e6 {
        format!("{:.0} MB/s", bps / 1.0e6)
    } else {
        format!("{:.0} KB/s", bps / 1.0e3)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn containers_are_counted_from_their_states() {
        assert_eq!(count_containers("running\nexited\nrunning\n"), Docker::Count { running: 2, total: 3 });
        assert_eq!(count_containers(""), Docker::Count { running: 0, total: 0 });
        assert_eq!(Docker::Count { running: 0, total: 0 }.label(), "NO CONTAINERS");
        assert_eq!(Docker::Off.label(), "DOCKER OFF");
        assert_eq!(Docker::Count { running: 2, total: 3 }.label(), "2 RUNNING");
        assert_eq!(Docker::Count { running: 0, total: 3 }.label(), "NONE RUNNING");
        assert_eq!(Docker::Count { running: 2, total: 3 }.detail(), "3 containers · start, stop, logs");
    }

    #[test]
    fn rates_read_like_the_prototype() {
        assert_eq!(rate(84.0e6), "84 MB/s");
        assert_eq!(rate(512.0e3), "512 KB/s");
    }
}
