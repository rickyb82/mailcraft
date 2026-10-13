//! `mailcraft-cli serve`: the server behind the web app. It keeps the engine session (accounts,
//! connections, cache) and speaks a tiny HTTP API: `POST /api/cmd` runs a command,
//! `GET /api/snapshot?since=N` returns the state when it changed, `GET /api/health`. It also
//! serves the web app's files from a directory when one is given.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mailcraft_engine::core::account::slug;
use mailcraft_engine::{Paths, Session};
use serde_json::{Value, json};

pub struct Options {
    pub bind: String,
    /// The service token: required beyond localhost; in multi-user mode it identifies the
    /// framework (which names the user with `X-Mailcraft-User`) and mints user tokens.
    pub token: String,
    pub static_dir: Option<PathBuf>,
    pub open: bool,
    /// Multi-user mode: one session per user under this directory.
    pub users_dir: Option<PathBuf>,
    /// Sessions on the built-in demo mailbox instead of real servers.
    pub demo: bool,
    /// Language preference applied to new sessions (`en`, `it`).
    pub lang: Option<String>,
}

type Shared = Arc<Mutex<Session>>;

/// Everything the request handlers share.
struct Hub {
    opts: Options,
    /// Single-user mode: the one session.
    single: Option<Shared>,
    /// Multi-user mode: sessions by user id, made on first use.
    sessions: Mutex<HashMap<String, Shared>>,
    /// User tokens (token → user), persisted in `tokens.json` under the users directory.
    tokens: Mutex<HashMap<String, String>>,
}

impl Hub {
    fn session_for(&self, user: &str) -> Result<Shared, String> {
        if let Some(s) = &self.single {
            return Ok(s.clone());
        }
        let user = slug(user);
        if user.is_empty() {
            return Err("missing user".into());
        }
        let mut map = self.sessions.lock().map_err(|_| "sessions unavailable".to_string())?;
        if let Some(s) = map.get(&user) {
            return Ok(s.clone());
        }
        if map.len() >= 2_000 {
            return Err("too many users loaded".into());
        }
        let dir = self.opts.users_dir.clone().ok_or("no users directory")?.join(&user);
        let session = if self.opts.demo {
            Session::demo()
        } else {
            let mut s = Session::new(mailcraft_net::factory(), Some(Paths::under(&dir)));
            if let Some(l) = &self.opts.lang
                && s.config().preferences.language.is_empty()
            {
                let _ = s.execute("prefs.set", &json!({"language": l}));
            }
            s
        };
        let shared = Arc::new(Mutex::new(session));
        map.insert(user, shared.clone());
        Ok(shared)
    }

    fn all_sessions(&self) -> Vec<Shared> {
        let mut out: Vec<Shared> = self.single.iter().cloned().collect();
        if let Ok(map) = self.sessions.lock() {
            out.extend(map.values().cloned());
        }
        out
    }

    fn tokens_path(&self) -> Option<PathBuf> {
        self.opts.users_dir.as_ref().map(|d| d.join("tokens.json"))
    }

    fn load_tokens(&self) {
        if let Some(p) = self.tokens_path()
            && let Ok(text) = std::fs::read_to_string(&p)
            && let Ok(map) = serde_json::from_str::<HashMap<String, String>>(&text)
            && let Ok(mut t) = self.tokens.lock()
        {
            *t = map;
        }
    }

    fn save_tokens(&self) {
        if let (Some(p), Ok(t)) = (self.tokens_path(), self.tokens.lock()) {
            let text = serde_json::to_string_pretty(&*t).unwrap_or_else(|_| "{}".into());
            let _ = mailcraft_engine::core::account::write_private(&p, text.as_bytes());
        }
    }

    fn mint_token(&self, user: &str) -> Result<String, String> {
        let token = random_token();
        self.tokens.lock().map_err(|_| "tokens unavailable".to_string())?.insert(token.clone(), slug(user));
        self.save_tokens();
        Ok(token)
    }

    fn revoke(&self, user: &str) -> usize {
        let user = slug(user);
        let n = self
            .tokens
            .lock()
            .map(|mut t| {
                let before = t.len();
                t.retain(|_, u| *u != user);
                before - t.len()
            })
            .unwrap_or(0);
        self.save_tokens();
        n
    }

    fn user_of_token(&self, token: &str) -> Option<String> {
        self.tokens.lock().ok()?.get(token).cloned()
    }

    fn users(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        if let Some(dir) = &self.opts.users_dir
            && let Ok(entries) = std::fs::read_dir(dir)
        {
            for e in entries.flatten().filter(|e| e.path().is_dir()) {
                out.push(e.file_name().to_string_lossy().into_owned());
            }
        }
        if let Ok(map) = self.sessions.lock() {
            for u in map.keys() {
                if !out.contains(u) {
                    out.push(u.clone());
                }
            }
        }
        out.sort();
        out
    }
}

/// 32 random bytes as hex, from the OS when it has `/dev/urandom`, else a hash mix.
fn random_token() -> String {
    let mut bytes = [0u8; 32];
    let from_os = std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut bytes)).is_ok();
    if !from_os {
        use std::hash::{BuildHasher, Hasher};
        for (i, chunk) in bytes.chunks_mut(8).enumerate() {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
            h.write_u32(std::process::id());
            h.write_usize(i);
            let v = h.finish().to_le_bytes();
            chunk.copy_from_slice(&v[..chunk.len()]);
        }
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

const MAX_BODY: usize = 64 * 1024 * 1024;

struct Request {
    method: String,
    path: String,
    query: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    if reader.read_line(&mut line).ok()? == 0 {
        return None;
    }
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (target.clone(), String::new()),
    };
    let mut headers = Vec::new();
    let mut length = 0usize;
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).ok()? == 0 || h.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            let (k, v) = (k.trim().to_ascii_lowercase(), v.trim().to_string());
            if k == "content-length" {
                length = v.parse().unwrap_or(0);
            }
            headers.push((k, v));
        }
        if headers.len() > 100 {
            return None;
        }
    }
    if length > MAX_BODY {
        return None;
    }
    let mut body = vec![0u8; length];
    if length > 0 {
        reader.read_exact(&mut body).ok()?;
    }
    Some(Request { method, path, query, headers, body })
}

fn header<'a>(req: &'a Request, name: &str) -> Option<&'a str> {
    req.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

fn query_param(query: &str, name: &str) -> Option<String> {
    query.split('&').find_map(|kv| kv.split_once('=').filter(|(k, _)| *k == name).map(|(_, v)| url_decode(v)))
}

/// Decodes `%XX` escapes and `+` in a path segment or query value.
fn url_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or_default();
                match u8::from_str_radix(hex, 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn respond(stream: &mut TcpStream, status: &str, content_type: &str, body: &[u8], origin: Option<&str>) {
    let cors = format!(
        "Access-Control-Allow-Origin: {}\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type, Authorization\r\nAccess-Control-Allow-Private-Network: true\r\nAccess-Control-Max-Age: 600\r\n",
        origin.unwrap_or("*")
    );
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n{cors}\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

fn json_response(stream: &mut TcpStream, status: &str, v: &Value, origin: Option<&str>) {
    respond(stream, status, "application/json; charset=utf-8", v.to_string().as_bytes(), origin);
}

fn mime_of(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or_default() {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript",
        "wasm" => "application/wasm",
        "css" => "text/css",
        "json" => "application/json",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        _ => "application/octet-stream",
    }
}

fn serve_static(stream: &mut TcpStream, dir: &Path, path: &str, origin: Option<&str>) {
    let rel = path.trim_start_matches('/');
    let rel = if rel.is_empty() { "index.html" } else { rel };
    if rel.contains("..") || rel.contains('\\') {
        respond(stream, "400 Bad Request", "text/plain", b"bad path", origin);
        return;
    }
    let file = dir.join(rel);
    match std::fs::read(&file) {
        Ok(bytes) => respond(stream, "200 OK", mime_of(&file), &bytes, origin),
        Err(_) => respond(stream, "404 Not Found", "text/plain; charset=utf-8", b"not found", origin),
    }
}

const WELCOME: &str = "MailCraft server is running.\n\nOpen the web app and point it here, or start the server with --static DIR to serve the web app's files (apps/mailcraft-web, built with trunk).\n\nAPI: POST /api/cmd {\"command\", \"params\"}  ·  GET /api/snapshot?since=N  ·  GET /api/health\n";

/// Who is calling: the framework (service token) naming a user, a user token, or, in
/// single-user mode, anyone the token policy lets in.
enum Caller {
    Service { user: Option<String> },
    User(String),
    Single,
}

fn authenticate(hub: &Hub, req: &Request) -> Result<Caller, &'static str> {
    let given = header(req, "authorization")
        .and_then(|a| a.strip_prefix("Bearer "))
        .map(str::trim)
        .map(str::to_string)
        .or_else(|| query_param(&req.query, "token"));
    let service = &hub.opts.token;
    if hub.single.is_some() {
        if service.is_empty() || given.as_deref() == Some(service.as_str()) {
            return Ok(Caller::Single);
        }
        return Err("missing or wrong token");
    }
    let Some(given) = given.filter(|g| !g.is_empty()) else { return Err("missing token") };
    if !service.is_empty() && given == *service {
        let user = header(req, "x-mailcraft-user")
            .map(str::to_string)
            .or_else(|| query_param(&req.query, "user"))
            .map(|u| slug(&u))
            .filter(|u| !u.is_empty());
        return Ok(Caller::Service { user });
    }
    match hub.user_of_token(&given) {
        Some(u) => Ok(Caller::User(u)),
        None => Err("unknown token"),
    }
}

fn handle(mut stream: TcpStream, hub: Arc<Hub>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let Some(req) = read_request(&mut stream) else { return };
    let origin = header(&req, "origin").map(str::to_string);
    let origin = origin.as_deref();
    if req.method == "OPTIONS" {
        respond(&mut stream, "204 No Content", "text/plain", b"", origin);
        return;
    }
    if !req.path.starts_with("/api/") {
        match (req.method.as_str(), &hub.opts.static_dir) {
            ("GET", Some(dir)) => serve_static(&mut stream, dir, &req.path, origin),
            ("GET", None) if req.path == "/" => respond(&mut stream, "200 OK", "text/plain; charset=utf-8", WELCOME.as_bytes(), origin),
            ("GET", None) => respond(&mut stream, "404 Not Found", "text/plain", b"not found", origin),
            _ => respond(&mut stream, "405 Method Not Allowed", "text/plain", b"method not allowed", origin),
        }
        return;
    }
    if req.method == "GET" && req.path == "/api/health" {
        json_response(
            &mut stream,
            "200 OK",
            &json!({"ok": true, "name": "mailcraft", "version": env!("CARGO_PKG_VERSION"), "multi_user": hub.single.is_none()}),
            origin,
        );
        return;
    }
    let caller = match authenticate(&hub, &req) {
        Ok(c) => c,
        Err(e) => {
            json_response(&mut stream, "401 Unauthorized", &json!({"error": e}), origin);
            return;
        }
    };
    // Administration (service token only).
    if let Some(rest) = req.path.strip_prefix("/api/users") {
        if !matches!(caller, Caller::Service { .. }) {
            json_response(&mut stream, "403 Forbidden", &json!({"error": "service token required"}), origin);
            return;
        }
        let decoded: Vec<String> = rest.trim_matches('/').split('/').filter(|p| !p.is_empty()).map(url_decode).collect();
        let parts: Vec<&str> = decoded.iter().map(String::as_str).collect();
        match (req.method.as_str(), parts.as_slice()) {
            ("GET", []) => json_response(&mut stream, "200 OK", &json!({"users": hub.users()}), origin),
            ("POST", [user, "token"]) => match hub.mint_token(user) {
                Ok(t) => json_response(&mut stream, "200 OK", &json!({"user": slug(user), "token": t}), origin),
                Err(e) => json_response(&mut stream, "500 Internal Server Error", &json!({"error": e}), origin),
            },
            ("POST", [user, "revoke"]) => json_response(&mut stream, "200 OK", &json!({"user": slug(user), "revoked": hub.revoke(user)}), origin),
            _ => json_response(&mut stream, "404 Not Found", &json!({"error": "unknown users endpoint"}), origin),
        }
        return;
    }
    let user = match &caller {
        Caller::Single => String::new(),
        Caller::User(u) => u.clone(),
        Caller::Service { user: Some(u) } => u.clone(),
        Caller::Service { user: None } => {
            json_response(&mut stream, "400 Bad Request", &json!({"error": "name the user with the X-Mailcraft-User header (or ?user=)"}), origin);
            return;
        }
    };
    let session = match hub.session_for(&user) {
        Ok(s) => s,
        Err(e) => {
            json_response(&mut stream, "500 Internal Server Error", &json!({"error": e}), origin);
            return;
        }
    };
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/api/snapshot") => {
            let since: u64 = query_param(&req.query, "since").and_then(|s| s.parse().ok()).unwrap_or(0);
            let snapshot = session.lock().map(|s| s.snapshot(since)).unwrap_or(None);
            match snapshot {
                Some(s) => json_response(&mut stream, "200 OK", &serde_json::to_value(s).unwrap_or(Value::Null), origin),
                None => json_response(&mut stream, "200 OK", &json!({"unchanged": true}), origin),
            }
        }
        ("POST", "/api/cmd") => {
            let body: Value = match serde_json::from_slice(&req.body) {
                Ok(v) => v,
                Err(e) => {
                    json_response(&mut stream, "400 Bad Request", &json!({"error": format!("invalid JSON: {e}")}), origin);
                    return;
                }
            };
            let command = body.get("command").and_then(Value::as_str).unwrap_or_default().to_string();
            let params = body.get("params").cloned().unwrap_or_else(|| json!({}));
            let result = match session.lock() {
                Ok(mut s) => s.execute(&command, &params).map_err(|e| e.to_string()),
                Err(_) => Err("the session is unavailable".to_string()),
            };
            match result {
                Ok(v) => json_response(&mut stream, "200 OK", &json!({"result": v}), origin),
                Err(e) => json_response(&mut stream, "200 OK", &json!({"error": e}), origin),
            }
        }
        _ => json_response(&mut stream, "404 Not Found", &json!({"error": "unknown endpoint"}), origin),
    }
}

fn is_loopback(bind: &str) -> bool {
    bind.starts_with("127.") || bind.starts_with("localhost") || bind.starts_with("[::1]")
}

/// Runs the server until the process ends. `session` is the single-user session; in
/// multi-user mode (`users_dir`) it is ignored and sessions are made per user.
pub fn run(session: Session, opts: Options) -> Result<(), String> {
    if !is_loopback(&opts.bind) && opts.token.is_empty() {
        return Err("binding beyond localhost needs --token SECRET (and an HTTPS reverse proxy in front)".into());
    }
    if opts.users_dir.is_some() && opts.token.is_empty() {
        return Err("multi-user mode (--users-dir) needs --token SECRET for the framework".into());
    }
    let listener = TcpListener::bind(&opts.bind).map_err(|e| format!("cannot listen on {}: {e}", opts.bind))?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    if let Some(d) = &opts.users_dir {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let single = opts.users_dir.is_none().then(|| Arc::new(Mutex::new(session)));
    let hub = Arc::new(Hub { opts, single, sessions: Mutex::new(HashMap::new()), tokens: Mutex::new(HashMap::new()) });
    hub.load_tokens();
    // Pump every session in the background so jobs complete while nobody asks.
    {
        let h = hub.clone();
        std::thread::Builder::new()
            .name("mailcraft-pump".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(Duration::from_millis(150));
                    for s in h.all_sessions() {
                        if let Ok(mut guard) = s.lock() {
                            let _ = guard.pump();
                        }
                    }
                }
            })
            .map_err(|e| e.to_string())?;
    }
    let url = format!(
        "http://{}{}",
        addr,
        if hub.opts.token.is_empty() || hub.single.is_none() { String::new() } else { format!("/?token={}", hub.opts.token) }
    );
    eprintln!("MailCraft server on {url}{}", if hub.single.is_none() { " (multi-user)" } else { "" });
    if let Some(d) = &hub.opts.static_dir {
        eprintln!("serving the web app from {}", d.display());
    } else {
        eprintln!("no --static DIR: the web app on https://rickyb82.github.io/mailcraft/ can connect to this server (Server field in Settings)");
    }
    if hub.opts.open {
        let _ = std::process::Command::new(if cfg!(target_os = "macos") {
            "open"
        } else if cfg!(windows) {
            "cmd"
        } else {
            "xdg-open"
        })
        .args(if cfg!(windows) { vec!["/C", "start", "", url.as_str()] } else { vec![url.as_str()] })
        .spawn();
    }
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let h = hub.clone();
        let _ = std::thread::Builder::new().name("mailcraft-http".into()).spawn(move || handle(stream, h));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(addr: &str, path: &str, token: &str) -> (String, Value) {
        let mut s = TcpStream::connect(addr).expect("connect");
        let auth = if token.is_empty() { String::new() } else { format!("Authorization: Bearer {token}\r\n") };
        s.write_all(format!("GET {path} HTTP/1.1\r\nHost: x\r\n{auth}Origin: https://example.org\r\n\r\n").as_bytes()).expect("write");
        let mut out = String::new();
        s.read_to_string(&mut out).expect("read");
        let (head, body) = out.split_once("\r\n\r\n").expect("response");
        (head.lines().next().unwrap_or_default().to_string(), serde_json::from_str(body).unwrap_or(Value::Null))
    }

    fn post(addr: &str, path: &str, body: &str) -> Value {
        let mut s = TcpStream::connect(addr).expect("connect");
        s.write_all(
            format!(
                "POST {path} HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer t0k\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        )
        .expect("write");
        let mut out = String::new();
        s.read_to_string(&mut out).expect("read");
        serde_json::from_str(out.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("null")).unwrap_or(Value::Null)
    }

    #[test]
    fn api_serves_snapshots_and_commands() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr").to_string();
        drop(listener);
        let a = addr.clone();
        std::thread::spawn(move || {
            let _ = run(
                Session::demo(),
                Options { bind: a, token: "t0k".into(), static_dir: None, open: false, users_dir: None, demo: false, lang: None },
            );
        });
        for _ in 0..50 {
            if TcpStream::connect(&addr).is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let (status, _) = get(&addr, "/api/snapshot", "");
        assert!(status.contains("401"), "{status}");
        let (status, v) = get(&addr, "/api/health", "t0k");
        assert!(status.contains("200") && v["ok"] == true);
        let (_, snap) = get(&addr, "/api/snapshot?since=0", "t0k");
        let rev = snap["revision"].as_u64().expect("revision");
        assert_eq!(snap["view"]["account"], "demo");
        assert!(snap["states"]["demo"]["envelopes"]["INBOX"].as_array().expect("inbox").len() > 5);
        assert!(snap.to_string().contains("\"passwords\":[\"demo\"]"));
        assert!(!snap.to_string().contains("\"demo\"}") || true);
        let (_, same) = get(&addr, &format!("/api/snapshot?since={rev}"), "t0k");
        assert_eq!(same["unchanged"], true);
        let v = post(&addr, "/api/cmd", r#"{"command": "mail.search", "params": {"query": "fattura"}}"#);
        assert!(v["result"]["job"].is_u64(), "{v}");
        std::thread::sleep(Duration::from_millis(400));
        let (_, snap2) = get(&addr, &format!("/api/snapshot?since={rev}"), "t0k");
        assert!(snap2["revision"].as_u64().expect("rev") > rev);
        assert!(snap2["completed"].as_array().expect("completed").iter().any(|c| c["command"] == "mail.search"));
        assert_eq!(snap2["view"]["results"].as_array().expect("results").len(), 1);
        let v = post(&addr, "/api/cmd", r#"{"command": "nope"}"#);
        assert!(v["error"].as_str().expect("error").contains("unknown command"));
        let v = post(&addr, "/api/cmd", "{");
        assert!(v["error"].as_str().expect("error").contains("invalid JSON"));
    }
}

#[cfg(test)]
mod remote_tests {
    use super::*;
    use mailcraft_engine::MailSession;
    use mailcraft_remote::RemoteSession;

    fn pump_until(s: &mut RemoteSession, mut done: impl FnMut(&RemoteSession, &[mailcraft_engine::Completed]) -> bool) -> bool {
        for _ in 0..200 {
            let completed = s.pump();
            if done(s, &completed) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        false
    }

    #[test]
    fn a_remote_session_mirrors_the_server_and_runs_commands() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr").to_string();
        drop(listener);
        let a = addr.clone();
        std::thread::spawn(move || {
            let _ = run(
                Session::demo(),
                Options { bind: a, token: String::new(), static_dir: None, open: false, users_dir: None, demo: false, lang: None },
            );
        });
        for _ in 0..50 {
            if TcpStream::connect(&addr).is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let mut r = RemoteSession::new(&format!("http://{addr}"), "");
        assert!(pump_until(&mut r, |s, _| s.problem().is_none() && !s.config().accounts.is_empty()), "snapshot arrives");
        assert_eq!(r.view().account, "demo");
        assert!(r.has_password("demo"));
        let inbox = r.current().expect("state").list("INBOX").len();
        assert!(inbox > 5);
        assert_eq!(r.folder_for("demo", mailcraft_engine::core::mailbox::Role::Trash).as_deref(), Some("Trash"));
        // A network command: queued, then its completion is attributed to the command.
        let uid = r.current().expect("state").list("INBOX")[0].uid;
        assert_eq!(r.execute("mail.open", &json!({"uid": uid})).expect("queued")["queued"], true);
        assert!(pump_until(&mut r, |s, done| done.iter().any(|c| c.command == "mail.open")
            || s.current().is_some_and(|st| st.message("INBOX", uid).is_some())));
        assert!(pump_until(&mut r, |s, _| s.current().is_some_and(|st| st.message("INBOX", uid).is_some())), "the opened message is mirrored");
        assert!(r.current().expect("state").message("INBOX", uid).expect("message").text.contains("lunedì"));
        // A local command: answered through pump as a completion with the value.
        r.execute("account.suggest", &json!({"email": "anna@gmail.com"})).expect("queued");
        assert!(pump_until(&mut r, |_, done| done
            .iter()
            .any(|c| c.command == "account.suggest" && c.result.as_ref().is_ok_and(|v| v["provider"] == "Gmail"))));
        // A failing command reports its error.
        r.execute("mail.move", &json!({"uids": [uid], "to": "Nowhere"})).expect("queued");
        assert!(pump_until(&mut r, |_, done| done.iter().any(|c| c.command == "mail.move" && c.result.is_err())));
        // Archive: the mirrored inbox shrinks.
        r.execute("mail.archive", &json!({"uids": [uid]})).expect("queued");
        assert!(pump_until(&mut r, |s, _| s.current().is_some_and(|st| st.list("INBOX").len() < inbox)));
    }
}

#[cfg(test)]
mod multi_user_tests {
    use super::*;
    use mailcraft_remote::RemoteSession;

    fn call(addr: &str, method: &str, path: &str, headers: &str, body: &str) -> (String, Value) {
        let mut s = TcpStream::connect(addr).expect("connect");
        s.write_all(
            format!("{method} {path} HTTP/1.1\r\nHost: x\r\n{headers}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len())
                .as_bytes(),
        )
        .expect("write");
        let mut out = String::new();
        s.read_to_string(&mut out).expect("read");
        let (head, body) = out.split_once("\r\n\r\n").expect("response");
        (head.lines().next().unwrap_or_default().to_string(), serde_json::from_str(body).unwrap_or(Value::Null))
    }

    #[test]
    fn users_are_isolated_tokens_are_minted_and_mcp_runs_over_the_server() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr").to_string();
        drop(listener);
        let dir = std::env::temp_dir().join(format!("mailcraft-multi-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let a = addr.clone();
        let d = dir.clone();
        std::thread::spawn(move || {
            let _ = run(
                Session::demo(),
                Options { bind: a, token: "svc".into(), static_dir: None, open: false, users_dir: Some(d), demo: true, lang: Some("it".into()) },
            );
        });
        for _ in 0..50 {
            if TcpStream::connect(&addr).is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        // No token, wrong token, service token without a user.
        assert!(call(&addr, "GET", "/api/snapshot", "", "").0.contains("401"));
        assert!(call(&addr, "GET", "/api/snapshot", "Authorization: Bearer nope\r\n", "").0.contains("401"));
        assert!(call(&addr, "GET", "/api/snapshot", "Authorization: Bearer svc\r\n", "").0.contains("400"));
        // The framework names the user; each user has their own session.
        let (st, v) = call(
            &addr,
            "POST",
            "/api/cmd",
            "Authorization: Bearer svc\r\nX-Mailcraft-User: Anna Rossi\r\n",
            r#"{"command": "prefs.set", "params": {"theme": "dark"}}"#,
        );
        assert!(st.contains("200"), "{v}");
        let (_, anna) = call(&addr, "GET", "/api/snapshot?since=0", "Authorization: Bearer svc\r\nX-Mailcraft-User: Anna Rossi\r\n", "");
        assert_eq!(anna["config"]["preferences"]["theme"], "dark");
        let (_, luca) = call(&addr, "GET", "/api/snapshot?since=0&user=luca", "Authorization: Bearer svc\r\n", "");
        assert_eq!(luca["config"]["preferences"]["theme"], "system", "another user, another session");
        // User tokens: minted by the service, usable by the page, revocable.
        let (_, users) = call(&addr, "GET", "/api/users", "Authorization: Bearer svc\r\n", "");
        assert!(users["users"].as_array().expect("users").iter().any(|u| u == "anna-rossi"));
        let (_, minted) = call(&addr, "POST", "/api/users/Anna%20Rossi/token", "Authorization: Bearer svc\r\n", "");
        let token = minted["token"].as_str().expect("token").to_string();
        assert_eq!(token.len(), 64);
        assert!(
            call(&addr, "POST", "/api/users/anna/token", &format!("Authorization: Bearer {token}\r\n"), "").0.contains("403"),
            "users cannot mint"
        );
        let (st, as_anna) = call(&addr, "GET", "/api/snapshot?since=0", &format!("Authorization: Bearer {token}\r\n"), "");
        assert!(st.contains("200"));
        assert_eq!(as_anna["config"]["preferences"]["theme"], "dark");
        // The MCP server for a user, over the network.
        let mut mcp = mailcraft_mcp::Server::new(RemoteSession::new(&format!("http://{addr}"), &token));
        let r = mcp
            .handle(
                &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "list_messages", "arguments": {"unread_only": true}}}),
            )
            .expect("reply");
        assert_eq!(r["result"]["isError"], false, "{r}");
        assert!(r["result"]["structuredContent"]["count"].as_u64().expect("count") >= 3);
        let r = mcp.handle(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "suggest_settings", "arguments": {"email": "x@libero.it"}}})).expect("reply");
        assert_eq!(r["result"]["structuredContent"]["provider"], "Libero");
        let r = mcp
            .handle(
                &json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "move", "arguments": {"uids": [1], "to": "Nowhere"}}}),
            )
            .expect("reply");
        assert_eq!(r["result"]["isError"], true, "a failing command reports its error: {r}");
        assert!(mcp.session().config().accounts.len() == 1);
        // Revocation.
        let (_, rev) = call(&addr, "POST", "/api/users/anna-rossi/revoke", "Authorization: Bearer svc\r\n", "");
        assert_eq!(rev["revoked"], 1);
        assert!(call(&addr, "GET", "/api/snapshot", &format!("Authorization: Bearer {token}\r\n"), "").0.contains("401"));
        assert!(dir.join("tokens.json").is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
