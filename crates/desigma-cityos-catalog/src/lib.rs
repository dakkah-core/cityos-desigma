//! Data-only, revision-bound catalog. This crate cannot create editor documents.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
mod input;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

pub const PROTOCOL: &str = "cityos.design-catalog.ipc.v1";
pub const MAX_FRAME: usize = 4 * 1024 * 1024;

/// Limit helper threads and inherited Git children; never constrain the UI/GPU process.
pub fn limit_background_cpu(child: &Child) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetProcessAffinityMask(process: *mut std::ffi::c_void, process_mask: *mut usize, system_mask: *mut usize) -> i32;
            fn SetProcessAffinityMask(process: *mut std::ffi::c_void, mask: usize) -> i32;
            fn SetPriorityClass(process: *mut std::ffi::c_void, class: u32) -> i32;
        }
        let mut allowed = 0usize;
        let mut system = 0usize;
        // SAFETY: the live child handle and output pointers remain valid for each call.
        unsafe {
            if GetProcessAffinityMask(child.as_raw_handle(), &mut allowed, &mut system) == 0 {
                return Err("Cannot establish the helper CPU budget".into());
            }
            let mut selected = 0usize;
            for _ in 0..2 {
                if allowed == 0 {
                    break;
                }
                let bit = 1usize << allowed.trailing_zeros();
                selected |= bit;
                allowed &= !bit;
            }
            if selected == 0 || SetProcessAffinityMask(child.as_raw_handle(), selected) == 0 || SetPriorityClass(child.as_raw_handle(), 0x00004000) == 0 {
                return Err("Cannot apply the helper low-impact profile".into());
            }
        }
    }
    #[cfg(not(windows))]
    let _ = child;
    Ok(())
}

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
    input: input::Input,
    output: Receiver<Result<Value, String>>,
    session: String,
    next_request: u64,
    pub generation: Option<String>,
    cancelled: Arc<AtomicBool>,
    source_changed: bool,
    cache_only_restore: bool,
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
        if let Err(error) = limit_background_cpu(&child) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
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
        let mut client = Self {
            child,
            input: input::Input::new(input),
            output,
            session,
            next_request: 0,
            generation: None,
            cancelled,
            source_changed: false,
            cache_only_restore: false,
        };
        let hello = client.request("Hello", json!({}), Duration::from_secs(10))?;
        client.cache_only_restore = cache_restore_supported(&hello);
        Ok(client)
    }

    pub fn request(&mut self, op: &str, args: Value, timeout: Duration) -> Result<Value, String> {
        let deadline = Instant::now() + timeout;
        self.next_request += 1;
        let request_id = format!("request-{}", self.next_request);
        let mut request = args.as_object().cloned().unwrap_or_default();
        request.insert("protocolVersion".into(), json!(PROTOCOL));
        request.insert("requestId".into(), json!(request_id));
        request.insert("sessionId".into(), json!(self.session));
        request.insert("generationId".into(), json!(self.generation));
        request.insert("op".into(), json!(op));
        let bytes = serde_json::to_vec(&request).map_err(|e| e.to_string())?;
        if bytes.len() + 1 > MAX_FRAME {
            return Err("Request exceeds 4 MiB".into());
        }
        if let Err(error) = self.input.send(&request_id, bytes) {
            let _ = self.child.kill();
            return Err(error);
        }
        if let Err(error) = self.input.written(&request_id, deadline.min(Instant::now() + Duration::from_secs(30)), &self.cancelled) {
            let _ = self.child.kill();
            return Err(error);
        }
        let mut silence = Instant::now();
        loop {
            if self.cancelled.load(Ordering::Acquire) {
                let cancel_id = format!("cancel-{}", self.next_request);
                let cancel = json!({"protocolVersion":PROTOCOL,"requestId":cancel_id,"sessionId":self.session,"generationId":self.generation,"op":"Cancel","targetRequestId":request_id});
                if self.input.send(&cancel_id, serde_json::to_vec(&cancel).map_err(|e| e.to_string())?).is_err() {
                    let _ = self.child.kill();
                    return Err("Catalog request cancelled; previous generation retained".into());
                }
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
    /// Never send this additive flag to an older reader which might ignore it
    /// and run a complete source scan on the foreground recovery budget.
    pub fn restore_cache(&mut self, config: &ReaderConfig, mode: &str) -> Result<Value, String> {
        if !self.cache_only_restore {
            return Err("CACHE_RESTORE_UNAVAILABLE: reader capability unavailable".into());
        }
        let result = self.request(
            "Scan",
            json!({"repositoryRoot":config.repository,"mode":mode,"cacheRoot":config.cache,
            "canonicalRef":config.canonical_ref,"preferCache":true,"cacheOnly":true}),
            Duration::from_secs(30),
        )?;
        if result["restoredCache"] != true {
            return Err("Cache-only reader returned a non-recovery result".into());
        }
        self.generation = Some(result["generationId"].as_str().ok_or("Missing generation identity")?.to_owned());
        Ok(result)
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

fn cache_restore_supported(hello: &Value) -> bool {
    hello["capabilities"]["cacheOnlyRestore"] == true
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
        if output.len() + count > MAX_FRAME {
            return Err("Reader response exceeds 4 MiB".into());
        }
        output.extend_from_slice(&bytes[..count]);
        let ended = bytes[count - 1] == b'\n';
        reader.consume(count);
        if ended {
            let (mut depth, mut quoted, mut escaped) = (0usize, false, false);
            for byte in output.iter().copied() {
                if quoted {
                    if escaped {
                        escaped = false;
                    } else if byte == b'\\' {
                        escaped = true;
                    } else if byte == b'"' {
                        quoted = false;
                    }
                } else if byte == b'"' {
                    quoted = true;
                } else if matches!(byte, b'[' | b'{') {
                    depth += 1;
                    if depth > 64 {
                        return Err("Reader JSON depth exceeds 64".into());
                    }
                } else if matches!(byte, b']' | b'}') {
                    depth = depth.saturating_sub(1);
                }
            }
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn foreground_cache_recovery_requires_an_explicit_reader_capability() {
        for hello in
            [json!({}), json!({"capabilities":{}}), json!({"capabilities":{"cacheOnlyRestore":false}}), json!({"capabilities":{"cacheOnlyRestore":"true"}})]
        {
            assert!(!cache_restore_supported(&hello));
        }
        assert!(cache_restore_supported(&json!({"capabilities":{"cacheOnlyRestore":true}})));
    }
    #[test]
    #[ignore = "requires the pinned packaged Node runtime"]
    fn stopped_reader_stdin_does_not_block_cancellation_or_termination() {
        let directory = std::env::temp_dir().join(format!(
            "desigma-reader-pipe-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let reader = directory.join("stalled.mjs");
        std::fs::write(
            &reader,
            r#"process.stdin.once('data',bytes=>{
          const request=JSON.parse(bytes.toString().trim());process.stdin.pause();
          process.stdout.write(JSON.stringify({protocolVersion:request.protocolVersion,sessionId:request.sessionId,requestId:request.requestId,
            generationId:null,kind:'result',result:{}})+'\n');setInterval(()=>{},1000);
        });"#,
        )
        .unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut client = Client::connect_with_cancel(
            &ReaderConfig {
                node: std::env::var("CITYOS_DESIGN_WORKSPACE_NODE").unwrap().into(),
                reader,
                repository: directory.clone(),
                cache: None,
                canonical_ref: None,
            },
            cancelled.clone(),
        )
        .unwrap();
        let flag = cancelled.clone();
        let interrupt = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            flag.store(true, Ordering::Release);
        });
        let started = Instant::now();
        let error = client.request("Query", json!({"search":"x".repeat(2*1024*1024)}), Duration::from_secs(30)).unwrap_err();
        assert_eq!(error, "Catalog request cancelled; previous generation retained");
        assert!(started.elapsed() < Duration::from_millis(500));
        while client.child.try_wait().unwrap().is_none() && started.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(client.child.try_wait().unwrap().is_some());
        drop(client);
        interrupt.join().unwrap();
        assert!(started.elapsed() < Duration::from_secs(2));
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    #[ignore = "requires the pinned packaged Node runtime"]
    fn healthy_reader_cancellation_keeps_the_same_process_and_retained_generation() {
        let directory = std::env::temp_dir().join(format!(
            "desigma-reader-healthy-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let reader = directory.join("healthy.mjs");
        std::fs::write(&reader,r#"import {createInterface} from 'node:readline';let generation=null,scans=0,pending=null;
          const send=(r,result)=>process.stdout.write(JSON.stringify({protocolVersion:r.protocolVersion,sessionId:r.sessionId,requestId:r.requestId,generationId:generation,kind:'result',result})+'\n');
          createInterface({input:process.stdin}).on('line',line=>{const r=JSON.parse(line);
            if(r.op==='Hello')return send(r,{});
            if(r.op==='Scan'){scans++;if(scans===1){generation='first';return send(r,{generationId:generation,readerProcess:process.pid});}pending=r;return;}
            if(r.op==='Cancel'){const retired=pending;pending=null;send(r,{cancelled:true});if(retired)setImmediate(()=>send(retired,{generationId:'obsolete'}));return;}
            if(r.op==='Query')send(r,{readerProcess:process.pid,generation,scans});
          });"#).unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let config = ReaderConfig {
            node: std::env::var("CITYOS_DESIGN_WORKSPACE_NODE").unwrap().into(),
            reader,
            repository: directory.clone(),
            cache: None,
            canonical_ref: None,
        };
        let mut client = Client::connect_with_cancel(&config, cancelled.clone()).unwrap();
        let first = client.scan(&config, "committed").unwrap();
        cancelled.store(true, Ordering::Release);
        assert_eq!(client.scan(&config, "committed").unwrap_err(), "Catalog request cancelled; previous generation retained");
        cancelled.store(false, Ordering::Release);
        client.request("Cancel", json!({"targetRequestId":"settled-refresh"}), Duration::from_secs(2)).unwrap();
        let retained = client.request("Query", json!({}), Duration::from_secs(2)).unwrap();
        assert_eq!(retained["readerProcess"], first["readerProcess"]);
        assert_eq!(retained["generation"], "first");
        assert_eq!(retained["scans"], 2);
        assert_eq!(client.generation.as_deref(), Some("first"));
        drop(client);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    #[ignore = "requires a separately recorded real captured-cache recovery trial"]
    fn packaged_cache_only_recovery_and_first_page_preserve_the_recorded_capture() {
        let evidence = PathBuf::from(std::env::var("CITYOS_DESIGN_WORKSPACE_EVIDENCE").unwrap());
        let report: Value = serde_json::from_slice(&std::fs::read(evidence.join("CATALOG-WARM-RECOVERY-20261017.json")).unwrap()).unwrap();
        let candidate: Value = serde_json::from_slice(&std::fs::read(evidence.join("catalog-warm-reader-candidate-20261017.json")).unwrap()).unwrap();
        let cache = PathBuf::from(report["scratchCache"].as_str().unwrap());
        let pointer = cache.join("active.json");
        let accepted = std::fs::read(&pointer).unwrap();
        let config = ReaderConfig {
            node: std::env::var("CITYOS_DESIGN_WORKSPACE_NODE").unwrap().into(),
            reader: candidate["reader"].as_str().unwrap().into(),
            repository: std::env::var("CITYOS_DESIGN_WORKSPACE_REPOSITORY").unwrap().into(),
            cache: Some(cache),
            canonical_ref: Some(report["migrated"]["snapshot"]["canonicalCommit"].as_str().unwrap().into()),
        };
        let mut levels = vec![];
        for run in 0..2 {
            let began = Instant::now();
            let mut client = Client::connect(&config).unwrap();
            let recovered = client.restore_cache(&config, "committed").unwrap();
            assert_eq!(recovered["generationId"], report["migrated"]["generationId"]);
            assert_eq!(recovered["restoredCache"], true);
            assert_eq!(recovered["accountingComplete"], true);
            assert_eq!(recovered["native"]["available"], false);
            let page = client.query(&Filters { collection: "design".into(), ..Default::default() }, 0).unwrap();
            assert!(!page.entries.is_empty() && page.entries.len() <= 256);
            assert!(page.entries.iter().all(|entry| !entry.editable && !entry.renderer_qualified && !entry.persistent_draft));
            assert_eq!(std::fs::read(&pointer).unwrap(), accepted);
            println!(
                "headless_native_client run={run} first_page={} usable_ms={} native_window_qualified=false",
                page.entries.len(),
                began.elapsed().as_millis()
            );
            if run == 0 {
                for number in 0..=9 {
                    let level = format!("L{number}");
                    let mut counts = serde_json::Map::new();
                    for collection in ["sources", "design"] {
                        let page = client.query(&Filters { collection: collection.into(), level: level.clone(), ..Default::default() }, 0).unwrap();
                        assert!(page
                            .entries
                            .iter()
                            .all(|entry| entry.level.as_deref() == Some(level.as_str()) && !entry.editable && !entry.renderer_qualified));
                        counts.insert(collection.into(), json!({"total":page.total,"loaded":page.entries.len(),"nextOffset":page.next_offset}));
                    }
                    println!(
                        "captured_level {level} sources={} designs={} actual_visual_qualification=false",
                        counts["sources"]["total"], counts["design"]["total"]
                    );
                    levels.push(json!({"level":level,"queries":counts,"nativeVisualQualified":false}));
                }
            }
        }
        std::fs::write(evidence.join("catalog-native-real-level-queries-20261017.json"),serde_json::to_vec_pretty(&json!({
            "schemaVersion":"cityos.desigma.captured-level-queries.v1","evidenceClass":"packaged-reader-through-native-client", "generation":report["migrated"]["generationId"],
            "snapshot":report["migrated"]["snapshot"],"levels":levels,"nativeWindowQualified":false,"ownerWrites":false})).unwrap()).unwrap();
    }
    #[test]
    fn bounded_frame_rejects_incomplete_or_oversized_response() {
        assert!(read_frame(&mut std::io::Cursor::new(b"partial"), &mut vec![]).is_err());
        assert!(read_frame(&mut std::io::Cursor::new(vec![b'x'; MAX_FRAME + 2]), &mut vec![]).is_err());
        let mut out = vec![];
        read_frame(&mut std::io::Cursor::new(b"{}\n"), &mut out).unwrap();
        assert_eq!(out, b"{}\n");
    }
    #[test]
    fn bounded_frame_rejects_depth_over_64_and_ignores_brackets_inside_strings() {
        let deep = format!("{}0{}\n", "[".repeat(65), "]".repeat(65));
        assert!(read_frame(&mut std::io::Cursor::new(deep.as_bytes()), &mut vec![]).is_err());
        let valid = format!("{}0{}\n", "[".repeat(64), "]".repeat(64));
        assert!(read_frame(&mut std::io::Cursor::new(valid.as_bytes()), &mut vec![]).is_ok());
        let quoted = format!("{{\"text\":{}}}\n", serde_json::to_string(&format!("\"\\{}", "[".repeat(100))).unwrap());
        assert!(read_frame(&mut std::io::Cursor::new(quoted.as_bytes()), &mut vec![]).is_ok());
    }
    #[test]
    fn catalog_entry_has_no_document_creation_or_editing_capability() {
        assert_eq!(PROTOCOL, "cityos.design-catalog.ipc.v1");
        assert_eq!(serde_json::to_value(Filters::default()).unwrap()["search"], "");
    }
}
