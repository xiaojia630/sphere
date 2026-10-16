use eframe::egui::{self, ScrollArea, Sense, Stroke, Ui};

use crate::services::Shared;
use crate::theme::{self, alpha, CYAN, DIM, ICE, LINE};

pub struct NotesApp {
    notes: Vec<(String, String)>,
    open: usize,
}

impl Default for NotesApp {
    fn default() -> Self {
        let n = |t: &str, b: &str| (t.to_string(), b.to_string());
        Self {
            notes: vec![
                n("Brief: Ada 1:1", "Open questions\n- Pricing: tiered or flat?\n- Launch date: Nov 14 or Dec 1?\n\nAda's last ask: a decision on both today."),
                n("Sphere OS ideas", "The core is the agent's presence.\nA command ring with fixed slots.\nPanels dock at the sides.\nThe stage is where you work.\nBorrow herdr's agent states."),
                n("Groceries", "Plantain\nRice\nPeppers\nCoffee"),
            ],
            open: 0,
        }
    }
}

impl NotesApp {
    pub fn ui(&mut self, ui: &mut Ui, _shared: &mut Shared) {
        egui::SidePanel::left("notes_list")
            .exact_width(170.0)
            .resizable(false)
            .frame(egui::Frame::new().stroke(Stroke::new(1.0, LINE)))
            .show_inside(ui, |ui| {
                for (i, (title, body)) in self.notes.iter().enumerate() {
                    let (rect, resp) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), 46.0),
                        Sense::click(),
                    );
                    let p = ui.painter();
                    if i == self.open {
                        p.rect_filled(rect, 0.0, alpha(CYAN, 0.1));
                    }
                    p.line_segment(
                        [rect.left_bottom(), rect.right_bottom()],
                        Stroke::new(1.0, LINE),
                    );
                    p.text(
                        rect.left_top() + egui::vec2(11.0, 8.0),
                        egui::Align2::LEFT_TOP,
                        title,
                        theme::medium(12.5),
                        ICE,
                    );
                    let first = body.lines().next().unwrap_or("");
                    p.text(
                        rect.left_top() + egui::vec2(11.0, 26.0),
                        egui::Align2::LEFT_TOP,
                        first,
                        theme::body(11.0),
                        DIM,
                    );
                    if resp.clicked() {
                        self.open = i;
                    }
                }
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(14, 12)))
            .show_inside(ui, |ui| {
                ScrollArea::vertical()
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut self.notes[self.open].1)
                                .frame(false)
                                .font(theme::body(13.5))
                                .desired_width(f32::INFINITY)
                                .desired_rows(14),
                        );
                    });
            });
    }
}
