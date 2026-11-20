//! Host side: the single authority that validates, orders and broadcasts every change.

use crate::entries::entries_now;
use crate::ops::{apply, doc_fields, LWW_FIELDS, RESERVED};
use crate::{diff, ClientMsg, Dest, Fix, Op, Out, PeerId, Role, ServerMsg};
use desigma_doc::{Document, NodeId, PEER_ID_BITS};
use std::collections::{BTreeMap, BTreeSet};

/// The host's sequencer. The host app edits its own [`Document`] directly (it is the
/// authoritative copy); the session keeps a shadow of the last state it numbered.
pub struct HostSession {
    shadow: Document,
    seq: u64,
    peers: BTreeMap<PeerId, Role>,
    /// Resume secret of every guest that ever joined (and was not forgotten), connected or not.
    tokens: BTreeMap<PeerId, String>,
    /// Role a disconnected guest had, restored when it resumes.
    departed: BTreeMap<PeerId, Role>,
    next_peer: PeerId,
    /// Images a guest announced and the host asked for.
    awaiting: BTreeSet<(PeerId, String)>,
}

impl HostSession {
    /// Starts a session on `doc`; the host allocates ids in range 0.
    pub fn new(doc: &mut Document) -> Self {
        doc.set_id_peer(0);
        Self {
            shadow: doc.clone_without_images(),
            seq: 0,
            peers: BTreeMap::new(),
            tokens: BTreeMap::new(),
            departed: BTreeMap::new(),
            next_peer: 1,
            awaiting: BTreeSet::new(),
        }
    }

    /// Current global sequence number.
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// Registers a joining guest and assigns its peer index.
    pub fn add_peer(&mut self, role: Role) -> PeerId {
        let p = self.next_peer;
        self.next_peer += 1;
        self.peers.insert(p, role);
        self.tokens.insert(p, new_token());
        p
    }

    /// A guest's connection ended. Its peer index (and id range) stays reserved for
    /// [`Self::resume`] until the session is over, or [`Self::forget_peer`].
    pub fn remove_peer(&mut self, peer: PeerId) {
        if let Some(role) = self.peers.remove(&peer) {
            self.departed.insert(peer, role);
        }
        self.awaiting.retain(|(p, _)| *p != peer);
    }

    /// Like [`Self::remove_peer`], but the guest may not come back (kicked).
    pub fn forget_peer(&mut self, peer: PeerId) {
        self.remove_peer(peer);
        self.tokens.remove(&peer);
        self.departed.remove(&peer);
    }

    /// The resume secret of `peer`.
    pub fn token(&self, peer: PeerId) -> Option<&str> {
        self.tokens.get(&peer).map(String::as_str)
    }

    /// The role of a connected guest.
    pub fn role(&self, peer: PeerId) -> Option<Role> {
        self.peers.get(&peer).copied()
    }

    /// Message that gives a guest its resume secret. Send it after the snapshot.
    pub fn resume_msg(&self, peer: PeerId) -> Option<ServerMsg> {
        self.tokens.get(&peer).map(|t| ServerMsg::Resume { token: t.clone() })
    }

    /// A guest that lost its connection comes back with the secret it was given: it gets its
    /// old peer index and id range again. The caller then sends a fresh [`Self::snapshot`] and
    /// [`Self::resume_msg`]. The guest keeps the role it had; `role` is for one that never had
    /// one. `None` if the secret is unknown. The old connection may still look
    /// alive to the transport; the caller drops it.
    pub fn resume(&mut self, token: &str, role: Role) -> Option<PeerId> {
        let peer = self.tokens.iter().find(|(_, t)| same(t, token)).map(|(p, _)| *p)?;
        self.awaiting.retain(|(p, _)| *p != peer);
        let role = self.departed.remove(&peer).or_else(|| self.peers.get(&peer).copied()).unwrap_or(role);
        self.peers.insert(peer, role);
        Some(peer)
    }

    pub fn set_role(&mut self, peer: PeerId, role: Role) {
        if let Some(r) = self.peers.get_mut(&peer) {
            *r = role;
        }
    }

    /// Join message for `peer`: the document as of [`Self::seq`], without image bytes. Deliver it
    /// first, then every later message (a guest must not miss one in between).
    pub fn snapshot(&self, peer: PeerId) -> ServerMsg {
        ServerMsg::Snapshot { doc: Box::new(self.shadow.clone()), seq: self.seq, peer }
    }

    /// Numbers the host's own edits: diffs `doc` against the shadow and broadcasts the result.
    /// Call it after every frame in which the document changed.
    pub fn local_changes(&mut self, doc: &Document) -> Vec<Out> {
        let ops = diff(&self.shadow, doc);
        if ops.is_empty() {
            return Vec::new();
        }
        if ops.iter().any(|op| apply(&mut self.shadow, op).is_err()) {
            self.shadow = doc.clone_without_images();
        }
        self.seq += 1;
        vec![Out { to: Dest::All, msg: ServerMsg::Ops { seq: self.seq, from: 0, ops } }]
    }

    /// Handles a guest message against the authoritative `doc`.
    pub fn receive(&mut self, doc: &mut Document, peer: PeerId, msg: ClientMsg) -> Vec<Out> {
        let Some(role) = self.peers.get(&peer).copied() else { return Vec::new() };
        // Host edits not yet numbered come first, so they are ordered before this batch.
        let mut outs = self.local_changes(doc);
        match msg {
            ClientMsg::Ops { seq, ops } => self.handle_ops(doc, peer, role, seq, ops, &mut outs),
            ClientMsg::Fetch { ids, deleting } => {
                let mut dead = BTreeSet::new();
                for root in deleting {
                    if doc.nodes.contains_key(&root) {
                        doc.walk(root, |n, _| {
                            dead.insert(n.id);
                        });
                    }
                }
                let mut fixes = Vec::new();
                let mut orders = Vec::new();
                for id in ids {
                    if doc.nodes.contains_key(&id) && !dead.contains(&id) {
                        if let Some(p) = doc.nodes[&id].parent.and_then(|p| doc.nodes.get(&p)) {
                            orders.push((p.id, p.children.clone()));
                        }
                        doc.walk(id, |n, _| {
                            if !dead.contains(&n.id) {
                                fixes.push(node_fix(doc, n.id));
                            }
                        });
                    } else {
                        fixes.push(Fix::Gone(id));
                    }
                }
                outs.push(Out { to: Dest::Peer(peer), msg: ServerMsg::Fetched { fixes, orders } });
            }
            ClientMsg::ImageRequest { hash } => {
                if let Some(bytes) = doc.images.get(&hash).filter(|b| !b.is_empty()) {
                    outs.push(Out { to: Dest::Peer(peer), msg: ServerMsg::ImageData { hash, bytes: bytes.clone() } });
                }
            }
            ClientMsg::ImageData { hash, bytes } => {
                if role == Role::Editor && self.awaiting.remove(&(peer, hash.clone())) {
                    doc.images.insert(hash.clone(), bytes);
                    self.shadow.images.insert(hash.clone(), Vec::new());
                    self.seq += 1;
                    outs.push(Out { to: Dest::AllBut(peer), msg: ServerMsg::Ops { seq: self.seq, from: peer, ops: vec![Op::Image { hash }] } });
                }
            }
        }
        outs
    }

    fn handle_ops(&mut self, doc: &mut Document, peer: PeerId, role: Role, client_seq: u64, ops: Vec<Op>, outs: &mut Vec<Out>) {
        let mut accepted = Vec::new();
        let mut rejected = Vec::new();
        let mut fixes = Vec::new();
        let mut bad = Vec::new();
        let mut touched = BTreeSet::new();
        let mut doc_keys = BTreeSet::new();
        let mut entry_keys = BTreeSet::new();
        for (i, mut op) in ops.into_iter().enumerate() {
            if role == Role::Viewer {
                rejected.push(i);
                self.viewer_fixes(doc, &op, &mut fixes);
                continue;
            }
            match &mut op {
                Op::Image { hash } => {
                    if doc.images.get(hash).is_none_or(|b| b.is_empty()) && self.awaiting.insert((peer, hash.clone())) {
                        outs.push(Out { to: Dest::Peer(peer), msg: ServerMsg::ImageRequest { hash: hash.clone() } });
                    }
                    continue;
                }
                Op::Patch { fields, .. } => fields.retain(|k, _| !RESERVED.contains(&k.as_str())),
                _ => {}
            }
            let in_range = match &op {
                Op::Create { node, .. } => node.id.0 >> PEER_ID_BITS == u64::from(peer),
                _ => true,
            };
            let old_parent = match &op {
                Op::Move { id, .. } | Op::Delete { id } => doc.nodes.get(id).and_then(|n| n.parent),
                _ => None,
            };
            if in_range && apply(doc, &op).is_ok() {
                touched.extend(old_parent);
                if let Op::DocPatch { fields } = &op {
                    doc_keys.extend(fields.keys().cloned());
                }
                if let Op::DocEntry { field, key, .. } = &op {
                    entry_keys.insert((field.clone(), key.clone()));
                }
                if let Op::Create { parent: Some(p), .. } | Op::Move { parent: p, .. } = &op {
                    touched.insert(*p);
                }
                let _ = apply(&mut self.shadow, &op);
                accepted.push(op);
            } else {
                rejected.push(i);
                bad.push(op);
            }
        }
        if !accepted.is_empty() {
            self.seq += 1;
            outs.push(Out { to: Dest::AllBut(peer), msg: ServerMsg::Ops { seq: self.seq, from: peer, ops: accepted } });
        }
        // Corrections describe the state after the whole batch, so later ops of the batch show.
        for op in &bad {
            self.reject_fix(doc, op, &mut fixes);
        }
        let orders = touched.into_iter().filter_map(|p| doc.nodes.get(&p).map(|n| (p, n.children.clone()))).collect();
        let now = if doc_keys.is_empty() { BTreeMap::new() } else { doc_fields(doc) };
        let doc_vals = now.into_iter().filter(|(k, _)| doc_keys.contains(k)).collect();
        let entries = entries_now(doc, &entry_keys);
        outs.push(Out { to: Dest::Peer(peer), msg: ServerMsg::Ack { client_seq, seq: self.seq, rejected, orders, doc: doc_vals, entries } });
        if !fixes.is_empty() {
            outs.push(Out { to: Dest::Peer(peer), msg: ServerMsg::Correction { client_seq, fixes } });
        }
    }

    fn reject_fix(&self, doc: &Document, op: &Op, fixes: &mut Vec<Fix>) {
        match op {
            Op::Create { node, .. } => fixes.push(Fix::Gone(node.id)),
            Op::Patch { id, .. } => fixes.push(node_fix(doc, *id)),
            // The whole subtree: the guest may have deleted it together with a target the host
            // already deleted, and needs all of it back.
            Op::Move { id, .. } => {
                if doc.nodes.contains_key(id) {
                    doc.walk(*id, |n, _| fixes.push(node_fix(doc, n.id)));
                } else {
                    fixes.push(Fix::Gone(*id));
                }
            }
            Op::DocPatch { fields } => {
                let now = doc_fields(doc);
                fixes.push(Fix::Doc(
                    fields.keys().filter(|k| LWW_FIELDS.contains(&k.as_str())).filter_map(|k| now.get(k).map(|v| (k.clone(), v.clone()))).collect(),
                ));
            }
            Op::DocEntry { field, key, .. } => fixes.push(Fix::Entries(entries_now(doc, [&(field.clone(), key.clone())]))),
            Op::Delete { .. } | Op::Image { .. } => {}
        }
    }

    fn viewer_fixes(&self, doc: &Document, op: &Op, fixes: &mut Vec<Fix>) {
        if let Op::Delete { id } = op {
            doc.walk(*id, |n, _| fixes.push(node_fix(doc, n.id)));
        } else {
            self.reject_fix(doc, op, fixes);
        }
    }
}

fn new_token() -> String {
    format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple())
}

/// Comparison that takes the same time wherever the strings differ.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

/// The host's current state of node `id`, or `Gone`.
fn node_fix(doc: &Document, id: NodeId) -> Fix {
    let Some(n) = doc.nodes.get(&id) else { return Fix::Gone(id) };
    let after =
        n.parent.and_then(|p| doc.nodes.get(&p)).and_then(|p| p.children.iter().position(|c| *c == id).and_then(|i| i.checked_sub(1)).map(|i| p.children[i]));
    let mut node = n.clone();
    node.children.clear();
    Fix::Node { node: Box::new(node), after }
}
