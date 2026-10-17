//! The Files app (design/apps/files): yazi-style columns (the parent, where you are, a preview), pinned places
//! along the top (your repos, Downloads, agents' worktrees), and a status line. Keys: H J K L (or arrows) move,
//! Enter opens with the default app, / filters, Space selects, Y yanks, P pastes here, D moves to the Trash (asks
//! twice), E opens in Zed (else the default editor), 1-9 pinned places, . shows hidden files, Esc closes.
use crate::appframe::*;
use crate::cmdbar::{BarTarget, CmdBar};
use crate::core::CoreCtl;
use crate::fonts::UiFonts;
use crate::panel::*;
use crate::service::Service;
use crate::util::*;
use bevy::prelude::*;
use bevy::text::LineHeight;
use std::path::{Path, PathBuf};

pub const APP: u8 = 5;
const TOP: usize = 0;
const PINS: usize = 1;
const PARENT: usize = 2;
const CUR: usize = 3;
const PREVIEW: usize = 4;
const STATUS: usize = 5;
const STRIP: usize = 6;
const PANES: [([f32; 5], bool); 7] = [
    ([22.0, 2.0, 1236.0, 32.0, 8.0], true),
    ([14.0, 42.0, 1252.0, 52.0, 30.0], false),
    ([14.0, 106.0, 300.0, 476.0, 14.0], false),
    ([326.0, 106.0, 380.0, 476.0, 34.0], false),
    ([718.0, 106.0, 548.0, 476.0, 24.0], false),
    ([14.0, 594.0, 1252.0, 34.0, 18.0], false),
    ([14.0, 638.0, 1252.0, 50.0, 22.0], false),
];
const ROWS: usize = 18;
const PREVIEW_LINES: usize = 22;

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub name: String,
    pub dir: bool,
    pub size: u64,
}

/// A directory's entries: folders first, then files, by name (hidden ones only when asked).
pub fn list(dir: &Path, hidden: bool) -> Vec<Entry> {
    let mut v: Vec<Entry> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .flatten()
            .take(2000)
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                if !hidden && name.starts_with('.') {
                    return None;
                }
                let md = e.metadata().ok();
                let dir = e.path().is_dir();
                Some(Entry { name, dir, size: md.map(|m| m.len()).unwrap_or(0) })
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    v.sort_by_key(|e| (!e.dir, e.name.to_lowercase()));
    v
}

pub fn human(n: u64) -> String {
    match n {
        n if n >= 1 << 30 => format!("{:.1} GB", n as f64 / (1u64 << 30) as f64),
        n if n >= 1 << 20 => format!("{:.1} MB", n as f64 / (1u64 << 20) as f64),
        n if n >= 1 << 10 => format!("{:.1} KB", n as f64 / 1024.0),
        n => format!("{n} B"),
    }
}

enum Preview {
    Dir(Vec<Entry>),
    Text(Vec<String>, usize),
    Info(String),
}

fn preview(p: &Path, hidden: bool) -> Preview {
    if p.is_dir() {
        return Preview::Dir(list(p, hidden));
    }
    let Ok(mut f) = std::fs::File::open(p) else { return Preview::Info("can't read it".into()) };
    use std::io::Read;
    let mut buf = vec![0u8; 16 * 1024];
    let n = f.read(&mut buf).unwrap_or(0);
    buf.truncate(n);
    if buf.contains(&0) {
        let size = p.metadata().map(|m| m.len()).unwrap_or(0);
        return Preview::Info(format!("binary · {}", human(size)));
    }
    let text = String::from_utf8_lossy(&buf);
    let total = if n < 16 * 1024 { text.lines().count() } else { std::fs::read_to_string(p).map(|s| s.lines().count()).unwrap_or(0) };
    Preview::Text(text.lines().take(PREVIEW_LINES).map(|l| l.replace('\t', "    ")).collect(), total)
}

fn copy_into(src: &Path, dir: &Path) -> std::io::Result<()> {
    let name = src.file_name().unwrap_or_default();
    let mut dst = dir.join(name);
    // never overwrite: "name copy", "name copy 2"…
    let mut k = 1;
    while dst.exists() {
        let stem = Path::new(name).file_stem().unwrap_or_default().to_string_lossy().to_string();
        let ext = Path::new(name).extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        dst = dir.join(if k == 1 { format!("{stem} copy{ext}") } else { format!("{stem} copy {k}{ext}") });
        k += 1;
    }
    if src.is_dir() {
        std::fs::create_dir_all(&dst)?;
        for e in std::fs::read_dir(src)?.flatten() {
            copy_into(&e.path(), &dst)?;
        }
        Ok(())
    } else {
        std::fs::copy(src, &dst).map(|_| ())
    }
}

/// Moves to the Trash (~/.Trash), keeping the name unique.
fn trash(p: &Path) -> Result<(), String> {
    let home = std::env::var("HOME").map_err(|_| "no HOME".to_string())?;
    let bin = PathBuf::from(home).join(".Trash");
    let name = p.file_name().ok_or("no name")?.to_string_lossy().to_string();
    let mut dst = bin.join(&name);
    let mut k = 2;
    while dst.exists() {
        dst = bin.join(format!("{name} {k}"));
        k += 1;
    }
    std::fs::rename(p, &dst).map_err(|e| format!("Could not move {name} to the Trash: {e}"))
}

#[derive(Resource)]
pub struct FilesApp {
    pub open: bool,
    pub amt: Spring,
    cwd: PathBuf,
    entries: Vec<Entry>,
    parent: Vec<Entry>,
    cursor: usize,
    filter: String,
    hidden: bool,
    selected: Vec<PathBuf>,
    yanked: Vec<PathBuf>,
    pins: Vec<(String, PathBuf)>,
    armed: Option<f32>,
    version: u64,
}

impl Default for FilesApp {
    fn default() -> Self {
        FilesApp { open: false, amt: Spring::new(0.0, 26.0, 9.0), cwd: PathBuf::new(), entries: Vec::new(), parent: Vec::new(), cursor: 0, filter: String::new(), hidden: false, selected: Vec::new(), yanked: Vec::new(), pins: Vec::new(), armed: None, version: 0 }
    }
}

impl FilesApp {
    pub fn open(&mut self) {
        self.open = true;
        if self.cwd.as_os_str().is_empty() {
            let home = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into()));
            let projects = home.join("Projects");
            // look tests: SPHERE_FILES_DIR starts there
            let start = std::env::var("SPHERE_FILES_DIR").map(PathBuf::from).ok().filter(|p| p.is_dir());
            self.go(start.unwrap_or(if projects.is_dir() { projects } else { home }), None);
        } else {
            self.reload();
        }
        self.version += 1;
    }
    pub fn close(&mut self) {
        self.open = false;
        self.armed = None;
        self.version += 1;
    }
    fn shown(&self) -> Vec<&Entry> {
        let f = self.filter.to_lowercase();
        self.entries.iter().filter(|e| f.is_empty() || e.name.to_lowercase().contains(&f)).collect()
    }
    fn current(&self) -> Option<PathBuf> {
        self.shown().get(self.cursor).map(|e| self.cwd.join(&e.name))
    }
    fn reload(&mut self) {
        self.entries = list(&self.cwd, self.hidden);
        self.parent = self.cwd.parent().map(|p| list(p, self.hidden)).unwrap_or_default();
        let n = self.shown().len();
        if self.cursor >= n {
            self.cursor = n.saturating_sub(1);
        }
        self.version += 1;
    }
    /// Goes to `dir`, with the cursor on `on` when given.
    fn go(&mut self, dir: PathBuf, on: Option<String>) {
        self.cwd = dir;
        self.filter.clear();
        self.cursor = 0;
        self.reload();
        if let Some(name) = on {
            if let Some(i) = self.shown().iter().position(|e| e.name == name) {
                self.cursor = i;
            }
        }
    }
}

/// Your repos under ~/Projects, Downloads, then the agents' worktrees: (label, path).
fn pins(tasks: &[sphere_proto::TaskView]) -> Vec<(String, PathBuf)> {
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into()));
    let mut v: Vec<(String, PathBuf)> = Vec::new();
    let projects = home.join("Projects");
    let mut repos: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(&projects)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.join(".git").is_dir()).map(|p| (p.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH), p)).collect())
        .unwrap_or_default();
    repos.sort_by_key(|r| std::cmp::Reverse(r.0));
    for (_, p) in repos.into_iter().take(4) {
        v.push((p.file_name().unwrap_or_default().to_string_lossy().to_string(), p));
    }
    let dl = home.join("Downloads");
    if dl.is_dir() {
        v.push(("Downloads".into(), dl));
    }
    for t in tasks.iter().filter(|t| !t.in_place).take(9usize.saturating_sub(v.len())) {
        if let Some(wd) = &t.workdir {
            if Path::new(wd).is_dir() {
                v.push((t.branch.clone().unwrap_or_else(|| t.title.clone()).trim_start_matches("sphere/").to_string(), PathBuf::from(wd)));
            }
        }
    }
    v.truncate(9);
    v
}

#[derive(Resource)]
pub struct FilesRoots(Workspace);

pub fn setup(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut images: ResMut<Assets<Image>>, (home, mut atlases): (Res<PanelAtlas>, ResMut<AppAtlases>), mut pmats: ResMut<Assets<PanelMaterial>>, args: Res<crate::Args>, mut app: ResMut<FilesApp>) {
    if args.open_app.as_deref() == Some("files") {
        app.open();
    }
    let ws = spawn_workspace(&mut commands, &mut meshes, &mut images, &home, &mut atlases, &mut pmats, APP, &PANES);
    commands.insert_resource(FilesRoots(ws));
}

#[allow(clippy::too_many_arguments)]
pub fn keys(
    mut ev: MessageReader<bevy::input::keyboard::KeyboardInput>,
    keys: Res<ButtonInput<KeyCode>>,
    (focus, moment, mut bar): (ResMut<Focus>, Res<crate::moment::Moment>, ResMut<CmdBar>),
    mut app: ResMut<FilesApp>,
    sim: Res<crate::core::Sim>,
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
        if e.state != bevy::input::ButtonState::Pressed || mods || bar_busy || focus.terminal || moment.shown || (e.repeat && !matches!(e.key_code, KeyCode::KeyJ | KeyCode::KeyK | KeyCode::ArrowDown | KeyCode::ArrowUp)) {
            continue;
        }
        codes.push(e.key_code);
    }
    // clicks on the panes: a button presses its key, a row picks its item
    for c in clicks.take(5) {
        match c {
            Click::Key(k) => codes.push(k),
            Click::Pick(i) => {
                // a second click on the same row opens it
                if i == app.cursor {
                    codes.push(KeyCode::Enter);
                } else {
                    app.cursor = i;
                    app.version += 1;
                }
            }
            Click::Alt(i) => {
                if let Some((_, p)) = app.pins.get(i).cloned() {
                    app.go(p, None);
                }
            }
        }
    }
    for key in codes {
        let n = app.shown().len();
        let cur = app.current();
        match key {
            KeyCode::Escape => {
                if !app.filter.is_empty() {
                    app.filter.clear();
                    app.reload();
                } else {
                    app.close();
                    sound.play_with("collapse", 0.7, 0.0);
                }
            }
            KeyCode::KeyJ | KeyCode::ArrowDown if n > 0 => {
                app.cursor = (app.cursor + 1).min(n - 1);
                app.armed = None;
                app.version += 1;
                sound.play_q("tick", 30);
            }
            KeyCode::KeyK | KeyCode::ArrowUp => {
                app.cursor = app.cursor.saturating_sub(1);
                app.armed = None;
                app.version += 1;
                sound.play_q("tick", 30);
            }
            KeyCode::KeyH | KeyCode::ArrowLeft | KeyCode::Backspace => {
                if let Some(p) = app.cwd.parent().map(|p| p.to_path_buf()) {
                    let name = app.cwd.file_name().map(|n| n.to_string_lossy().to_string());
                    app.go(p, name);
                    sound.play_q("tick", 30);
                }
            }
            KeyCode::KeyL | KeyCode::ArrowRight | KeyCode::Enter => match cur {
                Some(p) if p.is_dir() => {
                    app.go(p, None);
                    sound.play_q("tick", 30);
                }
                Some(p) if key == KeyCode::Enter => {
                    let _ = std::process::Command::new("open").arg(&p).spawn();
                    toasts.push_quiet(format!("Opening {}", p.file_name().unwrap_or_default().to_string_lossy()));
                    sound.play("open");
                }
                _ => {}
            },
            KeyCode::Slash => {
                bar.prompt(BarTarget::Filter, &mut focus);
                sound.play("open");
            }
            KeyCode::Period => {
                app.hidden = !app.hidden;
                app.reload();
                sound.play("toggle");
            }
            KeyCode::Space => {
                if let Some(p) = cur {
                    if let Some(i) = app.selected.iter().position(|s| s == &p) {
                        app.selected.remove(i);
                    } else {
                        app.selected.push(p);
                    }
                    app.cursor = (app.cursor + 1).min(n.saturating_sub(1));
                    app.version += 1;
                    sound.play("toggle");
                }
            }
            KeyCode::KeyY => {
                let picked = if app.selected.is_empty() { cur.into_iter().collect() } else { std::mem::take(&mut app.selected) };
                if !picked.is_empty() {
                    toasts.push_quiet(format!("Yanked {} item{}", picked.len(), if picked.len() == 1 { "" } else { "s" }));
                    app.yanked = picked;
                    app.version += 1;
                    sound.play("confirm");
                }
            }
            KeyCode::KeyP => {
                if app.yanked.is_empty() {
                    toasts.push_quiet("Nothing yanked (Y)");
                } else {
                    let dir = app.cwd.clone();
                    let mut errs = Vec::new();
                    for y in &app.yanked {
                        if let Err(e) = copy_into(y, &dir) {
                            errs.push(e.to_string());
                        }
                    }
                    let k = app.yanked.len();
                    app.reload();
                    match errs.first() {
                        Some(e) => toasts.push(format!("Paste: {e}")),
                        None => toasts.push_quiet(format!("Pasted {k} here")),
                    }
                    sound.play("drop");
                }
            }
            KeyCode::KeyD => {
                let targets: Vec<PathBuf> = if app.selected.is_empty() { cur.into_iter().collect() } else { app.selected.clone() };
                if targets.is_empty() {
                    continue;
                }
                if app.armed.is_some_and(|t| sim.t < t) {
                    app.armed = None;
                    let mut errs = Vec::new();
                    for t in &targets {
                        if let Err(e) = trash(t) {
                            errs.push(e);
                        }
                    }
                    app.selected.clear();
                    app.reload();
                    match errs.first() {
                        Some(e) => toasts.push(e.clone()),
                        None => toasts.push_quiet(format!("Moved {} to the Trash", targets.len())),
                    }
                    sound.play("discard");
                } else {
                    app.armed = Some(sim.t + 3.0);
                    app.version += 1;
                    toasts.push_quiet(format!("Press D again to move {} to the Trash", if targets.len() == 1 { targets[0].file_name().unwrap_or_default().to_string_lossy().to_string() } else { format!("{} items", targets.len()) }));
                    sound.play("toggle");
                }
            }
            KeyCode::KeyE => {
                if let Some(p) = cur {
                    let zed = ["/Applications/Zed.app/Contents/MacOS/cli", "/usr/local/bin/zed", "/opt/homebrew/bin/zed"].into_iter().find(|z| Path::new(z).exists());
                    let r = match zed {
                        Some(z) => std::process::Command::new(z).arg(&p).spawn(),
                        None => std::process::Command::new("open").arg("-t").arg(&p).spawn(),
                    };
                    match r {
                        Ok(_) => toasts.push_quiet(format!("Editing {}", p.file_name().unwrap_or_default().to_string_lossy())),
                        Err(e) => toasts.push(format!("Could not open an editor: {e}")),
                    }
                    sound.play("open");
                }
            }
            k @ (KeyCode::Digit1 | KeyCode::Digit2 | KeyCode::Digit3 | KeyCode::Digit4 | KeyCode::Digit5 | KeyCode::Digit6 | KeyCode::Digit7 | KeyCode::Digit8 | KeyCode::Digit9) => {
                const DIGITS: [KeyCode; 9] = [KeyCode::Digit1, KeyCode::Digit2, KeyCode::Digit3, KeyCode::Digit4, KeyCode::Digit5, KeyCode::Digit6, KeyCode::Digit7, KeyCode::Digit8, KeyCode::Digit9];
                let i = DIGITS.iter().position(|d| *d == k).unwrap_or(0);
                if let Some((_, p)) = app.pins.get(i).cloned() {
                    app.go(p, None);
                    sound.play("whoosh");
                }
            }
            _ => {}
        }
    }
}

pub fn update(sim: Res<crate::core::Sim>, mut atlases: ResMut<AppAtlases>, mut app: ResMut<FilesApp>, mut ctl: ResMut<CoreCtl>, svc: Res<Service>, mut bar: ResMut<CmdBar>, mut seen_tasks: Local<u64>) {
    let open = app.open;
    step_open(open, &mut app.amt, sim.dt, &mut ctl);
    atlases.want(APP, open || app.amt.x > 0.001);
    if app.armed.is_some_and(|t| sim.t > t) {
        app.armed = None;
        app.version += 1;
    }
    if let Some((BarTarget::Filter, text)) = bar.out.clone() {
        bar.out = None;
        app.filter = text;
        app.cursor = 0;
        app.version += 1;
    }
    if !app.open {
        return;
    }
    let s = svc.shared.lock().unwrap();
    if *seen_tasks != s.version + 1 || app.pins.is_empty() {
        *seen_tasks = s.version + 1;
        let p = pins(&s.tasks);
        if p != app.pins {
            app.pins = p;
            app.version += 1;
        }
    }
}

pub fn place(app: Res<FilesApp>, mut q: Query<(&mut Panel, &AppPart)>) {
    place_app(APP, app.amt.x, |_| false, &mut q);
}

fn tilde(p: &Path) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let s = p.to_string_lossy().to_string();
    if !home.is_empty() && s.starts_with(&home) {
        format!("~{}", &s[home.len()..])
    } else {
        s
    }
}

fn icon(e: &Entry) -> &'static str {
    if e.dir {
        "▸"
    } else {
        match Path::new(&e.name).extension().and_then(|x| x.to_str()).unwrap_or("") {
            "rs" | "ts" | "tsx" | "js" | "py" | "go" | "toml" | "json" | "yaml" | "yml" | "sh" | "wesl" | "wgsl" | "html" | "css" => "<>",
            "md" | "txt" => "≡",
            _ => "·",
        }
    }
}

pub fn draw(mut commands: Commands, app: Res<FilesApp>, svc: Res<Service>, roots: Option<Res<FilesRoots>>, f: Res<UiFonts>, theme: Res<crate::theme::Theme>, mut seen: Local<(u64, u64, u64)>) {
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
    let here = app.cwd.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "/".into());
    ui.corelab(c, ws.corelab, "Files", tilde(&app.cwd), ui.line);
    ui.top(c, ws.panes[TOP], "Files", tilde(&app.cwd).to_uppercase(), "files after yazi (MIT) · quick edit in Zed");
    ui.strip(c, ws.panes[STRIP], &agents, ["H J K L MOVE   ENTER OPEN   / FILTER   . HIDDEN   SPACE SELECT", "Y YANK   P PASTE   D TRASH   E EDIT   1-9 PINNED   ESC CLOSE"]);
    // pinned places
    let r = c.spawn(Node { width: percent(100), height: percent(100), flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(8), padding: UiRect::horizontal(px(14)), ..default() }).id();
    c.entity(ws.panes[PINS]).add_child(r);
    let lab = c.spawn(Node { flex_direction: FlexDirection::Column, margin: UiRect::right(px(6)), ..default() }).id();
    let l1 = ui.text(c, "PINNED", &f.ui600, 9.5, 2.4, ui.dim);
    let l2 = ui.text(c, "1-9 to jump", &f.ui500, 10.0, 0.4, ui.dim);
    c.entity(lab).add_children(&[l1, l2]);
    c.entity(r).add_child(lab);
    for (i, (name, p)) in app.pins.iter().enumerate() {
        let on = app.cwd.starts_with(p);
        let agent = p.to_string_lossy().contains("/.sphere/");
        let chip = c.spawn((Node { height: px(36), flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(8), padding: UiRect::horizontal(px(9)), border: UiRect::all(px(1)), ..default() }, BorderColor::all(with_alpha(if on { ui.hot } else { ui.line }, if on { 0.9 } else { 0.35 })), BackgroundColor(if on { with_alpha(ui.line, 0.08) } else { Color::NONE }), Click::Alt(i))).id();
        let k = ui.text(c, format!("{}", i + 1), &f.mono, 10.5, 0.0, ui.dim);
        let col = c.spawn(Node { flex_direction: FlexDirection::Column, ..default() }).id();
        let n = ui.text(c, clip(name, 16), &f.ui600, 12.0, 0.4, if agent { role_color(Role::Fill) } else { ui.hot });
        let sub = ui.text(c, clip(&tilde(p.parent().unwrap_or(p)), 18), &f.mono, 9.5, 0.0, ui.dim);
        c.entity(col).add_children(&[n, sub]);
        c.entity(chip).add_children(&[k, col]);
        c.entity(r).add_child(chip);
    }
    // parent
    let parent_name = app.cwd.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "/".into());
    column(c, &ui, ws.panes[PARENT], &parent_name, &app.parent, app.parent.iter().position(|e| e.name == here), &[], &app.cwd.parent().map(|p| p.to_path_buf()).unwrap_or_default(), false);
    // here
    let shown: Vec<Entry> = app.shown().into_iter().cloned().collect();
    let title = if app.filter.is_empty() { here.clone() } else { format!("{here} · /{}", app.filter) };
    column(c, &ui, ws.panes[CUR], &title, &shown, Some(app.cursor), &app.selected, &app.cwd, true);
    // preview
    let b = ui.body(c, ws.panes[PREVIEW]);
    match app.current() {
        Some(p) => {
            let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
            match preview(&p, app.hidden) {
                Preview::Dir(v) => column_into(c, &ui, b, &name, &v, None, &[], &p, false, KeyCode::KeyL),
                Preview::Text(lines, total) => {
                    let ext = p.extension().map(|e| e.to_string_lossy().to_uppercase()).unwrap_or_else(|| "TEXT".into());
                    let size = p.metadata().map(|m| m.len()).unwrap_or(0);
                    let h = ui.header(c, &name, &format!("{total} lines"), ui.dim);
                    let meta = ui.row(c, 10.0);
                    let tag = c.spawn((Node { padding: UiRect { left: px(5), right: px(5), top: px(2), bottom: px(1) }, border: UiRect::all(px(1)), margin: UiRect::left(px(18)), ..default() }, BorderColor::all(with_alpha(ui.line, 0.5)))).id();
                    let tt = ui.text(c, ext, &f.ui700, 9.0, 1.4, ui.line);
                    c.entity(tag).add_child(tt);
                    let m = ui.text(c, format!("{total} lines   {}", human(size)), &f.mono, 11.0, 0.0, ui.dim);
                    c.entity(meta).add_children(&[tag, m]);
                    let code = c.spawn(Node { flex_direction: FlexDirection::Column, padding: UiRect { left: px(14), right: px(14), top: px(8), ..default() }, overflow: Overflow::clip(), ..default() }).id();
                    for (i, l) in lines.iter().enumerate() {
                        let t = l.trim_start();
                        let comment = t.starts_with("//") || t.starts_with('#') || t.starts_with("--") || t.starts_with("/*") || t.starts_with('*');
                        let row = c.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(12), height: px(17), ..default() }).id();
                        let num = c.spawn((Text::new(format!("{:>3}", i + 1)), font(&f.mono, 11.0, 0.0), TextColor(ui.dim), Node { width: px(26), flex_shrink: 0.0, ..default() })).id();
                        let tx = c.spawn((Text::new(clip(l, 70)), font(&f.mono, 12.0, 0.0), TextColor(if comment { with_alpha(ui.line, 0.6) } else { ui.hot }), LineHeight::Px(17.0), TextLayout { linebreak: LineBreak::NoWrap, ..default() })).id();
                        c.entity(row).add_children(&[num, tx]);
                        c.entity(code).add_child(row);
                    }
                    c.entity(b).add_children(&[h, meta, code]);
                }
                Preview::Info(s) => {
                    let h = ui.header(c, &name, "", ui.dim);
                    let t = c.spawn((Text::new(s), font(&f.ui500, 13.0, 0.5), TextColor(ui.line), Node { margin: UiRect::all(px(18)), ..default() })).id();
                    c.entity(b).add_children(&[h, t]);
                }
            }
        }
        None => {
            let h = ui.header(c, "Preview", "", ui.dim);
            c.entity(b).add_child(h);
        }
    }
    // status line
    let r = c.spawn(Node { width: percent(100), height: percent(100), flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(18), padding: UiRect::horizontal(px(16)), ..default() }).id();
    c.entity(ws.panes[STATUS]).add_child(r);
    let crumbs = ui.row(c, 6.0);
    let parts: Vec<String> = tilde(&app.current().unwrap_or_else(|| app.cwd.clone())).split('/').map(|s| s.to_string()).collect();
    let last = parts.len().saturating_sub(1);
    for (i, s) in parts.iter().enumerate() {
        let t = ui.text(c, s.clone(), &f.mono, 12.0, 0.0, if i == last { ui.line } else { ui.hot });
        c.entity(crumbs).add_child(t);
        if i < last {
            let sl = ui.text(c, "/", &f.mono, 12.0, 0.0, ui.dim);
            c.entity(crumbs).add_child(sl);
        }
    }
    let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
    c.entity(r).add_children(&[crumbs, sp]);
    let modified = app.current().and_then(|p| p.metadata().ok()).and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| {
        let ago = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|n| n.as_secs()).unwrap_or(0).saturating_sub(d.as_secs());
        match ago {
            a if a < 3600 => format!("{} min ago", a / 60),
            a if a < 86400 => format!("{} h ago", a / 3600),
            a => format!("{} days ago", a / 86400),
        }
    });
    for (k, v) in [("SELECTED", app.selected.len().to_string()), ("YANKED", app.yanked.len().to_string()), ("ITEM", format!("{} / {}", if shown.is_empty() { 0 } else { app.cursor + 1 }, shown.len())), ("MODIFIED", modified.unwrap_or_else(|| "-".into()))] {
        let row = ui.row(c, 6.0);
        let kt = ui.text(c, k, &f.ui600, 9.5, 1.8, ui.dim);
        let vt = ui.text(c, v, &f.mono, 11.5, 0.0, ui.hot);
        c.entity(row).add_children(&[kt, vt]);
        c.entity(r).add_child(row);
    }
}

#[allow(clippy::too_many_arguments)]
fn column(c: &mut Commands, ui: &Ui, root: Entity, title: &str, entries: &[Entry], cursor: Option<usize>, selected: &[PathBuf], dir: &Path, active: bool) {
    let b = ui.body(c, root);
    column_into(c, ui, b, title, entries, cursor, selected, dir, active, KeyCode::KeyH);
}

#[allow(clippy::too_many_arguments)]
fn column_into(c: &mut Commands, ui: &Ui, b: Entity, title: &str, entries: &[Entry], cursor: Option<usize>, selected: &[PathBuf], dir: &Path, active: bool, other: KeyCode) {
    let h = ui.header(c, title, &entries.len().to_string(), ui.dim);
    let list = c.spawn(Node { flex_direction: FlexDirection::Column, padding: UiRect { left: px(8), right: px(10), top: px(8), ..default() }, ..default() }).id();
    c.entity(b).add_children(&[h, list]);
    if entries.is_empty() {
        let t = ui.text(c, "empty", &ui.f.ui500, 12.0, 0.4, ui.dim);
        c.entity(list).add_child(t);
    }
    let cur = cursor.unwrap_or(0);
    let first = cur.saturating_sub(ROWS - 3).min(entries.len().saturating_sub(ROWS));
    for (i, e) in entries.iter().enumerate().skip(first).take(ROWS) {
        let on = cursor == Some(i);
        let picked = selected.contains(&dir.join(&e.name));
        let row = c
            .spawn((
                Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, height: px(24), column_gap: px(10), padding: UiRect::horizontal(px(8)), border: UiRect::all(px(if on && active { 1.0 } else { 0.0 })), ..default() },
                BorderColor::all(ui.hot),
                BackgroundColor(if on { with_alpha(ui.line, if active { 0.12 } else { 0.07 }) } else if picked { with_alpha(role_color(Role::Acc), 0.12) } else { Color::NONE }),
            ))
            .id();
        // rows where you are pick (twice opens); the parent column goes up, the preview goes in
        c.entity(row).insert(if active { Click::Pick(i) } else { Click::Key(other) });
        let ic = c.spawn((Text::new(icon(e)), font(&ui.f.mono, 11.0, 0.0), TextColor(if e.dir { ui.hot } else { ui.dim }), Node { width: px(16), flex_shrink: 0.0, ..default() })).id();
        let nm = c.spawn((Text::new(if e.dir { format!("{}/", e.name) } else { e.name.clone() }), font(&ui.f.mono, 12.5, 0.0), TextColor(if picked { role_color(Role::Acc) } else if on { ui.hot } else { ui.line }), TextLayout { linebreak: LineBreak::NoWrap, ..default() }, Node { flex_grow: 1.0, flex_shrink: 1.0, overflow: Overflow::clip(), ..default() })).id();
        let sz = ui.text(c, if e.dir { String::new() } else { human(e.size) }, &ui.f.mono, 10.5, 0.0, ui.dim);
        c.entity(row).add_children(&[ic, nm, sz]);
        c.entity(list).add_child(row);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_folders_first_and_copies_without_overwriting() {
        let d = std::env::temp_dir().join(format!("sphere-files-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("b-dir")).unwrap();
        std::fs::write(d.join("a.txt"), "x").unwrap();
        std::fs::write(d.join(".hidden"), "x").unwrap();
        let names: Vec<String> = list(&d, false).into_iter().map(|e| e.name).collect();
        assert_eq!(names, vec!["b-dir", "a.txt"]);
        assert_eq!(list(&d, true).len(), 3);
        copy_into(&d.join("a.txt"), &d).unwrap();
        copy_into(&d.join("a.txt"), &d).unwrap();
        assert!(d.join("a copy.txt").exists() && d.join("a copy 2.txt").exists());
        assert_eq!(human(1536), "1.5 KB");
        let _ = std::fs::remove_dir_all(&d);
    }
}
