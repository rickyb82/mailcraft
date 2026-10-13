//! Settings: accounts (add, edit, test, find servers), preferences, the assistant (MCP) and
//! the keyboard shortcuts.

use egui::{Color32, RichText, Stroke, StrokeKind, vec2};
use mailcraft_engine::core::account::{Account, Security};
use mailcraft_engine::core::mailbox::Role;
use serde_json::{Value, json};

use crate::chrome::{avatar, color_for};
use crate::icons::{self, Icon};
use crate::theme::Tokens;
use crate::widgets::{icon_text_button, primary_button};
use crate::{Language, MailApp, Screen};

/// The account form.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AccountForm {
    pub id: Option<String>,
    pub name: String,
    pub email: String,
    pub display_name: String,
    pub username: String,
    pub password: String,
    pub imap_host: String,
    pub imap_port: String,
    pub imap_security: Security,
    pub smtp_host: String,
    pub smtp_port: String,
    pub smtp_security: Security,
    pub signature: String,
    pub trust_invalid_certs: bool,
    pub save_sent: bool,
    pub notes: Vec<String>,
}

impl AccountForm {
    pub fn from_email(email: &str) -> Self {
        Self { email: email.into(), save_sent: true, ..Self::default() }
    }

    pub fn from_account(a: &Account) -> Self {
        Self {
            id: Some(a.id.clone()),
            name: a.name.clone(),
            email: a.email.clone(),
            display_name: a.display_name.clone(),
            username: a.username.clone(),
            password: String::new(),
            imap_host: a.imap.host.clone(),
            imap_port: a.imap.port.to_string(),
            imap_security: a.imap.security,
            smtp_host: a.smtp.host.clone(),
            smtp_port: a.smtp.port.to_string(),
            smtp_security: a.smtp.security,
            signature: a.signature.clone(),
            trust_invalid_certs: a.trust_invalid_certs,
            save_sent: a.save_sent,
            notes: Vec::new(),
        }
    }

    fn params(&self) -> Value {
        let mut v = json!({
            "email": self.email,
            "name": self.name,
            "display_name": self.display_name,
            "username": self.username,
            "imap_host": self.imap_host,
            "imap_port": self.imap_port.trim().parse::<u32>().unwrap_or(0),
            "imap_security": self.imap_security.name(),
            "smtp_host": self.smtp_host,
            "smtp_port": self.smtp_port.trim().parse::<u32>().unwrap_or(0),
            "smtp_security": self.smtp_security.name(),
            "signature": self.signature,
            "trust_invalid_certs": self.trust_invalid_certs,
            "save_sent": self.save_sent,
        });
        if !self.password.is_empty() {
            v["password"] = json!(self.password);
        }
        if let Some(id) = &self.id {
            v["id"] = json!(id);
        }
        v
    }
}

#[derive(Default)]
pub struct SettingsState {
    pub editing: Option<AccountForm>,
    pub result: String,
    /// Reload the form from the saved account (after a probe changed the servers).
    pub reload: bool,
}

fn section(ui: &mut egui::Ui, t: &Tokens, text: &str) {
    ui.add_space(14.0);
    ui.label(RichText::new(text).size(16.0).strong().color(t.ui_text));
    ui.add_space(4.0);
}

fn security_combo(ui: &mut egui::Ui, id: &str, value: &mut Security) {
    egui::ComboBox::from_id_salt(id).selected_text(value.name()).width(100.0).show_ui(ui, |ui| {
        for s in Security::ALL {
            ui.selectable_value(value, s, s.name());
        }
    });
}

pub fn show(app: &mut MailApp, ui: &mut egui::Ui, t: &Tokens) {
    if app.settings.reload {
        app.settings.reload = false;
        if let Some(id) = app.settings.editing.as_ref().and_then(|f| f.id.clone())
            && let Some(a) = app.session.account(&id).cloned()
        {
            let notes = app.settings.editing.as_ref().map(|f| f.notes.clone()).unwrap_or_default();
            app.settings.editing = Some(AccountForm { notes, ..AccountForm::from_account(&a) });
        }
    }
    egui::CentralPanel::default().frame(egui::Frame::NONE.fill(t.panel)).show(ui, |ui| {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            egui::Frame::NONE.inner_margin(egui::Margin::symmetric(28, 18)).show(ui, |ui| {
                ui.set_max_width(860.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(app.tr("Settings")).size(22.0).strong().color(t.ui_text));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if !app.session.config().accounts.is_empty() && icon_text_button(ui, t, Icon::Back, app.tr("Done"), true).clicked() {
                            app.screen = Screen::Mail;
                        }
                    });
                });
                accounts_section(app, ui, t);
                if app.settings.editing.is_some() {
                    account_form(app, ui, t);
                } else {
                    preferences_section(app, ui, t);
                    assistant_section(app, ui, t);
                    shortcuts_section(app, ui, t);
                }
            });
        });
    });
}

fn accounts_section(app: &mut MailApp, ui: &mut egui::Ui, t: &Tokens) {
    section(ui, t, app.tr("Accounts"));
    if app.session.config().accounts.is_empty() {
        ui.label(RichText::new(app.tr("No account yet. Add one here, or ask the assistant (MCP) to set it up from a sentence.")).color(t.muted));
    }
    let accounts: Vec<Account> = app.session.config().accounts.clone();
    for a in accounts {
        let (unread, error, last) =
            app.session.state(&a.id).map_or((0, None, 0), |s| (s.by_role(Role::Inbox).map_or(0, |m| m.unread), s.error.clone(), s.last_sync));
        let frame = egui::Frame::NONE.fill(t.card).stroke(Stroke::new(1.0, t.border)).corner_radius(10).inner_margin(12);
        let resp = frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                avatar(ui, t, &a.name, color_for(&a.email, &a.color), 36.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.label(RichText::new(&a.name).strong().color(t.ui_text));
                    ui.label(RichText::new(format!("{} · IMAP {} · SMTP {}", a.email, a.imap.describe(), a.smtp.describe())).small().color(t.muted));
                    let when = if last > 0 { mailcraft_engine::core::message::time::long(last, app.italian()) } else { app.tr("never").to_string() };
                    match error {
                        Some(e) => ui.label(
                            RichText::new(format!("{}: {}", app.tr("Error"), app.language.tr_message(&e)))
                                .small()
                                .color(Color32::from_rgb(0xe5, 0x48, 0x4d)),
                        ),
                        None => {
                            ui.label(RichText::new(format!("{}: {when} · {} {unread}", app.tr("Last sync"), app.tr("Inbox"))).small().color(t.muted))
                        }
                    };
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(app.tr("Edit Account")).clicked() {
                        app.settings.editing = Some(AccountForm::from_account(&a));
                        app.settings.result.clear();
                    }
                    if ui.button(app.tr("Test connection")).clicked() {
                        app.run("account.test", json!({"id": a.id}));
                        app.settings.result = app.tr("Loading…").to_string();
                    }
                });
            });
        });
        let _ = resp;
        ui.add_space(6.0);
    }
    if app.settings.editing.is_none() && icon_text_button(ui, t, Icon::Plus, app.tr("Add account"), true).clicked() {
        app.settings.editing = Some(AccountForm { save_sent: true, ..AccountForm::default() });
        app.settings.result.clear();
    }
    if !app.settings.result.is_empty() {
        ui.add_space(4.0);
        ui.label(RichText::new(&app.settings.result).color(t.accent));
    }
}

fn account_form(app: &mut MailApp, ui: &mut egui::Ui, t: &Tokens) {
    let Some(mut f) = app.settings.editing.clone() else { return };
    section(ui, t, app.tr(if f.id.is_some() { "Edit Account" } else { "Add account" }));
    let frame = egui::Frame::NONE.fill(t.card).stroke(Stroke::new(1.0, t.border)).corner_radius(10).inner_margin(16);
    let mut action: Option<&str> = None;
    frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::Grid::new("account-form").num_columns(2).spacing(vec2(12.0, 8.0)).min_col_width(200.0).show(ui, |ui| {
            let row = |ui: &mut egui::Ui, label: &str, value: &mut String, password: bool| {
                ui.label(RichText::new(label).color(t.muted));
                ui.add(egui::TextEdit::singleline(value).password(password).desired_width(360.0));
                ui.end_row();
            };
            row(ui, app.tr("E-mail address"), &mut f.email, false);
            row(ui, app.tr("Password"), &mut f.password, true);
            row(ui, app.tr("Name"), &mut f.name, false);
            row(ui, app.tr("Display name"), &mut f.display_name, false);
            row(ui, app.tr("Username (if not the address)"), &mut f.username, false);
            ui.label(RichText::new(app.tr("IMAP server")).color(t.muted));
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut f.imap_host).hint_text(app.tr("Host")).desired_width(190.0));
                ui.add(egui::TextEdit::singleline(&mut f.imap_port).hint_text(app.tr("Port")).desired_width(56.0));
                security_combo(ui, "imap-sec", &mut f.imap_security);
            });
            ui.end_row();
            ui.label(RichText::new(app.tr("SMTP server")).color(t.muted));
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut f.smtp_host).hint_text(app.tr("Host")).desired_width(190.0));
                ui.add(egui::TextEdit::singleline(&mut f.smtp_port).hint_text(app.tr("Port")).desired_width(56.0));
                security_combo(ui, "smtp-sec", &mut f.smtp_security);
            });
            ui.end_row();
            ui.label(RichText::new(app.tr("Signature")).color(t.muted));
            ui.add(egui::TextEdit::multiline(&mut f.signature).desired_rows(3).desired_width(360.0));
            ui.end_row();
        });
        ui.checkbox(&mut f.save_sent, app.tr("Copy sent mail to the Sent folder"));
        ui.checkbox(&mut f.trust_invalid_certs, app.tr("Accept invalid certificates (local bridges, self-hosted servers)"));
        if !f.notes.is_empty() {
            ui.add_space(6.0);
            ui.label(RichText::new(app.tr("Provider notes")).small().strong().color(t.muted));
            for n in &f.notes {
                ui.label(RichText::new(n).small().color(t.ui_text));
            }
        }
        ui.add_space(10.0);
        ui.horizontal_wrapped(|ui| {
            if primary_button(ui, t, Some(Icon::Save), app.tr("Save account")).clicked() {
                action = Some("save");
            }
            if ui.button(app.tr("Suggest from the provider")).clicked() {
                action = Some("suggest");
            }
            if f.id.is_some() {
                if ui.button(app.tr("Find servers")).clicked() {
                    action = Some("probe");
                }
                if ui.button(app.tr("Test connection")).clicked() {
                    action = Some("test");
                }
                if ui.button(RichText::new(app.tr("Remove account")).color(Color32::from_rgb(0xe5, 0x48, 0x4d))).clicked() {
                    action = Some("remove");
                }
            }
            if ui.button(app.tr("Cancel")).clicked() {
                action = Some("cancel");
            }
        });
        ui.label(
            RichText::new(app.tr("Everything stays on this computer: the password is kept in a file only you can read.")).small().color(t.muted),
        );
    });
    match action {
        Some("suggest") => {
            if let Some(v) = app.run("account.suggest", json!({"email": f.email, "language": app.language.code()})) {
                f.imap_host = v["imap"]["host"].as_str().unwrap_or("").to_string();
                f.imap_port = v["imap"]["port"].to_string();
                f.imap_security = Security::parse(v["imap"]["security"].as_str().unwrap_or("tls")).unwrap_or_default();
                f.smtp_host = v["smtp"]["host"].as_str().unwrap_or("").to_string();
                f.smtp_port = v["smtp"]["port"].to_string();
                f.smtp_security = Security::parse(v["smtp"]["security"].as_str().unwrap_or("tls")).unwrap_or_default();
                if f.name.trim().is_empty() {
                    f.name = v["provider"]
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| mailcraft_engine::core::account::domain_of(&f.email).unwrap_or_default());
                }
                f.notes = v["notes"].as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
            }
        }
        Some("save") => {
            let cmd = if f.id.is_some() { "account.update" } else { "account.add" };
            if let Some(v) = app.run(cmd, f.params()) {
                let id = v["id"].as_str().unwrap_or_default().to_string();
                f.id = Some(id.clone());
                f.password.clear();
                app.settings.result = app.tr("Saved").to_string();
                if app.session.has_password(&id) {
                    app.run("account.test", json!({"id": id}));
                }
                app.settings.reload = true;
            }
        }
        Some("probe") => {
            if let Some(id) = &f.id {
                app.run("account.probe", json!({"id": id}));
                app.settings.result = app.tr("Loading…").to_string();
            }
        }
        Some("test") => {
            if let Some(id) = &f.id {
                if !f.password.is_empty() {
                    app.run("account.setPassword", json!({"id": id, "password": f.password}));
                }
                app.run("account.test", json!({"id": id}));
                app.settings.result = app.tr("Loading…").to_string();
            }
        }
        Some("remove") => {
            if let Some(id) = &f.id {
                app.run("account.remove", json!({"id": id}));
            }
            app.settings.editing = None;
            return;
        }
        Some("cancel") => {
            app.settings.editing = None;
            return;
        }
        _ => {}
    }
    app.settings.editing = Some(f);
}

fn preferences_section(app: &mut MailApp, ui: &mut egui::Ui, t: &Tokens) {
    section(ui, t, app.tr("Preferences"));
    let prefs = app.session.config().preferences.clone();
    let frame = egui::Frame::NONE.fill(t.card).stroke(Stroke::new(1.0, t.border)).corner_radius(10).inner_margin(16);
    frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::Grid::new("prefs").num_columns(2).spacing(vec2(12.0, 10.0)).min_col_width(240.0).show(ui, |ui| {
            ui.label(RichText::new(app.tr("Language")).color(t.muted));
            let current = if prefs.language.is_empty() {
                app.tr("System").to_string()
            } else {
                Language::parse(&prefs.language).map(|l| l.name().to_string()).unwrap_or_default()
            };
            egui::ComboBox::from_id_salt("lang").selected_text(current).show_ui(ui, |ui| {
                if ui.selectable_label(prefs.language.is_empty(), app.tr("System")).clicked() {
                    app.run("prefs.set", json!({"key": "language", "value": "system"}));
                    app.language = Language::system();
                }
                for l in Language::ALL {
                    if ui.selectable_label(prefs.language == l.code(), l.name()).clicked() {
                        app.run("prefs.set", json!({"language": l.code()}));
                        app.language = l;
                    }
                }
            });
            ui.end_row();
            ui.label(RichText::new(app.tr("Theme")).color(t.muted));
            let theme_label = match prefs.theme.as_str() {
                "dark" => app.tr("Dark"),
                "light" => app.tr("Light"),
                _ => app.tr("System"),
            };
            egui::ComboBox::from_id_salt("theme").selected_text(theme_label).show_ui(ui, |ui| {
                for (value, label) in [("system", "System"), ("light", "Light"), ("dark", "Dark")] {
                    if ui.selectable_label(prefs.theme == value, app.tr(label)).clicked() {
                        app.run("prefs.set", json!({"theme": value}));
                        app.dark = value == "dark";
                    }
                }
            });
            ui.end_row();
            ui.label(RichText::new(app.tr("Sync every (minutes, 0 = manual)")).color(t.muted));
            let mut minutes = prefs.sync_minutes;
            if ui.add(egui::DragValue::new(&mut minutes).range(0..=1440)).changed() {
                app.run("prefs.set", json!({"sync_minutes": minutes}));
            }
            ui.end_row();
            ui.label(RichText::new(app.tr("Messages per page")).color(t.muted));
            let mut page = prefs.page_size;
            if ui.add(egui::DragValue::new(&mut page).range(5..=500)).changed() {
                app.run("prefs.set", json!({"page_size": page}));
            }
            ui.end_row();
            ui.label(RichText::new(app.tr("Default account")).color(t.muted));
            let names: Vec<(String, String)> = app.session.config().accounts.iter().map(|a| (a.id.clone(), a.name.clone())).collect();
            let current = names.iter().find(|(id, _)| *id == prefs.default_account).map(|(_, n)| n.clone()).unwrap_or_else(|| "—".into());
            egui::ComboBox::from_id_salt("default-account").selected_text(current).show_ui(ui, |ui| {
                for (id, name) in names {
                    if ui.selectable_label(prefs.default_account == id, name).clicked() {
                        app.run("prefs.set", json!({"default_account": id}));
                    }
                }
            });
            ui.end_row();
        });
        for (key, label, value) in [
            ("reading_pane", "Reading pane beside the list", prefs.reading_pane),
            ("conversations", "Group messages into conversations", prefs.conversations),
            ("mark_read_on_open", "Mark messages read when opened", prefs.mark_read_on_open),
        ] {
            let mut v = value;
            if ui.checkbox(&mut v, app.tr(label)).changed() {
                app.run("prefs.set", json!({key: v}));
            }
        }
    });
}

fn assistant_section(app: &mut MailApp, ui: &mut egui::Ui, t: &Tokens) {
    section(ui, t, app.tr("Assistant (MCP)"));
    let frame = egui::Frame::NONE.fill(t.card).stroke(Stroke::new(1.0, t.border)).corner_radius(10).inner_margin(16);
    frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(40.0, 40.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 10.0, t.active);
            icons::paint(ui.painter(), rect.shrink(9.0), Icon::Robot, t.accent);
            ui.vertical(|ui| {
                ui.set_width(ui.available_width() - 20.0);
                ui.label(RichText::new(app.tr("Configure MailCraft in plain language: connect the MCP server to Claude Code, Claude Desktop or any MCP client and say, for example, “set up my Aruba mailbox info@example.it” or “dark theme, sync every 2 minutes”. The assistant can also read, triage and answer your mail.")).color(t.ui_text));
                ui.add_space(6.0);
                ui.label(RichText::new(app.tr("Command for Claude Code:")).small().color(t.muted));
                let cmd = "claude mcp add mailcraft -- mailcraft-cli mcp";
                let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width().min(420.0), 30.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, 6.0, t.panel);
                ui.painter().rect_stroke(rect, 6.0, Stroke::new(1.0, t.border), StrokeKind::Inside);
                ui.painter().text(rect.left_center() + vec2(10.0, 0.0), egui::Align2::LEFT_CENTER, cmd, egui::FontId::monospace(13.0), t.ui_text);
            });
        });
    });
}

fn shortcuts_section(app: &mut MailApp, ui: &mut egui::Ui, t: &Tokens) {
    section(ui, t, app.tr("Keyboard shortcuts"));
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), egui::Sense::hover());
        icons::paint(ui.painter(), r, Icon::Keyboard, t.muted);
        ui.label(RichText::new(app.tr("c compose · / search · j/k move · Enter open · e archive · # trash · s star · u unread · r reply · a reply all · f forward · Cmd+R sync · Esc close")).small().color(t.muted));
    });
    ui.add_space(20.0);
}
