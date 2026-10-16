//! State shared between apps, widgets and the command bar: the music
//! player, containers, system metrics and toasts.
//!
//! Everything here is simulated in this build. Each service keeps the shape a
//! real backend needs (Spotify, the Docker Engine API, system counters) so the
//! native versions can slot in behind the same methods.

use std::collections::{HashMap, VecDeque};

use crate::apps::AppKind;
use crate::runtime::{Rng, Runtime};

pub struct Shared {
    pub runtime: Runtime,
    pub player: Player,
    pub docker: Docker,
    pub metrics: Metrics,
    pub toasts: Vec<(String, f64)>,
    pub open_requests: Vec<AppKind>,
    pub time: f64,
    pub rng: Rng,
    second: f32,
}

impl Default for Shared {
    fn default() -> Self {
        let mut rng = Rng(0x2545f4914f6cdd1d);
        let mut metrics = Metrics::default();
        let mut docker = Docker::default();
        for _ in 0..40 {
            metrics.step(&mut rng, false);
        }
        for _ in 0..8 {
            docker.step(&mut rng);
        }
        Self {
            runtime: Runtime::default(),
            player: Player::default(),
            docker,
            metrics,
            toasts: Vec::new(),
            open_requests: Vec::new(),
            time: 0.0,
            rng,
            second: 0.0,
        }
    }
}

impl Shared {
    pub fn toast(&mut self, text: impl Into<String>) {
        self.toasts.push((text.into(), self.time));
    }

    pub fn open(&mut self, app: AppKind) {
        self.open_requests.push(app);
    }

    pub fn tick(&mut self, dt: f32) {
        self.time += dt as f64;
        for text in self.runtime.tick(dt) {
            self.toast(format!("Ready for review: {text}"));
        }
        self.player.tick(dt);
        self.docker.tick(dt);
        self.second += dt;
        if self.second >= 1.0 {
            self.second -= 1.0;
            let busy = self.runtime.working() > 0;
            self.metrics.step(&mut self.rng, busy);
            self.docker.step(&mut self.rng);
        }
        let now = self.time;
        self.toasts.retain(|(_, born)| now - born < 3.8);
    }
}

// ---------------------------------------------------------------------------
// Music
// ---------------------------------------------------------------------------

pub struct Track {
    pub title: &'static str,
    pub artist: &'static str,
    pub secs: f32,
}

pub const TRACKS: [Track; 8] = [
    Track {
        title: "Back in Black",
        artist: "AC/DC",
        secs: 255.0,
    },
    Track {
        title: "Derezzed",
        artist: "Daft Punk",
        secs: 104.0,
    },
    Track {
        title: "Last Last",
        artist: "Burna Boy",
        secs: 172.0,
    },
    Track {
        title: "Time",
        artist: "Hans Zimmer",
        secs: 275.0,
    },
    Track {
        title: "Free Mind",
        artist: "Tems",
        secs: 246.0,
    },
    Track {
        title: "Computer Love",
        artist: "Kraftwerk",
        secs: 435.0,
    },
    Track {
        title: "Midnight City",
        artist: "M83",
        secs: 243.0,
    },
    Track {
        title: "Essence",
        artist: "Wizkid",
        secs: 249.0,
    },
];

pub struct Player {
    pub index: usize,
    pub position: f32,
    pub playing: bool,
}

impl Default for Player {
    fn default() -> Self {
        Self {
            index: 0,
            position: 64.0,
            playing: true,
        }
    }
}

impl Player {
    pub fn track(&self) -> &'static Track {
        &TRACKS[self.index]
    }

    pub fn tick(&mut self, dt: f32) {
        if self.playing {
            self.position += dt;
            if self.position >= self.track().secs {
                self.next();
            }
        }
    }

    pub fn play_at(&mut self, index: usize) {
        self.index = index % TRACKS.len();
        self.position = 0.0;
        self.playing = true;
    }

    pub fn next(&mut self) {
        self.play_at(self.index + 1);
    }

    pub fn prev(&mut self) {
        if self.position > 3.0 {
            self.position = 0.0;
        } else {
            self.play_at(self.index + TRACKS.len() - 1);
        }
    }

    pub fn fraction(&self) -> f32 {
        self.position / self.track().secs
    }
}

pub fn clock(secs: f32) -> String {
    let s = secs.max(0.0) as u32;
    format!("{}:{:02}", s / 60, s % 60)
}

// ---------------------------------------------------------------------------
// Containers
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum CState {
    Running,
    Exited,
    Restarting(f32),
}

impl CState {
    pub fn label(self) -> &'static str {
        match self {
            CState::Running => "Running",
            CState::Exited => "Exited",
            CState::Restarting(_) => "Restarting",
        }
    }
}

pub struct Container {
    pub name: &'static str,
    pub image: &'static str,
    pub state: CState,
    pub cpu: f32,
    pub mem_mb: f32,
    pub ports: &'static str,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ContainerAction {
    Start,
    Stop,
    Restart,
}

impl ContainerAction {
    pub fn parse(word: &str) -> Option<Self> {
        match word {
            "start" => Some(Self::Start),
            "stop" => Some(Self::Stop),
            "restart" => Some(Self::Restart),
            _ => None,
        }
    }
}

pub struct Docker {
    pub containers: Vec<Container>,
    pub logs: HashMap<&'static str, VecDeque<String>>,
    pub selected: &'static str,
}

impl Default for Docker {
    fn default() -> Self {
        let c = |name, image, running, cpu, mem_mb, ports| Container {
            name,
            image,
            state: if running {
                CState::Running
            } else {
                CState::Exited
            },
            cpu,
            mem_mb,
            ports,
        };
        let containers = vec![
            c("api", "sphere/api:dev", true, 12.0, 182.0, "8080→8080"),
            c("web", "node:20-alpine", true, 6.0, 240.0, "3000→3000"),
            c("postgres", "postgres:16", true, 3.0, 96.0, "5432→5432"),
            c("redis", "redis:7", true, 1.0, 14.0, "6379→6379"),
            c("minio", "minio/minio", false, 0.0, 0.0, "9000→9000"),
        ];
        let mut logs: HashMap<&'static str, VecDeque<String>> = containers
            .iter()
            .map(|c| (c.name, VecDeque::new()))
            .collect();
        logs.get_mut("minio")
            .unwrap()
            .push_back("Exiting on signal: terminated".into());
        Self {
            containers,
            logs,
            selected: "api",
        }
    }
}

impl Docker {
    pub fn running(&self) -> usize {
        self.containers
            .iter()
            .filter(|c| c.state == CState::Running)
            .count()
    }

    fn log(&mut self, name: &'static str, line: String) {
        let stamp = chrono::Local::now().format("%H:%M:%S");
        let buf = self.logs.entry(name).or_default();
        buf.push_back(format!("{stamp}  {line}"));
        while buf.len() > 300 {
            buf.pop_front();
        }
    }

    pub fn act(&mut self, name: &str, action: ContainerAction) -> bool {
        let Some(c) = self.containers.iter_mut().find(|c| c.name == name) else {
            return false;
        };
        let name = c.name;
        let line = match action {
            ContainerAction::Stop => {
                c.state = CState::Exited;
                c.cpu = 0.0;
                c.mem_mb = 0.0;
                "received SIGTERM, shutting down"
            }
            ContainerAction::Start => {
                c.state = CState::Running;
                c.mem_mb = 40.0;
                "starting"
            }
            ContainerAction::Restart => {
                c.state = CState::Restarting(1.5);
                "restarting"
            }
        };
        self.log(name, line.into());
        true
    }

    fn tick(&mut self, dt: f32) {
        let mut ready = Vec::new();
        for c in &mut self.containers {
            if let CState::Restarting(left) = c.state {
                let left = left - dt;
                if left <= 0.0 {
                    c.state = CState::Running;
                    c.mem_mb = 60.0;
                    ready.push(c.name);
                } else {
                    c.state = CState::Restarting(left);
                }
            }
        }
        for name in ready {
            self.log(name, "ready".into());
        }
    }

    fn step(&mut self, rng: &mut Rng) {
        let mut lines = Vec::new();
        for c in self
            .containers
            .iter_mut()
            .filter(|c| c.state == CState::Running)
        {
            c.cpu = (c.cpu + rng.signed() * 2.0).clamp(0.2, 95.0);
            c.mem_mb = (c.mem_mb + rng.signed() * 3.0).max(8.0);
            if rng.next() < 0.6 {
                lines.push((c.name, sample_log(c.name, rng)));
            }
        }
        for (name, line) in lines {
            self.log(name, line);
        }
    }
}

fn sample_log(name: &str, rng: &mut Rng) -> String {
    match name {
        "api" => format!(
            "{} /v1/{} 200 {}ms",
            rng.pick(&["GET", "POST", "GET", "PATCH"]),
            rng.pick(&["tasks", "agents", "spaces", "tasks/42"]),
            4 + (rng.next() * 40.0) as u32
        ),
        "web" => rng
            .pick(&[
                "compiled client in 210ms",
                "GET / 200",
                "GET /_next/static/chunk.js 200",
                "hmr update",
            ])
            .to_string(),
        "postgres" => format!(
            "LOG:  {}",
            rng.pick(&[
                "checkpoint starting: time",
                "checkpoint complete: wrote 12 buffers",
                "connection authorized: user=sphere"
            ])
        ),
        "redis" => format!(
            "* {}",
            rng.pick(&[
                "1 changes in 3600 seconds. Saving...",
                "Background saving terminated with success",
                "DB saved on disk"
            ])
        ),
        _ => "API: http://0.0.0.0:9000".into(),
    }
}

// ---------------------------------------------------------------------------
// System metrics
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct Metrics {
    pub cpu: VecDeque<f32>,
    pub mem: VecDeque<f32>,
    pub net: VecDeque<f32>,
    walk: [f32; 3],
}

impl Metrics {
    fn step(&mut self, rng: &mut Rng, busy: bool) {
        if self.walk == [0.0; 3] {
            self.walk = [30.0, 54.0, 20.0];
        }
        let target = if busy { 38.0 } else { 20.0 };
        self.walk[0] =
            (self.walk[0] + rng.signed() * 7.0 + (target - self.walk[0]) * 0.1).clamp(4.0, 96.0);
        self.walk[1] = (self.walk[1] + rng.signed() * 1.5).clamp(30.0, 90.0);
        self.walk[2] = (self.walk[2] + rng.signed() * 10.0).clamp(1.0, 99.0);
        for (series, v) in [
            (&mut self.cpu, self.walk[0]),
            (&mut self.mem, self.walk[1]),
            (&mut self.net, self.walk[2]),
        ] {
            series.push_back(v);
            if series.len() > 60 {
                series.pop_front();
            }
        }
    }

    pub fn cpu_text(&self) -> String {
        format!("{:.0}%", self.cpu.back().copied().unwrap_or(0.0))
    }

    pub fn mem_text(&self) -> String {
        format!(
            "{:.1}G",
            self.mem.back().copied().unwrap_or(0.0) / 100.0 * 16.0
        )
    }

    pub fn net_text(&self) -> String {
        format!("{:.0}M", self.net.back().copied().unwrap_or(0.0))
    }
}
