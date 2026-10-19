//! Motion: how fast the transitions play. Fast (the default) runs the same springs on a quicker clock: stepping
//! a spring by `dt · s` is the spring with stiffness `k · s²` and damping `c · s`, so every path, overshoot and
//! damping ratio looks the same, only `s` times sooner. Cinematic is the original timing. Idle motion (drift,
//! spin, pulses) runs on the plain clock either way.
use std::sync::atomic::{AtomicU8, Ordering};

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
            (Motion::Fast, Pace::Big) => 4.0,
            (Motion::Fast, Pace::Ui) => 1.6,
        }
    }
}

static NOW: AtomicU8 = AtomicU8::new(0);

pub fn get() -> Motion {
    if NOW.load(Ordering::Relaxed) == 1 {
        Motion::Cinematic
    } else {
        Motion::Fast
    }
}

/// Sets the motion for the whole app and remembers it.
pub fn set(m: Motion) {
    use_motion(m);
    let f = pref_file();
    if let Some(d) = f.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(f, m.key());
}

/// Sets the motion without saving it (the saved one at start, the look tests).
pub fn use_motion(m: Motion) {
    NOW.store(if m == Motion::Cinematic { 1 } else { 0 }, Ordering::Relaxed);
}

/// The time step for a transition of this pace.
pub fn dt(p: Pace, dt: f32) -> f32 {
    dt * get().scale(p)
}

/// A delay of the original timing (a stagger, a pause), at the current speed.
pub fn delay(p: Pace, secs: f32) -> f32 {
    secs / get().scale(p)
}

/// A stagger between items of one move (the orbit's tiles fanning out): tighter still in Fast, so the last item
/// starts while the first is still moving.
pub fn stagger(secs: f32) -> f32 {
    match get() {
        Motion::Fast => secs / (Motion::Fast.scale(Pace::Big) * 1.6),
        Motion::Cinematic => secs,
    }
}

fn pref_file() -> std::path::PathBuf {
    sphere_proto::paths::default_state_dir().join("app").join("motion")
}

pub fn saved() -> Motion {
    std::fs::read_to_string(pref_file()).ok().and_then(|s| Motion::from_key(&s)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::Spring;

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
        assert!(fast < 0.25, "fast app opening settles in {fast} s");
        // panels: 120/20 moves and 50/13 fades
        assert!(settle(50.0, 13.0, Motion::Fast.scale(Pace::Ui)) < 0.5);
        assert!(settle(120.0, 20.0, Motion::Fast.scale(Pace::Ui)) < 0.32);
    }

    #[test]
    fn keys_round_trip_and_delays_shrink() {
        for m in [Motion::Fast, Motion::Cinematic] {
            assert_eq!(Motion::from_key(m.key()), Some(m));
        }
        assert_eq!(Motion::from_key("nope"), None);
        assert_eq!(Motion::default(), Motion::Fast);
        // the orbit's last tile starts 0.16 + 7 · 0.075 s in on the original clock
        use_motion(Motion::Fast);
        assert!(stagger(0.16 + 7.0 * 0.075) < 0.15);
        assert!(delay(Pace::Big, 0.03) < 0.008);
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
            assert!(fast < 0.4 && fast < slow / 3.0, "{name}: {fast} s");
        }
    }
}
