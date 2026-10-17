//! The Code app (design/apps/code): review an agent's work before it lands. The core shrinks into the top-left
//! corner and a glass workspace unfolds with the task and its checkpoints, the changed files, a delta-style diff
//! by hunk, the repo's tests and the review actions (merge, open a PR, ask for a change, open in Zed, discard),
//! over a strip of the repo's agents. The changes are read with git from the agent's worktree; tests run with the
//! repo's own test command. Keys: J K hunks, [ ] files, Tab next agent, R run tests, M P E Z D actions (M, P and
//! D ask twice), Esc closes.
use crate::core::CoreCtl;
use crate::fonts::UiFonts;
use crate::panel::*;
use crate::review::{self, FileDiff, LineKind, Review, TestRun};
use crate::service::{Cmd, Service};
use crate::appframe::{clip, Click, Ui};
use crate::util::*;
use bevy::prelude::*;
use bevy::text::LineHeight;
use sphere_proto::{TaskState, TaskView};
use std::sync::{mpsc, Mutex};

// the workspace, centred on the stage
const WS_W: f32 = 1280.0;
const WS_H: f32 = 700.0;
const WS_CX: f32 = 803.0;
const WS_CY: f32 = 552.0;
/// the panes sit a little behind the stage plane, so they're drawn this much larger to match the prototype's size
const K: f32 = 1.1;

/// The panes in workspace px: x, y, w, h, and how far each floats forward.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pane {
    Task,
    Tree,
    Diff,
    Tests,
    Acts,
    Strip,
    Top,
}

impl Pane {
    const ALL: [Pane; 7] = [Pane::Task, Pane::Tree, Pane::Diff, Pane::Tests, Pane::Acts, Pane::Strip, Pane::Top];
    fn rect(self) -> [f32; 5] {
        match self {
            Pane::Task => [14.0, 42.0, 262.0, 584.0, 26.0],
            Pane::Tree => [288.0, 42.0, 690.0, 200.0, 40.0],
            Pane::Diff => [288.0, 254.0, 690.0, 372.0, 18.0],
            Pane::Tests => [990.0, 42.0, 276.0, 296.0, 34.0],
            Pane::Acts => [990.0, 350.0, 276.0, 276.0, 46.0],
            Pane::Strip => [14.0, 638.0, 1252.0, 50.0, 22.0],
            Pane::Top => [22.0, 2.0, 1236.0, 32.0, 8.0],
        }
    }
}

/// Diff rows that fit the diff pane.
const DIFF_ROWS: usize = 15;
/// Files that fit the tree.
const TREE_ROWS: usize = 6;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Act {
    Merge,
    Pr,
    Discard,
}

#[derive(Resource)]
pub struct CodeApp {
    pub open: bool,
    pub amt: Spring,
    /// the task under review; `None` picks one
    task: Option<String>,
    review: Option<Review>,
    loading: bool,
    load_rx: Option<Mutex<mpsc::Receiver<(String, Review)>>>,
    /// (task, run version) the review was read for, and when
    loaded: (String, u64),
    load_t: f32,
    file: usize,
    hunk: usize,
    tests: Option<TestRun>,
    tests_rx: Option<Mutex<mpsc::Receiver<TestRun>>>,
    /// an action that asks twice: armed until this sim time
    armed: Option<(Act, f32)>,
    notes: (mpsc::Sender<String>, Mutex<mpsc::Receiver<String>>),
    /// bumps when anything on screen changes
    version: u64,
}

impl Default for CodeApp {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        CodeApp {
            open: false,
            amt: Spring::new(0.0, 26.0, 9.0),
            task: None,
            review: None,
            loading: false,
            load_rx: None,
            loaded: (String::new(), u64::MAX),
            load_t: -9.0,
            file: 0,
            hunk: 0,
            tests: None,
            tests_rx: None,
            armed: None,
            notes: (tx, Mutex::new(rx)),
            version: 0,
        }
    }
}

impl CodeApp {
    pub fn open(&mut self) {
        self.open = true;
        self.task = None;
        self.version += 1;
    }
    pub fn close(&mut self) {
        self.open = false;
        self.armed = None;
        self.version += 1;
    }
    fn file(&self) -> Option<&FileDiff> {
        self.review.as_ref().and_then(|r| r.files.get(self.file))
    }
    /// Moves to the next (+1) or previous (-1) hunk, across files.
    fn step_hunk(&mut self, d: i32) {
        let Some(r) = &self.review else { return };
        if r.files.is_empty() {
            return;
        }
        let n = r.files[self.file].hunks.len().max(1);
        if d > 0 {
            if self.hunk + 1 < n {
                self.hunk += 1;
            } else if self.file + 1 < r.files.len() {
                self.file += 1;
                self.hunk = 0;
            }
        } else if self.hunk > 0 {
            self.hunk -= 1;
        } else if self.file > 0 {
            self.file -= 1;
            self.hunk = r.files[self.file].hunks.len().saturating_sub(1);
        }
        self.version += 1;
    }
    fn step_file(&mut self, d: i32) {
        let n = self.review.as_ref().map(|r| r.files.len()).unwrap_or(0);
        if n == 0 {
            return;
        }
        self.file = (self.file as i32 + d).clamp(0, n as i32 - 1) as usize;
        self.hunk = 0;
        self.version += 1;
    }
}

/// The task to review: the one chosen, else the latest with a worktree (finished work first).
fn pick<'a>(app: &CodeApp, tasks: &'a [TaskView]) -> Option<&'a TaskView> {
    if let Some(id) = &app.task {
        if let Some(t) = tasks.iter().find(|t| &t.id == id) {
            return Some(t);
        }
    }
    let ready = |t: &&TaskView| t.workdir.is_some() && matches!(t.state, TaskState::Done | TaskState::NeedsYou);
    tasks.iter().filter(ready).max_by(|a, b| a.updated_at.cmp(&b.updated_at)).or_else(|| tasks.iter().filter(|t| t.workdir.is_some()).max_by(|a, b| a.updated_at.cmp(&b.updated_at)))
}

/// The tasks in the same repo, latest first (the agent strip, and Tab).
fn repo_tasks<'a>(tasks: &'a [TaskView], repo: &str) -> Vec<&'a TaskView> {
    let mut v: Vec<&TaskView> = tasks.iter().filter(|t| t.repo == repo && t.workdir.is_some()).collect();
    v.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    v
}

#[derive(Component)]
pub struct CodePane(#[allow(dead_code)] pub Pane);
#[derive(Component)]
pub struct CodeFrame;
#[derive(Component)]
pub struct CodeCoreLabel;

#[derive(Resource)]
pub struct CodeRoots {
    panes: Vec<(Pane, Entity)>,
    corelab: Entity,
}

pub fn setup(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut images: ResMut<Assets<Image>>, (home, mut atlases): (Res<PanelAtlas>, ResMut<AppAtlases>), mut pmats: ResMut<Assets<PanelMaterial>>, args: Res<crate::Args>, mut app: ResMut<CodeApp>) {
    if args.code {
        app.open();
    }
    // the app's own atlas, sharper than the home's (see `AppAtlases`)
    let mut atlas = Atlas::new(&mut commands, &mut images, home.scale * APP_SHARP, -11);
    let c = &mut commands;
    let at = |x: f32, y: f32, w: f32, h: f32, z: f32| {
        let mut p = stage_to_world(WS_CX + K * (x + w / 2.0 - WS_W / 2.0), WS_CY + K * (y + h / 2.0 - WS_H / 2.0));
        p.z = z;
        p
    };
    // the workspace glass: a frame with no content of its own
    let ui = spawn_ui_panel(c, &mut atlas, 4.0, 4.0);
    let e = spawn_panel_quad(c, &mut meshes, &mut pmats, &ui, &PanelSpec { id: PanelId::AppFrame, w: WS_W, h: WS_H, home: at(0.0, 0.0, WS_W, WS_H, -10.0), home_scale: K, side: 0.0, phase: 0.3, show_at: 0.0, unframed: false });
    c.entity(e).insert(CodeFrame);
    let mut panes = Vec::new();
    for (i, pane) in Pane::ALL.iter().enumerate() {
        let [x, y, w, h, z] = pane.rect();
        let ui = spawn_ui_panel(c, &mut atlas, w, h);
        let id = if *pane == Pane::Top { PanelId::AppBare } else { PanelId::App };
        let e = spawn_panel_quad(c, &mut meshes, &mut pmats, &ui, &PanelSpec { id, w, h, home: at(x, y, w, h, z), home_scale: K, side: 0.0, phase: 0.7 + i as f32 * 1.3, show_at: 0.0, unframed: false });
        c.entity(e).insert((CodePane(*pane), crate::appframe::AppPart { app: 0, pane: Some(i as u8) }, crate::appframe::PaneRoot(ui.root)));
        panes.push((*pane, ui.root));
    }
    // "CODE · 1 agent needs you" beside the small core
    let (lw, lh) = (300.0, 40.0);
    let ui = spawn_ui_panel(c, &mut atlas, lw, lh);
    let mut home = stage_to_world(132.0 + lw / 2.0, 52.0 + lh / 2.0);
    home.z = 20.0;
    let e = spawn_panel_quad(c, &mut meshes, &mut pmats, &ui, &PanelSpec { id: PanelId::AppBare, w: lw, h: lh, home, home_scale: 1.0, side: 0.0, phase: 2.2, show_at: 0.0, unframed: false });
    c.entity(e).insert(CodeCoreLabel);
    c.insert_resource(CodeRoots { panes, corelab: ui.root });
    atlases.add(0, atlas);
}

#[allow(clippy::too_many_arguments)]
pub fn keys(
    mut ev: MessageReader<bevy::input::keyboard::KeyboardInput>,
    keys: Res<ButtonInput<KeyCode>>,
    (focus, moment, mut bar): (ResMut<Focus>, Res<crate::moment::Moment>, ResMut<crate::cmdbar::CmdBar>),
    mut app: ResMut<CodeApp>,
    svc: Res<Service>,
    sim: Res<crate::core::Sim>,
    mut sound: ResMut<crate::sound::Sound>,
    mut toasts: ResMut<crate::live::Toasts>,
    mut clicks: ResMut<crate::appframe::AppClicks>,
    mut cmd_was: Local<bool>,
) {
    let mut focus = focus;
    let mods = keys.any_pressed([KeyCode::SuperLeft, KeyCode::SuperRight, KeyCode::ControlLeft, KeyCode::ControlRight, KeyCode::AltLeft, KeyCode::AltRight]);
    // the bar closing this frame (Esc, Enter) doesn't also act here
    let bar_busy = focus.cmd || *cmd_was;
    *cmd_was = focus.cmd;
    if !app.open {
        ev.clear();
        return;
    }
    let mut codes: Vec<KeyCode> = Vec::new();
    for e in ev.read() {
        if e.state != bevy::input::ButtonState::Pressed || mods || bar_busy || focus.terminal || moment.shown || e.repeat && !matches!(e.key_code, KeyCode::KeyJ | KeyCode::KeyK) {
            continue;
        }
        codes.push(e.key_code);
    }
    // clicks on the panes: a button presses its key, a row picks its item
    for c in clicks.take(0) {
        match c {
            Click::Key(k) => codes.push(k),
            Click::Pick(i) => {
                if app.review.as_ref().is_some_and(|r| i < r.files.len()) {
                    app.file = i;
                    app.hunk = 0;
                    app.version += 1;
                }
            }
            Click::Alt(i) => {
                app.hunk = i;
                app.version += 1;
            }
        }
    }
    for key in codes {
        let task = { pick(&app, &svc.shared.lock().unwrap().tasks).cloned() };
        match key {
            KeyCode::Escape => {
                app.close();
                sound.play_with("collapse", 0.7, 0.0);
            }
            KeyCode::KeyJ => {
                app.step_hunk(1);
                sound.play_q("tick", 40);
            }
            KeyCode::KeyK => {
                app.step_hunk(-1);
                sound.play_q("tick", 40);
            }
            KeyCode::BracketRight | KeyCode::ArrowDown => {
                app.step_file(1);
                sound.play_q("tick", 40);
            }
            KeyCode::BracketLeft | KeyCode::ArrowUp => {
                app.step_file(-1);
                sound.play_q("tick", 40);
            }
            KeyCode::Tab => {
                let next = {
                    let s = svc.shared.lock().unwrap();
                    task.as_ref().and_then(|t| {
                        let v = repo_tasks(&s.tasks, &t.repo);
                        let i = v.iter().position(|x| x.id == t.id).unwrap_or(0);
                        v.get((i + 1) % v.len().max(1)).map(|x| x.id.clone())
                    })
                };
                if next.is_some() && next != task.as_ref().map(|t| t.id.clone()) {
                    app.task = next;
                    app.review = None;
                    app.tests = None;
                    app.file = 0;
                    app.hunk = 0;
                    app.version += 1;
                    sound.play("toggle");
                }
            }
            KeyCode::KeyR => match task.as_ref().and_then(|t| t.workdir.clone()) {
                Some(wd) => {
                    if app.tests.as_ref().is_some_and(|t| !t.done) {
                        toasts.push_quiet("Tests are already running");
                    } else {
                        run_tests(&mut app, wd, &mut toasts);
                        sound.play("progress");
                    }
                }
                None => toasts.push("No worktree to test"),
            },
            KeyCode::KeyE => {
                if let Some(t) = &task {
                    bar.ask(t.id.clone(), t.title.clone(), &mut focus);
                    sound.play("open");
                }
            }
            KeyCode::KeyZ => {
                let target = app.file().map(|f| (f.path.clone(), f.hunks.get(app.hunk).map(|h| h.new_start).unwrap_or(1)));
                match (task.as_ref().and_then(|t| t.workdir.clone()), target) {
                    (Some(wd), Some((path, line))) => match open_in_zed(&wd, &path, line) {
                        Ok(()) => {
                            toasts.push_quiet(format!("Opening {path}:{line} in Zed"));
                            sound.play("open");
                        }
                        Err(e) => toasts.push(e),
                    },
                    _ => toasts.push("No file to open"),
                }
            }
            KeyCode::KeyU => toasts.push("Undo to a checkpoint comes with checkpoints in sphered"),
            k @ (KeyCode::KeyM | KeyCode::KeyP | KeyCode::KeyD) => {
                let act = match k {
                    KeyCode::KeyM => Act::Merge,
                    KeyCode::KeyP => Act::Pr,
                    _ => Act::Discard,
                };
                let Some(t) = task.clone() else {
                    toasts.push("No agent to review");
                    continue;
                };
                if app.armed.is_some_and(|(a, until)| a == act && sim.t < until) {
                    app.armed = None;
                    app.version += 1;
                    act_now(act, &t, &app, &svc);
                    sound.play(if act == Act::Discard { "discard" } else { "confirm" });
                } else {
                    app.armed = Some((act, sim.t + 3.0));
                    app.version += 1;
                    sound.play("toggle");
                }
            }
            _ => {}
        }
    }
}

/// Runs the review action in the background; its outcome comes back as a toast.
fn act_now(act: Act, t: &TaskView, app: &CodeApp, svc: &Service) {
    let tx = app.notes.0.clone();
    let (repo, wd, branch, base, title, in_place) = (t.repo.clone(), t.workdir.clone().unwrap_or_default(), t.branch.clone(), t.base.clone().unwrap_or_else(|| "main".into()), t.title.clone(), t.in_place);
    if act == Act::Discard && matches!(t.state, TaskState::Working | TaskState::NeedsYou | TaskState::Queued) {
        svc.send(Cmd::Stop { task: t.id.clone() });
    }
    std::thread::spawn(move || {
        let note = match act {
            Act::Merge => merge(&repo, &wd, branch.as_deref(), &base, &title, in_place),
            Act::Pr => open_pr(&wd, branch.as_deref(), &title),
            Act::Discard => discard(&repo, &wd, in_place),
        };
        let _ = tx.send(note.unwrap_or_else(|e| e));
    });
}

fn git(dir: &str, args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().map_err(|e| format!("git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).lines().next().unwrap_or("git failed").to_string())
    }
}

/// Commits what's left in the worktree, then merges the branch into the base in the repo.
fn merge(repo: &str, wd: &str, branch: Option<&str>, base: &str, title: &str, in_place: bool) -> Result<String, String> {
    let branch = branch.filter(|_| !in_place).ok_or("This agent worked in place: its changes are already in the repo")?;
    if !git(wd, &["status", "--porcelain"])?.is_empty() {
        git(wd, &["add", "-A"])?;
        git(wd, &["commit", "-q", "-m", title])?;
    }
    let cur = git(repo, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    if cur != base {
        return Err(format!("The repo is on {cur}, not {base}: switch to {base} to merge"));
    }
    if !git(repo, &["status", "--porcelain", "--untracked-files=no"])?.is_empty() {
        return Err(format!("The repo has uncommitted changes on {base}: commit or stash them to merge"));
    }
    git(repo, &["merge", "--no-ff", "--no-edit", branch]).map_err(|e| format!("Merge failed: {e}"))?;
    Ok(format!("Merged {branch} into {base}"))
}

/// Pushes the branch and opens a pull request with the GitHub CLI.
fn open_pr(wd: &str, branch: Option<&str>, title: &str) -> Result<String, String> {
    let branch = branch.ok_or("This agent has no branch to open a PR from")?;
    if !git(wd, &["status", "--porcelain"])?.is_empty() {
        git(wd, &["add", "-A"])?;
        git(wd, &["commit", "-q", "-m", title])?;
    }
    git(wd, &["push", "-u", "origin", branch]).map_err(|e| format!("Push failed: {e}"))?;
    let out = std::process::Command::new("gh").current_dir(wd).args(["pr", "create", "--fill", "--head", branch]).output().map_err(|_| "Pushed. Install the GitHub CLI (gh) to open the PR from here".to_string())?;
    if out.status.success() {
        let url = String::from_utf8_lossy(&out.stdout).lines().last().unwrap_or("").to_string();
        Ok(format!("Opened a PR · {url}"))
    } else {
        Err(format!("gh: {}", String::from_utf8_lossy(&out.stderr).lines().next().unwrap_or("could not open the PR")))
    }
}

/// Removes the agent's worktree (its branch stays, so the work can still be found).
fn discard(repo: &str, wd: &str, in_place: bool) -> Result<String, String> {
    if in_place {
        return Err("This agent worked in place: discard its changes in the repo yourself".into());
    }
    git(repo, &["worktree", "remove", "--force", wd]).map_err(|e| format!("Could not remove the worktree: {e}"))?;
    Ok("Discarded · the worktree is gone, its branch stays".into())
}

fn open_in_zed(wd: &str, path: &str, line: u32) -> Result<(), String> {
    let target = format!("{}:{line}", std::path::Path::new(wd).join(path).display());
    let cli = ["/Applications/Zed.app/Contents/MacOS/cli", "/usr/local/bin/zed", "/opt/homebrew/bin/zed"].into_iter().find(|p| std::path::Path::new(p).exists());
    let cmd = cli.map(str::to_string).or_else(|| std::process::Command::new("zed").arg("--version").output().ok().map(|_| "zed".to_string()));
    match cmd {
        Some(c) => std::process::Command::new(c).arg(target).spawn().map(|_| ()).map_err(|e| format!("Zed: {e}")),
        None => Err("Zed isn't installed (zed.dev)".into()),
    }
}

fn run_tests(app: &mut CodeApp, wd: String, toasts: &mut crate::live::Toasts) {
    let Some(cmd) = review::test_command(&wd) else {
        toasts.push("No test command for this repo yet (Cargo projects run cargo test)");
        return;
    };
    let (tx, rx) = mpsc::channel();
    app.tests_rx = Some(Mutex::new(rx));
    app.tests = Some(TestRun { command: cmd.join(" "), ..Default::default() });
    app.version += 1;
    std::thread::spawn(move || {
        use std::io::BufRead;
        let mut run = TestRun { command: cmd.join(" "), ..Default::default() };
        let child = std::process::Command::new(&cmd[0]).args(&cmd[1..]).current_dir(&wd).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn();
        let mut child = match child {
            Ok(c) => c,
            Err(e) => {
                run.error = Some(e.to_string());
                run.done = true;
                let _ = tx.send(run);
                return;
            }
        };
        // build errors arrive on stderr, results on stdout
        let err = child.stderr.take().map(|s| std::thread::spawn(move || std::io::BufReader::new(s).lines().map_while(Result::ok).find(|l| l.starts_with("error")).unwrap_or_default()));
        let mut last = std::time::Instant::now();
        if let Some(out) = child.stdout.take() {
            for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
                review::test_line(&mut run, &line);
                if last.elapsed().as_millis() > 250 {
                    last = std::time::Instant::now();
                    let _ = tx.send(run.clone());
                }
            }
        }
        let ok = child.wait().map(|s| s.success()).unwrap_or(false);
        if let Some(Ok(e)) = err.map(|h| h.join()) {
            if !ok && run.tests.is_empty() && !e.is_empty() {
                run.error = Some(e);
            }
        }
        run.done = true;
        let _ = tx.send(run);
    });
}

/// Springs, the core's corner, reading the review in the background, test progress and action results.
#[allow(clippy::too_many_arguments)]
pub fn update(sim: Res<crate::core::Sim>, mut atlases: ResMut<AppAtlases>, mut app: ResMut<CodeApp>, mut ctl: ResMut<CoreCtl>, svc: Res<Service>, mut toasts: ResMut<crate::live::Toasts>, mut sound: ResMut<crate::sound::Sound>) {
    app.amt.t = if app.open { 1.0 } else { 0.0 };
    app.amt.step(sim.dt);
    ctl.app = clamp(app.amt.x, 0.0, 1.0);
    atlases.want(0, app.open || app.amt.x > 0.001);
    if let Some((_, until)) = app.armed {
        if sim.t > until {
            app.armed = None;
            app.version += 1;
        }
    }
    let notes: Vec<String> = app.notes.1.lock().map(|r| r.try_iter().collect()).unwrap_or_default();
    for n in notes {
        toasts.push(n);
        app.loaded.1 = u64::MAX; // read the review again
        app.version += 1;
    }
    let got = app.tests_rx.as_ref().and_then(|r| r.lock().ok()?.try_iter().last());
    if let Some(t) = got {
        if t.done {
            sound.play(if t.failed > 0 || t.error.is_some() { "error" } else { "done" });
        }
        app.tests = Some(t);
        app.version += 1;
    }
    let got = app.load_rx.as_ref().and_then(|r| r.lock().ok()?.try_recv().ok());
    if let Some((task, r)) = got {
        app.loading = false;
        // look tests: SPHERE_CODE_KEYS plays keys once the first review is in (j k ] [ move, r runs the tests)
        if app.review.is_none() {
            if let Ok(keys) = std::env::var("SPHERE_CODE_KEYS") {
                let wd = r.workdir.clone();
                app.review = Some(r.clone());
                for k in keys.chars() {
                    match k {
                        'j' => app.step_hunk(1),
                        'k' => app.step_hunk(-1),
                        ']' => app.step_file(1),
                        '[' => app.step_file(-1),
                        'r' => run_tests(&mut app, wd.clone(), &mut toasts),
                        'm' => app.armed = Some((Act::Merge, sim.t + 1e6)),
                        _ => {}
                    }
                }
            }
        }
        if Some(&task) == app.task.as_ref() || app.task.is_none() {
            let n = r.files.len();
            app.review = Some(r);
            if app.file >= n {
                app.file = 0;
                app.hunk = 0;
            }
            app.version += 1;
        }
    }
    if !app.open || app.loading {
        return;
    }
    // read the review when the task or its files change (at most every 1.5 s)
    let (task, ver) = {
        let s = svc.shared.lock().unwrap();
        let t = pick(&app, &s.tasks).cloned();
        let v = t.as_ref().and_then(|t| s.runs.get(&t.id)).map(|r| r.version).unwrap_or(0);
        (t, v)
    };
    let Some(t) = task else { return };
    let (Some(wd), base) = (t.workdir.clone(), t.base.clone().unwrap_or_else(|| "main".into())) else { return };
    let stale = app.loaded.0 != t.id || (app.loaded.1 != ver && sim.t - app.load_t > 1.5);
    if !stale {
        return;
    }
    if app.loaded.0 != t.id {
        app.review = None;
        app.file = 0;
        app.hunk = 0;
        app.tests = None;
    }
    app.loaded = (t.id.clone(), ver);
    app.load_t = sim.t;
    app.loading = true;
    app.task.get_or_insert(t.id.clone());
    let (tx, rx) = mpsc::channel();
    app.load_rx = Some(Mutex::new(rx));
    let id = t.id.clone();
    std::thread::spawn(move || {
        let _ = tx.send((id, review::load(&wd, &base)));
    });
}

/// The panes follow the app: in from behind as it opens, back as it closes.
#[allow(clippy::type_complexity)]
pub fn place(app: Res<CodeApp>, mut q: Query<&mut Panel, Or<(With<CodePane>, With<CodeFrame>, With<CodeCoreLabel>)>>) {
    let amt = clamp(app.amt.x, 0.0, 1.0);
    for mut p in &mut q {
        if p.work.map(|w| w.0) != Some(amt) {
            p.work = Some((amt, 0.0));
        }
    }
}

// ------------------------------------------------------------------ drawing

fn ago(t: &TaskView) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    match crate::working::parse_rfc3339(&t.started_at) {
        Some(s) => {
            let m = (now - s).max(0) / 60;
            if m < 1 {
                "just now".into()
            } else if m < 60 {
                format!("{m} min ago")
            } else {
                format!("{} h {} min ago", m / 60, m % 60)
            }
        }
        None => "-".into(),
    }
}

fn state_words(t: &TaskView) -> (&'static str, Role) {
    match t.state {
        TaskState::Done => ("READY FOR REVIEW", Role::Hot),
        TaskState::NeedsYou => ("NEEDS YOU", Role::Acc),
        TaskState::Working | TaskState::Queued => ("WORKING", Role::Line),
        TaskState::Failed => ("FAILED", Role::Bad),
        TaskState::Stopped => ("STOPPED", Role::Dim),
    }
}

/// Rebuilds the panes when the review, the selection, the tests or the agents change.
#[allow(clippy::too_many_arguments)]
pub fn draw(mut commands: Commands, app: Res<CodeApp>, svc: Res<Service>, roots: Option<Res<CodeRoots>>, f: Res<UiFonts>, theme: Res<crate::theme::Theme>, time: Res<Time<Real>>, mut seen: Local<(u64, u64, u64, i64)>) {
    let Some(roots) = roots else { return };
    if app.amt.x < 0.001 && !app.open {
        return;
    }
    let (task, run, agents, sver) = {
        let s = svc.shared.lock().unwrap();
        let t = pick(&app, &s.tasks).cloned();
        let run = t.as_ref().and_then(|t| s.runs.get(&t.id).cloned()).unwrap_or_default();
        let agents: Vec<TaskView> = t.as_ref().map(|t| repo_tasks(&s.tasks, &t.repo).into_iter().take(3).cloned().collect()).unwrap_or_default();
        (t, run, agents, s.version)
    };
    let key = (app.version, sver, theme.version, time.elapsed_secs() as i64 / 30);
    if *seen == key {
        return;
    }
    *seen = key;
    let c = &mut commands;
    for (_, r) in &roots.panes {
        c.entity(*r).despawn_children();
    }
    c.entity(roots.corelab).despawn_children();
    let ui = Ui { f: &f, line: role_color(Role::Line), hot: role_color(Role::Hot), dim: role_color(Role::Dim) };
    let root = |p: Pane| roots.panes.iter().find(|x| x.0 == p).map(|x| x.1).unwrap();
    // the label by the small core
    let needs = agents.iter().filter(|t| t.state == TaskState::NeedsYou).count();
    let lab = c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(5), padding: UiRect::top(px(4)), ..default() }).id();
    let l1 = ui.text(c, "CODE", &f.ui600, 11.0, 3.1, ui.hot);
    let (l2s, l2c) = if needs > 0 { (format!("{needs} agent{} needs you", if needs == 1 { "" } else { "s" }), role_color(Role::Acc)) } else { (format!("{} agent{} on this repo", agents.len(), if agents.len() == 1 { "" } else { "s" }), ui.line) };
    let l2 = ui.text(c, l2s, &f.ui500, 11.5, 1.2, l2c);
    c.entity(lab).add_children(&[l1, l2]);
    c.entity(roots.corelab).add_child(lab);
    draw_top(c, &ui, root(Pane::Top), task.as_ref());
    draw_strip(c, &ui, root(Pane::Strip), &agents, task.as_ref());
    let Some(t) = task else {
        let b = ui.body(c, root(Pane::Diff));
        let h = ui.header(c, "Diff", "", ui.dim);
        let m = c.spawn((Text::new("No agent has worked on a repo yet. Start one with /  (repo: what to do)."), font(&f.ui500, 13.0, 0.5), TextColor(ui.line), Node { margin: UiRect::all(px(18)), ..default() })).id();
        c.entity(b).add_children(&[h, m]);
        for p in [Pane::Task, Pane::Tree, Pane::Tests, Pane::Acts] {
            let b = ui.body(c, root(p));
            let h = ui.header(c, match p { Pane::Task => "Task", Pane::Tree => "Changed files", Pane::Tests => "Tests", _ => "Review" }, "", ui.dim);
            c.entity(b).add_child(h);
        }
        return;
    };
    draw_task(c, &ui, root(Pane::Task), &t, &run, app.review.as_ref());
    draw_tree(c, &ui, root(Pane::Tree), &app);
    draw_diff(c, &ui, root(Pane::Diff), &app, &run);
    draw_tests(c, &ui, root(Pane::Tests), &t, app.tests.as_ref());
    draw_acts(c, &ui, root(Pane::Acts), &app);
}

fn draw_top(c: &mut Commands, ui: &Ui, root: Entity, t: Option<&TaskView>) {
    let r = c.spawn(Node { width: percent(100), height: percent(100), flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(12), ..default() }).id();
    c.entity(root).add_child(r);
    let ic = ui.text(c, "</>", &ui.f.mono, 12.0, 0.0, ui.hot);
    let name = ui.text(c, "CODE", &ui.f.ui600, 11.0, 3.5, ui.hot);
    let repo = t.map(|t| std::path::Path::new(&t.repo).file_name().map(|s| s.to_string_lossy().to_uppercase()).unwrap_or_default()).unwrap_or_default();
    let sub = ui.text(c, format!("{repo} · REVIEW"), &ui.f.ui500, 11.0, 2.6, ui.dim);
    let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
    let credit = ui.text(c, "diff after delta (MIT) · git · editor: Zed (GPL-3.0)", &ui.f.ui500, 10.5, 1.0, ui.dim);
    let esc = c.spawn((Node { padding: UiRect { left: px(6), right: px(6), top: px(3), bottom: px(2) }, border: UiRect::all(px(1)), ..default() }, BorderColor::all(with_alpha(ui.line, 0.55)), Click::Key(KeyCode::Escape))).id();
    let et = ui.text(c, "ESC", &ui.f.ui700, 10.0, 1.6, ui.hot);
    c.entity(esc).add_child(et);
    c.entity(r).add_children(&[ic, name, sub, sp, credit, esc]);
}

fn draw_task(c: &mut Commands, ui: &Ui, root: Entity, t: &TaskView, run: &crate::service::Run, r: Option<&Review>) {
    let b = ui.body(c, root);
    let id: String = t.id.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
    let h = ui.header(c, "Task", &format!("T-{}", id.to_uppercase()), ui.dim);
    let inner = c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(5), padding: UiRect { left: px(18), right: px(16), top: px(14), bottom: px(12) }, flex_grow: 1.0, ..default() }).id();
    c.entity(b).add_children(&[h, inner]);
    let title = c.spawn((Text::new(clip(&t.title, 90)), font(&ui.f.ui600, 17.0, 0.5), TextColor(ui.hot), LineHeight::Px(21.0), Node { margin: UiRect::bottom(px(10)), ..default() })).id();
    c.entity(inner).add_child(title);
    // the base can be a commit: show it short
    let base: String = t.base.clone().unwrap_or_else(|| "main".into()).chars().take(9).collect();
    let branch = if t.in_place { "in place".to_string() } else { format!("{} from {}", clip(t.branch.as_deref().unwrap_or("-").trim_start_matches("sphere/"), 12), base) };
    let agent = match &t.model {
        Some(m) => format!("Claude Code · {}", clip(m, 12)),
        None => "Claude Code".into(),
    };
    for (k, v) in [("AGENT", agent), ("BRANCH", branch), ("STARTED", ago(t))] {
        let row = c.spawn(Node { flex_direction: FlexDirection::Row, justify_content: JustifyContent::SpaceBetween, align_items: AlignItems::Center, height: px(19), ..default() }).id();
        let kt = ui.text(c, k, &ui.f.ui600, 10.5, 2.4, ui.dim);
        let vt = ui.text(c, v, &ui.f.ui500, 13.0, 0.4, ui.line);
        c.entity(row).add_children(&[kt, vt]);
        c.entity(inner).add_child(row);
    }
    let (sw, sr) = state_words(t);
    let row = c.spawn(Node { flex_direction: FlexDirection::Row, justify_content: JustifyContent::SpaceBetween, align_items: AlignItems::Center, height: px(22), ..default() }).id();
    let kt = ui.text(c, "STATUS", &ui.f.ui600, 10.5, 2.4, ui.dim);
    let chip = c.spawn((Node { padding: UiRect { left: px(6), right: px(6), top: px(3), bottom: px(2) }, border: UiRect::all(px(1)), ..default() }, BorderColor::all(role_color(sr)))).id();
    let ct = ui.text(c, sw, &ui.f.ui700, 10.5, 1.8, role_color(sr));
    c.entity(chip).add_child(ct);
    c.entity(row).add_children(&[kt, chip]);
    c.entity(inner).add_child(row);
    // checkpoints: the branch's commits, then what's still uncommitted (the agent's edits)
    let ch = c.spawn((Node { flex_direction: FlexDirection::Row, justify_content: JustifyContent::SpaceBetween, margin: UiRect { top: px(14), bottom: px(6), ..default() }, ..default() },)).id();
    let ct = ui.text(c, "CHECKPOINTS", &ui.f.ui600, 10.5, 3.0, ui.hot);
    let cr = ui.text(c, "U undo", &ui.f.ui500, 10.5, 1.0, ui.dim);
    c.entity(ch).add_children(&[ct, cr]);
    c.entity(inner).add_child(ch);
    let mut items: Vec<(String, String, bool)> = Vec::new();
    if let Some(r) = r {
        for (h, s) in r.commits.iter().rev() {
            items.push((s.clone(), h.clone(), true));
        }
    }
    let edits: Vec<&crate::service::RunStep> = run.steps.iter().filter(|s| s.kind == sphere_proto::StepKind::Edit).collect();
    if r.is_some_and(|r| r.dirty) || items.is_empty() {
        for s in edits.iter().rev().take(6).collect::<Vec<_>>().into_iter().rev() {
            let at = s.at.get(11..16).unwrap_or("").to_string();
            items.push((s.title.clone(), at, false));
        }
    }
    let n = items.len();
    let list = c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(10), ..default() }).id();
    for (i, (title, meta, commit)) in items.iter().skip(n.saturating_sub(7)).enumerate() {
        let last = i + 1 == n.min(7);
        let row = c.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(12), ..default() }).id();
        let d = c
            .spawn((
                Node { width: px(8), height: px(8), margin: UiRect::top(px(4)), border: UiRect::all(px(1)), flex_shrink: 0.0, ..default() },
                BackgroundColor(if last { ui.hot } else { Color::NONE }),
                BorderColor::all(if *commit { ui.hot } else { ui.line }),
                UiTransform { rotation: Rot2::degrees(45.0), ..default() },
            ))
            .id();
        let col = c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(2), flex_shrink: 1.0, ..default() }).id();
        let tt = c.spawn((Text::new(clip(title, 52)), font(&ui.f.ui600, 13.0, 0.3), TextColor(if last { ui.hot } else { ui.line }), LineHeight::Px(16.0))).id();
        let mt = ui.text(c, meta.clone(), &ui.f.mono, 10.5, 0.0, ui.dim);
        c.entity(col).add_children(&[tt, mt]);
        c.entity(row).add_children(&[d, col]);
        c.entity(list).add_child(row);
    }
    if n == 0 {
        let e = ui.text(c, "No edits yet", &ui.f.ui500, 13.0, 0.4, ui.dim);
        c.entity(list).add_child(e);
    }
    c.entity(inner).add_child(list);
}

fn bar_blocks(c: &mut Commands, ui: &Ui, add: usize, del: usize) -> Entity {
    let row = ui.row(c, 2.0);
    let total = (add + del).max(1);
    let green = ((add as f32 / total as f32) * 5.0).round() as usize;
    for i in 0..5 {
        let col = if add + del == 0 { with_alpha(ui.line, 0.2) } else if i < green { role_color(Role::Ok) } else { role_color(Role::Bad) };
        let b = c.spawn((Node { width: px(8), height: px(8), ..default() }, BackgroundColor(col))).id();
        c.entity(row).add_child(b);
    }
    row
}

fn draw_tree(c: &mut Commands, ui: &Ui, root: Entity, app: &CodeApp) {
    let b = ui.body(c, root);
    let r = app.review.as_ref();
    let sum = match r {
        Some(r) => format!("{} files · +{} −{}", r.files.len(), r.added(), r.removed()),
        None if app.loading => "reading…".into(),
        None => String::new(),
    };
    let h = ui.header(c, "Changed files", &sum, ui.dim);
    let list = c.spawn(Node { flex_direction: FlexDirection::Column, padding: UiRect { left: px(10), right: px(14), top: px(8), ..default() }, ..default() }).id();
    c.entity(b).add_children(&[h, list]);
    let Some(r) = r else { return };
    if let Some(e) = &r.error {
        let t = ui.text(c, clip(e, 80), &ui.f.ui500, 13.0, 0.4, role_color(Role::Bad));
        c.entity(list).add_child(t);
        return;
    }
    if r.files.is_empty() {
        let t = ui.text(c, "No changes yet against the base branch", &ui.f.ui500, 13.0, 0.4, ui.dim);
        c.entity(list).add_child(t);
        return;
    }
    let first = app.file.saturating_sub(TREE_ROWS - 1).min(r.files.len().saturating_sub(TREE_ROWS));
    for (i, fd) in r.files.iter().enumerate().skip(first).take(TREE_ROWS) {
        let sel = i == app.file;
        let row = c
            .spawn((
                Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, height: px(22), padding: UiRect { left: px(10), right: px(4), ..default() }, border: UiRect::left(px(2)), column_gap: px(8), ..default() },
                BackgroundColor(if sel { with_alpha(ui.line, 0.10) } else { Color::NONE }),
                BorderColor::all(if sel { ui.hot } else { Color::NONE }),
                Click::Pick(i),
            ))
            .id();
        let (dir, name) = match fd.path.rsplit_once('/') {
            Some((d, n)) => (format!("{d}/"), n.to_string()),
            None => (String::new(), fd.path.clone()),
        };
        let p = c.spawn(Node { flex_direction: FlexDirection::Row, flex_grow: 1.0, overflow: Overflow::clip(), ..default() }).id();
        let dt = ui.text(c, clip(&dir, 40), &ui.f.mono, 12.0, 0.0, ui.dim);
        let nt = ui.text(c, name, &ui.f.mono, 12.0, 0.0, if sel { ui.hot } else { ui.line });
        c.entity(p).add_children(&[dt, nt]);
        let st = c.spawn((Text::new(fd.status.letter()), font(&ui.f.mono, 12.0, 0.0), TextColor(if fd.status == review::Status::Added { role_color(Role::Ok) } else { ui.line }), Node { width: px(40), ..default() })).id();
        let a = c.spawn((Text::new(format!("+{}", fd.added)), font(&ui.f.mono, 12.0, 0.0), TextColor(role_color(Role::Ok)), Node { width: px(56), ..default() })).id();
        let d = c.spawn((Text::new(if fd.removed > 0 { format!("−{}", fd.removed) } else { "·".into() }), font(&ui.f.mono, 12.0, 0.0), TextColor(if fd.removed > 0 { role_color(Role::Bad) } else { ui.dim }), Node { width: px(56), ..default() })).id();
        let bars = bar_blocks(c, ui, fd.added, fd.removed);
        c.entity(row).add_children(&[p, st, a, d, bars]);
        c.entity(list).add_child(row);
    }
}

fn draw_diff(c: &mut Commands, ui: &Ui, root: Entity, app: &CodeApp, run: &crate::service::Run) {
    let b = ui.body(c, root);
    let h = ui.header(c, "Diff", "J K hunks · [ ] files · Z open in Zed", ui.dim);
    c.entity(b).add_child(h);
    let Some(fd) = app.file() else {
        let t = c.spawn((Text::new(if app.loading { "Reading the changes…" } else { "Nothing to show" }), font(&ui.f.ui500, 13.0, 0.4), TextColor(ui.dim), Node { margin: UiRect::all(px(18)), ..default() })).id();
        c.entity(b).add_child(t);
        return;
    };
    let fh = c.spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(10), padding: UiRect { left: px(18), right: px(18), top: px(8), bottom: px(6) }, ..default() }).id();
    let tri = ui.text(c, "Δ", &ui.f.mono, 13.0, 0.0, role_color(Role::Acc));
    let pt = ui.text(c, clip(&fd.path, 60), &ui.f.mono, 13.0, 0.0, ui.hot);
    let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
    let st = ui.text(c, format!("+{} −{}", fd.added, fd.removed), &ui.f.mono, 12.0, 0.0, ui.line);
    c.entity(fh).add_children(&[tri, pt, sp, st]);
    let body = c.spawn(Node { flex_direction: FlexDirection::Column, margin: UiRect { left: px(14), right: px(14), ..default() }, ..default() }).id();
    c.entity(b).add_children(&[fh, body]);
    if let Some(n) = &fd.note {
        let t = ui.text(c, n.clone(), &ui.f.ui500, 13.0, 0.4, ui.dim);
        c.entity(body).add_child(t);
        return;
    }
    let ok = role_color(Role::Ok);
    let bad = role_color(Role::Bad);
    let mut rows = 0;
    for (hi, hk) in fd.hunks.iter().enumerate().skip(app.hunk) {
        if rows >= DIFF_ROWS {
            break;
        }
        let sel = hi == app.hunk;
        let hh = c
            .spawn((
                Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, height: px(24), padding: UiRect { left: px(10), right: px(10), ..default() }, column_gap: px(10), border: UiRect::all(px(1)), margin: UiRect { top: px(if hi == app.hunk { 0.0 } else { 6.0 }), ..default() }, ..default() },
                BackgroundColor(with_alpha(ui.line, if sel { 0.08 } else { 0.03 })),
                BorderColor::all(with_alpha(ui.line, if sel { 0.55 } else { 0.2 })),
                Click::Alt(hi),
            ))
            .id();
        let at = ui.text(c, format!("{}:", hk.new_start), &ui.f.mono, 12.0, 0.0, role_color(Role::Acc));
        let ctx = ui.text(c, clip(if hk.context.is_empty() { &fd.path } else { &hk.context }, 40), &ui.f.mono, 12.0, 0.0, ui.hot);
        let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
        let hd = ui.text(c, hk.header.clone(), &ui.f.mono, 11.0, 0.0, ui.dim);
        let pm = c.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(6), ..default() }).id();
        let pa = ui.text(c, format!("+{}", hk.added()), &ui.f.mono, 11.0, 0.0, ok);
        let pd = ui.text(c, format!("−{}", hk.removed()), &ui.f.mono, 11.0, 0.0, bad);
        c.entity(pm).add_children(&[pa, pd]);
        c.entity(hh).add_children(&[at, ctx, sp, hd, pm]);
        c.entity(body).add_child(hh);
        rows += 1;
        if !sel {
            continue;
        }
        for l in &hk.lines {
            if rows >= DIFF_ROWS {
                break;
            }
            let (bg, mk, mc) = match l.kind {
                LineKind::Add => (with_alpha(ok, 0.075), "+", ok),
                LineKind::Del => (with_alpha(bad, 0.09), "−", bad),
                LineKind::Context => (Color::NONE, " ", ui.dim),
            };
            let row = c.spawn((Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, height: px(18), column_gap: px(8), padding: UiRect::left(px(6)), ..default() }, BackgroundColor(bg))).id();
            let num = |n: Option<u32>| n.map(|n| format!("{n:>4}")).unwrap_or_else(|| "    ".into());
            let nums = c.spawn((Text::new(format!("{} {}", num(l.old), num(l.new))), font(&ui.f.mono, 11.0, 0.0), TextColor(ui.dim), Node { width: px(78), flex_shrink: 0.0, ..default() })).id();
            let m = c.spawn((Text::new(mk), font(&ui.f.mono, 12.0, 0.0), TextColor(mc), Node { width: px(12), flex_shrink: 0.0, ..default() })).id();
            let code = c
                .spawn((Text::new(clip(&l.text, 86)), font(&ui.f.mono, 12.0, 0.0), TextColor(if l.kind == LineKind::Context { ui.line } else { ui.hot }), TextLayout { linebreak: LineBreak::NoWrap, ..default() }, Node { overflow: Overflow::clip(), flex_shrink: 1.0, ..default() }))
                .id();
            c.entity(row).add_children(&[nums, m, code]);
            c.entity(body).add_child(row);
            rows += 1;
        }
    }
    if fd.hunks.is_empty() {
        let t = ui.text(c, "Only the file mode changed", &ui.f.ui500, 13.0, 0.4, ui.dim);
        c.entity(body).add_child(t);
    }
    // the agent's reason for this file: what it said just before editing it, pinned over the hunk
    if let Some((why, _)) = run.reason_for(&fd.path) {
        let line = fd.hunks.get(app.hunk).map(|h| h.new_start).unwrap_or(1);
        let note = c
            .spawn((
                Node { position_type: PositionType::Absolute, right: px(22), top: px(96), width: px(300), flex_direction: FlexDirection::Column, row_gap: px(6), padding: UiRect::all(px(12)), border: UiRect::all(px(1)), ..default() },
                BorderColor::all(with_alpha(ui.line, 0.6)),
                BackgroundColor(Color::srgba(0.0, 0.01, 0.02, 0.92)),
            ))
            .id();
        let hr = ui.row(c, 8.0);
        let d = c.spawn((Node { width: px(6), height: px(6), ..default() }, BackgroundColor(ui.hot), UiTransform { rotation: Rot2::degrees(45.0), ..default() })).id();
        let t = ui.text(c, "AGENT'S REASON", &ui.f.ui700, 9.5, 2.2, ui.hot);
        let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
        let at = ui.text(c, clip(&format!("{}:{line}", fd.path), 24), &ui.f.mono, 10.0, 0.0, ui.dim);
        c.entity(hr).add_children(&[d, t, sp, at]);
        let tx = c.spawn((Text::new(clip(why.lines().find(|l| !l.trim().is_empty()).unwrap_or(why).trim(), 170)), font(&ui.f.ui500, 13.0, 0.3), TextColor(ui.hot), LineHeight::Px(17.0))).id();
        c.entity(note).add_children(&[hr, tx]);
        c.entity(b).add_child(note);
    }
}

fn draw_tests(c: &mut Commands, ui: &Ui, root: Entity, t: &TaskView, run: Option<&TestRun>) {
    let b = ui.body(c, root);
    let cmd = run.map(|r| r.command.clone()).or_else(|| t.workdir.as_deref().and_then(review::test_command).map(|v| v.join(" "))).unwrap_or_else(|| "no test command".into());
    let short = cmd.split(" --").next().unwrap_or(&cmd).to_string();
    let h = ui.header(c, "Tests", &short, ui.dim);
    let inner = c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(6), padding: UiRect { left: px(18), right: px(16), top: px(12), bottom: px(10) }, flex_grow: 1.0, ..default() }).id();
    c.entity(b).add_children(&[h, inner]);
    let Some(r) = run else {
        let m = c.spawn((Text::new("Not run yet. R (or click) runs the repo's tests in the agent's worktree."), font(&ui.f.ui500, 13.0, 0.4), TextColor(ui.line), LineHeight::Px(17.0), Click::Key(KeyCode::KeyR))).id();
        c.entity(inner).add_child(m);
        return;
    };
    let total = r.tests.len();
    let (state, sc) = if !r.done {
        ("RUNNING", Role::Hot)
    } else if r.error.is_some() && total == 0 {
        ("BUILD FAILED", Role::Bad)
    } else if r.failed > 0 {
        ("FAILED", Role::Bad)
    } else {
        ("PASSED", Role::Ok)
    };
    let top = c.spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::FlexEnd, column_gap: px(6), ..default() }).id();
    let big = c.spawn((Text::new(format!("{}", r.passed)), font(&ui.f.display, 26.0, 0.5), TextColor(ui.hot), LineHeight::Px(30.0))).id();
    let of = ui.text(c, format!("/ {total}"), &ui.f.ui500, 14.0, 0.5, ui.dim);
    let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
    let st = ui.text(c, state, &ui.f.ui700, 10.5, 2.2, role_color(sc));
    c.entity(top).add_children(&[big, of, sp, st]);
    // one cell per test (up to 48), filled as they pass
    let cells = c.spawn(Node { flex_direction: FlexDirection::Row, flex_wrap: FlexWrap::Wrap, column_gap: px(2), row_gap: px(2), margin: UiRect { top: px(4), bottom: px(6), ..default() }, ..default() }).id();
    let n = total.clamp(24, 48);
    for i in 0..n {
        let k = if total > 0 { i * total / n } else { usize::MAX };
        let col = match r.tests.get(k).map(|x| x.1) {
            Some(review::TestState::Pass) => role_color(Role::Fill),
            Some(review::TestState::Fail) => role_color(Role::Bad),
            Some(review::TestState::Ignored) => with_alpha(ui.line, 0.4),
            None => with_alpha(ui.line, 0.14),
        };
        let cell = c.spawn((Node { width: px(8), height: px(8), ..default() }, BackgroundColor(col))).id();
        c.entity(cells).add_child(cell);
    }
    c.entity(inner).add_children(&[top, cells]);
    if let Some(e) = r.error.as_ref().filter(|_| total == 0) {
        let t = c.spawn((Text::new(clip(e, 120)), font(&ui.f.mono, 11.0, 0.0), TextColor(role_color(Role::Bad)), LineHeight::Px(15.0))).id();
        c.entity(inner).add_child(t);
    }
    for (g, pass, tot, fail) in r.groups().into_iter().rev().take(5) {
        let row = c.spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(10), height: px(18), ..default() }).id();
        let mc = if fail { role_color(Role::Bad) } else if pass == tot { role_color(Role::Ok) } else { ui.dim };
        let m = c.spawn((Node { width: px(7), height: px(7), ..default() }, BackgroundColor(mc))).id();
        let nm = c.spawn((Text::new(clip(&g, 24)), font(&ui.f.mono, 12.0, 0.0), TextColor(ui.line), TextLayout { linebreak: LineBreak::NoWrap, ..default() }, Node { flex_grow: 1.0, overflow: Overflow::clip(), ..default() })).id();
        let ct = ui.text(c, format!("{pass} / {tot}"), &ui.f.mono, 11.0, 0.0, ui.dim);
        c.entity(row).add_children(&[m, nm, ct]);
        c.entity(inner).add_child(row);
    }
    let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
    let foot = c.spawn((Node { padding: UiRect::top(px(8)), border: UiRect::top(px(1)), ..default() }, BorderColor::all(with_alpha(ui.line, 0.2)), Click::Key(KeyCode::KeyR))).id();
    let ft = ui.text(c, if r.done { "R RUN AGAIN" } else { "RUNNING IN THE WORKTREE" }, &ui.f.ui600, 10.5, 2.0, ui.dim);
    c.entity(foot).add_child(ft);
    c.entity(inner).add_children(&[sp, foot]);
}

fn draw_acts(c: &mut Commands, ui: &Ui, root: Entity, app: &CodeApp) {
    let b = ui.body(c, root);
    let (a, d) = app.review.as_ref().map(|r| (r.added(), r.removed())).unwrap_or((0, 0));
    let tests = match &app.tests {
        Some(t) if !t.done => "tests running".to_string(),
        Some(t) if t.failed > 0 => format!("{} failing", t.failed),
        Some(t) if t.error.is_some() && t.tests.is_empty() => "build failed".into(),
        Some(_) => "tests pass".into(),
        None => "tests not run".into(),
    };
    let h = ui.header(c, "Review", &format!("+{a} −{d} · {tests}"), ui.dim);
    let inner = c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(9), padding: UiRect { left: px(16), right: px(16), top: px(14), bottom: px(14) }, ..default() }).id();
    c.entity(b).add_children(&[h, inner]);
    let armed = app.armed.map(|x| x.0);
    let buttons: [(&str, &str, Option<Act>, Role, bool); 5] = [
        ("MERGE", "M", Some(Act::Merge), Role::Hot, true),
        ("OPEN PR", "P", Some(Act::Pr), Role::Hot, false),
        ("ASK FOR CHANGE", "E", None, Role::Hot, false),
        ("OPEN IN ZED", "Z", None, Role::Hot, false),
        ("DISCARD", "D", Some(Act::Discard), Role::Bad, false),
    ];
    for (label, key, act, role, filled) in buttons {
        let is_armed = act.is_some() && act == armed;
        // armed: amber (red for discard) until you press again
        let col = if is_armed && role != Role::Bad { role_color(Role::Acc) } else { role_color(role) };
        let fill = filled || is_armed;
        let btn = c
            .spawn((
                Node { height: px(34), flex_direction: FlexDirection::Row, align_items: AlignItems::Center, padding: UiRect { left: px(12), right: px(10), ..default() }, border: UiRect::all(px(1)), ..default() },
                BackgroundColor(if fill { if is_armed && role == Role::Bad { col } else { with_alpha(col, if is_armed { 1.0 } else { 0.92 }) } } else { with_alpha(ui.line, 0.06) }),
                BorderColor::all(if fill { col } else { with_alpha(col, 0.55) }),
                Click::Key(match key {
                    "M" => KeyCode::KeyM,
                    "P" => KeyCode::KeyP,
                    "E" => KeyCode::KeyE,
                    "Z" => KeyCode::KeyZ,
                    _ => KeyCode::KeyD,
                }),
            ))
            .id();
        let text = if is_armed { format!("PRESS {key} AGAIN") } else { label.to_string() };
        let fg = if fill { Color::BLACK } else { col };
        let t = ui.text(c, text, &ui.f.ui600, 11.5, 2.3, fg);
        let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
        let kb = c.spawn((Node { padding: UiRect { left: px(4), right: px(4), top: px(1), bottom: px(0) }, border: UiRect::all(px(1)), ..default() }, BorderColor::all(with_alpha(fg, 0.5)))).id();
        let kt = ui.text(c, key, &ui.f.ui700, 9.5, 0.5, fg);
        c.entity(kb).add_child(kt);
        c.entity(btn).add_children(&[t, sp, kb]);
        c.entity(inner).add_child(btn);
    }
}

fn draw_strip(c: &mut Commands, ui: &Ui, root: Entity, agents: &[TaskView], cur: Option<&TaskView>) {
    let r = c.spawn(Node { width: percent(100), height: percent(100), flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(12), padding: UiRect { left: px(16), right: px(14), ..default() }, ..default() }).id();
    c.entity(root).add_child(r);
    let lab = ui.text(c, "AGENTS", &ui.f.ui600, 10.5, 3.2, ui.dim);
    c.entity(r).add_child(lab);
    for t in agents {
        let sel = cur.is_some_and(|c| c.id == t.id);
        let (col, sub) = match t.state {
            TaskState::Done => (role_color(Role::Ok), "Finished".to_string()),
            TaskState::NeedsYou => (role_color(Role::Acc), format!("Needs you · {}", clip(&t.status, 26))),
            TaskState::Failed => (role_color(Role::Bad), "Failed".to_string()),
            TaskState::Stopped => (ui.dim, "Stopped".to_string()),
            _ => (ui.line, clip(&t.status, 30)),
        };
        let chip = c
            .spawn((
                Node { height: px(36), flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(10), padding: UiRect { left: px(10), right: px(12), ..default() }, border: UiRect::all(px(1)), ..default() },
                BackgroundColor(if t.state == TaskState::NeedsYou { with_alpha(col, 0.12) } else { Color::NONE }),
                BorderColor::all(if sel { ui.hot } else { with_alpha(col, 0.5) }),
            ))
            .id();
        let dot = c.spawn((Node { width: px(8), height: px(8), ..default() }, BackgroundColor(col), UiTransform { rotation: Rot2::degrees(45.0), ..default() })).id();
        let col2 = c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(1), ..default() }).id();
        let tt = ui.text(c, clip(&t.title, 26), &ui.f.ui600, 12.5, 0.4, ui.hot);
        let st = ui.text(c, sub, &ui.f.ui500, 11.0, 0.4, col);
        c.entity(col2).add_children(&[tt, st]);
        c.entity(chip).add_children(&[dot, col2]);
        c.entity(r).add_child(chip);
    }
    if agents.is_empty() {
        let t = ui.text(c, "No agents on this repo", &ui.f.ui500, 12.0, 0.4, ui.dim);
        c.entity(r).add_child(t);
    }
    let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
    let legend = c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(4), align_items: AlignItems::FlexEnd, ..default() }).id();
    let l1 = ui.text(c, "J K HUNK   [ ] FILE   TAB AGENT   R TESTS   E ASK", &ui.f.ui600, 10.0, 1.6, ui.dim);
    let l2 = ui.text(c, "M MERGE   P PR   Z ZED   D DISCARD   ESC CLOSE", &ui.f.ui600, 10.0, 1.6, ui.dim);
    c.entity(legend).add_children(&[l1, l2]);
    c.entity(r).add_children(&[sp, legend]);
}
