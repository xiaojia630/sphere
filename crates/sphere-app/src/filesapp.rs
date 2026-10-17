//! The Files app (design/apps/files): yazi-style columns (the parent, where you are, a preview), pinned places
//! along the top (your repos, Downloads, agents' worktrees), and a status line. Keys: H J K L (or arrows) move,
//! Enter opens with the default app, / filters, Space selects, Y yanks, P pastes here (never over anything), D
//! moves to the Trash (asks twice; only things inside your home), E opens in Zed (else the default editor),
//! 1-9 pinned places, . shows hidden files, Shift+J/K or PageUp/PageDown scroll the preview, G / Shift+G jump to
//! the top / end, Esc clears the filter, then closes.
//! Everything that touches the disk and may be slow (listing a folder, reading a preview, copying) runs on a
//! worker thread; the frame only ever applies finished results.
use crate::appframe::*;
use crate::cmdbar::{BarTarget, CmdBar};
use crate::core::CoreCtl;
use crate::fonts::UiFonts;
use crate::panel::*;
use crate::service::Service;
use crate::util::*;
use bevy::prelude::*;
use bevy::text::LineHeight;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Mutex;

pub const APP: u8 = 5;
const TOP: usize = 0;
const PINS: usize = 1;
const PARENT: usize = 2;
const CUR: usize = 3;
const PREVIEW: usize = 4;
const STATUS: usize = 5;
const STRIP: usize = 6;
const ACTIONS: usize = 7;
const PANES: [([f32; 5], bool); 8] = [
    ([22.0, 2.0, 1236.0, 32.0, 8.0], true),
    ([14.0, 42.0, 1252.0, 52.0, 30.0], false),
    ([14.0, 106.0, 300.0, 440.0, 14.0], false),
    ([326.0, 106.0, 380.0, 440.0, 34.0], false),
    ([718.0, 106.0, 548.0, 440.0, 24.0], false),
    ([14.0, 594.0, 1252.0, 34.0, 18.0], false),
    ([14.0, 638.0, 1252.0, 50.0, 22.0], false),
    ([14.0, 552.0, 1252.0, 36.0, 20.0], false),
];
/// Rows of a column: data rows, and a footer row when the folder is longer than that.
const ROW_H: f32 = 22.0;
const ROWS: usize = 17;
const PREVIEW_LINES: usize = 20;
/// Entries read from one folder; more are counted as "not shown".
const MAX_ENTRIES: usize = 20_000;
/// Bytes of a text file previewed, and the lines and width kept from them.
const PREVIEW_BYTES: usize = 256 * 1024;
const PREVIEW_MAX_LINES: usize = 4000;
const LINE_CELLS: usize = 64;

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub name: String,
    pub dir: bool,
    pub size: u64,
    /// a symbolic link, and one whose target is gone
    pub link: bool,
    pub broken: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Listing {
    pub entries: Vec<Entry>,
    /// why the folder can't be read, in words
    pub error: Option<String>,
    /// entries beyond MAX_ENTRIES that were left out
    pub more: usize,
}

fn io_words(e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::PermissionDenied => "Permission denied".into(),
        std::io::ErrorKind::NotFound => "Not found".into(),
        _ => match e.raw_os_error() {
            Some(20) => "Not a folder".into(),
            _ => e.to_string(),
        },
    }
}

/// A directory's entries: folders first, then files, by name (hidden ones only when asked). Links are marked,
/// and broken ones kept; a folder that can't be read says why.
pub fn list_dir(dir: &Path, hidden: bool) -> Listing {
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) => return Listing { error: Some(io_words(&e)), ..Default::default() },
    };
    let mut entries = Vec::new();
    let mut more = 0;
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if !hidden && name.starts_with('.') {
            continue;
        }
        if entries.len() >= MAX_ENTRIES {
            more += 1;
            continue;
        }
        let Ok(ft) = e.file_type() else { continue };
        let en = if ft.is_symlink() {
            match std::fs::metadata(e.path()) {
                Ok(m) => Entry { name, dir: m.is_dir(), size: m.len(), link: true, broken: false },
                Err(_) => Entry { name, dir: false, size: 0, link: true, broken: true },
            }
        } else {
            Entry { name, dir: ft.is_dir(), size: if ft.is_dir() { 0 } else { e.metadata().map(|m| m.len()).unwrap_or(0) }, link: false, broken: false }
        };
        entries.push(en);
    }
    entries.sort_by_cached_key(|e| (!e.dir, e.name.to_lowercase()));
    Listing { entries, error: None, more }
}

pub fn human(n: u64) -> String {
    match n {
        n if n >= 1 << 30 => format!("{:.1} GB", n as f64 / (1u64 << 30) as f64),
        n if n >= 1 << 20 => format!("{:.1} MB", n as f64 / (1u64 << 20) as f64),
        n if n >= 1 << 10 => format!("{:.1} KB", n as f64 / 1024.0),
        n => format!("{n} B"),
    }
}

fn ago(t: std::time::SystemTime) -> String {
    let a = std::time::SystemTime::now().duration_since(t).map(|d| d.as_secs()).unwrap_or(0);
    match a {
        a if a < 60 => "just now".into(),
        a if a < 3600 => format!("{} min ago", a / 60),
        a if a < 86400 => format!("{} h ago", a / 3600),
        a => format!("{} days ago", a / 86400),
    }
}

// ------------------------------------------------------------------ text that fits

/// How many monospace cells a character takes: 2 for wide (CJK, emoji), 0 for combining marks and controls.
fn cells(c: char) -> usize {
    let u = c as u32;
    match u {
        0..=0x1f | 0x7f..=0x9f | 0x200b..=0x200f | 0xfe00..=0xfe0f => 0,
        0x300..=0x36f => 0,
        0x1100..=0x115f | 0x2e80..=0xa4cf | 0xac00..=0xd7a3 | 0xf900..=0xfaff | 0xfe30..=0xfe6f | 0xff00..=0xff60 | 0xffe0..=0xffe6 | 0x1f300..=0x1faff | 0x20000..=0x3fffd => 2,
        _ => 1,
    }
}

fn width(s: &str) -> usize {
    s.chars().map(cells).sum()
}

/// `s` cut to `max` cells with a "…" at the end.
fn clip_cells(s: &str, max: usize) -> String {
    if width(s) <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = cells(c);
        if w + cw + 1 > max {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

/// A file name cut to `max` cells before its extension, so the extension survives: "very-long-na….rs".
fn clip_name(s: &str, max: usize) -> String {
    if width(s) <= max {
        return s.to_string();
    }
    let ext_at = s.rfind('.').filter(|&i| i > 0 && width(&s[i..]) <= 8).unwrap_or(s.len());
    let tail_w = width(&s[ext_at..]);
    if tail_w + 3 >= max {
        return clip_cells(s, max);
    }
    format!("{}{}", clip_cells(&s[..ext_at], max - tail_w), &s[ext_at..])
}

// ------------------------------------------------------------------ scrolling

/// The first visible row so the cursor stays on screen with a little context: `top` moves only when it must.
fn fix_top(top: usize, cursor: usize, n: usize, rows: usize) -> usize {
    if n <= rows {
        return 0;
    }
    let ctx = 2.min(rows / 2);
    let mut t = top.min(n - rows);
    if cursor < t + ctx {
        t = cursor.saturating_sub(ctx);
    } else if cursor + ctx + 1 > t + rows {
        t = (cursor + ctx + 1).saturating_sub(rows);
    }
    t.min(n - rows)
}

/// The first row so `pos` sits in the middle of a column it isn't moving (the parent folder).
fn centered_top(pos: usize, n: usize, rows: usize) -> usize {
    if n <= rows {
        0
    } else {
        pos.saturating_sub(rows / 2).min(n - rows)
    }
}

// ------------------------------------------------------------------ previews

enum Preview {
    Dir(Listing),
    /// the first lines, how many lines the file has (or is known to have), and whether there is more after those
    Text { lines: Vec<String>, total: usize, more: bool, size: u64, ext: String },
    /// a card of facts: a kind, and (label, value) rows
    Info { kind: String, rows: Vec<(String, String)> },
}

fn info(kind: &str, rows: Vec<(&str, String)>) -> Preview {
    Preview::Info { kind: kind.into(), rows: rows.into_iter().map(|(k, v)| (k.into(), v)).collect() }
}

/// (width, height) of a PNG, GIF, JPEG, BMP or WebP from its first bytes.
fn image_dims(b: &[u8]) -> Option<(u32, u32)> {
    let be32 = |i: usize| b.get(i..i + 4).map(|x| u32::from_be_bytes([x[0], x[1], x[2], x[3]]));
    let le16 = |i: usize| b.get(i..i + 2).map(|x| u16::from_le_bytes([x[0], x[1]]) as u32);
    let le24 = |i: usize| b.get(i..i + 3).map(|x| x[0] as u32 | (x[1] as u32) << 8 | (x[2] as u32) << 16);
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some((be32(16)?, be32(20)?));
    }
    if b.starts_with(b"GIF8") {
        return Some((le16(6)?, le16(8)?));
    }
    if b.starts_with(b"BM") {
        let h = b.get(22..26).map(|x| i32::from_le_bytes([x[0], x[1], x[2], x[3]]).unsigned_abs())?;
        let w = b.get(18..22).map(|x| i32::from_le_bytes([x[0], x[1], x[2], x[3]]).unsigned_abs())?;
        return Some((w, h));
    }
    if b.len() > 30 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        return match &b[12..16] {
            b"VP8X" => Some((le24(24)? + 1, le24(27)? + 1)),
            b"VP8 " => Some((le16(26)? & 0x3fff, le16(28)? & 0x3fff)),
            b"VP8L" => {
                let v = u32::from_le_bytes([b[21], b[22], b[23], b[24]]);
                Some(((v & 0x3fff) + 1, ((v >> 14) & 0x3fff) + 1))
            }
            _ => None,
        };
    }
    if b.starts_with(&[0xff, 0xd8]) {
        let mut i = 2;
        while i + 9 < b.len() {
            if b[i] != 0xff {
                i += 1;
                continue;
            }
            let m = b[i + 1];
            if m == 0xff {
                i += 1;
                continue;
            }
            if (0xc0..=0xcf).contains(&m) && !matches!(m, 0xc4 | 0xc8 | 0xcc) {
                let h = u16::from_be_bytes([b[i + 5], b[i + 6]]) as u32;
                let w = u16::from_be_bytes([b[i + 7], b[i + 8]]) as u32;
                return Some((w, h));
            }
            let len = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
            i += 2 + len.max(2);
        }
    }
    None
}

fn image_kind(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "png" => "PNG image",
        "jpg" | "jpeg" => "JPEG image",
        "gif" => "GIF image",
        "webp" => "WebP image",
        "bmp" => "BMP image",
        "heic" | "heif" => "HEIC image",
        "tif" | "tiff" => "TIFF image",
        "ico" | "icns" => "Icon",
        _ => return None,
    })
}

fn preview(p: &Path, hidden: bool) -> Preview {
    let lmd = match std::fs::symlink_metadata(p) {
        Ok(m) => m,
        Err(e) => return info("Can't read it", vec![("why", io_words(&e))]),
    };
    let md = match std::fs::metadata(p) {
        Ok(m) => m,
        Err(_) => {
            let target = std::fs::read_link(p).map(|t| t.to_string_lossy().to_string()).unwrap_or_default();
            return info("Broken link", vec![("points to", target), ("but", "nothing is there".into())]);
        }
    };
    let link_rows = |mut rows: Vec<(&'static str, String)>| {
        if lmd.file_type().is_symlink() {
            rows.insert(0, ("link to", std::fs::read_link(p).map(|t| t.to_string_lossy().to_string()).unwrap_or_default()));
        }
        rows
    };
    if md.is_dir() {
        return Preview::Dir(list_dir(p, hidden));
    }
    let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let size = md.len();
    let when = md.modified().map(ago).unwrap_or_else(|_| "-".into());
    let mut f = match std::fs::File::open(p) {
        Ok(f) => f,
        Err(e) => return info("Can't read it", link_rows(vec![("why", io_words(&e)), ("size", human(size))])),
    };
    use std::io::Read;
    let mut buf = Vec::new();
    // one byte past the limit tells whether there is more
    if let Err(e) = (&mut f).take(PREVIEW_BYTES as u64 + 1).read_to_end(&mut buf) {
        return info("Can't read it", link_rows(vec![("why", io_words(&e)), ("size", human(size))]));
    }
    if let Some(kind) = image_kind(&ext) {
        let mut rows = vec![("size", human(size))];
        if let Some((w, h)) = image_dims(&buf) {
            rows.insert(0, ("dimensions", format!("{w} × {h}")));
        }
        rows.push(("modified", when));
        return info(kind, link_rows(rows));
    }
    if buf.is_empty() {
        return info("Empty file", link_rows(vec![("size", "0 B".into()), ("modified", when)]));
    }
    if buf[..buf.len().min(8192)].contains(&0) {
        let kind = match ext.as_str() {
            "" => "Binary file".to_string(),
            e => format!("{} file", e.to_uppercase()),
        };
        return info(&kind, link_rows(vec![("size", human(size)), ("modified", when)]));
    }
    let more = buf.len() > PREVIEW_BYTES;
    buf.truncate(PREVIEW_BYTES);
    let text = String::from_utf8_lossy(&buf);
    let all: Vec<&str> = text.lines().collect();
    let total = all.len();
    // a cut-off last line isn't a line of the file
    let keep = if more && total > 1 { total - 1 } else { total };
    let lines = all.iter().take(keep.min(PREVIEW_MAX_LINES)).map(|l| clip_cells(&l.replace('\t', "    ").replace('\r', ""), 400)).collect();
    Preview::Text { lines, total: keep, more: more || keep > PREVIEW_MAX_LINES, size, ext: p.extension().map(|e| e.to_string_lossy().to_uppercase()).unwrap_or_else(|| "TEXT".into()) }
}

// ------------------------------------------------------------------ copy and trash

/// A free name in `dir` for `name`: itself, else "stem{tag}", "stem{tag} 2"... (the extension kept for files).
/// With no tag the numbering starts at 2, like the Trash.
fn unique_in(dir: &Path, name: &std::ffi::OsStr, is_dir: bool, tag: &str) -> PathBuf {
    let mut dst = dir.join(name);
    let mut k = if tag.is_empty() { 2 } else { 1 };
    let (stem, ext) = if is_dir {
        (name.to_string_lossy().to_string(), String::new())
    } else {
        (Path::new(name).file_stem().unwrap_or_default().to_string_lossy().to_string(), Path::new(name).extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default())
    };
    while dst.symlink_metadata().is_ok() {
        dst = dir.join(if k == 1 { format!("{stem}{tag}{ext}") } else { format!("{stem}{tag} {k}{ext}") });
        k += 1;
    }
    dst
}

/// Copies `src` into `dir` and returns what it made. Never overwrites: "name copy", "name copy 2"... Links stay
/// links, and a folder can't be pasted into itself.
fn copy_into(src: &Path, dir: &Path) -> Result<PathBuf, String> {
    let lmd = std::fs::symlink_metadata(src).map_err(|e| format!("{} is gone ({})", src.file_name().unwrap_or_default().to_string_lossy(), io_words(&e)))?;
    let name = src.file_name().ok_or("no name")?;
    let is_dir = lmd.is_dir();
    if is_dir {
        if let (Ok(a), Ok(b)) = (src.canonicalize(), dir.canonicalize()) {
            if b.starts_with(&a) {
                return Err(format!("Can't paste {} into itself", name.to_string_lossy()));
            }
        }
    }
    let dst = unique_in(dir, name, is_dir, " copy");
    let fail = |e: std::io::Error| format!("{}: {}", name.to_string_lossy(), io_words(&e));
    if lmd.file_type().is_symlink() {
        let t = std::fs::read_link(src).map_err(fail)?;
        std::os::unix::fs::symlink(t, &dst).map_err(fail)?;
    } else if is_dir {
        std::fs::create_dir(&dst).map_err(fail)?;
        for e in std::fs::read_dir(src).map_err(fail)?.flatten() {
            copy_into(&e.path(), &dst)?;
        }
    } else {
        std::fs::copy(src, &dst).map_err(fail)?;
    }
    Ok(dst)
}

/// Moves `p` to the Trash (`home`/.Trash), keeping the name unique. Refuses anything outside `home`, the home
/// folder itself, and what is already in the Trash. A link is trashed as the link, never its target.
fn trash_in(home: &Path, p: &Path) -> Result<String, String> {
    let name = p.file_name().ok_or("no name")?;
    let shown = name.to_string_lossy().to_string();
    let home = home.canonicalize().map_err(|_| "no home folder".to_string())?;
    let parent = p.parent().ok_or("no folder")?.canonicalize().map_err(|e| format!("{shown}: {}", io_words(&e)))?;
    let real = parent.join(name);
    let md = real.symlink_metadata().map_err(|_| format!("{shown} is gone"))?;
    let bin = home.join(".Trash");
    if real == home || !real.starts_with(&home) {
        return Err(format!("Won't trash {shown}: it is outside your home folder"));
    }
    if real.starts_with(&bin) {
        return Err(format!("{shown} is already in the Trash"));
    }
    std::fs::create_dir_all(&bin).map_err(|e| format!("No Trash folder: {}", io_words(&e)))?;
    let dst = unique_in(&bin, name, md.is_dir(), "");
    std::fs::rename(&real, &dst).map_err(|e| if e.raw_os_error() == Some(18) { format!("Could not move {shown} to the Trash: it is on another disk") } else { format!("Could not move {shown} to the Trash: {}", io_words(&e)) })?;
    Ok(shown)
}

fn trash(p: &Path) -> Result<String, String> {
    let home = std::env::var("HOME").map_err(|_| "no HOME".to_string())?;
    trash_in(Path::new(&home), p)
}

// ------------------------------------------------------------------ the worker

enum Job {
    Dir { gen: u64, dir: PathBuf, hidden: bool },
    Prev { gen: u64, path: PathBuf, hidden: bool },
}

enum Msg {
    /// `dir` is where it really listed: the nearest folder that exists
    Dir { gen: u64, dir: PathBuf, moved: bool, listing: Listing, parent: Listing },
    Prev { gen: u64, path: PathBuf, preview: Preview },
    /// an operation (paste) finished: what to say, and whether it all worked
    Done { text: String, ok: bool },
}

fn work(jobs: Receiver<Job>, out: Sender<Msg>) {
    while let Ok(first) = jobs.recv() {
        let mut batch = vec![first];
        batch.extend(jobs.try_iter());
        // only the newest of each kind matters
        let last_dir = batch.iter().rposition(|j| matches!(j, Job::Dir { .. }));
        let last_prev = batch.iter().rposition(|j| matches!(j, Job::Prev { .. }));
        for (i, j) in batch.into_iter().enumerate() {
            let sent = match j {
                Job::Dir { gen, dir, hidden } if Some(i) == last_dir => {
                    let mut d = dir.clone();
                    while !d.is_dir() && d.pop() {}
                    if d.as_os_str().is_empty() {
                        d = PathBuf::from("/");
                    }
                    let listing = list_dir(&d, hidden);
                    let parent = d.parent().map(|p| list_dir(p, hidden)).unwrap_or_default();
                    out.send(Msg::Dir { gen, moved: d != dir, dir: d, listing, parent })
                }
                Job::Prev { gen, path, hidden } if Some(i) == last_prev => {
                    let preview = preview(&path, hidden);
                    out.send(Msg::Prev { gen, path, preview })
                }
                _ => Ok(()),
            };
            if sent.is_err() {
                return;
            }
        }
    }
}

// ------------------------------------------------------------------ the app

#[derive(Resource)]
pub struct FilesApp {
    pub open: bool,
    pub amt: Spring,
    cwd: PathBuf,
    entries: Vec<Entry>,
    /// the indexes of `entries` that pass the filter
    view: Vec<usize>,
    parent: Vec<Entry>,
    /// why the current folder can't be shown, and entries left out
    error: Option<String>,
    more: usize,
    loading: bool,
    cursor: usize,
    top: usize,
    pending_on: Option<String>,
    filter: String,
    hidden: bool,
    selected: Vec<PathBuf>,
    yanked: Vec<PathBuf>,
    pins: Vec<(String, PathBuf)>,
    /// the Trash is armed for these until the time
    armed: Option<(f32, Vec<PathBuf>)>,
    version: u64,
    prev: Option<(PathBuf, Preview)>,
    prev_req: Option<PathBuf>,
    prev_scroll: usize,
    dir_gen: u64,
    prev_gen: u64,
    busy: u32,
    jobs: Sender<Job>,
    msgs: Mutex<Receiver<Msg>>,
    done_tx: Sender<Msg>,
}

impl Default for FilesApp {
    fn default() -> Self {
        let (jobs, jrx) = channel();
        let (otx, orx) = channel();
        let done_tx = otx.clone();
        std::thread::spawn(move || work(jrx, otx));
        FilesApp {
            open: false,
            amt: Spring::new(0.0, 26.0, 9.0),
            cwd: PathBuf::new(),
            entries: Vec::new(),
            view: Vec::new(),
            parent: Vec::new(),
            error: None,
            more: 0,
            loading: false,
            cursor: 0,
            top: 0,
            pending_on: None,
            filter: String::new(),
            hidden: false,
            selected: Vec::new(),
            yanked: Vec::new(),
            pins: Vec::new(),
            armed: None,
            version: 0,
            prev: None,
            prev_req: None,
            prev_scroll: 0,
            dir_gen: 0,
            prev_gen: 0,
            busy: 0,
            jobs,
            msgs: Mutex::new(orx),
            done_tx,
        }
    }
}

impl FilesApp {
    pub fn open(&mut self) {
        self.open = true;
        if self.cwd.as_os_str().is_empty() {
            let home = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into()));
            let projects = home.join("Projects");
            // look tests: SPHERE_FILES_DIR starts there
            let start = std::env::var("SPHERE_FILES_DIR").map(PathBuf::from).ok().filter(|p| p.is_dir());
            self.go(start.unwrap_or(if projects.is_dir() { projects } else { home }), None);
        } else {
            self.reload();
        }
        self.version += 1;
    }
    pub fn close(&mut self) {
        self.open = false;
        self.armed = None;
        self.version += 1;
    }
    fn shown(&self) -> Vec<&Entry> {
        self.view.iter().filter_map(|&i| self.entries.get(i)).collect()
    }
    fn current_entry(&self) -> Option<&Entry> {
        self.view.get(self.cursor).and_then(|&i| self.entries.get(i))
    }
    fn current(&self) -> Option<PathBuf> {
        self.current_entry().map(|e| self.cwd.join(&e.name))
    }
    /// The path actions work on: nothing while a new folder is still loading (the list is the old one's).
    fn target(&self) -> Option<PathBuf> {
        if self.loading {
            None
        } else {
            self.current()
        }
    }
    fn refilter(&mut self) {
        let f = self.filter.to_lowercase();
        self.view = self.entries.iter().enumerate().filter(|(_, e)| f.is_empty() || e.name.to_lowercase().contains(&f)).map(|(i, _)| i).collect();
        self.cursor = self.cursor.min(self.view.len().saturating_sub(1));
        self.top = fix_top(self.top, self.cursor, self.view.len(), ROWS);
        self.version += 1;
    }
    /// Reads the folder again (on the worker); the cursor stays where it is.
    fn reload(&mut self) {
        self.dir_gen += 1;
        self.loading = true;
        let _ = self.jobs.send(Job::Dir { gen: self.dir_gen, dir: self.cwd.clone(), hidden: self.hidden });
        self.prev_req = None;
        self.version += 1;
    }
    /// Goes to `dir`, with the cursor on `on` when given.
    fn go(&mut self, dir: PathBuf, on: Option<String>) {
        self.cwd = dir;
        self.filter.clear();
        self.cursor = 0;
        self.top = 0;
        self.pending_on = on;
        self.reload();
    }
    fn move_to(&mut self, i: usize) {
        self.cursor = i.min(self.view.len().saturating_sub(1));
        self.top = fix_top(self.top, self.cursor, self.view.len(), ROWS);
        self.armed = None;
        self.version += 1;
    }
    /// How many lines the preview can scroll, when it is text.
    fn preview_span(&self) -> Option<usize> {
        match &self.prev {
            Some((p, Preview::Text { lines, .. })) if Some(p) == self.current().as_ref() => Some(lines.len().saturating_sub(PREVIEW_LINES)),
            _ => None,
        }
    }
    fn scroll_preview(&mut self, by: isize) {
        if let Some(span) = self.preview_span() {
            let to = (self.prev_scroll as isize + by).clamp(0, span as isize) as usize;
            if to != self.prev_scroll {
                self.prev_scroll = to;
                self.version += 1;
            }
        }
    }
    /// Takes what the worker finished, and asks for the preview of what the cursor is on.
    fn apply(&mut self, toasts: &mut crate::live::Toasts) {
        let msgs: Vec<Msg> = self.msgs.lock().map(|rx| rx.try_iter().collect()).unwrap_or_default();
        for m in msgs {
            match m {
                Msg::Dir { gen, dir, moved, listing, parent } => {
                    if gen != self.dir_gen {
                        continue;
                    }
                    if moved {
                        toasts.push_quiet(format!("{} is gone; showing {}", self.cwd.file_name().unwrap_or_default().to_string_lossy(), tilde(&dir)));
                        self.cwd = dir;
                    }
                    self.loading = false;
                    self.entries = listing.entries;
                    self.error = listing.error;
                    self.more = listing.more;
                    self.parent = parent.entries;
                    let on = self.pending_on.take();
                    let f = self.filter.to_lowercase();
                    self.view = self.entries.iter().enumerate().filter(|(_, e)| f.is_empty() || e.name.to_lowercase().contains(&f)).map(|(i, _)| i).collect();
                    if let Some(name) = on {
                        if let Some(i) = self.shown().iter().position(|e| e.name == name) {
                            self.cursor = i;
                        }
                    }
                    self.cursor = self.cursor.min(self.view.len().saturating_sub(1));
                    self.top = fix_top(self.top, self.cursor, self.view.len(), ROWS);
                    self.selected.retain(|p| p.symlink_metadata().is_ok());
                    self.prev_req = None;
                    self.version += 1;
                }
                Msg::Prev { gen, path, preview } => {
                    if gen == self.prev_gen && self.prev_req.as_ref() == Some(&path) {
                        self.prev = Some((path, preview));
                        self.version += 1;
                    }
                }
                Msg::Done { text, ok } => {
                    self.busy = self.busy.saturating_sub(1);
                    if ok {
                        toasts.push_quiet(text);
                    } else {
                        toasts.push(text);
                    }
                    self.reload();
                }
            }
        }
        if self.open && !self.loading {
            let want = self.current();
            if want != self.prev_req {
                self.prev_req = want.clone();
                self.prev_scroll = 0;
                self.prev_gen += 1;
                match want {
                    Some(p) => {
                        let _ = self.jobs.send(Job::Prev { gen: self.prev_gen, path: p, hidden: self.hidden });
                    }
                    None => self.prev = None,
                }
                self.version += 1;
            }
        }
    }
}

/// Folders worth a pin: your Projects folder and the newest repos in it, Home when there are no repos, then
/// Downloads (Desktop and Documents when there is little else): (label, path).
fn base_places(home: &Path, repos: &[PathBuf]) -> Vec<(String, PathBuf)> {
    let mut v: Vec<(String, PathBuf)> = Vec::new();
    let mut add = |label: &str, p: PathBuf| {
        if p.is_dir() && !v.iter().any(|(_, q)| q == &p) {
            v.push((label.to_string(), p));
        }
    };
    add("Projects", home.join("Projects"));
    for p in repos.iter().take(4) {
        add(&p.file_name().unwrap_or_default().to_string_lossy(), p.clone());
    }
    let few = repos.len() < 2;
    if few {
        add("Home", home.to_path_buf());
    }
    add("Downloads", home.join("Downloads"));
    if few {
        add("Desktop", home.join("Desktop"));
        add("Documents", home.join("Documents"));
    }
    v
}

/// The repos under ~/Projects, the newest first.
fn recent_repos(home: &Path) -> Vec<PathBuf> {
    let mut repos: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(home.join("Projects"))
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.join(".git").exists()).map(|p| (p.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH), p)).collect())
        .unwrap_or_default();
    repos.sort_by_key(|r| std::cmp::Reverse(r.0));
    repos.into_iter().map(|r| r.1).collect()
}

fn pins(repos: &[PathBuf], tasks: &[sphere_proto::TaskView]) -> Vec<(String, PathBuf)> {
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into()));
    let mut v = base_places(&home, repos);
    for t in tasks.iter().filter(|t| !t.in_place) {
        if v.len() >= 9 {
            break;
        }
        if let Some(wd) = &t.workdir {
            let p = PathBuf::from(wd);
            if p.is_dir() && !v.iter().any(|(_, q)| q == &p) {
                v.push((t.branch.clone().unwrap_or_else(|| t.title.clone()).trim_start_matches("sphere/").to_string(), p));
            }
        }
    }
    v.truncate(9);
    v
}

#[derive(Resource)]
pub struct FilesRoots(Workspace);

pub fn setup(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut images: ResMut<Assets<Image>>, (home, mut atlases): (Res<PanelAtlas>, ResMut<AppAtlases>), mut pmats: ResMut<Assets<PanelMaterial>>, args: Res<crate::Args>, mut app: ResMut<FilesApp>) {
    if args.open_app.as_deref() == Some("files") {
        app.open();
    }
    let ws = spawn_workspace(&mut commands, &mut meshes, &mut images, &home, &mut atlases, &mut pmats, APP, &PANES);
    commands.insert_resource(FilesRoots(ws));
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

fn name_of(p: &Path) -> String {
    p.file_name().unwrap_or_default().to_string_lossy().to_string()
}

#[allow(clippy::too_many_arguments)]
pub fn keys(
    mut ev: MessageReader<bevy::input::keyboard::KeyboardInput>,
    keys: Res<ButtonInput<KeyCode>>,
    (focus, moment, mut bar): (ResMut<Focus>, Res<crate::moment::Moment>, ResMut<CmdBar>),
    mut app: ResMut<FilesApp>,
    sim: Res<crate::core::Sim>,
    mut sound: ResMut<crate::sound::Sound>,
    mut toasts: ResMut<crate::live::Toasts>,
    mut clicks: ResMut<crate::appframe::AppClicks>,
    mut cmd_was: Local<bool>,
) {
    let mut focus = focus;
    let mods = keys.any_pressed([KeyCode::SuperLeft, KeyCode::SuperRight, KeyCode::ControlLeft, KeyCode::ControlRight, KeyCode::AltLeft, KeyCode::AltRight]);
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let bar_busy = focus.cmd || *cmd_was;
    *cmd_was = focus.cmd;
    if !app.open {
        ev.clear();
        return;
    }
    let mut codes: Vec<(KeyCode, bool)> = Vec::new();
    for e in ev.read() {
        if e.state != bevy::input::ButtonState::Pressed || mods || bar_busy || focus.terminal || moment.shown || (e.repeat && !matches!(e.key_code, KeyCode::KeyJ | KeyCode::KeyK | KeyCode::ArrowDown | KeyCode::ArrowUp | KeyCode::PageDown | KeyCode::PageUp)) {
            continue;
        }
        codes.push((e.key_code, shift));
    }
    // clicks on the panes: a button presses its key, a row picks its item
    for c in clicks.take(APP) {
        match c {
            Click::Key(k) => codes.push((k, false)),
            Click::Pick(i) => {
                // a second click on the same row opens it
                if i == app.cursor {
                    codes.push((KeyCode::Enter, false));
                } else {
                    app.move_to(i);
                }
            }
            Click::Alt(i) => {
                if let Some((_, p)) = app.pins.get(i).cloned() {
                    app.go(p, None);
                }
            }
        }
    }
    for (key, shift) in codes {
        let n = app.view.len();
        let cur = app.target();
        match key {
            KeyCode::Escape => {
                if app.armed.is_some() {
                    app.armed = None;
                    app.version += 1;
                } else if !app.filter.is_empty() {
                    app.filter.clear();
                    app.cursor = 0;
                    app.top = 0;
                    app.refilter();
                } else {
                    app.close();
                    sound.play_with("collapse", 0.7, 0.0);
                }
            }
            KeyCode::KeyJ | KeyCode::KeyK | KeyCode::ArrowDown | KeyCode::ArrowUp if shift => {
                app.scroll_preview(if matches!(key, KeyCode::KeyJ | KeyCode::ArrowDown) { 3 } else { -3 });
            }
            KeyCode::KeyJ | KeyCode::ArrowDown if n > 0 => {
                let c = app.cursor;
                app.move_to((c + 1).min(n - 1));
                sound.play_q("tick", 30);
            }
            KeyCode::KeyK | KeyCode::ArrowUp => {
                let c = app.cursor;
                app.move_to(c.saturating_sub(1));
                sound.play_q("tick", 30);
            }
            KeyCode::PageDown | KeyCode::PageUp => {
                let down = key == KeyCode::PageDown;
                if app.preview_span().is_some_and(|s| s > 0) {
                    app.scroll_preview(if down { PREVIEW_LINES as isize - 2 } else { -(PREVIEW_LINES as isize - 2) });
                } else if n > 0 {
                    let to = if down { (app.cursor + ROWS - 1).min(n - 1) } else { app.cursor.saturating_sub(ROWS - 1) };
                    app.move_to(to);
                    sound.play_q("tick", 30);
                }
            }
            KeyCode::KeyG | KeyCode::Home | KeyCode::End if n > 0 => {
                app.move_to(if shift || key == KeyCode::End { n - 1 } else { 0 });
                sound.play_q("tick", 30);
            }
            KeyCode::KeyH | KeyCode::ArrowLeft | KeyCode::Backspace => {
                if let Some(p) = app.cwd.parent().map(|p| p.to_path_buf()) {
                    let name = app.cwd.file_name().map(|n| n.to_string_lossy().to_string());
                    app.go(p, name);
                    sound.play_q("tick", 30);
                }
            }
            KeyCode::KeyL | KeyCode::ArrowRight | KeyCode::Enter => match cur {
                Some(p) if p.is_dir() => {
                    app.go(p, None);
                    sound.play_q("tick", 30);
                }
                Some(p) if key == KeyCode::Enter => {
                    if p.exists() {
                        let _ = std::process::Command::new("open").arg(&p).spawn();
                        toasts.push_quiet(format!("Opening {}", name_of(&p)));
                        sound.play("open");
                    } else {
                        toasts.push(format!("{} is a broken link; nothing to open", name_of(&p)));
                    }
                }
                _ => {}
            },
            KeyCode::Slash => {
                bar.prompt(BarTarget::Filter, &mut focus);
                sound.play("open");
            }
            KeyCode::Period => {
                app.hidden = !app.hidden;
                let on = app.hidden;
                // keep the cursor on the same name when it survives
                app.pending_on = app.current_entry().map(|e| e.name.clone());
                app.reload();
                toasts.push_quiet(if on { "Showing hidden files" } else { "Hiding hidden files" });
                sound.play("toggle");
            }
            KeyCode::Space => {
                if let Some(p) = cur {
                    if let Some(i) = app.selected.iter().position(|s| s == &p) {
                        app.selected.remove(i);
                    } else {
                        app.selected.push(p);
                    }
                    let c = app.cursor;
                    app.move_to((c + 1).min(n.saturating_sub(1)));
                    sound.play("toggle");
                }
            }
            KeyCode::KeyY => {
                let picked: Vec<PathBuf> = if app.selected.is_empty() { cur.into_iter().collect() } else { std::mem::take(&mut app.selected) };
                if !picked.is_empty() {
                    toasts.push_quiet(format!("Yanked {} item{}; P pastes it where you go", picked.len(), plural(picked.len())));
                    app.yanked = picked;
                    app.version += 1;
                    sound.play("confirm");
                }
            }
            KeyCode::KeyP => {
                if app.yanked.is_empty() {
                    toasts.push_quiet("Nothing yanked (Y yanks)");
                } else if app.loading {
                    toasts.push_quiet("Still reading this folder");
                } else {
                    let (dir, items, tx) = (app.cwd.clone(), app.yanked.clone(), app.done_tx.clone());
                    app.busy += 1;
                    app.version += 1;
                    toasts.push_quiet(format!("Pasting {} item{}...", items.len(), plural(items.len())));
                    std::thread::spawn(move || {
                        let mut made = 0;
                        let mut errs: Vec<String> = Vec::new();
                        for y in &items {
                            match copy_into(y, &dir) {
                                Ok(_) => made += 1,
                                Err(e) => errs.push(e),
                            }
                        }
                        let here = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "/".into());
                        let (text, ok) = match (errs.first(), made) {
                            (None, k) => (format!("Pasted {k} item{} into {here}", plural(k)), true),
                            (Some(e), 0) => (format!("Paste failed: {e}"), false),
                            (Some(e), k) => (format!("Pasted {k}, {} failed: {e}", errs.len()), false),
                        };
                        let _ = tx.send(Msg::Done { text, ok });
                    });
                    sound.play("drop");
                }
            }
            KeyCode::KeyD => {
                let targets: Vec<PathBuf> = if app.selected.is_empty() { cur.into_iter().collect() } else { app.selected.clone() };
                if targets.is_empty() {
                    continue;
                }
                if app.armed.as_ref().is_some_and(|(t, a)| sim.t < *t && *a == targets) {
                    app.armed = None;
                    let (mut done, mut errs) = (Vec::new(), Vec::new());
                    for t in &targets {
                        match trash(t) {
                            Ok(n) => done.push(n),
                            Err(e) => errs.push(e),
                        }
                    }
                    app.selected.clear();
                    app.reload();
                    match (errs.first(), done.len()) {
                        (None, 1) => toasts.push_quiet(format!("Moved {} to the Trash", done[0])),
                        (None, k) => toasts.push_quiet(format!("Moved {k} items to the Trash")),
                        (Some(e), 0) => toasts.push(e.clone()),
                        (Some(e), k) => toasts.push(format!("Moved {k} to the Trash; {} failed: {e}", errs.len())),
                    }
                    sound.play("discard");
                } else {
                    let what = if targets.len() == 1 { name_of(&targets[0]) } else { format!("{} items", targets.len()) };
                    app.armed = Some((sim.t + 3.0, targets));
                    app.version += 1;
                    toasts.push_quiet(format!("Press D again to move {what} to the Trash"));
                    sound.play("toggle");
                }
            }
            KeyCode::KeyE => {
                if let Some(p) = cur {
                    let zed = ["/Applications/Zed.app/Contents/MacOS/cli", "/usr/local/bin/zed", "/opt/homebrew/bin/zed"].into_iter().find(|z| Path::new(z).exists());
                    let r = match zed {
                        Some(z) => std::process::Command::new(z).arg(&p).spawn(),
                        None => std::process::Command::new("open").arg("-t").arg(&p).spawn(),
                    };
                    match r {
                        Ok(_) => toasts.push_quiet(format!("Editing {}", name_of(&p))),
                        Err(e) => toasts.push(format!("Could not open an editor: {e}")),
                    }
                    sound.play("open");
                }
            }
            k @ (KeyCode::Digit1 | KeyCode::Digit2 | KeyCode::Digit3 | KeyCode::Digit4 | KeyCode::Digit5 | KeyCode::Digit6 | KeyCode::Digit7 | KeyCode::Digit8 | KeyCode::Digit9) => {
                const DIGITS: [KeyCode; 9] = [KeyCode::Digit1, KeyCode::Digit2, KeyCode::Digit3, KeyCode::Digit4, KeyCode::Digit5, KeyCode::Digit6, KeyCode::Digit7, KeyCode::Digit8, KeyCode::Digit9];
                let i = DIGITS.iter().position(|d| *d == k).unwrap_or(0);
                if let Some((_, p)) = app.pins.get(i).cloned() {
                    app.go(p, None);
                    sound.play("whoosh");
                }
            }
            _ => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn update(sim: Res<crate::core::Sim>, mut atlases: ResMut<AppAtlases>, mut app: ResMut<FilesApp>, mut ctl: ResMut<CoreCtl>, svc: Res<Service>, mut bar: ResMut<CmdBar>, mut toasts: ResMut<crate::live::Toasts>, mut state: Local<(u64, f32, Vec<PathBuf>)>) {
    let open = app.open;
    step_open(open, &mut app.amt, sim.dt, &mut ctl);
    atlases.want(APP, open || app.amt.x > 0.001);
    if app.armed.as_ref().is_some_and(|(t, _)| sim.t > *t) {
        app.armed = None;
        app.version += 1;
    }
    if let Some((BarTarget::Filter, text)) = bar.out.clone() {
        bar.out = None;
        app.filter = text;
        app.cursor = 0;
        app.top = 0;
        app.refilter();
    }
    if !app.open {
        return;
    }
    app.apply(&mut toasts);
    // pinned places: the repos list is re-read at most every few seconds
    let s = svc.shared.lock().unwrap();
    let stale = sim.t - state.1 > 5.0;
    if state.0 != s.version + 1 || app.pins.is_empty() || stale {
        if stale {
            let home = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into()));
            state.2 = recent_repos(&home);
            state.1 = sim.t;
        }
        state.0 = s.version + 1;
        let p = pins(&state.2, &s.tasks);
        if p != app.pins {
            app.pins = p;
            app.version += 1;
        }
    }
}

pub fn place(app: Res<FilesApp>, mut q: Query<(&mut Panel, &AppPart)>) {
    place_app(APP, app.amt.x, |_| false, &mut q);
}

fn tilde(p: &Path) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let s = p.to_string_lossy().to_string();
    if !home.is_empty() && p.starts_with(&home) {
        format!("~{}", &s[home.len()..])
    } else {
        s
    }
}

fn icon(e: &Entry) -> &'static str {
    if e.broken {
        "×"
    } else if e.link {
        "@"
    } else if e.dir {
        "▸"
    } else {
        match Path::new(&e.name).extension().and_then(|x| x.to_str()).unwrap_or("") {
            "rs" | "ts" | "tsx" | "js" | "py" | "go" | "toml" | "json" | "yaml" | "yml" | "sh" | "wesl" | "wgsl" | "html" | "css" => "<>",
            "md" | "txt" => "≡",
            _ => "·",
        }
    }
}

pub fn draw(mut commands: Commands, app: Res<FilesApp>, svc: Res<Service>, roots: Option<Res<FilesRoots>>, f: Res<UiFonts>, theme: Res<crate::theme::Theme>, mut seen: Local<(u64, u64, u64)>) {
    let Some(roots) = roots else { return };
    if app.amt.x < 0.001 && !app.open {
        return;
    }
    let (agents, sver) = {
        let s = svc.shared.lock().unwrap();
        let mut v = s.tasks.clone();
        v.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        (v, s.version)
    };
    let key = (app.version, sver, theme.version);
    if *seen == key {
        return;
    }
    *seen = key;
    let c = &mut commands;
    let ws = &roots.0;
    for r in &ws.panes {
        c.entity(*r).despawn_children();
    }
    c.entity(ws.corelab).despawn_children();
    let ui = Ui::new(&f);
    let here = app.cwd.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "/".into());
    ui.corelab(c, ws.corelab, "Files", clip_cells(&tilde(&app.cwd), 40), ui.line);
    ui.top(c, ws.panes[TOP], "Files", clip_cells(&tilde(&app.cwd).to_uppercase(), 110), "files after yazi (MIT) · quick edit in Zed");
    ui.strip(c, ws.panes[STRIP], &agents, ["H J K L MOVE   ENTER OPEN   / FILTER   . HIDDEN   SPACE SELECT   SHIFT J K SCROLL PREVIEW", "Y YANK   P PASTE   D TRASH   E EDIT   1-9 PINNED   G / SHIFT G TOP / END   ESC CLEAR, CLOSE"]);
    // pinned places
    let r = c.spawn(Node { width: percent(100), height: percent(100), flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(8), padding: UiRect::horizontal(px(14)), ..default() }).id();
    c.entity(ws.panes[PINS]).add_child(r);
    let lab = c.spawn(Node { flex_direction: FlexDirection::Column, margin: UiRect::right(px(6)), ..default() }).id();
    let l1 = ui.text(c, "PINNED", &f.ui600, 9.5, 2.4, ui.dim);
    let l2 = ui.text(c, "1-9 to jump", &f.ui500, 10.0, 0.4, ui.dim);
    c.entity(lab).add_children(&[l1, l2]);
    c.entity(r).add_child(lab);
    for (i, (name, p)) in app.pins.iter().enumerate() {
        let on = app.cwd.starts_with(p);
        let agent = p.to_string_lossy().contains("/.sphere/");
        let chip = c.spawn((Node { height: px(36), flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(8), padding: UiRect::horizontal(px(9)), border: UiRect::all(px(1)), ..default() }, BorderColor::all(with_alpha(if on { ui.hot } else { ui.line }, if on { 0.9 } else { 0.35 })), BackgroundColor(if on { with_alpha(ui.line, 0.08) } else { Color::NONE }), Click::Alt(i))).id();
        let k = ui.text(c, format!("{}", i + 1), &f.mono, 10.5, 0.0, ui.dim);
        let col = c.spawn(Node { flex_direction: FlexDirection::Column, ..default() }).id();
        let n = ui.text(c, clip_name(name, 14), &f.ui600, 12.0, 0.4, if agent { role_color(Role::Fill) } else { ui.hot });
        let sub = ui.text(c, clip_cells(&tilde(p.parent().unwrap_or(p)), 16), &f.mono, 9.5, 0.0, ui.dim);
        c.entity(col).add_children(&[n, sub]);
        c.entity(chip).add_children(&[k, col]);
        c.entity(r).add_child(chip);
    }
    // parent
    let shown = app.shown();
    let parent_name = app.cwd.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "/".into());
    let ppos = app.parent.iter().position(|e| e.name == here);
    let pb = ui.body(c, ws.panes[PARENT]);
    let parent_refs: Vec<&Entry> = app.parent.iter().collect();
    let parent_dir = app.cwd.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let parent_title = clip_cells(&parent_name, 16);
    column_into(
        c,
        &ui,
        pb,
        Col { title: &parent_title, count: app.parent.len().to_string(), rows: &parent_refs, cursor: ppos, top: centered_top(ppos.unwrap_or(0), app.parent.len(), ROWS), selected: &[], dir: &parent_dir, active: false, other: KeyCode::KeyH, cells: 26, size: false, empty: if app.cwd.parent().is_none() { "top of the disk".into() } else { "empty".into() }, more: 0, footer: false },
    );
    // here
    let title = if app.filter.is_empty() { clip_name(&here, 22) } else { format!("{} · /{}", clip_name(&here, 12), clip_cells(&app.filter, 14)) };
    let empty = match (&app.error, app.loading, app.entries.is_empty()) {
        (Some(e), _, _) => format!("{e} (H goes back)"),
        (None, true, _) => "reading...".into(),
        (None, false, true) if !app.hidden => "empty folder (. shows hidden files)".into(),
        (None, false, true) => "empty folder".into(),
        _ => format!("nothing matches /{} (Esc clears)", clip_cells(&app.filter, 20)),
    };
    let cb = ui.body(c, ws.panes[CUR]);
    column_into(c, &ui, cb, Col { title: &title, count: shown.len().to_string(), rows: &shown, cursor: Some(app.cursor), top: app.top, selected: &app.selected, dir: &app.cwd, active: true, other: KeyCode::KeyH, cells: 33, size: true, empty, more: app.more, footer: true });
    // preview
    let b = ui.body(c, ws.panes[PREVIEW]);
    let cur_path = app.current();
    let ready = match (&app.prev, &cur_path) {
        (Some((pp, pv)), Some(cp)) if pp == cp => Some(pv),
        _ => None,
    };
    match (cur_path.as_ref(), ready) {
        (Some(p), Some(pv)) => {
            let name = clip_name(&name_of(p), 34);
            match pv {
                Preview::Dir(l) => {
                    let refs: Vec<&Entry> = l.entries.iter().collect();
                    let empty = l.error.clone().unwrap_or_else(|| "empty folder".into());
                    column_into(c, &ui, b, Col { title: &name, count: l.entries.len().to_string(), rows: &refs, cursor: None, top: 0, selected: &[], dir: p, active: false, other: KeyCode::KeyL, cells: 52, size: true, empty, more: l.more, footer: false });
                }
                Preview::Text { lines, total, more, size, ext } => {
                    let count = if *more { format!("{total}+ lines") } else if *total == 1 { "1 line".to_string() } else { format!("{total} lines") };
                    let h = ui.header(c, &name, &count, ui.dim);
                    let meta = ui.row(c, 10.0);
                    let tag = c.spawn((Node { padding: UiRect { left: px(5), right: px(5), top: px(2), bottom: px(1) }, border: UiRect::all(px(1)), margin: UiRect::left(px(18)), ..default() }, BorderColor::all(with_alpha(ui.line, 0.5)))).id();
                    let tt = ui.text(c, clip_cells(ext, 8), &f.ui700, 9.0, 1.4, ui.line);
                    c.entity(tag).add_child(tt);
                    let m = ui.text(c, format!("{count}   {}", human(*size)), &f.mono, 11.0, 0.0, ui.dim);
                    c.entity(meta).add_children(&[tag, m]);
                    let span = lines.len().saturating_sub(PREVIEW_LINES);
                    let at = app.prev_scroll.min(span);
                    if span > 0 {
                        let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
                        let pos = ui.text(c, format!("{}-{} of {}", at + 1, (at + PREVIEW_LINES).min(lines.len()), lines.len()), &f.mono, 10.5, 0.0, ui.dim);
                        let up = chip(c, &ui, "▲ PGUP", KeyCode::PageUp, at > 0);
                        let dn = chip(c, &ui, "▼ PGDN", KeyCode::PageDown, at < span);
                        c.entity(meta).add_children(&[sp, pos, up, dn]);
                        c.entity(meta).insert(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(10), width: percent(100), padding: UiRect::right(px(28)), ..default() });
                    }
                    let code = c.spawn(Node { flex_direction: FlexDirection::Column, padding: UiRect { left: px(14), right: px(14), top: px(8), ..default() }, overflow: Overflow::clip(), ..default() }).id();
                    for (i, l) in lines.iter().enumerate().skip(at).take(PREVIEW_LINES) {
                        let t = l.trim_start();
                        let comment = t.starts_with("//") || t.starts_with('#') || t.starts_with("--") || t.starts_with("/*") || t.starts_with('*');
                        let row = c.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(12), height: px(17), ..default() }).id();
                        let num = c.spawn((Text::new(format!("{:>4}", i + 1)), font(&f.mono, 11.0, 0.0), TextColor(ui.dim), Node { width: px(32), flex_shrink: 0.0, ..default() })).id();
                        let tx = c.spawn((Text::new(clip_cells(l, LINE_CELLS)), font(&f.mono, 12.0, 0.0), TextColor(if comment { with_alpha(ui.line, 0.6) } else { ui.hot }), LineHeight::Px(17.0), TextLayout { linebreak: LineBreak::NoWrap, ..default() })).id();
                        c.entity(row).add_children(&[num, tx]);
                        c.entity(code).add_child(row);
                    }
                    if *more && at == span {
                        let t = ui.text(c, "… the rest of the file isn't shown", &f.ui500, 11.0, 0.4, ui.dim);
                        c.entity(code).add_child(t);
                    }
                    c.entity(b).add_children(&[h, meta, code]);
                }
                Preview::Info { kind, rows } => {
                    let h = ui.header(c, &name, "", ui.dim);
                    let card = c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(6), margin: UiRect::all(px(18)), ..default() }).id();
                    let kt = ui.text(c, kind.to_uppercase(), &f.ui600, 13.0, 2.0, ui.hot);
                    c.entity(card).add_child(kt);
                    for (k, v) in rows {
                        let row = ui.row(c, 12.0);
                        let kk = c.spawn((Text::new(k.to_uppercase()), font(&f.ui600, 10.0, 1.8), TextColor(ui.dim), Node { width: px(96), flex_shrink: 0.0, ..default() })).id();
                        let vv = ui.text(c, clip_cells(v, 48), &f.mono, 12.0, 0.0, ui.line);
                        c.entity(row).add_children(&[kk, vv]);
                        c.entity(card).add_child(row);
                    }
                    c.entity(b).add_children(&[h, card]);
                }
            }
        }
        (Some(p), None) => {
            let h = ui.header(c, &clip_name(&name_of(p), 34), "", ui.dim);
            let t = c.spawn((Text::new("reading..."), font(&f.ui500, 12.0, 0.4), TextColor(ui.dim), Node { margin: UiRect::all(px(18)), ..default() })).id();
            c.entity(b).add_children(&[h, t]);
        }
        _ => {
            let h = ui.header(c, "Preview", "", ui.dim);
            c.entity(b).add_child(h);
        }
    }
    // the actions as buttons, so the mouse can do everything the keys can
    let ar = c.spawn(Node { width: percent(100), height: percent(100), flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(8), padding: UiRect::horizontal(px(14)), ..default() }).id();
    c.entity(ws.panes[ACTIONS]).add_child(ar);
    let armed = app.armed.is_some();
    for (label, k, key, col, on) in [
        ("Up", "H", KeyCode::KeyH, ui.hot, app.cwd.parent().is_some()),
        ("Open", "↵", KeyCode::Enter, ui.hot, cur_path.is_some()),
        ("Filter", "/", KeyCode::Slash, ui.hot, true),
        ("Hidden", ".", KeyCode::Period, if app.hidden { role_color(Role::Acc) } else { ui.hot }, true),
        ("Select", "SPC", KeyCode::Space, ui.hot, cur_path.is_some()),
        ("Yank", "Y", KeyCode::KeyY, ui.hot, cur_path.is_some()),
        ("Paste", "P", KeyCode::KeyP, ui.hot, !app.yanked.is_empty()),
        (if armed { "Trash again" } else { "Trash" }, "D", KeyCode::KeyD, role_color(Role::Bad), cur_path.is_some()),
        ("Edit", "E", KeyCode::KeyE, ui.hot, cur_path.is_some()),
    ] {
        let btn = ui.button(c, label, k, if on { col } else { with_alpha(ui.dim, 0.6) }, armed && key == KeyCode::KeyD);
        c.entity(btn).insert(Click::Key(key));
        c.entity(ar).add_child(btn);
    }
    // status line
    let r = c.spawn(Node { width: percent(100), height: percent(100), flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(18), padding: UiRect::horizontal(px(16)), ..default() }).id();
    c.entity(ws.panes[STATUS]).add_child(r);
    let crumbs = ui.row(c, 6.0);
    let path = tilde(&cur_path.clone().unwrap_or_else(|| app.cwd.clone()));
    let mut parts: Vec<String> = path.split('/').map(|s| clip_name(s, 24)).collect();
    if parts.len() > 5 {
        let tail = parts.split_off(parts.len() - 4);
        parts = std::iter::once("…".to_string()).chain(tail).collect();
    }
    let last = parts.len().saturating_sub(1);
    for (i, s) in parts.iter().enumerate() {
        let t = ui.text(c, s.clone(), &f.mono, 12.0, 0.0, if i == last { ui.line } else { ui.hot });
        c.entity(crumbs).add_child(t);
        if i < last {
            let sl = ui.text(c, "/", &f.mono, 12.0, 0.0, ui.dim);
            c.entity(crumbs).add_child(sl);
        }
    }
    let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
    c.entity(r).add_children(&[crumbs, sp]);
    let modified = cur_path.as_ref().and_then(|p| p.symlink_metadata().ok()).and_then(|m| m.modified().ok()).map(ago);
    let mut stats = vec![("SELECTED", app.selected.len().to_string()), ("YANKED", app.yanked.len().to_string()), ("ITEM", format!("{} / {}", if shown.is_empty() { 0 } else { app.cursor + 1 }, shown.len())), ("MODIFIED", modified.unwrap_or_else(|| "-".into()))];
    if app.busy > 0 {
        stats.insert(0, ("WORKING", "pasting".into()));
    }
    if app.hidden {
        stats.insert(0, ("HIDDEN", "shown".into()));
    }
    for (k, v) in stats {
        let row = ui.row(c, 6.0);
        let kt = ui.text(c, k, &f.ui600, 9.5, 1.8, ui.dim);
        let vt = ui.text(c, v, &f.mono, 11.5, 0.0, ui.hot);
        c.entity(row).add_children(&[kt, vt]);
        c.entity(r).add_child(row);
    }
}

/// A small button in a pane that presses `key` when clicked.
fn chip(c: &mut Commands, ui: &Ui, label: &str, key: KeyCode, on: bool) -> Entity {
    let col = if on { ui.hot } else { with_alpha(ui.dim, 0.5) };
    let e = c.spawn((Node { padding: UiRect { left: px(6), right: px(6), top: px(2), bottom: px(2) }, border: UiRect::all(px(1)), ..default() }, BorderColor::all(with_alpha(col, 0.6)), Click::Key(key))).id();
    let t = ui.text(c, label, &ui.f.ui600, 9.5, 1.2, col);
    c.entity(e).add_child(t);
    e
}

/// What a column shows: its rows, which one is the cursor, and where the window on them starts.
struct Col<'a> {
    title: &'a str,
    count: String,
    rows: &'a [&'a Entry],
    cursor: Option<usize>,
    top: usize,
    selected: &'a [PathBuf],
    dir: &'a Path,
    active: bool,
    /// the key a click on a row presses when the column isn't the one you are in
    other: KeyCode,
    /// cells a name may take
    cells: usize,
    size: bool,
    empty: String,
    /// entries left out of the listing
    more: usize,
    /// a TOP / END footer and a scroll bar when the rows don't fit
    footer: bool,
}

fn column_into(c: &mut Commands, ui: &Ui, b: Entity, col: Col) {
    let h = ui.header(c, col.title, &col.count, ui.dim);
    let list = c.spawn(Node { flex_direction: FlexDirection::Column, padding: UiRect { left: px(8), right: px(10), top: px(8), ..default() }, ..default() }).id();
    c.entity(b).add_children(&[h, list]);
    let n = col.rows.len();
    if n == 0 {
        let t = ui.text(c, col.empty.clone(), &ui.f.ui500, 12.0, 0.4, ui.dim);
        c.entity(list).add_child(t);
    }
    let first = col.top.min(n.saturating_sub(ROWS));
    for (i, e) in col.rows.iter().enumerate().skip(first).take(ROWS) {
        let on = col.cursor == Some(i);
        let picked = col.selected.iter().any(|p| p == &col.dir.join(&e.name));
        let row = c
            .spawn((
                Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, height: px(ROW_H), column_gap: px(10), padding: UiRect::horizontal(px(8)), border: UiRect::all(px(if on && col.active { 1.0 } else { 0.0 })), ..default() },
                BorderColor::all(ui.hot),
                BackgroundColor(if on { with_alpha(ui.line, if col.active { 0.12 } else { 0.07 }) } else if picked { with_alpha(role_color(Role::Acc), 0.12) } else { Color::NONE }),
            ))
            .id();
        // rows where you are pick (twice opens); the parent column goes up, the preview goes in
        c.entity(row).insert(if col.active { Click::Pick(i) } else { Click::Key(col.other) });
        let tint = if e.broken { role_color(Role::Bad) } else if e.dir { ui.hot } else { ui.dim };
        let ic = c.spawn((Text::new(icon(e)), font(&ui.f.mono, 11.0, 0.0), TextColor(tint), Node { width: px(16), flex_shrink: 0.0, ..default() })).id();
        let shown = if e.dir { format!("{}/", clip_name(&e.name, col.cells.saturating_sub(1))) } else { clip_name(&e.name, col.cells) };
        let nm = c.spawn((Text::new(shown), font(&ui.f.mono, 12.5, 0.0), TextColor(if e.broken { role_color(Role::Bad) } else if picked { role_color(Role::Acc) } else if on { ui.hot } else { ui.line }), TextLayout { linebreak: LineBreak::NoWrap, ..default() }, Node { flex_grow: 1.0, flex_shrink: 1.0, overflow: Overflow::clip(), ..default() })).id();
        c.entity(row).add_children(&[ic, nm]);
        if col.size {
            let sz = ui.text(c, if e.dir || e.broken { String::new() } else { human(e.size) }, &ui.f.mono, 10.5, 0.0, ui.dim);
            c.entity(row).add_child(sz);
        }
        c.entity(list).add_child(row);
    }
    if col.more > 0 {
        let t = ui.text(c, format!("… and {} more not shown", col.more), &ui.f.ui500, 11.0, 0.4, ui.dim);
        c.entity(list).add_child(t);
    }
    if col.footer && n > ROWS {
        let last = (first + ROWS).min(n);
        let foot = c.spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(8), height: px(ROW_H), padding: UiRect::horizontal(px(8)), ..default() }).id();
        let top = chip(c, ui, "⇱ TOP", KeyCode::Home, first > 0);
        let end = chip(c, ui, "⇲ END", KeyCode::End, last < n);
        let pos = ui.text(c, format!("{}-{} of {n}", first + 1, last), &ui.f.mono, 10.5, 0.0, ui.dim);
        c.entity(foot).add_children(&[top, end, pos]);
        c.entity(list).add_child(foot);
        // a thin bar showing where the window is
        let track_h = ROWS as f32 * ROW_H;
        let thumb_h = (track_h * ROWS as f32 / n as f32).max(14.0);
        let thumb_y = (track_h - thumb_h) * first as f32 / (n - ROWS).max(1) as f32;
        let track = c.spawn((Node { position_type: PositionType::Absolute, right: px(2), top: px(8), width: px(3), height: px(track_h), ..default() }, BackgroundColor(with_alpha(ui.line, 0.1)))).id();
        let thumb = c.spawn((Node { position_type: PositionType::Absolute, top: px(thumb_y), width: px(3), height: px(thumb_h), ..default() }, BackgroundColor(with_alpha(ui.hot, 0.7)))).id();
        c.entity(track).add_child(thumb);
        c.entity(list).add_child(track);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("sphere-files-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn lists_folders_first_and_copies_without_overwriting() {
        let d = tmp("list");
        std::fs::create_dir_all(d.join("b-dir")).unwrap();
        std::fs::write(d.join("a.txt"), "x").unwrap();
        std::fs::write(d.join(".hidden"), "x").unwrap();
        let names: Vec<String> = list_dir(&d, false).entries.into_iter().map(|e| e.name).collect();
        assert_eq!(names, vec!["b-dir", "a.txt"]);
        assert_eq!(list_dir(&d, true).entries.len(), 3);
        copy_into(&d.join("a.txt"), &d).unwrap();
        copy_into(&d.join("a.txt"), &d).unwrap();
        assert!(d.join("a copy.txt").exists() && d.join("a copy 2.txt").exists());
        assert_eq!(human(1536), "1.5 KB");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn unreadable_missing_and_empty_folders_say_so() {
        let d = tmp("errs");
        assert_eq!(list_dir(&d.join("nope"), false).error.as_deref(), Some("Not found"));
        std::fs::write(d.join("f"), "x").unwrap();
        assert!(list_dir(&d.join("f"), false).error.is_some());
        std::fs::create_dir(d.join("empty")).unwrap();
        let empty = list_dir(&d.join("empty"), false);
        assert!(empty.error.is_none() && empty.entries.is_empty());
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir(d.join("locked")).unwrap();
        std::fs::set_permissions(d.join("locked"), std::fs::Permissions::from_mode(0o000)).unwrap();
        let l = list_dir(&d.join("locked"), false);
        std::fs::set_permissions(d.join("locked"), std::fs::Permissions::from_mode(0o755)).unwrap();
        // root can read anything; everyone else is told why not
        if l.error.is_some() {
            assert_eq!(l.error.as_deref(), Some("Permission denied"));
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn links_are_marked_and_broken_ones_kept() {
        let d = tmp("links");
        std::fs::create_dir(d.join("real")).unwrap();
        std::fs::write(d.join("file"), "hello").unwrap();
        std::os::unix::fs::symlink(d.join("real"), d.join("to-dir")).unwrap();
        std::os::unix::fs::symlink(d.join("gone"), d.join("dangling")).unwrap();
        let l = list_dir(&d, false).entries;
        let get = |n: &str| l.iter().find(|e| e.name == n).unwrap().clone();
        assert!(get("to-dir").dir && get("to-dir").link && !get("to-dir").broken);
        assert!(get("dangling").broken && !get("dangling").dir);
        assert!(!get("file").link);
        match preview(&d.join("dangling"), false) {
            Preview::Info { kind, .. } => assert_eq!(kind, "Broken link"),
            _ => panic!("a broken link previews as a card"),
        }
        // copying keeps a link a link, even a broken one
        let made = copy_into(&d.join("dangling"), &d).unwrap();
        assert!(made.symlink_metadata().unwrap().file_type().is_symlink());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_huge_folder_lists_in_order() {
        let d = tmp("huge");
        for i in 0..3000 {
            std::fs::write(d.join(format!("f{i:05}")), "").unwrap();
        }
        let t = std::time::Instant::now();
        let l = list_dir(&d, false);
        assert_eq!(l.entries.len(), 3000);
        assert_eq!(l.entries[0].name, "f00000");
        assert_eq!(l.more, 0);
        assert!(t.elapsed().as_secs() < 5);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn names_are_clipped_by_width_and_keep_their_extension() {
        assert_eq!(clip_name("short.rs", 20), "short.rs");
        let c = clip_name("a-very-long-file-name-indeed-yes.rs", 16);
        assert!(width(&c) <= 16 && c.ends_with(".rs") && c.contains('…'), "{c}");
        // wide characters count twice
        let c = clip_name("日本語のとても長いファイル名.txt", 14);
        assert!(width(&c) <= 14 && c.ends_with(".txt"), "{c}");
        let c = clip_cells("ééééééééééééééééé", 6);
        assert_eq!(c.chars().count(), 6);
        assert_eq!(clip_cells("no ext at all and quite long", 10).chars().last(), Some('…'));
        // an absurd "extension" doesn't eat the name
        assert!(width(&clip_name("x.aaaaaaaaaaaaaaaaaaaaaaaaaaaa", 10)) <= 10);
    }

    #[test]
    fn the_window_follows_the_cursor() {
        assert_eq!(fix_top(0, 5, 10, 17), 0);
        let mut top = 0;
        for cur in 0..100 {
            top = fix_top(top, cur, 100, 17);
            assert!(cur >= top && cur < top + 17, "cursor {cur} top {top}");
        }
        assert_eq!(top, 100 - 17);
        for cur in (0..100).rev() {
            top = fix_top(top, cur, 100, 17);
            assert!(cur >= top && cur < top + 17);
        }
        assert_eq!(top, 0);
        assert_eq!(centered_top(50, 100, 17), 42);
        assert_eq!(centered_top(99, 100, 17), 83);
        assert_eq!(centered_top(3, 10, 17), 0);
    }

    #[test]
    fn previews_text_binary_empty_and_images() {
        let d = tmp("prev");
        let long: String = (0..60000).map(|i| format!("line {i}\n")).collect();
        std::fs::write(d.join("long.txt"), &long).unwrap();
        std::fs::write(d.join("empty"), "").unwrap();
        std::fs::write(d.join("bin.dat"), [1u8, 0, 2, 3]).unwrap();
        std::fs::write(d.join("tabs.txt"), "a\tb\r\nsecond").unwrap();
        // a PNG header: 640 x 480
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend_from_slice(&640u32.to_be_bytes());
        png.extend_from_slice(&480u32.to_be_bytes());
        png.extend_from_slice(&[8, 6, 0, 0, 0]);
        std::fs::write(d.join("pic.png"), &png).unwrap();
        match preview(&d.join("long.txt"), false) {
            Preview::Text { lines, more, .. } => {
                assert_eq!(lines.len(), PREVIEW_MAX_LINES);
                assert!(more);
                assert_eq!(lines[0], "line 0");
            }
            _ => panic!("text"),
        }
        match preview(&d.join("tabs.txt"), false) {
            Preview::Text { lines, total, more, .. } => assert_eq!((lines, total, more), (vec!["a    b".to_string(), "second".to_string()], 2, false)),
            _ => panic!("text"),
        }
        let kind = |p: &str| match preview(&d.join(p), false) {
            Preview::Info { kind, rows } => (kind, rows),
            _ => panic!("card"),
        };
        assert_eq!(kind("empty").0, "Empty file");
        assert_eq!(kind("bin.dat").0, "DAT file");
        let (k, rows) = kind("pic.png");
        assert_eq!(k, "PNG image");
        assert_eq!(rows[0], ("dimensions".to_string(), "640 × 480".to_string()));
        assert!(matches!(preview(&d, false), Preview::Dir(_)));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn image_sizes_come_from_headers() {
        let mut gif = b"GIF89a".to_vec();
        gif.extend_from_slice(&[10, 0, 20, 0]);
        assert_eq!(image_dims(&gif), Some((10, 20)));
        let jpeg = [0xff, 0xd8, 0xff, 0xe0, 0, 4, 0, 0, 0xff, 0xc0, 0, 11, 8, 0, 50, 0, 100, 3, 0, 0, 0];
        assert_eq!(image_dims(&jpeg), Some((100, 50)));
        assert_eq!(image_dims(b"not an image at all"), None);
    }

    #[test]
    fn paste_never_overwrites_and_never_recurses() {
        let d = tmp("paste");
        std::fs::create_dir_all(d.join("dir/inner")).unwrap();
        std::fs::write(d.join("dir/inner/f.txt"), "x").unwrap();
        std::fs::write(d.join("a.txt"), "1").unwrap();
        // a folder into its own subtree is refused
        assert!(copy_into(&d.join("dir"), &d.join("dir/inner")).unwrap_err().contains("into itself"));
        // a folder next to itself becomes "dir copy", with its contents
        let made = copy_into(&d.join("dir"), &d).unwrap();
        assert_eq!(made, d.join("dir copy"));
        assert!(d.join("dir copy/inner/f.txt").exists());
        // pasting over an existing name keeps the original
        let made = copy_into(&d.join("a.txt"), &d).unwrap();
        assert_eq!(std::fs::read_to_string(d.join("a.txt")).unwrap(), "1");
        assert_eq!(made, d.join("a copy.txt"));
        assert!(copy_into(&d.join("missing"), &d).unwrap_err().contains("gone"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn trash_stays_inside_home_and_keeps_names_unique() {
        let home = tmp("home");
        let outside = tmp("outside");
        std::fs::write(outside.join("precious"), "x").unwrap();
        assert!(trash_in(&home, &outside.join("precious")).unwrap_err().contains("outside your home"));
        assert!(outside.join("precious").exists());
        assert!(trash_in(&home, &home).is_err());
        assert!(trash_in(&home, Path::new("/")).is_err());
        std::fs::write(home.join("a.txt"), "1").unwrap();
        assert_eq!(trash_in(&home, &home.join("a.txt")).unwrap(), "a.txt");
        assert!(home.join(".Trash/a.txt").exists() && !home.join("a.txt").exists());
        std::fs::write(home.join("a.txt"), "2").unwrap();
        trash_in(&home, &home.join("a.txt")).unwrap();
        assert_eq!(std::fs::read_to_string(home.join(".Trash/a 2.txt")).unwrap(), "2");
        assert_eq!(std::fs::read_to_string(home.join(".Trash/a.txt")).unwrap(), "1");
        // what is in the Trash stays; what is gone says so
        assert!(trash_in(&home, &home.join(".Trash/a.txt")).unwrap_err().contains("already in the Trash"));
        assert!(trash_in(&home, &home.join("nope")).unwrap_err().contains("gone"));
        // a link is trashed as the link; its target stays
        std::fs::create_dir(home.join("d")).unwrap();
        std::os::unix::fs::symlink(outside.join("precious"), home.join("d/l")).unwrap();
        trash_in(&home, &home.join("d/l")).unwrap();
        assert!(outside.join("precious").exists());
        assert!(home.join(".Trash/l").symlink_metadata().is_ok());
        // ".." can't smuggle a path out
        assert!(trash_in(&home, &home.join("d/../../x")).is_err());
        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn places_without_a_projects_folder_are_still_useful() {
        let home = tmp("places");
        let v = base_places(&home, &[]);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].0, "Home");
        std::fs::create_dir(home.join("Downloads")).unwrap();
        std::fs::create_dir(home.join("Desktop")).unwrap();
        let names: Vec<String> = base_places(&home, &[]).into_iter().map(|p| p.0).collect();
        assert_eq!(names, vec!["Home", "Downloads", "Desktop"]);
        std::fs::create_dir_all(home.join("Projects/r1/.git")).unwrap();
        std::fs::create_dir_all(home.join("Projects/r2/.git")).unwrap();
        let repos = recent_repos(&home);
        assert_eq!(repos.len(), 2);
        let names: Vec<String> = base_places(&home, &repos).into_iter().map(|p| p.0).collect();
        assert_eq!(names[0], "Projects");
        assert!(names.contains(&"r1".to_string()) && names.contains(&"Downloads".to_string()) && !names.contains(&"Home".to_string()));
        let _ = std::fs::remove_dir_all(&home);
    }

    fn settle(app: &mut FilesApp, toasts: &mut crate::live::Toasts) {
        for _ in 0..500 {
            app.apply(toasts);
            if !app.loading && app.prev_req == app.current() && (app.current().is_none() || app.prev.as_ref().is_some_and(|(p, _)| Some(p) == app.current().as_ref())) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the worker never answered");
    }

    #[test]
    fn the_app_reads_in_the_background_filters_and_follows_a_vanished_folder() {
        let d = tmp("app");
        std::fs::create_dir_all(d.join("sub/deeper")).unwrap();
        for n in ["alpha.txt", "beta.rs", "gamma.txt"] {
            std::fs::write(d.join("sub").join(n), format!("{n}\n")).unwrap();
        }
        let mut toasts = crate::live::Toasts::default();
        let mut app = FilesApp { open: true, ..Default::default() };
        app.go(d.join("sub"), Some("beta.rs".into()));
        // the list is the old (empty) one until the worker answers, and actions have no target meanwhile
        assert!(app.loading && app.target().is_none());
        settle(&mut app, &mut toasts);
        assert_eq!(app.shown().iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["deeper", "alpha.txt", "beta.rs", "gamma.txt"]);
        assert_eq!(app.current(), Some(d.join("sub/beta.rs")));
        assert!(matches!(&app.prev, Some((p, Preview::Text { .. })) if p == &d.join("sub/beta.rs")));
        // the filter narrows the list and the cursor stays inside it
        app.filter = "TXT".into();
        app.refilter();
        assert_eq!(app.shown().len(), 2);
        assert!(app.cursor < 2);
        settle(&mut app, &mut toasts);
        // the cursor moves; the preview follows it and starts at the top again
        app.move_to(0);
        app.prev_scroll = 5;
        settle(&mut app, &mut toasts);
        assert_eq!(app.prev_scroll, 0);
        assert_eq!(app.current(), Some(d.join("sub/alpha.txt")));
        // the folder is deleted under it: the app moves up to what is left and says so
        app.filter.clear();
        std::fs::remove_dir_all(d.join("sub")).unwrap();
        app.reload();
        settle(&mut app, &mut toasts);
        assert_eq!(app.cwd, d);
        assert!(toasts.list.iter().any(|t| t.0.contains("is gone")));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_worker_finds_the_nearest_folder_that_still_exists() {
        let d = tmp("worker");
        std::fs::create_dir_all(d.join("a")).unwrap();
        let (jt, jr) = channel();
        let (ot, or) = channel();
        std::thread::spawn(move || work(jr, ot));
        jt.send(Job::Dir { gen: 1, dir: d.join("a/b/c"), hidden: false }).unwrap();
        match or.recv_timeout(std::time::Duration::from_secs(5)).unwrap() {
            Msg::Dir { gen, dir, moved, .. } => assert_eq!((gen, dir, moved), (1, d.join("a"), true)),
            _ => panic!("a listing"),
        }
        let _ = std::fs::remove_dir_all(&d);
    }
}
