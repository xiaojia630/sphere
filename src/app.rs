//! The desktop shell: the top bar, the dock, the side panels, the stage with
//! the core, app windows, the command bar and toasts.

use chrono::Timelike;
use eframe::egui::{
    self, Align, Align2, Color32, Context, CursorIcon, Id, Key, LayerId, Layout, Margin, Order,
    Painter, Pos2, Rect, RichText, ScrollArea, Sense, Stroke, StrokeKind, Ui, Vec2,
};

use crate::apps::{self, AppKind, Apps};
use crate::core_view::CoreView;
use crate::icons::{self, Icon};
use crate::runtime::{AgentState, PLAN};
use crate::services::{ContainerAction, Shared};
use crate::theme::{
    self, alpha, AMBER, BAR, CYAN, DIM, GREEN, ICE, LINE, LINE_STRONG, PANEL, PANEL_HI, RED, TEXT,
    VOID,
};

const SUGGESTIONS: [&str; 4] = [
    "Summarise my inbox",
    "Plan my afternoon",
    "open docker",
    "restart api",
];

/// Today's events for the side widget: start hour, label, title.
const TODAY: [(f32, &str, &str); 4] = [
    (9.5, "09:30", "Standup"),
    (14.0, "14:00", "1:1 with Ada"),
    (15.0, "15:00", "Focus block"),
    (16.5, "16:30", "Gym"),
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Workspace {
    Work,
    Life,
    Build,
}

impl Workspace {
    const ALL: [Workspace; 3] = [Workspace::Work, Workspace::Life, Workspace::Build];

    fn title(self) -> &'static str {
        match self {
            Workspace::Work => "Work",
            Workspace::Life => "Life",
            Workspace::Build => "Build",
        }
    }
}

/// An open app window.
struct Win {
    kind: AppKind,
    ws: Workspace,
    /// Top-left corner, relative to the stage.
    offset: Vec2,
    minimized: bool,
    maximized: bool,
}

impl Win {
    /// egui remembers a window's size by id, so the maximized view gets its
    /// own id and the normal size survives a maximize.
    fn id(&self) -> Id {
        Id::new(("window", self.kind, self.maximized))
    }
}

enum Chrome {
    Minimize,
    Maximize,
    Close,
}

pub struct SphereApp {
    shared: Shared,
    apps: Apps,
    core: CoreView,
    windows: Vec<Win>,
    ws: Workspace,
    focused: Option<AppKind>,
    command: String,
    stage: Rect,
    laid_out: bool,
}

impl SphereApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install(&cc.egui_ctx);
        Self {
            shared: Shared::default(),
            apps: Apps::default(),
            core: CoreView::default(),
            windows: Vec::new(),
            ws: Workspace::Work,
            focused: None,
            command: String::new(),
            stage: Rect::NOTHING,
            laid_out: false,
        }
    }

    /// The first layout, once the stage size is known: a few windows in each
    /// workspace.
    fn lay_out(&mut self) {
        let s = self.stage.size();
        let right =
            |kind: AppKind, y: f32| egui::vec2((s.x - kind.default_size().x - 16.0).max(16.0), y);
        let place = |kind, ws, offset| Win {
            kind,
            ws,
            offset,
            minimized: false,
            maximized: false,
        };
        self.windows = vec![
            place(AppKind::Agent, Workspace::Work, right(AppKind::Agent, 16.0)),
            place(AppKind::Calendar, Workspace::Life, egui::vec2(16.0, 16.0)),
            place(AppKind::Music, Workspace::Life, right(AppKind::Music, 60.0)),
            place(AppKind::Code, Workspace::Build, egui::vec2(16.0, 16.0)),
            place(
                AppKind::Containers,
                Workspace::Build,
                right(AppKind::Containers, (s.y - 380.0 - 104.0).max(60.0)),
            ),
        ];
        self.focused = Some(AppKind::Agent);
    }

    fn here(&self, kind: AppKind) -> bool {
        self.windows
            .iter()
            .any(|w| w.kind == kind && w.ws == self.ws && !w.minimized)
    }

    /// Opens `kind` in the current workspace, or brings it forward if it is
    /// already open.
    fn open(&mut self, ctx: &Context, kind: AppKind) {
        let ws = self.ws;
        let count = self
            .windows
            .iter()
            .filter(|w| w.ws == ws && !w.minimized)
            .count() as f32;
        match self.windows.iter_mut().find(|w| w.kind == kind) {
            Some(w) => {
                w.ws = ws;
                w.minimized = false;
            }
            None => self.windows.push(Win {
                kind,
                ws,
                offset: egui::vec2(48.0 + 28.0 * count, 28.0 + 24.0 * count),
                minimized: false,
                maximized: false,
            }),
        }
        if let Some(w) = self.windows.iter().find(|w| w.kind == kind) {
            ctx.move_to_top(LayerId::new(Order::Middle, w.id()));
        }
        self.focused = Some(kind);
        self.core.ring_open = false;
    }

    /// Runs a line from the command bar: app and music commands run directly,
    /// container commands go to Docker, anything else becomes an agent task.
    fn run_command(&mut self, ctx: &Context, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let lower = text.to_lowercase();
        let words: Vec<&str> = lower.split_whitespace().collect();
        let head = words[0];
        let arg = words[1..].join(" ");

        if head == "open" {
            match AppKind::parse(&arg) {
                Some(app) => self.open(ctx, app),
                None => self.shared.toast(format!("There is no app called {arg}")),
            }
            return;
        }
        if let Some(app) = AppKind::parse(&lower) {
            self.open(ctx, app);
            return;
        }
        let player = &mut self.shared.player;
        let music = match lower.as_str() {
            "play" | "resume" => {
                player.playing = true;
                true
            }
            "pause" => {
                player.playing = false;
                true
            }
            "next" | "skip" => {
                player.next();
                true
            }
            "prev" | "previous" | "back" => {
                player.prev();
                true
            }
            _ => false,
        };
        if music {
            let t = self.shared.player.track();
            let verb = if self.shared.player.playing {
                "Playing"
            } else {
                "Paused"
            };
            self.shared
                .toast(format!("{verb}: {} by {}", t.title, t.artist));
            return;
        }
        if let Some(action) = ContainerAction::parse(head) {
            if self.shared.docker.act(&arg, action) {
                let verb = match action {
                    ContainerAction::Start => "Starting",
                    ContainerAction::Stop => "Stopping",
                    ContainerAction::Restart => "Restarting",
                };
                self.shared.toast(format!("{verb} container {arg}"));
                return;
            }
        }
        self.shared.runtime.start(text);
        self.shared.toast(format!("On it: {text}"));
    }

    fn top_bar(&mut self, ctx: &Context) {
        let now = chrono::Local::now();
        let mut voice = false;
        egui::TopBottomPanel::top("top_bar")
            .exact_height(44.0)
            .frame(
                egui::Frame::new()
                    .fill(BAR)
                    .inner_margin(Margin::symmetric(16, 0)),
            )
            .show(ctx, |ui| {
                let full = ui.max_rect();
                ui.painter().text(
                    full.center(),
                    Align2::CENTER_CENTER,
                    now.format("%a %d %b   %H:%M:%S").to_string().to_uppercase(),
                    theme::display(12.0),
                    ICE,
                );
                ui.horizontal_centered(|ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                    ui.painter()
                        .circle_stroke(r.center(), 7.5, Stroke::new(1.2, CYAN));
                    ui.painter().circle_filled(r.center(), 3.2, ICE);
                    ui.label(
                        RichText::new("SPHERE·OS")
                            .font(theme::display(13.0))
                            .extra_letter_spacing(3.0)
                            .color(ICE),
                    );
                    ui.add_space(14.0);
                    ui.spacing_mut().item_spacing.x = 2.0;
                    for ws in Workspace::ALL {
                        let n = self
                            .windows
                            .iter()
                            .filter(|w| w.ws == ws && !w.minimized)
                            .count();
                        if workspace_tab(ui, ws.title(), n, ws == self.ws).clicked() {
                            self.ws = ws;
                            self.core.ring_open = false;
                        }
                    }
                    ui.spacing_mut().item_spacing.x = 10.0;
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let (r, mic) = ui.allocate_exact_size(Vec2::splat(26.0), Sense::click());
                        let hot = mic.hovered();
                        ui.painter().circle_stroke(
                            r.center(),
                            12.0,
                            Stroke::new(1.0, if hot { CYAN } else { LINE_STRONG }),
                        );
                        icons::paint(
                            ui.painter(),
                            Rect::from_center_size(r.center(), Vec2::splat(14.0)),
                            Icon::Mic,
                            if hot { ICE } else { CYAN },
                        );
                        voice = mic
                            .on_hover_text("Voice")
                            .on_hover_cursor(CursorIcon::PointingHand)
                            .clicked();

                        let rt = &self.shared.runtime;
                        match (rt.blocked(), rt.working()) {
                            (b, _) if b > 0 => theme::chip(ui, &format!("{b} needs you"), AMBER),
                            (_, w) if w > 0 => theme::chip(ui, &format!("{w} working"), CYAN),
                            _ => theme::chip(ui, "Agents idle", DIM),
                        };
                        let m = &self.shared.metrics;
                        ui.label(theme::label(format!("Net {}", m.net_text())));
                        ui.label(theme::label(format!("Mem {}", m.mem_text())));
                        ui.label(theme::label(format!("CPU {}", m.cpu_text())));
                    });
                });
            });
        if voice {
            self.shared
                .toast("Voice input arrives in step 3. Type in the command bar for now.");
        }
    }

    fn dock(&mut self, ctx: &Context) {
        let mut clicked = None;
        let mut home = false;
        egui::SidePanel::left("dock")
            .exact_width(60.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(BAR)
                    .inner_margin(Margin::symmetric(8, 10)),
            )
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                if dock_button(ui, Icon::Home, "Home: show the core", false, false, false).clicked()
                {
                    home = true;
                }
                ui.add_space(3.0);
                let (r, _) =
                    ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), Sense::hover());
                ui.painter().rect_filled(r, 0.0, LINE);
                ui.add_space(3.0);
                for kind in AppKind::ALL {
                    let win = self.windows.iter().find(|w| w.kind == kind);
                    let here = win.is_some_and(|w| w.ws == self.ws && !w.minimized);
                    let focused = here && self.focused == Some(kind);
                    if dock_button(ui, kind.icon(), kind.title(), win.is_some(), here, focused)
                        .clicked()
                    {
                        clicked = Some(kind);
                    }
                }
            });
        if home {
            let ws = self.ws;
            for w in self.windows.iter_mut().filter(|w| w.ws == ws) {
                w.minimized = true;
            }
            self.focused = None;
        }
        if let Some(kind) = clicked {
            if self.here(kind) && self.focused == Some(kind) {
                if let Some(w) = self.windows.iter_mut().find(|w| w.kind == kind) {
                    w.minimized = true;
                }
                self.focused = None;
            } else {
                self.open(ctx, kind);
            }
        }
    }

    /// The left column of live widgets.
    fn widgets(&mut self, ctx: &Context) {
        let mut open = None;
        egui::SidePanel::left("widgets")
            .exact_width(252.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(VOID).inner_margin(Margin::same(12)))
            .show(ctx, |ui| {
                ScrollArea::vertical()
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 10.0;
                        let s = &mut self.shared;
                        theme::widget_box(ui, |ui| {
                            if header(ui, "Agents live", "Open") {
                                open = Some(AppKind::Agent);
                            }
                            for t in s.runtime.tasks.iter().take(4) {
                                ui.horizontal(|ui| {
                                    let (r, _) = ui
                                        .allocate_exact_size(egui::vec2(8.0, 14.0), Sense::hover());
                                    ui.painter().circle_filled(
                                        r.center(),
                                        3.0,
                                        state_color(t.state),
                                    );
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(&t.text).size(12.0).color(TEXT),
                                        )
                                        .truncate(),
                                    );
                                });
                                if t.state == AgentState::Working {
                                    theme::progress(ui, t.progress, CYAN);
                                }
                            }
                        });
                        theme::widget_box(ui, |ui| {
                            if header(ui, "Now playing", "Open") {
                                open = Some(AppKind::Music);
                            }
                            apps::now_playing(ui, s, 46.0, "");
                        });
                        theme::widget_box(ui, |ui| {
                            if header(ui, "Today", "Open") {
                                open = Some(AppKind::Calendar);
                            }
                            let now = chrono::Local::now();
                            let hour = now.hour() as f32 + now.minute() as f32 / 60.0;
                            let next = TODAY.iter().position(|e| e.0 > hour);
                            for (i, (start, at, title)) in TODAY.iter().enumerate() {
                                let past = *start + 0.5 < hour;
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new(*at).font(theme::mono(11.0)).color(if past {
                                            alpha(DIM, 0.6)
                                        } else {
                                            CYAN
                                        }),
                                    );
                                    ui.label(RichText::new(*title).size(12.5).color(if past {
                                        DIM
                                    } else {
                                        ICE
                                    }));
                                    if Some(i) == next {
                                        ui.with_layout(
                                            Layout::right_to_left(Align::Center),
                                            |ui| theme::chip(ui, "Next", AMBER),
                                        );
                                    }
                                });
                            }
                        });
                        theme::widget_box(ui, |ui| {
                            if header(ui, "System", "Open") {
                                open = Some(AppKind::Monitor);
                            }
                            for (name, series, value) in [
                                ("CPU", &s.metrics.cpu, s.metrics.cpu_text()),
                                ("Net", &s.metrics.net, s.metrics.net_text()),
                            ] {
                                ui.horizontal(|ui| {
                                    ui.label(theme::label(name));
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        ui.label(theme::label(value).color(ICE))
                                    });
                                });
                                let (rect, _) = ui.allocate_exact_size(
                                    egui::vec2(ui.available_width(), 30.0),
                                    Sense::hover(),
                                );
                                let data: Vec<f32> = series.iter().copied().collect();
                                theme::sparkline(ui.painter(), rect, &data, false);
                            }
                        });
                    });
            });
        if let Some(app) = open {
            self.open(ctx, app);
        }
    }

    /// The right column: what agents are doing, with approve and discard.
    fn feed(&mut self, ctx: &Context) {
        let mut approve = None;
        let mut discard = None;
        egui::SidePanel::right("feed")
            .exact_width(290.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(VOID).inner_margin(Margin::same(12)))
            .show(ctx, |ui| {
                let rt = &self.shared.runtime;
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("ACTIVITY")
                            .font(theme::display(12.0))
                            .extra_letter_spacing(2.0)
                            .color(ICE),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if rt.blocked() > 0 {
                            theme::chip(ui, &format!("{} needs you", rt.blocked()), AMBER);
                        }
                        if rt.working() > 0 {
                            theme::chip(ui, &format!("{} working", rt.working()), CYAN);
                        }
                    });
                });
                ui.label(
                    RichText::new("What your agents are doing. Results wait for your OK.")
                        .size(11.5)
                        .color(DIM),
                );
                ui.add_space(6.0);
                ScrollArea::vertical()
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        for t in &rt.tasks {
                            let blocked = t.state == AgentState::Blocked;
                            let done = t.state == AgentState::Done;
                            let card = egui::Frame::new()
                                .fill(if done { Color32::TRANSPARENT } else { PANEL })
                                .stroke(Stroke::new(
                                    1.0,
                                    if blocked { alpha(AMBER, 0.55) } else { LINE },
                                ))
                                .inner_margin(Margin::same(10))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.horizontal(|ui| {
                                        theme::chip(ui, t.state.label(), state_color(t.state));
                                        ui.with_layout(
                                            Layout::right_to_left(Align::Center),
                                            |ui| {
                                                ui.label(
                                                    RichText::new(format!("#{}", t.id))
                                                        .font(theme::mono(10.5))
                                                        .color(DIM),
                                                );
                                            },
                                        );
                                    });
                                    ui.label(
                                        RichText::new(&t.text)
                                            .font(theme::medium(12.5))
                                            .color(if done { TEXT } else { ICE }),
                                    );
                                    match t.state {
                                        AgentState::Working => {
                                            let step = ((t.progress * PLAN.len() as f32) as usize)
                                                .min(PLAN.len() - 1);
                                            ui.horizontal(|ui| {
                                                ui.label(
                                                    RichText::new(format!(
                                                        "Step {} of {}",
                                                        step + 1,
                                                        PLAN.len()
                                                    ))
                                                    .size(11.5)
                                                    .color(TEXT),
                                                );
                                                ui.with_layout(
                                                    Layout::right_to_left(Align::Center),
                                                    |ui| {
                                                        ui.label(
                                                            RichText::new(format!(
                                                                "{:.0}%",
                                                                t.progress * 100.0
                                                            ))
                                                            .font(theme::mono(10.5))
                                                            .color(CYAN),
                                                        );
                                                    },
                                                );
                                            });
                                            ui.label(
                                                RichText::new(PLAN[step]).size(11.5).color(DIM),
                                            );
                                            theme::progress(ui, t.progress, CYAN);
                                        }
                                        AgentState::Blocked => {
                                            if let Some(r) = &t.result {
                                                ui.label(RichText::new(r).size(12.0).color(TEXT));
                                            }
                                            ui.add_space(2.0);
                                            ui.horizontal(|ui| {
                                                if theme::button(ui, "Approve", true).clicked() {
                                                    approve = Some(t.id);
                                                }
                                                if theme::button(ui, "Discard", false).clicked() {
                                                    discard = Some(t.id);
                                                }
                                            });
                                        }
                                        AgentState::Done => {
                                            if let Some(r) = &t.result {
                                                ui.label(RichText::new(r).size(11.5).color(DIM));
                                            }
                                        }
                                    }
                                });
                            if blocked {
                                theme::brackets(ui.painter(), card.response.rect);
                            }
                            ui.add_space(4.0);
                        }
                    });
            });
        if let Some(id) = approve {
            self.shared.runtime.approve(id);
            self.shared.toast("Approved. Your agent is sending it.");
        }
        if let Some(id) = discard {
            self.shared.runtime.discard(id);
            self.shared.toast("Discarded.");
        }
    }

    /// The stage: the grid, the core, and the launcher ring when it is open.
    fn stage(&mut self, ctx: &Context) {
        let state = self.shared.runtime.core_state();
        let time = self.shared.time;
        let launcher = self.core.ring_open;
        let ws = self.ws;
        let count = self
            .windows
            .iter()
            .filter(|w| w.ws == ws && !w.minimized)
            .count();
        let mut picked = None;
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(VOID))
            .show(ctx, |ui| {
                let stage = ui.max_rect();
                self.stage = stage;
                paint_grid(ui.painter(), stage);
                let caption = format!("{}  ·  {} open  ·  press / to command", ws.title(), count)
                    .to_uppercase();
                ui.painter().text(
                    stage.left_bottom() + egui::vec2(18.0, -16.0),
                    Align2::LEFT_BOTTOM,
                    caption,
                    theme::display(9.5),
                    alpha(DIM, 0.7),
                );
                if !launcher {
                    let (center, size) = core_place(stage);
                    picked = self.core.ui(ui, center, size, state, time);
                }
            });
        if launcher {
            let stage = self.stage;
            let (center, size) = core_place(stage);
            egui::Area::new(Id::new("launcher"))
                .order(Order::Foreground)
                .fixed_pos(stage.min)
                .show(ctx, |ui| {
                    let (rect, backdrop) = ui.allocate_exact_size(stage.size(), Sense::click());
                    ui.painter()
                        .rect_filled(rect, 0.0, Color32::from_black_alpha(165));
                    picked = self.core.ui(ui, center, size, state, time);
                    if backdrop.clicked() {
                        self.core.ring_open = false;
                    }
                });
        }
        if let Some(app) = picked {
            self.open(ctx, app);
        }
    }

    fn windows(&mut self, ctx: &Context) {
        let stage = self.stage;
        let pressed_layer = ctx
            .input(|i| {
                if i.pointer.any_pressed() {
                    i.pointer.interact_pos()
                } else {
                    None
                }
            })
            .and_then(|p| ctx.layer_id_at(p));
        let shadow = ctx.style().visuals.window_shadow;
        let mut chrome = Vec::new();
        let Self {
            windows,
            apps,
            shared,
            focused,
            ws,
            ..
        } = self;
        for w in windows.iter_mut().filter(|w| w.ws == *ws && !w.minimized) {
            let kind = w.kind;
            let is_focused = *focused == Some(kind);
            let frame = egui::Frame::new()
                .fill(PANEL)
                .stroke(Stroke::new(
                    1.0,
                    if is_focused { LINE_STRONG } else { LINE },
                ))
                .shadow(shadow);
            let mut window = egui::Window::new(kind.title())
                .id(w.id())
                .title_bar(false)
                .collapsible(false)
                .movable(false)
                .frame(frame)
                .constrain_to(stage);
            if w.maximized {
                window = window.fixed_rect(stage.shrink(1.0));
            } else {
                let max = (stage.size() - egui::vec2(8.0, 8.0)).max(egui::vec2(260.0, 180.0));
                window = window
                    .current_pos(stage.min + w.offset)
                    .default_size(kind.default_size().min(max - egui::vec2(0.0, 30.0)))
                    .min_size(egui::vec2(260.0, 150.0))
                    .max_size(max)
                    .resizable(true);
            }
            let maximized = w.maximized;
            let mut drag = Vec2::ZERO;
            let shown = window.show(ctx, |ui| {
                let (action, delta) = title_bar(ui, kind, is_focused, maximized);
                drag = delta;
                if let Some(action) = action {
                    chrome.push((kind, action));
                }
                apps.ui(kind, ui, shared);
            });
            if let Some(shown) = shown {
                let layer = shown.response.layer_id;
                if pressed_layer == Some(layer) {
                    *focused = Some(kind);
                }
                if is_focused {
                    theme::brackets(&ctx.layer_painter(layer), shown.response.rect);
                }
                if !maximized {
                    let limit = (stage.size() - egui::vec2(140.0, 40.0)).max(Vec2::ZERO);
                    w.offset = (w.offset + drag).clamp(Vec2::ZERO, limit);
                }
            }
        }
        for (kind, action) in chrome {
            let Some(i) = self.windows.iter().position(|w| w.kind == kind) else {
                continue;
            };
            match action {
                Chrome::Minimize => {
                    self.windows[i].minimized = true;
                    self.focused = None;
                }
                Chrome::Maximize => {
                    self.windows[i].maximized = !self.windows[i].maximized;
                    self.focused = Some(kind);
                }
                Chrome::Close => {
                    self.windows.remove(i);
                    self.focused = None;
                }
            }
        }
    }

    fn command_bar(&mut self, ctx: &Context) {
        let stage = self.stage;
        let width = (stage.width() - 48.0).clamp(260.0, 580.0);
        let slash =
            ctx.input(|i| i.key_pressed(Key::Slash)) && ctx.memory(|m| m.focused().is_none());
        let mut submit: Option<String> = None;
        let mut voice = false;
        egui::Area::new(Id::new("command_bar"))
            .order(Order::Foreground)
            .fixed_pos(stage.center_bottom() - egui::vec2(0.0, 34.0))
            .pivot(Align2::CENTER_BOTTOM)
            .show(ctx, |ui| {
                let out = egui::Frame::new()
                    .fill(BAR)
                    .stroke(Stroke::new(1.0, LINE_STRONG))
                    .inner_margin(Margin::same(10))
                    .shadow(ctx.style().visuals.window_shadow)
                    .show(ui, |ui| {
                        ui.set_width(width);
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                            for s in SUGGESTIONS {
                                if suggestion(ui, s).clicked() {
                                    submit = Some(s.to_string());
                                }
                            }
                        });
                        ui.add_space(2.0);
                        ui.horizontal(|ui| {
                            let (r, mic) =
                                ui.allocate_exact_size(Vec2::splat(30.0), Sense::click());
                            let hot = mic.hovered();
                            ui.painter().rect(
                                r,
                                0.0,
                                alpha(CYAN, if hot { 0.16 } else { 0.06 }),
                                Stroke::new(1.0, LINE_STRONG),
                                StrokeKind::Inside,
                            );
                            icons::paint(
                                ui.painter(),
                                Rect::from_center_size(r.center(), Vec2::splat(16.0)),
                                Icon::Mic,
                                if hot { ICE } else { CYAN },
                            );
                            voice = mic.on_hover_cursor(CursorIcon::PointingHand).clicked();

                            let edit = ui.add(
                                egui::TextEdit::singleline(&mut self.command)
                                    .id(Id::new("command_input"))
                                    .hint_text(
                                        RichText::new("Ask Sphere anything, or type a command")
                                            .color(DIM),
                                    )
                                    .font(theme::body(13.5))
                                    .margin(Margin::symmetric(8, 6))
                                    .desired_width(ui.available_width() - 78.0),
                            );
                            if edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                                submit = Some(std::mem::take(&mut self.command));
                                edit.request_focus();
                            }
                            if theme::button(ui, "Send", true).clicked() {
                                submit = Some(std::mem::take(&mut self.command));
                            }
                            if slash {
                                edit.request_focus();
                            }
                        });
                    });
                theme::brackets(ui.painter(), out.response.rect);
            });
        if voice {
            self.shared
                .toast("Voice input arrives in step 3. Type here for now.");
        }
        if let Some(text) = submit {
            self.run_command(ctx, &text);
        }
    }

    fn toasts(&self, ctx: &Context) {
        let now = self.shared.time;
        if self.shared.toasts.is_empty() {
            return;
        }
        egui::Area::new(Id::new("toasts"))
            .order(Order::Tooltip)
            .interactable(false)
            .fixed_pos(self.stage.right_top() + egui::vec2(-16.0, 16.0))
            .pivot(Align2::RIGHT_TOP)
            .show(ctx, |ui| {
                for (text, born) in self.shared.toasts.iter().rev().take(3) {
                    let age = (now - born) as f32;
                    let fade = (age / 0.2).min((3.8 - age) / 0.5).clamp(0.0, 1.0);
                    ui.scope(|ui| {
                        ui.set_opacity(fade);
                        egui::Frame::new()
                            .fill(PANEL_HI)
                            .stroke(Stroke::new(1.0, alpha(CYAN, 0.6)))
                            .inner_margin(Margin::symmetric(12, 9))
                            .show(ui, |ui| {
                                ui.set_max_width(320.0);
                                ui.horizontal(|ui| {
                                    let (r, _) = ui
                                        .allocate_exact_size(egui::vec2(8.0, 14.0), Sense::hover());
                                    ui.painter().circle_filled(r.center(), 3.0, CYAN);
                                    ui.label(RichText::new(text).size(12.5).color(ICE));
                                });
                            });
                    });
                    ui.add_space(6.0);
                }
            });
    }
}

impl eframe::App for SphereApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        VOID.to_normalized_gamma_f32()
    }

    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        let dt = ctx.input(|i| i.stable_dt).min(0.1);
        self.shared.tick(dt);
        self.apps.tick(&mut self.shared);
        for app in std::mem::take(&mut self.shared.open_requests) {
            self.open(ctx, app);
        }
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.core.ring_open = false;
        }

        let width = ctx.screen_rect().width();
        self.top_bar(ctx);
        self.dock(ctx);
        if width >= 1180.0 {
            self.widgets(ctx);
        }
        if width >= 860.0 {
            self.feed(ctx);
        }
        self.stage(ctx);
        if !self.laid_out {
            self.lay_out();
            self.laid_out = true;
        }
        self.windows(ctx);
        self.command_bar(ctx);
        self.toasts(ctx);
        ctx.request_repaint();
    }
}

/// Where the core sits on the stage, and how big it is.
fn core_place(stage: Rect) -> (Pos2, f32) {
    let size = (stage.height() * 0.5)
        .min(stage.width() * 0.5)
        .clamp(160.0, 320.0);
    (
        Pos2::new(
            stage.left() + stage.width() * 0.42,
            stage.top() + stage.height() * 0.44,
        ),
        size,
    )
}

fn paint_grid(p: &Painter, rect: Rect) {
    let step = 48.0;
    let line = Stroke::new(1.0, alpha(CYAN, 0.03));
    let mut x = rect.left() + step;
    while x < rect.right() {
        p.line_segment(
            [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
            line,
        );
        x += step;
    }
    let mut y = rect.top() + step;
    while y < rect.bottom() {
        p.line_segment(
            [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
            line,
        );
        y += step;
    }
}

fn state_color(state: AgentState) -> Color32 {
    match state {
        AgentState::Working => CYAN,
        AgentState::Blocked => AMBER,
        AgentState::Done => GREEN,
    }
}

/// A widget header: caption on the left, a clickable action on the right.
fn header(ui: &mut Ui, title: &str, action: &str) -> bool {
    let mut clicked = false;
    ui.horizontal(|ui| {
        ui.label(theme::label(title).color(ICE));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let link =
                ui.add(egui::Label::new(theme::label(action).color(CYAN)).sense(Sense::click()));
            clicked = link.on_hover_cursor(CursorIcon::PointingHand).clicked();
        });
    });
    clicked
}

fn workspace_tab(ui: &mut Ui, title: &str, count: usize, active: bool) -> egui::Response {
    let text = format!("{}  {}", title.to_uppercase(), count);
    let galley =
        ui.painter()
            .layout_no_wrap(text, theme::display(10.5), if active { ICE } else { DIM });
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(galley.size().x + 24.0, 44.0), Sense::click());
    let p = ui.painter();
    if active {
        p.rect_filled(rect, 0.0, alpha(CYAN, 0.08));
        p.rect_filled(
            Rect::from_min_max(
                Pos2::new(rect.left(), rect.bottom() - 2.0),
                rect.right_bottom(),
            ),
            0.0,
            CYAN,
        );
    } else if resp.hovered() {
        p.rect_filled(rect, 0.0, alpha(CYAN, 0.04));
    }
    p.galley(rect.center() - galley.size() / 2.0, galley, ICE);
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

fn dock_button(
    ui: &mut Ui,
    icon: Icon,
    tip: &str,
    open: bool,
    here: bool,
    focused: bool,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(44.0, 40.0), Sense::click());
    let hovered = resp.hovered();
    let p = ui.painter();
    if focused || hovered {
        p.rect(
            rect,
            0.0,
            alpha(CYAN, if focused { 0.14 } else { 0.07 }),
            Stroke::new(1.0, if focused { LINE_STRONG } else { LINE }),
            StrokeKind::Inside,
        );
    }
    if here {
        p.rect_filled(
            Rect::from_min_size(
                Pos2::new(rect.left() - 8.0, rect.top() + 9.0),
                egui::vec2(2.0, 22.0),
            ),
            0.0,
            CYAN,
        );
    } else if open {
        p.circle_filled(Pos2::new(rect.left() - 5.0, rect.center().y), 2.0, DIM);
    }
    let color = if focused || hovered {
        ICE
    } else if here {
        CYAN
    } else {
        DIM
    };
    icons::paint(
        p,
        Rect::from_center_size(rect.center(), Vec2::splat(20.0)),
        icon,
        color,
    );
    resp.on_hover_text(tip)
        .on_hover_cursor(CursorIcon::PointingHand)
}

/// A window's title bar: icon, title, and minimize, maximize and close.
/// Returns the button pressed and how far the bar was dragged.
fn title_bar(ui: &mut Ui, kind: AppKind, focused: bool, maximized: bool) -> (Option<Chrome>, Vec2) {
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 30.0),
        Sense::click_and_drag(),
    );
    let p = ui.painter();
    p.rect_filled(rect, 0.0, if focused { PANEL_HI } else { BAR });
    p.line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(1.0, LINE),
    );
    let accent = if focused { CYAN } else { DIM };
    icons::paint(
        p,
        Rect::from_center_size(
            Pos2::new(rect.left() + 18.0, rect.center().y),
            Vec2::splat(14.0),
        ),
        kind.icon(),
        accent,
    );
    p.text(
        Pos2::new(rect.left() + 34.0, rect.center().y),
        Align2::LEFT_CENTER,
        kind.title().to_uppercase(),
        theme::display(10.5),
        if focused { ICE } else { TEXT },
    );

    let mut action = None;
    let buttons = [
        (Icon::Close, Chrome::Close),
        (Icon::Max, Chrome::Maximize),
        (Icon::Min, Chrome::Minimize),
    ];
    for (i, (icon, act)) in buttons.into_iter().enumerate() {
        let r = Rect::from_min_size(
            Pos2::new(rect.right() - 32.0 * (i as f32 + 1.0), rect.top()),
            egui::vec2(32.0, 30.0),
        );
        let b = ui.interact(r, ui.id().with(("chrome", i)), Sense::click());
        let hot = b.hovered();
        if hot {
            let fill = if matches!(act, Chrome::Close) {
                alpha(RED, 0.25)
            } else {
                alpha(CYAN, 0.12)
            };
            p.rect_filled(r.shrink(1.0), 0.0, fill);
        }
        icons::paint(
            p,
            Rect::from_center_size(r.center(), Vec2::splat(12.0)),
            icon,
            if hot { ICE } else { DIM },
        );
        if b.on_hover_cursor(CursorIcon::PointingHand).clicked() {
            action = Some(act);
        }
    }
    if resp.double_clicked() {
        action = Some(Chrome::Maximize);
    }
    let drag = if resp.dragged() && !maximized {
        resp.drag_delta()
    } else {
        Vec2::ZERO
    };
    (action, drag)
}

/// A suggestion pill in the command bar.
fn suggestion(ui: &mut Ui, text: &str) -> egui::Response {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), theme::body(11.5), TEXT);
    let (rect, resp) =
        ui.allocate_exact_size(galley.size() + egui::vec2(18.0, 10.0), Sense::click());
    let hot = resp.hovered();
    ui.painter().rect(
        rect,
        0.0,
        alpha(CYAN, if hot { 0.12 } else { 0.03 }),
        Stroke::new(1.0, if hot { LINE_STRONG } else { LINE }),
        StrokeKind::Inside,
    );
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, TEXT);
    resp.on_hover_cursor(CursorIcon::PointingHand)
}
