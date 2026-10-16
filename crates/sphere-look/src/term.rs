//! Live terminal: a real PTY (portable-pty) running bash, VT state from alacritty_terminal, drawn into the
//! panel texture as Bevy UI text rows (coloured spans).
//!
//! M0 note: drawing rows as coloured text spans is enough to prove the look and the data path. Production
//! uses an instanced glyph-atlas renderer (one instance per cell, like Alacritty), which this replaces.
use crate::core::Sim;
use crate::fonts::UiFonts;
use crate::panel::*;
use crate::util::*;
use alacritty_terminal::{
    event::{Event, EventListener},
    grid::Dimensions,
    index::{Column, Line, Point},
    term::{cell::Flags, Config, Term, TermMode},
    vte::ansi::{Color as AColor, NamedColor, Processor},
};
use bevy::text::{LetterSpacing, LineHeight};
use bevy::{
    input::{keyboard::{Key, KeyboardInput}, ButtonState},
    prelude::*,
};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::{
    io::{Read, Write},
    sync::{mpsc, Arc, Mutex},
};

pub const COLS: usize = 66;
pub const ROWS: usize = 19;
pub const FONT_PX: f32 = 11.5;
pub const LINE_H: f32 = 17.25;
pub const TERM_W: f32 = 66.0 * 6.9 + 36.0;
pub const TERM_H: f32 = 14.0 + 23.0 + 6.0 + 4.0 + ROWS as f32 * LINE_H + 16.0;

/// Sphere palette for the 16 ANSI colours: cool blue-white by default, colour only for meaning.
pub fn ansi16() -> [Color; 16] {
    let h = |s: &str| hex(s);
    [
        h("#16252d"), // black
        h("#ff5a4f"), // red      (bad)
        h("#5fe3a1"), // green    (ok)
        h("#ffb347"), // yellow   (needs you)
        h("#5aa7d1"), // blue
        h("#b3a0ff"), // magenta
        h("#6fd8f0"), // cyan
        h("#cfe3ee"), // white
        h("#4f7384"), // bright black
        h("#ff8c82"), // bright red
        h("#92f0c0"), // bright green
        h("#ffd28a"), // bright yellow
        h("#8ec9ea"), // bright blue
        h("#d2c6ff"), // bright magenta
        h("#a8efff"), // bright cyan
        h("#f1f9fc"), // bright white
    ]
}

fn xterm256(i: u8, pal: &[Color; 16]) -> Color {
    match i {
        0..=15 => pal[i as usize],
        16..=231 => {
            let n = i - 16;
            let lv = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            Color::srgb_u8(lv(n / 36), lv((n / 6) % 6), lv(n % 6))
        }
        _ => {
            let v = 8 + (i - 232) * 10;
            Color::srgb_u8(v, v, v)
        }
    }
}

pub struct Listener {
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
}
impl EventListener for Listener {
    fn send_event(&self, ev: Event) {
        if let Event::PtyWrite(s) = ev {
            if let Ok(mut w) = self.writer.lock() {
                let _ = w.write_all(s.as_bytes());
                let _ = w.flush();
            }
        }
    }
}

struct Size;
impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        ROWS
    }
    fn screen_lines(&self) -> usize {
        ROWS
    }
    fn columns(&self) -> usize {
        COLS
    }
}

struct Inner {
    term: Term<Listener>,
    parser: Processor,
    rx: mpsc::Receiver<Vec<u8>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    _master: Box<dyn portable_pty::MasterPty + Send>,
    _child: Box<dyn portable_pty::Child + Send + Sync>,
}

#[derive(Resource)]
pub struct Terminal {
    inner: Mutex<Inner>,
    pub dirty: bool,
    pub exited: bool,
}

fn repo_dir() -> std::path::PathBuf {
    let cwd = std::env::current_dir().unwrap_or_default();
    let ok = std::process::Command::new("git").arg("-C").arg(&cwd).args(["rev-parse", "--show-toplevel"]).output();
    if let Ok(o) = ok {
        if o.status.success() {
            return std::path::PathBuf::from(String::from_utf8_lossy(&o.stdout).trim());
        }
    }
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

impl Terminal {
    pub fn spawn(first_cmd: &str) -> Result<Self, String> {
        let pty = native_pty_system();
        let pair = pty
            .openpty(PtySize { rows: ROWS as u16, cols: COLS as u16, pixel_width: 0, pixel_height: 0 })
            .map_err(|e| e.to_string())?;
        let mut cmd = CommandBuilder::new("bash");
        cmd.args(["--norc", "--noprofile", "-i"]);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        // a quiet Sphere prompt: cool blue, amber only for the sigil
        cmd.env("PS1", "\\[\\e[38;2;111;216;240m\\]sphere\\[\\e[0m\\] \\[\\e[38;2;166;211;230m\\]\\W\\[\\e[0m\\] \\[\\e[38;2;255;179;71m\\]›\\[\\e[0m\\] ");
        cmd.env("LS_COLORS", "di=1;36:ln=36:ex=1;32");
        cmd.env("GIT_PAGER", "cat");
        cmd.cwd(repo_dir());
        let child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let writer: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(pair.master.take_writer().map_err(|e| e.to_string())?));
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        let term = Term::new(Config::default(), &Size, Listener { writer: writer.clone() });
        {
            let mut w = writer.lock().unwrap();
            let _ = w.write_all(format!("{first_cmd}\n").as_bytes());
            let _ = w.flush();
        }
        Ok(Self {
            inner: Mutex::new(Inner { term, parser: Processor::new(), rx, writer, _master: pair.master, _child: child }),
            dirty: true,
            exited: false,
        })
    }

    pub fn send(&self, bytes: &[u8]) {
        if let Ok(i) = self.inner.lock() {
            if let Ok(mut w) = i.writer.lock() {
                let _ = w.write_all(bytes);
                let _ = w.flush();
            }
        }
    }
}

/// Drain PTY output into the VT state.
pub fn pump(mut t: ResMut<Terminal>) {
    let mut got = false;
    let mut gone = false;
    {
        let mut guard = t.inner.lock().unwrap();
        let i = &mut *guard;
        loop {
            match i.rx.try_recv() {
                Ok(b) => {
                    i.parser.advance(&mut i.term, &b);
                    got = true;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    gone = true;
                    break;
                }
            }
        }
    }
    if got {
        t.dirty = true;
    }
    if gone {
        t.exited = true;
    }
}

// ------------------------------------------------------------------ UI

#[derive(Resource)]
pub struct TermUi {
    pub rows: Vec<Entity>,
    pub hashes: Vec<u64>,
    pub status: Entity,
    pub last_focus: bool,
    pub last_blink: bool,
}

#[derive(Component)]
pub struct TermRow(pub usize);

pub fn build_term_ui(commands: &mut Commands, root: Entity, f: &UiFonts) -> TermUi {
    let line = role_color(Role::Line);
    let hot = role_color(Role::Hot);
    let dim = role_color(Role::Dim);
    let fill = role_color(Role::Fill);
    let body = commands
        .spawn(Node {
            width: percent(100),
            height: percent(100),
            flex_direction: FlexDirection::Column,
            padding: UiRect { left: px(18), right: px(18), top: px(14), bottom: px(16) },
            row_gap: px(4),
            ..default()
        })
        .id();
    commands.entity(root).add_child(body);
    let hdr = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: px(9),
                padding: UiRect { bottom: px(8), ..default() },
                margin: UiRect { bottom: px(6), ..default() },
                border: UiRect { bottom: px(1), ..default() },
                ..default()
            },
            BorderColor::all(with_alpha(line, 0.72)),
        ))
        .id();
    commands.entity(body).add_child(hdr);
    let sq = commands.spawn((Node { width: px(6), height: px(6), ..default() }, BackgroundColor(fill))).id();
    let t1 = commands.spawn((Text::new("TERMINAL"), font(&f.ui600, 12.0, 3.84), TextColor(hot), LineHeight::Px(14.4))).id();
    let status = commands
        .spawn((
            Text::new("BASH · LIVE PTY"),
            font(&f.ui500, 12.0, 2.16),
            TextColor(dim),
            LineHeight::Px(14.4),
            Node { margin: UiRect { left: auto(), ..default() }, ..default() },
        ))
        .id();
    commands.entity(hdr).add_children(&[sq, t1, status]);
    let grid = commands.spawn(Node { flex_direction: FlexDirection::Column, ..default() }).id();
    commands.entity(body).add_child(grid);
    let mut rows = Vec::new();
    for r in 0..ROWS {
        let e = commands
            .spawn((
                TermRow(r),
                Text::default(),
                TextFont { font: FontSource::Handle(f.mono.clone()), font_size: FontSize::Px(FONT_PX), ..default() },
                TextColor(line),
                LineHeight::Px(LINE_H),
                TextLayout { linebreak: LineBreak::NoWrap, ..default() },
                Node { height: px(LINE_H), ..default() },
            ))
            .id();
        commands.entity(grid).add_child(e);
        rows.push(e);
    }
    TermUi { rows, hashes: vec![u64::MAX; ROWS], status, last_focus: false, last_blink: true }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Style {
    fg: [u8; 4],
    bg: Option<[u8; 4]>,
    bold: bool,
}

fn rgba(c: Color) -> [u8; 4] {
    let s = c.to_srgba();
    [(s.red * 255.0) as u8, (s.green * 255.0) as u8, (s.blue * 255.0) as u8, (s.alpha * 255.0) as u8]
}
fn unrgba(c: [u8; 4]) -> Color {
    Color::srgba_u8(c[0], c[1], c[2], c[3])
}

/// Redraw rows whose cells changed. A row is a list of coloured text spans.
pub fn draw(
    mut commands: Commands,
    mut t: ResMut<Terminal>,
    mut ui: ResMut<TermUi>,
    f: Res<UiFonts>,
    focus: Res<Focus>,
    sim: Res<Sim>,
) {
    let blink = focus.terminal && (sim.t % 1.0) < 0.6;
    if !t.dirty && ui.last_focus == focus.terminal && ui.last_blink == blink {
        return;
    }
    ui.last_focus = focus.terminal;
    ui.last_blink = blink;
    t.dirty = false;
    let pal = ansi16();
    let def_fg = role_color(Role::Line);
    let guard = t.inner.lock().unwrap();
    let term = &guard.term;
    let content = term.renderable_content();
    let cursor = content.cursor.point;
    let cursor_visible = content.mode.contains(TermMode::SHOW_CURSOR);
    // gather visible cells
    let mut cells: Vec<Vec<(char, Style)>> = vec![vec![(' ', Style { fg: rgba(def_fg), bg: None, bold: false }); COLS]; ROWS];
    let grid = term.grid();
    for r in 0..ROWS {
        for c in 0..COLS {
            let cell = &grid[Point::new(Line(r as i32), Column(c))];
            let mut fg = resolve(cell.fg, &pal, def_fg, true);
            let mut bg = if matches!(cell.bg, AColor::Named(NamedColor::Background)) { None } else { Some(resolve(cell.bg, &pal, Color::BLACK, false)) };
            if cell.flags.contains(Flags::INVERSE) {
                let nb = fg;
                fg = bg.unwrap_or(Color::srgba(0.0, 0.0, 0.0, 1.0));
                bg = Some(nb);
            }
            if cell.flags.contains(Flags::BOLD) {
                // bold as bright for the 8 normal colours
                if let AColor::Named(n) = cell.fg {
                    let i = n as usize;
                    if i < 8 {
                        fg = pal[i + 8];
                    }
                }
            }
            if cell.flags.contains(Flags::DIM) {
                fg = with_alpha(fg, 0.6);
            }
            let ch = if cell.flags.contains(Flags::HIDDEN) || cell.flags.contains(Flags::WIDE_CHAR_SPACER) { ' ' } else { cell.c };
            cells[r][c] = (ch, Style { fg: rgba(fg), bg: bg.map(rgba), bold: cell.flags.contains(Flags::BOLD) });
        }
    }
    // cursor
    if cursor_visible && (cursor.line.0 as usize) < ROWS && cursor.column.0 < COLS {
        let (r, c) = (cursor.line.0 as usize, cursor.column.0);
        let cell = &mut cells[r][c];
        if focus.terminal {
            if blink {
                cell.1.bg = Some(rgba(role_color(Role::Fill)));
                cell.1.fg = [0, 0, 0, 255];
            }
        } else {
            cell.1.bg = Some(rgba(with_alpha(role_color(Role::Line), 0.35)));
        }
    }
    drop(guard);
    for r in 0..ROWS {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        use std::hash::{Hash, Hasher};
        for (ch, s) in &cells[r] {
            ch.hash(&mut h);
            s.hash(&mut h);
        }
        let hv = h.finish();
        if hv == ui.hashes[r] {
            continue;
        }
        ui.hashes[r] = hv;
        let row = ui.rows[r];
        // trim trailing default blanks
        let mut end = COLS;
        while end > 0 && cells[r][end - 1].0 == ' ' && cells[r][end - 1].1.bg.is_none() {
            end -= 1;
        }
        let mut runs: Vec<(String, Style)> = Vec::new();
        for (ch, s) in &cells[r][..end] {
            match runs.last_mut() {
                Some((txt, st)) if st == s => txt.push(*ch),
                _ => runs.push((ch.to_string(), *s)),
            }
        }
        commands.entity(row).despawn_children();
        commands.entity(row).insert(Text::default());
        for (txt, s) in runs {
            let mut e = commands.spawn((
                TextSpan::new(txt),
                TextColor(unrgba(s.fg)),
                TextFont {
                    font: FontSource::Handle(if s.bold { f.mono_bold.clone() } else { f.mono.clone() }),
                    font_size: FontSize::Px(FONT_PX),
                    ..default()
                },
            ));
            if let Some(bg) = s.bg {
                e.insert(TextBackgroundColor(unrgba(bg)));
            }
            let id = e.id();
            commands.entity(row).add_child(id);
        }
    }
}

fn resolve(c: AColor, pal: &[Color; 16], default: Color, fg: bool) -> Color {
    match c {
        AColor::Spec(rgb) => Color::srgb_u8(rgb.r, rgb.g, rgb.b),
        AColor::Indexed(i) => xterm256(i, pal),
        AColor::Named(n) => {
            let i = n as usize;
            if i < 16 {
                pal[i]
            } else {
                match n {
                    NamedColor::Foreground | NamedColor::BrightForeground => {
                        if fg {
                            default
                        } else {
                            Color::NONE
                        }
                    }
                    NamedColor::DimForeground => with_alpha(default, 0.6),
                    NamedColor::DimBlack => pal[0],
                    NamedColor::DimRed => with_alpha(pal[1], 0.7),
                    NamedColor::DimGreen => with_alpha(pal[2], 0.7),
                    NamedColor::DimYellow => with_alpha(pal[3], 0.7),
                    NamedColor::DimBlue => with_alpha(pal[4], 0.7),
                    NamedColor::DimMagenta => with_alpha(pal[5], 0.7),
                    NamedColor::DimCyan => with_alpha(pal[6], 0.7),
                    NamedColor::DimWhite => with_alpha(pal[7], 0.7),
                    _ => default,
                }
            }
        }
    }
}

// ------------------------------------------------------------------ keyboard

pub fn key_bytes(ev: &KeyboardInput, ctrl: bool, alt: bool, app_cursor: bool) -> Option<Vec<u8>> {
    let arrow = |c: u8| -> Vec<u8> {
        if app_cursor {
            vec![0x1b, b'O', c]
        } else {
            vec![0x1b, b'[', c]
        }
    };
    let b = match &ev.logical_key {
        Key::Enter => vec![b'\r'],
        Key::Backspace => vec![0x7f],
        Key::Tab => vec![b'\t'],
        Key::ArrowUp => arrow(b'A'),
        Key::ArrowDown => arrow(b'B'),
        Key::ArrowRight => arrow(b'C'),
        Key::ArrowLeft => arrow(b'D'),
        Key::Home => b"\x1b[H".to_vec(),
        Key::End => b"\x1b[F".to_vec(),
        Key::Delete => b"\x1b[3~".to_vec(),
        Key::PageUp => b"\x1b[5~".to_vec(),
        Key::PageDown => b"\x1b[6~".to_vec(),
        Key::Insert => b"\x1b[2~".to_vec(),
        Key::Character(s) => {
            let mut out = Vec::new();
            if ctrl {
                if let Some(ch) = s.chars().next() {
                    let lc = ch.to_ascii_lowercase();
                    if lc.is_ascii_lowercase() {
                        out.push(lc as u8 - b'a' + 1);
                    } else if ch == '[' {
                        out.push(0x1b);
                    }
                }
            } else {
                if alt {
                    out.push(0x1b);
                }
                out.extend_from_slice(ev.text.as_deref().unwrap_or(s.as_str()).as_bytes());
            }
            out
        }
        Key::Space => {
            if ctrl {
                vec![0]
            } else {
                vec![b' ']
            }
        }
        _ => match &ev.text {
            Some(t) if !t.is_empty() && !ctrl => t.as_bytes().to_vec(),
            _ => return None,
        },
    };
    if b.is_empty() {
        None
    } else {
        Some(b)
    }
}
pub fn app_cursor(t: &Terminal) -> bool {
    t.inner.lock().map(|g| g.term.mode().contains(TermMode::APP_CURSOR)).unwrap_or(false)
}

pub fn read_keys(
    mut ev: MessageReader<KeyboardInput>,
    keys: Res<ButtonInput<KeyCode>>,
    mut focus: ResMut<Focus>,
    mut ctl: ResMut<crate::core::CoreCtl>,
    t: Res<Terminal>,
) {
    let ctrl = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
    let alt = keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight);
    for e in ev.read() {
        if e.state != ButtonState::Pressed {
            continue;
        }
        if !focus.terminal {
            match e.key_code {
                KeyCode::KeyT => focus.terminal = true,
                KeyCode::KeyD => {
                    let m = if ctl.mode == crate::core::Mode::Depth { crate::core::Mode::Flat } else { crate::core::Mode::Depth };
                    ctl.set_mode(m);
                }
                _ => {}
            }
            continue;
        }
        if e.key_code == KeyCode::Escape {
            focus.terminal = false;
            continue;
        }
        if let Some(b) = key_bytes(e, ctrl, alt, app_cursor(&t)) {
            t.send(&b);
        }
    }
}
