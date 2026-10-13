//! The compose window: a card docked at the bottom right, like Gmail's.

use egui::{Align, FontId, Layout, RichText, Sense, Stroke, StrokeKind, vec2};

use crate::icons::{self, Icon};
use crate::theme::Tokens;
use crate::widgets::{icon_button, primary_button};
use crate::{MailApp, Request};

pub fn show(app: &mut MailApp, ui: &mut egui::Ui, t: &Tokens) {
    let Some(mut c) = app.compose.clone() else { return };
    let ctx = ui.ctx().clone();
    let title = if c.reply_to.is_some() {
        format!("{}: {}", app.tr("Reply"), c.subject)
    } else if c.forward_of.is_some() {
        format!("{}: {}", app.tr("Forward"), c.subject)
    } else if c.subject.trim().is_empty() {
        app.tr("New message").to_string()
    } else {
        c.subject.clone()
    };
    let width = 600.0_f32.min(ctx.content_rect().width() - 40.0);
    let mut close = false;
    let mut send = false;
    let mut save = false;
    let mut discard = false;
    egui::Area::new(egui::Id::new("compose")).anchor(egui::Align2::RIGHT_BOTTOM, vec2(-24.0, -34.0)).order(egui::Order::Middle).fade_in(false).show(
        &ctx,
        |ui| {
            egui::Frame::NONE
                .fill(t.card)
                .stroke(Stroke::new(1.0, t.border))
                .corner_radius(12)
                .shadow(egui::epaint::Shadow { offset: [0, 4], blur: 18, spread: 0, color: egui::Color32::from_black_alpha(50) })
                .show(ui, |ui| {
                    ui.set_width(width);
                    // Title bar.
                    let (bar, resp) = ui.allocate_exact_size(vec2(width, 40.0), Sense::click());
                    ui.painter().rect_filled(
                        bar,
                        egui::CornerRadius { nw: 12, ne: 12, sw: 0, se: 0 },
                        if t.dark { t.chrome } else { egui::Color32::from_rgb(0x40, 0x42, 0x4a) },
                    );
                    ui.painter().text(
                        bar.left_center() + vec2(14.0, 0.0),
                        egui::Align2::LEFT_CENTER,
                        &title,
                        FontId::proportional(14.0),
                        egui::Color32::WHITE,
                    );
                    let mut child =
                        ui.new_child(egui::UiBuilder::new().max_rect(bar.shrink2(vec2(6.0, 4.0))).layout(Layout::right_to_left(Align::Center)));
                    let light = Tokens { ui_text: egui::Color32::WHITE, hover: egui::Color32::from_white_alpha(30), ..*t };
                    if icon_button(&mut child, &light, Icon::Close, app.tr("Close"), true, false).clicked() {
                        close = true;
                    }
                    if icon_button(&mut child, &light, Icon::Chevron, if c.minimized { "▲" } else { "▼" }, true, false).clicked() {
                        c.minimized = !c.minimized;
                    }
                    if resp.double_clicked() {
                        c.minimized = !c.minimized;
                    }
                    if c.minimized {
                        return;
                    }
                    egui::Frame::NONE.inner_margin(egui::Margin::symmetric(14, 8)).show(ui, |ui| {
                        ui.set_width(width - 28.0);
                        let field = |ui: &mut egui::Ui, label: &str, value: &mut String, hint: &str| {
                            ui.horizontal(|ui| {
                                ui.add_sized(vec2(44.0, 24.0), egui::Label::new(RichText::new(label).color(t.muted)));
                                let w = ui.available_width() - 70.0;
                                ui.add(egui::TextEdit::singleline(value).hint_text(hint).desired_width(w).frame(egui::Frame::NONE));
                            });
                            ui.painter().hline(ui.min_rect().x_range(), ui.cursor().top(), Stroke::new(1.0, t.border));
                        };
                        ui.horizontal(|ui| {
                            ui.add_sized(vec2(44.0, 24.0), egui::Label::new(RichText::new(app.tr("To")).color(t.muted)));
                            let w = ui.available_width() - 80.0;
                            ui.add(egui::TextEdit::singleline(&mut c.to).hint_text(app.tr("Recipients")).desired_width(w).frame(egui::Frame::NONE));
                            if !c.show_cc && ui.small_button("Cc/Bcc").clicked() {
                                c.show_cc = true;
                            }
                        });
                        ui.painter().hline(ui.min_rect().x_range(), ui.cursor().top(), Stroke::new(1.0, t.border));
                        if c.show_cc {
                            field(ui, app.tr("Cc"), &mut c.cc, "");
                            field(ui, app.tr("Bcc"), &mut c.bcc, "");
                        }
                        field(ui, "", &mut c.subject, app.tr("Subject"));
                        ui.add_space(4.0);
                        let body = ui.add(
                            egui::TextEdit::multiline(&mut c.body)
                                .desired_rows(14)
                                .desired_width(width - 28.0)
                                .frame(egui::Frame::NONE)
                                .font(FontId::proportional(14.0)),
                        );
                        if c.focus_body {
                            body.request_focus();
                            if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), body.id) {
                                state.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(0))));
                                state.store(ui.ctx(), body.id);
                            }
                            c.focus_body = false;
                        }
                        if !c.forward_attachments.is_empty() {
                            ui.label(
                                RichText::new(
                                    app.language.fill("Original attachments included: {names}", &[("names", &c.forward_attachments.join(", "))]),
                                )
                                .small()
                                .color(t.muted),
                            );
                        }
                        if !c.attachments.is_empty() {
                            ui.horizontal_wrapped(|ui| {
                                let mut remove = None;
                                for (i, a) in c.attachments.iter().enumerate() {
                                    let label = format!("{} · {}", a.name, crate::reader::human_size(a.data.len()));
                                    let (rect, resp) = ui.allocate_exact_size(
                                        vec2(ui.painter().layout_no_wrap(label.clone(), FontId::proportional(12.5), t.ui_text).size().x + 44.0, 28.0),
                                        Sense::click(),
                                    );
                                    ui.painter().rect_filled(rect, 8.0, t.panel);
                                    ui.painter().rect_stroke(rect, 8.0, Stroke::new(1.0, t.border), StrokeKind::Inside);
                                    icons::paint(
                                        ui.painter(),
                                        egui::Rect::from_center_size(rect.left_center() + vec2(16.0, 0.0), vec2(14.0, 14.0)),
                                        Icon::Attachment,
                                        t.muted,
                                    );
                                    ui.painter().text(
                                        rect.left_center() + vec2(28.0, 0.0),
                                        egui::Align2::LEFT_CENTER,
                                        label,
                                        FontId::proportional(12.5),
                                        t.ui_text,
                                    );
                                    icons::paint(
                                        ui.painter(),
                                        egui::Rect::from_center_size(rect.right_center() - vec2(12.0, 0.0), vec2(12.0, 12.0)),
                                        Icon::Close,
                                        t.muted,
                                    );
                                    if resp.on_hover_text(app.tr("Discard")).clicked() {
                                        remove = Some(i);
                                    }
                                }
                                if let Some(i) = remove {
                                    c.attachments.remove(i);
                                }
                            });
                        }
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            if primary_button(ui, t, Some(Icon::Send), app.tr("Send")).on_hover_text(app.tip("mail.send")).clicked() {
                                send = true;
                            }
                            if icon_button(ui, t, Icon::Attachment, app.tr("Attach files"), true, false).clicked() {
                                app.requests.push(Request::AttachFiles);
                            }
                            if icon_button(ui, t, Icon::Save, &app.tip("draft.save"), true, false).clicked() {
                                save = true;
                            }
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if icon_button(ui, t, Icon::Trash, app.tr("Discard"), true, false).clicked() {
                                    discard = true;
                                }
                            });
                        });
                    });
                });
        },
    );
    app.compose = Some(c);
    if send {
        app.send_compose();
    } else if save {
        app.save_draft();
    } else if discard {
        app.discard_compose();
    } else if close {
        // Closing keeps the text as a draft when there is something to keep.
        if app.compose.as_ref().is_some_and(|c| !c.body.trim().is_empty() || !c.subject.trim().is_empty() || !c.to.trim().is_empty()) {
            app.save_draft();
        }
        app.compose = None;
    }
}
