//! The web playground's sounds: the app's 22, which the page fetches beside it (`scripts/build-web.sh` makes them
//! MP3s) and plays through Web Audio (`web/index.html`'s `sphereAudio`), and speech through the browser's own British voice. M mutes (kept by the browser).
use bevy::prelude::*;
use std::time::Duration;
use wasm_bindgen::prelude::*;
use web_time::Instant;

macro_rules! sounds {
    ($($n:ident),*) => {
        const NAMES: &[&str] = &[$(stringify!($n)),*];
    };
}
sounds!(hover, tick, click, toggle, progress, toast, open, close, whoosh, collapse, expand, theme, wake, listen, think, confirm, discard, error, done, boot, grab, drop);

#[wasm_bindgen(js_namespace = sphereAudio)]
extern "C" {
    #[wasm_bindgen(js_name = load)]
    fn js_load(i: u32, name: &str);
    #[wasm_bindgen(js_name = play)]
    fn js_play(i: u32, gain: f32, rate: f32);
    #[wasm_bindgen(js_name = say)]
    fn js_say(text: &str);
    #[wasm_bindgen(js_name = hush)]
    fn js_hush();
    #[wasm_bindgen(js_name = speaking)]
    fn js_speaking() -> bool;
    #[wasm_bindgen(js_name = muted)]
    fn js_muted() -> bool;
    #[wasm_bindgen(js_name = setMuted)]
    fn js_set_muted(m: bool);
}

/// How the voice is doing, for the VOICE ring's tooltip and the System panel's Voice row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VoiceState {
    Muted,
    Speaking,
    Ready,
    Loading,
    System,
}

impl VoiceState {
    pub fn tip(self) -> &'static str {
        match self {
            VoiceState::Muted => "Muted · / to type",
            VoiceState::Speaking => "Speaking · / to type",
            VoiceState::Ready => "Ready · George · / to type",
            VoiceState::Loading => "Loading voice… · / to type",
            VoiceState::System => "Browser voice · / to type",
        }
    }
    pub fn row(self) -> &'static str {
        match self {
            VoiceState::Muted => "MUTED",
            VoiceState::Speaking => "SPEAKING",
            VoiceState::Ready => "READY",
            VoiceState::Loading => "LOADING",
            VoiceState::System => "BROWSER VOICE",
        }
    }
}

/// One step of the level smoothing: quick to rise (30 ms), slower to fall (120 ms).
pub fn smooth_level(prev: f32, target: f32, dt: f32) -> f32 {
    let tau = if target > prev { 0.03 } else { 0.12 };
    let k = 1.0 - (-dt.max(0.0) / tau).exp();
    (prev + (target - prev) * k).clamp(0.0, 1.0)
}

#[derive(Resource)]
pub struct Sound {
    queue: Vec<(usize, f32, f32)>,
    last: Vec<Option<Instant>>,
    any_last: Option<Instant>,
    pub muted: bool,
    asked: Option<Instant>,
}

impl Sound {
    fn index(name: &str) -> Option<usize> {
        NAMES.iter().position(|n| *n == name)
    }
    pub fn play(&mut self, name: &str) {
        self.play_with(name, 1.0, 0.0);
    }
    pub fn play_with(&mut self, name: &str, gain: f32, pitch: f32) {
        let Some(i) = Self::index(name) else { return };
        let now = Instant::now();
        let min = if matches!(name, "hover" | "tick" | "progress") { 60 } else { 8 };
        if self.last[i].is_some_and(|t| now.duration_since(t).as_millis() < min) {
            return;
        }
        self.last[i] = Some(now);
        self.any_last = Some(now);
        self.queue.push((i, gain, pitch));
    }
    pub fn play_q(&mut self, name: &str, gap_ms: u128) {
        if self.any_last.is_some_and(|t| t.elapsed().as_millis() < gap_ms) {
            return;
        }
        self.play(name);
    }
    pub fn play_q_with(&mut self, name: &str, gain: f32, gap_ms: u128) {
        if self.any_last.is_some_and(|t| t.elapsed().as_millis() < gap_ms) {
            return;
        }
        self.play_with(name, gain, 0.0);
    }
    /// The browser can't be measured as it speaks: a steady middling level while it does.
    pub fn level(&self) -> f32 {
        if self.speaking() {
            0.5
        } else {
            0.0
        }
    }
    pub fn speaking(&self) -> bool {
        js_speaking() || self.asked.is_some_and(|t| t.elapsed() < Duration::from_millis(600))
    }
    pub fn voice_state(&self) -> VoiceState {
        if self.muted {
            VoiceState::Muted
        } else if self.speaking() {
            VoiceState::Speaking
        } else {
            VoiceState::System
        }
    }
    pub fn is_off(&self) -> bool {
        false
    }
    pub fn set_muted(&mut self, m: bool) {
        self.muted = m;
        if m {
            self.stop_speaking();
        }
        js_set_muted(m);
    }
    pub fn speak(&mut self, text: &str) {
        self.stop_speaking();
        if self.muted {
            return;
        }
        let t: String = text.lines().filter(|l| !l.trim_start().starts_with("```")).collect::<Vec<_>>().join(" ").replace(['*', '`', '#', '_'], "");
        if !t.trim().is_empty() {
            js_say(t.trim());
            self.asked = Some(Instant::now());
        }
    }
    pub fn stop_speaking(&mut self) {
        self.asked = None;
        js_hush();
    }
}

pub fn setup(mut commands: Commands) {
    for (i, n) in NAMES.iter().enumerate() {
        js_load(i as u32, n);
    }
    commands.insert_resource(Sound { queue: Vec::new(), last: vec![None; NAMES.len()], any_last: None, muted: js_muted(), asked: None });
}

/// Hands this frame's sounds to the page (pitch in semitones becomes a playback rate, as in the app).
pub fn flush(mut s: ResMut<Sound>) {
    if s.queue.is_empty() {
        return;
    }
    let q = std::mem::take(&mut s.queue);
    if s.muted {
        return;
    }
    for (i, gain, pitch) in q {
        js_play(i as u32, gain, 2f32.powf(pitch / 12.0));
    }
}
