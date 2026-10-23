//! Undo/redo. Every edit goes through [`Command`]; applying returns its inverse.

use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Command {
    /// Restore an ordinary native document within the current history baseline.
    /// Managed bindings must use their separately validated semantic boundary.
    RestoreDocument {
        #[serde(with = "crate::persistence::native_box")]
        document: Box<Document>,
    },
    /// Whole validated semantic state and its native projection share one undo boundary.
    CityosInstall {
        #[serde(with = "crate::persistence::native_box")]
        document: Box<Document>,
    },
    /// Insert subtree nodes (root first) under `parent` at `index`.
    Insert {
        parent: NodeId,
        index: usize,
        nodes: Vec<Node>,
    },
    Remove {
        id: NodeId,
    },
    Reparent {
        id: NodeId,
        parent: NodeId,
        index: usize,
    },
    /// Replace a node's own properties (children/parent are preserved).
    Update {
        node: Box<Node>,
    },
    /// Insert a page subtree (page node first) at `index` in `Document::pages`.
    InsertPage {
        index: usize,
        nodes: Vec<Node>,
    },
    RemovePage {
        id: NodeId,
    },
    /// Replace the document's prototype flow list (flow starting points).
    SetFlows {
        flows: Vec<Flow>,
    },
    /// Replace shared styles and their metadata (style create / edit / delete).
    SetStyles {
        styles: BTreeMap<String, Style>,
        style_meta: BTreeMap<String, StyleMeta>,
    },
    /// Replace all comment threads.
    SetComments {
        comments: Vec<CommentThread>,
    },
    /// Replace library bookkeeping (publish info, enabled libraries, asset copies).
    SetLibraryState {
        state: LibraryState,
        library_page: Option<NodeId>,
    },
    /// Replace the guides of one page.
    SetGuides {
        page: NodeId,
        guides: Vec<Guide>,
    },
    /// Replace the prototype device settings.
    SetPrototypeDevice {
        device: PrototypeDevice,
    },
    /// Replace all variables and collections (variable edits).
    SetVariables {
        variables: BTreeMap<String, Variable>,
        collections: BTreeMap<String, VariableCollection>,
    },
    /// Replace the board's timer and voting session.
    SetBoardSession {
        session: BoardSession,
    },
    Batch(Vec<Command>),
}

impl Command {
    /// Applies the command and returns the command that undoes it.
    pub fn apply(self, doc: &mut Document) -> Result<Command, DocError> {
        if doc.cityos.is_some() && !matches!(self, Command::CityosInstall { .. }) {
            return Err(DocError::CityosProtected);
        }
        Ok(match self {
            Command::RestoreDocument { document } => {
                if document.is_managed() {
                    return Err(DocError::CityosProtected);
                }
                let previous = std::mem::replace(doc, *document);
                Command::RestoreDocument { document: Box::new(previous) }
            }
            Command::CityosInstall { mut document } => {
                let current = doc.cityos.as_ref().ok_or(DocError::CityosProtected)?;
                let next = document.cityos.as_mut().ok_or(DocError::CityosProtected)?;
                if current.validate().is_err()
                    || next.validate().is_err()
                    || current.draft_id != next.draft_id
                    || current.base != next.base
                    || current.semantic_base != next.semantic_base
                {
                    return Err(DocError::CityosProtected);
                }
                // Undo/redo also creates a new local revision; no stale preview identity is reused.
                let revision = current.exchange().and_then(|d| d["draftRevision"].as_u64()).ok_or(DocError::CityosProtected)?;
                next.exchange_mut().ok_or(DocError::CityosProtected)?["draftRevision"] =
                    serde_json::json!(revision.checked_add(1).ok_or(DocError::CityosProtected)?);
                let old = std::mem::replace(doc, *document);
                Command::CityosInstall { document: Box::new(old) }
            }
            Command::Insert { parent, index, nodes } => {
                let root = nodes.first().map(|n| n.id).ok_or(DocError::NotFound(parent))?;
                let p = doc.get_mut(parent).ok_or(DocError::NotFound(parent))?;
                if !p.kind.is_container() {
                    return Err(DocError::NotAContainer(parent));
                }
                let index = index.min(p.children.len());
                p.children.insert(index, root);
                for mut n in nodes {
                    if n.id == root {
                        n.parent = Some(parent);
                    }
                    doc.nodes.insert(n.id, n);
                }
                doc.bump_ids();
                Command::Remove { id: root }
            }
            Command::Remove { id } => {
                let parent = doc.get(id).ok_or(DocError::NotFound(id))?.parent.ok_or(DocError::NotFound(id))?;
                let index = doc.get(parent).map(|p| p.children.iter().position(|c| *c == id).unwrap_or(0)).unwrap_or(0);
                let nodes = doc.remove(id)?;
                Command::Insert { parent, index, nodes }
            }
            Command::Reparent { id, parent, index } => {
                let n = doc.get(id).ok_or(DocError::NotFound(id))?;
                let old_parent = n.parent.ok_or(DocError::NotFound(id))?;
                let old_index = doc.get(old_parent).and_then(|p| p.children.iter().position(|c| *c == id)).unwrap_or(0);
                doc.reparent(id, parent, index)?;
                Command::Reparent { id, parent: old_parent, index: old_index }
            }
            Command::Update { mut node } => {
                let cur = doc.get_mut(node.id).ok_or(DocError::NotFound(node.id))?;
                node.parent = cur.parent;
                node.children = std::mem::take(&mut cur.children);
                let old = std::mem::replace(cur, *node);
                Command::Update { node: Box::new(Node { children: Vec::new(), ..old }) }
            }
            Command::InsertPage { index, nodes } => {
                let root = nodes.first().ok_or(DocError::NotFound(NodeId(0)))?;
                if !matches!(root.kind, NodeKind::Page { .. }) {
                    return Err(DocError::NotAContainer(root.id));
                }
                let root = root.id;
                for mut n in nodes {
                    if n.id == root {
                        n.parent = None;
                    }
                    doc.nodes.insert(n.id, n);
                }
                let index = index.min(doc.pages.len());
                doc.pages.insert(index, root);
                doc.bump_ids();
                Command::RemovePage { id: root }
            }
            Command::RemovePage { id } => {
                let index = doc.pages.iter().position(|p| *p == id).ok_or(DocError::NotFound(id))?;
                let nodes = doc.remove(id)?;
                Command::InsertPage { index, nodes }
            }
            Command::SetFlows { flows } => Command::SetFlows { flows: std::mem::replace(&mut doc.flows, flows) },
            Command::SetStyles { styles, style_meta } => {
                Command::SetStyles { styles: std::mem::replace(&mut doc.styles, styles), style_meta: std::mem::replace(&mut doc.style_meta, style_meta) }
            }
            Command::SetComments { comments } => Command::SetComments { comments: std::mem::replace(&mut doc.comments, comments) },
            Command::SetLibraryState { state, library_page } => Command::SetLibraryState {
                state: std::mem::replace(&mut doc.library, state),
                library_page: std::mem::replace(&mut doc.library_page, library_page),
            },
            Command::SetGuides { page, guides } => {
                let old = if guides.is_empty() { doc.guides.remove(&page) } else { doc.guides.insert(page, guides) };
                Command::SetGuides { page, guides: old.unwrap_or_default() }
            }
            Command::SetPrototypeDevice { device } => Command::SetPrototypeDevice { device: std::mem::replace(&mut doc.prototype_device, device) },
            Command::SetBoardSession { session } => Command::SetBoardSession { session: std::mem::replace(&mut doc.board_session, session) },
            Command::SetVariables { variables, collections } => Command::SetVariables {
                variables: std::mem::replace(&mut doc.variables, variables),
                collections: std::mem::replace(&mut doc.variable_collections, collections),
            },
            Command::Batch(cmds) => {
                let mut staged = doc.clone();
                let mut inv = Vec::with_capacity(cmds.len());
                for c in cmds {
                    inv.push(c.apply(&mut staged)?);
                }
                *doc = staged;
                inv.reverse();
                Command::Batch(inv)
            }
        })
    }
}

impl Document {
    pub(crate) fn bump_ids(&mut self) {
        if let Some((id, _)) = self.nodes.last_key_value() {
            self.next_id = self.next_id.max(id.0);
        }
    }
}

/// Linear undo stack.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct History {
    undo: Vec<Command>,
    redo: Vec<Command>,
    /// Local concurrency fence only; never an owner revision or approval.
    #[serde(default, rename = "baselineId", skip_serializing_if = "Option::is_none")]
    baseline_id: Option<String>,
    /// Process-local identity, renewed after each mutation or deserialization.
    /// It avoids serializing whole snapshots to check an asynchronous intent.
    #[serde(skip)]
    transition_fence: Arc<()>,
}

/// An opaque, process-local stack intent. Extensions validate document semantics
/// separately; this type grants neither semantic validation nor owner authority.
#[derive(Clone, Debug)]
pub struct HistoryTransition {
    redo: bool,
    baseline_id: Option<String>,
    undo_len: usize,
    redo_len: usize,
    fence: Arc<()>,
    command: Command,
}
impl HistoryTransition {
    pub fn command(&self) -> &Command {
        &self.command
    }
}

impl History {
    /// Cheap process-local observation of stack changes. Keep the Arc alive and
    /// compare with Arc::ptr_eq; this is neither serialized nor owner authority.
    pub fn change_identity(&self) -> Arc<()> {
        self.transition_fence.clone()
    }

    /// Inspect one stack entry without replaying a historical payload. The caller
    /// can validate an extension operation asynchronously before committing it.
    pub fn prepare_transition(&self, redo: bool) -> Option<HistoryTransition> {
        let command = (if redo { &self.redo } else { &self.undo }).last()?;
        Some(HistoryTransition {
            redo,
            baseline_id: self.baseline_id.clone(),
            undo_len: self.undo.len(),
            redo_len: self.redo.len(),
            fence: self.transition_fence.clone(),
            command: command.clone(),
        })
    }

    pub fn transition_current(&self, step: &HistoryTransition) -> bool {
        Arc::ptr_eq(&self.transition_fence, &step.fence)
            && self.baseline_id == step.baseline_id
            && self.undo.len() == step.undo_len
            && self.redo.len() == step.redo_len
    }

    /// Install a separately validated command and move the exact prepared stack
    /// entry atomically. No extension-specific revisions or payloads are interpreted.
    pub fn commit_transition(&mut self, doc: &mut Document, step: HistoryTransition, validated: Command) -> Result<(), DocError> {
        if !self.transition_current(&step) {
            return Err(DocError::HistoryChanged);
        }
        let mut staged = doc.clone();
        let inverse = validated.apply(&mut staged)?;
        *doc = staged;
        if step.redo {
            self.redo.pop();
            self.undo.push(inverse);
        } else {
            self.undo.pop();
            self.redo.push(inverse);
        }
        self.transition_fence = Arc::new(());
        Ok(())
    }

    /// Detaches the active undo/redo stacks at a baseline boundary. Keep the
    /// returned history with its old document; it is not an active undo source.
    pub fn seal(&mut self) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let previous = std::mem::take(self);
        let clock = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        self.baseline_id = Some(format!("{:x}-{:x}-{:x}", std::process::id(), clock, NEXT.fetch_add(1, Ordering::Relaxed)));
        previous
    }

    pub fn baseline_id(&self) -> Option<&str> {
        self.baseline_id.as_deref()
    }

    /// Empty active stacks within this same baseline, with a fresh process-local
    /// transition fence. This neither creates nor crosses a baseline boundary.
    pub fn empty_with_same_baseline(&self) -> Self {
        Self { baseline_id: self.baseline_id.clone(), ..Self::default() }
    }

    /// Documents retained by whole-document undo/redo commands, including batches.
    /// Persistence clients use these immutable snapshots to validate export policy
    /// and recovery dependencies; this never exposes mutable history entries.
    pub fn retained_documents(&self) -> Vec<&Document> {
        fn collect<'a>(command: &'a Command, out: &mut Vec<&'a Document>) {
            match command {
                Command::CityosInstall { document } | Command::RestoreDocument { document } => out.push(document),
                Command::Batch(commands) => commands.iter().for_each(|c| collect(c, out)),
                _ => {}
            }
        }
        let mut documents = Vec::new();
        for command in self.undo.iter().chain(&self.redo) {
            collect(command, &mut documents);
        }
        documents
    }

    pub fn exec(&mut self, doc: &mut Document, cmd: Command) -> Result<(), DocError> {
        let inv = cmd.apply(doc)?;
        self.undo.push(inv);
        self.redo.clear();
        self.transition_fence = Arc::new(());
        Ok(())
    }
    pub fn undo(&mut self, doc: &mut Document) -> Result<bool, DocError> {
        let Some(c) = self.undo.last().cloned() else { return Ok(false) };
        let mut staged = doc.clone();
        let inverse = c.apply(&mut staged)?;
        *doc = staged;
        self.undo.pop();
        self.redo.push(inverse);
        self.transition_fence = Arc::new(());
        Ok(true)
    }
    pub fn redo(&mut self, doc: &mut Document) -> Result<bool, DocError> {
        let Some(c) = self.redo.last().cloned() else { return Ok(false) };
        let mut staged = doc.clone();
        let inverse = c.apply(&mut staged)?;
        *doc = staged;
        self.redo.pop();
        self.undo.push(inverse);
        self.transition_fence = Arc::new(());
        Ok(true)
    }
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    /// The application durably saves whole-document restores before showing
    /// them. Inspecting the kind does not clone a potentially large snapshot.
    pub fn next_is_document_restore(&self, redo: bool) -> bool {
        matches!((if redo { &self.redo } else { &self.undo }).last(), Some(Command::RestoreDocument { .. }))
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepared_transition_is_fenced_after_stack_aba_and_baseline_change() {
        let mut document = Document::new("history");
        document.add_page("P");
        let mut history = History::default();
        history.exec(&mut document, Command::SetComments { comments: vec![] }).unwrap();
        let intent = history.prepare_transition(false).unwrap();
        history.undo(&mut document).unwrap();
        history.redo(&mut document).unwrap();
        assert!(!history.transition_current(&intent), "an identical stack must not revive an old asynchronous intent");
        let fresh = history.prepare_transition(false).unwrap();
        history.seal();
        assert!(!history.transition_current(&fresh));
    }

    #[test]
    fn rejected_prepared_transition_preserves_document_and_stacks() {
        let mut document = Document::new("history");
        document.add_page("P");
        let mut history = History::default();
        history.exec(&mut document, Command::SetComments { comments: vec![] }).unwrap();
        let intent = history.prepare_transition(false).unwrap();
        let before = document.to_json().unwrap();
        assert!(history.commit_transition(&mut document, intent.clone(), Command::Remove { id: NodeId(u64::MAX) }).is_err());
        assert_eq!(document.to_json().unwrap(), before);
        assert!(history.transition_current(&intent));
        history.commit_transition(&mut document, intent.clone(), intent.command().clone()).unwrap();
        assert!(!history.can_undo());
        assert!(history.can_redo());
        assert!(!history.transition_current(&intent));
    }

    #[test]
    fn process_local_transition_fence_does_not_change_checkpoint_format() {
        let mut document = Document::new("history");
        document.add_page("P");
        let mut history = History::default();
        history.exec(&mut document, Command::SetComments { comments: vec![] }).unwrap();
        let encoded = serde_json::to_string(&history).unwrap();
        assert!(!encoded.contains("transition_fence"));
        let restored: History = serde_json::from_str(&encoded).unwrap();
        assert!(restored.can_undo());
        assert!(restored.prepare_transition(false).is_some());
    }

    #[test]
    fn a_reloaded_history_cannot_consume_another_restores_pending_intent() {
        let mut document = Document::new("history");
        document.add_page("P");
        let mut history = History::default();
        history.exec(&mut document, Command::SetComments { comments: vec![] }).unwrap();
        let bytes = serde_json::to_string(&history).unwrap();
        let first: History = serde_json::from_str(&bytes).unwrap();
        let pending = first.prepare_transition(false).unwrap();
        let independently_restored: History = serde_json::from_str(&bytes).unwrap();
        assert!(!independently_restored.transition_current(&pending), "restoring identical bytes starts a new in-memory history context");
    }

    #[test]
    fn staged_history_mutation_does_not_invalidate_the_untouched_original() {
        let mut document = Document::new("history");
        document.add_page("P");
        let mut history = History::default();
        history.exec(&mut document, Command::SetComments { comments: vec![] }).unwrap();
        let pending = history.prepare_transition(false).unwrap();
        let mut staged = history.clone();
        staged.undo(&mut document.clone()).unwrap();
        assert!(history.transition_current(&pending));
        assert!(!staged.transition_current(&pending));
    }

    #[test]
    fn undo_redo_update_and_remove() {
        let mut d = Document::new("t");
        let page = d.add_page("P");
        let id = d.alloc_id();
        d.insert(page, Node::rectangle(id, Rect::new(0.0, 0.0, 10.0, 10.0))).unwrap();
        let mut h = History::default();

        let mut n = d.get(id).unwrap().clone();
        n.name = "Renamed".into();
        h.exec(&mut d, Command::Update { node: Box::new(n) }).unwrap();
        assert_eq!(d.get(id).unwrap().name, "Renamed");
        h.undo(&mut d).unwrap();
        assert_eq!(d.get(id).unwrap().name, "Rectangle");

        h.exec(&mut d, Command::Remove { id }).unwrap();
        assert!(d.get(id).is_none());
        h.undo(&mut d).unwrap();
        assert_eq!(d.get(page).unwrap().children, vec![id]);
        h.redo(&mut d).unwrap();
        assert!(d.get(id).is_none());
    }

    #[test]
    fn undo_redo_flows() {
        let mut d = Document::new("t");
        let page = d.add_page("P");
        let fid = d.alloc_id();
        let f = d.insert(page, Node::frame(fid, Rect::new(0.0, 0.0, 10.0, 10.0))).unwrap();
        let flow = Flow { name: "Flow 1".into(), page, start: f, description: String::new() };
        let mut h = History::default();
        h.exec(&mut d, Command::SetFlows { flows: vec![flow.clone()] }).unwrap();
        assert_eq!(d.flows, vec![flow]);
        h.undo(&mut d).unwrap();
        assert!(d.flows.is_empty());
        h.redo(&mut d).unwrap();
        assert_eq!(d.flows.len(), 1);
    }

    #[test]
    fn undo_redo_pages() {
        let mut d = Document::new("t");
        let p1 = d.add_page("P1");
        let pid = d.alloc_id();
        let mut page = Node::new(pid, NodeKind::Page { background: Color::WHITE });
        page.name = "P2".into();
        let mut h = History::default();
        h.exec(&mut d, Command::InsertPage { index: 0, nodes: vec![page] }).unwrap();
        assert_eq!(d.pages, vec![pid, p1]);
        let rid = d.alloc_id();
        d.insert(pid, Node::rectangle(rid, Rect::new(0.0, 0.0, 1.0, 1.0))).unwrap();

        h.exec(&mut d, Command::RemovePage { id: pid }).unwrap();
        assert_eq!(d.pages, vec![p1]);
        assert!(d.get(rid).is_none());
        h.undo(&mut d).unwrap();
        assert_eq!(d.pages, vec![pid, p1]);
        assert_eq!(d.get(pid).unwrap().children, vec![rid]);
        h.undo(&mut d).unwrap();
        assert_eq!(d.pages, vec![p1]);
    }
}

#[cfg(test)]
mod style_tests {
    use crate::*;

    #[test]
    fn set_styles_undo() {
        let mut d = Document::new("t");
        d.add_page("P");
        let mut h = History::default();
        let mut styles = d.styles.clone();
        styles.insert("s1".into(), Style::Fill(vec![Paint::solid(Color::WHITE)]));
        let mut meta = d.style_meta.clone();
        meta.insert("s1".into(), StyleMeta { name: "Brand/White".into(), ..Default::default() });
        h.exec(&mut d, Command::SetStyles { styles, style_meta: meta }).unwrap();
        assert_eq!(d.style_name("s1"), "Brand/White");
        h.undo(&mut d).unwrap();
        assert!(d.styles.is_empty() && d.style_meta.is_empty());
        h.redo(&mut d).unwrap();
        assert_eq!(d.styles.len(), 1);
    }
}
