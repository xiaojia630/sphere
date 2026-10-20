//! The swarm's interactions (SPEC 3.2, 3.4, 3.6, 4.1-4.6): hover and click on satellites and their labels, the keys
//! (1-9, Tab, N, and in focus O / A / X / Enter), the focus panel's buttons, the command bar's swarm words, the
//! merge (two presses), stop, and keeping `Swarm.mode` in step with the Agent moment card (`Mode::Request`).
//!
//! The mouse. `mouse::update` still turns the core and drags panels; while the swarm is open it leaves clicks to
//! this module (a satellite or label selects it, empty space closes a focus). The hover is the HUD label under the
//! pointer, else the nearest satellite whose screen circle holds it (`pick_target`).
//!
//! Requests. Selecting an agent that needs you (or a request arriving while the overview is showing) puts
//! `Mode::Request(id)` up and `Moment::show_task` raises the card on that agent's open request; the card rises from
//! the satellite (`rise_origin`, read by `panel::update_panels`). The mode follows the card afterwards: stepping to
//! another request, answering (back to the overview) or putting it away.
//!
//! Events. `model::Events` is drained by `feedback` in `Last`; this module only reads it, remembering how many it
//! has seen (and the last one, to notice a drain and refill between two frames).
use super::hud::{HudKeys, HudOut};
use super::model::{Agent, Event, Model, State};
use super::scene::SceneOut;
use super::{Mode, Swarm};
use crate::apps::Open;
use crate::cmdbar::CmdBar;
use crate::core::Sim;
use crate::live::{Live, Toasts};
use crate::moment::Moment;
use crate::mouse::{Hit, MouseState};
use crate::panel::Focus;
use crate::service::{Cmd, Service};
use crate::sound::Sound;
use crate::CamCore;
use bevy::ecs::system::SystemParam;
use bevy::input::keyboard::KeyboardInput;
use bevy::input::ButtonState;
use bevy::prelude::*;
use std::collections::HashSet;
use std::sync::mpsc;
use std::sync::Mutex;

/// How long the first Enter keeps the merge armed (seconds).
pub const ARM_SECS: f32 = 3.0;

pub fn build(app: &mut App) {
    app.init_resource::<Input>();
    // after the home keys, the mouse and the HUD's button clicks (so their keys land this frame), before the apps open
    app.add_systems(
        Update,
        (demo_requests, drive, pointer)
            .chain()
            .after(super::view::keys)
            .after(crate::mouse::update)
            .after(super::hud::panels::clicks)
            .before(crate::apps::apply),
    );
}

// ------------------------------------------------------------------ pure logic

/// A satellite's circle on screen.
#[derive(Clone, Debug, PartialEq)]
pub struct Circle {
    pub id: String,
    pub c: Vec2,
    pub r: f32,
}

/// The satellites that can be hovered: faded in (>= .3), not merged or stopped, on screen; radius `27*k*scale+12`
/// px (SPEC 4.1) with `k` the px per world unit at that depth (`proj` projects a world point; `right` is the
/// camera's right direction, to measure `k`).
pub fn node_circles(nodes: &[(String, Vec3, f32, f32, f32)], model: &Model, proj: &dyn Fn(Vec3) -> Option<Vec2>, right: Vec3) -> Vec<Circle> {
    nodes
        .iter()
        .filter(|n| n.3 >= 0.3 && model.agent(&n.0).is_some_and(selectable))
        .filter_map(|n| {
            let c = proj(n.1)?;
            let k = proj(n.1 + right * 10.0).map(|q| (q - c).length() / 10.0).unwrap_or(1.0).max(0.05);
            Some(Circle { id: n.0.clone(), c, r: 27.0 * k * n.2 + 12.0 })
        })
        .collect()
}

/// The agent under the pointer: a label plate's rect first, else the nearest satellite circle holding it.
pub fn pick_target(p: Vec2, labels: &[(String, Rect)], circles: &[Circle]) -> Option<String> {
    if let Some((id, _)) = labels.iter().find(|(_, r)| r.contains(p)) {
        return Some(id.clone());
    }
    circles.iter().filter(|c| c.c.distance(p) <= c.r).min_by(|a, b| a.c.distance(p).total_cmp(&b.c.distance(p))).map(|c| c.id.clone())
}

/// Merged and stopped agents are on their way out: they cannot be picked.
fn selectable(a: &Agent) -> bool {
    !matches!(a.state, State::Merged | State::Stopped)
}

/// Where selecting `id` goes (SPEC 4.2): an agent that needs you opens its request, a merged one is ignored, any
/// other opens its focus panel.
pub fn mode_for(m: &Model, id: &str) -> Option<Mode> {
    let a = m.agent(id)?;
    match a.state {
        State::Merged | State::Stopped => None,
        State::Needs => Some(Mode::Request(a.id.clone())),
        _ => Some(Mode::Focus(a.id.clone())),
    }
}

/// What a key asks the view to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Nav {
    Go(Mode),
    Toast(String),
    Nothing,
}

/// 1-9: the n-th agent (1-based) in the model's order.
pub fn jump(m: &Model, n: usize) -> Nav {
    match n.checked_sub(1).and_then(|i| m.agents.get(i)) {
        None => Nav::Toast(format!("No agent {n}")),
        Some(a) => mode_for(m, &a.id).map(Nav::Go).unwrap_or(Nav::Nothing),
    }
}

/// Tab (`dir` 1) / Shift+Tab (-1): the next / previous agent, wrapping; from the first / last when none is open.
pub fn cycle(m: &Model, cur: Option<&str>, dir: i32) -> Nav {
    let ids: Vec<&str> = m.agents.iter().filter(|a| selectable(a)).map(|a| a.id.as_str()).collect();
    if ids.is_empty() {
        return Nav::Nothing;
    }
    let n = ids.len() as i32;
    let at = cur.and_then(|c| ids.iter().position(|i| *i == c));
    let i = match at {
        Some(i) => (i as i32 + dir.signum()).rem_euclid(n) as usize,
        None if dir >= 0 => 0,
        None => ids.len() - 1,
    };
    mode_for(m, ids[i]).map(Nav::Go).unwrap_or(Nav::Nothing)
}

/// N: the next agent that needs you, after the one open now (cyclic).
pub fn next_needing(m: &Model, cur: Option<&str>) -> Nav {
    let ids: Vec<&str> = m.agents.iter().filter(|a| a.state == State::Needs).map(|a| a.id.as_str()).collect();
    match (ids.len(), cur) {
        (0, _) => Nav::Toast("Nothing needs you right now".into()),
        (1, Some(c)) if ids[0] == c => Nav::Toast("That is the only one waiting".into()),
        _ => {
            let i = cur.and_then(|c| ids.iter().position(|i| *i == c)).map(|i| (i + 1) % ids.len()).unwrap_or(0);
            Nav::Go(Mode::Request(ids[i].to_string()))
        }
    }
}

/// The agent a "merge" (command) aims at: the focused one if it is done, else the first done agent.
pub fn merge_target(m: &Model, focused: Option<&str>) -> Option<String> {
    focused.and_then(|f| m.agent(f)).filter(|a| a.state == State::Done).or_else(|| m.agents.iter().find(|a| a.state == State::Done)).map(|a| a.id.clone())
}

/// What a merge press does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Press {
    /// first press: armed for `ARM_SECS`
    Armed,
    /// second press in time: merge
    Merge,
}

/// The two-press merge: the first press arms `id`; a second for the same agent within `ARM_SECS` merges; after that
/// time (or for another agent) it arms again.
pub fn merge_press(arm: &mut Option<(String, f32)>, id: &str, now: f32) -> Press {
    if arm.as_ref().is_some_and(|(a, until)| a == id && now < *until) {
        *arm = None;
        return Press::Merge;
    }
    *arm = Some((id.to_string(), now + ARM_SECS));
    Press::Armed
}

/// The command bar's words while the swarm is open (queued in `Open::swarm_cmds`).
#[derive(Clone, Debug, PartialEq)]
pub enum SwarmCmd {
    Next,
    Merge,
    Stop,
    Jump(usize),
    Back,
    /// "status" / "how is the swarm doing": Sphere says `feedback::status_line`
    Status,
}

/// `l` is the lower-cased, trimmed text.
pub fn parse(l: &str) -> Option<SwarmCmd> {
    let l = l.trim();
    let is = |ws: &[&str]| ws.contains(&l);
    let starts = |ws: &[&str]| ws.iter().any(|w| l == *w || l.starts_with(&format!("{w} ")));
    if is(&["what needs me", "what needs you", "waiting", "inbox", "approval", "approvals", "request", "requests", "need", "needs", "next"]) || starts(&["next"]) {
        return Some(SwarmCmd::Next);
    }
    if starts(&["merge"]) {
        return Some(SwarmCmd::Merge);
    }
    if starts(&["stop", "kill"]) {
        return Some(SwarmCmd::Stop);
    }
    if is(&["back", "home", "close", "exit"]) {
        return Some(SwarmCmd::Back);
    }
    if is(&["status", "report", "how", "how is it going", "how is the swarm doing", "how are they doing", "how are the agents doing", "swarm status"]) || starts(&["status", "report"]) {
        return Some(SwarmCmd::Status);
    }
    let rest = ["open ", "focus ", "jump ", "go to ", "agent "].iter().find_map(|p| l.strip_prefix(p)).unwrap_or(l);
    match rest.trim().parse::<usize>() {
        Ok(n) if (1..=9).contains(&n) => Some(SwarmCmd::Jump(n)),
        _ => None,
    }
}

/// Where the Agent moment card's rise starts: the satellite whose request is on the card (while the swarm is open).
pub fn rise_origin(swarm: &Swarm, moment: &Moment, live: &Live, scene: &SceneOut) -> Option<Vec3> {
    if !swarm.open {
        return None;
    }
    let task = crate::moment::waiting(live, moment).get(moment.index).map(|r| r.info.task.clone())?;
    scene.nodes.iter().find(|n| n.0 == task).map(|n| n.1)
}

// ------------------------------------------------------------------ state

/// A merge's outcome, from the background thread: (agent id, ok, the note or the error).
type Note = (String, bool, String);

#[derive(Resource)]
pub struct Input {
    /// the agent armed for merging and when it disarms (sim seconds)
    arm: Option<(String, f32)>,
    /// the mode last frame (to see who changed it)
    prev: Mode,
    /// agents that just began to need you, waiting for their request to show up in `Live` (id, sim second)
    pending: Vec<(String, f32)>,
    /// how many `Events` this module has seen, and the last of them
    seen: usize,
    last: Option<Event>,
    down: Option<(Vec2, f32)>,
    hover: Option<String>,
    merging: HashSet<String>,
    tx: mpsc::Sender<Note>,
    rx: Mutex<mpsc::Receiver<Note>>,
}

impl Default for Input {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Input { arm: None, prev: Mode::Overview, pending: Vec::new(), seen: 0, last: None, down: None, hover: None, merging: HashSet::new(), tx, rx: Mutex::new(rx) }
    }
}

#[derive(SystemParam)]
struct Env<'w> {
    swarm: ResMut<'w, Swarm>,
    moment: ResMut<'w, Moment>,
    live: Res<'w, Live>,
    svc: Res<'w, Service>,
    focus: ResMut<'w, Focus>,
    bar: ResMut<'w, CmdBar>,
    toasts: ResMut<'w, Toasts>,
    sound: ResMut<'w, Sound>,
    say: ResMut<'w, crate::homepanels::Say>,
    open: ResMut<'w, Open>,
    sim: Res<'w, Sim>,
    args: Res<'w, crate::Args>,
}

// ------------------------------------------------------------------ acting

/// Changes the mode with its sounds (SPEC 4.3): focus `open` .8 (`tick` .8 switching agents), `close` .7; leaving a
/// request puts its card back. Entering a request is the card's job (`sync`).
fn go(e: &mut Env, mode: Mode) {
    let from = e.swarm.mode.clone();
    if from == mode {
        return;
    }
    if let Mode::Request(a) = &from {
        if !matches!(&mode, Mode::Request(b) if b != a) {
            e.moment.put_back(&e.live, &mut e.sound);
        }
    }
    match (&from, &mode) {
        (Mode::Focus(_), Mode::Focus(_)) => e.sound.play_with("tick", 0.8, 0.0),
        (_, Mode::Focus(_)) => e.sound.play_with("open", 0.8, 0.0),
        (Mode::Focus(_), Mode::Overview) => e.sound.play_with("close", 0.7, 0.0),
        _ => {}
    }
    e.swarm.mode = mode;
    e.swarm.version += 1;
}

fn apply_nav(e: &mut Env, nav: Nav) {
    match nav {
        Nav::Go(m) => go(e, m),
        Nav::Toast(t) => e.toasts.push(t),
        Nav::Nothing => {}
    }
}

fn open_in_code(e: &mut Env, id: &str) {
    let Some(a) = e.swarm.model.agent(id) else { return };
    let repo = e.swarm.model.repos.get(a.repo).map(|r| r.key.clone()).unwrap_or_default();
    e.toasts.push_quiet(format!("Opening {repo} in Code"));
    e.sound.play_with("confirm", 0.5, 0.0);
    e.open.code_task = Some(id.to_string());
    let Env { open, toasts, .. } = e;
    open.app(2, toasts);
}

fn ask(e: &mut Env, id: &str) {
    let Some(a) = e.swarm.model.agent(id) else { return };
    let title = a.title.clone();
    let Env { bar, focus, .. } = e;
    bar.ask(id.to_string(), title, focus);
    e.sound.play("toggle");
}

fn stop(e: &mut Env, id: &str) {
    let Some(a) = e.swarm.model.agent(id) else { return };
    let title = a.title.clone();
    e.svc.send(Cmd::Stop { task: id.to_string() });
    e.toasts.push_quiet(format!("Stopped · {title} · worktree kept"));
    e.sound.play_with("discard", 0.6, 0.0);
    go(e, Mode::Overview);
}

/// Enter on a done agent / the Merge button / "merge": the first press arms, the second merges.
fn merge(e: &mut Env, st: &mut Input, id: &str) {
    let Some(a) = e.swarm.model.agent(id) else { return };
    let branch = a.branch.clone();
    match merge_press(&mut st.arm, id, e.sim.t) {
        Press::Armed => {
            e.toasts.push_quiet(format!("Press Enter again to merge {branch}"));
            e.sound.play("toggle");
        }
        Press::Merge => {
            e.sound.play_with("confirm", 0.7, 0.0);
            start_merge(e, st, id);
        }
    }
}

/// Runs `codeapp::merge` for the task in a background thread; on success the task joins `Shared::merged` (the model
/// shows it Merged at once and the scene collapses it).
fn start_merge(e: &mut Env, st: &mut Input, id: &str) {
    if e.args.offline() {
        e.toasts.push("Merging needs sphered: not in the demo");
        return;
    }
    if !st.merging.insert(id.to_string()) {
        e.toasts.push_quiet("Still merging");
        return;
    }
    let task = e.svc.shared.lock().unwrap().tasks.iter().find(|t| t.id == id).cloned();
    let Some(t) = task else {
        st.merging.remove(id);
        e.toasts.push("That agent is gone");
        return;
    };
    let (tx, shared, id) = (st.tx.clone(), e.svc.shared.clone(), id.to_string());
    std::thread::spawn(move || {
        let r = crate::codeapp::merge(&t.repo, t.workdir.as_deref().unwrap_or(""), t.branch.as_deref(), t.base.as_deref().unwrap_or("main"), &t.title, t.in_place);
        let note = match r {
            Ok(note) => {
                let mut s = shared.lock().unwrap();
                s.merged.insert(id.clone());
                s.version += 1;
                (id, true, note)
            }
            Err(err) => (id, false, err),
        };
        let _ = tx.send(note);
    });
}

/// What a key does with the swarm open and nothing typing.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Key {
    Jump(usize),
    Cycle(i32),
    Next,
    Open,
    Ask,
    Stop,
    Enter,
}

fn key_of(code: KeyCode, shift: bool) -> Option<Key> {
    let digit = [KeyCode::Digit1, KeyCode::Digit2, KeyCode::Digit3, KeyCode::Digit4, KeyCode::Digit5, KeyCode::Digit6, KeyCode::Digit7, KeyCode::Digit8, KeyCode::Digit9].iter().position(|d| *d == code);
    Some(match code {
        _ if digit.is_some() => Key::Jump(digit.unwrap() + 1),
        KeyCode::Tab => Key::Cycle(if shift { -1 } else { 1 }),
        KeyCode::KeyN => Key::Next,
        KeyCode::KeyO => Key::Open,
        KeyCode::KeyA => Key::Ask,
        KeyCode::KeyX => Key::Stop,
        KeyCode::Enter => Key::Enter,
        _ => return None,
    })
}

fn press(e: &mut Env, st: &mut Input, k: Key) {
    let cur = e.swarm.current().map(str::to_string);
    let focused = match &e.swarm.mode {
        Mode::Focus(id) => Some(id.clone()),
        _ => None,
    };
    // the card owns the digits (a question's options) and Enter / A / X / O while it is up
    let card = matches!(e.swarm.mode, Mode::Request(_));
    match k {
        Key::Jump(n) if !card => {
            let nav = jump(&e.swarm.model, n);
            apply_nav(e, nav);
        }
        Key::Cycle(d) => {
            let nav = cycle(&e.swarm.model, cur.as_deref(), d);
            apply_nav(e, nav);
        }
        Key::Next => {
            let nav = next_needing(&e.swarm.model, cur.as_deref());
            apply_nav(e, nav);
        }
        Key::Open => {
            if let Some(id) = focused {
                open_in_code(e, &id);
            }
        }
        Key::Ask => {
            if let Some(id) = focused {
                ask(e, &id);
            }
        }
        Key::Stop => {
            if let Some(id) = focused {
                stop(e, &id);
            }
        }
        Key::Enter => {
            if let Some(id) = focused {
                if e.swarm.model.agent(&id).is_some_and(|a| a.state == State::Done) {
                    merge(e, st, &id);
                } else {
                    open_in_code(e, &id);
                }
            }
        }
        _ => {}
    }
}

fn command(e: &mut Env, st: &mut Input, c: SwarmCmd) {
    let cur = e.swarm.current().map(str::to_string);
    match c {
        SwarmCmd::Next => {
            let nav = next_needing(&e.swarm.model, cur.as_deref());
            apply_nav(e, nav);
        }
        SwarmCmd::Jump(n) => {
            let nav = jump(&e.swarm.model, n);
            apply_nav(e, nav);
        }
        SwarmCmd::Merge => match merge_target(&e.swarm.model, if matches!(e.swarm.mode, Mode::Focus(_)) { cur.as_deref() } else { None }) {
            None => e.toasts.push("Nothing is ready to merge yet"),
            Some(id) => {
                // Enter merges the focused agent, so bring this one up first
                go(e, Mode::Focus(id.clone()));
                merge(e, st, &id);
            }
        },
        SwarmCmd::Stop => match cur {
            Some(id) => stop(e, &id),
            None => e.toasts.push("Open an agent first, then say stop"),
        },
        SwarmCmd::Status => {
            let line = super::feedback::status_line(&e.swarm.model, e.live.waited_today_secs(crate::core::unix_now()));
            e.sound.speak(&line);
            e.say.say(line);
        }
        SwarmCmd::Back => {
            if e.swarm.mode == Mode::Overview {
                e.swarm.close();
                e.sound.play_with("close", 0.7, 0.0);
            } else {
                go(e, Mode::Overview);
            }
        }
    }
}

// ------------------------------------------------------------------ systems

/// `--swarm` / `--demo` has no sphered: an agent that needs you gets a request of its own (the prototype's two),
/// so the Agent moment card has something to show.
fn demo_requests(swarm: Res<Swarm>, args: Res<crate::Args>, mut live: ResMut<Live>) {
    if !args.offline() || !swarm.open {
        return;
    }
    let mut add = Vec::new();
    for a in swarm.model.agents.iter().filter(|a| a.state == State::Needs) {
        if !live.requests.iter().any(|r| r.info.task == a.id) {
            add.push(demo_request(a));
        }
    }
    if add.is_empty() {
        return;
    }
    for r in add {
        let title = swarm.model.agent(&r.info.task).map(|a| a.title.clone()).unwrap_or_default();
        live.titles.push((r.info.task.clone(), title));
        live.requests.push(r);
    }
    live.version += 1;
}

fn demo_request(a: &Agent) -> sphere_proto::RequestView {
    use sphere_proto::{RequestInfo, RequestKind, RequestStatus, RequestView};
    let (kind, tool, summary, input) = if a.id == "redis" {
        (
            RequestKind::Question,
            "AskUserQuestion",
            "Switching stores will log everyone out unless I migrate the 4,812 live sessions. Migrate them?".to_string(),
            serde_json::json!({"questions": [{"question": "Migrate the 4,812 live sessions?", "options": [{"label": "Migrate them, keep the old store for a week"}, {"label": "Skip it: everyone logs in again"}]}]}),
        )
    } else {
        (
            RequestKind::Permission,
            "Bash",
            "I want to run the end-to-end checkout tests twenty times and start the dev server on :3000.".to_string(),
            serde_json::json!({"command": "pnpm test:e2e checkout --repeat-each 20", "description": "Run the e2e checkout tests"}),
        )
    };
    let info = RequestInfo {
        id: format!("demo-{}", a.id),
        task: a.id.clone(),
        kind,
        tool: tool.into(),
        display_name: None,
        description: Some(a.step.clone()),
        summary,
        input,
        suggestions: vec![],
        blocked_path: None,
        tool_use_id: None,
        agent_request: None,
        revises: None,
    };
    RequestView { info, status: RequestStatus::Open, opened_at: rfc3339_now() }
}

/// The current time as RFC 3339 UTC (for a demo request's "waiting" age).
fn rfc3339_now() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // civil from days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// Keys, the focus buttons, the bar's words, the events and the card, in one pass.
#[allow(clippy::too_many_arguments)]
fn drive(mut ev: MessageReader<KeyboardInput>, keys: Res<ButtonInput<KeyCode>>, mut e: Env, mut st: ResMut<Input>, events: Res<crate::swarm::model::Events>, mut hud_keys: ResMut<HudKeys>) {
    // merge outcomes come back from their threads whether or not the swarm is still open
    let notes: Vec<Note> = st.rx.lock().unwrap().try_iter().collect();
    for (id, ok, text) in notes {
        st.merging.remove(&id);
        if ok {
            // the swarm's Merged event toasts "Merged into …" when the satellite reaches the core; a merge done
            // while the swarm is closed still says so here
            if !e.swarm.open {
                e.toasts.push_ok(text);
            }
        } else {
            e.toasts.push_bad(text);
            e.sound.play_with("error", 0.6, 0.0);
        }
    }
    // always read the key events (so a closed swarm does not replay them when it opens)
    let mods = keys.any_pressed([KeyCode::SuperLeft, KeyCode::SuperRight, KeyCode::ControlLeft, KeyCode::ControlRight, KeyCode::AltLeft, KeyCode::AltRight]);
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let pressed: Vec<KeyCode> = ev.read().filter(|k| k.state == ButtonState::Pressed && !k.repeat).map(|k| k.key_code).collect();
    let buttons: Vec<KeyCode> = std::mem::take(&mut hud_keys.0);
    // the events seen so far (a drain by the feedback worker shows as a shorter list, or a different one)
    let all = &events.0;
    if all.len() < st.seen || (st.seen > 0 && all.get(st.seen - 1) != st.last.as_ref()) {
        st.seen = 0;
    }
    let fresh: Vec<Event> = all[st.seen.min(all.len())..].to_vec();
    st.seen = all.len();
    st.last = all.last().cloned();
    if !e.swarm.open {
        st.prev = Mode::Overview;
        st.pending.clear();
        st.arm = None;
        return;
    }
    let now = e.sim.t;
    // the agent in focus was merged, stopped or is gone: back to the overview quietly, so its collapse shows
    if let Mode::Focus(id) = &e.swarm.mode {
        if e.swarm.model.agent(id).is_none_or(|a| matches!(a.state, State::Merged | State::Stopped)) {
            e.swarm.mode = Mode::Overview;
            e.swarm.version += 1;
        }
    }
    // a request arrived: from the overview, go to it (otherwise only the toast the feedback worker makes)
    for ev in fresh {
        if let Event::NeedsYou(id) = ev {
            st.pending.push((id, now));
        }
    }
    let typing = e.focus.cmd || e.focus.terminal || e.moment.editing;
    if !typing && !mods {
        for code in pressed {
            if let Some(k) = key_of(code, shift) {
                // with the card up, Tab and N still move between requests; the rest is the card's
                if e.moment.shown && !matches!(k, Key::Cycle(_) | Key::Next) {
                    continue;
                }
                press(&mut e, &mut st, k);
            }
        }
    }
    for code in buttons {
        if let Some(k) = key_of(code, false).filter(|k| matches!(k, Key::Open | Key::Ask | Key::Stop | Key::Enter)) {
            press(&mut e, &mut st, k);
        }
    }
    let cmds = std::mem::take(&mut e.open.swarm_cmds);
    for c in cmds {
        command(&mut e, &mut st, c);
    }
    // pending requests: go to them while the overview shows and the card is free
    let mut pending = std::mem::take(&mut st.pending);
    pending.retain(|(id, t0)| {
        if e.swarm.mode != Mode::Overview || now - t0 > 3.0 {
            return false;
        }
        if e.moment.busy() {
            return true;
        }
        if crate::moment::waiting(&e.live, &e.moment).iter().any(|r| &r.info.task == id) {
            go(&mut e, Mode::Request(id.clone()));
            return false;
        }
        true
    });
    st.pending = pending;
    sync(&mut e, &mut st);
}

/// Keeps `Swarm.mode` and the Agent moment card in step. A request mode raises the card on that agent's request; a
/// card that is up (it arrived, or N / the arrows moved it) sets the mode to its request; a card that went away
/// (put back, answered, everything done) returns to the overview.
fn sync(e: &mut Env, st: &mut Input) {
    let cur = e.swarm.mode.clone();
    if cur != st.prev {
        match &cur {
            Mode::Request(id) => {
                if e.moment.current_task(&e.live).as_deref() != Some(id.as_str()) {
                    let Env { moment, live, sound, .. } = e;
                    if !moment.show_task(live, id, sound) {
                        // its request is not waiting (snoozed, or the card is busy): show the agent instead
                        e.swarm.mode = Mode::Focus(id.clone());
                        e.swarm.version += 1;
                    }
                }
            }
            Mode::Focus(_) if matches!(st.prev, Mode::Request(_)) => {
                // a strip dot or the like took us out of a request
                let Env { moment, live, sound, .. } = e;
                moment.put_back(live, sound);
            }
            _ => {}
        }
    }
    if e.moment.all_done.is_some() {
        if matches!(e.swarm.mode, Mode::Request(_)) {
            e.swarm.mode = Mode::Overview;
            e.swarm.version += 1;
        }
    } else if !e.moment.busy() {
        let card = e.moment.current_task(&e.live);
        let want = match (card, &e.swarm.mode) {
            (Some(t), m) if *m != Mode::Request(t.clone()) => Some(Mode::Request(t)),
            (None, Mode::Request(_)) => Some(Mode::Overview),
            _ => None,
        };
        if let Some(m) = want {
            e.swarm.mode = m;
            e.swarm.version += 1;
        }
    }
    st.prev = e.swarm.mode.clone();
}

/// Hover (the label or satellite under the pointer) and clicks on them.
#[allow(clippy::too_many_arguments)]
fn pointer(
    mut e: Env,
    mut st: ResMut<Input>,
    buttons: Res<ButtonInput<MouseButton>>,
    time: Res<Time<Real>>,
    ms: Res<MouseState>,
    scene: Res<SceneOut>,
    hud: Res<HudOut>,
    cams: Query<(&Camera, &GlobalTransform), With<CamCore>>,
) {
    let set_hover = |e: &mut Env, st: &mut Input, h: Option<String>| {
        if e.swarm.hover != h {
            if h.is_some() && h != st.hover {
                e.sound.play_with("hover", 0.45, 0.0);
            }
            e.swarm.hover = h.clone();
        }
        st.hover = h;
    };
    if !e.swarm.open {
        st.down = None;
        set_hover(&mut e, &mut st, None);
        return;
    }
    let (Some(pos), Ok((cam, ctr))) = (ms.pos, cams.single()) else {
        set_hover(&mut e, &mut st, None);
        return;
    };
    let blocked = ms.dragging || matches!(ms.hit, Some(Hit::Visor) | Some(Hit::Panel(..)));
    let hover = if blocked {
        None
    } else {
        let proj = |p: Vec3| cam.world_to_viewport(ctr, p).ok();
        let circles = node_circles(&scene.nodes, &e.swarm.model, &proj, *ctr.right());
        pick_target(pos, &hud.labels, &circles)
    };
    set_hover(&mut e, &mut st, hover.clone());
    // a click: released within 450 ms and under 4 px of where it went down
    let now = time.elapsed_secs();
    if buttons.just_pressed(MouseButton::Left) && ms.hit != Some(Hit::Visor) {
        st.down = Some((pos, now));
    }
    if buttons.just_released(MouseButton::Left) {
        let Some((p0, t0)) = st.down.take() else { return };
        if (pos - p0).length() >= 4.0 || now - t0 >= 0.45 || ms.dragging || e.focus.cmd {
            return;
        }
        match hover {
            Some(id) => {
                if let Some(mode) = mode_for(&e.swarm.model, &id) {
                    go(&mut e, mode);
                }
            }
            None if matches!(e.swarm.mode, Mode::Focus(_)) && ms.hit == Some(Hit::None) => go(&mut e, Mode::Overview),
            None => {}
        }
    }
}

// ------------------------------------------------------------------ tests

#[cfg(test)]
mod tests {
    use super::*;

    fn ag(id: &str, state: State) -> Agent {
        Agent { id: id.into(), title: id.into(), branch: format!("agent/{id}"), state, ..default() }
    }

    fn model(list: &[(&str, State)]) -> Model {
        Model { agents: list.iter().map(|(i, s)| ag(i, *s)).collect(), ..default() }
    }

    #[test]
    fn labels_win_over_satellite_circles_and_the_nearest_circle_wins() {
        let labels = vec![("a".to_string(), Rect::from_corners(Vec2::new(100.0, 100.0), Vec2::new(200.0, 140.0)))];
        let circles = vec![Circle { id: "b".into(), c: Vec2::new(150.0, 120.0), r: 30.0 }, Circle { id: "c".into(), c: Vec2::new(300.0, 300.0), r: 40.0 }, Circle { id: "d".into(), c: Vec2::new(320.0, 300.0), r: 40.0 }];
        // inside the label and inside b's circle: the label
        assert_eq!(pick_target(Vec2::new(150.0, 120.0), &labels, &circles).as_deref(), Some("a"));
        // outside the label, inside only c
        assert_eq!(pick_target(Vec2::new(270.0, 300.0), &labels, &circles).as_deref(), Some("c"));
        // inside both c and d: d is nearer
        assert_eq!(pick_target(Vec2::new(312.0, 300.0), &labels, &circles).as_deref(), Some("d"));
        // nowhere
        assert_eq!(pick_target(Vec2::new(10.0, 10.0), &labels, &circles), None);
        // on the circle's edge counts
        assert_eq!(pick_target(Vec2::new(340.0, 300.0), &[], &circles[1..2]).as_deref(), Some("c"));
    }

    #[test]
    fn circles_skip_faded_merged_and_off_screen_nodes_and_scale_with_depth() {
        let m = model(&[("a", State::Working), ("b", State::Merged), ("c", State::Working), ("d", State::Working)]);
        let nodes = vec![
            ("a".to_string(), Vec3::new(0.0, 0.0, 0.0), 1.0, 1.0, 1.0),
            ("b".to_string(), Vec3::new(100.0, 0.0, 0.0), 1.0, 1.0, 1.0),
            ("c".to_string(), Vec3::new(200.0, 0.0, 0.0), 1.0, 0.2, 1.0),
            ("d".to_string(), Vec3::new(300.0, 0.0, 0.0), 2.0, 1.0, 1.0),
        ];
        // 2 px per world unit
        let proj = |p: Vec3| Some(Vec2::new(p.x, p.y) * 2.0);
        let c = node_circles(&nodes, &m, &proj, Vec3::X);
        assert_eq!(c.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), vec!["a", "d"]);
        assert!((c[0].r - (27.0 * 2.0 + 12.0)).abs() < 1e-3);
        assert!((c[1].r - (27.0 * 2.0 * 2.0 + 12.0)).abs() < 1e-3);
        assert!(node_circles(&nodes, &m, &|_| None, Vec3::X).is_empty());
    }

    #[test]
    fn selecting_follows_the_state() {
        let m = model(&[("w", State::Working), ("n", State::Needs), ("d", State::Done), ("q", State::Queued), ("g", State::Merged)]);
        assert_eq!(mode_for(&m, "w"), Some(Mode::Focus("w".into())));
        assert_eq!(mode_for(&m, "n"), Some(Mode::Request("n".into())));
        assert_eq!(mode_for(&m, "d"), Some(Mode::Focus("d".into())));
        assert_eq!(mode_for(&m, "q"), Some(Mode::Focus("q".into())));
        assert_eq!(mode_for(&m, "g"), None);
        assert_eq!(mode_for(&m, "nope"), None);
    }

    #[test]
    fn digits_jump_to_the_nth_agent() {
        let m = model(&[("w", State::Working), ("n", State::Needs), ("g", State::Merged)]);
        assert_eq!(jump(&m, 1), Nav::Go(Mode::Focus("w".into())));
        assert_eq!(jump(&m, 2), Nav::Go(Mode::Request("n".into())));
        assert_eq!(jump(&m, 3), Nav::Nothing);
        assert_eq!(jump(&m, 4), Nav::Toast("No agent 4".into()));
        assert_eq!(jump(&m, 0), Nav::Toast("No agent 0".into()));
    }

    #[test]
    fn tab_cycles_and_wraps_both_ways() {
        let m = model(&[("a", State::Working), ("b", State::Done), ("c", State::Working)]);
        let f = |s: &str| Nav::Go(Mode::Focus(s.into()));
        assert_eq!(cycle(&m, None, 1), f("a"));
        assert_eq!(cycle(&m, None, -1), f("c"));
        assert_eq!(cycle(&m, Some("a"), 1), f("b"));
        assert_eq!(cycle(&m, Some("c"), 1), f("a"));
        assert_eq!(cycle(&m, Some("a"), -1), f("c"));
        assert_eq!(cycle(&Model::default(), None, 1), Nav::Nothing);
        // merged agents are skipped
        let m = model(&[("a", State::Working), ("g", State::Merged), ("c", State::Working)]);
        assert_eq!(cycle(&m, Some("a"), 1), f("c"));
    }

    #[test]
    fn n_goes_to_the_next_agent_that_needs_you() {
        let m = model(&[("a", State::Needs), ("w", State::Working), ("b", State::Needs)]);
        let r = |s: &str| Nav::Go(Mode::Request(s.into()));
        assert_eq!(next_needing(&m, None), r("a"));
        assert_eq!(next_needing(&m, Some("a")), r("b"));
        assert_eq!(next_needing(&m, Some("b")), r("a"));
        // from an agent that does not need you: the first
        assert_eq!(next_needing(&m, Some("w")), r("a"));
        assert_eq!(next_needing(&model(&[("w", State::Working)]), None), Nav::Toast("Nothing needs you right now".into()));
        let one = model(&[("a", State::Needs), ("w", State::Working)]);
        assert_eq!(next_needing(&one, Some("a")), Nav::Toast("That is the only one waiting".into()));
        assert_eq!(next_needing(&one, None), r("a"));
    }

    #[test]
    fn merge_targets_the_focused_done_agent_else_the_first_done() {
        let m = model(&[("w", State::Working), ("d1", State::Done), ("d2", State::Done)]);
        assert_eq!(merge_target(&m, Some("d2")).as_deref(), Some("d2"));
        assert_eq!(merge_target(&m, Some("w")).as_deref(), Some("d1"));
        assert_eq!(merge_target(&m, None).as_deref(), Some("d1"));
        assert_eq!(merge_target(&model(&[("w", State::Working)]), None), None);
    }

    #[test]
    fn merge_needs_two_presses_within_three_seconds() {
        let mut arm = None;
        assert_eq!(merge_press(&mut arm, "a", 10.0), Press::Armed);
        assert_eq!(merge_press(&mut arm, "a", 11.5), Press::Merge);
        assert!(arm.is_none());
        // armed, then too late: arms again
        assert_eq!(merge_press(&mut arm, "a", 20.0), Press::Armed);
        assert_eq!(merge_press(&mut arm, "a", 23.5), Press::Armed);
        assert_eq!(merge_press(&mut arm, "a", 24.0), Press::Merge);
        // another agent re-arms instead of merging
        assert_eq!(merge_press(&mut arm, "a", 30.0), Press::Armed);
        assert_eq!(merge_press(&mut arm, "b", 30.5), Press::Armed);
        assert_eq!(merge_press(&mut arm, "b", 31.0), Press::Merge);
    }

    #[test]
    fn the_bars_swarm_words() {
        for w in ["next", "needs", "need", "what needs me", "what needs you", "waiting", "inbox", "approvals", "requests"] {
            assert_eq!(parse(w), Some(SwarmCmd::Next), "{w}");
        }
        assert_eq!(parse("merge"), Some(SwarmCmd::Merge));
        assert_eq!(parse("merge it"), Some(SwarmCmd::Merge));
        assert_eq!(parse("stop"), Some(SwarmCmd::Stop));
        assert_eq!(parse("kill"), Some(SwarmCmd::Stop));
        for w in ["back", "home", "close", "exit"] {
            assert_eq!(parse(w), Some(SwarmCmd::Back), "{w}");
        }
        for w in ["status", "how", "report", "how is the swarm doing", "status report"] {
            assert_eq!(parse(w), Some(SwarmCmd::Status), "{w}");
        }
        assert_eq!(parse("3"), Some(SwarmCmd::Jump(3)));
        assert_eq!(parse("open 3"), Some(SwarmCmd::Jump(3)));
        assert_eq!(parse("go to 9"), Some(SwarmCmd::Jump(9)));
        assert_eq!(parse("open 10"), None);
        assert_eq!(parse("open files"), None);
        assert_eq!(parse("low"), None);
    }

    #[test]
    fn keys_map_to_actions() {
        assert_eq!(key_of(KeyCode::Digit4, false), Some(Key::Jump(4)));
        assert_eq!(key_of(KeyCode::Digit0, false), None);
        assert_eq!(key_of(KeyCode::Tab, true), Some(Key::Cycle(-1)));
        assert_eq!(key_of(KeyCode::Tab, false), Some(Key::Cycle(1)));
        assert_eq!(key_of(KeyCode::KeyN, false), Some(Key::Next));
        assert_eq!(key_of(KeyCode::KeyS, false), None);
    }

    #[test]
    fn the_time_is_formatted_as_rfc3339() {
        let t = rfc3339_now();
        assert!(crate::working::parse_rfc3339(&t).is_some(), "{t}");
    }

    #[test]
    fn the_card_rises_from_the_satellite_only_while_the_swarm_is_open() {
        let mut swarm = Swarm::default();
        let (moment, live) = (Moment::default(), Live::default());
        let scene = SceneOut::default();
        assert_eq!(rise_origin(&swarm, &moment, &live, &scene), None);
        swarm.open = true;
        assert_eq!(rise_origin(&swarm, &moment, &live, &scene), None);
    }

    // ---- the systems, on a bare App

    fn args() -> crate::Args {
        crate::Args { cmd: None, bench: None, shot: None, record: None, record_frames: 180, frames: 120, depth: false, orbit: false, apps: false, work: false, swarm: false, code: false, git: false, open_app: None, icon: false, aa: "smaa".into(), offscreen: false, collapsed: false, focus_term: false, msaa: true, size: (1600, 1000), dt: 0.05, demo: true, live: false, moment: false, elsewhere: false, calendar: false, takeover: None, protocols: None, theme: None, motion: None }
    }

    fn request(task: &str) -> sphere_proto::RequestView {
        use sphere_proto::*;
        let info = RequestInfo { id: format!("r-{task}"), task: task.into(), kind: RequestKind::Permission, tool: "Bash".into(), display_name: None, description: None, summary: "run it".into(), input: serde_json::json!({"command": "ls"}), suggestions: vec![], blocked_path: None, tool_use_id: None, agent_request: None, revises: None };
        RequestView { info, status: RequestStatus::Open, opened_at: rfc3339_now() }
    }

    #[allow(clippy::field_reassign_with_default)]
    fn world() -> App {
        let mut app = App::new();
        let mut live = Live::default();
        live.requests = vec![request("a"), request("c")];
        let mut swarm = Swarm::default();
        swarm.open = true;
        swarm.model = model(&[("a", State::Needs), ("b", State::Working), ("c", State::Needs), ("d", State::Done)]);
        app.add_message::<KeyboardInput>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<Focus>()
            .init_resource::<CmdBar>()
            .init_resource::<Toasts>()
            .init_resource::<Sound>()
            .init_resource::<crate::homepanels::Say>()
            .init_resource::<Open>()
            .init_resource::<Moment>()
            .init_resource::<HudKeys>()
            .init_resource::<crate::swarm::model::Events>()
            .init_resource::<Input>()
            .insert_resource(live)
            .insert_resource(swarm)
            .insert_resource(crate::service::start(true))
            .insert_resource(Sim { dt: 0.05, t: 10.0, boot_t: 99.0, fixed_dt: Some(0.05), frozen: false })
            .insert_resource(args())
            .add_systems(Update, drive);
        app
    }

    fn tap(app: &mut App, code: KeyCode) {
        use bevy::input::keyboard::{Key, NativeKey};
        app.world_mut().write_message(KeyboardInput { key_code: code, logical_key: Key::Unidentified(NativeKey::Unidentified), state: ButtonState::Pressed, text: None, repeat: false, window: Entity::PLACEHOLDER });
        app.update();
    }

    #[test]
    fn n_raises_the_card_on_the_next_request_and_the_mode_follows_it() {
        let mut app = world();
        tap(&mut app, KeyCode::KeyN);
        assert_eq!(app.world().resource::<Swarm>().mode, Mode::Request("a".into()));
        let m = app.world().resource::<Moment>();
        assert!(m.shown && m.index == 0 && m.rise_seq == 1);
        // N again: the other request (a swap), the mode follows
        tap(&mut app, KeyCode::KeyN);
        assert_eq!(app.world().resource::<Swarm>().mode, Mode::Request("c".into()));
        assert_eq!(app.world().resource::<Moment>().index, 1);
        // the arrows on the card move it; the mode follows
        app.world_mut().resource_mut::<Moment>().index = 0;
        app.update();
        assert_eq!(app.world().resource::<Swarm>().mode, Mode::Request("a".into()));
        // the card put away: back to the overview
        app.world_mut().resource_mut::<Moment>().shown = false;
        app.update();
        assert_eq!(app.world().resource::<Swarm>().mode, Mode::Overview);
    }

    #[test]
    fn digits_focus_and_putting_the_card_back_when_leaving_a_request() {
        let mut app = world();
        tap(&mut app, KeyCode::Digit1);
        assert_eq!(app.world().resource::<Swarm>().mode, Mode::Request("a".into()));
        assert!(app.world().resource::<Moment>().shown);
        // the card owns the digits
        tap(&mut app, KeyCode::Digit2);
        assert_eq!(app.world().resource::<Swarm>().mode, Mode::Request("a".into()));
        // a strip dot (mode set from outside) leaves the request: the card goes away
        app.world_mut().resource_mut::<Swarm>().mode = Mode::Focus("b".into());
        app.update();
        assert_eq!(app.world().resource::<Swarm>().mode, Mode::Focus("b".into()));
        assert!(!app.world().resource::<Moment>().shown);
        tap(&mut app, KeyCode::Digit9);
        assert!(app.world().resource::<Toasts>().list.iter().any(|t| t.0 == "No agent 9"));
        // Tab goes on from the open agent
        tap(&mut app, KeyCode::Tab);
        assert_eq!(app.world().resource::<Swarm>().mode, Mode::Request("c".into()));
    }

    #[test]
    fn focus_lets_go_of_an_agent_that_was_merged_or_is_gone() {
        let mut app = world();
        app.world_mut().resource_mut::<Swarm>().mode = Mode::Focus("b".into());
        app.update();
        assert_eq!(app.world().resource::<Swarm>().mode, Mode::Focus("b".into()));
        for a in app.world_mut().resource_mut::<Swarm>().model.agents.iter_mut().filter(|a| a.id == "b") {
            a.state = State::Merged;
        }
        app.update();
        assert_eq!(app.world().resource::<Swarm>().mode, Mode::Overview);
        app.world_mut().resource_mut::<Swarm>().mode = Mode::Focus("nobody".into());
        app.update();
        assert_eq!(app.world().resource::<Swarm>().mode, Mode::Overview);
    }

    #[test]
    fn focus_keys_open_in_code_ask_and_stop() {
        let mut app = world();
        app.world_mut().resource_mut::<Swarm>().mode = Mode::Focus("b".into());
        tap(&mut app, KeyCode::KeyA);
        assert_eq!(app.world().resource::<CmdBar>().target, Some(crate::cmdbar::BarTarget::Task("b".into(), "b".into())));
        assert!(app.world().resource::<Focus>().cmd);
        app.world_mut().resource_mut::<Focus>().cmd = false;
        // Enter on a working agent opens it in Code
        tap(&mut app, KeyCode::Enter);
        assert_eq!(app.world().resource::<Open>().code_task.as_deref(), Some("b"));
        // the focus panel's Stop button
        app.world_mut().resource_mut::<HudKeys>().0.push(KeyCode::KeyX);
        app.update();
        assert_eq!(app.world().resource::<Swarm>().mode, Mode::Overview);
        assert!(app.world().resource::<Toasts>().list.iter().any(|t| t.0.starts_with("Stopped · b")));
    }

    #[test]
    fn merge_outcomes_toast_green_or_red() {
        use crate::live::ToastKind;
        let mut app = world();
        let tx = app.world().resource::<Input>().tx.clone();
        tx.send(("d".to_string(), true, "Merged agent/d".to_string())).unwrap();
        tx.send(("d".to_string(), false, "Merge failed: conflict".to_string())).unwrap();
        app.update();
        let kinds: Vec<(String, ToastKind)> = app.world().resource::<Toasts>().list.iter().map(|t| (t.0.clone(), t.2)).collect();
        // with the swarm open the Merged event toasts when the satellite lands; a failure is red at once
        assert!(!kinds.iter().any(|k| k.0 == "Merged agent/d"), "{kinds:?}");
        assert!(kinds.contains(&("Merge failed: conflict".to_string(), ToastKind::Bad)), "{kinds:?}");
        // a merge that finishes after the swarm closed toasts green itself
        app.world_mut().resource_mut::<Swarm>().open = false;
        tx.send(("d".to_string(), true, "Merged agent/d".to_string())).unwrap();
        app.update();
        assert!(app.world().resource::<Toasts>().list.iter().any(|t| t.0 == "Merged agent/d" && t.2 == ToastKind::Ok));
        app.world_mut().resource_mut::<Swarm>().open = true;
        // Stop toasts plain
        app.world_mut().resource_mut::<Swarm>().mode = Mode::Focus("b".into());
        app.world_mut().resource_mut::<HudKeys>().0.push(KeyCode::KeyX);
        app.update();
        assert!(app.world().resource::<Toasts>().list.iter().any(|t| t.0.starts_with("Stopped") && t.2 == ToastKind::Plain));
    }

    #[test]
    fn the_status_word_says_the_status_line() {
        let mut app = world();
        app.world_mut().resource_mut::<Open>().swarm_cmds.push(SwarmCmd::Status);
        app.update();
        assert_eq!(app.world().resource::<crate::homepanels::Say>().text, "4 agents: 1 working, 2 needs you, 1 ready to merge. About $0.00 today, API estimate.");
        // with 7 min already waited today (and two requests that just opened), it says so
        app.world_mut().resource_mut::<Live>().waited_closed_secs = 7 * 60;
        app.world_mut().resource_mut::<Open>().swarm_cmds.push(SwarmCmd::Status);
        app.update();
        assert_eq!(app.world().resource::<crate::homepanels::Say>().text, "4 agents: 1 working, 2 needs you, 1 ready to merge. About $0.00 today, API estimate. Agents waited 7 min on you today.");
    }

    #[test]
    fn enter_on_a_done_agent_arms_then_merges_and_disarms_after_three_seconds() {
        let mut app = world();
        app.world_mut().resource_mut::<Swarm>().mode = Mode::Focus("d".into());
        tap(&mut app, KeyCode::Enter);
        assert!(app.world().resource::<Toasts>().list.iter().any(|t| t.0 == "Press Enter again to merge agent/d"));
        // too late: arms again rather than merging
        app.world_mut().resource_mut::<Sim>().t = 14.0;
        tap(&mut app, KeyCode::Enter);
        assert_eq!(app.world().resource::<Toasts>().list.iter().filter(|t| t.0.starts_with("Press Enter again")).count(), 2);
        // in time: merges (the demo has no sphered to merge with, and says so)
        app.world_mut().resource_mut::<Sim>().t = 15.0;
        tap(&mut app, KeyCode::Enter);
        assert!(app.world().resource::<Toasts>().list.iter().any(|t| t.0.starts_with("Merging needs sphered")));
    }

    #[test]
    fn the_bars_words_drive_the_same_actions() {
        let mut app = world();
        app.world_mut().resource_mut::<Open>().swarm_cmds.push(SwarmCmd::Merge);
        app.update();
        // merge with nothing focused: the first done agent, brought up and armed
        assert_eq!(app.world().resource::<Swarm>().mode, Mode::Focus("d".into()));
        assert!(app.world().resource::<Toasts>().list.iter().any(|t| t.0.starts_with("Press Enter again")));
        app.world_mut().resource_mut::<Open>().swarm_cmds.push(SwarmCmd::Stop);
        app.update();
        assert_eq!(app.world().resource::<Swarm>().mode, Mode::Overview);
        app.world_mut().resource_mut::<Open>().swarm_cmds.push(SwarmCmd::Stop);
        app.update();
        assert!(app.world().resource::<Toasts>().list.iter().any(|t| t.0 == "Open an agent first, then say stop"));
        app.world_mut().resource_mut::<Open>().swarm_cmds.push(SwarmCmd::Back);
        app.update();
        assert!(!app.world().resource::<Swarm>().open);
    }

    #[test]
    fn a_request_arriving_in_the_overview_opens_it_and_in_a_focus_does_not() {
        let mut app = world();
        app.world_mut().resource_mut::<crate::swarm::model::Events>().0.push(Event::NeedsYou("c".into()));
        app.update();
        assert_eq!(app.world().resource::<Swarm>().mode, Mode::Request("c".into()));
        assert!(app.world().resource::<Moment>().shown);
        // the same event is not taken twice while it stays in the list
        app.world_mut().resource_mut::<Moment>().shown = false;
        app.update();
        assert_eq!(app.world().resource::<Swarm>().mode, Mode::Overview);
        app.update();
        assert_eq!(app.world().resource::<Swarm>().mode, Mode::Overview);
        // while an agent is in focus, a request waits for N
        app.world_mut().resource_mut::<crate::swarm::model::Events>().0.clear();
        app.world_mut().resource_mut::<Swarm>().mode = Mode::Focus("b".into());
        app.world_mut().resource_mut::<crate::swarm::model::Events>().0.push(Event::NeedsYou("a".into()));
        app.update();
        assert_eq!(app.world().resource::<Swarm>().mode, Mode::Focus("b".into()));
    }
}
