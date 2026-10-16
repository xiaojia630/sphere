use eframe::egui::{self, Ui};

use crate::services::Shared;
use crate::theme;

#[derive(Default)]
pub struct MonitorApp;

impl MonitorApp {
    pub fn ui(&mut self, ui: &mut Ui, shared: &mut Shared) {
        egui::Frame::new()
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                let m = &shared.metrics;
                for (name, series, value) in [
                    ("CPU", &m.cpu, m.cpu_text()),
                    ("Memory", &m.mem, m.mem_text()),
                    ("Network", &m.net, m.net_text()),
                ] {
                    ui.horizontal(|ui| {
                        ui.label(theme::label(name));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(theme::label(value).color(theme::ICE));
                        });
                    });
                    let (rect, _) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), 54.0),
                        egui::Sense::hover(),
                    );
                    let data: Vec<f32> = series.iter().copied().collect();
                    theme::sparkline(ui.painter(), rect, &data, true);
                    ui.add_space(8.0);
                }
                ui.label(theme::label("Simulated readings"));
            });
    }
}
