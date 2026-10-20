//! The command bar, pinned to the bottom of the screen like the prototype's dock. `/` or Cmd+K focuses it.
//! Typing a task starts an agent through sphered; `repo: task` picks the repo (a folder in ~/Projects, or a
//! path), otherwise the last repo used, otherwise the folder the app was started in. A line that opens with a
//! schedule (`every weekday 9:00 sphere: update the dependencies`) saves a protocol instead, and a preview above the
//! bar says what would be saved, or what is wrong with it.
use crate::fonts::UiFonts;
use crate::live::Toasts;
use crate::panel::{font, with_alpha, Focus};
use crate::service::{Cmd, Service};
use crate::util::*;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::text::LineHeight;
use sphere_proto::protocol::{self, Line, LocalZone, Zone};
use std::path::{Path, PathBuf};

#[derive(Resource, Default)]
pub struct CmdBar {
    pub open: bool,
    pub text: String,
    /// Bumps when `text` or `open` change, so the UI updates only then.
    pub version: u64,
    /// What you type goes here instead of starting a task (an app asked for it).
    pub target: Option<BarTarget>,
    /// Text typed for an app's target, waiting for the app to take it.
    pub out: Option<(BarTarget, String)>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum BarTarget {
    /// the Code app's "ask for a change": a message to this task (id, title)
    Task(String, String),
    /// the Git app: a commit message for this repo (path, branch)
    /// (path, branch, amend): with amend an empty message keeps the previous one
    Commit(String, String, bool),
    /// the Git app: a branch to switch to in this repo (path)
    Branch(String),
    /// the Files app: what to filter the folder by
    Filter,
}

impl CmdBar {
    /// Opens the bar to send a message to one agent.
    pub fn ask(&mut self, task: String, title: String, focus: &mut Focus) {
        self.prompt(BarTarget::Task(task, title), focus);
    }

    /// Opens the bar for an app: what you type goes to `t`.
    pub fn prompt(&mut self, t: BarTarget, focus: &mut Focus) {
        self.open = true;
        self.text.clear();
        self.target = Some(t);
        focus.cmd = true;
        self.version += 1;
    }
}

#[derive(Component)]
pub struct CmdFrame;
#[derive(Component)]
pub struct CmdText;
#[derive(Component)]
pub struct CmdCaret;
/// The protocol preview above the bar, and its two texts: the word PROTOCOL and what would be saved.
#[derive(Component)]
pub struct PreviewBox;
#[derive(Component)]
pub struct PreviewLabel;
#[derive(Component)]
pub struct PreviewRest;

const PLACEHOLDER: &str = "Ask Sphere, or start a task  ·  repo: task";

pub fn setup(mut commands: Commands, f: Res<UiFonts>, root: Res<crate::hud::OverlayRoot>) {
    let line = role_color(Role::Line);
    // the dock: centred above the ticker, min(1300, 100% - 32) wide, the command line then the controls
    let wrap = commands
        .spawn(Node { position_type: PositionType::Absolute, left: px(0), right: px(0), bottom: px(34), justify_content: JustifyContent::Center, ..default() })
        .id();
    let dock = commands
        .spawn((
            crate::visor::Dock,
            Node {
                width: percent(100),
                max_width: px(1300),
                margin: UiRect::horizontal(px(16)),
                flex_direction: FlexDirection::Row,
                flex_wrap: FlexWrap::Wrap,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                column_gap: px(18),
                row_gap: px(10),
                ..default()
            },
        ))
        .id();
    commands.entity(wrap).add_child(dock);
    let bar = commands
        .spawn((
            CmdFrame,
            Node {
                flex_grow: 1.0,
                flex_shrink: 1.0,
                flex_basis: px(440),
                min_width: px(0),
                height: px(48),
                padding: UiRect::horizontal(px(14)),
                column_gap: px(12),
                align_items: AlignItems::Center,
                border: UiRect::all(px(1)),
                ..default()
            },
            BackgroundGradient(vec![Gradient::Linear(LinearGradient::to_bottom(vec![
                ColorStop::percent(Color::srgba(0.07, 0.09, 0.1, 0.6), 0.0),
                ColorStop::percent(Color::srgba(0.03, 0.04, 0.05, 0.7), 100.0),
            ]))]),
            BorderColor::all(with_alpha(line, 0.34)),
        ))
        .id();
    let prompt = commands.spawn((Text::new("›"), font(&f.mono, 18.0, 0.0), TextColor(role_color(Role::Fill)))).id();
    let text = commands
        .spawn((
            CmdText,
            Text::new(PLACEHOLDER),
            font(&f.ui500, 16.0, 0.96),
            TextColor(with_alpha(line, 0.55)),
            LineHeight::Px(18.0),
            TextLayout { linebreak: LineBreak::NoWrap, ..default() },
            Node { overflow: Overflow::clip(), flex_shrink: 1.0, min_width: px(0), ..default() },
        ))
        .id();
    let caret = commands
        .spawn((CmdCaret, Node { width: px(2), height: px(18), margin: UiRect { left: px(-10), ..default() }, ..default() }, BackgroundColor(Color::NONE)))
        .id();
    let spacer = commands.spawn(Node { flex_grow: 1.0, ..default() }).id();
    let hint = commands
        .spawn((
            Node { padding: UiRect { left: px(6), right: px(6), top: px(5), bottom: px(4) }, border: UiRect::all(px(1)), ..default() },
            BorderColor::all(with_alpha(line, 0.13)),
        ))
        .id();
    let hint_t = commands.spawn((Text::new("ENTER"), font(&f.ui700, 9.5, 1.9), TextColor(role_color(Role::Dim)), LineHeight::Px(9.5))).id();
    commands.entity(hint).add_child(hint_t);
    commands.entity(bar).add_children(&[prompt, text, caret, spacer, hint]);
    commands.entity(dock).add_child(bar);
    // the protocol preview: black glass like a toast, hung above the bar's left edge, shown only while the line being
    // typed opens with a schedule
    let preview = commands
        .spawn((
            PreviewBox,
            Node {
                position_type: PositionType::Absolute,
                left: px(0),
                bottom: percent(100),
                margin: UiRect { bottom: px(8), ..default() },
                max_width: percent(100),
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: px(10),
                padding: UiRect { left: px(12), right: px(14), top: px(9), bottom: px(8) },
                border: UiRect { left: px(3), right: px(1), top: px(1), bottom: px(1) },
                overflow: Overflow::clip(),
                ..default()
            },
            BackgroundColor(Color::NONE),
            BorderColor::all(Color::NONE),
            UiTransform::default(),
            Visibility::Hidden,
        ))
        .id();
    let label = commands.spawn((PreviewLabel, Text::new("PROTOCOL"), font(&f.ui700, 11.0, 2.64), TextColor(Color::NONE), LineHeight::Px(11.0), TextLayout { linebreak: LineBreak::NoWrap, ..default() }, Node { flex_shrink: 0.0, ..default() })).id();
    let rest = commands
        .spawn((PreviewRest, Text::new(""), font(&f.ui600, 11.0, 2.64), TextColor(Color::NONE), LineHeight::Px(11.0), TextLayout { linebreak: LineBreak::NoWrap, ..default() }, Node { overflow: Overflow::clip(), flex_shrink: 1.0, min_width: px(0), ..default() }))
        .id();
    commands.entity(preview).add_children(&[label, rest]);
    commands.entity(dock).add_child(preview);
    commands.entity(root.0).add_child(wrap);
}

fn super_down(keys: &ButtonInput<KeyCode>) -> bool {
    keys.pressed(KeyCode::SuperLeft) || keys.pressed(KeyCode::SuperRight)
}

/// Keys while the bar is open, and the keys that open it. Runs before the card and the terminal.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn keys(
    mut ev: MessageReader<KeyboardInput>,
    keys: Res<ButtonInput<KeyCode>>,
    mut bar: ResMut<CmdBar>,
    mut focus: ResMut<Focus>,
    svc: Res<Service>,
    mut toasts: ResMut<Toasts>,
    mut terms: ResMut<crate::term::Terms>,
    moment: Res<crate::moment::Moment>,
    (mut acts, mut open, mut say, mut sound, mut working, mut swarm): (ResMut<crate::home::Acts>, ResMut<crate::apps::Open>, ResMut<crate::homepanels::Say>, ResMut<crate::sound::Sound>, ResMut<crate::working::Working>, ResMut<crate::swarm::Swarm>),
) {
    let cmd = super_down(&keys);
    for e in ev.read() {
        if e.state != ButtonState::Pressed {
            continue;
        }
        if !bar.open {
            let slash = matches!(&e.logical_key, Key::Character(c) if c == "/") && !focus.terminal && !moment.editing;
            if (cmd && e.key_code == KeyCode::KeyK) || slash {
                bar.open = true;
                focus.cmd = true;
                bar.version += 1;
            }
            continue;
        }
        match &e.logical_key {
            Key::Escape => {
                bar.open = false;
                bar.text.clear();
                bar.target = None;
            }
            Key::Enter => {
                let text = std::mem::take(&mut bar.text);
                bar.open = false;
                let t = text.trim();
                if let Some(target) = bar.target.take() {
                    match target {
                        BarTarget::Task(task, title) if !t.is_empty() => {
                            svc.send(Cmd::Send { task, text: t.to_string() });
                            toasts.push(format!("Sent to {title}"));
                            sound.play("confirm");
                        }
                        other if !t.is_empty() || matches!(other, BarTarget::Commit(_, _, true)) => bar.out = Some((other, t.to_string())),
                        _ => {}
                    }
                } else if matches!(protocol::classify(t, crate::core::unix_now(), &LocalZone), Line::Mistake(_)) {
                    // a schedule that does not read: the line stays in the bar to be fixed, and the preview says why
                    toasts.push_bad("Protocol not saved");
                    bar.text = text;
                    bar.open = true;
                } else if !home_command(t, &mut acts, &mut open, &mut toasts, &mut swarm, &mut sound) {
                    run(t, &svc, &mut toasts, &mut terms, &mut focus, &mut say, &mut sound, &mut working);
                }
            }
            Key::Backspace => {
                if cmd {
                    bar.text.clear();
                } else {
                    bar.text.pop();
                }
            }
            Key::Character(c) if cmd => {
                if c == "v" {
                    if let Some(p) = paste() {
                        bar.text.push_str(&p);
                    }
                } else if c == "k" {
                    bar.open = false;
                }
            }
            _ => {
                if let Some(t) = &e.text {
                    bar.text.extend(t.chars().filter(|c| !c.is_control()));
                }
            }
        }
        focus.cmd = bar.open;
        bar.version += 1;
    }
}

fn paste() -> Option<String> {
    let out = std::process::Command::new("pbpaste").output().ok()?;
    let s = String::from_utf8_lossy(&out.stdout).replace(['\n', '\r'], " ");
    Some(s)
}

/// The prototype's typed commands: views, collapse, themes, fit, reset, opening apps. True when handled.
pub(crate) fn home_command(t: &str, acts: &mut crate::home::Acts, open: &mut crate::apps::Open, toasts: &mut Toasts, swarm: &mut crate::swarm::Swarm, sound: &mut crate::sound::Sound) -> bool {
    use crate::core::Mode;
    use crate::home::Act;
    use crate::theme::ThemeId;
    let l = t.to_ascii_lowercase();
    let starts = |ws: &[&str]| ws.iter().any(|w| l == *w || l.starts_with(&format!("{w} ")));
    if starts(&["motion", "animation", "animations"]) {
        // "motion fast", "motion cinematic"; "motion" alone switches
        use crate::motion::{self, Motion};
        let rest = l.split_once(' ').map(|(_, r)| r).unwrap_or("");
        // "motion 1.5": Fast at 1.5 times its speed
        if let Some(b) = rest.trim().trim_end_matches('x').parse::<f32>().ok().filter(|b| b.is_finite() && *b > 0.0) {
            motion::set_boost(b);
            toasts.push(format!("Motion · Fast × {}", (motion::boost() * 100.0).round() / 100.0));
            return true;
        }
        let m = Motion::from_key(rest).unwrap_or(if motion::get() == Motion::Fast { Motion::Cinematic } else { Motion::Fast });
        if m == Motion::Fast && Motion::from_key(rest).is_some() {
            // "motion fast" is Fast as it comes
            motion::use_boost(1.0);
        }
        motion::set(m);
        toasts.push(format!("Motion · {}", if m == Motion::Fast { "Fast" } else { "Cinematic" }));
        return true;
    }
    if let Some(w) = crate::takeover::parse_words(&l) {
        open.takeover = Some(w);
        return true;
    }
    // `protocols` opens the card of what runs on a schedule; a longer line is a task (or a protocol to save)
    if matches!(l.trim(), "protocols" | "protocol" | "routines" | "routine" | "schedules") {
        open.protocols = true;
        return true;
    }
    // `code` opens the Code app; `code <path>` opens a folder in it (`code .` is the last repo used). Words that
    // aren't a folder ("code review this") are a task, as before
    if starts(&["code"]) {
        let arg = t.get(4..).unwrap_or("").trim();
        if !arg.is_empty() {
            match code_arg(arg, last_repo().as_deref(), &home()) {
                Some(Ok(dir)) => open.code_folder(dir),
                Some(Err(e)) => toasts.push(e),
                None => return false,
            }
            return true;
        }
        open.app(2, toasts);
        return true;
    }
    if starts(&["swarm", "agents swarm", "agent swarm", "all agents"]) {
        open.swarm = true;
        return true;
    }
    if swarm.open {
        use crate::swarm::Tilt;
        // next / merge / stop / a number / back are the swarm's (`swarm::input` runs them)
        if let Some(c) = crate::swarm::input::parse(&l) {
            open.swarm_cmds.push(c);
            return true;
        }
        let tilt = if starts(&["low"]) { Some(Tilt::Low) } else if starts(&["mid"]) { Some(Tilt::Mid) } else if starts(&["high"]) { Some(Tilt::High) } else { None };
        if let Some(t) = tilt {
            crate::swarm::view::set_tilt(swarm, t, sound, toasts);
            return true;
        }
        if starts(&["track", "tracks"]) {
            crate::swarm::view::toggle_tracks(swarm, sound, toasts);
            return true;
        }
    }
    if starts(&["system", "top", "activity", "processes"]) {
        open.system();
        return true;
    }
    let a = if starts(&["collapse", "fold", "close rings", "close ring"]) {
        Act::Collapse(Some(true))
    } else if starts(&["unfold", "open rings", "open ring"]) {
        Act::Collapse(Some(false))
    } else if starts(&["expand", "depth", "explode", "pull"]) {
        Act::Mode(Mode::Depth)
    } else if starts(&["flat", "assemble", "flatten"]) {
        Act::Mode(Mode::Flat)
    } else if starts(&["orbit", "gyro"]) {
        Act::Mode(Mode::Orbit)
    } else if starts(&["balanced", "default", "calm"]) {
        Act::Theme(Some(ThemeId::Balanced))
    } else if starts(&["full color", "full colour", "color", "colour", "rainbow"]) {
        Act::Theme(Some(ThemeId::Full))
    } else if starts(&["cyan", "blue"]) {
        Act::Theme(Some(ThemeId::Cyan))
    } else if starts(&["black", "mono", "dark"]) {
        Act::Theme(Some(ThemeId::Black))
    } else if starts(&["red"]) {
        Act::Theme(Some(ThemeId::Red))
    } else if starts(&["apps", "open apps", "show apps", "launcher", "open core", "open the core", "core open"]) {
        open.toggle_orbit = Some(true);
        return true;
    } else if starts(&["close", "close apps", "close core", "back"]) {
        open.toggle_orbit = Some(false);
        return true;
    } else if starts(&["fit", "frame"]) {
        Act::Fit
    } else if starts(&["reset", "home", "center", "centre"]) {
        Act::Reset
    } else if let Some(rest) = ["open ", "launch ", "show "].iter().find_map(|p| l.strip_prefix(p)) {
        let i = crate::homepanels::APPS.iter().position(|(n, _)| n.to_ascii_lowercase().starts_with(rest.trim()) || rest.trim().starts_with(&n.to_ascii_lowercase()));
        match i {
            Some(i) => {
                open.app(i, toasts);
                return true;
            }
            None => return false,
        }
    } else {
        return false;
    };
    acts.0.push(a);
    true
}

#[allow(clippy::too_many_arguments)]
fn run(text: &str, svc: &Service, toasts: &mut Toasts, terms: &mut crate::term::Terms, focus: &mut Focus, say: &mut crate::homepanels::Say, sound: &mut crate::sound::Sound, working: &mut crate::working::Working) {
    if text.is_empty() {
        return;
    }
    match text.to_ascii_lowercase().as_str() {
        "t" | "term" | "terminal" | "new tab" => {
            terms.open_tab();
            focus.terminal = true;
            return;
        }
        _ => {}
    }
    if text.eq_ignore_ascii_case("stop") {
        // stop the agent you heard from last that is still running
        let latest = {
            let s = svc.shared.lock().unwrap();
            s.tasks.iter().filter(|t| matches!(t.state, sphere_proto::TaskState::Working | sphere_proto::TaskState::NeedsYou | sphere_proto::TaskState::Queued)).max_by(|a, b| a.updated_at.cmp(&b.updated_at)).map(|t| (t.id.clone(), t.title.clone()))
        };
        match latest {
            Some((task, title)) => {
                toasts.push(format!("Stopping {title}"));
                svc.send(Cmd::Stop { task });
            }
            None => toasts.push("Nothing running to stop"),
        }
        return;
    }
    if let Some(msg) = text.strip_prefix('>') {
        // `> message` talks to the agent you heard from last
        let latest = {
            let s = svc.shared.lock().unwrap();
            s.tasks.iter().filter(|t| t.session_id.is_some()).max_by(|a, b| a.updated_at.cmp(&b.updated_at)).map(|t| (t.id.clone(), t.title.clone()))
        };
        match latest {
            Some((task, title)) => {
                toasts.push(format!("To {title}"));
                svc.send(Cmd::Send { task, text: msg.trim().to_string() });
            }
            None => toasts.push("No agent to talk to yet"),
        }
        return;
    }
    match protocol::classify(text, crate::core::unix_now(), &LocalZone) {
        Line::Task => {}
        Line::Mistake(e) => {
            toasts.push_bad(e.0);
            return;
        }
        Line::Protocol(p) => {
            // the repo is looked up here too, so a folder that is not one is said at once; sphered saves it and the
            // answer comes back as a toast ("Protocol saved · next Mon 09:00")
            match protocol_repo(p.repo.as_deref(), &home(), fallback_repo().as_deref(), is_repo) {
                Ok(repo) => {
                    let repo = repo.to_string_lossy().to_string();
                    if p.repo.is_some() {
                        remember(&repo);
                    }
                    svc.send(Cmd::ProtocolAdd { text: text.to_string(), repo: Some(repo) });
                }
                Err(e) => toasts.push(e),
            }
            return;
        }
    }
    match resolve(text) {
        Ok((repo, prompt)) => {
            let name = Path::new(&repo).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            toasts.push_quiet("Agent started");
            sound.play("done");
            let short: String = if prompt.chars().count() > 29 { prompt.chars().take(28).collect::<String>() + "…" } else { prompt.clone() };
            let words = format!("On it. I started an agent for “{short}” in {name}.");
            sound.speak(&words);
            say.say(words);
            // watch it work
            working.open_latest();
            remember(&repo);
            svc.send(Cmd::Start { repo, prompt });
        }
        Err(e) => toasts.push(e),
    }
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

fn is_repo(p: &Path) -> bool {
    p.join(".git").exists()
}

fn last_repo_file() -> PathBuf {
    sphere_proto::paths::default_state_dir().join("app").join("last-repo")
}

pub(crate) fn remember(repo: &str) {
    let f = last_repo_file();
    if let Some(d) = f.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(f, repo);
}

/// The last repo an agent was started in (or Code opened), when it is still one.
fn last_repo() -> Option<PathBuf> {
    let p = PathBuf::from(std::fs::read_to_string(last_repo_file()).ok()?.trim());
    is_repo(&p).then_some(p)
}

/// The folder a `code <path>` means: `~` and `~/x` are in your home, `/x` is absolute, `.` is the last repo used,
/// anything else is looked up in the last repo, then in `~/Projects`. The words say what is wrong when there is none.
pub fn resolve_code_path(arg: &str, last: Option<&Path>, home: &Path) -> Result<PathBuf, String> {
    let arg = arg.trim().trim_matches(|c| c == '"' || c == '\'');
    let tilde = |p: &Path| match p.strip_prefix(home) {
        Ok(r) if !home.as_os_str().is_empty() => format!("~/{}", r.display()),
        _ => p.display().to_string(),
    };
    let cands: Vec<PathBuf> = if arg == "." {
        match last {
            Some(l) => vec![l.to_path_buf()],
            None => return Err("No repo used yet: type  code <path>".into()),
        }
    } else if arg == "~" {
        vec![home.to_path_buf()]
    } else if let Some(r) = arg.strip_prefix("~/") {
        vec![home.join(r)]
    } else if arg.starts_with('/') {
        vec![PathBuf::from(arg)]
    } else {
        last.map(|l| l.join(arg)).into_iter().chain([home.join("Projects").join(arg)]).collect()
    };
    for p in &cands {
        if p.is_dir() {
            return Ok(std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()));
        }
    }
    let first = &cands[0];
    if first.is_file() {
        return Err(format!("{} is a file: code opens folders", tilde(first)));
    }
    Err(format!("No folder at {}", tilde(first)))
}

/// What `code <arg>` does: opens a folder, says what's wrong with a path, or (None) runs the words as a task. Only
/// something written as a path (`.`, `~`, `/x`, `./x`, `../x`) gets an error; other words that aren't a folder are a task.
pub fn code_arg(arg: &str, last: Option<&Path>, home: &Path) -> Option<Result<PathBuf, String>> {
    let a = arg.trim().trim_matches(|c| c == '"' || c == '\'');
    let pathy = a == "." || a == ".." || a == "~" || ["~/", "/", "./", "../"].iter().any(|p| a.starts_with(p));
    match resolve_code_path(arg, last, home) {
        Ok(d) => Some(Ok(d)),
        Err(e) if pathy => Some(Err(e)),
        Err(_) => None,
    }
}

/// `repo: task` names the repo; otherwise the last one used, otherwise the app's folder.
pub fn resolve(text: &str) -> Result<(String, String), String> {
    if let Some((head, rest)) = text.split_once(':') {
        let head = head.trim();
        if !head.is_empty() && !head.contains(' ') && !rest.trim().is_empty() {
            let p = repo_dir(head, &home());
            if is_repo(&p) {
                return Ok((p.to_string_lossy().to_string(), rest.trim().to_string()));
            }
            return Err(format!("No git repo at {}", p.display()));
        }
    }
    match fallback_repo() {
        Some(p) => Ok((p.to_string_lossy().to_string(), text.to_string())),
        None => Err("Which repo? Type  repo: task".into()),
    }
}

/// The folder a repo named in a line means: `~/x` is in your home, `/x` is absolute, anything else is a folder in
/// ~/Projects.
fn repo_dir(name: &str, home: &Path) -> PathBuf {
    if let Some(r) = name.strip_prefix("~/") {
        home.join(r)
    } else if name.starts_with('/') {
        PathBuf::from(name)
    } else {
        home.join("Projects").join(name)
    }
}

/// The repo a line that names none works in: the last one used, otherwise the folder the app was started in.
fn fallback_repo() -> Option<PathBuf> {
    last_repo().or_else(|| std::env::current_dir().ok().filter(|d| is_repo(d)))
}

/// The repo a protocol works in: the one its line names (it must be a git repo), else `fallback`.
pub fn protocol_repo(named: Option<&str>, home: &Path, fallback: Option<&Path>, is_repo: impl Fn(&Path) -> bool) -> Result<PathBuf, String> {
    match named {
        Some(name) => {
            let p = repo_dir(name, home);
            if is_repo(&p) {
                Ok(p)
            } else {
                Err(format!("No git repo at {}", p.display()))
            }
        }
        None => fallback.map(Path::to_path_buf).ok_or_else(|| "Which repo? Type  every day 9:00 repo: what to do".to_string()),
    }
}

/// What the bar says about the line being typed when it opens with a schedule.
#[derive(Clone, Debug, PartialEq)]
pub enum Preview {
    /// the protocol it would save: "every weekday 09:00 · next Mon 09:00 · sphere"
    Protocol(String),
    /// what is wrong with it, in words
    Mistake(String),
}

/// The preview of a line, or None for an ordinary task. `repo` looks up the folder a line works in (the one it names,
/// else the default): an error from it is a mistake too, so a folder that does not exist shows before Enter.
pub fn preview(text: &str, now: i64, zone: &dyn Zone, repo: impl Fn(Option<&str>) -> Result<PathBuf, String>) -> Option<Preview> {
    match protocol::classify(text, now, zone) {
        Line::Task => None,
        Line::Mistake(e) => Some(Preview::Mistake(e.0)),
        Line::Protocol(p) => {
            let dir = match repo(p.repo.as_deref()) {
                Ok(d) => d,
                Err(e) => return Some(Preview::Mistake(e)),
            };
            let name = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let mut parts = vec![p.schedule.describe(now, zone)];
            // a one-shot's schedule already says when
            if !p.schedule.is_one_shot() {
                if let Some(t) = p.schedule.next_after(now, zone) {
                    parts.push(format!("next {}", protocol::when_words(t, now, zone)));
                }
            }
            parts.push(name);
            Some(Preview::Protocol(parts.join(" · ")))
        }
    }
}

/// How many characters of the typed line fit in a bar `width` px wide: what the prompt, the hint and the gaps leave,
/// over the width of a character in the bar's font (a little more than it needs). A line's worth before the bar has
/// a width.
fn fits(width: Option<f32>) -> usize {
    match width {
        Some(w) if w > 0.0 => (((w - 130.0) / 8.8) as usize).max(12),
        _ => 60,
    }
}

/// The end of `text` that fits in `max` characters, led by a … when its start had to go: what is being typed stays in
/// view. A protocol line is long (when, where and what), so this matters more than it used to.
fn tail(text: &str, max: usize) -> String {
    let n = text.chars().count();
    if n <= max {
        return text.to_string();
    }
    let kept: String = text.chars().skip(n - max.saturating_sub(1)).collect();
    format!("…{kept}")
}

/// Redraws the bar when its text or focus changed, blinks the caret, and shows toasts.
#[allow(clippy::type_complexity)]
pub fn draw(
    bar: Res<CmdBar>,
    sim: Res<crate::core::Sim>,
    mut seen: Local<(u64, u64)>,
    frame: Query<&ComputedNode, With<CmdFrame>>,
    mut q: ParamSet<(
        Query<(&mut Text, &mut TextColor), With<CmdText>>,
        Query<&mut BorderColor, With<CmdFrame>>,
        Query<&mut BackgroundColor, With<CmdCaret>>,
    )>,
) {
    let line = role_color(Role::Line);
    if seen.0 != bar.version {
        seen.0 = bar.version;
        for (mut t, mut c) in &mut q.p0() {
            if bar.text.is_empty() && (!bar.open || bar.target.is_some()) {
                t.0 = match &bar.target {
                    Some(BarTarget::Task(_, title)) if bar.open => format!("Follow up with “{title}”  ·  Enter sends, Esc cancels"),
                    Some(BarTarget::Commit(_, branch, true)) if bar.open => format!("Amend {branch}  ·  Enter keeps the message, or type a new one  ·  Esc cancels"),
                    Some(BarTarget::Commit(_, branch, false)) if bar.open => format!("Commit message for {branch}  ·  Enter commits, Esc cancels"),
                    Some(BarTarget::Branch(_)) if bar.open => "Switch to branch…  ·  Enter switches, Esc cancels".to_string(),
                    Some(BarTarget::Filter) if bar.open => "Filter this folder…  ·  Enter filters, Esc cancels".to_string(),
                    _ => PLACEHOLDER.into(),
                };
                c.0 = with_alpha(line, 0.55);
            } else {
                let width = frame.single().ok().map(|n| n.size().x * n.inverse_scale_factor());
                t.0 = tail(&bar.text, fits(width));
                c.0 = role_color(Role::Hot);
            }
        }
        for mut b in &mut q.p1() {
            *b = BorderColor::all(if bar.open { with_alpha(role_color(Role::Fill), 0.9) } else { with_alpha(line, 0.34) });
        }
    }
    let on = bar.open && (sim.t % 1.0) < 0.5;
    let caret = if on { role_color(Role::Fill) } else { Color::NONE };
    for mut c in &mut q.p2() {
        c.set_if_neq(BackgroundColor(caret));
    }
}

/// The words of a preview, as the box shows them (capitals, like a toast).
fn preview_text(p: &Preview) -> String {
    match p {
        Preview::Protocol(s) | Preview::Mistake(s) => s.to_uppercase(),
    }
}

/// Shows what the line being typed would save as a protocol (or what is wrong with it) above the bar; eases in and out
/// in 0.2 s, and lifts the toasts out of its way while it is up.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn draw_preview(
    bar: Res<CmdBar>,
    sim: Res<crate::core::Sim>,
    mut seen: Local<u64>,
    mut want: Local<Option<Preview>>,
    mut shown: Local<Option<Preview>>,
    mut amt: Local<Option<Spring>>,
    mut boxes: Query<(&mut Visibility, &mut UiTransform, &mut BackgroundColor, &mut BorderColor), With<PreviewBox>>,
    mut texts: ParamSet<(Query<(&mut Text, &mut TextColor), With<PreviewLabel>>, Query<(&mut Text, &mut TextColor), With<PreviewRest>>)>,
    mut stack: Query<&mut Node, With<crate::visor::ToastStack>>,
) {
    if *seen != bar.version {
        *seen = bar.version;
        *want = if bar.open && bar.target.is_none() {
            preview(&bar.text, crate::core::unix_now(), &LocalZone, |named| protocol_repo(named, &home(), fallback_repo().as_deref(), is_repo))
        } else {
            None
        };
        if want.is_some() {
            *shown = want.clone();
        }
    }
    let amt = amt.get_or_insert_with(|| Spring::new(0.0, 160.0, 24.0));
    amt.t = if want.is_some() { 1.0 } else { 0.0 };
    amt.step(crate::motion::dt(crate::motion::Pace::Ui, sim.dt));
    let k = clamp(amt.x, 0.0, 1.0);
    if k < 0.01 && want.is_none() {
        *shown = None;
    }
    let Some(p) = shown.as_ref() else {
        for (mut v, ..) in &mut boxes {
            v.set_if_neq(Visibility::Hidden);
        }
        for mut n in &mut stack {
            n.bottom = px(crate::visor::TOASTS_BOTTOM);
        }
        return;
    };
    let (line, hot) = (role_color(Role::Line), role_color(Role::Hot));
    let accent = role_color(if matches!(p, Preview::Mistake(_)) { Role::Bad } else { Role::Fill });
    for (mut v, mut tr, mut bg, mut border) in &mut boxes {
        v.set_if_neq(Visibility::Inherited);
        tr.translation = Val2::px(0.0, (1.0 - k) * 6.0);
        bg.0 = Color::srgba(0.0, 0.0, 0.0, 0.72 * k);
        *border = BorderColor { left: with_alpha(accent, k), right: with_alpha(line, 0.13 * k), top: with_alpha(line, 0.13 * k), bottom: with_alpha(line, 0.13 * k) };
    }
    for (mut t, mut c) in &mut texts.p0() {
        c.0 = with_alpha(accent, k);
        if t.0 != "PROTOCOL" {
            t.0 = "PROTOCOL".into();
        }
    }
    let words = preview_text(p);
    // an error reads in the line colour, what would be saved in the bright one
    let rest = if matches!(p, Preview::Mistake(_)) { line } else { hot };
    for (mut t, mut c) in &mut texts.p1() {
        c.0 = with_alpha(rest, k);
        if t.0 != words {
            t.0 = words.clone();
        }
    }
    for mut n in &mut stack {
        n.bottom = px(crate::visor::TOASTS_BOTTOM + 22.0 * k);
    }
}

#[cfg(test)]
mod code_path_tests {
    use super::{code_arg, resolve_code_path};
    use std::path::Path;

    fn tree(name: &str) -> std::path::PathBuf {
        let base = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("sphere-cmdbar-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("home/Projects/x")).unwrap();
        std::fs::create_dir_all(base.join("repo/src")).unwrap();
        std::fs::write(base.join("repo/a.txt"), "a").unwrap();
        base
    }

    #[test]
    fn tilde_absolute_dot_and_relative_paths() {
        let b = tree("paths");
        let (home, repo) = (b.join("home"), b.join("repo"));
        assert_eq!(resolve_code_path("~/Projects/x", Some(&repo), &home), Ok(home.join("Projects/x")));
        assert_eq!(resolve_code_path("~", None, &home), Ok(home.clone()));
        assert_eq!(resolve_code_path(repo.to_str().unwrap(), None, &home), Ok(repo.clone()));
        assert_eq!(resolve_code_path(".", Some(&repo), &home), Ok(repo.clone()));
        // relative to the last repo first, then ~/Projects
        assert_eq!(resolve_code_path("src", Some(&repo), &home), Ok(repo.join("src")));
        assert_eq!(resolve_code_path("x", Some(&repo), &home), Ok(home.join("Projects/x")));
        assert_eq!(resolve_code_path("\"~/Projects/x\"", None, &home), Ok(home.join("Projects/x")));
        let _ = std::fs::remove_dir_all(&b);
    }

    #[test]
    fn a_missing_folder_says_so() {
        let b = tree("missing");
        let (home, repo) = (b.join("home"), b.join("repo"));
        let e = resolve_code_path("~/nope", Some(&repo), &home).unwrap_err();
        assert_eq!(e, "No folder at ~/nope");
        assert!(resolve_code_path(".", None, &home).unwrap_err().contains("No repo used yet"));
        assert!(resolve_code_path("a.txt", Some(&repo), &home).unwrap_err().contains("is a file"));
        assert!(resolve_code_path("/definitely/not/here", None, Path::new("/h")).unwrap_err().starts_with("No folder at /definitely"));
        let _ = std::fs::remove_dir_all(&b);
    }

    #[test]
    fn words_that_arent_a_folder_are_a_task() {
        let b = tree("task");
        let (home, repo) = (b.join("home"), b.join("repo"));
        // "code review this" starts an agent, as it did before `code <path>`
        assert_eq!(code_arg("review this", Some(&repo), &home), None);
        assert_eq!(code_arg("fix the login bug", None, &home), None);
        assert_eq!(code_arg("a.txt", Some(&repo), &home), None);
        // folders still open
        assert_eq!(code_arg("x", Some(&repo), &home), Some(Ok(home.join("Projects/x"))));
        assert_eq!(code_arg("src", Some(&repo), &home), Some(Ok(repo.join("src"))));
        // something written as a path says what's wrong
        assert_eq!(code_arg("~/nope", Some(&repo), &home), Some(Err("No folder at ~/nope".into())));
        assert!(matches!(code_arg(".", None, &home), Some(Err(e)) if e.contains("No repo used yet")));
        assert!(matches!(code_arg("./a.txt", Some(&repo), &home), Some(Err(_))));
        assert!(matches!(code_arg("/definitely/not/here", None, &home), Some(Err(_))));
        let _ = std::fs::remove_dir_all(&b);
    }
}

#[cfg(test)]
mod protocol_tests {
    use super::*;
    use protocol::FixedZone;

    /// 2026-10-19 09:00 UTC, a Monday.
    const NOW: i64 = 1_792_400_400;
    const UTC: FixedZone = FixedZone(0);

    /// Every folder is a repo; a line that names none works in `last`.
    fn anywhere(named: Option<&str>) -> Result<PathBuf, String> {
        Ok(PathBuf::from("/home/me/Projects").join(named.unwrap_or("last")))
    }

    fn said(line: &str) -> Option<Preview> {
        preview(line, NOW, &UTC, anywhere)
    }

    #[test]
    fn a_line_that_opens_with_a_schedule_previews_what_would_be_saved() {
        assert_eq!(said("every weekday 9:00 sphere: update the dependencies"), Some(Preview::Protocol("every weekday 09:00 · next Tue 09:00 · sphere".into())));
        // no repo named: the default one
        assert_eq!(said("every 2 hours: check the open PRs"), Some(Preview::Protocol("every 2 hours · next 11:00 · last".into())));
        assert_eq!(said("daily 9:00 api-server: tidy"), Some(Preview::Protocol("every day 09:00 · next Tue 09:00 · api-server".into())));
        // a one-shot says when once, not twice
        assert_eq!(said("at 18:00 sphere: ship it"), Some(Preview::Protocol("once · 18:00 · sphere".into())));
    }

    #[test]
    fn a_long_line_shows_its_end_where_the_typing_is() {
        assert_eq!(tail("fix the login test", 40), "fix the login test");
        assert_eq!(tail("every weekday 9:00 sphere: update", 10), "…e: update", "ten characters with the …");
        assert_eq!(tail("abcdef", 6), "abcdef", "exactly enough room");
        assert_eq!(tail("abcdefg", 6), "…cdefg");
        assert_eq!(tail("ééééé", 3), "…éé", "counts characters, not bytes");
        assert_eq!(tail("abc", 0), "…", "never a panic");
        // the room follows the bar's width
        assert_eq!(fits(None), 60);
        assert!(fits(Some(300.0)) < fits(Some(900.0)));
        assert_eq!(fits(Some(10.0)), 12, "a bar this narrow still shows a little");
    }

    #[test]
    fn an_ordinary_task_has_no_preview() {
        assert_eq!(said("fix the flaky login test"), None);
        assert_eq!(said("sphere: update the dependencies"), None);
        // words that merely start like a schedule are still a task
        assert_eq!(said("every test in the repo needs a comment: add them"), None);
        assert_eq!(said("in the readme, fix the typos"), None);
        assert_eq!(said(""), None);
    }

    #[test]
    fn a_schedule_that_does_not_read_says_what_to_fix() {
        let Some(Preview::Mistake(m)) = said("every weekday 9:00") else { panic!("a mistake") };
        assert!(m.starts_with("what should it do?"), "{m}");
        let Some(Preview::Mistake(m)) = said("every day 25:00 sphere: x") else { panic!("a mistake") };
        assert!(m.contains("25"), "{m}");
        // a folder that is not there is a mistake too
        let none = |_: Option<&str>| Err("No git repo at /home/me/Projects/nope".to_string());
        assert_eq!(preview("every day 9:00 nope: x", NOW, &UTC, none), Some(Preview::Mistake("No git repo at /home/me/Projects/nope".into())));
    }

    #[test]
    fn the_protocols_repo_is_the_one_named_else_the_last_used() {
        let home = Path::new("/home/me");
        let here = |p: &Path| p == Path::new("/home/me/Projects/sphere") || p == Path::new("/work/api") || p == Path::new("/home/me/x");
        assert_eq!(protocol_repo(Some("sphere"), home, None, here), Ok(PathBuf::from("/home/me/Projects/sphere")));
        assert_eq!(protocol_repo(Some("/work/api"), home, None, here), Ok(PathBuf::from("/work/api")));
        assert_eq!(protocol_repo(Some("~/x"), home, None, here), Ok(PathBuf::from("/home/me/x")));
        assert_eq!(protocol_repo(Some("nope"), home, None, here), Err("No git repo at /home/me/Projects/nope".into()));
        // none named: the last repo, and when there is none, a question
        assert_eq!(protocol_repo(None, home, Some(Path::new("/work/api")), here), Ok(PathBuf::from("/work/api")));
        assert!(protocol_repo(None, home, None, here).unwrap_err().starts_with("Which repo?"));
    }

    #[test]
    fn repo_names_in_a_line_mean_what_they_meant_for_a_task() {
        let home = Path::new("/home/me");
        assert_eq!(repo_dir("sphere", home), PathBuf::from("/home/me/Projects/sphere"));
        assert_eq!(repo_dir("~/work/x", home), PathBuf::from("/home/me/work/x"));
        assert_eq!(repo_dir("/srv/x", home), PathBuf::from("/srv/x"));
    }

    #[test]
    fn the_word_protocols_opens_the_card_and_longer_lines_do_not() {
        let mut open = crate::apps::Open::default();
        let (mut acts, mut toasts, mut sw, mut sound) = (crate::home::Acts::default(), Toasts::default(), crate::swarm::Swarm::default(), crate::sound::Sound::default());
        for word in ["protocols", "Protocols", "routines", " protocols "] {
            open.protocols = false;
            assert!(home_command(word, &mut acts, &mut open, &mut toasts, &mut sw, &mut sound) && open.protocols, "{word}");
        }
        open.protocols = false;
        // a task that starts with the word is a task, and a protocol line is not a command
        assert!(!home_command("protocols are slow, speed them up", &mut acts, &mut open, &mut toasts, &mut sw, &mut sound));
        assert!(!home_command("every day 9:00 sphere: tidy", &mut acts, &mut open, &mut toasts, &mut sw, &mut sound));
        assert!(!open.protocols);
    }
}
