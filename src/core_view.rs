//! The core: the agent's presence on the desktop. It breathes when idle,
//! spins up while agents work, turns amber when something needs you, and
//! opens a ring of apps when clicked.

use std::f32::consts::{PI, TAU};

use eframe::egui::{self, Color32, Id, Painter, Pos2, Rect, Sense, Shape, Stroke, Ui, Vec2};

use crate::apps::AppKind;
use crate::icons;
use crate::runtime::CoreState;
use crate::theme::{self, alpha, CYAN, DEEP, ICE, LINE_STRONG, PANEL};

#[derive(Default)]
pub struct CoreView {
    pub ring_open: bool,
    ring: f32,
    energy: f32,
}

impl CoreView {
    /// Draws the core centred at `center` with diameter `size`. Returns the
    /// app picked from the ring, if any.
    pub fn ui(
        &mut self,
        ui: &mut Ui,
        center: Pos2,
        size: f32,
        state: CoreState,
        time: f64,
    ) -> Option<AppKind> {
        let dt = ui.input(|i| i.stable_dt).min(0.1);
        let goal = match state {
            CoreState::Thinking => 1.0,
            CoreState::NeedsYou(_) => 0.6,
            CoreState::Idle => 0.2,
        };
        self.energy += (goal - self.energy) * (dt * 2.5).min(1.0);
        let ring_goal = if self.ring_open { 1.0 } else { 0.0 };
        self.ring += (ring_goal - self.ring) * (dt * 9.0).min(1.0);

        let rect = Rect::from_center_size(center, Vec2::splat(size));
        let core_resp = ui.interact(
            Rect::from_center_size(center, Vec2::splat(size * 0.62)),
            Id::new("core"),
            Sense::click(),
        );
        if core_resp.clicked() {
            self.ring_open = !self.ring_open;
        }
        let attention = matches!(state, CoreState::NeedsYou(_));
        paint_core(
            ui.painter(),
            rect,
            time as f32,
            self.energy,
            attention,
            core_resp.hovered(),
        );

        // State label under the core.
        let (title, hint) = match state {
            CoreState::Idle => ("IDLE".to_string(), "Click the core for apps"),
            CoreState::Thinking => ("THINKING".to_string(), "Agents are working"),
            CoreState::NeedsYou(n) => (format!("NEEDS YOU · {n}"), "Check the activity panel"),
        };
        let fade = 1.0 - self.ring;
        let p = ui.painter();
        p.text(
            Pos2::new(center.x, rect.bottom() + 6.0),
            egui::Align2::CENTER_TOP,
            title,
            theme::display(12.0),
            alpha(ICE, fade),
        );
        p.text(
            Pos2::new(center.x, rect.bottom() + 24.0),
            egui::Align2::CENTER_TOP,
            hint.to_uppercase(),
            theme::display(9.0),
            alpha(theme::DIM, fade),
        );

        // The app ring.
        let mut picked = None;
        if self.ring > 0.01 {
            let r = size * 0.62 * self.ring;
            let n = AppKind::ALL.len();
            for (i, app) in AppKind::ALL.iter().enumerate() {
                let a = -PI / 2.0 + i as f32 / n as f32 * TAU;
                let c = center + Vec2::new(a.cos(), a.sin()) * r;
                let rad = 29.0 * (0.4 + 0.6 * self.ring);
                let resp = ui.interact(
                    Rect::from_center_size(c, Vec2::splat(rad * 2.0)),
                    Id::new(("ring", i)),
                    Sense::click(),
                );
                let hovered = resp.hovered();
                let p = ui.painter();
                p.circle(
                    c,
                    rad,
                    if hovered {
                        Color32::from_rgb(12, 42, 64)
                    } else {
                        PANEL
                    },
                    Stroke::new(
                        1.0,
                        alpha(if hovered { CYAN } else { LINE_STRONG }, self.ring),
                    ),
                );
                let icon_rect = Rect::from_center_size(c - Vec2::new(0.0, 6.0), Vec2::splat(18.0));
                icons::paint(p, icon_rect, app.icon(), alpha(CYAN, self.ring));
                p.text(
                    c + Vec2::new(0.0, 11.0),
                    egui::Align2::CENTER_CENTER,
                    app.short().to_uppercase(),
                    theme::display(7.5),
                    alpha(ICE, self.ring),
                );
                if resp.clicked() && self.ring_open {
                    picked = Some(*app);
                }
            }
        }
        if picked.is_some() {
            self.ring_open = false;
        }
        picked
    }
}

fn arc(center: Pos2, r: f32, a0: f32, a1: f32) -> Vec<Pos2> {
    let steps = (((a1 - a0).abs() / TAU) * 96.0).ceil().max(2.0) as usize;
    (0..=steps)
        .map(|i| {
            let a = a0 + (a1 - a0) * i as f32 / steps as f32;
            center + Vec2::new(a.cos(), a.sin()) * r
        })
        .collect()
}

pub fn paint_core(p: &Painter, rect: Rect, t: f32, energy: f32, attention: bool, hovered: bool) {
    let c = rect.center();
    let s = rect.width();
    // The sphere stays cyan. When something needs you, the band and the
    // outer sweeps turn amber.
    let hue = CYAN;
    let accent = if attention { theme::AMBER } else { CYAN };
    let speed = t;

    // Soft glow: stacked translucent discs.
    for i in 0..14 {
        let f = i as f32 / 14.0;
        p.circle_filled(c, s * (0.5 - f * 0.32), alpha(hue, 0.012 + energy * 0.012));
    }

    // The sphere: discs from the dark rim to a bright, off-centre highlight.
    let pulse =
        1.0 + (speed * (1.6 + energy * 3.0)).sin() * 0.03 + if hovered { 0.03 } else { 0.0 };
    let r = s * 0.17 * pulse;
    let rim = Color32::from_rgb(10, 40, 70);
    for i in 0..28 {
        let f = i as f32 / 27.0;
        let radius = r * (1.0 - f * 0.92);
        let offset = Vec2::new(-0.3, -0.35) * r * f;
        let col = if f < 0.55 {
            rim.lerp_to_gamma(hue, f / 0.55)
        } else {
            hue.lerp_to_gamma(ICE, (f - 0.55) / 0.45)
        };
        p.circle_filled(c + offset, radius, col);
    }
    p.circle_stroke(c, r + 4.0, Stroke::new(1.5, alpha(ICE, 0.9)));
    p.circle_stroke(c, r + 7.0, Stroke::new(3.0, alpha(hue, 0.18)));

    // Tick ring.
    let r0 = s * 0.23;
    let rot = speed * 0.08;
    for i in 0..120 {
        let a = i as f32 / 120.0 * TAU + rot;
        let long = i % 10 == 0;
        let len = if long { 9.0 } else { 4.0 };
        let dir = Vec2::new(a.cos(), a.sin());
        p.line_segment(
            [c + dir * r0, c + dir * (r0 + len)],
            Stroke::new(
                if long { 1.5 } else { 1.0 },
                if long {
                    alpha(ICE, 0.8)
                } else {
                    alpha(hue, 0.55)
                },
            ),
        );
    }

    // Segmented band.
    let r1 = s * 0.3;
    for i in 0..10 {
        let a = i as f32 / 10.0 * TAU - speed * (0.2 + energy * 0.6);
        let col = if i % 2 == 1 {
            alpha(DEEP, 0.75)
        } else {
            alpha(accent, 0.85)
        };
        p.add(Shape::line(
            arc(c, r1, a, a + TAU / 10.0 * 0.78),
            Stroke::new(5.0, col),
        ));
    }
    p.circle_stroke(c, s * 0.335, Stroke::new(1.0, alpha(hue, 0.35)));

    // Equaliser bars.
    let rb = s * 0.355;
    let rot = -speed * 0.05;
    for i in 0..72 {
        let a = i as f32 / 72.0 * TAU + rot;
        let fi = i as f32;
        let n = 0.5
            + 0.5
                * (fi * 1.7 + speed * (1.0 + energy * 5.0)).sin()
                * (fi * 0.37 + speed * 0.7).sin();
        let h = 4.0 + n * (8.0 + energy * 22.0);
        let dir = Vec2::new(a.cos(), a.sin());
        let col = if i % 6 == 0 {
            alpha(ICE, 0.85)
        } else {
            alpha(Color32::from_rgb(40, 130, 255), 0.55 + n * 0.4)
        };
        p.line_segment([c + dir * rb, c + dir * (rb + h)], Stroke::new(3.0, col));
    }

    // Outer sweeps.
    let sw = speed * 0.06;
    p.add(Shape::line(
        arc(c, s * 0.46, sw + 0.4, sw + 4.2),
        Stroke::new(2.0, alpha(ICE, 0.55)),
    ));
    p.add(Shape::line(
        arc(c, s * 0.46, sw + 4.5, sw + 5.3),
        Stroke::new(5.0, alpha(accent, 0.8)),
    ));
    p.add(Shape::line(
        arc(c, s * 0.485, -sw, -sw + PI * 1.4),
        Stroke::new(1.0, alpha(accent, 0.45)),
    ));
}
