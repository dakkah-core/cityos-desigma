//! Spatial index of world-space bounds per page, plus a node snapshot used to
//! work out which page regions changed between document revisions.
//!
//! The index is hierarchical: every node stores the AABB of its whole drawn
//! subtree (including stroke / shadow / blur overflow, clipped by clipping
//! frames). The renderer walks the normal tree but skips any child whose
//! subtree box misses the window being painted.

use crate::geom::{to_sk_transform, transformed_bounds};
use desigma_doc::{Document, Effect, Node, NodeId, NodeKind};
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use tiny_skia as sk;

/// Multiplicative hasher for dense integer ids (SipHash is needlessly slow
/// for the per-node maps rebuilt on every revision).
#[derive(Default, Clone, Copy)]
pub(crate) struct IdHasher(u64);

impl Hasher for IdHasher {
    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.write_u64(*b as u64);
        }
    }
    fn write_u64(&mut self, n: u64) {
        self.0 = (self.0.rotate_left(5) ^ n).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
    fn finish(&self) -> u64 {
        // fold the high bits down: hashbrown uses both ends
        self.0 ^ (self.0 >> 32)
    }
}

pub(crate) type IdMap<V> = HashMap<NodeId, V, BuildHasherDefault<IdHasher>>;

/// Axis-aligned box in page space: (x0, y0, x1, y1).
pub(crate) type B = (f32, f32, f32, f32);

pub(crate) const EMPTY: B = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);

pub(crate) fn is_empty(b: B) -> bool {
    b.0 > b.2 || b.1 > b.3
}

pub(crate) fn union(a: B, b: B) -> B {
    (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3))
}

pub(crate) fn intersect(a: B, b: B) -> B {
    (a.0.max(b.0), a.1.max(b.1), a.2.min(b.2), a.3.min(b.3))
}

pub(crate) fn overlaps(a: B, b: B) -> bool {
    a.0 <= b.2 && b.0 <= a.2 && a.1 <= b.3 && b.1 <= a.3 && !is_empty(a) && !is_empty(b)
}

/// Extra extent (local units) around the node bounds touched by strokes and
/// effects. Shared with the renderer's own culling so both agree exactly.
pub(crate) fn node_margin(n: &Node) -> f32 {
    let mut m = 0.0f32;
    let has_stroke_paint = n.stroke.paints.iter().any(|p| p.visible) && (n.stroke.weight > 0.0 || n.stroke.side_weights.is_some());
    if has_stroke_paint {
        let sw = n.stroke.side_weights.map(|s| s.iter().cloned().fold(0.0, f32::max)).unwrap_or(0.0);
        m = m.max(n.stroke.weight.max(sw) * 2.0);
    }
    for e in &n.effects {
        match e {
            Effect::DropShadow { offset, radius, spread, visible: true, .. } => {
                m = m.max(offset.x.abs().max(offset.y.abs()) + spread.max(0.0) + radius * 1.6 + 2.0)
            }
            Effect::LayerBlur { radius, visible: true } => m = m.max(radius * 1.6 + 2.0),
            Effect::Texture(t) if t.visible => m = m.max(t.radius.max(0.0) + 2.0),
            _ => {}
        }
    }
    if let NodeKind::Text(t) = &n.kind {
        m = m.max(t.style.font_size * 2.0);
    }
    if matches!(n.kind, NodeKind::Sticky(_)) {
        m = m.max(crate::board::sticky_shadow_reach(n.size));
    }
    if matches!(n.kind, NodeKind::BoardSection(_)) {
        // the title pill above the box
        m = m.max(desigma_doc::SECTION_TITLE_REACH);
    }
    m
}

/// How far (local units) pixels of this node's effects depend on content
/// outside the node's own footprint. A tile must be rendered with at least
/// this much padding for blurs to match a full-page render.
fn blur_reach(n: &Node) -> f32 {
    let mut r = 0.0f32;
    for e in &n.effects {
        match e {
            Effect::DropShadow { offset, radius, spread, visible: true, .. } => {
                r = r.max(offset.x.abs().max(offset.y.abs()) + spread.max(0.0) + radius * 1.6 + 2.0)
            }
            Effect::InnerShadow { offset, radius, spread, visible: true, .. } => {
                r = r.max(offset.x.abs().max(offset.y.abs()) + spread.abs() + radius * 1.6 + 2.0)
            }
            Effect::LayerBlur { radius, visible: true } | Effect::BackgroundBlur { radius, visible: true } if *radius > 0.0 => r = r.max(radius * 1.6 + 2.0),
            Effect::Texture(t) if t.visible => r = r.max(t.radius.max(0.0) + 2.0),
            Effect::Glass(g) if g.visible => r = r.max(g.frost.max(0.0) * 1.6 + g.depth.max(0.0) * 2.0 + 6.0),
            _ => {}
        }
    }
    r
}

#[derive(Clone, Debug)]
pub(crate) struct Entry {
    /// Page-space bounds of the node and everything it draws (empty if hidden).
    pub bounds: B,
    pub parent: Option<NodeId>,
    /// Has a visible layer blur: any change below it spreads over its bounds.
    pub blur_container: bool,
    pub mask: bool,
    pub boolean: bool,
}

/// Cached world bounds for one page, rebuilt when the revision changes.
#[derive(Default)]
pub struct SceneIndex {
    page: Option<NodeId>,
    rev: u64,
    built: bool,
    pub(crate) entries: IdMap<Entry>,
    /// Nodes with blur-like effects: (subtree bounds, reach in page units).
    pub(crate) blurry: Vec<(B, f32)>,
    /// Bounds of nodes with a background blur (they read what is beneath).
    pub(crate) bg_blur: Vec<B>,
}

impl SceneIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Rebuilds if `rev` or `page` differ from the cached build. Returns the
    /// previous entries when a rebuild happened.
    pub(crate) fn ensure_prev(&mut self, doc: &Document, page: NodeId, rev: u64) -> Option<IdMap<Entry>> {
        if self.built && self.page == Some(page) && self.rev == rev {
            return None;
        }
        let prev = std::mem::take(&mut self.entries);
        self.blurry.clear();
        self.bg_blur.clear();
        self.page = Some(page);
        self.rev = rev;
        self.built = true;
        if let Some(p) = doc.get(page) {
            let id = sk::Transform::identity();
            for c in &p.children {
                self.build(doc, *c, id, Some(page));
            }
        }
        Some(prev)
    }

    pub fn ensure(&mut self, doc: &Document, page: NodeId, rev: u64) {
        self.ensure_prev(doc, page, rev);
    }

    /// Forgets the cached build so the next `ensure` rebuilds.
    pub fn clear(&mut self) {
        self.built = false;
        self.entries.clear();
    }

    /// True unless the subtree of `id` is known to miss `win`.
    pub(crate) fn may_touch(&self, id: NodeId, win: B) -> bool {
        match self.entries.get(&id) {
            Some(e) => overlaps(e.bounds, win),
            None => true,
        }
    }

    /// Page-space bounds (x0, y0, x1, y1) of a node's drawn subtree, if indexed and visible.
    pub fn bounds_of(&self, id: NodeId) -> Option<(f32, f32, f32, f32)> {
        self.entries.get(&id).map(|e| e.bounds).filter(|b| !is_empty(*b))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn build(&mut self, doc: &Document, id: NodeId, parent_ts: sk::Transform, parent: Option<NodeId>) -> B {
        let Some(n) = doc.get(id) else { return EMPTY };
        let n = &*desigma_doc::resolve_node(doc, n);
        let ts = parent_ts.pre_concat(to_sk_transform(&n.transform));
        let drawn = n.visible && !matches!(n.kind, NodeKind::Unknown { .. } | NodeKind::Page { .. });
        let m = node_margin(n);
        let mut sub = if drawn { transformed_bounds(&ts, -m, -m, n.size.x + 2.0 * m, n.size.y + 2.0 * m) } else { EMPTY };
        let boolean = matches!(n.kind, NodeKind::BooleanOp(_));
        let draws_kids = !n.children.is_empty() && !boolean;
        let mut kids = EMPTY;
        for c in &n.children {
            let cb = self.build(doc, *c, ts, Some(id));
            if draws_kids && drawn {
                kids = union(kids, cb);
            }
        }
        if !is_empty(kids) {
            if n.kind.frame_props().map(|f| f.clips_content).unwrap_or(false) {
                kids = intersect(kids, transformed_bounds(&ts, 0.0, 0.0, n.size.x, n.size.y));
            }
            if !is_empty(kids) {
                sub = union(sub, kids);
            }
        }
        let layer_blur = n.effects.iter().any(|e| matches!(e, Effect::LayerBlur { visible: true, radius } if *radius > 0.0));
        if drawn && !is_empty(sub) {
            let reach = blur_reach(n);
            if reach > 0.0 {
                // `ts` is a pure scale/rotation here; take its uniform scale
                let sc = (ts.sx * ts.sy - ts.kx * ts.ky).abs().sqrt().max(1e-6);
                self.blurry.push((sub, reach * sc));
            }
            if n.effects
                .iter()
                .any(|e| matches!(e, Effect::BackgroundBlur { visible: true, radius } if *radius > 0.0) || matches!(e, Effect::Glass(g) if g.visible))
            {
                self.bg_blur.push(sub);
            }
        }
        self.entries.insert(id, Entry { bounds: sub, parent, blur_container: layer_blur, mask: n.is_mask, boolean });
        sub
    }
}

/// Snapshot of every node of the page as of the last sync; compared against
/// the live document to find what changed.
#[derive(Default)]
pub(crate) struct Differ {
    snap: IdMap<Node>,
}

/// Result of comparing a revision against the previous one.
pub(crate) struct Changes {
    /// Nothing is known about the previous state: drop everything.
    pub all: bool,
    /// Page-space regions whose pixels may differ.
    pub rects: Vec<B>,
}

fn same_ignoring_structure(a: &Node, b: &Node) -> bool {
    if a == b {
        return true;
    }
    let mut t = b.clone();
    t.children = a.children.clone();
    t.parent = a.parent;
    t == *a
}

impl Differ {
    pub fn reset(&mut self) {
        self.snap.clear();
    }

    /// Compares the live page (indexed in `new`, previously indexed in `old`)
    /// with the snapshot and refreshes the snapshot.
    pub fn diff(&mut self, doc: &Document, page: NodeId, old: &IdMap<Entry>, new: &SceneIndex) -> Changes {
        let first = self.snap.is_empty();
        let mut rects: Vec<B> = Vec::new();
        // nodes whose change can spread to ancestors (blur layers, masks, boolean ops)
        let mut spread: Vec<NodeId> = Vec::new();
        let mut all = first;

        let obounds = |id: &NodeId| old.get(id).map(|e| e.bounds).unwrap_or(EMPTY);
        let nbounds = |id: &NodeId| new.entries.get(id).map(|e| e.bounds).unwrap_or(EMPTY);
        let is_mask = |id: &NodeId| new.entries.get(id).is_some_and(|e| e.mask) || old.get(id).is_some_and(|e| e.mask);

        // page node itself (background colour etc.)
        if let (Some(p), Some(s)) = (doc.get(page), self.snap.get(&page)) {
            if !same_ignoring_structure(s, p) {
                all = true;
            }
        }
        if let Some(p) = doc.get(page) {
            let changed_children = self.snap.get(&page).map(|s| s.children != p.children).unwrap_or(true);
            if changed_children {
                self.children_delta(
                    self.snap.get(&page).map(|s| s.children.clone()).unwrap_or_default(),
                    &p.children,
                    &obounds,
                    &nbounds,
                    &is_mask,
                    &mut rects,
                    &mut spread,
                );
            }
            self.snap.insert(page, p.clone());
        }

        for (id, ne) in new.entries.iter() {
            let Some(live) = doc.get(*id) else { continue };
            // Compare with variable bindings applied, so editing a variable or a
            // mode dirties exactly the nodes whose drawn values changed.
            let live = &*desigma_doc::resolve_node(doc, live);
            match self.snap.get(id) {
                None => {
                    rects.push(ne.bounds);
                    spread.push(*id);
                    self.snap.insert(*id, live.clone());
                }
                Some(s) if s == live => {}
                Some(s) => {
                    let props_changed = !same_ignoring_structure(s, live);
                    if props_changed {
                        rects.push(obounds(id));
                        rects.push(ne.bounds);
                        spread.push(*id);
                        if s.is_mask != live.is_mask {
                            spread.push(*id);
                        }
                    }
                    if s.children != live.children {
                        self.children_delta(s.children.clone(), &live.children, &obounds, &nbounds, &is_mask, &mut rects, &mut spread);
                        spread.push(*id);
                    }
                    self.snap.insert(*id, live.clone());
                }
            }
        }
        // removed nodes
        let gone: Vec<NodeId> = self.snap.keys().filter(|k| **k != page && !new.entries.contains_key(k)).copied().collect();
        for g in gone {
            rects.push(obounds(&g));
            self.snap.remove(&g);
        }

        if !all {
            // ancestors that re-blur / re-mask / re-union what changed below them
            let mut extra: Vec<B> = Vec::new();
            for id in spread {
                let Some(e) = new.entries.get(&id).or_else(|| old.get(&id)) else { continue };
                if e.mask {
                    // a mask clips every sibling that follows it
                    if let Some(sibs) = e.parent.and_then(|p| doc.get(p)).map(|p| &p.children) {
                        if let Some(pos) = sibs.iter().position(|c| *c == id) {
                            for c in &sibs[pos + 1..] {
                                extra.push(obounds(c));
                                extra.push(nbounds(c));
                            }
                        }
                    }
                }
                let mut parent = e.parent;
                while let Some(p) = parent {
                    let Some(pe) = new.entries.get(&p) else { break };
                    if pe.blur_container || pe.boolean {
                        extra.push(pe.bounds);
                        extra.push(obounds(&p));
                    }
                    parent = pe.parent;
                }
            }
            rects.extend(extra);
            rects.retain(|r| !is_empty(*r));
            // background blurs resample whatever lies beneath them
            if !new.bg_blur.is_empty() && !rects.is_empty() {
                let hits: Vec<B> = new.bg_blur.iter().copied().filter(|b| rects.iter().any(|r| overlaps(*r, *b))).collect();
                rects.extend(hits);
            }
        }
        Changes { all, rects }
    }

    /// Dirty regions for an edit of a child list: everything between the first
    /// and last position that differs (inserts, removals, z-order moves).
    #[allow(clippy::too_many_arguments)]
    fn children_delta(
        &self,
        old: Vec<NodeId>,
        new: &[NodeId],
        obounds: &dyn Fn(&NodeId) -> B,
        nbounds: &dyn Fn(&NodeId) -> B,
        is_mask: &dyn Fn(&NodeId) -> bool,
        rects: &mut Vec<B>,
        spread: &mut Vec<NodeId>,
    ) {
        let pre = old.iter().zip(new).take_while(|(a, b)| a == b).count();
        let suf = old[pre..].iter().rev().zip(new[pre..].iter().rev()).take_while(|(a, b)| a == b).count();
        let mut masks = false;
        for id in &old[pre..old.len() - suf] {
            rects.push(obounds(id));
            spread.push(*id);
            masks |= is_mask(id);
        }
        for id in &new[pre..new.len() - suf] {
            rects.push(nbounds(id));
            spread.push(*id);
            masks |= is_mask(id);
        }
        if masks {
            // a mask added / removed / moved changes every sibling after it
            for id in &new[pre..] {
                rects.push(nbounds(id));
                rects.push(obounds(id));
            }
        }
    }
}
