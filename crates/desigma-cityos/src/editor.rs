//! The editor owns native state; CityOS owns semantic workflow and presentation.
//!
//! No mutable editor, history, autosave or library reference crosses this port.
use desigma_doc::{CityosHistoryStep, Document};
use desigma_library::SubmissionRecord;
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Clone, Default)]
pub struct EditorStatus {
    pub home: bool,
    pub library_available: bool,
    pub library_file: bool,
    pub external_path: Option<PathBuf>,
    pub dirty: bool,
    pub save_failures: bool,
    pub frozen: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    pub history_pending: bool,
    pub toast: Option<EditorToast>,
}

#[derive(Clone)]
pub struct EditorToast {
    pub text: String,
    pub error: bool,
    pub remaining: std::time::Duration,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BaselineRecovery {
    pub file_id: String,
    pub draft_id: String,
    pub revision: u64,
    pub semantic_digest: String,
    /// Pins the full exchange payload, including its canonical base and dependencies.
    pub binding_digest: String,
}

pub enum BaselineInstall {
    Installed,
    /// The host may have committed. Read back this exact identity before retrying.
    Uncertain { recovery: BaselineRecovery, message: String },
}

pub trait EditorSession {
    fn name(&self) -> &str;
    /// Diagnostic identity, without granting storage access.
    fn file_id(&self) -> Option<&str>;
}

pub trait EditorHost {
    type Session: EditorSession;
    type Canvas;
    type CanvasState: Default;

    fn document(&self) -> &Document;
    fn status(&self) -> EditorStatus;
    fn save_before_switch(&mut self) -> Result<(), String>;
    fn save_to_draft_library(&mut self) -> Result<(), String>;
    /// Transfer ownership only after the final fallible outgoing save succeeds.
    fn replace_session(&mut self, document: Document, path: Option<PathBuf>) -> Result<Self::Session, String>;
    /// On failure, both the active state and the incoming slot remain owned.
    fn swap_session(&mut self, incoming: &mut Option<Self::Session>) -> Result<Self::Session, String>;
    fn enter_document(&mut self);
    fn open_files(&mut self) -> Result<(), String>;
    fn save(&mut self);
    fn undo(&mut self);
    fn redo(&mut self);
    fn open_dialog(&mut self);
    fn export_portable(&mut self) -> Result<(), String>;
    fn autosave_tick(&mut self, ctx: &egui::Context);
    fn flush_autosave(&mut self) -> bool;
    fn toast(&mut self, text: &str, error: bool);
    fn select_source(&mut self, source: &str);

    // These operations deliberately have different dirty/history semantics.
    fn install_presentation(&mut self, document: Document) -> Result<(), String>;
    fn install_edit(&mut self, document: Document) -> Result<(), String>;
    fn take_history_request(&mut self) -> Option<CityosHistoryStep>;
    fn history_step_current(&self, step: &CityosHistoryStep) -> bool;
    fn install_history(&mut self, step: CityosHistoryStep, document: Document) -> Result<(), String>;
    fn install_reviewed_baseline(&mut self, document: Document) -> Result<BaselineInstall, String>;
    /// true means the expected generation was recovered; false means the old one survived.
    fn recover_reviewed_baseline(&mut self, recovery: &BaselineRecovery) -> Result<bool, String>;

    fn dependencies_complete(&self, document: &Document, validate_history: bool) -> Result<bool, String>;
    fn validate_history_dependencies(&self) -> Result<(), String>;
    fn retain_dependencies(&self, cache: &Path, draft: &Value, response: &Value) -> Result<(), String>;

    fn submission_key(&self) -> Option<(PathBuf, String, String)>;
    fn submissions(&self) -> Result<Vec<SubmissionRecord>, String>;
    fn set_submission_frozen(&mut self, draft_id: Option<String>);
    fn recover_submission(&mut self, record: &SubmissionRecord) -> Result<bool, String>;
    fn install_accepted(&mut self, record: &SubmissionRecord, document: Document) -> Result<(), String>;

    fn temporary_canvas(document: Document) -> Self::Canvas;
    fn select_temporary_source(canvas: &mut Self::Canvas, source: &str);
    /// Inspection, selection, pan and zoom only. Authored mutations use install_edit.
    fn draw_canvas(&mut self, ui: &mut egui::Ui, native_scene: bool, state: &mut Self::CanvasState) -> Option<String>;
    fn draw_temporary_canvas(canvas: &mut Self::Canvas, ui: &mut egui::Ui, native_scene: bool, state: &mut Self::CanvasState) -> Option<String>;
    fn record_widget(ui: &egui::Ui, id: &str, rect: egui::Rect);
    fn workspace_tools(ui: &mut egui::Ui);
    fn workspace_tools_feedback(ui: &mut egui::Ui);
}
