//! The Ports & logs app: every TCP port listening on this machine around a hub, each with who
//! owns it (a process, or the container publishing it), and a detail pane for the selected one. Keys: ← → ↑ ↓
//! (or J) select, , . turn the page of tiles, Tab filter (all, containers, processes), O open in the browser, T
//! its logs (containers), K stop the process (asks twice), Esc closes.
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
/// Tiles on the two rings (8 inside, the rest outside): one page, which turns with the selection.
const PAGE: usize = 22;
/// Rows of the table.
const ROWS: usize = 7;
const FILTERS: [&str; 3] = ["ALL", "CONTAINERS", "PROCESSES"];

type Note = Result<String, String>;
type Listing = Result<Vec<Listener>, String>;

#[derive(Resource)]
pub struct PortsApp {
    pub open: bool,
    pub amt: Spring,
    list: Option<Listing>,
    filter: usize,
    /// the selected port (so a refresh never moves it)
    sel_port: Option<u16>,
    conns: Option<(u16, usize)>,
    conns_t: f32,
    conns_busy: bool,
    loading: bool,
    reload: bool,
    load_t: f32,
    rx: Option<Mutex<mpsc::Receiver<Listing>>>,
    conns_rx: Option<Mutex<mpsc::Receiver<(u16, usize)>>>,
    armed: Option<(f32, u16)>,
    notes: (mpsc::Sender<Note>, Mutex<mpsc::Receiver<Note>>),
    version: u64,
}

impl PortsApp {
    /// How many ports are listening, once a listing has come in (the Orbit tile's caption).
    pub fn listening(&self) -> Option<usize> {
        match &self.list {
            Some(Ok(v)) => Some(v.len()),
            _ => None,
        }
    }
}

impl Default for PortsApp {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        PortsApp { open: false, amt: Spring::new(0.0, 26.0, 9.0), list: None, filter: 0, sel_port: None, conns: None, conns_t: -99.0, conns_busy: false, loading: false, reload: false, load_t: -99.0, rx: None, conns_rx: None, armed: None, notes: (tx, Mutex::new(rx)), version: 0 }
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
    fn all(&self) -> &[Listener] {
        match &self.list {
            Some(Ok(v)) => v,
            _ => &[],
        }
    }
    /// The listeners the filter lets through, all of them.
    fn shown(&self) -> Vec<&Listener> {
        self.all().iter().filter(|l| self.filter == 0 || (self.filter == 1) == l.container.is_some()).collect()
    }
    /// Index of the selected port among those shown (the first when none is chosen or it has gone).
    fn sel(&self) -> usize {
        self.sel_port.and_then(|p| self.shown().iter().position(|l| l.port == p)).unwrap_or(0)
    }
    fn selected(&self) -> Option<Listener> {
        self.shown().get(self.sel()).map(|l| (*l).clone())
    }
    fn select(&mut self, i: usize) {
        let p = self.shown().get(i).map(|l| l.port);
        if p.is_some() && p != self.sel_port {
            self.sel_port = p;
            self.armed = None;
        }
        self.version += 1;
    }
    fn set_filter(&mut self, f: usize) {
        self.filter = f.min(FILTERS.len() - 1);
        self.sel_port = None;
        self.armed = None;
        self.version += 1;
    }
    fn pages(&self) -> usize {
        self.shown().len().div_ceil(PAGE).max(1)
    }
}

#[derive(Resource)]
pub struct PortsRoots(Workspace);

pub fn setup(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut images: ResMut<Assets<Image>>, (home, mut atlases): (Res<PanelAtlas>, ResMut<AppAtlases>), mut pmats: ResMut<Assets<PanelMaterial>>, args: Res<crate::Args>, mut app: ResMut<PortsApp>) {
    if args.open_app.as_deref() == Some("ports") {
        app.open();
    }
    let ws = spawn_workspace(&mut commands, &mut meshes, &mut images, &home, &mut atlases, &mut pmats, APP, &PANES);
    commands.insert_resource(PortsRoots(ws));
}

#[allow(clippy::too_many_arguments)]
/// The key the wheel presses over a pane: the map and the list step through the ports.
fn scroll_key(pane: Option<u8>, down: bool) -> Option<KeyCode> {
    match pane? as usize {
        MAP | LIST => Some(if down { KeyCode::KeyJ } else { KeyCode::ArrowUp }),
        _ => None,
    }
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
        if e.state != bevy::input::ButtonState::Pressed || mods || bar_busy || focus.terminal || moment.shown || e.repeat {
            continue;
        }
        codes.push(e.key_code);
    }
    codes.extend(crate::appframe::scroll_keys(scrolls.take(APP), scroll_key));
    // clicks on the panes: a button presses its key, a tile or row picks its port, a tab its filter
    for c in clicks.take(3) {
        match c {
            Click::Key(k) => codes.push(k),
            Click::Pick(i) => app.select(i),
            Click::Alt(i) => app.set_filter(i),
        }
    }
    for key in codes {
        let n = app.shown().len();
        let i = app.sel();
        let sel = app.selected();
        match key {
            KeyCode::Escape => {
                app.close();
                sound.play_with("collapse", 0.7, 0.0);
            }
            KeyCode::ArrowRight | KeyCode::ArrowDown | KeyCode::KeyJ if n > 0 => {
                app.select((i + 1) % n);
                sound.play_q("tick", 40);
            }
            KeyCode::ArrowLeft | KeyCode::ArrowUp if n > 0 => {
                app.select((i + n - 1) % n);
                sound.play_q("tick", 40);
            }
            KeyCode::Period if app.pages() > 1 => {
                let p = (i / PAGE + 1) % app.pages();
                app.select(p * PAGE);
                sound.play_q("tick", 40);
            }
            KeyCode::Comma if app.pages() > 1 => {
                let p = (i / PAGE + app.pages() - 1) % app.pages();
                app.select(p * PAGE);
                sound.play_q("tick", 40);
            }
            KeyCode::Tab => {
                let f = (app.filter + 1) % FILTERS.len();
                app.set_filter(f);
                sound.play("toggle");
            }
            KeyCode::KeyO => {
                if let Some(l) = sel {
                    let kind = port_name(l.port);
                    if NOT_WEB.contains(&kind) {
                        toasts.push_quiet(format!(":{} is {kind}, not a web page", l.port));
                    } else {
                        let url = format!("http://localhost:{}", l.port);
                        toasts.push_quiet(format!("Opening {url}"));
                        crate::util::spawn(move || {
                            let _ = std::process::Command::new("open").arg(url).status();
                        });
                        sound.play("open");
                    }
                }
            }
            KeyCode::KeyT => match sel.and_then(|l| l.container) {
                Some(name) => {
                    open.containers_focus = Some(name);
                    open.app(4, &mut toasts);
                }
                None => toasts.push_quiet("Logs are for container ports. Select one with a container behind it"),
            },
            KeyCode::KeyK => {
                if let Some(l) = sel {
                    if l.container.is_some() {
                        toasts.push("That port belongs to a container: stop it in Containers (T)");
                    } else if app.armed.is_some_and(|(t, p)| sim.t < t && p == l.port) {
                        app.armed = None;
                        let tx = app.notes.0.clone();
                        let (port, pid, name) = (l.port, l.pid, l.command.clone());
                        toasts.push_quiet(format!("Stopping {name}…"));
                        crate::util::spawn(move || {
                            let _ = tx.send(opsdata::terminate_listener(port, pid, &name));
                        });
                        sound.play("discard");
                    } else {
                        app.armed = Some((sim.t + 3.0, l.port));
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

pub fn update(sim: Res<crate::core::Sim>, mut atlases: ResMut<AppAtlases>, mut app: ResMut<PortsApp>, mut ctl: ResMut<CoreCtl>, mut toasts: ResMut<crate::live::Toasts>, mut sound: ResMut<crate::sound::Sound>) {
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
        // look again shortly: the process needs a moment to let go of its port
        if app.loading {
            app.reload = true;
        } else {
            app.load_t = sim.t - 3.5;
        }
    }
    let got: Vec<Listing> = app.rx.as_ref().and_then(|r| r.lock().ok().map(|r| r.try_iter().collect())).unwrap_or_default();
    for list in got {
        app.loading = false;
        if std::mem::take(&mut app.reload) {
            app.load_t = sim.t - 3.5;
        }
        // a new listener is news
        if let (Some(Ok(old)), Ok(new)) = (&app.list, &list) {
            if let Some(l) = new.iter().find(|l| !old.iter().any(|o| o.port == l.port)) {
                toasts.push_quiet(format!("New listener · :{} {}", l.port, l.container.clone().unwrap_or_else(|| l.command.clone())));
            }
        }
        app.list = Some(list);
        // pin the selection to a port (the first, until one is chosen)
        let i = app.sel();
        app.select(i);
        app.conns_t = -99.0;
        app.version += 1;
    }
    let got: Vec<(u16, usize)> = app.conns_rx.as_ref().and_then(|r| r.lock().ok().map(|r| r.try_iter().collect())).unwrap_or_default();
    for c in got {
        app.conns_busy = false;
        app.conns = Some(c);
        app.version += 1;
    }
    if !app.open {
        return;
    }
    if let Some(l) = app.selected() {
        let stale = app.conns.is_none_or(|c| c.0 != l.port) || sim.t - app.conns_t > 5.0;
        if stale && !app.conns_busy {
            app.conns_busy = true;
            app.conns_t = sim.t;
            let (tx, rx) = mpsc::channel();
            app.conns_rx = Some(Mutex::new(rx));
            let port = l.port;
            crate::util::spawn(move || {
                let _ = tx.send((port, opsdata::connections(port)));
            });
        }
    }
    if !app.loading && sim.t - app.load_t > 5.0 {
        app.load_t = sim.t;
        app.loading = true;
        let (tx, rx) = mpsc::channel();
        app.rx = Some(Mutex::new(rx));
        crate::util::spawn(move || {
            // without Docker the ports still show, just without container names
            let cs = opsdata::containers_quick().unwrap_or_default();
            let _ = tx.send(opsdata::listeners(&cs));
        });
    }
}

pub fn place(app: Res<PortsApp>, mut q: Query<(&mut Panel, &AppPart)>) {
    place_app(APP, app.amt.x, |_| false, &mut q);
}

/// What a well-known port usually is (a label, not a fact about the process).
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

/// Ports that usually speak something other than HTTP.
const NOT_WEB: [&str; 5] = ["postgres", "mysql", "redis", "mongo", "adb"];

/// This machine's name for the hub, read once.
fn host() -> &'static str {
    static H: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    H.get_or_init(|| {
        // SPHERE_HOSTNAME overrides it (recordings shouldn't show the machine's name)
        if let Ok(h) = std::env::var("SPHERE_HOSTNAME") {
            return h.to_uppercase();
        }
        #[cfg(target_arch = "wasm32")]
        let (buf, ok) = ([0u8; 1], false);
        #[cfg(not(target_arch = "wasm32"))]
        let mut buf = [0u8; 256];
        #[cfg(not(target_arch = "wasm32"))]
        let ok = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len() - 1) } == 0;
        let name = if ok { String::from_utf8_lossy(&buf[..buf.iter().position(|b| *b == 0).unwrap_or(0)]).to_string() } else { String::new() };
        name.split('.').next().unwrap_or("").to_uppercase()
    })
}

/// "all interfaces" for lsof's `*`.
fn bound(addr: &str) -> String {
    match addr {
        "*" => "all interfaces".into(),
        "127.0.0.1" | "[::1]" => format!("{addr} (this machine)"),
        a => a.to_string(),
    }
}

/// What a tile says under its port: the container's service or the process, with the usual name first.
fn tile_label(l: &Listener) -> String {
    let who = if l.container.is_some() { l.service.clone() } else { l.command.clone() };
    let label = if port_name(l.port).is_empty() { who } else { format!("{} · {who}", port_name(l.port)) };
    label.to_uppercase()
}

pub fn draw(mut commands: Commands, app: Res<PortsApp>, svc: Res<Service>, roots: Option<Res<PortsRoots>>, f: Res<UiFonts>, theme: Res<crate::theme::Theme>, mut seen: Local<(u64, u64, u64)>) {
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
    let all = app.all();
    let nc = all.iter().filter(|l| l.container.is_some()).count();
    let (l2, l2c) = match &app.list {
        None => ("looking…".to_string(), ui.line),
        Some(Err(_)) => ("lsof failed".to_string(), role_color(Role::Bad)),
        _ => (format!("{} listening", all.len()), ui.line),
    };
    ui.corelab(c, ws.corelab, "Ports & logs", l2, l2c);
    ui.top(c, ws.panes[TOP], "Ports & logs", format!("{} LISTENING", all.len()), "listener and process lookup after listeners (MIT) · lsof");
    ui.strip(c, ws.panes[STRIP], &agents, &merged, ["J STEP   , . PAGE   TAB FILTER   O OPEN   T TAIL LOGS", "K STOP PROCESS   ESC CLOSE"]);
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
        let t = c.spawn((Node { padding: UiRect { left: px(9), right: px(9), top: px(4), bottom: px(3) }, border: UiRect::all(px(1)), ..default() }, BorderColor::all(with_alpha(ui.line, 0.4)), BackgroundColor(if on { ui.hot } else { Color::NONE }), Click::Alt(i))).id();
        let tx = ui.text(c, *k, &f.ui600, 9.5, 1.8, if on { Color::BLACK } else { ui.line });
        c.entity(t).add_child(tx);
        c.entity(r).add_child(t);
    }
    let tab = crate::containers::chip(c, &ui, "TAB", KeyCode::Tab);
    c.entity(r).add_child(tab);
    // the map
    let map = c.spawn(Node { width: percent(100), height: percent(100), ..default() }).id();
    c.entity(ws.panes[MAP]).add_child(map);
    match &app.list {
        None => crate::containers::message(c, &ui, map, "Looking for listeners…", "", ui.line),
        Some(Err(e)) => crate::containers::message(c, &ui, map, "Ports can't be read", &format!("lsof said: {}", clip(e, 70)), role_color(Role::Bad)),
        Some(Ok(_)) => draw_map(c, &ui, map, &app),
    }
    match app.selected() {
        Some(l) => draw_detail(c, &ui, ws.panes[DETAIL], &l, app.conns.filter(|c| c.0 == l.port).map(|c| c.1), app.armed.is_some()),
        None => {
            let b = ui.body(c, ws.panes[DETAIL]);
            let h = ui.header(c, "Port", "", ui.dim);
            let t = ui.text(c, if app.list.is_none() { "Waiting for lsof" } else { "No port selected" }, &f.ui500, 12.5, 0.4, ui.dim);
            let t = c.spawn(Node { margin: UiRect::all(px(16)), ..default() }).add_child(t).id();
            c.entity(b).add_children(&[h, t]);
        }
    }
    // the table
    let b = ui.body(c, ws.panes[LIST]);
    let shown = app.shown();
    let sel = app.sel();
    let first = sel.saturating_sub(ROWS / 2).min(shown.len().saturating_sub(ROWS));
    let range = if shown.is_empty() { "0 shown".to_string() } else { format!("{}–{} of {}", first + 1, (first + ROWS).min(shown.len()), shown.len()) };
    let h = ui.header(c, "Listeners", &range, ui.dim);
    let (up, down) = (crate::containers::chip(c, &ui, "▲", KeyCode::ArrowUp), crate::containers::chip(c, &ui, "▼", KeyCode::ArrowDown));
    c.entity(h).add_children(&[up, down]);
    let list = c.spawn(Node { flex_direction: FlexDirection::Column, padding: UiRect { left: px(14), right: px(14), top: px(6), ..default() }, ..default() }).id();
    c.entity(b).add_children(&[h, list]);
    if shown.is_empty() {
        let t = ui.text(c, empty_note(&app), &f.ui500, 12.5, 0.4, ui.dim);
        c.entity(list).add_child(t);
    }
    for (i, l) in shown.iter().enumerate().skip(first).take(ROWS) {
        let on = i == sel;
        let row = c.spawn((Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, height: px(20), column_gap: px(10), padding: UiRect::left(px(6)), border: UiRect::left(px(if on { 2.0 } else { 0.0 })), overflow: Overflow::clip(), ..default() }, BorderColor::all(ui.hot), BackgroundColor(if on { with_alpha(ui.line, 0.1) } else { Color::NONE }), Click::Pick(i))).id();
        let p = c.spawn((Text::new(format!(":{}", l.port)), font(&f.mono, 12.0, 0.0), TextColor(ui.hot), Node { width: px(62), flex_shrink: 0.0, ..default() })).id();
        let who = ui.text(c, clip(l.container.as_deref().unwrap_or(&l.command), 22), &f.mono, 11.5, 0.0, ui.line);
        let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
        let kind = c.spawn((Text::new(if l.container.is_some() { "CONTAINER".to_string() } else { format!("PID {}", l.pid) }), font(&f.ui600, 9.5, 1.4), TextColor(ui.dim), Node { flex_shrink: 0.0, ..default() })).id();
        c.entity(row).add_children(&[p, who, sp, kind]);
        c.entity(list).add_child(row);
    }
}

fn empty_note(app: &PortsApp) -> &'static str {
    match (app.list.as_ref().map(|l| l.is_ok()), app.filter) {
        (Some(true), 1) => "No container is publishing a port",
        (Some(true), 2) => "No process is listening outside containers",
        (Some(true), _) => "Nothing is listening on a TCP port",
        (Some(false), _) => "Ports can't be read",
        _ => "Looking…",
    }
}

fn draw_map(c: &mut Commands, ui: &Ui, map: Entity, app: &PortsApp) {
    let shown = app.shown();
    let sel = app.sel();
    // one page: the selection's
    let page_first = sel / PAGE * PAGE;
    let page: Vec<(usize, &Listener)> = shown.iter().enumerate().skip(page_first).take(PAGE).map(|(i, l)| (i, *l)).collect();
    let (cx, cy) = (430.0, 262.0);
    let inner = page.len().min(8);
    let place = |k: usize| -> Vec2 {
        let (j, m, rx, ry, off) = if k < inner { (k, inner, 220.0, 120.0, 0.0) } else { (k - inner, page.len() - inner, 380.0, 220.0, 0.2) };
        let a = std::f32::consts::TAU * j as f32 / m.max(1) as f32 - std::f32::consts::FRAC_PI_2 + off;
        Vec2::new(cx + rx * a.cos(), cy + ry * a.sin())
    };
    // connectors from the hub (the selected port dashed and bright), under everything
    for (k, (i, l)) in page.iter().enumerate() {
        let to = place(k);
        let from = Vec2::new(cx, cy) + (to - Vec2::new(cx, cy)).normalize_or_zero() * 50.0;
        if *i == sel {
            ui.dashed(c, map, from, to, 1.5, with_alpha(ui.hot, 0.9), 5.0, 4.0);
        } else {
            ui.line(c, map, from, to, 1.0, with_alpha(if l.container.is_some() { role_color(Role::Fill) } else { ui.line }, 0.2));
        }
    }
    let hub = c
        .spawn((
            Node { position_type: PositionType::Absolute, left: px(cx - 48.0), top: px(cy - 48.0), width: px(96), height: px(96), border: UiRect::all(px(1)), border_radius: BorderRadius::MAX, flex_direction: FlexDirection::Column, align_items: AlignItems::Center, justify_content: JustifyContent::Center, ..default() },
            BorderColor::all(with_alpha(ui.line, 0.5)),
        ))
        .id();
    let h = ui.text(c, clip(host(), 12), &ui.f.ui600, 8.5, 1.6, ui.dim);
    let n = ui.text(c, format!("{}", app.all().len()), &ui.f.display, 18.0, 0.5, ui.hot);
    let d = ui.text(c, "LISTENING", &ui.f.ui600, 8.5, 2.0, ui.dim);
    c.entity(hub).add_children(&[h, n, d]);
    for (rx, ry) in [(220.0, 120.0), (380.0, 220.0)] {
        let ring = c.spawn((Node { position_type: PositionType::Absolute, left: px(cx - rx), top: px(cy - ry), width: px(rx * 2.0), height: px(ry * 2.0), border: UiRect::all(px(1)), border_radius: BorderRadius::MAX, ..default() }, BorderColor::all(with_alpha(ui.line, 0.13)))).id();
        c.entity(map).add_child(ring);
    }
    c.entity(map).add_child(hub);
    // inner ring holds up to 8, the rest go outside
    for (k, (i, l)) in page.iter().enumerate() {
        let Vec2 { x, y } = place(k);
        let on = *i == sel;
        let col = if l.container.is_some() { role_color(Role::Fill) } else { ui.line };
        let (w, h) = (100.0, 40.0);
        let tile = c
            .spawn((
                Node { position_type: PositionType::Absolute, left: px(x - w / 2.0), top: px(y - h / 2.0), width: px(w), height: px(h), flex_direction: FlexDirection::Column, align_items: AlignItems::Center, justify_content: JustifyContent::Center, border: UiRect::all(px(if on { 2.0 } else { 1.0 })), overflow: Overflow::clip(), ..default() },
                BorderColor::all(if on { ui.hot } else { with_alpha(col, 0.5) }),
                BackgroundColor(Color::srgba(0.0, 0.0, 0.0, if on { 0.65 } else { 0.45 })),
                Click::Pick(*i),
            ))
            .id();
        let p = ui.text(c, l.port.to_string(), &ui.f.mono, 13.5, 0.0, ui.hot);
        let s = ui.text(c, clip(&tile_label(l), 14), &ui.f.ui600, 8.0, 1.0, ui.dim);
        c.entity(tile).add_children(&[p, s]);
        c.entity(map).add_child(tile);
    }
    if shown.is_empty() && app.list.as_ref().is_some_and(|l| l.is_ok()) {
        let t = ui.text(c, empty_note(app), &ui.f.ui600, 14.0, 0.5, ui.line);
        let b = c.spawn(Node { position_type: PositionType::Absolute, left: px(cx - 200.0), top: px(cy + 70.0), width: px(400), justify_content: JustifyContent::Center, ..default() }).add_child(t).id();
        c.entity(map).add_child(b);
    }
    let pages = app.pages();
    if pages > 1 {
        let bar = c.spawn(Node { position_type: PositionType::Absolute, left: px(14), top: px(12), flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(8), ..default() }).id();
        let prev = crate::containers::chip(c, ui, "‹ ,", KeyCode::Comma);
        let t = ui.text(c, format!("{}–{} OF {} · PAGE {} / {pages}", page_first + 1, page_first + page.len(), shown.len(), page_first / PAGE + 1), &ui.f.ui600, 9.5, 1.4, ui.dim);
        let next = crate::containers::chip(c, ui, ". ›", KeyCode::Period);
        c.entity(bar).add_children(&[prev, t, next]);
        c.entity(map).add_child(bar);
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
    let mut rows = vec![("process", format!("{} · pid {}", clip(&l.command, 16), l.pid))];
    if l.container.is_some() {
        rows.push(("container", clip(l.container.as_deref().unwrap_or(""), 24)));
        rows.push(("publishes", if l.inner.is_empty() { "-".into() } else { l.inner.clone() }));
    } else {
        rows.push(("owner", "process".into()));
    }
    rows.push(("bound to", bound(&l.addr)));
    rows.push(("connections", conns.map(|n| format!("{n} established")).unwrap_or_else(|| "…".into())));
    let kind = port_name(l.port);
    rows.push(("usually", if kind.is_empty() { "-".into() } else { kind.to_string() }));
    for (k, v) in rows {
        let r = ui.kv(c, k, v, ui.line);
        c.entity(inner).add_child(r);
    }
    let btns = c.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(8), margin: UiRect::top(px(10)), ..default() }).id();
    let o = ui.button(c, "Open in browser", "O", ui.hot, true);
    let t = ui.button(c, "Tail logs", "T", ui.hot, false);
    let k = ui.button(c, if armed { "Press K again" } else { "Stop" }, "K", role_color(Role::Bad), armed);
    for (e, key) in [(o, KeyCode::KeyO), (t, KeyCode::KeyT), (k, KeyCode::KeyK)] {
        c.entity(e).insert(Click::Key(key));
    }
    c.entity(btns).add_children(&[o, t, k]);
    c.entity(inner).add_child(btns);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wheel_keys_per_pane() {
        assert_eq!(scroll_key(Some(LIST as u8), true), Some(KeyCode::KeyJ));
        assert_eq!(scroll_key(Some(MAP as u8), false), Some(KeyCode::ArrowUp));
        assert_eq!(scroll_key(Some(DETAIL as u8), true), None);
    }

    fn l(port: u16, container: bool) -> Listener {
        Listener { port, pid: 100 + port as u32, command: format!("proc{port}"), addr: "*".into(), container: container.then(|| format!("c{port}")), ..Default::default() }
    }

    fn app_with(v: Vec<Listener>) -> PortsApp {
        PortsApp { list: Some(Ok(v)), ..Default::default() }
    }

    #[test]
    fn filters_and_keeps_the_selection_on_its_port() {
        let mut a = app_with(vec![l(80, false), l(5432, true), l(8080, false), l(9000, true)]);
        assert_eq!(a.shown().len(), 4);
        a.set_filter(1);
        assert_eq!(a.shown().iter().map(|x| x.port).collect::<Vec<_>>(), vec![5432, 9000]);
        a.select(1);
        // a listener before it goes away: still the same port
        if let Some(Ok(v)) = &mut a.list {
            v.remove(0);
        }
        assert_eq!(a.selected().unwrap().port, 9000);
        a.set_filter(2);
        assert_eq!(a.shown().iter().map(|x| x.port).collect::<Vec<_>>(), vec![8080]);
        // the selected port isn't in this filter: first one shown
        assert_eq!(a.selected().unwrap().port, 8080);
    }

    #[test]
    fn every_listener_is_reachable_by_paging() {
        let a = app_with((1..=60).map(|p| l(p, false)).collect());
        assert_eq!(a.pages(), 3);
        let mut a = a;
        a.select(59);
        assert_eq!((a.sel(), a.selected().unwrap().port), (59, 60));
        assert_eq!(app_with(vec![]).pages(), 1);
    }

    #[test]
    fn labels_and_binding_are_plain() {
        let mut x = l(5432, true);
        x.service = "pgsql".into();
        assert_eq!(tile_label(&x), "POSTGRES · PGSQL");
        assert_eq!(tile_label(&l(4000, false)), "PROC4000");
        assert_eq!((bound("*").as_str(), bound("[::1]").as_str(), bound("10.0.0.2").as_str()), ("all interfaces", "[::1] (this machine)", "10.0.0.2"));
        assert!(!host().is_empty());
    }
}
