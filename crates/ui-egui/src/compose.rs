//! The compose window: a card docked at the bottom right, like Gmail's.

use egui::{Align, FontId, Layout, RichText, Sense, Stroke, StrokeKind, vec2};

use serde_json::json;

use crate::icons::{self, Icon};
use crate::theme::Tokens;
use crate::widgets::{icon_button, primary_button};
use crate::{MailApp, Request};

/// A recipients field with completion: while a name or address is typed after the last comma,
/// the people seen in mail are offered below it (click, or Enter for the first).
fn address_field(app: &mut MailApp, ui: &mut egui::Ui, t: &Tokens, label: &str, value: &mut String, hint: &str, extra: impl FnOnce(&mut egui::Ui)) {
    let resp = ui
        .horizontal(|ui| {
            ui.add_sized(vec2(44.0, 24.0), egui::Label::new(RichText::new(label).color(t.muted)));
            let w = ui.available_width() - 80.0;
            let r = ui.add(egui::TextEdit::singleline(value).hint_text(hint).desired_width(w).frame(egui::Frame::NONE));
            extra(ui);
            r
        })
        .inner;
    ui.painter().hline(ui.min_rect().x_range(), ui.cursor().top(), Stroke::new(1.0, t.border));
    let token = value.rsplit(',').next().unwrap_or_default().trim().to_string();
    if resp.has_focus() && token.chars().count() >= 2 && app.contact_query != token {
        app.contact_query = token.clone();
        app.contact_hits.clear();
        app.run("contacts.search", json!({"query": token, "limit": 6}));
    }
    if token.is_empty() || app.contact_query != token || app.contact_hits.is_empty() {
        return;
    }
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.contact_hits.clear();
        return;
    }
    let lower = value.to_ascii_lowercase();
    let hits: Vec<(String, String)> = app.contact_hits.iter().filter(|(_, email)| !lower.contains(&email.to_ascii_lowercase())).cloned().collect();
    let mut chosen: Option<String> = None;
    if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        chosen = hits.first().map(|h| h.0.clone());
    }
    egui::Frame::NONE.fill(t.panel).stroke(Stroke::new(1.0, t.border)).corner_radius(8).inner_margin(4).show(ui, |ui| {
        ui.set_width(ui.available_width());
        for (display, _) in &hits {
            if ui.selectable_label(false, display).clicked() {
                chosen = Some(display.clone());
            }
        }
    });
    if let Some(display) = chosen {
        MailApp::complete_recipient(value, &display);
        app.contact_hits.clear();
        app.contact_query.clear();
        resp.request_focus();
    }
}

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
                        // From: the sending account, when there is more than one.
                        let accounts: Vec<(String, String, String)> =
                            app.session.config().accounts.iter().map(|a| (a.id.clone(), a.name.clone(), a.email.clone())).collect();
                        if accounts.len() > 1 {
                            if c.from_account.is_empty() || !accounts.iter().any(|(id, ..)| *id == c.from_account) {
                                c.from_account = app.acting_account();
                            }
                            ui.horizontal(|ui| {
                                ui.add_sized(vec2(44.0, 24.0), egui::Label::new(RichText::new(app.tr("From")).color(t.muted)));
                                let current = accounts
                                    .iter()
                                    .find(|(id, ..)| *id == c.from_account)
                                    .map(|(_, name, email)| format!("{name} <{email}>"))
                                    .unwrap_or_default();
                                egui::ComboBox::from_id_salt("compose-from").selected_text(current).width(ui.available_width() - 70.0).show_ui(
                                    ui,
                                    |ui| {
                                        for (id, name, email) in &accounts {
                                            ui.selectable_value(&mut c.from_account, id.clone(), format!("{name} <{email}>"));
                                        }
                                    },
                                );
                            });
                            ui.painter().hline(ui.min_rect().x_range(), ui.cursor().top(), Stroke::new(1.0, t.border));
                        }
                        let field = |ui: &mut egui::Ui, label: &str, value: &mut String, hint: &str| {
                            ui.horizontal(|ui| {
                                ui.add_sized(vec2(44.0, 24.0), egui::Label::new(RichText::new(label).color(t.muted)));
                                let w = ui.available_width() - 70.0;
                                ui.add(egui::TextEdit::singleline(value).hint_text(hint).desired_width(w).frame(egui::Frame::NONE));
                            });
                            ui.painter().hline(ui.min_rect().x_range(), ui.cursor().top(), Stroke::new(1.0, t.border));
                        };
                        let (to_l, cc_l, bcc_l, hint) =
                            (app.tr("To").to_string(), app.tr("Cc").to_string(), app.tr("Bcc").to_string(), app.tr("Recipients").to_string());
                        let mut show_cc = c.show_cc;
                        address_field(app, ui, t, &to_l, &mut c.to, &hint, |ui| {
                            if !show_cc && ui.small_button("Cc/Bcc").clicked() {
                                show_cc = true;
                            }
                        });
                        c.show_cc = show_cc;
                        if c.show_cc {
                            address_field(app, ui, t, &cc_l, &mut c.cc, "", |_| {});
                            address_field(app, ui, t, &bcc_l, &mut c.bcc, "", |_| {});
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
