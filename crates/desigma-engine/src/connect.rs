use crate::{edit::Edit, util::v2};
use desigma_doc::*;
pub fn reroute_touched(doc: &mut Document, ed: &mut Edit) {
    if doc.kind != DocumentKind::Board {
        return;
    }
    let touched = ed.touched();
    // a moved connector drags its free ends with it
    for id in &touched {
        let Some(cur) = doc.get(*id) else { continue };
        let NodeKind::Connector(_) = cur.kind else { continue };
        // a connector that sits where its ends put it has not been moved
        let Some(g) = connector_geometry(doc, *id) else { continue };
        let (dx, dy) = (cur.transform.tx - g.transform.tx, cur.transform.ty - g.transform.ty);
        if dx.abs() < 1e-3 && dy.abs() < 1e-3 {
            continue;
        }
        let Some(n) = ed.node(doc, *id) else { continue };
        if let NodeKind::Connector(c) = &mut n.kind {
            for e in [&mut c.start, &mut c.end] {
                if e.node.is_none() {
                    e.point = v2(e.point.x + dx, e.point.y + dy);
                }
            }
        }
    }
    let mut ids = connectors_attached_to_tree(doc, &touched);
    ids.extend(touched.iter().copied().filter(|t| matches!(doc.get(*t).map(|n| &n.kind), Some(NodeKind::Connector(_)))));
    ids.sort();
    ids.dedup();
    for cid in ids {
        let Some(g) = connector_geometry(doc, cid) else { continue };
        let changed = doc.get(cid).is_some_and(|n| {
            let mut probe = n.clone();
            g.apply(&mut probe);
            probe != *n
        });
        if changed {
            if let Some(n) = ed.node(doc, cid) {
                g.apply(n);
            }
        }
    }
}

/// Connectors with an end on one of `nodes` or on a node inside one of them.
fn connectors_attached_to_tree(doc: &Document, nodes: &[NodeId]) -> Vec<NodeId> {
    let mut all = nodes.to_vec();
    for n in nodes {
        if doc.get(*n).is_some_and(|k| !k.children.is_empty()) {
            doc.walk(*n, |k, _| all.push(k.id));
        }
    }
    all.sort();
    all.dedup();
    connectors_attached_to(doc, &all)
}
