use chrono::Timelike;
use eframe::egui::{self, Pos2, Rect, ScrollArea, Stroke, Ui};

use crate::services::Shared;
use crate::theme::{self, alpha, AMBER, CYAN, DIM, ICE, RED};

/// Start hour, length in hours, title, note, suggested by an agent.
const EVENTS: [(f32, f32, &str, &str, bool); 5] = [
    (9.5, 0.5, "Standup", "", false),
    (11.0, 1.5, "Deep work", "Agent is holding your calls", true),
    (14.0, 1.0, "1:1 with Ada", "Brief ready in Notes", false),
    (15.0, 1.0, "Focus block", "Suggested by your agent", true),
    (16.5, 1.0, "Gym", "", false),
];

#[derive(Default)]
pub struct CalendarApp;

impl CalendarApp {
    pub fn ui(&mut self, ui: &mut Ui, _shared: &mut Shared) {
        let (start, end) = (8.0_f32, 19.0_f32);
        let px = 40.0;
        ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                let height = (end - start) * px + 24.0;
                let (rect, _) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), height),
                    egui::Sense::hover(),
                );
                let p = ui.painter();
                let left = rect.left() + 56.0;
                let y = |h: f32| rect.top() + 12.0 + (h - start) * px;
                p.line_segment(
                    [Pos2::new(left, y(start)), Pos2::new(left, y(end))],
                    Stroke::new(1.0, theme::LINE),
                );
                for h in start as u32..=end as u32 {
                    let yy = y(h as f32);
                    p.text(
                        Pos2::new(rect.left() + 10.0, yy),
                        egui::Align2::LEFT_CENTER,
                        format!("{h:02}:00"),
                        theme::mono(10.0),
                        DIM,
                    );
                    p.line_segment(
                        [Pos2::new(left, yy), Pos2::new(rect.right() - 8.0, yy)],
                        Stroke::new(1.0, alpha(CYAN, 0.06)),
                    );
                }
                for (s, d, title, note, ai) in EVENTS {
                    let r = Rect::from_min_max(
                        Pos2::new(left + 8.0, y(s) + 1.0),
                        Pos2::new(rect.right() - 8.0, y(s + d) - 2.0),
                    );
                    let accent = if ai { AMBER } else { CYAN };
                    p.rect_filled(r, 0.0, alpha(accent, 0.11));
                    p.rect_filled(
                        Rect::from_min_size(r.min, egui::vec2(2.0, r.height())),
                        0.0,
                        accent,
                    );
                    p.text(
                        r.left_top() + egui::vec2(10.0, 5.0),
                        egui::Align2::LEFT_TOP,
                        title,
                        theme::body(12.0),
                        ICE,
                    );
                    if !note.is_empty() && r.height() > 30.0 {
                        p.text(
                            r.left_top() + egui::vec2(10.0, 21.0),
                            egui::Align2::LEFT_TOP,
                            note,
                            theme::body(10.5),
                            DIM,
                        );
                    }
                }
                let now = chrono::Local::now();
                let h = now.hour() as f32 + now.minute() as f32 / 60.0;
                if h > start && h < end {
                    p.line_segment(
                        [Pos2::new(left, y(h)), Pos2::new(rect.right() - 8.0, y(h))],
                        Stroke::new(1.0, RED),
                    );
                    p.circle_filled(Pos2::new(left, y(h)), 3.0, RED);
                }
            });
    }
}
