//! The System app: this machine's processor, memory, GPU, network, disk and battery, sampled
//! every two seconds while it's open, and the processes using the most. Keys: ↑ ↓ (or J) select a process, S sort
//! by CPU or memory, K stop it (asks twice), Esc closes.
use crate::appframe::*;
use crate::core::CoreCtl;
use crate::fonts::UiFonts;
use crate::opsdata::{self, NetMark, Sys};
use crate::panel::*;
use crate::service::Service;
use crate::util::*;
use bevy::prelude::*;
use std::sync::{mpsc, Mutex};
use web_time::Instant;

pub const APP: u8 = 4;
const TOP: usize = 0;
const CPU: usize = 1;
const MEM: usize = 2;
const GPU: usize = 3;
const NET: usize = 4;
const DISK: usize = 5;
const PROCS: usize = 6;
const STRIP: usize = 7;
const PANES: [([f32; 5], bool); 8] = [
    ([22.0, 2.0, 1236.0, 32.0, 8.0], true),
    ([14.0, 42.0, 420.0, 400.0, 18.0], false),
    ([446.0, 42.0, 420.0, 200.0, 30.0], false),
    ([446.0, 254.0, 420.0, 188.0, 24.0], false),
    ([878.0, 42.0, 388.0, 200.0, 44.0], false),
    ([878.0, 254.0, 388.0, 188.0, 36.0], false),
    ([14.0, 454.0, 1252.0, 172.0, 40.0], false),
    ([14.0, 638.0, 1252.0, 50.0, 22.0], false),
];
const HIST: usize = 40;
/// Process rows shown at a time.
const ROWS: usize = 6;
/// The disk, swap and battery are read every this many samples.
const SLOW_EVERY: u32 = 5;

type Sample = (Sys, NetMark);
type Note = Result<String, String>;

#[derive(Resource)]
pub struct SysApp {
    pub open: bool,
    pub amt: Spring,
    sys: Option<Sys>,
    cpu_h: Vec<f32>,
    gpu_h: Vec<f32>,
    in_h: Vec<f32>,
    out_h: Vec<f32>,
    /// the whole processor's use 0..1 and each core's, from the last two tick readings (none until there are two)
    cpu: Option<f32>,
    cores: Option<Vec<f32>>,
    ticks_prev: Vec<(u64, u64)>,
    /// the next reading only sets the baseline (the last one is from before the app was closed)
    rebase: bool,
    /// disk bytes (read, written) last time and when, and the rates since
    disk_prev: Option<((u64, u64), Instant)>,
    disk_rw: Option<(f32, f32)>,
    disk_h: Vec<f32>,
    net_prev: NetMark,
    net_rate: Option<(f32, f32)>,
    loading: bool,
    load_t: f32,
    samples: u32,
    rx: Option<Mutex<mpsc::Receiver<Sample>>>,
    /// the selected process, by pid so a re-sort never moves it
    sel_pid: Option<u32>,
    by_mem: bool,
    armed: Option<(f32, u32)>,
    notes: (mpsc::Sender<Note>, Mutex<mpsc::Receiver<Note>>),
    version: u64,
}

impl Default for SysApp {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        SysApp { open: false, amt: Spring::new(0.0, 26.0, 9.0), sys: None, cpu_h: Vec::new(), gpu_h: Vec::new(), in_h: Vec::new(), out_h: Vec::new(), cpu: None, cores: None, ticks_prev: Vec::new(), rebase: false, disk_prev: None, disk_rw: None, disk_h: Vec::new(), net_prev: None, net_rate: None, loading: false, load_t: -99.0, samples: 0, rx: None, sel_pid: None, by_mem: false, armed: None, notes: (tx, Mutex::new(rx)), version: 0 }
    }
}

impl SysApp {
    pub fn open(&mut self) {
        self.open = true;
        self.load_t = -99.0;
        // rates are measured between two readings taken while the app is open; until the second one the last
        // rates stay on screen rather than blanks
        self.rebase = true;
        self.disk_prev = None;
        self.net_prev = None;
        self.version += 1;
    }
    pub fn close(&mut self) {
        self.open = false;
        self.armed = None;
        self.version += 1;
    }
    /// Every process sampled, by CPU or by memory.
    fn procs(&self) -> Vec<opsdata::Proc> {
        let mut v = self.sys.as_ref().map(|s| s.procs.clone()).unwrap_or_default();
        if self.by_mem {
            v.sort_by(|a, b| b.rss_mb.total_cmp(&a.rss_mb));
        } else {
            v.sort_by(|a, b| b.cpu.total_cmp(&a.cpu));
        }
        v
    }
    fn sel(&self, procs: &[opsdata::Proc]) -> usize {
        self.sel_pid.and_then(|p| procs.iter().position(|x| x.pid == p)).unwrap_or(0)
    }
    fn select(&mut self, procs: &[opsdata::Proc], i: usize) {
        if let Some(p) = procs.get(i) {
            if self.sel_pid != Some(p.pid) {
                self.sel_pid = Some(p.pid);
                self.armed = None;
            }
        }
        self.version += 1;
    }
}

fn push(h: &mut Vec<f32>, v: f32) {
    h.push(v);
    if h.len() > HIST {
        h.remove(0);
    }
}

#[derive(Resource)]
pub struct SysRoots(Workspace);

pub fn setup(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut images: ResMut<Assets<Image>>, (home, mut atlases): (Res<PanelAtlas>, ResMut<AppAtlases>), mut pmats: ResMut<Assets<PanelMaterial>>, args: Res<crate::Args>, mut app: ResMut<SysApp>) {
    if args.open_app.as_deref() == Some("system") {
        app.open();
    }
    let ws = spawn_workspace(&mut commands, &mut meshes, &mut images, &home, &mut atlases, &mut pmats, APP, &PANES);
    commands.insert_resource(SysRoots(ws));
}

#[allow(clippy::too_many_arguments)]
/// The key the wheel presses over a pane: the process table steps its rows.
fn scroll_key(pane: Option<u8>, down: bool) -> Option<KeyCode> {
    (pane? as usize == PROCS).then_some(if down { KeyCode::ArrowDown } else { KeyCode::ArrowUp })
}

#[allow(clippy::too_many_arguments)]
pub fn keys(
    mut ev: MessageReader<bevy::input::keyboard::KeyboardInput>,
    keys: Res<ButtonInput<KeyCode>>,
    (focus, moment): (Res<Focus>, Res<crate::moment::Moment>),
    mut app: ResMut<SysApp>,
    sim: Res<crate::core::Sim>,
    mut sound: ResMut<crate::sound::Sound>,
    mut toasts: ResMut<crate::live::Toasts>,
    (mut clicks, mut scrolls): (ResMut<crate::appframe::AppClicks>, ResMut<crate::appframe::AppScrolls>),
    mut cmd_was: Local<bool>,
) {
    let mods = keys.any_pressed([KeyCode::SuperLeft, KeyCode::SuperRight, KeyCode::ControlLeft, KeyCode::ControlRight, KeyCode::AltLeft, KeyCode::AltRight]);
    let bar_busy = focus.cmd || *cmd_was;
    *cmd_was = focus.cmd;
    if !app.open {
        ev.clear();
        return;
    }
    let mut codes: Vec<KeyCode> = Vec::new();
    for e in ev.read() {
        if e.state != bevy::input::ButtonState::Pressed || mods || bar_busy || focus.terminal || moment.shown || (e.repeat && !matches!(e.key_code, KeyCode::ArrowUp | KeyCode::ArrowDown | KeyCode::PageUp | KeyCode::PageDown)) {
            continue;
        }
        codes.push(e.key_code);
    }
    codes.extend(crate::appframe::scroll_keys(scrolls.take(4), scroll_key));
    // clicks on the panes: a button presses its key, a row picks its process
    for c in clicks.take(4) {
        match c {
            Click::Key(k) => codes.push(k),
            Click::Pick(i) => {
                let procs = app.procs();
                app.select(&procs, i);
            }
            Click::Alt(_) => {}
        }
    }
    for key in codes {
        let procs = app.procs();
        let i = app.sel(&procs);
        match key {
            KeyCode::Escape => {
                app.close();
                sound.play_with("collapse", 0.7, 0.0);
            }
            KeyCode::KeyJ | KeyCode::ArrowDown if !procs.is_empty() => {
                app.select(&procs, (i + 1).min(procs.len() - 1));
                sound.play_q("tick", 40);
            }
            KeyCode::ArrowUp => {
                app.select(&procs, i.saturating_sub(1));
                sound.play_q("tick", 40);
            }
            KeyCode::PageDown if !procs.is_empty() => app.select(&procs, (i + ROWS).min(procs.len() - 1)),
            KeyCode::PageUp => app.select(&procs, i.saturating_sub(ROWS)),
            KeyCode::KeyS => {
                app.by_mem = !app.by_mem;
                // the selection stays on its process
                app.version += 1;
                sound.play("toggle");
            }
            KeyCode::KeyK => {
                if let Some(p) = procs.get(i) {
                    if app.armed.is_some_and(|(t, pid)| sim.t < t && pid == p.pid) {
                        app.armed = None;
                        let tx = app.notes.0.clone();
                        let (pid, name) = (p.pid, p.name.clone());
                        crate::util::spawn(move || {
                            let _ = tx.send(opsdata::terminate_named(pid, &name));
                        });
                        sound.play("discard");
                    } else {
                        app.armed = Some((sim.t + 3.0, p.pid));
                        toasts.push_quiet(format!("Press K again to stop {} (pid {})", p.name, p.pid));
                        sound.play("toggle");
                    }
                    app.version += 1;
                }
            }
            _ => {}
        }
    }
}

pub fn update(sim: Res<crate::core::Sim>, mut atlases: ResMut<AppAtlases>, mut app: ResMut<SysApp>, mut ctl: ResMut<CoreCtl>, mut toasts: ResMut<crate::live::Toasts>, mut sound: ResMut<crate::sound::Sound>) {
    let open = app.open;
    step_open(open, &mut app.amt, sim.dt, &mut ctl);
    atlases.want(APP, open || app.amt.x > 0.001);
    if app.armed.is_some_and(|(t, _)| sim.t > t) {
        app.armed = None;
        app.version += 1;
    }
    let notes: Vec<Note> = app.notes.1.lock().map(|r| r.try_iter().collect()).unwrap_or_default();
    for n in notes {
        match n {
            Ok(s) => toasts.push_quiet(s),
            Err(e) => {
                toasts.push(clip(&e, 110));
                sound.play("error");
            }
        }
        // look again soon: the process needs a moment to go
        app.load_t = app.load_t.min(sim.t - 1.2);
    }
    let got: Vec<Sample> = app.rx.as_ref().and_then(|r| r.lock().ok().map(|r| r.try_iter().collect())).unwrap_or_default();
    for (s, net_mark) in got {
        app.loading = false;
        app.apply(s, net_mark);
    }
    if !app.open || app.loading || sim.t - app.load_t < 2.0 {
        return;
    }
    app.load_t = sim.t;
    app.loading = true;
    let prev = app.net_prev;
    let old = app.sys.clone();
    let slow = app.samples.is_multiple_of(SLOW_EVERY);
    let (tx, rx) = mpsc::channel();
    app.rx = Some(Mutex::new(rx));
    crate::util::spawn(move || {
        let _ = tx.send(opsdata::sample(prev, old.as_ref(), slow));
    });
}

impl SysApp {
    /// Takes in one sample: the rates are worked out against the one before, and show nothing until there is one.
    fn apply(&mut self, s: Sys, net_mark: NetMark) {
        self.samples += 1;
        let had_net = self.net_prev.is_some();
        self.net_prev = net_mark;
        if self.ticks_prev.len() == s.cores_ticks.len() && !self.ticks_prev.is_empty() && !std::mem::take(&mut self.rebase) {
            let cpu = opsdata::total_use(&self.ticks_prev, &s.cores_ticks);
            push(&mut self.cpu_h, cpu);
            self.cpu = Some(cpu);
            self.cores = Some(opsdata::core_use(&self.ticks_prev, &s.cores_ticks));
        }
        self.ticks_prev = s.cores_ticks.clone();
        if let (Some(((r0, w0), t0)), Some(at)) = (self.disk_prev, s.at) {
            let dt = at.duration_since(t0).as_secs_f32().max(0.2);
            let rw = (s.disk_bytes.0.saturating_sub(r0) as f32 / dt, s.disk_bytes.1.saturating_sub(w0) as f32 / dt);
            push(&mut self.disk_h, rw.0 + rw.1);
            self.disk_rw = Some(rw);
        }
        self.disk_prev = s.at.map(|at| (s.disk_bytes, at));
        if let Some(g) = s.gpu {
            push(&mut self.gpu_h, g);
        }
        if had_net {
            push(&mut self.in_h, s.net_in);
            push(&mut self.out_h, s.net_out);
            self.net_rate = Some((s.net_in, s.net_out));
        }
        self.sys = Some(s);
        let procs = self.procs();
        let i = self.sel(&procs);
        self.select(&procs, i);
    }
}

pub fn place(app: Res<SysApp>, mut q: Query<(&mut Panel, &AppPart)>) {
    place_app(APP, app.amt.x, |_| false, &mut q);
}

fn rate(b: f32) -> String {
    if b >= 1e6 {
        format!("{:.1} MB/s", b / 1e6)
    } else {
        format!("{:.0} kB/s", b / 1e3)
    }
}

/// A rate, or a dash before there are two readings to measure one.
fn rate_or_dash(r: Option<f32>) -> String {
    r.map(rate).unwrap_or_else(|| "-".into())
}

/// What a core is called: E and P cores number from the first of each (Apple silicon lists the efficiency cores
/// first), a processor without that split just numbers them.
fn core_label(i: usize, n: usize, p: u32, e: u32) -> String {
    if p + e == n as u32 && e > 0 {
        if i < e as usize {
            format!("E{i}")
        } else {
            format!("P{}", i - e as usize)
        }
    } else {
        format!("{i}")
    }
}

fn pct_or_dash(v: Option<f32>) -> String {
    v.map(|v| format!("{:.0}%", v * 100.0)).unwrap_or_else(|| "...".into())
}

pub fn draw(mut commands: Commands, app: Res<SysApp>, svc: Res<Service>, roots: Option<Res<SysRoots>>, f: Res<UiFonts>, theme: Res<crate::theme::Theme>, mut seen: Local<(u64, u64, u64)>) {
    let Some(roots) = roots else { return };
    if app.amt.x < 0.001 && !app.open {
        return;
    }
    let (agents, merged, sver) = {
        let s = svc.shared.lock().unwrap();
        let mut v = s.tasks.clone();
        v.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        (v, s.merged.clone(), s.version)
    };
    let key = (app.version, sver, theme.version);
    if *seen == key {
        return;
    }
    *seen = key;
    let c = &mut commands;
    let ws = &roots.0;
    for r in &ws.panes {
        c.entity(*r).despawn_children();
    }
    c.entity(ws.corelab).despawn_children();
    let ui = Ui::new(&f);
    let Some(s) = &app.sys else {
        ui.corelab(c, ws.corelab, "System", "sampling…".into(), ui.line);
        ui.top(c, ws.panes[TOP], "System", String::new(), "sampled from ps, vm_stat, sysctl, netstat, ioreg, pmset");
        let b = ui.body(c, ws.panes[CPU]);
        let h = ui.header(c, "Processor", "sampling…", ui.dim);
        c.entity(b).add_child(h);
        return;
    };
    // used the way Activity Monitor counts it: app, wired and compressed (cached files can be dropped)
    let used = s.mem[0] + s.mem[1] + s.mem[2];
    ui.corelab(c, ws.corelab, "System", format!("cpu {} · memory {:.0}%", pct_or_dash(app.cpu), 100.0 * used / s.mem_total.max(1.0)), ui.line);
    ui.top(c, ws.panes[TOP], "System", s.chip.to_uppercase(), "sampled from ps, vm_stat, sysctl, netstat, ioreg, pmset");
    ui.strip(c, ws.panes[STRIP], &agents, &merged, ["↑ ↓ SELECT   S SORT CPU / MEMORY", "K STOP PROCESS   ESC CLOSE"]);
    // processor
    let b = ui.body(c, ws.panes[CPU]);
    let ncores = app.ticks_prev.len();
    let header = if s.cores.0 + s.cores.1 == ncores as u32 && s.cores.1 > 0 { format!("{} P + {} E cores", s.cores.0, s.cores.1) } else { format!("{ncores} cores") };
    let h = ui.header(c, "Processor", &header, ui.dim);
    let inner = c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(8), padding: UiRect { left: px(18), right: px(18), top: px(8), ..default() }, align_items: AlignItems::Center, ..default() }).id();
    c.entity(b).add_children(&[h, inner]);
    // one spoke per core round the total, longer the busier
    let dial = c.spawn(Node { width: px(380), height: px(272), flex_shrink: 0.0, ..default() }).id();
    let (cx, cy) = (190.0, 136.0);
    let n = ncores.max(1);
    for i in 0..ncores {
        let u = app.cores.as_ref().and_then(|c| c.get(i)).copied();
        let a = std::f32::consts::TAU * i as f32 / n as f32 - std::f32::consts::FRAC_PI_2;
        let dir = Vec2::new(a.cos(), a.sin());
        let r0 = 66.0;
        let uv = u.unwrap_or(0.0);
        let col = if uv > 0.85 { role_color(Role::Bad) } else if uv > 0.5 { role_color(Role::Acc) } else { role_color(Role::Fill) };
        ui.line(c, dial, Vec2::new(cx, cy) + dir * r0, Vec2::new(cx, cy) + dir * (r0 + 52.0), 3.0, with_alpha(ui.line, 0.12));
        if u.is_some() {
            ui.line(c, dial, Vec2::new(cx, cy) + dir * r0, Vec2::new(cx, cy) + dir * (r0 + 4.0 + 48.0 * uv), 3.0, col);
        }
        let lp = Vec2::new(cx, cy) + dir * 134.0;
        let lab = c.spawn(Node { position_type: PositionType::Absolute, left: px(lp.x - 16.0), top: px(lp.y - 12.0), width: px(32), flex_direction: FlexDirection::Column, align_items: AlignItems::Center, ..default() }).id();
        let l1 = ui.text(c, core_label(i, ncores, s.cores.0, s.cores.1), &f.ui600, 8.5, 1.0, ui.dim);
        let l2 = ui.text(c, u.map(|u| format!("{:.0}", u * 100.0)).unwrap_or_else(|| "–".into()), &f.mono, 10.0, 0.0, ui.line);
        c.entity(lab).add_children(&[l1, l2]);
        c.entity(dial).add_child(lab);
    }
    let cpu = app.cpu.unwrap_or(0.0);
    let ring = c
        .spawn((
            Node { position_type: PositionType::Absolute, left: px(cx - 56.0), top: px(cy - 56.0), width: px(112), height: px(112), border: UiRect::all(px(2)), border_radius: BorderRadius::MAX, flex_direction: FlexDirection::Column, align_items: AlignItems::Center, justify_content: JustifyContent::Center, ..default() },
            BorderColor::all(with_alpha(if cpu > 0.85 { role_color(Role::Bad) } else { role_color(Role::Fill) }, 0.35 + 0.65 * cpu)),
        ))
        .id();
    let pct = ui.text(c, pct_or_dash(app.cpu), &f.display, 24.0, 0.5, ui.hot);
    let cl = ui.text(c, "CPU", &f.ui600, 9.0, 2.4, ui.dim);
    c.entity(ring).add_children(&[pct, cl]);
    c.entity(dial).add_child(ring);
    let la = ui.text(c, format!("load {:.2} · {:.2} · {:.2}", s.load[0], s.load[1], s.load[2]), &f.mono, 12.0, 0.0, ui.line);
    let sp = ui.spark(c, &app.cpu_h, 30.0, role_color(Role::Fill));
    c.entity(inner).add_children(&[dial, la, sp]);
    // memory
    let b = ui.body(c, ws.panes[MEM]);
    let h = ui.header(c, "Memory", &format!("{used:.1} of {:.0} GB", s.mem_total), ui.dim);
    let inner = c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(5), padding: UiRect { left: px(18), right: px(18), top: px(12), ..default() }, ..default() }).id();
    c.entity(b).add_children(&[h, inner]);
    for (k, v, col) in [("App", s.mem[0], role_color(Role::Fill)), ("Wired", s.mem[1], role_color(Role::Hot)), ("Compressed", s.mem[2], role_color(Role::Acc)), ("Cached", s.mem[3], ui.line), ("Free", s.mem[4], ui.dim)] {
        let row = ui.row(c, 10.0);
        let kt = c.spawn((Text::new(k.to_uppercase()), font(&f.ui600, 10.0, 2.0), TextColor(ui.dim), Node { width: px(96), ..default() })).id();
        let bar = ui.bar(c, v / s.mem_total.max(1.0), col, 200.0);
        let vt = ui.text(c, format!("{v:.1} GB"), &f.mono, 11.5, 0.0, ui.line);
        c.entity(row).add_children(&[kt, bar, vt]);
        c.entity(inner).add_child(row);
    }
    let sw = ui.kv(c, "swap", format!("{:.1} / {:.1} GB", s.swap.0, s.swap.1), ui.line);
    c.entity(inner).add_child(sw);
    // gpu
    let b = ui.body(c, ws.panes[GPU]);
    let h = ui.header(c, "GPU", "", ui.dim);
    let inner = c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(8), padding: UiRect { left: px(18), right: px(18), top: px(12), ..default() }, ..default() }).id();
    c.entity(b).add_children(&[h, inner]);
    match s.gpu {
        Some(g) => {
            let row = ui.row(c, 12.0);
            let u = ui.text(c, format!("{:.0}%", g * 100.0), &f.display, 22.0, 0.5, ui.hot);
            let ul = ui.text(c, "UTILISATION", &f.ui600, 9.5, 1.8, ui.dim);
            c.entity(row).add_children(&[u, ul]);
            let sp = ui.spark(c, &app.gpu_h, 60.0, ui.line);
            c.entity(inner).add_children(&[row, sp]);
        }
        None => {
            let t = ui.text(c, "This machine doesn't report GPU use", &f.ui500, 12.5, 0.4, ui.dim);
            c.entity(inner).add_child(t);
        }
    }
    // network
    let b = ui.body(c, ws.panes[NET]);
    let h = ui.header(c, "Network", "en*", ui.dim);
    let inner = c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(6), padding: UiRect { left: px(18), right: px(18), top: px(10), ..default() }, ..default() }).id();
    c.entity(b).add_children(&[h, inner]);
    let peak = app.in_h.iter().chain(app.out_h.iter()).cloned().fold(1e5, f32::max);
    let din: Vec<f32> = app.in_h.iter().map(|v| v / peak).collect();
    let dout: Vec<f32> = app.out_h.iter().map(|v| v / peak).collect();
    let a = ui.spark(c, &din, 56.0, role_color(Role::Fill));
    let bb = ui.spark(c, &dout, 30.0, role_color(Role::Acc));
    let row = ui.row(c, 18.0);
    let d = ui.text(c, format!("↓ {}", rate_or_dash(app.net_rate.map(|r| r.0))), &f.mono, 12.0, 0.0, ui.hot);
    let up = ui.text(c, format!("↑ {}", rate_or_dash(app.net_rate.map(|r| r.1))), &f.mono, 12.0, 0.0, ui.line);
    c.entity(row).add_children(&[d, up]);
    c.entity(inner).add_children(&[a, bb, row]);
    // disk and battery
    let b = ui.body(c, ws.panes[DISK]);
    let h = ui.header(c, "Disk · battery", "", ui.dim);
    let inner = c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(5), padding: UiRect { left: px(18), right: px(18), top: px(10), ..default() }, ..default() }).id();
    c.entity(b).add_children(&[h, inner]);
    let dk = ui.kv(c, "startup disk", format!("{:.0} of {:.0} GB", s.disk.0, s.disk.1), ui.line);
    let db = ui.bar(c, s.disk.0 / s.disk.1.max(1.0), role_color(Role::Fill), 340.0);
    let io = ui.kv(c, "disk i/o", format!("read {} · write {}", rate_or_dash(app.disk_rw.map(|r| r.0)), rate_or_dash(app.disk_rw.map(|r| r.1))), ui.line);
    let peak = app.disk_h.iter().cloned().fold(1e6, f32::max);
    let dh: Vec<f32> = app.disk_h.iter().map(|v| v / peak).collect();
    let dsp = ui.spark(c, &dh, 22.0, ui.line);
    c.entity(inner).add_children(&[dk, db, io, dsp]);
    match &s.battery {
        Some((pct, state, detail)) => {
            let bk = ui.kv(c, "battery", format!("{pct}% · {state}"), ui.line);
            let bbar = ui.bar(c, *pct as f32 / 100.0, if *pct < 20 { role_color(Role::Bad) } else { role_color(Role::Ok) }, 340.0);
            let bl = ui.kv(c, if detail.contains("remaining") || detail == "calculating" { "remaining" } else { "status" }, clip(detail.trim_end_matches(" remaining"), 30), ui.line);
            c.entity(inner).add_children(&[bk, bbar, bl]);
        }
        None => {
            let t = ui.kv(c, "battery", if s.power.is_empty() { "none".to_string() } else { format!("none · {}", s.power) }, ui.dim);
            c.entity(inner).add_child(t);
        }
    }
    // processes
    let b = ui.body(c, ws.panes[PROCS]);
    let procs = app.procs();
    let sel = app.sel(&procs);
    let first = sel.saturating_sub(ROWS / 2).min(procs.len().saturating_sub(ROWS));
    let range = if procs.is_empty() { String::new() } else { format!(" · {}–{} of {}", first + 1, (first + ROWS).min(procs.len()), procs.len()) };
    let h = ui.header(c, "Processes", &format!("{}{range}", if app.by_mem { "by memory · S" } else { "by CPU · S" }), ui.dim);
    let (up, down) = (crate::containers::chip(c, &ui, "▲", KeyCode::ArrowUp), crate::containers::chip(c, &ui, "▼", KeyCode::ArrowDown));
    c.entity(h).insert(Click::Key(KeyCode::KeyS)).add_children(&[up, down]);
    let list = c.spawn(Node { flex_direction: FlexDirection::Column, padding: UiRect { left: px(14), right: px(14), top: px(4), ..default() }, ..default() }).id();
    c.entity(b).add_children(&[h, list]);
    if procs.is_empty() {
        let t = ui.text(c, "No process list (ps gave nothing)", &f.ui500, 12.5, 0.4, ui.dim);
        c.entity(list).add_child(t);
    }
    for (i, p) in procs.iter().enumerate().skip(first).take(ROWS) {
        let on = i == sel;
        let armed = app.armed.is_some_and(|(_, pid)| pid == p.pid);
        let row = c.spawn((Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, height: px(20), column_gap: px(14), padding: UiRect::left(px(6)), border: UiRect::left(px(if on { 2.0 } else { 0.0 })), overflow: Overflow::clip(), ..default() }, BorderColor::all(ui.hot), BackgroundColor(if on { with_alpha(ui.line, 0.1) } else { Color::NONE }), Click::Pick(i))).id();
        let nm = c.spawn((Text::new(clip(&p.name, 40)), font(&f.mono, 12.0, 0.0), TextColor(if on { ui.hot } else { ui.line }), Node { width: px(380), flex_shrink: 0.0, overflow: Overflow::clip(), ..default() })).id();
        let pid = c.spawn((Text::new(p.pid.to_string()), font(&f.mono, 11.5, 0.0), TextColor(ui.dim), Node { width: px(80), flex_shrink: 0.0, ..default() })).id();
        let bar = ui.bar(c, p.cpu / 100.0, role_color(Role::Fill), 220.0);
        let cpu = c.spawn((Text::new(format!("{:.0}%", p.cpu)), font(&f.mono, 11.5, 0.0), TextColor(ui.line), Node { width: px(70), flex_shrink: 0.0, ..default() })).id();
        let mem = ui.text(c, if p.rss_mb > 1024.0 { format!("{:.1} GB", p.rss_mb / 1024.0) } else { format!("{:.0} MB", p.rss_mb) }, &f.mono, 11.5, 0.0, ui.line);
        let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
        let k = ui.text(c, if on && armed { "PRESS K AGAIN" } else if on { "K STOP" } else { "" }, &f.ui700, 9.5, 1.6, role_color(Role::Bad));
        if on {
            c.entity(k).insert(Click::Key(KeyCode::KeyK));
        }
        c.entity(row).add_children(&[nm, pid, bar, cpu, mem, sp, k]);
        c.entity(list).add_child(row);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wheel_keys_per_pane() {
        assert_eq!(scroll_key(Some(PROCS as u8), true), Some(KeyCode::ArrowDown));
        assert_eq!(scroll_key(Some(PROCS as u8), false), Some(KeyCode::ArrowUp));
        assert_eq!(scroll_key(Some(CPU as u8), true), None);
    }
    use crate::opsdata::Proc;

    fn sys(procs: Vec<Proc>, ticks: Vec<(u64, u64)>, bytes: (u64, u64), at: Instant) -> Sys {
        Sys { procs, cores_ticks: ticks, disk_bytes: bytes, at: Some(at), ..Default::default() }
    }

    fn p(pid: u32, cpu: f32, rss: f32) -> Proc {
        Proc { pid, cpu, rss_mb: rss, name: format!("p{pid}") }
    }

    #[test]
    fn the_first_sample_shows_no_rates() {
        let mut a = SysApp::default();
        let t0 = Instant::now();
        a.apply(sys(vec![], vec![(10, 100), (0, 100)], (1000, 1000), t0), Some(((100, 100), t0)));
        assert!(a.cpu.is_none() && a.cores.is_none() && a.disk_rw.is_none() && a.net_rate.is_none());
        let t1 = t0 + std::time::Duration::from_secs(2);
        let mut s = sys(vec![], vec![(60, 200), (0, 200)], (1000 + 2_000_000, 1000 + 4_000_000), t1);
        s.net_in = 5000.0;
        a.apply(s, Some(((100, 100), t1)));
        assert_eq!(a.cores, Some(vec![0.5, 0.0]));
        assert!((a.cpu.unwrap() - 0.25).abs() < 1e-6);
        let (r, w) = a.disk_rw.unwrap();
        assert!((r - 1_000_000.0).abs() < 1.0 && (w - 2_000_000.0).abs() < 1.0);
        // the network needs the previous byte totals, which the first sample only supplied
        assert_eq!(a.net_rate, Some((5000.0, 0.0)));
    }

    #[test]
    fn reopening_forgets_the_old_readings() {
        let mut a = SysApp::default();
        let t0 = Instant::now();
        a.apply(sys(vec![], vec![(10, 100)], (0, 0), t0), Some(((1, 1), t0)));
        a.open();
        a.apply(sys(vec![], vec![(500, 1000)], (9, 9), t0), Some(((2, 2), t0)));
        assert!(a.cpu.is_none() && a.disk_rw.is_none() && a.net_rate.is_none());
    }

    #[test]
    fn selection_follows_the_process_when_the_order_changes() {
        let mut a = SysApp::default();
        let t = Instant::now();
        a.apply(sys(vec![p(1, 90.0, 10.0), p(2, 50.0, 500.0), p(3, 5.0, 20.0)], vec![], (0, 0), t), None);
        let procs = a.procs();
        a.select(&procs, 1);
        assert_eq!(a.sel_pid, Some(2));
        // sorted by memory, process 2 is now first
        a.by_mem = true;
        let procs = a.procs();
        assert_eq!((procs[0].pid, a.sel(&procs)), (2, 0));
        // process 2 exits: back to the first row
        a.apply(sys(vec![p(1, 90.0, 10.0), p(3, 5.0, 20.0)], vec![], (0, 0), t), None);
        let procs = a.procs();
        assert_eq!(a.sel(&procs), 0);
        assert_eq!(a.procs().len(), 2);
    }

    #[test]
    fn names_cores_and_rates() {
        // Apple silicon lists the efficiency cores first
        let l: Vec<String> = (0..6).map(|i| core_label(i, 6, 4, 2)).collect();
        assert_eq!(l, vec!["E0", "E1", "P0", "P1", "P2", "P3"]);
        assert_eq!(core_label(3, 8, 0, 0), "3");
        assert_eq!((rate(512_000.0), rate(2_500_000.0), rate_or_dash(None)), ("512 kB/s".to_string(), "2.5 MB/s".to_string(), "-".to_string()));
        assert_eq!((pct_or_dash(Some(0.256)), pct_or_dash(None)), ("26%".to_string(), "...".to_string()));
    }
}
