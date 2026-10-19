//! The Containers app: your Docker or OrbStack containers around a hub, grouped by
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
/// Pills that fit the map: the ring shows one page of this many, and the selection turns the page.
const PAGE: usize = 10;
/// Log lines kept, shown at a time, and moved by one scroll step.
const LOG_KEEP: usize = 400;
const LOG_ROWS: usize = 9;
const LOG_STEP: usize = 8;

type Note = (u64, Result<String, String>);
type Listing = Result<Vec<Container>, String>;
type LogsMsg = (String, Result<Vec<String>, String>);

#[derive(Resource)]
pub struct ContainersApp {
    pub open: bool,
    pub amt: Spring,
    list: Option<Listing>,
    /// recent CPU % per container, newest last
    hist: HashMap<String, Vec<f32>>,
    /// the selected container, by id so a refresh never moves it
    sel_id: Option<String>,
    logs: Vec<String>,
    /// which container the lines belong to, and what went wrong reading them
    logs_for: String,
    logs_err: Option<String>,
    /// lines scrolled back from the newest
    log_off: usize,
    follow: bool,
    logs_dirty: bool,
    logs_busy: bool,
    loading: bool,
    reload: bool,
    load_t: f32,
    logs_t: f32,
    rx: Option<Mutex<mpsc::Receiver<(bool, Listing)>>>,
    logs_rx: Option<Mutex<mpsc::Receiver<LogsMsg>>>,
    armed: Option<(f32, String)>,
    want: Option<String>,
    notes: (mpsc::Sender<Note>, Mutex<mpsc::Receiver<Note>>),
    /// commands still running: (id, what is happening, which container, since)
    working: Vec<(u64, String, String, f32)>,
    next_op: u64,
    tick: u32,
    version: u64,
}

impl Default for ContainersApp {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        ContainersApp {
            open: false,
            amt: Spring::new(0.0, 26.0, 9.0),
            list: None,
            hist: HashMap::new(),
            sel_id: None,
            logs: Vec::new(),
            logs_for: String::new(),
            logs_err: None,
            log_off: 0,
            follow: true,
            logs_dirty: true,
            logs_busy: false,
            loading: false,
            reload: false,
            load_t: -99.0,
            logs_t: -99.0,
            rx: None,
            logs_rx: None,
            armed: None,
            want: None,
            notes: (tx, Mutex::new(rx)),
            working: Vec::new(),
            next_op: 0,
            tick: 0,
            version: 0,
        }
    }
}

impl ContainersApp {
    pub fn open(&mut self) {
        self.open = true;
        self.load_t = -99.0;
        self.logs_dirty = true;
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
    fn all(&self) -> &[Container] {
        match &self.list {
            Some(Ok(v)) => v,
            _ => &[],
        }
    }
    /// Index of the selected container (the first when none is chosen yet or it has gone).
    fn sel(&self) -> usize {
        self.sel_id.as_ref().and_then(|id| self.all().iter().position(|c| &c.id == id)).unwrap_or(0)
    }
    fn selected(&self) -> Option<&Container> {
        self.all().get(self.sel())
    }
    fn pages(&self) -> usize {
        self.all().len().div_ceil(PAGE).max(1)
    }
    /// The containers on the ring now: the page the selection is on, with their index in the whole list.
    fn shown(&self) -> impl Iterator<Item = (usize, &Container)> {
        let first = self.sel() / PAGE * PAGE;
        self.all().iter().enumerate().skip(first).take(PAGE)
    }
    fn select(&mut self, i: usize) {
        let Some(id) = self.all().get(i).map(|c| c.id.clone()) else { return };
        if self.sel_id.as_deref() != Some(&id) {
            self.sel_id = Some(id);
            self.logs.clear();
            self.logs_for.clear();
            self.logs_err = None;
            self.log_off = 0;
            self.logs_dirty = true;
            self.armed = None;
        }
        self.version += 1;
    }
    fn act(&mut self, now: f32, label: String, target: &str, op: DockerOp) {
        if self.working.iter().any(|w| w.2 == target) {
            return;
        }
        self.next_op += 1;
        let id = self.next_op;
        self.working.push((id, label, target.to_string(), now));
        let tx = self.notes.0.clone();
        std::thread::spawn(move || {
            let _ = tx.send((id, opsdata::docker(op)));
        });
        self.version += 1;
    }
    fn scroll_logs(&mut self, older: bool) {
        let max = self.logs.len().saturating_sub(LOG_ROWS);
        if older {
            self.log_off = (self.log_off + LOG_STEP).min(max);
            if self.log_off > 0 {
                self.follow = false;
            }
        } else {
            self.log_off = self.log_off.saturating_sub(LOG_STEP);
            if self.log_off == 0 {
                self.follow = true;
                self.logs_dirty = true;
            }
        }
        self.version += 1;
    }
}

#[derive(Resource)]
pub struct ContainersRoots(Workspace);

pub fn setup(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut images: ResMut<Assets<Image>>, (home, mut atlases): (Res<PanelAtlas>, ResMut<AppAtlases>), mut pmats: ResMut<Assets<PanelMaterial>>, args: Res<crate::Args>, mut app: ResMut<ContainersApp>) {
    if args.open_app.as_deref() == Some("containers") {
        app.open();
    }
    let ws = spawn_workspace(&mut commands, &mut meshes, &mut images, &home, &mut atlases, &mut pmats, APP, &PANES);
    commands.insert_resource(ContainersRoots(ws));
}

#[allow(clippy::too_many_arguments)]
/// The key the wheel presses over a pane: the map and the summary step containers, the logs scroll.
fn scroll_key(pane: Option<u8>, down: bool) -> Option<KeyCode> {
    match pane? as usize {
        MAP | SUM => Some(if down { KeyCode::KeyJ } else { KeyCode::KeyK }),
        LOGS => Some(if down { KeyCode::BracketRight } else { KeyCode::BracketLeft }),
        _ => None,
    }
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
        if e.state != bevy::input::ButtonState::Pressed || mods || bar_busy || focus.terminal || moment.shown || (e.repeat && !matches!(e.key_code, KeyCode::PageUp | KeyCode::PageDown | KeyCode::BracketLeft | KeyCode::BracketRight)) {
            continue;
        }
        codes.push(e.key_code);
    }
    codes.extend(crate::appframe::scroll_keys(scrolls.take(APP), scroll_key));
    // clicks on the panes: a button presses its key, a pill picks its container
    for c in clicks.take(2) {
        match c {
            Click::Key(k) => codes.push(k),
            Click::Pick(i) => app.select(i),
            Click::Alt(_) => {}
        }
    }
    for key in codes {
        let n = app.all().len();
        let i = app.sel();
        let sel = app.selected().cloned();
        match key {
            KeyCode::Escape => {
                app.close();
                sound.play_with("collapse", 0.7, 0.0);
            }
            KeyCode::ArrowRight | KeyCode::KeyJ | KeyCode::ArrowDown if n > 0 => {
                app.select((i + 1) % n);
                sound.play_q("tick", 40);
            }
            KeyCode::ArrowLeft | KeyCode::KeyK | KeyCode::ArrowUp if n > 0 => {
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
            KeyCode::BracketLeft | KeyCode::PageUp => app.scroll_logs(true),
            KeyCode::BracketRight | KeyCode::PageDown => app.scroll_logs(false),
            KeyCode::KeyF => {
                app.follow = !app.follow;
                if app.follow {
                    app.log_off = 0;
                    app.logs_dirty = true;
                }
                app.version += 1;
                sound.play("toggle");
            }
            KeyCode::KeyS | KeyCode::KeyR | KeyCode::KeyP | KeyCode::KeyD => {
                let Some(c) = sel else { continue };
                if app.working.iter().any(|w| w.2 == c.name) {
                    toasts.push_quiet(format!("Still working on {}", c.name));
                    continue;
                }
                let now = sim.t;
                match key {
                    KeyCode::KeyS => {
                        let running = c.running();
                        toasts.push_quiet(format!("{} {}…", if running { "Stopping" } else { "Starting" }, c.name));
                        app.act(now, format!("{} {}", if running { "stopping" } else { "starting" }, c.name), &c.name, if running { DockerOp::Stop(c.name.clone()) } else { DockerOp::Start(c.name.clone()) });
                        sound.play(if running { "discard" } else { "confirm" });
                    }
                    KeyCode::KeyR => {
                        toasts.push_quiet(format!("Restarting {}…", c.name));
                        app.act(now, format!("restarting {}", c.name), &c.name, DockerOp::Restart(c.name.clone()));
                        sound.play("progress");
                    }
                    KeyCode::KeyP => {
                        toasts.push_quiet(format!("Pulling {}…", c.image));
                        app.act(now, format!("pulling {}", c.image), &c.name, DockerOp::Pull(c.image.clone()));
                        sound.play("progress");
                    }
                    _ => {
                        if app.armed.as_ref().is_some_and(|(t, id)| sim.t < *t && *id == c.id) {
                            app.armed = None;
                            app.act(now, format!("removing {}", c.name), &c.name, DockerOp::Remove(c.name.clone()));
                            sound.play("discard");
                        } else {
                            app.armed = Some((sim.t + 3.0, c.id.clone()));
                            toasts.push_quiet(format!("Press D again to remove {}", c.name));
                            sound.play("toggle");
                        }
                        app.version += 1;
                    }
                }
            }
            _ => {}
        }
    }
}

pub fn update(sim: Res<crate::core::Sim>, mut atlases: ResMut<AppAtlases>, mut app: ResMut<ContainersApp>, mut ctl: ResMut<CoreCtl>, mut toasts: ResMut<crate::live::Toasts>, mut sound: ResMut<crate::sound::Sound>) {
    let open = app.open;
    step_open(open, &mut app.amt, sim.dt, &mut ctl);
    atlases.want(APP, open || app.amt.x > 0.001);
    if app.armed.as_ref().is_some_and(|(t, _)| sim.t > *t) {
        app.armed = None;
        app.version += 1;
    }
    // the clock in "pulling … 12 s"
    let tk = sim.t as u32;
    if !app.working.is_empty() && tk != app.tick {
        app.version += 1;
    }
    app.tick = tk;
    let notes: Vec<Note> = app.notes.1.lock().map(|r| r.try_iter().collect()).unwrap_or_default();
    for (id, res) in notes {
        app.working.retain(|w| w.0 != id);
        match res {
            Ok(s) => toasts.push_quiet(s),
            Err(e) => {
                toasts.push(clip(&e, 110));
                sound.play("error");
            }
        }
        // look again now (after the one in flight, if any), and re-read the logs
        if app.loading {
            app.reload = true;
        } else {
            app.load_t = -99.0;
        }
        app.logs_dirty = true;
        app.version += 1;
    }
    let got: Vec<(bool, Listing)> = app.rx.as_ref().and_then(|r| r.lock().ok().map(|r| r.try_iter().collect())).unwrap_or_default();
    for (done, list) in got {
        if done {
            app.loading = false;
            if std::mem::take(&mut app.reload) {
                app.load_t = -99.0;
            }
            if let Ok(v) = &list {
                for c in v.iter().filter(|c| c.running() && !c.mem.is_empty()) {
                    let h = app.hist.entry(c.id.clone()).or_default();
                    h.push(c.cpu);
                    if h.len() > 24 {
                        h.remove(0);
                    }
                }
                app.hist.retain(|id, _| v.iter().any(|c| &c.id == id));
            }
        }
        app.list = Some(list);
        if let Some(w) = app.want.take() {
            if let Some(i) = app.all().iter().position(|c| c.name == w) {
                app.select(i);
            }
        }
        // pin the selection to a container (the first, until one is chosen)
        let i = app.sel();
        app.select(i);
        app.version += 1;
    }
    let got: Vec<LogsMsg> = app.logs_rx.as_ref().and_then(|r| r.lock().ok().map(|r| r.try_iter().collect())).unwrap_or_default();
    for (id, res) in got {
        app.logs_busy = false;
        if app.selected().map(|c| &c.id) != Some(&id) {
            continue;
        }
        app.logs_for = id;
        match res {
            Ok(l) => {
                app.logs_err = None;
                if l != app.logs {
                    app.logs = l;
                    app.log_off = app.log_off.min(app.logs.len().saturating_sub(LOG_ROWS));
                }
            }
            Err(e) => app.logs_err = Some(e),
        }
        app.version += 1;
    }
    if !app.open {
        return;
    }
    if !app.loading && sim.t - app.load_t > 4.0 {
        app.load_t = sim.t;
        app.loading = true;
        let first = app.list.as_ref().is_none_or(|l| l.is_err());
        let (tx, rx) = mpsc::channel();
        app.rx = Some(Mutex::new(rx));
        std::thread::spawn(move || {
            // the names first, then the figures (docker stats takes a second or two)
            if first {
                if let Ok(v) = opsdata::containers_quick() {
                    let _ = tx.send((false, Ok(v)));
                }
            }
            let _ = tx.send((true, opsdata::containers()));
        });
    }
    if let Some((id, running)) = app.selected().map(|c| (c.id.clone(), c.running())) {
        let due = app.logs_dirty || (app.follow && running && sim.t - app.logs_t > 2.0);
        if due && !app.logs_busy {
            app.logs_t = sim.t;
            app.logs_dirty = false;
            app.logs_busy = true;
            let (tx, rx) = mpsc::channel();
            app.logs_rx = Some(Mutex::new(rx));
            std::thread::spawn(move || {
                let _ = tx.send((id.clone(), opsdata::logs(&id, LOG_KEEP)));
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

/// A small framed key chip in a pane header (the key is pressed when it's clicked).
pub fn chip(c: &mut Commands, ui: &Ui, label: &str, key: KeyCode) -> Entity {
    let e = c.spawn((Node { padding: UiRect { left: px(6), right: px(6), top: px(2), bottom: px(1) }, border: UiRect::all(px(1)), ..default() }, BorderColor::all(with_alpha(ui.line, 0.45)), Click::Key(key))).id();
    let t = ui.text(c, label, &ui.f.ui700, 9.0, 1.2, ui.hot);
    c.entity(e).add_child(t);
    e
}

/// "12.3%" under 10, "42%" above.
fn pct(v: f32) -> String {
    if v < 10.0 {
        format!("{v:.1}%")
    } else {
        format!("{v:.0}%")
    }
}

pub fn draw(mut commands: Commands, app: Res<ContainersApp>, svc: Res<Service>, roots: Option<Res<ContainersRoots>>, f: Res<UiFonts>, theme: Res<crate::theme::Theme>, mut seen: Local<(u64, u64, u64)>) {
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
    let up = all.iter().filter(|c| c.running()).count();
    let bad = all.iter().filter(|c| c.unhealthy()).count();
    let (l2, l2c) = match &app.list {
        None => ("asking docker…".to_string(), ui.line),
        Some(Err(e)) => (docker_headline(e).to_lowercase(), role_color(Role::Bad)),
        _ if !app.working.is_empty() => (app.working[0].1.clone(), role_color(Role::Acc)),
        _ if bad > 0 => (format!("{bad} unhealthy"), role_color(Role::Bad)),
        _ => (format!("{up} of {} running", all.len()), ui.line),
    };
    ui.corelab(c, ws.corelab, "Containers", clip(&l2, 34), l2c);
    ui.top(c, ws.panes[TOP], "Containers", format!("{} CONTAINERS · {up} UP", all.len()), "container model and actions after oxker (MIT) · docker");
    ui.strip(c, ws.panes[STRIP], &agents, &merged, ["J K / ↑ ↓ SELECT   , . PAGE   S START / STOP   R RESTART   P PULL", "D REMOVE   F FOLLOW   [ ] SCROLL LOGS   ESC CLOSE"]);
    // summary
    let r = c.spawn(Node { width: percent(100), height: percent(100), flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(22), padding: UiRect::horizontal(px(16)), ..default() }).id();
    c.entity(ws.panes[SUM]).add_child(r);
    let cpu: f32 = all.iter().map(|c| c.cpu).sum::<f32>().max(0.0);
    let mem: f64 = all.iter().map(|c| c.mem_used).sum();
    for (v, k) in [(format!("{cpu:.1}%"), "cpu"), (opsdata::fmt_bytes(mem), "memory"), (format!("{up} of {}", all.len()), "running")] {
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
        None => message(c, &ui, map, "Asking Docker…", "", ui.line),
        Some(Err(e)) => {
            let (head, hint) = opsdata::docker_problem(e);
            message(c, &ui, map, &head, &hint, role_color(Role::Bad));
        }
        Some(Ok(_)) => draw_map(c, &ui, map, &app),
    }
    // detail and logs
    match (app.selected(), &app.list) {
        (Some(sel), _) => {
            draw_detail(c, &ui, ws.panes[DETAIL], sel, app.hist.get(&sel.id).map(|v| v.as_slice()).unwrap_or(&[]), app.armed.is_some(), app.working.iter().find(|w| w.2 == sel.name).map(|w| (w.1.as_str(), (app.tick as f32 - w.3).max(0.0) as u32)));
            draw_logs(c, &ui, ws.panes[LOGS], sel, &app);
        }
        (None, list) => {
            let (note, col) = match list {
                None => ("Waiting for Docker", ui.dim),
                Some(Err(_)) => ("Docker is not available", role_color(Role::Bad)),
                Some(Ok(_)) => ("No containers yet", ui.dim),
            };
            for (pane, title) in [(DETAIL, "Container"), (LOGS, "Logs")] {
                let b = ui.body(c, ws.panes[pane]);
                let h = ui.header(c, title, "", ui.dim);
                let t = c.spawn((Text::new(note), font(&f.ui500, 12.5, 0.4), TextColor(col), Node { margin: UiRect::all(px(16)), ..default() })).id();
                c.entity(b).add_children(&[h, t]);
            }
        }
    }
}

fn docker_headline(e: &str) -> String {
    opsdata::docker_problem(e).0
}

/// A headline and a hint on the map (loading, or Docker is unavailable).
pub fn message(c: &mut Commands, ui: &Ui, map: Entity, head: &str, hint: &str, col: Color) {
    let b = c.spawn(Node { position_type: PositionType::Absolute, left: px(24), top: px(24), flex_direction: FlexDirection::Column, row_gap: px(8), ..default() }).id();
    let h = ui.text(c, head, &ui.f.ui600, 15.0, 0.6, col);
    c.entity(b).add_child(h);
    if !hint.is_empty() {
        let t = ui.text(c, clip(hint, 90), &ui.f.ui500, 12.5, 0.4, ui.line);
        c.entity(b).add_child(t);
    }
    c.entity(map).add_child(b);
}

fn draw_map(c: &mut Commands, ui: &Ui, map: Entity, app: &ContainersApp) {
    let shown: Vec<(usize, &Container)> = app.shown().collect();
    let sel = app.sel();
    let (cx, cy, rx, ry) = (430.0, 165.0, 330.0, 118.0);
    let m = shown.len().max(1);
    let angle = |k: usize| std::f32::consts::TAU * k as f32 / m as f32 - std::f32::consts::FRAC_PI_2;
    // connectors from the hub to each container (the selected one dashed and bright), under everything
    for (k, (i, ct)) in shown.iter().enumerate() {
        let a = angle(k);
        let to = Vec2::new(cx + rx * a.cos(), cy + ry * a.sin());
        let from = Vec2::new(cx, cy) + (to - Vec2::new(cx, cy)).normalize_or_zero() * 44.0;
        if *i == sel {
            ui.dashed(c, map, from, to, 1.5, with_alpha(state_col(ct), 0.9), 5.0, 4.0);
        } else {
            ui.line(c, map, from, to, 1.0, with_alpha(ui.line, 0.22));
        }
    }
    // the hub
    let hub = c
        .spawn((
            Node { position_type: PositionType::Absolute, left: px(cx - 42.0), top: px(cy - 42.0), width: px(84), height: px(84), border: UiRect::all(px(1)), border_radius: BorderRadius::MAX, flex_direction: FlexDirection::Column, align_items: AlignItems::Center, justify_content: JustifyContent::Center, ..default() },
            BorderColor::all(with_alpha(ui.line, 0.5)),
        ))
        .id();
    let n = ui.text(c, format!("{}", app.all().len()), &ui.f.display, 18.0, 0.5, ui.hot);
    let d = ui.text(c, "DOCKER", &ui.f.ui600, 9.0, 2.4, ui.dim);
    c.entity(hub).add_children(&[n, d]);
    let ring = c.spawn((Node { position_type: PositionType::Absolute, left: px(cx - rx), top: px(cy - ry), width: px(rx * 2.0), height: px(ry * 2.0), border: UiRect::all(px(1)), border_radius: BorderRadius::MAX, ..default() }, BorderColor::all(with_alpha(ui.line, 0.16)))).id();
    c.entity(map).add_children(&[ring, hub]);
    for (k, (i, ct)) in shown.iter().enumerate() {
        let a = angle(k);
        let (x, y) = (cx + rx * a.cos(), cy + ry * a.sin());
        let on = *i == sel;
        let col = state_col(ct);
        let (w, h) = (172.0, 56.0);
        let pill = c
            .spawn((
                Node { position_type: PositionType::Absolute, left: px(x - w / 2.0), top: px(y - h / 2.0), width: px(w), height: px(h), flex_direction: FlexDirection::Column, justify_content: JustifyContent::Center, row_gap: px(3), padding: UiRect::horizontal(px(14)), border: UiRect::all(px(if on { 2.0 } else { 1.0 })), border_radius: BorderRadius::all(px(28)), overflow: Overflow::clip(), ..default() },
                BorderColor::all(if on { col } else { with_alpha(ui.line, 0.4) }),
                BackgroundColor(Color::srgba(0.0, 0.0, 0.0, if on { 0.6 } else { 0.4 })),
                Click::Pick(*i),
            ))
            .id();
        let top = ui.row(c, 7.0);
        let dot = c.spawn((Node { width: px(7), height: px(7), flex_shrink: 0.0, border_radius: BorderRadius::MAX, ..default() }, BackgroundColor(col))).id();
        let nm = ui.text(c, clip(if ct.service.is_empty() { &ct.name } else { &ct.service }, 17), &ui.f.ui600, 13.0, 0.4, ui.hot);
        c.entity(top).add_children(&[dot, nm]);
        let bot = ui.row(c, 8.0);
        // which compose project it belongs to, or its state when it has none
        let st = ui.text(c, if ct.project.is_empty() { state_word(ct) } else { clip(&ct.project.to_uppercase(), 7) }, &ui.f.ui700, 8.5, 1.6, if ct.unhealthy() { col } else { ui.dim });
        let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
        let mm = ui.text(c, if ct.running() && !ct.mem.is_empty() { format!("{} · {}", pct(ct.cpu), opsdata::fmt_bytes(ct.mem_used)) } else if ct.running() { "…".to_string() } else { ct.state.to_uppercase() }, &ui.f.mono, 10.5, 0.0, ui.line);
        c.entity(bot).add_children(&[st, sp, mm]);
        c.entity(pill).add_children(&[top, bot]);
        c.entity(map).add_child(pill);
    }
    if shown.is_empty() {
        let t = ui.text(c, "No containers", &ui.f.ui600, 14.0, 0.5, ui.line);
        let h = ui.text(c, "Start one with `docker run` or `docker compose up`", &ui.f.ui500, 12.0, 0.4, ui.dim);
        let b = c.spawn(Node { position_type: PositionType::Absolute, left: px(cx - 170.0), top: px(cy + 62.0), width: px(340), flex_direction: FlexDirection::Column, align_items: AlignItems::Center, row_gap: px(6), ..default() }).id();
        c.entity(b).add_children(&[t, h]);
        c.entity(map).add_child(b);
    }
    // more than a page: which one, and the keys that turn it
    let pages = app.pages();
    if pages > 1 {
        let page = sel / PAGE;
        let bar = c.spawn(Node { position_type: PositionType::Absolute, left: px(14), top: px(12), flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(8), ..default() }).id();
        let lo = shown.first().map(|s| s.0 + 1).unwrap_or(0);
        let hi = shown.last().map(|s| s.0 + 1).unwrap_or(0);
        let prev = chip(c, ui, "‹ ,", KeyCode::Comma);
        let t = ui.text(c, format!("{lo}–{hi} OF {} · PAGE {} / {pages}", app.all().len(), page + 1), &ui.f.ui600, 9.5, 1.4, ui.dim);
        let next = chip(c, ui, ". ›", KeyCode::Period);
        c.entity(bar).add_children(&[prev, t, next]);
        c.entity(map).add_child(bar);
    }
}

/// Seconds as "12 s" or "3 m 05 s".
fn secs(s: u32) -> String {
    if s < 60 {
        format!("{s} s")
    } else {
        format!("{} m {:02} s", s / 60, s % 60)
    }
}

fn draw_detail(c: &mut Commands, ui: &Ui, root: Entity, ct: &Container, hist: &[f32], armed: bool, working: Option<(&str, u32)>) {
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
        ("compose", if ct.project.is_empty() { "-".into() } else { clip(&format!("{} · {}", ct.project, ct.service), 30) }),
        ("status", clip(&ct.status, 30)),
        ("health", ct.health.clone().unwrap_or_else(|| "no check".into())),
    ];
    for (k, v) in rows {
        let r = ui.kv(c, k, v, ui.line);
        c.entity(inner).add_child(r);
    }
    let running = ct.running();
    let max = hist.iter().cloned().fold(5.0, f32::max);
    let vals: Vec<f32> = hist.iter().map(|v| v / max).collect();
    let lab = ui.text(c, "CPU · LAST 24 SAMPLES", &ui.f.ui600, 9.5, 1.8, ui.dim);
    let sp = ui.spark(c, &vals, 34.0, ui.line);
    c.entity(inner).add_children(&[lab, sp]);
    let mem_txt = if !running {
        "-".to_string()
    } else if ct.mem.is_empty() {
        "...".to_string()
    } else {
        let limit = ct.mem.split(" / ").nth(1).map(opsdata::parse_bytes).unwrap_or(0.0);
        if limit > 0.0 { format!("{} / {}", opsdata::fmt_bytes(ct.mem_used), opsdata::fmt_bytes(limit)) } else { opsdata::fmt_bytes(ct.mem_used) }
    };
    for (k, v, frac) in [("CPU", if running { format!("{:.1}%", ct.cpu) } else { "-".into() }, ct.cpu / 100.0), ("MEM", mem_txt, ct.mem_frac)] {
        let row = ui.row(c, 10.0);
        let kt = ui.text(c, k, &ui.f.ui600, 9.5, 1.8, ui.dim);
        let bar = ui.bar(c, if running { frac } else { 0.0 }, role_color(Role::Fill), 150.0);
        let vt = ui.text(c, v, &ui.f.mono, 11.0, 0.0, ui.line);
        c.entity(row).add_children(&[kt, bar, vt]);
        c.entity(inner).add_child(row);
    }
    // a command in flight takes the buttons' place, so nothing is pressed twice
    if let Some((what, s)) = working {
        let w = ui.text(c, format!("{} · {}", clip(&what.to_uppercase(), 34), secs(s)), &ui.f.ui700, 10.5, 1.4, role_color(Role::Acc));
        let w = c.spawn(Node { margin: UiRect::top(px(14)), height: px(28), align_items: AlignItems::Center, ..default() }).add_child(w).id();
        c.entity(inner).add_child(w);
        return;
    }
    let btns = c.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(8), margin: UiRect::top(px(8)), ..default() }).id();
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

fn log_level(msg: &str) -> &'static str {
    let lower = msg.to_ascii_lowercase();
    if lower.contains("error") || lower.contains("fatal") || lower.contains("panic") || lower.contains("exception") {
        "ERROR"
    } else if lower.contains("warn") {
        "WARN"
    } else {
        "INFO"
    }
}

fn draw_logs(c: &mut Commands, ui: &Ui, root: Entity, ct: &Container, app: &ContainersApp) {
    let b = ui.body(c, root);
    let logs = &app.logs;
    let end = logs.len().saturating_sub(app.log_off);
    let start = end.saturating_sub(LOG_ROWS);
    let state = if app.log_off > 0 { format!("lines {}–{} of {} · F to follow", start + 1, end, logs.len()) } else if app.follow { "following".to_string() } else { "paused (F)".to_string() };
    let h = ui.header(c, "Logs", &format!("{} · {state}", clip(&ct.name, 30)), ui.dim);
    let (up, down) = (chip(c, ui, "▲ OLDER [", KeyCode::BracketLeft), chip(c, ui, "▼ NEWER ]", KeyCode::BracketRight));
    c.entity(h).insert(Click::Key(KeyCode::KeyF)).add_children(&[up, down]);
    let list = c.spawn(Node { flex_direction: FlexDirection::Column, padding: UiRect { left: px(16), right: px(16), top: px(6), ..default() }, ..default() }).id();
    c.entity(b).add_children(&[h, list]);
    if logs.is_empty() || app.logs_for != ct.id {
        let (t, col) = match &app.logs_err {
            Some(e) => (format!("Can't read the logs: {}", clip(e, 90)), role_color(Role::Bad)),
            None if app.logs_for != ct.id => ("Reading logs…".to_string(), ui.dim),
            None if ct.running() => ("No output yet".to_string(), ui.dim),
            None => ("No output. The container isn't running".to_string(), ui.dim),
        };
        let t = ui.text(c, t, &ui.f.ui500, 12.5, 0.4, col);
        c.entity(list).add_child(t);
        return;
    }
    for l in &logs[start..end] {
        // "2026-10-17T07:10:00.123456789Z message"
        let (ts, msg) = l.split_once(' ').unwrap_or(("", l));
        let time = opsdata::local_hms(ts);
        let lvl = log_level(msg);
        let col = match lvl {
            "ERROR" => role_color(Role::Bad),
            "WARN" => role_color(Role::Acc),
            _ => ui.line,
        };
        let row = c.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(14), width: percent(100), min_width: px(0), height: px(16), overflow: Overflow::clip(), ..default() }).id();
        let t = c.spawn((Text::new(time), font(&ui.f.mono, 11.0, 0.0), TextColor(ui.dim), Node { width: px(86), flex_shrink: 0.0, ..default() })).id();
        let lv = c.spawn((Text::new(lvl), font(&ui.f.mono, 11.0, 0.0), TextColor(col), Node { width: px(42), flex_shrink: 0.0, ..default() })).id();
        let m = ui.text(c, clip(msg.trim(), 150), &ui.f.mono, 11.5, 0.0, if lvl == "INFO" { ui.line } else { col });
        c.entity(m).insert(Node { flex_shrink: 1.0, min_width: px(0), overflow: Overflow::clip(), ..default() });
        c.entity(row).add_children(&[t, lv, m]);
        c.entity(list).add_child(row);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wheel_keys_per_pane() {
        assert_eq!(scroll_key(Some(MAP as u8), true), Some(KeyCode::KeyJ));
        assert_eq!(scroll_key(Some(SUM as u8), false), Some(KeyCode::KeyK));
        assert_eq!(scroll_key(Some(LOGS as u8), true), Some(KeyCode::BracketRight));
        assert_eq!(scroll_key(Some(LOGS as u8), false), Some(KeyCode::BracketLeft));
        assert_eq!(scroll_key(Some(DETAIL as u8), true), None);
    }

    fn mk(id: &str, running: bool) -> Container {
        Container { id: id.into(), name: format!("n-{id}"), state: if running { "running".into() } else { "exited".into() }, ..Default::default() }
    }

    fn app_with(n: usize) -> ContainersApp {
        ContainersApp { list: Some(Ok((0..n).map(|i| mk(&format!("c{i:02}"), true)).collect())), ..Default::default() }
    }

    #[test]
    fn pages_follow_the_selection() {
        let mut a = app_with(23);
        assert_eq!((a.pages(), a.shown().count()), (3, 10));
        a.select(12);
        assert_eq!(a.shown().map(|s| s.0).collect::<Vec<_>>(), (10..20).collect::<Vec<_>>());
        a.select(22);
        assert_eq!(a.shown().map(|s| s.0).collect::<Vec<_>>(), vec![20, 21, 22]);
        assert_eq!(a.selected().unwrap().id, "c22");
        assert_eq!(app_with(0).pages(), 1);
    }

    #[test]
    fn selection_stays_on_its_container_when_the_list_changes() {
        let mut a = app_with(5);
        a.select(3);
        // one before it goes away: the same container, a new index
        if let Some(Ok(v)) = &mut a.list {
            v.remove(0);
        }
        assert_eq!((a.selected().unwrap().id.as_str(), a.sel()), ("c03", 2));
        // the selected one goes away: back to the first
        if let Some(Ok(v)) = &mut a.list {
            v.retain(|c| c.id != "c03");
        }
        assert_eq!(a.sel(), 0);
    }

    #[test]
    fn scrolling_logs_pauses_and_resumes_following() {
        let mut a = app_with(1);
        a.logs = (0..40).map(|i| i.to_string()).collect();
        a.scroll_logs(false);
        assert_eq!((a.log_off, a.follow), (0, true));
        a.scroll_logs(true);
        assert_eq!((a.log_off, a.follow), (LOG_STEP, false));
        for _ in 0..10 {
            a.scroll_logs(true);
        }
        assert_eq!(a.log_off, 40 - LOG_ROWS);
        for _ in 0..10 {
            a.scroll_logs(false);
        }
        assert_eq!((a.log_off, a.follow, a.logs_dirty), (0, true, true));
    }

    #[test]
    fn only_one_command_per_container_at_a_time() {
        let mut a = app_with(1);
        a.act(0.0, "pulling x".into(), "n-c00", DockerOp::Pull("definitely/not-an-image:0".into()));
        a.act(0.1, "stopping".into(), "n-c00", DockerOp::Stop("n-c00".into()));
        assert_eq!(a.working.len(), 1);
    }

    #[test]
    fn reads_log_levels_and_durations() {
        assert_eq!(log_level("GET /health 200"), "INFO");
        assert_eq!(log_level("s3: Connect refused (WARN)"), "WARN");
        assert_eq!(log_level("upload failed: Error 12"), "ERROR");
        assert_eq!((secs(7), secs(125)), ("7 s".to_string(), "2 m 05 s".to_string()));
        assert_eq!((pct(0.04), pct(42.4)), ("0.0%".to_string(), "42%".to_string()));
    }
}
