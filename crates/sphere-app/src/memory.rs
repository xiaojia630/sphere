#![allow(deprecated)] // Interaction: Bevy 0.20 deprecates it in favour of ui_widgets, which this crate does not enable yet
//! Memory: the notes every agent starts with. What you tell Sphere to remember about a repo ("use pnpm, never npm") is
//! kept by sphered and given to each agent it starts there, so a correction is made once. A note comes from the command
//! bar (`remember: <note>`, `remember <repo>: <note>`, `remember all: <note>`), from a Change you just sent (R on the
//! Agent moment card, see `moment`), or from `sphere remember`. `memory` in the command bar opens a glass card with the
//! notes by repo, each with its age and where it came from. D removes the selected one; it cannot be undone, so it
//! takes two presses (the first arms it, the second does it). Esc closes.
use crate::fonts::UiFonts;
use crate::live::Toasts;
use crate::panel::{font, with_alpha, Focus};
use crate::service::{Cmd, Service, Shared};
use crate::takeover::Arm;
use crate::util::*;
use bevy::input::keyboard::KeyboardInput;
use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::text::LineHeight;
use sphere_proto::api::MemoryAdded;
use sphere_proto::memory::{ago, by_repo, repo_name, NOTES_MAX};
use sphere_proto::{clean_note, now_rfc3339, Note, NoteSource};
use std::ops::Range;

// ---------------------------------------------------------------- pure: words

/// "1 note", "4 notes".
pub fn notes_word(n: usize) -> String {
    format!("{n} note{}", if n == 1 { "" } else { "s" })
}

/// Whose notes: a repo's name, or "all repos".
fn scope_word(repo: Option<&str>) -> String {
    repo.map_or("all repos".to_string(), |r| repo_name(r).to_string())
}

/// What the toast says once a note is kept: "Remembered for sphere · 4 notes", or, when the very same note was there,
/// "Already remembered for sphere".
pub fn remembered(a: &MemoryAdded) -> String {
    let scope = scope_word(a.note.repo.as_deref());
    if a.added {
        format!("Remembered for {scope} · {}", notes_word(a.count))
    } else {
        format!("Already remembered for {scope}")
    }
}

/// What the toast says once a note is removed: "Removed from sphere · 2 notes left".
fn removed(repo: Option<&str>, left: usize) -> String {
    let left = if left == 0 { "none left".to_string() } else { format!("{} left", notes_word(left)) };
    format!("Removed from {} · {left}", scope_word(repo))
}

/// Where a note came from, in the card's words.
pub fn source_label(s: NoteSource) -> &'static str {
    match s {
        NoteSource::Typed => "typed",
        NoteSource::Change => "a change you sent",
        NoteSource::Cli => "the CLI",
    }
}

/// A note's second line in the card: "2d ago · a change you sent".
fn meta_line(n: &Note, age_secs: u64) -> String {
    format!("{} · {}", ago(age_secs), source_label(n.source))
}

/// A path for a heading: your home folder as `~`, a long one cut at its front.
fn short_path(path: &str, home: &str) -> String {
    let p = match path.strip_prefix(home).filter(|_| !home.is_empty() && home != "/") {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("~{rest}"),
        _ => path.to_string(),
    };
    let n = p.chars().count();
    if n > 34 {
        format!("…{}", p.chars().skip(n - 33).collect::<String>())
    } else {
        p
    }
}

/// A group's heading: the repo's name, how many notes it has, and where it is (every agent, for the notes for all).
fn heading(repo: Option<&str>, count: usize, home: &str) -> (String, String, String) {
    match repo {
        Some(r) => (repo_name(r).to_uppercase(), notes_word(count).to_uppercase(), short_path(r, home)),
        None => ("ALL REPOS".to_string(), notes_word(count).to_uppercase(), "every agent".to_string()),
    }
}

// ---------------------------------------------------------------- pure: the command bar

/// What a line typed in the command bar asks of the notes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Typed {
    /// `memory`: open the card
    Show,
    /// `remember: <note>`, `remember <repo>: <note>`, `remember all: <note>`
    Remember { to: To, note: String },
}

/// Which notes a `remember` is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum To {
    /// the last repo used
    Usual,
    /// a repo, named as `repo: task` names one
    Repo(String),
    /// every repo
    All,
}

/// `memory` and the `remember` forms. Other words, including "remember to update the docs", are a task.
pub fn parse(line: &str) -> Option<Typed> {
    let t = line.trim();
    if matches!(t.to_ascii_lowercase().as_str(), "memory" | "memories" | "notes" | "open memory" | "show memory") {
        return Some(Typed::Show);
    }
    let rest = t.get(..8).filter(|w| w.eq_ignore_ascii_case("remember")).and_then(|_| t.get(8..))?;
    let (to, note) = match rest.chars().next() {
        None => (To::Usual, ""),
        Some(':') => (To::Usual, &rest[1..]),
        Some(c) if c.is_whitespace() => {
            // one word before the colon: `all`, or the repo
            let (head, note) = rest.split_once(':')?;
            let head = head.trim();
            if head.is_empty() || head.contains(char::is_whitespace) {
                return None;
            }
            (if head.eq_ignore_ascii_case("all") { To::All } else { To::Repo(head.to_string()) }, note)
        }
        Some(_) => return None,
    };
    Some(Typed::Remember { to, note: note.trim().to_string() })
}

/// Runs a typed `memory` or `remember …`. False when the line is neither: it is a task.
pub fn command(line: &str, svc: &Service, toasts: &mut Toasts, open: &mut crate::apps::Open) -> bool {
    let Some(typed) = parse(line) else { return false };
    let (to, note) = match typed {
        Typed::Show => {
            open.memory = true;
            return true;
        }
        Typed::Remember { to, note } => (to, note),
    };
    if clean_note(&note).is_none() {
        toasts.push("Nothing to remember: type  remember: use pnpm, never npm");
        return true;
    }
    let repo = match to {
        To::All => Ok(None),
        To::Repo(name) => crate::cmdbar::named_repo(&name).map(Some),
        To::Usual => crate::cmdbar::usual_repo().map(Some).ok_or_else(|| "Which repo? Type  remember repo: note".to_string()),
    };
    match repo {
        Ok(repo) => save(svc, toasts, repo.as_deref(), &note, NoteSource::Typed),
        Err(e) => toasts.push(e),
    }
    true
}

// ---------------------------------------------------------------- keeping and removing

/// Keeps a note for `repo` (`None`: every repo). sphered keeps it and the toast follows when it has; with no sphered
/// (a demo) the app's own list does, at once.
pub fn save(svc: &Service, toasts: &mut Toasts, repo: Option<&str>, text: &str, source: NoteSource) {
    let Some(text) = clean_note(text) else {
        toasts.push("Nothing to remember");
        return;
    };
    if !svc.offline {
        svc.send(Cmd::Remember { repo: repo.map(str::to_string), text, source });
        return;
    }
    match keep_here(&mut svc.shared.lock().unwrap(), repo, &text, source) {
        Ok(added) => toasts.push_ok(remembered(&added)),
        Err(e) => toasts.push_bad(e),
    }
}

/// Removes a note. Without sphere's service (a demo) it goes from the app's own list.
pub fn forget(svc: &Service, id: u64) {
    if !svc.offline {
        svc.send(Cmd::Forget { id });
        return;
    }
    let mut s = svc.shared.lock().unwrap();
    s.memory.retain(|n| n.id != id);
    s.memory_rev += 1;
}

/// What sphered's `memory.add` does, for an app with no sphered: same cleaning, no duplicates in a scope, the same cap.
fn keep_here(s: &mut Shared, repo: Option<&str>, text: &str, source: NoteSource) -> Result<MemoryAdded, String> {
    let text = clean_note(text).ok_or("Nothing to remember")?;
    let same_scope = |n: &Note| n.repo.as_deref() == repo;
    if let Some(n) = s.memory.iter().find(|n| same_scope(n) && n.text == text) {
        let count = s.memory.iter().filter(|n| same_scope(n)).count();
        return Ok(MemoryAdded { note: n.clone(), added: false, count });
    }
    if s.memory.len() >= NOTES_MAX {
        return Err(format!("Memory is full ({NOTES_MAX} notes): remove one first"));
    }
    let id = s.memory.iter().map(|n| n.id).max().unwrap_or(0) + 1;
    let note = Note { id, repo: repo.map(str::to_string), text, created_at: now_rfc3339(), source };
    s.memory.insert(0, note.clone());
    s.memory_rev += 1;
    let count = s.memory.iter().filter(|n| same_scope(n)).count();
    Ok(MemoryAdded { note, added: true, count })
}

// ---------------------------------------------------------------- the notes, and what the card does with them

/// The notes as the app shows them, newest first.
#[derive(Resource, Default)]
pub struct Notes {
    pub list: Vec<Note>,
    /// bumps when the list changes (the card redraws on it)
    pub version: u64,
}

/// The notes in the order the card lists them, top to bottom: each repo's, newest first, then the ones for all repos.
pub fn order(notes: &[Note]) -> Vec<&Note> {
    by_repo(notes).into_iter().flat_map(|(_, group)| group).collect()
}

/// One line of the card: a repo's heading, or one of its notes.
#[derive(Debug, PartialEq)]
enum Line<'a> {
    /// the repo (`None`: all repos) and how many notes it has
    Head(Option<&'a str>, usize),
    Note(&'a Note),
}

fn lines(notes: &[Note]) -> Vec<Line<'_>> {
    let mut out = vec![];
    for (repo, group) in by_repo(notes) {
        out.push(Line::Head(repo, group.len()));
        out.extend(group.into_iter().map(Line::Note));
    }
    out
}

/// How many lines (headings and notes) the card shows at most.
const SHOWN: usize = 10;

/// The lines to draw so that the selected note is in view: the most that fit, around it, starting at a heading when that
/// keeps it in view.
fn visible(lines: &[Line], sel: Option<u64>) -> Range<usize> {
    if lines.len() <= SHOWN {
        return 0..lines.len();
    }
    let at = lines.iter().position(|l| matches!(l, Line::Note(n) if Some(n.id) == sel)).unwrap_or(0);
    let mut start = at.saturating_sub(SHOWN / 2).min(lines.len() - SHOWN);
    if let Some(h) = (0..=start).rev().find(|&i| matches!(lines[i], Line::Head(..))) {
        if at < h + SHOWN {
            start = h;
        }
    }
    start..start + SHOWN
}

/// What the card shows and where its selection is.
#[derive(Resource, Default)]
pub struct MemoryCard {
    pub open: bool,
    /// the selected note (by id, so it stays put when others come and go)
    pub sel: Option<u64>,
    /// where it was in `order`, for choosing a neighbour if it goes
    at: usize,
    pub arm: Arm,
    armed_at: f32,
    /// bumps when the card changes (it redraws on it)
    pub version: u64,
    /// the card just closed: give the keyboard back next frame (so the Esc that closed it is not heard twice)
    release: bool,
    /// the `--memory` look-test script already ran
    scripted: bool,
}

impl MemoryCard {
    /// Opens the card on the newest note of the first repo.
    pub fn open(&mut self, notes: &[Note]) {
        self.open = true;
        self.at = 0;
        self.sel = order(notes).first().map(|n| n.id);
        self.arm = Arm::Idle;
        self.version += 1;
    }

    /// Closes the card (which disarms it).
    pub fn close(&mut self) {
        if self.open {
            self.open = false;
            self.arm = Arm::Idle;
            self.release = true;
            self.version += 1;
        }
    }

    /// The next note (or the one before), wrapping round. Moving disarms.
    pub fn step(&mut self, notes: &[Note], back: bool) {
        self.go(notes, back, true);
    }

    /// The next note (or the one before) as the wheel moves: it stops at the ends instead of going round, so a trackpad's
    /// coasting does not spin the list. False when it did not move.
    pub fn scroll(&mut self, notes: &[Note], back: bool) -> bool {
        self.go(notes, back, false)
    }

    fn go(&mut self, notes: &[Note], back: bool, wrap: bool) -> bool {
        let list = order(notes);
        let n = list.len();
        if n == 0 {
            return false;
        }
        let from = self.sel.and_then(|id| list.iter().position(|x| x.id == id)).unwrap_or(self.at);
        let to = match (back, wrap) {
            (true, true) => (from + n - 1) % n,
            (false, true) => (from + 1) % n,
            (true, false) if from > 0 => from - 1,
            (false, false) if from + 1 < n => from + 1,
            _ => return false,
        };
        self.at = to;
        self.sel = Some(list[to].id);
        self.arm = self.arm.cancel();
        self.version += 1;
        true
    }

    /// A click on a note's row selects it (a click on the one that is armed leaves it armed).
    pub fn select(&mut self, notes: &[Note], id: u64) {
        if self.sel == Some(id) {
            return;
        }
        if let Some(at) = order(notes).iter().position(|n| n.id == id) {
            self.at = at;
            self.sel = Some(id);
            self.arm = self.arm.cancel();
            self.version += 1;
        }
    }

    /// One press of Remove (the key or the button) at time `now`: arms, then, the second time, gives the id to remove. The
    /// note below it takes the selection (the one above, if it was the last).
    pub fn press_remove(&mut self, notes: &[Note], now: f32) -> Option<u64> {
        let id = self.sel?;
        let (arm, fire) = self.arm.press(now, self.armed_at);
        if self.arm == Arm::Idle {
            self.armed_at = now;
        }
        self.arm = arm;
        self.version += 1;
        if !fire {
            return None;
        }
        let list = order(notes);
        let at = list.iter().position(|n| n.id == id)?;
        let next = list.get(at + 1).or_else(|| at.checked_sub(1).and_then(|i| list.get(i))).map(|n| n.id);
        self.sel = next;
        self.at = at.min(list.len().saturating_sub(2));
        self.arm = Arm::Idle;
        Some(id)
    }

    /// The list changed under the card: the selection stays on its note, or, if that is gone, on its neighbour, and an
    /// armed Remove is disarmed when the note it was for is not the one selected any more.
    pub fn notes_changed(&mut self, notes: &[Note]) {
        let list = order(notes);
        let was = self.sel;
        match was.and_then(|id| list.iter().position(|n| n.id == id)) {
            Some(at) => self.at = at,
            None => self.at = self.at.min(list.len().saturating_sub(1)),
        }
        self.sel = list.get(self.at).map(|n| n.id);
        if self.sel != was {
            self.arm = self.arm.cancel();
        }
        if self.open {
            self.version += 1;
        }
    }
}

// ---------------------------------------------------------------- the plugin

pub struct MemoryPlugin;

impl Plugin for MemoryPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Notes>().init_resource::<MemoryCard>();
        app.add_systems(Startup, setup.after(crate::mouse::setup_reticle));
        // the keys come once the bar and the Agent moment card have had theirs (the bar sets `Open::memory`)
        app.add_systems(Update, (crate::moment::offer_clock, sync, keys, buttons, wheel).chain().after(crate::moment::keys));
        // the keyboard is held, and given back, after everything that reads keys has had its turn (as the take over card does)
        app.add_systems(Update, commands.after(crate::takeover::commands).before(crate::apps::apply));
        app.add_systems(Update, (script, draw, hover_look).chain().after(crate::takeover::draw));
    }
}

/// Copies the notes out of the shared slot, and toasts what went through.
pub fn sync(svc: Res<Service>, mut notes: ResMut<Notes>, mut card: ResMut<MemoryCard>, mut toasts: ResMut<Toasts>, mut seen: Local<u64>) {
    let mut s = svc.shared.lock().unwrap();
    for line in s.done.drain(..) {
        toasts.push_ok(line);
    }
    if s.memory_rev == *seen {
        return;
    }
    *seen = s.memory_rev;
    notes.list = s.memory.clone();
    notes.version += 1;
    card.notes_changed(&notes.list);
}

/// Typed `memory` (the bar sets `Open::memory`) opens the card, which has the keyboard while it is up.
pub fn commands(
    mut open: ResMut<crate::apps::Open>,
    notes: Res<Notes>,
    mut card: ResMut<MemoryCard>,
    mut focus: ResMut<Focus>,
    bar: Res<crate::cmdbar::CmdBar>,
    mut sound: ResMut<crate::sound::Sound>,
) {
    if std::mem::take(&mut open.memory) {
        card.open(&notes.list);
        sound.play_with("open", 0.7, 0.0);
    }
    if card.open {
        // the home, the swarm and the apps stand by (they skip keys while the bar has focus)
        focus.cmd = true;
        focus.terminal = false;
    } else if std::mem::take(&mut card.release) {
        focus.cmd = bar.open;
    }
}

// ---------------------------------------------------------------- keys and buttons

#[derive(Component, Clone, Copy, PartialEq, Debug)]
pub enum Btn {
    /// a note's row
    Row(u64),
    /// Remove
    Remove,
    Close,
    /// anywhere else on the screen
    Away,
}

/// ↑ ↓ and J K choose a note, D removes it (twice), Esc closes.
#[allow(clippy::too_many_arguments)]
pub fn keys(
    mut ev: MessageReader<KeyboardInput>,
    mut card: ResMut<MemoryCard>,
    notes: Res<Notes>,
    bar: Res<crate::cmdbar::CmdBar>,
    svc: Res<Service>,
    sim: Res<crate::core::Sim>,
    mut sound: ResMut<crate::sound::Sound>,
    mut toasts: ResMut<Toasts>,
) {
    for e in ev.read() {
        if e.state != ButtonState::Pressed || bar.open || !card.open {
            continue;
        }
        let moving = matches!(e.key_code, KeyCode::ArrowDown | KeyCode::ArrowUp | KeyCode::KeyJ | KeyCode::KeyK);
        if e.repeat && !moving {
            continue;
        }
        match e.key_code {
            KeyCode::Escape => {
                card.close();
                sound.play_with("close", 0.6, 0.0);
            }
            KeyCode::ArrowDown | KeyCode::KeyJ => {
                card.step(&notes.list, false);
                sound.play("tick");
            }
            KeyCode::ArrowUp | KeyCode::KeyK => {
                card.step(&notes.list, true);
                sound.play("tick");
            }
            KeyCode::KeyD => remove(&mut card, &notes.list, &svc, sim.t, &mut sound, &mut toasts),
            _ => {}
        }
    }
}

/// One press of Remove: arms the selected note, then removes it.
fn remove(card: &mut MemoryCard, notes: &[Note], svc: &Service, now: f32, sound: &mut crate::sound::Sound, toasts: &mut Toasts) {
    let was = card.arm;
    let repo = card.sel.and_then(|id| notes.iter().find(|n| n.id == id)).map(|n| n.repo.clone());
    match card.press_remove(notes, now) {
        Some(id) => {
            forget(svc, id);
            let repo = repo.flatten();
            let left = notes.iter().filter(|n| n.id != id && n.repo == repo).count();
            toasts.push_quiet(removed(repo.as_deref(), left));
            sound.play_with("discard", 0.5, 0.0);
        }
        None if was == Arm::Idle && card.arm == Arm::Armed => sound.play_with("toggle", 0.7, 0.0),
        None => {}
    }
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn buttons(
    q: Query<(&Interaction, &Btn), Changed<Interaction>>,
    mut card: ResMut<MemoryCard>,
    notes: Res<Notes>,
    svc: Res<Service>,
    sim: Res<crate::core::Sim>,
    mut sound: ResMut<crate::sound::Sound>,
    mut toasts: ResMut<Toasts>,
) {
    for (i, b) in &q {
        if !card.open {
            return;
        }
        match (i, b) {
            (Interaction::Hovered, Btn::Row(_) | Btn::Remove | Btn::Close) => sound.play("hover"),
            (Interaction::Pressed, Btn::Row(id)) => {
                card.select(&notes.list, *id);
                sound.play("tick");
            }
            (Interaction::Pressed, Btn::Remove) => remove(&mut card, &notes.list, &svc, sim.t, &mut sound, &mut toasts),
            (Interaction::Pressed, Btn::Close | Btn::Away) => {
                card.close();
                sound.play_with("close", 0.6, 0.0);
            }
            _ => {}
        }
    }
}

/// The wheel over the card chooses a note: a step to every 40 px of it, as the app orbit's does.
pub fn wheel(mut ev: MessageReader<MouseWheel>, ms: Res<crate::mouse::MouseState>, mut card: ResMut<MemoryCard>, notes: Res<Notes>, mut sound: ResMut<crate::sound::Sound>, mut acc: Local<f32>) {
    let dy: f32 = ev
        .read()
        .map(|e| match e.unit {
            MouseScrollUnit::Line => e.y * 40.0,
            MouseScrollUnit::Pixel => e.y,
        })
        .sum();
    if !card.open || !ms.pos.zip(ms.block).is_some_and(|(p, r)| r.contains(p)) {
        *acc = 0.0;
        return;
    }
    *acc += dy;
    while acc.abs() >= 40.0 {
        // up goes to the note before
        if card.scroll(&notes.list, *acc > 0.0) {
            sound.play("tick");
        }
        *acc -= acc.signum() * 40.0;
    }
}

// ---------------------------------------------------------------- the card

#[derive(Component)]
pub struct MemRoot;
#[derive(Component)]
pub struct MemBox;

const CARD_W: f32 = 560.0;

pub fn setup(mut commands: Commands, root: Res<crate::hud::OverlayRoot>, svc: Res<Service>, args: Res<crate::Args>) {
    // the whole screen: a click away from the card closes it; the card hangs in it, centred, below the strip
    let e = commands
        .spawn((
            MemRoot,
            Interaction::None,
            Btn::Away,
            bevy::ui::FocusPolicy::Pass,
            Node { position_type: PositionType::Absolute, left: px(0), right: px(0), top: px(0), bottom: px(0), justify_content: JustifyContent::Center, align_items: AlignItems::FlexStart, padding: UiRect { top: percent(17), ..default() }, ..default() },
            Visibility::Hidden,
        ))
        .id();
    commands.entity(root.0).add_child(e);
    // screenshots and demos have no sphered to ask: they get notes of their own
    if args.offline() {
        let mut s = svc.shared.lock().unwrap();
        s.memory = demo_notes();
        s.memory_rev += 1;
    }
}

/// Rebuilds the card when it changes; eases it in and out; keeps the home's mouse off it.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn draw(
    mut commands: Commands,
    card: Res<MemoryCard>,
    notes: Res<Notes>,
    f: Option<Res<UiFonts>>,
    sim: Res<crate::core::Sim>,
    mut ms: ResMut<crate::mouse::MouseState>,
    mut root: Query<(Entity, &mut Visibility), With<MemRoot>>,
    boxes: Query<(&ComputedNode, &UiGlobalTransform), With<MemBox>>,
    mut seen: Local<(u64, u64)>,
    mut amt: Local<Option<Spring>>,
    mut blocking: Local<bool>,
    win: Query<&Window, With<bevy::window::PrimaryWindow>>,
    mut tfs: Query<&mut UiTransform, With<MemBox>>,
) {
    let Some(f) = f else { return };
    let Ok((rootent, mut vis)) = root.single_mut() else { return };
    let want = card.open;
    let amt = amt.get_or_insert_with(|| Spring::new(0.0, 80.0, 18.0));
    amt.t = if want { 1.0 } else { 0.0 };
    amt.step(crate::motion::dt(crate::motion::Pace::Ui, sim.dt));
    if amt.x < 0.01 && !want {
        vis.set_if_neq(Visibility::Hidden);
        // (the pointer is the take-over card's while that one is up: only what this card set is let go)
        if std::mem::take(&mut *blocking) {
            ms.block = None;
        }
        *seen = (0, 0);
        return;
    }
    vis.set_if_neq(Visibility::Inherited);
    if *seen != (card.version, notes.version) {
        *seen = (card.version, notes.version);
        commands.entity(rootent).despawn_children();
        if card.open {
            let home = std::env::var("HOME").unwrap_or_default();
            build_card(&mut commands, rootent, &f, &card, &notes.list, &home, &|n| n.age_secs());
        }
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
    *blocking = ms.block.is_some();
}

/// A button or row whose fill follows the pointer: `rest` without it, `hot` with it.
#[derive(Component)]
pub struct Hover {
    rest: Color,
    hot: Color,
}

#[allow(clippy::type_complexity)]
pub fn hover_look(mut q: Query<(&Interaction, &Hover, &mut BackgroundColor), Changed<Interaction>>) {
    for (i, h, mut bg) in &mut q {
        let hovered = matches!(i, Interaction::Hovered | Interaction::Pressed);
        bg.set_if_neq(BackgroundColor(if hovered { h.hot } else { h.rest }));
    }
}

/// Builds the card: a header, the notes by repo (a window of them when there are many), the buttons and the keys.
/// `age` says how old a note is, in seconds.
fn build_card(c: &mut Commands, root: Entity, f: &UiFonts, card: &MemoryCard, notes: &[Note], home: &str, age: &dyn Fn(&Note) -> u64) {
    let line = role_color(Role::Line);
    let hot = role_color(Role::Hot);
    let fill = role_color(Role::Fill);
    let acc = role_color(Role::Acc);
    // the small print: legible over the core behind the glass, and brighter on the selected note
    let faint = with_alpha(line, 0.58);
    let nowrap = TextLayout { linebreak: LineBreak::NoWrap, ..default() };
    let bx = c
        .spawn((
            MemBox,
            Interaction::None,
            bevy::ui::FocusPolicy::Block,
            Node {
                width: px(CARD_W),
                flex_direction: FlexDirection::Column,
                row_gap: px(6),
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
    // header
    let hdr = c
        .spawn((Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(9), padding: UiRect { bottom: px(8), ..default() }, border: UiRect { bottom: px(1), ..default() }, ..default() }, BorderColor::all(with_alpha(line, 0.72))))
        .id();
    let sq = c.spawn((Node { width: px(6), height: px(6), ..default() }, BackgroundColor(fill))).id();
    let head = if notes.is_empty() { "MEMORY".to_string() } else { format!("MEMORY · {}", notes_word(notes.len()).to_uppercase()) };
    let t1 = c.spawn((Text::new(head), font(&f.ui600, 12.0, 3.84), TextColor(hot), LineHeight::Px(14.4))).id();
    c.entity(hdr).add_children(&[sq, t1]);
    c.entity(bx).add_child(hdr);
    let sub = c.spawn((Text::new("Every agent starts with these: its repo's notes, then the ones for all repos."), font(&f.ui500, 13.0, 1.04), TextColor(line), LineHeight::Px(16.9), Node { margin: UiRect { bottom: px(4), ..default() }, ..default() })).id();
    c.entity(bx).add_child(sub);

    if notes.is_empty() {
        let none = c.spawn((Text::new("Nothing remembered yet."), font(&f.ui500, 15.0, 0.3), TextColor(hot), LineHeight::Px(20.0), Node { margin: UiRect { top: px(8), ..default() }, ..default() })).id();
        let how = c
            .spawn((Text::new("Type  remember: use pnpm, never npm  in the command bar, or press R after you send a change."), font(&f.ui500, 13.0, 0.52), TextColor(with_alpha(line, 0.85)), LineHeight::Px(17.0), Node { margin: UiRect { bottom: px(8), ..default() }, ..default() }))
            .id();
        c.entity(bx).add_children(&[none, how]);
    }
    let all = lines(notes);
    let shown = visible(&all, card.sel);
    if shown.start > 0 {
        let hidden = all[..shown.start].iter().filter(|l| matches!(l, Line::Note(_))).count();
        let more = more_line(c, f, format!("↑ {hidden} MORE"));
        c.entity(bx).add_child(more);
    }
    let mut first = true;
    for l in &all[shown.clone()] {
        match l {
            Line::Head(repo, count) => {
                let (name, count, place) = heading(*repo, *count, home);
                let row = c
                    .spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Baseline, column_gap: px(10), margin: UiRect { top: px(if first { 4.0 } else { 10.0 }), ..default() }, ..default() })
                    .id();
                let n = c.spawn((Text::new(name), font(&f.ui600, 12.5, 2.5), TextColor(hot), LineHeight::Px(15.0), nowrap)).id();
                let k = c.spawn((Text::new(count), font(&f.ui700, 9.5, 1.9), TextColor(faint), LineHeight::Px(10.0), nowrap)).id();
                let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
                let p = c.spawn((Text::new(place), font(&f.mono, 10.5, 0.0), TextColor(faint), LineHeight::Px(12.0), nowrap)).id();
                c.entity(row).add_children(&[n, k, sp, p]);
                c.entity(bx).add_child(row);
            }
            Line::Note(n) => {
                let on = card.sel == Some(n.id);
                let armed = on && card.arm == Arm::Armed;
                let edge = if armed { acc } else if on { fill } else { Color::NONE };
                let bg = if armed { with_alpha(acc, 0.1) } else if on { with_alpha(line, 0.08) } else { Color::NONE };
                let under_pointer = if armed { with_alpha(acc, 0.16) } else { with_alpha(line, if on { 0.12 } else { 0.05 }) };
                let row = c
                    .spawn((
                        Btn::Row(n.id),
                        Hover { rest: bg, hot: under_pointer },
                        Interaction::None,
                        bevy::ui::FocusPolicy::Block,
                        Node { flex_direction: FlexDirection::Column, row_gap: px(3), padding: UiRect { left: px(10), right: px(10), top: px(6), bottom: px(6) }, border: UiRect { left: px(2), ..default() }, ..default() },
                        BackgroundColor(bg),
                        BorderColor::all(edge),
                    ))
                    .id();
                // the selected note in full; the others on one line
                let text = if on {
                    c.spawn((Text::new(n.text.clone()), font(&f.ui500, 14.0, 0.28), TextColor(hot), LineHeight::Px(19.0))).id()
                } else {
                    c.spawn((Text::new(crate::appframe::clip(&n.text, 66)), font(&f.ui500, 14.0, 0.28), TextColor(with_alpha(hot, 0.88)), LineHeight::Px(19.0), nowrap, Node { width: percent(100), overflow: Overflow::clip(), ..default() })).id()
                };
                let meta = if armed { "Press D again to remove".to_string() } else { meta_line(n, age(n)) };
                let m = c.spawn((Text::new(meta), font(&f.ui500, 11.5, 0.9), TextColor(if armed { acc } else if on { with_alpha(line, 0.85) } else { faint }), LineHeight::Px(14.0), nowrap)).id();
                c.entity(row).add_children(&[text, m]);
                c.entity(bx).add_child(row);
            }
        }
        first = false;
    }
    if shown.end < all.len() {
        let hidden = all[shown.end..].iter().filter(|l| matches!(l, Line::Note(_))).count();
        let more = more_line(c, f, format!("↓ {hidden} MORE"));
        c.entity(bx).add_child(more);
    }

    // the buttons, and the keys in the corner
    let foot = c
        .spawn((Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(10), margin: UiRect { top: px(6), ..default() }, padding: UiRect { top: px(12), ..default() }, border: UiRect { top: px(1), ..default() }, ..default() }, BorderColor::all(with_alpha(line, 0.13))))
        .id();
    let armed = card.arm == Arm::Armed;
    if !notes.is_empty() {
        let (label, border, bg, col) = if armed { ("Press again to remove", acc, with_alpha(acc, 0.16), hot) } else { ("Remove", with_alpha(line, 0.34), Color::NONE, hot) };
        let b = button(c, f, label, Btn::Remove, border, bg, col);
        c.entity(foot).add_child(b);
    }
    let b = button(c, f, "Close", Btn::Close, Color::NONE, Color::NONE, line);
    c.entity(foot).add_child(b);
    let keys = c
        .spawn((Node { margin: UiRect { left: auto(), ..default() }, padding: UiRect { left: px(7), right: px(7), top: px(5), bottom: px(4) }, border: UiRect::all(px(1)), ..default() }, BorderColor::all(with_alpha(line, 0.13))))
        .id();
    let kt = c
        .spawn((Text::new(if notes.is_empty() { "ESC" } else { "↑ ↓ · J K · D D · ESC" }), font(&f.ui700, 9.5, 1.9), TextColor(with_alpha(line, 0.62)), LineHeight::Px(9.5)))
        .id();
    c.entity(keys).add_child(kt);
    c.entity(foot).add_child(keys);
    c.entity(bx).add_child(foot);
}

/// "↑ 3 MORE" / "↓ 5 MORE" at the edge of a list that goes on.
fn more_line(c: &mut Commands, f: &UiFonts, text: String) -> Entity {
    c.spawn((Text::new(text), font(&f.ui700, 9.5, 1.9), TextColor(with_alpha(role_color(Role::Line), 0.58)), LineHeight::Px(10.0), Node { margin: UiRect { left: px(12), ..default() }, ..default() })).id()
}

/// An outlined button of the card.
fn button(c: &mut Commands, f: &UiFonts, label: &str, btn: Btn, border: Color, bg: Color, col: Color) -> Entity {
    let b = c
        .spawn((
            btn,
            Hover { rest: bg, hot: if bg == Color::NONE { with_alpha(role_color(Role::Line), 0.1) } else { with_alpha(role_color(Role::Acc), 0.26) } },
            Interaction::None,
            bevy::ui::FocusPolicy::Block,
            Node { padding: UiRect { left: px(14), right: px(14), top: px(10), bottom: px(9) }, border: UiRect::all(px(1)), ..default() },
            BackgroundColor(bg),
            BorderColor::all(border),
        ))
        .id();
    let t = c.spawn((Text::new(label.to_uppercase()), font(&f.ui700, 10.5, 1.5), TextColor(col), LineHeight::Px(12.0))).id();
    c.entity(b).add_child(t);
    b
}

// ---------------------------------------------------------------- look tests

/// The look test's notes, newest first: six across two repos and "all repos", written over the last two weeks.
pub fn demo_notes() -> Vec<Note> {
    let now = crate::core::unix_now();
    let note = |id: u64, repo: Option<&str>, text: &str, source: NoteSource, ago_secs: i64| Note {
        id,
        repo: repo.map(str::to_string),
        text: text.to_string(),
        created_at: sphere_proto::rfc3339_from_unix_ms(((now - ago_secs) * 1000) as u64),
        source,
    };
    const H: i64 = 3600;
    const D: i64 = 24 * H;
    let mut notes = vec![
        note(6, Some("/demo/web-app"), "Run pnpm lint before every commit.", NoteSource::Typed, 3 * H),
        note(5, Some("/demo/api-server"), "Never edit src/gen by hand: change the proto and run make gen.", NoteSource::Cli, D + 2 * H),
        note(4, Some("/demo/web-app"), "Tests sit next to the code as *.test.ts, not in a top-level tests folder.", NoteSource::Change, 2 * D),
        note(3, Some("/demo/api-server"), "Migrations go in db/migrations with a timestamp prefix.", NoteSource::Typed, 4 * D),
        note(2, Some("/demo/web-app"), "Use pnpm, never npm.", NoteSource::Change, 6 * D),
        note(1, None, "Keep commits small, one logical change each, with a subject in plain words.", NoteSource::Typed, 12 * D),
    ];
    notes.sort_by_key(|n| std::cmp::Reverse(n.id));
    notes
}

/// `--memory card|armed|chip`: the look test opens the card of notes (with the first one armed for removal, in `armed`),
/// or sends a Change on the Agent moment card and leaves the chip that offers to remember it up.
#[allow(clippy::too_many_arguments)]
pub fn script(
    args: Res<crate::Args>,
    sim: Res<crate::core::Sim>,
    svc: Res<Service>,
    notes: Res<Notes>,
    mut card: ResMut<MemoryCard>,
    mut m: ResMut<crate::moment::Moment>,
    live: Res<crate::live::Live>,
) {
    let Some(stage) = args.memory.as_deref() else { return };
    if card.scripted {
        // the screenshot waits some seconds: the chip stays for it
        if stage == "chip" {
            crate::moment::hold_offer(&mut m);
        }
        return;
    }
    if sim.t < 1.0 {
        return;
    }
    match stage {
        "card" | "armed" => {
            card.open(&notes.list);
            if stage == "armed" {
                card.press_remove(&notes.list, sim.t - 1.0);
            }
        }
        "chip" => {
            // the card rises with the first frames; type the change once it is up
            if !m.shown || m.current.is_none() {
                return;
            }
            crate::moment::change(&mut m, &svc, &live, "Use pnpm instead of npm");
        }
        _ => {}
    }
    card.scripted = true;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmdbar::CmdBar;
    use crate::live::ToastKind;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::input::keyboard::Key;
    use tokio::sync::mpsc::UnboundedReceiver;

    fn note(id: u64, repo: Option<&str>, text: &str, source: NoteSource) -> Note {
        Note { id, repo: repo.map(str::to_string), text: text.into(), created_at: "2026-10-19T00:00:00.000Z".into(), source }
    }

    /// Six notes, newest first: sphere's 6, 4 and 1, api's 5 and 2, and 3 for every repo.
    fn six() -> Vec<Note> {
        vec![
            note(6, Some("/code/sphere"), "Run cargo fmt before committing.", NoteSource::Typed),
            note(5, Some("/code/api"), "Never edit src/gen by hand.", NoteSource::Cli),
            note(4, Some("/code/sphere"), "Use pnpm, never npm.", NoteSource::Change),
            note(3, None, "Keep commits small.", NoteSource::Typed),
            note(2, Some("/code/api"), "Migrations go in db/migrations.", NoteSource::Typed),
            note(1, Some("/code/sphere"), "Answer in plain words.", NoteSource::Typed),
        ]
    }

    fn ids(list: &[&Note]) -> Vec<u64> {
        list.iter().map(|n| n.id).collect()
    }

    fn added(repo: Option<&str>, text: &str, added: bool, count: usize) -> MemoryAdded {
        MemoryAdded { note: note(9, repo, text, NoteSource::Typed), added, count }
    }

    #[test]
    fn the_toast_after_a_note_is_kept_says_whose_and_how_many() {
        assert_eq!(remembered(&added(Some("/Users/me/code/sphere"), "x", true, 4)), "Remembered for sphere · 4 notes");
        assert_eq!(remembered(&added(Some("/code/sphere"), "x", true, 1)), "Remembered for sphere · 1 note");
        assert_eq!(remembered(&added(None, "x", true, 2)), "Remembered for all repos · 2 notes");
        assert_eq!(remembered(&added(Some("/code/sphere"), "x", false, 4)), "Already remembered for sphere");
        assert_eq!(removed(Some("/code/sphere"), 2), "Removed from sphere · 2 notes left");
        assert_eq!(removed(Some("/code/sphere"), 1), "Removed from sphere · 1 note left");
        assert_eq!(removed(None, 0), "Removed from all repos · none left");
    }

    #[test]
    fn a_note_says_where_it_came_from_and_how_old_it_is() {
        let n = note(1, None, "x", NoteSource::Change);
        assert_eq!(meta_line(&n, 3 * 86400), "3d ago · a change you sent");
        assert_eq!(meta_line(&note(1, None, "x", NoteSource::Typed), 10), "just now · typed");
        assert_eq!(meta_line(&note(1, None, "x", NoteSource::Cli), 3 * 3600), "3h ago · the CLI");
    }

    #[test]
    fn a_heading_has_the_repos_name_its_count_and_where_it_is() {
        assert_eq!(heading(Some("/Users/me/code/sphere"), 3, "/Users/me"), ("SPHERE".into(), "3 NOTES".into(), "~/code/sphere".into()));
        assert_eq!(heading(None, 1, "/Users/me"), ("ALL REPOS".into(), "1 NOTE".into(), "every agent".into()));
        // a path outside home is whole, a long one is cut at its front, and a home that is only a prefix of a name is not home
        assert_eq!(short_path("/work/api", "/Users/me"), "/work/api");
        assert_eq!(short_path("/Users/mel/api", "/Users/me"), "/Users/mel/api");
        assert_eq!(short_path("/Users/me", "/Users/me"), "~");
        let long = short_path("/Users/me/code/a-very-long-folder-name/with/more/folders/in/it/repo", "/Users/me");
        assert!(long.starts_with('…') && long.ends_with("folders/in/it/repo") && long.chars().count() <= 34, "{long}");
        assert_eq!(short_path("/Users/me/x", ""), "/Users/me/x");
    }

    #[test]
    fn memory_and_remember_are_read_from_the_bar() {
        let r = |to, note: &str| Some(Typed::Remember { to, note: note.into() });
        assert_eq!(parse("memory"), Some(Typed::Show));
        assert_eq!(parse("  Memory "), Some(Typed::Show));
        assert_eq!(parse("notes"), Some(Typed::Show));
        assert_eq!(parse("remember: use pnpm, never npm"), r(To::Usual, "use pnpm, never npm"));
        assert_eq!(parse("Remember:  use pnpm "), r(To::Usual, "use pnpm"));
        assert_eq!(parse("remember sphere: use pnpm"), r(To::Repo("sphere".into()), "use pnpm"));
        assert_eq!(parse("remember ~/code/sphere: use pnpm"), r(To::Repo("~/code/sphere".into()), "use pnpm"));
        assert_eq!(parse("remember all: answer in plain words"), r(To::All, "answer in plain words"));
        assert_eq!(parse("REMEMBER ALL: x"), r(To::All, "x"));
        // a colon in the note is part of it
        assert_eq!(parse("remember: run it as: make test"), r(To::Usual, "run it as: make test"));
        // nothing to remember is still a remember (the bar says what to type)
        assert_eq!(parse("remember:"), r(To::Usual, ""));
        assert_eq!(parse("remember all:"), r(To::All, ""));
        assert_eq!(parse("remember"), r(To::Usual, ""));
    }

    #[test]
    fn other_words_are_left_to_the_bar_as_a_task() {
        for t in [
            "remember to update the docs",
            "remember to update the docs: and the changelog",
            "remembering: x",
            "rememberme: x",
            "remember sphere use pnpm",
            "memory leak in the parser",
            "notes on the release",
            "fix the memory: it grows",
            "remember two words: x",
        ] {
            assert_eq!(parse(t), None, "{t}");
        }
    }

    #[test]
    fn the_card_lists_by_repo_newest_first_with_all_repos_last() {
        let notes = six();
        assert_eq!(ids(&order(&notes)), [6, 4, 1, 5, 2, 3]);
        let shape: Vec<String> = lines(&notes)
            .iter()
            .map(|l| match l {
                Line::Head(r, n) => format!("{} ({n})", r.unwrap_or("all")),
                Line::Note(n) => n.id.to_string(),
            })
            .collect();
        assert_eq!(shape, ["/code/sphere (3)", "6", "4", "1", "/code/api (2)", "5", "2", "all (1)", "3"]);
        assert!(order(&[]).is_empty() && lines(&[]).is_empty());
    }

    #[test]
    fn the_selection_moves_through_the_notes_and_round() {
        let notes = six();
        let mut c = MemoryCard::default();
        c.open(&notes);
        assert_eq!(c.sel, Some(6));
        for want in [4, 1, 5, 2, 3, 6] {
            c.step(&notes, false);
            assert_eq!(c.sel, Some(want));
        }
        c.step(&notes, true);
        assert_eq!(c.sel, Some(3), "back from the first wraps to the last");
        // a click on a row selects it; an empty list selects nothing
        c.select(&notes, 5);
        assert_eq!(c.sel, Some(5));
        let mut e = MemoryCard::default();
        e.open(&[]);
        e.step(&[], false);
        assert_eq!(e.sel, None);
        assert_eq!(e.press_remove(&[], 1.0), None);
    }

    #[test]
    fn the_wheel_stops_at_the_ends_where_the_arrows_go_round() {
        let notes = six();
        let mut c = MemoryCard::default();
        c.open(&notes);
        assert!(!c.scroll(&notes, true) && c.sel == Some(6), "already at the first");
        assert!(c.scroll(&notes, false) && c.sel == Some(4));
        c.press_remove(&notes, 1.0);
        assert!(c.scroll(&notes, true) && c.sel == Some(6) && c.arm == Arm::Idle, "moving disarms");
        for _ in 0..10 {
            c.scroll(&notes, false);
        }
        assert_eq!(c.sel, Some(3), "the last one, and no further");
        assert!(!c.scroll(&notes, false));
        c.step(&notes, false);
        assert_eq!(c.sel, Some(6), "the arrows go round");
        assert!(!MemoryCard::default().scroll(&[], false), "nothing to move through");
    }

    #[test]
    fn removing_takes_two_presses_and_the_note_below_is_selected_after() {
        let notes = six();
        let mut c = MemoryCard::default();
        c.open(&notes);
        c.step(&notes, false);
        assert_eq!(c.sel, Some(4));
        // the first press arms, the same click twice is not a decision, the second press after a moment removes
        assert_eq!(c.press_remove(&notes, 1.0), None);
        assert_eq!(c.arm, Arm::Armed);
        assert_eq!(c.press_remove(&notes, 1.1), None);
        assert_eq!(c.press_remove(&notes, 2.0), Some(4));
        assert_eq!((c.arm, c.sel), (Arm::Idle, Some(1)));
        // the last note of the list: the one above is selected
        let mut c = MemoryCard::default();
        c.open(&notes);
        c.step(&notes, true);
        assert_eq!(c.sel, Some(3));
        c.press_remove(&notes, 1.0);
        assert_eq!(c.press_remove(&notes, 2.0), Some(3));
        assert_eq!(c.sel, Some(2));
    }

    #[test]
    fn moving_or_clicking_another_row_disarms_remove_and_a_new_card_starts_unarmed() {
        let notes = six();
        let mut c = MemoryCard::default();
        c.open(&notes);
        c.press_remove(&notes, 1.0);
        assert_eq!(c.arm, Arm::Armed);
        c.step(&notes, false);
        assert_eq!(c.arm, Arm::Idle);
        c.press_remove(&notes, 5.0);
        c.select(&notes, 4);
        assert_eq!(c.arm, Arm::Armed, "the armed row clicked again stays armed");
        c.select(&notes, 1);
        assert_eq!(c.arm, Arm::Idle);
        c.press_remove(&notes, 9.0);
        c.close();
        assert!(!c.open && c.arm == Arm::Idle);
        c.open(&notes);
        assert_eq!(c.arm, Arm::Idle);
    }

    #[test]
    fn the_selection_stays_on_its_note_when_the_list_changes_under_the_card() {
        let mut notes = six();
        let mut c = MemoryCard::default();
        c.open(&notes);
        c.select(&notes, 5);
        c.press_remove(&notes, 1.0);
        // a new note elsewhere: the selection and the armed button stay
        notes.insert(0, note(7, Some("/code/web"), "x", NoteSource::Typed));
        c.notes_changed(&notes);
        assert_eq!((c.sel, c.arm), (Some(5), Arm::Armed));
        // its note removed elsewhere: the one in its place is selected, disarmed
        notes.retain(|n| n.id != 5);
        c.notes_changed(&notes);
        assert_ne!(c.sel, Some(5));
        assert!(c.sel.is_some() && c.arm == Arm::Idle);
        // everything gone
        c.notes_changed(&[]);
        assert_eq!(c.sel, None);
    }

    #[test]
    fn a_long_list_shows_a_window_that_keeps_the_selected_note_in_view() {
        let notes: Vec<Note> = (1..=30).map(|i| note(i, Some(if i % 2 == 0 { "/code/a" } else { "/code/b" }), "x", NoteSource::Typed)).collect();
        let all = lines(&notes);
        assert_eq!(all.len(), 32);
        for n in order(&notes) {
            let w = visible(&all, Some(n.id));
            assert_eq!(w.len(), SHOWN);
            assert!(all[w.clone()].iter().any(|l| matches!(l, Line::Note(x) if x.id == n.id)), "note {} is in {w:?}", n.id);
            assert!(w.end <= all.len());
        }
        // the first note starts at the top, with its heading; the last ends at the bottom
        assert_eq!(visible(&all, Some(order(&notes)[0].id)).start, 0);
        assert_eq!(visible(&all, Some(order(&notes)[29].id)).end, 32);
        // a short list is shown whole
        assert_eq!(visible(&lines(&six()), Some(6)), 0..9);
    }

    // ---- typed commands and the shared slot

    fn toasts_of(t: &Toasts) -> Vec<(String, ToastKind)> {
        t.list.iter().map(|x| (x.0.clone(), x.2)).collect()
    }

    /// A note sent to be kept: its repo, its words and where it came from.
    type Told = (Option<String>, String, NoteSource);

    /// The commands sent so far: each Remember, and the id of each Forget.
    fn drained(rx: &mut UnboundedReceiver<Cmd>) -> (Vec<Told>, Vec<u64>) {
        let (mut remembered, mut forgotten) = (vec![], vec![]);
        while let Ok(c) = rx.try_recv() {
            match c {
                Cmd::Remember { repo, text, source } => remembered.push((repo, text, source)),
                Cmd::Forget { id } => forgotten.push(id),
                _ => {}
            }
        }
        (remembered, forgotten)
    }

    #[test]
    fn a_note_typed_for_all_repos_goes_to_sphered_cleaned_up() {
        let (svc, mut rx) = Service::with_receiver();
        let (mut toasts, mut open) = (Toasts::default(), crate::apps::Open::default());
        assert!(command("remember all:   answer in\n plain words ", &svc, &mut toasts, &mut open));
        assert_eq!(drained(&mut rx).0, [(None, "answer in plain words".to_string(), NoteSource::Typed)]);
        // the toast comes when sphered has kept it, not before
        assert!(toasts.list.is_empty() && !open.memory);
        assert!(command("memory", &svc, &mut toasts, &mut open) && open.memory);
        // a task is not ours
        assert!(!command("fix the login test", &svc, &mut toasts, &mut open));
        assert!(drained(&mut rx).0.is_empty());
    }

    #[test]
    fn nothing_to_remember_and_a_repo_that_is_not_one_are_said() {
        let (svc, mut rx) = Service::with_receiver();
        let (mut toasts, mut open) = (Toasts::default(), crate::apps::Open::default());
        assert!(command("remember all:", &svc, &mut toasts, &mut open));
        assert!(command("remember:   ", &svc, &mut toasts, &mut open));
        assert!(command("remember definitely-not-a-repo-9f3a: use pnpm", &svc, &mut toasts, &mut open));
        let said: Vec<String> = toasts.list.iter().map(|t| t.0.clone()).collect();
        assert!(said[0].starts_with("Nothing to remember") && said[1].starts_with("Nothing to remember"), "{said:?}");
        assert!(said[2].starts_with("No git repo at ") && said[2].ends_with("Projects/definitely-not-a-repo-9f3a"), "{said:?}");
        assert!(drained(&mut rx).0.is_empty(), "nothing was sent");
    }

    #[test]
    fn with_no_sphered_the_app_keeps_the_notes_itself() {
        let (mut svc, mut rx) = Service::with_receiver();
        svc.offline = true;
        let mut toasts = Toasts::default();
        save(&svc, &mut toasts, Some("/code/sphere"), "Use pnpm,\n never npm", NoteSource::Change);
        save(&svc, &mut toasts, Some("/code/sphere"), "use pnpm, never npm ", NoteSource::Typed);
        save(&svc, &mut toasts, Some("/code/sphere"), "Use pnpm, never npm", NoteSource::Typed);
        save(&svc, &mut toasts, None, "Keep commits small.", NoteSource::Typed);
        save(&svc, &mut toasts, None, "  ", NoteSource::Typed);
        assert_eq!(
            toasts_of(&toasts),
            [
                ("Already remembered for sphere".to_string(), ToastKind::Ok),
                ("Remembered for all repos · 1 note".to_string(), ToastKind::Ok),
                ("Nothing to remember".to_string(), ToastKind::Plain)
            ],
            "the list keeps the last three"
        );
        let s = svc.shared.lock().unwrap();
        let said: Vec<(u64, Option<&str>, &str, NoteSource)> = s.memory.iter().map(|n| (n.id, n.repo.as_deref(), n.text.as_str(), n.source)).collect();
        // newest first; the same words in another case are another note, and the repeat of the first is not kept again
        assert_eq!(
            said,
            [
                (3, None, "Keep commits small.", NoteSource::Typed),
                (2, Some("/code/sphere"), "use pnpm, never npm", NoteSource::Typed),
                (1, Some("/code/sphere"), "Use pnpm, never npm", NoteSource::Change)
            ]
        );
        assert_eq!(s.memory_rev, 3, "each note kept moved the list");
        assert!(drained(&mut rx).0.is_empty(), "a demo sends nothing");
        drop(s);
        forget(&svc, 1);
        assert_eq!(svc.shared.lock().unwrap().memory.iter().map(|n| n.id).collect::<Vec<_>>(), [3, 2]);
    }

    #[test]
    fn a_full_memory_says_so_and_keeps_nothing_more() {
        let mut s = Shared::default();
        for i in 0..NOTES_MAX {
            keep_here(&mut s, Some("/r"), &format!("note {i}"), NoteSource::Typed).unwrap();
        }
        let e = keep_here(&mut s, Some("/r"), "one more", NoteSource::Typed).unwrap_err();
        assert!(e.contains("full") && e.contains("200"), "{e}");
        // a repeat is still just a repeat
        assert!(!keep_here(&mut s, Some("/r"), "note 3", NoteSource::Typed).unwrap().added);
        assert_eq!(s.memory.len(), NOTES_MAX);
    }

    #[test]
    fn notes_reach_the_card_and_what_went_through_is_toasted_once() {
        let (svc, _rx) = Service::with_receiver();
        let mut app = App::new();
        app.init_resource::<Notes>().init_resource::<MemoryCard>().init_resource::<Toasts>().insert_resource(svc.clone()).add_systems(Update, sync);
        app.update();
        assert!(app.world().resource::<Notes>().list.is_empty());
        {
            let mut s = svc.shared.lock().unwrap();
            s.memory = six();
            s.memory_rev = 1;
            s.done.push("Remembered for sphere · 3 notes".into());
        }
        app.update();
        assert_eq!(app.world().resource::<Notes>().list.len(), 6);
        assert_eq!(toasts_of(app.world().resource::<Toasts>()), [("Remembered for sphere · 3 notes".to_string(), ToastKind::Ok)]);
        let v = app.world().resource::<Notes>().version;
        app.update();
        assert_eq!((app.world().resource::<Notes>().version, app.world().resource::<Toasts>().list.len()), (v, 1), "nothing new, nothing done");
    }

    // ---- the keys and the buttons, on a bare App

    /// A bare App with the card open on `six`, the service's commands readable, and the keys running.
    fn open_card() -> (App, UnboundedReceiver<Cmd>) {
        let (svc, rx) = Service::with_receiver();
        let mut app = App::new();
        app.add_message::<KeyboardInput>()
            .init_resource::<Notes>()
            .init_resource::<MemoryCard>()
            .init_resource::<CmdBar>()
            .init_resource::<Toasts>()
            .init_resource::<crate::sound::Sound>()
            .insert_resource(crate::core::Sim { dt: 0.05, t: 10.0, boot_t: 99.0, fixed_dt: None, frozen: false })
            .insert_resource(svc)
            .add_systems(Update, (keys, buttons).chain());
        app.world_mut().resource_mut::<Notes>().list = six();
        let list = six();
        app.world_mut().resource_mut::<MemoryCard>().open(&list);
        (app, rx)
    }

    fn press(app: &mut App, code: KeyCode, repeat: bool) {
        app.world_mut().write_message(KeyboardInput { key_code: code, logical_key: Key::Unidentified(bevy::input::keyboard::NativeKey::Unidentified), state: ButtonState::Pressed, text: None, repeat, window: Entity::PLACEHOLDER });
        app.update();
    }

    fn at(app: &mut App, t: f32) {
        app.world_mut().resource_mut::<crate::core::Sim>().t = t;
    }

    fn card_of(app: &App) -> &MemoryCard {
        app.world().resource::<MemoryCard>()
    }

    #[test]
    fn the_arrows_and_j_k_choose_a_note() {
        let (mut app, _rx) = open_card();
        press(&mut app, KeyCode::ArrowDown, false);
        press(&mut app, KeyCode::KeyJ, false);
        assert_eq!(card_of(&app).sel, Some(1));
        press(&mut app, KeyCode::ArrowUp, false);
        press(&mut app, KeyCode::KeyK, false);
        assert_eq!(card_of(&app).sel, Some(6));
        // a held arrow keeps moving
        press(&mut app, KeyCode::ArrowDown, true);
        assert_eq!(card_of(&app).sel, Some(4));
    }

    #[test]
    fn d_twice_removes_the_selected_note_and_says_so() {
        let (mut app, mut rx) = open_card();
        press(&mut app, KeyCode::ArrowDown, false);
        at(&mut app, 20.0);
        press(&mut app, KeyCode::KeyD, false);
        assert_eq!(card_of(&app).arm, Arm::Armed);
        assert!(drained(&mut rx).1.is_empty(), "the first press only arms");
        // a held D is not a second press
        press(&mut app, KeyCode::KeyD, true);
        assert!(drained(&mut rx).1.is_empty());
        at(&mut app, 21.0);
        press(&mut app, KeyCode::KeyD, false);
        assert_eq!(drained(&mut rx).1, [4]);
        assert_eq!(card_of(&app).arm, Arm::Idle);
        assert_eq!(toasts_of(app.world().resource::<Toasts>()), [("Removed from sphere · 2 notes left".to_string(), ToastKind::Plain)]);
        // the next D arms the next note again
        press(&mut app, KeyCode::KeyD, false);
        assert_eq!((card_of(&app).arm, card_of(&app).sel), (Arm::Armed, Some(1)));
    }

    #[test]
    fn esc_closes_the_card_and_disarms_it() {
        let (mut app, mut rx) = open_card();
        press(&mut app, KeyCode::KeyD, false);
        press(&mut app, KeyCode::Escape, false);
        assert!(!card_of(&app).open && card_of(&app).arm == Arm::Idle);
        // closed: the keys are not the card's
        press(&mut app, KeyCode::KeyD, false);
        press(&mut app, KeyCode::KeyD, false);
        assert!(drained(&mut rx).1.is_empty());
    }

    #[test]
    fn the_bar_has_the_keys_while_it_is_open() {
        let (mut app, mut rx) = open_card();
        app.world_mut().resource_mut::<CmdBar>().open = true;
        press(&mut app, KeyCode::ArrowDown, false);
        press(&mut app, KeyCode::Escape, false);
        assert!(card_of(&app).open && card_of(&app).sel == Some(6));
        assert!(drained(&mut rx).1.is_empty());
    }

    #[test]
    fn the_buttons_and_rows_do_what_the_keys_do() {
        let (mut app, mut rx) = open_card();
        let press_btn = |app: &mut App, b: Btn| {
            app.world_mut().spawn((Interaction::Pressed, b));
            app.update();
        };
        press_btn(&mut app, Btn::Row(5));
        assert_eq!(card_of(&app).sel, Some(5));
        at(&mut app, 30.0);
        press_btn(&mut app, Btn::Remove);
        assert_eq!(card_of(&app).arm, Arm::Armed);
        at(&mut app, 31.0);
        press_btn(&mut app, Btn::Remove);
        assert_eq!(drained(&mut rx).1, [5]);
        press_btn(&mut app, Btn::Close);
        assert!(!card_of(&app).open);
        // away closes it too, from a card opened again
        let list = six();
        app.world_mut().resource_mut::<MemoryCard>().open(&list);
        press_btn(&mut app, Btn::Away);
        assert!(!card_of(&app).open);
    }

    #[test]
    fn a_button_or_row_takes_its_hot_fill_under_the_pointer_and_its_rest_fill_after() {
        let mut app = App::new();
        app.add_systems(Update, hover_look);
        let line = role_color(Role::Line);
        let (rest, hot) = (with_alpha(line, 0.08), with_alpha(line, 0.12));
        let e = app.world_mut().spawn((Interaction::None, Hover { rest, hot }, BackgroundColor(rest))).id();
        let fill = |app: &App| app.world().entity(e).get::<BackgroundColor>().unwrap().0;
        app.update();
        assert_eq!(fill(&app), rest);
        app.world_mut().entity_mut(e).insert(Interaction::Hovered);
        app.update();
        assert_eq!(fill(&app), hot);
        app.world_mut().entity_mut(e).insert(Interaction::None);
        app.update();
        assert_eq!(fill(&app), rest);
    }

    #[test]
    fn the_wheel_over_the_card_chooses_a_note_and_elsewhere_does_nothing() {
        use crate::mouse::MouseState;
        let (mut app, _rx) = open_card();
        app.init_resource::<MouseState>().add_message::<MouseWheel>().add_systems(Update, wheel);
        let turn = |app: &mut App, unit, y| {
            app.world_mut().write_message(MouseWheel { unit, x: 0.0, y, window: Entity::PLACEHOLDER, phase: bevy::input::touch::TouchPhase::Moved });
            app.update();
        };
        {
            let mut ms = app.world_mut().resource_mut::<MouseState>();
            ms.block = Some(Rect::new(100.0, 100.0, 400.0, 500.0));
            ms.pos = Some(Vec2::new(20.0, 20.0));
        }
        turn(&mut app, MouseScrollUnit::Line, -1.0);
        assert_eq!(card_of(&app).sel, Some(6), "the pointer is not over the card");
        app.world_mut().resource_mut::<MouseState>().pos = Some(Vec2::new(200.0, 200.0));
        // a line is 40 px: down goes to the next note, up to the one before
        turn(&mut app, MouseScrollUnit::Line, -1.0);
        assert_eq!(card_of(&app).sel, Some(4));
        turn(&mut app, MouseScrollUnit::Line, 1.0);
        assert_eq!(card_of(&app).sel, Some(6));
        // pixels add up to a step
        turn(&mut app, MouseScrollUnit::Pixel, -25.0);
        assert_eq!(card_of(&app).sel, Some(6));
        turn(&mut app, MouseScrollUnit::Pixel, -25.0);
        assert_eq!(card_of(&app).sel, Some(4));
        // a closed card has nothing to turn
        app.world_mut().resource_mut::<MemoryCard>().close();
        turn(&mut app, MouseScrollUnit::Line, -3.0);
        assert_eq!(card_of(&app).sel, Some(4));
    }

    #[test]
    fn typed_memory_opens_the_card_and_the_card_takes_the_keyboard_until_it_closes() {
        let (svc, _rx) = Service::with_receiver();
        let mut app = App::new();
        app.init_resource::<Notes>()
            .init_resource::<MemoryCard>()
            .init_resource::<crate::apps::Open>()
            .init_resource::<Focus>()
            .init_resource::<CmdBar>()
            .init_resource::<crate::sound::Sound>()
            .insert_resource(svc)
            .add_systems(Update, commands);
        app.world_mut().resource_mut::<Notes>().list = six();
        app.world_mut().resource_mut::<crate::apps::Open>().memory = true;
        app.update();
        assert!(card_of(&app).open && !app.world().resource::<crate::apps::Open>().memory);
        assert!(app.world().resource::<Focus>().cmd, "the home and the apps stand by");
        app.world_mut().resource_mut::<MemoryCard>().close();
        app.update();
        assert!(!app.world().resource::<Focus>().cmd);
    }

    // ---- what the card says

    fn fonts() -> UiFonts {
        UiFonts { ui500: default(), ui600: default(), ui700: default(), display: default(), mono: default(), mono_bold: default() }
    }

    /// Every text of the card built for `card` and `notes`, in the order they were made.
    fn card_texts(card: MemoryCard, notes: Vec<Note>) -> Vec<String> {
        let mut app = App::new();
        app.insert_resource(fonts());
        let root = app.world_mut().spawn(Node::default()).id();
        app.world_mut().run_system_once(move |mut c: Commands, f: Res<UiFonts>| build_card(&mut c, root, &f, &card, &notes, "/Users/me", &|_| 2 * 3600)).unwrap();
        let world = app.world_mut();
        world.query::<&Text>().iter(world).map(|t| t.0.clone()).collect()
    }

    fn says(said: &[String], t: &str) -> bool {
        said.iter().any(|s| s == t)
    }

    #[test]
    fn the_card_shows_each_repo_with_its_count_and_every_note_with_its_age_and_source() {
        let mut card = MemoryCard::default();
        card.open(&six());
        let said = card_texts(card, six());
        for t in ["MEMORY · 6 NOTES", "SPHERE", "3 NOTES", "/code/sphere", "API", "2 NOTES", "/code/api", "ALL REPOS", "1 NOTE", "every agent"] {
            assert!(says(&said, t), "{t}: {said:?}");
        }
        // the selected note (the first) in full with its age and where it came from; the others after it
        assert!(says(&said, "Run cargo fmt before committing.") && says(&said, "2h ago · typed"), "{said:?}");
        assert!(says(&said, "2h ago · a change you sent") && says(&said, "2h ago · the CLI"), "{said:?}");
        assert!(says(&said, "Never edit src/gen by hand.") && says(&said, "Keep commits small."), "{said:?}");
        // the buttons, and the keys in the corner
        assert!(says(&said, "REMOVE") && says(&said, "CLOSE") && says(&said, "↑ ↓ · J K · D D · ESC"), "{said:?}");
    }

    #[test]
    fn an_armed_remove_says_so_on_the_row_and_the_button() {
        let list = six();
        let mut card = MemoryCard::default();
        card.open(&list);
        card.press_remove(&list, 1.0);
        let said = card_texts(card, list);
        assert!(says(&said, "Press D again to remove") && says(&said, "PRESS AGAIN TO REMOVE"), "{said:?}");
        assert!(!says(&said, "REMOVE"), "the button changed its words");
    }

    #[test]
    fn an_empty_card_says_how_to_add_a_note() {
        let mut card = MemoryCard::default();
        card.open(&[]);
        let said = card_texts(card, vec![]);
        assert!(says(&said, "MEMORY") && says(&said, "Nothing remembered yet."), "{said:?}");
        assert!(said.iter().any(|s| s.contains("remember: use pnpm")), "{said:?}");
        assert!(says(&said, "CLOSE") && says(&said, "ESC") && !says(&said, "REMOVE"), "{said:?}");
    }

    #[test]
    fn a_long_list_says_how_many_notes_are_out_of_sight() {
        let notes: Vec<Note> = (1..=30).map(|i| note(i, Some("/code/a"), &format!("note {i}"), NoteSource::Typed)).collect();
        let mut card = MemoryCard::default();
        card.open(&notes);
        let said = card_texts(card, notes);
        // the window holds the heading and nine notes: twenty-one are below, none above
        assert!(says(&said, "↓ 21 MORE"), "{said:?}");
        assert!(!said.iter().any(|s| s.starts_with('↑') && s.ends_with("MORE")), "{said:?}");
    }

    #[test]
    fn the_demo_has_six_notes_in_two_repos_and_all_repos_newest_first() {
        let notes = demo_notes();
        assert_eq!(notes.len(), 6);
        assert!(notes.windows(2).all(|w| w[0].id > w[1].id));
        let groups = by_repo(&notes);
        assert_eq!(groups.iter().map(|(r, g)| (r.map(repo_name), g.len())).collect::<Vec<_>>(), [(Some("web-app"), 3), (Some("api-server"), 2), (None, 1)]);
        assert!(notes.iter().all(|n| n.age_secs() > 60), "written a while ago, not just now");
        // every source shows
        for s in [NoteSource::Typed, NoteSource::Change, NoteSource::Cli] {
            assert!(notes.iter().any(|n| n.source == s));
        }
    }
}
