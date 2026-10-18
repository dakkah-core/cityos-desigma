//! Local installation of an ALREADY authenticated, accepted owner outcome.
//! The trusted bridge must validate the complete projection before calling this
//! module. Nothing here authenticates/qualifies an owner or dispatches a write.
use crate::submissions::SUBMISSION_LOCK;
use crate::{payload_digest, write_atomic, Library, LibraryError, Result, SubmissionObservation, SubmissionRecord, SubmissionState};
use desigma_doc::{CityosBase, DocumentCheckpoint};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const SCHEMA: &str = "cityos.submission-sync.v1";

/// Immutable local intent, addressed by the ORIGINAL operation. Keep it after
/// completion: it binds the original acceptance, exact archive and payload roots.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SubmissionSyncIntent {
    pub schema_version: String,
    pub sync_id: String,
    pub file_id: String,
    pub accepted: SubmissionRecord,
    pub previous_checkpoint: String,
    pub accepted_checkpoint: String,
}

/// Exact local readback only. This receipt grants no remote acceptance authority.
#[derive(Clone, Debug)]
pub struct SubmissionSyncReceipt {
    pub sync_id: String,
    pub submission: SubmissionRecord,
    pub checkpoint: DocumentCheckpoint,
    pub recovered_after_error: bool,
}

fn invalid(message: &str) -> LibraryError {
    LibraryError::Invalid(message.into())
}

fn acceptance(record: &SubmissionRecord) -> Result<SubmissionObservation> {
    let receipt: SubmissionObservation = serde_json::from_value(record.outcome.clone().ok_or_else(|| invalid("Accepted receipt required"))?)?;
    if record.state != SubmissionState::AcceptedSyncPending
        || receipt.state != SubmissionState::AcceptedSyncPending
        || receipt.operation_id != record.command.operation_id
        || receipt.request_digest != record.request_digest
        || receipt.owner != record.command.owner
        || receipt.resource != record.command.resource
        || receipt.scope != record.command.scope
        || receipt.receipt_ref.trim().is_empty()
        || receipt.owner_revision.as_ref().is_none_or(|value| value.trim().is_empty())
        || receipt.conclusively_ineligible
        || ["tenantId", "actorId"].iter().any(|key| record.command.scope.get(*key).is_none_or(|value| value.trim().is_empty()))
    {
        return Err(invalid("Original accepted receipt identity/scope/revision mismatch"));
    }
    Ok(receipt)
}

impl SubmissionSyncIntent {
    fn digest(&self) -> Result<String> {
        Ok(payload_digest(&serde_json::to_vec(&(&self.schema_version, &self.file_id, &self.accepted, &self.previous_checkpoint, &self.accepted_checkpoint))?))
    }

    fn checkpoints(&self) -> Result<(DocumentCheckpoint, DocumentCheckpoint)> {
        crate::validate_id(&self.file_id)?;
        crate::validate_id(&self.accepted.command.operation_id)?;
        if self.schema_version != SCHEMA || self.sync_id != self.digest()? {
            return Err(invalid("Submission sync intent integrity failure"));
        }
        let previous = DocumentCheckpoint::from_json(&self.previous_checkpoint)?;
        let candidate = DocumentCheckpoint::from_json(&self.accepted_checkpoint)?;
        validate_candidate(&self.accepted, &previous, &candidate)?;
        Ok((previous, candidate))
    }
}

fn validate_candidate(record: &SubmissionRecord, previous: &DocumentCheckpoint, candidate: &DocumentCheckpoint) -> Result<()> {
    let receipt = acceptance(record)?;
    // A clean, freshly sealed active history must archive the ENTIRE preceding
    // checkpoint, including its existing archives and both undo/redo stacks.
    if candidate.history.baseline_id().is_none()
        || candidate.history.baseline_id() == previous.history.baseline_id()
        || candidate.history.can_undo()
        || candidate.history.can_redo()
        || candidate.retained.len() != previous.retained.len() + 1
    {
        return Err(invalid("Accepted checkpoint requires a fresh history boundary"));
    }
    let archived = candidate.retained.last().ok_or_else(|| invalid("Missing pre-acceptance archive"))?;
    let predecessor = DocumentCheckpoint {
        document: archived.document.clone(),
        history: archived.history.clone(),
        retained: candidate.retained[..candidate.retained.len() - 1].to_vec(),
    };
    if predecessor.to_json()? != previous.to_json()? {
        return Err(invalid("Accepted checkpoint does not archive the exact original draft/history"));
    }
    let before = previous.document.cityos.as_ref().ok_or_else(|| invalid("Original CityOS binding required"))?;
    let after = candidate.document.cityos.as_ref().ok_or_else(|| invalid("Accepted CityOS binding required"))?;
    before.validate().map_err(LibraryError::Invalid)?;
    after.validate().map_err(LibraryError::Invalid)?;
    // Legacy bindings cannot prove semantic base=draft. Native expanded bindings
    // remain subject to desigma-doc's existing qualification gate.
    let old = before.exchange().ok_or_else(|| invalid("Expanded original exchange required for synchronization"))?;
    let new = after.exchange().ok_or_else(|| invalid("Expanded accepted exchange required for synchronization"))?;
    if before.base != record.command.base
        || before.semantic_base != record.command.semantic_base
        || old["semanticDraft"] != record.command.submitted_snapshot
        || old["semanticDraft"]["id"].as_str() != Some(record.command.resource.as_str())
        || old["base"]["sourceDigest"].as_str() != Some(record.command.expected_revision.as_str())
        || before.document_id != after.document_id
        || before.draft_id != after.draft_id
        || before.schema_version != after.schema_version
        || old.get("profileRef") != new.get("profileRef")
        || old.get("semanticContract") != new.get("semanticContract")
        || new["semanticBase"]["id"].as_str() != Some(record.command.resource.as_str())
        || new["semanticBase"] != new["semanticDraft"]
    {
        return Err(invalid("Accepted projection does not match the original resource/submitted snapshot"));
    }
    // An accepted resource can include compatible concurrent owner edits. Bind
    // that COMPLETE semantic readback to the original durable owner receipt,
    // rather than reinstalling only our submitted delta and losing those edits.
    let readback_digest = payload_digest(&serde_json::to_vec(&after.semantic_base)?);
    let pinned_readback = receipt.details.get("acceptedSemanticDigest").and_then(|value| value.as_str());
    if pinned_readback.is_some_and(|digest| digest != readback_digest)
        || (after.semantic_base != record.command.submitted_snapshot && pinned_readback != Some(readback_digest.as_str())) {
        return Err(invalid("Full accepted semantic readback requires its original owner receipt digest"));
    }
    let revision = receipt.owner_revision.as_deref().ok_or_else(|| invalid("Accepted revision required"))?;
    match (&record.command.base, &after.base) {
        (CityosBase::CanonicalGit { repository_id: original, .. }, CityosBase::CanonicalGit { repository_id, commit })
            if repository_id == original
                && commit == revision
                && old["base"]["sourcePath"].as_str().is_some_and(|path| !path.trim().is_empty())
                && old["base"]["sourcePath"] == new["base"]["sourcePath"] => {}
        (CityosBase::NativeOwner { .. }, CityosBase::NativeOwner { owner, resource, revision: actual })
            if owner == &record.command.owner && resource == &record.command.resource && actual == revision => {}
        _ => return Err(invalid("Accepted checkpoint owner/resource/revision mismatch")),
    }
    Ok(())
}

impl Library {
    fn submission_sync_path(&self, operation_id: &str) -> Result<PathBuf> {
        crate::validate_id(operation_id)?;
        Ok(self.path("cityos/submission-sync").join(format!("{operation_id}.json")))
    }

    /// Inspect/recover by the original operation identity, without re-dispatch.
    pub fn load_submission_sync(&self, operation_id: &str) -> Result<SubmissionSyncIntent> {
        let intent: SubmissionSyncIntent = serde_json::from_slice(&std::fs::read(self.submission_sync_path(operation_id)?)?)?;
        if intent.accepted.command.operation_id != operation_id {
            return Err(invalid("Submission sync operation mismatch"));
        }
        let (previous, _) = intent.checkpoints()?;
        self.check_sync_acceptance(&intent.accepted)?;
        self.check_submission_file(operation_id, &intent.file_id, &previous)?;
        Ok(intent)
    }

    fn check_sync_acceptance(&self, accepted: &SubmissionRecord) -> Result<SubmissionRecord> {
        acceptance(accepted)?;
        let stored = self.load_submission(&accepted.command.operation_id)?;
        let mut original = stored.clone();
        if original.state == SubmissionState::Synchronized {
            original.state = SubmissionState::AcceptedSyncPending;
        }
        if original != *accepted {
            return Err(invalid("Synchronization requires the exact stored original acceptance"));
        }
        Ok(stored)
    }

    /// Persist before installation. Caller supplies a trusted, fully validated
    /// full checkpoint built with DocumentCheckpoint::install_baseline. The
    /// ORIGINAL receipt must already be durably AcceptedSyncPending. There is
    /// no owner dispatch, owner qualification, or UI activation in this method.
    pub fn prepare_submission_sync(&self, file_id: &str, accepted: &SubmissionRecord, candidate: &DocumentCheckpoint) -> Result<SubmissionSyncIntent> {
        let _submission = SUBMISSION_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let _writer = crate::store::write_lock(self)?;
        crate::validate_id(file_id)?;
        let stored = self.check_sync_acceptance(accepted)?;
        let candidate_json = DocumentCheckpoint::from_json(&candidate.to_json()?)?.to_json()?;
        let path = self.submission_sync_path(&accepted.command.operation_id)?;
        match std::fs::read(&path) {
            Ok(_) => {
                let intent = self.load_submission_sync(&accepted.command.operation_id)?;
                if intent.file_id != file_id || intent.accepted != *accepted || intent.accepted_checkpoint != candidate_json {
                    return Err(invalid("Submission synchronization identity is immutable"));
                }
                return Ok(intent);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        if stored.state != SubmissionState::AcceptedSyncPending {
            return Err(invalid("Synchronized record is missing its original sync intent"));
        }
        let previous = self.load_checkpoint_locked(file_id)?;
        self.check_submission_file(&accepted.command.operation_id, file_id, &previous)?;
        validate_candidate(accepted, &previous, candidate)?;
        for pin in candidate.dependency_pins().iter().chain(accepted.command.dependency_pins.iter()) {
            self.verify_cityos_payload(pin)?;
        }
        let mut intent = SubmissionSyncIntent {
            schema_version: SCHEMA.into(),
            sync_id: String::new(),
            file_id: file_id.into(),
            accepted: accepted.clone(),
            previous_checkpoint: previous.to_json()?,
            accepted_checkpoint: candidate_json,
        };
        intent.sync_id = intent.digest()?;
        sync_phase("before-intent", self, file_id)?;
        write_atomic(&path, &serde_json::to_vec(&intent)?)?;
        sync_phase("intent", self, file_id)?;
        if self.load_submission_sync(&accepted.command.operation_id)? != intent {
            return Err(invalid("Submission sync intent readback mismatch"));
        }
        Ok(intent)
    }

    /// Resume only the durable ORIGINAL intent. Editing must remain blocked if
    /// this returns an unresolved error. Never recreate/reseal a retry candidate.
    pub fn resume_submission_sync(&self, operation_id: &str) -> Result<SubmissionSyncReceipt> {
        let _submission = SUBMISSION_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        // Retain the writer lock through comparison, installation and finalization
        // so an autosave cannot invalidate the readback-to-state transition.
        let _writer = crate::store::write_lock(self)?;
        let intent = self.load_submission_sync(operation_id)?;
        let (_, candidate) = intent.checkpoints()?;
        let mut stored = self.check_sync_acceptance(&intent.accepted)?;
        // A submission-only pin is not necessarily in the accepted checkpoint.
        // Verify it BEFORE any installation or pending-save replay can mutate the
        // document, while keeping original inquiry/retention available on failure.
        for pin in candidate.dependency_pins().iter().chain(intent.accepted.command.dependency_pins.iter()) {
            self.verify_cityos_payload(pin)?;
        }
        let current = self.load_checkpoint_locked(&intent.file_id)?.to_json()?;
        let mut recovered_after_error = current == intent.accepted_checkpoint && stored.state != SubmissionState::Synchronized;
        if current != intent.accepted_checkpoint {
            if stored.state != SubmissionState::AcceptedSyncPending || current != intent.previous_checkpoint {
                return Err(LibraryError::CheckpointConflict(intent.file_id.clone()));
            }
            sync_phase("before-install", self, &intent.file_id)?;
            for pin in &intent.accepted.command.dependency_pins { self.verify_cityos_payload(pin)?; }
            let saved = self.save_checkpoint_reconciled_locked(&intent.file_id, &candidate)?;
            recovered_after_error = saved.recovered_after_error;
        }
        sync_phase("installed", self, &intent.file_id)?;
        let checkpoint = self.load_checkpoint_locked(&intent.file_id)?;
        if checkpoint.to_json()? != intent.accepted_checkpoint {
            return Err(LibraryError::ReconciliationRequired {
                file_id: intent.file_id.clone(),
                save_error: "Accepted checkpoint installation attempted".into(),
                recovery_error: "Exact accepted checkpoint readback required before synchronization".into(),
            });
        }
        for pin in &intent.accepted.command.dependency_pins {
            self.verify_cityos_payload(pin)?;
        }
        sync_phase("readback", self, &intent.file_id)?;
        if stored.state != SubmissionState::Synchronized {
            sync_phase("before-synchronized", self, &intent.file_id)?;
            for pin in checkpoint.dependency_pins().iter().chain(intent.accepted.command.dependency_pins.iter()) {
                self.verify_cityos_payload(pin)?;
            }
            stored.state = SubmissionState::Synchronized;
            // Preserve the ORIGINAL accepted observation, command and digest.
            self.write_submission_record(&stored)?;
        }
        sync_phase("synchronized", self, &intent.file_id)?;
        let actual = self.check_sync_acceptance(&intent.accepted)?;
        if actual != stored || actual.state != SubmissionState::Synchronized {
            return Err(invalid("Synchronized submission readback mismatch"));
        }
        Ok(SubmissionSyncReceipt { sync_id: intent.sync_id, submission: actual, checkpoint, recovered_after_error })
    }

    /// One-shot entry point; restart may use resume_submission_sync alone.
    pub fn synchronize_accepted_submission(&self, file_id: &str, accepted: &SubmissionRecord, candidate: &DocumentCheckpoint) -> Result<SubmissionSyncReceipt> {
        self.prepare_submission_sync(file_id, accepted, candidate)?;
        self.resume_submission_sync(&accepted.command.operation_id)
    }

    pub(crate) fn submission_sync_retention_roots(&self) -> Result<Vec<String>> {
        let entries = match std::fs::read_dir(self.path("cityos/submission-sync")) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut roots = Vec::new();
        for entry in entries {
            let entry = entry?;
            if entry.path().extension().is_some_and(|extension| extension == "json") {
                let id = entry.path().file_stem().ok_or_else(|| invalid("Invalid sync intent path"))?.to_string_lossy().into_owned();
                let intent = self.load_submission_sync(&id)?;
                roots.extend(intent.checkpoints()?.1.dependency_pins());
                roots.extend(intent.accepted.command.dependency_pins);
            }
        }
        Ok(roots)
    }
}

#[cfg(test)]
thread_local! {
    static SYNC_FAULT: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
    static SYNC_HOOK: std::cell::RefCell<Option<SyncHook>> = const { std::cell::RefCell::new(None) };
}
#[cfg(test)]
type SyncHook = (&'static str, Box<dyn FnOnce(&Library, &str)>);

fn sync_phase(_phase: &str, _library: &Library, _file: &str) -> Result<()> {
    #[cfg(test)]
    {
        let hook = SYNC_HOOK.with(|slot| {
            let mut hook = slot.borrow_mut();
            if hook.as_ref().is_some_and(|(phase, _)| *phase == _phase) {
                hook.take()
            } else {
                None
            }
        });
        if let Some((_, hook)) = hook {
            hook(_library, _file);
        }
        if SYNC_FAULT.with(|fault| {
            if fault.get() == Some(_phase) {
                fault.set(None);
                true
            } else {
                false
            }
        }) {
            return Err(std::io::Error::other(format!("Interrupted submission sync at {_phase}")).into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SubmissionCommand;
    use desigma_doc::{CityosBinding, Command, Document, History, Node, Rect};
    use serde_json::json;
    use std::collections::BTreeMap;

    struct Fixture {
        root: PathBuf,
        library: Library,
        file: String,
        accepted: SubmissionRecord,
        original: DocumentCheckpoint,
        candidate: DocumentCheckpoint,
        accepted_pin: String,
        submission_pin: String,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn semantic(elevation: &str) -> serde_json::Value {
        json!({"id":"frame", "root":{"props":{"elevation":elevation}}})
    }

    fn bound(pin: &str) -> Document {
        let mut document = Document::new("original projection");
        let page = document.add_page("P");
        let node = document.alloc_id();
        document.insert(page, Node::rectangle(node, Rect::new(0., 0., 40., 20.))).unwrap();
        document.images.insert("original image".into(), vec![1, 2, 3]);
        document.cityos = Some(CityosBinding {
            schema_version: "cityos.design-exchange.v3".into(),
            document_id: "document".into(),
            draft_id: "draft".into(),
            base: CityosBase::CanonicalGit { repository_id: "repo".into(), commit: "a".repeat(40) },
            semantic_base: semantic("flat"),
            dependency_pins: vec![pin.into()],
            occurrences: BTreeMap::new(),
            exchange_v2: None,
            exchange_v3: Some(json!({
                "schemaVersion":"cityos.design-exchange.v3", "documentId":"document", "draftId":"draft", "draftRevision":0,
                "semanticContract":"fixture-composition", "profileRef":"fixture-profile", "commandReceipts":[],
                "base":{"kind":"canonical-git", "repositoryId":"repo", "commit":"a".repeat(40),
                    "sourcePath":"config/frame.json", "sourceDigest":"d".repeat(64)},
                "semanticBase":semantic("flat"), "semanticDraft":semantic("flat"),
                "dependencyPins":[{"digest":pin}]
            })),
        });
        document
    }

    fn fixture() -> Fixture {
        let root = std::env::temp_dir().join(format!("desigma-submission-sync-{}", crate::new_id()));
        let library = Library::open(&root).unwrap();
        let old_pin = library.retain_cityos_payload(b"original history dependency").unwrap();
        let accepted_pin = library.retain_cityos_payload(b"accepted-only dependency").unwrap();
        let submission_pin = library.retain_cityos_payload(b"original submission-only dependency").unwrap();
        let project = library.create_project("P").unwrap();
        let mut original = DocumentCheckpoint::new(bound(&old_pin), History::default());
        original.install_baseline(bound(&old_pin));
        for elevation in ["resting", "raised"] {
            let mut next = original.document.clone();
            next.cityos.as_mut().unwrap().exchange_mut().unwrap()["semanticDraft"] = semantic(elevation);
            original.history.exec(&mut original.document, Command::CityosInstall { document: Box::new(next) }).unwrap();
        }
        original.history.undo(&mut original.document).unwrap();
        let file = library.create_file_checkpoint(&project.id, None, "F", &original).unwrap().id;
        // The observation is a fixture for an already authenticated owner client;
        // this local test deliberately makes no installed-owner qualification claim.
        let command = SubmissionCommand {
            operation_id: "operation".into(),
            intent_id: "intent".into(),
            predecessor: None,
            owner: "engineering".into(),
            resource: "frame".into(),
            scope: BTreeMap::from([("tenantId".into(), "tenant".into()), ("actorId".into(), "actor".into()), ("contextRevision".into(), "context".into())]),
            base: original.document.cityos.as_ref().unwrap().base.clone(),
            expected_revision: "d".repeat(64),
            semantic_base: semantic("flat"),
            submitted_snapshot: semantic("resting"),
            dependency_pins: vec![submission_pin.clone()],
        };
        let prepared = library.prepare_submission_for_file(&file, command).unwrap();
        let accepted = library
            .record_submission_observation(SubmissionObservation {
                operation_id: prepared.command.operation_id.clone(),
                request_digest: prepared.request_digest.clone(),
                owner: prepared.command.owner.clone(),
                resource: prepared.command.resource.clone(),
                scope: prepared.command.scope.clone(),
                state: SubmissionState::AcceptedSyncPending,
                receipt_ref: "fixture-original-acceptance".into(),
                owner_revision: Some("b".repeat(40)),
                conclusively_ineligible: false,
                details: json!({"fixture":true}),
            })
            .unwrap();
        let mut installed = original.document.clone();
        installed.name = "accepted full projection".into();
        installed.images.clear();
        installed.images.insert("accepted image".into(), vec![9, 8, 7]);
        let binding = installed.cityos.as_mut().unwrap();
        binding.base = CityosBase::CanonicalGit { repository_id: "repo".into(), commit: "b".repeat(40) };
        binding.semantic_base = semantic("resting");
        binding.dependency_pins = vec![accepted_pin.clone()];
        let exchange = binding.exchange_mut().unwrap();
        exchange["base"]["commit"] = json!("b".repeat(40));
        exchange["base"]["sourceDigest"] = json!("e".repeat(64));
        exchange["semanticBase"] = semantic("resting");
        exchange["semanticDraft"] = semantic("resting");
        exchange["dependencyPins"] = json!([{"digest":accepted_pin}]);
        let mut candidate = original.clone();
        candidate.install_baseline(installed);
        Fixture { root, library, file, accepted, original, candidate, accepted_pin, submission_pin }
    }

    #[test]
    fn full_authoritative_readback_preserves_compatible_concurrent_semantics() {
        let mut f = fixture();
        let mut accepted_semantics = semantic("resting");
        accepted_semantics["root"]["props"]["concurrentOwnerField"] = json!("preserved");
        let binding = f.candidate.document.cityos.as_mut().unwrap();
        binding.semantic_base = accepted_semantics.clone();
        let exchange = binding.exchange_mut().unwrap();
        exchange["semanticBase"] = accepted_semantics.clone();
        exchange["semanticDraft"] = accepted_semantics.clone();
        assert!(f.library.prepare_submission_sync(&f.file, &f.accepted, &f.candidate).is_err());
        // Fixture of the original authenticated receipt carrying full readback
        // identity, before any local synchronization is prepared.
        let digest = payload_digest(&serde_json::to_vec(&accepted_semantics).unwrap());
        f.accepted.outcome.as_mut().unwrap()["details"]["acceptedSemanticDigest"] = json!(digest);
        f.library.write_submission_record(&f.accepted).unwrap();
        let saved = f.library.synchronize_accepted_submission(&f.file, &f.accepted, &f.candidate).unwrap();
        let installed = saved.checkpoint.document.cityos.unwrap();
        assert_eq!(installed.semantic_base, accepted_semantics);
        assert_eq!(installed.exchange().unwrap()["semanticDraft"], accepted_semantics);
        assert_eq!(saved.submission.command.submitted_snapshot, semantic("resting"));
    }

    #[test]
    fn accepted_install_preserves_original_receipt_full_projection_history_and_pins() {
        let f = fixture();
        let receipt = f.library.synchronize_accepted_submission(&f.file, &f.accepted, &f.candidate).unwrap();
        assert_eq!(receipt.submission.state, SubmissionState::Synchronized);
        assert!(!receipt.submission.frozen());
        assert_eq!(receipt.submission.command, f.accepted.command);
        assert_eq!(receipt.submission.request_digest, f.accepted.request_digest);
        assert_eq!(receipt.submission.outcome, f.accepted.outcome);
        assert_eq!(f.library.document_submissions(&f.file).unwrap(), [receipt.submission.clone()]);
        let wire: serde_json::Value = serde_json::from_slice(&std::fs::read(f.library.submission_path("operation").unwrap()).unwrap()).unwrap();
        assert_eq!(wire["local_file"]["document_id"], "document");
        assert_eq!(wire["local_file"]["draft_id"], "draft");
        assert_eq!(receipt.checkpoint.to_json().unwrap(), f.candidate.to_json().unwrap());
        assert_eq!(receipt.checkpoint.document.images["accepted image"], [9, 8, 7]);
        assert!(!receipt.checkpoint.history.can_undo());
        assert!(!receipt.checkpoint.history.can_redo());
        let archived = receipt.checkpoint.retained.last().unwrap();
        assert!(archived.history.can_undo());
        assert!(archived.history.can_redo());
        assert_eq!(archived.document.images["original image"], [1, 2, 3]);
        let reopened = Library::open(&f.root).unwrap();
        let retry = reopened.resume_submission_sync("operation").unwrap();
        assert_eq!(retry.sync_id, receipt.sync_id);
        assert_eq!(retry.checkpoint.retained.len(), f.original.retained.len() + 1);
        assert!(reopened.cityos_retention_roots().unwrap().contains(&f.submission_pin));
        assert!(reopened.cityos_retention_roots().unwrap().contains(&f.accepted_pin));
        assert!(reopened.save_file_with_history(&f.file, &f.original.document, &f.original.history).is_err());
        assert!(reopened.save_checkpoint(&f.file, &f.original).is_err());
    }

    #[test]
    fn fault_at_every_sync_boundary_resumes_original_identity_without_resealing() {
        for phase in ["before-intent", "intent", "before-install", "installed", "readback", "before-synchronized", "synchronized"] {
            let f = fixture();
            SYNC_FAULT.with(|fault| fault.set(Some(phase)));
            assert!(f.library.synchronize_accepted_submission(&f.file, &f.accepted, &f.candidate).is_err(), "{phase}");
            let stored = f.library.load_submission("operation").unwrap();
            assert_eq!(stored.command, f.accepted.command, "{phase}");
            assert_eq!(stored.outcome, f.accepted.outcome, "{phase}");
            assert_eq!(stored.request_digest, f.accepted.request_digest, "{phase}");
            if phase != "synchronized" {
                assert_eq!(stored.state, SubmissionState::AcceptedSyncPending, "{phase}");
            }
            let reopened = Library::open(&f.root).unwrap();
            if phase == "before-intent" {
                assert!(reopened.load_submission_sync("operation").is_err());
                assert_eq!(reopened.load_checkpoint(&f.file).unwrap().to_json().unwrap(), f.original.to_json().unwrap());
                reopened.prepare_submission_sync(&f.file, &f.accepted, &f.candidate).unwrap();
            }
            let intent = reopened.load_submission_sync("operation").unwrap();
            assert_eq!(intent.accepted, f.accepted);
            assert_eq!(intent.accepted_checkpoint, f.candidate.to_json().unwrap());
            assert!(reopened.cityos_retention_roots().unwrap().contains(&f.accepted_pin));
            let result = reopened.resume_submission_sync("operation").unwrap();
            assert_eq!(result.sync_id, intent.sync_id);
            assert_eq!(result.checkpoint.to_json().unwrap(), f.candidate.to_json().unwrap());
        }
    }

    #[test]
    fn accepted_install_save_fault_matrix_recovers_without_resealing_or_redispatch() {
        for phase in ["before-journal", "journal", "payloads", "document", "checkpoint", "manifest", "complete"] {
            for repeats in [0, 1] {
                let f = fixture();
                let intent = f.library.prepare_submission_sync(&f.file, &f.accepted, &f.candidate).unwrap();
                crate::store::inject_save_fault(phase, repeats);
                let result = f.library.resume_submission_sync("operation");
                // A one-shot after-journal error is reconciled inside the same
                // call; repeating replay faults must leave acceptance frozen.
                if phase == "before-journal" || (repeats == 1 && matches!(phase, "payloads" | "document" | "checkpoint" | "manifest")) {
                    assert!(result.is_err(), "{phase}/{repeats}");
                    assert_eq!(f.library.load_submission("operation").unwrap(), f.accepted);
                } else {
                    assert_eq!(result.unwrap().submission.state, SubmissionState::Synchronized, "{phase}/{repeats}");
                }
                // before-journal and complete cannot repeat within that save.
                // Disarm any unused test fault before independently reopening.
                crate::store::inject_save_fault("unused", 0);
                let restarted = Library::open(&f.root).unwrap();
                assert_eq!(restarted.load_submission_sync("operation").unwrap(), intent);
                let saved = restarted.resume_submission_sync("operation").unwrap();
                assert_eq!(saved.checkpoint.to_json().unwrap(), f.candidate.to_json().unwrap());
                assert_eq!(saved.submission.command, f.accepted.command);
                assert_eq!(saved.submission.outcome, f.accepted.outcome);
                assert_eq!(restarted.document_submissions(&f.file).unwrap(), [saved.submission.clone()]);
                assert_eq!(saved.checkpoint.retained.len(), f.original.retained.len() + 1);
                assert!(restarted.save_checkpoint(&f.file, &f.original).is_err());
                let again = Library::open(&f.root).unwrap().resume_submission_sync("operation").unwrap();
                assert_eq!(again.sync_id, intent.sync_id);
                assert_eq!(again.checkpoint.to_json().unwrap(), f.candidate.to_json().unwrap());
            }
        }
    }

    #[test]
    fn missing_or_corrupt_sync_dependencies_preserve_inquiry_intent_and_target_until_repair() {
        for root in ["original-history", "accepted", "submission-only"] {
            for damage in ["missing", "corrupt"] {
                let f = fixture();
                let intent = f.library.prepare_submission_sync(&f.file, &f.accepted, &f.candidate).unwrap();
                let (pin, bytes): (String, &[u8]) = match root {
                    "original-history" => (f.original.dependency_pins()[0].clone(), b"original history dependency"),
                    "accepted" => (f.accepted_pin.clone(), b"accepted-only dependency"),
                    _ => (f.submission_pin.clone(), b"original submission-only dependency"),
                };
                let checkpoint_path = f.library.checkpoint_path(&f.file).unwrap();
                let checkpoint_before = std::fs::read(&checkpoint_path).unwrap();
                let path = f.library.path("cityos/payloads").join(&pin);
                if damage == "missing" { std::fs::remove_file(&path).unwrap(); }
                else { std::fs::write(&path, b"damaged").unwrap(); }
                let restarted = Library::open(&f.root).unwrap();
                assert!(restarted.resume_submission_sync("operation").is_err(), "{root}/{damage}");
                assert_eq!(std::fs::read(&checkpoint_path).unwrap(), checkpoint_before, "{root}/{damage}");
                assert_eq!(restarted.load_submission_sync("operation").unwrap(), intent);
                assert_eq!(restarted.document_submissions(&f.file).unwrap(), [f.accepted.clone()]);
                assert!(restarted.cityos_retention_roots().unwrap().contains(&pin));
                assert!(restarted.put_cityos_payload(&pin, b"wrong repair").is_err());
                restarted.put_cityos_payload(&pin, bytes).unwrap();
                let saved = restarted.resume_submission_sync("operation").unwrap();
                assert_eq!(saved.sync_id, intent.sync_id);
                assert_eq!(saved.checkpoint.to_json().unwrap(), f.candidate.to_json().unwrap());
                assert_eq!(saved.submission.outcome, f.accepted.outcome);
            }
        }
    }

    #[test]
    fn dependency_loss_during_install_or_finalization_cannot_mark_synchronized() {
        for phase in ["before-install", "installed", "readback", "before-synchronized"] {
            let f = fixture();
            let intent = f.library.prepare_submission_sync(&f.file, &f.accepted, &f.candidate).unwrap();
            let pin = f.submission_pin.clone();
            SYNC_HOOK.with(|slot| *slot.borrow_mut() = Some((phase, Box::new(move |library, _| {
                std::fs::remove_file(library.path("cityos/payloads").join(&pin)).unwrap();
            }))));
            assert!(f.library.resume_submission_sync("operation").is_err(), "{phase}");
            assert_eq!(f.library.document_submissions(&f.file).unwrap(), [f.accepted.clone()]);
            assert_eq!(f.library.load_submission_sync("operation").unwrap(), intent);
            if phase == "before-install" {
                assert_eq!(f.library.load_checkpoint(&f.file).unwrap().to_json().unwrap(), f.original.to_json().unwrap());
            }
            let restarted = Library::open(&f.root).unwrap();
            restarted.put_cityos_payload(&f.submission_pin, b"original submission-only dependency").unwrap();
            assert_eq!(restarted.resume_submission_sync("operation").unwrap().checkpoint.to_json().unwrap(), f.candidate.to_json().unwrap());
        }
    }

    #[test]
    fn bound_submission_cannot_install_into_another_file() {
        let f = fixture();
        let project = f.library.create_project("Other").unwrap();
        let other = f.library.create_file_checkpoint(&project.id, None, "Same draft copy", &f.original).unwrap();
        assert!(f.library.prepare_submission_sync(&other.id, &f.accepted, &f.candidate).is_err());
        assert!(!f.library.submission_sync_path("operation").unwrap().exists());
        assert_eq!(f.library.load_checkpoint(&other.id).unwrap().to_json().unwrap(), f.original.to_json().unwrap());
        assert!(f.library.document_submissions(&other.id).unwrap().is_empty());
    }

    #[test]
    #[ignore = "worker invoked by accepted_sync_recovery_survives_process_exit"]
    fn accepted_sync_interruption_worker() {
        let Ok(root) = std::env::var("DESIGMA_SYNC_TEST_ROOT") else { return };
        let file = std::env::var("DESIGMA_SYNC_TEST_FILE").unwrap();
        let phase = std::env::var("DESIGMA_SYNC_TEST_PHASE").unwrap();
        let library = Library::open(&root).unwrap();
        let candidate = DocumentCheckpoint::from_json(&std::fs::read_to_string(library.path("sync-test-candidate.json")).unwrap()).unwrap();
        let accepted = library.load_submission("operation").unwrap();
        match phase.as_str() {
            "save:before-journal" => crate::store::exit_during_save("before-journal"),
            "save:journal" => crate::store::exit_during_save("journal"),
            "save:payloads" => crate::store::exit_during_save("payloads"),
            "save:document" => crate::store::exit_during_save("document"),
            "save:checkpoint" => crate::store::exit_during_save("checkpoint"),
            "save:manifest" => crate::store::exit_during_save("manifest"),
            "save:complete" => crate::store::exit_during_save("complete"),
            _ => {
                let phase = match phase.as_str() {
                    "sync:before-intent" => "before-intent", "sync:intent" => "intent",
                    "sync:before-install" => "before-install", "sync:installed" => "installed",
                    "sync:readback" => "readback", "sync:before-synchronized" => "before-synchronized",
                    "sync:synchronized" => "synchronized", _ => panic!("Unexpected sync worker phase"),
                };
                SYNC_HOOK.with(|slot| *slot.borrow_mut() = Some((phase, Box::new(|_, _| std::process::exit(92)))));
            }
        }
        let _ = library.synchronize_accepted_submission(&file, &accepted, &candidate);
        panic!("Worker must exit inside the requested persistence phase");
    }

    #[test]
    fn accepted_sync_recovery_survives_process_exit() {
        for phase in ["sync:before-intent", "sync:intent", "sync:before-install", "sync:installed", "sync:readback",
            "sync:before-synchronized", "sync:synchronized", "save:before-journal", "save:journal", "save:payloads",
            "save:document", "save:checkpoint", "save:manifest", "save:complete"] {
            let f = fixture();
            std::fs::write(f.library.path("sync-test-candidate.json"), f.candidate.to_json().unwrap()).unwrap();
            let worker = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "submission_sync::tests::accepted_sync_interruption_worker", "--ignored", "--test-threads=1"])
                .env("DESIGMA_SYNC_TEST_ROOT", &f.root).env("DESIGMA_SYNC_TEST_FILE", &f.file)
                .env("DESIGMA_SYNC_TEST_PHASE", phase).output().unwrap();
            assert_eq!(worker.status.code(), Some(92), "{phase}: {}", String::from_utf8_lossy(&worker.stderr));
            let restarted = Library::open(&f.root).unwrap();
            let original = restarted.document_submissions(&f.file).unwrap();
            assert_eq!(original.len(), 1, "{phase}");
            assert_eq!(original[0].command, f.accepted.command);
            assert_eq!(original[0].outcome, f.accepted.outcome);
            if phase == "sync:before-intent" {
                assert!(!restarted.submission_sync_path("operation").unwrap().exists());
                assert_eq!(restarted.load_checkpoint(&f.file).unwrap().to_json().unwrap(), f.original.to_json().unwrap());
                restarted.prepare_submission_sync(&f.file, &f.accepted, &f.candidate).unwrap();
            }
            let intent = restarted.load_submission_sync("operation").unwrap();
            let saved = restarted.resume_submission_sync("operation").unwrap();
            assert_eq!(saved.checkpoint.to_json().unwrap(), f.candidate.to_json().unwrap(), "{phase}");
            assert_eq!(saved.submission.state, SubmissionState::Synchronized);
            assert_eq!(saved.submission.request_digest, f.accepted.request_digest);
            assert_eq!(saved.checkpoint.retained.len(), f.original.retained.len() + 1);
            assert_eq!(restarted.document_submissions(&f.file).unwrap(), [saved.submission]);
            assert!(restarted.cityos_retention_roots().unwrap().contains(&f.submission_pin));
            assert!(restarted.save_checkpoint(&f.file, &f.original).is_err());
            let again = Library::open(&f.root).unwrap().resume_submission_sync("operation").unwrap();
            assert_eq!(again.sync_id, intent.sync_id);
            assert_eq!(again.checkpoint.to_json().unwrap(), f.candidate.to_json().unwrap());
        }
    }

    #[test]
    fn scope_receipt_revision_and_preacceptance_mismatches_cannot_create_intent() {
        let f = fixture();
        for field in ["scope", "owner", "resource", "receipt", "revision", "operation", "digest", "state"] {
            let mut wrong = f.accepted.clone();
            match field {
                "scope" => {
                    wrong.outcome.as_mut().unwrap()["scope"]["tenantId"] = json!("other-tenant");
                }
                "owner" => wrong.outcome.as_mut().unwrap()["owner"] = json!("other-owner"),
                "resource" => wrong.outcome.as_mut().unwrap()["resource"] = json!("other-resource"),
                "receipt" => wrong.outcome.as_mut().unwrap()["receipt_ref"] = json!("another-receipt"),
                "revision" => wrong.outcome.as_mut().unwrap()["owner_revision"] = json!("c".repeat(40)),
                "operation" => wrong.outcome.as_mut().unwrap()["operation_id"] = json!("another-operation"),
                "digest" => wrong.outcome.as_mut().unwrap()["request_digest"] = json!("f".repeat(64)),
                _ => wrong.state = SubmissionState::Published,
            }
            assert!(f.library.prepare_submission_sync(&f.file, &wrong, &f.candidate).is_err(), "{field}");
            assert!(!f.library.submission_sync_path("operation").unwrap().exists());
        }
        for state in [SubmissionState::Prepared, SubmissionState::Pending, SubmissionState::Published, SubmissionState::Rejected, SubmissionState::Retired] {
            let mut record = f.accepted.clone();
            record.state = state;
            write_atomic(&f.library.submission_path("operation").unwrap(), &serde_json::to_vec(&record).unwrap()).unwrap();
            assert!(f.library.prepare_submission_sync(&f.file, &record, &f.candidate).is_err());
        }
        assert_eq!(f.library.load_checkpoint(&f.file).unwrap().to_json().unwrap(), f.original.to_json().unwrap());
    }

    #[test]
    fn wrong_projection_binding_baseline_or_dirty_semantics_never_install() {
        let f = fixture();
        for field in ["resource", "repository", "revision", "document", "draft", "path", "dirty", "base", "archive", "history"] {
            let mut wrong = f.candidate.clone();
            match field {
                "archive" => wrong.retained.last_mut().unwrap().document.name = "lost user work".into(),
                "history" => wrong.history = f.original.history.clone(),
                _ => {
                    let binding = wrong.document.cityos.as_mut().unwrap();
                    match field {
                        "resource" => {
                            binding.semantic_base["id"] = json!("other");
                            binding.exchange_mut().unwrap()["semanticBase"]["id"] = json!("other");
                            binding.exchange_mut().unwrap()["semanticDraft"]["id"] = json!("other");
                        }
                        "repository" => {
                            binding.base = CityosBase::CanonicalGit { repository_id: "other".into(), commit: "b".repeat(40) };
                            binding.exchange_mut().unwrap()["base"]["repositoryId"] = json!("other");
                        }
                        "revision" => {
                            binding.base = CityosBase::CanonicalGit { repository_id: "repo".into(), commit: "c".repeat(40) };
                            binding.exchange_mut().unwrap()["base"]["commit"] = json!("c".repeat(40));
                        }
                        "document" => {
                            binding.document_id = "other".into();
                            binding.exchange_mut().unwrap()["documentId"] = json!("other");
                        }
                        "draft" => {
                            binding.draft_id = "other".into();
                            binding.exchange_mut().unwrap()["draftId"] = json!("other");
                        }
                        "path" => binding.exchange_mut().unwrap()["base"]["sourcePath"] = json!("config/other.json"),
                        "dirty" => binding.exchange_mut().unwrap()["semanticDraft"] = semantic("raised"),
                        _ => {
                            binding.semantic_base = semantic("flat");
                            binding.exchange_mut().unwrap()["semanticBase"] = semantic("flat");
                            binding.exchange_mut().unwrap()["semanticDraft"] = semantic("flat");
                        }
                    }
                }
            }
            assert!(f.library.prepare_submission_sync(&f.file, &f.accepted, &wrong).is_err(), "{field}");
            assert_eq!(f.library.load_submission("operation").unwrap(), f.accepted);
        }
    }

    #[test]
    fn original_sync_intent_rejects_replacement_checkpoint_file_or_receipt_after_restart() {
        let f = fixture();
        let intent = f.library.prepare_submission_sync(&f.file, &f.accepted, &f.candidate).unwrap();
        let reopened = Library::open(&f.root).unwrap();
        let mut replacement = f.candidate.clone();
        replacement.document.name = "different full projection".into();
        assert!(reopened.prepare_submission_sync(&f.file, &f.accepted, &replacement).is_err());
        assert!(reopened.prepare_submission_sync("another-file", &f.accepted, &f.candidate).is_err());
        let mut wrong = f.accepted.clone();
        wrong.outcome.as_mut().unwrap()["details"] = json!({"another":true});
        assert!(reopened.prepare_submission_sync(&f.file, &wrong, &f.candidate).is_err());
        assert_eq!(reopened.load_submission_sync("operation").unwrap(), intent);
        reopened.resume_submission_sync("operation").unwrap();
        assert_eq!(reopened.prepare_submission_sync(&f.file, &f.accepted, &f.candidate).unwrap(), intent);
    }

    #[test]
    fn concurrent_draft_or_baseline_writer_after_intent_is_preserved_and_sync_stays_frozen() {
        for new_baseline in [false, true] {
            let f = fixture();
            f.library.prepare_submission_sync(&f.file, &f.accepted, &f.candidate).unwrap();
            let mut concurrent = f.original.clone();
            let mut document = concurrent.document.clone();
            document.name = "concurrent user work".into();
            if new_baseline {
                concurrent.install_baseline(document);
            } else {
                concurrent.document = document;
            }
            let writer = f.library.clone();
            let id = f.file.clone();
            let saved = concurrent.to_json().unwrap();
            std::thread::spawn(move || writer.save_checkpoint(&id, &concurrent)).join().unwrap().unwrap();
            let reopened = Library::open(&f.root).unwrap();
            assert!(matches!(reopened.resume_submission_sync("operation"), Err(LibraryError::CheckpointConflict(_))));
            assert_eq!(reopened.load_checkpoint(&f.file).unwrap().to_json().unwrap(), saved);
            assert_eq!(reopened.load_submission("operation").unwrap(), f.accepted);
            assert!(reopened.cityos_retention_roots().unwrap().contains(&f.accepted_pin));
        }
    }

    #[test]
    fn interrupted_install_does_not_overwrite_edits_on_accepted_baseline() {
        let f = fixture();
        f.library.prepare_submission_sync(&f.file, &f.accepted, &f.candidate).unwrap();
        SYNC_FAULT.with(|fault| fault.set(Some("installed")));
        assert!(f.library.resume_submission_sync("operation").is_err());
        let mut edited = f.library.load_checkpoint(&f.file).unwrap();
        edited.document.name = "new work on accepted baseline".into();
        f.library.save_checkpoint(&f.file, &edited).unwrap();
        let reopened = Library::open(&f.root).unwrap();
        assert!(matches!(reopened.resume_submission_sync("operation"), Err(LibraryError::CheckpointConflict(_))));
        assert_eq!(reopened.load_checkpoint(&f.file).unwrap().to_json().unwrap(), edited.to_json().unwrap());
        assert_eq!(reopened.load_submission("operation").unwrap(), f.accepted);
    }

    #[test]
    fn stale_autosave_waits_for_sync_writer_and_cannot_cross_its_new_baseline() {
        for phase in ["before-install", "installed"] {
            let f = fixture();
            f.library.prepare_submission_sync(&f.file, &f.accepted, &f.candidate).unwrap();
            let (started, start) = std::sync::mpsc::channel();
            let (done, result) = std::sync::mpsc::channel();
            let old = f.original.clone();
            SYNC_HOOK.with(|slot| {
                *slot.borrow_mut() = Some((
                    phase,
                    Box::new(move |library, file| {
                        let library = library.clone();
                        let file = file.to_owned();
                        std::thread::spawn(move || {
                            started.send(()).unwrap();
                            done.send(library.save_file_with_history(&file, &old.document, &old.history)).unwrap();
                        });
                        start.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
                    }),
                ))
            });
            f.library.resume_submission_sync("operation").unwrap();
            assert!(matches!(result.recv_timeout(std::time::Duration::from_secs(5)).unwrap(), Err(LibraryError::CheckpointConflict(_))));
            assert_eq!(f.library.load_checkpoint(&f.file).unwrap().to_json().unwrap(), f.candidate.to_json().unwrap());
        }
    }

    #[test]
    fn readback_mismatch_keeps_original_acceptance_and_never_finalizes() {
        let f = fixture();
        f.library.prepare_submission_sync(&f.file, &f.accepted, &f.candidate).unwrap();
        let mut different = f.candidate.clone();
        different.document.name = "unexpected storage readback".into();
        let file_path = f
            .library
            .list_projects()
            .iter()
            .find_map(|project| {
                let path = f.library.path(format!("projects/{}/files/{}.desigma", project.id, f.file));
                path.exists().then_some(path)
            })
            .unwrap();
        SYNC_HOOK.with(|slot| {
            *slot.borrow_mut() = Some((
                "installed",
                Box::new(move |library, file| {
                    write_atomic(&file_path, different.document.to_json().unwrap().as_bytes()).unwrap();
                    write_atomic(&library.checkpoint_path(file).unwrap(), different.to_json().unwrap().as_bytes()).unwrap();
                }),
            ))
        });
        assert!(matches!(f.library.resume_submission_sync("operation"), Err(LibraryError::ReconciliationRequired { .. })));
        assert_eq!(f.library.load_submission("operation").unwrap(), f.accepted);
    }

    #[test]
    fn missing_accepted_payload_or_corrupt_intent_fail_closed_and_keep_original() {
        let f = fixture();
        std::fs::remove_file(f.library.path("cityos/payloads").join(&f.accepted_pin)).unwrap();
        assert!(f.library.prepare_submission_sync(&f.file, &f.accepted, &f.candidate).is_err());
        assert_eq!(f.library.load_submission("operation").unwrap(), f.accepted);
        f.library.put_cityos_payload(&f.accepted_pin, b"accepted-only dependency").unwrap();
        let mut intent = f.library.prepare_submission_sync(&f.file, &f.accepted, &f.candidate).unwrap();
        intent.file_id = "other-file".into();
        write_atomic(&f.library.submission_sync_path("operation").unwrap(), &serde_json::to_vec(&intent).unwrap()).unwrap();
        assert!(f.library.resume_submission_sync("operation").is_err());
        assert!(f.library.cityos_retention_roots().is_err());
        assert_eq!(f.library.load_submission("operation").unwrap(), f.accepted);
        assert_eq!(f.library.load_checkpoint(&f.file).unwrap().to_json().unwrap(), f.original.to_json().unwrap());
    }

    #[test]
    fn intent_storage_failure_cannot_install_or_discard_the_original_acceptance() {
        let f = fixture();
        std::fs::write(f.library.path("cityos/submission-sync"), "blocked intent directory").unwrap();
        assert!(f.library.synchronize_accepted_submission(&f.file, &f.accepted, &f.candidate).is_err());
        assert_eq!(f.library.load_submission("operation").unwrap(), f.accepted);
        assert_eq!(f.library.load_checkpoint(&f.file).unwrap().to_json().unwrap(), f.original.to_json().unwrap());
        assert!(f.library.load_cityos_payload(&f.submission_pin).is_ok());
        std::fs::remove_file(f.library.path("cityos/submission-sync")).unwrap();
        let receipt = Library::open(&f.root).unwrap().synchronize_accepted_submission(&f.file, &f.accepted, &f.candidate).unwrap();
        assert_eq!(receipt.submission.state, SubmissionState::Synchronized);
    }
}
