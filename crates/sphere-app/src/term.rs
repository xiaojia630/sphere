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
use bevy::text::LineHeight;
use bevy::{
    input::{keyboard::{Key, KeyboardInput}, ButtonState},
    prelude::*,
};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::{
    io::{Read, Write},
    sync::{mpsc, Arc, Mutex},
    time::Instant,
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
    /// a PTY in this process (look test, or when the host can't be reached)
    _master: Option<Box<dyn portable_pty::MasterPty + Send>>,
    child: Option<Box<dyn portable_pty::Child + Send + Sync>>,
    /// a shell held by sphere-host (it outlives the app)
    host: Option<crate::hostterm::Handle>,
    /// reads what the tab prints for the commands that start and end in it (the emulator still gets every byte)
    scan: crate::scan::Scanner,
    /// bytes of old output the host is still replaying into this re-attached tab: its commands ended long ago,
    /// so they get no Fix it
    replay: u64,
    /// the look test's tab: its directory is its repo, no need to ask git
    in_repo: bool,
    /// answers from the threads that check where a failed command ran (`Terminal::fix`)
    checked_tx: mpsc::Sender<crate::fixit::Checked>,
    checked_rx: mpsc::Receiver<crate::fixit::Checked>,
    /// a scripted tab keeps the sender of its bytes, so the tab doesn't look like its shell exited
    _script: Option<mpsc::Sender<Vec<u8>>>,
}

impl Inner {
    fn new(term: Term<Listener>, rx: mpsc::Receiver<Vec<u8>>, writer: Arc<Mutex<Box<dyn Write + Send>>>) -> Self {
        let (checked_tx, checked_rx) = mpsc::channel();
        Self { term, parser: Processor::new(), rx, writer, _master: None, child: None, host: None, scan: crate::scan::Scanner::new(), replay: 0, in_repo: false, checked_tx, checked_rx, _script: None }
    }
}

pub struct Terminal {
    inner: Mutex<Inner>,
    pub dirty: bool,
    pub exited: bool,
    /// The app to show again when this tab's program exits (a Helix edit from Code or Files).
    pub back_to: Option<usize>,
    /// The program's name for the header (`zsh`, `bash`).
    pub name: String,
    /// The newest command that failed in this tab inside a git work tree, which Fix it offers a fix for. It goes
    /// when the next command starts, or when it is dismissed or used.
    pub fix: Option<crate::fixit::Failure>,
    /// Counts the commands that started in this tab, and the dismissals: an answer about an older failure is stale.
    pub fix_seq: u64,
}

/// The terminal tabs. Shells live in the app for now: sphered has no terminal API yet, so they end when the
/// app quits (docs/sphered.md moves them into sphere-host).
#[derive(Resource, Default)]
pub struct Terms {
    pub tabs: Vec<Terminal>,
    pub active: usize,
    /// Bumps when tabs open, close or switch, so the panel redraws everything.
    pub version: u64,
    /// The first tab's look-test setup (bash with the Sphere prompt) for `--shot`, `--bench` and `--demo`.
    pub demo: bool,
    /// Tabs being connected off the main thread at start (sphere-host may need starting); shown as connecting.
    pub pending: Option<Mutex<mpsc::Receiver<Vec<Terminal>>>>,
    /// The app a closed tab hands back to (its `back_to`), for `apps::apply`.
    pub returned: Option<usize>,
}

impl Terms {
    /// Connects your tabs in the background: the ones still alive in sphere-host from the last run, or a new
    /// shell (running `first` when given). The first frame doesn't wait for the host.
    pub fn connect(&mut self, first: Option<String>) {
        let (tx, rx) = mpsc::channel();
        self.pending = Some(Mutex::new(rx));
        let _ = std::thread::Builder::new().name("sphere-terms-connect".into()).spawn(move || {
            let mut tabs: Vec<Terminal> = crate::stats::timed("hostterm::running (connect/start sphere-host)", crate::hostterm::running).iter().filter_map(|ch| Terminal::host(Some(ch))).collect();
            if tabs.is_empty() {
                let t = Terminal::host(None).or_else(|| Terminal::spawn(None, false).map_err(|e| eprintln!("could not start a terminal: {e}")).ok());
                if let (Some(cmd), Some(t)) = (first.as_deref(), t.as_ref()) {
                    t.send(format!("{cmd}\n").as_bytes());
                }
                tabs.extend(t);
            }
            let _ = tx.send(tabs);
        });
    }

    pub fn active(&self) -> Option<&Terminal> {
        self.tabs.get(self.active)
    }
    pub fn open_tab(&mut self) {
        // your shells live in sphere-host so they outlive the app; in the app only if the host can't be reached
        if !self.demo {
            if let Some(t) = Terminal::host(None) {
                self.tabs.push(t);
                self.active = self.tabs.len() - 1;
                self.version += 1;
                return;
            }
        }
        match Terminal::spawn(None, false) {
            Ok(t) => {
                self.tabs.push(t);
                self.active = self.tabs.len() - 1;
            }
            Err(e) => eprintln!("could not start a terminal: {e}"),
        }
        self.version += 1;
    }
    pub fn close(&mut self, i: usize) {
        if i < self.tabs.len() {
            let t = self.tabs.remove(i);
            t.kill();
            if self.active >= self.tabs.len() && self.active > 0 {
                self.active = self.tabs.len() - 1;
            }
            if self.tabs.is_empty() {
                self.open_tab();
            }
            self.version += 1;
        }
    }
    pub fn switch(&mut self, i: usize) {
        if i < self.tabs.len() && i != self.active {
            self.active = i;
            self.version += 1;
        }
    }
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
    /// `look`: the look test's bash with the Sphere prompt; otherwise your own shell as a login shell.
    pub fn spawn(first_cmd: Option<&str>, look: bool) -> Result<Self, String> {
        let pty = native_pty_system();
        let pair = pty
            .openpty(PtySize { rows: ROWS as u16, cols: COLS as u16, pixel_width: 0, pixel_height: 0 })
            .map_err(|e| e.to_string())?;
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
        let mut cmd = if look {
            let mut c = CommandBuilder::new("bash");
            c.args(["--norc", "--noprofile", "-i"]);
            // a quiet Sphere prompt: cool blue, amber only for the sigil
            c.env("PS1", "\\[\\e[38;2;111;216;240m\\]sphere\\[\\e[0m\\] \\[\\e[38;2;166;211;230m\\]\\W\\[\\e[0m\\] \\[\\e[38;2;255;179;71m\\]›\\[\\e[0m\\] ");
            c.env("LS_COLORS", "di=1;36:ln=36:ex=1;32");
            c.env("GIT_PAGER", "cat");
            c.cwd(repo_dir());
            c
        } else {
            let launch = crate::shellint::launch(&shell);
            let mut c = CommandBuilder::new(&shell);
            c.args(&launch.args);
            for (k, v) in &launch.env {
                c.env(k, v);
            }
            c.cwd(std::env::var_os("HOME").unwrap_or_default());
            c
        };
        let name = if look { "bash".to_string() } else { std::path::Path::new(&shell).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default() };
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
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
        if let Some(first_cmd) = first_cmd {
            // wait for bash to draw its first prompt so the command is not echoed twice
            let w = writer.clone();
            let line = format!("{first_cmd}\n");
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(500));
                let mut w = w.lock().unwrap();
                let _ = w.write_all(line.as_bytes());
                let _ = w.flush();
            });
        }
        let mut inner = Inner::new(term, rx, writer);
        inner._master = Some(pair.master);
        inner.child = Some(child);
        Ok(Self::with(inner, name))
    }

    fn with(inner: Inner, name: String) -> Self {
        Self { inner: Mutex::new(inner), dirty: true, exited: false, back_to: None, name, fix: None, fix_seq: 0 }
    }

    /// A tab with no shell: it prints what is sent through the returned sender, and takes its directory for the
    /// repo when `in_repo`. The look test and the tests use it.
    pub fn scripted(in_repo: bool) -> (Self, mpsc::Sender<Vec<u8>>) {
        let (tx, rx) = mpsc::channel();
        let writer: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(Box::new(std::io::sink())));
        let term = Term::new(Config::default(), &Size, Listener { writer: writer.clone() });
        let mut inner = Inner::new(term, rx, writer);
        inner.in_repo = in_repo;
        inner._script = Some(tx.clone());
        (Self::with(inner, "bash".to_string()), tx)
    }

    /// Takes the failure away (the strip goes), whatever is still being looked up for it too.
    pub fn dismiss_fix(&mut self) -> Option<crate::fixit::Failure> {
        self.fix_seq += 1;
        self.fix.take()
    }

    pub fn kill(&self) {
        if let Ok(mut i) = self.inner.lock() {
            if let Some(c) = i.child.as_mut() {
                let _ = c.kill();
            }
            if let Some(h) = &i.host {
                h.kill();
            }
        }
    }

    /// A shell in sphere-host: new, or a tab from an earlier run re-attached.
    pub fn host(attach: Option<&sphere_host::ChildInfo>) -> Option<Self> {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
        let (h, rx, w) = match attach {
            Some(ch) => crate::hostterm::attach(ch)?,
            None => crate::hostterm::spawn(COLS as u16, ROWS as u16, &shell)?,
        };
        let name = std::path::Path::new(attach.map(|c| c.cmd.as_str()).unwrap_or(&shell)).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let writer: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(Box::new(w)));
        let term = Term::new(Config::default(), &Size, Listener { writer: writer.clone() });
        let mut inner = Inner::new(term, rx, writer);
        inner.host = Some(h);
        inner.replay = attach.map(crate::hostterm::replay_len).unwrap_or(0);
        Some(Self::with(inner, name))
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

/// Drain every tab's PTY output into its VT state; close tabs whose shell exited.
pub fn pump(mut terms: ResMut<Terms>) {
    let got = terms.pending.as_ref().and_then(|p| p.lock().ok()?.try_recv().ok());
    if let Some(tabs) = got {
        terms.pending = None;
        terms.tabs.extend(tabs);
        terms.version += 1;
    }
    for t in terms.tabs.iter_mut() {
        pump_one(t);
    }
    if let Some(i) = terms.tabs.iter().position(|t| t.exited) {
        if let Some(a) = terms.tabs[i].back_to {
            terms.returned = Some(a);
        }
        terms.close(i);
    }
}

fn pump_one(t: &mut Terminal) {
    pump_at(t, Instant::now());
}

/// Reads what the tab has printed, as `pump_one`, with `now` as the time its commands start and end at.
pub fn pump_at(t: &mut Terminal, now: Instant) {
    let mut got = false;
    let mut gone = false;
    {
        let mut guard = t.inner.lock().unwrap();
        let i = &mut *guard;
        // a re-attached tab replays up to 1 MB: parse it over a few frames rather than in one
        let mut budget: usize = 256 * 1024;
        while budget > 0 {
            match i.rx.try_recv() {
                Ok(b) => {
                    // the scanner reads the bytes on their way to the emulator; the old output of a re-attached
                    // tab is read too (a command may still be running in it) but ends no command for Fix it
                    let old = (i.replay as usize).min(b.len());
                    i.replay -= old as u64;
                    i.scan.feed(&b[..old], now);
                    for ev in i.scan.feed(&b[old..], now) {
                        match ev {
                            crate::scan::Event::Started => {
                                t.fix = None;
                                t.fix_seq += 1;
                            }
                            crate::scan::Event::Finished(done) if crate::fixit::wants_fix(done.exit, &done.command) => crate::fixit::check(done, t.fix_seq, i.in_repo, i.checked_tx.clone()),
                            crate::scan::Event::Finished(_) => {}
                        }
                    }
                    i.parser.advance(&mut i.term, &b);
                    budget = budget.saturating_sub(b.len());
                    got = true;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    gone = true;
                    break;
                }
            }
        }
        // a failure that was looked up meanwhile: it shows unless another command has started since
        while let Ok(c) = i.checked_rx.try_recv() {
            if c.seq == t.fix_seq {
                t.fix = c.failure;
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
    /// the lines, in a window that cuts what Fix it lifts out of the strip's way
    pub grid: Entity,
    /// Fix it's strip at the bottom of the panel
    pub strip: crate::fixit::Strip,
    /// the line the cursor was on when the lines were last drawn
    pub cursor_row: usize,
    pub version: u64,
    pub last_focus: bool,
    pub last_blink: bool,
}

#[derive(Component)]
#[allow(dead_code)]
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
    // the lines sit in a window the height of the screen; Fix it moves them up inside it (`grid.top`) when its strip
    // would cover the cursor's line
    let window = commands.spawn(Node { height: px(ROWS as f32 * LINE_H), flex_shrink: 0.0, overflow: Overflow::clip(), ..default() }).id();
    commands.entity(body).add_child(window);
    let grid = commands.spawn(Node { position_type: PositionType::Absolute, left: px(0), right: px(0), top: px(0), flex_direction: FlexDirection::Column, ..default() }).id();
    commands.entity(window).add_child(grid);
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
    let strip = crate::fixit::build_strip(commands, body, f);
    TermUi { rows, hashes: vec![u64::MAX; ROWS], status, grid, strip, cursor_row: 0, version: u64::MAX, last_focus: false, last_blink: true }
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
#[allow(clippy::too_many_arguments)]
pub fn draw(
    mut commands: Commands,
    mut terms: ResMut<Terms>,
    mut ui: ResMut<TermUi>,
    f: Res<UiFonts>,
    focus: Res<Focus>,
    sim: Res<Sim>,
    mut texts: Query<&mut Text>,
) {
    let blink = focus.terminal && (sim.t % 1.0) < 0.6;
    let switched = ui.version != terms.version;
    if switched {
        ui.version = terms.version;
        ui.hashes.iter_mut().for_each(|h| *h = u64::MAX);
        if let Ok(mut s) = texts.get_mut(ui.status) {
            s.0 = tabs_label(&terms);
        }
    }
    let active = terms.active;
    let Some(t) = terms.tabs.get_mut(active) else { return };
    if !switched && !t.dirty && ui.last_focus == focus.terminal && ui.last_blink == blink {
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
    ui.cursor_row = (cursor.line.0.max(0) as usize).min(ROWS - 1);
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
    #[allow(clippy::needless_range_loop)]
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

/// The header's right side: the active tab and how many there are.
fn tabs_label(terms: &Terms) -> String {
    let name = terms.active().map(|t| t.name.to_uppercase()).unwrap_or_default();
    if terms.tabs.is_empty() && terms.pending.is_some() {
        "CONNECTING…".to_string()
    } else if terms.tabs.len() > 1 {
        format!("{name} · TAB {} / {}", terms.active + 1, terms.tabs.len())
    } else if terms.demo {
        "BASH · LIVE PTY".to_string()
    } else {
        format!("{name} · CMD T NEW TAB")
    }
}

/// Cmd+T new tab, Cmd+W close, Cmd+1..9 switch (always); T focuses, Esc releases; D toggles Depth. While
/// focused, every other key goes to the active shell. The command bar and the card take keys first.
pub fn read_keys(
    mut ev: MessageReader<KeyboardInput>,
    keys: Res<ButtonInput<KeyCode>>,
    mut focus: ResMut<Focus>,
    mut terms: ResMut<Terms>,
    moment: Res<crate::moment::Moment>,
    working: Res<crate::working::Working>,
    swarm: Res<crate::swarm::Swarm>,
) {
    let ctrl = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
    let alt = keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight);
    let cmd = keys.pressed(KeyCode::SuperLeft) || keys.pressed(KeyCode::SuperRight);
    for e in ev.read() {
        if e.state != ButtonState::Pressed || focus.cmd {
            continue;
        }
        if cmd {
            let n = match e.key_code {
                KeyCode::Digit1 => Some(0),
                KeyCode::Digit2 => Some(1),
                KeyCode::Digit3 => Some(2),
                KeyCode::Digit4 => Some(3),
                KeyCode::Digit5 => Some(4),
                KeyCode::Digit6 => Some(5),
                KeyCode::Digit7 => Some(6),
                KeyCode::Digit8 => Some(7),
                KeyCode::Digit9 => Some(8),
                _ => None,
            };
            match e.key_code {
                KeyCode::KeyT => {
                    terms.open_tab();
                    focus.terminal = true;
                }
                KeyCode::KeyW => {
                    let i = terms.active;
                    terms.close(i);
                }
                KeyCode::KeyV if focus.terminal => {
                    if let (Ok(out), Some(t)) = (std::process::Command::new("pbpaste").output(), terms.active()) {
                        t.send(&out.stdout);
                    }
                }
                _ => {}
            }
            if let Some(i) = n {
                terms.switch(i);
                focus.terminal = true;
            }
            continue;
        }
        if moment.shown && !focus.terminal {
            continue;
        }
        if !focus.terminal {
            // in the Working view T brings the agent's output forward instead
            if e.key_code == KeyCode::KeyT && !working.open && !swarm.open {
                focus.terminal = true;
            }
            continue;
        }
        if e.key_code == KeyCode::Escape {
            focus.terminal = false;
            continue;
        }
        if let Some(t) = terms.active() {
            if let Some(b) = key_bytes(e, ctrl, alt, app_cursor(t)) {
                t.send(&b);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixit::{base64, Failure};
    use std::{path::Path, time::Duration};

    fn osc(body: &str) -> String {
        format!("\x1b]{body}\x07")
    }

    /// The start of a command as the shell integration prints it: its line, then the mark that its output starts.
    fn start(cmd: &str) -> String {
        format!("{}{}", osc(&format!("7700;cmd;{}", base64(cmd))), osc("133;C"))
    }

    /// The end of a command: the directory, the exit status, the next prompt.
    fn end(cwd: &str, exit: i32) -> String {
        format!("{}{}{}", osc(&format!("7;file://host{cwd}")), osc(&format!("133;D;{exit}")), osc("133;A"))
    }

    fn session(cmd: &str, cwd: &str, output: &str, exit: i32) -> Vec<u8> {
        format!("{}{output}{}", start(cmd), end(cwd, exit)).into_bytes()
    }

    /// Pumps the tab until `done`: the thread that looks where a command ran answers a moment later.
    fn pump_until(t: &mut Terminal, what: &str, done: impl Fn(&Terminal) -> bool) {
        let give_up = Instant::now() + Duration::from_secs(20);
        loop {
            pump_one(t);
            if done(t) {
                return;
            }
            assert!(Instant::now() < give_up, "gave up waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Pumps the tab, then once more after the lookup would have answered.
    fn pump_and_wait(t: &mut Terminal) {
        pump_one(t);
        std::thread::sleep(Duration::from_millis(150));
        pump_one(t);
    }

    fn screen(t: &Terminal) -> String {
        let g = t.inner.lock().unwrap();
        let grid = g.term.grid();
        (0..ROWS).map(|r| (0..COLS).map(|c| grid[Point::new(Line(r as i32), Column(c))].c).collect::<String>().trim_end().to_string()).collect::<Vec<_>>().join("\n")
    }

    fn failure() -> Failure {
        Failure { command: "make".into(), repo: "/work/sphere".into(), subdir: String::new(), exit: 2, duration: Duration::from_secs(1), output_tail: "boom".into(), cut: false }
    }

    #[test]
    fn a_failed_command_in_the_repo_is_kept_for_fix_it() {
        let (mut t, tx) = Terminal::scripted(true);
        tx.send(session("cargo test", "/work/sphere", "running 1 test\r\ntest a ... FAILED\r\n", 101)).unwrap();
        pump_until(&mut t, "the failure", |t| t.fix.is_some());
        let f = t.fix.as_ref().unwrap();
        assert_eq!((f.command.as_str(), f.exit, f.repo.as_path(), f.subdir.as_str()), ("cargo test", 101, Path::new("/work/sphere"), ""));
        assert_eq!(f.output_tail, "running 1 test\ntest a ... FAILED");
    }

    #[test]
    fn the_next_command_takes_the_failure_away() {
        let (mut t, tx) = Terminal::scripted(true);
        tx.send(session("make", "/work/sphere", "boom\r\n", 2)).unwrap();
        pump_until(&mut t, "the failure", |t| t.fix.is_some());
        // an empty Enter at the prompt starts nothing: the strip stays
        tx.send(osc("133;A").into_bytes()).unwrap();
        pump_one(&mut t);
        assert!(t.fix.is_some());
        tx.send(start("ls").into_bytes()).unwrap();
        pump_one(&mut t);
        assert!(t.fix.is_none());
    }

    #[test]
    fn stopping_a_command_or_succeeding_is_not_a_failure() {
        let (mut t, tx) = Terminal::scripted(true);
        for exit in [0, 130, 131, 146, 148] {
            tx.send(session("sleep 100", "/work/sphere", "", exit)).unwrap();
            pump_and_wait(&mut t);
            assert!(t.fix.is_none(), "exit {exit}");
        }
        // and a real failure after them still shows
        tx.send(session("make", "/work/sphere", "boom\r\n", 2)).unwrap();
        pump_until(&mut t, "the failure", |t| t.fix.is_some());
    }

    #[test]
    fn only_the_newest_failure_shows() {
        let (mut t, tx) = Terminal::scripted(true);
        let mut both = session("make", "/work/sphere", "first\r\n", 2);
        both.extend(session("cargo build", "/work/sphere", "second\r\n", 101));
        tx.send(both).unwrap();
        pump_until(&mut t, "the failure", |t| t.fix.is_some());
        pump_and_wait(&mut t);
        let f = t.fix.as_ref().unwrap();
        assert_eq!((f.command.as_str(), f.exit, f.output_tail.as_str()), ("cargo build", 101, "second"));
    }

    #[test]
    fn a_failure_still_being_looked_up_when_the_next_command_starts_never_shows() {
        let (mut t, tx) = Terminal::scripted(true);
        // the lookup of the first failure is answered after the second command has started
        let mut both = session("make", "/work/sphere", "boom\r\n", 2);
        both.extend(start("ls").into_bytes());
        tx.send(both).unwrap();
        pump_and_wait(&mut t);
        assert!(t.fix.is_none());
    }

    #[test]
    fn a_failure_in_a_git_work_tree_knows_its_repo_and_where_in_it() {
        let root = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("sphere-term-fix-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("crates/x")).unwrap();
        assert!(std::process::Command::new("git").arg("-C").arg(&root).args(["init", "-q"]).status().is_ok_and(|s| s.success()));
        let (mut t, tx) = Terminal::scripted(false);
        tx.send(session("cargo test", &root.join("crates/x").to_string_lossy(), "boom\r\n", 101)).unwrap();
        pump_until(&mut t, "the failure", |t| t.fix.is_some());
        let f = t.fix.as_ref().unwrap();
        assert_eq!((f.repo.as_path(), f.subdir.as_str()), (root.as_path(), "crates/x"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_failure_that_ran_nowhere_is_not_offered() {
        // no directory was reported, or it is gone: there is no repo to start an agent in
        let (mut t, tx) = Terminal::scripted(false);
        tx.send(format!("{}boom\r\n{}{}", start("make"), osc("133;D;2"), osc("133;A")).into_bytes()).unwrap();
        pump_and_wait(&mut t);
        assert!(t.fix.is_none());
        tx.send(session("make", "/nowhere/at/all", "boom\r\n", 2)).unwrap();
        pump_and_wait(&mut t);
        assert!(t.fix.is_none());
    }

    #[test]
    fn old_output_replayed_into_a_reattached_tab_gets_no_fix_it() {
        let (mut t, tx) = Terminal::scripted(true);
        let old = session("make", "/work/sphere", "old failure\r\n", 2);
        t.inner.lock().unwrap().replay = old.len() as u64;
        tx.send(old).unwrap();
        pump_and_wait(&mut t);
        assert!(t.fix.is_none(), "an old failure is not offered");
        assert_eq!(t.inner.lock().unwrap().replay, 0);
        tx.send(session("make", "/work/sphere", "new failure\r\n", 2)).unwrap();
        pump_until(&mut t, "the new failure", |t| t.fix.is_some());
        assert_eq!(t.fix.as_ref().unwrap().output_tail, "new failure");
    }

    #[test]
    fn a_command_that_was_running_when_the_app_came_back_can_still_fail() {
        // its start is in the replay and its end is live, in the same chunk or the next
        let old = format!("{}half of the output\r\n", start("make")).into_bytes();
        let live = format!("the other half\r\n{}", end("/work/sphere", 1)).into_bytes();
        for in_one_chunk in [false, true] {
            let (mut t, tx) = Terminal::scripted(true);
            t.inner.lock().unwrap().replay = old.len() as u64;
            if in_one_chunk {
                tx.send([old.clone(), live.clone()].concat()).unwrap();
            } else {
                tx.send(old.clone()).unwrap();
                tx.send(live.clone()).unwrap();
            }
            pump_until(&mut t, "the failure", |t| t.fix.is_some());
            assert_eq!(t.fix.as_ref().unwrap().output_tail, "half of the output\nthe other half", "one chunk: {in_one_chunk}");
        }
    }

    #[test]
    fn the_emulator_still_gets_every_byte() {
        let (mut t, tx) = Terminal::scripted(true);
        tx.send(crate::fixit::demo_bytes()).unwrap();
        pump_until(&mut t, "the failure", |t| t.fix.is_some());
        let s = screen(&t);
        assert!(s.contains("error: test failed, to rerun pass `-p sphered --lib`"), "{s}");
        assert!(s.trim_end().ends_with("me sphere ›"), "{s}");
        assert!(!s.contains("7700") && !s.contains("133"), "{s}");
    }

    #[test]
    fn dismissing_takes_the_failure_and_makes_a_late_answer_stale() {
        let (mut t, _tx) = Terminal::scripted(true);
        t.fix = Some(failure());
        let before = t.fix_seq;
        assert_eq!(t.dismiss_fix(), Some(failure()));
        assert!(t.fix.is_none());
        assert_eq!(t.fix_seq, before + 1);
        assert_eq!(t.dismiss_fix(), None);
    }
}
