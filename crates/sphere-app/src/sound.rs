//! Sphere's UI sounds and voice. The 22 sounds are the prototype's own synthesis, rendered to WAV
//! (assets/sounds/README.md) and embedded in the binary. Any system calls `Sound::play` (or `play_q`, which
//! stays quiet when another sound just played); `flush` starts them. M mutes (saved); muting also stops speech.
//! Speech goes through macOS `say` with the voice the prototype picks on a Mac (Daniel, en-GB).
//!
//! Playback runs on its own thread: it opens the audio device and decodes the sounds once, after the window is up,
//! so neither delays the first frame nor costs the frame a sound plays in.
use bevy::prelude::*;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Instant;

macro_rules! sounds {
    ($($n:ident),*) => {
        const NAMES: &[&str] = &[$(stringify!($n)),*];
        const DATA: &[&[u8]] = &[$(include_bytes!(concat!("../assets/sounds/", stringify!($n), ".wav"))),*];
    };
}
sounds!(hover, tick, click, toggle, progress, toast, open, close, whoosh, collapse, expand, theme, wake, listen, think, confirm, discard, error, done, boot, grab, drop);

#[derive(Resource)]
pub struct Sound {
    tx: Option<mpsc::Sender<(usize, f32, f32)>>,
    queue: Vec<(usize, f32, f32)>,
    last: Vec<Option<Instant>>,
    any_last: Option<Instant>,
    pub muted: bool,
    /// no audio device in screenshots and benches
    off: bool,
    voice: Option<Child>,
}

impl Sound {
    fn index(name: &str) -> Option<usize> {
        NAMES.iter().position(|n| *n == name)
    }

    /// Plays a sound at gain 1.
    pub fn play(&mut self, name: &str) {
        self.play_with(name, 1.0, 0.0);
    }

    /// `gain` 0..1.5, `pitch` in semitones.
    pub fn play_with(&mut self, name: &str, gain: f32, pitch: f32) {
        let Some(i) = Self::index(name) else { return };
        let now = Instant::now();
        // hover, tick and progress repeat at most every 60 ms, everything else every 8 ms
        let min = if matches!(name, "hover" | "tick" | "progress") { 60 } else { 8 };
        if self.last[i].is_some_and(|t| now.duration_since(t).as_millis() < min) {
            return;
        }
        self.last[i] = Some(now);
        self.any_last = Some(now);
        self.queue.push((i, gain, pitch));
    }

    /// The prototype's `sfxQ`: skip when any sound played in the last `gap_ms`.
    pub fn play_q(&mut self, name: &str, gap_ms: u128) {
        if self.any_last.is_some_and(|t| t.elapsed().as_millis() < gap_ms) {
            return;
        }
        self.play(name);
    }

    pub fn set_muted(&mut self, m: bool) {
        self.muted = m;
        if m {
            self.stop_speaking();
        }
        save_muted(m);
    }

    /// Speaks `text` (cutting off anything still being said). Markdown and code are cleaned like the prototype's.
    pub fn speak(&mut self, text: &str) {
        self.stop_speaking();
        if self.muted || self.off || !cfg!(target_os = "macos") {
            return;
        }
        let t = clean_for_speech(text);
        if t.is_empty() {
            return;
        }
        self.voice = Command::new("say").args(["-v", "Daniel", "-r", "180", "--", &t]).stdout(Stdio::null()).stderr(Stdio::null()).spawn().ok();
    }

    pub fn stop_speaking(&mut self) {
        if let Some(mut c) = self.voice.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// Code blocks become "code omitted", links "link", `[label|key]` its label, markdown symbols go.
fn clean_for_speech(s: &str) -> String {
    let mut out = String::new();
    let mut in_code = false;
    for line in s.lines() {
        if line.trim_start().starts_with("```") {
            if !in_code {
                out.push_str(" code omitted. ");
            }
            in_code = !in_code;
            continue;
        }
        if in_code {
            continue;
        }
        let words: Vec<String> = line
            .split_whitespace()
            .map(|w| if w.starts_with("http://") || w.starts_with("https://") { "link".to_string() } else { w.to_string() })
            .collect();
        out.push_str(&words.join(" "));
        out.push(' ');
    }
    let mut res = String::new();
    let mut chars = out.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '[' => {
                let mut label = String::new();
                for d in chars.by_ref() {
                    if d == ']' {
                        break;
                    }
                    label.push(d);
                }
                res.push_str(label.split('|').next().unwrap_or(""));
            }
            '*' | '_' | '`' | '#' | '>' => {}
            _ => res.push(c),
        }
    }
    res.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn muted_file() -> std::path::PathBuf {
    sphere_proto::paths::default_state_dir().join("app").join("muted")
}

fn save_muted(m: bool) {
    let f = muted_file();
    if let Some(d) = f.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(f, if m { "1" } else { "0" });
}

/// The playback thread: decoded sounds, mixed into the default output device.
fn start_audio() -> mpsc::Sender<(usize, f32, f32)> {
    let (tx, rx) = mpsc::channel::<(usize, f32, f32)>();
    let _ = std::thread::Builder::new().name("sphere-audio".into()).spawn(move || {
        use rodio::Source;
        let Ok(mut sink) = rodio::DeviceSinkBuilder::open_default_sink() else { return };
        sink.log_on_drop(false);
        let decoded: Vec<Option<(u16, u32, Vec<f32>)>> = DATA.iter().map(|b| decode(b)).collect();
        while let Ok((i, gain, pitch)) = rx.recv() {
            let Some((ch, rate, data)) = &decoded[i] else { continue };
            let (Some(ch), Some(rate)) = (std::num::NonZero::new(*ch), std::num::NonZero::new(*rate)) else { continue };
            let src = rodio::buffer::SamplesBuffer::new(ch, rate, data.clone()).speed(2f32.powf(pitch / 12.0)).amplify(gain);
            sink.mixer().add(src);
        }
    });
    tx
}

/// 16-bit PCM WAV to interleaved f32 samples.
fn decode(bytes: &[u8]) -> Option<(u16, u32, Vec<f32>)> {
    let r = hound::WavReader::new(std::io::Cursor::new(bytes)).ok()?;
    let spec = r.spec();
    let data = r.into_samples::<i16>().filter_map(Result::ok).map(|v| v as f32 / 32768.0).collect();
    Some((spec.channels, spec.sample_rate, data))
}

pub fn setup(mut commands: Commands, args: Res<crate::Args>) {
    let muted = std::fs::read_to_string(muted_file()).map(|s| s.trim() == "1").unwrap_or(false);
    commands.insert_resource(Sound {
        tx: if args.offline() { None } else { Some(start_audio()) },
        queue: Vec::new(),
        last: vec![None; NAMES.len()],
        any_last: None,
        muted,
        off: args.offline(),
        voice: None,
    });
}

/// Hands the queued sounds to the playback thread.
pub fn flush(mut s: ResMut<Sound>) {
    if s.queue.is_empty() {
        return;
    }
    let q = std::mem::take(&mut s.queue);
    if s.muted || s.off {
        return;
    }
    if let Some(tx) = &s.tx {
        for item in q {
            let _ = tx.send(item);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speech_drops_markup_code_and_links() {
        let t = clean_for_speech("**Done.** See https://x.dev and [the card|C].\n```\nlet x = 1;\n```\nok");
        assert_eq!(t, "Done. See link and the card. code omitted. ok");
    }

    #[test]
    fn every_sound_is_embedded() {
        assert_eq!(NAMES.len(), 22);
        assert!(DATA.iter().all(|d| d.len() > 1000 && &d[0..4] == b"RIFF"));
        assert!(DATA.iter().all(|d| decode(d).is_some_and(|(c, r, v)| c > 0 && r > 0 && !v.is_empty())));
    }
}
