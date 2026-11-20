//! Loopback browser panels. Document mutations are dispatched to the UI thread.
use super::store::Bundle;
use crate::loopback;
use serde::Deserialize;
use std::{
    net::{TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

pub struct InsertRequest {
    pub svg: String,
    pub name: String,
    pub reply: SyncSender<Result<String, String>>,
    pub cancelled: Arc<AtomicBool>,
}

pub struct Runtime {
    url: String,
    receiver: Receiver<InsertRequest>,
    stopped: Arc<AtomicBool>,
}

impl Runtime {
    pub fn start(bundle: Bundle, ctx: egui::Context) -> Result<Self, String> {
        if bundle.files.keys().any(|path| path.eq_ignore_ascii_case("host.json") || path.eq_ignore_ascii_case("insert-svg")) {
            return Err("Plugin assets collide with reserved host endpoints.".into());
        }
        if bundle.panel.contains(['/', '\\']) || !bundle.files.contains_key(&bundle.panel) || mime(&bundle.panel) != Some("text/html; charset=utf-8") {
            return Err("Plugin panel HTML is missing.".into());
        }
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("Cannot start plugin panel: {e}"))?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let host = listener.local_addr().map_err(|e| e.to_string())?.to_string();
        let token = uuid::Uuid::new_v4().simple().to_string();
        let origin = format!("http://{host}");
        let url = format!("{origin}/{token}/{}", bundle.panel);
        let (sender, receiver) = mpsc::sync_channel(8);
        let stopped = Arc::new(AtomicBool::new(false));
        let worker_stop = stopped.clone();
        let mut policy = "default-src 'none'; script-src 'self'; style-src 'self'; img-src blob:; connect-src 'self'".to_string();
        for origin in &bundle.network {
            // Defense in depth: only origins, never arbitrary CSP directives.
            let parsed = url::Url::parse(origin).map_err(|_| "Invalid plugin network origin")?;
            if parsed.scheme() != "https" || parsed.origin().ascii_serialization() != *origin || origin.contains([';', '\'', ' ', '\r', '\n']) {
                return Err("Invalid plugin network origin".into());
            }
            policy.push(' ');
            policy.push_str(origin);
        }
        policy.push_str("; object-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'");
        thread::Builder::new()
            .name("plugin-panel".into())
            .spawn(move || {
                let server = Server { bundle, host, origin, token, policy, sender, stopped: worker_stop, ctx };
                while !server.stopped.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((mut stream, _)) => server.handle(&mut stream),
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => thread::sleep(Duration::from_millis(15)),
                        Err(_) => break,
                    }
                }
            })
            .map_err(|e| format!("Cannot start plugin panel: {e}"))?;
        Ok(Self { url, receiver, stopped })
    }

    pub fn url(&self) -> &str {
        &self.url
    }
    pub fn try_recv(&self) -> Option<InsertRequest> {
        self.receiver.try_recv().ok()
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        while let Ok(request) = self.receiver.try_recv() {
            request.cancelled.store(true, Ordering::Release);
        }
    }
}

struct Server {
    bundle: Bundle,
    host: String,
    origin: String,
    token: String,
    policy: String,
    sender: SyncSender<InsertRequest>,
    stopped: Arc<AtomicBool>,
    ctx: egui::Context,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InsertBody {
    svg: String,
    name: String,
}

impl Server {
    fn handle(&self, stream: &mut TcpStream) {
        let _ = stream.set_read_timeout(Some(Duration::from_millis(250)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
        let response = self.request(stream);
        let (status, mime, body) = response.unwrap_or_else(|(status, message)| (status, "application/json", loopback::json_error(message)));
        loopback::write_response(stream, status, mime, &body, &format!("Content-Security-Policy: {}\r\n", self.policy));
    }

    fn request(&self, stream: &mut TcpStream) -> Result<(&'static str, &'static str, Vec<u8>), loopback::Failure> {
        let req = loopback::read_request(stream, &self.stopped, Instant::now() + Duration::from_secs(2))?;
        if req.headers.get("host").map(String::as_str) != Some(self.host.as_str()) {
            return Err(("403 Forbidden", "Invalid host"));
        }
        let prefix = format!("/{}/", self.token);
        let path = req.path.strip_prefix(&prefix).ok_or(("404 Not Found", "Not found"))?;
        if path.contains(['%', '\\', '?', '#']) || path.split('/').any(|s| s == "." || s == "..") {
            return Err(("404 Not Found", "Not found"));
        }
        if req.method == "GET" {
            if path == "host.json" {
                return Ok((
                    "200 OK",
                    "application/json",
                    serde_json::to_vec(&serde_json::json!({"api_version":1,"token":self.token,"permissions":self.bundle.plugin.permissions}))
                        .unwrap_or_default(),
                ));
            }
            let file = if path.is_empty() { self.bundle.panel.as_str() } else { path };
            let body = self.bundle.files.get(file).ok_or(("404 Not Found", "Not found"))?;
            return Ok(("200 OK", mime(file).ok_or(("404 Not Found", "Unsupported asset"))?, body.clone()));
        }
        if req.method != "POST" || path != "insert-svg" {
            return Err(("405 Method Not Allowed", "Unsupported operation"));
        }
        if req.headers.get("origin").map(String::as_str) != Some(self.origin.as_str())
            || req.headers.get("x-desigma-token").map(String::as_str) != Some(self.token.as_str())
        {
            return Err(("403 Forbidden", "Invalid panel session"));
        }
        if !self.bundle.plugin.permissions.iter().any(|p| p == "document:insert-svg") {
            return Err(("403 Forbidden", "SVG insertion permission is required"));
        }
        if req.headers.get("content-type").map(|s| s.split(';').next().unwrap_or_default().trim()) != Some("application/json") {
            return Err(("415 Unsupported Media Type", "JSON required"));
        }
        let body: InsertBody = serde_json::from_slice(&req.body).map_err(|_| ("400 Bad Request", "Invalid insertion request"))?;
        if body.svg.trim().is_empty() || body.name.trim().is_empty() || body.name.len() > 200 {
            return Err(("400 Bad Request", "SVG and a short layer name are required"));
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let (reply, receive) = mpsc::sync_channel(1);
        self.sender
            .try_send(InsertRequest { svg: body.svg, name: body.name, reply, cancelled: cancelled.clone() })
            .map_err(|_| ("503 Service Unavailable", "Editor is busy"))?;
        self.ctx.request_repaint();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if self.stopped.load(Ordering::Acquire) || Instant::now() >= deadline {
                cancelled.store(true, Ordering::Release);
                return Err(("503 Service Unavailable", "Editor session ended or timed out"));
            }
            match receive.recv_timeout(Duration::from_millis(25)) {
                Ok(Ok(id)) => return Ok(("200 OK", "application/json", serde_json::to_vec(&serde_json::json!({"ok":true,"node_id":id})).unwrap_or_default())),
                Ok(Err(message)) => {
                    return Ok(("422 Unprocessable Entity", "application/json", serde_json::to_vec(&serde_json::json!({"error":message})).unwrap_or_default()));
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    cancelled.store(true, Ordering::Release);
                    return Err(("503 Service Unavailable", "Editor session ended"));
                }
            }
        }
    }
}

fn mime(path: &str) -> Option<&'static str> {
    match path.rsplit('.').next()? {
        "html" => Some("text/html; charset=utf-8"),
        "js" => Some("text/javascript; charset=utf-8"),
        "css" => Some("text/css; charset=utf-8"),
        "json" => Some("application/json; charset=utf-8"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::super::store::InstalledPlugin;
    use super::*;
    use std::{
        collections::BTreeMap,
        io::{Read, Write},
    };

    fn runtime(permission: bool) -> Runtime {
        let plugin: InstalledPlugin = serde_json::from_value(serde_json::json!({
            "id":"test", "name":"Test", "version":"1", "license":"MIT", "source_url":"", "manifest_url":"", "main":"main.js",
            "permissions":if permission { vec!["document:insert-svg"] } else { vec![] }, "runtime_supported":true, "installed_at":0
        }))
        .unwrap();
        Runtime::start(
            Bundle { plugin, panel: "index.html".into(), files: BTreeMap::from([("index.html".into(), b"<html>panel</html>".to_vec())]), network: vec![] },
            egui::Context::default(),
        )
        .unwrap()
    }

    fn exchange(url: &str, method: &str, path: &str, extras: &str, body: &str) -> String {
        let url = url::Url::parse(url).unwrap();
        let host = format!("127.0.0.1:{}", url.port().unwrap());
        let mut stream = TcpStream::connect(&host).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(8))).unwrap();
        write!(stream, "{method} {path} HTTP/1.1\r\nHost: {host}\r\n{extras}Content-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        let mut result = String::new();
        // Oversized or malformed requests may leave bytes unread. Windows can
        // reset that socket after sending the complete HTTP response.
        if let Err(error) = stream.read_to_string(&mut result) {
            assert_eq!(error.kind(), std::io::ErrorKind::ConnectionReset);
            let (header, body) = result.split_once("\r\n\r\n").expect("complete response headers before reset");
            let length: usize = header.lines().find_map(|line| line.strip_prefix("Content-Length: ")).unwrap().trim().parse().unwrap();
            assert_eq!(body.len(), length, "complete response body before reset");
        }
        result
    }

    fn auth(url: &str) -> String {
        let url = url::Url::parse(url).unwrap();
        format!(
            "Origin: {}\r\nX-Desigma-Token: {}\r\nContent-Type: application/json\r\n",
            url.origin().ascii_serialization(),
            url.path().trim_matches('/').split('/').next().unwrap()
        )
    }

    fn base_path(url: &str) -> String {
        let url = url::Url::parse(url).unwrap();
        format!("{}/", url.path().rsplit_once('/').unwrap().0)
    }

    #[test]
    fn serves_only_session_assets_with_restrictive_headers() {
        let runtime = runtime(true);
        let path = base_path(runtime.url());
        let good = exchange(runtime.url(), "GET", &path, "", "");
        assert!(good.starts_with("HTTP/1.1 200"));
        assert!(good.contains("frame-ancestors 'none'"));
        assert!(good.contains("Referrer-Policy: no-referrer"));
        for path in ["/index.html".into(), format!("{path}../index.html"), format!("{path}%2e%2e/index.html"), format!("{path}unknown.js")] {
            assert!(exchange(runtime.url(), "GET", &path, "", "").starts_with("HTTP/1.1 404"));
        }
    }

    #[test]
    fn rejects_hostile_origins_tokens_and_missing_permission() {
        for permission in [true, false] {
            let runtime = runtime(permission);
            let path = format!("{}insert-svg", base_path(runtime.url()));
            let good = auth(runtime.url());
            let cases = if permission {
                vec![good.replace("Origin: http://127.0.0.1", "Origin: http://evil.example"), good.replace("X-Desigma-Token: ", "X-Desigma-Token: wrong")]
            } else {
                vec![good]
            };
            for headers in cases {
                let response = exchange(runtime.url(), "POST", &path, &headers, r#"{"svg":"<svg/>","name":"Icon"}"#);
                assert!(response.starts_with("HTTP/1.1 403"), "{response}");
                assert!(runtime.try_recv().is_none());
            }
        }
    }

    #[test]
    fn dispatches_to_editor_and_returns_actual_reply() {
        let runtime = runtime(true);
        let url = runtime.url().to_string();
        let worker = thread::spawn(move || {
            let path = format!("{}insert-svg", base_path(&url));
            exchange(&url, "POST", &path, &auth(&url), r#"{"svg":"<svg/>","name":"Icon"}"#)
        });
        let deadline = Instant::now() + Duration::from_secs(3);
        let request = loop {
            if let Some(request) = runtime.try_recv() {
                break request;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(request.name, "Icon");
        assert!(!request.cancelled.load(Ordering::Acquire));
        request.reply.send(Ok("node-42".into())).unwrap();
        let response = worker.join().unwrap();
        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.contains("\"node_id\":\"node-42\""));
        assert!(response.contains("\"ok\":true"));
    }

    #[test]
    fn dropping_runtime_cancels_pending_mutation() {
        let runtime = runtime(true);
        let url = runtime.url().to_string();
        let worker = thread::spawn(move || {
            let path = format!("{}insert-svg", base_path(&url));
            exchange(&url, "POST", &path, &auth(&url), r#"{"svg":"<svg/>","name":"Icon"}"#)
        });
        let deadline = Instant::now() + Duration::from_secs(3);
        let request = loop {
            if let Some(request) = runtime.try_recv() {
                break request;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        };
        drop(runtime);
        assert!(worker.join().unwrap().starts_with("HTTP/1.1 503"));
        assert!(request.cancelled.load(Ordering::Acquire));
    }
}
