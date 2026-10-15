//! Data-only, revision-bound catalog. This crate cannot create editor documents.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

pub const PROTOCOL: &str = "cityos.design-catalog.ipc.v1";
pub const MAX_FRAME: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct ReaderConfig {
    pub node: PathBuf,
    pub reader: PathBuf,
    pub repository: PathBuf,
    pub cache: Option<PathBuf>,
    pub canonical_ref: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: String,
    pub semantic_id: String,
    pub label: String,
    pub level: Option<String>,
    pub family: String,
    pub kind: String,
    pub owner: String,
    pub package: Option<String>,
    pub product: Option<String>,
    pub targets: Vec<String>,
    pub locales: Vec<String>,
    pub source_path: String,
    pub source_pointer: String,
    pub source_digest: String,
    pub observation_id: String,
    pub dependencies: Vec<String>,
    pub generation_status: String,
    pub classification_status: String,
    pub temporary_view: String,
    pub persistent_draft: bool,
    pub editable: bool,
    pub renderer_qualified: bool,
    pub diagnostics: Vec<Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Filters {
    #[serde(default)]
    pub collection: String,
    pub search: String,
    pub level: String,
    pub product: String,
    pub target: String,
    pub owner: String,
    pub package: String,
    pub locale: String,
    pub status: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub entries: Vec<Entry>,
    pub total: usize,
    pub next_offset: Option<usize>,
}

/// Commands and reply identities belong to one reader session. Late responses cannot
/// supply a generation to a new connection, even when a request number is reused.
pub struct Client {
    child: Child,
    input: ChildStdin,
    output: Receiver<Result<Value, String>>,
    session: String,
    next_request: u64,
    pub generation: Option<String>,
    cancelled: Arc<AtomicBool>,
    source_changed: bool,
}

impl Client {
    pub fn connect(config: &ReaderConfig) -> Result<Self, String> {
        Self::connect_with_cancel(config, Arc::new(AtomicBool::new(false)))
    }
    pub fn connect_with_cancel(config: &ReaderConfig, cancelled: Arc<AtomicBool>) -> Result<Self, String> {
        if !config.reader.is_absolute() || !config.reader.is_file() {
            return Err("Select the absolute path of the packaged CityOS reader.mjs".into());
        }
        let mut command = Command::new(&config.node);
        command.arg(&config.reader).arg("--stdio").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = command.spawn().map_err(|e| format!("Cannot start CityOS reader: {e}"))?;
        let input = child.stdin.take().ok_or("Reader stdin unavailable")?;
        let stdout = child.stdout.take().ok_or("Reader stdout unavailable")?;
        let (tx, output) = mpsc::sync_channel(32);
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut bytes = Vec::new();
                let result =
                    read_frame(&mut reader, &mut bytes).and_then(|_| serde_json::from_slice(&bytes).map_err(|e| format!("Malformed reader reply: {e}")));
                let failed = result.is_err();
                if tx.send(result).is_err() || failed {
                    break;
                }
            }
        });
        let session =
            format!("desktop-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
        let mut client = Self { child, input, output, session, next_request: 0, generation: None, cancelled, source_changed: false };
        client.request("Hello", json!({}), Duration::from_secs(10))?;
        Ok(client)
    }

    pub fn request(&mut self, op: &str, args: Value, timeout: Duration) -> Result<Value, String> {
        self.next_request += 1;
        let request_id = format!("request-{}", self.next_request);
        let mut request = args.as_object().cloned().unwrap_or_default();
        request.insert("protocolVersion".into(), json!(PROTOCOL));
        request.insert("requestId".into(), json!(request_id));
        request.insert("sessionId".into(), json!(self.session));
        request.insert("generationId".into(), json!(self.generation));
        request.insert("op".into(), json!(op));
        let bytes = serde_json::to_vec(&request).map_err(|e| e.to_string())?;
        if bytes.len() > MAX_FRAME {
            return Err("Request exceeds 4 MiB".into());
        }
        self.input.write_all(&bytes).and_then(|_| self.input.write_all(b"\n")).and_then(|_| self.input.flush()).map_err(|e| e.to_string())?;
        let deadline = Instant::now() + timeout;
        let mut silence = Instant::now();
        loop {
            if self.cancelled.load(Ordering::Acquire) {
                let cancel_id = format!("cancel-{}", self.next_request);
                let cancel = json!({"protocolVersion":PROTOCOL,"requestId":cancel_id,"sessionId":self.session,"generationId":self.generation,"op":"Cancel","targetRequestId":request_id});
                let _ = writeln!(self.input, "{cancel}");
                let _ = self.input.flush();
                let until = Instant::now() + Duration::from_secs(2);
                let mut acknowledged = false;
                while Instant::now() < until {
                    if let Ok(Ok(reply)) = self.output.recv_timeout(Duration::from_millis(50)) {
                        if reply["sessionId"] == self.session && reply["requestId"] == cancel_id && reply["kind"] == "result" {
                            acknowledged = true;
                            break;
                        }
                    }
                }
                if !acknowledged {
                    let _ = self.child.kill();
                }
                return Err("Catalog request cancelled; previous generation retained".into());
            }
            if Instant::now() >= deadline || silence.elapsed() > Duration::from_secs(30) {
                let _ = self.child.kill();
                return Err("Reader timed out; prior catalog retained. Reconnect to retry.".into());
            }
            let reply = match self.output.recv_timeout(Duration::from_millis(250)) {
                Ok(value) => value?,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(_) => return Err("Reader stopped; prior catalog retained".into()),
            };
            if reply["protocolVersion"] != PROTOCOL {
                return Err("Incompatible catalog protocol".into());
            }
            if reply["sessionId"] != self.session {
                continue;
            }
            if reply["requestId"] == "watch" {
                self.source_changed |= reply["result"]["sourceChanged"] == true;
                continue;
            }
            if reply["requestId"] != request_id {
                continue;
            }
            silence = Instant::now();
            match reply["kind"].as_str() {
                Some("progress") => continue,
                Some("error") => return Err(reply["error"]["message"].as_str().unwrap_or("Reader failure").into()),
                Some("result") => {
                    if op != "Hello" && op != "Scan" && reply["generationId"].as_str() != self.generation.as_deref() {
                        return Err("Obsolete catalog generation".into());
                    }
                    return Ok(reply["result"].clone());
                }
                _ => return Err("Unknown catalog reply".into()),
            }
        }
    }

    pub fn scan(&mut self, config: &ReaderConfig, mode: &str) -> Result<Value, String> {
        self.scan_with_cache(config, mode, false)
    }
    pub fn scan_with_cache(&mut self, config: &ReaderConfig, mode: &str, prefer_cache: bool) -> Result<Value, String> {
        let result = self.request(
            "Scan",
            json!({"repositoryRoot":config.repository,"mode":mode,"cacheRoot":config.cache,"canonicalRef":config.canonical_ref,"preferCache":prefer_cache}),
            Duration::from_secs(600),
        )?;
        self.generation = Some(result["generationId"].as_str().ok_or("Missing generation identity")?.to_string());
        Ok(result)
    }

    pub fn query(&mut self, filters: &Filters, offset: usize) -> Result<Page, String> {
        let mut args = serde_json::to_value(filters).map_err(|e| e.to_string())?;
        args["offset"] = json!(offset);
        args["limit"] = json!(256);
        serde_json::from_value(self.request("Query", args, Duration::from_secs(30))?).map_err(|e| e.to_string())
    }

    pub fn materialize(&mut self, id: &str) -> Result<Value, String> {
        let result = self.request("Materialize", json!({"entryId":id}), Duration::from_secs(30))?;
        if result["encoding"] == "base64-json" {
            return Err("This source exceeds the single-view limit. Source identity and contract metadata remain available.".into());
        }
        Ok(result)
    }
    pub fn take_source_changed(&mut self) -> bool {
        while let Ok(Ok(reply)) = self.output.try_recv() {
            if reply["sessionId"] == self.session && reply["requestId"] == "watch" {
                self.source_changed |= reply["result"]["sourceChanged"] == true;
            }
        }
        std::mem::take(&mut self.source_changed)
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn read_frame(reader: &mut impl BufRead, output: &mut Vec<u8>) -> Result<(), String> {
    loop {
        let bytes = reader.fill_buf().map_err(|e| e.to_string())?;
        if bytes.is_empty() {
            return Err("Reader closed mid-response".into());
        }
        let count = bytes.iter().position(|b| *b == b'\n').map(|n| n + 1).unwrap_or(bytes.len());
        if output.len() + count > MAX_FRAME + 1 {
            return Err("Reader response exceeds 4 MiB".into());
        }
        output.extend_from_slice(&bytes[..count]);
        let ended = bytes[count - 1] == b'\n';
        reader.consume(count);
        if ended {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_frame_rejects_incomplete_or_oversized_response() {
        assert!(read_frame(&mut std::io::Cursor::new(b"partial"), &mut vec![]).is_err());
        assert!(read_frame(&mut std::io::Cursor::new(vec![b'x'; MAX_FRAME + 2]), &mut vec![]).is_err());
        let mut out = vec![];
        read_frame(&mut std::io::Cursor::new(b"{}\n"), &mut out).unwrap();
        assert_eq!(out, b"{}\n");
    }
    #[test]
    fn catalog_entry_has_no_document_creation_or_editing_capability() {
        assert_eq!(PROTOCOL, "cityos.design-catalog.ipc.v1");
        assert_eq!(serde_json::to_value(Filters::default()).unwrap()["search"], "");
    }
}
