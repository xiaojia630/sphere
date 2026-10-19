//! Sphere's UI sounds and voice. The 22 sounds are the prototype's own synthesis, rendered to WAV
//! (assets/sounds/README.md) and embedded in the binary. Any system calls `Sound::play` (or `play_q`, which
//! stays quiet when another sound just played); `flush` starts them. M mutes (saved); muting also stops speech.
//! Speech is Kokoro's British George (crates/sphere-voice), loaded on its own thread a moment after the window is
//! up; until it is ready, or if it can't load, speech goes through macOS `say` with the prototype's voice (Daniel,
//! en-GB). `SPHERE_VOICE=system` keeps the `say` voice.
//!
//! Playback runs on its own thread: it opens the audio device and decodes the sounds once, after the window is up,
//! so neither delays the first frame nor costs the frame a sound plays in. Speech plays through the same device as
//! it is synthesised, a sentence at a time.
use bevy::prelude::*;
use std::collections::VecDeque;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

macro_rules! sounds {
    ($($n:ident),*) => {
        const NAMES: &[&str] = &[$(stringify!($n)),*];
        const DATA: &[&[u8]] = &[$(include_bytes!(concat!("../assets/sounds/", stringify!($n), ".wav"))),*];
    };
}
sounds!(hover, tick, click, toggle, progress, toast, open, close, whoosh, collapse, expand, theme, wake, listen, think, confirm, discard, error, done, boot, grab, drop);

/// What the playback thread is asked to do.
enum Play {
    /// a sound, its gain and pitch
    Sfx(usize, f32, f32),
    /// speech for utterance `gen`
    Speech(sphere_voice::Chunk),
    /// stop speaking; only utterance `gen` and later may play
    Hush(u64),
}

/// The voice starts loading this long after the app starts, so the first frames have the CPU.
const VOICE_DELAY: Duration = Duration::from_millis(1500);

/// What the playback thread measures of the speech it is playing: a smoothed RMS level 0..1 and whether it is speaking.
#[derive(Default)]
struct Meter {
    level: AtomicU32,
    speaking: AtomicBool,
}

/// How the voice is doing, for the VOICE ring's tooltip and the System panel's Voice row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VoiceState {
    Muted,
    Speaking,
    /// Kokoro (George) is loaded
    Ready,
    Loading,
    /// the macOS `say` fallback
    System,
}

impl VoiceState {
    /// From the facts: `kokoro` is None when it isn't going to start (SPHERE_VOICE=system), else (ready, failed).
    pub fn of(muted: bool, speaking: bool, kokoro: Option<(bool, bool)>) -> VoiceState {
        if muted {
            return VoiceState::Muted;
        }
        if speaking {
            return VoiceState::Speaking;
        }
        match kokoro {
            Some((true, _)) => VoiceState::Ready,
            Some((false, false)) => VoiceState::Loading,
            _ => VoiceState::System,
        }
    }
    /// The VOICE ring's tooltip line.
    pub fn tip(self) -> &'static str {
        match self {
            VoiceState::Muted => "Muted · / to type",
            VoiceState::Speaking => "Speaking · / to type",
            VoiceState::Ready => "Ready · George · / to type",
            VoiceState::Loading => "Loading voice… · / to type",
            VoiceState::System => "System voice · / to type",
        }
    }
    /// The System panel's Voice row.
    pub fn row(self) -> &'static str {
        match self {
            VoiceState::Muted => "MUTED",
            VoiceState::Speaking => "SPEAKING",
            VoiceState::Ready => "READY",
            VoiceState::Loading => "LOADING",
            VoiceState::System => "SYSTEM VOICE",
        }
    }
}

/// One step of the level smoothing: quick to rise (30 ms), slower to fall (120 ms), so the spokes follow the syllables.
pub fn smooth_level(prev: f32, target: f32, dt: f32) -> f32 {
    let tau = if target > prev { 0.03 } else { 0.12 };
    let k = 1.0 - (-dt.max(0.0) / tau).exp();
    (prev + (target - prev) * k).clamp(0.0, 1.0)
}

/// Root mean square of `samples`, scaled so ordinary speech (RMS ~0.1) reads about 0.6 and clipped to 0..1.
pub fn level_of(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let rms = (samples.iter().map(|v| v * v).sum::<f32>() / samples.len() as f32).sqrt();
    (rms * 6.0).min(1.0)
}

/// Where in the queued speech the speakers are now: chunks play back to back, each starting when the last ends.
#[derive(Default)]
struct Playhead {
    segs: VecDeque<(f64, Vec<f32>)>,
    end: f64,
}

impl Playhead {
    const RATE: f64 = sphere_voice::SAMPLE_RATE as f64;
    fn push(&mut self, now: f64, samples: Vec<f32>) {
        let start = self.end.max(now);
        self.end = start + samples.len() as f64 / Self::RATE;
        self.segs.push_back((start, samples));
    }
    fn clear(&mut self) {
        self.segs.clear();
        self.end = 0.0;
    }
    /// The level of the ~20 ms around `now`, and whether anything is queued or playing.
    fn level_at(&mut self, now: f64) -> (f32, bool) {
        while self.segs.front().is_some_and(|(s, v)| s + v.len() as f64 / Self::RATE < now) {
            self.segs.pop_front();
        }
        let busy = !self.segs.is_empty();
        let Some((s, v)) = self.segs.front().filter(|(s, _)| *s <= now) else { return (0.0, busy) };
        let i = ((now - s) * Self::RATE) as usize;
        let (a, b) = (i.saturating_sub(240), (i + 240).min(v.len()));
        (if a < b { level_of(&v[a..b]) } else { 0.0 }, busy)
    }
}

#[derive(Resource)]
pub struct Sound {
    tx: Option<mpsc::Sender<Play>>,
    queue: Vec<(usize, f32, f32)>,
    last: Vec<Option<Instant>>,
    any_last: Option<Instant>,
    pub muted: bool,
    /// no audio device in screenshots and benches
    off: bool,
    /// Kokoro, once started
    voice: Option<sphere_voice::Speaker>,
    /// when to start the voice (None: never, or already started)
    voice_at: Option<Instant>,
    /// the `say` fallback
    say: Option<Child>,
    /// the playback thread's measurements of the speech
    meter: Arc<Meter>,
    /// when the last Kokoro line was asked for (its first audio takes a moment)
    asked: Option<Instant>,
    /// `SPHERE_VOICE=system`: Kokoro is not going to load
    system_only: bool,
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

    /// `play_q` at a gain.
    pub fn play_q_with(&mut self, name: &str, gain: f32, gap_ms: u128) {
        if self.any_last.is_some_and(|t| t.elapsed().as_millis() < gap_ms) {
            return;
        }
        self.play_with(name, gain, 0.0);
    }

    /// How loud the speech being played is right now, 0..1 (smoothed RMS of the samples going out). The `say` voice
    /// can't be measured, so while it talks this is a steady middling level.
    pub fn level(&self) -> f32 {
        if self.say.is_some() {
            return 0.5;
        }
        f32::from_bits(self.meter.level.load(Ordering::Relaxed))
    }

    /// True while Sphere is saying something (either voice).
    pub fn speaking(&self) -> bool {
        self.say.is_some() || self.meter.speaking.load(Ordering::Relaxed) || self.asked.is_some_and(|t| t.elapsed() < Duration::from_millis(900))
    }

    pub fn voice_state(&self) -> VoiceState {
        let kokoro = if self.system_only || self.off { None } else { Some(self.voice.as_ref().map_or((false, false), |v| (v.ready(), v.failed()))) };
        let state = VoiceState::of(self.muted, self.speaking(), kokoro);
        // screenshots and benches have no audio device at all: show the voice as ready
        if self.off && state == VoiceState::System {
            return VoiceState::Ready;
        }
        state
    }

    /// True when the app has no audio at all (screenshots, benches, --demo): the spokes keep their idle motion.
    pub fn is_off(&self) -> bool {
        self.off
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
        if self.muted || self.off {
            return;
        }
        let t = clean_for_speech(text);
        if t.is_empty() {
            return;
        }
        if let (Some(v), Some(tx)) = (&self.voice, &self.tx) {
            if let Some(id) = v.speak(&t) {
                let _ = tx.send(Play::Hush(id));
                self.asked = Some(Instant::now());
                return;
            }
        }
        if !cfg!(target_os = "macos") {
            return;
        }
        self.asked = None;
        self.say = Command::new("say").args(["-v", "Daniel", "-r", "180", "--", &t]).stdout(Stdio::null()).stderr(Stdio::null()).spawn().ok();
    }

    pub fn stop_speaking(&mut self) {
        if let (Some(v), Some(tx)) = (&self.voice, &self.tx) {
            let _ = tx.send(Play::Hush(v.stop()));
        }
        self.asked = None;
        if let Some(mut c) = self.say.take() {
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

/// The playback thread: decoded sounds, mixed into the default output device, and a queue for speech.
fn start_audio(meter: Arc<Meter>) -> mpsc::Sender<Play> {
    let (tx, rx) = mpsc::channel::<Play>();
    let _ = std::thread::Builder::new().name("sphere-audio".into()).spawn(move || {
        use rodio::Source;
        let Ok(mut sink) = rodio::DeviceSinkBuilder::open_default_sink() else { return };
        sink.log_on_drop(false);
        let decoded: Vec<Option<(u16, u32, Vec<f32>)>> = DATA.iter().map(|b| decode(b)).collect();
        let mut speech = rodio::Player::connect_new(sink.mixer());
        let mut speaking = 0u64;
        let t0 = Instant::now();
        let mut head = Playhead::default();
        let (mut level, mut last, mut hold, mut idle) = (0.0f32, 0.0f64, 0.0f64, true);
        loop {
            // the meter looks every 16 ms while speech plays or has just stopped; with nothing to follow, the thread
            // sleeps until it is handed a sound
            let m = if idle {
                match rx.recv() {
                    Ok(m) => Some(m),
                    Err(_) => return,
                }
            } else {
                match rx.recv_timeout(Duration::from_millis(16)) {
                    Ok(m) => Some(m),
                    Err(mpsc::RecvTimeoutError::Timeout) => None,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            };
            let now = t0.elapsed().as_secs_f64();
            if idle {
                // asleep, the level stayed at zero: it starts moving from here, as it did when every tick looked
                last = now;
            }
            if let Some(m) = m {
              match m {
                Play::Sfx(i, gain, pitch) => {
                    let Some((ch, rate, data)) = &decoded[i] else { continue };
                    let (Some(ch), Some(rate)) = (std::num::NonZero::new(*ch), std::num::NonZero::new(*rate)) else { continue };
                    let src = rodio::buffer::SamplesBuffer::new(ch, rate, data.clone()).speed(2f32.powf(pitch / 12.0)).amplify(gain);
                    sink.mixer().add(src);
                }
                Play::Hush(gen) => {
                    // a fresh queue: dropping the old one stops whatever it was saying
                    speaking = gen;
                    head.clear();
                    if !speech.empty() {
                        speech = rodio::Player::connect_new(sink.mixer());
                    }
                }
                Play::Speech(c) => {
                    if c.gen == speaking && !c.samples.is_empty() {
                        let (Some(ch), Some(rate)) = (std::num::NonZero::new(1), std::num::NonZero::new(sphere_voice::SAMPLE_RATE)) else { continue };
                        head.push(now, c.samples.clone());
                        speech.append(rodio::buffer::SamplesBuffer::new(ch, rate, c.samples));
                    }
                }
              }
            }
            let (raw, busy) = head.level_at(now);
            if busy {
                hold = now + 0.35;
            }
            level = smooth_level(level, raw, (now - last) as f32);
            // the speech is over and its level has died away: nothing to follow until the next sound
            idle = !busy && now >= hold && level < 1e-3;
            if idle {
                level = 0.0;
            }
            last = now;
            meter.level.store(level.to_bits(), Ordering::Relaxed);
            meter.speaking.store(busy || now < hold, Ordering::Relaxed);
        }
    });
    tx
}

/// Starts Kokoro on its own thread, its speech going to the playback thread.
fn start_voice(tx: &mpsc::Sender<Play>) -> sphere_voice::Speaker {
    let tx = tx.clone();
    sphere_voice::Speaker::start(sphere_voice::data_dir(), move |c| {
        let _ = tx.send(Play::Speech(c));
    })
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
    let system_voice = std::env::var("SPHERE_VOICE").is_ok_and(|v| v == "system");
    let meter = Arc::new(Meter::default());
    commands.insert_resource(Sound {
        tx: if args.offline() { None } else { Some(start_audio(meter.clone())) },
        queue: Vec::new(),
        last: vec![None; NAMES.len()],
        any_last: None,
        muted,
        off: args.offline(),
        voice: None,
        voice_at: (!args.offline() && !system_voice).then(|| Instant::now() + VOICE_DELAY),
        say: None,
        meter,
        asked: None,
        system_only: system_voice,
    });
}

#[cfg(test)]
impl Default for Sound {
    fn default() -> Self {
        Sound { tx: None, queue: Vec::new(), last: vec![None; NAMES.len()], any_last: None, muted: false, off: true, voice: None, voice_at: None, say: None, meter: Arc::default(), asked: None, system_only: false }
    }
}

/// Hands the queued sounds to the playback thread (and starts the voice once the app has settled).
pub fn flush(mut s: ResMut<Sound>) {
    // the `say` voice is done when its process is
    if s.say.as_mut().is_some_and(|c| c.try_wait().map_or(true, |st| st.is_some())) {
        s.say = None;
    }
    if s.voice_at.is_some_and(|t| Instant::now() >= t) {
        s.voice_at = None;
        s.voice = s.tx.as_ref().map(start_voice);
    }
    if s.queue.is_empty() {
        return;
    }
    let q = std::mem::take(&mut s.queue);
    if s.muted || s.off {
        return;
    }
    if let Some(tx) = &s.tx {
        for (i, gain, pitch) in q {
            let _ = tx.send(Play::Sfx(i, gain, pitch));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_rises_fast_and_falls_slowly() {
        let up = smooth_level(0.0, 1.0, 0.016);
        let down = smooth_level(1.0, 0.0, 0.016);
        assert!(up > 0.3 && up < 0.5, "{up}");
        assert!(down > 0.85 && down < 0.95, "{down}");
        let mut v = 0.0;
        for _ in 0..120 {
            v = smooth_level(v, 0.4, 0.016);
        }
        assert!((v - 0.4).abs() < 0.01);
        assert_eq!(smooth_level(0.3, 0.9, 0.0), 0.3);
    }

    #[test]
    fn rms_scales_and_clips() {
        assert_eq!(level_of(&[]), 0.0);
        assert_eq!(level_of(&[0.0; 100]), 0.0);
        assert!((level_of(&[0.1; 100]) - 0.6).abs() < 1e-4);
        assert_eq!(level_of(&[0.9; 100]), 1.0);
    }

    #[test]
    fn playhead_follows_what_is_queued() {
        let mut h = Playhead::default();
        assert_eq!(h.level_at(0.0), (0.0, false));
        // half a second of a steady tone, queued at t = 1, then one more right behind it
        h.push(1.0, vec![0.1; 12_000]);
        h.push(1.1, vec![0.0; 12_000]);
        let (l, busy) = h.level_at(1.25);
        assert!(busy && (l - 0.6).abs() < 1e-3, "{l}");
        // the second chunk is silence, but still queued
        assert_eq!(h.level_at(1.75), (0.0, true));
        assert_eq!(h.level_at(2.5), (0.0, false));
        h.push(3.0, vec![0.1; 100]);
        h.clear();
        assert_eq!(h.level_at(3.0), (0.0, false));
    }

    #[test]
    fn voice_state_names() {
        use VoiceState::*;
        assert_eq!(VoiceState::of(true, true, Some((true, false))), Muted);
        assert_eq!(VoiceState::of(false, true, None), Speaking);
        assert_eq!(VoiceState::of(false, false, Some((true, false))), Ready);
        assert_eq!(VoiceState::of(false, false, Some((false, false))), Loading);
        assert_eq!(VoiceState::of(false, false, Some((false, true))), System);
        assert_eq!(VoiceState::of(false, false, None), System);
        assert_eq!(Ready.tip(), "Ready · George · / to type");
        assert_eq!(Speaking.tip(), "Speaking · / to type");
        assert_eq!(Loading.tip(), "Loading voice… · / to type");
        assert_eq!(System.tip(), "System voice · / to type");
        assert_eq!(Muted.tip(), "Muted · / to type");
        assert_eq!([Speaking.row(), Ready.row(), Muted.row(), Loading.row(), System.row()], ["SPEAKING", "READY", "MUTED", "LOADING", "SYSTEM VOICE"]);
    }

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
