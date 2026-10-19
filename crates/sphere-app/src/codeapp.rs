//! The Code app: review an agent's work before it lands. The core shrinks into the top-left
//! corner and a glass workspace unfolds with the task and its checkpoints, the changed files, a delta-style diff
//! by hunk, the repo's tests and the review actions (merge, open a PR, ask for a change, open in Zed, discard),
//! over a strip of the repo's agents. The changes are read with git from the agent's worktree; tests run with the
//! repo's own test command. Keys: J K (or the arrows) move through the diff line by line, H L hunk by hunk,
//! PgDn PgUp a page, [ ] files, Tab next agent, R run tests, M P E Z D actions (M, P and D ask twice), Esc
//! disarms an action, then closes.
use crate::core::CoreCtl;
use crate::fonts::UiFonts;
use crate::panel::*;
use crate::review::{self, FileDiff, LineKind, Review, TestRun};
use crate::service::{Cmd, Service};
use crate::appframe::{clip, Click, Ui};
use crate::gitdata::clip_start;
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
/// Lines of the selected hunk that show under its header.
const HUNK_ROWS: usize = DIFF_ROWS - 1;
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
    /// the first line of the selected hunk that shows
    scroll: usize,
    tests: Option<TestRun>,
    tests_rx: Option<Mutex<mpsc::Receiver<TestRun>>>,
    /// set to stop the running tests
    tests_cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// an action that asks twice: armed until this sim time
    armed: Option<(Act, f32)>,
    notes: (mpsc::Sender<String>, Mutex<mpsc::Receiver<String>>),
    /// a merge, PR or discard is running
    busy: bool,
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
            scroll: 0,
            tests: None,
            tests_rx: None,
            tests_cancel: None,
            armed: None,
            notes: (tx, Mutex::new(rx)),
            busy: false,
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
        self.scroll = 0;
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
    /// Lines in the selected hunk.
    fn hunk_len(&self) -> usize {
        self.file().and_then(|f| f.hunks.get(self.hunk)).map(|h| h.lines.len()).unwrap_or(0)
    }
    /// The furthest the selected hunk scrolls.
    fn max_scroll(&self) -> usize {
        self.hunk_len().saturating_sub(HUNK_ROWS)
    }
    /// Moves `d` lines through the diff: the selected hunk scrolls, and at its ends the next or previous hunk
    /// (and file) takes over.
    fn step_line(&mut self, d: i32) {
        if self.review.is_none() {
            return;
        }
        if d > 0 {
            if self.scroll < self.max_scroll() {
                self.scroll = (self.scroll + d as usize).min(self.max_scroll());
            } else {
                self.step_hunk(1);
            }
        } else if self.scroll > 0 {
            self.scroll = self.scroll.saturating_sub((-d) as usize);
        } else if self.hunk > 0 || self.file > 0 {
            self.step_hunk(-1);
            self.scroll = self.max_scroll();
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
        self.scroll = 0;
        self.version += 1;
    }
    fn select_task(&mut self, id: Option<String>) {
        self.task = id;
        self.review = None;
        self.tests = None;
        self.file = 0;
        self.hunk = 0;
        self.scroll = 0;
        self.armed = None;
        self.version += 1;
    }
}

/// A click on an agent chip picks the nth agent of the repo (file rows are numbered below this).
const AGENT_PICK: usize = 10_000;

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
/// The key the wheel presses over a pane (indices of `Pane::ALL`).
fn scroll_key(pane: Option<u8>, down: bool) -> Option<KeyCode> {
    match (Pane::ALL.get(pane? as usize)?, down) {
        (Pane::Diff, true) => Some(KeyCode::KeyJ),
        (Pane::Diff, false) => Some(KeyCode::KeyK),
        (Pane::Tree, true) => Some(KeyCode::BracketRight),
        (Pane::Tree, false) => Some(KeyCode::BracketLeft),
        _ => None,
    }
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
    (mut clicks, mut scrolls): (ResMut<crate::appframe::AppClicks>, ResMut<crate::appframe::AppScrolls>),
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
    // the wheel: over the diff it steps lines, over the files it steps files
    codes.extend(crate::appframe::scroll_keys(scrolls.take(0), scroll_key));
    // clicks on the panes: a button presses its key, a row picks its item
    for c in clicks.take(0) {
        match c {
            Click::Key(k) => codes.push(k),
            // a file row, or (10000 and up) an agent chip
            Click::Pick(i) if i >= AGENT_PICK => {
                let id = {
                    let s = svc.shared.lock().unwrap();
                    pick(&app, &s.tasks).and_then(|t| repo_tasks(&s.tasks, &t.repo).get(i - AGENT_PICK).map(|x| x.id.clone()))
                };
                if id.is_some() && id != pick(&app, &svc.shared.lock().unwrap().tasks).map(|t| t.id.clone()) {
                    app.select_task(id);
                    sound.play("toggle");
                }
            }
            Click::Pick(i) => {
                if app.review.as_ref().is_some_and(|r| i < r.files.len()) {
                    app.file = i;
                    app.hunk = 0;
                    app.scroll = 0;
                    app.version += 1;
                }
            }
            Click::Alt(i) => {
                if app.file().is_some_and(|f| i < f.hunks.len()) {
                    app.hunk = i;
                    app.scroll = 0;
                    app.version += 1;
                }
            }
        }
    }
    for key in codes {
        let task = { pick(&app, &svc.shared.lock().unwrap().tasks).cloned() };
        match key {
            KeyCode::Escape => {
                if app.armed.is_some() {
                    app.armed = None;
                    app.version += 1;
                } else {
                    app.close();
                    sound.play_with("collapse", 0.7, 0.0);
                }
            }
            KeyCode::KeyJ | KeyCode::ArrowDown => {
                app.step_line(1);
                sound.play_q("tick", 40);
            }
            KeyCode::KeyK | KeyCode::ArrowUp => {
                app.step_line(-1);
                sound.play_q("tick", 40);
            }
            KeyCode::PageDown => {
                app.step_line(HUNK_ROWS as i32 - 2);
                sound.play_q("tick", 40);
            }
            KeyCode::PageUp => {
                app.step_line(-(HUNK_ROWS as i32 - 2));
                sound.play_q("tick", 40);
            }
            KeyCode::KeyL => {
                app.step_hunk(1);
                sound.play_q("tick", 40);
            }
            KeyCode::KeyH => {
                app.step_hunk(-1);
                sound.play_q("tick", 40);
            }
            KeyCode::BracketRight => {
                app.step_file(1);
                sound.play_q("tick", 40);
            }
            KeyCode::BracketLeft => {
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
                    app.select_task(next);
                    sound.play("toggle");
                }
            }
            KeyCode::KeyR => match task.as_ref().and_then(|t| t.workdir.clone()) {
                Some(wd) => {
                    if app.tests.as_ref().is_some_and(|t| !t.done) {
                        if let Some(c) = &app.tests_cancel {
                            c.store(true, std::sync::atomic::Ordering::Relaxed);
                        }
                        toasts.push_quiet("Stopping the tests");
                        sound.play("toggle");
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
                let target = app.file().map(|f| (f.path.clone(), f.hunks.get(app.hunk).map(|h| h.new_start + app.scroll as u32).unwrap_or(1), f.status == review::Status::Deleted));
                match (task.as_ref().and_then(|t| t.workdir.clone()), target) {
                    (Some(_), Some((path, _, true))) => toasts.push(format!("{path} was deleted: nothing to open")),
                    (Some(wd), Some((path, line, _))) => match open_in_zed(&wd, &path, line) {
                        Ok(()) => {
                            toasts.push_quiet(format!("Opening {path}:{line} in Zed"));
                            sound.play("open");
                        }
                        Err(e) => toasts.push(e),
                    },
                    _ => toasts.push("No file to open"),
                }
            }
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
                if app.busy {
                    toasts.push_quiet("Still finishing the last action");
                    continue;
                }
                if act != Act::Discard && matches!(t.state, TaskState::Working | TaskState::Queued) {
                    toasts.push("The agent is still working: wait for it, or stop it first");
                    continue;
                }
                if t.workdir.as_deref().is_none_or(|w| !std::path::Path::new(w).is_dir()) {
                    toasts.push("The worktree is gone: nothing to act on");
                    continue;
                }
                if app.armed.is_some_and(|(a, until)| a == act && sim.t < until) {
                    app.armed = None;
                    app.busy = true;
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
    crate::gitdata::git_t(std::path::Path::new(dir), args)
}

/// Commits what's left in the worktree (a checkpoint of the agent's last edits).
fn commit_rest(wd: &str, title: &str) -> Result<(), String> {
    if !git(wd, &["status", "--porcelain"])?.is_empty() {
        git(wd, &["add", "-A"])?;
        git(wd, &["commit", "-q", "-m", title]).map_err(|e| format!("Could not commit the agent's changes: {e}"))?;
    }
    Ok(())
}

/// Commits what's left in the worktree, then merges the branch into the base in the repo.
fn merge(repo: &str, wd: &str, branch: Option<&str>, base: &str, title: &str, in_place: bool) -> Result<String, String> {
    let branch = branch.filter(|_| !in_place).ok_or("This agent worked in place: its changes are already in the repo")?;
    // sphered records the commit a task started from, not a branch: that one merges into whatever branch
    // the repo is on; a named base must be the branch the repo is on
    let cur = git(repo, &["symbolic-ref", "--short", "-q", "HEAD"]).unwrap_or_default();
    let is_branch = git(repo, &["rev-parse", "--verify", "-q", &format!("refs/heads/{base}")]).is_ok();
    let is_commit = !is_branch && git(repo, &["rev-parse", "--verify", "-q", &format!("{base}^{{commit}}")]).is_ok();
    if !is_branch && !is_commit {
        return Err(format!("{base} isn't a branch in the repo: merge {branch} by hand"));
    }
    if cur.is_empty() {
        return Err(format!("The repo is on a detached HEAD: switch to {} to merge", if is_branch { base } else { "a branch" }));
    }
    if is_branch && cur != base {
        return Err(format!("The repo is on {cur}, not {base}: switch to {base} to merge"));
    }
    let base = cur.as_str();
    if !git(repo, &["status", "--porcelain", "--untracked-files=no"])?.is_empty() {
        return Err(format!("The repo has uncommitted changes on {base}: commit or stash them to merge"));
    }
    commit_rest(wd, title)?;
    if git(repo, &["merge-base", "--is-ancestor", branch, base]).is_ok() {
        return Ok(format!("{branch} is already part of {base}: nothing to merge"));
    }
    match git(repo, &["merge", "--no-ff", "--no-edit", branch]) {
        Ok(_) => Ok(format!("Merged {branch} into {base}")),
        Err(e) => {
            // a merge that stops half way would leave the repo in a conflicted state
            let files = git(repo, &["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default();
            let _ = git(repo, &["merge", "--abort"]);
            let files: Vec<&str> = files.lines().take(3).collect();
            if files.is_empty() {
                Err(format!("Merge failed, nothing changed: {e}"))
            } else {
                Err(format!("Merge conflicts in {}: nothing changed. Ask the agent to merge {base} into its branch first", files.join(", ")))
            }
        }
    }
}

/// Pushes the branch and opens a pull request with the GitHub CLI.
fn open_pr(wd: &str, branch: Option<&str>, title: &str) -> Result<String, String> {
    let branch = branch.ok_or("This agent has no branch to open a PR from")?;
    if git(wd, &["remote"])?.is_empty() {
        return Err("This repo has no remote to push to".into());
    }
    commit_rest(wd, title)?;
    git(wd, &["push", "-q", "-u", "origin", branch]).map_err(|e| format!("Push failed: {e}"))?;
    let out = std::process::Command::new("gh").current_dir(wd).env("GH_PROMPT_DISABLED", "1").args(["pr", "create", "--fill", "--head", branch]).stdin(std::process::Stdio::null()).output().map_err(|_| format!("Pushed {branch}. Install the GitHub CLI (gh) to open the PR from here"))?;
    let text = |b: &[u8]| String::from_utf8_lossy(b).to_string();
    if out.status.success() {
        let url = text(&out.stdout).lines().rev().find(|l| l.starts_with("http")).unwrap_or("").to_string();
        Ok(format!("Opened a PR · {url}"))
    } else {
        let err = text(&out.stderr);
        if err.contains("already exists") {
            let url = err.lines().find(|l| l.trim().starts_with("http")).unwrap_or("").trim().to_string();
            return Ok(format!("Pushed {branch} · a PR already exists · {url}"));
        }
        Err(format!("Pushed {branch}, but gh: {}", crate::gitdata::err_line(&err)))
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
    let cmd = match review::detect_tests(&wd) {
        Ok(c) => c,
        Err(e) => {
            toasts.push(e);
            return;
        }
    };
    let (tx, rx) = mpsc::channel();
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    app.tests_rx = Some(Mutex::new(rx));
    app.tests_cancel = Some(cancel.clone());
    app.tests = Some(TestRun { command: cmd.join(" "), ..Default::default() });
    app.version += 1;
    std::thread::spawn(move || review::run_tests(&cmd, &wd, &tx, &cancel, review::TEST_TIMEOUT, review::KILL_GRACE));
}

/// Springs, the core's corner, reading the review in the background, test progress and action results.
#[allow(clippy::too_many_arguments)]
pub fn update(sim: Res<crate::core::Sim>, mut atlases: ResMut<AppAtlases>, mut app: ResMut<CodeApp>, mut ctl: ResMut<CoreCtl>, svc: Res<Service>, mut toasts: ResMut<crate::live::Toasts>, mut sound: ResMut<crate::sound::Sound>) {
    app.amt.t = if app.open { 1.0 } else { 0.0 };
    app.amt.step(crate::motion::dt(crate::motion::Pace::Big, sim.dt));
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
        app.busy = false;
        app.loaded.1 = u64::MAX; // read the review again
        app.version += 1;
    }
    let got = app.tests_rx.as_ref().and_then(|r| r.lock().ok()?.try_iter().last());
    if let Some(t) = got {
        if t.done {
            sound.play(match t.status() {
                review::RunStatus::Passed => "done",
                review::RunStatus::Cancelled => "collapse",
                _ => "error",
            });
        }
        app.tests = Some(t);
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
    if let Some(Some((task, r))) = got {
        app.loading = false;
        // look tests: SPHERE_CODE_KEYS plays keys once the first review is in (j k ] [ move, r runs the tests)
        if app.review.is_none() {
            if let Ok(keys) = std::env::var("SPHERE_CODE_KEYS") {
                let wd = r.workdir.clone();
                app.review = Some(r.clone());
                for k in keys.chars() {
                    match k {
                        'j' => app.step_line(1),
                        'k' => app.step_line(-1),
                        'l' => app.step_hunk(1),
                        'h' => app.step_hunk(-1),
                        'v' => app.step_line(HUNK_ROWS as i32 - 2),
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
            if app.review.as_ref() != Some(&r) {
                // keep the same file selected when the list changes under it
                let keep = app.file().map(|f| f.path.clone());
                let n = r.files.len();
                let at = keep.and_then(|p| r.files.iter().position(|f| f.path == p));
                app.review = Some(r);
                match at {
                    Some(i) => app.file = i,
                    None if app.file >= n => {
                        app.file = 0;
                        app.hunk = 0;
                    }
                    None => {}
                }
                app.hunk = app.hunk.min(app.file().map(|f| f.hunks.len().saturating_sub(1)).unwrap_or(0));
                app.scroll = app.scroll.min(app.max_scroll());
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
    let stale = app.loaded.0 != t.id || (app.loaded.1 != ver && sim.t - app.load_t > 1.5) || sim.t - app.load_t > 10.0;
    if !stale {
        return;
    }
    if app.loaded.0 != t.id {
        app.review = None;
        app.file = 0;
        app.hunk = 0;
        app.scroll = 0;
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
    let (task, run, agents, sver, link) = {
        let s = svc.shared.lock().unwrap();
        let t = pick(&app, &s.tasks).cloned();
        let run = t.as_ref().and_then(|t| s.runs.get(&t.id).cloned()).unwrap_or_default();
        let agents: Vec<TaskView> = t.as_ref().map(|t| repo_tasks(&s.tasks, &t.repo).into_iter().cloned().collect()).unwrap_or_default();
        (t, run, (agents, s.merged.clone()), s.version, s.link.clone())
    };
    let (agents, merged) = agents;
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
    draw_strip(c, &ui, root(Pane::Strip), &agents, &merged, task.as_ref());
    let Some(t) = task else {
        let b = ui.body(c, root(Pane::Diff));
        let h = ui.header(c, "Diff", "", ui.dim);
        let msg = match &link {
            Some(crate::service::Link::Down(e)) => format!("sphered isn't running ({}): agents and their changes come from it", clip(e, 70)),
            Some(crate::service::Link::Connecting) | Some(crate::service::Link::Starting) => "Connecting to sphered…".to_string(),
            _ => "No agent has worked on a repo yet. Start one with /  (repo: what to do).".to_string(),
        };
        let m = c.spawn((Text::new(msg), font(&f.ui500, 13.0, 0.5), TextColor(ui.line), Node { margin: UiRect::all(px(18)), ..default() })).id();
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
    let repo = t.map(|t| clip(&std::path::Path::new(&t.repo).file_name().map(|s| s.to_string_lossy().to_uppercase()).unwrap_or_default(), 40)).unwrap_or_default();
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
    let cr = ui.text(c, if r.is_some_and(|r| r.merged) { "MERGED" } else { "" }, &ui.f.ui600, 10.0, 1.6, role_color(Role::Ok));
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
            // real agents name files by absolute path in the worktree: show them relative to it
            let title = match t.workdir.as_deref() {
                Some(wd) => s.title.replace(&format!("{}/", wd.trim_end_matches('/')), ""),
                None => s.title.clone(),
            };
            items.push((title, at, false));
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
        let col = c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(2), flex_shrink: 1.0, overflow: Overflow::clip(), ..default() }).id();
        let tt = c.spawn((Text::new(clip(title, 30)), font(&ui.f.ui600, 13.0, 0.3), TextColor(if last { ui.hot } else { ui.line }), LineHeight::Px(16.0), TextLayout { linebreak: LineBreak::NoWrap, ..default() })).id();
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
        Some(r) if r.error.is_some() => String::new(),
        Some(r) if r.files.is_empty() => String::new(),
        Some(r) => format!("{} of {} files · +{} −{}{}", app.file + 1, r.files.len(), r.added(), r.removed(), if r.more > 0 { format!(" · {} more not listed", r.more) } else { String::new() }),
        None if app.loading => "reading…".into(),
        None => String::new(),
    };
    let h = ui.header(c, "Changed files", &sum, ui.dim);
    let list = c.spawn(Node { flex_direction: FlexDirection::Column, padding: UiRect { left: px(10), right: px(14), top: px(8), ..default() }, ..default() }).id();
    c.entity(b).add_children(&[h, list]);
    let Some(r) = r else { return };
    let note = |c: &mut Commands, s: String, col: Color| {
        let t = c.spawn((Text::new(s), font(&ui.f.ui500, 13.0, 0.4), TextColor(col), LineHeight::Px(18.0), Node { margin: UiRect::left(px(10)), ..default() })).id();
        c.entity(list).add_child(t);
    };
    if let Some(e) = &r.error {
        note(c, clip(e, 90), role_color(Role::Bad));
        return;
    }
    if r.files.is_empty() {
        let s = if r.merged { format!("Merged into {}: nothing left to review", r.base) } else { format!("No changes yet against {}", r.base) };
        note(c, s, ui.dim);
        return;
    }
    let first = app.file.saturating_sub(TREE_ROWS - 1).min(r.files.len().saturating_sub(TREE_ROWS));
    for (i, fd) in r.files.iter().enumerate().skip(first).take(TREE_ROWS) {
        let sel = i == app.file;
        let row = c
            .spawn((
                Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, height: px(22), padding: UiRect { left: px(10), right: px(4), ..default() }, border: UiRect::left(px(2)), column_gap: px(8), flex_shrink: 0.0, ..default() },
                BackgroundColor(if sel { with_alpha(ui.line, 0.10) } else { Color::NONE }),
                BorderColor::all(if sel { ui.hot } else { Color::NONE }),
                Click::Pick(i),
            ))
            .id();
        let (dir, name) = match fd.path.rsplit_once('/') {
            Some((d, n)) => (format!("{d}/"), n.to_string()),
            None => (String::new(), fd.path.clone()),
        };
        let p = c.spawn(Node { flex_direction: FlexDirection::Row, flex_grow: 1.0, flex_shrink: 1.0, overflow: Overflow::clip(), ..default() }).id();
        let name = clip_start(&name, 42);
        let dir = clip_start(&dir, 52usize.saturating_sub(name.chars().count()).max(8));
        let dt = ui.text(c, dir, &ui.f.mono, 12.0, 0.0, ui.dim);
        let nt = ui.text(c, name, &ui.f.mono, 12.0, 0.0, if sel { ui.hot } else { ui.line });
        c.entity(p).add_children(&[dt, nt]);
        let sc = match fd.status {
            review::Status::Added => role_color(Role::Ok),
            review::Status::Deleted => role_color(Role::Bad),
            review::Status::Renamed => role_color(Role::Acc),
            review::Status::Modified => ui.line,
        };
        let st = c.spawn((Text::new(fd.status.letter()), font(&ui.f.mono, 12.0, 0.0), TextColor(sc), Node { width: px(40), flex_shrink: 0.0, ..default() })).id();
        let (at, dt) = if fd.note.is_some() && fd.added + fd.removed == 0 { ("bin".to_string(), String::new()) } else { (format!("+{}", fd.added), if fd.removed > 0 { format!("−{}", fd.removed) } else { "·".into() }) };
        let a = c.spawn((Text::new(at), font(&ui.f.mono, 12.0, 0.0), TextColor(role_color(Role::Ok)), Node { width: px(56), flex_shrink: 0.0, ..default() })).id();
        let d = c.spawn((Text::new(dt), font(&ui.f.mono, 12.0, 0.0), TextColor(if fd.removed > 0 { role_color(Role::Bad) } else { ui.dim }), Node { width: px(56), flex_shrink: 0.0, ..default() })).id();
        let bars = bar_blocks(c, ui, fd.added, fd.removed);
        c.entity(row).add_children(&[p, st, a, d, bars]);
        c.entity(list).add_child(row);
    }
}

fn draw_diff(c: &mut Commands, ui: &Ui, root: Entity, app: &CodeApp, run: &crate::service::Run) {
    let b = ui.body(c, root);
    let h = ui.header(c, "Diff", "J K lines · H L hunks · [ ] files · Z open in Zed", ui.dim);
    c.entity(b).add_child(h);
    let Some(fd) = app.file() else {
        let msg = match app.review.as_ref() {
            Some(r) if r.error.is_some() => "Can't read the changes",
            _ if app.loading => "Reading the changes…",
            _ => "Nothing to show",
        };
        let t = c.spawn((Text::new(msg), font(&ui.f.ui500, 13.0, 0.4), TextColor(ui.dim), Node { margin: UiRect::all(px(18)), ..default() })).id();
        c.entity(b).add_child(t);
        return;
    };
    let hunk = fd.hunks.get(app.hunk);
    let fh = c.spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(10), padding: UiRect { left: px(18), right: px(18), top: px(8), bottom: px(6) }, flex_shrink: 0.0, ..default() }).id();
    let tri = ui.text(c, "Δ", &ui.f.mono, 13.0, 0.0, role_color(Role::Acc));
    let shown = match &fd.from {
        Some(old) => format!("{} → {}", clip_start(old, 20), clip_start(&fd.path, 28)),
        None => clip_start(&fd.path, 46),
    };
    let pt = ui.text(c, shown, &ui.f.mono, 13.0, 0.0, ui.hot);
    let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
    c.entity(fh).add_children(&[tri, pt, sp]);
    // where the view sits in the hunk, and the keys that move it (each clickable)
    if let Some(hk) = hunk.filter(|h| h.lines.len() > HUNK_ROWS) {
        let end = (app.scroll + HUNK_ROWS).min(hk.lines.len());
        let pos = ui.text(c, format!("{}–{} of {}", app.scroll + 1, end, hk.lines.len()), &ui.f.mono, 11.0, 0.0, ui.dim);
        c.entity(fh).add_child(pos);
    }
    for (label, key) in [("K ↑", KeyCode::KeyK), ("J ↓", KeyCode::KeyJ), ("H ‹", KeyCode::KeyH), ("L ›", KeyCode::KeyL)] {
        let chip = c.spawn((Node { padding: UiRect { left: px(5), right: px(5), top: px(2), bottom: px(1) }, border: UiRect::all(px(1)), flex_shrink: 0.0, ..default() }, BorderColor::all(with_alpha(ui.line, 0.4)), Click::Key(key))).id();
        let t = ui.text(c, label, &ui.f.mono, 11.0, 0.0, ui.line);
        c.entity(chip).add_child(t);
        c.entity(fh).add_child(chip);
    }
    let st = ui.text(c, format!("+{} −{}", fd.added, fd.removed), &ui.f.mono, 12.0, 0.0, ui.line);
    c.entity(fh).add_child(st);
    let body = c.spawn(Node { flex_direction: FlexDirection::Column, margin: UiRect { left: px(14), right: px(14), ..default() }, ..default() }).id();
    c.entity(b).add_children(&[fh, body]);
    let say = |c: &mut Commands, s: String| {
        let t = ui.text(c, s, &ui.f.ui500, 13.0, 0.4, ui.dim);
        c.entity(body).add_child(t);
    };
    if let Some(n) = &fd.note {
        say(c, n.clone());
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
                Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, height: px(24), padding: UiRect { left: px(10), right: px(10), ..default() }, column_gap: px(10), border: UiRect::all(px(1)), margin: UiRect { top: px(if hi == app.hunk { 0.0 } else { 6.0 }), ..default() }, flex_shrink: 0.0, ..default() },
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
        let start = app.scroll.min(hk.lines.len().saturating_sub(1));
        let room = DIFF_ROWS.saturating_sub(rows);
        let below = hk.lines.len().saturating_sub(start + room);
        // the last row says how much is left below, when there is more
        let take = if below > 0 { room.saturating_sub(1) } else { room };
        for l in hk.lines.iter().skip(start).take(take) {
            let (bg, mk, mc) = match l.kind {
                LineKind::Add => (with_alpha(ok, 0.075), "+", ok),
                LineKind::Del => (with_alpha(bad, 0.09), "−", bad),
                LineKind::Context => (Color::NONE, " ", ui.dim),
            };
            let row = c.spawn((Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, height: px(18), column_gap: px(8), padding: UiRect::left(px(6)), flex_shrink: 0.0, ..default() }, BackgroundColor(bg))).id();
            let num = |n: Option<u32>| n.map(|n| format!("{n:>4}")).unwrap_or_else(|| "    ".into());
            let nums = c.spawn((Text::new(format!("{} {}", num(l.old), num(l.new))), font(&ui.f.mono, 11.0, 0.0), TextColor(ui.dim), Node { width: px(78), flex_shrink: 0.0, ..default() })).id();
            let m = c.spawn((Text::new(mk), font(&ui.f.mono, 12.0, 0.0), TextColor(mc), Node { width: px(12), flex_shrink: 0.0, ..default() })).id();
            let code = c
                .spawn((Text::new(clip(&l.text, 74)), font(&ui.f.mono, 12.0, 0.0), TextColor(if l.kind == LineKind::Context { ui.line } else { ui.hot }), TextLayout { linebreak: LineBreak::NoWrap, ..default() }, Node { overflow: Overflow::clip(), flex_shrink: 1.0, ..default() }))
                .id();
            c.entity(row).add_children(&[nums, m, code]);
            c.entity(body).add_child(row);
            rows += 1;
        }
        if below > 0 {
            let more = c.spawn((Node { height: px(18), padding: UiRect::left(px(6)), flex_shrink: 0.0, align_items: AlignItems::Center, ..default() }, Click::Key(KeyCode::PageDown))).id();
            let t = ui.text(c, format!("{below} more line{} in this hunk · J or PgDn", if below == 1 { "" } else { "s" }), &ui.f.ui600, 10.0, 1.4, ui.dim);
            c.entity(more).add_child(t);
            c.entity(body).add_child(more);
            rows += 1;
        }
    }
    if fd.hunks.is_empty() {
        let s = match fd.status {
            review::Status::Added => "An empty file".to_string(),
            review::Status::Deleted => "An empty file, deleted".to_string(),
            review::Status::Renamed => format!("Renamed from {}, no other change", fd.from.as_deref().unwrap_or("?")),
            review::Status::Modified => "Only the file mode changed".to_string(),
        };
        say(c, s);
    }
    // the agent's reason for this file: what it said just before editing it, pinned over the hunk
    if let Some((why, _)) = run.reason_for(&fd.path) {
        let line = hunk.map(|h| h.new_start).unwrap_or(1);
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
        let at = ui.text(c, clip_start(&format!("{}:{line}", fd.path), 24), &ui.f.mono, 10.0, 0.0, ui.dim);
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
        let msg = match t.workdir.as_deref().map(review::detect_tests) {
            Some(Err(e)) => e,
            _ => "Not run yet. R (or click) runs the repo's tests in the agent's worktree.".to_string(),
        };
        let m = c.spawn((Text::new(msg), font(&ui.f.ui500, 13.0, 0.4), TextColor(ui.line), LineHeight::Px(17.0), Click::Key(KeyCode::KeyR))).id();
        c.entity(inner).add_child(m);
        return;
    };
    let total = r.total();
    let (state, sc) = match r.status() {
        review::RunStatus::Running => ("RUNNING", Role::Hot),
        review::RunStatus::Cancelling => ("CANCELLING", Role::Hot),
        review::RunStatus::Cancelled => ("CANCELLED", Role::Acc),
        review::RunStatus::TimedOut => ("TIMED OUT", Role::Bad),
        review::RunStatus::CouldNotRun => (if r.command.starts_with("cargo") { "BUILD FAILED" } else { "COULD NOT RUN" }, Role::Bad),
        review::RunStatus::Failed => ("FAILED", Role::Bad),
        review::RunStatus::Passed => ("PASSED", Role::Ok),
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
        let col = match r.cell(i, n) {
            Some(review::TestState::Pass) => role_color(Role::Fill),
            Some(review::TestState::Fail) => role_color(Role::Bad),
            Some(review::TestState::Ignored) => with_alpha(ui.line, 0.4),
            None => with_alpha(ui.line, 0.14),
        };
        let cell = c.spawn((Node { width: px(8), height: px(8), ..default() }, BackgroundColor(col))).id();
        c.entity(cells).add_child(cell);
    }
    c.entity(inner).add_children(&[top, cells]);
    let why = match r.status() {
        review::RunStatus::Cancelled => Some("Stopped. Nothing was kept from this run".to_string()),
        review::RunStatus::TimedOut => Some(format!("Stopped after {} min", review::TEST_TIMEOUT.as_secs() / 60)),
        _ if total == 0 || r.status() == review::RunStatus::Failed && r.failed == 0 => r.error.clone().or_else(|| r.note.clone()),
        review::RunStatus::Failed => r.note.clone(),
        _ => None,
    };
    if let Some(e) = why.as_ref() {
        let t = c.spawn((Text::new(clip(e, 120)), font(&ui.f.mono, 11.0, 0.0), TextColor(role_color(Role::Bad)), LineHeight::Px(15.0))).id();
        c.entity(inner).add_child(t);
    }
    let mut groups = r.groups();
    groups.reverse();
    // groups with a failure first
    groups.sort_by_key(|g| !g.3);
    for (g, pass, tot, fail) in groups.into_iter().take(5) {
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
    let ft = ui.text(c, if r.done { "R RUN AGAIN" } else if r.cancelling { "STOPPING…" } else { "R CANCEL" }, &ui.f.ui600, 10.5, 2.0, ui.dim);
    c.entity(foot).add_child(ft);
    c.entity(inner).add_children(&[sp, foot]);
}

fn draw_acts(c: &mut Commands, ui: &Ui, root: Entity, app: &CodeApp) {
    let b = ui.body(c, root);
    let (a, d) = app.review.as_ref().map(|r| (r.added(), r.removed())).unwrap_or((0, 0));
    let tests = match &app.tests {
        Some(t) => match t.status() {
            review::RunStatus::Running | review::RunStatus::Cancelling => "tests running".to_string(),
            review::RunStatus::Cancelled => "tests cancelled".into(),
            review::RunStatus::TimedOut => "tests timed out".into(),
            review::RunStatus::CouldNotRun => "tests could not run".into(),
            review::RunStatus::Failed if t.failed > 0 => format!("{} failing", t.failed),
            review::RunStatus::Failed => "tests failed".into(),
            review::RunStatus::Passed => "tests pass".into(),
        },
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

/// How many agent chips fit in the strip.
const STRIP_CHIPS: usize = 3;

/// The chips to draw out of `n` agents: a window of at most STRIP_CHIPS that always holds the selected one
/// (Tab can reach every agent, so the selection must never be hidden).
fn strip_window(n: usize, sel: Option<usize>) -> (usize, usize) {
    let from = sel.map_or(0, |i| (i + 1).saturating_sub(STRIP_CHIPS)).min(n.saturating_sub(STRIP_CHIPS));
    (from, (from + STRIP_CHIPS).min(n))
}

fn draw_strip(c: &mut Commands, ui: &Ui, root: Entity, agents: &[TaskView], merged: &std::collections::HashSet<String>, cur: Option<&TaskView>) {
    let r = c.spawn(Node { width: percent(100), height: percent(100), flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(12), padding: UiRect { left: px(16), right: px(14), ..default() }, ..default() }).id();
    c.entity(root).add_child(r);
    let lab = ui.text(c, "AGENTS", &ui.f.ui600, 10.5, 3.2, ui.dim);
    c.entity(r).add_child(lab);
    let (from, to) = strip_window(agents.len(), cur.and_then(|c| agents.iter().position(|t| t.id == c.id)));
    if from > 0 {
        let m = ui.text(c, format!("+{from}"), &ui.f.ui600, 11.0, 0.8, ui.dim);
        c.entity(r).add_child(m);
    }
    for (ai, t) in agents.iter().enumerate().take(to).skip(from) {
        let sel = cur.is_some_and(|c| c.id == t.id);
        let (col, sub) = match t.state {
            TaskState::Done => (role_color(Role::Ok), crate::appframe::finished(t, merged).to_string()),
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
                Click::Pick(AGENT_PICK + ai),
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
    if to < agents.len() {
        let m = ui.text(c, format!("+{}", agents.len() - to), &ui.f.ui600, 11.0, 0.8, ui.dim);
        c.entity(r).add_child(m);
    }
    if agents.is_empty() {
        let t = ui.text(c, "No agents on this repo", &ui.f.ui500, 12.0, 0.4, ui.dim);
        c.entity(r).add_child(t);
    }
    let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
    let legend = c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(4), align_items: AlignItems::FlexEnd, ..default() }).id();
    let l1 = ui.text(c, "J K LINE   H L HUNK   [ ] FILE   TAB AGENT   R TESTS   E ASK", &ui.f.ui600, 10.0, 1.6, ui.dim);
    let l2 = ui.text(c, "M MERGE   P PR   Z ZED   D DISCARD   ESC DISARM / CLOSE", &ui.f.ui600, 10.0, 1.6, ui.dim);
    c.entity(legend).add_children(&[l1, l2]);
    c.entity(r).add_children(&[sp, legend]);
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_strip_window_always_holds_the_selected_agent() {
        assert_eq!(super::strip_window(0, None), (0, 0));
        assert_eq!(super::strip_window(2, Some(1)), (0, 2));
        assert_eq!(super::strip_window(5, Some(0)), (0, 3));
        assert_eq!(super::strip_window(5, Some(2)), (0, 3));
        assert_eq!(super::strip_window(5, Some(3)), (1, 4));
        assert_eq!(super::strip_window(5, Some(4)), (2, 5));
        assert_eq!(super::strip_window(5, None), (0, 3));
    }

    use super::*;

    #[test]
    fn wheel_keys_per_pane() {
        let i = |p: Pane| Pane::ALL.iter().position(|x| *x == p).map(|i| i as u8);
        assert_eq!(scroll_key(i(Pane::Diff), true), Some(KeyCode::KeyJ));
        assert_eq!(scroll_key(i(Pane::Diff), false), Some(KeyCode::KeyK));
        assert_eq!(scroll_key(i(Pane::Tree), true), Some(KeyCode::BracketRight));
        assert_eq!(scroll_key(i(Pane::Tree), false), Some(KeyCode::BracketLeft));
        assert_eq!(scroll_key(i(Pane::Tests), true), None);
        assert_eq!(scroll_key(None, true), None);
    }
    use crate::review::{DiffLine, FileDiff, Hunk, Status};

    fn hunk(n: usize) -> Hunk {
        Hunk { header: "@@ -1 +1 @@".into(), context: String::new(), new_start: 10, lines: (0..n).map(|i| DiffLine { kind: LineKind::Add, old: None, new: Some(i as u32 + 10), text: format!("l{i}") }).collect() }
    }

    fn app(files: &[&[usize]]) -> CodeApp {
        let mut a = CodeApp::default();
        let files = files.iter().enumerate().map(|(i, hs)| FileDiff { path: format!("f{i}.rs"), from: None, status: Status::Modified, added: 0, removed: 0, hunks: hs.iter().map(|n| hunk(*n)).collect(), note: None }).collect();
        a.review = Some(Review { files, ..Default::default() });
        a
    }

    #[test]
    fn lines_scroll_through_a_long_hunk_before_moving_on() {
        let mut a = app(&[&[40, 3], &[2]]);
        assert_eq!(a.max_scroll(), 40 - HUNK_ROWS);
        for _ in 0..a.max_scroll() {
            a.step_line(1);
        }
        assert_eq!((a.file, a.hunk, a.scroll), (0, 0, 40 - HUNK_ROWS));
        // at the end of the hunk the next one takes over
        a.step_line(1);
        assert_eq!((a.file, a.hunk, a.scroll), (0, 1, 0));
        a.step_line(1);
        assert_eq!((a.file, a.hunk), (1, 0));
        // and back, landing at the end of the long hunk
        a.step_line(-1);
        a.step_line(-1);
        assert_eq!((a.file, a.hunk, a.scroll), (0, 0, 40 - HUNK_ROWS));
        a.step_line(-1000);
        assert_eq!(a.scroll, 0);
        a.step_line(-1);
        assert_eq!((a.file, a.hunk, a.scroll), (0, 0, 0));
    }

    #[test]
    fn pages_move_by_many_lines_and_hunks_jump() {
        let mut a = app(&[&[100, 5, 5]]);
        a.step_line(HUNK_ROWS as i32 - 2);
        assert_eq!(a.scroll, HUNK_ROWS - 2);
        a.step_hunk(1);
        assert_eq!((a.hunk, a.scroll), (1, 0));
        a.step_hunk(1);
        a.step_hunk(1);
        assert_eq!(a.hunk, 2);
        a.step_hunk(-1);
        assert_eq!(a.hunk, 1);
    }

    #[test]
    fn nothing_to_step_is_not_a_crash() {
        let mut a = CodeApp::default();
        a.step_line(1);
        a.step_line(-1);
        a.step_hunk(1);
        a.step_file(1);
        let mut a = app(&[]);
        a.step_line(1);
        a.step_hunk(-1);
        assert_eq!((a.file, a.hunk, a.scroll), (0, 0, 0));
        let mut a = app(&[&[]]);
        a.step_line(1);
        assert_eq!(a.hunk_len(), 0);
    }

    #[test]
    fn choosing_another_task_resets_the_view() {
        let mut a = app(&[&[40]]);
        a.scroll = 5;
        a.armed = Some((Act::Merge, 9.0));
        a.select_task(Some("t2".into()));
        assert!(a.review.is_none() && a.armed.is_none());
        assert_eq!((a.file, a.hunk, a.scroll), (0, 0, 0));
        assert_eq!(a.task.as_deref(), Some("t2"));
    }

    fn git_in(dir: &std::path::Path, args: &[&str]) {
        let out = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
        assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    fn repo_with_agent_branch(name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let base = std::env::temp_dir().join(format!("sphere-codeapp-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let repo = base.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        for a in [&["init", "-q", "-b", "main"][..], &["config", "user.email", "t@t"], &["config", "user.name", "t"], &["config", "commit.gpgsign", "false"]] {
            git_in(&repo, a);
        }
        std::fs::write(repo.join("a.txt"), "1\n").unwrap();
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "base"]);
        let wt = base.join("wt");
        git_in(&repo, &["worktree", "add", "-q", "-b", "sphere/agent", wt.to_str().unwrap()]);
        (repo, wt)
    }

    #[test]
    fn merges_the_agents_branch_and_commits_what_is_left() {
        let (repo, wt) = repo_with_agent_branch("merge");
        std::fs::write(wt.join("b.txt"), "new\n").unwrap();
        let note = merge(repo.to_str().unwrap(), wt.to_str().unwrap(), Some("sphere/agent"), "main", "Add b", false).unwrap();
        assert_eq!(note, "Merged sphere/agent into main");
        assert!(repo.join("b.txt").exists());
        // merging again has nothing to do
        let again = merge(repo.to_str().unwrap(), wt.to_str().unwrap(), Some("sphere/agent"), "main", "Add b", false).unwrap();
        assert!(again.contains("already part of"), "{again}");
        let _ = std::fs::remove_dir_all(repo.parent().unwrap());
    }

    #[test]
    fn merges_when_the_base_is_the_commit_the_task_started_from() {
        // sphered records a commit, not a branch
        let (repo, wt) = repo_with_agent_branch("sha");
        let sha = crate::gitdata::git_t(&repo, &["rev-parse", "HEAD"]).unwrap();
        // a fresh branch with no work of its own is not "merged"
        assert!(!crate::review::is_merged(wt.to_str().unwrap(), &sha));
        std::fs::write(wt.join("b.txt"), "new\n").unwrap();
        git_in(&wt, &["add", "."]);
        git_in(&wt, &["commit", "-q", "-m", "Add b"]);
        assert!(!crate::review::is_merged(wt.to_str().unwrap(), &sha), "committed but not merged yet");
        let note = merge(repo.to_str().unwrap(), wt.to_str().unwrap(), Some("sphere/agent"), &sha, "Add b", false).unwrap();
        assert_eq!(note, "Merged sphere/agent into main");
        assert!(repo.join("b.txt").exists());
        let r = crate::review::load(wt.to_str().unwrap(), &sha);
        assert!(r.merged, "a branch taken into main is merged");
        assert!(crate::review::is_merged(wt.to_str().unwrap(), &sha));
        let _ = std::fs::remove_dir_all(repo.parent().unwrap());
    }

    #[test]
    fn a_conflicting_merge_is_aborted_and_names_the_files() {
        let (repo, wt) = repo_with_agent_branch("conflict");
        std::fs::write(wt.join("a.txt"), "agent\n").unwrap();
        std::fs::write(repo.join("a.txt"), "mine\n").unwrap();
        git_in(&repo, &["commit", "-q", "-am", "mine"]);
        let e = merge(repo.to_str().unwrap(), wt.to_str().unwrap(), Some("sphere/agent"), "main", "Edit a", false).unwrap_err();
        assert!(e.contains("a.txt") && e.contains("nothing changed"), "{e}");
        // the repo is clean again, not stuck mid-merge
        assert!(!repo.join(".git/MERGE_HEAD").exists());
        assert_eq!(crate::gitdata::git_t(&repo, &["status", "--porcelain"]).unwrap(), "");
        let _ = std::fs::remove_dir_all(repo.parent().unwrap());
    }

    #[test]
    fn merge_explains_why_it_cant() {
        let (repo, wt) = repo_with_agent_branch("refuse");
        let (r, w) = (repo.to_str().unwrap(), wt.to_str().unwrap());
        assert!(merge(r, w, Some("sphere/agent"), "main", "t", true).unwrap_err().contains("in place"));
        assert!(merge(r, w, Some("sphere/agent"), "abc1234", "t", false).unwrap_err().contains("isn't a branch"));
        git_in(&repo, &["checkout", "-q", "--detach"]);
        assert!(merge(r, w, Some("sphere/agent"), "main", "t", false).unwrap_err().contains("detached"));
        git_in(&repo, &["checkout", "-q", "main"]);
        git_in(&repo, &["checkout", "-q", "-b", "other"]);
        assert!(merge(r, w, Some("sphere/agent"), "main", "t", false).unwrap_err().contains("switch to main"));
        git_in(&repo, &["checkout", "-q", "main"]);
        std::fs::write(repo.join("a.txt"), "dirty\n").unwrap();
        assert!(merge(r, w, Some("sphere/agent"), "main", "t", false).unwrap_err().contains("uncommitted"));
        let _ = std::fs::remove_dir_all(repo.parent().unwrap());
    }

    #[test]
    fn discarding_removes_the_worktree_but_keeps_the_branch() {
        let (repo, wt) = repo_with_agent_branch("discard");
        assert!(discard(repo.to_str().unwrap(), wt.to_str().unwrap(), true).is_err());
        let note = discard(repo.to_str().unwrap(), wt.to_str().unwrap(), false).unwrap();
        assert!(note.starts_with("Discarded"));
        assert!(!wt.exists());
        assert!(crate::gitdata::git_t(&repo, &["rev-parse", "--verify", "sphere/agent"]).is_ok());
        let r = review::load(wt.to_str().unwrap(), "main");
        assert!(r.error.unwrap().contains("gone"));
        let _ = std::fs::remove_dir_all(repo.parent().unwrap());
    }
}
