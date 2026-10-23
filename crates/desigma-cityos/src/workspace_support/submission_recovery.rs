//! Recovery of original, durable owner observations. No receipt import or remote writer.
use crate::editor::EditorHost;
use desigma_library::{SubmissionRecord, SubmissionState};

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
