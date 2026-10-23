//! CityOS semantic checks over the library's explicit submission policy port.
//! This interprets preserved wire contracts; it grants no owner write authority.
use crate::storage_policy::CityosStoragePolicy;
use desigma_doc::{CityosBase, Document};
use desigma_library::{
    payload_digest, LibraryError, Result, SubmissionCommand, SubmissionCommandFacts, SubmissionDocumentFacts, SubmissionObservation, SubmissionPolicy,
    SubmissionRecord,
};
use std::collections::BTreeMap;

fn invalid(message: &str) -> LibraryError {
    LibraryError::Invalid(message.into())
}

impl SubmissionPolicy for CityosStoragePolicy {
    fn validate_command(&self, command: &SubmissionCommand) -> Result<SubmissionCommandFacts> {
        if ["tenantId", "actorId"].iter().any(|key| command.scope.get(*key).is_none_or(|value| value.trim().is_empty())) {
            return Err(invalid("Owner, resource, expected revision and verified scope are required"));
        }
        match &command.base {
            CityosBase::CanonicalGit { repository_id, commit }
                if repository_id.is_empty() || commit.len() != 40 || !commit.bytes().all(|byte| byte.is_ascii_hexdigit()) =>
            {
                return Err(invalid("Canonical committed base required"))
            }
            CityosBase::NativeOwner { owner, resource, revision } if owner != &command.owner || resource != &command.resource || revision.is_empty() => {
                return Err(invalid("Native base owner/resource mismatch"))
            }
            _ => {}
        }
        Ok(SubmissionCommandFacts { overlap_scope: BTreeMap::from([("tenantId".into(), command.scope["tenantId"].clone())]) })
    }

    fn inspect_document(&self, command: &SubmissionCommand, document: &Document) -> Result<Option<SubmissionDocumentFacts>> {
        let Some(binding) = &document.cityos else {
            if document.is_managed() {
                return Err(invalid("Unsupported managed submission binding"));
            }
            return Ok(None);
        };
        binding.validate().map_err(LibraryError::Invalid)?;
        let matches_legacy_effect = binding.base == command.base
            && binding.semantic_base == command.semantic_base
            && binding.exchange().is_some_and(|exchange| exchange["semanticDraft"] == command.submitted_snapshot);
        let matches_submitted = matches_legacy_effect
            && binding.exchange().is_some_and(|exchange| {
                exchange["semanticDraft"]["id"].as_str() == Some(command.resource.as_str())
                    && exchange["base"]["sourceDigest"].as_str() == Some(command.expected_revision.as_str())
            });
        Ok(Some(SubmissionDocumentFacts {
            document_id: binding.document_id.clone(),
            draft_id: binding.draft_id.clone(),
            matches_submitted,
            matches_legacy_effect,
        }))
    }

    fn validate_accepted(&self, record: &SubmissionRecord, previous: &Document, candidate: &Document) -> Result<()> {
        self.validate_command(&record.command)?;
        let receipt: SubmissionObservation = serde_json::from_value(record.outcome.clone().ok_or_else(|| invalid("Accepted receipt required"))?)?;
        let before = previous.cityos.as_ref().ok_or_else(|| invalid("Original CityOS binding required"))?;
        let after = candidate.cityos.as_ref().ok_or_else(|| invalid("Accepted CityOS binding required"))?;
        before.validate().map_err(LibraryError::Invalid)?;
        after.validate().map_err(LibraryError::Invalid)?;
        // Native expanded bindings retain desigma-doc's existing qualification
        // gate. Moving this check must not enable a new submission capability.
        let old = before.exchange().ok_or_else(|| invalid("Expanded original exchange required for synchronization"))?;
        let new = after.exchange().ok_or_else(|| invalid("Expanded accepted exchange required for synchronization"))?;
        if !self.inspect_document(&record.command, previous)?.is_some_and(|facts| facts.matches_submitted)
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
        // Preserve the complete accepted resource, including compatible owner
        // changes, and its digest in the ORIGINAL durable observation.
        let readback_digest = payload_digest(&serde_json::to_vec(&after.semantic_base)?);
        let pinned_readback = receipt.details.get("acceptedSemanticDigest").and_then(|value| value.as_str());
        if pinned_readback.is_some_and(|digest| digest != readback_digest)
            || (after.semantic_base != record.command.submitted_snapshot && pinned_readback != Some(readback_digest.as_str()))
        {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage_policy::CITYOS_STORAGE_POLICY;
    use desigma_doc::{CityosBinding, DocumentCheckpoint, History};
    use desigma_library::{Library, SubmissionState};
    use serde_json::json;

    fn fixture() -> (SubmissionRecord, Document, Document) {
        let command = SubmissionCommand {
            intent_id: "intent".into(),
            operation_id: "operation".into(),
            predecessor: None,
            owner: "experience".into(),
            resource: "frame".into(),
            scope: BTreeMap::from([("tenantId".into(), "tenant".into()), ("actorId".into(), "actor".into())]),
            base: CityosBase::CanonicalGit { repository_id: "repo".into(), commit: "a".repeat(40) },
            expected_revision: "d".repeat(64),
            semantic_base: json!({"id":"frame", "elevation":"flat"}),
            submitted_snapshot: json!({"id":"frame", "elevation":"resting"}),
            dependency_pins: vec![],
        };
        let mut previous = Document::new("submitted projection");
        previous.cityos = Some(CityosBinding {
            schema_version: "cityos.design-exchange.v3".into(),
            document_id: "document".into(),
            draft_id: "draft".into(),
            base: command.base.clone(),
            semantic_base: command.semantic_base.clone(),
            dependency_pins: vec![],
            occurrences: BTreeMap::new(),
            exchange_v2: None,
            exchange_v3: Some(json!({
                "schemaVersion":"cityos.design-exchange.v3", "documentId":"document", "draftId":"draft", "draftRevision":1,
                "semanticContract":"fixture", "profileRef":"profile", "commandReceipts":[], "dependencyPins":[],
                "base":{"kind":"canonical-git","repositoryId":"repo","commit":"a".repeat(40),"sourcePath":"config/frame.json","sourceDigest":command.expected_revision},
                "semanticBase":command.semantic_base, "semanticDraft":command.submitted_snapshot,
            })),
        });
        let mut candidate = previous.clone();
        let binding = candidate.cityos.as_mut().unwrap();
        binding.base = CityosBase::CanonicalGit { repository_id: "repo".into(), commit: "b".repeat(40) };
        binding.semantic_base = command.submitted_snapshot.clone();
        let exchange = binding.exchange_mut().unwrap();
        exchange["base"]["commit"] = json!("b".repeat(40));
        exchange["base"]["sourceDigest"] = json!("e".repeat(64));
        exchange["semanticBase"] = command.submitted_snapshot.clone();
        let digest = payload_digest(&serde_json::to_vec(&command).unwrap());
        let observation = SubmissionObservation {
            operation_id: command.operation_id.clone(),
            request_digest: digest.clone(),
            owner: command.owner.clone(),
            resource: command.resource.clone(),
            scope: command.scope.clone(),
            state: SubmissionState::AcceptedSyncPending,
            receipt_ref: "original-owner-receipt".into(),
            owner_revision: Some("b".repeat(40)),
            conclusively_ineligible: false,
            details: json!({}),
        };
        (
            SubmissionRecord {
                schema_version: "cityos.submission.v1".into(),
                command,
                request_digest: digest,
                state: SubmissionState::AcceptedSyncPending,
                outcome: Some(serde_json::to_value(observation).unwrap()),
            },
            previous,
            candidate,
        )
    }

    #[test]
    fn command_scope_and_base_checks_preserve_actor_independent_legacy_overlap() {
        let (record, _, _) = fixture();
        let original = CITYOS_STORAGE_POLICY.validate_command(&record.command).unwrap().overlap_scope;
        let mut command = record.command.clone();
        command.scope.insert("actorId".into(), "second-actor".into());
        assert_eq!(CITYOS_STORAGE_POLICY.validate_command(&command).unwrap().overlap_scope, original);
        command.scope.insert("tenantId".into(), "second-tenant".into());
        assert_ne!(CITYOS_STORAGE_POLICY.validate_command(&command).unwrap().overlap_scope, original);
        command.scope.remove("actorId");
        assert!(CITYOS_STORAGE_POLICY.validate_command(&command).is_err());
        command = record.command.clone();
        command.base = CityosBase::CanonicalGit { repository_id: "repo".into(), commit: "invalid".into() };
        assert!(CITYOS_STORAGE_POLICY.validate_command(&command).is_err());
        command.base = CityosBase::NativeOwner { owner: "different-owner".into(), resource: "frame".into(), revision: "1".into() };
        assert!(CITYOS_STORAGE_POLICY.validate_command(&command).is_err());
    }

    #[test]
    fn exact_submission_and_legacy_alias_matching_remain_distinct_and_unknown_bindings_fail() {
        let (record, previous, _) = fixture();
        let facts = CITYOS_STORAGE_POLICY.inspect_document(&record.command, &previous).unwrap().unwrap();
        assert!(facts.matches_submitted && facts.matches_legacy_effect);
        for field in ["revision", "resource"] {
            let mut command = record.command.clone();
            if field == "revision" {
                command.expected_revision = "stale".into();
            } else {
                command.resource = "different".into();
            }
            let facts = CITYOS_STORAGE_POLICY.inspect_document(&command, &previous).unwrap().unwrap();
            assert!(!facts.matches_submitted);
            assert!(facts.matches_legacy_effect, "Unbound legacy effects still freeze semantic aliases");
        }
        let mut invalid = previous;
        invalid.cityos.as_mut().unwrap().schema_version = "unknown".into();
        assert!(CITYOS_STORAGE_POLICY.inspect_document(&record.command, &invalid).is_err());
        assert!(CITYOS_STORAGE_POLICY.inspect_document(&record.command, &Document::new("ordinary")).unwrap().is_none());
    }

    #[test]
    fn accepted_policy_preserves_original_identity_and_requires_full_readback_digest() {
        let (mut record, previous, candidate) = fixture();
        let command = serde_json::to_vec(&record.command).unwrap();
        CITYOS_STORAGE_POLICY.validate_accepted(&record, &previous, &candidate).unwrap();
        for field in ["profileRef", "semanticContract", "sourcePath", "commit", "semanticDraft"] {
            let mut wrong = candidate.clone();
            let exchange = wrong.cityos.as_mut().unwrap().exchange_mut().unwrap();
            match field {
                "sourcePath" | "commit" => exchange["base"][field] = json!("different"),
                "semanticDraft" => exchange[field]["elevation"] = json!("raised"),
                _ => exchange[field] = json!("different"),
            }
            assert!(CITYOS_STORAGE_POLICY.validate_accepted(&record, &previous, &wrong).is_err(), "{field}");
        }
        let mut complete = candidate;
        let binding = complete.cityos.as_mut().unwrap();
        binding.semantic_base["concurrentOwnerField"] = json!("preserved");
        let semantics = binding.semantic_base.clone();
        let exchange = binding.exchange_mut().unwrap();
        exchange["semanticBase"] = semantics.clone();
        exchange["semanticDraft"] = semantics.clone();
        assert!(CITYOS_STORAGE_POLICY.validate_accepted(&record, &previous, &complete).is_err());
        record.outcome.as_mut().unwrap()["details"]["acceptedSemanticDigest"] = json!(payload_digest(&serde_json::to_vec(&semantics).unwrap()));
        CITYOS_STORAGE_POLICY.validate_accepted(&record, &previous, &complete).unwrap();
        assert_eq!(serde_json::to_vec(&record.command).unwrap(), command);
        assert_eq!(complete.cityos.unwrap().exchange().unwrap()["semanticBase"], semantics);
    }

    #[test]
    fn explicit_policy_reopens_original_journal_without_dependencies_and_cannot_bypass_frozen_export() {
        let root = std::env::temp_dir().join(format!("desigma-cityos-submission-policy-{}", desigma_library::new_id()));
        let library = Library::open(&root).unwrap();
        let project = library.create_project("Fixture").unwrap();
        let (mut record, document, _) = fixture();
        let pin = library.retain_cityos_payload(b"original retained dependency").unwrap();
        record.command.dependency_pins.push(pin.clone());
        let file = library.create_file_checkpoint(&project.id, None, "Draft", &DocumentCheckpoint::new(document, History::default())).unwrap();
        let original = library.prepare_submission_for_file_with_policy(&CITYOS_STORAGE_POLICY, &file.id, record.command).unwrap();
        let path = library.path("cityos/submissions/operation.json");
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(library.path("cityos/payloads").join(pin)).unwrap();
        let reopened = Library::open(&root).unwrap();
        assert_eq!(reopened.load_submission_with_policy(&CITYOS_STORAGE_POLICY, "operation").unwrap(), original);
        assert!(reopened.load_submission("operation").is_err());
        assert!(reopened.export_checkpoint_with_policy(&CITYOS_STORAGE_POLICY, &file.id).is_err());
        let mut dispatches = 0;
        assert!(reopened
            .dispatch_prepared_for_file_with_policy(&CITYOS_STORAGE_POLICY, &file.id, original.command.clone(), |_| {
                dispatches += 1;
                Ok(())
            })
            .is_err());
        assert_eq!(dispatches, 0);
        assert_eq!(std::fs::read(path).unwrap(), bytes);
        drop(reopened);
        drop(library);
        std::fs::remove_dir_all(root).unwrap();
    }
}
