use eframe::egui::{self, RichText, ScrollArea, Stroke, Ui};

use crate::icons::{self, Icon};
use crate::services::Shared;
use crate::theme::{self, CYAN, DIM, ICE, LINE, TEXT};

#[derive(Default)]
pub struct WebApp;

impl WebApp {
    pub fn ui(&mut self, ui: &mut Ui, _shared: &mut Shared) {
        egui::TopBottomPanel::top("web_addr")
            .frame(
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(10, 8))
                    .stroke(Stroke::new(1.0, LINE)),
            )
            .show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    let (r, _) =
                        ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
                    icons::paint(ui.painter(), r, Icon::Web, DIM);
                    egui::Frame::new()
                        .stroke(Stroke::new(1.0, LINE))
                        .inner_margin(egui::Margin::symmetric(8, 4))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(
                                RichText::new("sphere://research/q4-market")
                                    .font(theme::mono(12.0))
                                    .color(ICE),
                            );
                        });
                });
            });
        egui::CentralPanel::default().frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(16, 14))).show_inside(ui, |ui| {
            ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
                ui.set_max_width(520.0);
                ui.label(theme::label("Agent research · 4 sources"));
                ui.label(RichText::new("Q4 market: what changed this quarter").font(theme::medium(17.0)).color(ICE));
                ui.add_space(6.0);
                for b in [
                    "Agent tools moved from pilots to budgets at mid-size firms.",
                    "Voice interfaces are growing fastest in field work and logistics.",
                    "Buyers ask about approvals and audit trails first.",
                ] {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("▸").color(CYAN));
                        ui.label(RichText::new(b).color(TEXT));
                    });
                }
                ui.add_space(8.0);
                ui.label(RichText::new("This summary is a placeholder. An agent fills this page from live sources once the agent link is online.").color(DIM));
            });
        });
    }
}
