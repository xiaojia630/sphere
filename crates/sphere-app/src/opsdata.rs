//! What the Containers, Ports & logs and System apps show, read from the machine's own tools: the `docker` CLI
//! (Docker Desktop or OrbStack), `lsof`, `ps`, `vm_stat`, `sysctl`, `netstat`, `df`, `pmset`, `ioreg`. Argv only,
//! never a shell. Everything here blocks, so the apps call it from background threads.
use std::process::Command;

fn run(cmd: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(cmd).args(args).output().map_err(|e| format!("{cmd}: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let e = String::from_utf8_lossy(&out.stderr);
        Err(e.lines().find(|l| !l.trim().is_empty()).unwrap_or("failed").trim().to_string())
    }
}

// ------------------------------------------------------------------ containers

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Container {
    pub id: String,
    pub name: String,
    pub image: String,
    /// running, exited, restarting, paused, created
    pub state: String,
    pub status: String,
    pub ports: String,
    pub project: String,
    pub service: String,
    pub health: Option<String>,
    pub cpu: f32,
    pub mem: String,
    pub mem_frac: f32,
}

impl Container {
    pub fn unhealthy(&self) -> bool {
        self.health.as_deref() == Some("unhealthy")
    }
    /// Host ports this container publishes.
    pub fn host_ports(&self) -> Vec<u16> {
        parse_published(&self.ports).into_iter().map(|p| p.0).collect()
    }
}

fn jstr(v: &serde_json::Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

/// `docker ps -a` only (no stats): quick, for mapping ports.
pub fn containers_quick() -> Result<Vec<Container>, String> {
    list(false)
}

/// `docker ps -a --format json`, plus `docker stats` for the running ones.
pub fn containers() -> Result<Vec<Container>, String> {
    list(true)
}

fn list(stats: bool) -> Result<Vec<Container>, String> {
    let raw = run("docker", &["ps", "-a", "--format", "{{json .}}"])?;
    let mut v: Vec<Container> = raw
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .map(|j| {
            let labels = jstr(&j, "Labels");
            let label = |k: &str| labels.split(',').find_map(|kv| kv.strip_prefix(k).and_then(|r| r.strip_prefix('='))).unwrap_or("").to_string();
            let status = jstr(&j, "Status");
            let health = if status.contains("(unhealthy)") {
                Some("unhealthy".into())
            } else if status.contains("(healthy)") {
                Some("healthy".into())
            } else if status.contains("(health: starting)") {
                Some("starting".into())
            } else {
                None
            };
            Container { id: jstr(&j, "ID"), name: jstr(&j, "Names"), image: jstr(&j, "Image"), state: jstr(&j, "State"), status, ports: jstr(&j, "Ports"), project: label("com.docker.compose.project"), service: label("com.docker.compose.service"), health, ..Default::default() }
        })
        .collect();
    if stats && v.iter().any(|c| c.state == "running") {
        if let Ok(stats) = run("docker", &["stats", "--no-stream", "--format", "{{json .}}"]) {
            for l in stats.lines() {
                let Ok(j) = serde_json::from_str::<serde_json::Value>(l) else { continue };
                let id = jstr(&j, "ID");
                if let Some(c) = v.iter_mut().find(|c| id.starts_with(&c.id) || c.id.starts_with(&id)) {
                    c.cpu = jstr(&j, "CPUPerc").trim_end_matches('%').parse().unwrap_or(0.0);
                    c.mem = jstr(&j, "MemUsage");
                    c.mem_frac = jstr(&j, "MemPerc").trim_end_matches('%').parse::<f32>().unwrap_or(0.0) / 100.0;
                }
            }
        }
    }
    // running first, then by project and name
    v.sort_by(|a, b| (a.state != "running", &a.project, &a.name).cmp(&(b.state != "running", &b.project, &b.name)));
    Ok(v)
}

pub fn logs(id: &str, n: usize) -> Vec<String> {
    let out = Command::new("docker").args(["logs", "--tail", &n.to_string(), "--timestamps", id]).output();
    match out {
        Ok(o) => {
            // the container's stdout and stderr, in time order (timestamps lead each line)
            let mut lines: Vec<String> = String::from_utf8_lossy(&o.stdout).lines().chain(String::from_utf8_lossy(&o.stderr).lines()).map(|s| s.to_string()).collect();
            lines.sort();
            let k = lines.len().saturating_sub(n);
            lines.split_off(k)
        }
        Err(e) => vec![format!("docker logs: {e}")],
    }
}

pub enum DockerOp {
    Start(String),
    Stop(String),
    Restart(String),
    Remove(String),
    Pull(String),
}

pub fn docker(op: DockerOp) -> Result<String, String> {
    match op {
        DockerOp::Start(n) => run("docker", &["start", &n]).map(|_| format!("Started {n}")),
        DockerOp::Stop(n) => run("docker", &["stop", &n]).map(|_| format!("Stopped {n}")),
        DockerOp::Restart(n) => run("docker", &["restart", &n]).map(|_| format!("Restarted {n}")),
        DockerOp::Remove(n) => run("docker", &["rm", "-f", &n]).map(|_| format!("Removed {n}")),
        DockerOp::Pull(i) => run("docker", &["pull", "-q", &i]).map(|_| format!("Pulled {i}")),
    }
}

/// "0.0.0.0:8090->80/tcp, [::]:8090->80/tcp" → [(8090, "80/tcp")], each host port once.
pub fn parse_published(ports: &str) -> Vec<(u16, String)> {
    let mut out: Vec<(u16, String)> = Vec::new();
    for p in ports.split(", ") {
        let Some((host, inner)) = p.split_once("->") else { continue };
        let Some(hp) = host.rsplit(':').next().and_then(|n| n.parse::<u16>().ok()) else { continue };
        if !out.iter().any(|o| o.0 == hp) {
            out.push((hp, inner.to_string()));
        }
    }
    out
}

// ------------------------------------------------------------------ listeners

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Listener {
    pub port: u16,
    pub pid: u32,
    pub command: String,
    pub addr: String,
    /// the container publishing it, when one does
    pub container: Option<String>,
}

/// `lsof -nP -iTCP -sTCP:LISTEN -F pcn` output: one listener per port.
pub fn parse_lsof(raw: &str) -> Vec<Listener> {
    let (mut pid, mut cmd) = (0u32, String::new());
    let mut out: Vec<Listener> = Vec::new();
    for l in raw.lines() {
        match l.split_at_checked(1) {
            Some(("p", v)) => pid = v.parse().unwrap_or(0),
            Some(("c", v)) => cmd = v.to_string(),
            Some(("n", v)) => {
                let Some((addr, port)) = v.rsplit_once(':') else { continue };
                let Ok(port) = port.parse::<u16>() else { continue };
                if !out.iter().any(|o| o.port == port) {
                    out.push(Listener { port, pid, command: cmd.clone(), addr: addr.to_string(), container: None });
                }
            }
            _ => {}
        }
    }
    out.sort_by_key(|l| l.port);
    out
}

pub fn listeners(containers: &[Container]) -> Result<Vec<Listener>, String> {
    let raw = run("lsof", &["-nP", "-iTCP", "-sTCP:LISTEN", "-F", "pcn"]).or_else(|e| if e == "failed" { Ok(String::new()) } else { Err(e) })?;
    let mut v = parse_lsof(&raw);
    for l in &mut v {
        l.container = containers.iter().find(|c| c.host_ports().contains(&l.port)).map(|c| c.name.clone());
    }
    Ok(v)
}

/// Established connections to a port.
pub fn connections(port: u16) -> usize {
    run("lsof", &["-nP", &format!("-iTCP:{port}"), "-sTCP:ESTABLISHED", "-t"]).map(|s| s.lines().count()).unwrap_or(0)
}

/// Asks a process to end (SIGTERM).
pub fn terminate(pid: u32) -> Result<String, String> {
    if pid <= 1 {
        return Err("Won't stop that process".into());
    }
    let r = unsafe { libc::kill(pid as i32, libc::SIGTERM) };
    if r == 0 {
        Ok(format!("Asked pid {pid} to quit"))
    } else {
        Err(format!("Could not stop pid {pid}: {}", std::io::Error::last_os_error()))
    }
}

// ------------------------------------------------------------------ system

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Proc {
    pub pid: u32,
    pub cpu: f32,
    pub rss_mb: f32,
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sys {
    pub chip: String,
    pub cores: (u32, u32),
    pub cpu: f32,
    pub load: [f32; 3],
    /// GB: app, wired, compressed, cached, free; total
    pub mem: [f32; 5],
    pub mem_total: f32,
    pub swap: (f32, f32),
    pub gpu: f32,
    pub disk: (f32, f32),
    pub net_in: f32,
    pub net_out: f32,
    pub battery: Option<(u32, String, String)>,
    pub procs: Vec<Proc>,
    /// each core's ticks so far: (busy, total)
    pub cores_ticks: Vec<(u64, u64)>,
    /// bytes read and written by the disks so far
    pub disk_bytes: (u64, u64),
}

/// Each core's busy and total CPU ticks since boot (`host_processor_info`).
#[cfg(target_os = "macos")]
pub fn core_ticks() -> Vec<(u64, u64)> {
    let mut count: libc::natural_t = 0;
    let mut info: libc::processor_info_array_t = std::ptr::null_mut();
    let mut n: libc::mach_msg_type_number_t = 0;
    #[allow(deprecated)]
    let r = unsafe { libc::host_processor_info(libc::mach_host_self(), libc::PROCESSOR_CPU_LOAD_INFO, &mut count, &mut info, &mut n) };
    if r != 0 || info.is_null() {
        return Vec::new();
    }
    let per = libc::CPU_STATE_MAX as usize;
    let mut out = Vec::with_capacity(count as usize);
    for i in 0..count as usize {
        let t = |k: i32| unsafe { *info.add(i * per + k as usize) } as u64;
        let (user, sys, idle, nice) = (t(libc::CPU_STATE_USER), t(libc::CPU_STATE_SYSTEM), t(libc::CPU_STATE_IDLE), t(libc::CPU_STATE_NICE));
        out.push((user + sys + nice, user + sys + nice + idle));
    }
    #[allow(deprecated)]
    unsafe {
        libc::vm_deallocate(libc::mach_task_self(), info as libc::vm_address_t, (n as usize * std::mem::size_of::<libc::integer_t>()) as libc::vm_size_t);
    }
    out
}

#[cfg(not(target_os = "macos"))]
pub fn core_ticks() -> Vec<(u64, u64)> {
    Vec::new()
}

/// `ioreg` block storage statistics: bytes read and written since boot, over all disks.
pub fn parse_ioreg_disk(raw: &str) -> (u64, u64) {
    let sum = |k: &str| raw.split(k).skip(1).filter_map(|r| r.trim_start_matches('=').split(|c: char| !c.is_ascii_digit()).next()?.parse::<u64>().ok()).sum::<u64>();
    (sum("\"Bytes (Read)\""), sum("\"Bytes (Write)\""))
}

/// Per-core use 0..1 between two tick readings.
pub fn core_use(prev: &[(u64, u64)], now: &[(u64, u64)]) -> Vec<f32> {
    now.iter().zip(prev.iter()).map(|(n, p)| (n.0.saturating_sub(p.0)) as f32 / (n.1.saturating_sub(p.1)).max(1) as f32).collect()
}

fn sysctl(k: &str) -> String {
    run("sysctl", &["-n", k]).unwrap_or_default().trim().to_string()
}

/// `vm_stat`: GB in (app, wired, compressed, cached, free).
pub fn parse_vm_stat(raw: &str) -> [f32; 5] {
    let page = raw.split("page size of ").nth(1).and_then(|s| s.split_whitespace().next()).and_then(|n| n.parse::<f64>().ok()).unwrap_or(16384.0);
    let get = |k: &str| raw.lines().find(|l| l.starts_with(k)).and_then(|l| l.rsplit(':').next()).and_then(|n| n.trim().trim_end_matches('.').parse::<f64>().ok()).unwrap_or(0.0) * page / 1e9;
    let app = (get("Anonymous pages") - get("Pages purgeable")).max(0.0);
    [app as f32, get("Pages wired down") as f32, get("Pages occupied by compressor") as f32, (get("File-backed pages") + get("Pages purgeable")) as f32, get("Pages free") as f32]
}

/// `netstat -ib`: total bytes in and out over the physical interfaces.
pub fn parse_netstat(raw: &str) -> (u64, u64) {
    let mut seen: Vec<String> = Vec::new();
    let (mut i, mut o) = (0u64, 0u64);
    for l in raw.lines().skip(1) {
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() < 10 || !f[0].starts_with("en") || seen.contains(&f[0].to_string()) || !f[2].starts_with("<Link") {
            continue;
        }
        seen.push(f[0].to_string());
        // Name Mtu Network Address Ipkts Ierrs Ibytes Opkts Oerrs Obytes
        let n = f.len();
        i += f[n - 5].parse::<u64>().unwrap_or(0);
        o += f[n - 2].parse::<u64>().unwrap_or(0);
    }
    (i, o)
}

pub fn parse_ps(raw: &str) -> Vec<Proc> {
    raw.lines()
        .skip(1)
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            let pid = f.next()?.parse().ok()?;
            let cpu = f.next()?.parse().ok()?;
            let rss: f32 = f.next()?.parse().ok()?;
            let cmd = f.collect::<Vec<_>>().join(" ");
            let name = std::path::Path::new(&cmd).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or(cmd);
            Some(Proc { pid, cpu, rss_mb: rss / 1024.0, name })
        })
        .collect()
}

/// One sample of the machine (network in and out need the previous byte totals).
/// Byte totals in and out, and when they were read.
pub type NetMark = Option<((u64, u64), std::time::Instant)>;

pub fn sample(prev_net: NetMark) -> (Sys, NetMark) {
    let mut s = Sys { chip: sysctl("machdep.cpu.brand_string"), ..Default::default() };
    s.cores = (sysctl("hw.perflevel0.physicalcpu").parse().unwrap_or(0), sysctl("hw.perflevel1.physicalcpu").parse().unwrap_or(0));
    let ncpu = sysctl("hw.ncpu").parse::<f32>().unwrap_or(1.0).max(1.0);
    let la = sysctl("vm.loadavg");
    let mut it = la.trim_matches(|c| c == '{' || c == '}' || c == ' ').split_whitespace().map(|x| x.parse::<f32>().unwrap_or(0.0));
    s.load = [it.next().unwrap_or(0.0), it.next().unwrap_or(0.0), it.next().unwrap_or(0.0)];
    let ps = run("ps", &["-Ao", "pid,pcpu,rss,comm", "-r"]).unwrap_or_default();
    let mut procs = parse_ps(&ps);
    s.cpu = (procs.iter().map(|p| p.cpu).sum::<f32>() / ncpu / 100.0).clamp(0.0, 1.0);
    procs.truncate(12);
    s.procs = procs;
    s.mem = parse_vm_stat(&run("vm_stat", &[]).unwrap_or_default());
    s.mem_total = sysctl("hw.memsize").parse::<f64>().unwrap_or(0.0) as f32 / 1e9;
    let sw = sysctl("vm.swapusage");
    let mb = |k: &str| sw.split(k).nth(1).and_then(|r| r.split_whitespace().next()).map(|n| n.trim_end_matches('M').parse::<f32>().unwrap_or(0.0) / 1024.0).unwrap_or(0.0);
    s.swap = (mb("used = "), mb("total = "));
    s.gpu = crate::sysstats::gpu();
    let df = run("df", &["-k", "/"]).unwrap_or_default();
    if let Some(l) = df.lines().nth(1) {
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() > 3 {
            let gb = |x: &str| x.parse::<f32>().unwrap_or(0.0) / 1e6;
            s.disk = (gb(f[1]) - gb(f[3]), gb(f[1]));
        }
    }
    let now = std::time::Instant::now();
    let net = parse_netstat(&run("netstat", &["-ib"]).unwrap_or_default());
    if let Some(((i0, o0), t0)) = prev_net {
        let dt = now.duration_since(t0).as_secs_f32().max(0.1);
        s.net_in = net.0.saturating_sub(i0) as f32 / dt;
        s.net_out = net.1.saturating_sub(o0) as f32 / dt;
    }
    s.cores_ticks = core_ticks();
    s.disk_bytes = parse_ioreg_disk(&run("ioreg", &["-c", "IOBlockStorageDriver", "-r", "-w0", "-d", "1"]).unwrap_or_default());
    let b = run("pmset", &["-g", "batt"]).unwrap_or_default();
    s.battery = b.lines().nth(1).and_then(|l| {
        let pct = l.split('\t').nth(1)?.split('%').next()?.trim().parse().ok()?;
        let parts: Vec<&str> = l.split(';').map(|p| p.trim()).collect();
        Some((pct, parts.get(1).unwrap_or(&"").to_string(), parts.get(2).map(|r| r.split(" present").next().unwrap_or(r).to_string()).unwrap_or_default()))
    });
    (s, Some((net, now)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_published_ports_and_lsof() {
        assert_eq!(parse_published("0.0.0.0:8090->80/tcp, [::]:8090->80/tcp, 0.0.0.0:5432->5432/tcp"), vec![(8090, "80/tcp".into()), (5432, "5432/tcp".into())]);
        let l = parse_lsof("p612\ncsphered\nn127.0.0.1:7878\np2210\ncadb\nn*:5037\nn[::1]:5037\n");
        assert_eq!(l.iter().map(|x| (x.port, x.pid, x.command.as_str())).collect::<Vec<_>>(), vec![(5037, 2210, "adb"), (7878, 612, "sphered")]);
    }

    #[test]
    fn reads_disk_bytes_and_core_use() {
        let raw = "\"Statistics\" = {\"Bytes (Read)\"=1000,\"Operations (Write)\"=4,\"Bytes (Write)\"=500}\n\"Statistics\" = {\"Bytes (Read)\"=24,\"Bytes (Write)\"=6}";
        assert_eq!(parse_ioreg_disk(raw), (1024, 506));
        assert_eq!(core_use(&[(10, 100), (0, 100)], &[(60, 200), (0, 200)]), vec![0.5, 0.0]);
        assert!(!core_ticks().is_empty() || !cfg!(target_os = "macos"));
    }

    #[test]
    fn reads_vm_stat_ps_and_netstat() {
        let vm = "Mach Virtual Memory Statistics: (page size of 16384 bytes)\nPages free:  100000.\nPages wired down:  200000.\nAnonymous pages:  400000.\nPages purgeable:  0.\nFile-backed pages:  50000.\nPages occupied by compressor:  10000.\n";
        let m = parse_vm_stat(vm);
        assert!((m[0] - 6.5536).abs() < 0.01 && (m[4] - 1.6384).abs() < 0.01);
        let p = parse_ps("  PID  %CPU    RSS COMM\n51233 151.0 765952 /usr/bin/cargo\n");
        assert_eq!((p[0].pid, p[0].name.as_str()), (51233, "cargo"));
        assert!((p[0].rss_mb - 748.0).abs() < 0.1);
        let n = parse_netstat("Name  Mtu   Network       Address            Ipkts Ierrs     Ibytes    Opkts Oerrs     Obytes  Coll\nen0   1500  <Link#11>   aa:bb:cc:dd:ee:ff  100     0     5000    50     0     2000     0\nen0   1500  192.168.1     192.168.1.2  100     -     5000    50     -     2000     -\n");
        assert_eq!(n, (5000, 2000));
    }
}
