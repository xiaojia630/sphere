use eframe::egui::{self, RichText, ScrollArea, Sense, Stroke, Ui};

use crate::icons::{self, Icon};
use crate::services::{clock, Shared, TRACKS};
use crate::theme::{self, alpha, CYAN, DIM, ICE, LINE, LINE_STRONG, TEXT};

const PLAYLISTS: [&str; 5] = [
    "Build Mode",
    "Focus Flow",
    "Afrobeats Now",
    "Liked Songs",
    "Discover Weekly",
];

#[derive(Default)]
pub struct MusicApp {
    playlist: usize,
    search: String,
}

impl MusicApp {
    pub fn ui(&mut self, ui: &mut Ui, shared: &mut Shared) {
        egui::SidePanel::left("music_lists")
            .exact_width(140.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(8, 10))
                    .stroke(Stroke::new(1.0, LINE)),
            )
            .show_inside(ui, |ui| {
                ui.label(theme::label("Library"));
                ui.add_space(4.0);
                for (i, name) in PLAYLISTS.iter().enumerate() {
                    let selected = i == self.playlist;
                    let (rect, resp) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), 26.0),
                        Sense::click(),
                    );
                    if selected {
                        ui.painter().rect_filled(rect, 0.0, alpha(CYAN, 0.1));
                    }
                    ui.painter().text(
                        rect.left_center() + egui::vec2(8.0, 0.0),
                        egui::Align2::LEFT_CENTER,
                        *name,
                        theme::body(12.0),
                        if selected { ICE } else { TEXT },
                    );
                    if resp.clicked() {
                        self.playlist = i;
                    }
                }
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::new().inner_margin(egui::Margin::same(14)))
            .show_inside(ui, |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.search)
                        .hint_text("Search Spotify")
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(10.0);
                now_playing(ui, shared, 92.0, PLAYLISTS[self.playlist]);
                ui.add_space(8.0);
                ui.separator();
                let query = self.search.to_lowercase();
                ScrollArea::vertical()
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        for (i, t) in TRACKS.iter().enumerate() {
                            if !query.is_empty()
                                && !format!("{} {}", t.title, t.artist)
                                    .to_lowercase()
                                    .contains(&query)
                            {
                                continue;
                            }
                            let current = i == shared.player.index;
                            let (rect, resp) = ui.allocate_exact_size(
                                egui::vec2(ui.available_width(), 38.0),
                                Sense::click(),
                            );
                            if resp.hovered() {
                                ui.painter().rect_filled(rect, 0.0, alpha(CYAN, 0.05));
                            }
                            let p = ui.painter();
                            let color = if current { CYAN } else { ICE };
                            p.text(
                                rect.left_center() + egui::vec2(4.0, 0.0),
                                egui::Align2::LEFT_CENTER,
                                format!("{}", i + 1),
                                theme::mono(11.0),
                                if current { CYAN } else { DIM },
                            );
                            p.text(
                                rect.left_top() + egui::vec2(30.0, 3.0),
                                egui::Align2::LEFT_TOP,
                                t.title,
                                theme::medium(13.0),
                                color,
                            );
                            p.text(
                                rect.left_top() + egui::vec2(30.0, 21.0),
                                egui::Align2::LEFT_TOP,
                                t.artist,
                                theme::body(11.0),
                                DIM,
                            );
                            p.text(
                                rect.right_center() - egui::vec2(4.0, 0.0),
                                egui::Align2::RIGHT_CENTER,
                                clock(t.secs),
                                theme::mono(11.0),
                                DIM,
                            );
                            if resp.clicked() {
                                shared.player.play_at(i);
                            }
                        }
                    });
            });
    }
}

/// The now-playing block, shared by the Music window and the side widget.
pub fn now_playing(ui: &mut Ui, shared: &mut Shared, art: f32, context: &str) {
    let track = shared.player.track();
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(art, art), Sense::hover());
        let (a, b) = theme::art_colors(&format!("{}{}", track.title, track.artist));
        theme::gradient_square(ui.painter(), rect, a, b);
        let initials: String = {
            let words: Vec<&str> = track
                .artist
                .split(|c: char| !c.is_alphabetic())
                .filter(|w| !w.is_empty())
                .collect();
            if words.len() > 1 {
                words
                    .iter()
                    .take(2)
                    .map(|w| w.chars().next().unwrap())
                    .collect()
            } else {
                words
                    .first()
                    .map(|w| w.chars().take(2).collect())
                    .unwrap_or_default()
            }
        };
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            initials.to_uppercase(),
            theme::display(art * 0.24),
            alpha(ICE, 0.85),
        );

        ui.vertical(|ui| {
            if !context.is_empty() {
                ui.label(theme::label(context));
            }
            ui.label(
                RichText::new(track.title)
                    .font(theme::medium(if art > 60.0 { 16.0 } else { 13.0 }))
                    .color(ICE),
            );
            ui.label(RichText::new(track.artist).size(11.5).color(DIM));
            seek_bar(ui, shared);
            if art > 60.0 {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(clock(shared.player.position))
                            .font(theme::mono(10.5))
                            .color(DIM),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(clock(shared.player.track().secs))
                                .font(theme::mono(10.5))
                                .color(DIM),
                        );
                    });
                });
            }
            controls(ui, shared, art > 60.0);
        });
    });
}

pub fn seek_bar(ui: &mut Ui, shared: &mut Shared) {
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 10.0),
        Sense::click_and_drag(),
    );
    let bar = egui::Rect::from_center_size(rect.center(), egui::vec2(rect.width(), 4.0));
    ui.painter().rect_filled(bar, 0.0, alpha(CYAN, 0.15));
    let mut fill = bar;
    fill.set_width(bar.width() * shared.player.fraction().clamp(0.0, 1.0));
    ui.painter().rect_filled(fill, 0.0, CYAN);
    if let Some(pos) = resp.interact_pointer_pos() {
        let f = ((pos.x - bar.left()) / bar.width()).clamp(0.0, 1.0);
        shared.player.position = f * shared.player.track().secs;
    }
}

pub fn controls(ui: &mut Ui, shared: &mut Shared, with_prev: bool) {
    ui.horizontal(|ui| {
        if with_prev && icon_button(ui, Icon::Prev, 30.0, false).clicked() {
            shared.player.prev();
        }
        let icon = if shared.player.playing {
            Icon::Pause
        } else {
            Icon::Play
        };
        if icon_button(ui, icon, if with_prev { 38.0 } else { 30.0 }, true).clicked() {
            shared.player.playing = !shared.player.playing;
        }
        if icon_button(ui, Icon::Next, 30.0, false).clicked() {
            shared.player.next();
        }
    });
}

fn icon_button(ui: &mut Ui, icon: Icon, size: f32, round: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(size, size), Sense::click());
    let p = ui.painter();
    let stroke = Stroke::new(
        1.0,
        if round {
            CYAN
        } else if resp.hovered() {
            LINE_STRONG
        } else {
            LINE
        },
    );
    if round {
        p.circle(
            rect.center(),
            size / 2.0,
            alpha(CYAN, if resp.hovered() { 0.28 } else { 0.15 }),
            stroke,
        );
    } else {
        p.rect_stroke(rect, 0.0, stroke, egui::StrokeKind::Inside);
    }
    icons::paint(
        p,
        egui::Rect::from_center_size(rect.center(), egui::vec2(14.0, 14.0)),
        icon,
        ICE,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}
