//! Explicit semantic interpretation for durable submission storage. Policies are
//! local validators, never owner authentication, admission or dispatch authority.
use crate::{CheckpointStoragePolicy, LibraryError, Result, SubmissionCommand, SubmissionRecord};
use desigma_doc::Document;
use std::collections::BTreeMap;

/// Ephemeral facts only: never serialized into a command, journal or digest.
pub struct SubmissionCommandFacts {
    /// The extension selects the scope used to detect an unresolved legacy
    /// effect on the same owner/resource, independently of the current actor.
    pub overlap_scope: BTreeMap<String, String>,
}

pub struct SubmissionDocumentFacts {
    pub document_id: String,
    pub draft_id: String,
    /// Exact submitted base/resource/revision and semantic snapshot match.
    pub matches_submitted: bool,
    /// Legacy unbound journals identify an effect by base and snapshot. Keep
    /// this distinct from admission of a new, file-bound submission.
    pub matches_legacy_effect: bool,
}

pub trait SubmissionPolicy {
    fn validate_command(&self, command: &SubmissionCommand) -> Result<SubmissionCommandFacts>;
    /// None means ordinary unbound content. Invalid or unsupported managed
    /// content must fail, not become an unbound document or a negative match.
    fn inspect_document(&self, command: &SubmissionCommand, document: &Document) -> Result<Option<SubmissionDocumentFacts>>;
    fn validate_accepted(&self, record: &SubmissionRecord, previous: &Document, candidate: &Document) -> Result<()>;
}

pub(crate) fn submission_policy(policy: &dyn CheckpointStoragePolicy) -> Result<&dyn SubmissionPolicy> {
    policy.submission_policy().ok_or_else(|| LibraryError::Invalid("Submission requires its explicit installed extension policy".into()))
}

pub(crate) fn inspect_document(
    policy: &dyn CheckpointStoragePolicy,
    command: &SubmissionCommand,
    document: &Document,
) -> Result<Option<SubmissionDocumentFacts>> {
    let facts = submission_policy(policy)?.inspect_document(command, document)?;
    if facts.as_ref().is_some_and(|facts| facts.document_id.trim().is_empty() || facts.draft_id.trim().is_empty()) || facts.is_none() && document.is_managed() {
        return Err(LibraryError::Invalid("Extension returned an invalid managed submission identity".into()));
    }
    Ok(facts)
}

// Frozen legacy fixture interpretation for the generic journal/fault suite.
// Production CityOS policy is owned and tested by desigma-cityos. This fixture
// is never compiled into the library runtime or used to qualify an owner.
#[cfg(test)]
mod fixture {
    use crate::storage_policy::tests::BoundFixturePolicy;
    use crate::{
        payload_digest, LibraryError, Result, SubmissionCommand, SubmissionCommandFacts, SubmissionDocumentFacts, SubmissionObservation, SubmissionPolicy,
        SubmissionRecord,
    };
    use desigma_doc::{CityosBase, Document};
    use std::collections::BTreeMap;

    fn invalid(message: &str) -> LibraryError {
        LibraryError::Invalid(message.into())
    }

    impl SubmissionPolicy for BoundFixturePolicy {
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
}
