#![allow(deprecated)] // Interaction: Bevy 0.20 deprecates it in favour of ui_widgets, which this crate does not enable yet
//! Protocols: agents on your schedule. A line typed in the command bar that opens with a schedule (`every weekday 9:00
//! sphere: update the dependencies`) is saved by sphered, which then runs it as an ordinary task whenever it comes due,
//! with the app open or not. Typing `protocols` opens this card: one row for each, with its schedule in words, the
//! repo, when it runs next, and how the last run went. Space switches the selected one on or off, Enter runs it now, D
//! removes it (the first press arms it, the second does it), Up and Down (or J and K) choose, Esc closes. Every row and
//! button answers the mouse too.
use crate::fonts::UiFonts;
use crate::panel::{font, with_alpha, Focus};
use crate::service::{Cmd, Service};
use crate::util::*;
use bevy::input::keyboard::KeyboardInput;
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::text::LineHeight;
use bevy::ui::FocusPolicy;
use sphere_proto::protocol::{self, LocalZone, Outcome, Protocol, Zone};
use sphere_proto::{TaskState, TaskView};

// ---------------------------------------------------------------- pure: the rows

/// Rows the card shows at once; more scroll with the selection.
pub const MAX_ROWS: usize = 6;
/// A second press this soon after the first is the same click twice, not a decision.
pub const ARM_DEBOUNCE: f32 = 0.35;

/// How a row's last run reads: its colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// not run yet, stopped, or left out
    Dim,
    /// the task is running
    Live,
    /// done
    Ok,
    /// the task waits on you
    Needs,
    /// failed, or could not start
    Bad,
}

/// One protocol as the card shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub id: String,
    pub prompt: String,
    /// `every weekday 09:00`
    pub schedule: String,
    /// the repo's folder name
    pub repo: String,
    /// `in 3 h · Mon 09:00`, or `off`
    pub next: String,
    /// how the last run went, in words: `done · 2 h ago`, `skipped: still running · 5 min ago`
    pub last: String,
    pub tone: Tone,
    pub on: bool,
}

/// The first line of a prompt, to read as one line.
fn one_line(prompt: &str) -> String {
    prompt.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// How the last run went: the words, and the colour they are in.
pub fn last_words(p: &Protocol, state_of: impl Fn(&str) -> Option<TaskState>, now: i64) -> (String, Tone) {
    let outcome = p.outcome(state_of);
    let tone = match &outcome {
        Outcome::Never | Outcome::Stopped | Outcome::Skipped(_) => Tone::Dim,
        Outcome::Running => Tone::Live,
        Outcome::Done => Tone::Ok,
        Outcome::NeedsYou => Tone::Needs,
        Outcome::Failed | Outcome::Error(_) => Tone::Bad,
    };
    let words = outcome.words();
    match &p.last_run {
        Some(l) if outcome != Outcome::Never => (format!("{words} · {} ago", protocol::span_words(now - l.at)), tone),
        _ => (words, tone),
    }
}

/// The rows of the card for the saved protocols. `state_of` says how a task is doing.
pub fn rows(protocols: &[Protocol], state_of: impl Fn(&str) -> Option<TaskState> + Copy, now: i64, zone: &dyn Zone) -> Vec<Row> {
    protocols
        .iter()
        .map(|p| {
            let (last, tone) = last_words(p, state_of, now);
            let next = match (p.enabled, p.next_run) {
                (true, Some(t)) => format!("in {} · {}", protocol::span_words(t - now), protocol::when_words(t, now, zone)),
                _ if p.schedule.is_one_shot() && p.last_run.is_some() => "ran".to_string(),
                _ => "off".to_string(),
            };
            Row { id: p.id.clone(), prompt: one_line(&p.prompt), schedule: p.schedule.describe(now, zone), repo: p.repo_name().to_string(), next, last, tone, on: p.enabled }
        })
        .collect()
}

/// Where the list of `len` rows starts so the selected one shows: `top` stays where it is while the selection is in view.
pub fn window(top: usize, sel: usize, len: usize) -> usize {
    let top = top.min(len.saturating_sub(MAX_ROWS));
    if sel < top {
        sel
    } else if sel >= top + MAX_ROWS {
        sel + 1 - MAX_ROWS
    } else {
        top
    }
}

// ---------------------------------------------------------------- state

#[derive(Resource, Default)]
pub struct Protocols {
    pub open: bool,
    /// the selected row
    pub sel: usize,
    /// the first row shown
    pub top: usize,
    /// what the card shows, from the saved protocols and the state of their tasks
    pub rows: Vec<Row>,
    /// the row whose removal a first press armed: (its id, when)
    pub armed: Option<(String, f32)>,
    /// bumps when the card changes (it redraws on it)
    pub version: u64,
    /// the card just closed: give the keyboard back next frame (so the Esc that closed it is not heard twice)
    release: bool,
    /// the rows are out of date (the card was just opened)
    stale: bool,
    /// the `--protocols` look-test script already ran
    scripted: bool,
}

impl Protocols {
    pub fn open_card(&mut self) {
        if !self.open {
            self.open = true;
            self.sel = 0;
            self.top = 0;
            self.armed = None;
            self.stale = true;
            self.version += 1;
        }
    }

    /// Closes the card (which disarms it).
    pub fn close(&mut self) {
        if self.open {
            self.open = false;
            self.armed = None;
            self.release = true;
            self.version += 1;
        }
    }

    /// New rows: the selection stays on the same protocol if it is still there, else at the same place. True when they differ.
    pub fn set_rows(&mut self, rows: Vec<Row>) -> bool {
        self.stale = false;
        if rows == self.rows {
            return false;
        }
        let id = self.rows.get(self.sel).map(|r| r.id.clone());
        self.sel = id.and_then(|id| rows.iter().position(|r| r.id == id)).unwrap_or(self.sel).min(rows.len().saturating_sub(1));
        self.top = window(self.top, self.sel, rows.len());
        // a protocol that went away cannot be armed
        if self.armed.as_ref().is_some_and(|(id, _)| !rows.iter().any(|r| &r.id == id)) {
            self.armed = None;
        }
        self.rows = rows;
        self.version += 1;
        true
    }

    pub fn selected(&self) -> Option<&Row> {
        self.rows.get(self.sel)
    }

    /// Moves the selection (it wraps) and disarms.
    pub fn step(&mut self, delta: isize) {
        let n = self.rows.len();
        if n == 0 {
            return;
        }
        self.sel = (self.sel as isize + delta).rem_euclid(n as isize) as usize;
        self.top = window(self.top, self.sel, n);
        self.armed = None;
        self.version += 1;
    }

    /// Selects row `i` (a click) and disarms.
    pub fn select(&mut self, i: usize) {
        if i < self.rows.len() && (i != self.sel || self.armed.is_some()) {
            self.sel = i;
            self.top = window(self.top, i, self.rows.len());
            self.armed = None;
            self.version += 1;
        }
    }

    /// D or the Remove button at time `now` (seconds): the first press arms it, a second one removes. Returns the id to
    /// remove. The same click twice is not a decision.
    pub fn press_remove(&mut self, now: f32) -> Option<String> {
        let id = self.selected()?.id.clone();
        match &self.armed {
            Some((armed, at)) if *armed == id && now - at >= ARM_DEBOUNCE => {
                self.armed = None;
                self.version += 1;
                Some(id)
            }
            Some((armed, _)) if *armed == id => None,
            _ => {
                self.armed = Some((id, now));
                self.version += 1;
                None
            }
        }
    }

    /// Esc while armed: disarm, and keep the card. True when it was armed.
    pub fn disarm(&mut self) -> bool {
        let was = self.armed.take().is_some();
        if was {
            self.version += 1;
        }
        was
    }

    /// Whether the selected row's removal is armed.
    pub fn is_armed(&self) -> bool {
        self.armed.is_some()
    }
}

// ---------------------------------------------------------------- opening, reading the saved protocols

/// Copies the saved protocols into the card's rows while it is open, when they, a task's state, or the minute changed.
pub fn sync(svc: Res<Service>, mut p: ResMut<Protocols>, mut seen: Local<(u64, u64, i64)>) {
    if !p.open {
        return;
    }
    let now = crate::core::unix_now();
    let s = svc.shared.lock().unwrap();
    let key = (s.protocols_rev, s.states, now / 60);
    if *seen == key && !p.stale {
        return;
    }
    *seen = key;
    let state_of = |id: &str| s.tasks.iter().find(|t| t.id == id).map(|t| t.state);
    let new = rows(&s.protocols, state_of, now, &LocalZone);
    drop(s);
    p.set_rows(new);
}

/// Typed `protocols` (the bar sets `Open::protocols`): opens the card. While it is up it has the keyboard.
#[allow(clippy::too_many_arguments)]
pub fn commands(
    mut open: ResMut<crate::apps::Open>,
    mut p: ResMut<Protocols>,
    mut take: ResMut<crate::takeover::TakeOver>,
    mut sound: ResMut<crate::sound::Sound>,
    mut focus: ResMut<Focus>,
    bar: Res<crate::cmdbar::CmdBar>,
) {
    if std::mem::take(&mut open.protocols) && !p.open {
        take.close();
        p.open_card();
        sound.play_with("open", 0.7, 0.0);
    }
    if p.open {
        // the card has the keyboard: the home, the swarm and the apps stand by (they skip keys while the bar has focus)
        focus.cmd = true;
        focus.terminal = false;
    } else if std::mem::take(&mut p.release) {
        focus.cmd = bar.open;
    }
}

// ---------------------------------------------------------------- keys and buttons

#[derive(Component, Clone, Copy, PartialEq, Debug)]
pub enum Btn {
    /// a row of the list: selects it
    Row(usize),
    /// a row's LED: switches that protocol on or off
    Led(usize),
    /// the selected protocol on or off
    Toggle,
    /// the selected protocol, now
    Run,
    /// the selected protocol, removed after two presses
    Remove,
    /// anywhere else on the screen
    Away,
}

/// What a key or a button does to the selected protocol.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Act {
    Toggle,
    Run,
    Remove,
}

/// Does `act` on the selected protocol: sends the command to sphered, or arms the removal. Anything else keeps the
/// protocol: it disarms a removal that was armed.
fn act(p: &mut Protocols, what: Act, now: f32, svc: &Service, sound: &mut crate::sound::Sound) {
    let Some(r) = p.selected().cloned() else { return };
    if what != Act::Remove {
        p.disarm();
    }
    match what {
        Act::Toggle => {
            svc.send(Cmd::ProtocolEnable { id: r.id, enabled: !r.on });
            sound.play_with("toggle", 0.7, 0.0);
        }
        Act::Run => {
            svc.send(Cmd::ProtocolRunNow { id: r.id });
            sound.play_with("confirm", 0.7, 0.0);
        }
        Act::Remove => match p.press_remove(now) {
            Some(id) => {
                svc.send(Cmd::ProtocolRemove { id });
                sound.play_with("close", 0.6, 0.0);
            }
            None => sound.play_with("toggle", 0.7, 0.0),
        },
    }
}

/// Up and Down (or K and J) choose, Space switches on or off, Enter runs now, D removes (twice), Esc disarms, then closes.
#[allow(clippy::too_many_arguments)]
pub fn keys(
    mut ev: MessageReader<KeyboardInput>,
    mut p: ResMut<Protocols>,
    bar: Res<crate::cmdbar::CmdBar>,
    svc: Res<Service>,
    sim: Res<crate::core::Sim>,
    mut sound: ResMut<crate::sound::Sound>,
) {
    for e in ev.read() {
        if e.state != ButtonState::Pressed || bar.open || !p.open {
            continue;
        }
        // the arrows repeat while held; the rest are one decision each
        let moving = matches!(e.key_code, KeyCode::ArrowUp | KeyCode::ArrowDown | KeyCode::KeyJ | KeyCode::KeyK);
        if e.repeat && !moving {
            continue;
        }
        match e.key_code {
            KeyCode::Escape => {
                if !p.disarm() {
                    p.close();
                }
                sound.play_with("close", 0.6, 0.0);
            }
            KeyCode::ArrowDown | KeyCode::KeyJ => {
                p.step(1);
                sound.play("tick");
            }
            KeyCode::ArrowUp | KeyCode::KeyK => {
                p.step(-1);
                sound.play("tick");
            }
            KeyCode::Space => act(&mut p, Act::Toggle, sim.t, &svc, &mut sound),
            KeyCode::Enter | KeyCode::NumpadEnter => act(&mut p, Act::Run, sim.t, &svc, &mut sound),
            KeyCode::KeyD => act(&mut p, Act::Remove, sim.t, &svc, &mut sound),
            // a modifier alone is half of a shortcut; any other key keeps the protocol
            KeyCode::ShiftLeft | KeyCode::ShiftRight | KeyCode::ControlLeft | KeyCode::ControlRight | KeyCode::AltLeft | KeyCode::AltRight | KeyCode::SuperLeft | KeyCode::SuperRight => {}
            _ => {
                p.disarm();
            }
        }
    }
}

#[allow(clippy::type_complexity)]
pub fn buttons(
    q: Query<(&Interaction, &Btn), Changed<Interaction>>,
    mut p: ResMut<Protocols>,
    svc: Res<Service>,
    sim: Res<crate::core::Sim>,
    mut sound: ResMut<crate::sound::Sound>,
) {
    for (i, b) in &q {
        if !p.open {
            return;
        }
        match (i, b) {
            (Interaction::Hovered, Btn::Away) => {}
            (Interaction::Hovered, _) => sound.play("hover"),
            (Interaction::Pressed, Btn::Row(r)) => {
                p.select(*r);
                sound.play("tick");
            }
            (Interaction::Pressed, Btn::Led(r)) => {
                p.select(*r);
                act(&mut p, Act::Toggle, sim.t, &svc, &mut sound);
            }
            (Interaction::Pressed, Btn::Toggle) => act(&mut p, Act::Toggle, sim.t, &svc, &mut sound),
            (Interaction::Pressed, Btn::Run) => act(&mut p, Act::Run, sim.t, &svc, &mut sound),
            (Interaction::Pressed, Btn::Remove) => act(&mut p, Act::Remove, sim.t, &svc, &mut sound),
            (Interaction::Pressed, Btn::Away) => {
                p.close();
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
/// A button's colours: at rest and under the pointer.
#[derive(Component)]
pub struct Hover {
    idle: Color,
    hover: Color,
}

const CARD_W: f32 = 720.0;

pub fn setup(mut commands: Commands, root: Res<crate::hud::OverlayRoot>) {
    // the whole screen: a click away from the card closes it; the card hangs in it, centred, below the strip
    let e = commands
        .spawn((
            CardRoot,
            Interaction::None,
            Btn::Away,
            FocusPolicy::Pass,
            Node { position_type: PositionType::Absolute, left: px(0), right: px(0), top: px(0), bottom: px(0), justify_content: JustifyContent::Center, align_items: AlignItems::FlexStart, padding: UiRect { top: percent(17), ..default() }, ..default() },
            Visibility::Hidden,
        ))
        .id();
    commands.entity(root.0).add_child(e);
}

/// Secondary words: the line colour, softened. The Dim role is too dark to read over the glass.
fn soft() -> Color {
    with_alpha(role_color(Role::Line), 0.62)
}

/// The tone's colour.
fn tone_color(t: Tone) -> Color {
    match t {
        Tone::Dim => soft(),
        Tone::Live => role_color(Role::Fill),
        Tone::Ok => role_color(Role::Ok),
        Tone::Needs => role_color(Role::Acc),
        Tone::Bad => role_color(Role::Bad),
    }
}

/// Rebuilds the card when it changes; eases it in and out; keeps the home's mouse off it.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn draw(
    mut commands: Commands,
    p: Res<Protocols>,
    f: Option<Res<UiFonts>>,
    sim: Res<crate::core::Sim>,
    mut ms: ResMut<crate::mouse::MouseState>,
    mut root: Query<(Entity, &mut Visibility), With<CardRoot>>,
    boxes: Query<(&ComputedNode, &UiGlobalTransform), With<CardBox>>,
    mut seen: Local<u64>,
    mut amt: Local<Option<Spring>>,
    mut blocking: Local<bool>,
    win: Query<&Window, With<bevy::window::PrimaryWindow>>,
    mut tfs: Query<&mut UiTransform, With<CardBox>>,
) {
    let Some(f) = f else { return };
    let Ok((rootent, mut vis)) = root.single_mut() else { return };
    let amt = amt.get_or_insert_with(|| Spring::new(0.0, 80.0, 18.0));
    amt.t = if p.open { 1.0 } else { 0.0 };
    amt.step(crate::motion::dt(crate::motion::Pace::Ui, sim.dt));
    if amt.x < 0.01 && !p.open {
        vis.set_if_neq(Visibility::Hidden);
        if std::mem::take(&mut *blocking) {
            ms.block = None;
        }
        *seen = 0;
        return;
    }
    vis.set_if_neq(Visibility::Inherited);
    if *seen != p.version {
        *seen = p.version;
        commands.entity(rootent).despawn_children();
        build_card(&mut commands, rootent, &f, &p);
    }
    let k = clamp(amt.x, 0.0, 1.0);
    for mut t in &mut tfs {
        t.translation = Val2::px(0.0, (1.0 - k) * 10.0);
        t.scale = Vec2::splat(lerp(0.97, 1.0, k));
    }
    // the card's rectangle in window px, for the mouse
    ms.block = boxes.iter().next().and_then(|(n, t)| {
        let scale = win.single().map(|w| w.scale_factor()).unwrap_or(1.0).max(0.01);
        let half = n.size() / 2.0 / scale;
        let c = t.translation / scale;
        (n.size().x > 1.0).then(|| Rect::from_corners(c - half, c + half))
    });
    *blocking = true;
}

fn build_card(c: &mut Commands, root: Entity, f: &UiFonts, p: &Protocols) {
    let line = role_color(Role::Line);
    let hot = role_color(Role::Hot);
    let fill = role_color(Role::Fill);
    let bx = c
        .spawn((
            CardBox,
            Interaction::None,
            FocusPolicy::Block,
            Node {
                width: px(CARD_W),
                max_width: percent(94),
                flex_direction: FlexDirection::Column,
                row_gap: px(9),
                padding: UiRect { left: px(18), right: px(18), top: px(14), bottom: px(14) },
                border: UiRect { left: px(3), right: px(1), top: px(1), bottom: px(1) },
                ..default()
            },
            BackgroundGradient(vec![Gradient::Linear(LinearGradient::to_bottom(vec![ColorStop::percent(Color::srgba(0.07, 0.09, 0.1, 0.9), 0.0), ColorStop::percent(Color::srgba(0.03, 0.04, 0.05, 0.94), 100.0)]))]),
            BorderColor { left: fill, right: with_alpha(line, 0.34), top: with_alpha(line, 0.34), bottom: with_alpha(line, 0.34) },
            UiTransform::default(),
        ))
        .id();
    c.entity(root).add_child(bx);
    // header: the name, and how many are on
    let hdr = c
        .spawn((Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(9), padding: UiRect { bottom: px(8), ..default() }, border: UiRect { bottom: px(1), ..default() }, ..default() }, BorderColor::all(with_alpha(line, 0.72))))
        .id();
    let sq = c.spawn((Node { width: px(6), height: px(6), ..default() }, BackgroundColor(fill))).id();
    let t1 = c.spawn((Text::new("PROTOCOLS"), font(&f.ui600, 12.0, 3.84), TextColor(hot), LineHeight::Px(14.4))).id();
    let on = p.rows.iter().filter(|r| r.on).count();
    let summary = if p.rows.is_empty() { String::new() } else { format!("{} · {on} ON", p.rows.len()) };
    let t2 = c.spawn((Text::new(summary), font(&f.ui500, 12.0, 2.16), TextColor(soft()), LineHeight::Px(14.4), Node { margin: UiRect { left: auto(), ..default() }, ..default() })).id();
    c.entity(hdr).add_children(&[sq, t1, t2]);
    c.entity(bx).add_child(hdr);
    if p.rows.is_empty() {
        build_empty(c, bx, f);
        let foot = footer(c, f, "ESC CLOSES");
        c.entity(bx).add_child(foot);
        return;
    }
    // the rows: a window of them around the selection
    let top = window(p.top, p.sel, p.rows.len());
    let end = (top + MAX_ROWS).min(p.rows.len());
    if top > 0 {
        let m = more(c, f, format!("{top} MORE ABOVE"));
        c.entity(bx).add_child(m);
    }
    let list = c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(2), ..default() }).id();
    for (i, r) in p.rows.iter().enumerate().take(end).skip(top) {
        let row = build_row(c, f, r, i, i == p.sel);
        c.entity(list).add_child(row);
    }
    c.entity(bx).add_child(list);
    if end < p.rows.len() {
        let m = more(c, f, format!("{} MORE BELOW", p.rows.len() - end));
        c.entity(bx).add_child(m);
    }
    // what can be done to the selected one
    if let Some(r) = p.selected() {
        let bar = c.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(10), margin: UiRect { top: px(4), ..default() }, ..default() }).id();
        let toggle = action(c, f, Btn::Toggle, if r.on { "Turn off" } else { "Turn on" }, "SPACE", Look::Plain);
        let run = action(c, f, Btn::Run, "Run now", "ENTER", Look::Plain);
        let rm = if p.is_armed() { action(c, f, Btn::Remove, "Press again to remove", "D", Look::Armed) } else { action(c, f, Btn::Remove, "Remove", "D", Look::Plain) };
        c.entity(bar).add_children(&[toggle, run, rm]);
        c.entity(bx).add_child(bar);
    }
    let keys = if p.is_armed() { "D AGAIN REMOVES   ·   ANY OTHER KEY KEEPS IT   ·   ESC CANCELS" } else { "UP/DOWN OR J/K CHOOSE   ·   SPACE ON/OFF   ·   ENTER RUNS NOW   ·   D D REMOVES   ·   ESC CLOSES" };
    let foot = footer(c, f, keys);
    c.entity(bx).add_child(foot);
}

/// "2 MORE ABOVE": the rows the window leaves out.
fn more(c: &mut Commands, f: &UiFonts, text: String) -> Entity {
    c.spawn((Text::new(text), font(&f.ui600, 10.0, 2.0), TextColor(soft()), LineHeight::Px(12.0))).id()
}

/// What the card says when there is nothing saved: how to write one.
fn build_empty(c: &mut Commands, bx: Entity, f: &UiFonts) {
    let (line, hot, fill) = (role_color(Role::Line), role_color(Role::Hot), role_color(Role::Fill));
    let a = c.spawn((Text::new("Nothing runs on a schedule yet."), font(&f.ui600, 15.0, 0.8), TextColor(hot), LineHeight::Px(19.0))).id();
    let b = c.spawn((Text::new("Type a line in the command bar: when it runs, the repo, and what to do."), font(&f.ui500, 13.0, 0.52), TextColor(with_alpha(line, 0.85)), LineHeight::Px(17.0))).id();
    let eg = c.spawn((Node { padding: UiRect { left: px(10), top: px(3), bottom: px(3), ..default() }, border: UiRect { left: px(1), ..default() }, ..default() }, BorderColor::all(with_alpha(line, 0.34)))).id();
    let egt = c.spawn((Text::new("every weekday 9:00 sphere: update the dependencies, run the tests and fix what breaks"), font(&f.mono, 12.0, 0.0), TextColor(fill), LineHeight::Px(16.0))).id();
    c.entity(eg).add_child(egt);
    let more = c.spawn((Text::new("every day 9:00  ·  every mon,wed,fri 18:30  ·  every 2 hours  ·  every 30 minutes  ·  tomorrow 9:00  ·  in 2 hours"), font(&f.ui500, 12.0, 0.5), TextColor(soft()), LineHeight::Px(15.0))).id();
    c.entity(bx).add_children(&[a, b, eg, more]);
}

/// What a button looks like.
#[derive(Clone, Copy, PartialEq)]
enum Look {
    Plain,
    /// the second press is due
    Armed,
}

/// A button of the action bar: its words, and the key that does the same.
fn action(c: &mut Commands, f: &UiFonts, btn: Btn, label: &str, key: &str, look: Look) -> Entity {
    let (line, hot, acc) = (role_color(Role::Line), role_color(Role::Hot), role_color(Role::Acc));
    let (idle, over, border, chip) = match look {
        Look::Plain => (Color::NONE, with_alpha(line, 0.1), with_alpha(line, 0.5), soft()),
        Look::Armed => (with_alpha(acc, 0.16), with_alpha(acc, 0.26), acc, acc),
    };
    let b = c
        .spawn((
            btn,
            Interaction::None,
            FocusPolicy::Block,
            Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(12), padding: UiRect { left: px(14), right: px(10), top: px(10), bottom: px(9) }, border: UiRect::all(px(1)), ..default() },
            BackgroundColor(idle),
            BorderColor::all(border),
            Hover { idle, hover: over },
        ))
        .id();
    let t = c.spawn((Text::new(label.to_uppercase()), font(&f.ui700, 10.5, 1.5), TextColor(hot), LineHeight::Px(12.0))).id();
    let k = c.spawn((Node { padding: UiRect { left: px(6), right: px(6), top: px(5), bottom: px(4) }, border: UiRect::all(px(1)), flex_shrink: 0.0, ..default() }, BorderColor::all(with_alpha(chip, 0.4)))).id();
    let kt = c.spawn((Text::new(key), font(&f.ui700, 9.5, 1.9), TextColor(chip), LineHeight::Px(9.5))).id();
    c.entity(k).add_child(kt);
    c.entity(b).add_children(&[t, k]);
    b
}

/// One protocol: its LED, what it does and when, when it runs next, and how the last run went.
fn build_row(c: &mut Commands, f: &UiFonts, r: &Row, i: usize, selected: bool) -> Entity {
    let (line, hot, fill) = (role_color(Role::Line), role_color(Role::Hot), role_color(Role::Fill));
    let idle = if selected { with_alpha(line, 0.08) } else { Color::NONE };
    let row = c
        .spawn((
            Btn::Row(i),
            Interaction::None,
            FocusPolicy::Block,
            Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(12), padding: UiRect { left: px(8), right: px(10), top: px(8), bottom: px(7) }, border: UiRect { left: px(2), ..default() }, ..default() },
            BackgroundColor(idle),
            BorderColor::all(if selected { fill } else { Color::NONE }),
            Hover { idle, hover: with_alpha(line, if selected { 0.12 } else { 0.05 }) },
        ))
        .id();
    // the LED: lit while it is on, blinking while its task runs; a click switches it
    let led_btn = c.spawn((Btn::Led(i), Interaction::None, FocusPolicy::Block, Node { width: px(24), height: px(24), justify_content: JustifyContent::Center, align_items: AlignItems::Center, flex_shrink: 0.0, ..default() })).id();
    let led = if r.on {
        let mut e = c.spawn((Node { width: px(8), height: px(8), ..default() }, BackgroundColor(fill)));
        if r.tone == Tone::Live {
            e.insert(crate::panel::Blink);
        }
        e.id()
    } else {
        c.spawn((Node { width: px(8), height: px(8), border: UiRect::all(px(1)), ..default() }, BorderColor::all(with_alpha(line, 0.5)))).id()
    };
    c.entity(led_btn).add_child(led);
    // what it does, and when and where
    let mid = c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(3), flex_grow: 1.0, flex_shrink: 1.0, min_width: px(0), overflow: Overflow::clip(), ..default() }).id();
    let prompt = c
        .spawn((Text::new(crate::appframe::clip(&r.prompt, 58)), font(&f.ui500, 14.0, 0.5), TextColor(if r.on { hot } else { with_alpha(line, 0.7) }), LineHeight::Px(17.0), TextLayout { linebreak: LineBreak::NoWrap, ..default() }))
        .id();
    let meta = c
        .spawn((Text::new(format!("{} · {}", r.schedule, r.repo)), font(&f.ui500, 12.0, 0.9), TextColor(with_alpha(line, 0.8)), LineHeight::Px(14.0), TextLayout { linebreak: LineBreak::NoWrap, ..default() }))
        .id();
    c.entity(mid).add_children(&[prompt, meta]);
    // when it runs next, and how the last run went
    let right = c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(4), align_items: AlignItems::FlexEnd, flex_shrink: 0.0, ..default() }).id();
    let next = c.spawn((Text::new(r.next.to_uppercase()), font(&f.ui600, 11.0, 1.6), TextColor(if r.on { line } else { soft() }), LineHeight::Px(13.0))).id();
    let last = c.spawn((Text::new(crate::appframe::clip(&r.last, 40).to_uppercase()), font(&f.ui700, 9.5, 1.9), TextColor(tone_color(r.tone)), LineHeight::Px(11.0))).id();
    c.entity(right).add_children(&[next, last]);
    c.entity(row).add_children(&[led_btn, mid, right]);
    row
}

fn footer(c: &mut Commands, f: &UiFonts, text: &str) -> Entity {
    let line = role_color(Role::Line);
    let e = c.spawn((Node { padding: UiRect { top: px(9), ..default() }, border: UiRect { top: px(1), ..default() }, justify_content: JustifyContent::FlexEnd, ..default() }, BorderColor::all(with_alpha(line, 0.13)))).id();
    let t = c.spawn((Text::new(text), font(&f.ui700, 9.5, 1.9), TextColor(soft()), LineHeight::Px(10.0))).id();
    c.entity(e).add_child(t);
    e
}

/// A button's look follows the pointer.
#[allow(clippy::type_complexity)]
pub fn hover_look(mut q: Query<(&Interaction, &Hover, &mut BackgroundColor), Changed<Interaction>>) {
    for (i, h, mut bg) in &mut q {
        let on = matches!(i, Interaction::Hovered | Interaction::Pressed);
        bg.set_if_neq(BackgroundColor(if on { h.hover } else { h.idle }));
    }
}

// ---------------------------------------------------------------- look tests

/// The look test's saved protocols and the tasks they started, for `--protocols`: written into the (offline) service's
/// shared slot, so the card, the Next 24h rail and the rows read them the way they read sphered's.
pub fn demo_setup(args: Res<crate::Args>, svc: Res<Service>, mut agents: ResMut<crate::core::Agents>) {
    if !args.offline() || !matches!(args.protocols.as_deref(), Some("card" | "armed" | "rail" | "tags" | "list" | "bar" | "mistake")) {
        return;
    }
    if args.protocols.as_deref() == Some("tags") {
        // two of the Agents panel's rows were started by a protocol
        for a in agents.list.iter_mut().filter(|a| matches!(a.name.as_str(), "Draft release notes" | "Reply to the API review")) {
            a.protocol = true;
        }
    }
    let (protocols, tasks) = demo(crate::core::unix_now(), &LocalZone);
    let mut s = svc.shared.lock().unwrap();
    s.protocols = protocols;
    s.protocols_rev += 1;
    s.tasks = tasks;
    s.states += 1;
    s.version += 1;
}

/// Four protocols in four states, and a fifth that is off: the last run done, waiting on you, failed, left out.
pub fn demo(now: i64, zone: &dyn Zone) -> (Vec<Protocol>, Vec<TaskView>) {
    use protocol::{classify, LastRun, Line};
    let made = |id: &str, line: &str, last: Option<LastRun>, enabled: bool| {
        let Line::Protocol(parsed) = classify(line, now, zone) else { panic!("demo line: {line}") };
        Protocol {
            id: id.into(),
            text: line.into(),
            next_run: enabled.then(|| parsed.schedule.next_after(now, zone)).flatten(),
            schedule: parsed.schedule,
            repo: format!("/Users/me/Projects/{}", parsed.repo.unwrap_or_default()),
            prompt: parsed.prompt,
            enabled,
            created_at: now - 9 * 86_400,
            last_run: last,
        }
    };
    let ran = |task: &str, ago: i64| Some(LastRun { at: now - ago, task: Some(task.into()), skipped: None, error: None });
    let protocols = vec![
        made("p1", "every day 9:00 sphere: update the dependencies, run the tests and fix what breaks", ran("t-p1", 14 * 3600), true),
        made("p2", "every 2 hours api-server: review the new pull requests and flag risky changes", ran("t-p2", 25 * 60), true),
        made("p3", "every mon,wed,fri 18:30 web-app: draft the changelog from the merged pull requests", ran("t-p3", 2 * 86_400 + 3600), true),
        made("p4", "every day 2:00 docs-site: look for broken links in the guide", Some(LastRun { at: now - 11 * 3600, task: None, skipped: Some("still running".into()), error: None }), true),
        made("p5", "every weekday 17:00 sphere: summarize what changed today", None, false),
    ];
    let task = |id: &str, title: &str, state: &str, protocol: &str| -> TaskView {
        serde_json::from_value(serde_json::json!({
            "id": id, "title": title, "prompt": "", "state": state, "status": "", "protocol": protocol,
            "repo": "/Users/me/Projects/demo", "in_place": false, "budget_usd": 5.0, "cost_usd": 0.4,
            "started_at": "2026-10-19T09:00:00Z", "updated_at": "2026-10-19T09:30:00Z", "steps": 12,
        }))
        .expect("demo task")
    };
    let tasks = vec![task("t-p1", "Update the dependencies", "done", "p1"), task("t-p2", "Review the new pull requests", "needs_you", "p2"), task("t-p3", "Draft the changelog", "failed", "p3")];
    (protocols, tasks)
}

/// `--protocols card|armed|list|empty|rail|tags|bar|mistake`: the look test opens the card (armed: with the second row's
/// removal armed; empty: with nothing saved), leaves it shut for the home (`rail`, `tags`), or types a protocol line in
/// the command bar (`bar`: one that reads, `mistake`: one that does not).
#[allow(clippy::too_many_arguments)]
pub fn script(
    args: Res<crate::Args>,
    sim: Res<crate::core::Sim>,
    mut p: ResMut<Protocols>,
    mut bar: ResMut<crate::cmdbar::CmdBar>,
    mut focus: ResMut<Focus>,
    mut sound: ResMut<crate::sound::Sound>,
) {
    let Some(stage) = args.protocols.as_deref() else { return };
    if p.scripted || sim.t < 1.0 {
        return;
    }
    p.scripted = true;
    match stage {
        "card" | "list" | "empty" | "armed" => {
            p.open_card();
            if stage == "armed" {
                // the rows arrive on the next `sync`
                let (protocols, tasks) = demo(crate::core::unix_now(), &LocalZone);
                let state_of = |id: &str| tasks.iter().find(|t| t.id == id).map(|t| t.state);
                p.set_rows(rows(&protocols, state_of, crate::core::unix_now(), &LocalZone));
                p.select(1);
                p.press_remove(sim.t - 1.0);
            }
            sound.play_with("open", 0.7, 0.0);
        }
        "bar" | "mistake" => {
            bar.open = true;
            bar.text = if stage == "bar" { "every weekday 9:00 sphere: update the dependencies" } else { "every 10 minutes sphere: check the build" }.into();
            bar.version += 1;
            focus.cmd = true;
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{FixedZone, LastRun, Schedule, Weekday};

    const NOW: i64 = 1_792_400_400; // 2026-10-19 09:00 UTC, a Monday
    const UTC: FixedZone = FixedZone(0);

    fn p(id: &str, last_run: Option<LastRun>, enabled: bool) -> Protocol {
        Protocol {
            id: id.into(),
            text: format!("every weekday 9:00 sphere: update {id}"),
            schedule: Schedule::Weekly { days: Weekday::WEEKDAYS.to_vec(), hour: 9, minute: 0 },
            repo: "/home/me/Projects/sphere".into(),
            prompt: format!("update {id}\nand then some"),
            enabled,
            created_at: NOW - 86_400,
            last_run,
            next_run: enabled.then_some(NOW + 3 * 3600),
        }
    }

    fn ran(task: &str, ago: i64) -> LastRun {
        LastRun { at: NOW - ago, task: Some(task.into()), skipped: None, error: None }
    }

    fn states(list: &'static [(&'static str, TaskState)]) -> impl Fn(&str) -> Option<TaskState> + Copy {
        move |id| list.iter().find(|(t, _)| *t == id).map(|(_, s)| *s)
    }

    #[test]
    fn a_row_says_what_when_where_and_how_the_last_run_went() {
        let list = [p("p1", Some(ran("t1", 2 * 3600)), true)];
        let r = &rows(&list, states(&[("t1", TaskState::Done)]), NOW, &UTC)[0];
        assert_eq!(r.prompt, "update p1", "the first line of the prompt");
        assert_eq!(r.schedule, "every weekday 09:00");
        assert_eq!(r.repo, "sphere");
        assert_eq!(r.next, "in 3 h · 12:00");
        assert_eq!((r.last.as_str(), r.tone), ("done · 2 h ago", Tone::Ok));
        assert!(r.on);
    }

    #[test]
    fn the_last_run_is_coloured_by_how_it_went() {
        let words = |last: Option<LastRun>, state: Option<(&'static str, TaskState)>| {
            let pr = p("p1", last, true);
            match state {
                Some((id, s)) => last_words(&pr, move |t| (t == id).then_some(s), NOW),
                None => last_words(&pr, |_| None, NOW),
            }
        };
        assert_eq!(words(None, None), ("not run yet".to_string(), Tone::Dim));
        assert_eq!(words(Some(ran("t1", 600)), Some(("t1", TaskState::Working))), ("running · 10 min ago".to_string(), Tone::Live));
        assert_eq!(words(Some(ran("t1", 600)), Some(("t1", TaskState::Queued))).1, Tone::Live);
        assert_eq!(words(Some(ran("t1", 600)), Some(("t1", TaskState::NeedsYou))), ("needs you · 10 min ago".to_string(), Tone::Needs));
        assert_eq!(words(Some(ran("t1", 600)), Some(("t1", TaskState::Failed))), ("failed · 10 min ago".to_string(), Tone::Bad));
        assert_eq!(words(Some(ran("t1", 600)), Some(("t1", TaskState::Stopped))).1, Tone::Dim);
        let skipped = LastRun { at: NOW - 300, task: None, skipped: Some("still running".into()), error: None };
        assert_eq!(words(Some(skipped), None), ("skipped: still running · 5 min ago".to_string(), Tone::Dim));
        let broke = LastRun { at: NOW - 30, task: None, skipped: None, error: Some("no git repo".into()) };
        assert_eq!(words(Some(broke), None), ("could not start: no git repo · <1 min ago".to_string(), Tone::Bad));
    }

    #[test]
    fn an_off_protocol_and_a_spent_one_have_no_next_run() {
        let off = p("p1", None, false);
        assert_eq!(rows(std::slice::from_ref(&off), |_| None, NOW, &UTC)[0].next, "off");
        assert!(!rows(&[off], |_| None, NOW, &UTC)[0].on);
        let mut once = p("p2", Some(ran("t2", 60)), false);
        once.schedule = Schedule::Once { at: NOW - 60 };
        assert_eq!(rows(&[once], |_| None, NOW, &UTC)[0].next, "ran");
    }

    #[test]
    fn the_window_keeps_the_selection_in_view() {
        assert_eq!(window(0, 0, 3), 0, "everything fits");
        assert_eq!(window(0, 5, 9), 0);
        assert_eq!(window(0, 6, 9), 1, "one past the last row: scroll by one");
        assert_eq!(window(1, 8, 9), 3);
        assert_eq!(window(3, 2, 9), 2, "up past the first row");
        assert_eq!(window(7, 8, 9), 3, "never past the last page");
        assert_eq!(window(0, 0, 0), 0);
    }

    fn many(n: usize) -> Vec<Row> {
        (0..n).map(|i| Row { id: format!("p{}", i + 1), prompt: format!("do {i}"), schedule: "every day 09:00".into(), repo: "r".into(), next: "off".into(), last: "not run yet".into(), tone: Tone::Dim, on: i % 2 == 0 }).collect()
    }

    #[test]
    fn the_selection_wraps_and_disarms() {
        let mut c = Protocols::default();
        c.open_card();
        assert!(c.set_rows(many(3)));
        c.step(-1);
        assert_eq!(c.sel, 2, "up from the first goes to the last");
        c.step(1);
        assert_eq!(c.sel, 0);
        c.press_remove(1.0);
        assert!(c.is_armed());
        c.step(1);
        assert!(!c.is_armed(), "moving on disarms");
        // no rows: nothing to move
        let mut e = Protocols::default();
        e.step(1);
        assert_eq!(e.sel, 0);
    }

    #[test]
    fn removing_takes_two_presses_on_the_same_row() {
        let mut c = Protocols::default();
        c.open_card();
        c.set_rows(many(3));
        assert_eq!(c.press_remove(1.0), None);
        assert_eq!(c.armed.as_ref().map(|a| a.0.as_str()), Some("p1"));
        // the same click twice is not a decision
        assert_eq!(c.press_remove(1.1), None);
        assert!(c.is_armed());
        assert_eq!(c.press_remove(2.0).as_deref(), Some("p1"));
        assert!(!c.is_armed());
        // Esc disarms and keeps the card; another Esc closes it
        c.press_remove(5.0);
        assert!(c.disarm() && c.open);
        assert!(!c.disarm());
        // a press on another row arms that one, not the first
        c.press_remove(6.0);
        c.select(2);
        assert!(!c.is_armed());
        assert_eq!(c.press_remove(7.0), None);
        assert_eq!(c.armed.as_ref().map(|a| a.0.as_str()), Some("p3"));
    }

    #[test]
    fn rows_that_change_keep_the_selection_on_the_same_protocol() {
        let mut c = Protocols::default();
        c.open_card();
        c.set_rows(many(4));
        c.select(2);
        // p1 is removed: p3 moves up one place and is still the selection
        let mut rest = many(4);
        rest.remove(0);
        assert!(c.set_rows(rest.clone()));
        assert_eq!((c.sel, c.selected().map(|r| r.id.as_str())), (1, Some("p3")));
        assert!(!c.set_rows(rest), "the same rows change nothing");
        // the selected one is removed: the selection stays at its place, and an armed removal is dropped
        c.press_remove(1.0);
        let mut fewer = many(4);
        fewer.remove(2);
        fewer.remove(0);
        c.set_rows(fewer);
        assert!(!c.is_armed());
        assert_eq!(c.sel, 1);
        // everything removed
        c.set_rows(vec![]);
        assert_eq!((c.sel, c.selected()), (0, None));
    }

    #[test]
    fn the_card_opens_once_and_closing_gives_the_keyboard_back() {
        let mut c = Protocols::default();
        c.open_card();
        let v = c.version;
        c.open_card();
        assert_eq!(c.version, v, "already open");
        c.close();
        assert!(!c.open && c.release);
        let v = c.version;
        c.close();
        assert_eq!(c.version, v, "already shut");
    }

    #[test]
    fn the_demo_has_four_states_and_one_off() {
        let (protocols, tasks) = demo(NOW, &UTC);
        let state_of = |id: &str| tasks.iter().find(|t| t.id == id).map(|t| t.state);
        let r = rows(&protocols, state_of, NOW, &UTC);
        assert_eq!(r.iter().map(|r| r.tone).collect::<Vec<_>>(), [Tone::Ok, Tone::Needs, Tone::Bad, Tone::Dim, Tone::Dim]);
        assert_eq!(r.iter().map(|r| r.on).collect::<Vec<_>>(), [true, true, true, true, false]);
        assert!(r[3].last.starts_with("skipped: still running"));
        assert_eq!(r[4].next, "off");
        // the three that are on and not weekly-only run in the next day; the demo is for the rail too
        let soon = protocols.iter().filter(|p| p.next_run.is_some_and(|t| t - NOW < 86_400)).count();
        assert!(soon >= 3, "{soon}");
    }

    /// What the card sent to sphered, as words.
    fn sent(rx: &mut tokio::sync::mpsc::UnboundedReceiver<Cmd>) -> Vec<String> {
        let mut out = vec![];
        while let Ok(c) = rx.try_recv() {
            out.push(match c {
                Cmd::ProtocolEnable { id, enabled } => format!("{} {id}", if enabled { "on" } else { "off" }),
                Cmd::ProtocolRunNow { id } => format!("run {id}"),
                Cmd::ProtocolRemove { id } => format!("remove {id}"),
                _ => "something else".into(),
            });
        }
        out
    }

    fn card() -> (Protocols, Service, tokio::sync::mpsc::UnboundedReceiver<Cmd>, crate::sound::Sound) {
        let mut c = Protocols::default();
        c.open_card();
        c.set_rows(many(3));
        let (svc, rx) = Service::with_receiver();
        (c, svc, rx, crate::sound::Sound::default())
    }

    #[test]
    fn space_switches_the_selected_one_and_enter_runs_it() {
        let (mut c, svc, mut rx, mut sound) = card();
        act(&mut c, Act::Toggle, 1.0, &svc, &mut sound);
        c.step(1);
        act(&mut c, Act::Toggle, 1.0, &svc, &mut sound);
        act(&mut c, Act::Run, 1.0, &svc, &mut sound);
        // `many` has p1 on and p2 off
        assert_eq!(sent(&mut rx), ["off p1", "on p2", "run p2"]);
    }

    #[test]
    fn removing_sends_nothing_until_the_second_press_and_anything_else_keeps_it() {
        let (mut c, svc, mut rx, mut sound) = card();
        act(&mut c, Act::Remove, 1.0, &svc, &mut sound);
        assert!(c.is_armed() && sent(&mut rx).is_empty(), "the first press only arms");
        // switching it instead keeps it, and the removal is off
        act(&mut c, Act::Toggle, 1.5, &svc, &mut sound);
        assert!(!c.is_armed());
        assert_eq!(sent(&mut rx), ["off p1"]);
        act(&mut c, Act::Remove, 2.0, &svc, &mut sound);
        act(&mut c, Act::Remove, 2.5, &svc, &mut sound);
        assert_eq!(sent(&mut rx), ["remove p1"]);
        assert!(!c.is_armed());
    }

    #[test]
    fn a_card_with_nothing_on_it_sends_nothing() {
        let mut c = Protocols::default();
        c.open_card();
        let (svc, mut rx) = Service::with_receiver();
        let mut sound = crate::sound::Sound::default();
        for what in [Act::Toggle, Act::Run, Act::Remove] {
            act(&mut c, what, 1.0, &svc, &mut sound);
        }
        assert!(sent(&mut rx).is_empty() && !c.is_armed());
    }

    /// Bevy refuses a system whose queries overlap when it sets it up, which only a real run would otherwise show:
    /// every system this feature adds or changes is set up here once.
    #[test]
    fn the_systems_of_the_feature_set_up_without_conflicts() {
        let mut world = World::new();
        let mut schedule = bevy::ecs::schedule::Schedule::default();
        schedule.add_systems((super::sync, super::commands, super::keys, super::buttons, super::draw, super::script, super::hover_look, super::demo_setup, crate::cmdbar::draw_preview, crate::homepanels::draw_rail));
        schedule.initialize(&mut world).unwrap();
    }

    #[test]
    fn the_prompt_reads_as_one_line() {
        assert_eq!(one_line("  update the\n  dependencies "), "update the");
        assert_eq!(one_line("\n\n  a   b\nc"), "a b");
        assert_eq!(one_line(""), "");
    }
}
