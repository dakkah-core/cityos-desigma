//! Recovery of original, durable owner observations. No receipt import or remote writer.
use crate::app::App;
use desigma_doc::DocumentCheckpoint;
use desigma_library::{SubmissionRecord, SubmissionState};

#[derive(Default)]
pub(super) struct Recovery {
    key: Option<(std::path::PathBuf, String, String)>,
    pub records: Vec<SubmissionRecord>,
    pub error: Option<String>,
    pub requested: Option<String>,
}

impl Recovery {
    pub fn refresh_if_changed(&mut self, app: &mut App) {
        let key = app.lib.as_ref().zip(app.lib_file.as_ref()).zip(app.doc.cityos.as_ref())
            .map(|((library, file), binding)| (library.root().to_path_buf(), file.id.clone(), binding.draft_id.clone()));
        if self.key != key { self.reload(app); }
    }

    pub fn reload(&mut self, app: &mut App) {
        self.key = app.lib.as_ref().zip(app.lib_file.as_ref()).zip(app.doc.cityos.as_ref())
            .map(|((library, file), binding)| (library.root().to_path_buf(), file.id.clone(), binding.draft_id.clone()));
        self.records.clear(); self.error = None; self.requested = None;
        app.ui.cityos_frozen_draft = None;
        let Some((_, file_id, draft_id)) = &self.key else { return; };
        match app.lib.as_ref().unwrap().document_submissions(file_id) {
            Ok(records) => {
                if records.iter().any(SubmissionRecord::frozen) { app.ui.cityos_frozen_draft = Some(draft_id.clone()); }
                self.records = records;
            }
            Err(error) => {
                // Failure to enumerate retained commands must never unlock a draft.
                app.ui.cityos_frozen_draft = Some(draft_id.clone());
                self.error = Some(format!("Submission recovery is unavailable; this draft stays frozen: {error}"));
            }
        }
    }

    pub fn accepted(&self) -> Option<&SubmissionRecord> {
        self.records.iter().find(|record| record.state == SubmissionState::AcceptedSyncPending)
    }

    pub fn recover_existing(&mut self, app: &mut App, record: &SubmissionRecord) -> Result<bool, String> {
        let library = app.lib.as_ref().ok_or("Draft library unavailable")?;
        match library.load_submission_sync(&record.command.operation_id) {
            Ok(intent) => {
                let file = app.lib_file.as_ref().ok_or("Draft file unavailable")?;
                if intent.file_id != file.id { return Err("Accepted-state recovery belongs to another document".into()); }
            }
            Err(desigma_library::LibraryError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.to_string()),
        }
        let receipt = library.resume_submission_sync(&record.command.operation_id).map_err(|error| error.to_string())?;
        Self::adopt(app, receipt.checkpoint);
        self.reload(app);
        Ok(true)
    }

    pub fn install(&mut self, app: &mut App, record: &SubmissionRecord, document: desigma_doc::Document) -> Result<(), String> {
        let library = app.lib.as_ref().ok_or("Draft library unavailable")?;
        let file = app.lib_file.as_ref().ok_or("Save the draft before synchronizing")?;
        let stored = library.load_submission(&record.command.operation_id).map_err(|error| error.to_string())?;
        if stored != *record || stored.state != SubmissionState::AcceptedSyncPending { return Err("Original accepted outcome changed; refresh recovery".into()); }
        let mut checkpoint = library.load_checkpoint(&file.id).map_err(|error| error.to_string())?;
        if checkpoint.document.to_json().map_err(|error| error.to_string())? != app.doc.to_json().map_err(|error| error.to_string())? {
            return Err("Frozen submitted document changed; reload its durable generation".into());
        }
        checkpoint.history = app.hist.clone();
        checkpoint.install_baseline(document);
        let receipt = library.synchronize_accepted_submission(&file.id, &stored, &checkpoint).map_err(|error| error.to_string())?;
        Self::adopt(app, receipt.checkpoint);
        self.reload(app);
        Ok(())
    }

    fn adopt(app: &mut App, checkpoint: DocumentCheckpoint) {
        app.doc = checkpoint.document; app.hist = checkpoint.history;
        app.history.reset(); app.changed(); app.sanitize();
        app.autosave.saved_hash = crate::home::doc_hash(&app.doc);
        app.autosave.changed_at = None;
        app.autosave.retry_at = None;
    }
}

pub(super) fn state_label(state: &SubmissionState) -> &'static str {
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
