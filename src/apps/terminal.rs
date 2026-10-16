use eframe::egui::{self, Color32, Key, RichText, ScrollArea, Ui};

use crate::apps::AppKind;
use crate::services::{ContainerAction, Shared};
use crate::theme::{self, CYAN, TEXT};

pub struct TerminalApp {
    output: String,
    input: String,
}

impl Default for TerminalApp {
    fn default() -> Self {
        Self {
            output: "Sphere OS shell 0.2. Type help.\n".into(),
            input: String::new(),
        }
    }
}

impl TerminalApp {
    fn run(&mut self, cmd: &str, shared: &mut Shared) {
        let out = &mut self.output;
        out.push_str(&format!("sphere ❯ {cmd}\n"));
        let mut words = cmd.split_whitespace();
        let head = words.next().unwrap_or("");
        let rest: Vec<&str> = words.collect();
        let arg = rest.join(" ");
        match head {
            "help" => out.push_str("help, ls, agents, docker ps, docker <start|stop|restart> <name>, open <app>, run <task>, date, whoami, clear\n"),
            "ls" => out.push_str("Brief-Ada.docx  Q4-market.pdf  invoice-0423.pdf  sphere-os/  Screenshots/\n"),
            "agents" => {
                for t in &shared.runtime.tasks {
                    out.push_str(&format!("{:>3}  {:<10} {:>3.0}%  {}\n", t.id, t.state.label(), t.progress * 100.0, t.text));
                }
            }
            "docker" => match rest.first().copied() {
                Some("ps") | None => {
                    out.push_str(&format!("{:<10} {:<16} {:<11} {}\n", "NAME", "IMAGE", "STATUS", "PORTS"));
                    for c in &shared.docker.containers {
                        out.push_str(&format!("{:<10} {:<16} {:<11} {}\n", c.name, c.image, c.state.label(), c.ports));
                    }
                }
                Some(verb) => match (ContainerAction::parse(verb), rest.get(1)) {
                    (Some(action), Some(name)) if shared.docker.act(name, action) => out.push_str(&format!("{name}\n")),
                    _ => out.push_str("usage: docker <start|stop|restart> <name>\n"),
                },
            },
            "open" => match AppKind::parse(&arg) {
                Some(app) => shared.open(app),
                None => out.push_str(&format!("no such app: {arg}\n")),
            },
            "run" if !arg.is_empty() => {
                shared.runtime.start(arg.clone());
                out.push_str(&format!("started task: {arg}\n"));
            }
            "date" => out.push_str(&format!("{}\n", chrono::Local::now().format("%a %d %b %Y %H:%M:%S"))),
            "whoami" => out.push_str("gabriel\n"),
            "clear" => out.clear(),
            "" => {}
            other => out.push_str(&format!("{other}: command not found\n")),
        }
    }

    pub fn ui(&mut self, ui: &mut Ui, shared: &mut Shared) {
        let mono = theme::mono(12.5);
        egui::TopBottomPanel::bottom("term_input")
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(2, 6, 11))
                    .inner_margin(egui::Margin::symmetric(10, 8)),
            )
            .show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("sphere ❯").font(mono.clone()).color(CYAN));
                    let edit = ui.add(
                        egui::TextEdit::singleline(&mut self.input)
                            .font(mono.clone())
                            .frame(false)
                            .desired_width(ui.available_width()),
                    );
                    if edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        let cmd = std::mem::take(&mut self.input);
                        self.run(cmd.trim(), shared);
                        edit.request_focus();
                    }
                });
            });
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(2, 6, 11))
                    .inner_margin(egui::Margin::symmetric(12, 10)),
            )
            .show_inside(ui, |ui| {
                ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        ui.label(RichText::new(&self.output).font(mono).color(TEXT));
                    });
            });
    }
}
