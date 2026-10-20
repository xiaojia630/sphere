//! Reads the bytes a terminal tab prints, on their way to the VT emulator, and notices commands: where each one
//! starts and ends, what it was, the directory it ran in, its exit status and what it printed. The shell
//! integration (shellint.rs) makes the shell send these as escape sequences:
//!   OSC 7700 ; cmd ; <base64>    the command line about to run
//!   OSC 133 ; C                  its output starts
//!   OSC 7 ; file://host/path     the working directory
//!   OSC 133 ; D ; <status>       it finished
//!   OSC 133 ; A                  a prompt is drawn
//! The scanner never changes the bytes (the emulator still gets all of them). It turns a command's output into
//! plain text, with colours and cursor moves stripped and carriage returns and backspaces applied, and keeps
//! only the end of it. Sequences may be split across any two chunks.
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// A command's output keeps at most this many lines and bytes (the end of it).
pub const KEEP_LINES: usize = 400;
pub const KEEP_BYTES: usize = 64 * 1024;
/// A line keeps at most this many characters.
const LINE_MAX: usize = 2000;
/// A command line is cut to this many bytes.
const CMD_MAX: usize = 4096;
/// An OSC longer than this is not worth keeping (ours are a few KB at most).
const OSC_MAX: usize = 16 * 1024;
/// CSI parameters beyond this many bytes are dropped.
const CSI_MAX: usize = 64;

/// What the scanner noticed.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A command's output starts.
    Started,
    /// A command ended.
    Finished(Finished),
}

/// A command that ended: what ran, where, how it ended and what it printed.
#[derive(Debug, Clone, PartialEq)]
pub struct Finished {
    pub command: String,
    /// The working directory when it ended (`cd src && cargo test` reports `src`).
    pub cwd: String,
    pub exit: i32,
    pub duration: Duration,
    /// The end of its output as plain text.
    pub output_tail: String,
    /// The start of the output was dropped to fit the caps.
    pub cut: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Ground,
    Esc,
    /// after `ESC` and an intermediate byte (`ESC ( B`): one more byte ends it
    EscInter,
    Csi,
    Osc,
    /// DCS, SOS, PM or APC: skipped up to the string terminator
    Str,
}

pub struct Scanner {
    mode: Mode,
    csi: Vec<u8>,
    osc: Vec<u8>,
    osc_over: bool,
    /// a UTF-8 character being decoded: bytes still to come and the bits so far
    u_need: u8,
    u_acc: u32,
    cwd: String,
    /// a command line seen before its start mark
    pending: Option<String>,
    run: Option<Run>,
}

struct Run {
    command: String,
    started: Instant,
    text: Text,
    /// the alternate screen is up (a full-screen program): its drawing is not output
    alt: bool,
}

impl Default for Scanner {
    fn default() -> Self {
        Self::new()
    }
}

impl Scanner {
    pub fn new() -> Self {
        Self { mode: Mode::Ground, csi: Vec::new(), osc: Vec::new(), osc_over: false, u_need: 0, u_acc: 0, cwd: String::new(), pending: None, run: None }
    }

    /// A command is running (its start mark came, its end has not).
    #[cfg(test)]
    fn running(&self) -> bool {
        self.run.is_some()
    }

    /// The directory the shell last reported.
    #[cfg(test)]
    fn cwd(&self) -> &str {
        &self.cwd
    }

    /// Scans a chunk of output. `now` stamps the commands that start or end in it.
    pub fn feed(&mut self, bytes: &[u8], now: Instant) -> Vec<Event> {
        let mut ev = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            if self.mode == Mode::Ground {
                match self.run.as_mut() {
                    // outside a command only escape sequences matter: skip the text between them
                    None => match bytes[i..].iter().position(|&b| b == 0x1b) {
                        Some(p) => i += p,
                        None => break,
                    },
                    // in one, a stretch of plain printable text goes in at once
                    Some(run) if !run.alt && self.u_need == 0 => {
                        let n = bytes[i..].iter().take_while(|b| (0x20..0x7f).contains(*b)).count();
                        if n > 0 {
                            run.text.put_ascii(&bytes[i..i + n]);
                            i += n;
                            continue;
                        }
                    }
                    Some(_) => {}
                }
            }
            self.step(bytes[i], now, &mut ev);
            i += 1;
        }
        ev
    }

    fn step(&mut self, b: u8, now: Instant, ev: &mut Vec<Event>) {
        match self.mode {
            Mode::Ground => {
                if b == 0x1b {
                    self.u_need = 0;
                    self.mode = Mode::Esc;
                } else {
                    self.text_byte(b);
                }
            }
            Mode::Esc => match b {
                b'[' => {
                    self.csi.clear();
                    self.mode = Mode::Csi;
                }
                b']' => {
                    self.osc.clear();
                    self.osc_over = false;
                    self.mode = Mode::Osc;
                }
                b'P' | b'X' | b'^' | b'_' => self.mode = Mode::Str,
                0x20..=0x2f => self.mode = Mode::EscInter,
                0x1b => {}
                _ => self.mode = Mode::Ground,
            },
            Mode::EscInter => match b {
                0x20..=0x2f => {}
                0x1b => self.mode = Mode::Esc,
                _ => self.mode = Mode::Ground,
            },
            Mode::Csi => match b {
                0x20..=0x3f => {
                    if self.csi.len() < CSI_MAX {
                        self.csi.push(b);
                    }
                }
                0x40..=0x7e => {
                    self.dispatch_csi(b);
                    self.mode = Mode::Ground;
                }
                0x1b => self.mode = Mode::Esc,
                0x18 | 0x1a => self.mode = Mode::Ground,
                _ => {}
            },
            Mode::Osc => match b {
                0x07 => {
                    self.dispatch_osc(now, ev);
                    self.mode = Mode::Ground;
                }
                // ESC ends the string (it is the start of ST, `ESC \`; a stray one ends it as well)
                0x1b => {
                    self.dispatch_osc(now, ev);
                    self.mode = Mode::Esc;
                }
                0x18 | 0x1a => self.mode = Mode::Ground,
                _ => {
                    if self.osc.len() < OSC_MAX {
                        self.osc.push(b);
                    } else {
                        self.osc_over = true;
                    }
                }
            },
            Mode::Str => match b {
                0x1b => self.mode = Mode::Esc,
                0x18 | 0x1a => self.mode = Mode::Ground,
                _ => {}
            },
        }
    }

    /// A byte of text (or a control) while a command runs.
    fn text_byte(&mut self, b: u8) {
        let Some(run) = self.run.as_mut() else { return };
        if run.alt {
            return;
        }
        let t = &mut run.text;
        if b >= 0x80 {
            // UTF-8: a lead byte says how many continuation bytes follow
            if self.u_need > 0 && b & 0xc0 == 0x80 {
                self.u_acc = (self.u_acc << 6) | (b & 0x3f) as u32;
                self.u_need -= 1;
                if self.u_need == 0 {
                    t.put(char::from_u32(self.u_acc).unwrap_or('\u{fffd}'));
                }
                return;
            }
            self.u_need = 0;
            match b {
                0xc2..=0xdf => (self.u_need, self.u_acc) = (1, (b & 0x1f) as u32),
                0xe0..=0xef => (self.u_need, self.u_acc) = (2, (b & 0x0f) as u32),
                0xf0..=0xf4 => (self.u_need, self.u_acc) = (3, (b & 0x07) as u32),
                _ => t.put('\u{fffd}'),
            }
            return;
        }
        // a plain byte ends any character that was half way
        self.u_need = 0;
        match b {
            0x20..=0x7e => t.put(b as char),
            b'\n' | 0x0b | 0x0c => t.newline(),
            b'\r' => t.col = 0,
            0x08 => t.col = t.col.saturating_sub(1),
            b'\t' => t.col = ((t.col / 8 + 1) * 8).min(LINE_MAX),
            _ => {}
        }
    }

    fn dispatch_csi(&mut self, fin: u8) {
        // most sequences (colours) are none of ours
        if !matches!(fin, b'K' | b'G' | b'C' | b'D' | b'J' | b'h' | b'l') {
            return;
        }
        let Some(run) = self.run.as_mut() else { return };
        let params = &self.csi;
        // a sequence with an intermediate byte (`CSI 1 SP q`) is none of ours either
        if params.iter().any(|b| (0x20..=0x2f).contains(b)) {
            return;
        }
        let private = params.first() == Some(&b'?');
        let mut nums = [0usize; 8];
        let mut count = 1;
        for &b in params {
            match b {
                b'0'..=b'9' => nums[count - 1] = nums[count - 1].saturating_mul(10).saturating_add((b - b'0') as usize),
                b';' | b':' => count = (count + 1).min(8),
                _ => {}
            }
        }
        if private {
            // the alternate screen: `?1049 h`, `?1047 h` and `?47 h` switch it on, `l` off
            if (fin == b'h' || fin == b'l') && nums[..count].iter().any(|n| matches!(n, 47 | 1047 | 1049)) {
                run.alt = fin == b'h';
            }
            return;
        }
        if run.alt {
            return;
        }
        let t = &mut run.text;
        let n0 = nums[0];
        let n = n0.max(1);
        match fin {
            b'K' => t.erase_line(n0),
            b'G' => t.col = (n - 1).min(LINE_MAX),
            b'C' => t.col = (t.col + n).min(LINE_MAX),
            b'D' => t.col = t.col.saturating_sub(n),
            // the screen was cleared: what came before is gone from it
            b'J' if n0 == 2 || n0 == 3 => t.clear(),
            _ => {}
        }
    }

    fn dispatch_osc(&mut self, now: Instant, ev: &mut Vec<Event>) {
        let payload = std::mem::take(&mut self.osc);
        if !self.osc_over {
            let (code, rest) = split_once(&payload, b';');
            match code {
                b"7" => self.set_cwd(rest),
                b"133" => self.mark(rest, now, ev),
                b"7700" => self.private(rest),
                _ => {}
            }
        }
        self.osc = payload;
        self.osc.clear();
    }

    fn set_cwd(&mut self, url: &[u8]) {
        if let Some(path) = file_url_path(url) {
            self.cwd = path;
        }
    }

    /// OSC 133 marks: `A` a prompt, `C` output starts, `D;status` it ended.
    fn mark(&mut self, rest: &[u8], now: Instant, ev: &mut Vec<Event>) {
        let (kind, args) = split_once(rest, b';');
        match kind {
            b"A" => {
                // a prompt while a command is open: its end never came (the shell was replaced, say)
                self.run = None;
                self.pending = None;
            }
            b"C" => {
                let command = self.pending.take().unwrap_or_default();
                self.u_need = 0;
                self.run = Some(Run { command, started: now, text: Text::default(), alt: false });
                ev.push(Event::Started);
            }
            b"D" => {
                let Some(run) = self.run.take() else { return };
                let (status, _) = split_once(args, b';');
                let Some(exit) = std::str::from_utf8(status).ok().and_then(|s| s.trim().parse::<i32>().ok()) else { return };
                let (output_tail, cut) = run.text.finish();
                ev.push(Event::Finished(Finished { command: run.command, cwd: self.cwd.clone(), exit, duration: now.saturating_duration_since(run.started), output_tail, cut }));
            }
            _ => {}
        }
    }

    /// Our own OSC 7700: `cmd;<base64 of the command line>`.
    fn private(&mut self, rest: &[u8]) {
        let (kind, args) = split_once(rest, b';');
        if kind != b"cmd" {
            return;
        }
        let text = command_text(&base64_decode(args).unwrap_or_default());
        match self.run.as_mut() {
            // sent after the start mark, though the shell scripts send it before
            Some(run) if run.command.is_empty() => run.command = text,
            _ => self.pending = Some(text),
        }
    }
}

fn split_once(s: &[u8], sep: u8) -> (&[u8], &[u8]) {
    match s.iter().position(|&b| b == sep) {
        Some(i) => (&s[..i], &s[i + 1..]),
        None => (s, &[]),
    }
}

/// The path of a `file://host/path` URL, percent-decoded.
fn file_url_path(url: &[u8]) -> Option<String> {
    let rest = url.strip_prefix(b"file://")?;
    let slash = rest.iter().position(|&b| b == b'/')?;
    Some(String::from_utf8_lossy(&percent_decode(&rest[slash..])).into_owned())
}

fn percent_decode(s: &[u8]) -> Vec<u8> {
    let hex = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if s[i] == b'%' && i + 2 < s.len() {
            if let (Some(h), Some(l)) = (hex(s[i + 1]), hex(s[i + 2])) {
                out.push(h << 4 | l);
                i += 3;
                continue;
            }
        }
        out.push(s[i]);
        i += 1;
    }
    out
}

/// A command line as text: decoded, trimmed and cut to `CMD_MAX` bytes.
fn command_text(bytes: &[u8]) -> String {
    let s = String::from_utf8_lossy(bytes);
    let s = s.trim();
    if s.len() <= CMD_MAX {
        return s.to_string();
    }
    let mut end = CMD_MAX;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

/// Standard base64, forgiving about padding and whitespace.
fn base64_decode(s: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for &c in s {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b'\n' | b'\r' | b' ' => continue,
            _ => return None,
        };
        acc = acc << 6 | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// A command's output as lines of plain text: the last `KEEP_LINES` lines or `KEEP_BYTES` bytes.
#[derive(Default)]
struct Text {
    lines: VecDeque<String>,
    bytes: usize,
    /// the line being written, and where the cursor is in it
    cur: Vec<char>,
    col: usize,
    cut: bool,
}

impl Text {
    fn put(&mut self, ch: char) {
        if self.col >= LINE_MAX {
            return;
        }
        if self.col < self.cur.len() {
            self.cur[self.col] = ch;
        } else {
            self.cur.resize(self.col, ' ');
            self.cur.push(ch);
        }
        self.col += 1;
    }

    /// Printable ASCII, `put` for each byte, but at once when it all goes at the end of the line.
    fn put_ascii(&mut self, bytes: &[u8]) {
        if self.col == self.cur.len() {
            let take = bytes.len().min(LINE_MAX.saturating_sub(self.col));
            self.cur.extend(bytes[..take].iter().map(|&b| b as char));
            self.col += take;
        } else {
            for &b in bytes {
                self.put(b as char);
            }
        }
    }

    /// The line so far, without the spaces at its end.
    fn line(&self) -> String {
        let end = self.cur.iter().rposition(|&c| c != ' ').map_or(0, |p| p + 1);
        self.cur[..end].iter().collect()
    }

    fn newline(&mut self) {
        let line = self.line();
        self.cur.clear();
        self.col = 0;
        self.bytes += line.len() + 1;
        self.lines.push_back(line);
        while self.lines.len() > KEEP_LINES || self.bytes > KEEP_BYTES {
            let Some(old) = self.lines.pop_front() else { break };
            self.bytes -= old.len() + 1;
            self.cut = true;
        }
    }

    /// `CSI n K`: 0 erases from the cursor to the end of the line, 1 from the start to the cursor, 2 all of it.
    fn erase_line(&mut self, mode: usize) {
        match mode {
            0 => self.cur.truncate(self.col),
            1 => {
                let end = (self.col + 1).min(self.cur.len());
                self.cur[..end].iter_mut().for_each(|c| *c = ' ');
            }
            2 => self.cur.clear(),
            _ => {}
        }
    }

    fn clear(&mut self) {
        *self = Self::default();
    }

    /// The text, without the blank lines around it, and whether its start was dropped.
    fn finish(self) -> (String, bool) {
        let last = self.line();
        let mut lines: Vec<&str> = self.lines.iter().map(String::as_str).collect();
        lines.push(&last);
        while lines.last().is_some_and(|l| l.is_empty()) {
            lines.pop();
        }
        let first = lines.iter().position(|l| !l.is_empty()).unwrap_or(lines.len());
        (lines[first..].join("\n"), self.cut)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b64(s: &str) -> String {
        const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for c in s.as_bytes().chunks(3) {
            let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
            for k in 0..4 {
                if k <= c.len() {
                    out.push(T[(n >> (18 - 6 * k) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    fn osc(body: &str) -> String {
        format!("\x1b]{body}\x07")
    }
    /// What the integration prints around a command: its line, the start, `output`, the directory, the end.
    fn session(cmd: &str, cwd: &str, output: &str, exit: i32) -> String {
        format!("{}{}{output}{}{}{}", osc(&format!("7700;cmd;{}", b64(cmd))), osc("133;C"), osc(&format!("7;file://host{cwd}")), osc(&format!("133;D;{exit}")), osc("133;A"))
    }
    fn scan(s: &str) -> Vec<Event> {
        Scanner::new().feed(s.as_bytes(), Instant::now())
    }
    fn finished(events: &[Event]) -> &Finished {
        events.iter().find_map(|e| if let Event::Finished(f) = e { Some(f) } else { None }).expect("a finished command")
    }

    #[test]
    fn a_finished_command_reports_command_cwd_exit_and_output() {
        let ev = scan(&session("cargo test", "/home/me/sphere", "running 3 tests\r\ntest a ... FAILED\r\n", 101));
        assert_eq!(ev[0], Event::Started);
        let f = finished(&ev);
        assert_eq!(f.command, "cargo test");
        assert_eq!(f.cwd, "/home/me/sphere");
        assert_eq!(f.exit, 101);
        assert_eq!(f.output_tail, "running 3 tests\ntest a ... FAILED");
        assert!(!f.cut);
    }

    #[test]
    fn splitting_the_stream_anywhere_changes_nothing() {
        let all = session("ls /nope", "/tmp/my dir", "ls: /nope: No such file\r\n", 2);
        let whole = scan(&all);
        for size in [1, 2, 3, 5, 7, 16] {
            let mut sc = Scanner::new();
            let now = Instant::now();
            let mut ev = Vec::new();
            for chunk in all.as_bytes().chunks(size) {
                ev.extend(sc.feed(chunk, now));
            }
            assert_eq!(finished(&ev), finished(&whole), "chunks of {size}");
        }
    }

    #[test]
    fn bel_and_st_both_end_an_osc() {
        let st = format!("\x1b]7700;cmd;{}\x1b\\\x1b]133;C\x1b\\hi\r\n\x1b]133;D;3\x1b\\", b64("make"));
        let f = finished(&scan(&st)).clone();
        assert_eq!((f.command.as_str(), f.exit, f.output_tail.as_str()), ("make", 3, "hi"));
    }

    #[test]
    fn colours_and_cursor_moves_are_stripped_from_the_output() {
        let out = "\x1b[1;31merror\x1b[0m: \x1b[38;2;1;2;3mbad\x1b[0m\x1b[?25l\x1b[2 q\r\n\x1b(B\x1b[m\x1b]0;a title\x07done\x1bP+q544e\x1b\\\r\n";
        let f = finished(&scan(&session("x", "/", out, 1))).clone();
        assert_eq!(f.output_tail, "error: bad\ndone");
    }

    #[test]
    fn a_carriage_return_overwrites_the_line() {
        let out = "10%\r50%\r100% done\r\n\x1b[2K\r\x1b[Kfresh\r\nabc\x08\x08X\r\n";
        let f = finished(&scan(&session("x", "/", out, 1))).clone();
        assert_eq!(f.output_tail, "100% done\nfresh\naXc");
    }

    #[test]
    fn erase_in_line_and_cursor_moves_edit_the_line() {
        let out = "hello world\x1b[6D\x1b[K!\r\nabcdef\x1b[3D\x1b[1KZ\r\nxy\x1b[5Gz\r\n";
        let f = finished(&scan(&session("x", "/", out, 1))).clone();
        assert_eq!(f.output_tail, "hello!\n   Zef\nxy  z");
    }

    #[test]
    fn tabs_move_to_the_next_stop_of_eight() {
        let f = finished(&scan(&session("x", "/", "a\tb\r\n12345678\tc\r\n", 1))).clone();
        assert_eq!(f.output_tail, "a       b\n12345678        c");
    }

    #[test]
    fn only_the_end_of_a_long_output_is_kept() {
        let mut out = String::new();
        for i in 0..1000 {
            out.push_str(&format!("line {i}\r\n"));
        }
        let f = finished(&scan(&session("x", "/", &out, 1))).clone();
        let lines: Vec<&str> = f.output_tail.lines().collect();
        assert_eq!(lines.len(), KEEP_LINES);
        assert_eq!(lines[0], "line 600");
        assert_eq!(lines[KEEP_LINES - 1], "line 999");
        assert!(f.cut);
    }

    #[test]
    fn the_byte_cap_drops_the_oldest_lines() {
        let long = "x".repeat(1000);
        let mut out = String::new();
        for i in 0..200 {
            out.push_str(&format!("{i:03}{long}\r\n"));
        }
        let f = finished(&scan(&session("x", "/", &out, 1))).clone();
        assert!(f.output_tail.len() <= KEEP_BYTES);
        assert!(f.output_tail.ends_with(&format!("199{long}")));
        assert!(f.cut);
    }

    #[test]
    fn a_huge_output_stays_within_the_caps() {
        let mut sc = Scanner::new();
        let now = Instant::now();
        sc.feed(osc("133;C").as_bytes(), now);
        let chunk = "some output that goes on and on\r\n".repeat(2000);
        for _ in 0..300 {
            sc.feed(chunk.as_bytes(), now);
        }
        let run = sc.run.as_ref().unwrap();
        assert!(run.text.lines.len() <= KEEP_LINES && run.text.bytes <= KEEP_BYTES + 100);
        // one enormous line is cut as well
        sc.feed("y".repeat(3_000_000).as_bytes(), now);
        assert_eq!(sc.run.as_ref().unwrap().text.cur.len(), LINE_MAX);
    }

    #[test]
    fn the_exit_status_must_be_a_number() {
        for bad in ["133;D", "133;D;", "133;D;abc", "133;D;1x"] {
            let ev = scan(&format!("{}{}", osc("133;C"), osc(bad)));
            assert_eq!(ev, vec![Event::Started], "{bad}");
        }
        let ev = scan(&format!("{}{}", osc("133;C"), osc("133;D;130;aid=7")));
        assert_eq!(finished(&ev).exit, 130);
        let ev = scan(&format!("{}{}", osc("133;C"), osc("133;D;0")));
        assert_eq!(finished(&ev).exit, 0);
    }

    #[test]
    fn the_directory_is_percent_decoded() {
        let mut sc = Scanner::new();
        let now = Instant::now();
        for (url, want) in [
            ("file://mac.local/Users/me/my%20proj/%C3%A9t%C3%A9", "/Users/me/my proj/été"),
            ("file:///var/tmp", "/var/tmp"),
            ("file://host/a%25b/100%/x%zz", "/a%b/100%/x%zz"),
            ("file://host/semi;colon", "/semi;colon"),
            ("file://host/new%0Aline%1B", "/new\nline\u{1b}"),
        ] {
            sc.feed(osc(&format!("7;{url}")).as_bytes(), now);
            assert_eq!(sc.cwd(), want, "{url}");
        }
        // not a file URL: the old one stays
        sc.feed(osc("7;http://example.com/x").as_bytes(), now);
        assert_eq!(sc.cwd(), "/new\nline\u{1b}");
    }

    #[test]
    fn the_command_is_the_one_sent_just_before_the_start() {
        let a = format!("{}{}", osc(&format!("7700;cmd;{}", b64("first"))), osc(&format!("7700;cmd;{}", b64("  second \n"))));
        let ev = scan(&format!("{a}{}{}", osc("133;C"), osc("133;D;1")));
        assert_eq!(finished(&ev).command, "second");
        // after the start mark works too
        let ev = scan(&format!("{}{}{}", osc("133;C"), osc(&format!("7700;cmd;{}", b64("late"))), osc("133;D;1")));
        assert_eq!(finished(&ev).command, "late");
        // none at all: an empty command
        let ev = scan(&format!("{}{}", osc("133;C"), osc("133;D;1")));
        assert_eq!(finished(&ev).command, "");
    }

    #[test]
    fn a_command_line_is_decoded_trimmed_and_capped() {
        let long = "é".repeat(5000);
        let ev = scan(&format!("{}{}{}", osc(&format!("7700;cmd;{}", b64(&long))), osc("133;C"), osc("133;D;1")));
        let c = &finished(&ev).command;
        assert!(c.len() <= CMD_MAX && c.len() > CMD_MAX - 4 && c.chars().all(|c| c == 'é'));
        // bad base64 is no command, not a crash
        let ev = scan(&format!("{}{}{}", osc("7700;cmd;!!not base64!!"), osc("133;C"), osc("133;D;1")));
        assert_eq!(finished(&ev).command, "");
    }

    #[test]
    fn a_prompt_while_a_command_is_open_drops_it() {
        let ev = scan(&format!("{}{}{}{}", osc("133;C"), "output", osc("133;A"), osc("133;D;1")));
        assert_eq!(ev, vec![Event::Started]);
    }

    #[test]
    fn an_end_without_a_start_is_ignored() {
        assert!(scan(&osc("133;D;1")).is_empty());
        // and the first prompt of a shell is not a command
        assert!(scan(&format!("{}{}", osc("7;file://h/tmp"), osc("133;A"))).is_empty());
    }

    #[test]
    fn full_screen_programs_leave_no_output() {
        let out = "before\r\n\x1b[?1049h\x1b[2J\x1b[Hmenu menu menu\r\nmore menu\x1b[?1049lafter\r\n";
        let f = finished(&scan(&session("htop", "/", out, 1))).clone();
        assert_eq!(f.output_tail, "before\nafter");
    }

    #[test]
    fn clearing_the_screen_drops_what_came_before() {
        let f = finished(&scan(&session("x", "/", "old\r\n\x1b[H\x1b[2Jnew\r\n", 1))).clone();
        assert_eq!(f.output_tail, "new");
    }

    #[test]
    fn utf8_split_between_chunks_is_whole_again() {
        let all = session("x", "/", "naïve ☃ 日本\r\n", 1);
        let mut sc = Scanner::new();
        let now = Instant::now();
        let mut ev = Vec::new();
        for b in all.as_bytes() {
            ev.extend(sc.feed(std::slice::from_ref(b), now));
        }
        assert_eq!(finished(&ev).output_tail, "naïve ☃ 日本");
    }

    #[test]
    fn broken_utf8_becomes_replacement_characters() {
        let mut bytes = osc("133;C").into_bytes();
        bytes.extend_from_slice(b"a\xffb\xe2\x98c\x80d\r\n");
        bytes.extend_from_slice(osc("133;D;1").as_bytes());
        let ev = Scanner::new().feed(&bytes, Instant::now());
        assert_eq!(finished(&ev).output_tail, "a\u{fffd}bc\u{fffd}d");
    }

    #[test]
    fn a_runaway_osc_is_dropped_and_the_next_output_is_fine() {
        let mut s = osc("133;C");
        s.push_str("\x1b]52;c;");
        s.push_str(&"A".repeat(100_000));
        s.push('\x07');
        s.push_str("after\r\n");
        s.push_str(&osc("133;D;1"));
        let f = finished(&scan(&s)).clone();
        assert_eq!(f.output_tail, "after");
    }

    #[test]
    fn an_aborted_escape_does_not_swallow_the_output() {
        // CAN and a fresh ESC both leave a half-written sequence
        let out = "a\x1b[31\x18b\x1b[3\x1b[0mc\x1b]0;title\x18d\r\n";
        let f = finished(&scan(&session("x", "/", out, 1))).clone();
        assert_eq!(f.output_tail, "abcd");
    }

    #[test]
    fn duration_comes_from_the_clock_it_is_given() {
        let mut sc = Scanner::new();
        let t0 = Instant::now();
        sc.feed(osc("133;C").as_bytes(), t0);
        let ev = sc.feed(osc("133;D;1").as_bytes(), t0 + Duration::from_millis(3200));
        assert_eq!(finished(&ev).duration, Duration::from_millis(3200));
    }

    #[test]
    fn several_commands_in_a_row_each_report_their_own_output() {
        let s = format!("{}{}{}", session("one", "/a", "1\r\n", 1), session("two", "/b", "2\r\n", 0), session("three", "/c", "", 7));
        let ev = scan(&s);
        let done: Vec<(&str, &str, i32, &str)> = ev.iter().filter_map(|e| if let Event::Finished(f) = e { Some((f.command.as_str(), f.cwd.as_str(), f.exit, f.output_tail.as_str())) } else { None }).collect();
        assert_eq!(done, vec![("one", "/a", 1, "1"), ("two", "/b", 0, "2"), ("three", "/c", 7, "")]);
        assert_eq!(ev.iter().filter(|e| **e == Event::Started).count(), 3);
    }

    #[test]
    fn output_with_bytes_before_any_mark_is_ignored() {
        let mut sc = Scanner::new();
        let now = Instant::now();
        assert!(sc.feed(b"\x1b[1mwelcome\x1b[0m\r\nlast login: today\r\n", now).is_empty());
        assert!(!sc.running());
    }
}
