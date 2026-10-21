//! Drawing tools of a board: the kind of brush a stroke was drawn with (marker, highlighter,
//! washi tape), and the geometry the eraser and hit testing need. A stroke is an ordinary
//! `Vector` node with a round-capped stroke; [`Node::brush`] only says how it is drawn.

use crate::vector::flatten_path;
use crate::*;
use serde::{Deserialize, Serialize};

/// Decoration of a washi tape strip.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WashiPattern {
    #[default]
    Stripes,
    Dots,
    Grid,
    Zigzag,
    Checks,
}

impl WashiPattern {
    pub const ALL: [WashiPattern; 5] = [WashiPattern::Stripes, WashiPattern::Dots, WashiPattern::Grid, WashiPattern::Zigzag, WashiPattern::Checks];

    pub fn label(self) -> &'static str {
        match self {
            WashiPattern::Stripes => "Stripes",
            WashiPattern::Dots => "Dots",
            WashiPattern::Grid => "Grid",
            WashiPattern::Zigzag => "Zigzag",
            WashiPattern::Checks => "Checks",
        }
    }
}

/// How a stroke node is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Brush {
    /// Opaque round pen.
    Marker,
    /// Wide, translucent, multiplied over what is below.
    Highlighter,
    /// A straight, patterned strip with torn ends.
    Washi(WashiPattern),
}

impl Brush {
    pub fn label(self) -> &'static str {
        match self {
            Brush::Marker => "Marker",
            Brush::Highlighter => "Highlighter",
            Brush::Washi(_) => "Washi tape",
        }
    }
}

/// Thickness choices of every brush, as multiples of the brush's base width.
pub const BRUSH_SIZES: [f32; 3] = [0.5, 1.0, 2.0];

/// Index in [`BRUSH_SIZES`] closest to `weight / base`.
pub fn brush_size_index(weight: f32, base: f32) -> usize {
    let r = weight / base.max(1e-3);
    (0..BRUSH_SIZES.len()).min_by(|a, b| (BRUSH_SIZES[*a] - r).abs().total_cmp(&(BRUSH_SIZES[*b] - r).abs())).unwrap_or(1)
}

/// Distance from `p` to the segment `a`-`b`.
pub fn segment_distance(a: Vec2, b: Vec2, p: Vec2) -> f32 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    let t = if len2 <= 1e-9 { 0.0 } else { (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0) };
    let (qx, qy) = (a.x + dx * t, a.y + dy * t);
    ((p.x - qx).powi(2) + (p.y - qy).powi(2)).sqrt()
}

impl Node {
    /// The flattened polylines of a stroke node in its local space.
    pub fn brush_polylines(&self) -> Vec<Vec<Vec2>> {
        match &self.kind {
            NodeKind::Vector { paths, .. } => paths.iter().flat_map(|p| flatten_path(p, 0.25)).map(|(pts, _)| pts).collect(),
            _ => Vec::new(),
        }
    }

    /// Distance from the local point `p` to the visible edge of a stroke node (negative inside
    /// the ink); `None` for nodes that are not strokes.
    pub fn brush_distance(&self, p: Vec2) -> Option<f32> {
        self.brush?;
        let mut best = f32::INFINITY;
        for line in self.brush_polylines() {
            if line.len() == 1 {
                best = best.min(segment_distance(line[0], line[0], p));
            }
            for w in line.windows(2) {
                best = best.min(segment_distance(w[0], w[1], p));
            }
        }
        best.is_finite().then(|| best - self.stroke.weight / 2.0)
    }
}

/// Stroke nodes under `root` touched by a disc of `radius` around the page point `p`, topmost
/// first. This is what the eraser deletes.
pub fn brush_hits(doc: &Document, root: NodeId, p: Vec2, radius: f32) -> Vec<NodeId> {
    let mut out = Vec::new();
    collect_hits(doc, root, p, radius, &mut out);
    out.reverse();
    out
}

fn collect_hits(doc: &Document, id: NodeId, p: Vec2, radius: f32, out: &mut Vec<NodeId>) {
    let Some(n) = doc.get(id) else { return };
    for &c in &n.children {
        let Some(k) = doc.get(c) else { continue };
        if !k.visible || k.locked {
            continue;
        }
        if k.brush.is_some() {
            let Some(inv) = doc.world_transform(c).invert() else { continue };
            // distances are measured in the stroke's own units, so scale the radius with it
            let scale = (inv.a * inv.a + inv.b * inv.b).sqrt().max(1e-6);
            if k.brush_distance(inv.apply(p)).is_some_and(|d| d <= radius * scale) {
                out.push(c);
            }
        } else if !k.children.is_empty() {
            collect_hits(doc, c, p, radius, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stroke(doc: &mut Document, parent: NodeId, a: Vec2, b: Vec2, brush: Brush) -> NodeId {
        let id = doc.alloc_id();
        let net = VectorNetwork::from_paths(&[Path { cmds: vec![PathCmd::MoveTo(a), PathCmd::LineTo(b)], fill_rule: FillRule::NonZero }]);
        let mut n = Node::new(id, NodeKind::Vector { paths: net.to_paths(), network: Some(net) });
        n.brush = Some(brush);
        n.stroke.weight = 6.0;
        doc.insert(parent, n).unwrap()
    }

    #[test]
    fn sizes_round_to_the_nearest_choice() {
        assert_eq!(brush_size_index(6.0, 6.0), 1);
        assert_eq!(brush_size_index(3.0, 6.0), 0);
        assert_eq!(brush_size_index(11.0, 6.0), 2);
    }

    #[test]
    fn distance_is_measured_to_the_ink_edge() {
        let mut d = Document::new("t");
        let page = d.add_page("p");
        let id = stroke(&mut d, page, Vec2::new(0.0, 0.0), Vec2::new(100.0, 0.0), Brush::Marker);
        let n = d.get(id).unwrap();
        assert!(n.brush_distance(Vec2::new(50.0, 1.0)).unwrap() < 0.0);
        assert!((n.brush_distance(Vec2::new(50.0, 13.0)).unwrap() - 10.0).abs() < 0.01);
        let r = d.alloc_id();
        let plain = d.insert(page, Node::rectangle(r, Rect::new(0.0, 0.0, 10.0, 10.0))).unwrap();
        assert!(d.get(plain).unwrap().brush_distance(Vec2::ZERO).is_none());
    }

    #[test]
    fn eraser_hits_only_strokes_it_touches() {
        let mut d = Document::new("t");
        let page = d.add_page("p");
        let a = stroke(&mut d, page, Vec2::new(0.0, 0.0), Vec2::new(100.0, 0.0), Brush::Marker);
        let b = stroke(&mut d, page, Vec2::new(0.0, 50.0), Vec2::new(100.0, 50.0), Brush::Washi(WashiPattern::Dots));
        assert_eq!(brush_hits(&d, page, Vec2::new(40.0, 4.0), 4.0), vec![a]);
        assert_eq!(brush_hits(&d, page, Vec2::new(40.0, 48.0), 2.0), vec![b]);
        assert!(brush_hits(&d, page, Vec2::new(40.0, 25.0), 4.0).is_empty());
    }

    #[test]
    fn brush_round_trips_and_old_files_load() {
        let mut n = Node::new(NodeId(1), NodeKind::Group);
        assert!(n.brush.is_none());
        let json = serde_json::to_string(&n).unwrap();
        assert!(!json.contains("brush"), "ordinary nodes omit the field");
        n.brush = Some(Brush::Washi(WashiPattern::Zigzag));
        let back: Node = serde_json::from_str(&serde_json::to_string(&n).unwrap()).unwrap();
        assert_eq!(back.brush, Some(Brush::Washi(WashiPattern::Zigzag)));
        let old: Node = serde_json::from_str(&json).unwrap();
        assert_eq!(old.brush, None);
    }
}
