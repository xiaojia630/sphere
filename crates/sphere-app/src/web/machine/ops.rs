//! docker, lsof and the system tools on the pretend machine: a MacBook Pro (M2 Pro, 12 cores, 32 GB) running a
//! compose project named "shop" (api, db, cache, mail, worker), a Vite dev server and a debug build of the api.
//! Output is what the tools print on macOS, so `opsdata`'s own parsers read it. Counters that the System app turns
//! into rates (CPU ticks, network and disk bytes) grow with every look; the clock is `web_time`, never `std::time`.
#![allow(dead_code)]
use std::sync::{Mutex, MutexGuard, OnceLock};
use web_time::{SystemTime, UNIX_EPOCH};

/// What `cmd args` prints; `Err` is the first line of the tool's complaint.
pub fn run(cmd: &str, args: &[&str]) -> Result<String, String> {
    match cmd {
        "docker" => docker(args),
        "lsof" => lsof(args),
        "ps" => ps(args),
        "sysctl" => sysctl(args),
        "vm_stat" => Ok(vm_stat()),
        "netstat" => Ok(netstat()),
        "df" => Ok(DF.to_string()),
        "pmset" => Ok(PMSET.to_string()),
        "ioreg" => ioreg(args),
        _ => Err(format!("{cmd}: command not found")),
    }
}

// ------------------------------------------------------------------ time and noise

/// Seconds since the epoch.
pub(super) fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// When this pretend machine's session began: its containers started a few hours before.
fn t0() -> i64 {
    static T: OnceLock<i64> = OnceLock::new();
    *T.get_or_init(now)
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn mix(a: u64, b: u64) -> u64 {
    let mut x = a.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ b.wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    x ^= x >> 29;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^ (x >> 32)
}

/// A steady pseudo-random number in 0..1 for `(a, b)`.
pub(super) fn unit(a: u64, b: u64) -> f32 {
    (mix(a, b) >> 40) as f32 / (1u64 << 24) as f32
}

/// `2026-10-20T10:00:01.234567890Z`, the way `docker logs --timestamps` stamps a line.
pub(super) fn iso(secs: i64, nanos: u32) -> String {
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{nanos:09}Z", rem / 3600, rem / 60 % 60, rem % 60)
}

/// "2026-10-20 10:00:01" for the services that print their own clock.
fn plain(secs: i64) -> String {
    iso(secs, 0)[..19].replace('T', " ")
}

/// How long, the way docker words it ("About an hour", "3 hours").
fn human(s: i64) -> String {
    match s {
        ..=1 => "Less than a second".into(),
        2..=59 => format!("{s} seconds"),
        60..=119 => "About a minute".into(),
        120..=3599 => format!("{} minutes", s / 60),
        3600..=7199 => "About an hour".into(),
        7200..=172_799 => format!("{} hours", s / 3600),
        _ => format!("{} days", s / 86400),
    }
}

// ------------------------------------------------------------------ docker

struct Ct {
    id: &'static str,
    name: &'static str,
    image: &'static str,
    service: &'static str,
    ports: &'static str,
    /// the image has a health check
    health: bool,
    cpu: f32,
    mem_mib: f32,
    /// started this long before the session began, and (if it has exited) ran for this long
    age: i64,
    ran: i64,
}

const CTS: [Ct; 5] = [
    Ct { id: "a3f1c9d2e847", name: "shop-api", image: "shop-api:dev", service: "api", ports: "0.0.0.0:8080->8080/tcp, [::]:8080->8080/tcp", health: false, cpu: 1.4, mem_mib: 38.2, age: 10_920, ran: 0 },
    Ct { id: "5b7e20c4d913", name: "shop-db", image: "postgres:16", service: "db", ports: "0.0.0.0:5432->5432/tcp, [::]:5432->5432/tcp", health: true, cpu: 0.9, mem_mib: 61.7, age: 11_100, ran: 0 },
    Ct { id: "c81d4a6f0e52", name: "shop-cache", image: "redis:7-alpine", service: "cache", ports: "0.0.0.0:6379->6379/tcp, [::]:6379->6379/tcp", health: true, cpu: 0.4, mem_mib: 7.4, age: 11_100, ran: 0 },
    Ct { id: "e290b5173a6c", name: "shop-mail", image: "axllent/mailpit", service: "mail", ports: "0.0.0.0:1025->1025/tcp, [::]:1025->1025/tcp, 0.0.0.0:8025->8025/tcp, [::]:8025->8025/tcp", health: true, cpu: 0.2, mem_mib: 22.3, age: 11_090, ran: 0 },
    Ct { id: "7f46d8a21b09", name: "shop-worker", image: "shop-worker:dev", service: "worker", ports: "", health: false, cpu: 0.0, mem_mib: 0.0, age: 10_860, ran: 8_100 },
];

/// Where one container is now.
#[derive(Clone)]
struct St {
    up: bool,
    gone: bool,
    started: i64,
    /// when it last stopped (only while it is down)
    ended: i64,
}

static DOCKER: Mutex<Vec<St>> = Mutex::new(Vec::new());

fn with_docker<R>(f: impl FnOnce(&mut Vec<St>) -> R) -> R {
    let mut v = lock(&DOCKER);
    if v.is_empty() {
        *v = CTS.iter().map(|c| St { up: c.ran == 0, gone: false, started: t0() - c.age, ended: t0() - c.age + c.ran }).collect();
    }
    f(&mut v)
}

fn find(name: &str) -> Option<usize> {
    CTS.iter().position(|c| c.name == name || (name.len() >= 3 && c.id.starts_with(name)) || (name.len() > 12 && name.starts_with(c.id)))
}

fn docker(a: &[&str]) -> Result<String, String> {
    let rest = || a.iter().skip(1).filter(|s| !s.starts_with('-')).copied().collect::<Vec<_>>();
    match a.first().copied().unwrap_or("") {
        "ps" => Ok(docker_ps(a)),
        "stats" => Ok(docker_stats()),
        "logs" => {
            let id = a.last().copied().unwrap_or("");
            let n = a.iter().position(|s| *s == "--tail").and_then(|i| a.get(i + 1)).and_then(|s| s.parse().ok()).unwrap_or(200usize);
            let i = find(id).filter(|i| !with_docker(|v| v[*i].gone)).ok_or_else(|| format!("Error response from daemon: No such container: {id}"))?;
            Ok(docker_logs(i, n))
        }
        op @ ("start" | "stop" | "restart" | "rm" | "kill" | "pause" | "unpause") => {
            let names = rest();
            for n in &names {
                let i = find(n).filter(|i| !with_docker(|v| v[*i].gone)).ok_or_else(|| format!("Error response from daemon: No such container: {n}"))?;
                with_docker(|v| {
                    let s = &mut v[i];
                    match op {
                        "start" | "restart" | "unpause" => (s.up, s.started) = (true, now()),
                        "stop" | "kill" => (s.up, s.ended) = (false, now()),
                        _ => s.gone = true,
                    }
                });
            }
            Ok(names.join("\n") + "\n")
        }
        "pull" => Ok(format!("sha256:{:064x}\n", mix(rest().first().map(|s| s.len() as u64).unwrap_or(0), 7) as u128 * 0x1_0000_0001)),
        "events" | "compose" | "network" | "volume" | "image" => Ok(String::new()),
        "version" | "info" => Ok("Docker Desktop 4.34.2\n".into()),
        other => Err(format!("docker: '{other}' is not a docker command.")),
    }
}

fn docker_ps(a: &[&str]) -> String {
    let filter = a.iter().position(|s| *s == "--filter").and_then(|i| a.get(i + 1)).and_then(|f| f.strip_prefix("name=")).unwrap_or("");
    let t = now();
    let mut out = String::new();
    with_docker(|v| {
        for (c, s) in CTS.iter().zip(v.iter()) {
            if s.gone || !c.name.contains(filter) {
                continue;
            }
            let (state, status) = if s.up {
                let health = if c.health { " (healthy)" } else { "" };
                ("running", format!("Up {}{health}", human(t - s.started).replace("Less than a second", "1 second")))
            } else {
                ("exited", format!("Exited (0) {} ago", human(t - s.ended)))
            };
            let ports = if s.up { c.ports } else { "" };
            let labels = format!("com.docker.compose.config-hash=9c2e{}f,com.docker.compose.project=shop,com.docker.compose.service={},com.docker.compose.version=2.29.2", &c.id[..4], c.service);
            out += &format!(
                "{{\"Command\":\"\\\"docker-entrypoint.sh\\\"\",\"CreatedAt\":\"{}\",\"ID\":\"{}\",\"Image\":\"{}\",\"Labels\":\"{labels}\",\"Names\":\"{}\",\"Ports\":\"{ports}\",\"RunningFor\":\"{} ago\",\"State\":\"{state}\",\"Status\":\"{status}\"}}\n",
                plain(s.started),
                c.id,
                c.image,
                c.name,
                human(t - s.started).to_lowercase()
            );
        }
    });
    out
}

fn docker_stats() -> String {
    let t = now();
    let mut out = String::new();
    with_docker(|v| {
        for (c, s) in CTS.iter().zip(v.iter()) {
            if !s.up || s.gone {
                continue;
            }
            let k = t as u64 / 2;
            let cpu = c.cpu * (0.4 + 1.4 * unit(k, c.age as u64));
            let mem = c.mem_mib + 0.9 * unit(k / 5, c.age as u64 + 1);
            out += &format!("{{\"BlockIO\":\"0B / 0B\",\"CPUPerc\":\"{cpu:.2}%\",\"ID\":\"{}\",\"MemPerc\":\"{:.2}%\",\"MemUsage\":\"{mem:.1}MiB / 7.653GiB\",\"Name\":\"{}\",\"NetIO\":\"1.2MB / 884kB\",\"PIDs\":\"{}\"}}\n", c.id, mem / (7.653 * 1024.0) * 100.0, c.name, 4 + c.age % 9);
        }
    });
    out
}

/// One log line: when, and what it says.
type Line = (i64, u32, String);

fn docker_logs(i: usize, n: usize) -> String {
    let c = &CTS[i];
    let (s, t) = (with_docker(|v| v[i].clone()), now());
    // a container that was started again begins its log again; one that is down keeps what it said
    let end = if s.up { t } else { s.ended };
    let mut v: Vec<Line> = log_lines(c.service, s.started, end, !s.up);
    v.retain(|l| l.0 <= end);
    v.sort_by_key(|l| (l.0, l.1));
    let skip = v.len().saturating_sub(n);
    v.into_iter().skip(skip).map(|(sec, ms, m)| format!("{} {m}\n", iso(sec, ms * 1_000_000 + (mix(sec as u64, ms as u64) % 1_000_000) as u32))).collect()
}

fn log_lines(service: &str, start: i64, end: i64, exited: bool) -> Vec<Line> {
    let mut v: Vec<Line> = Vec::new();
    let mut at = |dt: f32, m: String| v.push((start + dt as i64, (dt.fract() * 1000.0) as u32, m));
    match service {
        "api" => {
            at(0.31, "INFO shop_api: starting shop-api 0.4.2 (profile=dev, rustc 1.82.0)".into());
            at(0.32, "INFO shop_api::config: loaded configuration from the environment".into());
            at(0.44, "INFO shop_api::db: connecting to postgres://shop@db:5432/shop (pool: max 10, min 2)".into());
            at(0.71, "INFO sqlx::postgres::notice: relation \"_sqlx_migrations\" already exists, skipping".into());
            at(0.78, "INFO shop_api::db: 7 migrations applied, 0 pending".into());
            at(0.82, "INFO shop_api::cache: connected to redis://cache:6379/0".into());
            at(0.90, "INFO shop_api::jobs: queue \"emails\" ready, 0 jobs waiting".into());
            at(0.93, "INFO shop_api: listening on 0.0.0.0:8080".into());
            // a request every few seconds, the same ones for the same clock second, so a second look finds the same lines
            for b in (start + 4) / 4..=end / 4 {
                let (ts, r) = (b * 4 + (unit(b as u64, 1) * 3.0) as i64, unit(b as u64, 2));
                let ms = (unit(b as u64, 3) * 999.0) as u32;
                let took = 0.6 + unit(b as u64, 4) * 14.0;
                let (line, extra) = if r < 0.30 {
                    (format!("INFO http: GET /health 200 {:.1}ms", took / 6.0), None)
                } else if r < 0.48 {
                    (format!("INFO http: GET /products?limit=20 200 {took:.1}ms"), None)
                } else if r < 0.58 {
                    (format!("INFO http: GET /products/{} 200 {took:.1}ms", 100 + (unit(b as u64, 5) * 40.0) as u32), None)
                } else if r < 0.70 {
                    (format!("INFO http: POST /auth/login 200 {:.1}ms", 28.0 + took * 2.0), Some("INFO shop_api::auth: issued session token (ttl 900s)".to_string()))
                } else if r < 0.80 {
                    (format!("INFO http: GET /orders?page=1 200 {:.1}ms", took + 3.0), None)
                } else if r < 0.87 {
                    (format!("INFO http: POST /orders 201 {:.1}ms", 18.0 + took), Some(format!("INFO shop_api::orders: order #{} created, 1 email queued", 4400 + b % 400)))
                } else if r < 0.92 {
                    (format!("WARN http: GET /orders/{} 404 {took:.1}ms", 9000 + b % 90), None)
                } else if r < 0.96 {
                    ("WARN shop_api::auth: rejected an expired token for user 14".to_string(), Some("WARN http: GET /orders 401 0.4ms".to_string()))
                } else if r < 0.985 {
                    ("WARN sqlx::query: slow statement: SELECT o.* FROM orders o JOIN order_items i ON i.order_id = o.id (elapsed 312ms)".to_string(), None)
                } else {
                    ("ERROR http: GET /reports/daily 500 1203.4ms: connection reset by peer".to_string(), None)
                };
                if ts > start + 1 && ts <= end {
                    v.push((ts, ms, line));
                    if let Some(x) = extra {
                        v.push((ts, ms + 1, x));
                    }
                }
            }
        }
        "db" => {
            at(0.0, "PostgreSQL Database directory appears to contain a database; Skipping initialization".into());
            let lg = |dt: f32, m: &str| format!("{} UTC [{}] LOG:  {m}", plain(start + dt as i64), if dt < 1.0 { 1 } else { 29 });
            at(0.10, lg(0.1, "starting PostgreSQL 16.4 (Debian 16.4-1.pgdg120+2) on aarch64-unknown-linux-gnu, compiled by gcc (Debian 12.2.0-14) 12.2.0, 64-bit"));
            at(0.11, lg(0.1, "listening on IPv4 address \"0.0.0.0\", port 5432"));
            at(0.11, lg(0.1, "listening on IPv6 address \"::\", port 5432"));
            at(0.12, lg(0.1, "listening on Unix socket \"/var/run/postgresql/.s.PGSQL.5432\""));
            at(0.19, lg(1.0, "database system was shut down at 2026-10-19 21:12:44 UTC"));
            at(0.22, lg(0.2, "database system is ready to accept connections"));
            for b in (start + 30) / 300..=end / 300 {
                let ts = b * 300 + 4;
                if ts > start + 20 && ts <= end {
                    let (w, tot) = (30 + (unit(b as u64, 8) * 90.0) as u32, 3.0 + unit(b as u64, 9) * 6.0);
                    v.push((ts, 12, format!("{} UTC [27] LOG:  checkpoint starting: time", plain(ts))));
                    if ts + 9 <= end {
                        v.push((ts + 9, 390, format!("{} UTC [27] LOG:  checkpoint complete: wrote {w} buffers ({:.1}%); 0 WAL file(s) added, 0 removed, 0 recycled; write={tot:.3} s, sync=0.004 s, total={:.3} s; sync files=14, longest=0.002 s, average=0.001 s; distance=412 kB, estimate=412 kB", plain(ts + 9), w as f32 / 163.84, tot + 0.02)));
                    }
                }
            }
        }
        "cache" => {
            let rl = |role: &str, m: &str| format!("1:{role} {} {m}", fmt_redis(start));
            at(0.10, rl("C", "* oO0OoO0OoO0Oo Redis is starting oO0OoO0OoO0Oo"));
            at(0.10, rl("C", "* Redis version=7.4.1, bits=64, commit=00000000, modified=0, pid=1, just started"));
            at(0.10, rl("C", "# Warning: no config file specified, using the default config. In order to specify a config file use redis-server /path/to/redis.conf"));
            at(0.11, rl("M", "* monotonic clock: POSIX clock_gettime"));
            at(0.12, rl("M", "* Running mode=standalone, port=6379."));
            at(0.12, rl("M", "* Server initialized"));
            at(0.13, rl("M", "* Ready to accept connections tcp"));
            for b in (start + 30) / 300..=end / 300 {
                let ts = b * 300 + 61;
                if ts > start + 20 && ts <= end {
                    for (dt, m) in [(0, "* 100 changes in 300 seconds. Saving..."), (0, "* Background saving started by pid 38"), (1, "* DB saved on disk"), (1, "* Background saving terminated with success")] {
                        if ts + dt <= end {
                            v.push((ts + dt, 20, format!("{} {} {m}", if dt == 0 { "1:M" } else { "38:C" }, fmt_redis(ts + dt))));
                        }
                    }
                }
            }
        }
        "mail" => {
            let ml = |dt: i64, m: &str| format!("time=\"{}\" level=info msg=\"{m}\"", plain(start + dt).replace('-', "/"));
            at(0.64, ml(0, "[smtpd] starting on [::]:1025 (no encryption)"));
            at(0.64, ml(0, "[http] starting on [::]:8025"));
            at(0.65, ml(0, "[http] accessible via http://localhost:8025/"));
            for b in (start + 60) / 420..=end / 420 {
                let ts = b * 420 + (unit(b as u64, 11) * 200.0) as i64;
                if ts > start + 30 && ts <= end {
                    let who = ["amara.o@example.com", "tunde@example.com", "kofi.b@example.com", "li.wei@example.com"][(mix(b as u64, 12) % 4) as usize];
                    v.push((ts, 140, format!("time=\"{}\" level=info msg=\"[smtpd] accepted connection from 172.18.0.2:{}\"", plain(ts).replace('-', "/"), 40000 + b % 20000)));
                    v.push((ts, 310, format!("time=\"{}\" level=info msg=\"[smtpd] received message \\\"Your Shop order\\\" to {who}\"", plain(ts).replace('-', "/"))));
                }
            }
        }
        _ => {
            at(0.4, "INFO shop_worker: starting shop-worker 0.4.2".into());
            at(0.6, "INFO shop_worker: connected to redis://cache:6379/0, consuming queue \"emails\"".into());
            for (dt, m) in [(95.0, "INFO shop_worker: job 4401 send_receipt done in 212ms"), (96.0, "INFO shop_worker: job 4402 send_receipt done in 187ms"), (310.0, "INFO shop_worker: job 4403 resize_image done in 640ms"), (771.0, "WARN shop_worker: job 4404 send_receipt failed (smtp: connection refused), retry 1/3 in 30s"), (802.0, "INFO shop_worker: job 4404 send_receipt done in 233ms (retry 1)"), (1410.0, "INFO shop_worker: job 4405 send_receipt done in 201ms"), (2210.0, "INFO shop_worker: job 4406 export_orders done in 3.1s")] {
                if start + (dt as i64) < end {
                    at(dt, m.into());
                }
            }
            if exited {
                v.push((end - 1, 870, "INFO shop_worker: SIGTERM received, finishing the current job".into()));
                v.push((end, 40, "INFO shop_worker: shutdown complete (exit 0)".into()));
            }
        }
    }
    v
}

/// Redis's own clock: `20 Oct 2026 10:00:01.123`.
fn fmt_redis(secs: i64) -> String {
    let d = plain(secs);
    let months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let m = d[5..7].parse::<usize>().unwrap_or(1);
    format!("{} {} {} {}.{:03}", &d[8..10], months[m.clamp(1, 12) - 1], &d[..4], &d[11..], mix(secs as u64, 3) % 1000)
}

// ------------------------------------------------------------------ lsof and ps

const DOCKER_PID: u32 = 3318;
const VITE_PID: u32 = 51233;
const API_DEBUG_PID: u32 = 52408;

/// Everything that listens on the machine: (pid, command, [addr:port]).
fn listeners() -> Vec<(u32, &'static str, Vec<String>)> {
    let published: Vec<String> = with_docker(|v| {
        let mut p = Vec::new();
        for (c, s) in CTS.iter().zip(v.iter()) {
            if s.up && !s.gone {
                p.extend(crate::opsdata::parse_published(c.ports).into_iter().map(|x| format!("*:{}", x.0)));
            }
        }
        p
    });
    let mut out = vec![(605, "ControlCenter", vec!["*:7000".to_string(), "*:5000".to_string()]), (1412, "Spotify", vec!["127.0.0.1:57621".into(), "*:57621".into()]), (VITE_PID, "node", vec!["[::1]:5173".into()]), (API_DEBUG_PID, "shop-api", vec!["*:3000".into()]), (7790, "Code Helper (Plugin)", vec!["127.0.0.1:49177".into()])];
    if !published.is_empty() {
        out.push((DOCKER_PID, "com.docker.backend", published));
    }
    out
}

fn lsof(a: &[&str]) -> Result<String, String> {
    let has = |s: &str| a.contains(&s);
    let port = a.iter().find_map(|x| x.strip_prefix("-iTCP:")).and_then(|p| p.parse::<u16>().ok());
    let ls = listeners();
    let owner = |p: u16| ls.iter().find(|l| l.2.iter().any(|a| a.rsplit(':').next().and_then(|n| n.parse::<u16>().ok()) == Some(p)));
    if has("-sTCP:LISTEN") && has("-F") {
        let mut out = String::new();
        for (pid, cmd, addrs) in &ls {
            out += &format!("p{pid}\nc{cmd}\n");
            for a in addrs {
                out += &format!("n{a}\n");
            }
        }
        return Ok(out);
    }
    if has("-sTCP:LISTEN") && has("-t") {
        return port.and_then(owner).map(|l| format!("{}\n", l.0)).ok_or_else(|| "failed".into());
    }
    if has("-sTCP:ESTABLISHED") {
        let p = port.ok_or("failed")?;
        let t = now() as u64;
        let n = match p {
            5432 => 3 + (unit(t / 7, 1) * 3.0) as u16,
            8080 => 1 + (unit(t / 9, 2) * 3.0) as u16,
            6379 => 2,
            3000 => 1,
            5173 => 2 + (unit(t / 11, 3) * 2.0) as u16,
            _ => 0,
        };
        if n == 0 || owner(p).is_none() {
            return Err("failed".into());
        }
        let mut out = String::new();
        for i in 0..n {
            out += &format!("p{}\nn127.0.0.1:{p}->127.0.0.1:{}\n", 5000 + i, 52100 + i * 2 + (p % 50));
        }
        return Ok(out);
    }
    Err("failed".into())
}

/// (pid, path, cpu %, rss kB): what `ps -Ao pid,pcpu,rss,comm` lists, about as busy as a working Mac.
const PROCS: [(u32, &str, f32, u32); 30] = [
    (0, "kernel_task", 5.8, 31_104),
    (1, "/sbin/launchd", 0.1, 14_912),
    (88, "/usr/libexec/logd", 0.3, 11_840),
    (97, "/usr/sbin/mds", 0.6, 87_232),
    (131, "/System/Library/PrivateFrameworks/SkyLight.framework/Resources/WindowServer", 9.4, 318_976),
    (412, "/usr/libexec/trustd", 0.1, 20_016),
    (605, "/System/Library/CoreServices/ControlCenter.app/Contents/MacOS/ControlCenter", 0.4, 81_952),
    (611, "/System/Library/CoreServices/Dock.app/Contents/MacOS/Dock", 0.7, 98_304),
    (629, "/System/Library/CoreServices/Finder.app/Contents/MacOS/Finder", 0.3, 140_288),
    (702, "/System/Library/Frameworks/CoreServices.framework/Frameworks/Metadata.framework/Versions/A/Support/mds_stores", 0.8, 244_736),
    (955, "/Applications/Sphere.app/Contents/MacOS/Sphere", 7.6, 655_360),
    (961, "/Users/you/.sphere/bin/sphered", 0.3, 46_080),
    (1412, "/Applications/Spotify.app/Contents/MacOS/Spotify", 1.5, 301_056),
    (2204, "/Applications/Safari.app/Contents/MacOS/Safari", 3.2, 492_544),
    (2231, "/System/Library/Frameworks/WebKit.framework/Versions/A/XPCServices/com.apple.WebKit.WebContent.xpc/Contents/MacOS/com.apple.WebKit.WebContent", 2.4, 388_096),
    (2240, "/System/Library/Frameworks/WebKit.framework/Versions/A/XPCServices/com.apple.WebKit.Networking.xpc/Contents/MacOS/com.apple.WebKit.Networking", 0.5, 63_488),
    (3318, "/Applications/Docker.app/Contents/MacOS/com.docker.backend", 1.8, 214_016),
    (3322, "/System/Library/Frameworks/Virtualization.framework/Versions/A/XPCServices/com.apple.Virtualization.VirtualMachine.xpc/Contents/MacOS/com.apple.Virtualization.VirtualMachine", 4.1, 2_516_992),
    (4120, "/Applications/Slack.app/Contents/MacOS/Slack", 1.9, 372_736),
    (4131, "/Applications/Slack.app/Contents/Frameworks/Slack Helper (Renderer).app/Contents/MacOS/Slack Helper (Renderer)", 1.1, 421_888),
    (7702, "/Applications/Visual Studio Code.app/Contents/MacOS/Electron", 1.2, 267_264),
    (7731, "/Applications/Visual Studio Code.app/Contents/Frameworks/Code Helper (Renderer).app/Contents/MacOS/Code Helper (Renderer)", 5.3, 737_280),
    (7790, "/Applications/Visual Studio Code.app/Contents/Frameworks/Code Helper (Plugin).app/Contents/MacOS/Code Helper (Plugin)", 1.4, 419_840),
    (7804, "/Users/you/.rustup/toolchains/stable-aarch64-apple-darwin/bin/rust-analyzer", 4.6, 1_153_024),
    (8891, "/Applications/Ghostty.app/Contents/MacOS/ghostty", 0.9, 156_672),
    (8902, "/bin/zsh", 0.0, 6_144),
    (51233, "/opt/homebrew/Cellar/node/22.9.0/bin/node", 2.7, 296_960),
    (52408, "/Users/you/Projects/shop-api/target/debug/shop-api", 0.6, 49_152),
    (52611, "/Users/you/.cargo/bin/cargo", 0.0, 21_504),
    (1530, "/usr/bin/ssh-agent", 0.0, 3_072),
];

static SIM: Mutex<Sim> = Mutex::new(Sim::new());

/// The counters the System app turns into rates.
struct Sim {
    n: u64,
    /// per core: (busy, total) ticks
    ticks: Vec<(u64, u64)>,
    net: (u64, u64),
    disk: (u64, u64),
}

impl Sim {
    const fn new() -> Sim {
        Sim { n: 0, ticks: Vec::new(), net: (48_113_204_992, 6_207_881_216), disk: (4_218_443_776_000, 3_127_914_520_576) }
    }
}

fn ps(a: &[&str]) -> Result<String, String> {
    // ps -p PID -o comm=
    if let Some(p) = a.iter().position(|x| *x == "-p").and_then(|i| a.get(i + 1)).and_then(|p| p.parse::<u32>().ok()) {
        return PROCS.iter().find(|x| x.0 == p).map(|x| format!("{}\n", x.1)).ok_or_else(|| "failed".into());
    }
    let n = {
        let mut s = lock(&SIM);
        s.n += 1;
        s.n
    };
    let up = |pid: u32| match pid {
        3318 => with_docker(|v| v.iter().any(|s| s.up && !s.gone)),
        _ => true,
    };
    let mut out = String::from("  PID  %CPU    RSS COMM\n");
    for (pid, path, cpu, rss) in PROCS.iter().filter(|p| up(p.0)) {
        let wob = 0.35 + 1.3 * unit(n, *pid as u64);
        // now and then one of them has a burst of work
        let burst = if unit(n / 4, *pid as u64 + 77) > 0.96 { 5.0 } else { 1.0 };
        let cpu = if *cpu == 0.0 { 0.0 } else { (cpu * wob * burst).min(380.0) };
        let rss = (*rss as f32 * (1.0 + 0.02 * (unit(n / 8, *pid as u64 + 5) - 0.5))) as u32;
        out += &format!("{pid:5} {cpu:5.1} {rss:6} {path}\n");
    }
    Ok(out)
}

// ------------------------------------------------------------------ the system

/// Every core's busy and total ticks so far. The playground's `opsdata::core_ticks` reads this one.
#[allow(dead_code)]
pub fn core_ticks() -> Vec<(u64, u64)> {
    let mut s = lock(&SIM);
    let n = s.n;
    if s.ticks.is_empty() {
        s.ticks = (0..12).map(|i| (7_000_000 + i * 31_000, 9_000_000 + i * 31_000)).collect();
    }
    for (i, t) in s.ticks.iter_mut().enumerate() {
        // 120 ticks a look; the performance cores (the first eight) swing wider than the efficiency ones
        let load = if i < 8 { 0.12 + 0.5 * unit(n, i as u64 + 200) * unit(n / 3, i as u64 + 300) * 1.6 } else { 0.06 + 0.2 * unit(n, i as u64 + 200) };
        t.0 += (120.0 * load.min(0.95)) as u64;
        t.1 += 120;
    }
    s.n += 1;
    s.ticks.clone()
}

fn sysctl(a: &[&str]) -> Result<String, String> {
    let key = a.last().copied().unwrap_or("");
    let n = lock(&SIM).n;
    let l = |k: u64| 1.7 + 1.4 * unit(n / 6, k);
    Ok(match key {
        "machdep.cpu.brand_string" => "Apple M2 Pro\n".into(),
        "hw.perflevel0.physicalcpu" => "8\n".into(),
        "hw.perflevel1.physicalcpu" => "4\n".into(),
        "hw.ncpu" | "hw.logicalcpu" => "12\n".into(),
        "hw.memsize" => "34359738368\n".into(),
        "vm.loadavg" => format!("{{ {:.2} {:.2} {:.2} }}\n", l(1), l(2) * 0.95, l(3) * 0.9),
        "vm.swapusage" => "total = 3072.00M  used = 1218.75M  free = 1853.25M  (encrypted)\n".into(),
        _ => return Err(format!("unknown oid '{key}'")),
    })
}

fn vm_stat() -> String {
    let n = lock(&SIM).n;
    let wob = |k: u64, amp: f64| (unit(n / 3, k) as f64 - 0.5) * amp;
    let free = 331_208.0 + wob(1, 18_000.0);
    let anon = 481_044.0 + wob(2, 12_000.0);
    let file = 703_117.0 + wob(3, 9_000.0);
    format!(
        "Mach Virtual Memory Statistics: (page size of 16384 bytes)\n\
Pages free:                             {free:>10.0}.\n\
Pages active:                           {:>10.0}.\n\
Pages inactive:                         {:>10.0}.\n\
Pages speculative:                      {:>10.0}.\n\
Pages throttled:                                 0.\n\
Pages wired down:                       {:>10.0}.\n\
Pages purgeable:                        {:>10.0}.\n\
\"Translation faults\":                  {:>10}.\n\
Pages copy-on-write:                      91234821.\n\
Pages zero filled:                      {:>10}.\n\
Pages reactivated:                         812204.\n\
Pages purged:                             1904412.\n\
File-backed pages:                      {file:>10.0}.\n\
Anonymous pages:                        {anon:>10.0}.\n\
Pages stored in compressor:             {:>10.0}.\n\
Pages occupied by compressor:           {:>10.0}.\n\
Decompressions:                           7783116.\n\
Compressions:                            11002871.\n\
Pageins:                                  3319542.\n\
Pageouts:                                   51120.\n\
Swapins:                                    90661.\n\
Swapouts:                                  118403.\n",
        290_113.0 + wob(4, 10_000.0),
        271_882.0 + wob(5, 10_000.0),
        14_906.0,
        258_901.0 + wob(6, 2_000.0),
        18_211.0 + wob(7, 3_000.0),
        412_903_551u64 + n * 1_731,
        301_446_118u64 + n * 1_208,
        362_450.0,
        124_118.0 + wob(8, 3_000.0),
    )
}

fn netstat() -> String {
    let n = {
        let mut s = lock(&SIM);
        s.n += 1;
        s.n
    };
    let (i, o) = {
        let mut s = lock(&SIM);
        // a few hundred kB a look, with the odd download
        let burst = if unit(n / 3, 91) > 0.9 { 14.0 } else { 1.0 };
        s.net.0 += ((90_000.0 + 650_000.0 * unit(n, 41)) * burst) as u64;
        s.net.1 += (30_000.0 + 190_000.0 * unit(n, 42)) as u64;
        s.net
    };
    format!(
        "Name       Mtu   Network       Address            Ipkts Ierrs     Ibytes    Opkts Oerrs     Obytes  Coll\n\
lo0        16384 <Link#1>                           2410552     0  389024311  2410552     0  389024311     0\n\
lo0        16384 127           127.0.0.1            2410552     -  389024311  2410552     -  389024311     -\n\
gif0*      1280  <Link#2>                                 0     0          0        0     0          0     0\n\
en0        1500  <Link#6>    3c:22:fb:a1:5e:07  {:>10}     0 {i:>10}  {:>8}     0 {o:>10}     0\n\
en0        1500  fe80::1c5e  fe80::1c5e:b2:9a3f {:>10}     -  {i:>10}  {:>8}     -  {o:>10}     -\n\
en0        1500  192.168.1     192.168.1.24       {:>10}     -  {i:>10}  {:>8}     -  {o:>10}     -\n\
awdl0      1500  <Link#12>   ae:5c:1f:02:99:d4     312044     0   41220118   298114     0   33910342     0\n\
bridge100  1500  <Link#18>   3e:22:fb:a1:5e:64   1890224     0  612774300  2201143     0 1844920711     0\n",
        i / 1180,
        o / 1210,
        i / 1180,
        o / 1210,
        i / 1180,
        o / 1210
    )
}

const DF: &str = "Filesystem       1024-blocks      Used Available Capacity iused      ifree %iused  Mounted on\n/dev/disk3s1s1     971350180  15106904 395431816     4%  426148 3954318160    0%   /\n";

const PMSET: &str = "Now drawing from 'Battery Power'\n -InternalBattery-0 (id=4653155)\t78%; discharging; 4:52 remaining present: true\n";

fn ioreg(a: &[&str]) -> Result<String, String> {
    let has = |s: &str| a.contains(&s);
    if has("IOAccelerator") {
        let n = lock(&SIM).n;
        let gpu = 4 + (unit(n / 2, 61) * 22.0) as u32;
        return Ok(format!(
            "+-o AGXAcceleratorG14X  <class AGXAcceleratorG14X, id 0x100000395, registered, matched, active, busy 0 (0 ms), retain 28>\n    {{\n      \"IOClass\" = \"AGXAcceleratorG14X\"\n      \"PerformanceStatistics\" = {{\"Alloc system memory\"=1172504576,\"In use system memory\"=210157568,\"Renderer Utilization %\"={},\"Tiler Utilization %\"={},\"Device Utilization %\"={gpu},\"SplitSceneCount\"=0,\"TiledSceneBytes\"=2097152}}\n    }}\n",
            gpu.saturating_sub(2),
            gpu.saturating_sub(3)
        ));
    }
    if has("IOBlockStorageDriver") {
        let (r, w) = {
            let mut s = lock(&SIM);
            let n = s.n;
            let burst = if unit(n / 5, 71) > 0.92 { 25.0 } else { 1.0 };
            s.disk.0 += ((180_000.0 + 2_400_000.0 * unit(n, 51)) * burst) as u64;
            s.disk.1 += (90_000.0 + 1_300_000.0 * unit(n, 52)) as u64;
            s.disk
        };
        return Ok(format!(
            "+-o AppleANS3NVMeController  <class IOBlockStorageDriver, id 0x100000412, registered, matched, active, busy 0 (0 ms), retain 6>\n    {{\n      \"Statistics\" = {{\"Operations (Write)\"=91233412,\"Bytes (Read)\"={r},\"Bytes (Write)\"={w},\"Total Time (Read)\"=1920334411,\"Operations (Read)\"=71204118}}\n    }}\n"
        ));
    }
    Ok(String::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opsdata as od;

    /// The containers' state is shared, so the tests that read or change it take turns.
    static SERIAL: Mutex<()> = Mutex::new(());

    #[test]
    fn five_containers_with_stats_and_state_changes() {
        let _turn = lock(&SERIAL);
        let raw = run("docker", &["ps", "-a", "--format", "{{json .}}"]).unwrap();
        let mut v: Vec<od::Container> = raw.lines().filter_map(od::parse_ps_line).collect();
        assert_eq!(v.len(), 5);
        let by = |v: &[od::Container], n: &str| v.iter().find(|c| c.name == n).cloned().unwrap();
        let api = by(&v, "shop-api");
        assert_eq!((api.image.as_str(), api.project.as_str(), api.service.as_str(), api.running()), ("shop-api:dev", "shop", "api", true));
        assert_eq!(od::parse_published(&api.ports), vec![(8080, "8080/tcp".to_string())]);
        assert_eq!(by(&v, "shop-mail").health.as_deref(), Some("healthy"));
        assert_eq!(by(&v, "shop-mail").ports.matches("->").count(), 4);
        assert!(!by(&v, "shop-worker").running() && by(&v, "shop-worker").status.starts_with("Exited (0)"));
        od::apply_stats(&mut v, &run("docker", &["stats", "--no-stream", "--format", "{{json .}}"]).unwrap());
        let db = by(&v, "shop-db");
        assert!(db.mem_used > 40e6 && db.cpu > 0.0 && db.mem.ends_with("/ 7.653GiB"), "{db:?}");
        od::sort_containers(&mut v);
        assert_eq!(v.last().unwrap().name, "shop-worker");
        // the filter the recordings use
        assert_eq!(run("docker", &["ps", "-a", "--filter", "name=shop-d", "--format", "{{json .}}"]).unwrap().lines().count(), 1);
    }

    #[test]
    fn logs_are_timestamped_ordered_and_tailed() {
        let _turn = lock(&SERIAL);
        for (name, min) in [("shop-api", 20), ("shop-db", 12), ("shop-cache", 7), ("shop-mail", 3), ("shop-worker", 3)] {
            let raw = run("docker", &["logs", "--tail", "300", "--timestamps", name]).unwrap();
            let lines = od::merge_logs(&raw, "", 300);
            assert!(lines.len() >= min, "{name}: {}", lines.len());
            let mut last = None;
            for l in &lines {
                let (ts, msg) = l.split_once(' ').unwrap();
                let (secs, _) = od::epoch_from_iso(ts).unwrap_or_else(|| panic!("bad stamp in {l}"));
                assert!(last.is_none_or(|p| p <= secs) && !msg.is_empty());
                last = Some(secs);
                assert_eq!(od::local_hms(ts).len(), 12);
            }
        }
        let api = run("docker", &["logs", "--tail", "5", "--timestamps", "a3f1c9d2e847"]).unwrap();
        assert_eq!(api.lines().count(), 5);
        assert!(run("docker", &["logs", "--tail", "5", "--timestamps", "nope"]).unwrap_err().contains("No such container"));
        // a second look finds the same lines
        let again = run("docker", &["logs", "--tail", "5", "--timestamps", "a3f1c9d2e847"]).unwrap();
        assert_eq!(api, again);
    }

    #[test]
    fn container_actions_change_what_the_list_shows() {
        let _turn = lock(&SERIAL);
        let names = |f: &str| run("docker", &["ps", "-a", "--format", "{{json .}}", "--filter", f]).unwrap();
        assert!(names("name=shop-worker").contains("exited"));
        run("docker", &["start", "shop-worker"]).unwrap();
        assert!(names("name=shop-worker").contains("\"State\":\"running\""));
        assert!(od::listeners(&[]).is_ok());
        run("docker", &["stop", "shop-worker"]).unwrap();
        assert!(names("name=shop-worker").contains("\"State\":\"exited\""));
        assert!(run("docker", &["restart", "nope"]).is_err());
    }

    #[test]
    fn listening_ports_name_their_owners() {
        let _turn = lock(&SERIAL);
        let raw = run("lsof", &["+c", "0", "-nP", "-iTCP", "-sTCP:LISTEN", "-F", "pcn"]).unwrap();
        let mut ls = od::parse_lsof(&raw);
        let cs: Vec<od::Container> = run("docker", &["ps", "-a", "--format", "{{json .}}"]).unwrap().lines().filter_map(od::parse_ps_line).collect();
        od::map_containers(&mut ls, &cs);
        let at = |p: u16| ls.iter().find(|l| l.port == p).unwrap_or_else(|| panic!("nothing on {p}"));
        for (p, svc) in [(8080, "api"), (5432, "db"), (6379, "cache"), (8025, "mail"), (1025, "mail")] {
            assert_eq!((at(p).container.is_some(), at(p).service.as_str(), at(p).command.as_str()), (true, svc, "com.docker.backend"));
        }
        assert_eq!((at(5173).command.as_str(), at(5173).addr.as_str(), at(3000).command.as_str()), ("node", "[::1]", "shop-api"));
        assert!(at(3000).container.is_none());
        assert!(ls.windows(2).all(|w| w[0].port < w[1].port));
        // who owns a port, and how many are connected
        let owner = run("lsof", &["-nP", "-iTCP:5173", "-sTCP:LISTEN", "-t"]).unwrap();
        assert_eq!(owner.trim().parse::<u32>().unwrap(), at(5173).pid);
        assert_eq!(run("lsof", &["-nP", "-iTCP:9", "-sTCP:LISTEN", "-t"]).unwrap_err(), "failed");
        let c = run("lsof", &["-nP", "-iTCP:5432", "-sTCP:ESTABLISHED", "-F", "n"]).unwrap();
        assert!((3..=6).contains(&od::parse_connections(&c, 5432)));
        let comm = run("ps", &["-p", &at(5173).pid.to_string(), "-o", "comm="]).unwrap();
        assert!(comm.trim().ends_with("/node"));
    }

    #[test]
    fn a_system_sample_reads_and_its_counters_advance() {
        let ps = od::parse_ps(&run("ps", &["-Ao", "pid,pcpu,rss,comm"]).unwrap());
        assert!(ps.len() >= 25, "{}", ps.len());
        assert!(ps.iter().any(|p| p.name == "Sphere") && ps.iter().any(|p| p.name == "WindowServer") && ps.iter().any(|p| p.name == "Code Helper (Renderer)"));
        let top = od::top_procs(ps, 8);
        assert!(top.len() >= 8 && top[0].cpu > 3.0 && top.iter().any(|p| p.rss_mb > 1000.0));
        let m = od::parse_vm_stat(&run("vm_stat", &[]).unwrap());
        assert!(m[0] > 4.0 && m[1] > 3.0 && m[2] > 1.0 && m[3] > 8.0 && m[4] > 3.0, "{m:?}");
        assert!(m.iter().sum::<f32>() < 32.0, "{m:?}");
        let (n1, n2) = (od::parse_netstat(&run("netstat", &["-inb"]).unwrap()), od::parse_netstat(&run("netstat", &["-inb"]).unwrap()));
        assert!(n1.0 > 1_000_000 && n2.0 > n1.0 && n2.1 > n1.1, "{n1:?} {n2:?}");
        let d1 = od::parse_ioreg_disk(&run("ioreg", &["-c", "IOBlockStorageDriver", "-r", "-w0", "-d", "1"]).unwrap());
        let d2 = od::parse_ioreg_disk(&run("ioreg", &["-c", "IOBlockStorageDriver", "-r", "-w0", "-d", "1"]).unwrap());
        assert!(d2.0 > d1.0 && d2.1 > d1.1);
        let g = od::parse_gpu(&run("ioreg", &["-r", "-d", "1", "-w", "0", "-c", "IOAccelerator"]).unwrap()).unwrap();
        assert!(g > 0.0 && g < 0.5);
        let (used, total) = od::parse_df(&run("df", &["-k", "/"]).unwrap());
        assert!((total - 994.7).abs() < 1.0 && used > 100.0, "{used} {total}");
        let (src, bat) = od::parse_pmset(&run("pmset", &["-g", "batt"]).unwrap());
        assert_eq!((src.as_str(), bat), ("Battery Power", Some((78, "discharging".into(), "4:52 remaining".into()))));
        assert_eq!(run("sysctl", &["-n", "machdep.cpu.brand_string"]).unwrap().trim(), "Apple M2 Pro");
        assert_eq!(run("sysctl", &["-n", "hw.memsize"]).unwrap().trim().parse::<f64>().unwrap() / 1073741824.0, 32.0);
        assert!(run("sysctl", &["-n", "vm.swapusage"]).unwrap().contains("used = 1218.75M"));
        let la = run("sysctl", &["-n", "vm.loadavg"]).unwrap();
        let v: Vec<f32> = la.trim_matches(|c| c == '{' || c == '}' || c == ' ' || c == '\n').split_whitespace().map(|x| x.parse().unwrap()).collect();
        assert_eq!(v.len(), 3);
        // the cores' ticks only go up, and the use between two looks is a fraction
        let (a, b) = (core_ticks(), core_ticks());
        assert_eq!(a.len(), 12);
        let u = od::total_use(&a, &b);
        assert!(u > 0.02 && u < 0.9, "{u}");
        assert!(od::core_use(&a, &b).iter().all(|x| (0.0..=1.0).contains(x)));
    }

    #[test]
    fn stamps_are_rfc3339() {
        assert_eq!(iso(0, 0), "1970-01-01T00:00:00.000000000Z");
        assert_eq!(iso(1_792_221_000, 123_000_000), "2026-10-17T07:10:00.123000000Z");
        assert_eq!(plain(1_709_251_199), "2024-02-29 23:59:59");
        assert_eq!(fmt_redis(1_792_221_000).split(' ').take(3).collect::<Vec<_>>(), vec!["17", "Oct", "2026"]);
    }
}
