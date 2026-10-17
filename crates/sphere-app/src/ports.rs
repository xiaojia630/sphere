//! The Ports & logs app (design/apps/ports): every TCP port listening on this machine around a hub, each with who
//! owns it (a process, or the container publishing it), and a detail pane for the selected one. Keys: ← → (or J
//! K) select, Tab filter (all, containers, processes), O open in the browser, T its logs (containers), K stop
//! the process (asks twice), Esc closes.
use crate::appframe::*;
use crate::core::CoreCtl;
use crate::fonts::UiFonts;
use crate::opsdata::{self, Listener};
use crate::panel::*;
use crate::service::Service;
use crate::util::*;
use bevy::prelude::*;
use std::sync::{mpsc, Mutex};

pub const APP: u8 = 3;
const TOP: usize = 0;
const SUM: usize = 1;
const MAP: usize = 2;
const DETAIL: usize = 3;
const LIST: usize = 4;
const STRIP: usize = 5;
const PANES: [([f32; 5], bool); 6] = [
    ([22.0, 2.0, 1236.0, 32.0, 8.0], true),
    ([14.0, 42.0, 1252.0, 40.0, 12.0], false),
    ([14.0, 94.0, 860.0, 532.0, 16.0], false),
    ([886.0, 94.0, 380.0, 330.0, 44.0], false),
    ([886.0, 436.0, 380.0, 190.0, 30.0], false),
    ([14.0, 638.0, 1252.0, 50.0, 22.0], false),
];
const MAX_TILES: usize = 22;
const FILTERS: [&str; 3] = ["ALL", "CONTAINERS", "PROCESSES"];

type Note = Result<String, String>;
type Listing = Result<Vec<Listener>, String>;

#[derive(Resource)]
pub struct PortsApp {
    pub open: bool,
    pub amt: Spring,
    list: Option<Result<Vec<Listener>, String>>,
    filter: usize,
    sel: usize,
    conns: Option<(u16, usize)>,
    loading: bool,
    load_t: f32,
    rx: Option<Mutex<mpsc::Receiver<Listing>>>,
    conns_rx: Option<Mutex<mpsc::Receiver<(u16, usize)>>>,
    armed: Option<f32>,
    notes: (mpsc::Sender<Note>, Mutex<mpsc::Receiver<Note>>),
    version: u64,
}

impl Default for PortsApp {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        PortsApp { open: false, amt: Spring::new(0.0, 26.0, 9.0), list: None, filter: 0, sel: 0, conns: None, loading: false, load_t: -99.0, rx: None, conns_rx: None, armed: None, notes: (tx, Mutex::new(rx)), version: 0 }
    }
}

impl PortsApp {
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
    fn shown(&self) -> Vec<&Listener> {
        let Some(Ok(v)) = &self.list else { return Vec::new() };
        v.iter().filter(|l| self.filter == 0 || (self.filter == 1) == l.container.is_some()).take(MAX_TILES).collect()
    }
    fn selected(&self) -> Option<Listener> {
        self.shown().get(self.sel).map(|l| (*l).clone())
    }
}

#[derive(Resource)]
pub struct PortsRoots(Workspace);

pub fn setup(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut atlas: ResMut<PanelAtlas>, mut pmats: ResMut<Assets<PanelMaterial>>, args: Res<crate::Args>, mut app: ResMut<PortsApp>) {
    if args.open_app.as_deref() == Some("ports") {
        app.open();
    }
    let ws = spawn_workspace(&mut commands, &mut meshes, &mut atlas, &mut pmats, APP, &PANES);
    commands.insert_resource(PortsRoots(ws));
}

#[allow(clippy::too_many_arguments)]
pub fn keys(
    mut ev: MessageReader<bevy::input::keyboard::KeyboardInput>,
    keys: Res<ButtonInput<KeyCode>>,
    (focus, moment): (Res<Focus>, Res<crate::moment::Moment>),
    mut app: ResMut<PortsApp>,
    mut open: ResMut<crate::apps::Open>,
    sim: Res<crate::core::Sim>,
    mut sound: ResMut<crate::sound::Sound>,
    mut toasts: ResMut<crate::live::Toasts>,
    mut cmd_was: Local<bool>,
) {
    let mods = keys.any_pressed([KeyCode::SuperLeft, KeyCode::SuperRight, KeyCode::ControlLeft, KeyCode::ControlRight, KeyCode::AltLeft, KeyCode::AltRight]);
    let bar_busy = focus.cmd || *cmd_was;
    *cmd_was = focus.cmd;
    if !app.open {
        ev.clear();
        return;
    }
    for e in ev.read() {
        if e.state != bevy::input::ButtonState::Pressed || mods || bar_busy || focus.terminal || moment.shown || e.repeat {
            continue;
        }
        let n = app.shown().len();
        let sel = app.selected();
        match e.key_code {
            KeyCode::Escape => {
                app.close();
                sound.play_with("collapse", 0.7, 0.0);
            }
            KeyCode::ArrowRight | KeyCode::KeyJ | KeyCode::ArrowDown if n > 0 => {
                app.sel = (app.sel + 1) % n;
                app.armed = None;
                app.version += 1;
                sound.play_q("tick", 40);
            }
            KeyCode::ArrowLeft | KeyCode::KeyK | KeyCode::ArrowUp if n > 0 && e.key_code != KeyCode::KeyK => {
                app.sel = (app.sel + n - 1) % n;
                app.armed = None;
                app.version += 1;
                sound.play_q("tick", 40);
            }
            KeyCode::Tab => {
                app.filter = (app.filter + 1) % FILTERS.len();
                app.sel = 0;
                app.version += 1;
                sound.play("toggle");
            }
            KeyCode::KeyO => {
                if let Some(l) = sel {
                    let url = format!("http://localhost:{}", l.port);
                    let _ = std::process::Command::new("open").arg(&url).spawn();
                    toasts.push_quiet(format!("Opening {url}"));
                    sound.play("open");
                }
            }
            KeyCode::KeyT => match sel.and_then(|l| l.container) {
                Some(name) => {
                    open.containers_focus = Some(name);
                    open.app(4, &mut toasts);
                }
                None => toasts.push_quiet("Logs are for container ports for now"),
            },
            KeyCode::KeyK => {
                if let Some(l) = sel {
                    if l.container.is_some() {
                        toasts.push("That port belongs to a container: stop it in Containers (T)");
                    } else if app.armed.is_some_and(|t| sim.t < t) {
                        app.armed = None;
                        let tx = app.notes.0.clone();
                        let _ = tx.send(opsdata::terminate(l.pid));
                        sound.play("discard");
                        app.load_t = sim.t - 3.0;
                    } else {
                        app.armed = Some(sim.t + 3.0);
                        toasts.push_quiet(format!("Press K again to stop {} (pid {})", l.command, l.pid));
                        sound.play("toggle");
                    }
                    app.version += 1;
                }
            }
            _ => {}
        }
    }
}

pub fn update(sim: Res<crate::core::Sim>, mut app: ResMut<PortsApp>, mut ctl: ResMut<CoreCtl>, mut toasts: ResMut<crate::live::Toasts>, mut sound: ResMut<crate::sound::Sound>) {
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
    }
    let got = app.rx.as_ref().and_then(|r| r.lock().ok()?.try_recv().ok());
    if let Some(list) = got {
        app.loading = false;
        // a new listener is news
        if let (Some(Ok(old)), Ok(new)) = (&app.list, &list) {
            if let Some(l) = new.iter().find(|l| !old.iter().any(|o| o.port == l.port)) {
                toasts.push_quiet(format!("New listener · :{} {}", l.port, l.container.clone().unwrap_or_else(|| l.command.clone())));
            }
        }
        app.list = Some(list);
        let n = app.shown().len();
        if app.sel >= n {
            app.sel = 0;
        }
        app.conns = None;
        app.version += 1;
    }
    let got = app.conns_rx.as_ref().and_then(|r| r.lock().ok()?.try_recv().ok());
    if let Some(c) = got {
        app.conns = Some(c);
        app.version += 1;
    }
    if !app.open {
        return;
    }
    if let Some(l) = app.selected() {
        if app.conns.is_none_or(|c| c.0 != l.port) && app.conns_rx.is_none() {
            let (tx, rx) = mpsc::channel();
            app.conns_rx = Some(Mutex::new(rx));
            let port = l.port;
            std::thread::spawn(move || {
                let _ = tx.send((port, opsdata::connections(port)));
            });
        }
    }
    if app.conns.is_some() {
        app.conns_rx = None;
    }
    if !app.loading && sim.t - app.load_t > 5.0 {
        app.load_t = sim.t;
        app.loading = true;
        let (tx, rx) = mpsc::channel();
        app.rx = Some(Mutex::new(rx));
        std::thread::spawn(move || {
            let cs = opsdata::containers_quick().unwrap_or_default();
            let _ = tx.send(opsdata::listeners(&cs));
        });
    }
}

pub fn place(app: Res<PortsApp>, mut q: Query<(&mut Panel, &AppPart)>) {
    place_app(APP, app.amt.x, |_| false, &mut q);
}

/// What a well-known port usually is.
fn port_name(p: u16) -> &'static str {
    match p {
        3000 | 5173 | 4173 => "web",
        5432 => "postgres",
        3306 => "mysql",
        6379 => "redis",
        9000 => "minio",
        11434 => "ollama",
        8080 | 8000 | 8090 | 8088 => "http",
        5037 => "adb",
        7000 | 5000 => "airplay",
        27017 => "mongo",
        _ => "",
    }
}

pub fn draw(mut commands: Commands, app: Res<PortsApp>, svc: Res<Service>, roots: Option<Res<PortsRoots>>, f: Res<UiFonts>, theme: Res<crate::theme::Theme>, mut seen: Local<(u64, u64, u64)>) {
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
    let all: Vec<Listener> = match &app.list {
        Some(Ok(v)) => v.clone(),
        _ => Vec::new(),
    };
    let nc = all.iter().filter(|l| l.container.is_some()).count();
    ui.corelab(c, ws.corelab, "Ports & logs", format!("{} listening", all.len()), ui.line);
    ui.top(c, ws.panes[TOP], "Ports & logs", format!("{} LISTENING", all.len()), "listener and process lookup after listeners (MIT) · lsof");
    ui.strip(c, ws.panes[STRIP], &agents, ["← → STEP   TAB FILTER   O OPEN   T TAIL LOGS", "K STOP PROCESS   ESC CLOSE"]);
    // summary and filters
    let r = c.spawn(Node { width: percent(100), height: percent(100), flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(22), padding: UiRect::horizontal(px(16)), ..default() }).id();
    c.entity(ws.panes[SUM]).add_child(r);
    for (v, k) in [(nc, "containers"), (all.len() - nc, "processes")] {
        let row = ui.row(c, 6.0);
        let a = ui.text(c, v.to_string(), &f.mono, 13.0, 0.0, ui.hot);
        let b = ui.text(c, k, &f.ui500, 11.5, 0.6, ui.dim);
        c.entity(row).add_children(&[a, b]);
        c.entity(r).add_child(row);
    }
    let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
    c.entity(r).add_child(sp);
    for (i, k) in FILTERS.iter().enumerate() {
        let on = i == app.filter;
        let t = c.spawn((Node { padding: UiRect { left: px(9), right: px(9), top: px(4), bottom: px(3) }, border: UiRect::all(px(1)), ..default() }, BorderColor::all(with_alpha(ui.line, 0.4)), BackgroundColor(if on { ui.hot } else { Color::NONE }))).id();
        let tx = ui.text(c, *k, &f.ui600, 9.5, 1.8, if on { Color::BLACK } else { ui.line });
        c.entity(t).add_child(tx);
        c.entity(r).add_child(t);
    }
    // the map
    let map = c.spawn(Node { width: percent(100), height: percent(100), ..default() }).id();
    c.entity(ws.panes[MAP]).add_child(map);
    match &app.list {
        None => {
            let t = c.spawn((Text::new("Looking for listeners…"), font(&f.ui500, 13.0, 0.5), TextColor(ui.line), Node { margin: UiRect::all(px(18)), ..default() })).id();
            c.entity(map).add_child(t);
        }
        Some(Err(e)) => {
            let t = c.spawn((Text::new(format!("lsof failed: {}", clip(e, 70))), font(&f.ui500, 13.0, 0.5), TextColor(role_color(Role::Bad)), Node { margin: UiRect::all(px(18)), ..default() })).id();
            c.entity(map).add_child(t);
        }
        Some(Ok(_)) => draw_map(c, &ui, map, &app),
    }
    match app.selected() {
        Some(l) => draw_detail(c, &ui, ws.panes[DETAIL], &l, app.conns.filter(|c| c.0 == l.port).map(|c| c.1), app.armed.is_some()),
        None => {
            let b = ui.body(c, ws.panes[DETAIL]);
            let h = ui.header(c, "Port", "", ui.dim);
            c.entity(b).add_child(h);
        }
    }
    // the table
    let b = ui.body(c, ws.panes[LIST]);
    let h = ui.header(c, "Listeners", &format!("{} shown", app.shown().len()), ui.dim);
    let list = c.spawn(Node { flex_direction: FlexDirection::Column, padding: UiRect { left: px(14), right: px(14), top: px(6), ..default() }, ..default() }).id();
    c.entity(b).add_children(&[h, list]);
    let shown = app.shown();
    let first = app.sel.saturating_sub(5).min(shown.len().saturating_sub(6));
    for (i, l) in shown.iter().enumerate().skip(first).take(6) {
        let sel = i == app.sel;
        let row = c.spawn((Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, height: px(20), column_gap: px(10), padding: UiRect::left(px(6)), border: UiRect::left(px(if sel { 2.0 } else { 0.0 })), ..default() }, BorderColor::all(ui.hot), BackgroundColor(if sel { with_alpha(ui.line, 0.1) } else { Color::NONE }))).id();
        let p = c.spawn((Text::new(format!(":{}", l.port)), font(&f.mono, 12.0, 0.0), TextColor(ui.hot), Node { width: px(62), ..default() })).id();
        let who = ui.text(c, clip(l.container.as_deref().unwrap_or(&l.command), 22), &f.mono, 11.5, 0.0, ui.line);
        let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
        let kind = ui.text(c, if l.container.is_some() { "CONTAINER".to_string() } else { format!("PID {}", l.pid) }, &f.ui600, 9.5, 1.4, ui.dim);
        c.entity(row).add_children(&[p, who, sp, kind]);
        c.entity(list).add_child(row);
    }
}

fn draw_map(c: &mut Commands, ui: &Ui, map: Entity, app: &PortsApp) {
    let shown = app.shown();
    let (cx, cy) = (430.0, 262.0);
    let hub = c
        .spawn((
            Node { position_type: PositionType::Absolute, left: px(cx - 48.0), top: px(cy - 48.0), width: px(96), height: px(96), border: UiRect::all(px(1)), border_radius: BorderRadius::MAX, flex_direction: FlexDirection::Column, align_items: AlignItems::Center, justify_content: JustifyContent::Center, ..default() },
            BorderColor::all(with_alpha(ui.line, 0.5)),
        ))
        .id();
    let host = std::process::Command::new("hostname").arg("-s").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_uppercase()).unwrap_or_default();
    let h = ui.text(c, clip(&host, 12), &ui.f.ui600, 8.5, 1.6, ui.dim);
    let n = ui.text(c, format!("{}", app.list.as_ref().and_then(|l| l.as_ref().ok()).map(|v| v.len()).unwrap_or(0)), &ui.f.display, 18.0, 0.5, ui.hot);
    let d = ui.text(c, "LISTENING", &ui.f.ui600, 8.5, 2.0, ui.dim);
    c.entity(hub).add_children(&[h, n, d]);
    for (rx, ry) in [(220.0, 120.0), (380.0, 220.0)] {
        let ring = c.spawn((Node { position_type: PositionType::Absolute, left: px(cx - rx), top: px(cy - ry), width: px(rx * 2.0), height: px(ry * 2.0), border: UiRect::all(px(1)), border_radius: BorderRadius::MAX, ..default() }, BorderColor::all(with_alpha(ui.line, 0.13)))).id();
        c.entity(map).add_child(ring);
    }
    c.entity(map).add_child(hub);
    // inner ring holds up to 8, the rest go outside
    let inner = shown.len().min(8);
    for (i, l) in shown.iter().enumerate() {
        let (k, m, rx, ry, off) = if i < inner { (i, inner, 220.0, 120.0, 0.0) } else { (i - inner, shown.len() - inner, 380.0, 220.0, 0.2) };
        let a = std::f32::consts::TAU * k as f32 / m.max(1) as f32 - std::f32::consts::FRAC_PI_2 + off;
        let (x, y) = (cx + rx * a.cos(), cy + ry * a.sin());
        let sel = i == app.sel;
        let col = if l.container.is_some() { role_color(Role::Fill) } else { ui.line };
        let (w, h) = (92.0, 40.0);
        let tile = c
            .spawn((
                Node { position_type: PositionType::Absolute, left: px(x - w / 2.0), top: px(y - h / 2.0), width: px(w), height: px(h), flex_direction: FlexDirection::Column, align_items: AlignItems::Center, justify_content: JustifyContent::Center, border: UiRect::all(px(if sel { 2.0 } else { 1.0 })), ..default() },
                BorderColor::all(if sel { ui.hot } else { with_alpha(col, 0.5) }),
                BackgroundColor(Color::srgba(0.0, 0.0, 0.0, if sel { 0.65 } else { 0.45 })),
            ))
            .id();
        let p = ui.text(c, l.port.to_string(), &ui.f.mono, 13.5, 0.0, ui.hot);
        let who = l.container.as_deref().map(|c| c.rsplit('-').nth(1).unwrap_or(c).to_string()).unwrap_or_else(|| l.command.clone());
        let label = if port_name(l.port).is_empty() { who } else { format!("{} · {}", port_name(l.port), who) };
        let s = ui.text(c, clip(&label.to_uppercase(), 13), &ui.f.ui600, 8.0, 1.2, ui.dim);
        c.entity(tile).add_children(&[p, s]);
        c.entity(map).add_child(tile);
    }
}

fn draw_detail(c: &mut Commands, ui: &Ui, root: Entity, l: &Listener, conns: Option<usize>, armed: bool) {
    let b = ui.body(c, root);
    let name = l.container.clone().unwrap_or_else(|| l.command.clone());
    let h = ui.header(c, &clip(&name, 26), "LISTENING", role_color(Role::Ok));
    let inner = c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(4), padding: UiRect { left: px(16), right: px(16), top: px(8), ..default() }, ..default() }).id();
    c.entity(b).add_children(&[h, inner]);
    let big = ui.row(c, 10.0);
    let p = ui.text(c, format!(":{}", l.port), &ui.f.display, 30.0, 1.0, ui.hot);
    let t = ui.text(c, "TCP", &ui.f.ui700, 9.5, 1.6, ui.dim);
    c.entity(big).add_children(&[p, t]);
    let url = ui.text(c, format!("http://localhost:{}", l.port), &ui.f.mono, 12.0, 0.0, ui.line);
    c.entity(inner).add_children(&[big, url]);
    let rows = [
        ("process", format!("{} · pid {}", clip(&l.command, 16), l.pid)),
        ("owner", if l.container.is_some() { "container · docker".into() } else { "process".to_string() }),
        ("bound to", l.addr.clone()),
        ("connections", conns.map(|n| format!("{n} established")).unwrap_or_else(|| "…".into())),
        ("kind", if port_name(l.port).is_empty() { "-".into() } else { port_name(l.port).to_string() }),
    ];
    for (k, v) in rows {
        let r = ui.kv(c, k, v, ui.line);
        c.entity(inner).add_child(r);
    }
    let btns = c.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(8), margin: UiRect::top(px(12)), ..default() }).id();
    let o = ui.button(c, "Open in browser", "O", ui.hot, true);
    let t = ui.button(c, "Tail logs", "T", ui.hot, false);
    let k = ui.button(c, if armed { "Press K again" } else { "Stop" }, "K", role_color(Role::Bad), armed);
    c.entity(btns).add_children(&[o, t, k]);
    c.entity(inner).add_child(btns);
}
