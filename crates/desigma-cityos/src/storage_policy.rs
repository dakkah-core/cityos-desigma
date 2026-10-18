//! CityOS interpretation for the library's explicit, generic storage-policy port.
//! This is local portability/retention policy, never owner acceptance or a writer.
use desigma_doc::{CityosBase, DocumentCheckpoint};
use desigma_library::{checkpoint_documents, CheckpointStoragePolicy, LibraryError, Result};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug)]
pub struct CityosStoragePolicy;
pub const CITYOS_STORAGE_POLICY: CityosStoragePolicy = CityosStoragePolicy;

impl CheckpointStoragePolicy for CityosStoragePolicy {
    fn submission_policy(&self) -> Option<&dyn desigma_library::SubmissionPolicy> { Some(self) }
    fn check_portable(&self, checkpoint: &DocumentCheckpoint) -> Result<()> {
        for document in checkpoint_documents(checkpoint) {
            if let Some(binding) = &document.cityos {
                binding.validate().map_err(LibraryError::Invalid)?;
                if matches!(binding.base, CityosBase::NativeOwner { .. }) {
                    return Err(LibraryError::Invalid("Native owner portable retention/export policy is not qualified".into()));
                }
            }
        }
        self.retention_roots(checkpoint)?;
        Ok(())
    }
    fn retention_roots(&self, checkpoint: &DocumentCheckpoint) -> Result<Vec<String>> {
        let mut roots = BTreeSet::new();
        for document in checkpoint_documents(checkpoint) {
            if let Some(binding) = &document.cityos {
                binding.validate().map_err(LibraryError::Invalid)?;
                roots.extend(binding.dependency_pins.iter().cloned());
                // Retain both native and exchange pins. A stale wrapper cannot
                // drop dependencies held only by its expanded semantic payload.
                if let Some(exchange) = binding.exchange() {
                    let pins = exchange.get("dependencyPins").and_then(|pins| pins.as_array())
                        .ok_or_else(|| LibraryError::Invalid("Expanded CityOS dependency pins are missing or invalid".into()))?;
                    for pin in pins {
                        let digest = pin["digest"].as_str().ok_or_else(|| LibraryError::Invalid("CityOS dependency pin has no digest".into()))?;
                        roots.insert(digest.into());
                    }
                }
            }
        }
        if roots.iter().any(|root: &String| root.len() != 64 || !root.bytes().all(|byte| byte.is_ascii_hexdigit())) {
            return Err(LibraryError::Invalid("Invalid CityOS retained dependency digest".into()));
        }
        Ok(roots.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use desigma_doc::{CityosBinding, Command, Document, History, RetainedHistory};
    use desigma_library::{Library, UNMANAGED_STORAGE_POLICY, VerifiedPortableCheckpoint};
    use serde_json::{json, Value};
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn legacy(pin: &str, native: bool) -> Document {
        let mut document = Document::new("storage fixture");
        document.cityos = Some(CityosBinding {
            schema_version: "cityos.design-exchange.v1".into(), document_id: "document".into(), draft_id: "draft".into(),
            base: if native { CityosBase::NativeOwner { owner: "fixture-owner".into(), resource: "fixture".into(), revision: "1".into() } }
                else { CityosBase::CanonicalGit { repository_id: "fixture".into(), commit: "a".repeat(40) } },
            semantic_base: json!({"id":"fixture","opaque":{"dependencyPins":"not a declared dependency field"}}),
            dependency_pins: vec![pin.into()], occurrences: BTreeMap::new(), exchange_v2: None, exchange_v3: None,
        });
        document
    }
    fn expanded(native_pin: &str, semantic_pin: &str) -> Document {
        let mut document = legacy(native_pin, false);
        let binding = document.cityos.as_mut().unwrap();
        binding.schema_version = "cityos.design-exchange.v3".into();
        binding.exchange_v3 = Some(json!({
            "schemaVersion":"cityos.design-exchange.v3", "documentId":"document", "draftId":"draft", "draftRevision":0,
            "base":{"kind":"canonical-git","repositoryId":"fixture","commit":"a".repeat(40)},
            "semanticContract":"fixture", "semanticBase":binding.semantic_base, "semanticDraft":binding.semantic_base,
            "dependencyPins":[{"digest":semantic_pin}], "commandReceipts":[],
        }));
        document
    }
    fn nested_history_wire(document: Document, redo: bool) -> Value {
        let command = Command::Batch(vec![Command::Batch(vec![Command::CityosInstall { document: Box::new(document) }])]);
        if redo { json!({"undo":[],"redo":[command]}) } else { json!({"undo":[command],"redo":[]}) }
    }
    fn nested_history(document: Document, redo: bool) -> History {
        serde_json::from_value(nested_history_wire(document, redo)).unwrap()
    }
    fn assert_malformed_history_denied(document: Document, expected: &str) {
        assert!(document.to_json().is_err(), "Invalid bindings cannot be serialized into native history");
        // Construct malformed persisted bytes without asking the validating
        // native serializer to manufacture an impossible in-memory snapshot.
        let mut envelope: Value = serde_json::from_str(&expanded(&"a".repeat(64), &"b".repeat(64)).to_json().unwrap()).unwrap();
        let binding = document.cityos.as_ref().unwrap();
        envelope["schemaVersion"] = json!(if binding.exchange_v3.is_some() { "desigma.document.v4" }
            else if binding.exchange_v2.is_some() { "desigma.document.v3" } else { "desigma.document.v2" });
        envelope["document"]["cityos"] = serde_json::to_value(document.cityos).unwrap();
        let wire = json!({"undo":[],"redo":[{"Batch":[{"Batch":[{"CityosInstall":{"document":envelope}}]}]}]});
        let error = serde_json::from_value::<History>(wire).expect_err("Malformed nested redo must fail before retention scanning");
        assert!(error.to_string().contains(expected), "{error}");
    }
    fn placed(document: Document, placement: &str) -> DocumentCheckpoint {
        let mut checkpoint = DocumentCheckpoint::new(Document::new("ordinary current"), History::default());
        match placement {
            "current" => checkpoint.document = document,
            "archived" => checkpoint.retained.push(RetainedHistory { document, history: History::default() }),
            "undo" | "redo" => checkpoint.history = nested_history(document, placement == "redo"),
            _ => checkpoint.retained.push(RetainedHistory { document: Document::new("archive"),
                history: nested_history(document, placement == "archived-redo") }),
        }
        checkpoint
    }

    #[test]
    fn cityos_retention_unions_native_and_semantic_pins_in_current_archive_nested_undo_and_redo() {
        let hashes: Vec<_> = ['a','b','c','d','e','f','0'].iter().map(|letter| letter.to_string().repeat(64)).collect();
        let mut checkpoint = DocumentCheckpoint::new(expanded(&hashes[0], &hashes[1]), nested_history(legacy(&hashes[2], false), false));
        let redo = nested_history(legacy(&hashes[3], false), true);
        let mut wire = serde_json::to_value(&checkpoint.history).unwrap();
        wire["redo"] = serde_json::to_value(redo).unwrap()["redo"].clone();
        checkpoint.history = serde_json::from_value(wire).unwrap();
        checkpoint.retained.push(RetainedHistory { document: expanded(&hashes[4], &hashes[5]),
            history: nested_history(legacy(&hashes[6], false), true) });
        let before = checkpoint.to_json().unwrap();
        let mut expected = hashes; expected.sort();
        assert_eq!(CITYOS_STORAGE_POLICY.retention_roots(&checkpoint).unwrap(), expected);
        assert!(CITYOS_STORAGE_POLICY.check_portable(&checkpoint).is_ok());
        assert_eq!(checkpoint.to_json().unwrap(), before, "Storage policy cannot edit or qualify semantic state");
    }

    #[test]
    fn native_snapshots_keep_roots_while_export_stays_denied_at_every_archived_position() {
        let pin = "a".repeat(64);
        for placement in ["current", "archived", "undo", "redo", "archived-undo", "archived-redo"] {
            let checkpoint = placed(legacy(&pin, true), placement);
            let before = checkpoint.to_json().unwrap();
            assert!(CITYOS_STORAGE_POLICY.check_portable(&checkpoint).is_err(), "{placement}");
            assert_eq!(CITYOS_STORAGE_POLICY.retention_roots(&checkpoint).unwrap(), [pin.clone()], "{placement}");
            assert!(UNMANAGED_STORAGE_POLICY.check_portable(&checkpoint).is_err(), "{placement}");
            assert!(UNMANAGED_STORAGE_POLICY.retention_roots(&checkpoint).is_err(), "{placement}");
            assert_eq!(checkpoint.to_json().unwrap(), before);
        }
    }

    #[test]
    fn missing_invalid_or_opaque_expanded_dependency_declarations_never_become_empty_roots() {
        let pin = "a".repeat(64);
        for declaration in [None, Some(Value::Null), Some(json!(true)), Some(json!("opaque")), Some(json!({"digest":pin})),
            Some(json!([{}])), Some(json!([{"digest":null}])), Some(json!([{"digest":5}])), Some(json!([{"digest":"invalid"}]))] {
            let mut document = expanded(&pin, &pin);
            let exchange = document.cityos.as_mut().unwrap().exchange_mut().unwrap();
            match declaration {
                Some(value) => { exchange["dependencyPins"] = value; }
                None => { exchange.as_object_mut().unwrap().remove("dependencyPins"); }
            }
            for placement in ["current", "archived", "redo"] {
                let checkpoint = placed(document.clone(), placement);
                assert!(CITYOS_STORAGE_POLICY.retention_roots(&checkpoint).is_err(), "{placement}");
                assert!(CITYOS_STORAGE_POLICY.check_portable(&checkpoint).is_err(), "{placement}");
            }
        }
        let mut document = expanded(&pin, &pin);
        document.cityos.as_mut().unwrap().schema_version = "future.exchange".into();
        assert!(CITYOS_STORAGE_POLICY.retention_roots(&placed(document.clone(), "archived")).is_err());
        // Persisted history rejects an unknown binding before policy can inspect
        // it. An impossible in-memory history is not a valid retention fixture.
        assert_malformed_history_denied(document, "Unsupported or incomplete CityOS binding");
    }

    #[test]
    fn only_actual_bound_exchange_pins_are_interpreted_and_invalid_wrapper_roots_fail_closed() {
        let native_pin = "a".repeat(64); let semantic_pin = "b".repeat(64);
        let document = expanded(&native_pin, &semantic_pin);
        let checkpoint = placed(document.clone(), "current");
        // The helper's opaque semantic data contains a similarly named invalid
        // declaration. It is preserved rather than interpreted as an envelope.
        assert_eq!(CITYOS_STORAGE_POLICY.retention_roots(&checkpoint).unwrap(), [native_pin, semantic_pin]);
        assert!(CITYOS_STORAGE_POLICY.check_portable(&checkpoint).is_ok());
        let mut invalid = document;
        invalid.cityos.as_mut().unwrap().dependency_pins.push("opaque-invalid-root".into());
        assert!(CITYOS_STORAGE_POLICY.retention_roots(&placed(invalid, "undo")).is_err());
        let mut invalid = legacy(&"c".repeat(64), false);
        invalid.cityos.as_mut().unwrap().exchange_v2 = Some(json!({"dependencyPins":"opaque"}));
        assert!(CITYOS_STORAGE_POLICY.retention_roots(&placed(invalid.clone(), "current")).is_err());
        assert_malformed_history_denied(invalid, "Expanded payload cannot be stored in a legacy binding");
    }

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("desigma-cityos-policy-{}", desigma_library::new_id()));
            std::fs::create_dir_all(&path).unwrap(); Self(path)
        }
    }
    impl Drop for Temp { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }

    #[test]
    fn installed_policy_preserves_native_library_retention_without_enabling_portable_export() {
        let temp = Temp::new();
        let library = Library::open(temp.0.join("library")).unwrap();
        let project = library.create_project("P").unwrap();
        let pin = library.payloads("cityos").unwrap().retain(b"Native fixture retained without export qualification").unwrap();
        for placement in ["current", "archived", "undo", "redo", "archived-redo"] {
            let file = library.create_file(&project.id, None, placement, &Document::new("ordinary")).unwrap();
            let checkpoint = placed(legacy(&pin, true), placement);
            library.save_checkpoint(&file.id, &checkpoint).unwrap();
            let before = library.load_checkpoint(&file.id).unwrap().to_json().unwrap();
            assert!(library.export_checkpoint_with_policy(&CITYOS_STORAGE_POLICY, &file.id).is_err(), "{placement}");
            assert!(library.export_checkpoint(&file.id).is_err(), "{placement}");
            assert_eq!(library.load_checkpoint(&file.id).unwrap().to_json().unwrap(), before);
            assert_eq!(library.retention_roots_with_policy(&CITYOS_STORAGE_POLICY).unwrap(), [pin.clone()]);
        }
        assert!(library.cityos_retention_roots().is_err(), "No installed interpretation is an unavailable retention scan");
    }

    #[test]
    fn installed_policy_roundtrip_keeps_wire_format_and_requires_explicit_destination_policy() {
        let temp = Temp::new();
        let source = Library::open(temp.0.join("source")).unwrap();
        let project = source.create_project("Source").unwrap();
        let native_pin = source.payloads("cityos").unwrap().retain(b"native wrapper dependency").unwrap();
        let semantic_pin = source.payloads("cityos").unwrap().retain(b"actual bound semantic dependency").unwrap();
        let checkpoint = placed(expanded(&native_pin, &semantic_pin), "archived-redo");
        let file = source.create_file_checkpoint(&project.id, None, "Canonical", &checkpoint).unwrap();
        let portable = source.export_checkpoint_with_policy(&CITYOS_STORAGE_POLICY, &file.id).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&portable).unwrap()["schemaVersion"], "desigma.portable-checkpoint.v1");
        assert!(VerifiedPortableCheckpoint::parse(&portable).is_err());
        let admitted = VerifiedPortableCheckpoint::parse_with_policy(&CITYOS_STORAGE_POLICY, &portable).unwrap();
        let target = Library::open(temp.0.join("target")).unwrap();
        let target_project = target.create_project("Target").unwrap();
        assert!(target.import_checkpoint_as_new(&target_project.id, None, admitted).is_err());
        assert!(target.all_files().is_empty());
        let admitted = VerifiedPortableCheckpoint::parse_with_policy(&CITYOS_STORAGE_POLICY, &portable).unwrap();
        let imported = target.import_checkpoint_as_new_with_policy(&CITYOS_STORAGE_POLICY, &target_project.id, None, admitted).unwrap();
        assert_eq!(target.load_checkpoint(&imported.id).unwrap().to_json().unwrap(), checkpoint.to_json().unwrap());
        assert_eq!(target.export_checkpoint_with_policy(&CITYOS_STORAGE_POLICY, &imported.id).unwrap(), portable);
    }
}
