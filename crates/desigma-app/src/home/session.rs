//! The editor's link to the library: which library file is open, saving,
//! debounced autosave and thumbnails.

use super::{thumb, Home, Section};
use crate::app::App;
use desigma_doc::{Document, DocumentKind, History};
use desigma_library::Library;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Seconds without a document change before a library file is autosaved.
pub const AUTOSAVE_IDLE: f64 = 1.5;
/// Seconds of calm after which a revision change is compared with the saved document.
const PROBE_DELAY: f64 = 0.3;

/// The library file currently open in the editor.
#[derive(Clone, Debug, PartialEq)]
pub struct LibFile {
    pub id: String,
    pub project_id: String,
    pub project: String,
    pub name: String,
}

#[derive(Default)]
pub struct Autosave {
    /// Last canvas revision seen (a change bumps it).
    pub seen_rev: u64,
    /// When the document last changed, while a save is pending.
    pub changed_at: Option<f64>,
    /// The pending change was checked against `saved_hash` and really differs (view-only
    /// changes such as zooming bump the revision too, and must not show "Saving").
    pub confirmed: bool,
    /// Hash of the document as last written to (or read from) the library.
    pub saved_hash: u64,
    /// Serialized history identity from a successfully loaded/persisted checkpoint.
    /// External document-only files never establish durable undo/redo history.
    pub saved_history_hash: Option<u64>,
    pub seen_history: Option<Arc<()>>,
    /// Cache the expensive comparison only while this process-local fence holds.
    pub checked_history: Option<(Arc<()>, u64)>,
    /// Exact checkpoint observed before any editor-only name/projection changes.
    pub saved_checkpoint_fingerprint: Option<String>,
    /// Exact external bytes observed at load or returned by a successful write.
    pub external_fingerprint: Option<String>,
    /// Number of autosaves performed (tests).
    pub saves: u32,
    /// Automatic retries back off without clearing the unsaved revision.
    pub failures: u32,
    pub retry_at: Option<f64>,
}

impl Autosave {
    pub(crate) fn history_hash(history: &History) -> Result<u64, String> {
        struct Sink(std::collections::hash_map::DefaultHasher);
        impl std::io::Write for Sink {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> { self.0.write(bytes); Ok(bytes.len()) }
            fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
        }
        let mut sink = Sink(Default::default());
        serde_json::to_writer(&mut sink, history).map_err(|error| format!("Cannot compare undo history: {error}"))?;
        Ok(sink.0.finish())
    }

    pub(crate) fn library_checkpoint(rev: u64, doc: &Document, history: &History, fingerprint: String) -> Result<Self, String> {
        let hash = Self::history_hash(history)?;
        let identity = history.change_identity();
        Ok(Self { seen_rev: rev, saved_hash: doc_hash(doc), saved_history_hash: Some(hash),
            seen_history: Some(identity.clone()), checked_history: Some((identity, hash)),
            saved_checkpoint_fingerprint: Some(fingerprint), ..Default::default() })
    }

    pub(crate) fn created_library_checkpoint(rev: u64, doc: &Document, history: &History, mut checkpoint: desigma_doc::DocumentCheckpoint) -> Result<Self, String> {
        let fingerprint = desigma_library::checkpoint_fingerprint(&checkpoint).map_err(|error| error.to_string())?;
        // A fresh load may observe a competing writer. Never attach that writer's
        // expectation to our older candidate; only entry-name normalization is local.
        checkpoint.document.name = doc.name.clone();
        if doc_hash(&checkpoint.document) != doc_hash(doc) || Self::history_hash(&checkpoint.history)? != Self::history_hash(history)? {
            return Err("The newly created checkpoint changed before it could be observed; reopen/reconcile it".into());
        }
        Self::library_checkpoint(rev, doc, history, fingerprint)
    }

    fn history_changed(&mut self, history: &History) -> Result<bool, String> {
        let identity = history.change_identity();
        let hash = match &self.checked_history {
            Some((checked, hash)) if Arc::ptr_eq(checked, &identity) => *hash,
            _ => {
                let hash = Self::history_hash(history)?;
                self.checked_history = Some((identity, hash));
                hash
            }
        };
        Ok(self.saved_history_hash != Some(hash))
    }

    /// An edit is waiting for the idle delay before it is saved.
    pub fn pending(&self) -> bool {
        self.changed_at.is_some() && self.confirmed
    }
}

pub(crate) fn doc_hash(doc: &Document) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    // Expanded bound canvases are derived views. Their geometry, language and
    // occurrence allocation must not manufacture a saved semantic revision.
    // Keep every document-owned field and the complete exchange payload in the
    // dirty comparison; ordinary designs and local theme libraries stay native.
    if doc.cityos.as_ref().is_some_and(|binding| binding.exchange().is_some() && binding.validate().is_ok()) {
        let mut value = serde_json::to_value(doc).unwrap_or_default();
        if let Some(object) = value.as_object_mut() {
            for key in ["nodes", "pages", "next_id"] { object.remove(key); }
        }
        if let Some(binding) = value["cityos"].as_object_mut() { binding.remove("occurrences"); }
        value.to_string().hash(&mut h);
    } else {
        doc.to_json().unwrap_or_default().hash(&mut h);
    }
    h.finish()
}

impl App {
    pub(crate) fn persist_library_checkpoint(&self, doc: &Document, history: &History) -> Result<Autosave, String> {
        let (Some(lib), Some(file)) = (&self.lib, &self.lib_file) else { return Err("This file is not in the library".into()); };
        let expected = self.autosave.saved_checkpoint_fingerprint.as_deref().ok_or("This library file has no observed checkpoint; reopen it before saving")?;
        let mut saved = Autosave::library_checkpoint(self.canvas.tiles.rev(), doc, history, expected.to_owned())?;
        saved.saved_checkpoint_fingerprint = Some(lib.save_file_with_history_expected(&file.id, doc, history, expected).map_err(|error| format!("Cannot save: {error}"))?);
        saved.saves = self.autosave.saves;
        Ok(saved)
    }

    fn autosave_target_changed(&mut self) -> Result<bool, String> {
        if doc_hash(&self.doc) != self.autosave.saved_hash { return Ok(true); }
        if self.lib_file.is_some() { return self.autosave.history_changed(&self.hist); }
        Ok(false)
    }

    /// Same native target, including Save As selecting the current path. Resolving
    /// the path never supplies an expected generation from current disk contents.
    pub(crate) fn is_current_external_target(&self, path: &Path) -> bool {
        let mut target = path.to_path_buf();
        if target.extension().is_none() { target.set_extension(crate::io::NATIVE_EXT); }
        self.file_path.as_ref().is_some_and(|current| {
            current == &target || std::fs::canonicalize(current).ok().zip(std::fs::canonicalize(&target).ok())
                .is_some_and(|(left, right)| left == right || cfg!(windows) && left.to_string_lossy().eq_ignore_ascii_case(&right.to_string_lossy()))
        })
    }

    /// Persist only the external document. Never mark undo/redo as durable here.
    pub(crate) fn save_current_external(&mut self) -> Result<(), String> {
        let path = self.file_path.as_ref().ok_or("No external document is open")?;
        if !path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case(crate::io::NATIVE_EXT)) {
            return Err("Save the imported design as a native file before leaving it. The source file was preserved.".into());
        }
        let expected = self.autosave.external_fingerprint.as_deref().ok_or("The external file has no observed generation; reopen it before saving")?;
        let (path, fingerprint) = crate::io::save_path_expected(&self.doc, path, expected)?;
        self.file_path = Some(path);
        self.autosave.saved_hash = doc_hash(&self.doc);
        self.autosave.external_fingerprint = Some(fingerprint);
        self.autosave.saved_history_hash = None;
        self.autosave.saved_checkpoint_fingerprint = None;
        self.autosave.checked_history = None;
        self.autosave.changed_at = None;
        self.autosave.confirmed = false;
        self.autosave.failures = 0;
        self.autosave.retry_at = None;
        Ok(())
    }

    /// Persistent bound files use the same save/dirty rules inside and outside the library.
    fn save_autosave_target(&mut self) -> Result<(), String> {
        if self.lib_file.is_some() { return self.save_library_file(); }
        self.file_path.as_ref().filter(|p| self.doc.cityos.is_some()
            && p.extension().is_some_and(|e| e.eq_ignore_ascii_case(crate::io::NATIVE_EXT)))
            .ok_or("Save this draft to the library or a native file first")?;
        self.save_current_external()
    }
    fn has_autosave_target(&self) -> bool {
        self.lib_file.is_some() || self.doc.cityos.is_some() && self.file_path.as_ref()
            .is_some_and(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case(crate::io::NATIVE_EXT)))
    }
    /// Opens (or creates) the library at `root` and seeds it on first run.
    pub fn attach_library(&mut self, root: PathBuf) {
        // Shared libraries use the same root as the projects library.
        self.libs.set_root(root.clone());
        match Library::open(&root) {
            Ok(lib) => {
                match lib.seed_first_run(crate::demo::demo_document) {
                    // The sample file gets its thumbnail right away so the home screen is not blank.
                    Ok(Some(entry)) => {
                        if let Ok(doc) = lib.load_file(&entry.id) {
                            thumb::refresh(&lib, &entry.id, &doc);
                        }
                    }
                    Ok(None) => {}
                    Err(e) => self.show_toast(format!("Could not create the sample project: {e}"), true),
                }
                self.lib = Some(lib);
            }
            Err(e) => self.show_toast(format!("Cannot open the library at {}: {e}", root.display()), true),
        }
    }

    /// Shows the home screen ("Back to files"), saving pending edits first.
    pub fn go_home(&mut self) {
        let Some(lib) = self.lib.clone() else {
            self.show_toast("The projects library is unavailable", true);
            return;
        };
        crate::decks::ui::flush_notes(self);
        if !self.flush_autosave(){return;}
        self.save_close_version();
        self.stop_plugin();
        self.canvas.reset_interaction();
        let mut home = Home::new();
        home.refresh(&lib);
        self.home = Some(home);
        self.ui.menu = None;
        self.ui.ctx_menu = None;
    }

    /// Opens a library file in the editor.
    pub fn open_library_file(&mut self, id: &str) {
        let Some(lib) = self.lib.clone() else { return };
        // Observe our own pending save before loading its successor checkpoint.
        if self.lib_file.as_ref().is_some_and(|file| file.id == id) {
            self.exit_version_view();
            if !self.flush_autosave() { return; }
        }
        let loaded = lib.file(id).and_then(|e| {
            let (checkpoint, missing) = lib.load_checkpoint_for_recovery(id)?;
            let fingerprint = desigma_library::checkpoint_fingerprint(&checkpoint)?;
            Ok((e, checkpoint.document, checkpoint.history, missing, fingerprint))
        });
        let (entry, mut doc, history, missing, fingerprint) = match loaded {
            Ok(x) => x,
            Err(e) => {
                self.show_toast(format!("Cannot open the file: {e}"), true);
                return;
            }
        };
        let project = lib.project(&entry.project).map(|p| p.name).unwrap_or_default();
        doc.name = entry.name.clone();
        let autosave = match Autosave::library_checkpoint(self.canvas.tiles.rev(), &doc, &history, fingerprint) {
            Ok(autosave) => autosave,
            Err(error) => { self.show_toast(error, true); return; }
        };
        let needs_thumb = lib.thumbnail_path(&entry).is_none();
        if !self.replace_document(doc, None){return;}
        self.hist = history;
        self.lib_file = Some(LibFile { id: entry.id.clone(), project_id: entry.project.clone(), project, name: entry.name.clone() });
        self.autosave = autosave;
        self.autosave.seen_rev = self.canvas.tiles.rev();
        self.history_open_file();
        let _ = lib.mark_opened(id);
        self.home = None;
        self.ui.menu = None;
        if !missing.is_empty() { self.show_toast(format!("Recovered draft. {} dependencies must be verified before editing or export.",missing.len()),true); }
        if needs_thumb {
            thumb::refresh_in_background(lib, entry.id, self.export_doc().into_owned());
        }
    }

    /// Creates an empty design file in `project` (or the first / a new project) and opens it.
    pub fn new_library_file(&mut self, target: Option<(String, Option<String>)>) {
        self.new_library_file_of(target, DocumentKind::Design);
    }

    /// Creates an empty board in `project` (or the first / a new project) and opens it.
    pub fn new_library_board(&mut self, target: Option<(String, Option<String>)>) {
        self.new_library_file_of(target, DocumentKind::Board);
    }

    pub fn new_library_deck(&mut self, target: Option<(String, Option<String>)>) {
        self.new_library_file_of(target, DocumentKind::Deck);
    }

    pub fn new_deck_here(&mut self) {
        let target = self.lib_file.as_ref().map(|f| (f.project_id.clone(), None));
        self.new_library_deck(target);
    }

    fn new_library_file_of(&mut self, target: Option<(String, Option<String>)>, kind: DocumentKind) {
        if !self.flush_autosave(){return;}
        let Some(lib) = self.lib.clone() else {
            if kind == DocumentKind::Board {
                self.new_board_document();
            } else if kind == DocumentKind::Deck {
                self.new_deck_document();
            } else {
                self.new_document();
            }
            return;
        };
        let result = (|| {
            let (project, folder) = match target {
                Some(t) => t,
                None => match lib.list_projects().first() {
                    Some(p) => (p.id.clone(), None),
                    None => (lib.create_project("My project")?.id, None),
                },
            };
            let doc = match kind {
                DocumentKind::Board => crate::board::create::new_document("Untitled"),
                DocumentKind::Deck => crate::decks::new_document("Untitled deck"),
                DocumentKind::Design => {
                    let mut doc = Document::new("Untitled");
                    doc.add_page("Page 1");
                    doc
                }
            };
            lib.create_file(&project, folder.as_deref(), "Untitled", &doc)
        })();
        match result {
            Ok(e) => self.open_library_file(&e.id),
            Err(e) => self.show_toast(format!("Cannot create the file: {e}"), true),
        }
    }

    /// File → New file: next to the open library file when there is one.
    pub fn new_file_here(&mut self) {
        let target = self.lib_file.as_ref().map(|f| (f.project_id.clone(), None));
        self.new_library_file(target);
    }

    /// File → New board: next to the open library file when there is one.
    pub fn new_board_here(&mut self) {
        let target = self.lib_file.as_ref().map(|f| (f.project_id.clone(), None));
        self.new_library_board(target);
    }

    /// Writes the open library file (no toast); also refreshes its thumbnail.
    pub fn save_library_file(&mut self) -> Result<(), String> {
        crate::decks::ui::flush_notes(self);
        let (Some(lib), Some(lf)) = (self.lib.clone(), self.lib_file.clone()) else { return Err("This file is not in the library".into()) };
        if self.history.viewing.is_some() {
            // an old version is on screen: the file on disk must keep the live document
            return Err("Go back to the current version before saving".into());
        }
        // Prepare the comparison before writing; only install it after persistence.
        self.autosave = self.persist_library_checkpoint(&self.doc, &self.hist)?;
        thumb::refresh_in_background(lib, lf.id, self.export_doc().into_owned());
        Ok(())
    }

    /// Ctrl+S on a library file.
    pub fn save_library_file_with_toast(&mut self) {
        match self.save_library_file() {
            Ok(()) => {
                let project = self.lib_file.as_ref().map(|f| f.project.clone()).unwrap_or_default();
                self.show_toast(format!("Saved to {project}"), false);
            }
            Err(e) => self.show_toast(e, true),
        }
    }

    /// Saves pending edits right away (before leaving the editor or the app).
    pub fn flush_autosave(&mut self) -> bool {
        if self.history.viewing.is_some() {
            return true;
        }
        if self.has_autosave_target() {
            let result = self.autosave_target_changed().and_then(|changed| if changed { self.save_autosave_target() } else { Ok(()) });
            if let Err(e) = result {
                self.autosave.changed_at=Some(self.now);
                self.autosave.confirmed=true;
                self.show_toast(e, true);
                return false;
            }
        }
        self.autosave.changed_at = None;
        true
    }

    /// Called every frame: debounced autosave of the open library file.
    pub fn autosave_tick(&mut self, ctx: &egui::Context) {
        if !self.has_autosave_target() || self.home.is_some() || self.history.viewing.is_some() {
            self.autosave.changed_at = None;
            return;
        }
        let rev = self.canvas.tiles.rev();
        let history = self.hist.change_identity();
        let history_changed = self.lib_file.is_some() && self.autosave.seen_history.as_ref().is_none_or(|seen| !Arc::ptr_eq(seen, &history));
        if rev != self.autosave.seen_rev || history_changed {
            self.autosave.seen_rev = rev;
            self.autosave.seen_history = Some(history);
            self.autosave.changed_at = Some(self.now);
            self.autosave.confirmed = false;
        }
        let Some(t) = self.autosave.changed_at else { return };
        if let Some(at)=self.autosave.retry_at {
            if self.now<at {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64((at-self.now).min(5.)));
                return;
            }
        }
        let busy = ctx.input(|i| i.pointer.any_down()) || self.canvas.text.is_some();
        if busy {
            self.autosave.changed_at = Some(self.now);
            self.autosave.confirmed = false;
        } else if !self.autosave.confirmed && self.now - t >= PROBE_DELAY {
            // Cheap-ish check once the revision settled: did the document really change?
            match self.autosave_target_changed() {
                Ok(false) => { self.autosave.changed_at = None; return; }
                Ok(true) => {}
                Err(error) => { self.show_toast(error, true); return; }
            }
            self.autosave.confirmed = true;
        } else if self.autosave.confirmed && self.now - t >= if self.doc.cityos.is_some(){2.0}else{AUTOSAVE_IDLE} {
            match self.autosave_target_changed() {
                Ok(true) => match self.save_autosave_target() {
                    Ok(()) => self.autosave.saves += 1,
                    Err(e) => {
                        self.autosave.changed_at=Some(self.now);
                        self.autosave.failures=self.autosave.failures.saturating_add(1);
                        let backoff=(2_u64 << self.autosave.failures.saturating_sub(1).min(4)).min(30);
                        self.autosave.retry_at=Some(self.now+backoff as f64);
                        self.show_toast(e, true);
                        ctx.request_repaint_after(std::time::Duration::from_secs(backoff));
                    },
                },
                Ok(false) => self.autosave.changed_at = None,
                Err(error) => self.show_toast(error, true),
            }
            return;
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }

    /// Renames the open library file (header double-click).
    pub fn rename_library_file(&mut self, name: &str) {
        let (Some(lib), Some(lf)) = (self.lib.clone(), self.lib_file.clone()) else { return };
        let name = name.trim();
        if name.is_empty() || name == lf.name {
            return;
        }
        match lib.rename_file(&lf.id, name) {
            Ok(()) => {
                if let Some(f) = self.lib_file.as_mut() {
                    f.name = name.to_string();
                }
                self.doc.name = name.to_string();
            }
            Err(e) => self.show_toast(format!("Cannot rename: {e}"), true),
        }
    }

    /// Stores the open (external or unsaved) document as a new library file and keeps editing it there.
    pub fn save_to_library_as(&mut self, project: &str, folder: Option<&str>, name: &str) {
        crate::decks::ui::flush_notes(self);
        let Some(lib) = self.lib.clone() else { return };
        let mut doc = self.doc.clone();
        doc.name = name.trim().to_string();
        match lib.create_file_with_history(project, folder, name, &doc, &self.hist) {
            Ok(e) => {
                doc.name = e.name.clone();
                let autosave = lib.load_checkpoint(&e.id).map_err(|error| error.to_string())
                    .and_then(|checkpoint| Autosave::created_library_checkpoint(self.canvas.tiles.rev(), &doc, &self.hist, checkpoint));
                let autosave = match autosave {
                    Ok(autosave) => autosave,
                    Err(error) => { self.show_toast(format!("File created, but its checkpoint could not be observed: {error}"), true); return; }
                };
                self.doc.name = e.name.clone();
                let project_name = lib.project(project).map(|p| p.name).unwrap_or_default();
                self.lib_file = Some(LibFile { id: e.id.clone(), project_id: project.to_string(), project: project_name.clone(), name: e.name });
                self.autosave = autosave;
                self.history_open_file();
                self.file_path = None;
                thumb::refresh_in_background(lib, e.id, self.export_doc().into_owned());
                self.show_toast(format!("Saved to {project_name}"), false);
            }
            Err(e) => self.show_toast(format!("Cannot save to the library: {e}"), true),
        }
    }

    /// Imports files into a project; returns how many succeeded.
    pub fn import_into_library(&mut self, paths: &[PathBuf], project: &str, folder: Option<&str>) -> usize {
        let Some(lib) = self.lib.clone() else { return 0 };
        let mut ok = 0;
        let mut last_err = None;
        for p in paths {
            match lib.import_file_with_policy(&desigma_cityos::storage_policy::CITYOS_STORAGE_POLICY, p, project, folder) {
                Ok(e) => {
                    ok += 1;
                    if let Ok(doc) = lib.load_file(&e.id) {
                        thumb::refresh_in_background(lib.clone(), e.id, doc);
                    }
                }
                Err(e) => last_err = Some(format!("{}: {e}", p.file_name().and_then(|n| n.to_str()).unwrap_or("file"))),
            }
        }
        match (ok, last_err) {
            (0, Some(e)) => self.show_toast(format!("Import failed. {e}"), true),
            (n, Some(e)) => self.show_toast(format!("Imported {n}, but {e}"), true),
            (n, None) if n > 0 => self.show_toast(format!("Imported {n} file{}", if n == 1 { "" } else { "s" }), false),
            _ => {}
        }
        if let Some(h) = self.home.as_mut() {
            h.dirty = true;
        }
        ok
    }

    /// The section a "New design file" button creates into.
    pub fn home_target(&self) -> Option<(String, Option<String>)> {
        match &self.home.as_ref()?.section {
            Section::Project(p, f) => Some((p.clone(), f.clone())),
            _ => None,
        }
    }
}
