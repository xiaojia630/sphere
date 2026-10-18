//! What the Containers, Ports & logs and System apps show, read from the machine's own tools: the `docker` CLI
//! (Docker Desktop or OrbStack), `lsof`, `ps`, `vm_stat`, `sysctl`, `netstat`, `df`, `pmset`, `ioreg`. Argv only,
//! never a shell. Everything here blocks, so the apps call it from background threads.
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Runs a short local tool (ps, sysctl, vm_stat…): stdout on success, the first stderr line on failure.
fn run(cmd: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(cmd).args(args).output().map_err(|e| format!("{cmd}: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let e = String::from_utf8_lossy(&out.stderr);
        Err(e.lines().find(|l| !l.trim().is_empty()).unwrap_or("failed").trim().to_string())
    }
}

/// Where the docker CLI is: on PATH, or where Docker Desktop, OrbStack and Homebrew put it (an app started from
/// the Finder has a short PATH).
fn docker_bin() -> &'static str {
    static BIN: OnceLock<String> = OnceLock::new();
    BIN.get_or_init(|| {
        let home = std::env::var("HOME").unwrap_or_default();
        let mut cands: Vec<String> = std::env::var("PATH").unwrap_or_default().split(':').filter(|p| !p.is_empty()).map(|p| format!("{p}/docker")).collect();
        cands.extend(["/usr/local/bin/docker".into(), "/opt/homebrew/bin/docker".into(), format!("{home}/.orbstack/bin/docker"), "/Applications/Docker.app/Contents/Resources/bin/docker".into()]);
        cands.into_iter().find(|p| std::path::Path::new(p).is_file()).unwrap_or_else(|| "docker".into())
    })
}

/// Runs a tool that can hang (docker, lsof) with a deadline: (stdout, stderr), or the first stderr line when it
/// fails. A tool that overruns is killed.
fn exec(cmd: &str, args: &[&str], secs: u64) -> Result<(String, String), String> {
    let program = if cmd == "docker" { docker_bin() } else { cmd };
    let mut child = Command::new(program).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| format!("{cmd}: {e}"))?;
    let mut so = child.stdout.take().ok_or("no stdout")?;
    let mut se = child.stderr.take().ok_or("no stderr")?;
    let ho = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = so.read_to_end(&mut b);
        b
    });
    let he = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = se.read_to_end(&mut b);
        b
    });
    let deadline = Instant::now() + Duration::from_secs(secs);
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(4)),
            Err(e) => return Err(format!("{cmd}: {e}")),
        }
    };
    let out = String::from_utf8_lossy(&ho.join().unwrap_or_default()).into_owned();
    let err = String::from_utf8_lossy(&he.join().unwrap_or_default()).into_owned();
    match status {
        None => Err(format!("{cmd} did not answer in {secs} s")),
        Some(s) if s.success() => Ok((out, err)),
        Some(_) => Err(err.lines().find(|l| !l.trim().is_empty()).unwrap_or("failed").trim().to_string()),
    }
}

fn run_t(cmd: &str, args: &[&str], secs: u64) -> Result<String, String> {
    exec(cmd, args, secs).map(|o| o.0)
}

/// Bytes from "126.4MiB", "1.2GB", "512B", "3kB".
pub fn parse_bytes(s: &str) -> f64 {
    let s = s.trim();
    let num: String = s.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    let v: f64 = num.parse().unwrap_or(0.0);
    let unit = s[num.len()..].trim().to_ascii_lowercase();
    v * match unit.as_str() {
        "kib" => 1024.0,
        "mib" => 1048576.0,
        "gib" => 1073741824.0,
        "tib" => 1099511627776.0,
        "kb" => 1e3,
        "mb" => 1e6,
        "gb" => 1e9,
        "tb" => 1e12,
        _ => 1.0,
    }
}

/// "512 B", "126 MB", "1.4 GB" in 1024s, the way Activity Monitor counts memory.
pub fn fmt_bytes(b: f64) -> String {
    if b <= 0.0 {
        "0 B".to_string()
    } else if b >= 1073741824.0 {
        format!("{:.1} GB", b / 1073741824.0)
    } else if b >= 1048576.0 {
        format!("{:.0} MB", b / 1048576.0)
    } else if b >= 1024.0 {
        format!("{:.0} kB", b / 1024.0)
    } else {
        format!("{b:.0} B")
    }
}

/// Removes terminal colour codes and other control characters from a log line.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\u{1b}' {
            if it.peek() == Some(&'[') {
                it.next();
                for n in it.by_ref() {
                    if n.is_ascii_alphabetic() || n == '~' {
                        break;
                    }
                }
            } else {
                it.next();
            }
        } else if c == '\t' {
            out.push_str("  ");
        } else if !c.is_control() {
            out.push(c);
        }
    }
    out
}

/// Seconds since the epoch of "2026-10-17T07:10:00.123456789Z" (UTC), and the milliseconds.
pub fn epoch_from_iso(ts: &str) -> Option<(i64, u32)> {
    let b = ts.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[10] != b'T' {
        return None;
    }
    let n = |a: usize, z: usize| ts.get(a..z)?.parse::<i64>().ok();
    let (y, m, d, h, mi, s) = (n(0, 4)?, n(5, 7)?, n(8, 10)?, n(11, 13)?, n(14, 16)?, n(17, 19)?);
    // days from civil
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    let ms = ts.get(20..23).and_then(|x| x.parse::<u32>().ok()).filter(|_| b[19] == b'.').unwrap_or(0);
    Some((days * 86400 + h * 3600 + mi * 60 + s, ms))
}

/// "07:10:00.123" in this machine's time zone for a docker log timestamp (UTC).
pub fn local_hms(ts: &str) -> String {
    let Some((secs, ms)) = epoch_from_iso(ts) else { return String::new() };
    let t = secs as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
        return String::new();
    }
    format!("{:02}:{:02}:{:02}.{:03}", tm.tm_hour, tm.tm_min, tm.tm_sec, ms)
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
    /// "126.4MiB / 7.6GiB" as docker prints it
    pub mem: String,
    /// bytes in use
    pub mem_used: f64,
    pub mem_frac: f32,
}

impl Container {
    pub fn unhealthy(&self) -> bool {
        self.health.as_deref() == Some("unhealthy")
    }
    pub fn running(&self) -> bool {
        self.state == "running"
    }
}

fn jstr(v: &serde_json::Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

/// One line of `docker ps -a --format '{{json .}}'`.
pub fn parse_ps_line(l: &str) -> Option<Container> {
    let j = serde_json::from_str::<serde_json::Value>(l).ok()?;
    let labels = jstr(&j, "Labels");
    let label = |k: &str| labels.split(',').find_map(|kv| kv.strip_prefix(k).and_then(|r| r.strip_prefix('='))).unwrap_or("").to_string();
    let status = jstr(&j, "Status");
    let state = jstr(&j, "State");
    // a stopped container keeps its last health word; only a running one has a health state
    let hs = jstr(&j, "HealthStatus");
    let health = if state != "running" {
        None
    } else if !hs.is_empty() && hs != "none" {
        Some(hs)
    } else if status.contains("(unhealthy)") {
        Some("unhealthy".into())
    } else if status.contains("(healthy)") {
        Some("healthy".into())
    } else if status.contains("(health: starting)") {
        Some("starting".into())
    } else {
        None
    };
    let id = jstr(&j, "ID");
    if id.is_empty() {
        return None;
    }
    Some(Container { id, name: jstr(&j, "Names"), image: jstr(&j, "Image"), state, status, ports: jstr(&j, "Ports"), project: label("com.docker.compose.project"), service: label("com.docker.compose.service"), health, ..Default::default() })
}

/// Fills in cpu and memory from `docker stats --no-stream --format '{{json .}}'` output.
pub fn apply_stats(v: &mut [Container], stats: &str) {
    for l in stats.lines() {
        let Ok(j) = serde_json::from_str::<serde_json::Value>(l) else { continue };
        let id = jstr(&j, "ID");
        if id.is_empty() {
            continue;
        }
        if let Some(c) = v.iter_mut().find(|c| id.starts_with(&c.id) || c.id.starts_with(&id)) {
            c.cpu = jstr(&j, "CPUPerc").trim_end_matches('%').parse().unwrap_or(0.0);
            c.mem = jstr(&j, "MemUsage");
            c.mem_used = parse_bytes(c.mem.split(" / ").next().unwrap_or(""));
            c.mem_frac = jstr(&j, "MemPerc").trim_end_matches('%').parse::<f32>().unwrap_or(0.0) / 100.0;
        }
    }
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
    // SPHERE_DOCKER_FILTER (e.g. "name=demo-") limits what's listed: recordings show only demo containers
    let filter = std::env::var("SPHERE_DOCKER_FILTER").ok();
    let mut args = vec!["ps", "-a", "--format", "{{json .}}"];
    if let Some(f) = filter.as_deref() {
        args.extend(["--filter", f]);
    }
    let raw = run_t("docker", &args, 10)?;
    let mut v: Vec<Container> = raw.lines().filter_map(parse_ps_line).collect();
    if stats && v.iter().any(|c| c.running()) {
        if let Ok(stats) = run_t("docker", &["stats", "--no-stream", "--format", "{{json .}}"], 15) {
            apply_stats(&mut v, &stats);
        }
    }
    sort_containers(&mut v);
    Ok(v)
}

/// running first, then by compose project and name
pub fn sort_containers(v: &mut [Container]) {
    v.sort_by(|a, b| (!a.running(), &a.project, &a.name).cmp(&(!b.running(), &b.project, &b.name)));
}

/// What a docker failure means for the person: a headline and what to do.
pub fn docker_problem(e: &str) -> (String, String) {
    let l = e.to_ascii_lowercase();
    if l.contains("cannot connect") || l.contains("daemon running") || l.contains("connection refused") || l.contains("docker.sock") {
        ("Docker isn't running".into(), "Start Docker Desktop or OrbStack. This list fills in by itself.".into())
    } else if l.contains("no such file") || l.contains("os error 2") {
        ("Docker isn't installed".into(), "Install Docker Desktop or OrbStack, then open this app again.".into())
    } else if l.contains("did not answer") || l.contains("timed out") {
        ("Docker is not answering".into(), "It may still be starting. Trying again every few seconds.".into())
    } else if l.contains("permission denied") {
        ("Docker refused access".into(), "This user can't reach the Docker socket.".into())
    } else {
        ("Docker can't be read".into(), e.to_string())
    }
}

/// The last `n` log lines of a container, stdout and stderr in time order (each line starts with its timestamp).
pub fn logs(id: &str, n: usize) -> Result<Vec<String>, String> {
    let (o, e) = exec("docker", &["logs", "--tail", &n.to_string(), "--timestamps", id], 8)?;
    Ok(merge_logs(&o, &e, n))
}

pub fn merge_logs(stdout: &str, stderr: &str, n: usize) -> Vec<String> {
    let mut lines: Vec<String> = stdout.lines().chain(stderr.lines()).filter(|l| !l.trim().is_empty()).map(strip_ansi).collect();
    lines.sort_by(|a, b| a.split(' ').next().cmp(&b.split(' ').next()));
    let k = lines.len().saturating_sub(n);
    lines.split_off(k)
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
        DockerOp::Start(n) => run_t("docker", &["start", &n], 60).map(|_| format!("Started {n}")),
        DockerOp::Stop(n) => run_t("docker", &["stop", &n], 90).map(|_| format!("Stopped {n}")),
        DockerOp::Restart(n) => run_t("docker", &["restart", &n], 90).map(|_| format!("Restarted {n}")),
        DockerOp::Remove(n) => run_t("docker", &["rm", "-f", &n], 60).map(|_| format!("Removed {n}")),
        DockerOp::Pull(i) => run_t("docker", &["pull", "-q", &i], 900).map(|_| format!("Pulled {i}. Restart the container to use it")),
    }
}

/// "0.0.0.0:8090->80/tcp, [::]:8090->80/tcp, 0.0.0.0:8000-8002->8000-8002/tcp" → [(8090, "80/tcp"), (8000, "8000/tcp"), …],
/// each host port once.
pub fn parse_published(ports: &str) -> Vec<(u16, String)> {
    let mut out: Vec<(u16, String)> = Vec::new();
    for p in ports.split(", ") {
        let Some((host, inner)) = p.split_once("->") else { continue };
        let Some(hp) = host.rsplit(':').next() else { continue };
        let (lo, hi) = match hp.split_once('-') {
            Some((a, b)) => (a.parse::<u16>().ok(), b.parse::<u16>().ok()),
            None => (hp.parse::<u16>().ok(), hp.parse::<u16>().ok()),
        };
        let (Some(lo), Some(hi)) = (lo, hi) else { continue };
        let proto = inner.rsplit_once('/').map(|r| r.1).unwrap_or("tcp");
        let inner_lo = inner.split(['-', '/']).next().and_then(|n| n.parse::<u16>().ok());
        for (k, port) in (lo..=hi.min(lo.saturating_add(63))).enumerate() {
            if !out.iter().any(|o| o.0 == port) {
                let label = if lo == hi { inner.to_string() } else { format!("{}/{proto}", inner_lo.map(|b| b + k as u16).unwrap_or(0)) };
                out.push((port, label));
            }
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
    /// its compose service, and the port inside it ("80/tcp")
    pub service: String,
    pub inner: String,
}

/// `lsof +c 0 -nP -iTCP -sTCP:LISTEN -F pcn` output: one listener per port.
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
                    out.push(Listener { port, pid, command: cmd.clone(), addr: addr.to_string(), ..Default::default() });
                }
            }
            _ => {}
        }
    }
    out.sort_by_key(|l| l.port);
    out
}

/// Names the container behind each listener, from the ports the running containers publish.
pub fn map_containers(v: &mut [Listener], containers: &[Container]) {
    for l in v {
        let hit = containers.iter().filter(|c| c.running()).find_map(|c| parse_published(&c.ports).into_iter().find(|p| p.0 == l.port).map(|p| (c, p.1)));
        if let Some((c, inner)) = hit {
            l.container = Some(c.name.clone());
            l.service = if c.service.is_empty() { c.name.clone() } else { c.service.clone() };
            l.inner = inner;
        }
    }
}

pub fn listeners(containers: &[Container]) -> Result<Vec<Listener>, String> {
    // lsof exits 1 with no output when nothing matches
    let raw = run_t("lsof", &["+c", "0", "-nP", "-iTCP", "-sTCP:LISTEN", "-F", "pcn"], 10).or_else(|e| if e == "failed" { Ok(String::new()) } else { Err(e) })?;
    let mut v = parse_lsof(&raw);
    map_containers(&mut v, containers);
    Ok(v)
}

/// Established connections that came in to `port` (lsof -F n lines "local->remote").
pub fn parse_connections(raw: &str, port: u16) -> usize {
    raw.lines().filter_map(|l| l.strip_prefix('n')).filter(|n| n.split_once("->").is_some_and(|(local, _)| local.rsplit_once(':').is_some_and(|(_, p)| p.parse::<u16>().ok() == Some(port)))).count()
}

/// Established connections to a port.
pub fn connections(port: u16) -> usize {
    run_t("lsof", &["-nP", &format!("-iTCP:{port}"), "-sTCP:ESTABLISHED", "-F", "n"], 6).map(|s| parse_connections(&s, port)).unwrap_or(0)
}

/// Asks a process to end (SIGTERM).
pub fn terminate(pid: u32) -> Result<String, String> {
    if pid <= 1 || pid == std::process::id() {
        return Err("Won't stop that process".into());
    }
    let r = unsafe { libc::kill(pid as i32, libc::SIGTERM) };
    if r == 0 {
        Ok(format!("Asked pid {pid} to quit"))
    } else {
        Err(format!("Could not stop pid {pid}: {}", std::io::Error::last_os_error()))
    }
}

/// Stops the listener on `port` after checking that `pid` still owns it (the list is a few seconds old).
pub fn terminate_listener(port: u16, pid: u32, name: &str) -> Result<String, String> {
    let owners = run_t("lsof", &["-nP", &format!("-iTCP:{port}"), "-sTCP:LISTEN", "-t"], 6).unwrap_or_default();
    if !owners.lines().any(|l| l.trim().parse::<u32>().ok() == Some(pid)) {
        return Err(format!(":{port} is no longer held by {name}"));
    }
    terminate(pid).map(|_| format!("Asked {name} (pid {pid}) to quit"))
}

/// Stops a process after checking that `pid` still has the name that was on screen.
pub fn terminate_named(pid: u32, name: &str) -> Result<String, String> {
    let now = run("ps", &["-p", &pid.to_string(), "-o", "comm="]).unwrap_or_default();
    let now = std::path::Path::new(now.trim()).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    if now.is_empty() {
        return Err(format!("{name} has already gone"));
    }
    if now != name {
        return Err(format!("pid {pid} is now {now}, not {name}"));
    }
    terminate(pid).map(|_| format!("Asked {name} (pid {pid}) to quit"))
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
    /// performance and efficiency cores
    pub cores: (u32, u32),
    pub load: [f32; 3],
    /// GB: app, wired, compressed, cached, free
    pub mem: [f32; 5],
    pub mem_total: f32,
    pub swap: (f32, f32),
    /// None when the machine doesn't report it
    pub gpu: Option<f32>,
    pub disk: (f32, f32),
    pub net_in: f32,
    pub net_out: f32,
    /// the power source ("AC Power", "Battery Power") and the battery: percent, state, detail
    pub power: String,
    pub battery: Option<(u32, String, String)>,
    /// the busiest by CPU and the largest by memory
    pub procs: Vec<Proc>,
    /// each core's ticks so far: (busy, total)
    pub cores_ticks: Vec<(u64, u64)>,
    /// bytes read and written by the disks so far
    pub disk_bytes: (u64, u64),
    /// when the counters above were read
    pub at: Option<Instant>,
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
        // the kernel's counters are unsigned 32-bit values held in an int
        let t = |k: i32| unsafe { *info.add(i * per + k as usize) } as u32 as u64;
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

/// Ticks between two readings; the counters are 32-bit and wrap.
fn tick_delta(now: u64, prev: u64) -> u64 {
    (now.wrapping_sub(prev)) & 0xFFFF_FFFF
}

/// Per-core use 0..1 between two tick readings.
pub fn core_use(prev: &[(u64, u64)], now: &[(u64, u64)]) -> Vec<f32> {
    now.iter().zip(prev.iter()).map(|(n, p)| (tick_delta(n.0, p.0) as f32 / tick_delta(n.1, p.1).max(1) as f32).clamp(0.0, 1.0)).collect()
}

/// Use 0..1 of the whole processor between two tick readings.
pub fn total_use(prev: &[(u64, u64)], now: &[(u64, u64)]) -> f32 {
    let (busy, total) = now.iter().zip(prev.iter()).fold((0u64, 0u64), |a, (n, p)| (a.0 + tick_delta(n.0, p.0), a.1 + tick_delta(n.1, p.1)));
    (busy as f32 / total.max(1) as f32).clamp(0.0, 1.0)
}

fn sysctl(k: &str) -> String {
    run("sysctl", &["-n", k]).unwrap_or_default().trim().to_string()
}

/// `vm_stat`: GB (1024^3, as Activity Monitor shows) in (app, wired, compressed, cached, free). "Used" is app + wired + compressed, as Activity Monitor
/// counts it.
pub fn parse_vm_stat(raw: &str) -> [f32; 5] {
    let page = raw.split("page size of ").nth(1).and_then(|s| s.split_whitespace().next()).and_then(|n| n.parse::<f64>().ok()).unwrap_or(16384.0);
    let get = |k: &str| raw.lines().find(|l| l.starts_with(k)).and_then(|l| l.rsplit(':').next()).and_then(|n| n.trim().trim_end_matches('.').parse::<f64>().ok()).unwrap_or(0.0) * page / 1073741824.0;
    let app = (get("Anonymous pages") - get("Pages purgeable")).max(0.0);
    [app as f32, get("Pages wired down") as f32, get("Pages occupied by compressor") as f32, (get("File-backed pages") + get("Pages purgeable")) as f32, get("Pages free") as f32]
}

/// `netstat -inb` (-n: without it every address is looked up in DNS and it takes seconds): total bytes in and out over the physical interfaces. Columns are found from the header and
/// counted from the right, since a row without a hardware address is one field short.
pub fn parse_netstat(raw: &str) -> (u64, u64) {
    let mut lines = raw.lines();
    let Some(header) = lines.next() else { return (0, 0) };
    let h: Vec<&str> = header.split_whitespace().collect();
    let (Some(ib), Some(ob)) = (h.iter().position(|x| *x == "Ibytes"), h.iter().position(|x| *x == "Obytes")) else { return (0, 0) };
    let mut seen: Vec<String> = Vec::new();
    let (mut i, mut o) = (0u64, 0u64);
    for l in lines {
        let f: Vec<&str> = l.split_whitespace().collect();
        // link rows carry "<Link#n>"; the address rows under them repeat the same counters
        if f.len() < h.len() - 1 || !f[0].starts_with("en") || seen.contains(&f[0].to_string()) || !f.get(2).is_some_and(|n| n.starts_with("<Link")) {
            continue;
        }
        seen.push(f[0].to_string());
        let n = f.len();
        i += f[n - (h.len() - ib)].parse::<u64>().unwrap_or(0);
        o += f[n - (h.len() - ob)].parse::<u64>().unwrap_or(0);
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

/// The `n` busiest by CPU and the `n` largest by memory, each process once.
pub fn top_procs(mut all: Vec<Proc>, n: usize) -> Vec<Proc> {
    all.sort_by(|a, b| b.cpu.total_cmp(&a.cpu));
    let mut out: Vec<Proc> = all.iter().take(n).cloned().collect();
    all.sort_by(|a, b| b.rss_mb.total_cmp(&a.rss_mb));
    for p in all.into_iter().take(n) {
        if !out.iter().any(|o| o.pid == p.pid) {
            out.push(p);
        }
    }
    out
}

/// `df -k /`: (used, total) GB, used being what is not available (the whole APFS container, as Finder counts it).
pub fn parse_df(raw: &str) -> (f32, f32) {
    let Some(l) = raw.lines().nth(1) else { return (0.0, 0.0) };
    let f: Vec<&str> = l.split_whitespace().collect();
    if f.len() < 4 {
        return (0.0, 0.0);
    }
    let gb = |x: &str| x.parse::<f64>().unwrap_or(0.0) * 1024.0 / 1e9;
    ((gb(f[1]) - gb(f[3])).max(0.0) as f32, gb(f[1]) as f32)
}

/// `pmset -g batt`: the power source, and the battery when there is one: percent, state, detail.
pub fn parse_pmset(raw: &str) -> (String, Option<(u32, String, String)>) {
    let source = raw.lines().next().and_then(|l| l.split('\'').nth(1)).unwrap_or("").to_string();
    let battery = raw.lines().find(|l| l.contains('%') && l.contains(';')).and_then(|l| {
        let pct = l.split('%').next()?.rsplit(|c: char| !c.is_ascii_digit()).next()?.parse().ok()?;
        let parts: Vec<&str> = l.split(';').map(|p| p.trim()).collect();
        let detail = parts.get(2).map(|r| r.split(" present").next().unwrap_or(r).trim().to_string()).unwrap_or_default();
        let detail = if detail.contains("no estimate") { "calculating".to_string() } else { detail };
        Some((pct, parts.get(1).unwrap_or(&"").to_string(), detail))
    });
    (source, battery)
}

/// The GPU's utilisation 0..1 from `ioreg -r -d 1 -w 0 -c IOAccelerator`, when it reports one.
pub fn parse_gpu(raw: &str) -> Option<f32> {
    let v = raw.split("\"Device Utilization %\"=").skip(1).filter_map(|s| s.split(|c: char| !c.is_ascii_digit()).next()?.parse::<f32>().ok()).fold(None, |a: Option<f32>, v| Some(a.map_or(v, |a| a.max(v))))?;
    Some((v / 100.0).clamp(0.0, 1.0))
}

/// What never changes while the app runs: the chip, its cores, how many there are, and the memory installed.
struct Fixed {
    chip: String,
    cores: (u32, u32),
    mem_total: f32,
}

fn fixed() -> &'static Fixed {
    static F: OnceLock<Fixed> = OnceLock::new();
    F.get_or_init(|| Fixed {
        chip: sysctl("machdep.cpu.brand_string"),
        cores: (sysctl("hw.perflevel0.physicalcpu").parse().unwrap_or(0), sysctl("hw.perflevel1.physicalcpu").parse().unwrap_or(0)),
        mem_total: sysctl("hw.memsize").parse::<f64>().unwrap_or(0.0) as f32 / 1073741824.0,
    })
}

/// Byte totals in and out, and when they were read.
pub type NetMark = Option<((u64, u64), Instant)>;

/// One sample of the machine. Everything that moves is read each time; the disk, swap and battery (slow to change)
/// are read when `slow` is set and otherwise carried over from `old`. The network rate needs the previous totals.
pub fn sample(prev_net: NetMark, old: Option<&Sys>, slow: bool) -> (Sys, NetMark) {
    let fx = fixed();
    let mut s = Sys { chip: fx.chip.clone(), cores: fx.cores, mem_total: fx.mem_total, ..Default::default() };
    let la = sysctl("vm.loadavg");
    let mut it = la.trim_matches(|c| c == '{' || c == '}' || c == ' ').split_whitespace().map(|x| x.parse::<f32>().unwrap_or(0.0));
    s.load = [it.next().unwrap_or(0.0), it.next().unwrap_or(0.0), it.next().unwrap_or(0.0)];
    s.procs = top_procs(parse_ps(&run("ps", &["-Ao", "pid,pcpu,rss,comm"]).unwrap_or_default()), 40);
    s.mem = parse_vm_stat(&run("vm_stat", &[]).unwrap_or_default());
    s.gpu = parse_gpu(&run("ioreg", &["-r", "-d", "1", "-w", "0", "-c", "IOAccelerator"]).unwrap_or_default());
    let net = parse_netstat(&run("netstat", &["-inb"]).unwrap_or_default());
    s.cores_ticks = core_ticks();
    s.disk_bytes = parse_ioreg_disk(&run("ioreg", &["-c", "IOBlockStorageDriver", "-r", "-w0", "-d", "1"]).unwrap_or_default());
    let now = Instant::now();
    s.at = Some(now);
    if let Some(((i0, o0), t0)) = prev_net {
        let dt = now.duration_since(t0).as_secs_f32().max(0.1);
        s.net_in = net.0.saturating_sub(i0) as f32 / dt;
        s.net_out = net.1.saturating_sub(o0) as f32 / dt;
    }
    match old {
        Some(o) if !slow => {
            s.swap = o.swap;
            s.disk = o.disk;
            s.power = o.power.clone();
            s.battery = o.battery.clone();
        }
        _ => {
            let sw = sysctl("vm.swapusage");
            let mb = |k: &str| sw.split(k).nth(1).and_then(|r| r.split_whitespace().next()).map(|n| n.trim_end_matches('M').parse::<f32>().unwrap_or(0.0) / 1024.0).unwrap_or(0.0);
            s.swap = (mb("used = "), mb("total = "));
            s.disk = parse_df(&run("df", &["-k", "/"]).unwrap_or_default());
            (s.power, s.battery) = parse_pmset(&run("pmset", &["-g", "batt"]).unwrap_or_default());
        }
    }
    (s, Some((net, now)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PS_RUNNING: &str = r#"{"Command":"\"nginx\"","HealthStatus":"healthy","ID":"d4b8e345e18b","Image":"nginx:alpine","Labels":"com.docker.compose.project.config_files=/a/compose.yaml,com.docker.compose.project=shop,com.docker.compose.service=web,maintainer=NGINX Docker Maintainers","Names":"shop-web-1","Ports":"0.0.0.0:8090->80/tcp, [::]:8090->80/tcp","State":"running","Status":"Up 3 hours (healthy)"}"#;
    const PS_EXITED: &str = r#"{"HealthStatus":"unhealthy","ID":"95223ad7c93b","Image":"redis:7-alpine","Labels":"","Names":"cache","Ports":"","State":"exited","Status":"Exited (0) 10 days ago"}"#;

    #[test]
    fn reads_docker_ps_lines() {
        let c = parse_ps_line(PS_RUNNING).unwrap();
        assert_eq!((c.name.as_str(), c.project.as_str(), c.service.as_str(), c.health.as_deref()), ("shop-web-1", "shop", "web", Some("healthy")));
        assert_eq!(parse_published(&c.ports)[0].0, 8090);
        // a stopped container's old health word is not shown
        let e = parse_ps_line(PS_EXITED).unwrap();
        assert!(e.health.is_none() && !e.running() && e.project.is_empty());
        assert!(parse_ps_line("not json").is_none());
        assert!(parse_ps_line("{}").is_none());
        // older Docker without the HealthStatus field: read it from the status text
        let o = parse_ps_line(r#"{"ID":"abc","Names":"x","State":"running","Status":"Up 2 minutes (unhealthy)"}"#).unwrap();
        assert!(o.unhealthy());
    }

    #[test]
    fn reads_docker_stats() {
        let mut v = vec![parse_ps_line(PS_RUNNING).unwrap(), parse_ps_line(PS_EXITED).unwrap()];
        apply_stats(&mut v, "{\"CPUPerc\":\"12.50%\",\"ID\":\"d4b8e345e18b\",\"MemPerc\":\"1.60%\",\"MemUsage\":\"126.4MiB / 7.653GiB\"}\nnot json\n");
        assert!((v[0].cpu - 12.5).abs() < 1e-4 && (v[0].mem_frac - 0.016).abs() < 1e-5);
        assert!((v[0].mem_used - 126.4 * 1048576.0).abs() < 1.0);
        assert_eq!(v[1].mem, "");
    }

    #[test]
    fn sorts_running_first_then_project_and_name() {
        let mk = |n: &str, st: &str, p: &str| Container { name: n.into(), state: st.into(), project: p.into(), ..Default::default() };
        let mut v = vec![mk("z", "exited", ""), mk("b", "running", "shop"), mk("a", "running", "shop"), mk("c", "running", "")];
        sort_containers(&mut v);
        assert_eq!(v.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), vec!["c", "a", "b", "z"]);
    }

    #[test]
    fn reads_bytes_and_formats_them() {
        assert_eq!(parse_bytes("126.4MiB"), 126.4 * 1048576.0);
        assert_eq!(parse_bytes("1GiB"), 1073741824.0);
        assert_eq!(parse_bytes("512B"), 512.0);
        assert_eq!(parse_bytes("3kB"), 3000.0);
        assert_eq!(parse_bytes("0B"), 0.0);
        assert_eq!(parse_bytes(""), 0.0);
        assert_eq!(fmt_bytes(512.0), "512 B");
        assert_eq!(fmt_bytes(-0.0), "0 B");
        assert_eq!(fmt_bytes(133.0 * 1048576.0), "133 MB");
        assert_eq!(fmt_bytes(1.5 * 1073741824.0), "1.5 GB");
        assert_eq!(fmt_bytes(44.0 * 1024.0), "44 kB");
    }

    #[test]
    fn explains_docker_failures() {
        assert_eq!(docker_problem("docker: No such file or directory (os error 2)").0, "Docker isn't installed");
        assert_eq!(docker_problem("Cannot connect to the Docker daemon at unix:///Users/x/.orbstack/run/docker.sock. Is the docker daemon running?").0, "Docker isn't running");
        assert_eq!(docker_problem("failed to connect: dial unix /var/run/docker.sock: connect: no such file or directory").0, "Docker isn't running");
        assert_eq!(docker_problem("docker did not answer in 10 s").0, "Docker is not answering");
        assert_eq!(docker_problem("something odd").1, "something odd");
    }

    #[test]
    fn cleans_log_lines_and_merges_streams() {
        assert_eq!(strip_ansi("\u{1b}[31mERROR\u{1b}[0m boom\tx"), "ERROR boom  x");
        let m = merge_logs("2026-10-17T07:10:00.5Z a\n2026-10-17T07:10:02.0Z c\n", "2026-10-17T07:10:01.0Z b\n\n", 10);
        assert_eq!(m, vec!["2026-10-17T07:10:00.5Z a", "2026-10-17T07:10:01.0Z b", "2026-10-17T07:10:02.0Z c"]);
        assert_eq!(merge_logs("1 a\n2 b\n3 c\n", "", 2), vec!["2 b", "3 c"]);
    }

    #[test]
    fn reads_log_timestamps() {
        assert_eq!(epoch_from_iso("1970-01-01T00:00:00Z"), Some((0, 0)));
        assert_eq!(epoch_from_iso("2026-10-17T07:10:00.123456789Z"), Some((1792221000, 123)));
        assert_eq!(epoch_from_iso("2024-02-29T23:59:59.5Z").map(|x| x.0), Some(1709251199));
        assert_eq!(epoch_from_iso("hello world"), None);
        assert_eq!(local_hms("nope"), "");
        assert_eq!(local_hms("2026-10-17T07:10:00.123Z").len(), 12);
    }

    #[test]
    fn reads_published_ports_and_lsof() {
        assert_eq!(parse_published("0.0.0.0:8090->80/tcp, [::]:8090->80/tcp, 0.0.0.0:5432->5432/tcp"), vec![(8090, "80/tcp".into()), (5432, "5432/tcp".into())]);
        assert_eq!(parse_published("0.0.0.0:8000-8002->9000-9002/tcp, 3306/tcp"), vec![(8000, "9000/tcp".into()), (8001, "9001/tcp".into()), (8002, "9002/tcp".into())]);
        assert!(parse_published("").is_empty());
        let l = parse_lsof("p612\ncsphered\nn127.0.0.1:7878\np2210\ncadb\nn*:5037\nn[::1]:5037\n");
        assert_eq!(l.iter().map(|x| (x.port, x.pid, x.command.as_str())).collect::<Vec<_>>(), vec![(5037, 2210, "adb"), (7878, 612, "sphered")]);
        assert_eq!(l[1].addr, "127.0.0.1");
        let l = parse_lsof("p9\ncCode Helper (Plugin)\nf36\nn[::1]:49177\n");
        assert_eq!((l[0].command.as_str(), l[0].addr.as_str()), ("Code Helper (Plugin)", "[::1]"));
        assert!(parse_lsof("").is_empty());
    }

    #[test]
    fn maps_listeners_to_running_containers() {
        let web = parse_ps_line(PS_RUNNING).unwrap();
        let mut stopped = parse_ps_line(PS_EXITED).unwrap();
        stopped.ports = "0.0.0.0:6379->6379/tcp".into();
        let mut l = parse_lsof("p1\ncOrbStack\nn*:8090\nn*:6379\n");
        map_containers(&mut l, &[web, stopped]);
        let by = |p: u16| l.iter().find(|x| x.port == p).unwrap();
        assert_eq!((by(8090).container.as_deref(), by(8090).service.as_str(), by(8090).inner.as_str()), (Some("shop-web-1"), "web", "80/tcp"));
        assert!(by(6379).container.is_none());
    }

    #[test]
    fn counts_connections_into_a_port() {
        let raw = "p10\nn127.0.0.1:5432->127.0.0.1:50122\nn127.0.0.1:5432->127.0.0.1:50123\np11\nn127.0.0.1:50122->127.0.0.1:5432\nn[::1]:5432->[::1]:60000\n";
        assert_eq!(parse_connections(raw, 5432), 3);
        assert_eq!(parse_connections("", 5432), 0);
        assert!(terminate(1).is_err());
        // never the app itself, and never a pid that isn't what was on screen
        assert!(terminate(std::process::id()).is_err());
        assert!(terminate_named(std::process::id(), "definitely-not-this-name").unwrap_err().contains("is now"));
        assert!(terminate_named(4_000_000_000, "ghost").unwrap_err().contains("already gone"));
    }

    #[test]
    fn reads_disk_bytes_and_core_use() {
        let raw = "\"Statistics\" = {\"Bytes (Read)\"=1000,\"Operations (Write)\"=4,\"Bytes (Write)\"=500}\n\"Statistics\" = {\"Bytes (Read)\"=24,\"Bytes (Write)\"=6}";
        assert_eq!(parse_ioreg_disk(raw), (1024, 506));
        assert_eq!(parse_ioreg_disk(""), (0, 0));
        assert_eq!(core_use(&[(10, 100), (0, 100)], &[(60, 200), (0, 200)]), vec![0.5, 0.0]);
        assert!((total_use(&[(10, 100), (0, 100)], &[(60, 200), (0, 200)]) - 0.25).abs() < 1e-6);
        // the 32-bit counter wrapped between readings: 20 busy of 100
        assert!((core_use(&[(0xFFFF_FFF0, 0xFFFF_FFF0)], &[(4, 84)])[0] - 0.2).abs() < 1e-6);
        assert!(!core_ticks().is_empty() || !cfg!(target_os = "macos"));
    }

    #[test]
    fn reads_vm_stat_ps_and_netstat() {
        let vm = "Mach Virtual Memory Statistics: (page size of 16384 bytes)\nPages free:  100000.\nPages wired down:  200000.\nAnonymous pages:  400000.\nPages purgeable:  0.\nFile-backed pages:  50000.\nPages occupied by compressor:  10000.\n";
        let m = parse_vm_stat(vm);
        assert!((m[0] - 6.1035).abs() < 0.01 && (m[4] - 1.5259).abs() < 0.01);
        // purgeable memory counts as cached, not as app memory
        let m = parse_vm_stat("page size of 4096 bytes\nAnonymous pages: 1000000.\nPages purgeable: 250000.\n");
        assert!((m[0] - 2.8610).abs() < 0.01 && (m[3] - 0.9537).abs() < 0.01);
        assert_eq!(parse_vm_stat(""), [0.0; 5]);
        let p = parse_ps("  PID  %CPU    RSS COMM\n51233 151.0 765952 /usr/bin/cargo\n  812   0.0   2048 /Applications/Google Chrome.app/Contents/MacOS/Google Chrome Helper (Renderer)\ngarbage\n");
        assert_eq!((p[0].pid, p[0].name.as_str()), (51233, "cargo"));
        assert!((p[0].rss_mb - 748.0).abs() < 0.1);
        assert_eq!(p[1].name, "Google Chrome Helper (Renderer)");
        assert_eq!(p.len(), 2);
    }

    #[test]
    fn reads_netstat_with_and_without_addresses() {
        let n = parse_netstat("Name  Mtu   Network       Address            Ipkts Ierrs     Ibytes    Opkts Oerrs     Obytes  Coll\nen0   1500  <Link#11>   aa:bb:cc:dd:ee:ff  100     0     5000    50     0     2000     0\nen0   1500  192.168.1     192.168.1.2  100     -     5000    50     -     2000     -\nen5   1500  <Link#14>   10     0     700    20     0     300     0\nlo0   16384 <Link#1>   50     0     9999    50     0     9999     0\nbridge100 1500 <Link#9> aa:bb:cc:dd:ee:00 1 0 1 1 0 1 0\n");
        assert_eq!(n, (5700, 2300));
        assert_eq!(parse_netstat(""), (0, 0));
    }

    #[test]
    fn picks_top_processes_by_cpu_and_memory() {
        let mk = |pid, cpu, rss_mb| Proc { pid, cpu, rss_mb, name: format!("p{pid}") };
        let v = top_procs(vec![mk(1, 50.0, 1.0), mk(2, 40.0, 1.0), mk(3, 0.0, 900.0), mk(4, 0.0, 800.0), mk(5, 0.0, 1.0)], 2);
        assert_eq!(v.iter().map(|p| p.pid).collect::<Vec<_>>(), vec![1, 2, 3, 4]);
    }

    #[test]
    fn reads_df_pmset_and_gpu() {
        let (used, total) = parse_df("Filesystem   1024-blocks      Used Available Capacity iused ifree %iused  Mounted on\n/dev/disk3s1s1   487311612  15106904 183431816     8% 426148 1834318160    0%   /\n");
        assert!((total - 499.0).abs() < 0.5 && (used - 311.3).abs() < 0.5, "{used} {total}");
        assert_eq!(parse_df(""), (0.0, 0.0));
        let (src, b) = parse_pmset("Now drawing from 'Battery Power'\n -InternalBattery-0 (id=23593059)\t89%; discharging; 5:38 remaining present: true\n");
        assert_eq!((src.as_str(), b), ("Battery Power", Some((89, "discharging".into(), "5:38 remaining".into()))));
        let (_, b) = parse_pmset("Now drawing from 'AC Power'\n -InternalBattery-0 (id=1)\t95%; AC attached; not charging present: true\n");
        assert_eq!(b, Some((95, "AC attached".into(), "not charging".into())));
        let (_, b) = parse_pmset("Now drawing from 'Battery Power'\n -InternalBattery-0 (id=1)\t7%; discharging; (no estimate) present: true\n");
        assert_eq!(b.map(|x| (x.0, x.2)), Some((7, "calculating".into())));
        // a desktop has no battery
        let (src, b) = parse_pmset("Now drawing from 'AC Power'\n");
        assert_eq!((src.as_str(), b), ("AC Power", None));
        assert_eq!(parse_gpu("\"PerformanceStatistics\" = {\"Device Utilization %\"=37,\"Renderer Utilization %\"=30}"), Some(0.37));
        assert_eq!(parse_gpu("nothing"), None);
    }

    /// Run by hand: `cargo test --release -p sphere-app sample_cost -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn sample_cost() {
        let (mut old, mut net) = sample(None, None, true);
        let t = Instant::now();
        for i in 0..5 {
            let (s, n) = sample(net, Some(&old), i == 0);
            (old, net) = (s, n);
        }
        println!("sample: {:?} each, {} procs", t.elapsed() / 5, old.procs.len());
    }
}
