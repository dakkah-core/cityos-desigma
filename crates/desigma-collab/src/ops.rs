//! Ops, and how they are applied to a document.

use crate::entries::set_entry;
use desigma_doc::{Document, Node, NodeId, NodeKind};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// One change to the document. A batch of ops applies in order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Op {
    /// New node with all its own fields (its `children` are empty; each child has its own
    /// `Create`). Placed after sibling `after`, or first when `None`. `parent` is `None` only
    /// for a page, whose position is the `pages` field of a [`Op::DocPatch`].
    Create { node: Box<Node>, parent: Option<NodeId>, after: Option<NodeId> },
    /// Changed top-level `Node` fields, as field name to JSON value. `null` means the field is
    /// absent (back to its default).
    Patch { id: NodeId, fields: BTreeMap<String, Value> },
    /// Reparent or reorder. An unknown `after` sibling means last, `None` means first.
    Move { id: NodeId, parent: NodeId, after: Option<NodeId> },
    /// Remove a node and its subtree.
    Delete { id: NodeId },
    /// Changed whole-value document fields ([`LWW_FIELDS`]): last writer wins as a unit.
    DocPatch { fields: BTreeMap<String, Value> },
    /// One entry of a keyed document collection ([`crate::ENTRY_FIELDS`]): set it to `value`, or
    /// remove it when `None`. Entries merge independently, so two people adding or changing
    /// different ones both win. `key` is the path to the entry (see `entries.rs`).
    DocEntry { field: String, key: Vec<String>, value: Option<Value> },
    /// An embedded image with this content hash exists; its bytes are fetched separately.
    Image { hash: String },
}

/// Why an op was rejected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApplyError {
    /// `Create` of an id that already exists.
    Exists(NodeId),
    /// `Move` of a node that is gone (someone deleted it first). The mover is told to drop it too.
    Missing(NodeId),
    /// The parent does not exist.
    NoParent(NodeId),
    /// The parent cannot have children.
    NotContainer(NodeId),
    /// The move would put a node inside itself.
    Cycle(NodeId),
    /// Pages, or nodes with a parent that does not fit their kind, cannot be placed this way.
    BadPlacement(NodeId),
    /// A patch that does not produce a valid node.
    BadPatch(NodeId),
    /// A doc patch with an unknown or malformed field.
    BadDocField(String),
    /// An entry whose parent is gone (a reply to a deleted comment thread, a vote in a session
    /// that no longer exists).
    NoEntryParent(String),
}

impl fmt::Display for ApplyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exists(id) => write!(f, "node {id:?} already exists"),
            Self::Missing(id) => write!(f, "node {id:?} not found"),
            Self::NoParent(id) => write!(f, "parent {id:?} not found"),
            Self::NotContainer(id) => write!(f, "node {id:?} cannot have children"),
            Self::Cycle(id) => write!(f, "cannot move node {id:?} into its own subtree"),
            Self::BadPlacement(id) => write!(f, "node {id:?} cannot be placed there"),
            Self::BadPatch(id) => write!(f, "invalid patch for node {id:?}"),
            Self::BadDocField(name) => write!(f, "invalid document field {name:?}"),
            Self::NoEntryParent(name) => write!(f, "entry of {name:?} has no parent"),
        }
    }
}

impl std::error::Error for ApplyError {}

/// Document-level fields synced by [`Op::DocPatch`] and [`Op::DocEntry`]. Everything in
/// [`Document`] except `nodes`, `images` and the id counters.
pub const DOC_FIELDS: &[&str] = &[
    "name",
    "kind",
    "pages",
    "styles",
    "style_meta",
    "flows",
    "variables",
    "variable_collections",
    "guides",
    "timelines",
    "prototype_device",
    "comments",
    "library",
    "library_page",
    "board_session",
];

/// Fields written whole by [`Op::DocPatch`]. `pages` has its own merge (pages the writer did not
/// know survive); the rest are plain last-writer-wins values.
pub const LWW_FIELDS: &[&str] = &["name", "kind", "pages", "prototype_device", "library", "library_page"];

/// Node fields a patch never touches: identity and placement.
pub(crate) const RESERVED: &[&str] = &["id", "parent", "children"];

/// JSON of every document-level field.
pub fn doc_fields(doc: &Document) -> BTreeMap<String, Value> {
    fn v<T: Serialize>(x: &T) -> Value {
        serde_json::to_value(x).unwrap_or(Value::Null)
    }
    let mut m = BTreeMap::new();
    let mut put = |k: &str, val: Value| {
        m.insert(k.to_string(), val);
    };
    put("name", v(&doc.name));
    put("kind", v(&doc.kind));
    put("pages", v(&doc.pages));
    put("styles", v(&doc.styles));
    put("style_meta", v(&doc.style_meta));
    put("flows", v(&doc.flows));
    put("variables", v(&doc.variables));
    put("variable_collections", v(&doc.variable_collections));
    put("guides", v(&doc.guides));
    put("timelines", v(&doc.timelines));
    put("prototype_device", v(&doc.prototype_device));
    put("comments", v(&doc.comments));
    put("library", v(&doc.library));
    put("library_page", v(&doc.library_page));
    put("board_session", v(&doc.board_session));
    m
}

fn set_field<T: serde::de::DeserializeOwned>(slot: &mut T, name: &str, v: &Value) -> Result<(), ApplyError> {
    *slot = serde_json::from_value(v.clone()).map_err(|_| ApplyError::BadDocField(name.to_string()))?;
    Ok(())
}

fn set_doc_field(doc: &mut Document, name: &str, v: &Value) -> Result<(), ApplyError> {
    match name {
        "name" => set_field(&mut doc.name, name, v),
        "kind" => set_field(&mut doc.kind, name, v),
        "pages" => {
            // Page order is last-writer-wins, but pages the writer did not know about survive.
            let incoming: Vec<NodeId> = serde_json::from_value(v.clone()).map_err(|_| ApplyError::BadDocField(name.to_string()))?;
            let mut seen = BTreeSet::new();
            let mut out: Vec<NodeId> = incoming.into_iter().filter(|id| doc.nodes.get(id).is_some_and(|n| n.parent.is_none()) && seen.insert(*id)).collect();
            out.extend(doc.pages.iter().copied().filter(|id| seen.insert(*id)));
            doc.pages = out;
            Ok(())
        }
        "prototype_device" => set_field(&mut doc.prototype_device, name, v),
        "library" => set_field(&mut doc.library, name, v),
        "library_page" => set_field(&mut doc.library_page, name, v),
        _ => Err(ApplyError::BadDocField(name.to_string())),
    }
}

/// JSON object of a node's own fields (no `id`, `parent`, `children`).
pub(crate) fn node_fields(n: &Node) -> Map<String, Value> {
    let mut m = match serde_json::to_value(n) {
        Ok(Value::Object(m)) => m,
        _ => Map::new(),
    };
    for k in RESERVED {
        m.remove(*k);
    }
    m
}

/// Fields of `new` that differ from `old` (`null` when `new` lacks the field).
pub(crate) fn field_diff(old: &Node, new: &Node) -> BTreeMap<String, Value> {
    let (a, b) = (node_fields(old), node_fields(new));
    let mut out = BTreeMap::new();
    for (k, v) in &b {
        if a.get(k) != Some(v) {
            out.insert(k.clone(), v.clone());
        }
    }
    for k in a.keys() {
        if !b.contains_key(k) {
            out.insert(k.clone(), Value::Null);
        }
    }
    out
}

pub(crate) fn patch_node(node: &mut Node, fields: &BTreeMap<String, Value>) -> Result<(), ApplyError> {
    let bad = || ApplyError::BadPatch(node.id);
    let Value::Object(mut m) = serde_json::to_value(&*node).map_err(|_| bad())? else { return Err(bad()) };
    for (k, v) in fields {
        if RESERVED.contains(&k.as_str()) {
            continue;
        }
        if v.is_null() {
            m.remove(k);
        } else {
            m.insert(k.clone(), v.clone());
        }
    }
    let mut next: Node = serde_json::from_value(Value::Object(m)).map_err(|_| bad())?;
    next.parent = node.parent;
    next.children = std::mem::take(&mut node.children);
    *node = next;
    Ok(())
}

/// Detaches `id` from its parent and inserts it under `parent` after sibling `after`.
pub(crate) fn place(doc: &mut Document, id: NodeId, parent: NodeId, after: Option<NodeId>) {
    if let Some(old) = doc.nodes.get(&id).and_then(|n| n.parent) {
        if let Some(p) = doc.nodes.get_mut(&old) {
            p.children.retain(|c| *c != id);
        }
    }
    let Some(p) = doc.nodes.get_mut(&parent) else { return };
    let at = match after {
        None => 0,
        Some(a) => p.children.iter().position(|c| *c == a).map_or(p.children.len(), |i| i + 1),
    };
    p.children.insert(at, id);
    if let Some(n) = doc.nodes.get_mut(&id) {
        n.parent = Some(parent);
    }
}

/// Reorders the children of `parent` that also appear in `host` into the host's order. Other
/// children (local, not yet acknowledged) keep their slots.
pub(crate) fn apply_order(doc: &mut Document, parent: NodeId, host: &[NodeId]) {
    let Some(p) = doc.nodes.get_mut(&parent) else { return };
    let rank: std::collections::HashMap<NodeId, usize> = host.iter().enumerate().map(|(i, c)| (*c, i)).collect();
    let slots: Vec<usize> = (0..p.children.len()).filter(|i| rank.contains_key(&p.children[*i])).collect();
    let mut members: Vec<NodeId> = slots.iter().map(|i| p.children[*i]).collect();
    members.sort_by_key(|c| rank[c]);
    for (slot, id) in slots.into_iter().zip(members) {
        p.children[slot] = id;
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Host: reject anything invalid.
    Strict,
    /// Guest applying an op the host already accepted. The guest's own unacknowledged edits can
    /// make the local state differ, so conflicts are resolved towards the host instead of failing.
    Remote,
}

/// Applies `op`, rejecting what would break the tree: duplicate ids, a missing or non-container
/// parent, cycles, and moving a node that is gone. An unknown `after` sibling appends; patches of
/// missing nodes are no-ops (delete wins).
pub fn apply(doc: &mut Document, op: &Op) -> Result<(), ApplyError> {
    apply_mode(doc, op, Mode::Strict)
}

pub(crate) fn apply_mode(doc: &mut Document, op: &Op, mode: Mode) -> Result<(), ApplyError> {
    let strict = mode == Mode::Strict;
    match op {
        Op::Create { node, parent, after } => {
            let id = node.id;
            if doc.nodes.contains_key(&id) {
                return if strict { Err(ApplyError::Exists(id)) } else { Ok(()) };
            }
            let is_page = matches!(node.kind, NodeKind::Page { .. });
            match parent {
                None if is_page => {}
                None => return Err(ApplyError::BadPlacement(id)),
                Some(_) if is_page => return Err(ApplyError::BadPlacement(id)),
                Some(p) => match doc.nodes.get(p) {
                    None if strict => return Err(ApplyError::NoParent(*p)),
                    // Its parent is gone here (a local delete not yet acknowledged): so is the node.
                    None => return Ok(()),
                    Some(pn) if strict && !pn.kind.is_container() => return Err(ApplyError::NotContainer(*p)),
                    Some(_) => {}
                },
            }
            let mut n = (**node).clone();
            n.children.clear();
            n.parent = None;
            doc.nodes.insert(id, n);
            if let Some(p) = parent {
                place(doc, id, *p, *after);
            }
            Ok(())
        }
        Op::Patch { id, fields } => match doc.nodes.get_mut(id) {
            Some(n) => patch_node(n, fields),
            None => Ok(()),
        },
        Op::Move { id, parent, after } => {
            let Some(n) = doc.nodes.get(id) else { return if strict { Err(ApplyError::Missing(*id)) } else { Ok(()) } };
            if n.parent.is_none() {
                return Err(ApplyError::BadPlacement(*id));
            }
            if id == parent {
                return if strict { Err(ApplyError::Cycle(*id)) } else { Ok(()) };
            }
            let Some(pn) = doc.nodes.get(parent) else {
                return if strict {
                    Err(ApplyError::NoParent(*parent))
                } else {
                    // The target is gone here (a local delete not yet acknowledged): the host
                    // deletes the moved node together with it.
                    let _ = doc.remove(*id);
                    Ok(())
                };
            };
            if strict && !pn.kind.is_container() {
                return Err(ApplyError::NotContainer(*parent));
            }
            if doc.is_ancestor(*id, *parent) {
                if strict {
                    return Err(ApplyError::Cycle(*id));
                }
                // Local, not yet acknowledged moves made the target a descendant. The host's order
                // wins: take the target out of the way; the host corrects the local move.
                if let Some(up) = doc.nodes.get(id).and_then(|n| n.parent) {
                    place(doc, *parent, up, Some(*id));
                }
            }
            place(doc, *id, *parent, *after);
            Ok(())
        }
        Op::Delete { id } => {
            if doc.nodes.contains_key(id) {
                let _ = doc.remove(*id);
            }
            Ok(())
        }
        Op::DocPatch { fields } => {
            for (k, v) in fields {
                set_doc_field(doc, k, v)?;
            }
            Ok(())
        }
        Op::DocEntry { field, key, value } => set_entry(doc, field, key, value.as_ref(), strict),
        Op::Image { hash } => {
            doc.images.entry(hash.clone()).or_default();
            Ok(())
        }
    }
}
