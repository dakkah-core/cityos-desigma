use crate::layout;
use desigma_doc::*;
use std::collections::HashSet;
pub fn changed_variables(doc: &Document, store: &VariableStore, removed: &[String]) -> HashSet<String> {
    let mut changed: HashSet<String> = store.variables.iter().filter(|(id, v)| doc.variables.get(*id) != Some(v)).map(|(id, _)| id.clone()).collect();
    for (cid, c) in &store.collections {
        if doc.variable_collections.get(cid).is_some_and(|old| old.modes != c.modes || old.default_mode != c.default_mode) {
            changed.extend(c.variables.iter().cloned());
        }
    }
    loop {
        let more: Vec<String> = store
            .variables
            .iter()
            .filter(|(id, v)| !changed.contains(*id) && v.values.values().any(|x| matches!(x, VariableValue::Alias(a) if changed.contains(a))))
            .map(|(id, _)| id.clone())
            .collect();
        if more.is_empty() {
            break;
        }
        changed.extend(more);
    }
    changed.retain(|id| !removed.contains(id));
    changed
}

/// Nodes with a binding to one of `vars`.
pub fn bound_nodes(doc: &Document, vars: &HashSet<String>) -> Vec<NodeId> {
    if vars.is_empty() {
        return vec![];
    }
    doc.nodes.values().filter(|n| n.variables.bindings.values().any(|v| vars.contains(v))).map(|n| n.id).collect()
}

/// Writes the new values of `vars` into the bound properties of `nodes` (which are already
/// resolved against the new store in `doc`), re-fits auto-sized text and re-runs auto layout.
pub fn reflow_bound(doc: &mut Document, vars: &HashSet<String>, nodes: &[NodeId]) {
    if doc.is_managed() {
        return;
    }
    for id in nodes {
        let binds: Vec<(String, String)> = match doc.get(*id) {
            Some(n) => n.variables.bindings.iter().filter(|(_, v)| vars.contains(*v)).map(|(k, v)| (k.clone(), v.clone())).collect(),
            None => continue,
        };
        for (key, var) in binds {
            if let Some(val) = doc.resolve_variable(*id, &var) {
                if let Some(n) = doc.get_mut(*id) {
                    set_bound_property(n, &key, &val);
                }
            }
        }
        let fitted = doc.get(*id).and_then(|n| match &resolve_node(doc, n).kind {
            NodeKind::Text(t) => Some(layout::fit_text(t, n.size)),
            _ => None,
        });
        if let (Some(size), Some(n)) = (fitted, doc.get_mut(*id)) {
            n.size = size;
        }
    }
    let mut ed = crate::edit::Edit::new();
    for id in nodes {
        layout::relayout_around(doc, &mut ed, *id);
    }
}
