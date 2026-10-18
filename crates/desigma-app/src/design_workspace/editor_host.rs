//! Native ownership and persistence behind the CityOS editor port.
//!
//! The workspace fences in-flight operations and uncertain recovery. This host
//! never gives it mutable document, history, autosave or library references.
use super::{presentation, session::EditorSession};
use crate::app::App;
use desigma_cityos::draft::{draft, revision};
use desigma_cityos::editor::{BaselineInstall, BaselineRecovery, EditorHost, EditorStatus, EditorToast};
use desigma_doc::{Command, Document, DocumentCheckpoint};
use desigma_cityos::history::{CityosHistory, CityosHistoryStep};
use desigma_library::{Library, SubmissionRecord, SubmissionState};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[cfg(test)]
thread_local! {
    static BEFORE_REVIEWED_SAVE: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

fn editable(app: &App) -> Result<(), String> {
    if app.cityos_submission_frozen() {
        return Err("This draft is frozen. Recover its original document or owner outcome before editing.".into());
    }
    if app.history.viewing.is_some() {
        return Err("Return to the current version before changing this draft.".into());
    }
    Ok(())
}

fn next_revision(app: &App, document: &Document) -> Result<(), String> {
    app.doc.to_json().map_err(|error| error.to_string())?;
    document.to_json().map_err(|error| error.to_string())?;
    let previous = revision(&app.doc).ok_or("Current draft is unavailable")?;
    let next = revision(document).ok_or("Incoming draft is unavailable")?;
    if previous.0 != next.0 || previous.1.checked_add(1) != Some(next.1)
        || app.doc.cityos.as_ref().map(|binding| &binding.document_id)
            != document.cityos.as_ref().map(|binding| &binding.document_id)
    {
        return Err("Obsolete document generation discarded".into());
    }
    Ok(())
}

fn command_binding_current(app: &App, document: &Document) -> Result<(), String> {
    next_revision(app, document)?;
    let current = draft(&app.doc).ok_or("Current semantic binding is unavailable")?;
    let next = draft(document).ok_or("Incoming semantic binding is unavailable")?;
    let immutable = |value: &Value| {
        let mut value = value.clone();
        if let Some(object) = value.as_object_mut() {
            for field in ["semanticDraft", "semanticDigest", "draftRevision", "commandReceipts"] { object.remove(field); }
        }
        value
    };
    if immutable(current) != immutable(next) {
        return Err("An edit cannot replace the current immutable semantic baseline".into());
    }
    if current["schemaVersion"] == "cityos.design-exchange.v3" {
        let receipts = next["commandReceipts"].as_array().ok_or("Missing validated command receipt")?;
        let receipt = receipts.last().ok_or("Missing validated command receipt")?;
        let digest = receipt["requestDigest"].as_str().unwrap_or("");
        if receipt["operationId"].as_str().is_none_or(str::is_empty)
            || receipt["revision"] != next["draftRevision"]
            || digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err("Command receipt does not identify this semantic revision".into());
        }
        let mut expected = current["commandReceipts"].as_array().ok_or("Missing current command receipt chain")?.clone();
        expected.push(receipt.clone());
        if expected.len() > 256 { expected.drain(..expected.len() - 256); }
        if *receipts != expected { return Err("A delayed edit cannot replace the current command receipt chain".into()); }
    }
    Ok(())
}

fn identity(file_id: &str, document: &Document) -> Result<BaselineRecovery, String> {
    let value = draft(document).ok_or("Recovery requires a bound draft")?;
    Ok(BaselineRecovery {
        file_id: file_id.to_owned(),
        draft_id: value["draftId"].as_str().filter(|id| !id.is_empty()).ok_or("Missing recovery draft identity")?.into(),
        revision: value["draftRevision"].as_u64().ok_or("Missing recovery revision")?,
        semantic_digest: value["semanticDigest"].as_str().filter(|digest| !digest.is_empty())
            .ok_or("Missing recovery semantic digest")?.into(),
        binding_digest: desigma_library::payload_digest(&serde_json::to_vec(value).map_err(|error| error.to_string())?),
    })
}

fn same_live_state(app: &App, checkpoint: &DocumentCheckpoint) -> Result<bool, String> {
    // Never compare failed serializations as equal, or accept a matching document
    // with a different undo/redo stack as the original generation.
    Ok(checkpoint.document.to_json().map_err(|error| error.to_string())?
        == app.doc.to_json().map_err(|error| error.to_string())?
        && serde_json::to_value(&checkpoint.history).map_err(|error| error.to_string())?
            == serde_json::to_value(&app.hist).map_err(|error| error.to_string())?)
}

fn freeze(app: &mut App) {
    app.ui.cityos_frozen_draft = app.doc.cityos.as_ref().map(|binding| binding.draft_id.clone());
    app.ui.cityos_history_request = None;
}

fn refresh_submission_freeze(app: &mut App) -> Result<(), String> {
    // Read first. An unreadable record must never briefly unlock native inputs.
    match <App as EditorHost>::submissions(app) {
        Ok(records) => {
            app.ui.cityos_frozen_draft = if records.iter().any(SubmissionRecord::frozen) {
                app.doc.cityos.as_ref().map(|binding| binding.draft_id.clone())
            } else { None };
            Ok(())
        }
        Err(error) => { freeze(app); Err(error) }
    }
}

fn adopt(app: &mut App, checkpoint: DocumentCheckpoint) -> Result<(), String> {
    let fingerprint = desigma_library::checkpoint_fingerprint(&checkpoint).map_err(|error| error.to_string())?;
    let mut autosave = crate::home::Autosave::library_checkpoint(app.canvas.tiles.rev(), &checkpoint.document, &checkpoint.history, fingerprint)?;
    app.doc = checkpoint.document;
    app.hist = checkpoint.history;
    app.history.reset();
    app.changed();
    app.sanitize();
    autosave.seen_rev = app.canvas.tiles.rev();
    autosave.saved_hash = crate::home::doc_hash(&app.doc);
    autosave.saves = app.autosave.saves;
    app.autosave = autosave;
    Ok(())
}

fn submission_context(app: &App, record: &SubmissionRecord) -> Result<(Library, String), String> {
    let library = app.lib.clone().ok_or("Draft library unavailable")?;
    let file = app.lib_file.as_ref().ok_or("Draft file unavailable")?;
    let belongs = library.document_submissions(&file.id).map_err(|error| error.to_string())?
        .into_iter().any(|stored| stored.command == record.command && stored.request_digest == record.request_digest
            && stored.schema_version == record.schema_version);
    if !belongs { return Err("Accepted-state recovery belongs to another document or original operation".into()); }
    Ok((library, file.id.clone()))
}

fn draw(app: &mut App, ui: &mut egui::Ui, native_scene: bool, state: &mut Option<presentation::AutoFit>) -> Option<String> {
    let ctx = ui.ctx().clone();
    let rect = ui.available_rect_before_wrap();
    app.canvas_rect = rect;
    if native_scene { presentation::fit_canvas(state, app); }
    else if !app.first_fit_done {
        app.first_fit_done = true;
        app.fit_all();
    }
    crate::canvas::tiles::paint(app, ui, rect);
    // Do not run the ordinary canvas tools: all authored changes enter through
    // install_edit or separately validated history/baseline operations.
    let response = ui.interact(rect, ui.id().with("cityos-native-canvas"), egui::Sense::click_and_drag());
    if response.dragged_by(egui::PointerButton::Middle) {
        let delta = ctx.input(|input| input.pointer.delta());
        app.view.offset.x -= delta.x / app.view.zoom;
        app.view.offset.y -= delta.y / app.view.zoom;
    }
    if response.hovered() {
        let zoom = ctx.input(|input| input.zoom_delta());
        if zoom != 1. {
            let position = ctx.input(|input| input.pointer.hover_pos()).unwrap_or(rect.center());
            app.view.zoom_at(rect.min, position, app.view.zoom * zoom);
        }
    }
    let selected = if response.clicked() {
        response.interact_pointer_pos().and_then(|position| {
            let point = app.view.to_page(rect.min, position);
            app.doc.nodes.values().rev().find(|node| {
                !app.doc.pages.contains(&node.id) && {
                    let transform = app.doc.world_transform(node.id);
                    point.x >= transform.tx && point.y >= transform.ty
                        && point.x <= transform.tx + node.size.x && point.y <= transform.ty + node.size.y
                }
            }).and_then(|node| node.source_id.clone())
        })
    } else { None };
    if let Some(source) = &selected { <App as EditorHost>::select_source(app, source); }
    crate::canvas::draw::overlays(app, ui, rect);
    selected
}

impl EditorHost for App {
    type Session = EditorSession;
    type Canvas = Box<App>;
    type CanvasState = Option<presentation::AutoFit>;

    fn document(&self) -> &Document { &self.doc }

    fn status(&self) -> EditorStatus {
        EditorStatus {
            home: self.home.is_some(),
            library_available: self.lib.is_some(),
            library_file: self.lib_file.is_some(),
            external_path: self.file_path.clone(),
            // Autosave owns the debounced hash probe; status is read repeatedly
            // while drawing and must not serialize an estate-sized draft.
            dirty: self.autosave.changed_at.is_some(),
            save_failures: self.autosave.failures > 0,
            frozen: self.cityos_submission_frozen(),
            can_undo: self.hist.can_undo(),
            can_redo: self.hist.can_redo(),
            history_pending: self.ui.cityos_history_request.is_some(),
            toast: self.toast.as_ref().filter(|toast| self.now < toast.until).map(|toast| EditorToast {
                text: toast.text.clone(), error: toast.error,
                remaining: Duration::from_secs_f64(toast.until - self.now),
            }),
        }
    }

    fn save_before_switch(&mut self) -> Result<(), String> {
        crate::decks::ui::flush_notes(self);
        self.finish_variable_editors();
        if self.home.is_some() { return Ok(()); }
        if self.lib_file.is_some() { return self.save_library_file(); }
        if self.file_path.is_some() { return self.save_current_external(); }
        if self.doc.cityos.is_some() { return <Self as EditorHost>::save_to_draft_library(self); }
        if self.hist.can_undo() { return Err("Save your current design before opening a bound draft.".into()); }
        Ok(())
    }

    fn save_to_draft_library(&mut self) -> Result<(), String> {
        crate::decks::ui::flush_notes(self);
        self.finish_variable_editors();
        if self.lib_file.is_some() { return self.save_library_file(); }
        let library = self.lib.clone().ok_or("The library is unavailable. Save this draft before switching.")?;
        let project = library.list_projects().into_iter().find(|project| project.name == "CityOS drafts")
            .map(Ok).unwrap_or_else(|| library.create_project("CityOS drafts")).map_err(|error| error.to_string())?;
        let file = library.create_file_with_history(&project.id, None, &self.doc.name, &self.doc, &self.hist)
            .map_err(|error| error.to_string())?;
        let checkpoint = library.load_checkpoint(&file.id).map_err(|error| error.to_string())?;
        let autosave = crate::home::Autosave::created_library_checkpoint(self.canvas.tiles.rev(), &self.doc, &self.hist, checkpoint)?;
        self.lib_file = Some(crate::home::LibFile {
            id: file.id, project_id: project.id, project: project.name, name: file.name,
        });
        self.file_path = None;
        self.autosave = autosave;
        Ok(())
    }

    fn replace_session(&mut self, document: Document, path: Option<PathBuf>) -> Result<Self::Session, String> {
        if self.ui.cityos_history_request.is_some() { return Err("Finish the pending CityOS history operation before switching drafts.".into()); }
        // The controller prepared the switch. The native transaction still
        // performs its final fallible flush before transferring either owner.
        EditorSession::replace(self, document, path).ok_or_else(|| "Current draft could not be saved; open cancelled".into())
    }

    fn swap_session(&mut self, incoming: &mut Option<Self::Session>) -> Result<Self::Session, String> {
        if incoming.is_none() { return Err("The incoming editor session is unavailable".into()); }
        if self.ui.cityos_history_request.is_some() { return Err("Finish the pending CityOS history operation before switching drafts.".into()); }
        EditorSession::swap(self, incoming).ok_or_else(|| "Current draft could not be saved; tab switch cancelled".into())
    }

    fn enter_document(&mut self) { self.home = None; }

    fn open_files(&mut self) -> Result<(), String> {
        if self.home.is_some() { return Ok(()); }
        if self.lib.is_none() { return Err("The Desigma file library is unavailable. Save this draft before leaving.".into()); }
        if self.ui.cityos_history_request.is_some() { return Err("Finish the pending CityOS history operation before leaving.".into()); }
        <Self as EditorHost>::save_before_switch(self)?;
        self.go_home();
        if self.home.is_none() { return Err("Could not open Desigma files. The current draft is still open.".into()); }
        Ok(())
    }

    fn save(&mut self) { App::save(self, false); }
    fn undo(&mut self) { App::undo(self); }
    fn redo(&mut self) { App::redo(self); }
    fn open_dialog(&mut self) { App::open_dialog(self); }

    fn export_portable(&mut self) -> Result<(), String> {
        <Self as EditorHost>::save_before_switch(self)?;
        let library = self.lib.as_ref().ok_or("Draft library unavailable")?;
        let file = self.lib_file.as_ref().ok_or("Save this draft to the library before exporting")?;
        let payload = library.export_checkpoint(&file.id).map_err(|error| error.to_string())?;
        if let Some(path) = rfd::FileDialog::new().add_filter("Portable Desigma checkpoint", &["json"])
            .set_file_name(format!("{}.portable.json", self.doc.name)).save_file()
        {
            desigma_library::write_atomic(&path, payload.as_bytes()).map_err(|error| error.to_string())?;
            self.show_toast("Portable draft exported with verified dependencies and retained history", false);
        }
        Ok(())
    }

    fn autosave_tick(&mut self, ctx: &egui::Context) { App::autosave_tick(self, ctx); }
    fn flush_autosave(&mut self) -> bool { App::flush_autosave(self) }
    fn toast(&mut self, text: &str, error: bool) { self.show_toast(text, error); }

    fn select_source(&mut self, source: &str) {
        if let Some(id) = self.doc.nodes.values().find(|node| node.source_id.as_deref() == Some(source)).map(|node| node.id) {
            self.select_only(id);
        }
    }

    fn install_presentation(&mut self, document: Document) -> Result<(), String> {
        presentation::install(self, document)
    }

    fn install_edit(&mut self, document: Document) -> Result<(), String> {
        editable(self)?;
        if self.ui.cityos_history_request.is_some() { return Err("A CityOS history operation is pending".into()); }
        command_binding_current(self, &document)?;
        self.hist.exec(&mut self.doc, Command::CityosInstall { document: Box::new(document) })
            .map_err(|error| error.to_string())?;
        self.changed();
        self.sanitize();
        Ok(())
    }

    fn take_history_request(&mut self) -> Option<CityosHistoryStep> { self.ui.cityos_history_request.take() }

    fn history_step_current(&self, step: &CityosHistoryStep) -> bool {
        !self.cityos_submission_frozen() && self.hist.cityos_step_current(&self.doc, step)
    }

    fn install_history(&mut self, step: CityosHistoryStep, document: Document) -> Result<(), String> {
        editable(self)?;
        if !self.hist.cityos_step_current(&self.doc, &step) {
            return Err("Obsolete CityOS history operation discarded".into());
        }
        command_binding_current(self, &document)?;
        // Only the projection and validated binding change. Preserve local
        // annotations, variables, styles, images and the user's metadata.
        let mut current = serde_json::to_value(&self.doc).map_err(|error| error.to_string())?;
        let projected = serde_json::to_value(document).map_err(|error| error.to_string())?;
        for field in ["nodes", "pages", "next_id", "cityos"] { current[field] = projected[field].clone(); }
        let document: Document = serde_json::from_value(current).map_err(|error| error.to_string())?;
        document.to_json().map_err(|error| error.to_string())?;
        self.hist.commit_cityos(&mut self.doc, step, document).map_err(|error| error.to_string())?;
        self.changed();
        self.sanitize();
        Ok(())
    }

    fn install_reviewed_baseline(&mut self, mut document: Document) -> Result<BaselineInstall, String> {
        editable(self)?;
        if self.ui.cityos_history_request.is_some() { return Err("Finish the pending history operation before applying incoming changes".into()); }
        document.name = self.doc.name.clone();
        next_revision(self, &document)?;
        let library = self.lib.clone().ok_or("The draft library is unavailable")?;
        let file = self.lib_file.as_ref().ok_or("Save this draft to the library before applying incoming changes")?;
        let recovery = identity(&file.id, &document)?;
        let fingerprint = self.autosave.saved_checkpoint_fingerprint.as_deref().ok_or("This library file has no observed checkpoint; reopen it before applying incoming changes")?;
        let mut checkpoint = library.load_checkpoint(&file.id).map_err(|error| error.to_string())?;
        if desigma_library::checkpoint_fingerprint(&checkpoint).map_err(|error| error.to_string())? != fingerprint {
            return Err("The durable checkpoint changed; current draft retained. Review/reconcile it before applying incoming changes".into());
        }
        if checkpoint.history.baseline_id() != self.hist.baseline_id() {
            return Err("The durable baseline changed; recover it before applying incoming changes".into());
        }
        checkpoint.document = self.doc.clone();
        checkpoint.history = self.hist.clone();
        let original = checkpoint.to_json().map_err(|error| error.to_string())?;
        checkpoint.install_baseline(document);
        let expected = checkpoint.to_json().map_err(|error| error.to_string())?;
        #[cfg(test)]
        if let Some(hook) = BEFORE_REVIEWED_SAVE.with(|slot| slot.borrow_mut().take()) { hook(); }
        let saved = library.save_checkpoint_reconciled_expected(&file.id, &checkpoint, fingerprint);
        if let Err(error) = saved {
            // The library's locked expected-generation check rejected before
            // writing. This authoritative non-write is not an uncertain install.
            if matches!(&error, desigma_library::LibraryError::CheckpointConflict(_) | desigma_library::LibraryError::StorageBusy) {
                return Err(format!("Incoming changes were not installed; current draft retained: {error}"));
            }
            // A journal may have committed despite the reported I/O failure.
            // Both document AND history must be known before retrying a save.
            match library.load_checkpoint_for_recovery(&file.id) {
                Ok((recovered, missing)) if missing.is_empty() => {
                    let saved = recovered.to_json().map_err(|error| error.to_string());
                    if saved.as_ref().is_ok_and(|saved| saved == &expected) {
                        adopt(self, recovered)?;
                        return Ok(BaselineInstall::Installed);
                    }
                    if saved.as_ref().is_ok_and(|saved| saved == &original) {
                        return Err(format!("Incoming changes were not installed; current draft retained: {error}"));
                    }
                }
                _ => {}
            }
            freeze(self);
            return Ok(BaselineInstall::Uncertain {
                recovery,
                message: "Document generation outcome is uncertain. Recover it before editing, saving or leaving.".into(),
            });
        }
        // The reconciled save holds the library writer lock through exact
        // readback. Install that receipt, never the unverified input candidate.
        adopt(self, saved.map_err(|error| error.to_string())?.checkpoint)?;
        Ok(BaselineInstall::Installed)
    }

    fn recover_reviewed_baseline(&mut self, recovery: &BaselineRecovery) -> Result<bool, String> {
        // Reject a recovery token from another active file/draft before reading
        // or installing anything. The live original remains owned while frozen.
        let file = self.lib_file.as_ref().ok_or("Draft file unavailable")?;
        if file.id != recovery.file_id { return Err("Recovery belongs to another draft file".into()); }
        let original = identity(&file.id, &self.doc)?;
        if original.draft_id != recovery.draft_id || original.revision.checked_add(1) != Some(recovery.revision) {
            return Err("Recovery identity no longer matches the active original draft".into());
        }
        let library = self.lib.as_ref().ok_or("Draft library unavailable")?;
        let (checkpoint, missing) = library.load_checkpoint_for_recovery(&recovery.file_id).map_err(|error| error.to_string())?;
        if !missing.is_empty() { return Err("Recovery dependencies remain unavailable".into()); }
        if identity(&recovery.file_id, &checkpoint.document)? == *recovery {
            // A reviewed baseline must retain the exact original state and seal
            // its stacks; matching revision/digest alone cannot resurrect undo.
            let previous = checkpoint.retained.last().ok_or("Recovered baseline has no original history archive")?;
            let original = DocumentCheckpoint::new(previous.document.clone(), previous.history.clone());
            if !same_live_state(self, &original)? || checkpoint.history.can_undo() || checkpoint.history.can_redo()
                || checkpoint.history.baseline_id().is_none() || checkpoint.history.baseline_id() == self.hist.baseline_id()
            {
                return Err("Recovered baseline does not retain and seal the exact original history".into());
            }
            refresh_submission_freeze(self)?;
            adopt(self, checkpoint)?;
            return Ok(true);
        }
        if !same_live_state(self, &checkpoint)? {
            return Err("Recovered generation differs from both known states; keep the draft frozen for inspection".into());
        }
        refresh_submission_freeze(self)?;
        Ok(false)
    }

    fn dependencies_complete(&self, document: &Document, validate_history: bool) -> Result<bool, String> {
        let library = self.lib.as_ref().ok_or("The draft library is unavailable; dependencies cannot be retained")?;
        if !desigma_cityos::dependencies::complete(library, document) { return Ok(false); }
        if validate_history { <Self as EditorHost>::validate_history_dependencies(self)?; }
        Ok(true)
    }

    fn validate_history_dependencies(&self) -> Result<(), String> {
        let library = self.lib.as_ref().ok_or("The draft library is unavailable")?;
        if let Some(file) = &self.lib_file {
            library.load_checkpoint(&file.id).map_err(|error| format!(
                "Retained history dependencies are unavailable; restore a portable checkpoint before editing: {error}"
            ))?;
        }
        for pin in DocumentCheckpoint::new(self.doc.clone(), self.hist.clone()).dependency_pins() {
            library.load_cityos_payload(&pin).map_err(|error| format!("Active history dependencies are unavailable: {error}"))?;
        }
        Ok(())
    }

    fn retain_dependencies(&self, cache: &Path, draft: &Value, response: &Value) -> Result<(), String> {
        let library = self.lib.as_ref().ok_or("The draft library is unavailable; dependencies cannot be retained")?;
        desigma_cityos::dependencies::retain(cache, library, draft, response)
    }

    fn submission_key(&self) -> Option<(PathBuf, String, String)> {
        self.lib.as_ref().zip(self.lib_file.as_ref()).zip(self.doc.cityos.as_ref())
            .map(|((library, file), binding)| (library.root().to_path_buf(), file.id.clone(), binding.draft_id.clone()))
    }

    fn submissions(&self) -> Result<Vec<SubmissionRecord>, String> {
        let library = self.lib.as_ref().ok_or("Draft library unavailable")?;
        let file = self.lib_file.as_ref().ok_or("Draft file unavailable")?;
        library.document_submissions(&file.id).map_err(|error| error.to_string())
    }

    fn set_submission_frozen(&mut self, draft_id: Option<String>) { self.ui.cityos_frozen_draft = draft_id; }

    fn recover_submission(&mut self, record: &SubmissionRecord) -> Result<bool, String> {
        freeze(self);
        let (library, file_id) = submission_context(self, record)?;
        let intent = match library.load_submission_sync(&record.command.operation_id) {
            Ok(intent) => intent,
            Err(desigma_library::LibraryError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.to_string()),
        };
        if intent.file_id != file_id || intent.accepted != *record {
            return Err("Accepted-state recovery does not match the original document and operation".into());
        }
        let original = DocumentCheckpoint::from_json(&intent.previous_checkpoint).map_err(|error| error.to_string())?;
        let accepted = DocumentCheckpoint::from_json(&intent.accepted_checkpoint).map_err(|error| error.to_string())?;
        if !same_live_state(self, &original)? && !same_live_state(self, &accepted)? {
            return Err("Frozen submitted document changed; reload its durable generation".into());
        }
        let receipt = library.resume_submission_sync(&record.command.operation_id).map_err(|error| error.to_string())?;
        adopt(self, receipt.checkpoint)?;
        refresh_submission_freeze(self)?;
        Ok(true)
    }

    fn install_accepted(&mut self, record: &SubmissionRecord, mut document: Document) -> Result<(), String> {
        freeze(self);
        document.name = self.doc.name.clone();
        next_revision(self, &document)?;
        let (library, file_id) = submission_context(self, record)?;
        let stored = library.load_submission(&record.command.operation_id).map_err(|error| error.to_string())?;
        if stored != *record || stored.state != SubmissionState::AcceptedSyncPending {
            return Err("Original accepted outcome changed; refresh recovery".into());
        }
        let mut checkpoint = library.load_checkpoint(&file_id).map_err(|error| error.to_string())?;
        if checkpoint.document.to_json().map_err(|error| error.to_string())? != self.doc.to_json().map_err(|error| error.to_string())? {
            return Err("Frozen submitted document changed; reload its durable generation".into());
        }
        checkpoint.history = self.hist.clone();
        checkpoint.install_baseline(document);
        let receipt = library.synchronize_accepted_submission(&file_id, &stored, &checkpoint).map_err(|error| error.to_string())?;
        adopt(self, receipt.checkpoint)?;
        refresh_submission_freeze(self)?;
        Ok(())
    }

    fn temporary_canvas(document: Document) -> Self::Canvas { Box::new(App::new(document)) }
    fn select_temporary_source(canvas: &mut Self::Canvas, source: &str) {
        <App as EditorHost>::select_source(canvas.as_mut(), source);
    }

    fn draw_canvas(&mut self, ui: &mut egui::Ui, native_scene: bool, state: &mut Self::CanvasState) -> Option<String> {
        draw(self, ui, native_scene, state)
    }

    fn draw_temporary_canvas(canvas: &mut Self::Canvas, ui: &mut egui::Ui, native_scene: bool, state: &mut Self::CanvasState) -> Option<String> {
        draw(canvas.as_mut(), ui, native_scene, state)
    }

    fn record_widget(ui: &egui::Ui, id: &str, rect: egui::Rect) { crate::widgets::record(ui, id, rect); }
    fn workspace_tools(ui: &mut egui::Ui) { crate::cityos_catalog::Browser::atlas_workspace_menu(ui); }
    fn workspace_tools_feedback(ui: &mut egui::Ui) { crate::cityos_catalog::Browser::atlas_workspace_feedback(ui); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use desigma_doc::{CityosBase, CityosBinding, Node, Rect};
    use serde_json::json;

    fn document() -> Document {
        let semantic = json!({"id":"fixture","root":{"id":"material","props":{"raised":false}}});
        let mut document = Document::new("Native draft");
        let page = document.add_page("Canvas");
        let id = document.alloc_id();
        let mut node = Node::frame(id, Rect::new(0., 0., 560., 720.));
        node.source_id = Some("material".into());
        node.locked = true;
        document.insert(page, node).unwrap();
        document.cityos = Some(CityosBinding {
            schema_version: "cityos.design-exchange.v3".into(), document_id: "document".into(), draft_id: "draft".into(),
            base: CityosBase::CanonicalGit { repository_id: "fixture".into(), commit: "a".repeat(40) },
            semantic_base: semantic.clone(), dependency_pins: vec![], occurrences: Default::default(), exchange_v2: None,
            exchange_v3: Some(json!({"schemaVersion":"cityos.design-exchange.v3","semanticContract":"fixture",
                "documentId":"document","draftId":"draft","draftRevision":0,"semanticDigest":"original",
                "profileRef":"fixture-profile","base":{"kind":"canonical-git","repositoryId":"fixture","commit":"a".repeat(40)},
                "semanticBase":semantic,"semanticDraft":semantic,"dependencyPins":[],"commandReceipts":[]})),
        });
        document
    }

    fn command(document: &Document, raised: bool, operation: &str) -> Document {
        let mut document = document.clone();
        let value = document.cityos.as_mut().unwrap().exchange_mut().unwrap();
        let revision = value["draftRevision"].as_u64().unwrap() + 1;
        value["semanticDraft"]["root"]["props"]["raised"] = json!(raised);
        value["semanticDigest"] = json!(operation);
        value["draftRevision"] = json!(revision);
        value["commandReceipts"].as_array_mut().unwrap().push(json!({
            "operationId":operation,"requestDigest":"b".repeat(64),"revision":revision,
        }));
        document
    }

    fn reviewed(document: &Document) -> Document {
        let mut document = command(document, true, "reviewed");
        let binding = document.cityos.as_mut().unwrap();
        binding.base = CityosBase::CanonicalGit { repository_id: "fixture".into(), commit: "c".repeat(40) };
        binding.semantic_base = binding.exchange().unwrap()["semanticDraft"].clone();
        let semantic = binding.semantic_base.clone();
        let value = binding.exchange_mut().unwrap();
        value["base"]["commit"] = json!("c".repeat(40));
        value["semanticBase"] = semantic;
        document
    }

    struct Temp(PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            crate::home::thumb::finish_library_jobs(&self.0);
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn saved_app() -> (Temp, App) {
        let root = Temp(std::env::temp_dir().join(format!("desigma-editor-host-{}", uuid::Uuid::new_v4())));
        let library = Library::open(&root.0).unwrap();
        let project = library.create_project("Native host").unwrap();
        let mut app = App::new(document());
        let file = library.create_file_with_history(&project.id, None, &app.doc.name, &app.doc, &app.hist).unwrap();
        let checkpoint = library.load_checkpoint(&file.id).unwrap();
        let fingerprint = desigma_library::checkpoint_fingerprint(&checkpoint).unwrap();
        app.autosave = crate::home::Autosave::library_checkpoint(app.canvas.tiles.rev(), &app.doc, &app.hist, fingerprint).unwrap();
        app.lib = Some(library);
        app.lib_file = Some(crate::home::LibFile { id: file.id, project_id: project.id, project: project.name, name: file.name });
        (root, app)
    }

    fn active_state(app: &App) -> (String, Value, u64, Option<f64>) {
        (app.doc.to_json().unwrap(), serde_json::to_value(&app.hist).unwrap(), app.autosave.saved_hash, app.autosave.changed_at)
    }

    #[test]
    fn stale_reviewed_baseline_retains_dirty_candidate_without_freezing_or_adopting_latest() {
        let (_root, mut app) = saved_app();
        let library = app.lib.clone().unwrap();
        let file = app.lib_file.as_ref().unwrap().id.clone();
        let fingerprint = app.autosave.saved_checkpoint_fingerprint.clone().unwrap();
        let local = command(&app.doc, true, "local");
        <App as EditorHost>::install_edit(&mut app, local).unwrap();
        let next = reviewed(&app.doc);
        let mut other = library.load_checkpoint(&file).unwrap();
        other.document.name = "Newer writer".into();
        let newer = library.save_file_with_history_expected(&file, &other.document, &other.history, &fingerprint).unwrap();
        let before = active_state(&app);
        assert!(<App as EditorHost>::install_reviewed_baseline(&mut app, next).is_err());
        assert_eq!(active_state(&app), before);
        assert!(!app.cityos_submission_frozen());
        assert_eq!(app.autosave.saved_checkpoint_fingerprint.as_deref(), Some(fingerprint.as_str()));
        assert_eq!(desigma_library::checkpoint_fingerprint(&library.load_checkpoint(&file).unwrap()).unwrap(), newer);
    }

    #[test]
    fn storage_contention_after_review_read_is_a_nonwrite_and_keeps_the_draft_editable() {
        let (_root, mut app) = saved_app();
        let library = app.lib.clone().unwrap();
        let file = app.lib_file.as_ref().unwrap().id.clone();
        let persisted = library.load_checkpoint(&file).unwrap().to_json().unwrap();
        let local = command(&app.doc, true, "local");
        <App as EditorHost>::install_edit(&mut app, local).unwrap();
        let next = reviewed(&app.doc);
        let state = active_state(&app);
        let observation = app.autosave.saved_checkpoint_fingerprint.clone();
        let held = std::sync::Arc::new(std::sync::Mutex::new(None));
        let held_by_hook = held.clone();
        BEFORE_REVIEWED_SAVE.with(|slot| *slot.borrow_mut() = Some(Box::new(move || {
            let lock = std::fs::OpenOptions::new().read(true).write(true).open(library.path(".storage.lock")).unwrap();
            lock.try_lock().unwrap();
            *held_by_hook.lock().unwrap() = Some(lock);
        })));
        let outcome = <App as EditorHost>::install_reviewed_baseline(&mut app, next);
        held.lock().unwrap().take();
        assert!(outcome.is_err(), "storage contention must remain retryable");
        assert_eq!(active_state(&app), state);
        assert_eq!(app.autosave.saved_checkpoint_fingerprint, observation);
        assert!(!app.cityos_submission_frozen(), "no durable install was attempted");
        assert_eq!(app.lib.as_ref().unwrap().load_checkpoint(&file).unwrap().to_json().unwrap(), persisted);
    }

    #[test]
    fn installed_reviewed_checkpoint_updates_observation_and_does_not_autosave_its_projection() {
        let (_root, mut app) = saved_app();
        let next = reviewed(&app.doc);
        assert!(matches!(<App as EditorHost>::install_reviewed_baseline(&mut app, next).unwrap(), BaselineInstall::Installed));
        let library = app.lib.clone().unwrap();
        let file = app.lib_file.as_ref().unwrap().id.clone();
        let checkpoint = library.load_checkpoint(&file).unwrap();
        let fingerprint = desigma_library::checkpoint_fingerprint(&checkpoint).unwrap();
        assert_eq!(app.autosave.saved_checkpoint_fingerprint.as_deref(), Some(fingerprint.as_str()));
        assert_eq!(app.autosave.saved_history_hash, Some(crate::home::Autosave::history_hash(&checkpoint.history).unwrap()));
        assert!(app.flush_autosave());
        assert_eq!(desigma_library::checkpoint_fingerprint(&library.load_checkpoint(&file).unwrap()).unwrap(), fingerprint);
    }

    #[test]
    fn native_edits_reject_frozen_pending_obsolete_and_substituted_bindings_without_mutating_state() {
        for fault in ["frozen", "pending-history", "revision", "document", "profile", "receipt", "chain"] {
            let mut app = App::new(document());
            let first = command(&app.doc, true, "first");
            <App as EditorHost>::install_edit(&mut app, first).unwrap();
            let mut next = command(&app.doc, false, "second");
            match fault {
                "frozen" => freeze(&mut app),
                "pending-history" => app.ui.cityos_history_request = app.hist.prepare_cityos(&app.doc, false).unwrap(),
                "revision" => next.cityos.as_mut().unwrap().exchange_mut().unwrap()["draftRevision"] = json!(7),
                "document" => {
                    let binding = next.cityos.as_mut().unwrap();
                    binding.document_id = "other".into();
                    binding.exchange_mut().unwrap()["documentId"] = json!("other");
                }
                "profile" => next.cityos.as_mut().unwrap().exchange_mut().unwrap()["profileRef"] = json!("other"),
                "receipt" => next.cityos.as_mut().unwrap().exchange_mut().unwrap()["commandReceipts"][1]["revision"] = json!(1),
                _ => { next.cityos.as_mut().unwrap().exchange_mut().unwrap()["commandReceipts"].as_array_mut().unwrap().remove(0); }
            }
            let before = active_state(&app);
            assert!(<App as EditorHost>::install_edit(&mut app, next).is_err(), "{fault}");
            assert_eq!(active_state(&app), before, "{fault}");
        }
    }


    #[test]
    fn failed_final_save_preserves_active_state_and_the_owned_incoming_session() {
        let (root, mut app) = saved_app();
        let edited = command(&app.doc, true, "edit");
        <App as EditorHost>::install_edit(&mut app, edited).unwrap();
        let mut other = App::new(document());
        other.doc.name = "Incoming tab".into();
        let mut incoming = Some(<App as EditorHost>::replace_session(&mut other, document(), None).unwrap());
        let file = app.lib_file.clone().unwrap();
        let manifest = root.0.join("projects").join(&file.project_id).join("project.json");
        let original_manifest = std::fs::read(&manifest).unwrap();
        crate::home::thumb::finish_library_jobs(&root.0);
        std::fs::write(&manifest, b"{damaged manifest").unwrap();
        let before = active_state(&app);
        assert!(<App as EditorHost>::swap_session(&mut app, &mut incoming).is_err());
        assert_eq!(active_state(&app), before);
        assert_eq!(app.lib_file.as_ref().unwrap(), &file);
        assert!(app.hist.can_undo());
        assert!(app.autosave.pending());
        assert_eq!(desigma_cityos::editor::EditorSession::name(incoming.as_ref().unwrap()), "Incoming tab");
        assert!(<App as EditorHost>::replace_session(&mut app, document(), None).is_err());
        assert_eq!(active_state(&app), before);
        std::fs::write(manifest, original_manifest).unwrap();
    }

    #[test]
    fn recovery_checks_file_draft_revision_digest_and_original_archive_before_installing() {
        let (_root, mut app) = saved_app();
        let next = reviewed(&app.doc);
        let file = app.lib_file.as_ref().unwrap().id.clone();
        let expected = identity(&file, &next).unwrap();
        let mut substituted = next.clone();
        let binding = substituted.cityos.as_mut().unwrap();
        let desigma_doc::CityosBase::CanonicalGit { commit, .. } = &mut binding.base else { unreachable!() };
        *commit = "b".repeat(40);
        binding.exchange_mut().unwrap()["base"]["commit"] = serde_json::json!("b".repeat(40));
        let substituted_identity = identity(&file, &substituted).unwrap();
        assert_eq!(substituted_identity.semantic_digest, expected.semantic_digest);
        assert_ne!(substituted_identity, expected, "Recovery must pin the base even when authored semantics match");
        let before = active_state(&app);
        freeze(&mut app);
        assert!(!<App as EditorHost>::recover_reviewed_baseline(&mut app, &expected).unwrap());
        assert_eq!(active_state(&app), before);
        let mut checkpoint = app.lib.as_ref().unwrap().load_checkpoint(&file).unwrap();
        checkpoint.install_baseline(next);
        app.lib.as_ref().unwrap().save_checkpoint(&file, &checkpoint).unwrap();
        freeze(&mut app);
        let before = active_state(&app);
        for fault in ["file", "draft", "revision", "digest", "binding"] {
            let mut wrong = expected.clone();
            match fault {
                "file" => wrong.file_id = "another-file".into(),
                "draft" => wrong.draft_id = "another-draft".into(),
                "revision" => wrong.revision += 1,
                "digest" => wrong.semantic_digest = "another-digest".into(),
                _ => wrong.binding_digest = "another-binding".into(),
            }
            assert!(<App as EditorHost>::recover_reviewed_baseline(&mut app, &wrong).is_err(), "{fault}");
            assert_eq!(active_state(&app), before, "{fault}");
            assert!(app.cityos_submission_frozen());
        }
        assert!(<App as EditorHost>::recover_reviewed_baseline(&mut app, &expected).unwrap());
        assert_eq!(app.doc.to_json().unwrap(), checkpoint.document.to_json().unwrap());
        assert!(!app.cityos_submission_frozen());
        assert!(!app.hist.can_undo());
    }

}
