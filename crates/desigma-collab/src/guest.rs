//! Guest side: sends local changes, applies the host's ops without clobbering unacknowledged
//! local edits.

use crate::entries::{entries, entry_id};
use crate::ops::{apply_mode, apply_order, field_diff, node_fields, Mode};
use crate::{diff, ApplyError, ClientMsg, Entry, Fix, Op, OutMsg, PeerId, ServerMsg};
use desigma_doc::{Document, NodeId};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

/// Pseudo node id for document-level pending fields and entries (see `entry_id`).
const DOC_KEY: NodeId = NodeId(u64::MAX);
/// Pseudo field for a node's place in the tree (parent and sibling order).
const POS: &str = "@pos";

type Key = (NodeId, String);

struct Batch {
    seq: u64,
    ops: Vec<Op>,
    keys: Vec<Key>,
    /// Every node the batch's deletes removed locally, and the delete roots.
    tombs: Vec<NodeId>,
    roots: Vec<NodeId>,
    /// Nodes created or moved by the batch, by op index.
    placed: Vec<(usize, NodeId, Option<NodeId>, Option<NodeId>)>,
    /// Patches in the batch, by op index.
    patches: Vec<(usize, NodeId, BTreeMap<String, Value>)>,
}

/// What [`GuestSession::receive`] did.
#[derive(Debug, Default)]
pub struct Received {
    /// Messages to send back (image requests and answers).
    pub out: Vec<ClientMsg>,
    /// The remote ops that actually changed the document, with held-back fields removed. Feed
    /// these to [`crate::rebase_history`].
    pub applied: Vec<Op>,
    /// Whether the document may have changed.
    pub changed: bool,
}

/// A guest's view of the sync state. The caller owns the [`Document`]; the session owns the
/// shadow of what has been sent or received.
///
/// Invariant: after every call the shadow equals the document, except for edits made to the
/// document since the last [`Self::local_changes`] / [`Self::receive`].
pub struct GuestSession {
    peer: PeerId,
    /// Secret for [`HostSession::resume`](crate::HostSession::resume), once the host sent it.
    token: Option<String>,
    shadow: Document,
    /// Exact replica of the host's state as of `seq`: only host-ordered ops are applied to it.
    /// Whenever nothing is in flight the document is brought back to it, which settles any
    /// divergence the optimistic local state picked up.
    base: Document,
    /// Highest global sequence number seen.
    seq: u64,
    next_client_seq: u64,
    /// Changes captured but not yet sent.
    open: Vec<Op>,
    open_keys: Vec<Key>,
    open_tombs: Vec<NodeId>,
    open_roots: Vec<NodeId>,
    open_placed: Vec<(usize, NodeId, Option<NodeId>, Option<NodeId>)>,
    open_patches: Vec<(usize, NodeId, BTreeMap<String, Value>)>,
    inflight: VecDeque<Batch>,
    /// Number of unacknowledged local writes per (node, field).
    pending: HashMap<Key, u32>,
    /// Nodes removed by local deletes the host has not acknowledged, with a count.
    tombstones: HashMap<NodeId, u32>,
    requested: HashSet<String>,
}

impl GuestSession {
    /// Starts from the host's snapshot. Returns the session, the document to edit (its ids come
    /// from this guest's range) and the image requests to send.
    pub fn join(snapshot: ServerMsg) -> Option<(Self, Document, Vec<ClientMsg>)> {
        let ServerMsg::Snapshot { doc, seq, peer } = snapshot else { return None };
        let mut doc = *doc;
        doc.set_id_peer(peer);
        let missing: Vec<String> = doc.images.iter().filter(|(_, b)| b.is_empty()).map(|(h, _)| h.clone()).collect();
        let s = Self {
            peer,
            token: None,
            shadow: doc.clone_without_images(),
            base: doc.clone_without_images(),
            seq,
            next_client_seq: 1,
            open: Vec::new(),
            open_keys: Vec::new(),
            open_tombs: Vec::new(),
            open_roots: Vec::new(),
            open_placed: Vec::new(),
            open_patches: Vec::new(),
            inflight: VecDeque::new(),
            pending: HashMap::new(),
            tombstones: HashMap::new(),
            requested: missing.iter().cloned().collect(),
        };
        let reqs = missing.into_iter().map(|hash| ClientMsg::ImageRequest { hash }).collect();
        Some((s, doc, reqs))
    }

    pub fn peer(&self) -> PeerId {
        self.peer
    }

    /// The secret to rejoin as this peer after the connection was lost; `None` until the host
    /// has sent it (right after the snapshot).
    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    /// Continues the session on a new connection after the old one was lost. `snapshot` is the
    /// host's fresh [`ServerMsg::Snapshot`] for the same peer (see
    /// [`HostSession::resume`](crate::HostSession::resume)); `None` if it is another peer (the
    /// secret was not known) or not a snapshot.
    ///
    /// Local work the host never confirmed is the difference between the last state the host is
    /// known to have (`base`) and `doc`. It is laid over the new snapshot with the usual rules
    /// (per field, the local write is the later one; deleted nodes stay deleted) and `doc` is
    /// brought to that result in place. The next [`Self::local_changes`] then sends exactly
    /// what the host is still missing. Image requests for what is missing come back in `out`.
    pub fn rejoin(&mut self, doc: &mut Document, snapshot: ServerMsg) -> Option<Received> {
        let ServerMsg::Snapshot { doc: snap, seq, peer } = snapshot else { return None };
        if peer != self.peer {
            return None;
        }
        let snap = *snap;
        let unconfirmed: Vec<Op> = diff(&self.base, doc).into_iter().filter(|op| !matches!(op, Op::Image { .. })).collect();
        let mut target = snap.clone_without_images();
        for op in &unconfirmed {
            let _ = apply_mode(&mut target, op, Mode::Remote);
        }
        let mut r = Received::default();
        for op in diff(doc, &target).into_iter().filter(|op| !matches!(op, Op::Image { .. })) {
            let _ = apply_mode(doc, &op, Mode::Remote);
            r.applied.push(op);
        }
        r.changed = !r.applied.is_empty();
        let missing: Vec<String> = snap.images.keys().filter(|h| doc.images.get(*h).is_none_or(|b| b.is_empty())).cloned().collect();
        for hash in snap.images.keys() {
            doc.images.entry(hash.clone()).or_default();
        }
        self.shadow = snap.clone_without_images();
        self.base = snap.clone_without_images();
        self.seq = seq;
        self.open.clear();
        self.open_keys.clear();
        self.open_tombs.clear();
        self.open_roots.clear();
        self.open_placed.clear();
        self.open_patches.clear();
        self.inflight.clear();
        self.pending.clear();
        self.tombstones.clear();
        self.requested = missing.iter().cloned().collect();
        r.out = missing.into_iter().map(|hash| ClientMsg::ImageRequest { hash }).collect();
        Some(r)
    }

    /// Global sequence number of the last host message handled.
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// Whether local changes are waiting for the host's acknowledgement.
    pub fn has_pending(&self) -> bool {
        !self.inflight.is_empty() || !self.open.is_empty()
    }

    /// Diffs `doc` against the shadow and returns the batch to send, if anything changed.
    /// Call it after every frame in which the document changed.
    pub fn local_changes(&mut self, doc: &Document) -> Option<OutMsg> {
        self.capture(doc);
        if self.open.is_empty() {
            return None;
        }
        let seq = self.next_client_seq;
        self.next_client_seq += 1;
        let ops = std::mem::take(&mut self.open);
        let keys = std::mem::take(&mut self.open_keys);
        let tombs = std::mem::take(&mut self.open_tombs);
        let roots = std::mem::take(&mut self.open_roots);
        let placed = std::mem::take(&mut self.open_placed);
        let patches = std::mem::take(&mut self.open_patches);
        self.inflight.push_back(Batch { seq, ops: ops.clone(), keys, tombs, roots, placed, patches });
        Some(ClientMsg::Ops { seq, ops })
    }

    fn capture(&mut self, doc: &Document) {
        let ops = diff(&self.shadow, doc);
        if ops.is_empty() {
            return;
        }
        let mut ok = true;
        for op in &ops {
            if let Op::Create { node, parent, after } = op {
                self.open_placed.push((self.open.len() + ops_index(&ops, op), node.id, *parent, *after));
            }
            if let Op::Move { id, parent, after } = op {
                self.open_placed.push((self.open.len() + ops_index(&ops, op), *id, Some(*parent), *after));
            }
            if let Op::Patch { id, fields } = op {
                self.open_patches.push((self.open.len() + ops_index(&ops, op), *id, fields.clone()));
            }
            self.register(op);
            if let Op::Delete { id } = op {
                self.open_roots.push(*id);
                let mut dead = Vec::new();
                self.shadow.walk(*id, |n, _| dead.push(n.id));
                for d in dead {
                    *self.tombstones.entry(d).or_insert(0) += 1;
                    self.open_tombs.push(d);
                }
            }
            ok &= apply_mode(&mut self.shadow, op, Mode::Strict).is_ok();
        }
        if !ok {
            self.shadow = doc.clone_without_images();
        }
        self.open.extend(ops);
    }

    fn register(&mut self, op: &Op) {
        let mut keys = Vec::new();
        match op {
            Op::Create { node, .. } => keys.push((node.id, POS.to_string())),
            Op::Move { id, .. } => keys.push((*id, POS.to_string())),
            Op::Patch { id, fields } => keys.extend(fields.keys().map(|k| (*id, k.clone()))),
            Op::DocPatch { fields } => keys.extend(fields.keys().map(|k| (DOC_KEY, k.clone()))),
            Op::DocEntry { field, key, .. } => keys.push((DOC_KEY, entry_id(field, key))),
            Op::Delete { .. } | Op::Image { .. } => {}
        }
        for k in keys {
            *self.pending.entry(k.clone()).or_insert(0) += 1;
            self.open_keys.push(k);
        }
    }

    fn is_pending(&self, id: NodeId, field: &str) -> bool {
        self.pending.contains_key(&(id, field.to_string()))
    }

    /// Handles a host message. Local edits not yet captured are captured first, so they are not
    /// overwritten and are sent by the next [`Self::local_changes`].
    pub fn receive(&mut self, doc: &mut Document, msg: ServerMsg) -> Received {
        self.capture(doc);
        let mut r = Received::default();
        match msg {
            ServerMsg::Snapshot { .. } => {}
            ServerMsg::Resume { token } => self.token = Some(token),
            ServerMsg::Ops { seq, ops, .. } => {
                self.seq = self.seq.max(seq);
                for op in ops {
                    let _ = apply_mode(&mut self.base, &op, Mode::Remote);
                    self.remote(doc, op, &mut r);
                }
            }
            ServerMsg::Ack { client_seq, seq, rejected, orders, doc: fields, entries } => {
                self.seq = self.seq.max(seq);
                self.ack(doc, client_seq, &rejected, &orders, (fields, entries), &mut r);
            }
            ServerMsg::Correction { fixes, .. } => {
                let mut refetch: Vec<NodeId> = Vec::new();
                for fix in fixes {
                    // What a local delete removed is the host's call: ask what survives.
                    if let Fix::Node { node, .. } = &fix {
                        if self.tombstones.contains_key(&node.id) {
                            if !node.parent.is_some_and(|p| refetch.contains(&p)) {
                                refetch.push(node.id);
                            }
                            continue;
                        }
                    }
                    for op in self.fix_ops(doc, fix) {
                        self.remote(doc, op, &mut r);
                    }
                }
                if !refetch.is_empty() {
                    r.out.push(self.fetch(refetch));
                }
            }
            ServerMsg::Fetched { fixes, orders } => {
                for fix in fixes {
                    for op in self.fix_ops(doc, fix) {
                        self.remote(doc, op, &mut r);
                    }
                }
                for (parent, order) in &orders {
                    apply_order(doc, *parent, order);
                    apply_order(&mut self.shadow, *parent, order);
                }
            }
            ServerMsg::ImageRequest { hash } => {
                if let Some(bytes) = doc.images.get(&hash).filter(|b| !b.is_empty()) {
                    r.out.push(ClientMsg::ImageData { hash, bytes: bytes.clone() });
                }
            }
            ServerMsg::ImageData { hash, bytes } => {
                self.requested.remove(&hash);
                self.shadow.images.entry(hash.clone()).or_default();
                doc.images.insert(hash, bytes);
                r.changed = true;
            }
        }
        self.reconcile(doc, &mut r);
        r
    }

    /// With nothing in flight the document must equal the host's state; if the optimistic
    /// handling left a difference, take the host's version.
    fn reconcile(&mut self, doc: &mut Document, r: &mut Received) {
        if self.has_pending() {
            return;
        }
        let ops: Vec<Op> = diff(doc, &self.base).into_iter().filter(|op| !matches!(op, Op::Image { .. })).collect();
        for op in ops {
            let _ = apply_mode(doc, &op, Mode::Remote);
            let _ = apply_mode(&mut self.shadow, &op, Mode::Remote);
            r.changed = true;
            r.applied.push(op);
        }
    }

    /// The host handled batch `client_seq`: those writes are no longer pending. Sibling order of
    /// the parents the batch touched is taken from the host, since remote inserts anchored on
    /// nodes this guest had already moved or deleted locally may have landed elsewhere.
    fn ack(
        &mut self,
        doc: &mut Document,
        client_seq: u64,
        rejected: &[usize],
        orders: &[(NodeId, Vec<NodeId>)],
        (fields, host_entries): (BTreeMap<String, Value>, Vec<Entry>),
        r: &mut Received,
    ) {
        let Some(at) = self.inflight.iter().position(|b| b.seq == client_seq) else { return };
        let Some(batch) = self.inflight.remove(at) else { return };
        for (i, op) in batch.ops.iter().enumerate() {
            if !rejected.contains(&i) {
                let _ = apply_mode(&mut self.base, op, Mode::Remote);
            }
        }
        for t in &batch.tombs {
            if let Some(n) = self.tombstones.get_mut(t) {
                *n -= 1;
                if *n == 0 {
                    self.tombstones.remove(t);
                }
            }
        }
        for k in &batch.keys {
            if let Some(n) = self.pending.get_mut(k) {
                *n -= 1;
                if *n == 0 {
                    self.pending.remove(k);
                }
            }
        }
        // A remote move may have pushed one of our nodes out of the place the host gave it.
        for (i, id, parent, after) in &batch.placed {
            let Some(parent) = parent else { continue };
            if !rejected.contains(i) && !self.is_pending(*id, POS) && doc.nodes.get(id).is_some_and(|n| n.parent != Some(*parent)) {
                let _ = self.apply_both(doc, &Op::Move { id: *id, parent: *parent, after: *after });
                r.changed = true;
            }
        }
        for (parent, order) in orders {
            apply_order(doc, *parent, order);
            apply_order(&mut self.shadow, *parent, order);
            r.changed = true;
        }
        // Accepted edits whose result a remote delete or a correction wiped from the local
        // document (a node removed from under them): the host has them, so fetch.
        let mut lost: Vec<NodeId> = batch
            .placed
            .iter()
            .filter(|(i, id, ..)| !rejected.contains(i) && !doc.nodes.contains_key(id) && !self.tombstones.contains_key(id))
            .map(|(_, id, ..)| *id)
            .collect();
        for (i, id, fields) in &batch.patches {
            if rejected.contains(i) || self.tombstones.contains_key(id) {
                continue;
            }
            let clobbered = match doc.nodes.get(id) {
                None => true,
                Some(n) => {
                    let now = node_fields(n);
                    fields.iter().any(|(k, v)| !self.is_pending(*id, k) && now.get(k).unwrap_or(&Value::Null) != v)
                }
            };
            if clobbered {
                lost.push(*id);
            }
        }
        lost.sort();
        lost.dedup();
        if !lost.is_empty() {
            r.out.push(self.fetch(lost));
        }
        // Fields rewritten by later local batches stay as they are.
        let fields: BTreeMap<String, Value> = fields.into_iter().filter(|(k, _)| !self.is_pending(DOC_KEY, k)).collect();
        if !fields.is_empty() {
            let _ = self.apply_both(doc, &Op::DocPatch { fields });
            r.changed = true;
        }
        // Likewise per entry. Where the host's value is not what we have, it is somebody
        // else's write that landed after ours; let history know, like any remote write.
        let mut mine: BTreeMap<String, BTreeMap<Vec<String>, Value>> = BTreeMap::new();
        let settled: Vec<Entry> = host_entries.into_iter().filter(|e| !self.is_pending(DOC_KEY, &entry_id(&e.field, &e.key))).collect();
        for e in settled {
            let local = mine.entry(e.field.clone()).or_insert_with(|| entries(doc, &e.field));
            if local.get(&e.key) == e.value.as_ref() {
                continue;
            }
            mine.remove(&e.field);
            let op = Op::DocEntry { field: e.field, key: e.key, value: e.value };
            let _ = self.apply_both(doc, &op);
            r.changed = true;
            r.applied.push(op);
        }
    }

    fn apply_both(&mut self, doc: &mut Document, op: &Op) -> Result<(), ApplyError> {
        let _ = apply_mode(&mut self.shadow, op, Mode::Remote);
        apply_mode(doc, op, Mode::Remote)
    }

    /// Applies one op the host decided on, leaving out whatever this guest has pending: a remote
    /// write that arrives while ours is unacknowledged is ordered before ours, so ours wins.
    fn remote(&mut self, doc: &mut Document, op: Op, r: &mut Received) {
        let op = match op {
            Op::Patch { id, mut fields } => {
                fields.retain(|k, _| !self.is_pending(id, k));
                if fields.is_empty() {
                    return;
                }
                Op::Patch { id, fields }
            }
            Op::DocPatch { mut fields } => {
                fields.retain(|k, _| !self.is_pending(DOC_KEY, k));
                if fields.is_empty() {
                    return;
                }
                Op::DocPatch { fields }
            }
            Op::DocEntry { field, key, value } => {
                if self.is_pending(DOC_KEY, &entry_id(&field, &key)) {
                    return;
                }
                Op::DocEntry { field, key, value }
            }
            Op::Move { id, .. } if self.is_pending(id, POS) => return,
            op => op,
        };
        // The host moved a node this guest no longer has: it deleted it locally together with a
        // subtree the node had been moved out of first. The node survives; ask for its state.
        if let Op::Move { id, .. } = &op {
            if !doc.nodes.contains_key(id) && !r.out.iter().any(|m| matches!(m, ClientMsg::Fetch { ids, .. } if ids.contains(id))) {
                r.out.push(self.fetch(vec![*id]));
            }
        }
        let _ = self.apply_both(doc, &op);
        if let Op::Image { hash } = &op {
            if doc.images.get(hash).is_none_or(|b| b.is_empty()) && self.requested.insert(hash.clone()) {
                r.out.push(ClientMsg::ImageRequest { hash: hash.clone() });
            }
        }
        r.changed = true;
        r.applied.push(op);
    }

    fn fetch(&self, ids: Vec<NodeId>) -> ClientMsg {
        let deleting = self.inflight.iter().flat_map(|b| b.roots.iter()).chain(&self.open_roots).copied().collect();
        ClientMsg::Fetch { ids, deleting }
    }

    /// A correction as ops against the local document.
    fn fix_ops(&self, doc: &Document, fix: Fix) -> Vec<Op> {
        match fix {
            Fix::Gone(id) => vec![Op::Delete { id }],
            Fix::Doc(fields) => vec![Op::DocPatch { fields: fields.into_iter().collect() }],
            Fix::Entries(es) => es.into_iter().map(|e| Op::DocEntry { field: e.field, key: e.key, value: e.value }).collect(),
            Fix::Node { node, after } => {
                let id = node.id;
                match doc.nodes.get(&id) {
                    None => vec![Op::Create { parent: node.parent, node, after }],
                    Some(local) => {
                        let mut ops = Vec::new();
                        let fields = field_diff(local, &node);
                        if !fields.is_empty() {
                            ops.push(Op::Patch { id, fields });
                        }
                        if let Some(parent) = node.parent {
                            ops.push(Op::Move { id, parent, after });
                        }
                        ops
                    }
                }
            }
        }
    }
}

fn ops_index(ops: &[Op], op: &Op) -> usize {
    ops.iter().position(|o| std::ptr::eq(o, op)).unwrap_or(0)
}
