//! Hold the Swarm, in Sphere ("play" in the command bar, `--game`): the Agent swarm becomes a daily run. Agents join on
//! a timetable, circle the core on their repos' rings and ask before each step; you answer on a card with the Agent
//! moment card's own keys (Enter, E, A, Shift+Enter, L, the arrows) before their patience runs out. A dangerous ask let
//! through, or an agent left waiting too long, costs one of three shields; the third ends the run. A task whose steps
//! all went through merges, and its satellite goes out to the ready ring and down into the core as a real merge does.
//!
//! The rules are `engine` (no Bevy; the browser prototype's numbers and streams, checked against it), the asks
//! `content`, what the swarm draws `view`, the glass panels `panels`, and today's best and the streak `store`
//! (`~/.sphere/app/game`). Everyone gets the same run on the same UTC day.
//!
//! The game owns the swarm while it is on (`Swarm.game`): it writes `Swarm.model` itself, and the swarm's own sync,
//! keys, pointer, clicks and feedback stand aside. Real work still wins: the run pauses while the Agent moment card is
//! up, while you type in the command bar or the terminal, and when Sphere goes to the background.
//!
//! Look tests: `--game` (the title), `--game play[:secs]` (a bot plays the first `secs`, then the asks wait),
//! `--game bot[:secs]` (it goes on playing), `--game pause`, `--game over` (a bot's finished run). Offline runs are
//! Run 001 and save nothing.
pub mod content;
pub mod engine;
pub mod panels;
pub mod store;
pub mod view;

use crate::appframe::{AppClicks, Click};
use crate::apps::Open;
use crate::live::{ToastKind, Toasts};
use crate::panel::Focus;
use crate::sound::Sound;
use crate::swarm::feedback::SwarmTicker;
use crate::swarm::Swarm;
use crate::util::Role;
use bevy::input::keyboard::KeyboardInput;
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use engine::{Answer, Bot, Day, End, Ev, How, Run, Style};

/// The game's atlas in `AppAtlases` (the swarm's are 6 and 7): the strip, Today, Asking and the card.
pub const ATLAS: u8 = 8;
/// The title, pause and results panel's own atlas: resident only while it shows.
pub const ATLAS_BIG: u8 = 9;

/// The Say line's idle hint during a run.
pub const HINT: &str = "ENTER RUN · E BLOCK · A ALWAYS · ⇧ ENTER ALL · L LATER · ← → STEP · ESC PAUSE";

/// Where a copied result sends people.
pub const SHARE_URL: &str = "https://github.com/xiaojia630/sphere";

/// After a run ends, Enter waits this long before it plays again (so a last answer doesn't start a new run).
const OVER_GUARD_SECS: f32 = 1.2;

/// The command bar's words that open the game: only these exactly, since a longer line is a task for an agent.
pub fn is_game_words(l: &str) -> bool {
    matches!(l.trim(), "play" | "game" | "play a game" | "hold the swarm" | "play hold the swarm" | "swarm game")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Screen {
    #[default]
    Off,
    Title,
    Play,
    Paused,
    Over,
}

/// Why Sphere paused the run for you.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hold {
    /// a real agent's request is on the Agent moment card
    Request,
    /// the command bar or the terminal has the keys
    Typing,
    /// Sphere went to the background
    Away,
}

#[derive(Resource)]
pub struct Game {
    pub screen: Screen,
    pub day: Day,
    pub run: Run,
    pub view: view::View,
    pub record: store::Record,
    /// the run that just ended is today's best
    pub best: bool,
    /// seconds on this screen
    pub screen_t: f32,
    /// paused by Sphere, not by you
    pub hold: Option<Hold>,
    /// seconds left on the results' "copied" note
    pub copied: f32,
    /// a key that does nothing here: the card shakes once
    pub nudge: bool,
    /// the swarm was open before the game took it: leaving goes back to it
    had_swarm: bool,
    /// look tests: Run 001, nothing saved
    offline: bool,
    /// look tests: a bot plays
    bot: Option<Bot>,
    /// merges whose impact lands when their satellite reaches the core (seconds left)
    later: Vec<f32>,
}

impl Default for Game {
    fn default() -> Self {
        let day = Day::first();
        Game {
            screen: Screen::Off,
            run: Run::new(&day.key),
            day,
            view: view::View::default(),
            record: store::Record::default(),
            best: false,
            screen_t: 0.0,
            hold: None,
            copied: 0.0,
            nudge: false,
            had_swarm: false,
            offline: false,
            bot: None,
            later: Vec::new(),
        }
    }
}

impl Game {
    pub fn on(&self) -> bool {
        self.screen != Screen::Off
    }

    fn go(&mut self, s: Screen) {
        if self.screen != s {
            self.screen = s;
            self.screen_t = 0.0;
        }
    }

    /// A fresh run of today's.
    fn start(&mut self) {
        self.run = Run::new(&self.day.key);
        self.view.reset();
        self.best = false;
        self.hold = None;
        self.later.clear();
        self.go(Screen::Play);
    }

    /// The run ended: today's best and the streak move (and are saved, outside the look tests).
    fn record_end(&mut self) {
        self.best = self.record.played(&self.day, self.run.score);
        if !self.offline {
            store::save(&self.record);
        }
    }
}

pub struct GamePlugin;

impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Game>();
        app.add_systems(Startup, (panels::setup.after(crate::swarm::hud::panels::setup), view::setup_pops.after(crate::hud::setup_overlay), start));
        // the bar's "play" and this frame's clicks are in; the swarm opens (apps::apply) in the same frame
        app.add_systems(Update, control.after(crate::cmdbar::keys).after(crate::appframe::route_clicks).before(crate::apps::apply));
        app.add_systems(Update, tick.after(crate::apps::apply));
        app.add_systems(Update, (panels::place, panels::draw, panels::live).chain().before(crate::panel::update_panels));
        app.add_systems(Update, view::draw_pops.after(crate::swarm::scene::update));
    }
}

/// `--game`: opens on the first frame.
fn start(args: Res<crate::Args>, mut open: ResMut<Open>) {
    if args.game.is_some() {
        open.game = true;
    }
}

/// What a key or a click asks for.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Act {
    Answer(Answer),
    Step(i32),
    Pick(usize),
    Start,
    Pause,
    Resume,
    Quit,
    Copy,
}

fn act_of(screen: Screen, k: KeyCode, shift: bool) -> Option<Act> {
    use KeyCode::*;
    Some(match (screen, k) {
        (Screen::Play, Enter | NumpadEnter) => Act::Answer(if shift { Answer::All } else { Answer::Yes }),
        (Screen::Play, KeyE) => Act::Answer(Answer::Change),
        (Screen::Play, KeyA) => Act::Answer(Answer::Always),
        (Screen::Play, KeyL) => Act::Answer(Answer::Later),
        (Screen::Play, ArrowRight | ArrowDown) => Act::Step(1),
        (Screen::Play, ArrowLeft | ArrowUp) => Act::Step(-1),
        (Screen::Play, Escape | KeyP) => Act::Pause,
        (Screen::Title, Enter | NumpadEnter | Space) => Act::Start,
        (Screen::Paused, Enter | NumpadEnter | Space | KeyP) => Act::Resume,
        (Screen::Over, Enter | NumpadEnter) => Act::Start,
        (Screen::Over, KeyC) => Act::Copy,
        (Screen::Title | Screen::Paused | Screen::Over, Escape) => Act::Quit,
        _ => return None,
    })
}

/// Opens the game ("play", `--game`), and turns its keys and its panels' clicks into answers and screens.
#[allow(clippy::too_many_arguments)]
fn control(
    mut ev: MessageReader<KeyboardInput>,
    keys: Res<ButtonInput<KeyCode>>,
    focus: Res<Focus>,
    moment: Res<crate::moment::Moment>,
    args: Res<crate::Args>,
    mut clicks: ResMut<AppClicks>,
    mut game: ResMut<Game>,
    mut swarm: ResMut<Swarm>,
    mut open: ResMut<Open>,
    mut sound: ResMut<Sound>,
    mut toasts: ResMut<Toasts>,
    mut ticker: ResMut<SwarmTicker>,
    mut fx: ResMut<crate::fx::Fx>,
) {
    let pressed: Vec<(KeyCode, bool)> = ev.read().filter(|k| k.state == ButtonState::Pressed).map(|k| (k.key_code, k.repeat)).collect();
    let taken: Vec<Click> = clicks.take(ATLAS).into_iter().chain(clicks.take(ATLAS_BIG)).collect();
    if std::mem::take(&mut open.game) && !game.on() {
        game.had_swarm = swarm.open;
        game.offline = args.offline();
        game.day = if game.offline { Day::first() } else { Day::at(crate::core::unix_now()) };
        game.record = if game.offline { store::Record::default() } else { store::load() };
        game.run = Run::new(&game.day.key);
        game.view.reset();
        game.bot = None;
        game.later.clear();
        game.go(Screen::Title);
        swarm.game = true;
        swarm.mode = crate::swarm::Mode::Overview;
        swarm.hover = None;
        swarm.version += 1;
        ticker.0.clear();
        if swarm.open {
            sound.play_with("open", 0.8, 0.0);
        }
        // apps::apply opens the swarm this frame (and closes any app)
        open.swarm = true;
        return;
    }
    if !game.on() {
        return;
    }
    let mods = keys.any_pressed([KeyCode::SuperLeft, KeyCode::SuperRight, KeyCode::ControlLeft, KeyCode::ControlRight, KeyCode::AltLeft, KeyCode::AltRight]);
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let mut acts: Vec<Act> = Vec::new();
    if !(mods || focus.cmd || focus.terminal || moment.shown) {
        acts.extend(pressed.into_iter().filter(|(_, rep)| !rep).filter_map(|(k, _)| act_of(game.screen, k, shift)));
    }
    for c in taken {
        let a = match c {
            Click::Key(k) => act_of(game.screen, k, false),
            Click::Alt(_) if game.screen == Screen::Play => Some(Act::Answer(Answer::All)),
            Click::Pick(i) if game.screen == Screen::Play => Some(Act::Pick(i)),
            _ => None,
        };
        acts.extend(a);
    }
    for a in acts {
        match a {
            Act::Answer(x) => game.run.answer(x),
            Act::Step(d) => {
                if game.run.queue.len() > 1 {
                    game.run.step(d);
                    sound.play_with("tick", 0.5, 0.0);
                } else {
                    game.nudge = true;
                }
            }
            Act::Pick(i) => {
                if i < game.run.queue.len() && i != game.run.cur {
                    game.run.cur = i;
                    sound.play_with("tick", 0.5, 0.0);
                }
            }
            Act::Start => {
                if game.screen == Screen::Over && game.screen_t < OVER_GUARD_SECS {
                    continue;
                }
                game.start();
                ticker.0.clear();
                sound.play_with("boot", 0.6, 0.0);
                sound.play_with("whoosh", 0.6, 0.0);
                fx.shock(Role::Hot);
            }
            Act::Pause => {
                game.go(Screen::Paused);
                game.hold = None;
                sound.play_with("collapse", 0.6, 0.0);
            }
            Act::Resume => {
                game.go(Screen::Play);
                game.hold = None;
                sound.play_with("expand", 0.6, 0.0);
            }
            Act::Quit => {
                leave(&mut game, &mut swarm, &mut ticker);
                sound.play_with("close", 0.7, 0.0);
                return;
            }
            Act::Copy => {
                let text = game.run.share_text(&game.day, SHARE_URL);
                if copy(&text) {
                    game.copied = 2.4;
                    toasts.push_kind("Result copied: paste it anywhere", ToastKind::Ok);
                    sound.play_with("confirm", 0.6, 0.0);
                } else {
                    toasts.push("Copying isn't available here");
                }
            }
        }
    }
}

/// Hands the swarm back: to the real agents if it was open before, else home.
fn leave(game: &mut Game, swarm: &mut Swarm, ticker: &mut SwarmTicker) {
    game.go(Screen::Off);
    game.view.reset();
    game.later.clear();
    game.bot = None;
    game.hold = None;
    swarm.game = false;
    swarm.game_panel = false;
    swarm.model = crate::swarm::model::Model::default();
    swarm.hover = None;
    swarm.version += 1;
    ticker.0.clear();
    if !game.had_swarm && swarm.open {
        swarm.close();
    }
}

/// The result on the clipboard: macOS only for now; elsewhere C says copying isn't available.
#[cfg(target_os = "macos")]
fn copy(text: &str) -> bool {
    use std::io::Write;
    let Ok(mut child) = std::process::Command::new("pbcopy").stdin(std::process::Stdio::piped()).spawn() else { return false };
    let ok = child.stdin.take().is_some_and(|mut s| s.write_all(text.as_bytes()).is_ok());
    child.wait().is_ok_and(|s| s.success()) && ok
}

#[cfg(not(target_os = "macos"))]
fn copy(_text: &str) -> bool {
    false
}

/// Look tests: the screen `--game` asks for, set up once: `play:40` (a bot plays the first 40 s, then the asks wait
/// for you), `bot:40` (it goes on playing), `pause`, `over`.
fn look_setup(game: &mut Game, mode: &str) {
    let (what, secs) = match mode.split_once(':') {
        Some((w, s)) => (w, s.parse::<f64>().ok()),
        None => (mode, None),
    };
    let ff = |game: &mut Game, style: Style, until: f64| {
        let mut bot = Bot::new(style, &game.day.key);
        while game.run.playing() && game.run.t < until {
            game.run.update(0.05);
            if let Some(a) = bot.act(&game.run, 0.05) {
                game.run.answer(a);
            }
            game.run.events.clear();
        }
        bot
    };
    match what {
        "play" | "bot" => {
            game.start();
            ff(game, Style::Expert, secs.unwrap_or(0.0));
            // the clips' player: slower than the expert, and wrong now and then
            game.bot = (what == "bot").then(|| Bot::new(Style::Casual, &game.day.key));
        }
        "pause" => {
            game.start();
            ff(game, Style::Expert, secs.unwrap_or(24.0));
            game.go(Screen::Paused);
        }
        "over" => {
            game.start();
            ff(game, Style::Casual, secs.unwrap_or(600.0));
            if game.run.playing() {
                // the casual bot held on: end it the way most runs end
                game.run.shields = 1;
            }
            while game.run.playing() {
                game.run.update(0.05);
                game.run.events.clear();
            }
            game.run.events.clear();
            game.record_end();
            game.go(Screen::Over);
            game.screen_t = OVER_GUARD_SECS;
        }
        _ => {}
    }
}

/// Moves the run on, pauses it when real work needs you, turns its events into sounds, flashes, toasts and points
/// over the satellites, and writes the swarm's model.
#[allow(clippy::too_many_arguments)]
fn tick(
    sim: Res<crate::core::Sim>,
    args: Res<crate::Args>,
    focus: Res<Focus>,
    moment: Res<crate::moment::Moment>,
    win: Query<&Window, With<PrimaryWindow>>,
    mut game: ResMut<Game>,
    mut swarm: ResMut<Swarm>,
    mut out: Out,
    mut was_focused: Local<Option<bool>>,
    mut looked: Local<bool>,
) {
    if !game.on() {
        *was_focused = None;
        return;
    }
    // something else took the screen (the Working view, an app): the game is over
    if !swarm.open {
        leave(&mut game, &mut swarm, &mut out.ticker);
        return;
    }
    let dt = sim.dt;
    game.screen_t += dt;
    game.copied = (game.copied - dt).max(0.0);
    if !*looked {
        *looked = true;
        if let Some(mode) = args.game.as_deref().filter(|_| game.offline) {
            look_setup(&mut game, mode);
        }
    }
    // real work first
    if game.screen == Screen::Play {
        let focused = win.single().ok().map(|w| w.focused);
        let away = *was_focused == Some(true) && focused == Some(false);
        let hold = if moment.shown {
            Some(Hold::Request)
        } else if focus.cmd || focus.terminal {
            Some(Hold::Typing)
        } else if away {
            Some(Hold::Away)
        } else {
            None
        };
        if let Some(h) = hold {
            game.go(Screen::Paused);
            game.hold = Some(h);
            out.sound.play_with("collapse", 0.5, 0.0);
        }
        *was_focused = focused;
    }
    if game.screen == Screen::Play {
        let step = f64::from(dt.min(0.05));
        let g = &mut *game;
        if let Some(bot) = g.bot.as_mut() {
            if let Some(a) = bot.act(&g.run, step) {
                g.run.answer(a);
            }
        }
        g.run.update(step);
    }
    let evs = std::mem::take(&mut game.run.events);
    for e in evs {
        feedback(&mut game, e, &mut out);
    }
    // a merge's impact when its satellite reaches the core
    let mut due = 0;
    game.later.retain_mut(|t| {
        *t -= dt;
        if *t <= 0.0 {
            due += 1;
        }
        *t > 0.0
    });
    if due > 0 {
        out.fx.sweep();
        out.fx.shock(Role::Ok);
        out.ctl.flash = out.ctl.flash.max(0.55);
    }
    let g = &mut *game;
    let m = g.view.model(&g.run, 0);
    if swarm.model != m {
        swarm.model = m;
        swarm.version += 1;
    }
    let big = matches!(g.screen, Screen::Title | Screen::Paused | Screen::Over);
    if swarm.game_panel != big {
        swarm.game_panel = big;
    }
}

/// What the run's events reach.
#[derive(bevy::ecs::system::SystemParam)]
struct Out<'w> {
    sound: ResMut<'w, Sound>,
    toasts: ResMut<'w, Toasts>,
    say: ResMut<'w, crate::homepanels::Say>,
    fx: ResMut<'w, crate::fx::Fx>,
    ctl: ResMut<'w, crate::core::CoreCtl>,
    ticker: ResMut<'w, SwarmTicker>,
}

fn ticker(out: &mut Out, key: &str, val: String, warn: bool) {
    out.ticker.0.insert(0, (key.to_string(), val, warn));
    out.ticker.0.truncate(crate::swarm::feedback::TICKER_MAX);
}

/// One event's sound, flash, toast, ticker item and the points floating up from its satellite.
fn feedback(game: &mut Game, e: Ev, out: &mut Out) {
    let clip = |s: &str| crate::appframe::clip(s, 40);
    match e {
        Ev::Joined(a) => {
            if game.run.t > 1.0 {
                out.sound.play_with("open", 0.45, 0.0);
                game.view.pop(&game.run, a, "JOINED", Role::Hot);
                let repo = game.run.agents[a].repo;
                ticker(out, "JOINED", format!("{repo} · {}", game.run.agents[a].task), false);
            }
        }
        Ev::Asked(_) => out.sound.play_q_with("toast", 0.45, 120),
        Ev::Wave { n, trap } => {
            out.sound.play_with("think", 0.7, 0.0);
            out.toasts.push_kind(format!("{n} agents ask the same · Shift+Enter answers them all"), ToastKind::Plain);
            ticker(out, "WAVE", format!("{n} agents ask the same"), trap);
        }
        Ev::Approved { agent, pts, how, .. } => {
            if how == How::Auto {
                out.sound.play_q_with("tick", 0.3, 90);
            } else {
                out.sound.play_with("confirm", 0.55, 0.0);
            }
            game.view.pop(&game.run, agent, format!("+{pts}"), Role::Ok);
        }
        Ev::Merged { agent, pts, task } => {
            out.sound.play_with("done", 0.8, 0.0);
            game.view.merged(&game.run, agent, task);
            game.view.pop(&game.run, agent, format!("MERGED +{pts}"), Role::Ok);
            game.later.push(crate::motion::delay(crate::motion::Pace::Big, 0.62));
            let repo = game.run.agents[agent].repo;
            ticker(out, "MERGED", format!("{repo} ← {task}"), false);
        }
        Ev::Caught { agent, pts, text, why } => {
            out.sound.play_with("expand", 0.7, 0.0);
            out.fx.shock(Role::Fill);
            game.view.pop(&game.run, agent, format!("CAUGHT +{pts}"), Role::Fill);
            out.toasts.push_kind(format!("Caught · {}", clip(&text)), ToastKind::Ok);
            out.say.say(format!("Good catch. {why}"));
            ticker(out, "CAUGHT", clip(&text), false);
        }
        Ev::Needless { agent, text, note } => {
            out.sound.play_with("discard", 0.6, 0.0);
            game.view.pop(&game.run, agent, "NO NEED", Role::Acc);
            out.toasts.push_kind(if note.is_empty() { format!("That one was safe · {}", clip(&text)) } else { note.to_string() }, ToastKind::Plain);
        }
        Ev::Incident { agent, text, .. } => {
            out.sound.play_with("error", 0.8, 0.0);
            out.fx.shock(Role::Bad);
            out.ctl.flash = out.ctl.flash.max(0.6);
            game.view.pop(&game.run, agent, "INCIDENT", Role::Bad);
            out.toasts.push_kind(format!("Incident · {}", clip(&text)), ToastKind::Bad);
            out.say.say(game.run.lesson.clone());
            ticker(out, "INCIDENT", clip(&text), true);
        }
        Ev::Stalled { agent, .. } => {
            out.sound.play_with("collapse", 0.7, 0.0);
            game.view.pop(&game.run, agent, "STALLED", Role::Acc);
            out.toasts.push_kind("An agent waited too long and stalled", ToastKind::Bad);
            let a = &game.run.agents[agent];
            let line = format!("{} · {}", a.repo, a.task);
            ticker(out, "STALLED", line, true);
        }
        Ev::Always(rule) => {
            out.sound.play("toggle");
            out.toasts.push_kind(format!("Always · {rule}"), ToastKind::Plain);
            ticker(out, "ALWAYS", rule, false);
        }
        Ev::AllowedAll(n) => out.toasts.push_kind(format!("Allowed for {n} agents"), ToastKind::Ok),
        Ev::Later => out.sound.play_with("whoosh", 0.45, 0.0),
        Ev::Nudge => {
            out.sound.play_with("error", 0.25, 0.0);
            game.nudge = true;
        }
        Ev::Over(why) => {
            out.sound.play_with("collapse", 0.9, 0.0);
            out.fx.shock(Role::Bad);
            out.ctl.flash = out.ctl.flash.max(0.5);
            game.record_end();
            game.go(Screen::Over);
            let r = &game.run;
            let line = format!("Shift over. {} points, {} shipped, {} caught.", engine::thousands(r.score), r.shipped, r.caught);
            out.sound.speak(&line);
            out.say.say(line);
            ticker(out, "OVER", if why == End::Incident { "three incidents".to_string() } else { "an agent waited too long".to_string() }, true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_exact_words_open_the_game() {
        for w in ["play", "game", " play ", "hold the swarm", "swarm game"] {
            assert!(is_game_words(w), "{w}");
        }
        for w in ["play with the parser", "game plan for the api", "swarm", "playground"] {
            assert!(!is_game_words(w), "{w}");
        }
    }

    #[test]
    fn the_keys_are_the_moment_card_s() {
        assert_eq!(act_of(Screen::Play, KeyCode::Enter, false), Some(Act::Answer(Answer::Yes)));
        assert_eq!(act_of(Screen::Play, KeyCode::Enter, true), Some(Act::Answer(Answer::All)));
        assert_eq!(act_of(Screen::Play, KeyCode::KeyE, false), Some(Act::Answer(Answer::Change)));
        assert_eq!(act_of(Screen::Play, KeyCode::KeyA, false), Some(Act::Answer(Answer::Always)));
        assert_eq!(act_of(Screen::Play, KeyCode::KeyL, false), Some(Act::Answer(Answer::Later)));
        assert_eq!(act_of(Screen::Play, KeyCode::Escape, false), Some(Act::Pause));
        assert_eq!(act_of(Screen::Paused, KeyCode::Escape, false), Some(Act::Quit));
        assert_eq!(act_of(Screen::Title, KeyCode::KeyE, false), None);
        assert_eq!(act_of(Screen::Over, KeyCode::KeyC, false), Some(Act::Copy));
    }

    #[test]
    fn a_finished_run_records_today_s_best_without_saving_offline() {
        let mut g = Game { offline: true, ..default() };
        look_setup(&mut g, "over");
        assert_eq!(g.screen, Screen::Over);
        assert!(!g.run.playing());
        assert!(g.best);
        assert_eq!(g.record.best_today(&g.day), g.run.score);
        assert_eq!(g.record.streak, 1);
    }

    #[test]
    fn the_look_test_plays_from_where_it_was_asked() {
        let mut g = Game { offline: true, ..default() };
        look_setup(&mut g, "play:40");
        assert_eq!(g.screen, Screen::Play);
        assert!(g.run.t >= 40.0 && g.run.playing());
        assert!(g.bot.is_none());
        assert!(g.run.agents.len() >= 5);
        let mut g = Game { offline: true, ..default() };
        look_setup(&mut g, "bot:20");
        assert!(g.run.t >= 20.0 && g.bot.is_some());
    }
}
