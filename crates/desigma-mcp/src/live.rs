//! Attaching to the running editor: discovery file, ping, and forwarding tool calls over loopback HTTP.
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    time::Duration,
};

pub const ENV_FILE: &str = "DESIGMA_BRIDGE_FILE";

#[derive(Deserialize)]
pub struct Discovery {
    pub version: u32,
    pub port: u16,
    pub token: String,
    pub pid: u32,
    pub document: String,
    pub path: Option<PathBuf>,
}

pub struct Editor {
    pub port: u16,
    pub token: String,
}
#[derive(Debug)]
pub enum Reply {
    Ok(Value),
    ToolError(String),
}

/// `$DESIGMA_BRIDGE_FILE` unset = the default discovery path; set to an empty
/// string = explicitly disabled (`None`, never probes); set to anything else
/// = that path instead of the default.
pub fn discovery_path() -> Option<PathBuf> {
    match std::env::var_os(ENV_FILE) {
        Some(v) if v.is_empty() => None,
        Some(v) => Some(PathBuf::from(v)),
        None => Some(desigma_library::Library::default_root().join("bridge.json")),
    }
}

pub fn probe_at(file: &Path) -> Option<Editor> {
    let d: Discovery = serde_json::from_slice(&std::fs::read(file).ok()?).ok()?;
    if d.version != 1 {
        return None;
    }
    let editor = Editor { port: d.port, token: d.token };
    let (status, _) = http(&editor, "GET", "ping", &[], Duration::from_millis(300)).ok()?;
    (status == 200).then_some(editor)
}

pub fn forward(editor: &Editor, name: &str, args: &Value) -> Result<Reply, String> {
    let body = serde_json::to_vec(&json!({"name":name,"args":args})).map_err(|e| e.to_string())?;
    let (status, text) = http(editor, "POST", "call", &body, Duration::from_secs(10))?;
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    match status {
        200 => Ok(Reply::Ok(v["result"].clone())),
        422 => Ok(Reply::ToolError(v["error"].as_str().unwrap_or("editor rejected the call").to_string())),
        403 | 404 => Err("Editor refused the bridge token; restart desigma-mcp".into()),
        // the editor's own message ("Editor is busy", "Editor did not answer within 5 s; ...")
        503 => Err(v["error"].as_str().map_or_else(|| format!("editor answered HTTP 503: {text}"), str::to_string)),
        s => Err(format!("editor answered HTTP {s}: {}", v["error"].as_str().unwrap_or(&text))),
    }
}

/// One HTTP/1.1 exchange on loopback; returns (status, body).
fn http(editor: &Editor, method: &str, route: &str, body: &[u8], timeout: Duration) -> Result<(u16, String), String> {
    let addr = SocketAddr::from(([127, 0, 0, 1], editor.port));
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_millis(300)).map_err(|e| format!("cannot reach the editor: {e}"))?;
    s.set_read_timeout(Some(timeout)).map_err(|e| e.to_string())?;
    s.set_write_timeout(Some(Duration::from_secs(2))).map_err(|e| e.to_string())?;
    let head = format!(
        "{method} /{}/{route} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nX-Desigma-Token: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        editor.token,
        editor.port,
        editor.token,
        body.len()
    );
    // One write, so the head and body reach the editor in a single segment.
    s.write_all(&[head.as_bytes(), body].concat()).map_err(|e| format!("cannot send to the editor: {e}"))?;
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).map_err(|e| format!("editor did not answer: {e}"))?;
    let raw = String::from_utf8_lossy(&raw);
    let (head, body) = raw.split_once("\r\n\r\n").ok_or("malformed answer from the editor")?;
    let status: u16 = head.split(' ').nth(1).and_then(|s| s.parse().ok()).ok_or("malformed status line from the editor")?;
    Ok((status, body.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    /// Minimal editor stand-in: answers ping and echoes the call name back, or fails auth.
    fn fake_editor(token: &'static str, accept: bool) -> u16 {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        thread::spawn(move || {
            for s in l.incoming() {
                let mut s = s.unwrap();
                let mut buf = vec![0; 8192];
                let n = s.read(&mut buf).unwrap();
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let body = if req.starts_with(&format!("GET /{token}/ping")) {
                    ("200 OK", r#"{"ok":true,"document":"D","path":null}"#.to_string())
                } else if !accept {
                    ("403 Forbidden", r#"{"error":"Invalid bridge token"}"#.to_string())
                } else if req.contains("desigma_get_node") {
                    ("422 Unprocessable Entity", r#"{"error":"node 7 not found"}"#.to_string())
                } else {
                    ("200 OK", r#"{"result":{"echo":true}}"#.to_string())
                };
                write!(s, "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.0, body.1.len(), body.1)
                    .unwrap();
            }
        });
        port
    }
    fn discovery(dir: &std::path::Path, port: u16, token: &str) -> PathBuf {
        let f = dir.join("bridge.json");
        std::fs::write(&f, format!(r#"{{"version":1,"port":{port},"token":"{token}","pid":1,"document":"D","path":null}}"#)).unwrap();
        f
    }

    #[test]
    fn probe_finds_a_live_editor_and_ignores_dead_or_missing_files() {
        let dir = std::env::temp_dir().join(format!("desigma-live-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let port = fake_editor("tok", true);
        let file = discovery(&dir, port, "tok");
        assert!(probe_at(&file).is_some());
        let dead = discovery(&dir, 1, "tok");
        assert!(probe_at(&dead).is_none());
        assert!(probe_at(&dir.join("absent.json")).is_none());
    }

    #[test]
    fn forward_distinguishes_tool_errors_from_transport_and_auth_failures() {
        let ok = Editor { port: fake_editor("tok", true), token: "tok".into() };
        assert!(matches!(forward(&ok, "desigma_get_document", &json!({})).unwrap(), Reply::Ok(v) if v["echo"] == true));
        assert!(matches!(forward(&ok, "desigma_get_node", &json!({"node_id":7})).unwrap(), Reply::ToolError(m) if m.contains("not found")));
        let bad = Editor { port: fake_editor("tok", false), token: "tok".into() };
        assert!(forward(&bad, "desigma_get_document", &json!({})).unwrap_err().contains("refused"));
        let gone = Editor { port: 1, token: "tok".into() };
        assert!(forward(&gone, "desigma_get_document", &json!({})).is_err());
    }
}
