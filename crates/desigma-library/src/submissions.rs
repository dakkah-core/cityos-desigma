//! Durable immutable pre-dispatch records. No credentials or remote writer live here.
use crate::{write_atomic, Library, LibraryError, Result};
use desigma_doc::CityosBase;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

static SUBMISSION_LOCK: Mutex<()> = Mutex::new(());
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
    pub expected_revision: String,
    pub semantic_base: Value,
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
    fn submission_path(&self, id: &str) -> Result<PathBuf> {
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
        write_atomic(&self.payload_path(&hash)?, bytes)?;
        Ok(hash)
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
        let record = self.prepare_submission(command)?;
        if !matches!(record.state, SubmissionState::Prepared | SubmissionState::Pending) {
            return Err(LibraryError::Invalid("Inquire about the existing outcome before dispatch".into()));
        }
        dispatch(&record)
    }
    /// Deliberately no API for purging these payloads. Unresolved and accepted-but-
    /// unsynchronized records remain retention roots, including after restart.
    pub fn cityos_retention_roots(&self) -> Result<Vec<String>> {
        let mut roots = vec![];
        let folder = self.path("cityos/submissions");
        if !folder.exists() {
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
}
