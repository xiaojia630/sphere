//! The System app (design/apps/system): this machine's processor, memory, GPU, network, disk and battery, sampled
//! every two seconds while it's open, and the processes using the most. Keys: J K select a process, S sort by CPU
//! or memory, K stop it (asks twice), Esc closes.
use crate::appframe::*;
use crate::core::CoreCtl;
use crate::fonts::UiFonts;
use crate::opsdata::{self, Sys};
use crate::panel::*;
use crate::service::Service;
use crate::util::*;
use bevy::prelude::*;
use std::sync::{mpsc, Mutex};

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

type Sample = (Sys, Option<((u64, u64), std::time::Instant)>);
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
    /// each core's use 0..1, from the last two tick readings
    cores: Vec<f32>,
    ticks_prev: Vec<(u64, u64)>,
    /// disk bytes (read, written) last time, when, and the rates since
    disk_prev: Option<((u64, u64), f32)>,
    disk_rw: (f32, f32),
    disk_h: Vec<f32>,
    net_prev: Option<((u64, u64), std::time::Instant)>,
    loading: bool,
    load_t: f32,
    rx: Option<Mutex<mpsc::Receiver<Sample>>>,
    sel: usize,
    by_mem: bool,
    armed: Option<f32>,
    notes: (mpsc::Sender<Note>, Mutex<mpsc::Receiver<Note>>),
    version: u64,
}

impl Default for SysApp {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        SysApp { open: false, amt: Spring::new(0.0, 26.0, 9.0), sys: None, cpu_h: Vec::new(), gpu_h: Vec::new(), in_h: Vec::new(), out_h: Vec::new(), cores: Vec::new(), ticks_prev: Vec::new(), disk_prev: None, disk_rw: (0.0, 0.0), disk_h: Vec::new(), net_prev: None, loading: false, load_t: -99.0, rx: None, sel: 0, by_mem: false, armed: None, notes: (tx, Mutex::new(rx)), version: 0 }
    }
}

impl SysApp {
    pub fn open(&mut self) {
        self.open = true;
        self.load_t = -99.0;
        self.version += 1;
    }
    pub fn close(&mut self) {
        self.open = false;
        self.armed = None;
        self.version += 1;
    }
    fn procs(&self) -> Vec<opsdata::Proc> {
        let mut v = self.sys.as_ref().map(|s| s.procs.clone()).unwrap_or_default();
        if self.by_mem {
            v.sort_by(|a, b| b.rss_mb.total_cmp(&a.rss_mb));
        }
        v.truncate(6);
        v
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

pub fn setup(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut atlas: ResMut<PanelAtlas>, mut pmats: ResMut<Assets<PanelMaterial>>, args: Res<crate::Args>, mut app: ResMut<SysApp>) {
    if args.open_app.as_deref() == Some("system") {
        app.open();
    }
    let ws = spawn_workspace(&mut commands, &mut meshes, &mut atlas, &mut pmats, APP, &PANES);
    commands.insert_resource(SysRoots(ws));
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
    mut clicks: ResMut<crate::appframe::AppClicks>,
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
        if e.state != bevy::input::ButtonState::Pressed || mods || bar_busy || focus.terminal || moment.shown || e.repeat {
            continue;
        }
        codes.push(e.key_code);
    }
    // clicks on the panes: a button presses its key, a row picks its item
    for c in clicks.take(4) {
        match c {
            Click::Key(k) => codes.push(k),
            Click::Pick(i) => {
                app.sel = i;
                app.armed = None;
                app.version += 1;
            }
            Click::Alt(_) => {}
        }
    }
    for key in codes {
        let procs = app.procs();
        match key {
            KeyCode::Escape => {
                app.close();
                sound.play_with("collapse", 0.7, 0.0);
            }
            KeyCode::KeyJ | KeyCode::ArrowDown if !procs.is_empty() => {
                app.sel = (app.sel + 1).min(procs.len() - 1);
                app.armed = None;
                app.version += 1;
                sound.play_q("tick", 40);
            }
            KeyCode::ArrowUp => {
                app.sel = app.sel.saturating_sub(1);
                app.armed = None;
                app.version += 1;
                sound.play_q("tick", 40);
            }
            KeyCode::KeyS => {
                app.by_mem = !app.by_mem;
                app.sel = 0;
                app.version += 1;
                sound.play("toggle");
            }
            KeyCode::KeyK => {
                if let Some(p) = procs.get(app.sel) {
                    if app.armed.is_some_and(|t| sim.t < t) {
                        app.armed = None;
                        let _ = app.notes.0.send(opsdata::terminate(p.pid));
                        sound.play("discard");
                    } else {
                        app.armed = Some(sim.t + 3.0);
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

pub fn update(sim: Res<crate::core::Sim>, mut app: ResMut<SysApp>, mut ctl: ResMut<CoreCtl>, mut toasts: ResMut<crate::live::Toasts>, mut sound: ResMut<crate::sound::Sound>) {
    let open = app.open;
    step_open(open, &mut app.amt, sim.dt, &mut ctl);
    if app.armed.is_some_and(|t| sim.t > t) {
        app.armed = None;
        app.version += 1;
    }
    let notes: Vec<Note> = app.notes.1.lock().map(|r| r.try_iter().collect()).unwrap_or_default();
    for n in notes {
        match n {
            Ok(s) => toasts.push_quiet(s),
            Err(e) => {
                toasts.push(e);
                sound.play("error");
            }
        }
        app.load_t = -99.0;
    }
    let got = app.rx.as_ref().and_then(|r| r.lock().ok()?.try_recv().ok());
    if let Some((s, prev)) = got {
        app.loading = false;
        let had_net = app.net_prev.is_some();
        app.net_prev = prev;
        push(&mut app.cpu_h, s.cpu);
        if !app.ticks_prev.is_empty() {
            app.cores = crate::opsdata::core_use(&app.ticks_prev, &s.cores_ticks);
        }
        app.ticks_prev = s.cores_ticks.clone();
        if let Some(((r0, w0), t0)) = app.disk_prev {
            let dt = (sim.t - t0).max(0.5);
            app.disk_rw = (s.disk_bytes.0.saturating_sub(r0) as f32 / dt, s.disk_bytes.1.saturating_sub(w0) as f32 / dt);
            let rw = app.disk_rw.0 + app.disk_rw.1;
            push(&mut app.disk_h, rw);
        }
        app.disk_prev = Some((s.disk_bytes, sim.t));
        push(&mut app.gpu_h, s.gpu);
        if had_net {
            push(&mut app.in_h, s.net_in);
            push(&mut app.out_h, s.net_out);
        }
        app.sys = Some(s);
        app.version += 1;
    }
    if !app.open || app.loading || sim.t - app.load_t < 2.0 {
        return;
    }
    app.load_t = sim.t;
    app.loading = true;
    let prev = app.net_prev;
    let (tx, rx) = mpsc::channel();
    app.rx = Some(Mutex::new(rx));
    std::thread::spawn(move || {
        let _ = tx.send(opsdata::sample(prev));
    });
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

pub fn draw(mut commands: Commands, app: Res<SysApp>, svc: Res<Service>, roots: Option<Res<SysRoots>>, f: Res<UiFonts>, theme: Res<crate::theme::Theme>, mut seen: Local<(u64, u64, u64)>) {
    let Some(roots) = roots else { return };
    if app.amt.x < 0.001 && !app.open {
        return;
    }
    let (agents, sver) = {
        let s = svc.shared.lock().unwrap();
        let mut v = s.tasks.clone();
        v.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        (v, s.version)
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
    ui.corelab(c, ws.corelab, "System", format!("cpu {:.0}% · memory {:.0}%", s.cpu * 100.0, 100.0 * (s.mem[0] + s.mem[1] + s.mem[2]) / s.mem_total.max(1.0)), ui.line);
    ui.top(c, ws.panes[TOP], "System", s.chip.to_uppercase(), "sampled from ps, vm_stat, sysctl, netstat, ioreg, pmset");
    ui.strip(c, ws.panes[STRIP], &agents, ["J K SELECT   S SORT CPU / MEMORY", "K STOP PROCESS   ESC CLOSE"]);
    // processor
    let b = ui.body(c, ws.panes[CPU]);
    let h = ui.header(c, "Processor", &format!("{} P + {} E cores", s.cores.0, s.cores.1), ui.dim);
    let inner = c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(8), padding: UiRect { left: px(18), right: px(18), top: px(8), ..default() }, align_items: AlignItems::Center, ..default() }).id();
    c.entity(b).add_children(&[h, inner]);
    // one spoke per core round the total, longer the busier (P cores first, then E)
    let dial = c.spawn(Node { width: px(380), height: px(250), ..default() }).id();
    let (cx, cy) = (190.0, 122.0);
    let n = app.cores.len().max(1);
    for (i, u) in app.cores.iter().enumerate() {
        let a = std::f32::consts::TAU * i as f32 / n as f32 - std::f32::consts::FRAC_PI_2;
        let dir = Vec2::new(a.cos(), a.sin());
        let r0 = 66.0;
        let col = if *u > 0.85 { role_color(Role::Bad) } else if *u > 0.5 { role_color(Role::Acc) } else { role_color(Role::Fill) };
        ui.line(c, dial, Vec2::new(cx, cy) + dir * r0, Vec2::new(cx, cy) + dir * (r0 + 52.0), 3.0, with_alpha(ui.line, 0.12));
        ui.line(c, dial, Vec2::new(cx, cy) + dir * r0, Vec2::new(cx, cy) + dir * (r0 + 4.0 + 48.0 * u), 3.0, col);
        let lp = Vec2::new(cx, cy) + dir * 134.0;
        let p = i < s.cores.0 as usize;
        let k = if p { format!("P{i}") } else { format!("E{}", i - s.cores.0 as usize) };
        let lab = c.spawn(Node { position_type: PositionType::Absolute, left: px(lp.x - 16.0), top: px(lp.y - 12.0), width: px(32), flex_direction: FlexDirection::Column, align_items: AlignItems::Center, ..default() }).id();
        let l1 = ui.text(c, k, &f.ui600, 8.5, 1.0, ui.dim);
        let l2 = ui.text(c, format!("{:.0}", u * 100.0), &f.mono, 10.0, 0.0, ui.line);
        c.entity(lab).add_children(&[l1, l2]);
        c.entity(dial).add_child(lab);
    }
    let ring = c
        .spawn((
            Node { position_type: PositionType::Absolute, left: px(cx - 56.0), top: px(cy - 56.0), width: px(112), height: px(112), border: UiRect::all(px(2)), border_radius: BorderRadius::MAX, flex_direction: FlexDirection::Column, align_items: AlignItems::Center, justify_content: JustifyContent::Center, ..default() },
            BorderColor::all(with_alpha(if s.cpu > 0.85 { role_color(Role::Bad) } else { role_color(Role::Fill) }, 0.35 + 0.65 * s.cpu)),
        ))
        .id();
    let pct = ui.text(c, format!("{:.0}%", s.cpu * 100.0), &f.display, 24.0, 0.5, ui.hot);
    let cl = ui.text(c, "CPU", &f.ui600, 9.0, 2.4, ui.dim);
    c.entity(ring).add_children(&[pct, cl]);
    c.entity(dial).add_child(ring);
    let la = ui.text(c, format!("load {:.2} · {:.2} · {:.2}", s.load[0], s.load[1], s.load[2]), &f.mono, 12.0, 0.0, ui.line);
    let sp = ui.spark(c, &app.cpu_h, 46.0, role_color(Role::Fill));
    c.entity(inner).add_children(&[dial, la, sp]);
    // memory
    let b = ui.body(c, ws.panes[MEM]);
    // used the way Activity Monitor counts it: app, wired and compressed (cached files can be dropped)
    let used = s.mem[0] + s.mem[1] + s.mem[2];
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
    let row = ui.row(c, 12.0);
    let u = ui.text(c, format!("{:.0}%", s.gpu * 100.0), &f.display, 22.0, 0.5, ui.hot);
    let ul = ui.text(c, "UTILISATION", &f.ui600, 9.5, 1.8, ui.dim);
    c.entity(row).add_children(&[u, ul]);
    let sp = ui.spark(c, &app.gpu_h, 60.0, ui.line);
    c.entity(inner).add_children(&[row, sp]);
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
    let d = ui.text(c, format!("DOWN {}", rate(s.net_in)), &f.mono, 12.0, 0.0, ui.hot);
    let up = ui.text(c, format!("UP {}", rate(s.net_out)), &f.mono, 12.0, 0.0, ui.line);
    c.entity(row).add_children(&[d, up]);
    c.entity(inner).add_children(&[a, bb, row]);
    // disk and battery
    let b = ui.body(c, ws.panes[DISK]);
    let h = ui.header(c, "Disk · battery", "", ui.dim);
    let inner = c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(5), padding: UiRect { left: px(18), right: px(18), top: px(10), ..default() }, ..default() }).id();
    c.entity(b).add_children(&[h, inner]);
    let dk = ui.kv(c, "macintosh hd", format!("{:.0} of {:.0} GB", s.disk.0, s.disk.1), ui.line);
    let db = ui.bar(c, s.disk.0 / s.disk.1.max(1.0), role_color(Role::Fill), 340.0);
    let io = ui.kv(c, "disk i/o", format!("read {} · write {}", rate(app.disk_rw.0), rate(app.disk_rw.1)), ui.line);
    let peak = app.disk_h.iter().cloned().fold(1e6, f32::max);
    let dh: Vec<f32> = app.disk_h.iter().map(|v| v / peak).collect();
    let dsp = ui.spark(c, &dh, 22.0, ui.line);
    c.entity(inner).add_children(&[dk, db, io, dsp]);
    match &s.battery {
        Some((pct, state, left)) => {
            let bk = ui.kv(c, "battery", format!("{pct}% · {state}"), ui.line);
            let bbar = ui.bar(c, *pct as f32 / 100.0, if *pct < 20 { role_color(Role::Bad) } else { role_color(Role::Ok) }, 340.0);
            let bl = ui.kv(c, "remaining", clip(left, 30), ui.line);
            c.entity(inner).add_children(&[bk, bbar, bl]);
        }
        None => {
            let t = ui.kv(c, "battery", "none".into(), ui.dim);
            c.entity(inner).add_child(t);
        }
    }
    // processes
    let b = ui.body(c, ws.panes[PROCS]);
    let h = ui.header(c, "Processes", if app.by_mem { "by memory · S" } else { "by CPU · S" }, ui.dim);
    c.entity(h).insert(Click::Key(KeyCode::KeyS));
    let list = c.spawn(Node { flex_direction: FlexDirection::Column, padding: UiRect { left: px(14), right: px(14), top: px(4), ..default() }, ..default() }).id();
    c.entity(b).add_children(&[h, list]);
    for (i, p) in app.procs().iter().enumerate() {
        let sel = i == app.sel;
        let row = c.spawn((Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, height: px(20), column_gap: px(14), padding: UiRect::left(px(6)), border: UiRect::left(px(if sel { 2.0 } else { 0.0 })), ..default() }, BorderColor::all(ui.hot), BackgroundColor(if sel { with_alpha(ui.line, 0.1) } else { Color::NONE }), Click::Pick(i))).id();
        let nm = c.spawn((Text::new(clip(&p.name, 40)), font(&f.mono, 12.0, 0.0), TextColor(if sel { ui.hot } else { ui.line }), Node { width: px(380), overflow: Overflow::clip(), ..default() })).id();
        let pid = c.spawn((Text::new(p.pid.to_string()), font(&f.mono, 11.5, 0.0), TextColor(ui.dim), Node { width: px(80), ..default() })).id();
        let bar = ui.bar(c, p.cpu / 200.0, role_color(Role::Fill), 220.0);
        let cpu = c.spawn((Text::new(format!("{:.0}%", p.cpu)), font(&f.mono, 11.5, 0.0), TextColor(ui.line), Node { width: px(70), ..default() })).id();
        let mem = ui.text(c, if p.rss_mb > 1024.0 { format!("{:.1} GB", p.rss_mb / 1024.0) } else { format!("{:.0} MB", p.rss_mb) }, &f.mono, 11.5, 0.0, ui.line);
        let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
        let k = ui.text(c, if sel && app.armed.is_some() { "PRESS K AGAIN" } else if sel { "K STOP" } else { "" }, &f.ui700, 9.5, 1.6, role_color(Role::Bad));
        if sel {
            c.entity(k).insert(Click::Key(KeyCode::KeyK));
        }
        c.entity(row).add_children(&[nm, pid, bar, cpu, mem, sp, k]);
        c.entity(list).add_child(row);
    }
}
