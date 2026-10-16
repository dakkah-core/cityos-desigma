//! Durable immutable pre-dispatch records. No credentials or remote writer live here.
use crate::{write_atomic, Library, LibraryError, Result};
use desigma_doc::CityosBase;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

pub(crate) static SUBMISSION_LOCK: Mutex<()> = Mutex::new(());
#[cfg(test)]
thread_local! {
    static DISPATCH_AFTER_PREPARE: std::cell::RefCell<Option<DispatchHook>> = const { std::cell::RefCell::new(None) };
}
#[cfg(test)]
type DispatchHook = Box<dyn FnOnce(&Library, &SubmissionRecord)>;
pub fn payload_digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn identity(value: &str) -> Result<()> {
    if value.is_empty() || value.len() > 160 || !value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        return Err(LibraryError::Invalid("Invalid submission identity".into()));
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SubmissionCommand {
    pub intent_id: String,
    pub operation_id: String,
    pub predecessor: Option<String>,
    pub owner: String,
    pub resource: String,
    pub scope: BTreeMap<String, String>,
    pub base: CityosBase,
    /// The owner's original CAS revision (canonical source digest for Git drafts).
    pub expected_revision: String,
    pub semantic_base: Value,
    /// Exact submitted semantic value, not the enclosing exchange/projection.
    pub submitted_snapshot: Value,
    pub dependency_pins: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SubmissionState {
    Prepared,
    Pending,
    Published,
    AcceptedSyncPending,
    Synchronized,
    Rejected,
    Retired,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SubmissionRecord {
    pub schema_version: String,
    pub command: SubmissionCommand,
    pub request_digest: String,
    pub state: SubmissionState,
    pub outcome: Option<Value>,
}
/// An authenticated owner-client observation, not admission or merge authority.
#[derive(Clone,Debug,Serialize,Deserialize,PartialEq)]
pub struct SubmissionObservation {
    pub operation_id:String,pub request_digest:String,pub owner:String,pub resource:String,pub scope:BTreeMap<String,String>,
    pub state:SubmissionState,pub receipt_ref:String,pub owner_revision:Option<String>,pub conclusively_ineligible:bool,pub details:Value,
}
impl SubmissionRecord {
    pub fn frozen(&self) -> bool {
        !matches!(self.state, SubmissionState::Synchronized | SubmissionState::Rejected | SubmissionState::Retired)
    }
}
fn validate(command: &SubmissionCommand) -> Result<()> {
    identity(&command.operation_id)?;
    identity(&command.intent_id)?;
    if let Some(id) = &command.predecessor {
        identity(id)?;
    }
    if command.owner.is_empty()
        || command.resource.is_empty()
        || command.expected_revision.is_empty()
        || !command.scope.contains_key("tenantId")
        || !command.scope.contains_key("actorId")
    {
        return Err(LibraryError::Invalid("Owner, resource, expected revision and verified scope are required".into()));
    }
    fn credentials(value: &Value) -> bool {
        match value {
            Value::Object(map) => map.iter().any(|(key, v)| {
                matches!(
                    key.to_ascii_lowercase().as_str(),
                    "authorization" | "password" | "access_token" | "refresh_token" | "api_key" | "private_key" | "credentials"
                ) || credentials(v)
            }),
            Value::Array(items) => items.iter().any(credentials),
            _ => false,
        }
    }
    if credentials(&command.submitted_snapshot)
        || credentials(&command.semantic_base)
        || command.scope.keys().any(|k| matches!(k.to_ascii_lowercase().as_str(), "authorization" | "password" | "access_token" | "credentials"))
    {
        return Err(LibraryError::Invalid("Credentials cannot be stored in submission records".into()));
    }
    match &command.base {
        CityosBase::CanonicalGit { repository_id, commit }
            if repository_id.is_empty() || commit.len() != 40 || !commit.bytes().all(|b| b.is_ascii_hexdigit()) =>
        {
            return Err(LibraryError::Invalid("Canonical committed base required".into()))
        }
        CityosBase::NativeOwner { owner, resource, revision } if owner != &command.owner || resource != &command.resource || revision.is_empty() => {
            return Err(LibraryError::Invalid("Native base owner/resource mismatch".into()))
        }
        _ => {}
    }
    Ok(())
}
impl Library {
    pub(crate) fn submission_path(&self, id: &str) -> Result<PathBuf> {
        identity(id)?;
        Ok(self.path("cityos/submissions").join(format!("{id}.json")))
    }
    fn payload_path(&self, hash: &str) -> Result<PathBuf> {
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(LibraryError::Invalid("Invalid payload digest".into()));
        }
        Ok(self.path("cityos/payloads").join(hash))
    }
    pub fn retain_cityos_payload(&self, bytes: &[u8]) -> Result<String> {
        let hash = payload_digest(bytes);
        self.put_cityos_payload(&hash, bytes)?;
        Ok(hash)
    }

    /// Materialize a pinned payload from the trusted bridge's artifact cache.
    /// Hash the exact bytes; JSON reformatting changes identity. This is local
    /// content retention and grants no owner admission or execution authority.
    pub fn put_cityos_payload(&self, expected_digest: &str, bytes: &[u8]) -> Result<()> {
        let path = self.payload_path(expected_digest)?;
        if payload_digest(bytes) != expected_digest {
            return Err(LibraryError::Invalid("Dependency digest mismatch".into()));
        }
        match self.verify_cityos_payload(expected_digest) {
            Ok(()) => return Ok(()),
            Err(LibraryError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {}
            // A caller supplying the verified canonical bytes may still repair
            // a damaged object, as before. Verified immutable objects are a no-op.
            Err(LibraryError::Invalid(_)) => {}
            Err(error) => return Err(error),
        }
        write_atomic(&path, bytes)?;
        self.verify_cityos_payload(expected_digest)
    }

    /// Verify a durable CAS reference without allocating the entire payload.
    pub(crate) fn verify_cityos_payload(&self, hash: &str) -> Result<()> {
        use std::io::Read;
        let mut file = std::fs::File::open(self.payload_path(hash)?)?;
        let mut digest = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 { break; }
            digest.update(&buffer[..read]);
        }
        if format!("{:x}", digest.finalize()) != hash {
            return Err(LibraryError::Invalid("Dependency digest mismatch".into()));
        }
        Ok(())
    }
    pub fn load_cityos_payload(&self, hash: &str) -> Result<Vec<u8>> {
        let bytes = std::fs::read(self.payload_path(hash)?)?;
        if payload_digest(&bytes) != hash {
            return Err(LibraryError::Invalid("Dependency digest mismatch".into()));
        }
        Ok(bytes)
    }
    pub fn load_submission(&self, id: &str) -> Result<SubmissionRecord> {
        let bytes = std::fs::read(self.submission_path(id)?)?;
        let record: SubmissionRecord = serde_json::from_slice(&bytes)?;
        validate(&record.command)?;
        if record.schema_version != "cityos.submission.v1"
            || record.command.operation_id != id
            || payload_digest(&serde_json::to_vec(&record.command)?) != record.request_digest
        {
            return Err(LibraryError::Invalid("Submission integrity failure".into()));
        }
        Ok(record)
    }
    /// On failure this API returns before the caller may send any mutation.
    pub fn prepare_submission(&self, command: SubmissionCommand) -> Result<SubmissionRecord> {
        let _guard = SUBMISSION_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        validate(&command)?;
        for hash in &command.dependency_pins {
            self.load_cityos_payload(hash)?;
        }
        let path = self.submission_path(&command.operation_id)?;
        let request_digest = payload_digest(&serde_json::to_vec(&command)?);
        if path.exists() {
            let previous = self.load_submission(&command.operation_id)?;
            if previous.command != command || previous.request_digest != request_digest {
                return Err(LibraryError::Invalid("Operation identity is immutable".into()));
            }
            return Ok(previous);
        }
        if let Some(predecessor) = &command.predecessor {
            let old = self.load_submission(predecessor)?;
            if old.state != SubmissionState::Retired || old.command.intent_id != command.intent_id {
                return Err(LibraryError::Invalid("Predecessor must be conclusively retired under the same intent".into()));
            }
        }
        // Freeze all operations for the same intent until the authoritative outcome
        // is synchronized or retired. The owner still enforces global eligibility.
        let folder = self.path("cityos/submissions");
        if folder.exists() {
            for item in std::fs::read_dir(&folder)? {
                let item = item?;
                if item.path().extension().is_some_and(|e| e == "json") {
                    let id = item.path().file_stem().unwrap().to_string_lossy().into_owned();
                    let previous = self.load_submission(&id)?;
                    if previous.command.intent_id == command.intent_id && previous.frozen() {
                        return Err(LibraryError::Invalid("An unresolved operation already freezes this intent".into()));
                    }
                }
            }
        }
        let record =
            SubmissionRecord { schema_version: "cityos.submission.v1".into(), command, request_digest, state: SubmissionState::Prepared, outcome: None };
        write_atomic(&path, &serde_json::to_vec(&record)?)?;
        let saved = self.load_submission(&record.command.operation_id)?;
        if saved != record {
            return Err(LibraryError::Invalid("Submission readback mismatch".into()));
        }
        Ok(saved)
    }
    /// The immutable durable record is the sole dispatch input. An identical retry
    /// retains its original operation/digest; inquiry is the restart entry point.
    pub fn dispatch_prepared<T>(&self, command: SubmissionCommand, dispatch: impl FnOnce(&SubmissionRecord) -> Result<T>) -> Result<T> {
        let prepared = self.prepare_submission(command)?;
        #[cfg(test)]
        if let Some(hook) = DISPATCH_AFTER_PREPARE.with(|slot| slot.borrow_mut().take()) { hook(self, &prepared); }
        let record = {
            // Inquiry/synchronization may have advanced the original record while
            // prepare's lock was released. Never overwrite acceptance with Pending.
            let _guard = SUBMISSION_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let mut record = self.load_submission(&prepared.command.operation_id)?;
            if !matches!(record.state, SubmissionState::Prepared | SubmissionState::Pending) {
                return Err(LibraryError::Invalid("Inquire about the existing outcome before dispatch".into()));
            }
            if record.state == SubmissionState::Prepared {
                record.state = SubmissionState::Pending;
                write_atomic(&self.submission_path(&record.command.operation_id)?, &serde_json::to_vec(&record)?)?;
                let saved = self.load_submission(&record.command.operation_id)?;
                if saved != record { return Err(LibraryError::Invalid("Submission dispatch readback mismatch".into())); }
                record = saved;
            }
            record
        };
        dispatch(&record)
    }
    /// Called only after the existing owner client authenticates and scopes inquiry.
    /// Receipt storage cannot qualify an owner or make an MR eligible for acceptance.
    pub fn record_submission_observation(&self,observation:SubmissionObservation)->Result<SubmissionRecord>{
        let _guard=SUBMISSION_LOCK.lock().unwrap_or_else(|e|e.into_inner());
        let mut record=self.load_submission(&observation.operation_id)?;
        if observation.request_digest!=record.request_digest||observation.owner!=record.command.owner||observation.resource!=record.command.resource||observation.scope!=record.command.scope||observation.receipt_ref.is_empty(){
            return Err(LibraryError::Invalid("Owner observation identity/scope mismatch".into()));}
        if !matches!(observation.state,SubmissionState::Pending|SubmissionState::Published|SubmissionState::AcceptedSyncPending|SubmissionState::Rejected|SubmissionState::Retired)
            || observation.state==SubmissionState::AcceptedSyncPending&&observation.owner_revision.as_ref().is_none_or(|r|r.is_empty())
            || observation.state==SubmissionState::Retired&&!observation.conclusively_ineligible {return Err(LibraryError::Invalid("Incomplete owner outcome; keep submission frozen".into()));}
        let outcome=serde_json::to_value(&observation)?;
        fn secret(value:&Value)->bool{match value {Value::Object(map)=>map.iter().any(|(k,v)|matches!(k.to_ascii_lowercase().as_str(),"authorization"|"password"|"access_token"|"refresh_token"|"api_key"|"private_key"|"credentials")||secret(v)),Value::Array(items)=>items.iter().any(secret),_=>false}}
        if secret(&outcome){return Err(LibraryError::Invalid("Credentials cannot enter owner receipts".into()));}
        if record.state==observation.state&&record.outcome.as_ref()==Some(&outcome){return Ok(record);}
        match record.state {
            SubmissionState::Rejected|SubmissionState::Retired|SubmissionState::Synchronized=>return Err(LibraryError::Invalid("Terminal outcome cannot be rewritten".into())),
            SubmissionState::AcceptedSyncPending=>return Err(LibraryError::Invalid("Accepted outcome must synchronize before any successor or retirement".into())),
            SubmissionState::Published if observation.state==SubmissionState::Pending=>return Err(LibraryError::Invalid("Publication cannot regress to pending".into())),
            _=>{}
        }
        record.state=observation.state;record.outcome=Some(outcome);
        write_atomic(&self.submission_path(&record.command.operation_id)?,&serde_json::to_vec(&record)?)?;
        self.load_submission(&record.command.operation_id)
    }
    /// Deliberately no API for purging these payloads. Unresolved and accepted-but-
    /// unsynchronized records remain retention roots, including after restart.
    pub fn cityos_retention_roots(&self) -> Result<Vec<String>> {
        let mut roots = self.local_document_retention_roots()?;
        roots.extend(self.submission_sync_retention_roots()?);
        let folder = self.path("cityos/submissions");
        if !folder.exists() {
            roots.sort();
            roots.dedup();
            return Ok(roots);
        }
        for item in std::fs::read_dir(folder)? {
            let item = item?;
            if item.path().extension().is_some_and(|e| e == "json") {
                let id = item.path().file_stem().unwrap().to_string_lossy().into_owned();
                roots.extend(self.load_submission(&id)?.command.dependency_pins);
            }
        }
        roots.sort();
        roots.dedup();
        Ok(roots)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn command() -> SubmissionCommand {
        SubmissionCommand {
            intent_id: "intent-1".into(),
            operation_id: "operation-1".into(),
            predecessor: None,
            owner: "experience".into(),
            resource: "frame".into(),
            scope: BTreeMap::from([("tenantId".into(), "t".into()), ("actorId".into(), "a".into())]),
            base: CityosBase::CanonicalGit { repository_id: "repo".into(), commit: "a".repeat(40) },
            expected_revision: "a".repeat(40),
            semantic_base: serde_json::json!({"elevation":"flat"}),
            submitted_snapshot: serde_json::json!({"elevation":"resting"}),
            dependency_pins: vec![],
        }
    }
    #[test]
    fn failed_prepare_means_zero_dispatch_and_identity_survives_restart() {
        let root = std::env::temp_dir().join(format!("desigma-submit-{}", crate::new_id()));
        let library = Library::open(&root).unwrap();
        std::fs::write(root.join("cityos"), "blocked").unwrap();
        let mut count = 0;
        assert!(library
            .dispatch_prepared(command(), |_| {
                count += 1;
                Ok(())
            })
            .is_err());
        assert_eq!(count, 0);
        std::fs::remove_file(root.join("cityos")).unwrap();
        let payload = library.retain_cityos_payload(b"immutable dependency").unwrap();
        let mut c = command();
        c.dependency_pins.push(payload.clone());
        let saved = library.prepare_submission(c.clone()).unwrap();
        drop(library);
        let reopened = Library::open(&root).unwrap();
        assert_eq!(reopened.load_submission("operation-1").unwrap(), saved);
        assert_eq!(reopened.prepare_submission(c.clone()).unwrap().request_digest, saved.request_digest);
        assert_eq!(reopened.cityos_retention_roots().unwrap(), vec![payload]);
        c.submitted_snapshot = serde_json::json!({"different":true});
        assert!(reopened.prepare_submission(c).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn credentials_and_missing_dependencies_fail_before_dispatch() {
        let root = std::env::temp_dir().join(format!("desigma-submit-{}", crate::new_id()));
        let library = Library::open(&root).unwrap();
        let mut c = command();
        c.submitted_snapshot = serde_json::json!({"credentials":"forbidden"});
        assert!(library.prepare_submission(c).is_err());
        let mut c = command();
        c.dependency_pins.push("f".repeat(64));
        assert!(library.prepare_submission(c).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
    fn observation(record:&SubmissionRecord,state:SubmissionState)->SubmissionObservation{
        SubmissionObservation{operation_id:record.command.operation_id.clone(),request_digest:record.request_digest.clone(),owner:record.command.owner.clone(),resource:record.command.resource.clone(),
            scope:record.command.scope.clone(),state,receipt_ref:"fixture-owner-inquiry".into(),owner_revision:Some("accepted-1".into()),conclusively_ineligible:false,details:serde_json::json!({"fixture":true})}
    }
    #[test]
    fn lost_acceptance_is_recovered_by_original_identity_without_dispatching_again(){
        let root=std::env::temp_dir().join(format!("desigma-inquiry-{}",crate::new_id()));let lib=Library::open(&root).unwrap();let mut effects=0;
        assert!(lib.dispatch_prepared(command(),|record|{assert_eq!(record.state,SubmissionState::Pending);effects+=1;Err::<(),_>(LibraryError::Invalid("lost response".into()))}).is_err());drop(lib);
        let lib=Library::open(&root).unwrap();let pending=lib.load_submission("operation-1").unwrap();assert!(pending.frozen());
        let outcome=observation(&pending,SubmissionState::AcceptedSyncPending);let accepted=lib.record_submission_observation(outcome.clone()).unwrap();
        assert_eq!(lib.record_submission_observation(outcome).unwrap(),accepted);
        assert!(lib.dispatch_prepared(command(),|_|{effects+=1;Ok(())}).is_err());assert_eq!(effects,1);
        let mut retirement=observation(&accepted,SubmissionState::Retired);retirement.conclusively_ineligible=true;assert!(lib.record_submission_observation(retirement).is_err());
        assert_eq!(lib.load_submission("operation-1").unwrap().command,pending.command);std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn acceptance_arriving_between_prepare_and_pending_is_never_overwritten_or_redispatched() {
        let root = std::env::temp_dir().join(format!("desigma-dispatch-race-{}", crate::new_id()));
        let library = Library::open(&root).unwrap();
        DISPATCH_AFTER_PREPARE.with(|slot| *slot.borrow_mut() = Some(Box::new(|library, prepared| {
            library.record_submission_observation(observation(prepared, SubmissionState::AcceptedSyncPending)).unwrap();
        })));
        let mut sends = 0;
        assert!(library.dispatch_prepared(command(), |_| { sends += 1; Ok(()) }).is_err());
        assert_eq!(sends, 0);
        let stored = Library::open(&root).unwrap().load_submission("operation-1").unwrap();
        assert_eq!(stored.state, SubmissionState::AcceptedSyncPending);
        assert_eq!(stored.outcome, Some(serde_json::to_value(observation(&stored, SubmissionState::AcceptedSyncPending)).unwrap()));
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn successor_remains_blocked_until_original_retirement_is_conclusive(){
        let root=std::env::temp_dir().join(format!("desigma-retire-{}",crate::new_id()));let lib=Library::open(&root).unwrap();let original=lib.prepare_submission(command()).unwrap();
        let mut successor=command();successor.operation_id="operation-2".into();successor.predecessor=Some("operation-1".into());assert!(lib.prepare_submission(successor.clone()).is_err());
        let mut retirement=observation(&original,SubmissionState::Retired);assert!(lib.record_submission_observation(retirement.clone()).is_err());
        retirement.conclusively_ineligible=true;lib.record_submission_observation(retirement).unwrap();
        assert_eq!(lib.prepare_submission(successor).unwrap().command.predecessor,Some("operation-1".into()));
        let mut other=observation(&original,SubmissionState::Published);other.scope.insert("tenantId".into(),"another".into());assert!(lib.record_submission_observation(other).is_err());
        assert_eq!(lib.load_submission("operation-1").unwrap().command,original.command);std::fs::remove_dir_all(root).unwrap();
    }
}
