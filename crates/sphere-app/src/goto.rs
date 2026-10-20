#![allow(deprecated)] // Interaction: Bevy 0.20 deprecates it in favour of ui_widgets, which this crate does not enable yet
//! Go to (G, or `go` / `go <filter>` in the command bar) and the NEW indicator. One list of every agent and every
//! terminal tab, grouped by repo, that you filter by typing and jump from: an agent opens in the Working view (or in
//! Code when it finished with changes to review), a tab opens in the Terminal. Results you haven't looked at yet sit
//! in a NEW section first, and the home shows how many there are ("N NEW", clickable) over the Agents panel.
//!
//! Row building, grouping, ordering and filtering are plain functions (`lines`, `matches`, `plan`); the rest is the
//! glass card (the Elsewhere list's style), its keys and mouse, and the system that tells sphered a result was seen
//! (`task.seen`) once the Working view, Code review, the swarm's focus panel or a follow-up shows it.
use crate::fonts::UiFonts;
use crate::live::Toasts;
use crate::panel::{font, with_alpha, Focus, Panel, PanelId};
use crate::service::Service;
use crate::util::*;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::text::LineHeight;
use sphere_proto::{TaskState, TaskView};
use std::collections::BTreeMap;

// ------------------------------------------------------------------ the list, as plain data

/// An agent as Go to lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentRow {
    pub id: String,
    pub title: String,
    /// the repo's folder name
    pub repo: String,
    pub repo_path: String,
    pub branch: String,
    pub state: TaskState,
    pub unseen: bool,
    /// when it last changed, unix seconds
    pub at: i64,
    /// its place among the tasks (creation order)
    pub seq: usize,
    /// the task that started it as its helper: (id, title; empty when that task is gone)
    pub helper_of: Option<(String, String)>,
    /// finished with changes: it opens in Code to review
    pub review: bool,
}

/// A terminal tab as Go to lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct TabRow {
    pub index: usize,
    pub label: String,
    pub cwd: Option<String>,
}

/// One line of the list: a section header, an agent or a tab.
#[derive(Clone, Debug, PartialEq)]
pub enum Line {
    Header { title: String, new: bool, count: usize },
    /// `indent`: a helper under its parent in the list; `show_repo`: the repo is not the section's heading (NEW)
    Agent { row: AgentRow, indent: bool, show_repo: bool },
    Tab(TabRow),
}

impl Line {
    fn selectable(&self) -> bool {
        !matches!(self, Line::Header { .. })
    }
}

/// Where a jump goes.
#[derive(Clone, Debug, PartialEq)]
pub enum Plan {
    /// Code review of this task
    Code(String),
    /// the Working view on this task
    Working(String),
    /// the Terminal with this tab in front
    Tab(usize),
}

/// What the line opens: an agent in the Working view, or in Code when it finished with changes; a tab in the Terminal.
pub fn plan(l: &Line) -> Option<Plan> {
    match l {
        Line::Header { .. } => None,
        Line::Agent { row, .. } if row.review => Some(Plan::Code(row.id.clone())),
        Line::Agent { row, .. } => Some(Plan::Working(row.id.clone())),
        Line::Tab(t) => Some(Plan::Tab(t.index)),
    }
}

/// Every agent as a row. A start that never ran (no session, step or cost) is not an agent.
pub fn agent_rows(tasks: &[TaskView]) -> Vec<AgentRow> {
    let title_of = |id: &str| tasks.iter().find(|t| t.id == id).map(|t| t.title.clone()).unwrap_or_default();
    tasks
        .iter()
        .enumerate()
        .filter(|(_, t)| !crate::live::never_ran(t))
        .map(|(seq, t)| AgentRow {
            id: t.id.clone(),
            title: t.title.clone(),
            repo: short_name(&t.repo),
            repo_path: t.repo.clone(),
            branch: t.branch.clone().unwrap_or_default(),
            state: t.state,
            unseen: t.unseen,
            at: crate::working::parse_rfc3339(&t.updated_at).unwrap_or(0),
            seq,
            helper_of: t.parent_task.as_ref().map(|p| (p.clone(), title_of(p))),
            review: t.state == TaskState::Done && !t.files.is_empty() && t.workdir.is_some(),
        })
        .collect()
}

/// The terminal tabs as rows: "zsh · tab 2" and the folder each started in.
pub fn tab_rows(terms: &crate::term::Terms) -> Vec<TabRow> {
    terms.tabs.iter().enumerate().map(|(i, t)| TabRow { index: i, label: format!("{} · tab {}", t.name, i + 1), cwd: t.cwd.clone() }).collect()
}

fn short_name(path: &str) -> String {
    std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).filter(|n| !n.is_empty()).unwrap_or_else(|| path.to_string())
}

/// Whether every word of `filter` appears in `hay` as a subsequence, ignoring case ("lgn" finds "Fix login test").
pub fn matches(filter: &str, hay: &str) -> bool {
    let hay: Vec<char> = hay.to_lowercase().chars().collect();
    filter.split_whitespace().all(|w| {
        let mut at = hay.iter();
        w.to_lowercase().chars().all(|c| at.any(|h| *h == c))
    })
}

/// Every word of `filter` is a subsequence of one of the fields (so "api" doesn't hide in the gaps between them).
fn matches_in(filter: &str, fields: &[&str]) -> bool {
    filter.split_whitespace().all(|w| fields.iter().any(|f| matches(w, f)))
}

/// Needs you first, then working (queued waits like working), then the rest.
fn rank(r: &AgentRow) -> u8 {
    match r.state {
        TaskState::NeedsYou => 0,
        TaskState::Working | TaskState::Queued => 1,
        _ => 2,
    }
}

/// The repo a tab belongs to: the longest repo folder that holds its start folder.
fn tab_repo<'a>(cwd: &str, repos: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    repos.filter(|r| !r.is_empty() && (cwd == *r || cwd.strip_prefix(*r).is_some_and(|rest| rest.starts_with('/')))).max_by_key(|r| r.len())
}

/// What the card shows for `filter`: a NEW section first (results you haven't looked at, newest first), then one
/// section per repo (the one that needs you, or changed last, first): its agents, needs-you first, then working, then
/// the rest by recency, each parent followed by its helpers; then the repo's terminal tabs, and the tabs that belong
/// to no repo under TERMINAL. A helper whose parent isn't in the list stands on its own with "helper of …".
pub fn lines(agents: &[AgentRow], tabs: &[TabRow], filter: &str) -> Vec<Line> {
    let keep: Vec<&AgentRow> = agents.iter().filter(|a| matches_in(filter, &[&a.title, &a.repo, &a.branch])).collect();
    let in_keep = |id: &str| keep.iter().any(|a| a.id == id);
    let helper_here = |a: &AgentRow| a.helper_of.as_ref().is_some_and(|(p, _)| in_keep(p));
    let mut kids: BTreeMap<&str, Vec<&AgentRow>> = BTreeMap::new();
    for a in keep.iter().filter(|a| helper_here(a)) {
        kids.entry(a.helper_of.as_ref().map(|(p, _)| p.as_str()).unwrap_or("")).or_default().push(a);
    }
    for v in kids.values_mut() {
        v.sort_by_key(|a| a.seq);
    }
    let tops: Vec<&AgentRow> = keep.iter().copied().filter(|a| !helper_here(a)).collect();
    let newest = |a: &&AgentRow, b: &&AgentRow| b.at.cmp(&a.at).then(a.seq.cmp(&b.seq));
    let put = |out: &mut Vec<Line>, a: &AgentRow, show_repo: bool| {
        out.push(Line::Agent { row: a.clone(), indent: false, show_repo });
        for k in kids.get(a.id.as_str()).into_iter().flatten() {
            out.push(Line::Agent { row: (*k).clone(), indent: true, show_repo });
        }
    };
    let count_of = |rows: &[&AgentRow]| rows.iter().map(|a| 1 + kids.get(a.id.as_str()).map_or(0, |k| k.len())).sum::<usize>();
    let mut out = Vec::new();
    // NEW
    let mut fresh: Vec<&AgentRow> = tops.iter().copied().filter(|a| a.unseen).collect();
    fresh.sort_by(newest);
    if !fresh.is_empty() {
        out.push(Line::Header { title: "NEW".into(), new: true, count: count_of(&fresh) });
        for a in &fresh {
            put(&mut out, a, true);
        }
    }
    // the repos
    struct Group<'a> {
        name: String,
        rows: Vec<&'a AgentRow>,
        tabs: Vec<&'a TabRow>,
    }
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    for a in tops.iter().copied().filter(|a| !a.unseen) {
        groups.entry(a.repo_path.clone()).or_insert_with(|| Group { name: a.repo.clone(), rows: Vec::new(), tabs: Vec::new() }).rows.push(a);
    }
    let repos: Vec<&str> = agents.iter().map(|a| a.repo_path.as_str()).collect();
    let tab_text = |t: &TabRow| format!("{} {}", t.label, t.cwd.as_deref().unwrap_or(""));
    for t in tabs.iter().filter(|t| matches(filter, &tab_text(t))) {
        let key = t.cwd.as_deref().and_then(|c| tab_repo(c, repos.iter().copied())).unwrap_or("").to_string();
        let name = if key.is_empty() { "TERMINAL".to_string() } else { short_name(&key) };
        groups.entry(key).or_insert_with(|| Group { name, rows: Vec::new(), tabs: Vec::new() }).tabs.push(t);
    }
    let mut ordered: Vec<(String, Group)> = groups.into_iter().collect();
    for (_, g) in ordered.iter_mut() {
        g.rows.sort_by(|a, b| rank(a).cmp(&rank(b)).then(newest(a, b)));
    }
    // a repo that needs you, then one that works, then the most recently changed; tabs-only groups (and TERMINAL) last
    let key = |k: &String, g: &Group| (k.is_empty(), g.rows.is_empty(), g.rows.first().map_or(3, |a| rank(a)), std::cmp::Reverse(g.rows.iter().map(|a| a.at).max().unwrap_or(0)), g.name.clone());
    ordered.sort_by_key(|a| key(&a.0, &a.1));
    for (_, g) in ordered {
        out.push(Line::Header { title: g.name.to_uppercase(), new: false, count: count_of(&g.rows) + g.tabs.len() });
        for a in &g.rows {
            put(&mut out, a, false);
        }
        for t in &g.tabs {
            out.push(Line::Tab((*t).clone()));
        }
    }
    out
}

/// How long ago, as the list says it: "now", "5m", "2h", "3d".
pub fn ago(secs: i64) -> String {
    match secs {
        s if s < 45 => "now".into(),
        s if s < 3600 => format!("{}m", (s + 30) / 60),
        s if s < 86_400 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86_400),
    }
}

/// How many of the lines are things to jump to (the headers are not).
pub fn item_count(lines: &[Line]) -> usize {
    lines.iter().filter(|l| l.selectable()).count()
}

/// The `n`-th jumpable line, and its place among all the lines.
pub fn item_at(lines: &[Line], n: usize) -> Option<(usize, &Line)> {
    lines.iter().enumerate().filter(|(_, l)| l.selectable()).nth(n)
}

/// The first line to show so that line `at` is in view, keeping its section's heading with it when it fits.
pub fn window(lines: &[Line], at: usize, scroll: usize, max: usize) -> usize {
    let max = max.max(1);
    let mut s = scroll;
    if at < s {
        s = at;
        if s > 0 && !lines[s - 1].selectable() {
            s -= 1;
        }
    } else if at >= s + max {
        s = at + 1 - max;
    }
    s.min(lines.len().saturating_sub(max))
}

/// The results you haven't looked at, newest first, as the home's indicator counts them.
pub fn unseen_count(tasks: &[TaskView]) -> usize {
    tasks.iter().filter(|t| t.unseen).count()
}

// ------------------------------------------------------------------ state

#[derive(Resource, Default)]
pub struct Goto {
    pub open: bool,
    pub filter: String,
    /// the selected one among the jumpable lines
    pub sel: usize,
    scroll: usize,
    /// bumps when anything on the card changes
    pub version: u64,
    lines: Vec<Line>,
    pending: Option<Plan>,
    /// the card just closed: the keyboard goes back to the bar's state after this frame's other key systems
    release: bool,
    /// how many lines the card shows at most (set by `draw` from the window's height)
    rows: usize,
    wheel_acc: f32,
}

impl Goto {
    pub fn show(&mut self, filter: &str) {
        self.open = true;
        self.filter = filter.to_string();
        self.sel = 0;
        self.scroll = 0;
        self.version += 1;
    }
    fn close(&mut self) {
        self.open = false;
        self.release = true;
        self.version += 1;
    }
    fn typed(&mut self) {
        self.sel = 0;
        self.scroll = 0;
        self.version += 1;
    }
    fn step(&mut self, d: i32) {
        let n = item_count(&self.lines).max(1) as i32;
        self.sel = (self.sel as i32 + d).rem_euclid(n) as usize;
        self.version += 1;
    }
    fn jump(&mut self) {
        if let Some((_, l)) = item_at(&self.lines, self.sel) {
            self.pending = plan(l);
        }
    }
}

/// Rebuilds the list from the service's tasks and the terminal's tabs.
fn rebuild(g: &mut Goto, tasks: &[TaskView], tabs: &[TabRow]) {
    g.lines = lines(&agent_rows(tasks), tabs, &g.filter);
    g.sel = g.sel.min(item_count(&g.lines).saturating_sub(1));
}

fn snapshot(svc: &Service, terms: &crate::term::Terms) -> (Vec<TaskView>, Vec<TabRow>) {
    (svc.shared.lock().unwrap().tasks.clone(), tab_rows(terms))
}

/// Registers the card's systems around the ones they hand the keyboard to and the mouse's: after the command bar's keys,
/// before the other views read theirs; the jump before the apps open.
pub fn build(app: &mut App) {
    app.add_systems(
        Update,
        (
            keys.after(crate::cmdbar::keys).before(crate::takeover::keys),
            buttons.after(crate::visor::buttons).before(apply),
            apply.after(crate::appframe::route_clicks).before(crate::home::apply),
            mark_seen,
            draw.after(crate::takeover::draw),
            place_chip.after(crate::mouse::update),
        ),
    );
}

// ------------------------------------------------------------------ keys

/// G (nothing else has the keyboard, no app is open) opens the card. While it is open it has the keyboard: type to
/// filter, ↑ ↓ select, Enter jumps, Esc closes. Runs right after the command bar's keys.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn keys(
    mut ev: MessageReader<KeyboardInput>,
    mut wheel: MessageReader<MouseWheel>,
    keys: Res<ButtonInput<KeyCode>>,
    mut g: ResMut<Goto>,
    mut focus: ResMut<Focus>,
    bar: Res<crate::cmdbar::CmdBar>,
    moment: Res<crate::moment::Moment>,
    svc: Res<Service>,
    terms: Res<crate::term::Terms>,
    mut sound: ResMut<crate::sound::Sound>,
    apps: (Res<crate::codeapp::CodeApp>, Res<crate::gitapp::GitApp>, Res<crate::containers::ContainersApp>, Res<crate::ports::PortsApp>, Res<crate::sysapp::SysApp>, Res<crate::filesapp::FilesApp>),
) {
    let mods = keys.any_pressed([KeyCode::SuperLeft, KeyCode::SuperRight, KeyCode::ControlLeft, KeyCode::ControlRight, KeyCode::AltLeft, KeyCode::AltRight]);
    let cmd = keys.any_pressed([KeyCode::SuperLeft, KeyCode::SuperRight]);
    let ctrl_cmd = cmd || keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let app_open = apps.0.open || apps.1.open || apps.2.open || apps.3.open || apps.4.open || apps.5.open;
    if !g.open {
        wheel.clear();
        for e in ev.read() {
            if e.state == ButtonState::Pressed && e.key_code == KeyCode::KeyG && !e.repeat && !mods && !bar.open && !focus.cmd && !focus.terminal && !moment.shown && !app_open {
                g.show("");
                sound.play_with("open", 0.7, 0.0);
                focus.cmd = true;
                break;
            }
        }
        return;
    }
    focus.cmd = true;
    focus.terminal = false;
    if ev.is_empty() && wheel.is_empty() {
        return;
    }
    let (tasks, tabs) = snapshot(&svc, &terms);
    rebuild(&mut g, &tasks, &tabs);
    for e in ev.read() {
        if e.state != ButtonState::Pressed || !g.open {
            continue;
        }
        match &e.logical_key {
            Key::Escape => {
                g.close();
                sound.play_with("close", 0.6, 0.0);
            }
            Key::Enter => {
                g.jump();
                rebuild(&mut g, &tasks, &tabs);
            }
            Key::ArrowDown => {
                g.step(1);
                sound.play_q("tick", 40);
            }
            Key::ArrowUp => {
                g.step(-1);
                sound.play_q("tick", 40);
            }
            Key::PageDown => g.step(8),
            Key::PageUp => g.step(-8),
            Key::Backspace => {
                if cmd {
                    g.filter.clear();
                } else {
                    g.filter.pop();
                }
                g.typed();
                rebuild(&mut g, &tasks, &tabs);
            }
            Key::Character(c) if cmd => {
                if c == "k" {
                    g.close();
                }
            }
            _ => {
                if let Some(t) = e.text.as_ref().filter(|_| !ctrl_cmd) {
                    let before = g.filter.len();
                    g.filter.extend(t.chars().filter(|c| !c.is_control()));
                    if g.filter.len() != before {
                        g.typed();
                        rebuild(&mut g, &tasks, &tabs);
                    }
                }
            }
        }
    }
    // the wheel selects: every 40 px one line
    let mut dy = 0.0;
    for w in wheel.read() {
        dy += match w.unit {
            MouseScrollUnit::Line => w.y * 40.0,
            MouseScrollUnit::Pixel => w.y,
        };
    }
    if dy != 0.0 && g.open {
        g.wheel_acc += dy;
        while g.wheel_acc.abs() >= 40.0 {
            let d = if g.wheel_acc > 0.0 { -1 } else { 1 };
            g.wheel_acc -= g.wheel_acc.signum() * 40.0;
            if (d < 0 && g.sel > 0) || (d > 0 && g.sel + 1 < item_count(&g.lines)) {
                g.step(d);
                sound.play_q("tick", 40);
            }
        }
    }
}

/// Carries out a jump, opens the card for the bar's `go` and the indicator, and gives the keyboard back once the card
/// closed. Runs after the other key systems, before the apps open.
#[allow(clippy::too_many_arguments)]
pub fn apply(
    mut g: ResMut<Goto>,
    mut open: ResMut<crate::apps::Open>,
    mut focus: ResMut<Focus>,
    bar: Res<crate::cmdbar::CmdBar>,
    svc: Res<Service>,
    mut terms: ResMut<crate::term::Terms>,
    mut toasts: ResMut<Toasts>,
    mut sound: ResMut<crate::sound::Sound>,
) {
    if let Some(f) = open.goto.take() {
        g.show(&f);
        focus.cmd = true;
        focus.terminal = false;
    }
    if let Some(p) = g.pending.take() {
        match p {
            Plan::Code(id) => {
                svc.seen(&id);
                open.code_task = Some(id);
                open.app(crate::apps::CODE, &mut toasts);
            }
            Plan::Working(id) => {
                svc.seen(&id);
                open.work_task = Some(id);
                open.app(1, &mut toasts);
            }
            Plan::Tab(i) => {
                terms.switch(i);
                open.app(0, &mut toasts);
            }
        }
        sound.play("tick");
        g.close();
    }
    if std::mem::take(&mut g.release) {
        focus.cmd = bar.open;
    }
}

// ------------------------------------------------------------------ marking results seen

/// Tells sphered a result was seen once something shows it: the Working view on its task, Code review of it, the swarm's
/// focus panel on it. Only a result that is new, and only once (`Service::seen`); a list scrolling past does nothing.
pub fn mark_seen(svc: Res<Service>, working: Res<crate::working::Working>, code: Res<crate::codeapp::CodeApp>, swarm: Res<crate::swarm::Swarm>, mut seen: Local<u64>) {
    let mut shown = {
        let s = svc.shared.lock().unwrap();
        if s.version == *seen && !working.is_changed() && !code.is_changed() && !swarm.is_changed() {
            return;
        }
        *seen = s.version;
        let mut shown: Vec<String> = Vec::new();
        if working.open && working.amt.x > 0.5 {
            shown.extend(crate::working::pick(&working, &s.tasks).map(|t| t.id.clone()));
        }
        if let Some(t) = code.shown_task(&s.tasks) {
            shown.push(t.id.clone());
        }
        shown
    };
    if let Some(id) = swarm.current().filter(|_| swarm.open) {
        shown.push(id.to_string());
    }
    for id in shown {
        svc.seen(&id);
    }
}

// ------------------------------------------------------------------ the card

#[derive(Component)]
pub struct GoRoot;
#[derive(Component)]
pub struct GoBox;
#[derive(Component)]
pub struct GoCaret;

#[derive(Component, Clone, Copy, PartialEq, Debug)]
pub enum Btn {
    /// anywhere off the card
    Away,
    /// the n-th jumpable line
    Row(usize),
}

/// The home's NEW indicator, over the Agents panel.
#[derive(Component)]
pub struct NewChip;
#[derive(Component)]
pub struct NewChipText;

pub fn setup(mut commands: Commands, root: Res<crate::hud::OverlayRoot>, f: Res<UiFonts>, args: Res<crate::Args>, svc: Res<Service>, mut terms: ResMut<crate::term::Terms>, mut g: ResMut<Goto>) {
    // the full screen: a click off the card closes it; the card hangs in it, centred, below the strip
    let e = commands
        .spawn((
            GoRoot,
            Interaction::None,
            Btn::Away,
            BackgroundColor(Color::NONE),
            bevy::ui::FocusPolicy::Pass,
            Node { position_type: PositionType::Absolute, left: px(0), right: px(0), top: px(0), bottom: px(0), justify_content: JustifyContent::Center, align_items: AlignItems::FlexStart, padding: UiRect { top: percent(11), ..default() }, ..default() },
            GlobalZIndex(6),
            Visibility::Hidden,
        ))
        .id();
    commands.entity(root.0).add_child(e);
    // the indicator: a small outlined chip, pinned over the Agents panel by `place_chip`
    let chip = commands
        .spawn((
            NewChip,
            Interaction::None,
            bevy::ui::FocusPolicy::Block,
            Node { position_type: PositionType::Absolute, left: px(0), top: px(0), padding: UiRect { left: px(8), right: px(9), top: px(5), bottom: px(4) }, border: UiRect::all(px(1)), column_gap: px(7), align_items: AlignItems::Center, ..default() },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.5)),
            BorderColor::all(with_alpha(role_color(Role::Fill), 0.8)),
            GlobalZIndex(2),
            Visibility::Hidden,
        ))
        .id();
    let sq = commands.spawn((Node { width: px(5), height: px(5), ..default() }, BackgroundColor(role_color(Role::Fill)))).id();
    let t = commands.spawn((NewChipText, Text::new("NEW"), font(&f.ui700, 10.0, 2.2), TextColor(role_color(Role::Hot)), LineHeight::Px(10.0))).id();
    commands.entity(chip).add_children(&[sq, t]);
    commands.entity(root.0).add_child(chip);
    if args.offline() && args.tasks {
        crate::service::seed_demo(&svc);
        // the demo's tabs: one in a repo, one anywhere
        if let Some(t) = terms.tabs.first_mut() {
            t.cwd = Some("/demo/api-server".into());
        }
        if let Ok(mut t) = crate::term::Terminal::spawn(None, true) {
            t.cwd = Some("/demo/notes".into());
            terms.tabs.push(t);
        }
    }
    if let Some(f) = &args.goto {
        g.show(f);
    }
}

/// Rebuilds the card when something on it changed, eases it in and out, keeps the home's mouse off it.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn draw(
    mut commands: Commands,
    mut g: ResMut<Goto>,
    f: Option<Res<UiFonts>>,
    sim: Res<crate::core::Sim>,
    svc: Res<Service>,
    terms: Res<crate::term::Terms>,
    mut ms: ResMut<crate::mouse::MouseState>,
    mut root: Query<(Entity, &mut Visibility), With<GoRoot>>,
    boxes: Query<(&ComputedNode, &UiGlobalTransform), With<GoBox>>,
    mut tfs: Query<&mut UiTransform, With<GoBox>>,
    mut carets: Query<&mut BackgroundColor, With<GoCaret>>,
    mut dimmer: Query<&mut BackgroundColor, (With<GoRoot>, Without<GoCaret>)>,
    win: Query<&Window, With<bevy::window::PrimaryWindow>>,
    mut seen: Local<(u64, u64, u64, i64, usize)>,
    mut amt: Local<Option<Spring>>,
) {
    let Some(f) = f else { return };
    let Ok((rootent, mut vis)) = root.single_mut() else { return };
    let want = g.open;
    let amt = amt.get_or_insert_with(|| Spring::new(0.0, 80.0, 18.0));
    amt.t = if want { 1.0 } else { 0.0 };
    amt.step(crate::motion::dt(crate::motion::Pace::Ui, sim.dt));
    if amt.x < 0.01 && !want {
        vis.set_if_neq(Visibility::Hidden);
        if ms.block.is_some() && *seen != (0, 0, 0, 0, 0) {
            ms.block = None;
        }
        *seen = (0, 0, 0, 0, 0);
        return;
    }
    vis.set_if_neq(Visibility::Inherited);
    // as many lines as the window holds under the card's top (a line is about 32 px)
    let h = win.single().map(|w| w.height()).unwrap_or(1000.0);
    let rows = (((h * 0.80 - 210.0) / 32.0) as usize).clamp(5, 14);
    let now = crate::core::unix_now();
    let key = (g.version, svc.shared.lock().unwrap().version, terms.version, now / 60, rows);
    if *seen != key {
        *seen = key;
        g.rows = rows;
        let (tasks, tabs) = snapshot(&svc, &terms);
        rebuild(&mut g, &tasks, &tabs);
        let at = item_at(&g.lines, g.sel).map(|(i, _)| i).unwrap_or(0);
        g.scroll = window(&g.lines, at, g.scroll, rows);
        commands.entity(rootent).despawn_children();
        build_card(&mut commands, rootent, &f, &g, now);
    }
    let k = clamp(amt.x, 0.0, 1.0);
    for mut t in &mut tfs {
        t.translation = Val2::px(0.0, (1.0 - k) * 10.0);
        t.scale = Vec2::splat(lerp(0.97, 1.0, k));
    }
    for mut d in &mut dimmer {
        d.set_if_neq(BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.5 * k)));
    }
    let on = g.open && (sim.t % 1.0) < 0.55;
    for mut c in &mut carets {
        c.set_if_neq(BackgroundColor(if on { role_color(Role::Fill) } else { Color::NONE }));
    }
    // the card's rectangle in window px, for the mouse
    ms.block = boxes.iter().next().and_then(|(n, t)| {
        let scale = win.single().map(|w| w.scale_factor()).unwrap_or(1.0).max(0.01);
        let half = n.size() / 2.0 / scale;
        let c = t.translation / scale;
        (n.size().x > 1.0).then(|| Rect::from_corners(c - half, c + half))
    });
}

fn mark_color(s: TaskState) -> Color {
    match s {
        TaskState::NeedsYou => role_color(Role::Acc),
        TaskState::Working => role_color(Role::Fill),
        TaskState::Done => role_color(Role::Ok),
        TaskState::Failed => role_color(Role::Bad),
        TaskState::Queued | TaskState::Stopped => role_color(Role::Dim),
    }
}

fn tilde(p: &str) -> String {
    match std::env::var("HOME") {
        Ok(h) if !h.is_empty() && p == h => "~".into(),
        Ok(h) if !h.is_empty() && p.strip_prefix(&h).is_some_and(|r| r.starts_with('/')) => format!("~{}", &p[h.len()..]),
        _ => p.to_string(),
    }
}

fn build_card(c: &mut Commands, root: Entity, f: &UiFonts, g: &Goto, now: i64) {
    let line = role_color(Role::Line);
    let hot = role_color(Role::Hot);
    let dim = role_color(Role::Dim);
    let fill = role_color(Role::Fill);
    let nowrap = TextLayout { linebreak: LineBreak::NoWrap, ..default() };
    let bx = c
        .spawn((
            GoBox,
            Interaction::None,
            bevy::ui::FocusPolicy::Block,
            Node {
                width: px(720),
                max_width: percent(94),
                flex_direction: FlexDirection::Column,
                padding: UiRect { left: px(18), right: px(18), top: px(14), bottom: px(12) },
                border: UiRect { left: px(3), right: px(1), top: px(1), bottom: px(1) },
                ..default()
            },
            BackgroundGradient(vec![Gradient::Linear(LinearGradient::to_bottom(vec![ColorStop::percent(Color::srgba(0.07, 0.09, 0.1, 1.0), 0.0), ColorStop::percent(Color::srgba(0.03, 0.04, 0.05, 1.0), 100.0)]))]),
            BorderColor { left: fill, right: with_alpha(line, 0.34), top: with_alpha(line, 0.34), bottom: with_alpha(line, 0.34) },
            UiTransform::default(),
        ))
        .id();
    c.entity(root).add_child(bx);
    // header: what it is, and what it holds
    let hdr = c
        .spawn((Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(9), padding: UiRect { bottom: px(9), ..default() }, border: UiRect { bottom: px(1), ..default() }, ..default() }, BorderColor::all(with_alpha(line, 0.72))))
        .id();
    let sq = c.spawn((Node { width: px(6), height: px(6), ..default() }, BackgroundColor(fill))).id();
    let t1 = c.spawn((Text::new("GO TO"), font(&f.ui600, 12.0, 3.84), TextColor(hot), LineHeight::Px(14.4))).id();
    let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
    let agents = g.lines.iter().filter(|l| matches!(l, Line::Agent { .. })).count();
    let tabs = g.lines.iter().filter(|l| matches!(l, Line::Tab(_))).count();
    let plural = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let count = format!("{} · {}", plural(agents, "AGENT", "AGENTS"), plural(tabs, "TAB", "TABS"));
    let t2 = c.spawn((Text::new(count), font(&f.ui600, 10.0, 2.4), TextColor(dim), LineHeight::Px(12.0))).id();
    c.entity(hdr).add_children(&[sq, t1, sp, t2]);
    c.entity(bx).add_child(hdr);
    // what you type
    let inp = c.spawn((Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(11), height: px(44), border: UiRect { bottom: px(1), ..default() }, ..default() }, BorderColor::all(with_alpha(line, 0.22)))).id();
    let pr = c.spawn((Text::new("›"), font(&f.mono, 18.0, 0.0), TextColor(fill))).id();
    let (txt, col) = if g.filter.is_empty() { ("Type to filter agents and tabs".to_string(), with_alpha(line, 0.5)) } else { (g.filter.clone(), hot) };
    let it = c.spawn((Text::new(txt), font(&f.ui500, 17.0, 0.4), TextColor(col), LineHeight::Px(20.0), nowrap, Node { flex_shrink: 1.0, min_width: px(0), overflow: Overflow::clip(), ..default() })).id();
    let caret = c.spawn((GoCaret, Node { width: px(2), height: px(19), margin: UiRect { left: px(-9), ..default() }, ..default() }, BackgroundColor(Color::NONE))).id();
    c.entity(inp).add_children(&[pr, it, caret]);
    c.entity(bx).add_child(inp);
    // the lines
    let list = c.spawn(Node { flex_direction: FlexDirection::Column, margin: UiRect { top: px(6), bottom: px(6), ..default() }, min_height: px(64), ..default() }).id();
    c.entity(bx).add_child(list);
    if g.lines.is_empty() {
        let e = c.spawn((Text::new("NOTHING MATCHES"), font(&f.ui600, 11.0, 3.0), TextColor(dim), LineHeight::Px(13.0), Node { margin: UiRect { top: px(22), left: px(12), ..default() }, ..default() })).id();
        c.entity(list).add_child(e);
    }
    let sel_line = item_at(&g.lines, g.sel).map(|(i, _)| i);
    let mut item = g.lines.iter().take(g.scroll).filter(|l| l.selectable()).count();
    let mut end = (g.scroll + g.rows.max(1)).min(g.lines.len());
    while end > g.scroll + 1 && !g.lines[end - 1].selectable() {
        end -= 1;
    }
    for (i, l) in g.lines.iter().enumerate().skip(g.scroll).take(end - g.scroll) {
        let e = match l {
            Line::Header { title, new, count } => header_line(c, f, title, *new, *count),
            Line::Agent { row, indent, show_repo } => {
                let e = agent_line(c, f, row, *indent, *show_repo, Some(i) == sel_line, item, now);
                item += 1;
                e
            }
            Line::Tab(t) => {
                let e = tab_line(c, f, t, Some(i) == sel_line, item);
                item += 1;
                e
            }
        };
        c.entity(list).add_child(e);
    }
    // the keys
    let keys = c.spawn((Node { padding: UiRect { top: px(10), ..default() }, border: UiRect { top: px(1), ..default() }, justify_content: JustifyContent::SpaceBetween, ..default() }, BorderColor::all(with_alpha(line, 0.13)))).id();
    let kt = c.spawn((Text::new("UP DOWN SELECT   ·   ENTER OPEN   ·   ESC CLOSE"), font(&f.ui700, 9.5, 1.9), TextColor(dim), LineHeight::Px(10.0))).id();
    let more = g.lines.len().saturating_sub(end);
    let kr = c.spawn((Text::new(if more > 0 { format!("+{more} MORE") } else { String::new() }), font(&f.ui700, 9.5, 1.9), TextColor(dim), LineHeight::Px(10.0))).id();
    c.entity(keys).add_children(&[kt, kr]);
    c.entity(bx).add_child(keys);
}

fn header_line(c: &mut Commands, f: &UiFonts, title: &str, new: bool, count: usize) -> Entity {
    let line = role_color(Role::Line);
    let col = if new { role_color(Role::Fill) } else { role_color(Role::Dim) };
    let e = c.spawn(Node { height: px(28), flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(10), padding: UiRect { left: px(14), right: px(12), top: px(6), ..default() }, ..default() }).id();
    if new {
        let sq = c.spawn((Node { width: px(5), height: px(5), ..default() }, BackgroundColor(col))).id();
        c.entity(e).add_child(sq);
    }
    let t = c.spawn((Text::new(title), font(&f.ui700, 9.5, 3.0), TextColor(col), LineHeight::Px(10.0))).id();
    let n = c.spawn((Text::new(count.to_string()), font(&f.mono, 10.0, 0.0), TextColor(role_color(Role::Dim)), LineHeight::Px(10.0))).id();
    let rule = c.spawn((Node { flex_grow: 1.0, height: px(1), ..default() }, BackgroundColor(with_alpha(line, if new { 0.3 } else { 0.13 })))).id();
    c.entity(e).add_children(&[t, n, rule]);
    e
}

fn row_shell(c: &mut Commands, sel: bool, item: usize, indent: bool) -> Entity {
    let line = role_color(Role::Line);
    c.spawn((
        Interaction::None,
        bevy::ui::FocusPolicy::Block,
        Btn::Row(item),
        Node { height: px(34), flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(10), padding: UiRect { left: px(if indent { 36 } else { 12 }), right: px(12), ..default() }, border: UiRect { left: px(2), ..default() }, ..default() },
        BackgroundColor(if sel { with_alpha(line, 0.1) } else { Color::NONE }),
        BorderColor { left: if sel { role_color(Role::Fill) } else { Color::NONE }, ..default() },
    ))
    .id()
}

#[allow(clippy::too_many_arguments)]
fn agent_line(c: &mut Commands, f: &UiFonts, r: &AgentRow, indent: bool, show_repo: bool, sel: bool, item: usize, now: i64) -> Entity {
    let line = role_color(Role::Line);
    let hot = role_color(Role::Hot);
    let dim = role_color(Role::Dim);
    let nowrap = TextLayout { linebreak: LineBreak::NoWrap, ..default() };
    let e = row_shell(c, sel, item, indent);
    if indent {
        // the elbow from its parent
        let el = c.spawn((Node { width: px(9), height: px(11), margin: UiRect { left: px(-16), bottom: px(5), ..default() }, border: UiRect { left: px(1), bottom: px(1), ..default() }, flex_shrink: 0.0, ..default() }, BorderColor::all(with_alpha(line, 0.34)))).id();
        c.entity(e).add_child(el);
    }
    let d = if indent { 6.0 } else { 7.0 };
    let mk = mark_color(r.state);
    let live = matches!(r.state, TaskState::Working | TaskState::NeedsYou);
    let mark = c.spawn((Node { width: px(d), height: px(d), border: UiRect::all(px(1)), flex_shrink: 0.0, ..default() }, BackgroundColor(if live { mk } else { Color::NONE }), BorderColor::all(mk), UiTransform { rotation: Rot2::degrees(45.0), ..default() })).id();
    let title = c
        .spawn((Text::new(crate::appframe::clip(&r.title, 44)), font(&f.ui500, 15.0, 0.3), TextColor(if sel { hot } else { line }), LineHeight::Px(18.0), nowrap, Node { flex_shrink: 1.0, min_width: px(0), overflow: Overflow::clip(), ..default() }))
        .id();
    c.entity(e).add_children(&[mark, title]);
    if let Some((_, parent)) = &r.helper_of {
        let t = if parent.is_empty() { "helper".to_string() } else { format!("helper of {}", crate::appframe::clip(parent, 26)) };
        let h = c.spawn((Text::new(t), font(&f.ui500, 12.0, 0.5), TextColor(with_alpha(line, 0.55)), LineHeight::Px(14.0), nowrap, Node { flex_shrink: 0.0, ..default() })).id();
        c.entity(e).add_child(h);
    }
    let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
    c.entity(e).add_child(sp);
    if show_repo {
        let rp = c.spawn((Text::new(r.repo.to_uppercase()), font(&f.ui600, 9.5, 1.6), TextColor(with_alpha(line, 0.55)), LineHeight::Px(10.0), nowrap, Node { flex_shrink: 0.0, ..default() })).id();
        c.entity(e).add_child(rp);
    }
    if r.unseen {
        let fill = role_color(Role::Fill);
        let nw = c.spawn((Node { padding: UiRect { left: px(6), right: px(6), top: px(3), bottom: px(2) }, border: UiRect::all(px(1)), column_gap: px(5), align_items: AlignItems::Center, flex_shrink: 0.0, ..default() }, BorderColor::all(with_alpha(fill, 0.8)))).id();
        let sq = c.spawn((Node { width: px(4), height: px(4), ..default() }, BackgroundColor(fill))).id();
        let t = c.spawn((Text::new("NEW"), font(&f.ui700, 9.5, 1.9), TextColor(hot), LineHeight::Px(10.0))).id();
        c.entity(nw).add_children(&[sq, t]);
        c.entity(e).add_child(nw);
    }
    let chip = crate::working::chip_of(c, f, r.state, false);
    let at = c.spawn((Node { width: px(34), justify_content: JustifyContent::FlexEnd, flex_shrink: 0.0, ..default() },)).id();
    let att = c.spawn((Text::new(ago(now - r.at)), font(&f.mono, 11.0, 0.0), TextColor(dim), LineHeight::Px(13.0))).id();
    c.entity(at).add_child(att);
    c.entity(e).add_children(&[chip, at]);
    e
}

fn tab_line(c: &mut Commands, f: &UiFonts, t: &TabRow, sel: bool, item: usize) -> Entity {
    let line = role_color(Role::Line);
    let hot = role_color(Role::Hot);
    let nowrap = TextLayout { linebreak: LineBreak::NoWrap, ..default() };
    let e = row_shell(c, sel, item, false);
    let glyph = c.spawn((Node { width: px(7), justify_content: JustifyContent::Center, flex_shrink: 0.0, ..default() }, Text::new(">_"), font(&f.mono_bold, 10.0, 0.0), TextColor(with_alpha(line, 0.6)), LineHeight::Px(12.0), nowrap)).id();
    let title = c.spawn((Text::new(t.label.clone()), font(&f.ui500, 15.0, 0.3), TextColor(if sel { hot } else { line }), LineHeight::Px(18.0), nowrap, Node { flex_shrink: 0.0, margin: UiRect { left: px(9), ..default() }, ..default() })).id();
    let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
    let cwd = t.cwd.as_deref().map(tilde).unwrap_or_default();
    let cw = c.spawn((Text::new(crate::appframe::clip(&cwd, 40)), font(&f.mono, 11.0, 0.0), TextColor(with_alpha(line, 0.5)), LineHeight::Px(13.0), nowrap, Node { flex_shrink: 1.0, min_width: px(0), overflow: Overflow::clip(), ..default() })).id();
    c.entity(e).add_children(&[glyph, title, sp, cw]);
    e
}

/// Hover selects (once the pointer moved), a press jumps, a press off the card closes it.
#[allow(clippy::type_complexity)]
pub fn buttons(
    q: Query<(&Interaction, &Btn), Changed<Interaction>>,
    mut g: ResMut<Goto>,
    ms: Res<crate::mouse::MouseState>,
    mut sound: ResMut<crate::sound::Sound>,
    mut last: Local<Option<Vec2>>,
) {
    let moved = *last != ms.pos;
    *last = ms.pos;
    if !g.open {
        return;
    }
    for (i, b) in &q {
        match (i, b) {
            (Interaction::Hovered, Btn::Row(r)) if moved && g.sel != *r => {
                g.sel = *r;
                g.version += 1;
                sound.play_q("hover", 60);
            }
            (Interaction::Pressed, Btn::Row(r)) => {
                g.sel = *r;
                g.jump();
            }
            (Interaction::Pressed, Btn::Away) => {
                g.close();
                sound.play_with("close", 0.6, 0.0);
            }
            _ => {}
        }
    }
}

// ------------------------------------------------------------------ the home's NEW indicator

/// Pins the indicator over the Agents panel's top right corner (wherever the panel is: it moves in Depth and when
/// dragged), shows the count, and opens the card when pressed. Hidden at zero, and with the panel.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn place_chip(
    svc: Res<Service>,
    g: Res<Goto>,
    mut open: ResMut<crate::apps::Open>,
    mut sound: ResMut<crate::sound::Sound>,
    cams: Query<(&Camera, &Transform), With<crate::CamCore>>,
    panels: Query<(&Panel, &Transform)>,
    mut chip: Query<(&Interaction, &mut Node, &mut Visibility, &mut BorderColor, &ComputedNode), (With<NewChip>, Without<Panel>)>,
    mut text: Query<&mut Text, With<NewChipText>>,
    mut was: Local<Interaction>,
) {
    let Ok((inter, mut node, mut vis, mut border, cn)) = chip.single_mut() else { return };
    let n = unseen_count(&svc.shared.lock().unwrap().tasks);
    let panel = panels.iter().find(|(p, _)| p.id == PanelId::Agents);
    let rect = panel.filter(|(p, _)| p.op.x > 0.5).and_then(|(p, tr)| {
        let (cam, ctr) = cams.single().ok()?;
        let gt = GlobalTransform::from(*ctr);
        let mut r: Option<Rect> = None;
        for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            let v = cam.world_to_viewport(&gt, tr.transform_point(Vec3::new(sx * p.w / 2.0, sy * p.h / 2.0, 0.0))).ok()?;
            r = Some(match r {
                None => Rect { min: v, max: v },
                Some(a) => Rect { min: a.min.min(v), max: a.max.max(v) },
            });
        }
        r
    });
    let show = n > 0 && !g.open && rect.is_some();
    vis.set_if_neq(if show { Visibility::Inherited } else { Visibility::Hidden });
    if let Ok(mut t) = text.single_mut() {
        let want = format!("{n} NEW");
        if t.0 != want {
            t.0 = want;
        }
    }
    if let Some(r) = rect {
        let size = cn.size() * cn.inverse_scale_factor();
        let size = if size.y > 1.0 { size } else { Vec2::new(64.0, 24.0) };
        // a tab on the panel's top edge, towards its right end
        let (l, t) = ((r.max.x - size.x - 26.0).round(), (r.min.y - size.y + 3.0).round());
        if node.left != px(l) || node.top != px(t) {
            node.left = px(l);
            node.top = px(t);
        }
    }
    let hover = *inter == Interaction::Hovered || *inter == Interaction::Pressed;
    let bc = with_alpha(role_color(Role::Fill), if hover { 1.0 } else { 0.8 });
    border.set_if_neq(BorderColor { top: bc, left: bc, right: bc, bottom: Color::NONE });
    if show && *inter != *was {
        match inter {
            Interaction::Hovered => sound.play_q("hover", 60),
            Interaction::Pressed => {
                open.goto = Some(String::new());
                sound.play_q("click", 90);
            }
            Interaction::None => {}
        }
    }
    *was = *inter;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(id: &str, repo: &str, state: TaskState, at: i64) -> AgentRow {
        AgentRow { id: id.into(), title: format!("Task {id}"), repo: repo.into(), repo_path: format!("/w/{repo}"), branch: format!("sphere/{id}"), state, unseen: false, at, seq: at as usize, helper_of: None, review: false }
    }

    fn helper(id: &str, of: &AgentRow, state: TaskState, at: i64) -> AgentRow {
        AgentRow { helper_of: Some((of.id.clone(), of.title.clone())), ..agent(id, &of.repo, state, at) }
    }

    fn tab(i: usize, cwd: &str) -> TabRow {
        TabRow { index: i, label: format!("zsh · tab {}", i + 1), cwd: Some(cwd.into()) }
    }

    fn shape(ls: &[Line]) -> Vec<String> {
        ls.iter()
            .map(|l| match l {
                Line::Header { title, count, .. } => format!("# {title} {count}"),
                Line::Agent { row, indent, .. } => format!("{}{}", if *indent { "  " } else { "" }, row.id),
                Line::Tab(t) => format!("tab{}", t.index),
            })
            .collect()
    }

    #[test]
    fn filtering_is_a_case_blind_subsequence_per_word() {
        assert!(matches("", "anything"));
        assert!(matches("LGN", "Fix login test"));
        assert!(matches("fix test", "Fix login test"));
        assert!(matches("api rate", "Add rate limiting api-server"));
        assert!(!matches("tsl", "Fix login test"));
        assert!(!matches("zz", "Fix login test"));
        assert!(matches("  ", "x"));
        assert!(matches_in("api rate", &["Add rate limiting", "api-server"]));
        assert!(!matches_in("api", &["Speed up cold start", "sphere-cli", "perf/cold-start"]));
    }

    #[test]
    fn needs_you_first_then_working_then_the_rest_by_recency() {
        let a = [agent("d1", "api", TaskState::Done, 50), agent("w", "api", TaskState::Working, 10), agent("n", "api", TaskState::NeedsYou, 5), agent("d2", "api", TaskState::Failed, 70), agent("q", "api", TaskState::Queued, 1)];
        assert_eq!(shape(&lines(&a, &[], "")), ["# API 5", "n", "w", "q", "d2", "d1"]);
    }

    #[test]
    fn repos_that_need_you_or_changed_last_come_first() {
        let a = [agent("a", "old", TaskState::Done, 100), agent("b", "busy", TaskState::Working, 10), agent("c", "wait", TaskState::NeedsYou, 5), agent("d", "new", TaskState::Done, 200)];
        let heads: Vec<String> = shape(&lines(&a, &[], "")).into_iter().filter(|s| s.starts_with('#')).collect();
        assert_eq!(heads, ["# WAIT 1", "# BUSY 1", "# NEW 1", "# OLD 1"]);
    }

    #[test]
    fn unseen_results_lead_in_their_own_section_newest_first() {
        let mut a = [agent("old", "api", TaskState::Done, 10), agent("new1", "web", TaskState::Done, 30), agent("new2", "api", TaskState::Done, 20), agent("w", "web", TaskState::Working, 5)];
        a[1].unseen = true;
        a[2].unseen = true;
        let ls = lines(&a, &[], "");
        assert_eq!(shape(&ls), ["# NEW 2", "new1", "new2", "# WEB 1", "w", "# API 1", "old"]);
        assert!(matches!(&ls[0], Line::Header { new: true, .. }));
        assert!(matches!(&ls[1], Line::Agent { show_repo: true, .. }));
        assert!(matches!(&ls[4], Line::Agent { show_repo: false, .. }));
    }

    #[test]
    fn helpers_follow_their_parent_indented() {
        let p = agent("p", "api", TaskState::Working, 100);
        let h1 = helper("h1", &p, TaskState::Working, 90);
        let h2 = helper("h2", &p, TaskState::Done, 95);
        let other = agent("o", "api", TaskState::Working, 200);
        let ls = lines(&[h2.clone(), p.clone(), other, h1.clone()], &[], "");
        assert_eq!(shape(&ls), ["# API 4", "o", "p", "  h1", "  h2"]);
        assert!(matches!(&ls[3], Line::Agent { indent: true, .. }));
        // an unseen parent takes its helpers into NEW with it
        let mut q = p.clone();
        q.unseen = true;
        let ls = lines(&[q, h1, h2], &[], "");
        assert_eq!(shape(&ls), ["# NEW 3", "p", "  h1", "  h2"]);
    }

    #[test]
    fn a_helper_whose_parent_is_filtered_out_stands_on_its_own() {
        let p = agent("p", "api", TaskState::Working, 100);
        let mut h = helper("h", &p, TaskState::Working, 90);
        h.title = "Audit routes".into();
        let ls = lines(&[p, h], &[], "audit");
        assert_eq!(shape(&ls), ["# API 1", "h"]);
        assert!(matches!(&ls[1], Line::Agent { indent: false, row, .. } if row.helper_of.is_some()));
    }

    #[test]
    fn the_filter_reads_title_repo_branch_and_tab() {
        let a = [agent("1", "api-server", TaskState::Working, 1), agent("2", "web-app", TaskState::Working, 2)];
        let tabs = [tab(0, "/w/api-server/src"), tab(1, "/tmp/x")];
        assert_eq!(shape(&lines(&a, &tabs, "web")), ["# WEB-APP 1", "2"]);
        assert_eq!(shape(&lines(&a, &tabs, "sphere/1")), ["# API-SERVER 1", "1"]);
        assert_eq!(shape(&lines(&a, &tabs, "zsh 2")), ["# TERMINAL 1", "tab1"]);
        assert_eq!(shape(&lines(&a, &tabs, "nothing at all")), Vec::<String>::new());
    }

    #[test]
    fn tabs_sit_under_the_repo_they_started_in_and_the_rest_under_terminal() {
        let a = [agent("1", "api", TaskState::Working, 1), agent("2", "web", TaskState::Done, 2)];
        let tabs = [tab(0, "/w/api"), tab(1, "/w/api/src/deep"), tab(2, "/w/apiary"), tab(3, "/home/me")];
        let ls = lines(&a, &tabs, "");
        assert_eq!(shape(&ls), ["# API 3", "1", "tab0", "tab1", "# WEB 1", "2", "# TERMINAL 2", "tab2", "tab3"]);
        // a tab alone in a repo with no agent shown still gets its heading
        let ls = lines(&a, &tabs, "deep");
        assert_eq!(shape(&ls), ["# API 1", "tab1"]);
    }

    #[test]
    fn jumps_go_to_code_when_there_is_something_to_review() {
        let mut done = agent("d", "api", TaskState::Done, 1);
        done.review = true;
        let working = agent("w", "api", TaskState::Working, 2);
        let ls = lines(&[done, working], &[tab(2, "/home")], "");
        let plans: Vec<Option<Plan>> = ls.iter().map(plan).collect();
        assert_eq!(plans, vec![None, Some(Plan::Working("w".into())), Some(Plan::Code("d".into())), None, Some(Plan::Tab(2))]);
    }

    #[test]
    fn items_skip_the_headers() {
        let ls = lines(&[agent("a", "x", TaskState::Working, 1), agent("b", "y", TaskState::Working, 2)], &[], "");
        assert_eq!(item_count(&ls), 2);
        assert_eq!(item_at(&ls, 0).map(|(i, _)| i), Some(1));
        assert_eq!(item_at(&ls, 1).map(|(i, _)| i), Some(3));
        assert!(item_at(&ls, 2).is_none());
    }

    #[test]
    fn the_window_keeps_the_selection_and_its_heading_in_view() {
        let a: Vec<AgentRow> = (0..6).map(|i| agent(&format!("a{i}"), "x", TaskState::Working, i)).chain((0..6).map(|i| agent(&format!("b{i}"), "y", TaskState::Working, 100 + i))).collect();
        let ls = lines(&a, &[], "");
        assert_eq!(ls.len(), 14);
        assert_eq!(window(&ls, 1, 0, 5), 0);
        assert_eq!(window(&ls, 6, 0, 5), 2);
        // back up to the first line of a section: its heading shows too
        let at = item_at(&ls, 6).map(|(i, _)| i).unwrap();
        assert!(matches!(ls[at - 1], Line::Header { .. }));
        assert_eq!(window(&ls, at, at + 3, 5), at - 1);
        // never past the end
        assert_eq!(window(&ls, 13, 0, 5), 9);
        assert_eq!(window(&ls, 2, 99, 5), 2);
    }

    #[test]
    fn ages_read_short() {
        assert_eq!(ago(10), "now");
        assert_eq!(ago(90), "2m");
        assert_eq!(ago(59 * 60), "59m");
        assert_eq!(ago(3 * 3600 + 5), "3h");
        assert_eq!(ago(2 * 86_400 + 9), "2d");
    }

    #[test]
    fn a_tab_belongs_to_the_longest_repo_holding_it() {
        let repos = ["/w/a", "/w/a/b", "/w/ab"];
        assert_eq!(tab_repo("/w/a/b/c", repos.iter().copied()), Some("/w/a/b"));
        assert_eq!(tab_repo("/w/a", repos.iter().copied()), Some("/w/a"));
        assert_eq!(tab_repo("/w/abc", repos.iter().copied()), None);
        assert_eq!(tab_repo("/x", repos.iter().copied()), None);
    }

    fn task(id: &str, state: TaskState, parent: Option<&str>, unseen: bool, files: bool) -> TaskView {
        serde_json::from_value(serde_json::json!({
            "id": id, "title": format!("Task {id}"), "prompt": "", "state": state, "status": "", "repo": "/w/api", "workdir": format!("/wt/{id}"),
            "branch": format!("sphere/{id}"), "in_place": false, "budget_usd": 0.0, "cost_usd": 0.5, "started_at": "2026-10-19T10:00:00Z", "updated_at": "2026-10-19T10:05:00Z",
            "steps": 3, "parent_task": parent, "unseen": unseen,
            "files": if files { serde_json::json!([{"path": "a.rs", "added": 1, "removed": 0}]) } else { serde_json::json!([]) },
        }))
        .unwrap()
    }

    #[test]
    fn rows_come_from_tasks_with_their_parents_and_review_flag() {
        let tasks = [task("p", TaskState::Working, None, false, false), task("h", TaskState::Done, Some("p"), false, false), task("d", TaskState::Done, None, true, true)];
        let rows = agent_rows(&tasks);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1].helper_of, Some(("p".into(), "Task p".into())));
        assert!(rows[2].review && rows[2].unseen && !rows[1].review);
        assert_eq!((rows[0].repo.as_str(), rows[0].branch.as_str()), ("api", "sphere/p"));
        assert_eq!(unseen_count(&tasks), 1);
        // a start that never ran is not an agent
        let mut dud = task("x", TaskState::Failed, None, false, false);
        dud.steps = 0;
        dud.cost_usd = 0.0;
        assert!(agent_rows(&[dud]).is_empty());
    }

    // ---- the keys, on a bare App

    fn press(app: &mut App, code: KeyCode, text: Option<&str>) {
        app.world_mut().write_message(KeyboardInput { key_code: code, logical_key: text.map_or(Key::Unidentified(bevy::input::keyboard::NativeKey::Unidentified), |t| Key::Character(t.into())), state: ButtonState::Pressed, text: text.map(Into::into), repeat: false, window: Entity::PLACEHOLDER });
        app.update();
    }

    fn named(app: &mut App, key: Key, code: KeyCode) {
        app.world_mut().write_message(KeyboardInput { key_code: code, logical_key: key, state: ButtonState::Pressed, text: None, repeat: false, window: Entity::PLACEHOLDER });
        app.update();
    }

    fn keyed() -> App {
        let mut app = App::new();
        app.add_message::<KeyboardInput>()
            .add_message::<MouseWheel>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<Focus>()
            .init_resource::<Goto>()
            .init_resource::<crate::cmdbar::CmdBar>()
            .init_resource::<crate::moment::Moment>()
            .init_resource::<crate::term::Terms>()
            .init_resource::<crate::sound::Sound>()
            .init_resource::<crate::apps::Open>()
            .init_resource::<Toasts>()
            .init_resource::<crate::codeapp::CodeApp>()
            .init_resource::<crate::gitapp::GitApp>()
            .init_resource::<crate::containers::ContainersApp>()
            .init_resource::<crate::ports::PortsApp>()
            .init_resource::<crate::sysapp::SysApp>()
            .init_resource::<crate::filesapp::FilesApp>();
        let (svc, _rx) = Service::with_receiver();
        {
            let mut s = svc.shared.lock().unwrap();
            s.tasks = vec![task("a", TaskState::Working, None, false, false), task("b", TaskState::Done, None, true, true)];
        }
        app.insert_resource(svc).add_systems(Update, (keys, apply).chain());
        app
    }

    #[test]
    fn g_opens_the_card_and_it_takes_the_keyboard() {
        let mut app = keyed();
        press(&mut app, KeyCode::KeyG, Some("g"));
        let g = app.world().resource::<Goto>();
        assert!(g.open && g.filter.is_empty(), "the opening g is not typed");
        assert!(app.world().resource::<Focus>().cmd);
        // typing filters, backspace edits, escape closes and gives the keyboard back
        press(&mut app, KeyCode::KeyB, Some("b"));
        assert_eq!(app.world().resource::<Goto>().filter, "b");
        assert_eq!(item_count(&app.world().resource::<Goto>().lines), 1);
        named(&mut app, Key::Backspace, KeyCode::Backspace);
        assert_eq!(app.world().resource::<Goto>().filter, "");
        named(&mut app, Key::Escape, KeyCode::Escape);
        assert!(!app.world().resource::<Goto>().open);
        assert!(!app.world().resource::<Focus>().cmd, "the keyboard is given back");
    }

    #[test]
    fn g_does_nothing_while_the_bar_the_terminal_or_an_app_has_the_keyboard() {
        for how in 0..3 {
            let mut app = keyed();
            match how {
                0 => app.world_mut().resource_mut::<crate::cmdbar::CmdBar>().open = true,
                1 => app.world_mut().resource_mut::<Focus>().terminal = true,
                _ => app.world_mut().resource_mut::<crate::filesapp::FilesApp>().open = true,
            }
            press(&mut app, KeyCode::KeyG, Some("g"));
            assert!(!app.world().resource::<Goto>().open, "case {how}");
        }
    }

    #[test]
    fn enter_jumps_to_the_selected_result_and_marks_it_seen() {
        let mut app = keyed();
        app.world_mut().resource_mut::<Goto>().show("");
        app.update();
        // NEW first: "b" finished with changes, so it opens in Code
        named(&mut app, Key::Enter, KeyCode::Enter);
        let o = app.world().resource::<crate::apps::Open>();
        assert_eq!(o.code_task.as_deref(), Some("b"));
        assert!(!app.world().resource::<Goto>().open);
        // down once more would have been the working one
        let mut app = keyed();
        app.world_mut().resource_mut::<Goto>().show("");
        app.update();
        named(&mut app, Key::ArrowDown, KeyCode::ArrowDown);
        named(&mut app, Key::Enter, KeyCode::Enter);
        assert_eq!(app.world().resource::<crate::apps::Open>().work_task.as_deref(), Some("a"));
    }

    #[test]
    fn the_bars_go_opens_the_card_with_its_filter() {
        let mut app = keyed();
        app.world_mut().resource_mut::<crate::apps::Open>().goto = Some("api".into());
        app.update();
        let g = app.world().resource::<Goto>();
        assert!(g.open);
        assert_eq!(g.filter, "api");
    }

    #[test]
    fn seen_is_told_once_while_a_result_is_new() {
        let (svc, mut rx) = Service::with_receiver();
        svc.shared.lock().unwrap().tasks = vec![task("b", TaskState::Done, None, true, true), task("c", TaskState::Done, None, false, true)];
        assert!(svc.seen("b"));
        assert!(!svc.seen("b"), "once");
        assert!(!svc.seen("c"), "not new");
        assert!(!svc.seen("nope"));
        assert!(matches!(rx.try_recv(), Ok(crate::service::Cmd::Seen { task }) if task == "b"));
        assert!(rx.try_recv().is_err());
    }
}
