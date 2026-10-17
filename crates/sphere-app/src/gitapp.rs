//! The Git app (design/apps/git): your repos and their state at a glance, a commit graph across branches, the
//! working tree with staging, a commit box, branches and the remote, over the agents' strip. Everything is read
//! and done with git in the background. Keys: [ ] repo, J K file, S stage, U unstage, A stage all, X unstage all,
//! C commit (with M toggling amend), B switch branch, L pull, P push, Esc closes.
use crate::appframe::{clip, Click, Ui};
use crate::cmdbar::{BarTarget, CmdBar};
use crate::core::CoreCtl;
use crate::fonts::UiFonts;
use crate::gitdata::{self, Change, Op, Repo, RepoCard};
use crate::panel::*;
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
const REPOS: usize = 3;

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

type Loaded = (usize, Repo, Vec<RepoCard>);
type Preview = Vec<(char, String)>;
type Note = Result<String, String>;

#[derive(Resource)]
pub struct GitApp {
    pub open: bool,
    pub amt: Spring,
    repos: Vec<PathBuf>,
    sel: usize,
    repo: Option<Repo>,
    cards: Vec<RepoCard>,
    loading: bool,
    load_rx: Option<Mutex<mpsc::Receiver<Loaded>>>,
    load_t: f32,
    /// the selected change: staged first, then unstaged
    cursor: usize,
    preview: Vec<(char, String)>,
    preview_rx: Option<Mutex<mpsc::Receiver<Preview>>>,
    preview_for: String,
    amend: bool,
    /// the graph: a list instead of the tilted view, and how far the view is turned (degrees)
    flat: bool,
    yaw: f32,
    busy: bool,
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
            load_rx: None,
            load_t: -99.0,
            cursor: 0,
            preview: Vec::new(),
            preview_rx: None,
            preview_for: String::new(),
            amend: false,
            flat: false,
            yaw: 22.0,
            busy: false,
            notes: (tx, Mutex::new(rx)),
            version: 0,
        }
    }
}

impl GitApp {
    pub fn open(&mut self) {
        self.open = true;
        self.load_t = -99.0;
        self.version += 1;
    }
    pub fn close(&mut self) {
        self.open = false;
        self.version += 1;
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

pub fn setup(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut atlas: ResMut<PanelAtlas>, mut pmats: ResMut<Assets<PanelMaterial>>, args: Res<crate::Args>, mut app: ResMut<GitApp>) {
    if args.git {
        app.open();
    }
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
}

#[allow(clippy::too_many_arguments)]
pub fn keys(
    mut ev: MessageReader<bevy::input::keyboard::KeyboardInput>,
    keys: Res<ButtonInput<KeyCode>>,
    (focus, moment, mut bar): (ResMut<Focus>, Res<crate::moment::Moment>, ResMut<CmdBar>),
    mut app: ResMut<GitApp>,
    mut sound: ResMut<crate::sound::Sound>,
    mut toasts: ResMut<crate::live::Toasts>,
    mut clicks: ResMut<crate::appframe::AppClicks>,
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
    for c in clicks.take(1) {
        match c {
            Click::Key(k) => codes.push(k),
            Click::Pick(i) => {
                app.cursor = i;
                app.version += 1;
            }
            Click::Alt(i) => {
                if i < app.repos.len() && i != app.sel {
                    app.sel = i;
                    app.repo = None;
                    app.cursor = 0;
                    app.load_t = -99.0;
                    app.version += 1;
                }
            }
        }
    }
    for key in codes {
        let n = app.changes().len();
        let sel = app.changes().get(app.cursor).map(|(c, staged)| (c.path.clone(), *staged));
        match key {
            KeyCode::Escape => {
                app.close();
                sound.play_with("collapse", 0.7, 0.0);
            }
            KeyCode::KeyJ | KeyCode::ArrowDown if n > 0 => {
                app.cursor = (app.cursor + 1).min(n - 1);
                app.version += 1;
                sound.play_q("tick", 40);
            }
            KeyCode::KeyK | KeyCode::ArrowUp => {
                app.cursor = app.cursor.saturating_sub(1);
                app.version += 1;
                sound.play_q("tick", 40);
            }
            k @ (KeyCode::BracketLeft | KeyCode::BracketRight) => {
                let m = app.repos.len();
                if m > 1 {
                    app.sel = if k == KeyCode::BracketRight { (app.sel + 1) % m } else { (app.sel + m - 1) % m };
                    app.repo = None;
                    app.cursor = 0;
                    app.load_t = -99.0;
                    app.version += 1;
                    sound.play("toggle");
                }
            }
            KeyCode::KeyS => match sel {
                Some((p, false)) => {
                    app.act(Op::Stage(p));
                    sound.play("confirm");
                }
                Some(_) => toasts.push_quiet("Already staged"),
                None => toasts.push_quiet("Nothing to stage"),
            },
            KeyCode::KeyU => match sel {
                Some((p, true)) => {
                    app.act(Op::Unstage(p));
                    sound.play("discard");
                }
                _ => toasts.push_quiet("Pick a staged file to unstage"),
            },
            KeyCode::KeyA => {
                app.act(Op::StageAll);
                sound.play("confirm");
            }
            KeyCode::KeyX => {
                app.act(Op::UnstageAll);
                sound.play("discard");
            }
            KeyCode::KeyM => {
                app.amend = !app.amend;
                app.version += 1;
                sound.play("toggle");
            }
            KeyCode::KeyC => {
                let staged = app.repo.as_ref().map(|r| r.staged.len()).unwrap_or(0);
                if staged == 0 && !app.amend {
                    toasts.push("Stage something first (S, or A for everything)");
                } else if let (Some(d), Some(r)) = (app.dir(), &app.repo) {
                    let b = r.card.branch.clone();
                    bar.prompt(BarTarget::Commit(d.to_string_lossy().to_string(), b), &mut focus);
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
                app.version += 1;
                sound.play("toggle");
            }
            k @ (KeyCode::Comma | KeyCode::Period) => {
                app.yaw = (app.yaw + if k == KeyCode::Comma { -10.0 } else { 10.0 }).clamp(-60.0, 40.0);
                app.version += 1;
                sound.play_q("tick", 40);
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
pub fn update(sim: Res<crate::core::Sim>, mut app: ResMut<GitApp>, mut ctl: ResMut<CoreCtl>, svc: Res<Service>, mut bar: ResMut<CmdBar>, mut toasts: ResMut<crate::live::Toasts>, mut sound: ResMut<crate::sound::Sound>) {
    app.amt.t = if app.open { 1.0 } else { 0.0 };
    app.amt.step(sim.dt);
    ctl.app = ctl.app.max(clamp(app.amt.x, 0.0, 1.0));
    // what you typed in the bar for us
    if let Some((t, text)) = bar.out.take() {
        match t {
            BarTarget::Commit(..) => {
                let amend = app.amend;
                app.act(Op::Commit { msg: text, amend });
                app.amend = false;
                sound.play("done");
            }
            BarTarget::Branch(..) => {
                app.act(Op::Switch(text));
                sound.play("whoosh");
            }
            other => bar.out = Some((other, text)),
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
    let got = app.load_rx.as_ref().and_then(|r| r.lock().ok()?.try_recv().ok());
    if let Some((sel, repo, cards)) = got {
        app.loading = false;
        // look tests: SPHERE_GIT_REPO=n starts on the nth repo
        if let Some(n) = std::env::var("SPHERE_GIT_REPO").ok().and_then(|v| v.parse::<usize>().ok()).filter(|n| *n != app.sel && app.repo.is_none() && *n < app.repos.len()) {
            app.sel = n;
            app.load_t = -99.0;
            app.cards = cards;
            return;
        }
        if sel == app.sel {
            let n = repo.staged.len() + repo.unstaged.len();
            if app.cursor >= n {
                app.cursor = n.saturating_sub(1);
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
    // the file under the cursor: its change, for the preview
    let want = app.changes().get(app.cursor).map(|(c, s)| (c.path.clone(), *s, c.code == '?'));
    let key = want.as_ref().map(|w| format!("{}{}{}", w.0, w.1, app.repo.as_ref().map(|r| r.unstaged.len() + r.staged.len()).unwrap_or(0))).unwrap_or_default();
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
            _ => app.preview.clear(),
        }
    }
    // read the repos on open, after an action, and every 4 s (you may change things elsewhere)
    if app.loading || sim.t - app.load_t < 4.0 {
        return;
    }
    app.load_t = sim.t;
    app.loading = true;
    if app.repos.is_empty() || app.repo.is_none() {
        let agent_repos: Vec<String> = {
            let s = svc.shared.lock().unwrap();
            let mut v: Vec<&TaskView> = s.tasks.iter().collect();
            v.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
            v.into_iter().map(|t| t.repo.clone()).collect()
        };
        if app.repos.is_empty() {
            app.repos = gitdata::discover(&agent_repos, REPOS);
        }
    }
    let (repos, sel) = (app.repos.clone(), app.sel);
    let (tx, rx) = mpsc::channel();
    app.load_rx = Some(Mutex::new(rx));
    std::thread::spawn(move || {
        let Some(dir) = repos.get(sel) else { return };
        let repo = gitdata::load(dir);
        let cards = repos.iter().map(|p| gitdata::card(p)).collect();
        let _ = tx.send((sel, repo, cards));
    });
}

#[allow(clippy::type_complexity)]
pub fn place(app: Res<GitApp>, mut q: Query<(&mut Panel, &GitPart)>) {
    let amt = clamp(app.amt.x, 0.0, 1.0);
    for (mut p, part) in &mut q {
        // a repo card with no repo stays away
        let a = match part.0 {
            Some(Pane::Repo(i)) if i as usize >= app.cards.len() => 0.0,
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
    let (agents, sver) = {
        let s = svc.shared.lock().unwrap();
        let mut v: Vec<TaskView> = s.tasks.clone();
        v.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        v.truncate(3);
        (v, s.version)
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
    for i in 0..REPOS {
        draw_card(c, &ui, root(Pane::Repo(i as u8)), app.cards.get(i), i == app.sel, i);
    }
    draw_strip(c, &ui, root(Pane::Strip), &agents);
    let Some(r) = &app.repo else {
        let b = ui.body(c, root(Pane::Graph));
        let h = ui.header(c, "Graph", "", ui.dim);
        let m = c.spawn((Text::new(if app.repos.is_empty() && !app.loading { "No git repos found (agents' repos and ~/Projects)" } else { "Reading…" }), font(&f.ui500, 13.0, 0.5), TextColor(ui.line), Node { margin: UiRect::all(px(18)), ..default() })).id();
        c.entity(b).add_children(&[h, m]);
        for p in [Pane::Branches, Pane::Commit, Pane::Status, Pane::Remote] {
            let b = ui.body(c, root(p));
            let h = ui.header(c, match p { Pane::Branches => "Branches", Pane::Commit => "Commit", Pane::Status => "Working tree", _ => "Remote" }, "", ui.dim);
            c.entity(b).add_child(h);
        }
        return;
    };
    if let Some(e) = &r.error {
        let b = ui.body(c, root(Pane::Graph));
        let h = ui.header(c, "Graph", "", ui.dim);
        let m = ui.text(c, clip(e, 80), &f.ui500, 13.0, 0.4, role_color(Role::Bad));
        c.entity(b).add_children(&[h, m]);
        return;
    }
    draw_branches(c, &ui, root(Pane::Branches), r);
    if app.flat {
        draw_graph(c, &ui, root(Pane::Graph), r);
    } else {
        draw_graph3d(c, &ui, root(Pane::Graph), r, app.yaw);
    }
    draw_commit(c, &ui, root(Pane::Commit), r, app.amend, app.busy);
    draw_status(c, &ui, root(Pane::Status), r, app.cursor, &app.preview, &app.preview_for);
    draw_remote(c, &ui, root(Pane::Remote), r);
}

fn draw_top(c: &mut Commands, ui: &Ui, root: Entity, app: &GitApp) {
    let r = c.spawn(Node { width: percent(100), height: percent(100), flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(12), ..default() }).id();
    c.entity(root).add_child(r);
    let name = ui.text(c, "GIT", &ui.f.ui600, 11.0, 3.5, ui.hot);
    let names: Vec<String> = app.cards.iter().map(|c| c.name.to_uppercase()).collect();
    let sub = ui.text(c, names.join(" · "), &ui.f.ui500, 11.0, 2.6, ui.dim);
    let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
    let credit = ui.text(c, "git UI after gitui (MIT) · git", &ui.f.ui500, 10.5, 1.0, ui.dim);
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
    let nm = ui.text(c, k.name.to_uppercase(), &ui.f.display, 13.0, 1.6, ui.hot);
    let dot = c.spawn((Node { width: px(6), height: px(6), ..default() }, BackgroundColor(if k.changed > 0 { role_color(Role::Acc) } else { role_color(Role::Ok) }))).id();
    let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
    let lg = ui.text(c, k.lang.clone(), &ui.f.ui500, 10.0, 2.0, ui.dim);
    c.entity(top).add_children(&[nm, dot, sp, lg]);
    let br = ui.text(c, format!("⌥ {}", clip(&k.branch, 24)), &ui.f.mono, 11.5, 0.0, ui.line);
    let st = ui.row(c, 8.0);
    let a = ui.text(c, format!("↑{}", k.ahead), &ui.f.mono, 11.0, 0.0, role_color(Role::Ok));
    let bh = ui.text(c, format!("↓{}", k.behind), &ui.f.mono, 11.0, 0.0, role_color(Role::Acc));
    let ch = ui.text(c, format!("{} CHANGED", k.changed), &ui.f.ui600, 10.0, 1.8, ui.dim);
    let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
    let stash = ui.text(c, if k.stashes > 0 { format!("STASH {}", k.stashes) } else { String::new() }, &ui.f.ui600, 9.5, 1.4, ui.dim);
    c.entity(st).add_children(&[a, bh, ch, sp, stash]);
    c.entity(b).add_children(&[top, br, st]);
}

fn draw_branches(c: &mut Commands, ui: &Ui, root: Entity, r: &Repo) {
    let b = ui.body(c, root);
    let h = ui.header(c, "Branches", &format!("{} local", r.branches.len()), ui.dim);
    let sw = c.spawn((Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, height: px(28), margin: UiRect { left: px(12), right: px(12), top: px(10), bottom: px(6) }, padding: UiRect::horizontal(px(10)), border: UiRect::all(px(1)), column_gap: px(8), ..default() }, BorderColor::all(with_alpha(ui.line, 0.4)), Click::Key(KeyCode::KeyB))).id();
    let st = ui.text(c, "› Switch branch…", &ui.f.ui500, 12.5, 0.5, ui.dim);
    let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
    let kb = c.spawn((Node { padding: UiRect { left: px(4), right: px(4), top: px(1), bottom: px(0) }, border: UiRect::all(px(1)), ..default() }, BorderColor::all(with_alpha(ui.line, 0.5)))).id();
    let kt = ui.text(c, "B", &ui.f.ui700, 9.5, 0.5, ui.line);
    c.entity(kb).add_child(kt);
    c.entity(sw).add_children(&[st, sp, kb]);
    let list = c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(9), padding: UiRect { left: px(14), right: px(12), top: px(4), ..default() }, ..default() }).id();
    c.entity(b).add_children(&[h, sw, list]);
    for br in r.branches.iter().take(6) {
        let row = c.spawn((Node { flex_direction: FlexDirection::Row, column_gap: px(10), padding: UiRect::left(px(4)), border: UiRect::left(px(if br.current { 2.0 } else { 0.0 })), ..default() }, BorderColor::all(ui.hot))).id();
        let mk = c
            .spawn((
                Node { width: px(7), height: px(7), margin: UiRect::top(px(4)), border: UiRect::all(px(1)), flex_shrink: 0.0, ..default() },
                BackgroundColor(if br.current { ui.hot } else { Color::NONE }),
                BorderColor::all(if br.agent { role_color(Role::Fill) } else { ui.line }),
                UiTransform { rotation: Rot2::degrees(if br.agent { 45.0 } else { 0.0 }), ..default() },
            ))
            .id();
        let col = c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(1), flex_grow: 1.0, overflow: Overflow::clip(), ..default() }).id();
        let nm = ui.text(c, clip(&br.name, 26), &ui.f.mono, 12.0, 0.0, if br.current { ui.hot } else { ui.line });
        let sub = ui.text(c, format!("{}{}", if br.agent { "AGENT · " } else { "" }, br.age.to_uppercase()), &ui.f.ui600, 9.5, 1.6, ui.dim);
        c.entity(col).add_children(&[nm, sub]);
        let tr = ui.text(c, if br.ahead + br.behind > 0 { format!("↑{} ↓{}", br.ahead, br.behind) } else { String::new() }, &ui.f.mono, 10.5, 0.0, role_color(Role::Ok));
        c.entity(row).add_children(&[mk, col, tr]);
        c.entity(list).add_child(row);
    }
}

fn draw_graph(c: &mut Commands, ui: &Ui, root: Entity, r: &Repo) {
    let b = ui.body(c, root);
    let h = ui.header(c, "Graph", "all branches · newest first", ui.dim);
    let list = c.spawn(Node { flex_direction: FlexDirection::Column, padding: UiRect { left: px(16), right: px(16), top: px(8), ..default() }, ..default() }).id();
    c.entity(b).add_children(&[h, list]);
    let lanes_col = [role_color(Role::Hot), role_color(Role::Fill), role_color(Role::Acc), role_color(Role::Ok), ui.line];
    let lanes_w = r.graph.iter().take(GRAPH_ROWS).map(|g| g.lanes.trim_end().chars().count()).max().unwrap_or(1).min(12) as f32 * 7.4 + 4.0;
    for g in r.graph.iter().take(GRAPH_ROWS) {
        let row = c.spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, width: percent(100), height: px(20), column_gap: px(8), overflow: Overflow::clip(), ..default() }).id();
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
            let sj = c.spawn((Text::new(clip(&g.subject, 60)), font(&ui.f.ui500, 13.0, 0.3), TextColor(ui.line), TextLayout { linebreak: LineBreak::NoWrap, ..default() }, Node { flex_grow: 1.0, flex_shrink: 1.0, overflow: Overflow::clip(), ..default() })).id();
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
    let foot = c.spawn((Node { position_type: PositionType::Absolute, left: px(16), bottom: px(10), ..default() }, Click::Key(KeyCode::KeyV))).id();
    let ft = ui.text(c, "< > TURN (, .)   ·   V LIST", &ui.f.ui600, 9.5, 2.0, ui.dim);
    c.entity(foot).add_child(ft);
    c.entity(area).add_child(foot);
}

fn draw_commit(c: &mut Commands, ui: &Ui, root: Entity, r: &Repo, amend: bool, busy: bool) {
    let b = ui.body(c, root);
    let repo = &r.card.name;
    let h = ui.header(c, "Commit", &format!("{repo} · {}", r.card.branch), ui.dim);
    let inner = c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(10), padding: UiRect { left: px(16), right: px(16), top: px(12), bottom: px(10) }, flex_grow: 1.0, ..default() }).id();
    c.entity(b).add_children(&[h, inner]);
    let box_ = c.spawn((Node { height: px(84), padding: UiRect::all(px(12)), border: UiRect::all(px(1)), ..default() }, BorderColor::all(with_alpha(ui.line, 0.4)), BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.3)), Click::Key(KeyCode::KeyC))).id();
    let ph = ui.text(c, "C writes the commit message (in the bar below)", &ui.f.ui500, 14.0, 0.6, ui.dim);
    c.entity(box_).add_child(ph);
    let row = ui.row(c, 14.0);
    let n = r.staged.len();
    let what = ui.text(c, format!("{n} FILE{} TO {}", if n == 1 { "" } else { "S" }, r.card.branch.to_uppercase()), &ui.f.ui600, 11.0, 1.8, ui.hot);
    let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
    let am_box = c.spawn((Node { width: px(10), height: px(10), border: UiRect::all(px(1)), ..default() }, BorderColor::all(ui.line), BackgroundColor(if amend { ui.hot } else { Color::NONE }), Click::Key(KeyCode::KeyM))).id();
    let am = c.spawn((Text::new("AMEND  M"), font(&ui.f.ui600, 10.5, 1.8), TextColor(if amend { ui.hot } else { ui.dim }), Click::Key(KeyCode::KeyM))).id();
    let btn = c.spawn((Node { height: px(30), padding: UiRect::horizontal(px(14)), align_items: AlignItems::Center, column_gap: px(14), ..default() }, BackgroundColor(if n > 0 || amend { ui.hot } else { with_alpha(ui.line, 0.2) }), Click::Key(KeyCode::KeyC))).id();
    let bt = ui.text(c, if busy { "WORKING…" } else { "COMMIT" }, &ui.f.ui700, 11.5, 2.6, Color::BLACK);
    let bk = ui.text(c, "C", &ui.f.ui700, 10.0, 0.5, Color::BLACK);
    c.entity(btn).add_children(&[bt, bk]);
    c.entity(row).add_children(&[what, sp, am_box, am, btn]);
    let last = match &r.last {
        Some((hsh, s, a, t)) => format!("{hsh}  {}  ·  {a}  ·  {t}", clip(s, 50)),
        None => "no commits yet".into(),
    };
    let lt = ui.text(c, last, &ui.f.mono, 11.0, 0.0, ui.dim);
    c.entity(inner).add_children(&[box_, row, lt]);
}

fn draw_status(c: &mut Commands, ui: &Ui, root: Entity, r: &Repo, cursor: usize, preview: &[(char, String)], _for: &str) {
    let b = ui.body(c, root);
    let h = ui.header(c, "Working tree", &format!("{} staged", r.staged.len()), ui.dim);
    let inner = c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(2), padding: UiRect { left: px(12), right: px(12), top: px(8), ..default() }, flex_grow: 1.0, ..default() }).id();
    c.entity(b).add_children(&[h, inner]);
    let mut idx = 0usize;
    for (title, list, staged, act) in [("STAGED", &r.staged, true, "X UNSTAGE ALL"), ("UNSTAGED", &r.unstaged, false, "A STAGE ALL")] {
        let hr = c.spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, height: px(24), margin: UiRect::top(px(4)), ..default() }).id();
        let t = ui.text(c, format!("{title}  {}", list.len()), &ui.f.ui600, 10.5, 2.6, ui.hot);
        let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
        let a = c.spawn((Node { padding: UiRect { left: px(5), right: px(5), top: px(2), bottom: px(1) }, border: UiRect::all(px(1)), ..default() }, BorderColor::all(with_alpha(ui.line, 0.5)), Click::Key(if staged { KeyCode::KeyX } else { KeyCode::KeyA }))).id();
        let at = ui.text(c, act, &ui.f.ui700, 9.5, 1.2, ui.line);
        c.entity(a).add_child(at);
        c.entity(hr).add_children(&[t, sp, a]);
        c.entity(inner).add_child(hr);
        let start = if cursor >= idx && cursor < idx + list.len() { (cursor - idx).saturating_sub(FILE_ROWS - 1) } else { 0 };
        for (k, ch) in list.iter().enumerate() {
            let i = idx + k;
            if k < start || k >= start + FILE_ROWS {
                continue;
            }
            let sel = i == cursor;
            let row = c
                .spawn((
                    Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, height: px(22), column_gap: px(8), padding: UiRect::left(px(6)), border: UiRect::left(px(if sel { 2.0 } else { 0.0 })), ..default() },
                    BorderColor::all(ui.hot),
                    BackgroundColor(if sel { with_alpha(ui.line, 0.10) } else { Color::NONE }),
                    Click::Pick(i),
                ))
                .id();
            let cc = match ch.code {
                'A' | '?' => role_color(Role::Ok),
                'D' => role_color(Role::Bad),
                _ => role_color(Role::Acc),
            };
            let code = ui.text(c, ch.code.to_string(), &ui.f.mono, 12.0, 0.0, cc);
            // long paths keep their end (the file name)
            let n = ch.path.chars().count();
            let shown = if n > 26 { format!("…{}", ch.path.chars().skip(n - 25).collect::<String>()) } else { ch.path.clone() };
            let p = c.spawn((Text::new(shown), font(&ui.f.mono, 12.0, 0.0), TextColor(if sel { ui.hot } else { ui.line }), TextLayout { linebreak: LineBreak::NoWrap, ..default() }, Node { flex_grow: 1.0, overflow: Overflow::clip(), ..default() })).id();
            let n = match (ch.added, ch.removed) {
                (Some(a), Some(d)) => format!("+{a} −{d}"),
                _ if ch.code == '?' => "new".into(),
                _ => "bin".into(),
            };
            let nt = ui.text(c, n, &ui.f.mono, 11.0, 0.0, ui.dim);
            let bx = c.spawn((Node { width: px(9), height: px(9), border: UiRect::all(px(1)), ..default() }, BorderColor::all(ui.line), BackgroundColor(if staged { ui.hot } else { Color::NONE }))).id();
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
        let pv = c.spawn((Node { flex_direction: FlexDirection::Column, padding: UiRect { top: px(6), bottom: px(8), ..default() }, border: UiRect::top(px(1)), ..default() }, BorderColor::all(with_alpha(ui.line, 0.2)))).id();
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

fn draw_remote(c: &mut Commands, ui: &Ui, root: Entity, r: &Repo) {
    let b = ui.body(c, root);
    let h = ui.header(c, "Remote", r.upstream.as_deref().map(|u| u.split('/').next().unwrap_or(u)).unwrap_or("no upstream"), ui.dim);
    let inner = c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(10), padding: UiRect { left: px(14), right: px(14), top: px(12), ..default() }, ..default() }).id();
    c.entity(b).add_children(&[h, inner]);
    let row = ui.row(c, 10.0);
    for (label, n, key, col) in [("PULL", r.card.behind, "L", role_color(Role::Acc)), ("PUSH", r.card.ahead, "P", role_color(Role::Ok))] {
        let btn = c.spawn((Node { flex_grow: 1.0, height: px(32), padding: UiRect::horizontal(px(12)), align_items: AlignItems::Center, column_gap: px(8), border: UiRect::all(px(1)), ..default() }, BorderColor::all(with_alpha(ui.line, 0.55)), BackgroundColor(with_alpha(ui.line, 0.05)), Click::Key(if label == "PULL" { KeyCode::KeyL } else { KeyCode::KeyP }))).id();
        let t = ui.text(c, label, &ui.f.ui600, 11.5, 2.3, ui.hot);
        let nt = ui.text(c, if n > 0 { format!("{}{n}", if label == "PULL" { "↓" } else { "↑" }) } else { String::new() }, &ui.f.mono, 11.5, 0.0, col);
        let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
        let kt = ui.text(c, key, &ui.f.ui700, 9.5, 0.5, ui.dim);
        c.entity(btn).add_children(&[t, nt, sp, kt]);
        c.entity(row).add_child(btn);
    }
    let url = r.remote_url.trim_start_matches("https://").trim_start_matches("git@").replace(':', "/");
    let info = ui.text(c, clip(&format!("{} to push · {} to pull · {}", r.card.ahead, r.card.behind, if url.is_empty() { "no remote".into() } else { url }), 44), &ui.f.ui500, 11.0, 0.4, ui.dim);
    c.entity(inner).add_children(&[row, info]);
}

fn draw_strip(c: &mut Commands, ui: &Ui, root: Entity, agents: &[TaskView]) {
    let r = c.spawn(Node { width: percent(100), height: percent(100), flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(12), padding: UiRect { left: px(16), right: px(14), ..default() }, ..default() }).id();
    c.entity(root).add_child(r);
    let lab = ui.text(c, "AGENTS", &ui.f.ui600, 10.5, 3.2, ui.dim);
    c.entity(r).add_child(lab);
    for t in agents {
        let (col, sub) = match t.state {
            TaskState::Done => (role_color(Role::Ok), format!("Finished · {}", clip(t.branch.as_deref().unwrap_or("in place"), 22))),
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
    let l1 = ui.text(c, "J K FILE   S STAGE   U UNSTAGE   A ALL   C COMMIT   M AMEND", &ui.f.ui600, 10.0, 1.6, ui.dim);
    let l2 = ui.text(c, "B BRANCH   L PULL   P PUSH   [ ] REPO   ESC CLOSE", &ui.f.ui600, 10.0, 1.6, ui.dim);
    c.entity(legend).add_children(&[l1, l2]);
    c.entity(r).add_children(&[sp, legend]);
}
