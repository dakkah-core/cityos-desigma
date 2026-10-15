//! Viewing and restoring versions: swapping the editor's document in and out.

use super::{Snapshot, Viewing};
use crate::app::App;
use crate::home::doc_hash;
use desigma_doc::{Document, History};

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
                self.flush_autosave();
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
        old.name = lf.name.clone();
        self.exit_version_view();
        if let Err(e) = lib.save_version(&lf.id, &self.doc, None, true) {
            self.show_toast(format!("Cannot keep the current version, restore cancelled: {e}"), true);
            return;
        }
        // Persist before installing the historical projection. A failed disk write
        // must not turn a memory-only restore into a success notification.
        if let Err(e)=lib.save_file(&lf.id,&old) {
            self.show_toast(format!("Cannot restore; current document retained: {e}"),true);
            return;
        }
        let before = self.take_snapshot();
        let page = Some(before.page);
        self.install(old, History::default(), page, vec![]);
        self.history.restore_undo = Some(before);
        self.history.restore_redo = None;
        self.history.dirty = true;
        self.history.last_version_hash = doc_hash(&self.doc);
        self.autosave.saved_hash=doc_hash(&self.doc);
        self.autosave.changed_at=None;
        self.show_toast("Version restored. Undo brings the previous state back", false);
    }

    /// Undo with nothing left to undo steps back over a restore.
    pub fn try_undo_restore(&mut self) -> bool {
        if self.hist.can_undo() || self.history.viewing.is_some() {
            return false;
        }
        let Some(snap) = self.history.restore_undo.take() else { return false };
        if let (Some(lib),Some(lf))=(self.lib.clone(),self.lib_file.clone()) {
            if let Err(e)=lib.save_file(&lf.id,&snap.doc){self.history.restore_undo=Some(snap);self.show_toast(format!("Cannot undo restore: {e}"),true);return false;}
        }
        let now = self.take_snapshot();
        self.install(snap.doc, snap.hist, Some(snap.page), snap.selection);
        let expected = doc_hash(&self.doc);
        self.history.restore_redo = Some((now, expected));
        self.autosave.saved_hash=doc_hash(&self.doc);
        self.autosave.changed_at=None;
        true
    }

    /// Redo of an undone restore, while the document is still as undo left it.
    pub fn try_redo_restore(&mut self) -> bool {
        if self.history.viewing.is_some() {
            return false;
        }
        let Some((snap, expected)) = self.history.restore_redo.take() else { return false };
        if doc_hash(&self.doc) != expected {
            return false;
        }
        if let (Some(lib),Some(lf))=(self.lib.clone(),self.lib_file.clone()) {
            if let Err(e)=lib.save_file(&lf.id,&snap.doc){self.history.restore_redo=Some((snap,expected));self.show_toast(format!("Cannot redo restore: {e}"),true);return false;}
        }
        let before = self.take_snapshot();
        self.install(snap.doc, snap.hist, Some(snap.page), snap.selection);
        self.history.restore_undo = Some(before);
        self.autosave.saved_hash=doc_hash(&self.doc);
        self.autosave.changed_at=None;
        true
    }
}
