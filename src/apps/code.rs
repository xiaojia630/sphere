//! A terminal-first code editor: file tree, highlighted editor, an agent
//! suggestion, and a shell pane underneath.

use std::collections::VecDeque;

use eframe::egui::{self, Color32, Key, RichText, ScrollArea, Stroke, Ui};
use egui_extras::syntax_highlighting::{highlight, CodeTheme};

use crate::services::Shared;
use crate::theme::{self, alpha, AMBER, CYAN, ICE, LINE, PANEL, TEXT};

struct File {
    path: &'static str,
    lang: &'static str,
    text: String,
}

pub struct CodeApp {
    files: Vec<File>,
    open: usize,
    suggestion: bool,
    output: String,
    input: String,
    queued: VecDeque<(f64, String)>,
}

impl Default for CodeApp {
    fn default() -> Self {
        let f = |path, lang, text: &str| File {
            path,
            lang,
            text: text.to_string(),
        };
        Self {
            files: vec![
                f("src/main.rs", "rs", MAIN_RS),
                f("src/runtime.rs", "rs", RUNTIME_RS),
                f("Cargo.toml", "toml", CARGO_TOML),
                f("README.md", "md", README_MD),
            ],
            open: 0,
            suggestion: true,
            output: "~/sphere-os (claude/sphere-hud)\n".into(),
            input: String::new(),
            queued: VecDeque::new(),
        }
    }
}

impl CodeApp {
    pub fn tick(&mut self, shared: &mut Shared) {
        while let Some((at, _)) = self.queued.front() {
            if *at > shared.time {
                break;
            }
            let (_, line) = self.queued.pop_front().unwrap();
            self.output.push_str(&line);
            self.output.push('\n');
        }
    }

    fn stream(&mut self, now: f64, lines: &[&str]) {
        let start = self.queued.back().map(|(t, _)| *t).unwrap_or(now).max(now);
        for (i, l) in lines.iter().enumerate() {
            self.queued
                .push_back((start + 0.26 * (i + 1) as f64, l.to_string()));
        }
    }

    fn run(&mut self, cmd: &str, shared: &mut Shared) {
        self.output.push_str(&format!("❯ {cmd}\n"));
        let now = shared.time;
        match cmd {
            "clear" => self.output.clear(),
            c if c.starts_with("cargo build") => self.stream(
                now,
                &[
                    "   Compiling egui v0.31.1",
                    "   Compiling eframe v0.31.1",
                    "   Compiling sphere-os v0.2.0 (~/sphere-os)",
                    "    Finished `release` profile [optimized] target(s) in 6.84s",
                ],
            ),
            c if c.starts_with("cargo run") => self.stream(
                now,
                &["    Finished `release` profile [optimized] target(s) in 0.31s", "     Running `target/release/sphere-os`", "Sphere core online."],
            ),
            "git status" => self.output.push_str(
                "On branch claude/sphere-hud\nYour branch is up to date with 'origin/claude/sphere-hud'.\n\nnothing to commit, working tree clean\n",
            ),
            "ls" => self.output.push_str("Cargo.lock  Cargo.toml  README.md  assets/  src/  web/\n"),
            "claude" => self.output.push_str("Claude Code runs here in the desktop app.\n"),
            other => {
                let head = other.split_whitespace().next().unwrap_or("");
                self.output.push_str(&format!("{head}: command not found (simulated shell)\n"));
            }
        }
    }

    pub fn ui(&mut self, ui: &mut Ui, shared: &mut Shared) {
        let mono = theme::mono(12.0);

        egui::TopBottomPanel::bottom("code_status")
            .exact_height(22.0)
            .frame(
                egui::Frame::new()
                    .fill(alpha(CYAN, 0.12))
                    .inner_margin(egui::Margin::symmetric(10, 3)),
            )
            .show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    let s = |t: &str| RichText::new(t).font(theme::mono(10.5)).color(ICE);
                    ui.label(s("⎇ claude/sphere-hud"));
                    let lang = match self.files[self.open].lang {
                        "rs" => "Rust",
                        "toml" => "TOML",
                        _ => "Markdown",
                    };
                    ui.label(s(lang));
                    ui.label(s("UTF-8"));
                    ui.label(s("Agent watching"));
                });
            });

        egui::SidePanel::left("code_tree")
            .exact_width(150.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(0, 8))
                    .stroke(Stroke::new(1.0, LINE)),
            )
            .show_inside(ui, |ui| {
                let dir = |ui: &mut Ui, name: &str, depth: f32| {
                    ui.horizontal(|ui| {
                        ui.add_space(10.0 + depth * 12.0);
                        ui.label(
                            RichText::new(format!("▾ {name}"))
                                .font(theme::mono(12.0))
                                .color(theme::DIM),
                        );
                    });
                };
                dir(ui, "sphere-os/", 0.0);
                dir(ui, "src/", 1.0);
                let mut clicked = None;
                for (i, f) in self.files.iter().enumerate() {
                    let depth = if f.path.starts_with("src/") { 2.0 } else { 1.0 };
                    let name = f.path.rsplit('/').next().unwrap();
                    let selected = i == self.open;
                    let (rect, resp) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), 20.0),
                        egui::Sense::click(),
                    );
                    if selected {
                        ui.painter().rect_filled(rect, 0.0, alpha(CYAN, 0.12));
                    } else if resp.hovered() {
                        ui.painter().rect_filled(rect, 0.0, alpha(CYAN, 0.05));
                    }
                    ui.painter().text(
                        rect.left_center() + egui::vec2(10.0 + depth * 12.0, 0.0),
                        egui::Align2::LEFT_CENTER,
                        name,
                        theme::mono(12.0),
                        if selected { ICE } else { TEXT },
                    );
                    if resp.clicked() {
                        clicked = Some(i);
                    }
                }
                if let Some(i) = clicked {
                    self.open = i;
                }
            });

        egui::TopBottomPanel::bottom("code_term")
            .exact_height(140.0)
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(2, 6, 11))
                    .stroke(Stroke::new(1.0, LINE))
                    .inner_margin(egui::Margin::symmetric(10, 6)),
            )
            .show_inside(ui, |ui| {
                egui::TopBottomPanel::bottom("code_term_input")
                    .frame(egui::Frame::new())
                    .show_inside(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("❯").font(mono.clone()).color(CYAN));
                            let edit = ui.add(
                                egui::TextEdit::singleline(&mut self.input)
                                    .font(mono.clone())
                                    .frame(false)
                                    .hint_text("cargo build, git status, ls")
                                    .desired_width(ui.available_width()),
                            );
                            if edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                                let cmd = std::mem::take(&mut self.input);
                                if !cmd.trim().is_empty() {
                                    self.run(cmd.trim(), shared);
                                }
                                edit.request_focus();
                            }
                        });
                    });
                ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        ui.label(RichText::new(&self.output).font(mono.clone()).color(TEXT));
                    });
            });

        if self.suggestion {
            egui::TopBottomPanel::bottom("code_suggest")
                .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(10, 6)).stroke(Stroke::new(1.0, alpha(AMBER, 0.6))))
                .show_inside(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("Agent:").color(AMBER).strong());
                        ui.label(RichText::new("update_hud runs every frame. I can skip it when nothing changed (2 lines).").color(ICE));
                        if theme::button(ui, "Apply", true).clicked() {
                            self.suggestion = false;
                            shared.toast("Applied: 1 file changed, 2 lines");
                        }
                        if theme::button(ui, "Dismiss", false).clicked() {
                            self.suggestion = false;
                        }
                    });
                });
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(PANEL))
            .show_inside(ui, |ui| {
                // Tab strip.
                let name = self.files[self.open]
                    .path
                    .rsplit('/')
                    .next()
                    .unwrap()
                    .to_string();
                ui.horizontal(|ui| {
                    let galley = ui.painter().layout_no_wrap(name, mono.clone(), ICE);
                    let (rect, _) = ui.allocate_exact_size(
                        galley.size() + egui::vec2(24.0, 10.0),
                        egui::Sense::hover(),
                    );
                    ui.painter()
                        .line_segment([rect.left_top(), rect.right_top()], Stroke::new(1.0, CYAN));
                    ui.painter().line_segment(
                        [rect.right_top(), rect.right_bottom()],
                        Stroke::new(1.0, LINE),
                    );
                    ui.painter()
                        .galley(rect.center() - galley.size() / 2.0, galley, ICE);
                });
                let file = &mut self.files[self.open];
                let lang = file.lang;
                let theme = CodeTheme::dark(12.0);
                // The built-in highlighter has no Markdown; TOML's `#` comments
                // give headings a colour of their own.
                let highlight_as = if lang == "md" { "toml" } else { lang };
                let mut layouter = |ui: &Ui, text: &str, wrap: f32| {
                    let mut job = highlight(ui.ctx(), ui.style(), &theme, text, highlight_as);
                    job.wrap.max_width = wrap;
                    ui.fonts(|f| f.layout_job(job))
                };
                ScrollArea::both().auto_shrink([false; 2]).show(ui, |ui| {
                    ui.horizontal_top(|ui| {
                        let lines = file.text.lines().count().max(1);
                        let gutter: String = (1..=lines).map(|n| format!("{n:>3}\n")).collect();
                        ui.label(
                            RichText::new(gutter)
                                .font(mono.clone())
                                .color(Color32::from_rgb(51, 83, 106)),
                        );
                        ui.add(
                            egui::TextEdit::multiline(&mut file.text)
                                .font(mono.clone())
                                .code_editor()
                                .frame(false)
                                .desired_width(f32::INFINITY)
                                .desired_rows(lines)
                                .layouter(&mut layouter),
                        );
                    });
                });
            });
    }
}

const MAIN_RS: &str = r#"//! Sphere OS: an agent-first desktop.

mod apps;
mod runtime;
mod theme;

use eframe::egui;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions::default();
    eframe::run_native(
        "Sphere OS",
        options,
        Box::new(|cc| Ok(Box::new(app::SphereApp::new(cc)))),
    )
}

/// Keeps the HUD quiet when nothing changed.
fn update_hud(ui: &mut egui::Ui, state: &State) {
    let label = format!("{} agents", state.working);
    ui.label(label);
}
"#;

const RUNTIME_RS: &str = r#"/// What an agent task is doing, in herdr's vocabulary.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AgentState {
    Working,
    /// Waiting on a person.
    Blocked,
    Done,
}

pub fn core_state(blocked: usize, working: usize) -> CoreState {
    match (blocked, working) {
        (n, _) if n > 0 => CoreState::NeedsYou(n),
        (_, n) if n > 0 => CoreState::Thinking,
        _ => CoreState::Idle,
    }
}
"#;

const CARGO_TOML: &str = r#"[package]
name = "sphere-os"
version = "0.2.0"
edition = "2021"

[dependencies]
# Windows, panels, text and painting.
eframe = { version = "=0.31.1", default-features = false }
egui_extras = { version = "=0.31.1", default-features = false }
chrono = { version = "0.4", default-features = false }
"#;

const README_MD: &str = r#"# Sphere OS

An agent-first desktop for developers and builders.

# Run it

cargo run --release
"#;
