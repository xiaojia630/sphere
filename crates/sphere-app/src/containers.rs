//! The Containers app (design/apps/containers): your Docker or OrbStack containers around a hub, grouped by
//! compose project, with a detail pane and the selected one's logs. Keys: ← → (or J K) select, S start or stop,
//! R restart, P pull its image, D remove (asks twice), F follow logs, Esc closes.
use crate::appframe::*;
use crate::core::CoreCtl;
use crate::fonts::UiFonts;
use crate::opsdata::{self, Container, DockerOp};
use crate::panel::*;
use crate::service::Service;
use crate::util::*;
use bevy::prelude::*;
use std::collections::HashMap;
use std::sync::{mpsc, Mutex};

pub const APP: u8 = 2;
const TOP: usize = 0;
const SUM: usize = 1;
const MAP: usize = 2;
const DETAIL: usize = 3;
const LOGS: usize = 4;
const STRIP: usize = 5;
const PANES: [([f32; 5], bool); 6] = [
    ([22.0, 2.0, 1236.0, 32.0, 8.0], true),
    ([14.0, 42.0, 1252.0, 40.0, 12.0], false),
    ([14.0, 94.0, 860.0, 330.0, 16.0], false),
    ([886.0, 94.0, 380.0, 330.0, 44.0], false),
    ([14.0, 436.0, 1252.0, 190.0, 30.0], false),
    ([14.0, 638.0, 1252.0, 50.0, 22.0], false),
];
/// Pills that fit the map.
const MAX_PILLS: usize = 10;

type Note = Result<String, String>;
type Listing = Result<Vec<Container>, String>;

#[derive(Resource)]
pub struct ContainersApp {
    pub open: bool,
    pub amt: Spring,
    list: Option<Result<Vec<Container>, String>>,
    /// recent CPU % per container, newest last
    hist: HashMap<String, Vec<f32>>,
    sel: usize,
    logs: Vec<String>,
    follow: bool,
    loading: bool,
    load_t: f32,
    logs_t: f32,
    rx: Option<Mutex<mpsc::Receiver<Listing>>>,
    logs_rx: Option<Mutex<mpsc::Receiver<Vec<String>>>>,
    armed: Option<f32>,
    want: Option<String>,
    notes: (mpsc::Sender<Note>, Mutex<mpsc::Receiver<Note>>),
    version: u64,
}

impl Default for ContainersApp {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        ContainersApp { open: false, amt: Spring::new(0.0, 26.0, 9.0), list: None, hist: HashMap::new(), sel: 0, logs: Vec::new(), follow: true, loading: false, load_t: -99.0, logs_t: -99.0, rx: None, logs_rx: None, armed: None, want: None, notes: (tx, Mutex::new(rx)), version: 0 }
    }
}

impl ContainersApp {
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
    /// Selects a container by name once the list is in.
    pub fn focus(&mut self, name: String) {
        self.want = Some(name);
    }
    fn shown(&self) -> &[Container] {
        match &self.list {
            Some(Ok(v)) => &v[..v.len().min(MAX_PILLS)],
            _ => &[],
        }
    }
    fn selected(&self) -> Option<&Container> {
        self.shown().get(self.sel)
    }
    fn act(&mut self, op: DockerOp) {
        let tx = self.notes.0.clone();
        std::thread::spawn(move || {
            let _ = tx.send(opsdata::docker(op));
        });
    }
}

#[derive(Resource)]
pub struct ContainersRoots(Workspace);

pub fn setup(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut atlas: ResMut<PanelAtlas>, mut pmats: ResMut<Assets<PanelMaterial>>, args: Res<crate::Args>, mut app: ResMut<ContainersApp>) {
    if args.open_app.as_deref() == Some("containers") {
        app.open();
    }
    let ws = spawn_workspace(&mut commands, &mut meshes, &mut atlas, &mut pmats, APP, &PANES);
    commands.insert_resource(ContainersRoots(ws));
}

#[allow(clippy::too_many_arguments)]
pub fn keys(
    mut ev: MessageReader<bevy::input::keyboard::KeyboardInput>,
    keys: Res<ButtonInput<KeyCode>>,
    (focus, moment): (Res<Focus>, Res<crate::moment::Moment>),
    mut app: ResMut<ContainersApp>,
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
    for c in clicks.take(2) {
        match c {
            Click::Key(k) => codes.push(k),
            Click::Pick(i) => {
                app.sel = i;
                app.logs_t = -99.0;
                app.armed = None;
                app.version += 1;
            }
            Click::Alt(_) => {}
        }
    }
    for key in codes {
        let n = app.shown().len();
        let sel = app.selected().cloned();
        match key {
            KeyCode::Escape => {
                app.close();
                sound.play_with("collapse", 0.7, 0.0);
            }
            KeyCode::ArrowRight | KeyCode::KeyJ | KeyCode::ArrowDown if n > 0 => {
                app.sel = (app.sel + 1) % n;
                app.logs_t = -99.0;
                app.armed = None;
                app.version += 1;
                sound.play_q("tick", 40);
            }
            KeyCode::ArrowLeft | KeyCode::KeyK | KeyCode::ArrowUp if n > 0 => {
                app.sel = (app.sel + n - 1) % n;
                app.logs_t = -99.0;
                app.armed = None;
                app.version += 1;
                sound.play_q("tick", 40);
            }
            KeyCode::KeyF => {
                app.follow = !app.follow;
                app.version += 1;
                sound.play("toggle");
            }
            KeyCode::KeyS => {
                if let Some(c) = sel {
                    let running = c.state == "running";
                    toasts.push_quiet(format!("{} {}…", if running { "Stopping" } else { "Starting" }, c.name));
                    app.act(if running { DockerOp::Stop(c.name) } else { DockerOp::Start(c.name) });
                    sound.play(if running { "discard" } else { "confirm" });
                }
            }
            KeyCode::KeyR => {
                if let Some(c) = sel {
                    toasts.push_quiet(format!("Restarting {}…", c.name));
                    app.act(DockerOp::Restart(c.name));
                    sound.play("progress");
                }
            }
            KeyCode::KeyP => {
                if let Some(c) = sel {
                    toasts.push_quiet(format!("Pulling {}…", c.image));
                    app.act(DockerOp::Pull(c.image));
                    sound.play("progress");
                }
            }
            KeyCode::KeyD => {
                if let Some(c) = sel {
                    if app.armed.is_some_and(|t| sim.t < t) {
                        app.armed = None;
                        app.act(DockerOp::Remove(c.name));
                        sound.play("discard");
                    } else {
                        app.armed = Some(sim.t + 3.0);
                        toasts.push_quiet(format!("Press D again to remove {}", c.name));
                        sound.play("toggle");
                    }
                    app.version += 1;
                }
            }
            _ => {}
        }
    }
}

pub fn update(sim: Res<crate::core::Sim>, mut app: ResMut<ContainersApp>, mut ctl: ResMut<CoreCtl>, mut toasts: ResMut<crate::live::Toasts>, mut sound: ResMut<crate::sound::Sound>) {
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
        app.logs_t = -99.0;
    }
    let got = app.rx.as_ref().and_then(|r| r.lock().ok()?.try_recv().ok());
    if let Some(list) = got {
        app.loading = false;
        if let Ok(v) = &list {
            for c in v {
                let h = app.hist.entry(c.id.clone()).or_default();
                h.push(c.cpu);
                if h.len() > 24 {
                    h.remove(0);
                }
            }
        }
        app.list = Some(list);
        if let Some(w) = app.want.take() {
            if let Some(i) = app.shown().iter().position(|c| c.name == w) {
                app.sel = i;
                app.logs_t = -99.0;
            }
        }
        let n = app.shown().len();
        if app.sel >= n {
            app.sel = 0;
        }
        app.version += 1;
    }
    let got = app.logs_rx.as_ref().and_then(|r| r.lock().ok()?.try_recv().ok());
    if let Some(l) = got {
        if l != app.logs {
            app.logs = l;
            app.version += 1;
        }
    }
    if !app.open {
        return;
    }
    if !app.loading && sim.t - app.load_t > 4.0 {
        app.load_t = sim.t;
        app.loading = true;
        let (tx, rx) = mpsc::channel();
        app.rx = Some(Mutex::new(rx));
        std::thread::spawn(move || {
            let _ = tx.send(opsdata::containers());
        });
    }
    let wait = if app.follow { 2.0 } else { 1e9 };
    if let Some(id) = app.selected().map(|c| c.id.clone()) {
        if sim.t - app.logs_t > wait {
            app.logs_t = sim.t;
            let (tx, rx) = mpsc::channel();
            app.logs_rx = Some(Mutex::new(rx));
            std::thread::spawn(move || {
                let _ = tx.send(opsdata::logs(&id, 9));
            });
        }
    }
}

pub fn place(app: Res<ContainersApp>, mut q: Query<(&mut Panel, &AppPart)>) {
    place_app(APP, app.amt.x, |_| false, &mut q);
}

fn state_col(c: &Container) -> Color {
    if c.unhealthy() {
        role_color(Role::Bad)
    } else {
        match c.state.as_str() {
            "running" => role_color(Role::Ok),
            "restarting" => role_color(Role::Acc),
            _ => role_color(Role::Dim),
        }
    }
}

fn state_word(c: &Container) -> String {
    if c.unhealthy() {
        "UNHEALTHY".into()
    } else {
        c.state.to_uppercase()
    }
}

pub fn draw(mut commands: Commands, app: Res<ContainersApp>, svc: Res<Service>, roots: Option<Res<ContainersRoots>>, f: Res<UiFonts>, theme: Res<crate::theme::Theme>, mut seen: Local<(u64, u64, u64)>) {
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
    let all: &[Container] = match &app.list {
        Some(Ok(v)) => v,
        _ => &[],
    };
    let up = all.iter().filter(|c| c.state == "running").count();
    let bad = all.iter().filter(|c| c.unhealthy()).count();
    let (l2, l2c) = if bad > 0 { (format!("{bad} unhealthy"), role_color(Role::Bad)) } else { (format!("{up} of {} running", all.len()), ui.line) };
    ui.corelab(c, ws.corelab, "Containers", l2, l2c);
    ui.top(c, ws.panes[TOP], "Containers", format!("{} CONTAINERS · {up} UP", all.len()), "container model and actions after oxker (MIT) · docker");
    ui.strip(c, ws.panes[STRIP], &agents, ["← → SELECT   S START / STOP   R RESTART   P PULL IMAGE", "D REMOVE   F FOLLOW LOGS   ESC CLOSE"]);
    // summary
    let r = c.spawn(Node { width: percent(100), height: percent(100), flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(22), padding: UiRect::horizontal(px(16)), ..default() }).id();
    c.entity(ws.panes[SUM]).add_child(r);
    let cpu: f32 = all.iter().map(|c| c.cpu).sum::<f32>().max(0.0);
    let mem_mb: f32 = all.iter().filter_map(|c| c.mem.split(" / ").next()).map(mib).sum::<f32>().max(0.0);
    for (v, k) in [(format!("{cpu:.1}%"), "cpu"), (format!("{mem_mb:.0} MB"), "memory"), (format!("{up} of {}", all.len()), "running")] {
        let row = ui.row(c, 6.0);
        let a = ui.text(c, v, &f.mono, 13.0, 0.0, ui.hot);
        let b = ui.text(c, k, &f.ui500, 11.5, 0.6, ui.dim);
        c.entity(row).add_children(&[a, b]);
        c.entity(r).add_child(row);
    }
    let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
    c.entity(r).add_child(sp);
    for (k, col) in [("RUNNING", role_color(Role::Ok)), ("STOPPED", ui.dim), ("RESTARTING", role_color(Role::Acc)), ("UNHEALTHY", role_color(Role::Bad))] {
        let row = ui.row(c, 6.0);
        let d = c.spawn((Node { width: px(7), height: px(7), border_radius: BorderRadius::MAX, ..default() }, BackgroundColor(col))).id();
        let t = ui.text(c, k, &f.ui600, 9.5, 1.8, ui.dim);
        c.entity(row).add_children(&[d, t]);
        c.entity(r).add_child(row);
    }
    // the map: a hub, the containers on an ellipse round it
    let map = c.spawn(Node { width: percent(100), height: percent(100), ..default() }).id();
    c.entity(ws.panes[MAP]).add_child(map);
    match &app.list {
        None => {
            let t = c.spawn((Text::new("Asking Docker…"), font(&f.ui500, 13.0, 0.5), TextColor(ui.line), Node { margin: UiRect::all(px(18)), ..default() })).id();
            c.entity(map).add_child(t);
        }
        Some(Err(e)) => {
            let t = c.spawn((Text::new(format!("Docker isn't answering: {}", clip(e, 70))), font(&f.ui500, 13.0, 0.5), TextColor(role_color(Role::Bad)), Node { margin: UiRect::all(px(18)), ..default() })).id();
            c.entity(map).add_child(t);
        }
        Some(Ok(_)) => draw_map(c, &ui, map, &app),
    }
    // detail and logs
    match app.selected() {
        Some(sel) => {
            draw_detail(c, &ui, ws.panes[DETAIL], sel, app.hist.get(&sel.id).map(|v| v.as_slice()).unwrap_or(&[]), app.armed.is_some());
            draw_logs(c, &ui, ws.panes[LOGS], sel, &app.logs, app.follow);
        }
        None => {
            let b = ui.body(c, ws.panes[DETAIL]);
            let h = ui.header(c, "Container", "", ui.dim);
            c.entity(b).add_child(h);
            let b = ui.body(c, ws.panes[LOGS]);
            let h = ui.header(c, "Logs", "", ui.dim);
            c.entity(b).add_child(h);
        }
    }
}

/// "126.4MiB" → MB
fn mib(s: &str) -> f32 {
    let s = s.trim();
    let num: String = s.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    let v: f32 = num.parse().unwrap_or(0.0);
    if s.contains("GiB") {
        v * 1073.74
    } else if s.contains("KiB") || s.contains("kB") {
        v / 1000.0
    } else {
        v * 1.048_576
    }
}

fn draw_map(c: &mut Commands, ui: &Ui, map: Entity, app: &ContainersApp) {
    let shown = app.shown();
    let (cx, cy, rx, ry) = (430.0, 165.0, 300.0, 112.0);
    // the hub
    let hub = c
        .spawn((
            Node { position_type: PositionType::Absolute, left: px(cx - 42.0), top: px(cy - 42.0), width: px(84), height: px(84), border: UiRect::all(px(1)), border_radius: BorderRadius::MAX, flex_direction: FlexDirection::Column, align_items: AlignItems::Center, justify_content: JustifyContent::Center, ..default() },
            BorderColor::all(with_alpha(ui.line, 0.5)),
        ))
        .id();
    let n = ui.text(c, format!("{}", shown.len()), &ui.f.display, 18.0, 0.5, ui.hot);
    let d = ui.text(c, "DOCKER", &ui.f.ui600, 9.0, 2.4, ui.dim);
    c.entity(hub).add_children(&[n, d]);
    let ring = c.spawn((Node { position_type: PositionType::Absolute, left: px(cx - rx), top: px(cy - ry), width: px(rx * 2.0), height: px(ry * 2.0), border: UiRect::all(px(1)), border_radius: BorderRadius::MAX, ..default() }, BorderColor::all(with_alpha(ui.line, 0.16)))).id();
    c.entity(map).add_children(&[ring, hub]);
    let m = shown.len().max(1);
    for (i, ct) in shown.iter().enumerate() {
        let a = std::f32::consts::TAU * i as f32 / m as f32 - std::f32::consts::FRAC_PI_2;
        let (x, y) = (cx + rx * a.cos(), cy + ry * a.sin());
        let sel = i == app.sel;
        let col = state_col(ct);
        let (w, h) = (178.0, 56.0);
        let pill = c
            .spawn((
                Node { position_type: PositionType::Absolute, left: px(x - w / 2.0), top: px(y - h / 2.0), width: px(w), height: px(h), flex_direction: FlexDirection::Column, justify_content: JustifyContent::Center, row_gap: px(3), padding: UiRect::horizontal(px(14)), border: UiRect::all(px(if sel { 2.0 } else { 1.0 })), border_radius: BorderRadius::all(px(28)), ..default() },
                BorderColor::all(if sel { col } else { with_alpha(ui.line, 0.4) }),
                BackgroundColor(Color::srgba(0.0, 0.0, 0.0, if sel { 0.6 } else { 0.4 })),
                Click::Pick(i),
            ))
            .id();
        let top = ui.row(c, 7.0);
        let dot = c.spawn((Node { width: px(7), height: px(7), border_radius: BorderRadius::MAX, ..default() }, BackgroundColor(col))).id();
        let nm = ui.text(c, clip(if ct.service.is_empty() { &ct.name } else { &ct.service }, 18), &ui.f.ui600, 13.0, 0.4, ui.hot);
        c.entity(top).add_children(&[dot, nm]);
        let bot = ui.row(c, 8.0);
        // which compose project it belongs to (the dot already says its state)
        let st = ui.text(c, if ct.project.is_empty() { state_word(ct) } else { clip(&ct.project.to_uppercase(), 12) }, &ui.f.ui700, 8.5, 1.8, if ct.unhealthy() { col } else { ui.dim });
        let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
        let mm = ui.text(c, if ct.state == "running" { format!("{:.1}% · {:.0} MB", ct.cpu, mib(ct.mem.split(" / ").next().unwrap_or(""))) } else { ct.state.to_uppercase() }, &ui.f.mono, 10.5, 0.0, ui.line);
        c.entity(bot).add_children(&[st, sp, mm]);
        c.entity(pill).add_children(&[top, bot]);
        c.entity(map).add_child(pill);
    }
    if shown.is_empty() {
        let t = c.spawn((Text::new("No containers"), font(&ui.f.ui500, 13.0, 0.5), TextColor(ui.dim), Node { position_type: PositionType::Absolute, left: px(cx - 50.0), top: px(cy + 60.0), ..default() })).id();
        c.entity(map).add_child(t);
    }
}

fn draw_detail(c: &mut Commands, ui: &Ui, root: Entity, ct: &Container, hist: &[f32], armed: bool) {
    let b = ui.body(c, root);
    let col = state_col(ct);
    let h = ui.header(c, &clip(&ct.name, 26), &state_word(ct), col);
    let inner = c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(4), padding: UiRect { left: px(16), right: px(16), top: px(10), ..default() }, ..default() }).id();
    c.entity(b).add_children(&[h, inner]);
    let img = ui.text(c, clip(&ct.image, 44), &ui.f.mono, 12.5, 0.0, ui.hot);
    c.entity(inner).add_child(img);
    let ports: Vec<String> = opsdata::parse_published(&ct.ports).into_iter().map(|(h, i)| format!("{h} → {i}")).collect();
    let rows = [
        ("id", ct.id.chars().take(12).collect::<String>()),
        ("ports", if ports.is_empty() { "-".into() } else { clip(&ports.join(", "), 30) }),
        ("compose", if ct.project.is_empty() { "-".into() } else { format!("{} · {}", ct.project, ct.service) }),
        ("status", clip(&ct.status, 30)),
        ("health", ct.health.clone().unwrap_or_else(|| "no check".into())),
    ];
    for (k, v) in rows {
        let r = ui.kv(c, k, v, ui.line);
        c.entity(inner).add_child(r);
    }
    let max = hist.iter().cloned().fold(5.0, f32::max);
    let vals: Vec<f32> = hist.iter().map(|v| v / max).collect();
    let lab = ui.text(c, "CPU · LAST 24 SAMPLES", &ui.f.ui600, 9.5, 1.8, ui.dim);
    let sp = ui.spark(c, &vals, 34.0, ui.line);
    c.entity(inner).add_children(&[lab, sp]);
    for (k, v, frac) in [("CPU", format!("{:.1}%", ct.cpu), ct.cpu / 100.0), ("MEM", clip(&ct.mem, 22), ct.mem_frac)] {
        let row = ui.row(c, 10.0);
        let kt = ui.text(c, k, &ui.f.ui600, 9.5, 1.8, ui.dim);
        let bar = ui.bar(c, frac, role_color(Role::Fill), 150.0);
        let vt = ui.text(c, v, &ui.f.mono, 11.0, 0.0, ui.line);
        c.entity(row).add_children(&[kt, bar, vt]);
        c.entity(inner).add_child(row);
    }
    let btns = c.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(8), margin: UiRect::top(px(8)), ..default() }).id();
    let running = ct.state == "running";
    let a = ui.button(c, if running { "Stop" } else { "Start" }, "S", ui.hot, false);
    let r = ui.button(c, "Restart", "R", ui.hot, false);
    let p = ui.button(c, "Pull", "P", ui.hot, false);
    let d = ui.button(c, if armed { "Press D again" } else { "Remove" }, "D", role_color(Role::Bad), armed);
    for (e, k) in [(a, KeyCode::KeyS), (r, KeyCode::KeyR), (p, KeyCode::KeyP), (d, KeyCode::KeyD)] {
        c.entity(e).insert(Click::Key(k));
    }
    c.entity(btns).add_children(&[a, r, p, d]);
    c.entity(inner).add_child(btns);
}

fn draw_logs(c: &mut Commands, ui: &Ui, root: Entity, ct: &Container, logs: &[String], follow: bool) {
    let b = ui.body(c, root);
    let h = ui.header(c, "Logs", &format!("{} · {}", ct.name, if follow { "following" } else { "paused (F)" }), ui.dim);
    c.entity(h).insert(Click::Key(KeyCode::KeyF));
    let list = c.spawn(Node { flex_direction: FlexDirection::Column, padding: UiRect { left: px(16), right: px(16), top: px(6), ..default() }, ..default() }).id();
    c.entity(b).add_children(&[h, list]);
    if logs.is_empty() {
        let t = ui.text(c, if ct.state == "running" { "No output yet" } else { "Not running" }, &ui.f.ui500, 12.5, 0.4, ui.dim);
        c.entity(list).add_child(t);
    }
    for l in logs {
        // "2026-10-17T07:10:00.123456789Z message"
        let (ts, msg) = l.split_once(' ').unwrap_or(("", l));
        let time = ts.get(11..23).unwrap_or("").to_string();
        let lower = msg.to_ascii_lowercase();
        let (lvl, col) = if lower.contains("error") || lower.contains("fatal") || lower.contains("panic") {
            ("ERROR", role_color(Role::Bad))
        } else if lower.contains("warn") {
            ("WARN", role_color(Role::Acc))
        } else {
            ("INFO", ui.line)
        };
        let row = c.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(14), height: px(17), overflow: Overflow::clip(), ..default() }).id();
        let t = ui.text(c, time, &ui.f.mono, 11.0, 0.0, ui.dim);
        let lv = c.spawn((Text::new(lvl), font(&ui.f.mono, 11.0, 0.0), TextColor(col), Node { width: px(42), flex_shrink: 0.0, ..default() })).id();
        let m = ui.text(c, clip(msg.trim(), 140), &ui.f.mono, 11.5, 0.0, if lvl == "INFO" { ui.line } else { col });
        c.entity(row).add_children(&[t, lv, m]);
        c.entity(list).add_child(row);
    }
}
