#![allow(deprecated)] // Interaction: Bevy 0.20 deprecates it in favour of ui_widgets, which this crate does not enable yet
//! Take over a running agent. Claude sessions that run outside Sphere (in herdr, tmux, a terminal tab, over SSH, in
//! the Claude background) are polled from sphered every 5 s while the window is visible; the Agents panel lists them
//! as dim "elsewhere" rows and the Agent swarm draws them as ghost satellites. A row, a ghost, or the command bar
//! (`take over`, `elsewhere`) opens a glass card with one action: Take over. That stops the session where it runs, so
//! it takes two presses (the first arms the button, the second does it); Esc cancels and disarms.
use crate::fonts::UiFonts;
use crate::live::Toasts;
use crate::panel::{font, with_alpha, Focus};
use crate::service::{Cmd, Service};
use crate::util::*;
use bevy::input::keyboard::KeyboardInput;
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::text::LineHeight;
use sphere_proto::api::{OutsideSession, TakeOverResult};

// ---------------------------------------------------------------- pure: words

/// Where a session runs, as a phrase: "in herdr", "in Terminal", "over SSH", "in Claude background".
pub fn where_label(w: &str) -> String {
    let w = w.trim();
    if w.is_empty() {
        "elsewhere".to_string()
    } else if w.starts_with("over ") {
        w.to_string()
    } else {
        format!("in {w}")
    }
}

/// Where it ran, after "from": "herdr", "a terminal", "SSH".
pub fn from_label(w: &str) -> String {
    let w = w.trim();
    w.strip_prefix("over ").unwrap_or(w).to_string()
}

/// "working", "waiting on you", "idle".
pub fn state_label(state: &str) -> &'static str {
    match state {
        "working" => "working",
        "waiting" => "waiting on you",
        "idle" => "idle",
        _ => "running",
    }
}

/// Waiting sessions come first, then working, idle, unknown.
fn rank(state: &str) -> u8 {
    match state {
        "waiting" => 0,
        "working" => 1,
        "idle" => 2,
        _ => 3,
    }
}

/// The folder a session works in (the last part of its cwd).
pub fn repo_name(cwd: Option<&str>) -> String {
    cwd.and_then(|c| std::path::Path::new(c.trim_end_matches('/')).file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
}

/// Its title, or its folder when it has none.
pub fn title_of(s: &OutsideSession) -> String {
    s.title.as_deref().map(str::trim).filter(|t| !t.is_empty()).map(str::to_string).unwrap_or_else(|| {
        let r = repo_name(s.cwd.as_deref());
        if r.is_empty() {
            "Claude session".to_string()
        } else {
            format!("Session in {r}")
        }
    })
}

/// One session's identity across polls: its session id, else its pid, else folder and title.
pub fn key_of(s: &OutsideSession) -> String {
    match (&s.session_id, s.pid) {
        (Some(id), _) => id.clone(),
        (None, Some(p)) => format!("pid:{p}"),
        _ => format!("{}|{}", s.cwd.clone().unwrap_or_default(), s.title.clone().unwrap_or_default()),
    }
}

/// How many sessions share this one's folder (itself included).
pub fn in_folder(list: &[OutsideSession], s: &OutsideSession) -> usize {
    match &s.cwd {
        Some(c) => list.iter().filter(|o| o.cwd.as_deref() == Some(c.as_str())).count().max(1),
        None => 1,
    }
}

/// What the card says about a session that cannot be told apart from another one.
pub fn ambiguous_note(n: usize) -> String {
    format!("{} sessions in this folder; can't tell them apart", n.max(2))
}

/// Whether it can be taken over: it has a session id and is not mistaken for another.
pub fn takeable(s: &OutsideSession) -> bool {
    s.session_id.is_some() && !s.ambiguous
}

/// The last message, cut to fit the card.
pub fn excerpt(msg: &str, max: usize) -> String {
    let one: String = msg.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() > max {
        one.chars().take(max.saturating_sub(1)).collect::<String>().trim_end().to_string() + "…"
    } else {
        one
    }
}

/// Which repo's orbit a session sits on: the repo whose path holds its cwd (the deepest one when they nest).
pub fn match_repo<'a>(repo_paths: impl IntoIterator<Item = &'a str>, cwd: &str) -> Option<usize> {
    let cwd = cwd.trim_end_matches('/');
    repo_paths
        .into_iter()
        .enumerate()
        .filter_map(|(i, p)| {
            let p = p.trim_end_matches('/');
            (!p.is_empty() && (cwd == p || cwd.starts_with(&format!("{p}/")))).then_some((p.len(), i))
        })
        .max_by_key(|(len, _)| *len)
        .map(|(_, i)| i)
}

// ---------------------------------------------------------------- pure: the two-press button

/// The Take over button: the first press arms it, the second does it, and while the call runs it only waits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Arm {
    #[default]
    Idle,
    Armed,
    Pending,
}

/// A second press this soon after the first is the same click twice, not a decision.
pub const ARM_DEBOUNCE: f32 = 0.35;

impl Arm {
    /// A press at time `now` (seconds); `armed_at` is when it was armed. Returns the new state and whether to do it.
    pub fn press(self, now: f32, armed_at: f32) -> (Arm, bool) {
        match self {
            Arm::Idle => (Arm::Armed, false),
            Arm::Armed if now - armed_at < ARM_DEBOUNCE => (Arm::Armed, false),
            Arm::Armed => (Arm::Pending, true),
            Arm::Pending => (Arm::Pending, false),
        }
    }
    /// Esc, or anything that changes what is being taken over.
    pub fn cancel(self) -> Arm {
        match self {
            Arm::Armed => Arm::Idle,
            other => other,
        }
    }
}

/// The button's words.
pub fn button_label(arm: Arm, where_: &str) -> String {
    match arm {
        Arm::Idle => "Take over".to_string(),
        Arm::Armed => format!("Press again: stops it {}, continues here", where_label(where_)),
        Arm::Pending => "Taking over…".to_string(),
    }
}

/// What the command bar's words do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Words {
    TakeOver,
    List,
}

/// `take over` / `elsewhere` (and a few ways to say them).
pub fn parse_words(l: &str) -> Option<Words> {
    let l = l.trim();
    if matches!(l, "take over" | "takeover" | "take it over" | "take over agent" | "take over a session") {
        Some(Words::TakeOver)
    } else if matches!(l, "elsewhere" | "outside" | "running elsewhere" | "agents elsewhere") {
        Some(Words::List)
    } else {
        None
    }
}

/// What `take over` opens: nothing, the only session, or the list to choose from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Nothing,
    One(usize),
    Choose,
}

pub fn target_for(n: usize) -> Target {
    match n {
        0 => Target::Nothing,
        1 => Target::One(0),
        _ => Target::Choose,
    }
}

// ---------------------------------------------------------------- state

/// Claude sessions running outside Sphere, waiting first.
#[derive(Resource, Default)]
pub struct Outside {
    pub list: Vec<OutsideSession>,
    /// bumps when the list changes (the Agents panel rebuilds on it)
    pub version: u64,
}

impl Outside {
    /// Takes a new list; true when it differs.
    pub fn set(&mut self, mut l: Vec<OutsideSession>) -> bool {
        l.sort_by(|a, b| rank(&a.state).cmp(&rank(&b.state)).then_with(|| b.updated_at.cmp(&a.updated_at)));
        if l == self.list {
            return false;
        }
        self.list = l;
        self.version += 1;
        true
    }

    /// The look test's sessions: one waiting in herdr, one idle in Terminal, two in one folder that can't be told apart.
    pub fn demo() -> Outside {
        let s = |id: Option<&str>, pid: u32, cwd: &str, title: &str, w: &str, state: &str, amb: bool, msg: &str, at: &str| OutsideSession {
            session_id: id.map(str::to_string),
            pid: Some(pid),
            cwd: Some(cwd.to_string()),
            title: Some(title.to_string()),
            where_: w.to_string(),
            state: state.to_string(),
            ambiguous: amb,
            last_message: Some(msg.to_string()),
            updated_at: Some(at.to_string()),
        };
        let mut o = Outside::default();
        o.set(vec![
            s(
                Some("d1c7-demo-herdr"),
                4121,
                "/demo/api-server",
                "Migrate billing webhooks to v2",
                "herdr",
                "waiting",
                false,
                "The handler and its tests are updated and green. Should I also drop the legacy /v1/webhook route, or keep it until the next release?",
                "2026-10-19T09:41:00Z",
            ),
            s(
                Some("e9a2-demo-term"),
                5530,
                "/demo/web-app",
                "Tidy the settings page",
                "Terminal",
                "idle",
                false,
                "Done. The settings page now groups notifications by channel; nothing else changed.",
                "2026-10-19T09:30:00Z",
            ),
            s(None, 6012, "/demo/docs-site", "Fix broken links in the guide", "tmux", "working", true, "Checking the remaining pages for dead anchors.", "2026-10-19T09:44:00Z"),
            s(None, 6188, "/demo/docs-site", "Rewrite the install page", "a terminal", "idle", true, "Draft is in place; waiting for your notes.", "2026-10-19T09:20:00Z"),
        ]);
        o
    }
}

/// What the card shows.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    /// the list to choose from (true), or one session
    pub choose: bool,
    pub key: String,
    pub session: OutsideSession,
    /// sessions sharing its folder
    pub siblings: usize,
    /// it stopped running outside while the card was up
    pub gone: bool,
    pub arm: Arm,
    pub armed_at: f32,
    pub error: Option<String>,
    /// the highlighted row of the list
    pub sel: usize,
}

#[derive(Resource, Default)]
pub struct TakeOver {
    pub card: Option<Card>,
    /// bumps when the card changes (it redraws on it)
    pub version: u64,
    /// the card just closed: give the keyboard back next frame (so the Esc that closed it is not heard twice)
    release: bool,
    /// a take-over asked for and not answered yet: (session key)
    asked: Vec<String>,
    /// the `--takeover` look-test script already ran
    scripted: bool,
}

impl TakeOver {
    pub fn open_one(&mut self, list: &[OutsideSession], i: usize) {
        let Some(s) = list.get(i) else { return };
        self.card = Some(Card { choose: false, key: key_of(s), session: s.clone(), siblings: in_folder(list, s), gone: false, arm: Arm::Idle, armed_at: 0.0, error: None, sel: i });
        self.version += 1;
    }
    pub fn open_list(&mut self, list: &[OutsideSession]) {
        let Some(s) = list.first() else { return };
        self.card = Some(Card { choose: true, key: key_of(s), session: s.clone(), siblings: 1, gone: false, arm: Arm::Idle, armed_at: 0.0, error: None, sel: 0 });
        self.version += 1;
    }
    /// Closes the card (which disarms it).
    pub fn close(&mut self) {
        if self.card.take().is_some() {
            self.release = true;
            self.version += 1;
        }
    }
    /// The Take over button or T / Enter: arms it, then does it. Returns the session id to take over.
    pub fn press(&mut self, now: f32) -> Option<String> {
        let c = self.card.as_mut()?;
        if c.choose || c.gone || !takeable(&c.session) {
            return None;
        }
        let (arm, fire) = c.arm.press(now, c.armed_at);
        if c.arm == Arm::Idle {
            c.armed_at = now;
        }
        c.arm = arm;
        c.error = None;
        self.version += 1;
        if fire {
            let id = c.session.session_id.clone()?;
            self.asked.push(key_of(&c.session));
            return Some(id);
        }
        None
    }
    /// Esc: disarms and closes the card.
    pub fn back(&mut self) {
        if let Some(c) = &mut self.card {
            c.arm = c.arm.cancel();
        }
        self.close();
    }
    /// The polled list changed: keep the card's session up to date, and disarm if it changed state.
    pub fn refresh(&mut self, list: &[OutsideSession]) {
        let Some(c) = &mut self.card else { return };
        if c.choose {
            c.sel = c.sel.min(list.len().saturating_sub(1));
            if list.is_empty() {
                self.close();
            } else {
                self.version += 1;
            }
            return;
        }
        match list.iter().find(|s| key_of(s) == c.key) {
            Some(s) => {
                let n = in_folder(list, s);
                if *s != c.session || n != c.siblings {
                    if s.state != c.session.state {
                        c.arm = c.arm.cancel();
                    }
                    c.session = s.clone();
                    c.siblings = n;
                    c.gone = false;
                    self.version += 1;
                }
            }
            None if c.arm != Arm::Pending && !c.gone => {
                c.gone = true;
                c.arm = Arm::Idle;
                self.version += 1;
            }
            None => {}
        }
    }
    /// The call failed: show why, disarmed.
    pub fn fail(&mut self, key: &str, e: String) {
        self.asked.retain(|k| k != key);
        if let Some(c) = &mut self.card {
            if c.key == key {
                c.arm = Arm::Idle;
                c.error = Some(e);
                self.version += 1;
            }
        }
    }
}

// ---------------------------------------------------------------- visibility, results, commands

/// Tells the link thread whether the window can be seen: hidden, minimised or fully covered stops the polling.
pub fn watch_visibility(mut occluded: MessageReader<bevy::window::WindowOccluded>, svc: Res<Service>, args: Res<crate::Args>) {
    for e in occluded.read() {
        // a bench measures drawing, covered or not
        let hidden = e.occluded && args.bench.is_none();
        let mut s = svc.shared.lock().unwrap();
        if s.hidden != hidden {
            s.hidden = hidden;
            s.version += 1;
        }
    }
}

/// Copies the polled list out of the shared state, and handles take-overs that came back.
#[allow(clippy::too_many_arguments)]
pub fn sync(
    svc: Res<Service>,
    args: Res<crate::Args>,
    mut outside: ResMut<Outside>,
    mut take: ResMut<TakeOver>,
    mut seen: Local<u64>,
    mut toasts: ResMut<Toasts>,
    mut working: ResMut<crate::working::Working>,
    mut bar: ResMut<crate::cmdbar::CmdBar>,
    mut focus: ResMut<Focus>,
    mut sound: ResMut<crate::sound::Sound>,
) {
    if args.offline() {
        return;
    }
    let (list, done) = {
        let mut s = svc.shared.lock().unwrap();
        if s.version == *seen {
            return;
        }
        *seen = s.version;
        (s.outside.clone(), std::mem::take(&mut s.taken))
    };
    if outside.set(list) {
        take.refresh(&outside.list);
    }
    for (id, r) in done {
        let key = outside.list.iter().find(|s| s.session_id.as_deref() == Some(id.as_str())).map(key_of).unwrap_or_else(|| id.clone());
        match r {
            Ok(r) => {
                {
                    let mut s = svc.shared.lock().unwrap();
                    s.outside.retain(|o| o.session_id.as_deref() != Some(id.as_str()));
                }
                finished(&r, &key, &mut outside, &mut take, &mut toasts, &mut working, &mut bar, &mut focus, &mut sound);
            }
            Err(e) => {
                take.fail(&key, e.clone());
                if take.card.is_none() {
                    toasts.push_bad(format!("Could not take over: {e}"));
                }
            }
        }
    }
}

/// A take-over went through: the session is a Sphere agent now. Toast it, watch it work, and when it was idle
/// open the bar to say the next thing to it.
#[allow(clippy::too_many_arguments)]
fn finished(
    r: &TakeOverResult,
    key: &str,
    outside: &mut Outside,
    take: &mut TakeOver,
    toasts: &mut Toasts,
    working: &mut crate::working::Working,
    bar: &mut crate::cmdbar::CmdBar,
    focus: &mut Focus,
    sound: &mut crate::sound::Sound,
) {
    take.asked.retain(|k| k != key);
    take.close();
    let rest: Vec<OutsideSession> = outside.list.iter().filter(|s| key_of(s) != key).cloned().collect();
    outside.set(rest);
    toasts.push_ok(format!("Took over {} from {}", r.title, from_label(&r.from)));
    sound.play_with("whoosh", 0.7, 0.0);
    working.open_task(r.task.clone());
    if r.idle {
        // `> ` talks to the agent heard from last, which is this one
        bar.open = true;
        bar.text = "> ".to_string();
        bar.target = None;
        focus.cmd = true;
        focus.terminal = false;
        bar.version += 1;
        take.release = false;
    }
}

/// Typed `take over` / `elsewhere` (the bar sets `Open::takeover`), and a click on the Agents panel.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn commands(
    mut open: ResMut<crate::apps::Open>,
    outside: Res<Outside>,
    mut take: ResMut<TakeOver>,
    mut toasts: ResMut<Toasts>,
    mut sound: ResMut<crate::sound::Sound>,
    root: Query<(&ComputedNode, &UiGlobalTransform), With<crate::panel::AgentsRoot>>,
    rows: Query<(&ComputedNode, &UiGlobalTransform, &ElsewhereRow)>,
    mut focus: ResMut<Focus>,
    bar: Res<crate::cmdbar::CmdBar>,
) {
    if let Some(w) = open.takeover.take() {
        match (w, target_for(outside.list.len())) {
            (_, Target::Nothing) => toasts.push("Nothing running elsewhere"),
            (_, Target::One(i)) => {
                take.open_one(&outside.list, i);
                sound.play_with("open", 0.7, 0.0);
            }
            (_, Target::Choose) => {
                take.open_list(&outside.list);
                sound.play_with("open", 0.7, 0.0);
            }
        }
    }
    if let Some(at) = open.agents_click.take() {
        let hit = root.single().ok().and_then(|(rn, rt)| {
            let size = rn.size();
            let p = rt.translation - size / 2.0 + at * (size.x / PANEL_W);
            rows.iter().find(|(n, t, _)| (p - t.translation).abs().cmple(n.size() / 2.0).all()).map(|(_, _, r)| r.0)
        });
        match hit {
            Some(i) if i < outside.list.len() => {
                take.open_one(&outside.list, i);
                sound.play_with("open", 0.7, 0.0);
            }
            _ => open.app(1, &mut toasts),
        }
    }
    if take.card.is_some() {
        // the card has the keyboard: the home, the swarm and the apps stand by (they skip keys while the bar has focus)
        focus.cmd = true;
        focus.terminal = false;
    } else if std::mem::take(&mut take.release) {
        focus.cmd = bar.open;
    }
}

/// The Agents panel's width in its own px (the panel is built at this size).
pub const PANEL_W: f32 = 356.0;

/// An "elsewhere" row of the Agents panel: its index in `Outside::list`.
#[derive(Component)]
pub struct ElsewhereRow(pub usize);

// ---------------------------------------------------------------- keys and buttons

#[derive(Component, Clone, Copy, PartialEq, Debug)]
pub enum Btn {
    /// Take over
    Act,
    /// a row of the list
    Row(usize),
    /// anywhere else on the screen
    Away,
}

/// T / Enter take over (twice), Esc cancels; in the list the arrows and 1-9 choose and Enter opens.
#[allow(clippy::too_many_arguments)]
pub fn keys(
    mut ev: MessageReader<KeyboardInput>,
    mut take: ResMut<TakeOver>,
    outside: Res<Outside>,
    bar: Res<crate::cmdbar::CmdBar>,
    svc: Res<Service>,
    sim: Res<crate::core::Sim>,
    mut sound: ResMut<crate::sound::Sound>,
    mut toasts: ResMut<Toasts>,
) {
    for e in ev.read() {
        if e.state != ButtonState::Pressed || e.repeat || bar.open || take.card.is_none() {
            continue;
        }
        let choose = take.card.as_ref().is_some_and(|c| c.choose);
        match e.key_code {
            KeyCode::Escape => {
                take.back();
                sound.play_with("close", 0.6, 0.0);
            }
            KeyCode::KeyT | KeyCode::Enter | KeyCode::NumpadEnter if !choose => act(&mut take, &svc, sim.t, &mut sound, &mut toasts),
            KeyCode::Enter | KeyCode::NumpadEnter if choose => {
                let i = take.card.as_ref().map_or(0, |c| c.sel);
                take.open_one(&outside.list, i);
                sound.play("tick");
            }
            KeyCode::ArrowDown | KeyCode::ArrowUp if choose => {
                let n = outside.list.len().max(1);
                if let Some(c) = &mut take.card {
                    c.sel = if e.key_code == KeyCode::ArrowDown { (c.sel + 1) % n } else { (c.sel + n - 1) % n };
                }
                take.version += 1;
                sound.play("tick");
            }
            k if choose => {
                let digit = match k {
                    KeyCode::Digit1 | KeyCode::Numpad1 => Some(0),
                    KeyCode::Digit2 | KeyCode::Numpad2 => Some(1),
                    KeyCode::Digit3 | KeyCode::Numpad3 => Some(2),
                    KeyCode::Digit4 | KeyCode::Numpad4 => Some(3),
                    KeyCode::Digit5 | KeyCode::Numpad5 => Some(4),
                    KeyCode::Digit6 | KeyCode::Numpad6 => Some(5),
                    KeyCode::Digit7 | KeyCode::Numpad7 => Some(6),
                    KeyCode::Digit8 | KeyCode::Numpad8 => Some(7),
                    KeyCode::Digit9 | KeyCode::Numpad9 => Some(8),
                    _ => None,
                };
                if let Some(i) = digit.filter(|i| *i < outside.list.len()) {
                    take.open_one(&outside.list, i);
                    sound.play("tick");
                }
            }
            _ => {}
        }
    }
}

/// One press of the action (a key or the button): arms, then takes over.
fn act(take: &mut TakeOver, svc: &Service, now: f32, sound: &mut crate::sound::Sound, toasts: &mut Toasts) {
    let was = take.card.as_ref().map(|c| c.arm);
    match take.press(now) {
        Some(id) => {
            svc.send(Cmd::TakeOver { session_id: id });
            sound.play_with("confirm", 0.7, 0.0);
        }
        None => {
            if was == Some(Arm::Idle) && take.card.as_ref().is_some_and(|c| c.arm == Arm::Armed) {
                sound.play_with("toggle", 0.7, 0.0);
            } else if take.card.as_ref().is_some_and(|c| !c.choose && !takeable(&c.session)) {
                toasts.push_quiet("It can't be told apart from another session; close the other one first");
            }
        }
    }
}

#[allow(clippy::type_complexity)]
pub fn buttons(
    q: Query<(&Interaction, &Btn), Changed<Interaction>>,
    mut take: ResMut<TakeOver>,
    outside: Res<Outside>,
    svc: Res<Service>,
    sim: Res<crate::core::Sim>,
    mut sound: ResMut<crate::sound::Sound>,
    mut toasts: ResMut<Toasts>,
) {
    for (i, b) in &q {
        if take.card.is_none() {
            return;
        }
        match (i, b) {
            (Interaction::Hovered, Btn::Act | Btn::Row(_)) => sound.play("hover"),
            (Interaction::Pressed, Btn::Act) => act(&mut take, &svc, sim.t, &mut sound, &mut toasts),
            (Interaction::Pressed, Btn::Row(r)) => {
                take.open_one(&outside.list, *r);
                sound.play("tick");
            }
            (Interaction::Pressed, Btn::Away) => {
                take.close();
                sound.play_with("close", 0.6, 0.0);
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------- the card

#[derive(Component)]
pub struct CardRoot;
#[derive(Component)]
pub struct CardBox;

const CARD_W: f32 = 460.0;

pub fn setup(mut commands: Commands, root: Res<crate::hud::OverlayRoot>) {
    // the whole screen: a click away from the card closes it; the card hangs in it, centred, below the strip
    let e = commands
        .spawn((
            CardRoot,
            Interaction::None,
            Btn::Away,
            bevy::ui::FocusPolicy::Pass,
            Node { position_type: PositionType::Absolute, left: px(0), right: px(0), top: px(0), bottom: px(0), justify_content: JustifyContent::Center, align_items: AlignItems::FlexStart, padding: UiRect { top: percent(17) , ..default() }, ..default() },
            Visibility::Hidden,
        ))
        .id();
    commands.entity(root.0).add_child(e);
}

fn chip(c: &mut Commands, f: &UiFonts, text: &str, fg: Color, border: Color) -> Entity {
    let e = c.spawn((Node { padding: UiRect { left: px(6), right: px(6), top: px(4), bottom: px(3) }, border: UiRect::all(px(1)), ..default() }, BorderColor::all(border))).id();
    let t = c.spawn((Text::new(text.to_uppercase()), font(&f.ui700, 10.0, 2.0), TextColor(fg), LineHeight::Px(10.0))).id();
    c.entity(e).add_child(t);
    e
}

fn state_color(state: &str) -> Color {
    match state {
        "waiting" => role_color(Role::Acc),
        "working" => role_color(Role::Line),
        _ => role_color(Role::Dim),
    }
}

/// Rebuilds the card when it changes; eases it in and out; keeps the home's mouse off it.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn draw(
    mut commands: Commands,
    take: Res<TakeOver>,
    outside: Res<Outside>,
    f: Option<Res<UiFonts>>,
    sim: Res<crate::core::Sim>,
    mut ms: ResMut<crate::mouse::MouseState>,
    mut root: Query<(Entity, &mut Visibility), With<CardRoot>>,
    boxes: Query<(&ComputedNode, &UiGlobalTransform, &CardBox)>,
    mut seen: Local<u64>,
    mut amt: Local<Option<Spring>>,
    win: Query<&Window, With<bevy::window::PrimaryWindow>>,
    mut tfs: Query<&mut UiTransform, With<CardBox>>,
) {
    let Some(f) = f else { return };
    let Ok((rootent, mut vis)) = root.single_mut() else { return };
    let want = take.card.is_some();
    let amt = amt.get_or_insert_with(|| Spring::new(0.0, 80.0, 18.0));
    amt.t = if want { 1.0 } else { 0.0 };
    amt.step(crate::motion::dt(crate::motion::Pace::Ui, sim.dt));
    if amt.x < 0.01 && !want {
        vis.set_if_neq(Visibility::Hidden);
        ms.block = None;
        *seen = 0;
        return;
    }
    vis.set_if_neq(Visibility::Inherited);
    if *seen != take.version {
        *seen = take.version;
        commands.entity(rootent).despawn_children();
        if let Some(c) = &take.card {
            build_card(&mut commands, rootent, &f, c, &outside);
        }
    }
    let k = clamp(amt.x, 0.0, 1.0);
    for mut t in &mut tfs {
        t.translation = Val2::px(0.0, (1.0 - k) * 10.0);
        t.scale = Vec2::splat(lerp(0.97, 1.0, k));
    }
    // the card's rectangle in window px, for the mouse
    ms.block = boxes.iter().next().and_then(|(n, t, _)| {
        let scale = win.single().map(|w| w.scale_factor()).unwrap_or(1.0).max(0.01);
        let half = n.size() / 2.0 / scale;
        let c = t.translation / scale;
        (n.size().x > 1.0).then(|| Rect::from_corners(c - half, c + half))
    });
}

fn build_card(c: &mut Commands, root: Entity, f: &UiFonts, card: &Card, outside: &Outside) {
    let line = role_color(Role::Line);
    let hot = role_color(Role::Hot);
    let dim = role_color(Role::Dim);
    let fill = role_color(Role::Fill);
    let acc = role_color(Role::Acc);
    let bad = role_color(Role::Bad);
    let s = &card.session;
    let waiting = !card.choose && s.state == "waiting";
    let accent = if waiting { acc } else { fill };
    let bx = c
        .spawn((
            CardBox,
            Interaction::None,
            bevy::ui::FocusPolicy::Block,
            Node {
                width: px(CARD_W),
                flex_direction: FlexDirection::Column,
                row_gap: px(9),
                padding: UiRect { left: px(18), right: px(18), top: px(14), bottom: px(14) },
                border: UiRect { left: px(3), right: px(1), top: px(1), bottom: px(1) },
                ..default()
            },
            BackgroundGradient(vec![Gradient::Linear(LinearGradient::to_bottom(vec![ColorStop::percent(Color::srgba(0.07, 0.09, 0.1, 0.9), 0.0), ColorStop::percent(Color::srgba(0.03, 0.04, 0.05, 0.94), 100.0)]))]),
            BorderColor { left: accent, right: with_alpha(line, 0.34), top: with_alpha(line, 0.34), bottom: with_alpha(line, 0.34) },
            UiTransform::default(),
        ))
        .id();
    c.entity(root).add_child(bx);
    // header
    let hdr = c
        .spawn((Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(9), padding: UiRect { bottom: px(8), ..default() }, border: UiRect { bottom: px(1), ..default() }, ..default() }, BorderColor::all(with_alpha(line, 0.72))))
        .id();
    let sq = c.spawn((Node { width: px(6), height: px(6), ..default() }, BackgroundColor(accent))).id();
    let head = if card.choose { format!("ELSEWHERE · {}", outside.list.len()) } else { "ELSEWHERE".to_string() };
    let t1 = c.spawn((Text::new(head), font(&f.ui600, 12.0, 3.84), TextColor(hot), LineHeight::Px(14.4))).id();
    c.entity(hdr).add_children(&[sq, t1]);
    if !card.choose {
        let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
        let col = state_color(&s.state);
        let ch = chip(c, f, state_label(&s.state), if waiting { Color::BLACK } else { col }, with_alpha(col, if waiting { 1.0 } else { 0.72 }));
        if waiting {
            c.entity(ch).insert(BackgroundColor(acc));
        }
        c.entity(hdr).add_children(&[sp, ch]);
    }
    c.entity(bx).add_child(hdr);
    if card.choose {
        for (i, o) in outside.list.iter().enumerate().take(9) {
            let on = i == card.sel;
            let row = c
                .spawn((
                    Btn::Row(i),
                    Interaction::None,
                    bevy::ui::FocusPolicy::Block,
                    Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(10), padding: UiRect { left: px(8), right: px(8), top: px(6), bottom: px(6) }, border: UiRect { left: px(2), ..default() }, ..default() },
                    BackgroundColor(if on { with_alpha(line, 0.08) } else { Color::NONE }),
                    BorderColor::all(if on { state_color(&o.state) } else { Color::NONE }),
                ))
                .id();
            let n = c.spawn((Text::new(format!("{}", i + 1)), font(&f.mono, 11.0, 0.0), TextColor(dim), LineHeight::Px(12.0))).id();
            let col = c.spawn(Node { flex_direction: FlexDirection::Column, flex_grow: 1.0, flex_shrink: 1.0, min_width: px(0), ..default() }).id();
            let tt = c
                .spawn((Text::new(crate::appframe::clip(&title_of(o), 40).to_uppercase()), font(&f.ui600, 13.0, 1.3), TextColor(hot), LineHeight::Px(16.0), TextLayout { linebreak: LineBreak::NoWrap, ..default() }))
                .id();
            let meta = if o.ambiguous { format!("{} · {} · can't be told apart", repo_name(o.cwd.as_deref()), where_label(&o.where_)) } else { format!("{} · {}", repo_name(o.cwd.as_deref()), where_label(&o.where_)) };
            let mt = c.spawn((Text::new(meta), font(&f.ui500, 12.0, 0.9), TextColor(with_alpha(line, 0.8)), LineHeight::Px(14.0), TextLayout { linebreak: LineBreak::NoWrap, ..default() })).id();
            c.entity(col).add_children(&[tt, mt]);
            let st = c.spawn((Text::new(state_label(&o.state).to_uppercase()), font(&f.ui700, 9.5, 1.9), TextColor(state_color(&o.state)), LineHeight::Px(10.0))).id();
            c.entity(row).add_children(&[n, col, st]);
            c.entity(bx).add_child(row);
        }
        let foot = footer(c, f, "1-9 OR ARROWS CHOOSE   ·   ENTER OPENS   ·   ESC CLOSES");
        c.entity(bx).add_child(foot);
        return;
    }
    // title and where
    let title = c.spawn((Text::new(crate::appframe::clip(&title_of(s), 44).to_uppercase()), font(&f.ui600, 16.0, 1.9), TextColor(hot), LineHeight::Px(19.0))).id();
    let folder = repo_name(s.cwd.as_deref());
    let meta = if folder.is_empty() { where_label(&s.where_) } else { format!("{folder}  ·  {}", where_label(&s.where_)) };
    let m = c.spawn((Text::new(meta), font(&f.ui500, 13.0, 1.04), TextColor(line), LineHeight::Px(16.9))).id();
    c.entity(bx).add_children(&[title, m]);
    if let Some(msg) = s.last_message.as_deref().filter(|m| !m.trim().is_empty()) {
        let q = c
            .spawn((Node { padding: UiRect { left: px(10), right: px(0), top: px(2), bottom: px(2) }, border: UiRect { left: px(1), ..default() }, ..default() }, BorderColor::all(with_alpha(line, 0.34))))
            .id();
        let t = c.spawn((Text::new(excerpt(msg, 230)), font(&f.ui500, 13.0, 0.52), TextColor(with_alpha(line, 0.85)), LineHeight::Px(17.0))).id();
        c.entity(q).add_child(t);
        c.entity(bx).add_child(q);
    }
    if s.ambiguous || card.siblings > 1 && s.session_id.is_none() {
        let t = c.spawn((Text::new(ambiguous_note(card.siblings)), font(&f.ui600, 13.0, 1.04), TextColor(acc), LineHeight::Px(16.9))).id();
        c.entity(bx).add_child(t);
    } else if card.gone {
        let t = c.spawn((Text::new("It is not running elsewhere any more"), font(&f.ui600, 13.0, 1.04), TextColor(acc), LineHeight::Px(16.9))).id();
        c.entity(bx).add_child(t);
    } else if takeable(s) {
        let (label, col, border, bg) = match card.arm {
            Arm::Idle => (button_label(card.arm, &s.where_), hot, with_alpha(line, 0.5), Color::NONE),
            Arm::Armed => (button_label(card.arm, &s.where_), hot, acc, with_alpha(acc, 0.16)),
            Arm::Pending => (button_label(card.arm, &s.where_), with_alpha(line, 0.7), with_alpha(line, 0.22), Color::NONE),
        };
        let b = c
            .spawn((
                Btn::Act,
                Interaction::None,
                bevy::ui::FocusPolicy::Block,
                Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, justify_content: JustifyContent::SpaceBetween, column_gap: px(12), padding: UiRect { left: px(14), right: px(10), top: px(11), bottom: px(10) }, border: UiRect::all(px(1)), margin: UiRect { top: px(4), ..default() }, ..default() },
                BackgroundColor(bg),
                BorderColor::all(border),
                ActBtn(card.arm),
            ))
            .id();
        let t = c.spawn((Text::new(label.to_uppercase()), font(&f.ui700, 10.5, 1.5), TextColor(col), LineHeight::Px(12.0))).id();
        c.entity(b).add_child(t);
        if card.arm == Arm::Idle {
            let k = c.spawn((Node { padding: UiRect { left: px(6), right: px(6), top: px(5), bottom: px(4) }, border: UiRect::all(px(1)), flex_shrink: 0.0, ..default() }, BorderColor::all(with_alpha(line, 0.13)))).id();
            let kt = c.spawn((Text::new("T · ENTER"), font(&f.ui700, 9.5, 1.9), TextColor(dim), LineHeight::Px(9.5))).id();
            c.entity(k).add_child(kt);
            c.entity(b).add_child(k);
        }
        c.entity(bx).add_child(b);
    }
    if let Some(e) = &card.error {
        let t = c.spawn((Text::new(format!("Could not take over: {}", crate::appframe::clip(e, 160))), font(&f.ui600, 13.0, 1.04), TextColor(bad), LineHeight::Px(16.9))).id();
        c.entity(bx).add_child(t);
    }
    let foot = footer(c, f, if card.arm == Arm::Armed { "ESC CANCELS   ·   THE SESSION STOPS WHERE IT RUNS" } else { "ESC CLOSES" });
    c.entity(bx).add_child(foot);
}

/// The button's look follows the pointer.
#[derive(Component)]
pub struct ActBtn(Arm);

#[allow(clippy::type_complexity)]
pub fn hover_look(mut q: Query<(&Interaction, &ActBtn, &mut BackgroundColor), Changed<Interaction>>) {
    let line = role_color(Role::Line);
    let acc = role_color(Role::Acc);
    for (i, a, mut bg) in &mut q {
        let hovered = matches!(i, Interaction::Hovered | Interaction::Pressed);
        let want = match (a.0, hovered) {
            (Arm::Idle, true) => with_alpha(line, 0.1),
            (Arm::Armed, true) => with_alpha(acc, 0.26),
            (Arm::Armed, false) => with_alpha(acc, 0.16),
            _ => Color::NONE,
        };
        bg.set_if_neq(BackgroundColor(want));
    }
}

fn footer(c: &mut Commands, f: &UiFonts, text: &str) -> Entity {
    let line = role_color(Role::Line);
    let e = c.spawn((Node { padding: UiRect { top: px(9), ..default() }, border: UiRect { top: px(1), ..default() }, ..default() }, BorderColor::all(with_alpha(line, 0.13)))).id();
    let t = c.spawn((Text::new(text), font(&f.ui700, 9.5, 1.9), TextColor(role_color(Role::Dim)), LineHeight::Px(10.0))).id();
    c.entity(e).add_child(t);
    e
}

// ---------------------------------------------------------------- look tests

/// `--takeover card|armed|after`: the look test opens the card, arms it, or plays a successful take-over.
#[allow(clippy::too_many_arguments)]
pub fn script(
    args: Res<crate::Args>,
    sim: Res<crate::core::Sim>,
    svc: Res<Service>,
    mut outside: ResMut<Outside>,
    mut take: ResMut<TakeOver>,
    mut toasts: ResMut<Toasts>,
    mut working: ResMut<crate::working::Working>,
    mut bar: ResMut<crate::cmdbar::CmdBar>,
    mut focus: ResMut<Focus>,
    mut sound: ResMut<crate::sound::Sound>,
) {
    let Some(stage) = args.takeover.as_deref() else { return };
    // keep the toast of the finished look test on screen while the screenshot settles
    if take.scripted && stage == "after" {
        if let Some(t) = toasts.list.last_mut() {
            t.1 = 1.2;
        }
    }
    if take.scripted || sim.t < 1.0 {
        return;
    }
    take.scripted = true;
    match stage {
        "card" | "armed" | "ambiguous" | "list" => {
            let list = outside.list.clone();
            match stage {
                "list" => take.open_list(&list),
                "ambiguous" => take.open_one(&list, 2),
                _ => take.open_one(&list, 0),
            }
            if stage == "armed" {
                take.press(sim.t - 1.0);
            }
        }
        "after" => {
            let (task, title) = demo_task(&svc);
            let r = TakeOverResult { task, title, from: "herdr".into(), idle: false };
            finished(&r, "d1c7-demo-herdr", &mut outside, &mut take, &mut toasts, &mut working, &mut bar, &mut focus, &mut sound);
        }
        _ => {}
    }
}

/// The look test's taken-over agent: a task with a few steps, as sphered would have it after a take-over.
fn demo_task(svc: &Service) -> (String, String) {
    use sphere_proto::{Event, EventBody, StepKind, TaskView};
    let t: TaskView = serde_json::from_value(serde_json::json!({
        "id": "t-takeover", "title": "Migrate billing webhooks to v2", "prompt": "", "state": "working", "status": "Running the webhook tests",
        "repo": "/demo/api-server", "in_place": true, "budget_usd": 5.0, "cost_usd": 0.84,
        "started_at": "2026-10-19T09:20:00Z", "updated_at": "2026-10-19T09:44:00Z", "steps": 5,
    }))
    .expect("demo task");
    let ev = |body| Event { seq: 0, task: "t-takeover".into(), at: "2026-10-19T09:44:00Z".into(), body };
    let step = |id: &str, kind, tool: &str, title: &str| ev(EventBody::Step { id: id.into(), step_kind: kind, tool: tool.into(), title: title.into(), detail: String::new(), parent: None });
    let mut run = crate::service::Run::default();
    run.apply(&ev(EventBody::TakenOver { from: "herdr".into() }));
    for e in [
        step("a", StepKind::Read, "Read", "src/webhooks/handler.rs"),
        step("b", StepKind::Edit, "Edit", "Editing src/webhooks/v2.rs"),
        step("c", StepKind::Edit, "Edit", "Editing tests/webhooks.rs"),
        step("d", StepKind::Run, "Bash", "cargo test webhooks"),
    ] {
        run.apply(&e);
    }
    for id in ["a", "b", "c"] {
        run.apply(&ev(EventBody::StepDone { id: id.into(), ok: true, output: None }));
    }
    run.apply(&ev(EventBody::Say { text: "Picking up where the session left off: the v2 handler is in, now running its tests.".into() }));
    let mut s = svc.shared.lock().unwrap();
    s.runs.insert(t.id.clone(), run);
    s.tasks = vec![t.clone()];
    s.version += 1;
    (t.id, t.title)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sess(id: Option<&str>, cwd: &str, state: &str, amb: bool) -> OutsideSession {
        OutsideSession { session_id: id.map(str::to_string), pid: Some(1), cwd: Some(cwd.into()), title: Some("T".into()), where_: "herdr".into(), state: state.into(), ambiguous: amb, last_message: None, updated_at: None }
    }

    #[test]
    fn where_reads_as_a_phrase() {
        assert_eq!(where_label("herdr"), "in herdr");
        assert_eq!(where_label("Terminal"), "in Terminal");
        assert_eq!(where_label("a terminal"), "in a terminal");
        assert_eq!(where_label("Claude background"), "in Claude background");
        assert_eq!(where_label("over SSH"), "over SSH");
        assert_eq!(where_label(""), "elsewhere");
        assert_eq!(from_label("herdr"), "herdr");
        assert_eq!(from_label("over SSH"), "SSH");
        assert_eq!(state_label("waiting"), "waiting on you");
        assert_eq!(state_label("working"), "working");
        assert_eq!(state_label("idle"), "idle");
        assert_eq!(state_label("???"), "running");
    }

    #[test]
    fn the_folder_is_the_last_part_of_the_cwd() {
        assert_eq!(repo_name(Some("/Users/me/Projects/api-server")), "api-server");
        assert_eq!(repo_name(Some("/Users/me/Projects/api-server/")), "api-server");
        assert_eq!(repo_name(None), "");
        let mut s = sess(Some("a"), "/x/web", "idle", false);
        s.title = None;
        assert_eq!(title_of(&s), "Session in web");
    }

    #[test]
    fn a_session_sits_on_the_orbit_of_the_repo_that_holds_its_cwd() {
        let repos = ["/work/api-server", "/work/web-app", "/work/web-app/packages/ui"];
        assert_eq!(match_repo(repos, "/work/api-server"), Some(0));
        assert_eq!(match_repo(repos, "/work/api-server/src"), Some(0));
        assert_eq!(match_repo(repos, "/work/web-app/src"), Some(1));
        // the deepest repo wins
        assert_eq!(match_repo(repos, "/work/web-app/packages/ui/src"), Some(2));
        // a folder that only shares a prefix is not inside
        assert_eq!(match_repo(repos, "/work/api-server-old"), None);
        assert_eq!(match_repo(repos, "/elsewhere"), None);
        assert_eq!(match_repo(["/work/api-server/"], "/work/api-server"), Some(0));
    }

    #[test]
    fn the_button_needs_two_presses() {
        let (a, fire) = Arm::Idle.press(1.0, 0.0);
        assert_eq!((a, fire), (Arm::Armed, false));
        // the same click twice is not a decision
        assert_eq!(a.press(1.1, 1.0), (Arm::Armed, false));
        let (b, fire) = a.press(2.0, 1.0);
        assert_eq!((b, fire), (Arm::Pending, true));
        // while it runs, nothing more happens
        assert_eq!(b.press(3.0, 1.0), (Arm::Pending, false));
        // Esc disarms, but cannot recall a call that is out
        assert_eq!(Arm::Armed.cancel(), Arm::Idle);
        assert_eq!(Arm::Pending.cancel(), Arm::Pending);
        assert_eq!(Arm::Idle.cancel(), Arm::Idle);
    }

    #[test]
    fn the_button_says_what_the_second_press_does() {
        assert_eq!(button_label(Arm::Idle, "herdr"), "Take over");
        assert_eq!(button_label(Arm::Armed, "herdr"), "Press again: stops it in herdr, continues here");
        assert_eq!(button_label(Arm::Armed, "over SSH"), "Press again: stops it over SSH, continues here");
        assert_eq!(button_label(Arm::Pending, "herdr"), "Taking over…");
    }

    #[test]
    fn the_card_fires_only_on_the_second_press_and_esc_disarms() {
        let list = vec![sess(Some("s1"), "/a", "idle", false)];
        let mut t = TakeOver::default();
        t.open_one(&list, 0);
        assert_eq!(t.press(1.0), None);
        assert_eq!(t.card.as_ref().unwrap().arm, Arm::Armed);
        // Esc closes the card, and a new card starts unarmed
        t.back();
        assert!(t.card.is_none());
        t.open_one(&list, 0);
        assert_eq!(t.card.as_ref().unwrap().arm, Arm::Idle);
        assert_eq!(t.press(5.0), None);
        assert_eq!(t.press(6.0).as_deref(), Some("s1"));
        assert_eq!(t.card.as_ref().unwrap().arm, Arm::Pending);
        // an error shows in the card and disarms it
        t.fail("s1", "it already ended".into());
        let c = t.card.as_ref().unwrap();
        assert_eq!((c.arm, c.error.as_deref()), (Arm::Idle, Some("it already ended")));
    }

    #[test]
    fn an_ambiguous_session_cannot_be_taken_over() {
        let list = vec![sess(None, "/a", "working", true), sess(None, "/a", "idle", true)];
        assert!(!takeable(&list[0]));
        assert_eq!(in_folder(&list, &list[0]), 2);
        assert_eq!(ambiguous_note(2), "2 sessions in this folder; can't tell them apart");
        assert_eq!(ambiguous_note(3), "3 sessions in this folder; can't tell them apart");
        let mut t = TakeOver::default();
        t.open_one(&list, 0);
        assert_eq!(t.press(1.0), None);
        assert_eq!(t.press(9.0), None);
        assert_eq!(t.card.as_ref().unwrap().arm, Arm::Idle);
    }

    #[test]
    fn a_state_change_disarms_the_card() {
        let mut list = vec![sess(Some("s1"), "/a", "idle", false)];
        let mut t = TakeOver::default();
        t.open_one(&list, 0);
        t.press(1.0);
        assert_eq!(t.card.as_ref().unwrap().arm, Arm::Armed);
        // new message, same state: stays armed
        list[0].last_message = Some("more".into());
        t.refresh(&list);
        assert_eq!(t.card.as_ref().unwrap().arm, Arm::Armed);
        list[0].state = "working".into();
        t.refresh(&list);
        assert_eq!(t.card.as_ref().unwrap().arm, Arm::Idle);
        // it ended elsewhere
        t.refresh(&[]);
        assert!(t.card.as_ref().unwrap().gone);
        assert_eq!(t.press(20.0), None);
    }

    #[test]
    fn take_over_picks_the_only_one_else_lists_them() {
        assert_eq!(target_for(0), Target::Nothing);
        assert_eq!(target_for(1), Target::One(0));
        assert_eq!(target_for(3), Target::Choose);
        assert_eq!(parse_words("take over"), Some(Words::TakeOver));
        assert_eq!(parse_words("elsewhere"), Some(Words::List));
        assert_eq!(parse_words("take over the world"), None);
    }

    #[test]
    fn waiting_sessions_come_first() {
        let mut o = Outside::default();
        assert!(o.set(vec![sess(Some("a"), "/a", "idle", false), sess(Some("b"), "/b", "working", false), sess(Some("c"), "/c", "waiting", false)]));
        let states: Vec<&str> = o.list.iter().map(|s| s.state.as_str()).collect();
        assert_eq!(states, ["waiting", "working", "idle"]);
        let v = o.version;
        assert!(!o.set(o.list.clone()));
        assert_eq!(o.version, v);
    }

    #[test]
    fn rows_elsewhere_take_room_from_the_agents_rows() {
        use crate::panel::{agent_rows, agent_rows_in, elsewhere_rows};
        assert_eq!(elsewhere_rows(0), (0, 0));
        assert_eq!(elsewhere_rows(2), (2, 0));
        assert_eq!(elsewhere_rows(5), (3, 2));
        // with nothing elsewhere the panel is as it was
        assert_eq!(agent_rows_in(4, 0.0), agent_rows(4));
        assert_eq!(agent_rows_in(6, 0.0), (3, 3));
        // two sessions elsewhere leave room for two agents: one shows, the rest are counted
        let extra = 24.0 + 2.0 * 36.0;
        assert_eq!(agent_rows_in(2, extra), (2, 0));
        assert_eq!(agent_rows_in(4, extra), (1, 3));
        // Sphere's own agents keep two rows: elsewhere shrinks to one and a "+N more"
        use crate::panel::elsewhere_rows_with;
        assert_eq!(elsewhere_rows_with(3, 4), (1, 2));
        assert_eq!(elsewhere_rows_with(3, 1), (3, 0));
        assert_eq!(elsewhere_rows_with(5, 2), (2, 3));
        assert_eq!(elsewhere_rows_with(2, 0), (2, 0));
        assert_eq!(elsewhere_rows_with(0, 4), (0, 0));
    }

    #[test]
    fn the_excerpt_is_one_short_line() {
        assert_eq!(excerpt("a\n  b   c", 20), "a b c");
        assert_eq!(excerpt("abcdefghij", 5), "abcd…");
    }
}
