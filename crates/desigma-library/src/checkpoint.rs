//! Portable, self-contained local checkpoints. Importing never dispatches an
//! owner operation or confers acceptance/qualification on embedded semantic data.
use crate::{payload_digest, Library, LibraryError, Result};
use desigma_doc::{Document, DocumentCheckpoint, History};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// A verified local disk outcome, not owner acceptance or qualification.
#[derive(Clone, Debug)]
pub struct CheckpointSaveReceipt {
    pub checkpoint: DocumentCheckpoint,
    pub recovered_after_error: bool,
}

/// SHA-256 of the exact serialized checkpoint, including both history stacks
/// and retained baselines. Process-local history fences are not serialized.
/// Fingerprint the checkpoint returned by load; this token grants no owner rights.
pub fn checkpoint_fingerprint(checkpoint: &DocumentCheckpoint) -> Result<String> {
    Ok(payload_digest(checkpoint.to_json()?.as_bytes()))
}

impl Library {
    /// Retention includes inactive/trashed state and version snapshots as well
    /// as live checkpoints. A damaged source fails closed; never turn an unread
    /// history into an empty dependency set for a future collector.
    pub(crate) fn local_document_retention_roots(&self) -> Result<Vec<String>> {
        let _guard = crate::store::write_lock(self)?;
        self.recover_pending_saves_locked()?;
        self.recover_pending_versions_locked()?;
        let _versions = crate::versions::lock();
        let mut roots = BTreeSet::new();
        let mut directories = vec![self.path("projects"), self.path("versions"), self.path("trash"), self.path("checkpoints")];
        while let Some(directory) = directories.pop() {
            let entries = match std::fs::read_dir(&directory) {
                Ok(entries) => entries,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.into()),
            };
            for entry in entries {
                let entry = entry?;
                let file_type = entry.file_type()?;
                let path = entry.path();
                if file_type.is_dir() { directories.push(path); }
                else if file_type.is_file() && path.extension().is_some_and(|ext| ext == "desigma") {
                    let document = Document::from_json(&std::fs::read_to_string(&path)?)?;
                    roots.extend(DocumentCheckpoint::new(document, History::default()).dependency_pins());
                } else if file_type.is_file() && directory == self.path("checkpoints") && path.extension().is_some_and(|ext| ext == "json") {
                    roots.extend(DocumentCheckpoint::from_json(&std::fs::read_to_string(&path)?)?.dependency_pins());
                }
            }
        }
        Ok(roots.into_iter().collect())
    }

    pub(crate) fn checkpoint_path(&self, id: &str) -> Result<PathBuf> {
        crate::validate_id(id)?;
        Ok(self.path("checkpoints").join(format!("{id}.json")))
    }

    /// A document predating checkpoints gets an empty history. A damaged,
    /// unsupported or mismatched checkpoint is an error, never an empty-stack
    /// fallback that silently loses the recoverable original history.
    pub fn load_checkpoint(&self, id: &str) -> Result<DocumentCheckpoint> {
        let _guard = crate::store::write_lock(self)?;
        self.load_checkpoint_locked(id)
    }

    pub(crate) fn load_checkpoint_locked(&self, id: &str) -> Result<DocumentCheckpoint> {
        let checkpoint = self.load_checkpoint_state_locked(id)?;
        for hash in checkpoint.dependency_pins() { self.verify_cityos_payload(&hash)?; }
        Ok(checkpoint)
    }

    /// Recover the original document and history when legacy dependency bytes
    /// are missing. The caller must keep this state read-only/incomplete until
    /// every returned pin has been verified and materialized. Corrupt payloads
    /// remain errors rather than being accepted as absent.
    pub fn load_checkpoint_for_recovery(&self, id: &str) -> Result<(DocumentCheckpoint, Vec<String>)> {
        let _guard = crate::store::write_lock(self)?;
        let checkpoint = self.load_checkpoint_state_locked(id)?;
        let mut missing = Vec::new();
        for hash in checkpoint.dependency_pins() {
            match self.verify_cityos_payload(&hash) {
                Ok(_) => {}
                Err(LibraryError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => missing.push(hash),
                Err(e) => return Err(e),
            }
        }
        Ok((checkpoint, missing))
    }

    pub(crate) fn load_checkpoint_state_locked(&self, id: &str) -> Result<DocumentCheckpoint> {
        let path = self.checkpoint_path(id)?;
        let document = self.load_file_locked(id)?;
        let checkpoint = match std::fs::read_to_string(path) {
            Ok(text) => {
                let checkpoint = DocumentCheckpoint::from_json(&text)?;
                if checkpoint.document.to_json()? != document.to_json()? {
                    return Err(LibraryError::Invalid("Checkpoint does not match the current document".into()));
                }
                checkpoint
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => DocumentCheckpoint::new(document, History::default()),
            Err(e) => return Err(e.into()),
        };
        Ok(checkpoint)
    }

    /// Convenience for the app's normal autosave path. Previously archived
    /// baseline histories are carried forward in the same save transaction.
    /// For a new baseline, use `DocumentCheckpoint::install_baseline` and
    /// `save_checkpoint` instead so the pre-baseline document is also retained.
    pub fn save_file_with_history(&self, id: &str, document: &Document, history: &History) -> Result<()> {
        let _guard = crate::store::write_lock(self)?;
        let mut checkpoint = self.load_checkpoint_state_locked(id)?;
        if checkpoint.history.baseline_id() != history.baseline_id() {
            return Err(LibraryError::CheckpointConflict(id.into()));
        }
        checkpoint.document = document.clone();
        checkpoint.history = history.clone();
        self.save_checkpoint_payloads_locked(id, &checkpoint, BTreeMap::new())
    }

    /// Save only if the complete persisted predecessor still matches the loaded
    /// checkpoint. Return the fingerprint of the reconciled saved generation.
    /// The legacy unconditional API remains for explicit internal workflows.
    pub fn save_file_with_history_expected(&self, id: &str, document: &Document, history: &History, expected_fingerprint: &str) -> Result<String> {
        let _guard = crate::store::write_lock(self)?;
        let mut checkpoint = self.check_checkpoint_fingerprint_locked(id, expected_fingerprint)?;
        if checkpoint.history.baseline_id() != history.baseline_id() {
            return Err(LibraryError::CheckpointConflict(id.into()));
        }
        checkpoint.document = document.clone();
        checkpoint.history = history.clone();
        let receipt = self.save_checkpoint_reconciled_locked(id, &checkpoint)?;
        checkpoint_fingerprint(&receipt.checkpoint)
    }

    /// Compare without replaying any pending document intent. A caller must
    /// explicitly recover/reload first, then fingerprint that loaded generation.
    /// Otherwise CAS itself could change disk before rejecting a stale writer.
    fn check_checkpoint_fingerprint_locked(&self, id: &str, expected: &str) -> Result<DocumentCheckpoint> {
        crate::validate_id(id)?;
        match std::fs::read_dir(self.path("projects")) {
            Ok(entries) => {
                for entry in entries {
                    let entry = entry?;
                    if entry.file_type()?.is_dir() && entry.path().join(".save.pending.json").try_exists()? {
                        return Err(LibraryError::ReconciliationRequired {
                            file_id: id.into(), save_error: "CAS save was not attempted".into(),
                            recovery_error: "Recover the pending document intent explicitly before comparing checkpoints".into(),
                        });
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        // No document intent can appear while this root guard is held.
        let checkpoint = self.load_checkpoint_state_locked(id)?;
        if checkpoint_fingerprint(&checkpoint)? != expected {
            return Err(LibraryError::CheckpointConflict(id.into()));
        }
        Ok(checkpoint)
    }

    /// Save, recover an error-after-journal, and read back without releasing the
    /// root/process writer lock. Install only the returned checkpoint in the UI. An
    /// unresolved error must block editing/saving until recovery is conclusive.
    /// Recovered new baselines also fence out stale autosaves in the library.
    pub fn save_checkpoint_reconciled(&self, id: &str, candidate: &DocumentCheckpoint) -> Result<CheckpointSaveReceipt> {
        let _guard = crate::store::write_lock(self)?;
        self.save_checkpoint_reconciled_locked(id, candidate)
    }

    /// CAS the complete predecessor before any save or recovery write, retaining
    /// the same root/process guard through installation and exact readback.
    pub fn save_checkpoint_reconciled_expected(&self, id: &str, candidate: &DocumentCheckpoint, expected_fingerprint: &str) -> Result<CheckpointSaveReceipt> {
        let _guard = crate::store::write_lock(self)?;
        self.check_checkpoint_fingerprint_locked(id, expected_fingerprint)?;
        self.save_checkpoint_reconciled_locked(id, candidate)
    }

    /// Caller retains the writer lock through its compare/install/readback transaction.
    pub(crate) fn save_checkpoint_reconciled_locked(&self, id: &str, candidate: &DocumentCheckpoint) -> Result<CheckpointSaveReceipt> {
        let expected = DocumentCheckpoint::from_json(&candidate.to_json()?)?.to_json()?;
        let outcome = self.save_checkpoint_payloads_locked(id, candidate, BTreeMap::new());
        match self.load_checkpoint_locked(id) {
            Ok(checkpoint) if checkpoint.to_json()? == expected => Ok(CheckpointSaveReceipt {
                checkpoint, recovered_after_error: outcome.is_err(),
            }),
            Ok(_) => match outcome {
                Err(error) => Err(error),
                Ok(()) => Err(LibraryError::ReconciliationRequired {
                    file_id: id.into(), save_error: "Save returned success".into(),
                    recovery_error: "Readback does not match candidate".into(),
                }),
            },
            Err(error) => Err(LibraryError::ReconciliationRequired {
                file_id: id.into(),
                save_error: outcome.err().map(|error| error.to_string()).unwrap_or_else(|| "Save returned success".into()),
                recovery_error: error.to_string(),
            }),
        }
    }

    /// Ordinary saves must retain the exact archive; a new baseline must append
    /// exactly one archive from the currently persisted history lineage. This
    /// rejects old whole-checkpoint retries after another baseline was recovered.
    pub(crate) fn check_checkpoint_lineage_locked(&self, id: &str, candidate: &DocumentCheckpoint) -> Result<()> {
        let previous = self.load_checkpoint_state_locked(id)?;
        // A pre-checkpoint/fresh file has no baseline fence yet. Explicit import
        // may bring multiple inactive histories into that new local lineage.
        if previous.history.baseline_id().is_none() && previous.retained.is_empty() {
            return Ok(());
        }
        let same_baseline = previous.history.baseline_id() == candidate.history.baseline_id();
        let expected_len = previous.retained.len() + usize::from(!same_baseline);
        if candidate.retained.len() != expected_len || (!same_baseline && (
            candidate.history.baseline_id().is_none()
            || candidate.retained.last().and_then(|state| state.history.baseline_id()) != previous.history.baseline_id()
        )) {
            return Err(LibraryError::CheckpointConflict(id.into()));
        }
        for (old, new) in previous.retained.iter().zip(&candidate.retained) {
            if old.document.to_json()? != new.document.to_json()? || serde_json::to_value(&old.history)? != serde_json::to_value(&new.history)? {
                return Err(LibraryError::CheckpointConflict(id.into()));
            }
        }
        Ok(())
    }

    pub fn load_file_with_history(&self, id: &str) -> Result<(Document, History)> {
        let checkpoint = self.load_checkpoint(id)?;
        Ok((checkpoint.document, checkpoint.history))
    }

    /// Exports native document, active and inactive history, embedded assets and
    /// the exact CityOS payload closure. No filesystem paths or credentials are
    /// added. Shared-library copies already live inside the document; remote
    /// library update services and owner eligibility remain external capabilities.
    pub fn export_checkpoint(&self, id: &str) -> Result<String> {
        // Match prepare's lock order. The checkpoint and original submission
        // journal must be observed together; a later prepare cannot race export.
        let _submissions = crate::submissions::SUBMISSION_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let _guard = crate::store::write_lock(self)?;
        let checkpoint = self.load_checkpoint_locked(id)?;
        self.ensure_portable_submission_safe_locked(Some(id), &checkpoint)?;
        crate::portable::check_policy(&checkpoint)?;
        let payloads = checkpoint.dependency_pins().into_iter()
            .map(|hash| self.load_cityos_payload(&hash).map(|bytes| (hash, bytes))).collect::<Result<_>>()?;
        crate::portable::encode(&checkpoint, payloads)
    }

    /// Imports into an existing library file. Validate everything before the
    /// durable intent is written; dependencies then travel in that same journal.
    /// The parent owns whether an incoming baseline should be archived/reconciled
    /// before import, and must not treat this local import as owner acceptance.
    pub fn import_checkpoint(&self, id: &str, text: &str) -> Result<()> {
        let (checkpoint, payloads) = crate::VerifiedPortableCheckpoint::parse(text)?.into_parts();
        let _submissions = crate::submissions::SUBMISSION_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let _guard = crate::store::write_lock(self)?;
        self.ensure_portable_submission_safe_locked(Some(id), &checkpoint)?;
        self.save_checkpoint_payloads_locked(id, &checkpoint, payloads)
    }

    /// Explicit local restore into a fresh file identity. There is no empty
    /// placeholder: document, active/inactive history, payloads and manifest
    /// share the existing replayable save journal. Opening grants no owner right.
    pub fn import_checkpoint_as_new(&self, project: &str, folder: Option<&str>, portable: crate::VerifiedPortableCheckpoint) -> Result<crate::FileEntry> {
        let (checkpoint, payloads) = portable.into_parts();
        let _submissions = crate::submissions::SUBMISSION_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let _guard = crate::store::write_lock(self)?;
        self.ensure_portable_submission_safe_locked(None, &checkpoint)?;
        let name = if checkpoint.document.name.trim().is_empty() { "Imported draft" } else { checkpoint.document.name.as_str() };
        self.create_file_state_locked(project, folder, name, &checkpoint.document, Some((checkpoint.to_json()?, payloads)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use desigma_doc::{CityosBase, CityosBinding, Document};

    fn disk_bytes(root: &std::path::Path) -> BTreeMap<PathBuf, Vec<u8>> {
        fn visit(root: &std::path::Path, directory: &std::path::Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for entry in std::fs::read_dir(directory).unwrap() {
                let entry = entry.unwrap();
                if entry.file_type().unwrap().is_dir() { visit(root, &entry.path(), files); }
                else { files.insert(entry.path().strip_prefix(root).unwrap().to_owned(), std::fs::read(entry.path()).unwrap()); }
            }
        }
        let mut files = BTreeMap::new();
        visit(root, root, &mut files);
        files
    }

    #[test]
    fn expected_history_save_rejects_stale_same_baseline_without_changing_disk() {
        for sealed in [false, true] {
            for change_document in [false, true] {
                let (root, library, id) = Temp::new();
                if sealed {
                    let mut checkpoint = library.load_checkpoint(&id).unwrap();
                    checkpoint.install_baseline(Document::new("sealed baseline"));
                    library.save_checkpoint(&id, &checkpoint).unwrap();
                }
                let other = Library::open(&root.0).unwrap();
                let mut a = library.load_checkpoint(&id).unwrap();
                let mut b = other.load_checkpoint(&id).unwrap();
                let expected = checkpoint_fingerprint(&a).unwrap();
                if change_document { b.document.name = "B is current".into(); }
                // Even identical document bytes with a changed history stack must conflict.
                b.history.exec(&mut b.document, desigma_doc::Command::SetComments { comments: vec![] }).unwrap();
                assert_eq!(a.history.baseline_id(), b.history.baseline_id());
                let saved = other.save_file_with_history_expected(&id, &b.document, &b.history, &expected).unwrap();
                assert_ne!(saved, expected);
                assert_eq!(saved, checkpoint_fingerprint(&other.load_checkpoint(&id).unwrap()).unwrap());
                let before = disk_bytes(&root.0);
                a.document.name = "stale A".into();
                a.history.exec(&mut a.document, desigma_doc::Command::SetComments { comments: vec![] }).unwrap();
                assert!(matches!(library.save_file_with_history_expected(&id, &a.document, &a.history, &expected),
                    Err(LibraryError::CheckpointConflict(ref file)) if file == &id));
                assert_eq!(disk_bytes(&root.0), before);
                assert_eq!(library.load_checkpoint(&id).unwrap().to_json().unwrap(), b.to_json().unwrap());
            }
        }
    }

    #[test]
    fn expected_reconciled_save_rejects_stale_predecessor_before_sealing_or_writing() {
        let (root, library, id) = Temp::new();
        let mut initial = library.load_checkpoint(&id).unwrap();
        initial.install_baseline(Document::new("shared baseline"));
        library.save_checkpoint(&id, &initial).unwrap();
        let other = Library::open(&root.0).unwrap();
        let a = library.load_checkpoint(&id).unwrap();
        let mut b = other.load_checkpoint(&id).unwrap();
        let old_fingerprint = checkpoint_fingerprint(&a).unwrap();
        b.document.name = "B newer edit".into();
        b.history.exec(&mut b.document, desigma_doc::Command::SetComments { comments: vec![] }).unwrap();
        let current = other.save_file_with_history_expected(&id, &b.document, &b.history, &old_fingerprint).unwrap();
        assert_eq!(a.history.baseline_id(), b.history.baseline_id());
        let mut incoming = a;
        incoming.install_baseline(Document::new("reviewed from stale A"));
        let before = disk_bytes(&root.0);
        assert!(matches!(library.save_checkpoint_reconciled_expected(&id, &incoming, &old_fingerprint),
            Err(LibraryError::CheckpointConflict(ref file)) if file == &id));
        assert_eq!(disk_bytes(&root.0), before);
        assert_eq!(library.load_checkpoint(&id).unwrap().to_json().unwrap(), b.to_json().unwrap());
        let mut incoming = b.clone();
        incoming.install_baseline(Document::new("reviewed from current B"));
        let receipt = library.save_checkpoint_reconciled_expected(&id, &incoming, &current).unwrap();
        assert_eq!(receipt.checkpoint.to_json().unwrap(), incoming.to_json().unwrap());
        let archived = receipt.checkpoint.retained.last().unwrap();
        assert_eq!(archived.document.to_json().unwrap(), b.document.to_json().unwrap());
        assert_eq!(serde_json::to_value(&archived.history).unwrap(), serde_json::to_value(&b.history).unwrap());
    }

    #[test]
    fn expected_saves_never_replay_pending_intents_during_comparison() {
        let (root, library, id) = Temp::new();
        let original = library.load_checkpoint(&id).unwrap();
        let fingerprint = checkpoint_fingerprint(&original).unwrap();
        let mut candidate = original.clone();
        candidate.install_baseline(Document::new("committed intent"));
        crate::store::inject_save_fault("journal", 0);
        assert!(library.save_checkpoint(&id, &candidate).is_err());
        let before = disk_bytes(&root.0);
        assert!(matches!(library.save_file_with_history_expected(&id, &original.document, &original.history, &fingerprint),
            Err(LibraryError::ReconciliationRequired { .. })));
        assert_eq!(disk_bytes(&root.0), before);
        assert!(matches!(library.save_checkpoint_reconciled_expected(&id, &candidate, &fingerprint),
            Err(LibraryError::ReconciliationRequired { .. })));
        assert_eq!(disk_bytes(&root.0), before);
        // Explicit recovery, followed by a fresh load/fingerprint, remains available.
        let recovered = library.load_checkpoint(&id).unwrap();
        assert_eq!(recovered.to_json().unwrap(), candidate.to_json().unwrap());
        let current = checkpoint_fingerprint(&recovered).unwrap();
        assert_ne!(current, fingerprint);
        let receipt = library.save_checkpoint_reconciled_expected(&id, &recovered, &current).unwrap();
        assert_eq!(receipt.checkpoint.to_json().unwrap(), recovered.to_json().unwrap());
    }

    #[test]
    fn expected_history_save_returns_the_recovered_fingerprint_after_its_own_journal_commits() {
        for phase in ["journal", "document", "checkpoint", "manifest", "complete"] {
            let (_root, library, id) = Temp::new();
            let mut checkpoint = library.load_checkpoint(&id).unwrap();
            let expected = checkpoint_fingerprint(&checkpoint).unwrap();
            checkpoint.document.name = "recover this exact save".into();
            checkpoint.history.exec(&mut checkpoint.document, desigma_doc::Command::SetComments { comments: vec![] }).unwrap();
            crate::store::inject_save_fault(phase, 0);
            let saved = library.save_file_with_history_expected(&id, &checkpoint.document, &checkpoint.history, &expected).unwrap();
            let recovered = library.load_checkpoint(&id).unwrap();
            assert_eq!(saved, checkpoint_fingerprint(&recovered).unwrap(), "{phase}");
            assert_eq!(recovered.to_json().unwrap(), checkpoint.to_json().unwrap(), "{phase}");
        }
    }

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> (Self, Library, String) {
            let path = std::env::temp_dir().join(format!("desigma_checkpoint_{}_{}", std::process::id(), crate::new_id()));
            let library = Library::open(&path).unwrap();
            let project = library.create_project("P").unwrap();
            let file = library.create_file(&project.id, None, "F", &Document::new("empty")).unwrap();
            (Self(path), library, file.id)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
    }

    fn bound(hash: String) -> Document {
        let mut document = Document::new("bound");
        document.images.insert("image".into(), vec![2, 4, 6]);
        document.cityos = Some(CityosBinding {
            schema_version: "cityos.design-exchange.v1".into(), document_id: "document".into(), draft_id: "draft".into(),
            base: CityosBase::CanonicalGit { repository_id: "repo".into(), commit: "a".repeat(40) },
            semantic_base: serde_json::json!({"preserved":true}), dependency_pins: vec![hash], occurrences: BTreeMap::new(),
            exchange_v2: None, exchange_v3: None,
        });
        document
    }

    #[test]
    fn portable_checkpoint_transfers_inactive_baseline_dependencies_to_an_empty_library() {
        let (_source, source, source_id) = Temp::new();
        let hash = source.retain_cityos_payload(b"retained baseline dependency").unwrap();
        let mut checkpoint = DocumentCheckpoint::new(bound(hash.clone()), History::default());
        checkpoint.install_baseline(Document::new("incoming"));
        source.save_checkpoint(&source_id, &checkpoint).unwrap();
        let portable = source.export_checkpoint(&source_id).unwrap();
        let (target_dir, target, target_id) = Temp::new();
        target.import_checkpoint(&target_id, &portable).unwrap();
        drop(target);
        let reopened = Library::open(&target_dir.0).unwrap();
        let mut restored = reopened.load_checkpoint(&target_id).unwrap();
        assert_eq!(restored.document.name, "incoming");
        assert!(!restored.history.undo(&mut restored.document).unwrap());
        assert_eq!(restored.retained[0].document.images["image"], [2, 4, 6]);
        assert_eq!(restored.dependency_pins(), [hash.clone()]);
        assert_eq!(reopened.load_cityos_payload(&hash).unwrap(), b"retained baseline dependency");
    }

    #[test]
    fn portable_file_import_creates_one_complete_generation_with_bindings_history_and_payloads() {
        let (_source, source, source_id) = Temp::new();
        let hash = source.retain_cityos_payload(b"portable file dependency").unwrap();
        let mut checkpoint = DocumentCheckpoint::new(bound(hash.clone()), History::default());
        let mut next = bound(hash.clone()); next.name = "Portable restored draft".into();
        let binding = next.cityos.as_mut().unwrap();
        binding.schema_version = "cityos.design-exchange.v2".into();
        binding.exchange_v2 = Some(serde_json::json!({"schemaVersion":binding.schema_version,
            "documentId":binding.document_id,"draftId":binding.draft_id,"draftRevision":0,
            "base":{"kind":"canonical-git","repositoryId":"repo","commit":"a".repeat(40)},
            "semanticBase":binding.semantic_base,"semanticDraft":binding.semantic_base}));
        binding.validate().unwrap();
        checkpoint.install_baseline(next);
        let mut edited = checkpoint.document.clone(); edited.name.push_str(" edited");
        checkpoint.history.exec(&mut checkpoint.document, desigma_doc::Command::CityosInstall { document: Box::new(edited) }).unwrap();
        source.save_checkpoint(&source_id, &checkpoint).unwrap();
        let (target_dir, target, _) = Temp::new();
        let project = target.list_projects()[0].id.clone();
        let before = target.files_in(&project, None).unwrap().len();
        let file = target_dir.0.join("restore.portable.json");
        std::fs::write(&file, source.export_checkpoint(&source_id).unwrap()).unwrap();
        let imported = target.import_file(&file, &project, None).expect("Portable export must reopen through the normal file import path");
        drop(target);
        let reopened = Library::open(&target_dir.0).unwrap();
        assert_eq!(reopened.files_in(&project, None).unwrap().len(), before + 1);
        let restored = reopened.load_checkpoint(&imported.id).unwrap();
        assert_eq!(restored.to_json().unwrap(), checkpoint.to_json().unwrap());
        assert!(restored.history.can_undo());
        assert_eq!(restored.retained.len(), 1);
        assert_eq!(reopened.load_cityos_payload(&hash).unwrap(), b"portable file dependency");
        assert!(!reopened.path("projects").join(&project).join(".save.pending.json").exists());
    }

    #[test]
    fn portable_export_does_not_claim_native_retention_policy_for_active_or_retained_owner_data() {
        for retained in [false, true] {
            let (_dir, library, id) = Temp::new();
            let hash = library.retain_cityos_payload(b"Native fixture data without export qualification").unwrap();
            let mut document = bound(hash);
            document.cityos.as_mut().unwrap().base = CityosBase::NativeOwner { owner: "theme-owner".into(), resource: "theme".into(), revision: "1".into() };
            let mut checkpoint = DocumentCheckpoint::new(document, History::default());
            if retained { checkpoint.install_baseline(Document::new("Current repository design")); }
            library.save_checkpoint(&id, &checkpoint).unwrap();
            assert!(library.export_checkpoint(&id).is_err(), "Unqualified Native payloads cannot enter portable files through history");
        }
    }

    #[test]
    fn portable_new_file_replays_one_complete_generation_after_every_save_boundary() {
        let (_source_dir, source, source_id) = Temp::new();
        let hash = source.retain_cityos_payload(b"portable recovery dependency").unwrap();
        let checkpoint = DocumentCheckpoint::new(bound(hash.clone()), History::default());
        source.save_checkpoint(&source_id, &checkpoint).unwrap();
        let portable = source.export_checkpoint(&source_id).unwrap();
        for phase in ["before-journal", "journal", "payloads", "document", "checkpoint", "manifest", "complete"] {
            let (directory, target, _) = Temp::new();
            let project = target.list_projects()[0].id.clone();
            let before: BTreeSet<_> = target.files_in(&project, None).unwrap().into_iter().map(|file| file.id).collect();
            crate::store::inject_save_fault(phase, 0);
            let verified = crate::VerifiedPortableCheckpoint::parse(&portable).unwrap();
            assert!(target.import_checkpoint_as_new(&project, None, verified).is_err(), "{phase}");
            drop(target);
            let reopened = Library::open(&directory.0).unwrap();
            let imported: Vec<_> = reopened.files_in(&project, None).unwrap().into_iter().filter(|file| !before.contains(&file.id)).collect();
            if phase == "before-journal" {
                assert!(imported.is_empty());
            } else {
                assert_eq!(imported.len(), 1, "{phase}");
                assert_eq!(reopened.load_checkpoint(&imported[0].id).unwrap().to_json().unwrap(), checkpoint.to_json().unwrap(), "{phase}");
                assert_eq!(reopened.load_cityos_payload(&hash).unwrap(), b"portable recovery dependency", "{phase}");
            }
            assert!(!reopened.path("projects").join(&project).join(".save.pending.json").exists(), "{phase}");
        }
    }

    #[test]
    fn portable_corruption_missing_extra_and_future_payloads_leave_target_untouched() {
        let (_source, source, source_id) = Temp::new();
        let hash = source.retain_cityos_payload(b"dependency").unwrap();
        source.save_checkpoint(&source_id, &DocumentCheckpoint::new(bound(hash.clone()), History::default())).unwrap();
        let original: serde_json::Value = serde_json::from_str(&source.export_checkpoint(&source_id).unwrap()).unwrap();
        let (_target_dir, target, target_id) = Temp::new();
        for fault in ["missing", "corrupt", "extra", "future"] {
            let mut value = original.clone();
            match fault {
                "missing" => { value["payloads"].as_object_mut().unwrap().remove(&hash); }
                "corrupt" => value["payloads"][&hash] = serde_json::json!([0]),
                "extra" => { value["payloads"][payload_digest(b"extra")] = serde_json::json!(b"extra".to_vec()); }
                _ => value["schemaVersion"] = serde_json::json!("future"),
            }
            assert!(target.import_checkpoint(&target_id, &value.to_string()).is_err(), "{fault}");
            assert_eq!(target.load_file(&target_id).unwrap().name, "empty");
            assert!(!target.path("cityos/payloads").exists());
            assert!(!target.checkpoint_path(&target_id).unwrap().exists());
        }
    }

    #[test]
    fn convenience_history_saves_preserve_inactive_baseline_archives() {
        let (_dir, library, id) = Temp::new();
        let mut checkpoint = DocumentCheckpoint::new(Document::new("old baseline"), History::default());
        checkpoint.install_baseline(Document::new("new baseline"));
        library.save_checkpoint(&id, &checkpoint).unwrap();
        let (mut document, mut history) = library.load_file_with_history(&id).unwrap();
        history.exec(&mut document, desigma_doc::Command::SetComments { comments: vec![] }).unwrap();
        library.save_file_with_history(&id, &document, &history).unwrap();
        let reopened = Library::open(library.root()).unwrap();
        let checkpoint = reopened.load_checkpoint(&id).unwrap();
        assert!(checkpoint.history.can_undo());
        assert_eq!(checkpoint.retained.len(), 1);
        assert_eq!(checkpoint.retained[0].document.name, "old baseline");
    }

    #[test]
    fn retention_roots_include_inactive_histories_versions_and_trashed_files() {
        let (_dir, library, id) = Temp::new();
        let first = library.retain_cityos_payload(b"inactive history").unwrap();
        let second = library.retain_cityos_payload(b"named version").unwrap();
        let mut checkpoint = DocumentCheckpoint::new(bound(first.clone()), History::default());
        checkpoint.install_baseline(Document::new("current"));
        library.save_checkpoint(&id, &checkpoint).unwrap();
        library.save_version(&id, &bound(second.clone()), Some("Keep".into()), false).unwrap();
        library.trash_file(&id).unwrap();
        let mut expected = vec![first, second]; expected.sort();
        assert_eq!(library.cityos_retention_roots().unwrap(), expected);
        std::fs::write(library.checkpoint_path(&id).unwrap(), "{truncated").unwrap();
        assert!(library.cityos_retention_roots().is_err());
    }

    #[test]
    fn recovery_exposes_missing_pins_then_verified_materialization_enables_history_save() {
        let (_dir, library, id) = Temp::new();
        let bytes = b"exact artifact cache bytes";
        let hash = payload_digest(bytes);
        library.save_file(&id, &bound(hash.clone())).unwrap();
        let (checkpoint, missing) = library.load_checkpoint_for_recovery(&id).unwrap();
        assert_eq!(missing, [hash.clone()]);
        assert_eq!(checkpoint.document.name, "bound");
        assert!(library.save_file_with_history(&id, &checkpoint.document, &checkpoint.history).is_err());
        assert!(library.put_cityos_payload(&hash, b"incorrect").is_err());
        assert!(!library.path("cityos/payloads").exists());
        library.put_cityos_payload(&hash, bytes).unwrap();
        library.save_file_with_history(&id, &checkpoint.document, &checkpoint.history).unwrap();
        assert!(library.load_checkpoint_for_recovery(&id).unwrap().1.is_empty());
        assert!(library.load_checkpoint(&id).is_ok());
        assert!(library.put_cityos_payload(&hash, b"replacement").is_err());
        assert_eq!(library.load_cityos_payload(&hash).unwrap(), bytes);
    }

    #[test]
    fn missing_active_undo_redo_and_inactive_dependencies_preserve_exact_recovery_and_export_closure() {
        let (_dir, library, id) = Temp::new();
        let payloads: Vec<_> = [b"inactive".as_slice(), b"undo-only".as_slice(), b"active".as_slice(), b"redo-only".as_slice()]
            .into_iter().map(|bytes| (library.retain_cityos_payload(bytes).unwrap(), bytes)).collect();
        // Whole-document history commands require an expanded exchange; a
        // legacy binding cannot supply the monotonic draftRevision fence.
        let expanded = |hash: String| {
            let mut document = bound(hash.clone());
            let binding = document.cityos.as_mut().unwrap();
            binding.schema_version = "cityos.design-exchange.v3".into();
            binding.exchange_v3 = Some(serde_json::json!({
                "schemaVersion":"cityos.design-exchange.v3", "documentId":"document", "draftId":"draft", "draftRevision":0,
                "semanticContract":"fixture", "commandReceipts":[],
                "base":{"kind":"canonical-git", "repositoryId":"repo", "commit":"a".repeat(40)},
                "semanticBase":binding.semantic_base, "semanticDraft":{"id":"frame"}, "dependencyPins":[{"digest":hash}],
            }));
            document
        };
        let mut checkpoint = DocumentCheckpoint::new(expanded(payloads[0].0.clone()), History::default());
        checkpoint.install_baseline(expanded(payloads[1].0.clone()));
        for index in [2, 3] {
            checkpoint.history.exec(&mut checkpoint.document, desigma_doc::Command::CityosInstall {
                document: Box::new(expanded(payloads[index].0.clone())),
            }).unwrap();
        }
        checkpoint.history.undo(&mut checkpoint.document).unwrap();
        assert!(checkpoint.history.can_undo());
        assert!(checkpoint.history.can_redo());
        library.save_checkpoint(&id, &checkpoint).unwrap();
        let expected = checkpoint.to_json().unwrap();
        let original_bytes = std::fs::read(library.checkpoint_path(&id).unwrap()).unwrap();
        for (pin, bytes) in &payloads {
            let path = library.path("cityos/payloads").join(pin);
            std::fs::remove_file(&path).unwrap();
            let restarted = Library::open(library.root()).unwrap();
            let (recovered, missing) = restarted.load_checkpoint_for_recovery(&id).unwrap();
            assert_eq!(recovered.to_json().unwrap(), expected);
            assert_eq!(missing, [pin.clone()]);
            assert!(restarted.load_checkpoint(&id).is_err());
            assert!(restarted.export_checkpoint(&id).is_err());
            assert!(restarted.cityos_retention_roots().unwrap().contains(pin));
            assert_eq!(std::fs::read(restarted.checkpoint_path(&id).unwrap()).unwrap(), original_bytes);
            std::fs::write(&path, b"corrupt dependency").unwrap();
            assert!(restarted.load_checkpoint_for_recovery(&id).is_err(), "corruption must not become a missing-only recovery");
            assert!(restarted.put_cityos_payload(pin, b"wrong bytes").is_err());
            restarted.put_cityos_payload(pin, bytes).unwrap();
            assert_eq!(restarted.load_checkpoint(&id).unwrap().to_json().unwrap(), expected);
        }
        let portable = library.export_checkpoint(&id).unwrap();
        let (target_dir, target, target_id) = Temp::new();
        target.import_checkpoint(&target_id, &portable).unwrap();
        let imported = Library::open(&target_dir.0).unwrap();
        assert_eq!(imported.load_checkpoint(&target_id).unwrap().to_json().unwrap(), expected);
        assert_eq!(imported.cityos_retention_roots().unwrap(), checkpoint.dependency_pins());
        for (pin, bytes) in &payloads { assert_eq!(imported.load_cityos_payload(pin).unwrap(), *bytes); }
    }

    #[test]
    fn complete_checkpoint_rejects_unmaterialized_dependencies_but_legacy_save_preserves_them() {
        let (_dir, library, id) = Temp::new();
        let document = bound("b".repeat(64));
        library.save_file(&id, &document).unwrap();
        assert_eq!(library.load_file(&id).unwrap().cityos.unwrap().dependency_pins, ["b".repeat(64)]);
        assert!(library.save_checkpoint(&id, &DocumentCheckpoint::new(document, History::default())).is_err());
        assert!(library.export_checkpoint(&id).is_err());
        assert!(!library.checkpoint_path(&id).unwrap().exists());
    }

    #[test]
    fn legacy_save_invalidates_checkpoint_instead_of_reusing_wrong_history() {
        let (_dir, library, id) = Temp::new();
        library.save_checkpoint(&id, &DocumentCheckpoint::new(Document::new("checkpoint"), History::default())).unwrap();
        library.save_file(&id, &Document::new("replaced")).unwrap();
        assert!(!library.checkpoint_path(&id).unwrap().exists());
        assert_eq!(library.load_checkpoint(&id).unwrap().document.name, "replaced");
    }

    #[test]
    fn checkpoint_mismatch_and_corruption_do_not_silently_discard_history() {
        let (_dir, library, id) = Temp::new();
        library.save_checkpoint(&id, &DocumentCheckpoint::new(Document::new("saved"), History::default())).unwrap();
        let path = library.checkpoint_path(&id).unwrap();
        std::fs::write(&path, DocumentCheckpoint::new(Document::new("different"), History::default()).to_json().unwrap()).unwrap();
        assert!(library.load_checkpoint(&id).is_err());
        std::fs::write(path, "truncated {").unwrap();
        assert!(library.load_checkpoint(&id).is_err());
    }
}
