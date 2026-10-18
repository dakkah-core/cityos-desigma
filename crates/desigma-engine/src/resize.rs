use crate::{edit::Edit, layout};
use desigma_doc::*;
pub fn apply_resize(doc: &mut Document, ed: &mut Edit, id: NodeId, size: Vec2, transform: Affine, relayout: bool) {
    let Some((old, old_transform)) = doc.get(id).map(|n| (n.size, n.transform)) else { return };
    if let Some(n) = ed.node(doc, id) {
        n.size = size;
        n.transform = transform;
        if let NodeKind::Text(t) = &mut n.kind {
            if t.auto_resize == TextAutoResize::WidthAndHeight {
                t.auto_resize = TextAutoResize::None;
            }
        }
    }
    // a table scales its columns and rows with it
    if matches!(doc.get(id).map(|n| &n.kind), Some(NodeKind::Table(_))) {
        crate::table::scale_table(doc, ed, id, size);
        if relayout {
            layout::relayout_up(doc, ed, id);
        }
        return;
    }
    // a section's content stays where it is on the board when the section grows or shrinks
    if matches!(doc.get(id).map(|n| &n.kind), Some(NodeKind::BoardSection(_))) {
        keep_children_in_place(doc, ed, id, old_transform);
    }
    if doc.get(id).map(|n| n.kind.is_container()).unwrap_or(false) {
        layout::apply_constraints(doc, ed, id, old, size);
        layout::layout_node(doc, ed, id);
    }
    if relayout {
        layout::relayout_up(doc, ed, id);
    }
}

/// After the box of `id` moved from `old` (its transform before the edit) to its current transform,
/// re-expresses the children so they keep their place on the board.
fn keep_children_in_place(doc: &mut Document, ed: &mut Edit, id: NodeId, old: Affine) {
    let Some(n) = doc.get(id) else { return };
    let Some(inv) = n.transform.invert() else { return };
    let delta = inv * old;
    if delta.tx.abs() < 1e-4 && delta.ty.abs() < 1e-4 && (delta.a - 1.0).abs() < 1e-5 && (delta.d - 1.0).abs() < 1e-5 {
        return;
    }
    for c in n.children.clone() {
        if let Some(k) = ed.node(doc, c) {
            k.transform = delta * k.transform;
        }
    }
}
