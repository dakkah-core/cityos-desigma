//! Recovery of original, durable owner observations. No receipt import or remote writer.
use crate::editor::EditorHost;
use desigma_doc::DocumentCheckpoint;
use desigma_library::{SubmissionRecord, SubmissionState, SubmissionSyncIntent, SubmissionSyncReceipt};

/// Native state and storage operations needed to recover an existing intent.
/// The storage owner validates and resumes the journal under its writer lock;
/// the host owns the live document, history and autosave adoption.
pub trait AcceptedRecoveryHost {
    fn freeze_for_recovery(&mut self);
    fn recovery_context(&self) -> Result<(String, Vec<SubmissionRecord>), String>;
    fn recovery_intent(&self, operation_id: &str) -> Result<Option<SubmissionSyncIntent>, String>;
    fn live_matches_checkpoint(&self, checkpoint: &DocumentCheckpoint) -> Result<bool, String>;
    fn resume_recovery(&self, operation_id: &str) -> Result<SubmissionSyncReceipt, String>;
    fn adopt_recovery(&mut self, checkpoint: DocumentCheckpoint) -> Result<(), String>;
    fn refresh_recovery_freeze(&mut self) -> Result<(), String>;
}

/// Resume only the original accepted operation, without rebuilding a candidate.
pub fn recover_accepted<H: AcceptedRecoveryHost>(host: &mut H, record: &SubmissionRecord) -> Result<bool, String> {
    host.freeze_for_recovery();
    let (file_id, records) = host.recovery_context()?;
    if !records.iter().any(|stored| {
        stored.command == record.command && stored.request_digest == record.request_digest && stored.schema_version == record.schema_version
    }) {
        return Err("Accepted-state recovery belongs to another document or original operation".into());
    }
    let Some(intent) = host.recovery_intent(&record.command.operation_id)? else {
        return Ok(false);
    };
    if intent.file_id != file_id || intent.accepted != *record {
        return Err("Accepted-state recovery does not match the original document and operation".into());
    }
    let original = DocumentCheckpoint::from_json(&intent.previous_checkpoint).map_err(|error| error.to_string())?;
    let accepted = DocumentCheckpoint::from_json(&intent.accepted_checkpoint).map_err(|error| error.to_string())?;
    if !host.live_matches_checkpoint(&original)? && !host.live_matches_checkpoint(&accepted)? {
        return Err("Frozen submitted document changed; reload its durable generation".into());
    }
    let receipt = host.resume_recovery(&record.command.operation_id)?;
    let mut synchronized = record.clone();
    synchronized.state = SubmissionState::Synchronized;
    if receipt.sync_id != intent.sync_id
        || receipt.submission != synchronized
        || receipt.checkpoint.to_json().map_err(|error| error.to_string())? != intent.accepted_checkpoint
    {
        return Err("Recovered receipt does not match the original accepted generation".into());
    }
    host.adopt_recovery(receipt.checkpoint)?;
    host.refresh_recovery_freeze()?;
    Ok(true)
}

#[derive(Default)]
pub struct Recovery {
    key: Option<(std::path::PathBuf, String, String)>,
    pub records: Vec<SubmissionRecord>,
    pub error: Option<String>,
    pub requested: Option<String>,
}

impl Recovery {
    pub fn refresh_if_changed<H: EditorHost>(&mut self, app: &mut H) {
        let key = app.submission_key();
        if self.key != key {
            self.reload(app);
        }
    }

    pub fn reload<H: EditorHost>(&mut self, app: &mut H) {
        self.key = app.submission_key();
        self.records.clear();
        self.error = None;
        self.requested = None;
        app.set_submission_frozen(None);
        let Some((_, _, draft_id)) = &self.key else {
            return;
        };
        match app.submissions() {
            Ok(records) => {
                if records.iter().any(SubmissionRecord::frozen) {
                    app.set_submission_frozen(Some(draft_id.clone()));
                }
                self.records = records;
            }
            Err(error) => {
                // Failure to enumerate retained commands must never unlock a draft.
                app.set_submission_frozen(Some(draft_id.clone()));
                self.error = Some(format!("Submission recovery is unavailable; this draft stays frozen: {error}"));
            }
        }
    }

    pub fn accepted(&self) -> Option<&SubmissionRecord> {
        self.records.iter().find(|record| record.state == SubmissionState::AcceptedSyncPending)
    }

    pub fn recover_existing<H: EditorHost>(&mut self, app: &mut H, record: &SubmissionRecord) -> Result<bool, String> {
        if !app.recover_submission(record)? {
            return Ok(false);
        }
        self.reload(app);
        Ok(true)
    }

    pub fn install<H: EditorHost>(&mut self, app: &mut H, record: &SubmissionRecord, document: desigma_doc::Document) -> Result<(), String> {
        app.install_accepted(record, document)?;
        self.reload(app);
        Ok(())
    }
}

pub fn state_label(state: &SubmissionState) -> &'static str {
    match state {
        SubmissionState::Prepared => "Prepared · immutable draft frozen",
        SubmissionState::Pending => "Pending or uncertain · inquire using the original operation",
        SubmissionState::Published => "Published · source acceptance pending",
        SubmissionState::AcceptedSyncPending => "Accepted · synchronization pending",
        SubmissionState::Synchronized => "Synchronized",
        SubmissionState::Rejected => "Conclusively non-accepted",
        SubmissionState::Retired => "Conclusively retired",
    }
}

#[cfg(test)]
mod accepted_tests {
    use super::*;
    use desigma_doc::{CityosBase, Document, History};
    use desigma_library::SubmissionCommand;
    use serde_json::json;
    use std::cell::Cell;
    use std::collections::BTreeMap;

    struct Host {
        record: SubmissionRecord,
        intent: Option<SubmissionSyncIntent>,
        receipt: SubmissionSyncReceipt,
        file: String,
        frozen: bool,
        matching: bool,
        fail: &'static str,
        resumes: Cell<usize>,
        adopted: bool,
        refreshed: bool,
    }

    impl Host {
        fn new() -> Self {
            let record = SubmissionRecord {
                schema_version: "v1".into(),
                command: SubmissionCommand {
                    intent_id: "intent".into(), operation_id: "operation".into(), predecessor: None,
                    owner: "owner".into(), resource: "resource".into(), scope: BTreeMap::new(),
                    base: CityosBase::CanonicalGit { repository_id: "repo".into(), commit: "a".repeat(40) },
                    expected_revision: "revision".into(), semantic_base: json!({}), submitted_snapshot: json!({}), dependency_pins: vec![],
                },
                request_digest: "digest".into(), state: SubmissionState::AcceptedSyncPending, outcome: Some(json!({"receipt":"original"})),
            };
            let previous = DocumentCheckpoint::new(Document::new("original"), History::default());
            let accepted = DocumentCheckpoint::new(Document::new("accepted"), History::default());
            let intent = SubmissionSyncIntent {
                schema_version: "v1".into(), sync_id: "sync".into(), file_id: "file".into(), accepted: record.clone(),
                previous_checkpoint: previous.to_json().unwrap(), accepted_checkpoint: accepted.to_json().unwrap(),
            };
            let mut synchronized = record.clone();
            synchronized.state = SubmissionState::Synchronized;
            Self {
                record, intent: Some(intent), receipt: SubmissionSyncReceipt { sync_id: "sync".into(), submission: synchronized, checkpoint: accepted, recovered_after_error: false },
                file: "file".into(), frozen: false, matching: true, fail: "", resumes: Cell::new(0), adopted: false, refreshed: false,
            }
        }
    }

    impl AcceptedRecoveryHost for Host {
        fn freeze_for_recovery(&mut self) { self.frozen = true; }
        fn recovery_context(&self) -> Result<(String, Vec<SubmissionRecord>), String> {
            assert!(self.frozen);
            if self.fail == "context" { return Err("unreadable".into()); }
            Ok((self.file.clone(), vec![self.record.clone()]))
        }
        fn recovery_intent(&self, _: &str) -> Result<Option<SubmissionSyncIntent>, String> {
            assert!(self.frozen);
            if self.fail == "intent" { return Err("unreadable".into()); }
            Ok(self.intent.clone())
        }
        fn live_matches_checkpoint(&self, _: &DocumentCheckpoint) -> Result<bool, String> {
            if self.fail == "compare" { return Err("unreadable".into()); }
            Ok(self.matching)
        }
        fn resume_recovery(&self, operation: &str) -> Result<SubmissionSyncReceipt, String> {
            assert!(self.frozen);
            assert_eq!(operation, self.record.command.operation_id);
            self.resumes.set(self.resumes.get() + 1);
            if self.fail == "resume" { return Err("uncertain".into()); }
            Ok(self.receipt.clone())
        }
        fn adopt_recovery(&mut self, _: DocumentCheckpoint) -> Result<(), String> {
            assert!(self.frozen);
            if self.fail == "adopt" { return Err("unavailable".into()); }
            self.adopted = true;
            Ok(())
        }
        fn refresh_recovery_freeze(&mut self) -> Result<(), String> {
            assert!(self.frozen && self.adopted);
            if self.fail == "refresh" { return Err("unreadable".into()); }
            self.refreshed = true;
            Ok(())
        }
    }

    #[test]
    fn failures_remain_frozen_and_do_not_adopt_unverified_state() {
        for failure in ["context", "intent", "compare", "resume", "adopt", "refresh"] {
            let mut host = Host::new();
            host.fail = failure;
            let record = host.record.clone();
            assert!(recover_accepted(&mut host, &record).is_err(), "{failure}");
            assert!(host.frozen && !host.refreshed, "{failure}");
            assert_eq!(host.adopted, failure == "refresh", "{failure}");
        }
        let mut host = Host::new();
        host.intent = None;
        let record = host.record.clone();
        assert_eq!(recover_accepted(&mut host, &record), Ok(false));
        assert!(host.frozen && !host.adopted && host.resumes.get() == 0);
    }

    #[test]
    fn rejects_wrong_identity_or_receipt_before_adoption() {
        for mismatch in ["file", "operation", "resource", "outcome", "live", "sync", "receipt", "checkpoint"] {
            let mut host = Host::new();
            match mismatch {
                "file" => host.intent.as_mut().unwrap().file_id = "other".into(),
                "operation" => host.intent.as_mut().unwrap().accepted.command.operation_id = "other".into(),
                "resource" => host.record.command.resource = "other".into(),
                "outcome" => host.intent.as_mut().unwrap().accepted.outcome = Some(json!({"receipt":"other"})),
                "live" => host.matching = false,
                "sync" => host.receipt.sync_id = "other".into(),
                "receipt" => host.receipt.submission.outcome = Some(json!({"receipt":"other"})),
                "checkpoint" => host.receipt.checkpoint.document.name = "other".into(),
                _ => unreachable!(),
            }
            let record = if mismatch == "resource" { host.intent.as_ref().unwrap().accepted.clone() } else { host.record.clone() };
            assert!(recover_accepted(&mut host, &record).is_err(), "{mismatch}");
            assert!(host.frozen && !host.adopted && !host.refreshed, "{mismatch}");
            assert_eq!(host.resumes.get(), usize::from(["sync", "receipt", "checkpoint"].contains(&mismatch)), "{mismatch}");
        }
    }

    #[test]
    fn exact_receipt_is_adopted_before_refresh() {
        let mut host = Host::new();
        let record = host.record.clone();
        assert_eq!(recover_accepted(&mut host, &record), Ok(true));
        assert!(host.adopted && host.refreshed && host.resumes.get() == 1);
    }
}
