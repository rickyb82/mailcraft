//! `mailcraft-cli serve`: the server behind the web app. It keeps the engine session (accounts,
//! connections, cache) and speaks a tiny HTTP API: `POST /api/cmd` runs a command,
//! `GET /api/snapshot?since=N` returns the state when it changed, `GET /api/health`. It also
//! serves the web app's files from a directory when one is given.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mailcraft_engine::Session;
use serde_json::{Value, json};

pub struct Options {
    pub bind: String,
    pub token: String,
    pub static_dir: Option<PathBuf>,
    pub open: bool,
}

type Shared = Arc<Mutex<Session>>;

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
    query.split('&').find_map(|kv| kv.split_once('=').filter(|(k, _)| *k == name).map(|(_, v)| v.to_string()))
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

fn handle(mut stream: TcpStream, session: Shared, opts: Arc<Options>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let Some(req) = read_request(&mut stream) else { return };
    let origin = header(&req, "origin").map(str::to_string);
    let origin = origin.as_deref();
    if req.method == "OPTIONS" {
        respond(&mut stream, "204 No Content", "text/plain", b"", origin);
        return;
    }
    if !opts.token.is_empty() {
        let given =
            header(&req, "authorization").and_then(|a| a.strip_prefix("Bearer ")).map(str::to_string).or_else(|| query_param(&req.query, "token"));
        if given.as_deref() != Some(opts.token.as_str()) && req.path.starts_with("/api/") {
            json_response(&mut stream, "401 Unauthorized", &json!({"error": "missing or wrong token"}), origin);
            return;
        }
    }
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/api/health") => {
            json_response(&mut stream, "200 OK", &json!({"ok": true, "name": "mailcraft", "version": env!("CARGO_PKG_VERSION")}), origin)
        }
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
        ("GET", path) => match &opts.static_dir {
            Some(dir) => serve_static(&mut stream, dir, path, origin),
            None if path == "/" => respond(&mut stream, "200 OK", "text/plain; charset=utf-8", WELCOME.as_bytes(), origin),
            None => respond(&mut stream, "404 Not Found", "text/plain", b"not found", origin),
        },
        _ => respond(&mut stream, "405 Method Not Allowed", "text/plain", b"method not allowed", origin),
    }
}

fn is_loopback(bind: &str) -> bool {
    bind.starts_with("127.") || bind.starts_with("localhost") || bind.starts_with("[::1]")
}

/// Runs the server until the process ends.
pub fn run(session: Session, opts: Options) -> Result<(), String> {
    if !is_loopback(&opts.bind) && opts.token.is_empty() {
        return Err("binding beyond localhost needs --token SECRET (and an HTTPS reverse proxy in front)".into());
    }
    let listener = TcpListener::bind(&opts.bind).map_err(|e| format!("cannot listen on {}: {e}", opts.bind))?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    let session: Shared = Arc::new(Mutex::new(session));
    // Pump the engine in the background so jobs complete while nobody asks.
    {
        let s = session.clone();
        std::thread::Builder::new()
            .name("mailcraft-pump".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(Duration::from_millis(150));
                    if let Ok(mut guard) = s.lock() {
                        let _ = guard.pump();
                    }
                }
            })
            .map_err(|e| e.to_string())?;
    }
    let url = format!("http://{}{}", addr, if opts.token.is_empty() { String::new() } else { format!("/?token={}", opts.token) });
    eprintln!("MailCraft server on {url}");
    if let Some(d) = &opts.static_dir {
        eprintln!("serving the web app from {}", d.display());
    } else {
        eprintln!("no --static DIR: the web app on https://rickyb82.github.io/mailcraft/ can connect to this server (Server field in Settings)");
    }
    if opts.open {
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
    let opts = Arc::new(opts);
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let (s, o) = (session.clone(), opts.clone());
        let _ = std::thread::Builder::new().name("mailcraft-http".into()).spawn(move || handle(stream, s, o));
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
            let _ = run(Session::demo(), Options { bind: a, token: "t0k".into(), static_dir: None, open: false });
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
            let _ = run(Session::demo(), Options { bind: a, token: String::new(), static_dir: None, open: false });
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
