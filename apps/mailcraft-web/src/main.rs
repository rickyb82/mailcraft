//! MailCraft in the browser. Build: `cd apps/mailcraft-web && trunk build --release` (trunk
//! generates the JS loader; no handwritten JS).
//!
//! The browser cannot speak IMAP, so the page mirrors a `mailcraft-cli serve` server (your
//! computer by default: `http://127.0.0.1:7878`, or the page's own origin when the server serves
//! the page). URL parameters: `?server=http://host:7878`, `?token=…`, `?lang=it`. Attachments
//! come in through the browser's file picker and go out as downloads.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[cfg(target_arch = "wasm32")]
mod web {
    use std::sync::{Arc, Mutex};

    use eframe::wasm_bindgen::JsCast;
    use mailcraft_remote::RemoteSession;
    use mailcraft_ui_egui::core::message::DraftAttachment;
    use mailcraft_ui_egui::{Language, MailApp, Request};
    use serde_json::{Value, json};

    const STORAGE_KEY: &str = "mailcraft";
    const DEFAULT_SERVER: &str = "http://127.0.0.1:7878";

    /// Files picked asynchronously, waiting to be attached on the next frame.
    type Inbox = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

    pub struct App {
        app: MailApp,
        inbox: Inbox,
        token: String,
    }

    fn query_param(name: &str) -> Option<String> {
        let search = web_sys::window()?.location().search().ok()?;
        web_sys::UrlSearchParams::new_with_str(&search).ok()?.get(name)
    }

    /// The page's origin when it is served by mailcraft-cli itself (not a static host).
    fn own_server() -> Option<String> {
        let origin = web_sys::window()?.location().origin().ok()?;
        let host = web_sys::window()?.location().hostname().ok()?;
        (!host.ends_with("github.io") && !origin.starts_with("file:")).then_some(origin)
    }

    /// Hands `bytes` to the browser as a file download.
    fn download(name: &str, bytes: &[u8]) {
        let Some(document) = web_sys::window().and_then(|w| w.document()) else { return };
        let array = js_sys::Uint8Array::from(bytes);
        let parts = js_sys::Array::new();
        parts.push(&array.buffer());
        let options = web_sys::BlobPropertyBag::new();
        options.set_type("application/octet-stream");
        let Ok(blob) = web_sys::Blob::new_with_buffer_source_sequence_and_options(&parts, &options) else { return };
        let Ok(url) = web_sys::Url::create_object_url_with_blob(&blob) else { return };
        if let Some(a) = document.create_element("a").ok().and_then(|e| e.dyn_into::<web_sys::HtmlAnchorElement>().ok()) {
            a.set_href(&url);
            a.set_download(name);
            a.click();
        }
        let _ = web_sys::Url::revoke_object_url(&url);
    }

    fn mime_for(name: &str) -> &'static str {
        match name.rsplit('.').next().unwrap_or_default().to_ascii_lowercase().as_str() {
            "pdf" => "application/pdf",
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "txt" | "md" => "text/plain",
            "csv" => "text/csv",
            "zip" => "application/zip",
            _ => "application/octet-stream",
        }
    }

    impl App {
        pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
            let saved: Option<Value> = cc.storage.and_then(|s| s.get_string(STORAGE_KEY)).and_then(|t| serde_json::from_str(&t).ok());
            let server = query_param("server")
                .or_else(|| saved.as_ref().and_then(|v| v.get("server")?.as_str().map(str::to_string)))
                .or_else(own_server)
                .unwrap_or_else(|| DEFAULT_SERVER.to_string());
            let token =
                query_param("token").or_else(|| saved.as_ref().and_then(|v| v.get("token")?.as_str().map(str::to_string))).unwrap_or_default();
            let mut app = MailApp::new(Box::new(RemoteSession::new(&server, &token)));
            let browser_language = web_sys::window().and_then(|w| w.navigator().language()).and_then(|l| Language::parse(&l));
            app.language =
                saved.as_ref().and_then(|v| v.get("language")?.as_str().and_then(Language::parse)).or(browser_language).unwrap_or_default();
            if let Some(l) = query_param("lang").and_then(|l| Language::parse(&l)) {
                app.language = l;
            }
            app.dark = saved.as_ref().and_then(|v| v.get("dark")?.as_bool()).unwrap_or_else(|| cc.egui_ctx.system_theme() == Some(egui::Theme::Dark));
            Self { app, inbox: Arc::default(), token }
        }

        fn serve_requests(&mut self, ctx: &egui::Context) {
            for request in std::mem::take(&mut self.app.requests) {
                match request {
                    Request::AttachFiles => {
                        let inbox = self.inbox.clone();
                        let ctx = ctx.clone();
                        wasm_bindgen_futures::spawn_local(async move {
                            let files = rfd::AsyncFileDialog::new().pick_files().await.unwrap_or_default();
                            for file in files.into_iter().take(50) {
                                let bytes = file.read().await;
                                if let Ok(mut q) = inbox.lock() {
                                    q.push((file.file_name(), bytes));
                                }
                            }
                            ctx.request_repaint();
                        });
                    }
                    Request::SaveAttachment { name, data } => download(&name, &data),
                    Request::OpenUrl(url) => {
                        if (url.starts_with("http://") || url.starts_with("https://"))
                            && let Some(w) = web_sys::window()
                        {
                            let _ = w.open_with_url_and_target(&url, "_blank");
                        }
                    }
                }
            }
        }
    }

    impl eframe::App for App {
        fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
            let files: Vec<(String, Vec<u8>)> = self.inbox.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default();
            if !files.is_empty()
                && let Some(c) = &mut self.app.compose
            {
                for (name, data) in files {
                    c.attachments.push(DraftAttachment { mime: mime_for(&name).into(), name, data, cid: String::new() });
                }
            }
            self.app.ui(ui);
            let ctx = ui.ctx().clone();
            self.serve_requests(&ctx);
            // The remote session polls the server; keep frames coming while it does.
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }

        fn save(&mut self, storage: &mut dyn eframe::Storage) {
            let state =
                json!({"language": self.app.language.code(), "dark": self.app.dark, "server": self.app.session.server(), "token": self.token});
            storage.set_string(STORAGE_KEY, state.to_string());
        }
    }

    pub fn start() {
        let options = eframe::WebOptions::default();
        wasm_bindgen_futures::spawn_local(async move {
            let Some(canvas) = web_sys::window()
                .and_then(|w| w.document())
                .and_then(|d| d.get_element_by_id("mailcraft"))
                .and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok())
            else {
                return;
            };
            let started = eframe::WebRunner::new().start(canvas, options, Box::new(|cc| Ok(Box::new(App::new(cc))))).await;
            if let Err(e) = started {
                eframe::web_sys::console::error_1(&e);
            }
        });
    }
}

#[cfg(target_arch = "wasm32")]
fn main() {
    web::start();
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!("mailcraft-web runs in the browser: cd apps/mailcraft-web && trunk serve --release (and mailcraft-cli serve)");
}
