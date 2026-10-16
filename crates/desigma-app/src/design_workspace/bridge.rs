//! Separate exchange process; catalog v1 remains read-only.
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc,atomic::{AtomicBool,Ordering}};
use std::time::{Duration,Instant};

const PROTOCOL: &str = "cityos.design-workspace.ipc.v1";
const MAX_FRAME: usize = 4 * 1024 * 1024;
#[derive(Clone)]
pub struct Config {
    pub node: PathBuf,
    pub bridge: PathBuf,
    pub repository: PathBuf,
    pub cache: PathBuf,
}
pub struct Bridge {
    pub jobs: SyncSender<Value>,
    pub replies: Receiver<Result<Value, String>>,
    cancel: Arc<AtomicBool>,
}
impl Drop for Bridge {fn drop(&mut self){self.cancel.store(true,Ordering::Release);}}
struct Client {
    child: Child,
    input: Option<ChildStdin>,
    output: Receiver<Result<Value, String>>,
    session: String,
    sequence: u64,
}
impl Drop for Client {
    fn drop(&mut self) {
        // EOF lets a healthy helper release its transfer leases and loopback
        // server. Cancellation still terminates a wedged helper within bounds.
        self.input.take();
        let start=Instant::now();
        while start.elapsed()<Duration::from_millis(1500) {
            if self.child.try_wait().ok().flatten().is_some(){return;}
            std::thread::sleep(Duration::from_millis(25));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn frame(reader: &mut impl BufRead) -> Result<Value, String> {
    let mut bytes = Vec::new();
    loop {
        let chunk = reader.fill_buf().map_err(|e| e.to_string())?;
        if chunk.is_empty() {
            return Err("Exchange helper stopped".into());
        }
        let end = chunk.iter().position(|b| *b == b'\n');
        let count = end.map_or(chunk.len(), |n| n + 1);
        if bytes.len() + count > MAX_FRAME {
            return Err("Exchange frame exceeds 4 MiB".into());
        }
        bytes.extend_from_slice(&chunk[..count]);
        reader.consume(count);
        if end.is_some() {
            break;
        }
    }
    let mut depth = 0usize;
    let mut quoted = false;
    let mut escaped = false;
    for b in &bytes {
        if quoted {
            if escaped {
                escaped = false;
            } else if *b == b'\\' {
                escaped = true;
            } else if *b == b'"' {
                quoted = false;
            }
        } else if *b == b'"' {
            quoted = true;
        } else if *b == b'{' || *b == b'[' {
            depth += 1;
            if depth > 64 {
                return Err("Exchange JSON depth exceeds 64".into());
            }
        } else if *b == b'}' || *b == b']' {
            depth = depth.saturating_sub(1);
        }
    }
    serde_json::from_slice(&bytes).map_err(|e| format!("Malformed exchange response: {e}"))
}
impl Client {
    fn connect(config: &Config,cancel:&AtomicBool) -> Result<Self, String> {
        if !config.bridge.is_absolute() || !config.bridge.is_file() {
            return Err("Packaged workspace.mjs is unavailable; repackage the CityOS reader".into());
        }
        let mut cmd = Command::new(&config.node);
        cmd.arg(&config.bridge).arg("--stdio").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000);
        }
        let mut child = cmd.spawn().map_err(|e| format!("Cannot start exchange helper: {e}"))?;
        if let Err(error)=desigma_cityos_catalog::limit_background_cpu(&child){let _=child.kill();let _=child.wait();return Err(error);}
        let input = child.stdin.take().ok_or("Missing exchange stdin")?;
        let stdout = child.stdout.take().ok_or("Missing exchange stdout")?;
        let (tx, output) = mpsc::sync_channel(4);
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let result = frame(&mut reader);
                let failed = result.is_err();
                if tx.send(result).is_err() || failed {
                    break;
                }
            }
        });
        let mut client = Self { child, input:Some(input), output, session: uuid::Uuid::new_v4().to_string(), sequence: 0 };
        compatible(&client.request(json!({"op":"Hello"}),cancel)?)?;
        Ok(client)
    }
    fn request(&mut self, mut request: Value,cancel:&AtomicBool) -> Result<Value, String> {
        if cancel.load(Ordering::Acquire){return Err("Obsolete workspace request cancelled".into());}
        let timeout=Duration::from_secs(if request["op"]=="Hello"{10}else{30});
        self.sequence += 1;
        let id = format!("workspace-{}", self.sequence);
        request["protocolVersion"] = json!(PROTOCOL);
        request["sessionId"] = json!(self.session);
        request["requestId"] = json!(id);
        let bytes = serde_json::to_vec(&request).map_err(|e| e.to_string())?;
        if bytes.len() > MAX_FRAME {
            return Err("Exchange request exceeds 4 MiB".into());
        }
        let input=self.input.as_mut().ok_or("Exchange helper input is closed")?;
        input.write_all(&bytes).and_then(|_| input.write_all(b"\n")).and_then(|_| input.flush()).map_err(|e| e.to_string())?;
        let started=Instant::now();
        let response=loop {
            if cancel.load(Ordering::Acquire){return Err("Obsolete workspace request cancelled".into());}
            if started.elapsed()>=timeout{return Err("Exchange timed out; draft retained".into());}
            match self.output.recv_timeout(Duration::from_millis(50)){
                Ok(response)=>break response?,
                Err(mpsc::RecvTimeoutError::Timeout)=>{},
                Err(_)=>return Err("Exchange helper stopped; draft retained".into()),
            }
        };
        if response["protocolVersion"] != PROTOCOL || response["sessionId"] != self.session || response["requestId"] != id {
            return Err("Obsolete or incompatible exchange reply".into());
        }
        if let Some(error) = response["error"].as_str() {
            return Err(error.into());
        }
        Ok(response["result"].clone())
    }
}
impl Bridge {
    pub fn spawn(config: Config) -> Self {
        let (jobs, rx) = mpsc::sync_channel::<Value>(8);
        let (tx, replies) = mpsc::channel();
        let cancel=Arc::new(AtomicBool::new(false));let canceled=cancel.clone();
        std::thread::spawn(move || {
            let mut client: Option<Client> = None;
            while let Ok(mut request) = rx.recv() {
                {
                    request["repository"] = json!(config.repository);
                    request["cacheRoot"] = json!(config.cache);
                }
                let result = crate::cityos_jobs::run(true,Some(&canceled),|| {
                    if client.is_none() {
                        client = Some(Client::connect(&config,&canceled)?);
                    }
                    let result=client.as_mut().unwrap().request(request,&canceled);
                    // Settle a failed/cancelled process while retaining the heavy
                    // job lease, so a replacement cannot overlap a live old helper.
                    if result.is_err() { client=None; }
                    result
                });
                if result.is_err() {
                    client = None;
                }
                if tx.send(result).is_err() {
                    break;
                }
            }
        });
        Self { jobs, replies,cancel }
    }
}

fn compatible(hello: &Value) -> Result<(), String> {
    let supports=|key:&str,value:&str| hello[key].as_array().is_some_and(|rows|rows.iter().any(|row|row==value));
    if !supports("exchangeVersions","cityos.design-exchange.v3") || !supports("documentContainers","desigma.document.v4")
        || ["Resources","Open","Project","Command","Export","Preview","Dependencies","ReleaseDependencies","Incoming","Reconcile"].iter().any(|op|!supports("operations",op)) {
        return Err("Packaged exchange helper is incompatible. Install the matching reader, bridge and desktop build.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn handshake_rejects_a_helper_that_would_drop_expanded_bindings_or_dependency_recovery() {
        let mut hello=json!({"exchangeVersions":["cityos.design-exchange.v3"],"documentContainers":["desigma.document.v4"],
            "operations":["Resources","Open","Project","Command","Export","Preview","Dependencies","ReleaseDependencies","Incoming","Reconcile"]});
        assert!(compatible(&hello).is_ok());hello["documentContainers"]=json!(["desigma.document.v3"]);assert!(compatible(&hello).is_err());
        hello["documentContainers"]=json!(["desigma.document.v4"]);hello["operations"]=json!(["Open","Preview"]);assert!(compatible(&hello).is_err());
    }
    #[test]
    #[ignore="requires the pinned packaged Node runtime"]
    fn obsolete_unresponsive_helpers_release_the_foreground_budget_within_two_seconds(){
        let directory=std::env::temp_dir().join(format!("desigma-cancel-{}",uuid::Uuid::new_v4()));std::fs::create_dir(&directory).unwrap();
        let script=directory.join("inert.mjs");std::fs::write(&script,"import fs from 'node:fs';process.chdir(import.meta.dirname);fs.writeFileSync('ready','ready');process.stdin.resume();setInterval(()=>{},1000);").unwrap();
        let bridge=Bridge::spawn(Config{node:std::env::var("CITYOS_DESIGN_WORKSPACE_NODE").unwrap().into(),bridge:script,repository:directory.clone(),cache:directory.clone()});
        bridge.jobs.send(json!({"op":"Resources"})).unwrap();
        let ready=Instant::now();while !directory.join("ready").is_file() && ready.elapsed()<Duration::from_secs(5){std::thread::sleep(Duration::from_millis(25));}
        assert!(directory.join("ready").is_file(),"Test helper never acquired the heavy-job lease");
        let start=Instant::now();drop(bridge);
        crate::cityos_jobs::run(true,None,||Ok(())).unwrap();assert!(start.elapsed()<Duration::from_secs(2));
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn bounded_frames_reject_oversize_depth_and_partial_response() {
        assert!(frame(&mut &b"{}\n"[..]).is_ok());
        assert!(frame(&mut &b"{\n"[..]).is_err());
        assert!(frame(&mut &b"{}"[..]).is_err());
        let deep = format!("{}0{}\n", "[".repeat(65), "]".repeat(65));
        assert!(frame(&mut deep.as_bytes()).is_err());
        let huge = vec![b' '; MAX_FRAME + 1];
        assert!(frame(&mut huge.as_slice()).is_err());
    }
}
