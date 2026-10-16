use eframe::egui::{self, Color32, RichText, ScrollArea, Stroke, Ui};

use crate::services::{CState, ContainerAction, Shared};
use crate::theme::{self, AMBER, CYAN, DIM, GREEN, ICE, LINE, TEXT};

#[derive(Default)]
pub struct ContainersApp;

impl ContainersApp {
    pub fn ui(&mut self, ui: &mut Ui, shared: &mut Shared) {
        let docker = &mut shared.docker;

        egui::TopBottomPanel::top("ctr_summary")
            .frame(
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(12, 8))
                    .stroke(Stroke::new(1.0, LINE)),
            )
            .show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(theme::label(format!(
                        "{} containers · {} running",
                        docker.containers.len(),
                        docker.running()
                    )));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(theme::label("Docker Engine · local"));
                    });
                });
            });

        egui::TopBottomPanel::bottom("ctr_logs")
            .exact_height(120.0)
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(2, 6, 11))
                    .stroke(Stroke::new(1.0, LINE))
                    .inner_margin(egui::Margin::symmetric(10, 6)),
            )
            .show_inside(ui, |ui| {
                ui.label(theme::label(format!(
                    "Logs · {} · click a name to switch",
                    docker.selected
                )));
                ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        if let Some(lines) = docker.logs.get(docker.selected) {
                            let start = lines.len().saturating_sub(80);
                            for line in lines.iter().skip(start) {
                                ui.label(RichText::new(line).font(theme::mono(11.0)).color(TEXT));
                            }
                        }
                    });
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(10, 6)))
            .show_inside(ui, |ui| {
                let mut action = None;
                let mut select = None;
                ScrollArea::both().auto_shrink([false; 2]).show(ui, |ui| {
                    egui::Grid::new("ctr_grid")
                        .num_columns(6)
                        .spacing([14.0, 8.0])
                        .show(ui, |ui| {
                            for h in ["Name", "Status", "CPU", "Mem", "Ports", ""] {
                                ui.label(theme::label(h));
                            }
                            ui.end_row();
                            for c in &docker.containers {
                                let selected = c.name == docker.selected;
                                let cell = ui
                                    .vertical(|ui| {
                                        ui.set_min_width(112.0);
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(c.name)
                                                    .color(if selected { CYAN } else { ICE })
                                                    .strong(),
                                            )
                                            .extend(),
                                        );
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(c.image).size(11.0).color(DIM),
                                            )
                                            .extend(),
                                        );
                                    })
                                    .response;
                                if ui
                                    .interact(
                                        cell.rect,
                                        ui.id().with(("ctr", c.name)),
                                        egui::Sense::click(),
                                    )
                                    .clicked()
                                {
                                    select = Some(c.name);
                                }
                                let color = match c.state {
                                    CState::Running => GREEN,
                                    CState::Exited => DIM,
                                    CState::Restarting(_) => AMBER,
                                };
                                theme::chip(ui, c.state.label(), color);
                                let running = c.state == CState::Running;
                                let mono = |t: String| {
                                    RichText::new(t).font(theme::mono(12.0)).color(TEXT)
                                };
                                ui.label(mono(if running {
                                    format!("{:.1}%", c.cpu)
                                } else {
                                    "–".into()
                                }));
                                ui.label(mono(if running {
                                    format!("{:.0} MB", c.mem_mb)
                                } else {
                                    "–".into()
                                }));
                                ui.label(mono(c.ports.to_string()));
                                ui.horizontal(|ui| {
                                    let toggle = if c.state == CState::Exited {
                                        ("Start", ContainerAction::Start)
                                    } else {
                                        ("Stop", ContainerAction::Stop)
                                    };
                                    if theme::button(ui, toggle.0, false).clicked() {
                                        action = Some((c.name, toggle.1));
                                    }
                                    if theme::button(ui, "Restart", false).clicked() {
                                        action = Some((c.name, ContainerAction::Restart));
                                    }
                                });
                                ui.end_row();
                            }
                        });
                });
                if let Some(name) = select {
                    docker.selected = name;
                }
                if let Some((name, a)) = action {
                    docker.act(name, a);
                    docker.selected = name;
                }
            });
    }
}
