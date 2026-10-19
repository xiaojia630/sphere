//! Motion: how fast the transitions play. Fast (the default) runs the same springs on a quicker clock: stepping
//! a spring by `dt · s` is the spring with stiffness `k · s²` and damping `c · s`, so every path, overshoot and
//! damping ratio looks the same, only `s` times sooner. Cinematic is the original timing. Idle motion (drift,
//! spin, pulses) runs on the plain clock either way. Fast can be tuned further with a multiplier (`motion 1.5`).
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Motion {
    #[default]
    Fast,
    Cinematic,
}

/// Which clock a transition runs on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pace {
    /// Big moves: an app or the Working view opening, the Agent moment card, the core opening into apps, boot.
    Big,
    /// The rest of the interface: panels, the core's rings, the home view.
    Ui,
}

impl Motion {
    pub fn key(self) -> &'static str {
        match self {
            Motion::Fast => "fast",
            Motion::Cinematic => "cinematic",
        }
    }
    pub fn from_key(s: &str) -> Option<Motion> {
        match s.trim().to_ascii_lowercase().as_str() {
            "fast" | "quick" | "snappy" => Some(Motion::Fast),
            "cinematic" | "slow" | "film" => Some(Motion::Cinematic),
            _ => None,
        }
    }
    /// How many times faster than the original timing a transition of this pace plays.
    pub fn scale(self, p: Pace) -> f32 {
        match (self, p) {
            (Motion::Cinematic, _) => 1.0,
            (Motion::Fast, Pace::Big) => 6.0,
            (Motion::Fast, Pace::Ui) => 2.4,
        }
    }
}

static NOW: AtomicU8 = AtomicU8::new(0);
/// Fast's own multiplier (`motion 1.5`), as f32 bits; 1 by default.
static BOOST: AtomicU32 = AtomicU32::new(0x3f80_0000);

/// The range a typed multiplier is held to: slower than a quarter of Fast is Cinematic's job, and past 4x the
/// transitions are over in a frame or two.
pub const BOOST_MIN: f32 = 0.25;
pub const BOOST_MAX: f32 = 4.0;

pub fn boost() -> f32 {
    f32::from_bits(BOOST.load(Ordering::Relaxed))
}

/// Sets Fast's multiplier (held to BOOST_MIN..=BOOST_MAX) without saving it.
pub fn use_boost(b: f32) {
    let b = if b.is_finite() { b.clamp(BOOST_MIN, BOOST_MAX) } else { 1.0 };
    BOOST.store(b.to_bits(), Ordering::Relaxed);
}

/// How many times faster than the original timing a transition of this pace plays now.
pub fn scale(p: Pace) -> f32 {
    match get() {
        Motion::Fast => Motion::Fast.scale(p) * boost(),
        Motion::Cinematic => 1.0,
    }
}

pub fn get() -> Motion {
    if NOW.load(Ordering::Relaxed) == 1 {
        Motion::Cinematic
    } else {
        Motion::Fast
    }
}

/// Sets the motion for the whole app and remembers it (Fast with its multiplier).
pub fn set(m: Motion) {
    use_motion(m);
    save();
}

/// Fast at `b` times its speed, remembered.
pub fn set_boost(b: f32) {
    use_motion(Motion::Fast);
    use_boost(b);
    save();
}

fn save() {
    let f = pref_file();
    if let Some(d) = f.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(f, key_with_boost(get(), boost()));
}

/// What the preference file holds: "cinematic", "fast" or "fast 1.5".
fn key_with_boost(m: Motion, b: f32) -> String {
    if m == Motion::Fast && (b - 1.0).abs() > 1e-3 {
        format!("fast {}", (b * 100.0).round() / 100.0)
    } else {
        m.key().to_string()
    }
}

/// Reads "fast", "cinematic" or "fast 1.5".
fn parse_saved(s: &str) -> Option<(Motion, f32)> {
    let mut it = s.split_whitespace();
    let m = Motion::from_key(it.next()?)?;
    let b = it.next().and_then(|v| v.parse::<f32>().ok()).filter(|b| b.is_finite()).unwrap_or(1.0).clamp(BOOST_MIN, BOOST_MAX);
    Some((m, b))
}

/// Sets the motion without saving it (the saved one at start, the look tests).
pub fn use_motion(m: Motion) {
    NOW.store(if m == Motion::Cinematic { 1 } else { 0 }, Ordering::Relaxed);
}

/// The time step for a transition of this pace.
pub fn dt(p: Pace, dt: f32) -> f32 {
    dt * scale(p)
}

/// A delay of the original timing (a stagger, a pause), at the current speed.
pub fn delay(p: Pace, secs: f32) -> f32 {
    secs / scale(p)
}

/// A stagger between items of one move (the orbit's tiles fanning out): tighter still in Fast, so the last item
/// starts while the first is still moving.
pub fn stagger(secs: f32) -> f32 {
    match get() {
        Motion::Fast => secs / (scale(Pace::Big) * 1.6),
        Motion::Cinematic => secs,
    }
}

fn pref_file() -> std::path::PathBuf {
    sphere_proto::paths::default_state_dir().join("app").join("motion")
}

/// The saved motion and Fast's multiplier.
pub fn saved() -> (Motion, f32) {
    std::fs::read_to_string(pref_file()).ok().and_then(|s| parse_saved(&s)).unwrap_or((Motion::Fast, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::Spring;

    /// The motion and multiplier are process-wide: tests that change them take turns.
    static GLOBAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Seconds for a spring from 0 to settle within 2 % of 1, stepped at 60 fps with this scale.
    fn settle(k: f32, c: f32, s: f32) -> f32 {
        let mut sp = Spring::new(0.0, k, c);
        sp.t = 1.0;
        let mut t = 0.0;
        let mut last_out = 0.0;
        while t < 10.0 {
            sp.step(s / 60.0);
            t += 1.0 / 60.0;
            if (sp.x - 1.0).abs() > 0.02 {
                last_out = t;
            }
        }
        last_out
    }

    #[test]
    fn scaling_the_clock_is_the_same_spring_sooner() {
        // stepping by dt·s matches the spring with k·s² and c·s on the plain clock
        let (k, c, s) = (26.0, 9.0, 4.0);
        let mut a = Spring::new(0.0, k, c);
        let mut b = Spring::new(0.0, k * s * s, c * s);
        a.t = 1.0;
        b.t = 1.0;
        for _ in 0..30 {
            a.step(s / 60.0);
            for _ in 0..10 {
                b.step(1.0 / 600.0);
            }
            // the same curve (the scaled one is sub-stepped finer, so only the discretisation differs)
            assert!((a.x - b.x).abs() < 0.01, "{} vs {}", a.x, b.x);
        }
    }

    #[test]
    fn fast_settles_app_opening_in_about_a_third_of_a_second() {
        let slow = settle(26.0, 9.0, Motion::Cinematic.scale(Pace::Big));
        let fast = settle(26.0, 9.0, Motion::Fast.scale(Pace::Big));
        assert!(slow > 0.8, "cinematic stays as it was: {slow}");
        assert!(fast < 0.16, "fast app opening settles in {fast} s");
        // panels: 120/20 moves and 50/13 fades
        assert!(settle(50.0, 13.0, Motion::Fast.scale(Pace::Ui)) < 0.32);
        assert!(settle(120.0, 20.0, Motion::Fast.scale(Pace::Ui)) < 0.21);
    }

    #[test]
    fn keys_round_trip_and_delays_shrink() {
        let _g = GLOBAL.lock().unwrap_or_else(|e| e.into_inner());
        for m in [Motion::Fast, Motion::Cinematic] {
            assert_eq!(Motion::from_key(m.key()), Some(m));
        }
        assert_eq!(Motion::from_key("nope"), None);
        assert_eq!(Motion::default(), Motion::Fast);
        // the orbit's last tile starts 0.16 + 7 · 0.075 s in on the original clock
        use_motion(Motion::Fast);
        assert!(stagger(0.16 + 7.0 * 0.075) < 0.15);
        assert!(delay(Pace::Big, 0.03) < 0.006);
        use_motion(Motion::Cinematic);
        assert_eq!(stagger(0.685), 0.685);
        assert_eq!(delay(Pace::Big, 0.03), 0.03);
        use_motion(Motion::Fast);
    }

    /// Seconds until a transition settles (every part within 2 % of where it ends), at 60 fps: `lead` is the
    /// transition's own spring, `follow` the springs that chase it (an app's panes: opacity 50/13, depth 60/14, from
    /// panel.rs), each `items` stagger delay a part of its own (the orbit's tiles).
    fn settle_chain(m: Motion, lead: Spring, follow: &[(f32, f32)], items: &[(f32, Spring)]) -> f32 {
        use_motion(m);
        let dt = 1.0 / 60.0;
        let mut lead = lead;
        lead.t = 1.0;
        let mut fol: Vec<Spring> = follow.iter().map(|&(k, c)| Spring::new(0.0, k, c)).collect();
        let mut its: Vec<(f32, Spring)> = items.iter().map(|&(d, sp)| (stagger(d), sp)).collect();
        let (mut t, mut last) = (0.0, 0.0);
        while t < 6.0 {
            let sdt = self::dt(Pace::Big, dt);
            lead.step(sdt);
            let mut off = (lead.x - 1.0).abs() > 0.02;
            for f in &mut fol {
                f.t = lead.x;
                f.step(sdt);
                off |= (f.x - 1.0).abs() > 0.02;
            }
            for (d, sp) in &mut its {
                if t >= *d {
                    sp.t = 1.0;
                }
                sp.step(sdt);
                off |= (sp.x - 1.0).abs() > 0.02;
            }
            t += dt;
            if off {
                last = t;
            }
        }
        use_motion(Motion::Fast);
        last
    }

    /// `cargo test -p sphere-app timings -- --nocapture` prints the table.
    #[test]
    fn timings() {
        let _g = GLOBAL.lock().unwrap_or_else(|e| e.into_inner());
        let panes = [(50.0, 13.0), (60.0, 14.0)];
        let app = crate::codeapp::CodeApp::default().amt;
        let work = crate::working::Working::default().amt;
        let orb = crate::orbit::Orb::default();
        let tiles: Vec<(f32, Spring)> = (0..8).map(|k| (0.16 + k as f32 * 0.075, orb.em[k])).collect();
        let rows = [
            ("app open (panes)", settle_chain(Motion::Cinematic, app, &panes, &[]), settle_chain(Motion::Fast, app, &panes, &[])),
            ("Working view (W)", settle_chain(Motion::Cinematic, work, &panes, &[]), settle_chain(Motion::Fast, work, &panes, &[])),
            ("app orbit (A)", settle_chain(Motion::Cinematic, orb.amt, &[], &tiles), settle_chain(Motion::Fast, orb.amt, &[], &tiles)),
        ];
        for (name, slow, fast) in rows {
            println!("{name:18} cinematic {slow:.2} s  fast {fast:.2} s");
            assert!(fast < 0.25 && fast < slow / 5.0, "{name}: {fast} s");
        }
    }

    #[test]
    fn a_typed_multiplier_speeds_fast_up_and_is_remembered_as_text() {
        let _g = GLOBAL.lock().unwrap_or_else(|e| e.into_inner());
        use_motion(Motion::Fast);
        use_boost(1.5);
        assert!((scale(Pace::Big) - 9.0).abs() < 1e-4);
        assert_eq!(key_with_boost(Motion::Fast, 1.5), "fast 1.5");
        assert_eq!(key_with_boost(Motion::Fast, 1.0), "fast");
        assert_eq!(key_with_boost(Motion::Cinematic, 1.5), "cinematic");
        assert_eq!(parse_saved("fast 1.5"), Some((Motion::Fast, 1.5)));
        assert_eq!(parse_saved("fast"), Some((Motion::Fast, 1.0)));
        assert_eq!(parse_saved("cinematic\n"), Some((Motion::Cinematic, 1.0)));
        assert_eq!(parse_saved("fast 99"), Some((Motion::Fast, BOOST_MAX)));
        assert_eq!(parse_saved("nope"), None);
        // held to its range; Cinematic ignores it
        use_boost(100.0);
        assert_eq!(boost(), BOOST_MAX);
        use_boost(f32::NAN);
        assert_eq!(boost(), 1.0);
        use_motion(Motion::Cinematic);
        use_boost(2.0);
        assert_eq!(scale(Pace::Big), 1.0);
        use_boost(1.0);
        use_motion(Motion::Fast);
    }

    #[test]
    fn the_fastest_setting_is_still_the_same_curve() {
        // at the top multiplier even the stiffest spring (the kick, 220/10) lands every frame exactly where the
        // original curve is at that moment of its own time: the sub-steps keep it stable (no ringing or jitter),
        // it just gets there sooner
        let s = Motion::Fast.scale(Pace::Big) * BOOST_MAX;
        let mut fast = Spring::new(0.0, 220.0, 10.0);
        let mut slow = Spring::new(0.0, 220.0, 10.0);
        fast.t = 1.0;
        slow.t = 1.0;
        for _ in 0..120 {
            fast.step(s / 60.0);
            // the same span of spring time, in the original 120 Hz sub-steps
            for _ in 0..((s / 60.0 * 120.0).ceil() as usize) {
                slow.step(s / 60.0 / (s / 60.0 * 120.0).ceil());
            }
            assert!(fast.x.is_finite() && (fast.x - slow.x).abs() < 1e-3, "{} vs {}", fast.x, slow.x);
        }
        assert!((fast.x - 1.0).abs() < 1e-3);
    }
}
