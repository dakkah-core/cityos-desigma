//! Canvas hit testing and marquee selection.

use crate::util::world_aabb;
use desigma_doc::{Affine, Document, NodeId, NodeKind, Rect, Vec2};

fn local_hit(kind: &NodeKind, size: Vec2, local: Vec2, tol: f32) -> bool {
    match kind {
        NodeKind::Line => local.x >= -tol && local.x <= size.x + tol && local.y.abs() <= tol.max(size.y / 2.0 + tol),
        NodeKind::Ellipse(_) | NodeKind::Stamp { .. } => {
            let rx = size.x / 2.0 + tol;
            let ry = size.y / 2.0 + tol;
            if rx <= 0.0 || ry <= 0.0 {
                return false;
            }
            let dx = (local.x - size.x / 2.0) / rx;
            let dy = (local.y - size.y / 2.0) / ry;
            dx * dx + dy * dy <= 1.0
        }
        NodeKind::ShapeWithText(s) if s.shape == desigma_doc::ShapeKind::Ellipse => local_hit(&NodeKind::Ellipse(Default::default()), size, local, tol),
        _ => local.x >= -tol && local.y >= -tol && local.x <= size.x + tol && local.y <= size.y + tol,
    }
}

/// Whether `local` (section space) is on the title pill or within a border band of the section.
fn section_hit(n: &desigma_doc::Node, local: Vec2, tol: f32) -> bool {
    let pill = desigma_render::board::section_title_rect(n);
    if pill.x - tol <= local.x && local.x <= pill.x + pill.w + tol && pill.y - tol <= local.y && local.y <= pill.y + pill.h + tol {
        return true;
    }
    let band = tol * 5.0;
    let (w, h) = (n.size.x, n.size.y);
    let inside_outer = local.x >= -tol && local.y >= -tol && local.x <= w + tol && local.y <= h + tol;
    let inside_inner = local.x > band && local.y > band && local.x < w - band && local.y < h - band;
    inside_outer && !inside_inner
}

/// True if the container kind is "transparent" to clicks: clicking a child
/// selects the child directly (frames, sections, pages).
fn selects_through(kind: &NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::Page { .. } | NodeKind::Frame(_) | NodeKind::Section | NodeKind::BoardSection(_) | NodeKind::Component(_) | NodeKind::ComponentSet(_)
    )
}

/// Finds the node under `p` (page space).
///
/// * `deep` selects the deepest node (ctrl/cmd-click).
/// * `entered` lists containers (usually ancestors of the current selection)
///   that groups/instances may be descended into.
pub fn hit_test(doc: &Document, root: NodeId, p: Vec2, tol: f32, deep: bool, entered: &[NodeId]) -> Option<NodeId> {
    hit_test_excluding(doc, root, p, tol, deep, entered, &[])
}

/// [`hit_test`] that ignores the `exclude`d nodes (and everything inside them).
pub fn hit_test_excluding(doc: &Document, root: NodeId, p: Vec2, tol: f32, deep: bool, entered: &[NodeId], exclude: &[NodeId]) -> Option<NodeId> {
    hit_in(doc, root, Affine::IDENTITY, p, tol, deep, entered, exclude)
}

/// Whether `p` (in the connector's parent space) is on the line of connector `id`.
fn connector_hit(doc: &Document, id: NodeId, parent_local: Vec2, tol: f32) -> bool {
    let Some(n) = doc.get(id) else { return false };
    let Some(route) = desigma_doc::connector_route(doc, id) else { return false };
    route.distance_to(parent_local) <= n.stroke.weight / 2.0 + tol * 3.0
}

#[allow(clippy::too_many_arguments)]
fn hit_in(doc: &Document, parent: NodeId, parent_world: Affine, p: Vec2, tol: f32, deep: bool, entered: &[NodeId], exclude: &[NodeId]) -> Option<NodeId> {
    let pn = doc.get(parent)?;
    for &cid in pn.children.iter().rev() {
        let Some(n) = doc.get(cid) else { continue };
        if !n.visible || n.locked || exclude.contains(&cid) {
            continue;
        }
        let world = parent_world * n.transform;
        let Some(inv) = world.invert() else { continue };
        let local = inv.apply(p);
        let inside = match &n.kind {
            // a connector is its line, not its bounding box
            NodeKind::Connector(_) => parent_world.invert().is_some_and(|pi| connector_hit(doc, cid, pi.apply(p), tol)),
            // a section is picked by its title pill and its border; its empty inside is the canvas
            NodeKind::BoardSection(_) => section_hit(n, local, tol),
            // a drawn stroke is its ink, not its bounding box
            NodeKind::Vector { .. } if n.brush.is_some() => n.brush_distance(local).is_some_and(|d| d <= tol),
            k => local_hit(k, n.size, local, tol),
        };
        // a table is one object: its cells are edited, not picked
        let is_container = n.kind.is_container() && !matches!(n.kind, NodeKind::Table(_));
        if let NodeKind::BoardSection(s) = &n.kind {
            if s.contents_hidden {
                if inside {
                    return Some(cid);
                }
                continue;
            }
        }
        if is_container {
            let descend = deep || selects_through(&n.kind) || entered.contains(&cid);
            let clips = n.kind.frame_props().map(|f| f.clips_content).unwrap_or(false);
            if descend && (!clips || inside) {
                if let Some(h) = hit_in(doc, cid, world, p, tol, deep, entered, exclude) {
                    return Some(h);
                }
            }
            if inside {
                return Some(cid);
            }
        } else if inside {
            return Some(cid);
        }
    }
    None
}

/// Nodes selected by a marquee (page space). Fully-enclosed top-level objects
/// are selected as a whole; frames only partly covered are entered.
pub fn marquee_select(doc: &Document, root: NodeId, r: &Rect) -> Vec<NodeId> {
    let mut out = Vec::new();
    marquee_in(doc, root, r, &mut out);
    out
}

fn marquee_in(doc: &Document, parent: NodeId, r: &Rect, out: &mut Vec<NodeId>) {
    let Some(pn) = doc.get(parent) else { return };
    for &cid in &pn.children {
        let Some(n) = doc.get(cid) else { continue };
        if !n.visible || n.locked {
            continue;
        }
        let bb = world_aabb(doc, cid);
        if !bb.intersects(r) {
            continue;
        }
        let contained = bb.x >= r.x && bb.y >= r.y && bb.x + bb.w <= r.x + r.w && bb.y + bb.h <= r.y + r.h;
        let hidden = matches!(&n.kind, NodeKind::BoardSection(s) if s.contents_hidden);
        // a marquee takes a section only when it encloses it; otherwise it picks what is inside
        if matches!(n.kind, NodeKind::BoardSection(_)) && !contained {
            if !hidden {
                marquee_in(doc, cid, r, out);
            }
            continue;
        }
        if n.kind.is_container() && selects_through(&n.kind) && !contained && !n.children.is_empty() && !hidden {
            marquee_in(doc, cid, r, out);
        } else {
            out.push(cid);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::v2;
    use desigma_doc::{FrameProps, Node};

    fn sample() -> (Document, NodeId, NodeId, NodeId, NodeId, NodeId) {
        let mut d = Document::new("t");
        let page = d.add_page("p");
        let fid = d.alloc_id();
        let frame = d.insert(page, Node::frame(fid, Rect::new(0.0, 0.0, 200.0, 200.0))).unwrap();
        let gid = d.alloc_id();
        let mut g = Node::new(gid, NodeKind::Group);
        g.size = v2(50.0, 50.0);
        g.transform = Affine::translate(100.0, 100.0);
        let group = d.insert(frame, g).unwrap();
        let rid = d.alloc_id();
        let rect_in_group = d.insert(group, Node::rectangle(rid, Rect::new(0.0, 0.0, 50.0, 50.0))).unwrap();
        let r2 = d.alloc_id();
        let rect = d.insert(frame, Node::rectangle(r2, Rect::new(10.0, 10.0, 30.0, 30.0))).unwrap();
        (d, page, frame, group, rect_in_group, rect)
    }

    #[test]
    fn click_selects_child_of_top_frame() {
        let (d, page, _frame, _g, _rg, rect) = sample();
        assert_eq!(hit_test(&d, page, v2(20.0, 20.0), 0.0, false, &[]), Some(rect));
    }

    #[test]
    fn click_group_selects_group_not_child() {
        let (d, page, _f, group, rg, _r) = sample();
        assert_eq!(hit_test(&d, page, v2(120.0, 120.0), 0.0, false, &[]), Some(group));
        assert_eq!(hit_test(&d, page, v2(120.0, 120.0), 0.0, true, &[]), Some(rg));
        assert_eq!(hit_test(&d, page, v2(120.0, 120.0), 0.0, false, &[group]), Some(rg));
    }

    #[test]
    fn click_empty_frame_area_selects_frame() {
        let (d, page, frame, ..) = sample();
        assert_eq!(hit_test(&d, page, v2(180.0, 20.0), 0.0, false, &[]), Some(frame));
        assert_eq!(hit_test(&d, page, v2(500.0, 20.0), 0.0, false, &[]), None);
    }

    #[test]
    fn locked_and_hidden_are_skipped() {
        let (mut d, page, _f, _g, _rg, rect) = sample();
        d.get_mut(rect).unwrap().locked = true;
        let f = hit_test(&d, page, v2(20.0, 20.0), 0.0, false, &[]).unwrap();
        assert_ne!(f, rect);
        d.get_mut(rect).unwrap().locked = false;
        d.get_mut(rect).unwrap().visible = false;
        assert_ne!(hit_test(&d, page, v2(20.0, 20.0), 0.0, false, &[]), Some(rect));
    }

    #[test]
    fn ellipse_corner_misses() {
        let mut d = Document::new("t");
        let page = d.add_page("p");
        let id = d.alloc_id();
        let e = d.insert(page, Node::ellipse(id, Rect::new(0.0, 0.0, 100.0, 100.0))).unwrap();
        assert_eq!(hit_test(&d, page, v2(50.0, 50.0), 0.0, false, &[]), Some(e));
        assert_eq!(hit_test(&d, page, v2(2.0, 2.0), 0.0, false, &[]), None);
    }

    #[test]
    fn rotated_node_hit() {
        let mut d = Document::new("t");
        let page = d.add_page("p");
        let id = d.alloc_id();
        let mut n = Node::rectangle(id, Rect::new(0.0, 0.0, 100.0, 10.0));
        n.transform = Affine::translate(50.0, 50.0) * Affine::rotate(std::f32::consts::FRAC_PI_2);
        let r = d.insert(page, n).unwrap();
        // rotated 90deg clockwise: occupies x in [40,50], y in [50,150]
        assert_eq!(hit_test(&d, page, v2(45.0, 100.0), 0.0, false, &[]), Some(r));
        assert_eq!(hit_test(&d, page, v2(100.0, 55.0), 0.0, false, &[]), None);
    }

    #[test]
    fn marquee_enters_partially_covered_frame() {
        let (d, page, frame, _g, _rg, rect) = sample();
        let partial = marquee_select(&d, page, &Rect::new(0.0, 0.0, 60.0, 60.0));
        assert_eq!(partial, vec![rect]);
        let all = marquee_select(&d, page, &Rect::new(-10.0, -10.0, 400.0, 400.0));
        assert_eq!(all, vec![frame]);
    }

    #[test]
    fn a_table_is_one_hit_even_for_a_deep_click() {
        let mut d = Document::new("t");
        let page = d.add_page("p");
        let nodes = desigma_doc::table_nodes(&mut d, v2(0.0, 0.0), 2, 2);
        let id = nodes[0].id;
        desigma_doc::Command::Insert { parent: page, index: 0, nodes }.apply(&mut d).unwrap();
        assert_eq!(hit_test(&d, page, v2(10.0, 10.0), 0.0, true, &[]), Some(id), "a cell is not picked on its own");
        assert_eq!(hit_test(&d, page, v2(300.0, 100.0), 0.0, false, &[]), Some(id));
        assert_eq!(hit_test(&d, page, v2(900.0, 100.0), 0.0, false, &[]), None);
    }

    #[test]
    fn a_section_is_hit_on_its_title_and_border_but_its_children_come_first() {
        let mut d = Document::new("t");
        let page = d.add_page("p");
        let sid = d.alloc_id();
        let sec = d.insert(page, Node::board_section(sid, Rect::new(100.0, 100.0, 400.0, 300.0), "Plan")).unwrap();
        let rid = d.alloc_id();
        let child = d.insert(sec, Node::rectangle(rid, Rect::new(50.0, 50.0, 80.0, 80.0))).unwrap();
        // empty inside: the canvas
        assert_eq!(hit_test(&d, page, v2(400.0, 300.0), 0.0, false, &[]), None);
        // the title pill above the top left corner
        assert_eq!(hit_test(&d, page, v2(110.0, 100.0 - 17.0), 0.0, false, &[]), Some(sec));
        // the border band
        assert_eq!(hit_test(&d, page, v2(103.0, 300.0), 2.0, false, &[]), Some(sec));
        // a child inside is picked directly
        assert_eq!(hit_test(&d, page, v2(170.0, 170.0), 0.0, false, &[]), Some(child));
        // with the contents hidden the child is out of reach
        if let Some(NodeKind::BoardSection(s)) = d.get_mut(sec).map(|n| &mut n.kind) {
            s.contents_hidden = true;
        }
        assert_eq!(hit_test(&d, page, v2(170.0, 170.0), 0.0, true, &[]), None);
        assert_eq!(hit_test(&d, page, v2(103.0, 300.0), 2.0, false, &[]), Some(sec));
    }

    #[test]
    fn empty_frame_props_default_ok() {
        assert!(!FrameProps::default().clips_content);
    }
}
