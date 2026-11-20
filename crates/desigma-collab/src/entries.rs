//! Keyed document collections: styles, variables, comments, board votes, flows, guides and
//! timelines merge per entry instead of as one value.
//!
//! Every such field is seen as a flat map from an entry key (a path of strings) to a JSON
//! value. [`entries`] reads it off a document, [`set_entry`] writes one entry, and
//! [`diff_entries`] turns the difference of two documents into [`Op::DocEntry`]s. Two people
//! changing different entries therefore never overwrite each other; the same entry is last
//! writer wins in host order.
//!
//! | field | entry keys |
//! |---|---|
//! | `styles` (with `style_meta`) | `[id]` -> `{style, meta}` |
//! | `variables`, `variable_collections` | `[id]` |
//! | `comments` | `[thread]` -> the thread without its messages, `[thread, message]` -> the message |
//! | `board_session` | `[timer]`, `[vote]` -> the vote settings, `[vote, node, voter]` -> that voter's count on the node |
//! | `flows` | `[start node]` |
//! | `guides`, `timelines` | `[page]` |

use crate::ops::ApplyError;
use crate::Op;
use desigma_doc::{
    Comment, CommentThread, Document, Flow, Guide, NodeId, Style, StyleMeta, TimelineSettings, TimerState, Variable, VariableCollection, VoteSession,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Document fields that travel as [`Op::DocEntry`]. `style_meta` is part of `styles`: a style
/// and its metadata are written together, so a concurrent edit never mixes the two.
pub const ENTRY_FIELDS: &[&str] = &["styles", "variables", "variable_collections", "comments", "board_session", "flows", "guides", "timelines"];

/// The current value of one entry, as the host has it. Carried by acknowledgements and
/// corrections.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub field: String,
    pub key: Vec<String>,
    /// `None`: the entry does not exist.
    pub value: Option<Value>,
}

type EntryMap = BTreeMap<Vec<String>, Value>;

fn to_value<T: Serialize>(x: &T) -> Value {
    serde_json::to_value(x).unwrap_or(Value::Null)
}

/// Every entry of `field` in `doc`, by key. Empty for a field that is not keyed.
pub fn entries(doc: &Document, field: &str) -> BTreeMap<Vec<String>, Value> {
    let mut m = EntryMap::new();
    let k = |s: &str| vec![s.to_string()];
    match field {
        "styles" => {
            for id in doc.styles.keys().chain(doc.style_meta.keys()) {
                let mut o = Map::new();
                if let Some(s) = doc.styles.get(id) {
                    o.insert("style".into(), to_value(s));
                }
                if let Some(s) = doc.style_meta.get(id) {
                    o.insert("meta".into(), to_value(s));
                }
                m.insert(k(id), Value::Object(o));
            }
        }
        "variables" => m.extend(doc.variables.iter().map(|(id, v)| (k(id), to_value(v)))),
        "variable_collections" => m.extend(doc.variable_collections.iter().map(|(id, v)| (k(id), to_value(v)))),
        "comments" => {
            for t in &doc.comments {
                if m.contains_key(&k(&t.id)) {
                    continue;
                }
                let mut head = to_value(t);
                if let Value::Object(o) = &mut head {
                    o.remove("messages");
                }
                m.insert(k(&t.id), head);
                for c in &t.messages {
                    m.entry(vec![t.id.clone(), c.id.clone()]).or_insert_with(|| to_value(c));
                }
            }
        }
        "board_session" => {
            if let Some(t) = &doc.board_session.timer {
                m.insert(k("timer"), to_value(t));
            }
            if let Some(v) = &doc.board_session.vote {
                m.insert(k("vote"), json!({ "votes_per_person": v.votes_per_person, "ended": v.ended }));
                for (node, by) in &v.votes {
                    for (voter, n) in by.iter().filter(|(_, n)| **n > 0) {
                        m.insert(vec!["vote".into(), node.0.to_string(), voter.clone()], json!(n));
                    }
                }
            }
        }
        "flows" => {
            for f in &doc.flows {
                m.entry(k(&f.start.0.to_string())).or_insert_with(|| to_value(f));
            }
        }
        "guides" => m.extend(doc.guides.iter().filter(|(_, g)| !g.is_empty()).map(|(p, g)| (k(&p.0.to_string()), to_value(g)))),
        "timelines" => m.extend(doc.timelines.iter().map(|(p, t)| (k(&p.0.to_string()), to_value(t)))),
        _ => {}
    }
    m
}

/// The ops that turn `old` into `new` in `field`: removals first (an entry under a removed
/// parent goes with it), then sets, parents before children.
pub(crate) fn diff_entries(field: &str, old: &EntryMap, new: &EntryMap, ops: &mut Vec<Op>) {
    let gone: BTreeSet<Vec<String>> = old.keys().filter(|k| !new.contains_key(*k)).cloned().collect();
    for key in &gone {
        if key.len() < 2 || !gone.contains(&key[..key.len() - 1]) {
            ops.push(Op::DocEntry { field: field.to_string(), key: key.clone(), value: None });
        }
    }
    for (key, v) in new {
        if old.get(key) != Some(v) {
            ops.push(Op::DocEntry { field: field.to_string(), key: key.clone(), value: Some(v.clone()) });
        }
    }
}

/// The ops for every keyed field that differs between `shadow` and `doc`.
pub(crate) fn diff_docs(shadow: &Document, doc: &Document, ops: &mut Vec<Op>) {
    for field in ENTRY_FIELDS {
        if same_field(shadow, doc, field) {
            continue;
        }
        diff_entries(field, &entries(shadow, field), &entries(doc, field), ops);
    }
}

/// The current state of the entries `keys` (of any fields) in `doc`.
pub(crate) fn entries_now<'a>(doc: &Document, keys: impl IntoIterator<Item = &'a (String, Vec<String>)>) -> Vec<Entry> {
    let mut cache: BTreeMap<&str, EntryMap> = BTreeMap::new();
    keys.into_iter()
        .map(|(field, key)| {
            let all = cache.entry(field.as_str()).or_insert_with(|| entries(doc, field));
            Entry { field: field.clone(), key: key.clone(), value: all.get(key).cloned() }
        })
        .collect()
}

/// Whether `field` is equal in both documents. Cheap, so the usual diff (a few fields changed)
/// does not serialize the rest.
fn same_field(a: &Document, b: &Document, field: &str) -> bool {
    match field {
        "styles" => {
            a.style_meta == b.style_meta
                && a.styles.len() == b.styles.len()
                && a.styles.iter().zip(&b.styles).all(|((ka, va), (kb, vb))| ka == kb && to_value(va) == to_value(vb))
        }
        "variables" => a.variables == b.variables,
        "variable_collections" => a.variable_collections == b.variable_collections,
        "comments" => a.comments == b.comments,
        "board_session" => a.board_session == b.board_session,
        "flows" => a.flows == b.flows,
        "guides" => a.guides == b.guides,
        "timelines" => a.timelines == b.timelines,
        _ => false,
    }
}

/// Stable text for a field and key, for bookkeeping of pending writes.
pub(crate) fn entry_id(field: &str, key: &[String]) -> String {
    let mut s = field.to_string();
    for part in key {
        s.push('\u{1f}');
        s.push_str(part);
    }
    s
}

fn bad(field: &str) -> ApplyError {
    ApplyError::BadDocField(field.to_string())
}

fn parse<T: DeserializeOwned>(field: &str, v: &Value) -> Result<T, ApplyError> {
    serde_json::from_value(v.clone()).map_err(|_| bad(field))
}

fn node_key(field: &str, key: &str) -> Result<NodeId, ApplyError> {
    key.parse().map(NodeId).map_err(|_| bad(field))
}

/// Sets (`Some`) or removes (`None`) one entry. A missing parent (a reply to a deleted thread, a
/// vote in a session that ended up gone) is an error when `strict`, otherwise the write is
/// dropped: the parent's removal wins. Removing what does not exist is fine.
pub(crate) fn set_entry(doc: &mut Document, field: &str, key: &[String], value: Option<&Value>, strict: bool) -> Result<(), ApplyError> {
    let orphan = || if strict { Err(ApplyError::NoEntryParent(field.to_string())) } else { Ok(()) };
    match (field, key) {
        ("styles", [id]) => match value {
            None => {
                doc.styles.remove(id);
                doc.style_meta.remove(id);
            }
            Some(v) => {
                let (style, meta) = (v.get("style"), v.get("meta"));
                match style {
                    Some(s) => {
                        doc.styles.insert(id.clone(), parse::<Style>(field, s)?);
                    }
                    None => {
                        doc.styles.remove(id);
                    }
                }
                match meta {
                    Some(m) => {
                        doc.style_meta.insert(id.clone(), parse::<StyleMeta>(field, m)?);
                    }
                    None => {
                        doc.style_meta.remove(id);
                    }
                }
            }
        },
        ("variables", [id]) => match value {
            None => {
                doc.variables.remove(id);
            }
            Some(v) => {
                doc.variables.insert(id.clone(), parse::<Variable>(field, v)?);
            }
        },
        ("variable_collections", [id]) => match value {
            None => {
                doc.variable_collections.remove(id);
            }
            Some(v) => {
                let mut c = parse::<VariableCollection>(field, v)?;
                // Variables two people added to the same collection at once: each wrote a list
                // with only its own. Keep the ones the list now lacks and that still exist.
                if let Some(old) = doc.variable_collections.get(id) {
                    let extra: Vec<String> = old
                        .variables
                        .iter()
                        .filter(|x| !c.variables.contains(x) && doc.variables.get(*x).is_some_and(|v| v.collection == *id))
                        .cloned()
                        .collect();
                    c.variables.extend(extra);
                }
                doc.variable_collections.insert(id.clone(), c);
            }
        },
        ("comments", [tid]) => match value {
            None => doc.comments.retain(|t| t.id != *tid),
            Some(v) => {
                let mut o = v.as_object().cloned().ok_or_else(|| bad(field))?;
                o.insert("id".into(), json!(tid));
                o.insert("messages".into(), json!([]));
                let head: CommentThread = parse(field, &Value::Object(o))?;
                match doc.comments.iter_mut().find(|t| t.id == *tid) {
                    Some(t) => {
                        t.anchor = head.anchor;
                        t.resolved = head.resolved;
                    }
                    None => doc.comments.push(head),
                }
                sort_comments(doc);
            }
        },
        ("comments", [tid, mid]) => {
            let Some(t) = doc.comments.iter_mut().find(|t| t.id == *tid) else { return if value.is_none() { Ok(()) } else { orphan() } };
            match value {
                None => t.messages.retain(|c| c.id != *mid),
                Some(v) => {
                    let mut c: Comment = parse(field, v)?;
                    c.id = mid.clone();
                    match t.messages.iter_mut().find(|m| m.id == *mid) {
                        Some(m) => *m = c,
                        None => t.messages.push(c),
                    }
                }
            }
            sort_comments(doc);
        }
        ("board_session", [k]) if k == "timer" => {
            doc.board_session.timer = value.map(|v| parse::<TimerState>(field, v)).transpose()?;
        }
        ("board_session", [k]) if k == "vote" => match value {
            None => doc.board_session.vote = None,
            Some(v) => {
                let per = v.get("votes_per_person").and_then(Value::as_u64).ok_or_else(|| bad(field))? as u32;
                let ended = v.get("ended").and_then(Value::as_bool).ok_or_else(|| bad(field))?;
                let vote = doc.board_session.vote.get_or_insert_with(|| VoteSession::new(1));
                vote.votes_per_person = per;
                vote.ended = ended;
            }
        },
        ("board_session", [k, node, voter]) if k == "vote" => {
            let node = node_key(field, node)?;
            let n = match value {
                None => 0,
                Some(v) => v.as_u64().ok_or_else(|| bad(field))? as u32,
            };
            let Some(vote) = doc.board_session.vote.as_mut() else { return if n == 0 { Ok(()) } else { orphan() } };
            if n == 0 {
                if let Some(by) = vote.votes.get_mut(&node) {
                    by.remove(voter);
                    if by.is_empty() {
                        vote.votes.remove(&node);
                    }
                }
            } else {
                vote.votes.entry(node).or_default().insert(voter.clone(), n);
            }
        }
        ("flows", [start]) => {
            let start = node_key(field, start)?;
            doc.flows.retain(|f| f.start != start);
            if let Some(v) = value {
                let mut f: Flow = parse(field, v)?;
                f.start = start;
                doc.flows.push(f);
            }
            doc.flows.sort_by_key(|f| f.start);
        }
        ("guides", [page]) => {
            let page = node_key(field, page)?;
            let g: Vec<Guide> = value.map(|v| parse(field, v)).transpose()?.unwrap_or_default();
            if g.is_empty() {
                doc.guides.remove(&page);
            } else {
                doc.guides.insert(page, g);
            }
        }
        ("timelines", [page]) => {
            let page = node_key(field, page)?;
            match value {
                None => {
                    doc.timelines.remove(&page);
                }
                Some(v) => {
                    doc.timelines.insert(page, parse::<TimelineSettings>(field, v)?);
                }
            }
        }
        _ => return Err(bad(field)),
    }
    Ok(())
}

/// Threads by when they were opened, messages by when they were written, ties by id, so every
/// peer ends up with the same order whatever order the entries arrived in.
fn sort_comments(doc: &mut Document) {
    for t in &mut doc.comments {
        t.messages.sort_by(|a, b| (&a.created, &a.id).cmp(&(&b.created, &b.id)));
    }
    doc.comments.sort_by(|a, b| (a.messages.first().map(|m| &m.created), &a.id).cmp(&(b.messages.first().map(|m| &m.created), &b.id)));
}
