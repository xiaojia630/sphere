//! The apps that open as windows on the stage.

mod agent;
mod calendar;
mod code;
mod containers;
mod files;
mod mail;
mod monitor;
mod music;
mod notes;
mod terminal;
mod web;

use eframe::egui::{self, Ui, Vec2};

use crate::icons::Icon;
use crate::services::Shared;

pub use music::now_playing;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum AppKind {
    Agent,
    Code,
    Containers,
    Music,
    Mail,
    Calendar,
    Notes,
    Files,
    Web,
    Terminal,
    Monitor,
}

impl AppKind {
    pub const ALL: [AppKind; 11] = [
        AppKind::Agent,
        AppKind::Code,
        AppKind::Containers,
        AppKind::Music,
        AppKind::Mail,
        AppKind::Calendar,
        AppKind::Notes,
        AppKind::Files,
        AppKind::Web,
        AppKind::Terminal,
        AppKind::Monitor,
    ];

    pub fn title(self) -> &'static str {
        match self {
            AppKind::Agent => "Agent",
            AppKind::Code => "Code",
            AppKind::Containers => "Containers",
            AppKind::Music => "Music",
            AppKind::Mail => "Mail",
            AppKind::Calendar => "Calendar",
            AppKind::Notes => "Notes",
            AppKind::Files => "Files",
            AppKind::Web => "Web",
            AppKind::Terminal => "Terminal",
            AppKind::Monitor => "Monitor",
        }
    }

    /// Name used on the core's app ring, where space is tight.
    pub fn short(self) -> &'static str {
        match self {
            AppKind::Containers => "Docker",
            AppKind::Terminal => "Shell",
            other => other.title(),
        }
    }

    pub fn icon(self) -> Icon {
        match self {
            AppKind::Agent => Icon::Agent,
            AppKind::Code => Icon::Code,
            AppKind::Containers => Icon::Containers,
            AppKind::Music => Icon::Music,
            AppKind::Mail => Icon::Mail,
            AppKind::Calendar => Icon::Calendar,
            AppKind::Notes => Icon::Notes,
            AppKind::Files => Icon::Files,
            AppKind::Web => Icon::Web,
            AppKind::Terminal => Icon::Terminal,
            AppKind::Monitor => Icon::Monitor,
        }
    }

    pub fn default_size(self) -> Vec2 {
        let (w, h) = match self {
            AppKind::Agent => (340.0, 440.0),
            AppKind::Code => (620.0, 460.0),
            AppKind::Containers => (580.0, 380.0),
            AppKind::Music => (520.0, 400.0),
            AppKind::Mail => (560.0, 380.0),
            AppKind::Calendar => (330.0, 480.0),
            AppKind::Notes => (480.0, 340.0),
            AppKind::Files => (440.0, 300.0),
            AppKind::Web => (480.0, 380.0),
            AppKind::Terminal => (480.0, 300.0),
            AppKind::Monitor => (340.0, 320.0),
        };
        egui::vec2(w, h)
    }

    pub fn parse(name: &str) -> Option<Self> {
        let name = name.to_lowercase();
        AppKind::ALL.into_iter().find(|a| {
            a.title().to_lowercase() == name
                || a.short().to_lowercase() == name
                || (name == "docker" && *a == AppKind::Containers)
                || (name == "spotify" && *a == AppKind::Music)
                || (name == "shell" && *a == AppKind::Terminal)
        })
    }
}

/// Every app's state. Each app keeps its state while its window is closed.
#[derive(Default)]
pub struct Apps {
    agent: agent::AgentApp,
    code: code::CodeApp,
    containers: containers::ContainersApp,
    music: music::MusicApp,
    mail: mail::MailApp,
    calendar: calendar::CalendarApp,
    notes: notes::NotesApp,
    files: files::FilesApp,
    web: web::WebApp,
    terminal: terminal::TerminalApp,
    monitor: monitor::MonitorApp,
}

impl Apps {
    pub fn ui(&mut self, kind: AppKind, ui: &mut Ui, shared: &mut Shared) {
        match kind {
            AppKind::Agent => self.agent.ui(ui, shared),
            AppKind::Code => self.code.ui(ui, shared),
            AppKind::Containers => self.containers.ui(ui, shared),
            AppKind::Music => self.music.ui(ui, shared),
            AppKind::Mail => self.mail.ui(ui, shared),
            AppKind::Calendar => self.calendar.ui(ui, shared),
            AppKind::Notes => self.notes.ui(ui, shared),
            AppKind::Files => self.files.ui(ui, shared),
            AppKind::Web => self.web.ui(ui, shared),
            AppKind::Terminal => self.terminal.ui(ui, shared),
            AppKind::Monitor => self.monitor.ui(ui, shared),
        }
    }

    /// Lets the agent window follow tasks started from elsewhere.
    pub fn tick(&mut self, shared: &mut Shared) {
        self.agent.tick(shared);
        self.code.tick(shared);
    }
}
