use eframe::egui::{self, ScrollArea, Sense, Stroke, Ui};

use crate::icons::{self, Icon};
use crate::services::Shared;
use crate::theme::{self, alpha, CYAN, LINE, TEXT};

const FILES: [(Icon, &str); 8] = [
    (Icon::Doc, "Brief-Ada.docx"),
    (Icon::Doc, "Q4-market.pdf"),
    (Icon::Doc, "invoice-0423.pdf"),
    (Icon::Image, "arc-reactor.png"),
    (Icon::Files, "sphere-os"),
    (Icon::Files, "Screenshots"),
    (Icon::Image, "hud-ref.png"),
    (Icon::Doc, "Contract-v3.pdf"),
];

#[derive(Default)]
pub struct FilesApp {
    selected: Option<usize>,
}

impl FilesApp {
    pub fn ui(&mut self, ui: &mut Ui, _shared: &mut Shared) {
        ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                ui.add_space(12.0);
                ui.horizontal_wrapped(|ui| {
                    ui.add_space(12.0);
                    for (i, (icon, name)) in FILES.iter().enumerate() {
                        let (rect, resp) =
                            ui.allocate_exact_size(egui::vec2(96.0, 86.0), Sense::click());
                        let p = ui.painter();
                        if Some(i) == self.selected {
                            p.rect(
                                rect,
                                0.0,
                                alpha(CYAN, 0.1),
                                Stroke::new(1.0, theme::LINE_STRONG),
                                egui::StrokeKind::Inside,
                            );
                        } else if resp.hovered() {
                            p.rect(
                                rect,
                                0.0,
                                alpha(CYAN, 0.05),
                                Stroke::new(1.0, LINE),
                                egui::StrokeKind::Inside,
                            );
                        }
                        icons::paint(
                            p,
                            egui::Rect::from_center_size(
                                rect.center_top() + egui::vec2(0.0, 28.0),
                                egui::vec2(30.0, 30.0),
                            ),
                            *icon,
                            CYAN,
                        );
                        p.text(
                            rect.center_top() + egui::vec2(0.0, 58.0),
                            egui::Align2::CENTER_TOP,
                            *name,
                            theme::body(11.0),
                            TEXT,
                        );
                        if resp.clicked() {
                            self.selected = Some(i);
                        }
                    }
                });
            });
    }
}
