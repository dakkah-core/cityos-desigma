//! Edit sessions: mutate the document directly (for live previews while
//! dragging) while remembering the original state of every touched node, then
//! turn the net change into one undoable `Command::Batch`.

use desigma_doc::{Command, DocError, Document, History, Node, NodeId};
use std::collections::BTreeMap;

#[derive(Default)]
pub struct Edit {
    before: BTreeMap<NodeId, Node>,
}

impl Edit {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records the original state of `id` (first touch wins).
    pub fn touch(&mut self, doc: &Document, id: NodeId) {
        if let std::collections::btree_map::Entry::Vacant(slot) = self.before.entry(id) {
            if let Some(n) = doc.get(id) {
                slot.insert(n.clone());
            }
        }
    }

    /// Ids of every node touched so far.
    pub fn touched(&self) -> Vec<NodeId> {
        self.before.keys().copied().collect()
    }

    /// Mutable access to a node, recording its original state.
    pub fn node<'a>(&mut self, doc: &'a mut Document, id: NodeId) -> Option<&'a mut Node> {
        if doc.cityos.is_some() { return None; }
        self.touch(doc, id);
        doc.get_mut(id)
    }

    /// Puts every touched node back to its original state.
    pub fn revert(&self, doc: &mut Document) {
        for (id, n) in &self.before {
            if let Some(cur) = doc.get_mut(*id) {
                // Structure (parent/children) is owned by structural commands.
                let (parent, children) = (cur.parent, std::mem::take(&mut cur.children));
                *cur = n.clone();
                cur.parent = parent;
                cur.children = children;
            }
        }
    }

    /// Update commands for every touched node whose properties changed.
    fn updates(&self, doc: &Document) -> Vec<Command> {
        let mut out = Vec::new();
        for (id, old) in &self.before {
            if let Some(cur) = doc.get(*id) {
                let mut o = old.clone();
                o.parent = cur.parent;
                o.children = cur.children.clone();
                if *cur != o {
                    out.push(Command::Update { node: Box::new(cur.clone()) });
                }
            }
        }
        out
    }
}

/// A transaction: structural commands applied immediately plus an [`Edit`] for
/// property changes, committed as one undo step.
#[derive(Default)]
pub struct Tx {
    pub ed: Edit,
    cmds: Vec<Command>,
    inverses: Vec<Command>,
}

impl Tx {
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies a structural command now and remembers it for the commit.
    pub fn cmd(&mut self, doc: &mut Document, c: Command) -> Result<(), DocError> {
        let inv = c.clone().apply(doc)?;
        self.cmds.push(c);
        self.inverses.push(inv);
        Ok(())
    }

    /// Restores the document to its state before the transaction began.
    pub fn cancel(mut self, doc: &mut Document) {
        self.ed.revert(doc);
        while let Some(inv) = self.inverses.pop() {
            let _ = inv.apply(doc);
        }
    }

    pub fn commit(mut self, doc: &mut Document, hist: &mut History) -> Result<bool, DocError> {
        // connectors follow the nodes they join (and a moved connector its free ends)
        crate::board::connect::reroute_touched(doc, &mut self.ed);
        let mut updates = self.ed.updates(doc);
        crate::comp_ops::record_overrides(doc, &self.ed.before, &mut updates);
        self.ed.revert(doc);
        while let Some(inv) = self.inverses.pop() {
            inv.apply(doc)?;
        }
        let mut all = std::mem::take(&mut self.cmds);
        all.extend(updates);
        if all.is_empty() {
            return Ok(false);
        }
        let cmd = if all.len() == 1 { all.pop().unwrap() } else { Command::Batch(all) };
        // Edits to a main component also update its instances, in the same undo step.
        let cmd = crate::comp_ops::with_sync(doc, cmd);
        hist.exec(doc, cmd)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use desigma_doc::{Rect, Vec2};

    #[test]
    fn tx_structural_plus_props_undo_together() {
        let mut d = Document::new("t");
        let page = d.add_page("p");
        let a = d.alloc_id();
        d.insert(page, Node::rectangle(a, Rect::new(0.0, 0.0, 10.0, 10.0))).unwrap();
        let mut h = History::default();
        let mut tx = Tx::new();
        let nid = d.alloc_id();
        tx.cmd(&mut d, Command::Insert { parent: page, index: 1, nodes: vec![Node::rectangle(nid, Rect::new(0.0, 0.0, 5.0, 5.0))] }).unwrap();
        tx.ed.node(&mut d, nid).unwrap().size = Vec2::new(50.0, 50.0);
        tx.ed.node(&mut d, a).unwrap().opacity = 0.1;
        assert!(tx.commit(&mut d, &mut h).unwrap());
        assert_eq!(d.get(nid).unwrap().size.x, 50.0);
        assert_eq!(d.get(a).unwrap().opacity, 0.1);
        h.undo(&mut d).unwrap();
        assert!(d.get(nid).is_none());
        assert_eq!(d.get(a).unwrap().opacity, 1.0);
        assert!(!h.can_undo());
        h.redo(&mut d).unwrap();
        assert_eq!(d.get(nid).unwrap().size.x, 50.0);
    }

    #[test]
    fn tx_cancel_restores() {
        let mut d = Document::new("t");
        let page = d.add_page("p");
        let mut tx = Tx::new();
        let nid = d.alloc_id();
        tx.cmd(&mut d, Command::Insert { parent: page, index: 0, nodes: vec![Node::rectangle(nid, Rect::new(0.0, 0.0, 5.0, 5.0))] }).unwrap();
        tx.cancel(&mut d);
        assert!(d.get(nid).is_none());
        assert!(d.get(page).unwrap().children.is_empty());
    }

    #[test]
    fn commit_is_single_undo_step() {
        let mut d = Document::new("t");
        let page = d.add_page("p");
        let a = d.alloc_id();
        d.insert(page, Node::rectangle(a, Rect::new(0.0, 0.0, 10.0, 10.0))).unwrap();
        let b = d.alloc_id();
        d.insert(page, Node::rectangle(b, Rect::new(0.0, 0.0, 10.0, 10.0))).unwrap();
        let mut h = History::default();
        let mut e = Tx::new();
        e.ed.node(&mut d, a).unwrap().opacity = 0.5;
        e.ed.node(&mut d, b).unwrap().opacity = 0.25;
        assert!(e.commit(&mut d, &mut h).unwrap());
        assert_eq!(d.get(a).unwrap().opacity, 0.5);
        h.undo(&mut d).unwrap();
        assert_eq!(d.get(a).unwrap().opacity, 1.0);
        assert_eq!(d.get(b).unwrap().opacity, 1.0);
        assert!(!h.can_undo());
    }

    #[test]
    fn noop_edit_records_nothing() {
        let mut d = Document::new("t");
        let page = d.add_page("p");
        let a = d.alloc_id();
        d.insert(page, Node::rectangle(a, Rect::new(0.0, 0.0, 10.0, 10.0))).unwrap();
        let mut h = History::default();
        let mut e = Tx::new();
        e.ed.node(&mut d, a);
        assert!(!e.commit(&mut d, &mut h).unwrap());
        assert!(!h.can_undo());
    }
}
