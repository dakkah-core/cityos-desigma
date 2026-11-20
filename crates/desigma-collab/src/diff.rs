//! Turning the difference between a shadow copy and the current document into ops.

use crate::entries::diff_docs;
use crate::ops::{doc_fields, field_diff, Op, LWW_FIELDS};
use desigma_doc::{Document, NodeId};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// Ops that turn `shadow` into `doc`: `apply`ing them in order to a clone of `shadow` gives a
/// document equal to `doc` (nodes, pages, doc-level fields; images by hash).
///
/// Order: creates (parents first), patches, reparenting moves (never through a transient
/// cycle), sibling-order moves, deletes of subtree roots, doc patch, doc entries, images. Nodes equal by
/// `PartialEq` are skipped without serializing.
pub fn diff(shadow: &Document, doc: &Document) -> Vec<Op> {
    let mut created = BTreeSet::new();
    let mut deleted = BTreeSet::new();
    let mut changed = Vec::new();
    {
        let (mut a, mut b) = (shadow.nodes.iter().peekable(), doc.nodes.iter().peekable());
        loop {
            match (a.peek(), b.peek()) {
                (None, None) => break,
                (Some((ia, _)), None) => {
                    deleted.insert(**ia);
                    a.next();
                }
                (None, Some((ib, _))) => {
                    created.insert(**ib);
                    b.next();
                }
                (Some((ia, na)), Some((ib, nb))) => match ia.cmp(ib) {
                    std::cmp::Ordering::Less => {
                        deleted.insert(**ia);
                        a.next();
                    }
                    std::cmp::Ordering::Greater => {
                        created.insert(**ib);
                        b.next();
                    }
                    std::cmp::Ordering::Equal => {
                        if na != nb {
                            changed.push(**ia);
                        }
                        a.next();
                        b.next();
                    }
                },
            }
        }
    }

    let mut ops = Vec::new();
    let mut s = Scratch { shadow, doc, lists: HashMap::new(), parent_of: HashMap::new() };
    let mut touched: BTreeSet<NodeId> = BTreeSet::new();

    // Creates, parent first.
    let mut order = Vec::with_capacity(created.len());
    let mut stack: Vec<NodeId> = created.iter().rev().copied().filter(|id| doc.nodes[id].parent.is_none_or(|p| !created.contains(&p))).collect();
    while let Some(id) = stack.pop() {
        order.push(id);
        stack.extend(doc.nodes[&id].children.iter().rev().filter(|c| created.contains(c)));
    }
    for id in order {
        let n = &doc.nodes[&id];
        let after = n.parent.and_then(|p| {
            touched.insert(p);
            s.insert_after_anchor(p, id)
        });
        let mut node = n.clone();
        node.children.clear();
        ops.push(Op::Create { node: Box::new(node), parent: n.parent, after });
    }

    // Field patches and the structure bookkeeping of changed nodes.
    let mut reparent = Vec::new();
    for id in changed {
        let (old, new) = (&shadow.nodes[&id], &doc.nodes[&id]);
        let fields = field_diff(old, new);
        if !fields.is_empty() {
            ops.push(Op::Patch { id, fields });
        }
        if old.parent != new.parent {
            if let Some(p) = new.parent {
                reparent.push((id, p));
            }
        }
        if old.children != new.children {
            touched.insert(id);
        }
    }

    // Reparenting moves, ordered so no step puts a node inside its own subtree.
    while !reparent.is_empty() {
        let before = reparent.len();
        let mut rest = Vec::new();
        for (id, np) in std::mem::take(&mut reparent) {
            if s.is_blocked(id, np) {
                rest.push((id, np));
            } else {
                s.emit_reparent(id, np, &mut touched, &mut ops);
            }
        }
        if rest.len() == before {
            for (id, np) in rest {
                s.emit_reparent(id, np, &mut touched, &mut ops);
            }
            break;
        }
        reparent = rest;
    }

    // Sibling order: keep the longest run that is already in order, move the rest.
    for p in touched {
        let Some(pn) = doc.nodes.get(&p) else { continue };
        let target = &pn.children;
        let pos: HashMap<NodeId, usize> = target.iter().enumerate().map(|(i, c)| (*c, i)).collect();
        let cur: Vec<NodeId> = s.list(p).iter().copied().filter(|c| pos.contains_key(c)).collect();
        let idx: Vec<usize> = cur.iter().map(|c| pos[c]).collect();
        if idx.windows(2).all(|w| w[0] < w[1]) && cur.len() == target.len() {
            continue;
        }
        let keep: HashSet<NodeId> = lis(&idx).into_iter().map(|i| cur[i]).collect();
        for (i, c) in target.iter().enumerate() {
            if !keep.contains(c) && doc.nodes.contains_key(c) {
                ops.push(Op::Move { id: *c, parent: p, after: i.checked_sub(1).map(|j| target[j]) });
            }
        }
    }

    // Deletes of subtree roots.
    for id in &deleted {
        if shadow.nodes[id].parent.is_none_or(|p| !deleted.contains(&p)) {
            ops.push(Op::Delete { id: *id });
        }
    }

    let (a, b) = (doc_fields(shadow), doc_fields(doc));
    let fields: BTreeMap<_, _> = b.into_iter().filter(|(k, v)| LWW_FIELDS.contains(&k.as_str()) && a.get(k) != Some(v)).collect();
    if !fields.is_empty() {
        ops.push(Op::DocPatch { fields });
    }
    diff_docs(shadow, doc, &mut ops);
    for hash in doc.images.keys() {
        if !shadow.images.contains_key(hash) {
            ops.push(Op::Image { hash: hash.clone() });
        }
    }
    ops
}

/// Scratch copy of the child lists the ops built so far would produce, loaded lazily.
struct Scratch<'a> {
    shadow: &'a Document,
    doc: &'a Document,
    lists: HashMap<NodeId, Vec<NodeId>>,
    /// Parent after the moves emitted so far.
    parent_of: HashMap<NodeId, Option<NodeId>>,
}

impl Scratch<'_> {
    fn list(&mut self, p: NodeId) -> &mut Vec<NodeId> {
        let shadow = self.shadow;
        self.lists.entry(p).or_insert_with(|| shadow.nodes.get(&p).map(|n| n.children.clone()).unwrap_or_default())
    }

    fn parent(&self, id: NodeId) -> Option<NodeId> {
        match self.parent_of.get(&id) {
            Some(p) => *p,
            None => self.shadow.nodes.get(&id).and_then(|n| n.parent),
        }
    }

    /// Whether moving `id` under `np` now would create a cycle.
    fn is_blocked(&self, id: NodeId, np: NodeId) -> bool {
        let mut cur = Some(np);
        while let Some(c) = cur {
            if c == id {
                return true;
            }
            cur = self.parent(c);
        }
        false
    }

    /// Inserts `id` into scratch list of `p` right after its nearest preceding sibling (in the
    /// target order) that is already there; returns that sibling.
    fn insert_after_anchor(&mut self, p: NodeId, id: NodeId) -> Option<NodeId> {
        let target: &[NodeId] = self.doc.nodes.get(&p).map_or(&[], |n| &n.children);
        let at = target.iter().position(|c| *c == id).unwrap_or(0);
        let list = self.list(p);
        let present: HashSet<NodeId> = list.iter().copied().collect();
        let anchor = target[..at].iter().rev().find(|c| present.contains(c)).copied();
        let i = anchor.and_then(|a| list.iter().position(|c| *c == a)).map_or(0, |i| i + 1);
        list.insert(i, id);
        anchor
    }

    fn emit_reparent(&mut self, id: NodeId, np: NodeId, touched: &mut BTreeSet<NodeId>, ops: &mut Vec<Op>) {
        if let Some(old) = self.parent(id) {
            self.list(old).retain(|c| *c != id);
            touched.insert(old);
        }
        let after = self.insert_after_anchor(np, id);
        self.parent_of.insert(id, Some(np));
        touched.insert(np);
        ops.push(Op::Move { id, parent: np, after });
    }
}

/// Indices of a longest strictly increasing subsequence of `seq`.
fn lis(seq: &[usize]) -> Vec<usize> {
    let mut tails: Vec<usize> = Vec::new();
    let mut prev = vec![usize::MAX; seq.len()];
    for (i, v) in seq.iter().enumerate() {
        let k = tails.partition_point(|t| seq[*t] < *v);
        if k > 0 {
            prev[i] = tails[k - 1];
        }
        if k == tails.len() {
            tails.push(i);
        } else {
            tails[k] = i;
        }
    }
    let mut out = Vec::new();
    let mut cur = tails.last().copied();
    while let Some(i) = cur {
        out.push(i);
        cur = (prev[i] != usize::MAX).then_some(prev[i]);
    }
    out
}
