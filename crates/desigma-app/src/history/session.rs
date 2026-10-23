//! Viewing and restoring versions: swapping the editor's document in and out.

use super::{Snapshot, Viewing};
use crate::app::App;
use crate::home::doc_hash;
use desigma_doc::{Command, Document, History};

impl App {
    fn take_snapshot(&mut self) -> Snapshot {
        crate::decks::ui::flush_notes(self);
        Snapshot { doc: std::mem::take(&mut self.doc), hist: std::mem::take(&mut self.hist), page: self.page, selection: std::mem::take(&mut self.selection) }
    }

    /// Puts `doc` on the canvas with the given undo stack, keeping the view.
    fn install(&mut self, doc: Document, hist: History, page: Option<desigma_doc::NodeId>, selection: Vec<desigma_doc::NodeId>) {
        self.stop_plugin();
        let mut doc = doc;
        crate::io::ensure_page(&mut doc);
        self.page = page.filter(|p| doc.pages.contains(p)).unwrap_or(doc.pages[0]);
        self.doc = doc;
        self.decks = Default::default();
        self.hist = hist;
        self.selection = selection;
        self.selection.retain(|id| self.doc.get(*id).is_some());
        self.hover = None;
        self.hover_layer = None;
        self.canvas.reset_interaction();
        self.layers.reset();
        self.fonts.load_for(&self.doc);
        self.comments.close_popovers();
        self.changed();
    }

    /// Shows version `n` read-only on the canvas.
    pub fn view_version(&mut self, n: u32) {
        let (Some(lib), Some(lf)) = (self.lib.clone(), self.lib_file.clone()) else { return };
        let mut doc = match lib.load_version(&lf.id, n) {
            Ok(d) => d,
            Err(e) => {
                self.show_toast(format!("Cannot open the version: {e}"), true);
                return;
            }
        };
        doc.name = lf.name.clone();
        self.refresh_versions();
        let title = self.history.list.iter().find(|v| v.n == n).map(super::version_title).unwrap_or_else(|| format!("version {n}"));
        let saved = match self.history.viewing.take() {
            Some(v) => v.saved,
            None => {
                // edits still waiting for the autosave must be on disk before the live document is parked
                if !self.flush_autosave() {
                    return;
                }
                self.take_snapshot()
            }
        };
        let page = Some(self.page);
        self.install(doc, History::default(), page, vec![]);
        self.history.viewing = Some(Viewing { n, title, saved });
    }

    /// "Back to current": drops the read-only view.
    pub fn exit_version_view(&mut self) {
        let Some(v) = self.history.viewing.take() else { return };
        let s = v.saved;
        self.install(s.doc, s.hist, Some(s.page), s.selection);
    }

    /// "Restore this version": the current state becomes an automatic version first, then
    /// version `n` becomes the current document. One undo brings the old state back.
    pub fn restore_version(&mut self, n: u32) {
        let (Some(lib), Some(lf)) = (self.lib.clone(), self.lib_file.clone()) else { return };
        let mut old = match lib.load_version(&lf.id, n) {
            Ok(d) => d,
            Err(e) => {
                self.show_toast(format!("Cannot restore the version: {e}"), true);
                return;
            }
        };
        if self.doc.cityos.is_some() || old.cityos.is_some() || self.history.viewing.as_ref().is_some_and(|view| view.saved.doc.cityos.is_some()) {
            self.show_toast("Bound drafts require their reviewed baseline recovery; ordinary version restore is unavailable", true);
            return;
        }
        old.name = lf.name.clone();
        self.exit_version_view();
        crate::decks::ui::flush_notes(self);
        crate::io::ensure_page(&mut old);
        if let Err(e) = lib.save_version(&lf.id, &self.doc, None, true) {
            self.show_toast(format!("Cannot keep the current version, restore cancelled: {e}"), true);
            return;
        }
        // Persist before installing the historical projection. A failed disk write
        // must not turn a memory-only restore into a success notification.
        let mut document = self.doc.clone();
        let mut history = self.hist.clone();
        if let Err(e) = history.exec(&mut document, Command::RestoreDocument { document: Box::new(old) }) {
            self.show_toast(format!("Cannot restore; current document retained: {e}"), true);
            return;
        }
        let saved = match self.persist_library_checkpoint(&document, &history) {
            Ok(saved) => saved,
            Err(e) => {
                self.show_toast(format!("Cannot restore; current document retained: {e}"), true);
                return;
            }
        };
        self.install(document, history, Some(self.page), vec![]);
        self.history.dirty = true;
        self.history.last_version_hash = doc_hash(&self.doc);
        self.autosave = saved;
        self.autosave.seen_rev = self.canvas.tiles.rev();
        self.show_toast("Version restored. Undo brings the previous state back", false);
    }

    /// Persist a restore transition before installing its document. Returning
    /// true also consumes failures so the ordinary Undo path cannot bypass CAS.
    pub fn try_undo_restore(&mut self) -> bool {
        self.try_restore_history(false)
    }

    /// Redo uses the persisted stack; a new edit clears it through normal history.
    pub fn try_redo_restore(&mut self) -> bool {
        self.try_restore_history(true)
    }

    fn try_restore_history(&mut self, redo: bool) -> bool {
        if self.history.viewing.is_some() || !self.hist.next_is_document_restore(redo) {
            return false;
        }
        let action = if redo { "redo" } else { "undo" };
        let mut document = self.doc.clone();
        let mut history = self.hist.clone();
        let result = if redo { history.redo(&mut document) } else { history.undo(&mut document) };
        if let Err(error) = result {
            self.show_toast(format!("Cannot {action} restore: {error}"), true);
            return true;
        }
        let saved = if self.lib_file.is_some() {
            match self.persist_library_checkpoint(&document, &history) {
                Ok(saved) => Some(saved),
                Err(error) => {
                    self.show_toast(format!("Cannot {action} restore: {error}"), true);
                    return true;
                }
            }
        } else {
            None
        };
        self.install(document, history, Some(self.page), self.selection.clone());
        if let Some(saved) = saved {
            self.autosave = saved;
            self.autosave.seen_rev = self.canvas.tiles.rev();
        }
        true
    }
}
