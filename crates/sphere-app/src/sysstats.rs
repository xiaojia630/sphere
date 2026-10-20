//! This machine's load for the Telemetry panel and the core's load deck: processor, memory, GPU and network,
//! sampled every two seconds on a background thread straight from the kernel and the GPU driver (macOS; no process
//! is started), and only while Sphere is on screen. Containers are counted with the docker CLI at start and again
//! after each change `docker events` reports. Nothing here runs on the frame path.
use bevy::prelude::*;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::mpsc::{sync_channel, Receiver};
use std::sync::{Arc, Mutex, Once};
use std::time::{Duration, Instant};

mod native;
use native::{Meter, Reading};

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
    // containers: on their own thread, so a stuck engine never holds the load figures
    let dk = s.0.clone();
    let _ = std::thread::Builder::new().name("sphere-docker".into()).spawn(move || watch_docker(&dk));
    let out = s.0.clone();
    let _ = std::thread::Builder::new().name("sphere-stats".into()).spawn(move || {
        let mut meter = Meter::new();
        let mut last: Option<(Reading, Instant)> = None;
        loop {
            if crate::summon::on_screen() {
                // the first look (at start, or after being hidden) has nothing to compare with: take it now and the one to compare it with shortly
                let (r0, t0) = last.take().unwrap_or_else(|| {
                    let first = (meter.read(), Instant::now());
                    std::thread::sleep(Duration::from_millis(250));
                    first
                });
                let (r1, t1) = (meter.read(), Instant::now());
                let net = net_rate(&r0.nets, &r1.nets, t1.duration_since(t0).as_secs_f32());
                let mut st = out.lock().unwrap();
                let docker = st.docker;
                *st = Stats { cpu: busy(&r0.ticks, &r1.ticks), mem: r1.mem, gpu: r1.gpu, net, ready: true, docker };
                drop(st);
                last = Some((r1, t1));
            } else {
                // hidden: not even a baseline is kept, it would be stale by the time Sphere is back
                last = None;
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    });
    s
}

/// How busy the processors were between two looks at their ticks, 0..1: everything but idle, over all of it.
fn busy(a: &[u32; 4], b: &[u32; 4]) -> f32 {
    const IDLE: usize = 2;
    // all zeros: the kernel didn't answer
    if *a == [0; 4] || *b == [0; 4] {
        return 0.0;
    }
    let d = |i: usize| b[i].wrapping_sub(a[i]) as u64;
    let total: u64 = (0..4).map(d).sum();
    if total == 0 {
        0.0
    } else {
        (total - d(IDLE)) as f32 / total as f32
    }
}

/// Bytes per second over the interfaces both looks saw. An interface that came or went in between counts for nothing, and so does a
/// counter that went backwards (it was recreated); the span is never taken as less than a tenth of a second.
fn net_rate(a: &[(u16, u64)], b: &[(u16, u64)], secs: f32) -> f32 {
    let moved: u64 = b.iter().filter_map(|(i, n)| a.iter().find(|(j, _)| j == i).map(|(_, m)| n.saturating_sub(*m))).sum();
    moved as f32 / secs.max(0.1)
}

/// How long the docker watcher waits for what. A change is counted once the engine has been quiet for `quiet`, or `cap` after the first
/// of a burst; a `docker ps` still running after `give_up` is killed (an engine that is starting can hang); while Sphere is off screen
/// `visible` is looked at every `poll`. The tests use quicker ones.
struct Pace {
    quiet: Duration,
    cap: Duration,
    give_up: Duration,
    poll: Duration,
    visible: fn() -> bool,
}

const LIVE: Pace = Pace { quiet: Duration::from_millis(300), cap: Duration::from_secs(2), give_up: Duration::from_secs(5), poll: Duration::from_secs(2), visible: crate::summon::on_screen };

/// What `docker events` is asked for: the changes that move the counts, not the exec, attach and health-check chatter a busy stack makes all
/// day. The format names `Action`: Docker 29's CLI rejects `Status`, which older ones had.
const EVENT_ARGS: &[&str] = &[
    "events", "--filter", "type=container", "--filter", "event=create", "--filter", "event=destroy", "--filter", "event=die", "--filter", "event=pause", "--filter", "event=restart", "--filter", "event=start",
    "--filter", "event=unpause", "--format", "{{.Action}}",
];

/// Keeps `Stats::docker` current, while Sphere is on screen: a count at the start and after each change the engine reports, and with no
/// engine (or no CLI) another look every 30 s.
fn watch_docker(out: &Mutex<Stats>) {
    let publish = |d: Docker| {
        if let Ok(mut st) = out.lock() {
            st.docker = d;
        }
    };
    loop {
        wait_on_screen(&LIVE);
        watch_once(&docker_bin(), std::env::var("SPHERE_DOCKER_FILTER").ok().as_deref(), &publish, &LIVE);
        std::thread::sleep(Duration::from_secs(30));
    }
}

/// One stretch of watching: counts, then counts again after each change until the event stream ends (the engine quit, or there never was
/// one), then once more to say what is left. Comes back at once, saying `Off`, when the first count finds no engine.
fn watch_once(bin: &Path, filter: Option<&str>, publish: &dyn Fn(Docker), pace: &Pace) {
    // the events first, so a change between them starting and the count isn't missed
    let Some(events) = Events::start(bin) else {
        publish(Docker::Off);
        return;
    };
    let first = count(bin, filter, pace.give_up);
    publish(first);
    if first == Docker::Off {
        return;
    }
    while settle(&events.rx, pace) {
        wait_on_screen(pace);
        publish(count(bin, filter, pace.give_up));
    }
    wait_on_screen(pace);
    publish(count(bin, filter, pace.give_up));
}

/// Waits for the engine to report a change, then for it to go quiet (`pace.quiet` without another, or `pace.cap` after the first, so a
/// stack coming up is one change). False once the stream has ended.
fn settle(changes: &Receiver<()>, pace: &Pace) -> bool {
    if changes.recv().is_err() {
        return false;
    }
    let first = Instant::now();
    while let Some(left) = pace.cap.checked_sub(first.elapsed()) {
        if changes.recv_timeout(pace.quiet.min(left)).is_err() {
            break;
        }
    }
    true
}

/// Sleeps until Sphere is on screen, looking every `pace.poll`.
fn wait_on_screen(pace: &Pace) {
    while !(pace.visible)() {
        std::thread::sleep(pace.poll);
    }
}

/// The pid of the `docker events` child that is running (0: none), for [`kill_events_at_exit`].
static EVENTS_PID: AtomicI32 = AtomicI32::new(0);
static EXIT_HOOK: Once = Once::new();

/// Runs when the process exits (Cmd+Q, the last window closing, `main` returning), which drops nothing: it ends the events child. A crash or
/// a kill -9 gets there too late, but then the child's pipe has no reader, and it dies of SIGPIPE the next time it has an event to write.
extern "C" fn kill_events_at_exit() {
    let pid = EVENTS_PID.load(Ordering::SeqCst);
    if pid > 0 {
        // SAFETY: kill(2) takes no pointers; at worst the process is gone already.
        unsafe { libc::kill(pid, libc::SIGKILL) };
    }
}

/// A running `docker events`: its child, and a channel that carries a ping when events arrive (one waiting is as good as many) and hangs
/// up when the stream ends. Dropping it kills the child.
struct Events {
    child: Child,
    rx: Receiver<()>,
}

impl Events {
    fn start(bin: &Path) -> Option<Events> {
        let mut child = Command::new(bin).args(EVENT_ARGS).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
        EVENTS_PID.store(child.id() as i32, Ordering::SeqCst);
        // SAFETY: registers a plain function that only calls kill(2).
        EXIT_HOOK.call_once(|| unsafe {
            libc::atexit(kill_events_at_exit);
        });
        let out = child.stdout.take();
        let (tx, rx) = sync_channel(1);
        // from here on, leaving early drops this, and the child with it
        let events = Events { child, rx };
        let out = out?;
        std::thread::Builder::new()
            .name("sphere-docker-events".into())
            .spawn(move || {
                for line in BufReader::new(out).split(b'\n') {
                    if line.is_err() {
                        break;
                    }
                    let _ = tx.try_send(());
                }
            })
            .ok()?;
        Some(events)
    }
}

impl Drop for Events {
    fn drop(&mut self) {
        let _ = self.child.kill();
        // forgotten before it is reaped, so the exit hook can never signal a pid that has been handed out again
        let _ = EVENTS_PID.compare_exchange(self.child.id() as i32, 0, Ordering::SeqCst, Ordering::SeqCst);
        let _ = self.child.wait();
    }
}

/// The docker CLI where Docker Desktop and Homebrew (OrbStack too) put it, since an app opened from the Dock has only the bare PATH.
fn docker_bin() -> PathBuf {
    ["/usr/local/bin/docker", "/opt/homebrew/bin/docker", "/Applications/Docker.app/Contents/Resources/bin/docker"].into_iter().find(|p| Path::new(p).exists()).unwrap_or("docker").into()
}

/// Counts containers with `docker ps -a`, giving up after `give_up` (an engine that is starting can hang).
fn count(bin: &Path, filter: Option<&str>, give_up: Duration) -> Docker {
    let mut args = vec!["ps", "-a", "--format", "{{.State}}"];
    if let Some(f) = filter {
        args.extend(["--filter", f]);
    }
    let Ok(mut child) = Command::new(bin).args(&args).stdout(Stdio::piped()).stderr(Stdio::null()).stdin(Stdio::null()).spawn() else { return Docker::Off };
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() < give_up => std::thread::sleep(Duration::from_millis(40)),
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
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::AtomicBool;

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

    #[test]
    fn busy_is_the_share_of_ticks_that_were_not_idle() {
        let near = |a: f32, b: f32| (a - b).abs() < 1e-6;
        // user, system, idle, nice: 100 + 50 + 50 busy against 800 idle
        assert!(near(busy(&[100, 50, 800, 50], &[200, 100, 1600, 100]), 0.2));
        assert_eq!(busy(&[1, 1, 1, 1], &[1, 1, 51, 1]), 0.0);
        assert_eq!(busy(&[1, 1, 1, 1], &[101, 1, 1, 1]), 1.0);
        // nothing ticked, or the kernel gave no answer (zeros)
        assert_eq!(busy(&[7, 7, 7, 7], &[7, 7, 7, 7]), 0.0);
        assert_eq!(busy(&[0; 4], &[5, 5, 5, 5]), 0.0);
        assert_eq!(busy(&[5, 5, 5, 5], &[0; 4]), 0.0);
        // the counters are 32-bit and wrap: 20 busy and 20 idle ticks across the wrap
        assert!(near(busy(&[u32::MAX - 9, 0, 20, 0], &[10, 0, 40, 0]), 0.5));
    }

    #[test]
    fn network_rates_count_only_interfaces_both_looks_saw() {
        // en5 went and en6 came in between: only en4's 2000 bytes in 2 s
        assert_eq!(net_rate(&[(4, 1_000), (5, 5_000)], &[(4, 3_000), (6, 9_999)], 2.0), 1_000.0);
        // a counter that went backwards moved nothing; a missing look moves nothing
        assert_eq!(net_rate(&[(4, 9_000)], &[(4, 3_000)], 2.0), 0.0);
        assert_eq!(net_rate(&[], &[(4, 3_000)], 2.0), 0.0);
        // the span is at least 0.1 s
        assert_eq!(net_rate(&[(4, 0)], &[(4, 50)], 0.0), 500.0);
        // 64-bit counters carry on past 4 GB
        assert_eq!(net_rate(&[(4, 6_000_000_000)], &[(4, 6_400_000_000)], 2.0), 200_000_000.0);
    }

    #[test]
    fn a_burst_of_changes_settles_into_one() {
        // quiet is far longer than the gaps, so a busy machine that wakes the feeder late still lands inside it
        let pace = Pace { quiet: Duration::from_millis(500), cap: Duration::from_secs(4), ..FAST };
        let (tx, rx) = sync_channel(1);
        // changes at 0, 50, 100 and 150 ms: settled 500 ms after the last, long before the cap
        tx.send(()).unwrap();
        let feeder = std::thread::spawn(move || {
            for _ in 0..3 {
                std::thread::sleep(Duration::from_millis(50));
                let _ = tx.try_send(());
            }
            std::thread::sleep(Duration::from_millis(1200));
            tx
        });
        let t = Instant::now();
        assert!(settle(&rx, &pace));
        assert!(t.elapsed() >= Duration::from_millis(640) && t.elapsed() < Duration::from_millis(3000), "settled after {:?}", t.elapsed());
        // the stream ends: whatever was still waiting counts once more, then it is over
        let tx = feeder.join().unwrap();
        tx.send(()).unwrap();
        drop(tx);
        assert!(settle(&rx, &pace));
        assert!(!settle(&rx, &pace));
    }

    #[test]
    fn changes_that_never_stop_are_counted_at_the_cap() {
        // quiet is far longer than the gaps, so only the cap can end it, even when a busy machine wakes the feeder late
        let pace = Pace { quiet: Duration::from_millis(500), cap: Duration::from_secs(1), ..FAST };
        let (tx, rx) = sync_channel(1);
        tx.send(()).unwrap();
        // a change every 50 ms for up to 3 s
        let feeder = std::thread::spawn(move || {
            for _ in 0..60 {
                std::thread::sleep(Duration::from_millis(50));
                if tx.try_send(()).is_err() && tx.send(()).is_err() {
                    break;
                }
            }
        });
        let t = Instant::now();
        assert!(settle(&rx, &pace));
        assert!(t.elapsed() >= Duration::from_millis(990) && t.elapsed() < Duration::from_secs(3), "settled after {:?}", t.elapsed());
        drop(rx);
        feeder.join().unwrap();
    }

    // ---- the docker watcher, against a stand-in `docker`

    /// Works like `docker` for what the watcher asks, steered through files in its own directory: `ps` prints `ps.out` (fails if `ps.fail`
    /// exists, hangs if `ps.hang` does) and logs its arguments to `ps.log`; `events` notes its pid in `events.pid` and prints two lines, 0.1 s apart, for
    /// each `fire` file it finds, until `stop` appears (or about half a minute is up).
    const FAKE: &str = r#"#!/bin/sh
d=$(dirname "$0")
case "$1" in
ps)
  echo "$*" >> "$d/ps.log"
  if [ -e "$d/ps.fail" ]; then exit 1; fi
  if [ -e "$d/ps.hang" ]; then echo $$ > "$d/ps.pid"; exec sleep 30; fi
  cat "$d/ps.out" ;;
events)
  echo $$ > "$d/events.pid"
  n=0
  while [ ! -e "$d/stop" ] && [ $n -lt 600 ]; do
    if [ -e "$d/fire" ]; then rm -f "$d/fire"; echo start; sleep 0.1; echo die; fi
    sleep 0.05
    n=$((n+1))
  done ;;
esac
"#;

    static SHOWN: AtomicBool = AtomicBool::new(true);

    fn shown() -> bool {
        SHOWN.load(Ordering::SeqCst)
    }

    const FAST: Pace = Pace { quiet: Duration::from_millis(400), cap: Duration::from_secs(2), give_up: Duration::from_secs(5), poll: Duration::from_millis(20), visible: shown };

    /// The watcher tests share the stand-in's pid bookkeeping and the clock, so they take turns.
    fn turn() -> std::sync::MutexGuard<'static, ()> {
        static TURN: Mutex<()> = Mutex::new(());
        let guard = TURN.lock().unwrap_or_else(|e| e.into_inner());
        SHOWN.store(true, Ordering::SeqCst);
        guard
    }

    /// A directory with the stand-in `docker` in it. When it goes out of scope (a test that fails halfway too) the stand-in's `events` is told
    /// to stop, and the directory is removed once that has ended, so nothing is left running or lying about.
    struct Fake {
        dir: PathBuf,
        bin: PathBuf,
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            let _ = std::fs::write(self.dir.join("stop"), "");
            if let Some(pid) = pid_in(&self.dir, "events.pid") {
                let t = Instant::now();
                while alive(pid) && t.elapsed() < Duration::from_secs(2) {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn fake_docker(name: &str) -> Fake {
        let dir = std::env::temp_dir().join(format!("sphere-sysstats-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("docker");
        std::fs::write(&bin, FAKE).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        // a script just written can look busy to exec if another thread forked meanwhile: run it once until it starts
        for _ in 0..200 {
            if Command::new(&bin).arg("noop").status().is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Fake { dir, bin }
    }

    fn put(dir: &Path, file: &str, text: &str) {
        std::fs::write(dir.join(file), text).unwrap();
    }

    /// Makes the stand-in's `events` print its two lines (the file appears whole, never half written).
    fn fire(dir: &Path) {
        put(dir, "fire.tmp", "");
        std::fs::rename(dir.join("fire.tmp"), dir.join("fire")).unwrap();
    }

    fn ps_calls(dir: &Path) -> usize {
        std::fs::read_to_string(dir.join("ps.log")).map_or(0, |s| s.lines().count())
    }

    fn pid_in(dir: &Path, file: &str) -> Option<i32> {
        std::fs::read_to_string(dir.join(file)).ok()?.trim().parse().ok()
    }

    fn alive(pid: i32) -> bool {
        // SAFETY: signal 0 only checks that the process exists.
        unsafe { libc::kill(pid, 0) == 0 }
    }

    fn wait_for(what: &str, mut ok: impl FnMut() -> bool) {
        let t = Instant::now();
        while !ok() {
            assert!(t.elapsed() < Duration::from_secs(10), "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn containers_are_counted_at_the_start_and_after_each_burst_of_changes() {
        let _turn = turn();
        let fake = fake_docker("watch");
        let Fake { dir, bin } = &fake;
        put(dir, "ps.out", "running\nexited\n");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (log, bin) = (seen.clone(), bin.clone());
        let watcher = std::thread::spawn(move || watch_once(&bin, Some("label=demo"), &|d| log.lock().unwrap().push(d), &FAST));
        wait_for("the first count", || *seen.lock().unwrap() == [Docker::Count { running: 1, total: 2 }]);
        // two events 0.1 s apart (the quiet time is longer) are one more count, not two
        put(dir, "ps.out", "running\nrunning\nexited\n");
        fire(dir);
        wait_for("the count after the change", || seen.lock().unwrap().last() == Some(&Docker::Count { running: 2, total: 3 }));
        std::thread::sleep(FAST.quiet * 2);
        assert_eq!(ps_calls(dir), 2);
        // the filter is passed on to `ps`
        assert!(std::fs::read_to_string(dir.join("ps.log")).unwrap().contains("--filter label=demo"));
        // the engine goes away: the events end, and the count after them says so
        let events = pid_in(dir, "events.pid").unwrap();
        put(dir, "ps.fail", "");
        put(dir, "stop", "");
        watcher.join().unwrap();
        assert_eq!(*seen.lock().unwrap(), [Docker::Count { running: 1, total: 2 }, Docker::Count { running: 2, total: 3 }, Docker::Off]);
        assert!(!alive(events));
    }

    #[test]
    fn nothing_is_counted_while_hidden() {
        let _turn = turn();
        let fake = fake_docker("hidden");
        let Fake { dir, bin } = &fake;
        put(dir, "ps.out", "running\n");
        let bin = bin.clone();
        let watcher = std::thread::spawn(move || watch_once(&bin, None, &|_| {}, &FAST));
        wait_for("the first count", || ps_calls(dir) == 1);
        SHOWN.store(false, Ordering::SeqCst);
        fire(dir);
        std::thread::sleep(FAST.quiet * 3);
        assert_eq!(ps_calls(dir), 1);
        // back on screen: what changed meanwhile is counted
        SHOWN.store(true, Ordering::SeqCst);
        wait_for("the count after coming back", || ps_calls(dir) == 2);
        put(dir, "stop", "");
        watcher.join().unwrap();
    }

    #[test]
    fn a_missing_cli_or_a_hung_ps_is_off() {
        let _turn = turn();
        let seen = Mutex::new(Vec::new());
        watch_once(Path::new("/nonexistent/docker"), None, &|d| seen.lock().unwrap().push(d), &FAST);
        assert_eq!(*seen.lock().unwrap(), [Docker::Off]);
        let fake = fake_docker("hung");
        let Fake { dir, bin } = &fake;
        put(dir, "ps.hang", "");
        let t = Instant::now();
        assert_eq!(count(bin, None, Duration::from_millis(500)), Docker::Off);
        assert!(t.elapsed() < Duration::from_secs(5), "gave up after {:?}", t.elapsed());
        // and the hung one was killed
        assert!(pid_in(dir, "ps.pid").is_none_or(|pid| !alive(pid)));
        // an engine that is off: the first count says so and the events child doesn't linger
        put(dir, "ps.fail", "");
        seen.lock().unwrap().clear();
        watch_once(bin, None, &|d| seen.lock().unwrap().push(d), &FAST);
        assert_eq!(*seen.lock().unwrap(), [Docker::Off]);
        assert!(pid_in(dir, "events.pid").is_none_or(|pid| !alive(pid)));
    }

    #[test]
    fn the_events_child_dies_with_its_handle_and_at_exit() {
        let _turn = turn();
        let fake = fake_docker("exit");
        let Fake { dir, bin } = &fake;
        let events = Events::start(bin).unwrap();
        wait_for("the events child", || pid_in(dir, "events.pid").is_some());
        let pid = pid_in(dir, "events.pid").unwrap();
        assert!(alive(pid));
        drop(events);
        assert!(!alive(pid));
        // what the exit hook does
        let mut events = Events::start(bin).unwrap();
        kill_events_at_exit();
        wait_for("the child to die", || events.child.try_wait().unwrap().is_some());
    }
}
