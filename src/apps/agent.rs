use eframe::egui::{self, Align, Color32, Key, Layout, RichText, ScrollArea, Stroke, Ui};

use crate::runtime::{AgentState, PLAN};
use crate::services::Shared;
use crate::theme::{self, alpha, CYAN, GREEN, ICE, LINE, TEXT};

enum Msg {
    Me(String),
    Ai(String),
    /// A live plan for a task: shows its steps ticking off.
    Plan {
        task: u64,
        reported: bool,
    },
    /// A finished plan from before this session.
    DonePlan(Vec<&'static str>),
}

pub struct AgentApp {
    msgs: Vec<Msg>,
    input: String,
}

impl Default for AgentApp {
    fn default() -> Self {
        Self {
            msgs: vec![
                Msg::Me("Prep me for the 2pm with Ada".into()),
                Msg::DonePlan(vec![
                    "Pulled your last 3 threads with Ada",
                    "Listed the open questions (pricing, launch date)",
                    "Drafted a one-page brief in Notes",
                ]),
            ],
            input: String::new(),
        }
    }
}

impl AgentApp {
    pub fn tick(&mut self, shared: &mut Shared) {
        let mut reports = Vec::new();
        for m in &mut self.msgs {
            if let Msg::Plan { task, reported } = m {
                if !*reported {
                    match shared.runtime.get(*task) {
                        Some(t) if t.state != AgentState::Working => {
                            *reported = true;
                            reports.push(format!(
                                "{} It's in your activity panel to approve.",
                                t.result.clone().unwrap_or_default()
                            ));
                        }
                        None => *reported = true,
                        _ => {}
                    }
                }
            }
        }
        self.msgs.extend(reports.into_iter().map(Msg::Ai));
    }

    pub fn ui(&mut self, ui: &mut Ui, shared: &mut Shared) {
        egui::TopBottomPanel::bottom("agent_composer")
            .frame(
                egui::Frame::new()
                    .inner_margin(egui::Margin::same(8))
                    .stroke(Stroke::new(1.0, LINE)),
            )
            .show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    let send = theme::button(ui, "Send", true);
                    let edit = ui.add(
                        egui::TextEdit::singleline(&mut self.input)
                            .hint_text("Give the agent a task")
                            .desired_width(ui.available_width()),
                    );
                    let submit = send.clicked()
                        || (edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)));
                    if submit && !self.input.trim().is_empty() {
                        let text = std::mem::take(&mut self.input);
                        let task = shared.runtime.start(text.trim());
                        self.msgs.push(Msg::Me(text.trim().to_string()));
                        self.msgs.push(Msg::Plan {
                            task,
                            reported: false,
                        });
                        edit.request_focus();
                    }
                });
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::new().inner_margin(egui::Margin::same(12)))
            .show_inside(ui, |ui| {
                ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        for m in &self.msgs {
                            match m {
                                Msg::Me(text) => bubble(ui, true, |ui| {
                                    ui.label(RichText::new(text).color(ICE));
                                }),
                                Msg::Ai(text) => bubble(ui, false, |ui| {
                                    ui.label(RichText::new(text).color(TEXT));
                                }),
                                Msg::DonePlan(steps) => bubble(ui, false, |ui| {
                                    ui.label("Here's what I did:");
                                    for s in steps {
                                        step(ui, s, StepState::Done);
                                    }
                                }),
                                Msg::Plan { task, .. } => {
                                    let progress =
                                        shared.runtime.get(*task).map(|t| (t.progress, t.state));
                                    bubble(ui, false, |ui| {
                                        ui.label("On it.");
                                        for (i, s) in PLAN.iter().enumerate() {
                                            let state = match progress {
                                                Some((_, st)) if st != AgentState::Working => {
                                                    StepState::Done
                                                }
                                                Some((p, _)) => {
                                                    let current = ((p * 100.0) / 34.0) as usize;
                                                    if i < current {
                                                        StepState::Done
                                                    } else if i == current {
                                                        StepState::Running
                                                    } else {
                                                        StepState::Todo
                                                    }
                                                }
                                                None => StepState::Todo,
                                            };
                                            step(ui, s, state);
                                        }
                                    });
                                }
                            }
                            ui.add_space(4.0);
                        }
                    });
            });
    }
}

fn bubble(ui: &mut Ui, me: bool, add: impl FnOnce(&mut Ui)) {
    let layout = if me {
        Layout::right_to_left(Align::Min)
    } else {
        Layout::left_to_right(Align::Min)
    };
    ui.with_layout(layout, |ui| {
        let max = ui.available_width() * 0.88;
        let frame = if me {
            egui::Frame::new().fill(alpha(CYAN, 0.14))
        } else {
            egui::Frame::new().stroke(Stroke::new(1.0, LINE))
        };
        frame
            .inner_margin(egui::Margin::symmetric(11, 8))
            .show(ui, |ui| {
                ui.set_max_width(max);
                ui.vertical(add);
            });
    });
}

#[derive(PartialEq)]
enum StepState {
    Done,
    Running,
    Todo,
}

fn step(ui: &mut Ui, text: &str, state: StepState) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
        let t = ui.input(|i| i.time) as f32;
        match state {
            StepState::Done => ui.painter().rect_filled(rect, 0.0, GREEN),
            StepState::Running => {
                let a = 0.5 + 0.5 * (t * 6.0).sin();
                ui.painter().rect_stroke(
                    rect,
                    0.0,
                    Stroke::new(1.0, alpha(CYAN, 0.4 + 0.6 * a)),
                    egui::StrokeKind::Inside,
                )
            }
            StepState::Todo => ui.painter().rect_stroke(
                rect,
                0.0,
                Stroke::new(1.0, theme::DIM),
                egui::StrokeKind::Inside,
            ),
        };
        let color = if state == StepState::Todo {
            theme::DIM
        } else {
            Color32::from_rgb(200, 225, 240)
        };
        ui.label(RichText::new(text).size(12.0).color(color));
    });
}
