//! The left navigation: compose, the special folders, the other folders, the accounts.

use egui::{Align2, FontId, Rect, RichText, Sense, vec2};
use mailcraft_engine::core::mailbox::Role;

use crate::chrome::{avatar, color_for};
use crate::icons::{self, Icon};
use crate::theme::Tokens;
use crate::widgets::primary_button;
use crate::{MailApp, Screen};

const NAV_W: f32 = 236.0;

fn nav_item(ui: &mut egui::Ui, t: &Tokens, icon: Option<Icon>, label: &str, count: u32, active: bool, indent: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
    let p = ui.painter();
    let pill = Rect::from_min_max(rect.min, rect.max).shrink2(vec2(0.0, 1.0));
    if active {
        p.rect_filled(pill, 16.0, t.active);
    } else if resp.hovered() {
        p.rect_filled(pill, 16.0, t.hover);
    }
    let x = rect.left() + 14.0 + indent;
    if let Some(i) = icon {
        icons::paint(p, Rect::from_center_size(egui::pos2(x + 9.0, rect.center().y), vec2(18.0, 18.0)), i, if active { t.accent } else { t.ui_text });
    }
    let text_x = if icon.is_some() { x + 28.0 } else { x };
    let font = FontId::proportional(14.0);
    let color = if active { t.accent } else { t.ui_text };
    let max_w = rect.right() - 48.0 - text_x;
    let galley = p.layout(label.to_string(), font, color, max_w.max(20.0));
    let label_pos = egui::pos2(text_x, rect.center().y - galley.size().y / 2.0);
    p.galley(label_pos, galley, color);
    if count > 0 {
        p.text(
            egui::pos2(rect.right() - 12.0, rect.center().y),
            Align2::RIGHT_CENTER,
            count.to_string(),
            FontId::proportional(12.5),
            if active { t.accent } else { t.ui_text },
        );
    }
    resp
}

pub fn show(app: &mut MailApp, ui: &mut egui::Ui, t: &Tokens) {
    let frame = egui::Frame::NONE.fill(t.chrome).inner_margin(egui::Margin { left: 10, right: 10, top: 12, bottom: 10 });
    egui::Panel::left("nav").exact_size(NAV_W).resizable(false).frame(frame).show(ui, |ui| {
        if primary_button(ui, t, Some(Icon::Compose), app.tr("Compose")).on_hover_text(app.tip("mail.send")).clicked() {
            app.compose_new();
        }
        ui.add_space(10.0);
        let account = app.session.view().account.clone();
        let view_mailbox = app.session.view().mailbox.clone();
        let starred = app.search.trim().eq_ignore_ascii_case("is:starred");
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let roles = [
                (Role::Inbox, Icon::Inbox),
                (Role::Starred, Icon::Star),
                (Role::Sent, Icon::Sent),
                (Role::Drafts, Icon::Drafts),
                (Role::Archive, Icon::Archive),
                (Role::Spam, Icon::Spam),
                (Role::Trash, Icon::Trash),
            ];
            let mut shown: Vec<String> = Vec::new();
            for (role, icon) in roles {
                let path = app.session.folder_for(&account, role);
                if role == Role::Starred {
                    if nav_item(ui, t, Some(icon), app.tr("Starred"), 0, starred, 0.0).clicked() {
                        app.search = "is:starred".into();
                        app.select_mailbox("INBOX");
                        app.search = "is:starred".into();
                        app.run("mail.search", serde_json::json!({"query": "is:starred"}));
                    }
                    continue;
                }
                let Some(path) = path else { continue };
                let (unread, total) = app.session.current().and_then(|s| s.mailbox(&path)).map_or((0, 0), |m| (m.unread, m.total));
                let count = match role {
                    Role::Drafts => total,
                    Role::Inbox | Role::Spam => unread,
                    _ => 0,
                };
                let active = view_mailbox == path && !starred;
                if nav_item(ui, t, Some(icon), app.tr(role.label(false)), count, active, 0.0).clicked() {
                    app.select_mailbox(&path);
                }
                shown.push(path);
            }
            // The other folders.
            let others: Vec<(String, String, u32, usize, bool)> = app
                .session
                .current()
                .map(|s| {
                    let d = s.delimiter.clone();
                    s.mailboxes
                        .iter()
                        .filter(|m| !shown.contains(&m.path) && m.role != Role::Inbox)
                        .map(|m| (m.path.clone(), m.name.clone(), m.unread, m.depth(&d), m.no_select))
                        .collect()
                })
                .unwrap_or_default();
            if !others.is_empty() {
                ui.add_space(8.0);
                ui.label(RichText::new(app.tr("Folders")).small().strong().color(t.muted));
            }
            for (path, name, unread, depth, no_select) in others {
                let active = view_mailbox == path && !starred;
                let r = nav_item(ui, t, Some(Icon::Folder), &name, unread, active, (depth as f32 * 12.0).min(48.0));
                if r.clicked() && !no_select {
                    app.select_mailbox(&path);
                }
            }
            // Accounts.
            ui.add_space(12.0);
            ui.label(RichText::new(app.tr("Accounts")).small().strong().color(t.muted));
            let accounts: Vec<(String, String, String, String, u32)> = app
                .session
                .config()
                .accounts
                .iter()
                .map(|a| {
                    (
                        a.id.clone(),
                        a.name.clone(),
                        a.email.clone(),
                        a.color.clone(),
                        app.session.state(&a.id).and_then(|s| s.by_role(Role::Inbox)).map_or(0, |m| m.unread),
                    )
                })
                .collect();
            for (id, name, email, color, unread) in accounts {
                let active = id == account;
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
                if active {
                    ui.painter().rect_filled(rect, 10.0, t.active);
                } else if resp.hovered() {
                    ui.painter().rect_filled(rect, 10.0, t.hover);
                }
                let mut child = ui.new_child(
                    egui::UiBuilder::new().max_rect(rect.shrink2(vec2(8.0, 4.0))).layout(egui::Layout::left_to_right(egui::Align::Center)),
                );
                avatar(&mut child, t, &name, color_for(&email, &color), 28.0);
                child.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.label(RichText::new(&name).size(13.0).color(if active { t.accent } else { t.ui_text }));
                    ui.label(RichText::new(&email).size(11.0).color(t.muted));
                });
                if unread > 0 {
                    ui.painter().text(
                        egui::pos2(rect.right() - 10.0, rect.center().y),
                        Align2::RIGHT_CENTER,
                        unread.to_string(),
                        FontId::proportional(12.0),
                        t.muted,
                    );
                }
                if resp.on_hover_text(&email).clicked() {
                    app.select_account(&id);
                }
            }
            if nav_item(ui, t, Some(Icon::Plus), app.tr("Add account"), 0, false, 0.0).clicked() {
                app.screen = Screen::Settings;
                app.settings.editing = Some(crate::settings::AccountForm::from_email(""));
            }
        });
    });
}
