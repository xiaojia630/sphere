use eframe::egui::{self, RichText, ScrollArea, Sense, Stroke, Ui};

use crate::services::Shared;
use crate::theme::{self, alpha, AMBER, CYAN, DIM, ICE, LINE, TEXT};

struct Mail {
    from: &'static str,
    subject: &'static str,
    body: &'static str,
    draft: Option<&'static str>,
}

pub struct MailApp {
    mails: Vec<Mail>,
    open: usize,
}

impl Default for MailApp {
    fn default() -> Self {
        let m = |from, subject, body, draft| Mail {
            from,
            subject,
            body,
            draft,
        };
        Self {
            mails: vec![
                m("Tolu Adeyemi", "Invoice #0423", "Hi, just checking when we can expect payment for the March invoice.", Some("Hi Tolu, thanks for the nudge. Payment goes out by the 24th. Best, Gabriel")),
                m("Ada Okafor", "Agenda for 2pm", "Can we cover pricing and the launch date? I'd like to lock both today.", None),
                m("Stripe", "Your weekly summary", "Gross volume was up 12% on last week.", None),
                m("Kemi", "Saturday?", "Are we still on for Saturday? Thinking 7pm.", Some("Yes! 7pm works. See you then.")),
                m("GitHub", "PR #1 opened on jarvis", "Add Sphere OS: a JARVIS-style sphere you can rotate, zoom and expand.", None),
            ],
            open: 0,
        }
    }
}

impl MailApp {
    pub fn ui(&mut self, ui: &mut Ui, shared: &mut Shared) {
        egui::SidePanel::left("mail_list")
            .exact_width(200.0)
            .resizable(false)
            .frame(egui::Frame::new().stroke(Stroke::new(1.0, LINE)))
            .show_inside(ui, |ui| {
                ScrollArea::vertical()
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        for (i, m) in self.mails.iter().enumerate() {
                            let h = if m.draft.is_some() { 66.0 } else { 46.0 };
                            let (rect, resp) = ui.allocate_exact_size(
                                egui::vec2(ui.available_width(), h),
                                Sense::click(),
                            );
                            let p = ui.painter();
                            if i == self.open {
                                p.rect_filled(rect, 0.0, alpha(CYAN, 0.1));
                            } else if resp.hovered() {
                                p.rect_filled(rect, 0.0, alpha(CYAN, 0.04));
                            }
                            p.line_segment(
                                [rect.left_bottom(), rect.right_bottom()],
                                Stroke::new(1.0, LINE),
                            );
                            p.text(
                                rect.left_top() + egui::vec2(11.0, 8.0),
                                egui::Align2::LEFT_TOP,
                                m.from,
                                theme::medium(12.5),
                                ICE,
                            );
                            p.text(
                                rect.left_top() + egui::vec2(11.0, 26.0),
                                egui::Align2::LEFT_TOP,
                                m.subject,
                                theme::body(11.0),
                                DIM,
                            );
                            if m.draft.is_some() {
                                let g = p.layout_no_wrap(
                                    "DRAFT READY".into(),
                                    theme::display(9.0),
                                    AMBER,
                                );
                                let r = egui::Rect::from_min_size(
                                    rect.left_top() + egui::vec2(11.0, 44.0),
                                    g.size() + egui::vec2(12.0, 6.0),
                                );
                                p.rect_stroke(
                                    r,
                                    0.0,
                                    Stroke::new(1.0, AMBER),
                                    egui::StrokeKind::Inside,
                                );
                                p.galley(r.center() - g.size() / 2.0, g, AMBER);
                            }
                            if resp.clicked() {
                                self.open = i;
                            }
                        }
                    });
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(14, 12)))
            .show_inside(ui, |ui| {
                let m = &mut self.mails[self.open];
                ui.label(theme::label(m.from));
                ui.label(
                    RichText::new(m.subject)
                        .font(theme::medium(15.0))
                        .color(ICE),
                );
                ui.add_space(4.0);
                ui.label(RichText::new(m.body).color(TEXT));
                if let Some(draft) = m.draft {
                    ui.add_space(10.0);
                    let mut sent = false;
                    egui::Frame::new()
                        .stroke(Stroke::new(1.0, AMBER))
                        .inner_margin(egui::Margin::same(10))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(theme::label("Agent draft").color(AMBER));
                            ui.label(RichText::new(draft).color(ICE));
                            ui.add_space(4.0);
                            ui.horizontal(|ui| {
                                sent = theme::button(ui, "Send", true).clicked();
                                theme::button(ui, "Edit", false);
                            });
                        });
                    if sent {
                        m.draft = None;
                        shared.toast("Sent");
                    }
                }
            });
    }
}
