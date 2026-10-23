//! Durable immutable pre-dispatch records. No credentials or remote writer live here.
use crate::submission_policy::{inspect_document, submission_policy};
use crate::{write_atomic, Library, LibraryError, Result};
use crate::{CheckpointStoragePolicy, SubmissionCommandFacts, UNMANAGED_STORAGE_POLICY};
use desigma_doc::{CityosBase, DocumentCheckpoint};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

pub(crate) static SUBMISSION_LOCK: Mutex<()> = Mutex::new(());
#[cfg(test)]
thread_local! {
    static DISPATCH_AFTER_PREPARE: std::cell::RefCell<Option<DispatchHook>> = const { std::cell::RefCell::new(None) };
    static SUBMISSION_FAULT: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
}
#[cfg(test)]
type DispatchHook = Box<dyn FnOnce(&Library, &SubmissionRecord)>;

fn submission_phase(_phase: &str) -> Result<()> {
    #[cfg(test)]
    if SUBMISSION_FAULT.with(|fault| {
        if fault.get() == Some(_phase) {
            fault.set(None);
            true
        } else {
            false
        }
    }) {
        return Err(std::io::Error::other(format!("Interrupted submission at {_phase}")).into());
    }
    Ok(())
}
pub fn payload_digest(bytes: &[u8]) -> String {
    crate::content_digest(bytes)
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
    /// Preserved legacy wire DTO. Only the installed policy interprets its
    /// semantics; changing this representation would change request digests.
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

// Local routing metadata belongs to the SAME atomic pre-dispatch record, not
// the owner command/digest. Legacy unbound records remain readable by identity.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct SubmissionFileBinding {
    file_id: String,
    document_id: Option<String>,
    draft_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    submitted_checkpoint: Option<String>,
    digest: String,
}

#[derive(Serialize, Deserialize)]
struct StoredSubmission {
    #[serde(flatten)]
    record: SubmissionRecord,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    local_file: Option<SubmissionFileBinding>,
}

impl SubmissionFileBinding {
    fn digest(&self, record: &SubmissionRecord) -> Result<String> {
        if let Some(checkpoint) = &self.submitted_checkpoint {
            return Ok(payload_digest(&serde_json::to_vec(&(
                "cityos.submission-file.v2",
                &self.file_id,
                &self.document_id,
                &self.draft_id,
                &record.command.operation_id,
                &record.request_digest,
                checkpoint,
            ))?));
        }
        Ok(payload_digest(&serde_json::to_vec(&(
            "cityos.submission-file.v1",
            &self.file_id,
            &self.document_id,
            &self.draft_id,
            &record.command.operation_id,
            &record.request_digest,
        ))?))
    }
}

fn validate_submitted_checkpoint(policy: &dyn CheckpointStoragePolicy, command: &SubmissionCommand, checkpoint: &DocumentCheckpoint) -> Result<()> {
    if !inspect_document(policy, command, &checkpoint.document)?.is_some_and(|facts| facts.matches_submitted) {
        return Err(LibraryError::Invalid("Command does not match the exact persisted submitted draft".into()));
    }
    Ok(())
}
/// An authenticated owner-client observation, not admission or merge authority.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SubmissionObservation {
    pub operation_id: String,
    pub request_digest: String,
    pub owner: String,
    pub resource: String,
    pub scope: BTreeMap<String, String>,
    pub state: SubmissionState,
    pub receipt_ref: String,
    pub owner_revision: Option<String>,
    pub conclusively_ineligible: bool,
    pub details: Value,
}
impl SubmissionRecord {
    pub fn frozen(&self) -> bool {
        !matches!(self.state, SubmissionState::Synchronized | SubmissionState::Rejected | SubmissionState::Retired)
    }
}
fn validate(policy: &dyn CheckpointStoragePolicy, command: &SubmissionCommand) -> Result<SubmissionCommandFacts> {
    identity(&command.operation_id)?;
    identity(&command.intent_id)?;
    if let Some(id) = &command.predecessor {
        identity(id)?;
    }
    if command.owner.trim().is_empty() || command.resource.trim().is_empty() || command.expected_revision.trim().is_empty() {
        return Err(LibraryError::Invalid("Owner, resource, expected revision and verified scope are required".into()));
    }
    if command.dependency_pins.iter().any(|pin| pin.len() != 64 || !pin.bytes().all(|byte| byte.is_ascii_hexdigit())) {
        return Err(LibraryError::Invalid("Invalid submission dependency identity".into()));
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
    submission_policy(policy)?.validate_command(command)
}
impl Library {
    pub(crate) fn submission_path(&self, id: &str) -> Result<PathBuf> {
        identity(id)?;
        Ok(self.path("cityos/submissions").join(format!("{id}.json")))
    }
    pub fn retain_cityos_payload(&self, bytes: &[u8]) -> Result<String> {
        self.payloads("cityos")?.retain(bytes)
    }

    /// Materialize a pinned payload from the trusted bridge's artifact cache.
    /// Hash the exact bytes; JSON reformatting changes identity. This is local
    /// content retention and grants no owner admission or execution authority.
    pub fn put_cityos_payload(&self, expected_digest: &str, bytes: &[u8]) -> Result<()> {
        self.payloads("cityos")?.put(expected_digest, bytes)
    }

    /// Verify a durable CAS reference without allocating the entire payload.
    pub(crate) fn verify_cityos_payload(&self, hash: &str) -> Result<()> {
        self.payloads("cityos")?.verify(hash)
    }
    pub fn load_cityos_payload(&self, hash: &str) -> Result<Vec<u8>> {
        self.payloads("cityos")?.load(hash)
    }
    fn load_stored_submission(&self, policy: &dyn CheckpointStoragePolicy, id: &str) -> Result<StoredSubmission> {
        let bytes = std::fs::read(self.submission_path(id)?)?;
        let wire: Value = serde_json::from_slice(&bytes)?;
        if wire.as_object().is_none_or(|object| {
            object.keys().any(|key| !matches!(key.as_str(), "schema_version" | "command" | "request_digest" | "state" | "outcome" | "local_file"))
        }) {
            return Err(LibraryError::Invalid("Unknown submission record fields".into()));
        }
        let stored: StoredSubmission = serde_json::from_slice(&bytes)?;
        let record = &stored.record;
        validate(policy, &record.command)?;
        if record.schema_version != "cityos.submission.v1"
            || record.command.operation_id != id
            || payload_digest(&serde_json::to_vec(&record.command)?) != record.request_digest
        {
            return Err(LibraryError::Invalid("Submission integrity failure".into()));
        }
        match (&record.state, &record.outcome) {
            (SubmissionState::Prepared | SubmissionState::Pending, None) => {}
            (_, Some(value)) => {
                let observation: SubmissionObservation = serde_json::from_value(value.clone())?;
                let expected = if record.state == SubmissionState::Synchronized { SubmissionState::AcceptedSyncPending } else { record.state.clone() };
                if !matches!(
                    expected,
                    SubmissionState::Pending
                        | SubmissionState::Published
                        | SubmissionState::AcceptedSyncPending
                        | SubmissionState::Rejected
                        | SubmissionState::Retired
                ) || observation.state != expected
                    || observation.operation_id != id
                    || observation.request_digest != record.request_digest
                    || observation.owner != record.command.owner
                    || observation.resource != record.command.resource
                    || observation.scope != record.command.scope
                    || observation.receipt_ref.trim().is_empty()
                    || (expected == SubmissionState::AcceptedSyncPending
                        && (observation.conclusively_ineligible || observation.owner_revision.as_ref().is_none_or(|revision| revision.trim().is_empty())))
                    || (expected == SubmissionState::Retired && !observation.conclusively_ineligible)
                {
                    return Err(LibraryError::Invalid("Stored owner observation integrity failure".into()));
                }
            }
            _ => return Err(LibraryError::Invalid("Missing original owner observation".into())),
        }
        if let Some(binding) = &stored.local_file {
            crate::validate_id(&binding.file_id)?;
            if binding.document_id.is_some() != binding.draft_id.is_some()
                || binding.document_id.iter().chain(binding.draft_id.iter()).any(|id| id.trim().is_empty())
                || binding.digest != binding.digest(record)?
            {
                return Err(LibraryError::Invalid("Submission file binding integrity failure".into()));
            }
            if let Some(text) = &binding.submitted_checkpoint {
                let checkpoint = DocumentCheckpoint::from_json(text)?;
                validate_submitted_checkpoint(policy, &record.command, &checkpoint)?;
                let facts = inspect_document(policy, &record.command, &checkpoint.document)?;
                if binding.document_id.as_deref() != facts.as_ref().map(|facts| facts.document_id.as_str())
                    || binding.draft_id.as_deref() != facts.as_ref().map(|facts| facts.draft_id.as_str())
                {
                    return Err(LibraryError::Invalid("Submitted checkpoint identity mismatch".into()));
                }
            }
        }
        Ok(stored)
    }

    /// Inquiry by ORIGINAL identity remains available even when dependencies or
    /// the local document are missing. This performs no dispatch or state change.
    pub fn load_submission(&self, id: &str) -> Result<SubmissionRecord> {
        self.load_submission_with_policy(&UNMANAGED_STORAGE_POLICY, id)
    }
    pub fn load_submission_with_policy(&self, policy: &dyn CheckpointStoragePolicy, id: &str) -> Result<SubmissionRecord> {
        Ok(self.load_stored_submission(policy, id)?.record)
    }

    /// Caller holds SUBMISSION_LOCK. All state transitions retain the original
    /// file association and command; an observation must never erase routing.
    pub(crate) fn write_submission_record(&self, policy: &dyn CheckpointStoragePolicy, record: &SubmissionRecord) -> Result<()> {
        let mut stored = self.load_stored_submission(policy, &record.command.operation_id)?;
        if stored.record.command != record.command
            || stored.record.request_digest != record.request_digest
            || stored.record.schema_version != record.schema_version
        {
            return Err(LibraryError::Invalid("Operation identity is immutable".into()));
        }
        stored.record = record.clone();
        write_atomic(&self.submission_path(&record.command.operation_id)?, &serde_json::to_vec(&stored)?)
    }

    pub(crate) fn check_submission_file(
        &self,
        policy: &dyn CheckpointStoragePolicy,
        operation_id: &str,
        file_id: &str,
        checkpoint: &DocumentCheckpoint,
    ) -> Result<()> {
        let stored = self.load_stored_submission(policy, operation_id)?;
        if let Some(binding) = stored.local_file {
            let facts = inspect_document(policy, &stored.record.command, &checkpoint.document)?;
            if binding.file_id != file_id
                || binding.document_id.as_deref() != facts.as_ref().map(|facts| facts.document_id.as_str())
                || binding.draft_id.as_deref() != facts.as_ref().map(|facts| facts.draft_id.as_str())
            {
                return Err(LibraryError::Invalid("Submission belongs to a different local file/document/draft".into()));
            }
            if let Some(submitted) = &binding.submitted_checkpoint {
                if checkpoint.to_json()? != *submitted {
                    return Err(LibraryError::CheckpointConflict(file_id.into()));
                }
            }
        }
        Ok(())
    }

    /// Read-only restart/inquiry listing, ordered by original operation ID. No
    /// payload/file reads are required, including for trashed or incomplete files.
    /// Legacy unbound records are accessible via load_submission, never guessed
    /// into this list. Any malformed record fails closed rather than hiding it.
    pub fn document_submissions(&self, file_id: &str) -> Result<Vec<SubmissionRecord>> {
        self.document_submissions_with_policy(&UNMANAGED_STORAGE_POLICY, file_id)
    }
    pub fn document_submissions_with_policy(&self, policy: &dyn CheckpointStoragePolicy, file_id: &str) -> Result<Vec<SubmissionRecord>> {
        crate::validate_id(file_id)?;
        let _guard = SUBMISSION_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let entries = match std::fs::read_dir(self.path("cityos/submissions")) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut records = Vec::new();
        for entry in entries {
            let path = entry?.path();
            if path.extension().is_some_and(|extension| extension == "json") {
                let id = path.file_stem().and_then(|id| id.to_str()).ok_or_else(|| LibraryError::Invalid("Invalid submission path".into()))?;
                let stored = self.load_stored_submission(policy, id)?;
                if stored.local_file.is_some_and(|binding| binding.file_id == file_id) {
                    records.push(stored.record);
                }
            }
        }
        records.sort_by(|a, b| a.command.operation_id.cmp(&b.command.operation_id));
        Ok(records)
    }

    /// Caller holds SUBMISSION_LOCK before acquiring the document writer lock,
    /// matching prepare's order. A portable copy cannot shed an original
    /// unresolved effect merely by selecting another file ID for the same draft.
    pub(crate) fn ensure_portable_submission_safe_locked(
        &self,
        policy: &dyn CheckpointStoragePolicy,
        file_id: Option<&str>,
        checkpoint: &DocumentCheckpoint,
    ) -> Result<()> {
        if let Some(id) = file_id {
            crate::validate_id(id)?;
        }
        let states =
            std::iter::once((&checkpoint.document, &checkpoint.history)).chain(checkpoint.retained.iter().map(|state| (&state.document, &state.history)));
        let documents: Vec<_> = states.flat_map(|(document, history)| std::iter::once(document).chain(history.retained_documents())).collect();
        let entries = match std::fs::read_dir(self.path("cityos/submissions")) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        for entry in entries {
            let path = entry?.path();
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            let id = path.file_stem().and_then(|id| id.to_str()).ok_or_else(|| LibraryError::Invalid("Invalid submission path".into()))?;
            let stored = self.load_stored_submission(policy, id)?;
            if !stored.record.frozen() {
                continue;
            }
            let same_file = stored.local_file.as_ref().is_some_and(|binding| file_id == Some(binding.file_id.as_str()));
            let mut same_draft = false;
            for document in &documents {
                if let Some(facts) = inspect_document(policy, &stored.record.command, document)? {
                    same_draft |= stored.local_file.as_ref().is_some_and(|binding| {
                        binding.document_id.as_deref() == Some(facts.document_id.as_str()) && binding.draft_id.as_deref() == Some(facts.draft_id.as_str())
                    }) || stored.local_file.is_none() && facts.matches_legacy_effect;
                }
            }
            if same_file || same_draft {
                return Err(LibraryError::Invalid("Unresolved submission requires original-journal inquiry before portable export or import".into()));
            }
        }
        Ok(())
    }
    /// On failure this API returns before the caller may send any mutation.
    pub fn prepare_submission(&self, command: SubmissionCommand) -> Result<SubmissionRecord> {
        self.prepare_submission_with_policy(&UNMANAGED_STORAGE_POLICY, command)
    }
    pub fn prepare_submission_with_policy(&self, policy: &dyn CheckpointStoragePolicy, command: SubmissionCommand) -> Result<SubmissionRecord> {
        self.prepare_submission_bound(policy, None, command)
    }

    /// Atomically persist the file association AND original command before any
    /// dispatch, including the exact persisted submitted document/history. A new
    /// intent cannot bypass an unresolved operation on this file or document/draft.
    /// Existing operations cannot be rebound or retroactively claimed. The command
    /// schema and owner request digest are unchanged.
    pub fn prepare_submission_for_file(&self, file_id: &str, command: SubmissionCommand) -> Result<SubmissionRecord> {
        self.prepare_submission_for_file_with_policy(&UNMANAGED_STORAGE_POLICY, file_id, command)
    }
    pub fn prepare_submission_for_file_with_policy(
        &self,
        policy: &dyn CheckpointStoragePolicy,
        file_id: &str,
        command: SubmissionCommand,
    ) -> Result<SubmissionRecord> {
        crate::validate_id(file_id)?;
        self.prepare_submission_bound(policy, Some(file_id), command)
    }

    fn prepare_submission_bound(&self, policy: &dyn CheckpointStoragePolicy, file_id: Option<&str>, command: SubmissionCommand) -> Result<SubmissionRecord> {
        let _guard = SUBMISSION_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let command_facts = validate(policy, &command)?;
        let path = self.submission_path(&command.operation_id)?;
        let request_digest = payload_digest(&serde_json::to_vec(&command)?);
        // An identical retry is an inquiry about the original record, not a new
        // prepare. Lost CAS bytes must not make its identity/outcome unreachable.
        match std::fs::read(&path) {
            Ok(_) => {
                let stored = self.load_stored_submission(policy, &command.operation_id)?;
                let previous = stored.record;
                if previous.command != command || previous.request_digest != request_digest {
                    return Err(LibraryError::Invalid("Operation identity is immutable".into()));
                }
                if file_id.is_some_and(|file| stored.local_file.as_ref().is_none_or(|binding| binding.file_id != file)) {
                    return Err(LibraryError::Invalid("Original operation has no matching durable file association".into()));
                }
                return Ok(previous);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        for hash in &command.dependency_pins {
            self.verify_cityos_payload(hash)?;
        }
        // Pin the complete persisted generation under the writer lock. Both
        // history stacks, inactive archives, assets and dependencies are retained.
        let _writer = file_id.map(|_| crate::store::write_lock(self)).transpose()?;
        let document = file_id.map(|file| self.load_checkpoint_locked(file)).transpose()?;
        if let Some(checkpoint) = &document {
            validate_submitted_checkpoint(policy, &command, checkpoint)?;
        }
        let document_facts = document.as_ref().map(|checkpoint| inspect_document(policy, &command, &checkpoint.document)).transpose()?.flatten();
        if let Some(predecessor) = &command.predecessor {
            let old = self.load_submission_with_policy(policy, predecessor)?;
            if old.state != SubmissionState::Retired || old.command.intent_id != command.intent_id {
                return Err(LibraryError::Invalid("Predecessor must be conclusively retired under the same intent".into()));
            }
        }
        // Freeze unresolved original effects independently of caller-generated
        // intent IDs. The owner still enforces global eligibility.
        let folder = self.path("cityos/submissions");
        if folder.exists() {
            for item in std::fs::read_dir(&folder)? {
                let item = item?;
                if item.path().extension().is_some_and(|e| e == "json") {
                    let id = item.path().file_stem().unwrap().to_string_lossy().into_owned();
                    let previous = self.load_stored_submission(policy, &id)?;
                    let same_file_or_draft = previous.local_file.as_ref().is_some_and(|binding| {
                        file_id == Some(binding.file_id.as_str())
                            || document_facts.as_ref().is_some_and(|facts| {
                                binding.document_id.as_deref() == Some(facts.document_id.as_str())
                                    && binding.draft_id.as_deref() == Some(facts.draft_id.as_str())
                            })
                    });
                    // An unbound legacy call cannot sidestep a known unresolved
                    // effect by omitting the association and minting a new intent.
                    let ambiguous_legacy_effect = (file_id.is_none() || previous.local_file.is_none())
                        && previous.record.command.owner == command.owner
                        && previous.record.command.resource == command.resource
                        && validate(policy, &previous.record.command)?.overlap_scope == command_facts.overlap_scope;
                    if previous.record.frozen() && (previous.record.command.intent_id == command.intent_id || same_file_or_draft || ambiguous_legacy_effect) {
                        return Err(LibraryError::Invalid("An unresolved operation freezes this file/draft or original effect".into()));
                    }
                }
            }
        }
        let record =
            SubmissionRecord { schema_version: "cityos.submission.v1".into(), command, request_digest, state: SubmissionState::Prepared, outcome: None };
        let local_file = file_id
            .map(|file| {
                let mut binding = SubmissionFileBinding {
                    file_id: file.into(),
                    document_id: document_facts.as_ref().map(|facts| facts.document_id.clone()),
                    draft_id: document_facts.as_ref().map(|facts| facts.draft_id.clone()),
                    submitted_checkpoint: document.as_ref().map(DocumentCheckpoint::to_json).transpose()?,
                    digest: String::new(),
                };
                binding.digest = binding.digest(&record)?;
                Ok::<_, LibraryError>(binding)
            })
            .transpose()?;
        let stored = StoredSubmission { record: record.clone(), local_file };
        submission_phase("before-prepare")?;
        write_atomic(&path, &serde_json::to_vec(&stored)?)?;
        submission_phase("prepared")?;
        let saved = self.load_stored_submission(policy, &record.command.operation_id)?;
        if saved.record != record || saved.local_file != stored.local_file {
            return Err(LibraryError::Invalid("Submission readback mismatch".into()));
        }
        Ok(saved.record)
    }
    /// The immutable durable record is the sole dispatch input. An identical retry
    /// retains its original operation/digest; inquiry is the restart entry point.
    pub fn dispatch_prepared<T>(&self, command: SubmissionCommand, dispatch: impl FnOnce(&SubmissionRecord) -> Result<T>) -> Result<T> {
        self.dispatch_prepared_with_policy(&UNMANAGED_STORAGE_POLICY, command, dispatch)
    }
    pub fn dispatch_prepared_with_policy<T>(
        &self,
        policy: &dyn CheckpointStoragePolicy,
        command: SubmissionCommand,
        dispatch: impl FnOnce(&SubmissionRecord) -> Result<T>,
    ) -> Result<T> {
        let prepared = self.prepare_submission_with_policy(policy, command)?;
        self.dispatch_submission(policy, prepared, dispatch)
    }

    /// File-bound counterpart of dispatch_prepared. On restart, list/inquire
    /// original records first; never create a replacement operation to retry.
    pub fn dispatch_prepared_for_file<T>(&self, file_id: &str, command: SubmissionCommand, dispatch: impl FnOnce(&SubmissionRecord) -> Result<T>) -> Result<T> {
        self.dispatch_prepared_for_file_with_policy(&UNMANAGED_STORAGE_POLICY, file_id, command, dispatch)
    }
    pub fn dispatch_prepared_for_file_with_policy<T>(
        &self,
        policy: &dyn CheckpointStoragePolicy,
        file_id: &str,
        command: SubmissionCommand,
        dispatch: impl FnOnce(&SubmissionRecord) -> Result<T>,
    ) -> Result<T> {
        let prepared = self.prepare_submission_for_file_with_policy(policy, file_id, command)?;
        self.dispatch_submission(policy, prepared, dispatch)
    }

    fn dispatch_submission<T>(
        &self,
        policy: &dyn CheckpointStoragePolicy,
        prepared: SubmissionRecord,
        dispatch: impl FnOnce(&SubmissionRecord) -> Result<T>,
    ) -> Result<T> {
        #[cfg(test)]
        if let Some(hook) = DISPATCH_AFTER_PREPARE.with(|slot| slot.borrow_mut().take()) {
            hook(self, &prepared);
        }
        let record = {
            // Inquiry/synchronization may have advanced the original record while
            // prepare's lock was released. Never overwrite acceptance with Pending.
            let _guard = SUBMISSION_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let mut record = self.load_submission_with_policy(policy, &prepared.command.operation_id)?;
            if !matches!(record.state, SubmissionState::Prepared | SubmissionState::Pending) {
                return Err(LibraryError::Invalid("Inquire about the existing outcome before dispatch".into()));
            }
            for hash in &record.command.dependency_pins {
                self.verify_cityos_payload(hash)?;
            }
            let stored = self.load_stored_submission(policy, &record.command.operation_id)?;
            let _writer = stored.local_file.as_ref().map(|_| crate::store::write_lock(self)).transpose()?;
            if let Some(binding) = &stored.local_file {
                if binding.submitted_checkpoint.is_none() {
                    return Err(LibraryError::Invalid("Original checkpoint unavailable; retain identity and inquire".into()));
                }
                let current = self.load_checkpoint_locked(&binding.file_id)?;
                self.check_submission_file(policy, &record.command.operation_id, &binding.file_id, &current)?;
            }
            if record.state == SubmissionState::Prepared {
                record.state = SubmissionState::Pending;
                self.write_submission_record(policy, &record)?;
                submission_phase("pending")?;
                let saved = self.load_submission_with_policy(policy, &record.command.operation_id)?;
                if saved != record {
                    return Err(LibraryError::Invalid("Submission dispatch readback mismatch".into()));
                }
                record = saved;
            }
            record
        };
        submission_phase("before-dispatch")?;
        dispatch(&record)
    }
    /// Called only after the existing owner client authenticates and scopes inquiry.
    /// Receipt storage cannot qualify an owner or make an MR eligible for acceptance.
    pub fn record_submission_observation(&self, observation: SubmissionObservation) -> Result<SubmissionRecord> {
        self.record_submission_observation_with_policy(&UNMANAGED_STORAGE_POLICY, observation)
    }
    pub fn record_submission_observation_with_policy(
        &self,
        policy: &dyn CheckpointStoragePolicy,
        observation: SubmissionObservation,
    ) -> Result<SubmissionRecord> {
        let _guard = SUBMISSION_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut record = self.load_submission_with_policy(policy, &observation.operation_id)?;
        if observation.request_digest != record.request_digest
            || observation.owner != record.command.owner
            || observation.resource != record.command.resource
            || observation.scope != record.command.scope
            || observation.receipt_ref.trim().is_empty()
        {
            return Err(LibraryError::Invalid("Owner observation identity/scope mismatch".into()));
        }
        if !matches!(
            observation.state,
            SubmissionState::Pending | SubmissionState::Published | SubmissionState::AcceptedSyncPending | SubmissionState::Rejected | SubmissionState::Retired
        ) || observation.state == SubmissionState::AcceptedSyncPending
            && (observation.conclusively_ineligible || observation.owner_revision.as_ref().is_none_or(|r| r.trim().is_empty()))
            || observation.state == SubmissionState::Retired && !observation.conclusively_ineligible
        {
            return Err(LibraryError::Invalid("Incomplete owner outcome; keep submission frozen".into()));
        }
        let outcome = serde_json::to_value(&observation)?;
        fn secret(value: &Value) -> bool {
            match value {
                Value::Object(map) => map.iter().any(|(k, v)| {
                    matches!(
                        k.to_ascii_lowercase().as_str(),
                        "authorization" | "password" | "access_token" | "refresh_token" | "api_key" | "private_key" | "credentials"
                    ) || secret(v)
                }),
                Value::Array(items) => items.iter().any(secret),
                _ => false,
            }
        }
        if secret(&outcome) {
            return Err(LibraryError::Invalid("Credentials cannot enter owner receipts".into()));
        }
        if record.state == observation.state && record.outcome.as_ref() == Some(&outcome) {
            return Ok(record);
        }
        match record.state {
            SubmissionState::Rejected | SubmissionState::Retired | SubmissionState::Synchronized => {
                return Err(LibraryError::Invalid("Terminal outcome cannot be rewritten".into()))
            }
            SubmissionState::AcceptedSyncPending => {
                return Err(LibraryError::Invalid("Accepted outcome must synchronize before any successor or retirement".into()))
            }
            SubmissionState::Published if observation.state == SubmissionState::Pending => {
                return Err(LibraryError::Invalid("Publication cannot regress to pending".into()))
            }
            _ => {}
        }
        record.state = observation.state;
        record.outcome = Some(outcome);
        self.write_submission_record(policy, &record)?;
        submission_phase("observation")?;
        self.load_submission_with_policy(policy, &record.command.operation_id)
    }
    /// Deliberately no API for purging these payloads. Unresolved and accepted-but-
    /// unsynchronized records remain retention roots, including after restart.
    pub fn cityos_retention_roots(&self) -> Result<Vec<String>> {
        self.retention_roots_with_policy(&crate::UNMANAGED_STORAGE_POLICY)
    }
    /// Extension policy supplies checkpoint interpretation. Original submission
    /// and synchronization journals remain retention roots in every state.
    pub fn retention_roots_with_policy(&self, policy: &dyn crate::CheckpointStoragePolicy) -> Result<Vec<String>> {
        let mut roots = self.local_document_retention_roots(policy)?;
        roots.extend(self.submission_sync_retention_roots(policy)?);
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
                let stored = self.load_stored_submission(policy, &id)?;
                roots.extend(stored.record.command.dependency_pins);
                if let Some(text) = stored.local_file.and_then(|binding| binding.submitted_checkpoint) {
                    roots.extend(crate::storage_policy::checked_retention_roots(policy, &DocumentCheckpoint::from_json(&text)?)?);
                }
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
            .dispatch_prepared_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, command(), |_| {
                count += 1;
                Ok(())
            })
            .is_err());
        assert_eq!(count, 0);
        std::fs::remove_file(root.join("cityos")).unwrap();
        let payload = library.retain_cityos_payload(b"immutable dependency").unwrap();
        let mut c = command();
        c.dependency_pins.push(payload.clone());
        let saved = library.prepare_submission_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, c.clone()).unwrap();
        drop(library);
        let reopened = Library::open(&root).unwrap();
        assert_eq!(reopened.load_submission_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, "operation-1").unwrap(), saved);
        assert_eq!(
            reopened.prepare_submission_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, c.clone()).unwrap().request_digest,
            saved.request_digest
        );
        assert_eq!(reopened.retention_roots_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY).unwrap(), vec![payload]);
        c.submitted_snapshot = serde_json::json!({"different":true});
        assert!(reopened.prepare_submission_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, c).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn credentials_and_missing_dependencies_fail_before_dispatch() {
        let root = std::env::temp_dir().join(format!("desigma-submit-{}", crate::new_id()));
        let library = Library::open(&root).unwrap();
        let mut c = command();
        c.submitted_snapshot = serde_json::json!({"credentials":"forbidden"});
        assert!(library.prepare_submission_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, c).is_err());
        let mut c = command();
        c.dependency_pins.push("f".repeat(64));
        assert!(library.prepare_submission_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, c).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
    fn observation(record: &SubmissionRecord, state: SubmissionState) -> SubmissionObservation {
        SubmissionObservation {
            operation_id: record.command.operation_id.clone(),
            request_digest: record.request_digest.clone(),
            owner: record.command.owner.clone(),
            resource: record.command.resource.clone(),
            scope: record.command.scope.clone(),
            state,
            receipt_ref: "fixture-owner-inquiry".into(),
            owner_revision: Some("accepted-1".into()),
            conclusively_ineligible: false,
            details: serde_json::json!({"fixture":true}),
        }
    }

    struct BoundFixture {
        root: PathBuf,
        library: Library,
        file: String,
        command: SubmissionCommand,
    }
    impl BoundFixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("desigma-bound-submission-{}", crate::new_id()));
            let library = Library::open(&root).unwrap();
            let project = library.create_project("P").unwrap();
            let mut command = command();
            command.semantic_base = serde_json::json!({"id":"frame", "elevation":"flat"});
            command.submitted_snapshot = serde_json::json!({"id":"frame", "elevation":"resting"});
            command.dependency_pins.push(library.retain_cityos_payload(b"original operation dependency").unwrap());
            let mut document = desigma_doc::Document::new("draft");
            document.cityos = Some(desigma_doc::CityosBinding {
                schema_version: "cityos.design-exchange.v3".into(),
                document_id: "document".into(),
                draft_id: "draft".into(),
                base: command.base.clone(),
                semantic_base: command.semantic_base.clone(),
                dependency_pins: vec![],
                occurrences: BTreeMap::new(),
                exchange_v2: None,
                exchange_v3: Some(serde_json::json!({
                    "schemaVersion":"cityos.design-exchange.v3", "documentId":"document", "draftId":"draft", "draftRevision":0,
                    "semanticContract":"fixture", "profileRef":"fixture", "commandReceipts":[],
                    "base":{"kind":"canonical-git", "repositoryId":"repo", "commit":"a".repeat(40), "sourceDigest":command.expected_revision},
                    "semanticBase":command.semantic_base, "semanticDraft":command.submitted_snapshot,
                })),
            });
            let file = library.create_file_checkpoint(&project.id, None, "F", &DocumentCheckpoint::new(document, desigma_doc::History::default())).unwrap().id;
            Self { root, library, file, command }
        }
    }
    impl Drop for BoundFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn managed_submission_requires_explicit_policy_before_prepare_dispatch_or_observation() {
        let f = BoundFixture::new();
        let path = f.library.submission_path(&f.command.operation_id).unwrap();
        let checkpoint = f.library.load_checkpoint(&f.file).unwrap().to_json().unwrap();
        let mut dispatches = 0;
        assert!(f.library.prepare_submission(f.command.clone()).is_err());
        assert!(f.library.prepare_submission_for_file(&f.file, f.command.clone()).is_err());
        assert!(f
            .library
            .dispatch_prepared(f.command.clone(), |_| {
                dispatches += 1;
                Ok(())
            })
            .is_err());
        assert!(f
            .library
            .dispatch_prepared_for_file(&f.file, f.command.clone(), |_| {
                dispatches += 1;
                Ok(())
            })
            .is_err());
        assert_eq!(dispatches, 0);
        assert!(!path.exists());
        let policy = &crate::storage_policy::tests::BOUND_FIXTURE_POLICY;
        let original = f.library.prepare_submission_for_file_with_policy(policy, &f.file, f.command.clone()).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(f.library.load_submission(&f.command.operation_id).is_err());
        assert!(f.library.document_submissions(&f.file).is_err());
        assert!(f.library.record_submission_observation(observation(&original, SubmissionState::Published)).is_err());
        assert_eq!(std::fs::read(path).unwrap(), bytes);
        assert_eq!(f.library.load_checkpoint(&f.file).unwrap().to_json().unwrap(), checkpoint);
        assert_eq!(f.library.load_submission_with_policy(policy, &f.command.operation_id).unwrap(), original);
        assert_eq!(f.library.document_submissions_with_policy(policy, &f.file).unwrap(), [original]);
    }

    #[test]
    fn frozen_submissions_cannot_leave_their_recovery_journal_through_portable_export_or_aliases() {
        let f = BoundFixture::new();
        let checkpoint = f.library.load_checkpoint(&f.file).unwrap();
        let project = f.library.file(&f.file).unwrap().project;
        let alias = f.library.create_file_checkpoint(&project, None, "Same draft alias", &checkpoint).unwrap();
        let portable = f.library.export_checkpoint_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &f.file).unwrap();
        let original =
            f.library.prepare_submission_for_file_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &f.file, f.command.clone()).unwrap();
        for file in [&f.file, &alias.id] {
            assert!(
                f.library.export_checkpoint_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, file).is_err(),
                "A portable draft must not drop its frozen submission identity"
            );
            assert!(
                f.library.import_checkpoint_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, file, &portable).is_err(),
                "An earlier export cannot bypass the original frozen journal"
            );
        }
        let before = f.library.files_in(&project, None).unwrap().len();
        let verified = crate::VerifiedPortableCheckpoint::parse_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &portable).unwrap();
        assert!(f.library.import_checkpoint_as_new_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &project, None, verified).is_err());
        assert_eq!(f.library.files_in(&project, None).unwrap().len(), before);
        assert_eq!(
            f.library.load_submission_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &original.command.operation_id).unwrap(),
            original
        );
        assert!(f.library.load_checkpoint(&f.file).is_ok());
    }

    #[test]
    fn bound_prepare_dispatch_and_observation_interruptions_retain_one_original_record() {
        for phase in ["before-prepare", "prepared", "pending", "before-dispatch", "observation"] {
            let f = BoundFixture::new();
            SUBMISSION_FAULT.with(|fault| fault.set(Some(phase)));
            let mut sends = 0;
            let outcome =
                f.library.dispatch_prepared_for_file_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &f.file, f.command.clone(), |record| {
                    sends += 1;
                    f.library.record_submission_observation_with_policy(
                        &crate::storage_policy::tests::BOUND_FIXTURE_POLICY,
                        observation(record, SubmissionState::AcceptedSyncPending),
                    )
                });
            assert!(outcome.is_err(), "{phase}");
            assert_eq!(sends, usize::from(phase == "observation"), "{phase}");
            let reopened = Library::open(&f.root).unwrap();
            let records = reopened.document_submissions_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &f.file).unwrap();
            if phase == "before-prepare" {
                assert!(records.is_empty());
                assert!(!reopened.submission_path("operation-1").unwrap().exists());
                continue;
            }
            assert_eq!(records.len(), 1, "{phase}");
            assert_eq!(records[0].command, f.command, "{phase}");
            assert_eq!(records[0].request_digest, payload_digest(&serde_json::to_vec(&f.command).unwrap()));
            assert_eq!(records[0], reopened.load_submission_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, "operation-1").unwrap());
            assert_eq!(
                records[0].state,
                match phase {
                    "prepared" => SubmissionState::Prepared,
                    "observation" => SubmissionState::AcceptedSyncPending,
                    _ => SubmissionState::Pending,
                }
            );
            assert_eq!(
                reopened.prepare_submission_for_file_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &f.file, f.command.clone()).unwrap(),
                records[0]
            );
            assert!(reopened.retention_roots_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY).unwrap().contains(&f.command.dependency_pins[0]));
            let wire: Value = serde_json::from_slice(&std::fs::read(reopened.submission_path("operation-1").unwrap()).unwrap()).unwrap();
            assert_eq!(wire["local_file"]["file_id"], f.file);
            assert!(!reopened.path("cityos/submission-files").exists(), "no sidecar association gap");
        }
    }

    #[test]
    fn missing_or_corrupt_payloads_do_not_hide_original_inquiry_or_allow_dispatch() {
        for damage in ["missing", "corrupt"] {
            let f = BoundFixture::new();
            let original =
                f.library.prepare_submission_for_file_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &f.file, f.command.clone()).unwrap();
            let path = f.library.path("cityos/payloads").join(&f.command.dependency_pins[0]);
            if damage == "missing" {
                std::fs::remove_file(&path).unwrap();
            } else {
                std::fs::write(&path, b"damaged").unwrap();
            }
            let reopened = Library::open(&f.root).unwrap();
            assert_eq!(reopened.document_submissions_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &f.file).unwrap(), [original.clone()]);
            assert_eq!(
                reopened.prepare_submission_for_file_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &f.file, f.command.clone()).unwrap(),
                original
            );
            assert_eq!(reopened.prepare_submission_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, f.command.clone()).unwrap(), original);
            let mut sends = 0;
            assert!(reopened
                .dispatch_prepared_for_file_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &f.file, f.command.clone(), |_| {
                    sends += 1;
                    Ok(())
                })
                .is_err());
            assert_eq!(sends, 0);
            assert_eq!(reopened.load_submission_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, "operation-1").unwrap(), original);
            let accepted = reopened
                .record_submission_observation_with_policy(
                    &crate::storage_policy::tests::BOUND_FIXTURE_POLICY,
                    observation(&original, SubmissionState::AcceptedSyncPending),
                )
                .unwrap();
            reopened.trash_file(&f.file).unwrap();
            let restarted = Library::open(&f.root).unwrap();
            assert_eq!(restarted.document_submissions_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &f.file).unwrap(), [accepted.clone()]);
            assert_eq!(
                restarted.load_submission_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, "operation-1").unwrap().outcome,
                accepted.outcome
            );
            assert_eq!(restarted.retention_roots_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY).unwrap(), f.command.dependency_pins);
        }
    }

    #[test]
    fn dispatch_rechecks_dependencies_after_prepare_without_losing_file_binding() {
        let f = BoundFixture::new();
        DISPATCH_AFTER_PREPARE.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(|library, record| {
                std::fs::remove_file(library.path("cityos/payloads").join(&record.command.dependency_pins[0])).unwrap();
            }))
        });
        let mut sends = 0;
        assert!(f
            .library
            .dispatch_prepared_for_file_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &f.file, f.command.clone(), |_| {
                sends += 1;
                Ok(())
            })
            .is_err());
        assert_eq!(sends, 0);
        let stored = f.library.document_submissions_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &f.file).unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].state, SubmissionState::Prepared);
    }

    #[test]
    fn file_association_is_immutable_legacy_compatible_and_never_inferred() {
        let f = BoundFixture::new();
        let original =
            f.library.prepare_submission_for_file_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &f.file, f.command.clone()).unwrap();
        assert!(f
            .library
            .prepare_submission_for_file_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, "other-file", f.command.clone())
            .is_err());
        assert!(f.library.document_submissions_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, "other-file").unwrap().is_empty());
        let mut second = f.command.clone();
        second.operation_id = "operation-2".into();
        second.intent_id = "intent-2".into();
        second.resource = "legacy-unbound-resource".into();
        let legacy = f.library.prepare_submission_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, second.clone()).unwrap();
        let legacy_bytes = std::fs::read(f.library.submission_path("operation-2").unwrap()).unwrap();
        assert_eq!(legacy_bytes, serde_json::to_vec(&legacy).unwrap(), "unbound legacy wire format is unchanged");
        let bound_bytes = std::fs::read(f.library.submission_path("operation-1").unwrap()).unwrap();
        assert_eq!(
            serde_json::from_slice::<SubmissionRecord>(&bound_bytes).unwrap(),
            original,
            "existing record consumers can still deserialize the atomic record"
        );
        assert!(f.library.prepare_submission_for_file_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &f.file, second).is_err());
        assert_eq!(f.library.load_submission_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, "operation-2").unwrap(), legacy);
        assert_eq!(f.library.document_submissions_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &f.file).unwrap(), [original.clone()]);
        let original = f
            .library
            .record_submission_observation_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, observation(&original, SubmissionState::Rejected))
            .unwrap();
        let mut third = f.command.clone();
        third.operation_id = "operation-0".into();
        third.intent_id = "intent-0".into();
        let first = f.library.prepare_submission_for_file_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &f.file, third).unwrap();
        assert_eq!(f.library.document_submissions_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &f.file).unwrap(), [first, original]);
        let mut missing = f.command.clone();
        missing.operation_id = "missing-file-operation".into();
        missing.intent_id = "another-intent".into();
        assert!(f.library.prepare_submission_for_file_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, "missing-file", missing).is_err());
        assert!(!f.library.submission_path("missing-file-operation").unwrap().exists());
    }

    #[test]
    fn different_intent_cannot_bypass_unresolved_file_draft_or_unbound_effect() {
        for state in [SubmissionState::Prepared, SubmissionState::Pending, SubmissionState::Published, SubmissionState::AcceptedSyncPending] {
            let f = BoundFixture::new();
            let prepared =
                f.library.prepare_submission_for_file_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &f.file, f.command.clone()).unwrap();
            let original = if state == SubmissionState::Prepared {
                prepared
            } else {
                f.library.record_submission_observation_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, observation(&prepared, state)).unwrap()
            };
            let mut replacement = f.command.clone();
            replacement.operation_id = "replacement-operation".into();
            replacement.intent_id = "different-intent".into();
            let mut sends = 0;
            assert!(f
                .library
                .prepare_submission_for_file_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &f.file, replacement.clone())
                .is_err());
            assert!(f
                .library
                .dispatch_prepared_for_file_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &f.file, replacement.clone(), |_| {
                    sends += 1;
                    Ok(())
                })
                .is_err());
            assert!(
                f.library.prepare_submission_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, replacement.clone()).is_err(),
                "omitting file association must not bypass a known unresolved effect"
            );
            assert_eq!(sends, 0);
            assert!(!f.library.submission_path("replacement-operation").unwrap().exists());
            assert_eq!(f.library.document_submissions_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &f.file).unwrap(), [original.clone()]);

            let checkpoint = f.library.load_checkpoint(&f.file).unwrap();
            let project = f.library.create_project("Copy").unwrap();
            let copy = f.library.create_file_checkpoint(&project.id, None, "Same draft", &checkpoint).unwrap();
            assert!(
                f.library.prepare_submission_for_file_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &copy.id, replacement.clone()).is_err(),
                "a new file ID cannot bypass the original draft freeze"
            );

            let mut rebound = checkpoint.clone();
            let cityos = rebound.document.cityos.as_mut().unwrap();
            cityos.document_id = "rebound-document".into();
            cityos.draft_id = "rebound-draft".into();
            cityos.exchange_mut().unwrap()["documentId"] = serde_json::json!("rebound-document");
            cityos.exchange_mut().unwrap()["draftId"] = serde_json::json!("rebound-draft");
            f.library.save_checkpoint(&f.file, &rebound).unwrap();
            let restarted = Library::open(&f.root).unwrap();
            assert_eq!(
                restarted.document_submissions_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &f.file).unwrap(),
                [original.clone()],
                "rebound files retain the old unresolved effect"
            );
            assert!(restarted
                .document_submissions_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &f.file)
                .unwrap()
                .iter()
                .any(SubmissionRecord::frozen));
            assert!(
                restarted.prepare_submission_for_file_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &f.file, replacement).is_err(),
                "new draft identity must not unlock the same persisted file"
            );
            assert_eq!(restarted.load_submission_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, "operation-1").unwrap(), original);
        }
    }

    #[test]
    fn submitted_command_must_match_saved_draft_and_exact_history_generation_is_retained() {
        let f = BoundFixture::new();
        for field in ["base", "semantic_base", "snapshot", "revision", "resource"] {
            let mut wrong = f.command.clone();
            match field {
                "base" => wrong.base = CityosBase::CanonicalGit { repository_id: "repo".into(), commit: "b".repeat(40) },
                "semantic_base" => wrong.semantic_base["elevation"] = serde_json::json!("wrong"),
                "snapshot" => wrong.submitted_snapshot["elevation"] = serde_json::json!("unsaved"),
                "revision" => wrong.expected_revision = "stale".into(),
                _ => wrong.resource = "other-resource".into(),
            }
            assert!(f.library.prepare_submission_for_file_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &f.file, wrong).is_err(), "{field}");
            assert!(!f.library.submission_path("operation-1").unwrap().exists());
        }
        let mut checkpoint = f.library.load_checkpoint(&f.file).unwrap();
        let next = checkpoint.document.clone();
        checkpoint.history.exec(&mut checkpoint.document, desigma_doc::Command::CityosInstall { document: Box::new(next) }).unwrap();
        f.library.save_checkpoint(&f.file, &checkpoint).unwrap();
        let original =
            f.library.prepare_submission_for_file_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &f.file, f.command.clone()).unwrap();
        let stored = f.library.load_stored_submission(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, "operation-1").unwrap();
        assert_eq!(stored.local_file.unwrap().submitted_checkpoint.unwrap(), checkpoint.to_json().unwrap());
        let mut changed_history = checkpoint.clone();
        changed_history.history = desigma_doc::History::default();
        assert_eq!(changed_history.document.to_json().unwrap(), checkpoint.document.to_json().unwrap());
        f.library.save_checkpoint(&f.file, &changed_history).unwrap();
        let mut sends = 0;
        assert!(matches!(
            f.library.dispatch_prepared_for_file_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &f.file, f.command.clone(), |_| {
                sends += 1;
                Ok(())
            }),
            Err(LibraryError::CheckpointConflict(_))
        ));
        assert_eq!(sends, 0);
        assert_eq!(f.library.document_submissions_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &f.file).unwrap(), [original.clone()]);
        let accepted = f
            .library
            .record_submission_observation_with_policy(
                &crate::storage_policy::tests::BOUND_FIXTURE_POLICY,
                observation(&original, SubmissionState::AcceptedSyncPending),
            )
            .unwrap();
        let mut candidate = changed_history.clone();
        candidate.install_baseline(changed_history.document.clone());
        assert!(matches!(
            f.library.prepare_submission_sync_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &f.file, &accepted, &candidate),
            Err(LibraryError::CheckpointConflict(_))
        ));
    }

    #[test]
    fn simultaneous_different_intents_cannot_both_prepare_for_one_file() {
        let f = BoundFixture::new();
        let ready = std::sync::Arc::new(std::sync::Barrier::new(2));
        let mut workers = Vec::new();
        for index in [0, 1] {
            let library = f.library.clone();
            let file = f.file.clone();
            let ready = ready.clone();
            let mut command = f.command.clone();
            command.operation_id = format!("racing-operation-{index}");
            command.intent_id = format!("racing-intent-{index}");
            workers.push(std::thread::spawn(move || {
                ready.wait();
                library.prepare_submission_for_file_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &file, command)
            }));
        }
        let results: Vec<_> = workers.into_iter().map(|worker| worker.join().unwrap()).collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(results.iter().filter(|result| result.is_err()).count(), 1);
        let restarted = Library::open(&f.root).unwrap();
        let records = restarted.document_submissions_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &f.file).unwrap();
        assert_eq!(records.len(), 1);
        assert!(records[0].frozen());
    }

    #[test]
    fn rebound_file_keeps_original_history_only_payloads_as_submission_roots() {
        let f = BoundFixture::new();
        let pin = f.library.retain_cityos_payload(b"original history only").unwrap();
        let mut checkpoint = f.library.load_checkpoint(&f.file).unwrap();
        checkpoint.document.cityos.as_mut().unwrap().dependency_pins.push(pin.clone());
        let mut next = checkpoint.document.clone();
        next.cityos.as_mut().unwrap().dependency_pins.clear();
        checkpoint.history.exec(&mut checkpoint.document, desigma_doc::Command::CityosInstall { document: Box::new(next) }).unwrap();
        f.library.save_checkpoint(&f.file, &checkpoint).unwrap();
        let original =
            f.library.prepare_submission_for_file_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &f.file, f.command.clone()).unwrap();
        let mut rebound = checkpoint.clone();
        rebound.history = desigma_doc::History::default();
        let binding = rebound.document.cityos.as_mut().unwrap();
        binding.document_id = "new-document".into();
        binding.draft_id = "new-draft".into();
        binding.exchange_mut().unwrap()["documentId"] = serde_json::json!("new-document");
        binding.exchange_mut().unwrap()["draftId"] = serde_json::json!("new-draft");
        assert!(!rebound.dependency_pins().contains(&pin));
        f.library.save_checkpoint(&f.file, &rebound).unwrap();
        let restarted = Library::open(&f.root).unwrap();
        assert!(restarted.retention_roots_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY).unwrap().contains(&pin));
        std::fs::remove_file(restarted.path("cityos/payloads").join(&pin)).unwrap();
        assert_eq!(restarted.document_submissions_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &f.file).unwrap(), [original.clone()]);
        assert!(restarted.document_submissions_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &f.file).unwrap()[0].frozen());
        assert!(restarted.retention_roots_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY).unwrap().contains(&pin));
    }

    #[test]
    fn damaged_file_binding_or_owner_outcome_cannot_hide_or_reassign_an_original() {
        let f = BoundFixture::new();
        let original =
            f.library.prepare_submission_for_file_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &f.file, f.command.clone()).unwrap();
        let accepted = f
            .library
            .record_submission_observation_with_policy(
                &crate::storage_policy::tests::BOUND_FIXTURE_POLICY,
                observation(&original, SubmissionState::AcceptedSyncPending),
            )
            .unwrap();
        let path = f.library.submission_path("operation-1").unwrap();
        let bytes = std::fs::read(&path).unwrap();
        for damage in ["binding", "document", "draft", "digest", "state", "receipt", "scope", "unknown", "truncated"] {
            let mut wire: Value = serde_json::from_slice(&bytes).unwrap();
            match damage {
                "binding" => wire["local_file"]["file_id"] = serde_json::json!("another-file"),
                "document" => wire["local_file"]["document_id"] = serde_json::json!("another-document"),
                "draft" => wire["local_file"]["draft_id"] = serde_json::json!("another-draft"),
                "digest" => wire["local_file"]["digest"] = serde_json::json!("0".repeat(64)),
                "state" => wire["state"] = serde_json::json!("rejected"),
                "receipt" => wire["outcome"] = Value::Null,
                "scope" => wire["outcome"]["scope"]["tenantId"] = serde_json::json!("another-tenant"),
                "unknown" => wire["unknown"] = serde_json::json!(true),
                _ => {}
            }
            std::fs::write(&path, if damage == "truncated" { b"{truncated".to_vec() } else { serde_json::to_vec(&wire).unwrap() }).unwrap();
            assert!(f.library.load_submission_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, "operation-1").is_err(), "{damage}");
            assert!(f.library.document_submissions_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &f.file).is_err(), "{damage}");
            assert!(f.library.retention_roots_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY).is_err(), "{damage}");
            assert!(
                f.library.prepare_submission_for_file_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &f.file, f.command.clone()).is_err(),
                "{damage}"
            );
        }
        std::fs::write(path, bytes).unwrap();
        assert_eq!(f.library.document_submissions_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &f.file).unwrap(), [accepted]);
    }

    #[test]
    fn blank_verified_scope_or_inconsistent_acceptance_cannot_enter_the_journal() {
        let f = BoundFixture::new();
        for field in ["tenantId", "actorId", "owner", "resource", "expected_revision"] {
            let mut invalid = f.command.clone();
            match field {
                "owner" => invalid.owner = " ".into(),
                "resource" => invalid.resource = " ".into(),
                "expected_revision" => invalid.expected_revision = " ".into(),
                _ => {
                    invalid.scope.insert(field.into(), " ".into());
                }
            }
            assert!(
                f.library.prepare_submission_for_file_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &f.file, invalid).is_err(),
                "{field}"
            );
            assert!(!f.library.submission_path("operation-1").unwrap().exists());
        }
        let original =
            f.library.prepare_submission_for_file_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &f.file, f.command.clone()).unwrap();
        for field in ["receipt", "revision", "ineligible"] {
            let mut invalid = observation(&original, SubmissionState::AcceptedSyncPending);
            match field {
                "receipt" => invalid.receipt_ref = " ".into(),
                "revision" => invalid.owner_revision = Some(" ".into()),
                _ => invalid.conclusively_ineligible = true,
            }
            assert!(f.library.record_submission_observation_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, invalid).is_err(), "{field}");
            assert_eq!(f.library.document_submissions_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &f.file).unwrap(), [original.clone()]);
        }
    }
    #[test]
    fn lost_acceptance_is_recovered_by_original_identity_without_dispatching_again() {
        let root = std::env::temp_dir().join(format!("desigma-inquiry-{}", crate::new_id()));
        let lib = Library::open(&root).unwrap();
        let mut effects = 0;
        assert!(lib
            .dispatch_prepared_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, command(), |record| {
                assert_eq!(record.state, SubmissionState::Pending);
                effects += 1;
                Err::<(), _>(LibraryError::Invalid("lost response".into()))
            })
            .is_err());
        drop(lib);
        let lib = Library::open(&root).unwrap();
        let pending = lib.load_submission_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, "operation-1").unwrap();
        assert!(pending.frozen());
        let outcome = observation(&pending, SubmissionState::AcceptedSyncPending);
        let accepted = lib.record_submission_observation_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, outcome.clone()).unwrap();
        assert_eq!(lib.record_submission_observation_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, outcome).unwrap(), accepted);
        assert!(lib
            .dispatch_prepared_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, command(), |_| {
                effects += 1;
                Ok(())
            })
            .is_err());
        assert_eq!(effects, 1);
        let mut retirement = observation(&accepted, SubmissionState::Retired);
        retirement.conclusively_ineligible = true;
        assert!(lib.record_submission_observation_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, retirement).is_err());
        assert_eq!(lib.load_submission_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, "operation-1").unwrap().command, pending.command);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn acceptance_arriving_between_prepare_and_pending_is_never_overwritten_or_redispatched() {
        let root = std::env::temp_dir().join(format!("desigma-dispatch-race-{}", crate::new_id()));
        let library = Library::open(&root).unwrap();
        DISPATCH_AFTER_PREPARE.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(|library, prepared| {
                library
                    .record_submission_observation_with_policy(
                        &crate::storage_policy::tests::BOUND_FIXTURE_POLICY,
                        observation(prepared, SubmissionState::AcceptedSyncPending),
                    )
                    .unwrap();
            }))
        });
        let mut sends = 0;
        assert!(library
            .dispatch_prepared_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, command(), |_| {
                sends += 1;
                Ok(())
            })
            .is_err());
        assert_eq!(sends, 0);
        let stored = Library::open(&root).unwrap().load_submission_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, "operation-1").unwrap();
        assert_eq!(stored.state, SubmissionState::AcceptedSyncPending);
        assert_eq!(stored.outcome, Some(serde_json::to_value(observation(&stored, SubmissionState::AcceptedSyncPending)).unwrap()));
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn successor_remains_blocked_until_original_retirement_is_conclusive() {
        let root = std::env::temp_dir().join(format!("desigma-retire-{}", crate::new_id()));
        let lib = Library::open(&root).unwrap();
        let original = lib.prepare_submission_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, command()).unwrap();
        let mut successor = command();
        successor.operation_id = "operation-2".into();
        successor.predecessor = Some("operation-1".into());
        assert!(lib.prepare_submission_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, successor.clone()).is_err());
        let mut retirement = observation(&original, SubmissionState::Retired);
        assert!(lib.record_submission_observation_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, retirement.clone()).is_err());
        retirement.conclusively_ineligible = true;
        lib.record_submission_observation_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, retirement).unwrap();
        assert_eq!(
            lib.prepare_submission_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, successor).unwrap().command.predecessor,
            Some("operation-1".into())
        );
        let mut other = observation(&original, SubmissionState::Published);
        other.scope.insert("tenantId".into(), "another".into());
        assert!(lib.record_submission_observation_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, other).is_err());
        assert_eq!(lib.load_submission_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, "operation-1").unwrap().command, original.command);
        std::fs::remove_dir_all(root).unwrap();
    }
}
