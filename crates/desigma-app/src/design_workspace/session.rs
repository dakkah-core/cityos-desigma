//! Opaque native editor state. Ownership moves only inside a successful switch.
use crate::app::{App, View};
use crate::home::{Autosave, LibFile};
use desigma_doc::{Document, History, NodeId};
use std::path::PathBuf;

pub(super) struct EditorSession {
    document: Document,
    path: Option<PathBuf>,
    state: SessionState,
}

struct SessionState {
    history: History,
    file: Option<LibFile>,
    autosave: Autosave,
    page: NodeId,
    view: View,
    selection: Vec<NodeId>,
    first_fit_done: bool,
}

impl EditorSession {
    // Called only from App's post-save transfer closure. No history clones and
    // no disposable placeholder is ever exposed to persistence or a UI frame.
    fn take(app: &mut App) -> Self {
        Self {
            document: std::mem::replace(&mut app.doc, Document::new("Switching document")),
            path: app.file_path.take(),
            state: SessionState {
                history: std::mem::take(&mut app.hist),
                file: app.lib_file.take(),
                autosave: std::mem::take(&mut app.autosave),
                page: app.page,
                view: app.view,
                selection: std::mem::take(&mut app.selection),
                first_fit_done: app.first_fit_done,
            },
        }
    }

    pub(super) fn name(&self) -> &str { &self.document.name }

    /// A failed save leaves the incoming document and the active editor owned
    /// by their original holders. The callback is never invoked on failure.
    pub(super) fn replace(app: &mut App, document: Document, path: Option<PathBuf>) -> Option<Self> {
        app.replace_document_transaction(move |app| (document, path, Self::take(app)))
    }

    /// The incoming slot is consumed only after App has saved the outgoing state.
    pub(super) fn swap(app: &mut App, incoming: &mut Option<Self>) -> Option<Self> {
        if incoming.is_none() { return None; }
        let (outgoing, state) = app.replace_document_transaction(|app| {
            let outgoing = Self::take(app);
            let Self { document, path, state } = incoming.take().expect("checked session remains owned until transfer");
            (document, path, (outgoing, state))
        })?;
        state.restore(app);
        Some(outgoing)
    }

    #[cfg(test)]
    pub(super) fn file_id(&self) -> Option<&str> { self.state.file.as_ref().map(|file| file.id.as_str()) }
}

impl SessionState {
    fn restore(self, app: &mut App) {
        app.hist = self.history;
        app.lib_file = self.file;
        app.autosave = self.autosave;
        app.page = self.page;
        app.view = self.view;
        app.selection = self.selection;
        app.first_fit_done = self.first_fit_done;
        app.home = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use desigma_doc::{Command, Node, Rect, Vec2};

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let path=std::env::temp_dir().join(format!("desigma-editor-session-{}",uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&path).unwrap(); Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            crate::home::thumb::finish_library_jobs(&self.0);
            let _=std::fs::remove_dir_all(&self.0);
        }
    }
    fn document(name:&str)->Document {
        let mut document=Document::new(name);let page=document.add_page("Page");
        let id=document.alloc_id();document.insert(page,Node::rectangle(id,Rect::new(0.,0.,80.,60.))).unwrap();document
    }
    fn edit(app:&mut App) {
        let id=*app.doc.nodes.keys().find(|id|!app.doc.pages.contains(id)).unwrap();
        let mut node=app.doc.get(id).unwrap().clone();node.name="Edited rectangle".into();
        app.hist.exec(&mut app.doc,Command::Update{node:Box::new(node)}).unwrap();app.changed();
    }
    #[test]
    fn final_save_failure_preserves_active_history_and_incoming_session_after_an_earlier_save() {
        let root=Temp::new();let library=desigma_library::Library::open(&root.0).unwrap();
        let project=library.create_project("Sessions").unwrap();
        let mut app=App::new(document("Current"));
        let file=library.create_file_with_history(&project.id,None,"Current",&app.doc,&app.hist).unwrap();
        app.lib=Some(library);app.home=None;
        app.lib_file=Some(LibFile{id:file.id.clone(),project_id:project.id.clone(),project:project.name,name:file.name});
        app.save_library_file().unwrap();
        crate::home::thumb::finish_library_jobs(&root.0);
        // The first save succeeded. The final flush must still fail closed if
        // state changes before transfer and that save can no longer complete.
        edit(&mut app);
        let manifest=root.0.join("projects").join(&project.id).join("project.json");
        let original_manifest=std::fs::read(&manifest).unwrap();
        std::fs::write(&manifest,b"{damaged manifest").unwrap();
        let mut target=App::new(document("Incoming"));
        let mut incoming=Some(EditorSession::take(&mut target));
        let before=app.doc.to_json().unwrap();let history=serde_json::to_value(&app.hist).unwrap();
        let saved=app.autosave.saved_hash;
        assert!(EditorSession::swap(&mut app,&mut incoming).is_none());
        assert_eq!(app.doc.to_json().unwrap(),before);
        assert_eq!(serde_json::to_value(&app.hist).unwrap(),history);
        assert!(app.hist.can_undo());
        assert_eq!(app.lib_file.as_ref().unwrap().id,file.id);
        assert_eq!(app.autosave.saved_hash,saved);
        assert!(app.autosave.pending());
        assert_eq!(incoming.as_ref().unwrap().name(),"Incoming");
        std::fs::write(manifest,original_manifest).unwrap();
    }
    #[test]
    fn successful_session_swap_restores_history_selection_view_and_autosave_without_cloning() {
        let mut app=App::new(document("First"));edit(&mut app);
        app.view=View{offset:Vec2::new(20.,30.),zoom:1.75};app.first_fit_done=true;
        app.selection=app.doc.nodes.keys().filter(|id|!app.doc.pages.contains(id)).copied().collect();
        app.autosave.saves=7;let selection=app.selection.clone();let first=app.doc.to_json().unwrap();
        let mut other=App::new(document("Second"));
        let mut incoming=Some(EditorSession::take(&mut other));
        let outgoing=EditorSession::swap(&mut app,&mut incoming).unwrap();
        assert!(incoming.is_none());assert_eq!(app.doc.name,"Second");assert!(!app.hist.can_undo());
        let mut restore=Some(outgoing);
        let second=EditorSession::swap(&mut app,&mut restore).unwrap();
        assert!(restore.is_none());assert_eq!(second.name(),"Second");
        assert_eq!(app.doc.to_json().unwrap(),first);assert!(app.hist.can_undo());
        assert_eq!(app.selection,selection);assert_eq!(app.view.offset,Vec2::new(20.,30.));assert_eq!(app.view.zoom,1.75);
        assert!(app.first_fit_done);assert_eq!(app.autosave.saves,7);
    }
}
