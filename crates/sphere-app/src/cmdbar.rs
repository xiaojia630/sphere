//! The command bar, pinned to the bottom of the screen like the prototype's dock. `/` or Cmd+K focuses it.
//! Typing a task starts an agent through sphered; `repo: task` picks the repo (a folder in ~/Projects, or a
//! path), otherwise the last repo used, otherwise the folder the app was started in.
use crate::fonts::UiFonts;
use crate::live::Toasts;
use crate::panel::{font, with_alpha, Focus};
use crate::service::{Cmd, Service};
use crate::util::*;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::text::LineHeight;
use std::path::{Path, PathBuf};

#[derive(Resource, Default)]
pub struct CmdBar {
    pub open: bool,
    pub text: String,
    /// Bumps when `text` or `open` change, so the UI updates only then.
    pub version: u64,
}

#[derive(Component)]
pub struct CmdFrame;
#[derive(Component)]
pub struct CmdText;
#[derive(Component)]
pub struct CmdCaret;
#[derive(Component)]
pub struct ToastText;

const PLACEHOLDER: &str = "Ask Sphere or start a task  ·  repo: task  ·  > reply to the last agent";

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
                padding: UiRect { left: px(8), right: px(14), ..default() },
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
    // the mic (voice arrives with M5): a 32 px ring with a small capsule
    let mic = commands
        .spawn((
            Node { width: px(32), height: px(32), flex_shrink: 0.0, border: UiRect::all(px(1)), border_radius: BorderRadius::MAX, justify_content: JustifyContent::Center, align_items: AlignItems::Center, ..default() },
            BorderColor::all(with_alpha(line, 0.34)),
        ))
        .id();
    let cap = commands
        .spawn((Node { width: px(5), height: px(9), border: UiRect::all(px(1)), border_radius: BorderRadius::all(px(2.5)), ..default() }, BorderColor::all(role_color(Role::Hot))))
        .id();
    commands.entity(mic).add_child(cap);
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
    commands.entity(bar).add_children(&[mic, prompt, text, caret, spacer, hint]);
    commands.entity(dock).add_child(bar);
    commands.entity(root.0).add_child(wrap);
}

fn super_down(keys: &ButtonInput<KeyCode>) -> bool {
    keys.pressed(KeyCode::SuperLeft) || keys.pressed(KeyCode::SuperRight)
}

/// Keys while the bar is open, and the keys that open it. Runs before the card and the terminal.
#[allow(clippy::too_many_arguments)]
pub fn keys(
    mut ev: MessageReader<KeyboardInput>,
    keys: Res<ButtonInput<KeyCode>>,
    mut bar: ResMut<CmdBar>,
    mut focus: ResMut<Focus>,
    svc: Res<Service>,
    mut toasts: ResMut<Toasts>,
    mut terms: ResMut<crate::term::Terms>,
    moment: Res<crate::moment::Moment>,
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
            }
            Key::Enter => {
                let text = std::mem::take(&mut bar.text);
                bar.open = false;
                run(text.trim(), &svc, &mut toasts, &mut terms, &mut focus);
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

fn run(text: &str, svc: &Service, toasts: &mut Toasts, terms: &mut crate::term::Terms, focus: &mut Focus) {
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
    match resolve(text) {
        Ok((repo, prompt)) => {
            let name = Path::new(&repo).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            toasts.push(format!("Starting in {name}"));
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

fn remember(repo: &str) {
    let f = last_repo_file();
    if let Some(d) = f.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(f, repo);
}

/// `repo: task` names the repo; otherwise the last one used, otherwise the app's folder.
pub fn resolve(text: &str) -> Result<(String, String), String> {
    if let Some((head, rest)) = text.split_once(':') {
        let head = head.trim();
        if !head.is_empty() && !head.contains(' ') && !rest.trim().is_empty() {
            let p = if let Some(r) = head.strip_prefix("~/") {
                home().join(r)
            } else if head.starts_with('/') {
                PathBuf::from(head)
            } else {
                home().join("Projects").join(head)
            };
            if is_repo(&p) {
                return Ok((p.to_string_lossy().to_string(), rest.trim().to_string()));
            }
            return Err(format!("No git repo at {}", p.display()));
        }
    }
    if let Ok(s) = std::fs::read_to_string(last_repo_file()) {
        let p = PathBuf::from(s.trim());
        if is_repo(&p) {
            return Ok((p.to_string_lossy().to_string(), text.to_string()));
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        if is_repo(&cwd) {
            return Ok((cwd.to_string_lossy().to_string(), text.to_string()));
        }
    }
    Err("Which repo? Type  repo: task".into())
}

/// Redraws the bar when its text or focus changed, blinks the caret, and shows toasts.
#[allow(clippy::type_complexity)]
pub fn draw(
    bar: Res<CmdBar>,
    sim: Res<crate::core::Sim>,
    mut seen: Local<(u64, u64)>,
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
            if bar.text.is_empty() && !bar.open {
                t.0 = PLACEHOLDER.into();
                c.0 = with_alpha(line, 0.55);
            } else {
                t.0 = bar.text.clone();
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
