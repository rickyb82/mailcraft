//! MailCraft's MCP server (layer L6): the Model Context Protocol over stdio (newline-delimited
//! JSON-RPC 2.0), so AI agents can configure accounts in plain language and read, triage and
//! write mail.
//!
//! Every tool is an engine command ([`Session::run_blocking`]): what the interface does, an
//! agent can do, and nothing else. Passwords are stored by the engine (a user-only file) and
//! never returned by any tool.
#![recursion_limit = "512"]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::io::{BufRead, Write};

use mailcraft_engine::core::message::time;
use mailcraft_engine::{COMMANDS, Paths, Session};
use serde_json::{Value, json};

/// MCP protocol revision this server prefers.
pub const PROTOCOL_VERSION: &str = "2025-06-18";
/// Revisions it also speaks; a client asking for another gets [`PROTOCOL_VERSION`].
pub const SUPPORTED_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
/// The accounts and their status (`resources/read`).
pub const ACCOUNTS_URI: &str = "mailcraft://accounts";
/// The current inbox as a text list (`resources/read`).
pub const INBOX_URI: &str = "mailcraft://inbox";
/// Longest request line read (bytes).
const MAX_LINE: usize = 32 * 1024 * 1024;
/// Longest message body returned by read_message.
const MAX_TEXT: usize = 200_000;

const INSTRUCTIONS: &str = "MailCraft is an IMAP/SMTP mail client. Configure it in plain language: suggest_settings guesses the servers of \
an address (Gmail, Outlook, Aruba, Libero, PEC…), add_account stores an account (the password goes to a user-only secrets file) and \
tests it, find_servers probes the usual host names when the provider is unknown, set_preferences changes language/theme/sync. \
Prompts: setup_account (configure a mailbox from a description), triage_inbox (summarise and act on unread mail), write_email, configure. \
Mail: sync, list_mailboxes, list_messages (uids), search (from: subject: is:unread has:attachment newer_than:7d), read_message, reply, \
forward, send_mail, mark, archive, trash, spam, move, save_draft, save_attachment. Never invent a password: ask the person for it. \
Before sending or deleting on their behalf, show what will be sent or removed and wait for a yes.";

type ToolResult = Result<Value, String>;

/// The server: one engine session on the user's configuration.
pub struct Server {
    session: Session,
}

fn tools() -> Value {
    let account = json!({"type": "string", "description": "Account id, address or name (default: the current account)"});
    let mailbox = json!({"type": "string", "description": "IMAP folder path (default: the inbox or the folder in view)"});
    let role = json!({"type": "string", "enum": ["inbox", "sent", "drafts", "trash", "spam", "archive"], "description": "A special folder instead of `mailbox`"});
    let uids = json!({"type": "array", "items": {"type": "integer", "minimum": 1}, "description": "Message uids from list_messages/search"});
    let uid = json!({"type": "integer", "minimum": 1, "description": "A message uid from list_messages/search"});
    let language = json!({"type": "string", "enum": ["en", "it"], "description": "Language of notes and quotes (default: the preference)"});
    let security = json!({"type": "string", "enum": ["tls", "starttls", "none"]});
    let port = json!({"type": "integer", "minimum": 1, "maximum": 65535});
    let addresses = json!({"type": "string", "description": "Recipients, comma separated: `Name <addr>` or `addr`"});
    let server_fields = json!({
        "name": {"type": "string", "description": "Display name of the account (default: the provider or the domain)"},
        "display_name": {"type": "string", "description": "The name put in From:"},
        "username": {"type": "string", "description": "Login when it is not the address"},
        "imap_host": {"type": "string"}, "imap_port": port, "imap_security": security,
        "smtp_host": {"type": "string"}, "smtp_port": port, "smtp_security": security,
        "signature": {"type": "string"},
        "color": {"type": "string", "description": "#rrggbb"},
        "trust_invalid_certs": {"type": "boolean", "description": "Accept a server certificate that does not validate (local bridges, self-hosted servers)"},
        "save_sent": {"type": "boolean", "description": "Copy sent mail to the Sent folder (off for Gmail, which does it itself)"},
    });
    let mut add_props = server_fields.clone();
    if let Some(o) = add_props.as_object_mut() {
        o.insert("email".into(), json!({"type": "string"}));
        o.insert("password".into(), json!({"type": "string", "description": "The mailbox or app password (stored locally, never returned)"}));
        o.insert(
            "test".into(),
            json!({"type": "boolean", "description": "Log in to IMAP and SMTP after saving (default true); unknown providers are probed first"}),
        );
        o.insert("language".into(), language.clone());
    }
    let mut update_props = server_fields;
    if let Some(o) = update_props.as_object_mut() {
        o.insert("id".into(), account.clone());
        o.insert("email".into(), json!({"type": "string"}));
        o.insert("password".into(), json!({"type": "string"}));
        o.insert("folders".into(), json!({"type": "object", "description": "Special folders by IMAP path", "properties": {"sent": {"type": "string"}, "drafts": {"type": "string"}, "trash": {"type": "string"}, "spam": {"type": "string"}, "archive": {"type": "string"}}}));
    }
    json!([
        {"name": "suggest_settings", "description": "IMAP/SMTP settings for an e-mail address: the provider's documented servers (Gmail, Outlook, Yahoo, iCloud, Aruba, PEC, Libero, Virgilio, Tiscali, TIM, Fastweb, GMX, Zoho, Fastmail, Proton Bridge…) or conventional guesses for an unknown domain, with notes (app passwords, OAuth, bridges).",
         "inputSchema": {"type": "object", "properties": {"email": {"type": "string"}, "language": language}, "required": ["email"]}},
        {"name": "add_account", "description": "Add (or replace) an account from an address and password; servers not given come from suggest_settings. With test=true (default) it logs in to IMAP and SMTP; for unknown domains it probes the usual host names first and keeps the working one. Returns the saved account, what was tested and the provider's notes.",
         "inputSchema": {"type": "object", "properties": add_props, "required": ["email"]}},
        {"name": "list_accounts", "description": "The configured accounts (never passwords): servers, signature, status, unread count.",
         "inputSchema": {"type": "object", "properties": {}}},
        {"name": "update_account", "description": "Change an account's name, servers, signature, folders, password… Only the fields given change.",
         "inputSchema": {"type": "object", "properties": update_props, "required": ["id"]}},
        {"name": "remove_account", "description": "Remove an account, its password and its cache.",
         "inputSchema": {"type": "object", "properties": {"id": account}, "required": ["id"]}},
        {"name": "test_account", "description": "Log in to the account's IMAP and SMTP servers and count its folders.",
         "inputSchema": {"type": "object", "properties": {"id": account}}},
        {"name": "find_servers", "description": "Try the suggested IMAP/SMTP servers for an account until one accepts the login, and save the working pair (for domains whose provider is unknown).",
         "inputSchema": {"type": "object", "properties": {"id": account}}},
        {"name": "set_password", "description": "Store or replace an account's password.",
         "inputSchema": {"type": "object", "properties": {"id": account, "password": {"type": "string"}}, "required": ["password"]}},
        {"name": "get_preferences", "description": "Interface preferences: language, theme, sync interval, page size, reading pane, conversations, mark read on open, undo send seconds, default account.",
         "inputSchema": {"type": "object", "properties": {}}},
        {"name": "set_preferences", "description": "Change preferences (only the fields given).",
         "inputSchema": {"type": "object", "properties": {
            "language": {"type": "string", "enum": ["en", "it", "system"]}, "theme": {"type": "string", "enum": ["light", "dark", "system"]},
            "sync_minutes": {"type": "integer", "minimum": 0, "maximum": 1440}, "page_size": {"type": "integer", "minimum": 5, "maximum": 500},
            "reading_pane": {"type": "boolean"}, "conversations": {"type": "boolean"}, "mark_read_on_open": {"type": "boolean"},
            "undo_send_seconds": {"type": "integer", "minimum": 0, "maximum": 30}, "default_account": account}}},
        {"name": "sync", "description": "Refresh the folders and the first page of a mailbox from the server.",
         "inputSchema": {"type": "object", "properties": {"account": account, "mailbox": mailbox, "all": {"type": "boolean", "description": "Every account"}}}},
        {"name": "list_mailboxes", "description": "The account's folders with roles (inbox, sent, drafts, trash, spam, archive) and unread/total counts.",
         "inputSchema": {"type": "object", "properties": {"account": account}}},
        {"name": "list_messages", "description": "A page of messages, newest first: uid, date, sender, subject, snippet, flags (● unread, ★ starred, 📎 attachment).",
         "inputSchema": {"type": "object", "properties": {"account": account, "mailbox": mailbox, "role": role, "page": {"type": "integer", "minimum": 0}, "unread_only": {"type": "boolean"}, "refresh": {"type": "boolean", "description": "Fetch from the server (default true)"}}}},
        {"name": "search", "description": "Search a folder: words plus from:, to:, subject:, is:unread, is:starred, has:attachment, newer_than:7d.",
         "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}, "account": account, "mailbox": mailbox, "role": role, "page": {"type": "integer", "minimum": 0}}, "required": ["query"]}},
        {"name": "read_message", "description": "The full message: headers, text body (HTML reduced to text), attachment list. Marks it read unless mark_read=false.",
         "inputSchema": {"type": "object", "properties": {"uid": uid, "account": account, "mailbox": mailbox, "mark_read": {"type": "boolean"}}, "required": ["uid"]}},
        {"name": "save_attachment", "description": "Save an attachment of an open message to a path (a directory keeps the file's name).",
         "inputSchema": {"type": "object", "properties": {"uid": uid, "index": {"type": "integer", "minimum": 0}, "path": {"type": "string"}, "account": account, "mailbox": mailbox}, "required": ["uid", "index", "path"]}},
        {"name": "send_mail", "description": "Send a new message (the account's signature is added). Show the person the text first and send only when they agree.",
         "inputSchema": {"type": "object", "properties": {"to": addresses, "cc": addresses, "bcc": addresses, "subject": {"type": "string"}, "body": {"type": "string"}, "account": account,
            "attachments": {"type": "array", "items": {"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}}}, "required": ["to", "subject", "body"]}},
        {"name": "reply", "description": "Reply to a message (quoting it) with the given text; all=true answers everyone.",
         "inputSchema": {"type": "object", "properties": {"uid": uid, "body": {"type": "string"}, "all": {"type": "boolean"}, "quote": {"type": "boolean"}, "account": account, "mailbox": mailbox}, "required": ["uid", "body"]}},
        {"name": "forward", "description": "Forward a message (attachments included) to recipients, with an optional note.",
         "inputSchema": {"type": "object", "properties": {"uid": uid, "to": addresses, "body": {"type": "string"}, "account": account, "mailbox": mailbox}, "required": ["uid", "to"]}},
        {"name": "mark", "description": "Mark messages read/unread and/or starred/unstarred.",
         "inputSchema": {"type": "object", "properties": {"uids": uids, "read": {"type": "boolean"}, "starred": {"type": "boolean"}, "account": account, "mailbox": mailbox}, "required": ["uids"]}},
        {"name": "archive", "description": "Archive messages (move them out of the inbox).",
         "inputSchema": {"type": "object", "properties": {"uids": uids, "account": account, "mailbox": mailbox}, "required": ["uids"]}},
        {"name": "trash", "description": "Move messages to the trash (from the trash: delete for good).",
         "inputSchema": {"type": "object", "properties": {"uids": uids, "account": account, "mailbox": mailbox}, "required": ["uids"]}},
        {"name": "spam", "description": "Move messages to the spam folder.",
         "inputSchema": {"type": "object", "properties": {"uids": uids, "account": account, "mailbox": mailbox}, "required": ["uids"]}},
        {"name": "move", "description": "Move messages to a folder (IMAP path from list_mailboxes).",
         "inputSchema": {"type": "object", "properties": {"uids": uids, "to": {"type": "string"}, "account": account, "mailbox": mailbox}, "required": ["uids", "to"]}},
        {"name": "save_draft", "description": "Save a draft in the Drafts folder (draft_uid replaces an earlier one).",
         "inputSchema": {"type": "object", "properties": {"to": addresses, "cc": addresses, "bcc": addresses, "subject": {"type": "string"}, "body": {"type": "string"}, "draft_uid": uid, "account": account}}},
        {"name": "create_folder", "description": "Create an IMAP folder.",
         "inputSchema": {"type": "object", "properties": {"name": {"type": "string"}, "account": account}, "required": ["name"]}},
        {"name": "list_commands", "description": "Every engine command with its parameters.",
         "inputSchema": {"type": "object", "properties": {}}},
        {"name": "run_command", "description": "Run any engine command by id (see list_commands).",
         "inputSchema": {"type": "object", "properties": {"command": {"type": "string"}, "params": {"type": "object"}}, "required": ["command"]}},
    ])
}

fn prompts() -> Value {
    json!([
        {"name": "setup_account", "title": "Set up a mailbox", "description": "Configure an e-mail account from a plain-language description (address, provider, password, preferences).",
         "arguments": [
            {"name": "description", "description": "E.g. 'la mia mail Aruba info@sirius.it, password …, firma Riccardo'", "required": true},
            {"name": "language", "description": "en or it", "required": false}]},
        {"name": "triage_inbox", "title": "Triage the inbox", "description": "Summarise unread mail, group it by urgency and propose replies, archiving and deletions.",
         "arguments": [
            {"name": "account", "description": "Account id, address or name", "required": false},
            {"name": "focus", "description": "What matters today (a client, a project, invoices…)", "required": false},
            {"name": "language", "description": "en or it", "required": false}]},
        {"name": "write_email", "title": "Write an e-mail", "description": "Draft a message from a brief, show it, then send it or save it as a draft.",
         "arguments": [
            {"name": "to", "description": "Recipients", "required": true},
            {"name": "about", "description": "What the message should say", "required": true},
            {"name": "tone", "description": "formal, friendly, short…", "required": false},
            {"name": "language", "description": "en or it", "required": false}]},
        {"name": "configure", "title": "Change settings", "description": "Apply a settings request in plain language: theme, language, sync interval, signature, folders, servers.",
         "arguments": [{"name": "request", "description": "E.g. 'tema scuro, sincronizza ogni 2 minuti, firma: Riccardo Burrai'", "required": true}]},
    ])
}

fn prompt_text(name: &str, args: &Value) -> Option<(&'static str, String)> {
    let a = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or_default().trim().to_string();
    let lang = a("language");
    let answer_in = if lang.is_empty() { "the person's language".to_string() } else { lang.clone() };
    match name {
        "setup_account" => {
            let d = a("description");
            if d.is_empty() {
                return None;
            }
            Some((
                "Configure an e-mail account from a description",
                format!(
                    "Set up an e-mail account in MailCraft from this description (answer in {answer_in}):\n\n{d}\n\n\
Work like this:\n\
1. Find the address in the description. If it is missing, ask for it and stop.\n\
2. Call suggest_settings with the address (language {l}). Read the provider, the servers and the notes.\n\
3. If the description names servers, ports or security, they win over the suggestion. Keep the person's words for name, display name and signature.\n\
4. If the password is in the description, use it. If not, ask for it (an app password where the notes say so) and stop until it arrives. Never guess a password.\n\
5. Call add_account with email, password and anything the person specified (test=true). For an unknown domain the tool probes the usual servers; if it fails, show the tried servers and ask for the IMAP and SMTP hosts.\n\
6. If the test fails with an authentication error, repeat the notes (app password, IMAP enabled, OAuth-only providers) and ask whether to retry with another password.\n\
7. Report in {answer_in}: the account saved, the servers, what was tested, the unread count, and the preferences you changed (set_preferences) if the description asked for a language, theme or sync interval.",
                    l = if lang.is_empty() { "it if the description is Italian, else en" } else { &lang }
                ),
            ))
        }
        "triage_inbox" => {
            let account = a("account");
            let focus = a("focus");
            Some((
                "Summarise and act on unread mail",
                format!(
                    "Triage the inbox{} in MailCraft (answer in {answer_in}){}.\n\n\
1. Call sync, then list_messages with unread_only=true (and search is:unread newer_than:7d if the page is short).\n\
2. Call read_message on each unread message that looks important (skip obvious newsletters and notifications after reading their snippet).\n\
3. Group the messages: needs a reply, needs an action, informational, noise. For each important one give sender, subject, a one-line summary and the uid.\n\
4. Propose what to do: replies (draft them in full), archive (uids), trash (uids), star. Do not send, archive or delete anything until the person says so; then use reply, archive, trash, mark.\n\
5. End with the list of uids left unread.",
                    if account.is_empty() { String::new() } else { format!(" of account `{account}`") },
                    if focus.is_empty() { String::new() } else { format!(". Today's focus: {focus}") },
                ),
            ))
        }
        "write_email" => {
            let to = a("to");
            let about = a("about");
            if to.is_empty() || about.is_empty() {
                return None;
            }
            let tone = a("tone");
            Some((
                "Draft, show and send an e-mail",
                format!(
                    "Write an e-mail in MailCraft to: {to}\nAbout: {about}\n{}\n\
Write it in {answer_in}: a clear subject, a short greeting, the message, a closing line (the account's signature is added automatically; do not write one). \
Show the person the recipients, subject and full text and ask whether to send, change or save as a draft. Only then call send_mail (or save_draft).",
                    if tone.is_empty() { String::new() } else { format!("Tone: {tone}") },
                ),
            ))
        }
        "configure" => {
            let r = a("request");
            if r.is_empty() {
                return None;
            }
            Some((
                "Apply a settings request",
                format!(
                    "Apply this MailCraft settings request:\n\n{r}\n\n\
Map it to tools: language/theme/sync interval/page size/reading pane/conversations → set_preferences; signature, display name, servers, ports, security, special folders, password → update_account \
(call list_accounts first to find the account id when several exist; ask which one if it is ambiguous); a new mailbox → the setup_account flow (suggest_settings, add_account). \
Confirm what changed, in the person's language."
                ),
            ))
        }
        _ => None,
    }
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

fn flag(e: &Value) -> String {
    let mut f = String::new();
    if e["unread"].as_bool().unwrap_or(false) {
        f.push('●');
    }
    if e["starred"].as_bool().unwrap_or(false) {
        f.push('★');
    }
    if e["has_attachments"].as_bool().unwrap_or(false) {
        f.push('📎');
    }
    if e["answered"].as_bool().unwrap_or(false) {
        f.push('↩');
    }
    f
}

/// One line per message, for lists.
fn list_text(envelopes: &[Value], italian: bool) -> String {
    if envelopes.is_empty() {
        return if italian { "Nessun messaggio.".into() } else { "No messages.".into() };
    }
    let now = time::now();
    envelopes
        .iter()
        .map(|e| {
            let from = e["from"].as_array().and_then(|a| a.first()).and_then(Value::as_str).unwrap_or("");
            let from = from.split(" <").next().unwrap_or(from);
            let date = time::short(e["date"].as_i64().unwrap_or(0), now, italian);
            let snippet = e["snippet"].as_str().unwrap_or("");
            let snippet =
                if snippet.chars().count() > 90 { format!("{}…", snippet.chars().take(90).collect::<String>()) } else { snippet.to_string() };
            format!("[{}] {:<3} {:>9}  {} — {} — {}", e["uid"], flag(e), date, from, e["subject"].as_str().unwrap_or("(no subject)"), snippet)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

impl Default for Server {
    fn default() -> Self {
        Self::new(Session::new(Box::new(|_| Box::new(mailcraft_engine::core::mock::MockBackend::new())), None))
    }
}

impl Server {
    pub fn new(session: Session) -> Self {
        Self { session }
    }

    /// A server on the user's configuration with the given transport factory.
    pub fn with_factory(factory: mailcraft_engine::core::backend::BackendFactory, paths: Option<Paths>) -> Self {
        Self::new(Session::new(factory, paths))
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    fn run(&mut self, command: &str, params: Value) -> ToolResult {
        self.session.run_blocking(command, params).map_err(|e| e.to_string())
    }

    fn italian(&self, args: &Value) -> bool {
        str_arg(args, "language").map_or(self.session.italian(), |l| l.eq_ignore_ascii_case("it"))
    }

    fn pass_through(&self, args: &Value, keys: &[&str]) -> Value {
        let mut v = json!({});
        for k in keys {
            if let Some(x) = args.get(k) {
                v[k] = x.clone();
            }
        }
        v
    }

    fn call(&mut self, name: &str, args: &Value) -> ToolResult {
        let italian = self.italian(args);
        let account_keys = ["account", "mailbox", "role"];
        match name {
            "suggest_settings" => self.run("account.suggest", self.pass_through(args, &["email", "language"])),
            "add_account" => {
                let keys = [
                    "email",
                    "password",
                    "name",
                    "display_name",
                    "username",
                    "imap_host",
                    "imap_port",
                    "imap_security",
                    "smtp_host",
                    "smtp_port",
                    "smtp_security",
                    "signature",
                    "color",
                    "trust_invalid_certs",
                    "save_sent",
                    "language",
                ];
                let added = self.run("account.add", self.pass_through(args, &keys))?;
                let id = added["id"].as_str().unwrap_or_default().to_string();
                let mut out =
                    json!({"account": added["account"], "notes": added["suggestion"]["notes"], "provider": added["suggestion"]["provider"]});
                let test = args.get("test").and_then(Value::as_bool).unwrap_or(true);
                if !test {
                    return Ok(out);
                }
                if str_arg(args, "password").is_none() {
                    out["test"] = json!("skipped: no password given (set it with set_password, then test_account)");
                    return Ok(out);
                }
                let explicit = str_arg(args, "imap_host").is_some() || str_arg(args, "smtp_host").is_some();
                if added["suggestion"]["confidence"] == "guess" && !explicit {
                    match self.run("account.probe", json!({"id": id})) {
                        Ok(p) => out["probe"] = p,
                        Err(e) => {
                            out["probe"] = json!(format!("failed: {e}"));
                            out["test"] = json!(
                                "not run: no server accepted the login. Ask for the IMAP and SMTP hosts and call update_account, then test_account."
                            );
                            return Ok(out);
                        }
                    }
                }
                match self.run("account.test", json!({"id": id})) {
                    Ok(t) => out["test"] = t,
                    Err(e) => out["test"] = json!(format!("failed: {e}")),
                }
                if let Ok(a) = self.run("account.list", json!({})) {
                    out["account"] =
                        a["accounts"].as_array().and_then(|l| l.iter().find(|x| x["id"] == id).cloned()).unwrap_or(out["account"].clone());
                }
                Ok(out)
            }
            "list_accounts" => self.run("account.list", json!({})),
            "update_account" => {
                let keys = [
                    "id",
                    "email",
                    "password",
                    "name",
                    "display_name",
                    "username",
                    "imap_host",
                    "imap_port",
                    "imap_security",
                    "smtp_host",
                    "smtp_port",
                    "smtp_security",
                    "signature",
                    "color",
                    "trust_invalid_certs",
                    "save_sent",
                    "folders",
                ];
                self.run("account.update", self.pass_through(args, &keys))
            }
            "remove_account" => self.run("account.remove", self.pass_through(args, &["id"])),
            "test_account" => self.run("account.test", self.pass_through(args, &["id"])),
            "find_servers" => self.run("account.probe", self.pass_through(args, &["id"])),
            "set_password" => self.run("account.setPassword", self.pass_through(args, &["id", "password"])),
            "get_preferences" => self.run("prefs.get", json!({})),
            "set_preferences" => self.run("prefs.set", args.clone()),
            "sync" => {
                let v = self.run("mail.sync", self.pass_through(args, &["account", "mailbox", "all"]))?;
                let boxes = self.run("mail.mailboxes", self.pass_through(args, &["account"]))?;
                Ok(
                    json!({"synced": true, "mailboxes": boxes["mailboxes"], "first_page": list_text(v["envelopes"].as_array().map(Vec::as_slice).unwrap_or_default(), italian)}),
                )
            }
            "list_mailboxes" => {
                if self.session.current().is_some_and(|s| s.mailboxes.is_empty()) {
                    let _ = self.run("mail.sync", self.pass_through(args, &["account"]));
                }
                let v = self.run("mail.mailboxes", self.pass_through(args, &["account", "language"]))?;
                let lines: Vec<String> = v["mailboxes"]
                    .as_array()
                    .map(|l| {
                        l.iter()
                            .map(|m| {
                                format!(
                                    "{} [{}] unread {} / {}",
                                    m["path"].as_str().unwrap_or(""),
                                    m["role"].as_str().unwrap_or(""),
                                    m["unread"],
                                    m["total"]
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                Ok(json!({"text": lines.join("\n"), "mailboxes": v["mailboxes"]}))
            }
            "list_messages" => {
                let mut prm = self.pass_through(args, &["account", "mailbox", "role", "page"]);
                prm["refresh"] = json!(args.get("refresh").and_then(Value::as_bool).unwrap_or(true));
                let v = self.run("mail.list", prm)?;
                let mut list: Vec<Value> = v["envelopes"].as_array().cloned().unwrap_or_default();
                if args.get("unread_only").and_then(Value::as_bool).unwrap_or(false) {
                    list.retain(|e| e["unread"].as_bool().unwrap_or(false));
                }
                Ok(json!({"text": list_text(&list, italian), "mailbox": v["mailbox"], "count": list.len(), "messages": list}))
            }
            "search" => {
                let v = self.run("mail.search", self.pass_through(args, &["query", "account", "mailbox", "role", "page"]))?;
                let list: Vec<Value> = v["envelopes"].as_array().cloned().unwrap_or_default();
                Ok(json!({"text": list_text(&list, italian), "count": list.len(), "messages": list}))
            }
            "read_message" => {
                let v = self.run("mail.open", self.pass_through(args, &["uid", "account", "mailbox", "mark_read"]))?;
                let text = v["text"].as_str().unwrap_or_default();
                let text = mailcraft_engine::core::clip(text, MAX_TEXT);
                let header = format!(
                    "From: {}\nTo: {}\n{}Date: {}\nSubject: {}\n{}",
                    v["from"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default(),
                    v["to"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default(),
                    v["cc"]
                        .as_array()
                        .filter(|a| !a.is_empty())
                        .map(|a| format!("Cc: {}\n", a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")))
                        .unwrap_or_default(),
                    time::long(v["date"].as_i64().unwrap_or(0), italian),
                    v["subject"].as_str().unwrap_or(""),
                    v["attachments"]
                        .as_array()
                        .filter(|a| !a.is_empty())
                        .map(|a| format!(
                            "Attachments: {}\n",
                            a.iter()
                                .map(|x| format!(
                                    "[{}] {} ({}, {} bytes)",
                                    x["index"],
                                    x["name"].as_str().unwrap_or(""),
                                    x["mime"].as_str().unwrap_or(""),
                                    x["size"]
                                ))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ))
                        .unwrap_or_default(),
                );
                let mut out = v.clone();
                out["text"] = json!(format!("{header}\n{text}"));
                if let Some(o) = out.as_object_mut() {
                    o.remove("html");
                }
                Ok(out)
            }
            "save_attachment" => self.run("mail.attachment", self.pass_through(args, &["uid", "index", "path", "account", "mailbox"])),
            "send_mail" => self.run("mail.send", self.pass_through(args, &["to", "cc", "bcc", "subject", "body", "account", "attachments"])),
            "reply" => self.run("mail.reply", self.pass_through(args, &["uid", "body", "all", "quote", "account", "mailbox", "language"])),
            "forward" => self.run("mail.forward", self.pass_through(args, &["uid", "to", "body", "account", "mailbox", "language"])),
            "mark" => {
                let mut out = json!({});
                if let Some(read) = args.get("read").and_then(Value::as_bool) {
                    let mut prm = self.pass_through(args, &account_keys);
                    prm["uids"] = args["uids"].clone();
                    prm["read"] = json!(read);
                    out["read"] = self.run("mail.markRead", prm)?;
                }
                if let Some(starred) = args.get("starred").and_then(Value::as_bool) {
                    let mut prm = self.pass_through(args, &account_keys);
                    prm["uids"] = args["uids"].clone();
                    prm["starred"] = json!(starred);
                    out["starred"] = self.run("mail.star", prm)?;
                }
                if out.as_object().is_some_and(|o| o.is_empty()) {
                    return Err("pass `read` and/or `starred`".into());
                }
                Ok(out)
            }
            "archive" | "trash" | "spam" => {
                let mut prm = self.pass_through(args, &account_keys);
                prm["uids"] = args["uids"].clone();
                self.run(&format!("mail.{name}"), prm)
            }
            "move" => {
                let mut prm = self.pass_through(args, &account_keys);
                prm["uids"] = args["uids"].clone();
                prm["to"] = args["to"].clone();
                self.run("mail.move", prm)
            }
            "save_draft" => self.run("draft.save", self.pass_through(args, &["to", "cc", "bcc", "subject", "body", "draft_uid", "account"])),
            "create_folder" => self.run("mailbox.create", self.pass_through(args, &["name", "account"])),
            "list_commands" => Ok(
                json!({"commands": COMMANDS.iter().map(|c| json!({"id": c.id, "label": c.label, "menu": c.menu, "shortcut": c.shortcut, "params": c.params, "network": c.network})).collect::<Vec<_>>()}),
            ),
            "run_command" => {
                let command = str_arg(args, "command").ok_or("`command` is required")?.to_string();
                let params = args.get("params").cloned().unwrap_or_else(|| json!({}));
                self.run(&command, params)
            }
            _ => Err(format!("unknown tool {:?}", name.chars().take(40).collect::<String>())),
        }
    }

    fn accounts_text(&self) -> String {
        self.session
            .config()
            .accounts
            .iter()
            .map(|a| {
                let st = self.session.state(&a.id);
                format!(
                    "{} — {} <{}> — IMAP {} — SMTP {} — unread {}{}",
                    a.id,
                    a.name,
                    a.email,
                    a.imap.describe(),
                    a.smtp.describe(),
                    st.and_then(|s| s.by_role(mailcraft_engine::core::mailbox::Role::Inbox).map(|m| m.unread)).unwrap_or(0),
                    st.and_then(|s| s.error.as_ref()).map(|e| format!(" — error: {e}")).unwrap_or_default()
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Handles one JSON-RPC message; `None` for notifications.
    pub fn handle(&mut self, msg: &Value) -> Option<Value> {
        let id = msg.get("id").cloned()?;
        let method = msg.get("method").and_then(Value::as_str).unwrap_or_default();
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let result = match method {
            "initialize" => Ok(json!({
                "protocolVersion": params.get("protocolVersion").and_then(Value::as_str).filter(|v| SUPPORTED_VERSIONS.contains(v)).unwrap_or(PROTOCOL_VERSION),
                "capabilities": {"tools": {"listChanged": false}, "resources": {"listChanged": false, "subscribe": false}, "prompts": {"listChanged": false}},
                "serverInfo": {"name": "mailcraft", "title": "MailCraft", "version": env!("CARGO_PKG_VERSION")},
                "instructions": INSTRUCTIONS,
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": tools()})),
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or_default();
                let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
                Ok(match self.call(name, &args) {
                    Ok(v) => {
                        let text = match v.get("text").and_then(Value::as_str) {
                            Some(t) => t.to_string(),
                            None => serde_json::to_string_pretty(&v).unwrap_or_default(),
                        };
                        let structured = if v.is_object() { v } else { json!({"result": v}) };
                        json!({"content": [{"type": "text", "text": text}], "structuredContent": structured, "isError": false})
                    }
                    Err(e) => json!({"content": [{"type": "text", "text": e}], "isError": true}),
                })
            }
            "prompts/list" => Ok(json!({"prompts": prompts()})),
            "prompts/get" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or_default();
                let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
                match prompt_text(name, &args) {
                    Some((description, text)) => {
                        Ok(json!({"description": description, "messages": [{"role": "user", "content": {"type": "text", "text": text}}]}))
                    }
                    None => Err((-32602, format!("unknown prompt or missing arguments: {:?}", name.chars().take(40).collect::<String>()))),
                }
            }
            "resources/list" => Ok(json!({"resources": [
                {"uri": ACCOUNTS_URI, "name": "accounts", "title": "Accounts", "description": "Configured accounts, servers and status", "mimeType": "text/plain"},
                {"uri": INBOX_URI, "name": "inbox", "title": "Inbox", "description": "The current inbox, one line per message", "mimeType": "text/plain"},
            ]})),
            "resources/templates/list" => Ok(json!({"resourceTemplates": []})),
            "resources/read" => match params.get("uri").and_then(Value::as_str) {
                Some(ACCOUNTS_URI) => Ok(json!({"contents": [{"uri": ACCOUNTS_URI, "mimeType": "text/plain", "text": self.accounts_text()}]})),
                Some(INBOX_URI) => {
                    let italian = self.session.italian();
                    let list: Vec<Value> = self
                        .session
                        .current()
                        .map(|s| s.list("INBOX").iter().take(50).map(mailcraft_engine::envelope_json).collect())
                        .unwrap_or_default();
                    Ok(json!({"contents": [{"uri": INBOX_URI, "mimeType": "text/plain", "text": list_text(&list, italian)}]}))
                }
                _ => Err((-32002, "resource not found".to_string())),
            },
            _ => Err((-32601, format!("method not found: {}", method.chars().take(64).collect::<String>()))),
        };
        Some(match result {
            Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
            Err((code, message)) => json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}),
        })
    }

    /// Serves until `input` ends: one JSON-RPC message per line in, one response per line out.
    pub fn serve(&mut self, input: impl BufRead, mut output: impl Write) -> std::io::Result<()> {
        for line in input.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let reply = if line.len() > MAX_LINE {
                Some(json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32600, "message": "request too large"}}))
            } else {
                match serde_json::from_str::<Value>(&line) {
                    Ok(Value::Array(batch)) => {
                        let replies: Vec<Value> = batch.iter().filter_map(|m| self.handle(m)).collect();
                        (!replies.is_empty()).then_some(Value::Array(replies))
                    }
                    Ok(msg) => self.handle(&msg),
                    Err(e) => Some(json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": format!("parse error: {e}")}})),
                }
            };
            if let Some(r) = reply {
                writeln!(output, "{r}")?;
                output.flush()?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mailcraft_engine::core::mock::MockBackend;

    fn server(mock: &MockBackend) -> Server {
        let m = mock.clone();
        Server::new(Session::new(Box::new(move |_| Box::new(m.clone())), None))
    }

    fn call(s: &mut Server, name: &str, args: Value) -> Value {
        let r = s.handle(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": args}})).unwrap();
        assert_eq!(r["result"]["isError"], false, "{name}: {r}");
        r["result"]["structuredContent"].clone()
    }

    fn fails(s: &mut Server, name: &str, args: Value) -> String {
        let r = s.handle(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": args}})).unwrap();
        assert_eq!(r["result"]["isError"], true, "{name}: {r}");
        r["result"]["content"][0]["text"].as_str().unwrap().to_string()
    }

    #[test]
    fn handshake_tools_prompts_and_resources() {
        let mut s = Server::default();
        let init = s.handle(&json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}})).unwrap();
        assert_eq!(init["result"]["serverInfo"]["name"], "mailcraft");
        assert_eq!(init["result"]["protocolVersion"], "2024-11-05");
        assert!(s.handle(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).is_none());
        let tools = s.handle(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})).unwrap();
        let names: Vec<&str> = tools["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.len() >= 28, "{names:?}");
        for t in tools["result"]["tools"].as_array().unwrap() {
            assert!(t["inputSchema"]["type"] == "object", "{}", t["name"]);
        }
        let prompts = s.handle(&json!({"jsonrpc": "2.0", "id": 2, "method": "prompts/list"})).unwrap();
        assert_eq!(prompts["result"]["prompts"].as_array().unwrap().len(), 4);
        let p = s.handle(&json!({"jsonrpc": "2.0", "id": 3, "method": "prompts/get", "params": {"name": "setup_account", "arguments": {"description": "la mia mail aruba info@sirius-example.it", "language": "it"}}})).unwrap();
        assert!(p["result"]["messages"][0]["content"]["text"].as_str().unwrap().contains("suggest_settings"));
        let bad = s.handle(&json!({"jsonrpc": "2.0", "id": 4, "method": "prompts/get", "params": {"name": "write_email", "arguments": {}}})).unwrap();
        assert_eq!(bad["error"]["code"], -32602);
        let res = s.handle(&json!({"jsonrpc": "2.0", "id": 5, "method": "resources/read", "params": {"uri": ACCOUNTS_URI}})).unwrap();
        assert!(res["result"]["contents"][0]["text"].is_string());
        assert!(s.handle(&json!({"jsonrpc": "2.0", "id": 6, "method": "nope"})).unwrap()["error"]["code"] == -32601);
    }

    #[test]
    fn an_agent_configures_an_account_and_works_the_inbox() {
        let mock = MockBackend::demo().require_password("app-pw");
        let mut s = server(&mock);
        let sg = call(&mut s, "suggest_settings", json!({"email": "riccardo@gmail.com", "language": "it"}));
        assert_eq!(sg["provider"], "Gmail");
        assert!(sg["notes"][0].as_str().unwrap().contains("password per le app"));
        // Wrong password: the account is saved, the test reports the failure.
        let v = call(&mut s, "add_account", json!({"email": "riccardo@gmail.com", "password": "nope", "name": "Personale"}));
        assert!(v["test"].as_str().unwrap().starts_with("failed"), "{v}");
        assert!(v["account"]["has_password"] == true);
        call(&mut s, "set_password", json!({"id": "riccardo-gmail-com", "password": "app-pw"}));
        let t = call(&mut s, "test_account", json!({"id": "riccardo-gmail-com"}));
        assert_eq!(t["ok"], true);
        // Unknown domain: probed, then tested.
        let v = call(&mut s, "add_account", json!({"email": "mario@sirius-example.it", "password": "app-pw", "signature": "Mario"}));
        assert_eq!(v["probe"]["imap"], "imap.sirius-example.it:993 (tls)");
        assert_eq!(v["test"]["ok"], true);
        let accounts = call(&mut s, "list_accounts", json!({}));
        assert_eq!(accounts["accounts"].as_array().unwrap().len(), 2);
        assert!(!accounts.to_string().contains("app-pw"), "passwords never leave");
        call(&mut s, "set_preferences", json!({"theme": "dark", "language": "it", "sync_minutes": 3}));
        assert_eq!(call(&mut s, "get_preferences", json!({}))["preferences"]["theme"], "dark");
        assert!(fails(&mut s, "set_preferences", json!({"theme": "neon"})).contains("theme"));
        // Mail.
        let v = call(&mut s, "sync", json!({"account": "riccardo-gmail-com"}));
        assert!(v["first_page"].as_str().unwrap().contains("Riunione"));
        let boxes = call(&mut s, "list_mailboxes", json!({"account": "riccardo-gmail-com"}));
        assert!(boxes["text"].as_str().unwrap().contains("INBOX [inbox]"));
        let list = call(&mut s, "list_messages", json!({"account": "riccardo-gmail-com", "unread_only": true}));
        assert_eq!(list["count"], 4);
        let uid = list["messages"][0]["uid"].as_u64().unwrap();
        let m = call(&mut s, "read_message", json!({"uid": uid, "account": "riccardo-gmail-com"}));
        let text = m["text"].as_str().unwrap();
        assert!(text.starts_with("From: ") && text.contains("Subject: ") && text.contains("lunedì"), "{text}");
        assert!(m.get("html").is_none());
        let found = call(&mut s, "search", json!({"query": "fattura has:attachment", "account": "riccardo-gmail-com"}));
        assert_eq!(found["count"], 1);
        let r = call(&mut s, "reply", json!({"uid": uid, "body": "Va bene per lunedì.", "account": "riccardo-gmail-com"}));
        assert_eq!(r["sent"], true);
        assert_eq!(mock.sent().len(), 1);
        call(&mut s, "mark", json!({"uids": [uid], "starred": true, "account": "riccardo-gmail-com"}));
        assert!(mock.flags_of("INBOX", uid as u32).unwrap().flagged);
        call(&mut s, "archive", json!({"uids": [uid], "account": "riccardo-gmail-com"}));
        assert_eq!(mock.count("Archive"), 1);
        let sent = call(&mut s, "send_mail", json!({"to": "luca@example.com", "subject": "Ciao", "body": "Test", "account": "riccardo-gmail-com"}));
        assert_eq!(sent["sent"], true);
        assert!(fails(&mut s, "send_mail", json!({"to": "", "subject": "x", "body": "y"})).contains("to"));
        assert!(fails(&mut s, "mark", json!({"uids": [1]})).contains("read"));
        call(&mut s, "remove_account", json!({"id": "mario-sirius-example-it"}));
        assert_eq!(call(&mut s, "list_accounts", json!({}))["accounts"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn serve_speaks_line_delimited_json_rpc() {
        let mut s = Server::default();
        let input = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\nnot json\n\n[{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}]\n";
        let mut out = Vec::new();
        s.serve(&input[..], &mut out).unwrap();
        let lines: Vec<&str> = std::str::from_utf8(&out).unwrap().lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].contains("\"id\":1"));
        assert!(lines[1].contains("parse error"));
        assert!(lines[2].starts_with('['));
    }
}
