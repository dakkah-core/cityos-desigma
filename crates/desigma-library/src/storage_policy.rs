//! Explicit extension policy over generic checkpoint storage. No owner authority
//! or policy registration is inferred from finding a binding on disk.
use crate::{LibraryError, Result};
use desigma_doc::{Document, DocumentCheckpoint};
use std::collections::BTreeSet;

pub trait CheckpointStoragePolicy {
    /// Explicit installation only. Discovering a journal or binding grants no
    /// semantic interpretation or submission capability.
    fn submission_policy(&self) -> Option<&dyn crate::SubmissionPolicy> { None }
    /// Authorization/retention policy belongs to the installed extension.
    fn check_portable(&self, checkpoint: &DocumentCheckpoint) -> Result<()>;
    /// Return the complete dependency union, including inactive and redo state.
    /// An unreadable/unrecognized state must fail, never become an empty union.
    fn retention_roots(&self, checkpoint: &DocumentCheckpoint) -> Result<Vec<String>>;
}

/// Current, inactive-baseline and whole-document undo/redo states, including
/// nested batches. The library traverses native state, not extension semantics.
pub fn checkpoint_documents(checkpoint: &DocumentCheckpoint) -> Vec<&Document> {
    std::iter::once((&checkpoint.document, &checkpoint.history))
        .chain(checkpoint.retained.iter().map(|state| (&state.document, &state.history)))
        .flat_map(|(document, history)| std::iter::once(document).chain(history.retained_documents()))
        .collect()
}

#[derive(Clone, Copy, Debug)]
pub struct UnmanagedStoragePolicy;
pub const UNMANAGED_STORAGE_POLICY: UnmanagedStoragePolicy = UnmanagedStoragePolicy;

impl CheckpointStoragePolicy for UnmanagedStoragePolicy {
    fn check_portable(&self, checkpoint: &DocumentCheckpoint) -> Result<()> {
        if checkpoint_documents(checkpoint).iter().any(|document| document.is_managed()) {
            return Err(LibraryError::Invalid("Managed checkpoint requires its installed extension storage policy".into()));
        }
        Ok(())
    }
    fn retention_roots(&self, checkpoint: &DocumentCheckpoint) -> Result<Vec<String>> {
        self.check_portable(checkpoint)?;
        Ok(Vec::new())
    }
}

pub(crate) fn checked_retention_roots(policy: &dyn CheckpointStoragePolicy, checkpoint: &DocumentCheckpoint) -> Result<Vec<String>> {
    let roots = policy.retention_roots(checkpoint)?;
    if roots.iter().any(|root| root.len() != 64 || !root.bytes().all(|byte| byte.is_ascii_hexdigit())) {
        return Err(LibraryError::Invalid("Extension policy returned an invalid dependency digest".into()));
    }
    Ok(roots.into_iter().collect::<BTreeSet<_>>().into_iter().collect())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use desigma_doc::{CityosBase, CityosBinding, Command, History, RetainedHistory};
    use crate::{Library, VerifiedPortableCheckpoint};
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    /// A fixture policy for the library's existing storage fault tests. The
    /// production CityOS interpretation is tested in desigma-cityos, never
    /// linked back into the generic crate.
    pub(crate) struct BoundFixturePolicy;
    pub(crate) const BOUND_FIXTURE_POLICY: BoundFixturePolicy = BoundFixturePolicy;
    impl CheckpointStoragePolicy for BoundFixturePolicy {
        fn submission_policy(&self) -> Option<&dyn crate::SubmissionPolicy> { Some(self) }
        fn check_portable(&self, checkpoint: &DocumentCheckpoint) -> Result<()> {
            if checkpoint_documents(checkpoint).iter().any(|document| document.cityos.as_ref()
                .is_some_and(|binding| matches!(binding.base, CityosBase::NativeOwner { .. }))) {
                return Err(LibraryError::Invalid("Fixture Native export is unavailable".into()));
            }
            Ok(())
        }
        fn retention_roots(&self, checkpoint: &DocumentCheckpoint) -> Result<Vec<String>> {
            Ok(checkpoint.dependency_pins())
        }
    }

    fn managed() -> Document {
        let mut document = Document::new("managed fixture");
        document.cityos = Some(CityosBinding {
            schema_version: "cityos.design-exchange.v1".into(), document_id: "document".into(), draft_id: "draft".into(),
            base: CityosBase::CanonicalGit { repository_id: "fixture".into(), commit: "a".repeat(40) },
            semantic_base: serde_json::json!({"opaque":"not an authority receipt"}), dependency_pins: vec![],
            occurrences: BTreeMap::new(), exchange_v2: None, exchange_v3: None,
        });
        document
    }

    fn history_with_snapshot(document: Document, redo: bool) -> History {
        let command = Command::Batch(vec![Command::Batch(vec![Command::RestoreDocument { document: Box::new(document) }])]);
        serde_json::from_value(if redo { serde_json::json!({"undo":[],"redo":[command]}) }
            else { serde_json::json!({"undo":[command],"redo":[]}) }).unwrap()
    }

    #[test]
    fn generic_defaults_reject_managed_current_archived_and_nested_undo_redo_states() {
        for placement in ["current", "archived", "undo", "redo", "archived-undo", "archived-redo"] {
            let mut checkpoint = DocumentCheckpoint::new(Document::new("ordinary"), History::default());
            match placement {
                "current" => checkpoint.document = managed(),
                "archived" => checkpoint.retained.push(RetainedHistory { document: managed(), history: History::default() }),
                "undo" | "redo" => checkpoint.history = history_with_snapshot(managed(), placement == "redo"),
                _ => checkpoint.retained.push(RetainedHistory { document: Document::new("archive"),
                    history: history_with_snapshot(managed(), placement == "archived-redo") }),
            }
            assert!(UNMANAGED_STORAGE_POLICY.check_portable(&checkpoint).is_err(), "{placement}");
            assert!(UNMANAGED_STORAGE_POLICY.retention_roots(&checkpoint).is_err(), "{placement}");
            let portable = crate::portable::encode_with_policy(&BOUND_FIXTURE_POLICY, &checkpoint, BTreeMap::new()).unwrap();
            assert!(VerifiedPortableCheckpoint::parse(&portable).is_err(), "{placement}");
        }
        let ordinary = DocumentCheckpoint::new(Document::new("ordinary"), History::default());
        assert!(UNMANAGED_STORAGE_POLICY.check_portable(&ordinary).is_ok());
        assert!(UNMANAGED_STORAGE_POLICY.retention_roots(&ordinary).unwrap().is_empty());
    }

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("desigma-storage-policy-{}", crate::new_id()));
            std::fs::create_dir_all(&root).unwrap();
            Self(root)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
    }
    fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        fn visit(root: &Path, directory: &Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for entry in std::fs::read_dir(directory).unwrap() {
                let entry = entry.unwrap();
                if entry.file_type().unwrap().is_dir() { visit(root, &entry.path(), result); }
                else { result.insert(entry.path().strip_prefix(root).unwrap().to_owned(), std::fs::read(entry.path()).unwrap()); }
            }
        }
        let mut result = BTreeMap::new(); visit(root, root, &mut result); result
    }
    struct RevokeAfterParse(std::cell::Cell<u32>);
    impl CheckpointStoragePolicy for RevokeAfterParse {
        fn check_portable(&self, _checkpoint: &DocumentCheckpoint) -> Result<()> {
            let call = self.0.get(); self.0.set(call + 1);
            if call == 0 { Ok(()) } else { Err(LibraryError::Invalid("Fixture destination policy revoked".into())) }
        }
        fn retention_roots(&self, checkpoint: &DocumentCheckpoint) -> Result<Vec<String>> {
            Ok(checkpoint.dependency_pins())
        }
    }

    #[test]
    fn destination_policy_is_rechecked_before_existing_new_or_file_import_can_mutate() {
        let temp = Temp::new();
        let library = Library::open(temp.0.join("library")).unwrap();
        let project = library.create_project("P").unwrap();
        let file = library.create_file(&project.id, None, "F", &Document::new("original")).unwrap();
        let checkpoint = DocumentCheckpoint::new(managed(), History::default());
        let portable = crate::portable::encode_with_policy(&BOUND_FIXTURE_POLICY, &checkpoint, BTreeMap::new()).unwrap();
        let path = temp.0.join("portable.json"); std::fs::write(&path, &portable).unwrap();
        for operation in ["existing", "new", "file"] {
            let policy = RevokeAfterParse(std::cell::Cell::new(0));
            let before = files(library.root());
            let result = match operation {
                "existing" => library.import_checkpoint_with_policy(&policy, &file.id, &portable),
                "new" => {
                    let verified = VerifiedPortableCheckpoint::parse_with_policy(&policy, &portable).unwrap();
                    library.import_checkpoint_as_new_with_policy(&policy, &project.id, None, verified).map(|_| ())
                }
                _ => library.import_file_with_policy(&policy, &path, &project.id, None).map(|_| ()),
            };
            assert!(result.is_err(), "{operation}"); assert_eq!(policy.0.get(), 2, "{operation}");
            assert_eq!(files(library.root()), before, "No placeholder, payload or journal writes: {operation}");
        }
        let before = files(library.root());
        assert!(library.import_checkpoint(&file.id, &portable).is_err());
        let verified = VerifiedPortableCheckpoint::parse_with_policy(&BOUND_FIXTURE_POLICY, &portable).unwrap();
        assert!(library.import_checkpoint_as_new(&project.id, None, verified).is_err());
        assert!(library.import_file(&path, &project.id, None).is_err());
        assert!(VerifiedPortableCheckpoint::read(&path).is_err());
        assert_eq!(files(library.root()), before, "An admitted immutable value carries no destination policy grant");
    }

    struct InvalidRoots;
    impl CheckpointStoragePolicy for InvalidRoots {
        fn check_portable(&self, _checkpoint: &DocumentCheckpoint) -> Result<()> { Ok(()) }
        fn retention_roots(&self, _checkpoint: &DocumentCheckpoint) -> Result<Vec<String>> { Ok(vec!["opaque-invalid-digest".into()]) }
    }
    #[test]
    fn malformed_extension_roots_fail_before_export_import_or_retention_can_succeed() {
        let temp = Temp::new();
        let library = Library::open(temp.0.join("library")).unwrap();
        let project = library.create_project("P").unwrap();
        let checkpoint = DocumentCheckpoint::new(Document::new("ordinary"), History::default());
        let file = library.create_file(&project.id, None, "F", &checkpoint.document).unwrap();
        let portable = crate::portable::encode(&checkpoint, BTreeMap::new()).unwrap();
        let before = files(library.root());
        assert!(library.export_checkpoint_with_policy(&InvalidRoots, &file.id).is_err());
        assert!(VerifiedPortableCheckpoint::parse_with_policy(&InvalidRoots, &portable).is_err());
        assert!(library.import_checkpoint_with_policy(&InvalidRoots, &file.id, &portable).is_err());
        assert!(library.retention_roots_with_policy(&InvalidRoots).is_err());
        assert_eq!(files(library.root()), before);
    }
}
