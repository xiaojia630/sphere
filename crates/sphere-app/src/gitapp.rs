//! The Git app: your repos and their state at a glance, a commit graph across branches, the
//! working tree with staging, a commit box, branches and the remote, over the agents' strip. Everything is read
//! and done with git in the background. Keys: [ ] repo, Tab the list J K move in (files, branches, graph),
//! Enter stage or unstage the file / switch to the branch, S stage, U unstage, A stage all, X unstage all,
//! C commit (with M toggling amend), B switch branch, F fetch, L pull, P push, V graph/list, R pull requests
//! (then J K pick one, Enter or O opens it on GitHub, C checks it out, M twice merges it), Esc closes.
use crate::appframe::{clip, Click, Ui};
use crate::gitdata::clip_start;
use crate::cmdbar::{BarTarget, CmdBar};
use crate::core::CoreCtl;
use crate::fonts::UiFonts;
use crate::gitdata::{self, Change, Op, Repo, RepoCard};
use crate::panel::*;
use crate::prs::{self, Detail, Pr, PrList, Tone};
use crate::service::Service;
use crate::util::*;
use bevy::prelude::*;
use bevy::text::LineHeight;
use sphere_proto::{TaskState, TaskView};
use std::path::PathBuf;
use std::sync::{mpsc, Mutex};

const WS_W: f32 = 1280.0;
const WS_H: f32 = 700.0;
const WS_CX: f32 = 803.0;
const WS_CY: f32 = 552.0;
const K: f32 = 1.1;
/// Repo cards on screen, and repos kept in all.
const REPOS: usize = 3;
const REPOS_MAX: usize = 12;
/// A click on a branch row, a graph row, a stage box (in `Pick`/`Alt`) is numbered from these.
const BRANCH_PICK: usize = 1000;
const GRAPH_PICK: usize = 2000;
const STAGE_ALT: usize = 1000;
const PR_PICK: usize = 3000;
const PR_ROWS: usize = 7;
/// Pull requests are read again this often while the app is open.
const PR_EVERY: f32 = 30.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pane {
    Repo(u8),
    Branches,
    Graph,
    Commit,
    Status,
    Remote,
    Strip,
    Top,
}

impl Pane {
    fn all() -> Vec<Pane> {
        let mut v: Vec<Pane> = (0..REPOS as u8).map(Pane::Repo).collect();
        v.extend([Pane::Branches, Pane::Graph, Pane::Commit, Pane::Status, Pane::Remote, Pane::Strip, Pane::Top]);
        v
    }
    fn rect(self) -> [f32; 5] {
        match self {
            Pane::Repo(i) => [14.0, 42.0 + 88.0 * i as f32, 240.0, 78.0, 30.0 + 6.0 * i as f32],
            Pane::Branches => [14.0, 314.0, 240.0, 312.0, 26.0],
            Pane::Graph => [266.0, 42.0, 700.0, 346.0, 16.0],
            Pane::Commit => [266.0, 400.0, 700.0, 226.0, 38.0],
            Pane::Status => [978.0, 42.0, 288.0, 432.0, 44.0],
            Pane::Remote => [978.0, 486.0, 288.0, 140.0, 28.0],
            Pane::Strip => [14.0, 638.0, 1252.0, 50.0, 22.0],
            Pane::Top => [22.0, 2.0, 1236.0, 32.0, 8.0],
        }
    }
}

const GRAPH_ROWS: usize = 14;
const FILE_ROWS: usize = 5;
const BRANCH_ROWS: usize = 6;

/// (the repos, the selected one, its state, the cards)
type Loaded = (Vec<PathBuf>, usize, Repo, Vec<RepoCard>);
type Preview = Vec<(char, String)>;
type Note = Result<String, String>;

/// Which list J and K move in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Zone {
    Files,
    Branches,
    Graph,
    Prs,
}

/// The first repo card shown, so the selected repo is among the REPOS on screen.
fn window_first(sel: usize, len: usize) -> usize {
    sel.saturating_sub(REPOS - 1).min(len.saturating_sub(REPOS))
}

/// The first of `shown` rows to draw so the cursor row is in view.
fn first_row(cursor: usize, len: usize, shown: usize) -> usize {
    cursor.saturating_sub(shown - 1).min(len.saturating_sub(shown))
}

#[derive(Resource)]
pub struct GitApp {
    pub open: bool,
    pub amt: Spring,
    repos: Vec<PathBuf>,
    sel: usize,
    repo: Option<Repo>,
    cards: Vec<RepoCard>,
    loading: bool,
    /// look for repos again on the next read (each open), keeping what is on screen until it's in
    rediscover: bool,
    load_rx: Option<Mutex<mpsc::Receiver<Loaded>>>,
    load_t: f32,
    /// the selected change: staged first, then unstaged
    cursor: usize,
    zone: Zone,
    /// the selected branch and graph row
    bcur: usize,
    gcur: usize,
    /// reads so far (the preview is read again after each)
    loads: u64,
    /// look tests: SPHERE_GIT_KEYS played once the first repo is in
    pending: Vec<KeyCode>,
    preview: Vec<(char, String)>,
    preview_rx: Option<Mutex<mpsc::Receiver<Preview>>>,
    preview_for: String,
    amend: bool,
    /// the graph: a list instead of the tilted view, and how far the view is turned (degrees)
    flat: bool,
    yaw: f32,
    busy: bool,
    /// the graph and commit panes show the repo's pull requests instead (R)
    prs_view: bool,
    prs: Option<PrList>,
    prs_for: Option<PathBuf>,
    prs_t: f32,
    prs_rx: Option<Mutex<mpsc::Receiver<(PathBuf, PrList)>>>,
    pcur: usize,
    detail: Option<Detail>,
    detail_for: String,
    detail_rx: Option<Mutex<mpsc::Receiver<Detail>>>,
    /// an armed merge: (PR number, when M was first pressed)
    arm: Option<(u64, f32)>,
    notes: (mpsc::Sender<Note>, Mutex<mpsc::Receiver<Note>>),
    version: u64,
}

impl Default for GitApp {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        GitApp {
            open: false,
            amt: Spring::new(0.0, 26.0, 9.0),
            repos: Vec::new(),
            sel: 0,
            repo: None,
            cards: Vec::new(),
            loading: false,
            rediscover: false,
            load_rx: None,
            load_t: -99.0,
            cursor: 0,
            zone: Zone::Files,
            bcur: 0,
            gcur: 0,
            loads: 0,
            pending: Vec::new(),
            preview: Vec::new(),
            preview_rx: None,
            preview_for: String::new(),
            amend: false,
            flat: false,
            yaw: 22.0,
            busy: false,
            prs_view: false,
            prs: None,
            prs_for: None,
            prs_t: -99.0,
            prs_rx: None,
            pcur: 0,
            detail: None,
            detail_for: String::new(),
            detail_rx: None,
            arm: None,
            notes: (tx, Mutex::new(rx)),
            version: 0,
        }
    }
}

impl GitApp {
    /// "main · 3 changed" for the repo on screen, if one is loaded (the Orbit tile's caption).
    pub fn caption(&self) -> Option<String> {
        let c = &self.repo.as_ref()?.card;
        Some(format!("{} · {}", c.branch, if c.changed == 0 { "clean".to_string() } else { format!("{} changed", c.changed) }))
    }
}

impl GitApp {
    pub fn open(&mut self) {
        self.open = true;
        self.load_t = -99.0;
        self.prs_t = -99.0;
        // look for repos again (agents may have started in new ones), but keep showing the last read until the
        // new one is in, so the app opens on its content instead of "Reading…"
        self.rediscover = true;
        self.version += 1;
    }
    /// Selects repo `i`.
    fn select_repo(&mut self, i: usize) {
        if i < self.repos.len() && i != self.sel {
            self.sel = i;
            self.repo = None;
            self.cursor = 0;
            self.bcur = 0;
            self.gcur = 0;
            self.zone = Zone::Files;
            self.preview.clear();
            self.load_t = -99.0;
            self.prs_t = -99.0;
            self.version += 1;
        }
    }
    /// Rows the cursor of `zone` moves over.
    fn len_of(&self, zone: Zone) -> usize {
        match (&self.repo, zone) {
            (Some(r), Zone::Files) => r.staged.len() + r.unstaged.len(),
            (Some(r), Zone::Branches) => r.branches.len(),
            (Some(r), Zone::Graph) => r.graph.len(),
            (_, Zone::Prs) => self.pr_list().len(),
            _ => 0,
        }
    }
    /// Moves the cursor of the zone J K work in.
    fn step(&mut self, d: i32) {
        let n = self.len_of(self.zone);
        if n == 0 {
            return;
        }
        match self.zone {
            Zone::Files => self.cursor = (self.cursor as i32 + d).clamp(0, n as i32 - 1) as usize,
            Zone::Branches => self.bcur = (self.bcur as i32 + d).clamp(0, n as i32 - 1) as usize,
            Zone::Prs => self.pcur = (self.pcur as i32 + d).clamp(0, n as i32 - 1) as usize,
            Zone::Graph => {
                // connector rows between commits have nothing to select
                let Some(r) = &self.repo else { return };
                let mut i = self.gcur as i32;
                loop {
                    i += d.signum();
                    if !(0..n as i32).contains(&i) {
                        return;
                    }
                    if !r.graph[i as usize].hash.is_empty() {
                        break;
                    }
                }
                self.gcur = i as usize;
            }
        }
        self.version += 1;
    }
    /// The zones Tab goes through.
    fn next_zone(&mut self) {
        let flat = self.flat;
        self.zone = match self.zone {
            Zone::Files => Zone::Branches,
            Zone::Branches if self.prs_view => Zone::Prs,
            Zone::Branches if flat => Zone::Graph,
            _ => Zone::Files,
        };
        self.version += 1;
    }
    pub fn close(&mut self) {
        self.open = false;
        self.version += 1;
    }
    fn pr_list(&self) -> &[Pr] {
        match &self.prs {
            Some(PrList::Prs(v)) => v,
            _ => &[],
        }
    }
    fn pr(&self) -> Option<&Pr> {
        self.pr_list().get(self.pcur)
    }
    /// Switches between the graph and the pull requests.
    fn toggle_prs(&mut self) {
        self.prs_view = !self.prs_view;
        self.zone = if self.prs_view { Zone::Prs } else if self.zone == Zone::Prs { Zone::Files } else { self.zone };
        self.arm = None;
        self.version += 1;
    }
    /// Runs `f` in the background; its outcome comes back as a toast and the repo is read again.
    fn act_with(&mut self, dir: PathBuf, f: impl FnOnce(&std::path::Path) -> Note + Send + 'static) {
        self.busy = true;
        self.version += 1;
        let tx = self.notes.0.clone();
        std::thread::spawn(move || {
            let _ = tx.send(f(&dir));
        });
    }
    fn changes(&self) -> Vec<(&Change, bool)> {
        match &self.repo {
            Some(r) => r.staged.iter().map(|c| (c, true)).chain(r.unstaged.iter().map(|c| (c, false))).collect(),
            None => Vec::new(),
        }
    }
    fn dir(&self) -> Option<PathBuf> {
        self.repos.get(self.sel).cloned()
    }
    /// Runs a git action in the background; the outcome comes back as a toast and the repo is read again.
    fn act(&mut self, op: Op) {
        let Some(dir) = self.dir() else { return };
        self.act_at(dir, op);
    }
    fn act_at(&mut self, dir: PathBuf, op: Op) {
        self.busy = true;
        self.version += 1;
        let tx = self.notes.0.clone();
        std::thread::spawn(move || {
            let _ = tx.send(gitdata::run(&dir, op));
        });
    }
}

#[derive(Component)]
pub struct GitPart(Option<Pane>);

#[derive(Resource)]
pub struct GitRoots {
    panes: Vec<(Pane, Entity)>,
    corelab: Entity,
}

pub fn setup(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut images: ResMut<Assets<Image>>, (home, mut atlases): (Res<PanelAtlas>, ResMut<AppAtlases>), mut pmats: ResMut<Assets<PanelMaterial>>, args: Res<crate::Args>, mut app: ResMut<GitApp>) {
    if args.git {
        app.open();
    }
    // the app's own atlas, sharper than the home's (see `AppAtlases`)
    let mut atlas = Atlas::new(&mut commands, &mut images, home.scale * APP_SHARP, -11 - 1);
    let c = &mut commands;
    let at = |x: f32, y: f32, w: f32, h: f32, z: f32| {
        let mut p = stage_to_world(WS_CX + K * (x + w / 2.0 - WS_W / 2.0), WS_CY + K * (y + h / 2.0 - WS_H / 2.0));
        p.z = z;
        p
    };
    let ui = spawn_ui_panel(c, &mut atlas, 4.0, 4.0);
    let e = spawn_panel_quad(c, &mut meshes, &mut pmats, &ui, &PanelSpec { id: PanelId::AppFrame, w: WS_W, h: WS_H, home: at(0.0, 0.0, WS_W, WS_H, -10.0), home_scale: K, side: 0.0, phase: 0.5, show_at: 0.0, unframed: false });
    c.entity(e).insert(GitPart(None));
    let mut panes = Vec::new();
    for (i, pane) in Pane::all().into_iter().enumerate() {
        let [x, y, w, h, z] = pane.rect();
        let ui = spawn_ui_panel(c, &mut atlas, w, h);
        let id = if pane == Pane::Top { PanelId::AppBare } else { PanelId::App };
        let e = spawn_panel_quad(c, &mut meshes, &mut pmats, &ui, &PanelSpec { id, w, h, home: at(x, y, w, h, z), home_scale: K, side: 0.0, phase: 0.4 + i as f32 * 1.1, show_at: 0.0, unframed: false });
        c.entity(e).insert((GitPart(Some(pane)), crate::appframe::AppPart { app: 1, pane: Some(i as u8) }, crate::appframe::PaneRoot(ui.root)));
        panes.push((pane, ui.root));
    }
    let (lw, lh) = (300.0, 40.0);
    let ui = spawn_ui_panel(c, &mut atlas, lw, lh);
    let mut home = stage_to_world(132.0 + lw / 2.0, 52.0 + lh / 2.0);
    home.z = 20.0;
    let e = spawn_panel_quad(c, &mut meshes, &mut pmats, &ui, &PanelSpec { id: PanelId::AppBare, w: lw, h: lh, home, home_scale: 1.0, side: 0.0, phase: 2.6, show_at: 0.0, unframed: false });
    c.entity(e).insert(GitPart(None));
    c.insert_resource(GitRoots { panes, corelab: ui.root });
    atlases.add(1, atlas);
}

#[allow(clippy::too_many_arguments)]
/// What the wheel does over a pane: step the cursor of a zone, or press a key.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Scroll {
    Zone(Zone, KeyCode),
    Key(KeyCode),
}

fn scroll_target(pane: Option<u8>, down: bool, prs_view: bool) -> Option<Scroll> {
    let k = if down { KeyCode::KeyJ } else { KeyCode::KeyK };
    match Pane::all().get(pane? as usize)? {
        Pane::Repo(_) => Some(Scroll::Key(if down { KeyCode::BracketRight } else { KeyCode::BracketLeft })),
        Pane::Branches => Some(Scroll::Zone(Zone::Branches, k)),
        Pane::Graph | Pane::Commit if prs_view => Some(Scroll::Zone(Zone::Prs, k)),
        Pane::Graph | Pane::Commit => Some(Scroll::Zone(Zone::Graph, k)),
        Pane::Status => Some(Scroll::Zone(Zone::Files, k)),
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
pub fn keys(
    mut ev: MessageReader<bevy::input::keyboard::KeyboardInput>,
    keys: Res<ButtonInput<KeyCode>>,
    (focus, moment, mut bar): (ResMut<Focus>, Res<crate::moment::Moment>, ResMut<CmdBar>),
    mut app: ResMut<GitApp>,
    mut sound: ResMut<crate::sound::Sound>,
    mut toasts: ResMut<crate::live::Toasts>,
    sim: Res<crate::core::Sim>,
    (mut clicks, mut scrolls): (ResMut<crate::appframe::AppClicks>, ResMut<crate::appframe::AppScrolls>),
    mut cmd_was: Local<bool>,
) {
    let mut focus = focus;
    let mods = keys.any_pressed([KeyCode::SuperLeft, KeyCode::SuperRight, KeyCode::ControlLeft, KeyCode::ControlRight, KeyCode::AltLeft, KeyCode::AltRight]);
    let bar_busy = focus.cmd || *cmd_was;
    *cmd_was = focus.cmd;
    if !app.open {
        ev.clear();
        return;
    }
    let mut codes: Vec<KeyCode> = Vec::new();
    for e in ev.read() {
        if e.state != bevy::input::ButtonState::Pressed || mods || bar_busy || focus.terminal || moment.shown || (e.repeat && !matches!(e.key_code, KeyCode::KeyJ | KeyCode::KeyK)) {
            continue;
        }
        codes.push(e.key_code);
    }
    // clicks on the panes: a button presses its key, a row picks its item
    codes.append(&mut app.pending);
    // the wheel moves the cursor of the zone under it
    for (pane, n) in scrolls.take(1) {
        if let Some(t) = scroll_target(pane, n > 0, app.prs_view) {
            let (zone, key) = match t {
                Scroll::Zone(z, k) => (Some(z), k),
                Scroll::Key(k) => (None, k),
            };
            if let Some(z) = zone.filter(|z| *z != app.zone) {
                app.zone = z;
                app.version += 1;
            }
            codes.extend(std::iter::repeat_n(key, n.unsigned_abs() as usize));
        }
    }
    for c in clicks.take(1) {
        match c {
            Click::Key(k) => codes.push(k),
            Click::Pick(i) if i >= PR_PICK => {
                app.zone = Zone::Prs;
                app.pcur = (i - PR_PICK).min(app.len_of(Zone::Prs).saturating_sub(1));
                app.arm = None;
                app.version += 1;
            }
            Click::Pick(i) if i >= GRAPH_PICK => {
                app.zone = Zone::Graph;
                app.gcur = (i - GRAPH_PICK).min(app.len_of(Zone::Graph).saturating_sub(1));
                app.version += 1;
            }
            // a branch row: the first click selects it, a click on the selected one switches to it
            Click::Pick(i) if i >= BRANCH_PICK => {
                let b = i - BRANCH_PICK;
                if app.zone == Zone::Branches && app.bcur == b {
                    codes.push(KeyCode::Enter);
                } else {
                    app.zone = Zone::Branches;
                    app.bcur = b.min(app.len_of(Zone::Branches).saturating_sub(1));
                    app.version += 1;
                }
            }
            Click::Pick(i) => {
                app.zone = Zone::Files;
                app.cursor = i.min(app.len_of(Zone::Files).saturating_sub(1));
                app.version += 1;
            }
            // a stage box toggles its file
            Click::Alt(i) if i >= STAGE_ALT => {
                app.zone = Zone::Files;
                app.cursor = (i - STAGE_ALT).min(app.len_of(Zone::Files).saturating_sub(1));
                codes.push(KeyCode::Enter);
            }
            Click::Alt(i) => {
                app.select_repo(i);
                sound.play("toggle");
            }
        }
    }
    for key in codes {
        let sel = app.changes().get(app.cursor).map(|(c, staged)| (c.path.clone(), c.from.clone(), *staged));
        // a second action while one runs would race it
        if app.busy && matches!(key, KeyCode::KeyS | KeyCode::KeyU | KeyCode::KeyA | KeyCode::KeyX | KeyCode::KeyL | KeyCode::KeyP | KeyCode::KeyF | KeyCode::Enter) {
            toasts.push_quiet("Still working on the last action");
            continue;
        }
        if app.busy && app.prs_view && matches!(key, KeyCode::KeyC | KeyCode::KeyM | KeyCode::KeyO) {
            toasts.push_quiet("Still working on the last action");
            continue;
        }
        match key {
            KeyCode::Escape if app.prs_view => {
                app.toggle_prs();
                sound.play("toggle");
            }
            KeyCode::KeyR => {
                app.toggle_prs();
                sound.play("toggle");
            }
            KeyCode::Enter | KeyCode::KeyO if app.prs_view => match (app.pr().map(|p| p.number), app.dir()) {
                (Some(n), Some(d)) => {
                    app.act_with(d, move |d| prs::open_web(d, n, &prs::run_gh));
                    sound.play("confirm");
                }
                _ => toasts.push_quiet("No pull request selected"),
            },
            KeyCode::KeyC if app.prs_view => match (app.pr().map(|p| p.number), app.dir()) {
                (Some(n), Some(d)) => {
                    let dirty = app.repo.as_ref().map(|r| r.staged.len() + r.unstaged.len()).unwrap_or(0);
                    if dirty > 0 {
                        toasts.push(format!("Commit or stash your {dirty} uncommitted change{} before checking out a pull request", if dirty == 1 { "" } else { "s" }));
                    } else {
                        app.act_with(d, move |d| prs::checkout(d, n, &prs::run_gh));
                        sound.play("whoosh");
                    }
                }
                _ => toasts.push_quiet("No pull request selected"),
            },
            KeyCode::KeyM if app.prs_view => match (app.pr().map(|p| p.number), app.dir()) {
                (Some(n), Some(d)) => match prs::press_merge(&mut app.arm, n, sim.t) {
                    prs::MergePress::Armed => {
                        toasts.push(format!("Press M again to merge #{n}"));
                        app.version += 1;
                        sound.play("open");
                    }
                    prs::MergePress::Merge => {
                        app.version += 1;
                        app.act_with(d, move |d| prs::merge(d, n, &prs::run_gh));
                        sound.play("progress");
                    }
                },
                _ => toasts.push_quiet("No pull request selected"),
            },
            KeyCode::Escape => {
                app.close();
                sound.play_with("collapse", 0.7, 0.0);
            }
            KeyCode::KeyJ | KeyCode::ArrowDown => {
                app.step(1);
                sound.play_q("tick", 40);
            }
            KeyCode::KeyK | KeyCode::ArrowUp => {
                app.step(-1);
                sound.play_q("tick", 40);
            }
            KeyCode::Tab => {
                app.next_zone();
                sound.play("toggle");
            }
            k @ (KeyCode::BracketLeft | KeyCode::BracketRight) => {
                let m = app.repos.len();
                if m > 1 {
                    let to = if k == KeyCode::BracketRight { (app.sel + 1) % m } else { (app.sel + m - 1) % m };
                    app.select_repo(to);
                    sound.play("toggle");
                }
            }
            KeyCode::Enter => match app.zone {
                Zone::Files => match sel {
                    Some((p, _, false)) => {
                        app.act(Op::Stage(p));
                        sound.play("confirm");
                    }
                    Some((p, from, true)) => {
                        app.act(Op::Unstage(p, from));
                        sound.play("discard");
                    }
                    None => toasts.push_quiet("Nothing to stage"),
                },
                Zone::Branches => {
                    let b = app.repo.as_ref().and_then(|r| r.branches.get(app.bcur)).map(|b| (b.name.clone(), b.current));
                    match b {
                        Some((_, true)) => toasts.push_quiet("Already on that branch"),
                        Some((name, false)) => {
                            app.act(Op::Switch(name));
                            sound.play("whoosh");
                        }
                        None => {}
                    }
                }
                Zone::Graph | Zone::Prs => {}
            },
            KeyCode::KeyS => match sel {
                Some((p, _, false)) => {
                    app.act(Op::Stage(p));
                    sound.play("confirm");
                }
                Some(_) => toasts.push_quiet("Already staged"),
                None => toasts.push_quiet("Nothing to stage"),
            },
            KeyCode::KeyU => match sel {
                Some((p, from, true)) => {
                    app.act(Op::Unstage(p, from));
                    sound.play("discard");
                }
                _ => toasts.push_quiet("Pick a staged file to unstage"),
            },
            KeyCode::KeyA => {
                if app.repo.as_ref().is_some_and(|r| r.unstaged.is_empty()) {
                    toasts.push_quiet("Nothing left to stage");
                } else {
                    app.act(Op::StageAll);
                    sound.play("confirm");
                }
            }
            KeyCode::KeyX => {
                if app.repo.as_ref().is_some_and(|r| r.staged.is_empty()) {
                    toasts.push_quiet("Nothing staged");
                } else {
                    app.act(Op::UnstageAll);
                    sound.play("discard");
                }
            }
            KeyCode::KeyM => {
                app.amend = !app.amend;
                app.version += 1;
                sound.play("toggle");
            }
            KeyCode::KeyC => {
                let staged = app.repo.as_ref().map(|r| r.staged.len()).unwrap_or(0);
                if app.repo.as_ref().is_some_and(|r| r.card.empty) && app.amend {
                    toasts.push("There is no commit to amend yet");
                } else if staged == 0 && !app.amend {
                    toasts.push("Stage something first (S, or A for everything)");
                } else if let (Some(d), Some(r)) = (app.dir(), &app.repo) {
                    let b = r.card.branch.clone();
                    bar.prompt(BarTarget::Commit(d.to_string_lossy().to_string(), b, app.amend), &mut focus);
                    sound.play("open");
                }
            }
            KeyCode::KeyB => {
                if let Some(d) = app.dir() {
                    bar.prompt(BarTarget::Branch(d.to_string_lossy().to_string()), &mut focus);
                    sound.play("open");
                }
            }
            KeyCode::KeyV => {
                app.flat = !app.flat;
                if !app.flat && app.zone == Zone::Graph {
                    app.zone = Zone::Files;
                }
                app.version += 1;
                sound.play("toggle");
            }
            k @ (KeyCode::Comma | KeyCode::Period) => {
                app.yaw = (app.yaw + if k == KeyCode::Comma { -10.0 } else { 10.0 }).clamp(-60.0, 40.0);
                app.version += 1;
                sound.play_q("tick", 40);
            }
            KeyCode::KeyF => {
                app.act(Op::Fetch);
                sound.play("progress");
            }
            KeyCode::KeyL => {
                app.act(Op::Pull);
                sound.play("progress");
            }
            KeyCode::KeyP => {
                app.act(Op::Push);
                sound.play("progress");
            }
            _ => {}
        }
    }
}

/// Springs, the core's corner, reading the repos in the background, previews and action results.
#[allow(clippy::too_many_arguments)]
pub fn update(sim: Res<crate::core::Sim>, mut atlases: ResMut<AppAtlases>, mut app: ResMut<GitApp>, mut ctl: ResMut<CoreCtl>, svc: Res<Service>, mut bar: ResMut<CmdBar>, mut toasts: ResMut<crate::live::Toasts>, mut sound: ResMut<crate::sound::Sound>) {
    app.amt.t = if app.open { 1.0 } else { 0.0 };
    app.amt.step(crate::motion::dt(crate::motion::Pace::Big, sim.dt));
    ctl.app = ctl.app.max(clamp(app.amt.x, 0.0, 1.0));
    atlases.want(1, app.open || app.amt.x > 0.001);
    // what you typed in the bar for us (it waits while an action runs, so it isn't lost)
    if !app.busy {
        if let Some((t, text)) = bar.out.take() {
            match t {
                BarTarget::Commit(dir, _, _) => {
                    let amend = app.amend;
                    app.act_at(PathBuf::from(dir), Op::Commit { msg: text, amend });
                    app.amend = false;
                    sound.play("done");
                }
                BarTarget::Branch(dir) => {
                    app.act_at(PathBuf::from(dir), Op::Switch(text));
                    sound.play("whoosh");
                }
                other => bar.out = Some((other, text)),
            }
        }
    }
    let notes: Vec<Result<String, String>> = app.notes.1.lock().map(|r| r.try_iter().collect()).unwrap_or_default();
    for n in notes {
        app.busy = false;
        match n {
            Ok(s) => toasts.push_quiet(s),
            Err(e) => {
                toasts.push(e);
                sound.play("error");
            }
        }
        app.load_t = -99.0;
        app.version += 1;
    }
    let got = app.load_rx.as_ref().and_then(|r| match r.lock().ok()?.try_recv() {
        Ok(v) => Some(Some(v)),
        Err(mpsc::TryRecvError::Disconnected) => Some(None),
        Err(_) => None,
    });
    if got == Some(None) {
        // the reader died: try again later
        app.loading = false;
        app.load_rx = None;
    }
    if let Some(Some((repos, sel, repo, cards))) = got {
        app.loading = false;
        app.loads += 1;
        if app.repos != repos {
            // the list changed (a new repo was found): stay on the repo that was selected
            let cur = app.repos.get(app.sel).cloned();
            app.repos = repos;
            match cur.and_then(|p| app.repos.iter().position(|r| *r == p)) {
                Some(i) => app.sel = i,
                None => {
                    app.sel = 0;
                    app.repo = None;
                    app.cursor = 0;
                    app.bcur = 0;
                    app.gcur = 0;
                }
            }
        }
        // look tests: SPHERE_GIT_REPO=n starts on the nth repo
        if let Some(n) = std::env::var("SPHERE_GIT_REPO").ok().and_then(|v| v.parse::<usize>().ok()).filter(|n| *n != app.sel && app.repo.is_none() && *n < app.repos.len()) {
            app.sel = n;
            app.load_t = -99.0;
            app.cards = cards;
            return;
        }
        if app.repos.is_empty() {
            app.repo = None;
        } else if sel == app.sel {
            let n = repo.staged.len() + repo.unstaged.len();
            app.cursor = app.cursor.min(n.saturating_sub(1));
            app.bcur = app.bcur.min(repo.branches.len().saturating_sub(1));
            app.gcur = app.gcur.min(repo.graph.len().saturating_sub(1));
            if repo.graph.get(app.gcur).is_some_and(|g| g.hash.is_empty()) {
                app.gcur = repo.graph.iter().position(|g| !g.hash.is_empty()).unwrap_or(0);
            }
            if app.sel >= app.repos.len() {
                app.sel = 0;
            }
            if app.repo.is_none() {
                // a repo's first read: start on its current branch
                app.bcur = repo.branches.iter().position(|b| b.current).unwrap_or(0);
                if let Ok(keys) = std::env::var("SPHERE_GIT_KEYS") {
                    app.pending = keys
                        .chars()
                        .filter_map(|k| match k {
                            'j' => Some(KeyCode::KeyJ),
                            'k' => Some(KeyCode::KeyK),
                            't' => Some(KeyCode::Tab),
                            'e' => Some(KeyCode::Enter),
                            'v' => Some(KeyCode::KeyV),
                            'r' => Some(KeyCode::KeyR),
                            ']' => Some(KeyCode::BracketRight),
                            '[' => Some(KeyCode::BracketLeft),
                            ',' => Some(KeyCode::Comma),
                            '.' => Some(KeyCode::Period),
                            _ => None,
                        })
                        .collect();
                }
            }
            app.repo = Some(repo);
        }
        app.cards = cards;
        app.version += 1;
    }
    let got = app.preview_rx.as_ref().and_then(|r| r.lock().ok()?.try_recv().ok());
    if let Some(p) = got {
        app.preview = p;
        app.version += 1;
    }
    if !app.open {
        return;
    }
    // the file under the cursor: its change, for the preview (read again after each load)
    let want = app.changes().get(app.cursor).map(|(c, s)| (c.path.clone(), *s, c.code == '?'));
    let key = want.as_ref().map(|w| format!("{}{}{}", w.0, w.1, app.loads)).unwrap_or_default();
    if key != app.preview_for {
        app.preview_for = key;
        match (want, app.dir()) {
            (Some((p, staged, untracked)), Some(d)) => {
                let (tx, rx) = mpsc::channel();
                app.preview_rx = Some(Mutex::new(rx));
                std::thread::spawn(move || {
                    let _ = tx.send(gitdata::preview(&d, &p, staged, untracked));
                });
            }
            _ => {
                if !app.preview.is_empty() {
                    app.preview.clear();
                    app.version += 1;
                }
            }
        }
    }
    pr_update(&mut app, sim.t);
    // read the repos on open, after an action, and every 4 s (you may change things elsewhere)
    if app.loading || sim.t - app.load_t < 4.0 {
        return;
    }
    app.load_t = sim.t;
    app.loading = true;
    let rediscover = std::mem::take(&mut app.rediscover);
    let agent_repos: Vec<String> = if app.repos.is_empty() || rediscover {
        let s = svc.shared.lock().unwrap();
        let mut v: Vec<&TaskView> = s.tasks.iter().collect();
        v.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        v.into_iter().map(|t| t.repo.clone()).collect()
    } else {
        Vec::new()
    };
    let (known, sel, old) = (app.repos.clone(), app.sel, app.cards.clone());
    let sel_path = known.get(sel).cloned();
    let (tx, rx) = mpsc::channel();
    app.load_rx = Some(Mutex::new(rx));
    std::thread::spawn(move || {
        // finding the repos runs git in each, so it happens here, off the frame
        let repos = if known.is_empty() || rediscover { gitdata::discover(&agent_repos, REPOS_MAX) } else { known };
        let sel = sel_path.and_then(|p| repos.iter().position(|r| *r == p)).unwrap_or(if rediscover { 0 } else { sel }).min(repos.len().saturating_sub(1));
        let Some(dir) = repos.get(sel) else {
            let _ = tx.send((repos, 0, Repo::default(), Vec::new()));
            return;
        };
        let repo = gitdata::load(dir);
        let first = window_first(sel, repos.len());
        let cards = repos
            .iter()
            .enumerate()
            .map(|(i, p)| {
                if i == sel {
                    repo.card.clone()
                } else if (first..first + REPOS).contains(&i) {
                    gitdata::card(p)
                } else {
                    old.get(i).filter(|c| c.path == p.to_string_lossy()).cloned().unwrap_or_else(|| gitdata::bare_card(p))
                }
            })
            .collect();
        let _ = tx.send((repos, sel, repo, cards));
    });
}

/// Pull requests: read in the background on open, on a repo change and every `PR_EVERY` s; the selected one's
/// diff and checks are read when the pane shows it; an armed merge lapses.
fn pr_update(app: &mut GitApp, now: f32) {
    if prs::arm_expired(&app.arm, now) {
        app.arm = None;
        app.version += 1;
    }
    let cur = app.dir();
    if app.prs_for != cur {
        app.prs_for = cur.clone();
        app.prs = None;
        app.detail = None;
        app.detail_for.clear();
        app.pcur = 0;
        app.prs_rx = None;
        app.prs_t = -99.0;
        *prs::SUMMARY.lock().unwrap() = None;
        app.version += 1;
    }
    let got = app.prs_rx.as_ref().and_then(|r| r.lock().ok()?.try_recv().ok());
    if let Some((dir, list)) = got {
        app.prs_rx = None;
        if Some(&dir) == cur.as_ref() {
            let keep = app.pr().map(|p| p.number);
            app.prs = Some(list);
            app.pcur = keep.and_then(|n| app.pr_list().iter().position(|p| p.number == n)).unwrap_or(0).min(app.pr_list().len().saturating_sub(1));
            *prs::SUMMARY.lock().unwrap() = match &app.prs {
                Some(PrList::Prs(v)) => Some(prs::summarize(v)),
                _ => None,
            };
            app.version += 1;
        }
    }
    if let Some(d) = cur.clone().filter(|_| app.prs_rx.is_none() && now - app.prs_t >= PR_EVERY) {
        app.prs_t = now;
        let (tx, rx) = mpsc::channel();
        app.prs_rx = Some(Mutex::new(rx));
        std::thread::spawn(move || {
            let list = prs::list(&d, &prs::run_gh);
            let _ = tx.send((d, list));
        });
    }
    let got = app.detail_rx.as_ref().and_then(|r| r.lock().ok()?.try_recv().ok());
    if let Some(d) = got {
        app.detail_rx = None;
        if app.pr().is_some_and(|p| p.number == d.number) {
            app.detail = Some(d);
            app.version += 1;
        }
    }
    // the selected pull request's files and checks (read again when it changes on GitHub)
    if app.prs_view {
        if let (Some(p), Some(dir)) = (app.pr(), cur) {
            let key = format!("{}:{}:{}", p.number, p.updated, app.prs_t);
            let n = p.number;
            if app.detail_for != key && app.detail_rx.is_none() {
                app.detail_for = key;
                let (tx, rx) = mpsc::channel();
                app.detail_rx = Some(Mutex::new(rx));
                std::thread::spawn(move || {
                    let _ = tx.send(prs::detail(&dir, n, &prs::run_gh));
                });
            }
        }
    }
}

#[allow(clippy::type_complexity)]
pub fn place(app: Res<GitApp>, mut q: Query<(&mut Panel, &GitPart)>) {
    let amt = clamp(app.amt.x, 0.0, 1.0);
    let first = window_first(app.sel, app.cards.len());
    for (mut p, part) in &mut q {
        // a repo card with no repo stays away
        let a = match part.0 {
            Some(Pane::Repo(i)) if first + i as usize >= app.cards.len() => 0.0,
            _ => amt,
        };
        if p.work.map(|w| w.0) != Some(a) {
            p.work = Some((a, 0.0));
        }
    }
}

// ------------------------------------------------------------------ drawing

#[allow(clippy::too_many_arguments)]
pub fn draw(mut commands: Commands, app: Res<GitApp>, svc: Res<Service>, roots: Option<Res<GitRoots>>, f: Res<UiFonts>, theme: Res<crate::theme::Theme>, mut seen: Local<(u64, u64, u64)>) {
    let Some(roots) = roots else { return };
    if app.amt.x < 0.001 && !app.open {
        return;
    }
    let (agents, merged, sver) = {
        let s = svc.shared.lock().unwrap();
        let mut v: Vec<TaskView> = s.tasks.clone();
        v.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        v.truncate(3);
        (v, s.merged.clone(), s.version)
    };
    let key = (app.version, sver, theme.version);
    if *seen == key {
        return;
    }
    *seen = key;
    let c = &mut commands;
    for (_, r) in &roots.panes {
        c.entity(*r).despawn_children();
    }
    c.entity(roots.corelab).despawn_children();
    let ui = Ui::new(&f);
    let root = |p: Pane| roots.panes.iter().find(|x| x.0 == p).map(|x| x.1).unwrap();
    // beside the small core
    let needs = agents.iter().filter(|t| t.state == TaskState::NeedsYou).count();
    let lab = c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(5), padding: UiRect::top(px(4)), ..default() }).id();
    let l1 = ui.text(c, "GIT", &f.ui600, 11.0, 3.1, ui.hot);
    let (l2s, l2c) = if needs > 0 { (format!("{needs} agent{} needs you", if needs == 1 { "" } else { "s" }), role_color(Role::Acc)) } else { (format!("{} repos", app.repos.len()), ui.line) };
    let l2 = ui.text(c, l2s, &f.ui500, 11.5, 1.2, l2c);
    c.entity(lab).add_children(&[l1, l2]);
    c.entity(roots.corelab).add_child(lab);
    draw_top(c, &ui, root(Pane::Top), &app);
    let first = window_first(app.sel, app.cards.len());
    for i in 0..REPOS {
        draw_card(c, &ui, root(Pane::Repo(i as u8)), app.cards.get(first + i), first + i == app.sel, first + i);
    }
    draw_strip(c, &ui, root(Pane::Strip), &agents, &merged, app.prs_view);
    let empty_panes = |c: &mut Commands, except_graph: bool| {
        for p in [Pane::Branches, Pane::Commit, Pane::Status, Pane::Remote].into_iter().chain((!except_graph).then_some(Pane::Graph)) {
            let b = ui.body(c, root(p));
            let h = ui.header(c, match p { Pane::Branches => "Branches", Pane::Commit => "Commit", Pane::Status => "Working tree", Pane::Graph => "Graph", _ => "Remote" }, "", ui.dim);
            c.entity(b).add_child(h);
        }
    };
    let Some(r) = &app.repo else {
        let b = ui.body(c, root(Pane::Graph));
        let h = ui.header(c, "Graph", "", ui.dim);
        let m = c.spawn((Text::new(if app.repos.is_empty() && !app.loading { "No git repos found: agents' repos and the repos in ~/Projects show here" } else { "Reading…" }), font(&f.ui500, 13.0, 0.5), TextColor(ui.line), Node { margin: UiRect::all(px(18)), ..default() })).id();
        c.entity(b).add_children(&[h, m]);
        empty_panes(c, true);
        return;
    };
    if let Some(e) = &r.error {
        let b = ui.body(c, root(Pane::Graph));
        let h = ui.header(c, "Graph", "", ui.dim);
        let m = c.spawn((Text::new(clip(e, 120)), font(&f.ui500, 13.0, 0.4), TextColor(role_color(Role::Bad)), Node { margin: UiRect::all(px(18)), ..default() })).id();
        c.entity(b).add_children(&[h, m]);
        empty_panes(c, true);
        return;
    }
    draw_branches(c, &ui, root(Pane::Branches), r, app.bcur, app.zone == Zone::Branches);
    if app.prs_view {
        draw_prs(c, &ui, root(Pane::Graph), &app);
        draw_pr_detail(c, &ui, root(Pane::Commit), &app);
    } else {
        if app.flat {
            draw_graph(c, &ui, root(Pane::Graph), r, app.gcur, app.zone == Zone::Graph);
        } else {
            draw_graph3d(c, &ui, root(Pane::Graph), r, app.yaw);
        }
        draw_commit(c, &ui, root(Pane::Commit), r, app.amend, app.busy);
    }
    draw_status(c, &ui, root(Pane::Status), r, app.cursor, app.zone == Zone::Files, &app.preview);
    draw_remote(c, &ui, root(Pane::Remote), r, app.busy);
}

fn draw_top(c: &mut Commands, ui: &Ui, root: Entity, app: &GitApp) {
    let r = c.spawn(Node { width: percent(100), height: percent(100), flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(12), ..default() }).id();
    c.entity(root).add_child(r);
    let name = ui.text(c, "GIT", &ui.f.ui600, 11.0, 3.5, ui.hot);
    let mut names: Vec<String> = app.cards.iter().take(5).map(|c| clip(&c.name.to_uppercase(), 18)).collect();
    if app.cards.len() > 5 {
        names.push(format!("+{}", app.cards.len() - 5));
    }
    let sub = ui.text(c, names.join(" · "), &ui.f.ui500, 11.0, 2.6, ui.dim);
    let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
    let credit = ui.text(c, "git UI after gitui (MIT) · git · gh", &ui.f.ui500, 10.5, 1.0, ui.dim);
    let esc = c.spawn((Node { padding: UiRect { left: px(6), right: px(6), top: px(3), bottom: px(2) }, border: UiRect::all(px(1)), ..default() }, BorderColor::all(with_alpha(ui.line, 0.55)), Click::Key(KeyCode::Escape))).id();
    let et = ui.text(c, "ESC", &ui.f.ui700, 10.0, 1.6, ui.hot);
    c.entity(esc).add_child(et);
    c.entity(r).add_children(&[name, sub, sp, credit, esc]);
}

fn draw_card(c: &mut Commands, ui: &Ui, root: Entity, card: Option<&RepoCard>, sel: bool, index: usize) {
    let Some(k) = card else { return };
    let b = c
        .spawn((
            Node { width: percent(100), height: percent(100), flex_direction: FlexDirection::Column, row_gap: px(4), padding: UiRect { left: px(14), right: px(12), top: px(10), bottom: px(8) }, border: UiRect::left(px(if sel { 2.0 } else { 0.0 })), ..default() },
            BorderColor::all(ui.hot),
            BackgroundColor(if sel { with_alpha(ui.line, 0.07) } else { Color::NONE }),
            Click::Alt(index),
        ))
        .id();
    c.entity(root).add_child(b);
    let top = ui.row(c, 8.0);
    let nm = ui.text(c, clip(&k.name.to_uppercase(), 12), &ui.f.display, 13.0, 1.6, ui.hot);
    let dot = c.spawn((Node { width: px(6), height: px(6), ..default() }, BackgroundColor(if k.changed > 0 { role_color(Role::Acc) } else { role_color(Role::Ok) }))).id();
    let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
    let lg = ui.text(c, k.lang.clone(), &ui.f.ui500, 10.0, 2.0, ui.dim);
    c.entity(top).add_children(&[nm, dot, sp, lg]);
    let br = ui.text(c, format!("⌥ {}", clip_start(&k.branch, 24)), &ui.f.mono, 11.5, 0.0, ui.line);
    let st = ui.row(c, 8.0);
    let a = ui.text(c, format!("↑{}", k.ahead), &ui.f.mono, 11.0, 0.0, role_color(Role::Ok));
    let bh = ui.text(c, format!("↓{}", k.behind), &ui.f.mono, 11.0, 0.0, role_color(Role::Acc));
    let ch = ui.text(c, format!("{} CHANGED", k.changed), &ui.f.ui600, 10.0, 1.8, ui.dim);
    let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
    let stash = ui.text(c, if k.stashes > 0 { format!("STASH {}", k.stashes) } else { String::new() }, &ui.f.ui600, 9.5, 1.4, ui.dim);
    c.entity(st).add_children(&[a, bh, ch, sp, stash]);
    c.entity(b).add_children(&[top, br, st]);
}

fn draw_branches(c: &mut Commands, ui: &Ui, root: Entity, r: &Repo, cursor: usize, active: bool) {
    let b = ui.body(c, root);
    let n = r.branches.len();
    let first = first_row(cursor, n, BRANCH_ROWS);
    let count = if n > BRANCH_ROWS { format!("{}–{} of {n}", first + 1, (first + BRANCH_ROWS).min(n)) } else { format!("{n} local") };
    let h = ui.header(c, "Branches", &count, ui.dim);
    let sw = c.spawn((Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, height: px(28), margin: UiRect { left: px(12), right: px(12), top: px(10), bottom: px(6) }, padding: UiRect::horizontal(px(10)), border: UiRect::all(px(1)), column_gap: px(8), flex_shrink: 0.0, ..default() }, BorderColor::all(with_alpha(ui.line, 0.4)), Click::Key(KeyCode::KeyB))).id();
    let st = ui.text(c, "› Switch branch…", &ui.f.ui500, 12.5, 0.5, ui.dim);
    let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
    let kb = c.spawn((Node { padding: UiRect { left: px(4), right: px(4), top: px(1), bottom: px(0) }, border: UiRect::all(px(1)), ..default() }, BorderColor::all(with_alpha(ui.line, 0.5)))).id();
    let kt = ui.text(c, "B", &ui.f.ui700, 9.5, 0.5, ui.line);
    c.entity(kb).add_child(kt);
    c.entity(sw).add_children(&[st, sp, kb]);
    let list = c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(5), padding: UiRect { left: px(8), right: px(12), top: px(4), ..default() }, ..default() }).id();
    c.entity(b).add_children(&[h, sw, list]);
    if n == 0 {
        let t = ui.text(c, "No branches", &ui.f.ui500, 12.5, 0.4, ui.dim);
        c.entity(list).add_child(t);
    }
    for (i, br) in r.branches.iter().enumerate().skip(first).take(BRANCH_ROWS) {
        let sel = active && i == cursor;
        let row = c
            .spawn((
                Node { flex_direction: FlexDirection::Row, column_gap: px(10), padding: UiRect { left: px(4), top: px(2), bottom: px(2), ..default() }, border: UiRect::left(px(if br.current || sel { 2.0 } else { 0.0 })), flex_shrink: 0.0, ..default() },
                BorderColor::all(if sel { role_color(Role::Acc) } else { ui.hot }),
                BackgroundColor(if sel { with_alpha(ui.line, 0.10) } else { Color::NONE }),
                Click::Pick(BRANCH_PICK + i),
            ))
            .id();
        let mk = c
            .spawn((
                Node { width: px(7), height: px(7), margin: UiRect::top(px(4)), border: UiRect::all(px(1)), flex_shrink: 0.0, ..default() },
                BackgroundColor(if br.current { ui.hot } else { Color::NONE }),
                BorderColor::all(if br.agent { role_color(Role::Fill) } else { ui.line }),
                UiTransform { rotation: Rot2::degrees(if br.agent { 45.0 } else { 0.0 }), ..default() },
            ))
            .id();
        let col = c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(1), flex_grow: 1.0, flex_shrink: 1.0, overflow: Overflow::clip(), ..default() }).id();
        let nm = ui.text(c, clip_start(&br.name, 22), &ui.f.mono, 12.0, 0.0, if br.current { ui.hot } else { ui.line });
        let sub = ui.text(c, clip(&format!("{}{}", if br.agent { "AGENT · " } else { "" }, br.age.to_uppercase()), 26), &ui.f.ui600, 9.5, 1.6, ui.dim);
        c.entity(col).add_children(&[nm, sub]);
        let tr = ui.text(c, if br.ahead + br.behind > 0 { format!("↑{} ↓{}", br.ahead, br.behind) } else { String::new() }, &ui.f.mono, 10.5, 0.0, role_color(Role::Ok));
        c.entity(row).add_children(&[mk, col, tr]);
        c.entity(list).add_child(row);
    }
}

fn draw_graph(c: &mut Commands, ui: &Ui, root: Entity, r: &Repo, cursor: usize, active: bool) {
    let b = ui.body(c, root);
    let n = r.graph.len();
    let first = first_row(cursor, n, GRAPH_ROWS);
    let right = if n > GRAPH_ROWS { format!("{}–{} of {n}", first + 1, (first + GRAPH_ROWS).min(n)) } else { "all branches".to_string() };
    let h = ui.header(c, "Graph", &right, ui.dim);
    let list = c.spawn(Node { flex_direction: FlexDirection::Column, padding: UiRect { left: px(16), right: px(16), top: px(8), ..default() }, ..default() }).id();
    c.entity(b).add_children(&[h, list]);
    // back to the tilted graph
    let chip = c.spawn((Node { position_type: PositionType::Absolute, right: px(150), top: px(10), padding: UiRect { left: px(6), right: px(6), top: px(2), bottom: px(1) }, border: UiRect::all(px(1)), ..default() }, BorderColor::all(with_alpha(ui.line, 0.45)), Click::Key(KeyCode::KeyV))).id();
    let ct = ui.text(c, "V GRAPH", &ui.f.ui700, 9.5, 1.4, ui.line);
    c.entity(chip).add_child(ct);
    c.entity(b).add_child(chip);
    let kc = key_chip(c, ui, "R PRS", KeyCode::KeyR, 240.0);
    c.entity(b).add_child(kc);
    if n == 0 {
        let t = ui.text(c, "No commits yet", &ui.f.ui500, 13.0, 0.4, ui.dim);
        c.entity(list).add_child(t);
        return;
    }
    let lanes_col = [role_color(Role::Hot), role_color(Role::Fill), role_color(Role::Acc), role_color(Role::Ok), ui.line];
    let lanes_w = r.graph.iter().skip(first).take(GRAPH_ROWS).map(|g| g.lanes.trim_end().chars().count()).max().unwrap_or(1).min(12) as f32 * 7.4 + 4.0;
    for (i, g) in r.graph.iter().enumerate().skip(first).take(GRAPH_ROWS) {
        let sel = active && i == cursor && !g.hash.is_empty();
        let row = c.spawn((Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, width: percent(100), height: px(20), column_gap: px(8), overflow: Overflow::clip(), flex_shrink: 0.0, ..default() }, BackgroundColor(if sel { with_alpha(ui.line, 0.10) } else { Color::NONE }), Click::Pick(GRAPH_PICK + i))).id();
        // the lanes, each column in its own colour
        let lanes = c.spawn(Node { flex_direction: FlexDirection::Row, width: px(lanes_w), flex_shrink: 0.0, overflow: Overflow::clip(), ..default() }).id();
        for (i, ch) in g.lanes.chars().enumerate() {
            let col = if ch == '*' { ui.hot } else { with_alpha(lanes_col[(i / 2) % lanes_col.len()], 0.75) };
            let glyph = if ch == '*' { "◆".to_string() } else { ch.to_string() };
            let t = c.spawn((Text::new(glyph), font(&ui.f.mono, 12.0, 0.0), TextColor(col), Node { width: px(7.4), ..default() })).id();
            c.entity(lanes).add_child(t);
        }
        c.entity(row).add_child(lanes);
        if !g.hash.is_empty() {
            let hs = ui.text(c, g.hash.clone(), &ui.f.mono, 11.5, 0.0, ui.dim);
            c.entity(row).add_child(hs);
            for rf in g.refs.iter().take(2) {
                let agent = rf.contains("sphere/") || rf.contains("claude/");
                let chip = c.spawn((Node { padding: UiRect { left: px(5), right: px(5), top: px(1), bottom: px(0) }, border: UiRect::all(px(1)), flex_shrink: 0.0, ..default() }, BorderColor::all(with_alpha(if agent { role_color(Role::Fill) } else { ui.line }, 0.7)))).id();
                let t = ui.text(c, clip(&rf.to_uppercase(), 22), &ui.f.ui700, 10.0, 1.4, ui.hot);
                c.entity(chip).add_child(t);
                c.entity(row).add_child(chip);
            }
            let sj = c.spawn((Text::new(clip(&g.subject, 60)), font(&ui.f.ui500, 13.0, 0.3), TextColor(if sel { ui.hot } else { ui.line }), TextLayout { linebreak: LineBreak::NoWrap, ..default() }, Node { flex_grow: 1.0, flex_shrink: 1.0, overflow: Overflow::clip(), ..default() })).id();
            let ag = ui.text(c, g.age.replace(" ago", ""), &ui.f.ui500, 11.0, 0.6, ui.dim);
            c.entity(row).add_children(&[sj, ag]);
        }
        c.entity(list).add_child(row);
    }
}

/// The commit graph as the prototype draws it: a plane of lanes seen from above at an angle, newest commits to
/// the right, diamonds for commits, branch labels on their heads (agents' marked).
fn draw_graph3d(c: &mut Commands, ui: &Ui, root: Entity, r: &Repo, yaw_deg: f32) {
    let b = ui.body(c, root);
    let h = ui.header(c, "Graph", "", ui.dim);
    let area = c.spawn(Node { width: percent(100), flex_grow: 1.0, ..default() }).id();
    c.entity(b).add_children(&[h, area]);
    let nodes = &r.dag;
    if nodes.is_empty() {
        let t = ui.text(c, "No commits yet", &ui.f.ui500, 13.0, 0.4, ui.dim);
        c.entity(area).add_child(t);
        return;
    }
    let rows = nodes.len() as f32;
    let lanes = nodes.iter().map(|n| n.lane).max().unwrap_or(0) as f32 + 1.0;
    let (sx, sz) = (520.0 / rows.max(8.0), 46.0);
    let (yaw, pitch) = (yaw_deg.to_radians(), 62f32.to_radians());
    let (cx, cy, k, f) = (350.0, 150.0, 1.0, 900.0);
    // newest to the right, lanes going back into the plane
    let project = |row: f32, lane: f32| -> Vec2 {
        let x = (rows - 1.0 - row) * sx - (rows - 1.0) * sx / 2.0;
        let z = lane * sz - (lanes - 1.0) * sz / 2.0;
        let (x1, z1) = (x * yaw.cos() - z * yaw.sin(), x * yaw.sin() + z * yaw.cos());
        let (y2, z2) = (z1 * pitch.sin(), z1 * pitch.cos());
        let p = f / (f + z2);
        Vec2::new(cx + x1 * k * p, cy - y2 * k * p)
    };
    // the plane: a faint line along each lane
    for l in 0..lanes as usize {
        ui.line(c, area, project(0.0, l as f32), project(rows - 1.0, l as f32), 1.0, with_alpha(ui.line, 0.07));
    }
    let lane_col = [ui.line, role_color(Role::Fill), role_color(Role::Acc), role_color(Role::Ok), role_color(Role::Hot)];
    let pos: std::collections::HashMap<&str, (Vec2, usize)> = nodes.iter().map(|n| (n.hash.as_str(), (project(n.row as f32, n.lane as f32), n.lane))).collect();
    // edges, child to each parent on screen
    for n in nodes {
        let a = pos[n.hash.as_str()].0;
        for p in &n.parents {
            if let Some((bpos, bl)) = pos.get(p.as_str()) {
                let col = lane_col[n.lane.max(*bl) % lane_col.len()];
                ui.line(c, area, a, *bpos, 1.5, with_alpha(col, 0.55));
            }
        }
    }
    // commits
    for n in nodes {
        let p = pos[n.hash.as_str()].0;
        // branch heads are filled
        let head = !n.refs.is_empty();
        let col = lane_col[n.lane % lane_col.len()];
        let d = c
            .spawn((
                Node { position_type: PositionType::Absolute, left: px(p.x - 4.5), top: px(p.y - 4.5), width: px(9), height: px(9), border: UiRect::all(px(1.5)), ..default() },
                BorderColor::all(col),
                BackgroundColor(if head { col } else { Color::srgba(0.0, 0.0, 0.0, 0.8) }),
                UiTransform { rotation: Rot2::degrees(45.0), ..default() },
            ))
            .id();
        c.entity(area).add_child(d);
    }
    // labels on branch heads, up and out of the way
    let mut placed: Vec<Vec2> = Vec::new();
    for n in nodes.iter().filter(|n| !n.refs.is_empty()).take(7) {
        let p = pos[n.hash.as_str()].0;
        let local: Vec<&String> = n.refs.iter().filter(|r| !r.starts_with("origin/")).collect();
        let name = local.first().copied().or(n.refs.first()).map(|s| s.as_str()).unwrap_or("");
        let agent = name.starts_with("sphere/") || name.starts_with("claude/");
        let mut at = p + Vec2::new(-20.0, -30.0);
        at.x = at.x.clamp(8.0, 700.0 - 200.0);
        at.y = at.y.max(6.0);
        while placed.iter().any(|q| (q.y - at.y).abs() < 20.0 && (q.x - at.x).abs() < 170.0) {
            at.y -= 22.0;
        }
        placed.push(at);
        ui.line(c, area, p, at + Vec2::new(14.0, 18.0), 1.0, with_alpha(ui.line, 0.35));
        let chip = c.spawn((Node { position_type: PositionType::Absolute, left: px(at.x), top: px(at.y), padding: UiRect { left: px(6), right: px(6), top: px(2), bottom: px(1) }, border: UiRect::all(px(1)), column_gap: px(6), ..default() }, BorderColor::all(with_alpha(if agent { role_color(Role::Fill) } else { ui.line }, 0.7)), BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.75)))).id();
        let t = ui.text(c, clip(&name.to_uppercase(), 26), &ui.f.ui700, 10.0, 1.6, ui.hot);
        c.entity(chip).add_child(t);
        if agent {
            let d = c.spawn((Node { width: px(5), height: px(5), align_self: AlignSelf::Center, ..default() }, BackgroundColor(role_color(Role::Fill)), UiTransform { rotation: Rot2::degrees(45.0), ..default() })).id();
            let a = ui.text(c, "AGENT", &ui.f.ui700, 8.5, 1.2, role_color(Role::Fill));
            c.entity(chip).add_children(&[d, a]);
        }
        c.entity(area).add_child(chip);
    }
    let foot = c.spawn(Node { position_type: PositionType::Absolute, left: px(16), bottom: px(10), column_gap: px(14), ..default() }).id();
    for (label, key) in [("‹ TURN ,", KeyCode::Comma), ("TURN . ›", KeyCode::Period), ("V LIST", KeyCode::KeyV), ("R PRS", KeyCode::KeyR)] {
        let part = c.spawn((Node { padding: UiRect::axes(px(4), px(2)), ..default() }, Click::Key(key))).id();
        let ft = ui.text(c, label, &ui.f.ui600, 9.5, 2.0, ui.dim);
        c.entity(part).add_child(ft);
        c.entity(foot).add_child(part);
    }
    c.entity(area).add_child(foot);
}

/// A small bordered key hint in a pane's top right, `right` px from the edge.
fn key_chip(c: &mut Commands, ui: &Ui, label: &str, key: KeyCode, right: f32) -> Entity {
    let chip = c.spawn((Node { position_type: PositionType::Absolute, right: px(right), top: px(10), padding: UiRect { left: px(6), right: px(6), top: px(2), bottom: px(1) }, border: UiRect::all(px(1)), ..default() }, BorderColor::all(with_alpha(ui.line, 0.45)), Click::Key(key))).id();
    let ct = ui.text(c, label, &ui.f.ui700, 9.5, 1.4, ui.line);
    c.entity(chip).add_child(ct);
    chip
}

fn tone_color(ui: &Ui, t: Tone) -> Color {
    match t {
        Tone::Ok => role_color(Role::Ok),
        Tone::Bad => role_color(Role::Bad),
        Tone::Acc => role_color(Role::Acc),
        Tone::Dim => ui.dim,
    }
}

/// A bordered word in a tone's colour.
fn tone_chip(c: &mut Commands, ui: &Ui, s: &str, t: Tone) -> Entity {
    let col = tone_color(ui, t);
    let chip = c.spawn((Node { padding: UiRect { left: px(5), right: px(5), top: px(1), bottom: px(0) }, border: UiRect::all(px(1)), flex_shrink: 0.0, ..default() }, BorderColor::all(with_alpha(col, 0.6)))).id();
    let t = ui.text(c, s, &ui.f.ui700, 9.5, 1.3, col);
    c.entity(chip).add_child(t);
    chip
}

/// The repo's open pull requests, in the graph's place (R).
fn draw_prs(c: &mut Commands, ui: &Ui, root: Entity, app: &GitApp) {
    let b = ui.body(c, root);
    let list = app.pr_list();
    let n = list.len();
    let first = first_row(app.pcur, n, PR_ROWS);
    let right = if n > PR_ROWS { format!("{}–{} of {n}", first + 1, (first + PR_ROWS).min(n)) } else { format!("{n} open") };
    let h = ui.header(c, "Pull requests", &right, ui.dim);
    let col = c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(4), padding: UiRect { left: px(12), right: px(16), top: px(8), ..default() }, ..default() }).id();
    c.entity(b).add_children(&[h, col]);
    let kc = key_chip(c, ui, "R GRAPH", KeyCode::KeyR, 150.0);
    c.entity(b).add_child(kc);
    let say = |c: &mut Commands, s: &str, colr: Color| {
        let t = c.spawn((Text::new(clip(s, 80)), font(&ui.f.ui500, 13.0, 0.4), TextColor(colr), Node { margin: UiRect { left: px(6), top: px(10), ..default() }, ..default() })).id();
        c.entity(col).add_child(t);
    };
    match &app.prs {
        None => say(c, "Reading pull requests…", ui.dim),
        Some(PrList::Note(s)) => {
            let bad = s != prs::NO_PRS;
            say(c, s, if bad { role_color(Role::Acc) } else { ui.dim });
        }
        Some(PrList::Prs(_)) => {}
    }
    let active = app.zone == Zone::Prs;
    for (i, p) in list.iter().enumerate().skip(first).take(PR_ROWS) {
        let sel = i == app.pcur;
        let row = c
            .spawn((
                Node { flex_direction: FlexDirection::Column, row_gap: px(2), padding: UiRect { left: px(8), top: px(3), bottom: px(3), right: px(4) }, border: UiRect::left(px(if sel { 2.0 } else { 0.0 })), flex_shrink: 0.0, ..default() },
                BorderColor::all(if active { role_color(Role::Acc) } else { with_alpha(ui.hot, 0.4) }),
                BackgroundColor(if sel { with_alpha(ui.line, if active { 0.10 } else { 0.05 }) } else { Color::NONE }),
                Click::Pick(PR_PICK + i),
            ))
            .id();
        let l1 = ui.row(c, 8.0);
        let num = ui.text(c, format!("#{}", p.number), &ui.f.mono, 12.0, 0.0, ui.dim);
        let title = c.spawn((Text::new(clip(&p.title, 58)), font(&ui.f.ui500, 13.0, 0.3), TextColor(if sel { ui.hot } else { ui.line }), TextLayout { linebreak: LineBreak::NoWrap, ..default() }, Node { flex_grow: 1.0, flex_shrink: 1.0, overflow: Overflow::clip(), ..default() })).id();
        c.entity(l1).add_children(&[num, title]);
        if p.draft {
            let d = tone_chip(c, ui, "DRAFT", Tone::Dim);
            c.entity(l1).add_child(d);
        }
        let (cl, ct) = p.checks.label();
        let chk = tone_chip(c, ui, &cl, ct);
        let rv = tone_chip(c, ui, p.review.label(), p.review.tone());
        let ch = ui.text(c, format!("+{} −{}", p.additions, p.deletions), &ui.f.mono, 11.0, 0.0, ui.dim);
        c.entity(l1).add_children(&[chk, rv, ch]);
        let l2 = ui.row(c, 8.0);
        let sub = ui.text(c, clip(&format!("{} → {}  ·  {}  ·  {}", clip_start(&p.head, 30), clip_start(&p.base, 14), clip(&p.author, 16), age(&p.updated)), 90), &ui.f.mono, 10.5, 0.0, ui.dim);
        c.entity(l2).add_child(sub);
        c.entity(row).add_children(&[l1, l2]);
        c.entity(col).add_child(row);
    }
}

/// "3d ago" from an ISO timestamp, against the clock.
fn age(iso: &str) -> String {
    let secs = prs::iso_secs(iso).map(|t| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0) - t);
    match secs {
        Some(s) if s < 90 => "just now".into(),
        Some(s) if s < 5400 => format!("{}m ago", s / 60),
        Some(s) if s < 129_600 => format!("{}h ago", s / 3600),
        Some(s) => format!("{}d ago", s / 86400),
        None => String::new(),
    }
}

/// The selected pull request: its checks and its diff (files, then the first lines of the first change).
fn draw_pr_detail(c: &mut Commands, ui: &Ui, root: Entity, app: &GitApp) {
    let b = ui.body(c, root);
    let Some(p) = app.pr() else {
        let h = ui.header(c, "Pull request", "", ui.dim);
        c.entity(b).add_child(h);
        return;
    };
    let armed = app.arm.is_some_and(|(n, _)| n == p.number);
    let h = ui.header(c, &format!("#{} · {}", p.number, clip(&p.title, 60)), if armed { "PRESS M AGAIN TO MERGE" } else { "" }, role_color(Role::Bad));
    let inner = c.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(18), padding: UiRect { left: px(16), right: px(16), top: px(10), bottom: px(6) }, flex_grow: 1.0, overflow: Overflow::clip(), ..default() }).id();
    c.entity(b).add_children(&[h, inner]);
    let d = app.detail.as_ref().filter(|d| d.number == p.number);
    let left = c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(3), width: px(230), flex_shrink: 0.0, ..default() }).id();
    let right = c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(2), flex_grow: 1.0, flex_shrink: 1.0, overflow: Overflow::clip(), ..default() }).id();
    c.entity(inner).add_children(&[left, right]);
    let (cl, ct) = p.checks.label();
    let lh = ui.text(c, format!("CHECKS  {}", cl), &ui.f.ui600, 10.5, 2.0, tone_color(ui, ct));
    c.entity(left).add_child(lh);
    match d {
        None => {
            let t = ui.text(c, "Reading…", &ui.f.ui500, 12.0, 0.4, ui.dim);
            c.entity(left).add_child(t);
        }
        Some(d) => {
            if d.checks.is_empty() {
                let t = ui.text(c, "No checks reported", &ui.f.ui500, 12.0, 0.4, ui.dim);
                c.entity(left).add_child(t);
            }
            for (name, state, tone) in d.checks.iter().take(6) {
                let r = ui.row(c, 6.0);
                let dot = c.spawn((Node { width: px(6), height: px(6), flex_shrink: 0.0, ..default() }, BackgroundColor(tone_color(ui, *tone)))).id();
                let t = ui.text(c, clip(name, 22), &ui.f.mono, 11.0, 0.0, ui.line);
                let st = ui.text(c, state.clone(), &ui.f.ui600, 9.5, 1.0, tone_color(ui, *tone));
                c.entity(r).add_children(&[dot, t, st]);
                c.entity(left).add_child(r);
            }
            if d.checks.len() > 6 {
                let t = ui.text(c, format!("+{} more", d.checks.len() - 6), &ui.f.ui500, 11.0, 0.4, ui.dim);
                c.entity(left).add_child(t);
            }
        }
    }
    let nfiles = d.map(|d| d.files.len()).unwrap_or(0);
    let rh = ui.text(c, format!("DIFF  {} → {}  ·  {nfiles} FILE{}  +{} −{}", clip_start(&p.head, 24).to_uppercase(), clip_start(&p.base, 12).to_uppercase(), if nfiles == 1 { "" } else { "S" }, p.additions, p.deletions), &ui.f.ui600, 10.5, 1.6, ui.hot);
    c.entity(right).add_child(rh);
    if let Some(d) = d {
        if let Some(e) = &d.error {
            let t = ui.text(c, clip(e, 70), &ui.f.ui500, 12.0, 0.4, role_color(Role::Bad));
            c.entity(right).add_child(t);
        }
        for f in d.files.iter().take(4) {
            let r = ui.row(c, 8.0);
            let st = ui.text(c, f.status.letter(), &ui.f.mono, 11.5, 0.0, role_color(match f.status { crate::review::Status::Added => Role::Ok, crate::review::Status::Deleted => Role::Bad, _ => Role::Acc }));
            let pt = c.spawn((Text::new(clip_start(&f.path, 52)), font(&ui.f.mono, 11.5, 0.0), TextColor(ui.line), TextLayout { linebreak: LineBreak::NoWrap, ..default() }, Node { flex_grow: 1.0, flex_shrink: 1.0, overflow: Overflow::clip(), ..default() })).id();
            let n = ui.text(c, if let Some(note) = &f.note { clip(note, 12) } else { format!("+{} −{}", f.added, f.removed) }, &ui.f.mono, 11.0, 0.0, ui.dim);
            c.entity(r).add_children(&[st, pt, n]);
            c.entity(right).add_child(r);
        }
        if d.files.len() > 4 {
            let t = ui.text(c, format!("+{} more files", d.files.len() - 4), &ui.f.ui500, 11.0, 0.4, ui.dim);
            c.entity(right).add_child(t);
        }
        // the first lines of the first change
        if let Some(h) = d.files.first().and_then(|f| f.hunks.first()) {
            let pv = c.spawn((Node { flex_direction: FlexDirection::Column, margin: UiRect::top(px(4)), padding: UiRect::top(px(4)), border: UiRect::top(px(1)), overflow: Overflow::clip(), flex_shrink: 0.0, ..default() }, BorderColor::all(with_alpha(ui.line, 0.2)))).id();
            for l in h.lines.iter().take(4) {
                let (mark, colr, bg) = match l.kind {
                    crate::review::LineKind::Add => ('+', role_color(Role::Ok), with_alpha(role_color(Role::Ok), 0.08)),
                    crate::review::LineKind::Del => ('-', role_color(Role::Bad), with_alpha(role_color(Role::Bad), 0.10)),
                    crate::review::LineKind::Context => (' ', ui.line, Color::NONE),
                };
                let t = c.spawn((Text::new(clip(&format!("{mark}{}", l.text), 80)), font(&ui.f.mono, 11.0, 0.0), TextColor(colr), LineHeight::Px(14.0), BackgroundColor(bg), TextLayout { linebreak: LineBreak::NoWrap, ..default() })).id();
                c.entity(pv).add_child(t);
            }
            c.entity(right).add_child(pv);
        }
    }
    // the actions, as buttons for the keys
    let acts = c.spawn(Node { position_type: PositionType::Absolute, right: px(16), bottom: px(10), column_gap: px(8), ..default() }).id();
    for (label, key, hot) in [("ENTER  OPEN ON GITHUB", KeyCode::Enter, false), ("C  CHECK OUT", KeyCode::KeyC, false), (if armed { "M  MERGE NOW" } else { "M  MERGE" }, KeyCode::KeyM, armed)] {
        let colr = if hot { role_color(Role::Bad) } else { ui.line };
        let btn = c.spawn((Node { padding: UiRect { left: px(8), right: px(8), top: px(4), bottom: px(3) }, border: UiRect::all(px(1)), ..default() }, BorderColor::all(with_alpha(colr, 0.6)), BackgroundColor(if hot { with_alpha(colr, 0.15) } else { with_alpha(ui.line, 0.05) }), Click::Key(key))).id();
        let t = ui.text(c, label, &ui.f.ui700, 10.0, 1.4, colr);
        c.entity(btn).add_child(t);
        c.entity(acts).add_child(btn);
    }
    c.entity(b).add_child(acts);
}

fn draw_commit(c: &mut Commands, ui: &Ui, root: Entity, r: &Repo, amend: bool, busy: bool) {
    let b = ui.body(c, root);
    let repo = &r.card.name;
    let state = r.in_progress.map(|p| format!(" · {}", p.to_uppercase())).unwrap_or_default();
    let h = ui.header(c, "Commit", &format!("{} · {}{state}", clip(repo, 20), clip_start(&r.card.branch, 28)), if r.in_progress.is_some() { role_color(Role::Acc) } else { ui.dim });
    let inner = c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(10), padding: UiRect { left: px(16), right: px(16), top: px(12), bottom: px(10) }, flex_grow: 1.0, ..default() }).id();
    c.entity(b).add_children(&[h, inner]);
    let box_ = c.spawn((Node { height: px(84), padding: UiRect::all(px(12)), border: UiRect::all(px(1)), ..default() }, BorderColor::all(with_alpha(ui.line, 0.4)), BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.3)), Click::Key(KeyCode::KeyC))).id();
    let hint = if r.card.detached {
        "Detached HEAD: a commit here belongs to no branch. B switches to one first"
    } else if r.staged.is_empty() && !amend {
        "Nothing staged. S stages the selected file, A everything"
    } else if amend {
        "C amends the last commit: Enter in the bar keeps its message, or type a new one"
    } else {
        "C writes the commit message (in the bar below)"
    };
    let ph = c.spawn((Text::new(hint), font(&ui.f.ui500, 14.0, 0.6), TextColor(ui.dim), LineHeight::Px(19.0))).id();
    c.entity(box_).add_child(ph);
    let row = ui.row(c, 14.0);
    let n = r.staged.len();
    let what = ui.text(c, format!("{n} FILE{} TO {}", if n == 1 { "" } else { "S" }, clip_start(&r.card.branch.to_uppercase(), 28)), &ui.f.ui600, 11.0, 1.8, ui.hot);
    let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
    let am_box = c.spawn((Node { width: px(10), height: px(10), border: UiRect::all(px(1)), ..default() }, BorderColor::all(ui.line), BackgroundColor(if amend { ui.hot } else { Color::NONE }), Click::Key(KeyCode::KeyM))).id();
    let am = c.spawn((Text::new("AMEND  M"), font(&ui.f.ui600, 10.5, 1.8), TextColor(if amend { ui.hot } else { ui.dim }), Click::Key(KeyCode::KeyM))).id();
    let btn = c.spawn((Node { height: px(30), padding: UiRect::horizontal(px(14)), align_items: AlignItems::Center, column_gap: px(14), ..default() }, BackgroundColor(if n > 0 || amend { ui.hot } else { with_alpha(ui.line, 0.2) }), Click::Key(KeyCode::KeyC))).id();
    let bt = ui.text(c, if busy { "WORKING…" } else { "COMMIT" }, &ui.f.ui700, 11.5, 2.6, Color::BLACK);
    let bk = ui.text(c, "C", &ui.f.ui700, 10.0, 0.5, Color::BLACK);
    c.entity(btn).add_children(&[bt, bk]);
    c.entity(row).add_children(&[what, sp, am_box, am, btn]);
    let last = match &r.last {
        Some((hsh, s, a, t)) => format!("{hsh}  {}  ·  {}  ·  {t}", clip(s, 46), clip(a, 16)),
        None => "no commits yet".into(),
    };
    let lt = ui.text(c, last, &ui.f.mono, 11.0, 0.0, ui.dim);
    c.entity(inner).add_children(&[box_, row, lt]);
}

fn draw_status(c: &mut Commands, ui: &Ui, root: Entity, r: &Repo, cursor: usize, active: bool, preview: &[(char, String)]) {
    let b = ui.body(c, root);
    let h = ui.header(c, "Working tree", &format!("{} staged", r.staged.len()), ui.dim);
    let inner = c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(2), padding: UiRect { left: px(12), right: px(12), top: px(8), ..default() }, flex_grow: 1.0, ..default() }).id();
    c.entity(b).add_children(&[h, inner]);
    let mut idx = 0usize;
    for (title, list, staged, act) in [("STAGED", &r.staged, true, "X UNSTAGE ALL"), ("UNSTAGED", &r.unstaged, false, "A STAGE ALL")] {
        let hr = c.spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, height: px(24), margin: UiRect::top(px(4)), flex_shrink: 0.0, ..default() }).id();
        let start = if cursor >= idx && cursor < idx + list.len() { first_row(cursor - idx, list.len(), FILE_ROWS) } else { 0 };
        let range = if list.len() > FILE_ROWS { format!("  {}–{}/{}", start + 1, (start + FILE_ROWS).min(list.len()), list.len()) } else { format!("  {}", list.len()) };
        let t = ui.text(c, format!("{title}{range}"), &ui.f.ui600, 10.5, 2.0, ui.hot);
        let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
        let a = c.spawn((Node { padding: UiRect { left: px(5), right: px(5), top: px(2), bottom: px(1) }, border: UiRect::all(px(1)), ..default() }, BorderColor::all(with_alpha(ui.line, 0.5)), Click::Key(if staged { KeyCode::KeyX } else { KeyCode::KeyA }))).id();
        let at = ui.text(c, act, &ui.f.ui700, 9.5, 1.2, ui.line);
        c.entity(a).add_child(at);
        c.entity(hr).add_children(&[t, sp, a]);
        c.entity(inner).add_child(hr);
        for (k, ch) in list.iter().enumerate() {
            let i = idx + k;
            if k < start || k >= start + FILE_ROWS {
                continue;
            }
            let sel = i == cursor;
            let row = c
                .spawn((
                    Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, height: px(22), column_gap: px(8), padding: UiRect::left(px(6)), border: UiRect::left(px(if sel { 2.0 } else { 0.0 })), flex_shrink: 0.0, ..default() },
                    BorderColor::all(if active { ui.hot } else { with_alpha(ui.hot, 0.4) }),
                    BackgroundColor(if sel { with_alpha(ui.line, if active { 0.10 } else { 0.05 }) } else { Color::NONE }),
                    Click::Pick(i),
                ))
                .id();
            let cc = match ch.code {
                'A' | '?' => role_color(Role::Ok),
                'D' | 'U' => role_color(Role::Bad),
                _ => role_color(Role::Acc),
            };
            let code = ui.text(c, ch.code.to_string(), &ui.f.mono, 12.0, 0.0, cc);
            // long paths keep their end (the file name)
            let shown = clip_start(&ch.path, 24);
            let p = c.spawn((Text::new(shown), font(&ui.f.mono, 12.0, 0.0), TextColor(if sel { ui.hot } else { ui.line }), TextLayout { linebreak: LineBreak::NoWrap, ..default() }, Node { flex_grow: 1.0, flex_shrink: 1.0, overflow: Overflow::clip(), ..default() })).id();
            let n = match (ch.added, ch.removed) {
                (Some(a), Some(d)) => format!("+{a} −{d}"),
                _ if ch.code == '?' => if ch.path.ends_with('/') { "folder".into() } else { "new".into() },
                _ if ch.code == 'U' => "conflict".into(),
                _ => "bin".into(),
            };
            let nt = ui.text(c, n, &ui.f.mono, 11.0, 0.0, ui.dim);
            let bx = c.spawn((Node { width: px(11), height: px(11), border: UiRect::all(px(1)), flex_shrink: 0.0, ..default() }, BorderColor::all(ui.line), BackgroundColor(if staged { ui.hot } else { Color::NONE }), Click::Alt(STAGE_ALT + i))).id();
            c.entity(row).add_children(&[code, p, nt, bx]);
            c.entity(inner).add_child(row);
        }
        if list.is_empty() {
            let e = ui.text(c, if staged { "Nothing staged" } else { "Clean" }, &ui.f.ui500, 12.0, 0.4, ui.dim);
            c.entity(inner).add_child(e);
        }
        idx += list.len();
    }
    let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
    c.entity(inner).add_child(sp);
    if !preview.is_empty() {
        let pv = c.spawn((Node { flex_direction: FlexDirection::Column, padding: UiRect { top: px(6), bottom: px(8), ..default() }, border: UiRect::top(px(1)), overflow: Overflow::clip(), flex_shrink: 0.0, ..default() }, BorderColor::all(with_alpha(ui.line, 0.2)))).id();
        for (k, l) in preview.iter().take(7) {
            let col = match k {
                '+' => role_color(Role::Ok),
                '-' => role_color(Role::Bad),
                '@' => ui.dim,
                _ => ui.line,
            };
            let bg = match k {
                '+' => with_alpha(role_color(Role::Ok), 0.08),
                '-' => with_alpha(role_color(Role::Bad), 0.10),
                _ => Color::NONE,
            };
            let t = c.spawn((Text::new(clip(&format!("{}{}", if *k == '@' { ' ' } else { *k }, l), 40)), font(&ui.f.mono, 11.0, 0.0), TextColor(col), LineHeight::Px(15.0), BackgroundColor(bg), TextLayout { linebreak: LineBreak::NoWrap, ..default() })).id();
            c.entity(pv).add_child(t);
        }
        c.entity(inner).add_child(pv);
    }
}

fn draw_remote(c: &mut Commands, ui: &Ui, root: Entity, r: &Repo, busy: bool) {
    let b = ui.body(c, root);
    let h = ui.header(c, "Remote", &clip(r.upstream.as_deref().map(|u| u.split('/').next().unwrap_or(u)).unwrap_or(if r.has_remote { "no upstream" } else { "no remote" }), 20), ui.dim);
    let inner = c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(10), padding: UiRect { left: px(14), right: px(14), top: px(12), ..default() }, ..default() }).id();
    c.entity(b).add_children(&[h, inner]);
    let row = ui.row(c, 6.0);
    for (label, n, key, col) in [("PULL", r.card.behind, "L", role_color(Role::Acc)), ("PUSH", r.card.ahead, "P", role_color(Role::Ok)), ("FETCH", 0, "F", ui.dim)] {
        let btn = c
            .spawn((
                Node { flex_grow: 1.0, height: px(32), padding: UiRect::horizontal(px(8)), align_items: AlignItems::Center, column_gap: px(5), border: UiRect::all(px(1)), ..default() },
                BorderColor::all(with_alpha(ui.line, if busy { 0.25 } else { 0.55 })),
                BackgroundColor(with_alpha(ui.line, 0.05)),
                Click::Key(match label {
                    "PULL" => KeyCode::KeyL,
                    "PUSH" => KeyCode::KeyP,
                    _ => KeyCode::KeyF,
                }),
            ))
            .id();
        let t = ui.text(c, label, &ui.f.ui600, 10.5, 1.6, if busy { ui.dim } else { ui.hot });
        let nt = ui.text(c, if n > 0 { format!("{}{n}", if label == "PULL" { "↓" } else { "↑" }) } else { String::new() }, &ui.f.mono, 11.0, 0.0, col);
        let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
        let kt = ui.text(c, key, &ui.f.ui700, 9.5, 0.5, ui.dim);
        c.entity(btn).add_children(&[t, nt, sp, kt]);
        c.entity(row).add_child(btn);
    }
    let url = r.remote_url.trim_start_matches("https://").trim_start_matches("git@").replace(':', "/");
    let line = if busy {
        "Working…".to_string()
    } else if !r.has_remote {
        "No remote: add one with git remote add".to_string()
    } else {
        clip(&format!("{} to push · {} to pull · {}", r.card.ahead, r.card.behind, if url.is_empty() { "no origin".into() } else { url }), 44)
    };
    let info = ui.text(c, line, &ui.f.ui500, 11.0, 0.4, ui.dim);
    c.entity(inner).add_children(&[row, info]);
}

fn draw_strip(c: &mut Commands, ui: &Ui, root: Entity, agents: &[TaskView], merged: &std::collections::HashSet<String>, prs_view: bool) {
    let r = c.spawn(Node { width: percent(100), height: percent(100), flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(12), padding: UiRect { left: px(16), right: px(14), ..default() }, ..default() }).id();
    c.entity(root).add_child(r);
    let lab = ui.text(c, "AGENTS", &ui.f.ui600, 10.5, 3.2, ui.dim);
    c.entity(r).add_child(lab);
    for t in agents {
        let (col, sub) = match t.state {
            TaskState::Done => (role_color(Role::Ok), format!("{} · {}", crate::appframe::finished(t, merged), clip(t.branch.as_deref().unwrap_or("in place"), 22))),
            TaskState::NeedsYou => (role_color(Role::Acc), format!("Needs you · {}", clip(&t.status, 24))),
            TaskState::Failed => (role_color(Role::Bad), "Failed".to_string()),
            TaskState::Stopped => (ui.dim, "Stopped".to_string()),
            _ => (ui.line, clip(&t.status, 30)),
        };
        let chip = c.spawn((Node { height: px(36), flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(10), padding: UiRect { left: px(10), right: px(12), ..default() }, border: UiRect::all(px(1)), ..default() }, BorderColor::all(with_alpha(col, 0.5)))).id();
        let dot = c.spawn((Node { width: px(8), height: px(8), ..default() }, BackgroundColor(col), UiTransform { rotation: Rot2::degrees(45.0), ..default() })).id();
        let col2 = c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(1), ..default() }).id();
        let tt = ui.text(c, clip(&t.title, 26), &ui.f.ui600, 12.5, 0.4, ui.hot);
        let st = ui.text(c, sub, &ui.f.ui500, 11.0, 0.4, col);
        c.entity(col2).add_children(&[tt, st]);
        c.entity(chip).add_children(&[dot, col2]);
        c.entity(r).add_child(chip);
    }
    let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
    let legend = c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(4), align_items: AlignItems::FlexEnd, ..default() }).id();
    let (t1, t2) = if prs_view {
        ("J K MOVE   ENTER OPEN ON GITHUB   O OPEN   C CHECK OUT   M M SQUASH MERGE", "R GRAPH   F FETCH   L PULL   P PUSH   [ ] REPO   ESC BACK")
    } else {
        ("J K MOVE   TAB LIST   ENTER STAGE / SWITCH   S U STAGE   A X ALL   C COMMIT   M AMEND", "B BRANCH   F FETCH   L PULL   P PUSH   V GRAPH   R PRS   [ ] REPO   ESC CLOSE")
    };
    let l1 = ui.text(c, t1, &ui.f.ui600, 10.0, 1.6, ui.dim);
    let l2 = ui.text(c, t2, &ui.f.ui600, 10.0, 1.6, ui.dim);
    c.entity(legend).add_children(&[l1, l2]);
    c.entity(r).add_children(&[sp, legend]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reopening_keeps_the_last_read_on_screen() {
        let mut a = GitApp { repos: vec![PathBuf::from("/r/one"), PathBuf::from("/r/two")], sel: 1, repo: Some(Repo::default()), ..Default::default() };
        a.open();
        assert_eq!(a.repos.len(), 2, "the repos stay until the new read is in");
        assert!(a.repo.is_some() && a.sel == 1);
        assert!(a.rediscover, "and the next read looks for new repos");
    }

    #[test]
    fn wheel_targets_per_pane() {
        let i = |p: Pane| Pane::all().iter().position(|x| *x == p).map(|i| i as u8);
        assert_eq!(scroll_target(i(Pane::Graph), true, false), Some(Scroll::Zone(Zone::Graph, KeyCode::KeyJ)));
        assert_eq!(scroll_target(i(Pane::Commit), false, false), Some(Scroll::Zone(Zone::Graph, KeyCode::KeyK)));
        assert_eq!(scroll_target(i(Pane::Branches), true, false), Some(Scroll::Zone(Zone::Branches, KeyCode::KeyJ)));
        assert_eq!(scroll_target(i(Pane::Status), false, false), Some(Scroll::Zone(Zone::Files, KeyCode::KeyK)));
        assert_eq!(scroll_target(i(Pane::Repo(1)), true, false), Some(Scroll::Key(KeyCode::BracketRight)));
        assert_eq!(scroll_target(i(Pane::Remote), true, false), None);
    }
    use crate::gitdata::{Branch, GraphRow};

    fn app() -> GitApp {
        let mut a = GitApp::default();
        let ch = |p: &str| Change { path: p.into(), code: 'M', from: None, added: None, removed: None };
        let graph = ["*", "|\\", "*", "|/", "*"].iter().enumerate().map(|(i, l)| GraphRow { lanes: l.to_string(), hash: if l.starts_with('*') { format!("h{i}") } else { String::new() }, ..Default::default() }).collect();
        a.repo = Some(Repo { staged: vec![ch("a")], unstaged: vec![ch("b"), ch("c")], branches: (0..9).map(|i| Branch { name: format!("b{i}"), ..Default::default() }).collect(), graph, ..Default::default() });
        a
    }

    #[test]
    fn the_repo_window_follows_the_selection() {
        assert_eq!(window_first(0, 12), 0);
        assert_eq!(window_first(2, 12), 0);
        assert_eq!(window_first(3, 12), 1);
        assert_eq!(window_first(11, 12), 9);
        assert_eq!(window_first(1, 2), 0);
        assert_eq!((first_row(0, 20, 6), first_row(5, 20, 6), first_row(6, 20, 6), first_row(19, 20, 6), first_row(2, 3, 6)), (0, 0, 1, 14, 0));
    }

    #[test]
    fn j_and_k_move_in_the_zone_tab_picked() {
        let mut a = app();
        a.step(1);
        a.step(1);
        a.step(1);
        assert_eq!(a.cursor, 2, "files stop at the last one");
        a.step(-5);
        assert_eq!(a.cursor, 0);
        a.next_zone();
        assert_eq!(a.zone, Zone::Branches);
        for _ in 0..20 {
            a.step(1);
        }
        assert_eq!(a.bcur, 8);
        // the graph zone is only on Tab in the list view
        a.next_zone();
        assert_eq!(a.zone, Zone::Files);
        a.flat = true;
        a.next_zone();
        a.next_zone();
        assert_eq!(a.zone, Zone::Graph);
        a.step(1);
        assert_eq!(a.gcur, 2, "connector rows are skipped");
        a.step(1);
        assert_eq!(a.gcur, 4);
        a.step(1);
        assert_eq!(a.gcur, 4);
        a.step(-1);
        assert_eq!(a.gcur, 2);
    }

    #[test]
    fn stepping_with_nothing_loaded_does_nothing() {
        let mut a = GitApp::default();
        a.step(1);
        a.next_zone();
        a.step(-1);
        assert_eq!((a.cursor, a.bcur, a.gcur), (0, 0, 0));
        assert!(a.changes().is_empty());
    }
}
