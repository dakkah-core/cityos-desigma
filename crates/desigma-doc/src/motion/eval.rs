//! Evaluating keyframes and clips at a point in time.
//!
//! Nothing here mutates the stored document. Three levels of API, cheapest first:
//!
//! * [`evaluate_node`]: pure function of one node and a time, returns the animated property
//!   values as an [`EvaluatedNode`] (only the properties that are animated are `Some`).
//! * [`MotionPlayer`]: for playback. Clones the document once, then [`MotionPlayer::seek`] only
//!   rewrites the animated properties of animated nodes each frame (no deep clone per frame).
//! * [`evaluate`]: one-shot convenience that returns an evaluated *copy* of the document.
//!
//! ## How a node's transform is animated
//!
//! `Node::transform` is decomposed around the node's centre `h = size / 2`:
//!
//! ```text
//! transform = T(position + h) * R(rotation) * S(scale) * T(-h)        (shear of the base is kept)
//! ```
//!
//! so `position` is the layout position of the unrotated, unscaled box (the X/Y fields), and
//! rotation and scale pivot about the centre without moving `position`. An untouched component
//! keeps the node's static value, which makes [`AnimProperty::Rotation`] *absolute* (the base is
//! the node's current angle) and [`AnimProperty::Scale`] *relative* to the static transform (the
//! base is 1.0). Width/height animation changes `size` only; children and auto layout are not
//! re-run, and the pivot follows the new size.

use super::clip::ClipKind;
use super::value::{AnimProperty, AnimValue};
use crate::{Affine, Color, Document, Effect, Node, NodeId, PaintKind, Vec2};

/// Animated property values of one node at one time. A field is `Some` only when the node has a
/// keyframe track or a visible clip for it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EvaluatedNode {
    pub position: Option<Vec2>,
    pub scale: Option<Vec2>,
    /// Degrees, counter-clockwise positive.
    pub rotation: Option<f32>,
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub opacity: Option<f32>,
    pub corner_radius: Option<f32>,
    pub fill_color: Option<Color>,
    pub stroke_color: Option<Color>,
    pub stroke_weight: Option<f32>,
    /// Offset (x), offset (y), blur and colour of the first drop shadow.
    pub shadow_x: Option<f32>,
    pub shadow_y: Option<f32>,
    pub shadow_blur: Option<f32>,
    pub shadow_color: Option<Color>,
    /// Radius of the first layer blur.
    pub blur_radius: Option<f32>,
}

impl EvaluatedNode {
    pub fn is_empty(&self) -> bool {
        *self == EvaluatedNode::default()
    }

    /// The evaluated value of `property`, if it is animated.
    pub fn get(&self, property: AnimProperty) -> Option<AnimValue> {
        Some(match property {
            AnimProperty::Position => AnimValue::Vec2(self.position?),
            AnimProperty::Scale => AnimValue::Vec2(self.scale?),
            AnimProperty::Rotation => AnimValue::Scalar(self.rotation?),
            AnimProperty::Width => AnimValue::Scalar(self.width?),
            AnimProperty::Height => AnimValue::Scalar(self.height?),
            AnimProperty::Opacity => AnimValue::Scalar(self.opacity?),
            AnimProperty::CornerRadius => AnimValue::Scalar(self.corner_radius?),
            AnimProperty::FillColor => AnimValue::Color(self.fill_color?),
            AnimProperty::StrokeColor => AnimValue::Color(self.stroke_color?),
            AnimProperty::StrokeWeight => AnimValue::Scalar(self.stroke_weight?),
            AnimProperty::ShadowX => AnimValue::Scalar(self.shadow_x?),
            AnimProperty::ShadowY => AnimValue::Scalar(self.shadow_y?),
            AnimProperty::ShadowBlur => AnimValue::Scalar(self.shadow_blur?),
            AnimProperty::ShadowColor => AnimValue::Color(self.shadow_color?),
            AnimProperty::BlurRadius => AnimValue::Scalar(self.blur_radius?),
        })
    }

    /// Sets `property`; a value of the wrong shape is ignored.
    pub fn set(&mut self, property: AnimProperty, value: AnimValue) {
        match (property, value) {
            (AnimProperty::Position, AnimValue::Vec2(v)) => self.position = Some(v),
            (AnimProperty::Scale, AnimValue::Vec2(v)) => self.scale = Some(v),
            (AnimProperty::Rotation, AnimValue::Scalar(v)) => self.rotation = Some(v),
            (AnimProperty::Width, AnimValue::Scalar(v)) => self.width = Some(v),
            (AnimProperty::Height, AnimValue::Scalar(v)) => self.height = Some(v),
            (AnimProperty::Opacity, AnimValue::Scalar(v)) => self.opacity = Some(v),
            (AnimProperty::CornerRadius, AnimValue::Scalar(v)) => self.corner_radius = Some(v),
            (AnimProperty::FillColor, AnimValue::Color(v)) => self.fill_color = Some(v),
            (AnimProperty::StrokeColor, AnimValue::Color(v)) => self.stroke_color = Some(v),
            (AnimProperty::StrokeWeight, AnimValue::Scalar(v)) => self.stroke_weight = Some(v),
            (AnimProperty::ShadowX, AnimValue::Scalar(v)) => self.shadow_x = Some(v),
            (AnimProperty::ShadowY, AnimValue::Scalar(v)) => self.shadow_y = Some(v),
            (AnimProperty::ShadowBlur, AnimValue::Scalar(v)) => self.shadow_blur = Some(v),
            (AnimProperty::ShadowColor, AnimValue::Color(v)) => self.shadow_color = Some(v),
            (AnimProperty::BlurRadius, AnimValue::Scalar(v)) => self.blur_radius = Some(v),
            _ => {}
        }
    }

    /// Writes the animated values onto `out`, computing transform parts relative to `base`.
    ///
    /// `base` is the node as stored in the document and `out` a copy of it (possibly from an
    /// earlier frame): only animated properties are written, so frames never accumulate.
    pub fn apply_to(&self, base: &Node, out: &mut Node) {
        if let Some(o) = self.opacity {
            out.opacity = if o.is_finite() { o.clamp(0.0, 1.0) } else { base.opacity };
        }
        if let Some(r) = self.corner_radius.filter(|r| r.is_finite()) {
            out.corner_radii = [r.max(0.0); 4];
        }
        if let (Some(c), Some(i)) = (self.fill_color, first_solid(&base.fills)) {
            if let Some(p) = out.fills.get_mut(i) {
                p.kind = PaintKind::Solid(c);
            }
        }
        if let (Some(c), Some(i)) = (self.stroke_color, first_solid(&base.stroke.paints)) {
            if let Some(p) = out.stroke.paints.get_mut(i) {
                p.kind = PaintKind::Solid(c);
            }
        }
        if let Some(w) = self.stroke_weight.filter(|w| w.is_finite()) {
            out.stroke.weight = w.max(0.0);
        }
        if let Some(i) = first_drop_shadow(&base.effects) {
            if let Some(Effect::DropShadow { color, offset, radius, .. }) = out.effects.get_mut(i) {
                if let Some(v) = self.shadow_x.filter(|v| v.is_finite()) {
                    offset.x = v;
                }
                if let Some(v) = self.shadow_y.filter(|v| v.is_finite()) {
                    offset.y = v;
                }
                if let Some(v) = self.shadow_blur.filter(|v| v.is_finite()) {
                    *radius = v.max(0.0);
                }
                if let Some(c) = self.shadow_color {
                    *color = c;
                }
            }
        }
        if let (Some(r), Some(i)) = (self.blur_radius.filter(|r| r.is_finite()), first_layer_blur(&base.effects)) {
            if let Some(Effect::LayerBlur { radius, .. }) = out.effects.get_mut(i) {
                *radius = r.max(0.0);
            }
        }
        let transform_touched = self.position.is_some() || self.scale.is_some() || self.rotation.is_some() || self.width.is_some() || self.height.is_some();
        if !transform_touched {
            return;
        }
        let size = Vec2::new(
            self.width.filter(|v| v.is_finite()).map_or(base.size.x, |v| v.max(0.0)),
            self.height.filter(|v| v.is_finite()).map_or(base.size.y, |v| v.max(0.0)),
        );
        out.size = size;
        let pos = self.position.filter(|p| p.x.is_finite() && p.y.is_finite()).unwrap_or_else(|| base_position(base));
        let dtheta = self.rotation.filter(|r| r.is_finite()).map_or(0.0, |r| -(r - base_rotation_deg(base)).to_radians());
        let k = self.scale.filter(|s| s.x.is_finite() && s.y.is_finite()).unwrap_or(Vec2::new(1.0, 1.0));
        let t = base.transform;
        let linear = Affine { tx: 0.0, ty: 0.0, ..t };
        let l = Affine::rotate(dtheta) * linear * Affine::scale(k.x, k.y);
        let h = Vec2::new(size.x * 0.5, size.y * 0.5);
        out.transform = Affine { tx: pos.x + h.x - (l.a * h.x + l.c * h.y), ty: pos.y + h.y - (l.b * h.x + l.d * h.y), ..l };
    }
}

fn first_solid(paints: &[crate::Paint]) -> Option<usize> {
    paints.iter().position(|p| matches!(p.kind, PaintKind::Solid(_)))
}

/// Index of the first drop shadow, the one the shadow properties animate.
pub fn first_drop_shadow(effects: &[Effect]) -> Option<usize> {
    effects.iter().position(|e| matches!(e, Effect::DropShadow { .. }))
}

/// Index of the first layer blur, the one [`AnimProperty::BlurRadius`] animates.
pub fn first_layer_blur(effects: &[Effect]) -> Option<usize> {
    effects.iter().position(|e| matches!(e, Effect::LayerBlur { .. }))
}

/// The node's static layout position: the translation that, with no rotation or scale about the
/// centre, would place the same box. Equals `transform.tx/ty` for unrotated nodes.
pub fn base_position(node: &Node) -> Vec2 {
    let t = node.transform;
    let (hx, hy) = (node.size.x * 0.5, node.size.y * 0.5);
    Vec2::new(t.tx - hx + (t.a * hx + t.c * hy), t.ty - hy + (t.b * hx + t.d * hy))
}

/// The node's static rotation in degrees, counter-clockwise positive (what the Rotation field shows).
pub fn base_rotation_deg(node: &Node) -> f32 {
    -node.transform.rotation_rad().to_degrees()
}

/// The static (un-animated) value of `property` on `node`; `None` when the node cannot animate it
/// (a fill or stroke colour without a solid paint).
pub fn base_value(node: &Node, property: AnimProperty) -> Option<AnimValue> {
    Some(match property {
        AnimProperty::Position => AnimValue::Vec2(base_position(node)),
        AnimProperty::Scale => AnimValue::Vec2(Vec2::new(1.0, 1.0)),
        AnimProperty::Rotation => AnimValue::Scalar(base_rotation_deg(node)),
        AnimProperty::Width => AnimValue::Scalar(node.size.x),
        AnimProperty::Height => AnimValue::Scalar(node.size.y),
        AnimProperty::Opacity => AnimValue::Scalar(node.opacity),
        AnimProperty::CornerRadius => AnimValue::Scalar(node.corner_radii[0]),
        AnimProperty::FillColor => match node.fills.get(first_solid(&node.fills)?)?.kind {
            PaintKind::Solid(c) => AnimValue::Color(c),
            _ => return None,
        },
        AnimProperty::StrokeColor => match node.stroke.paints.get(first_solid(&node.stroke.paints)?)?.kind {
            PaintKind::Solid(c) => AnimValue::Color(c),
            _ => return None,
        },
        AnimProperty::StrokeWeight => AnimValue::Scalar(node.stroke.weight),
        AnimProperty::ShadowX | AnimProperty::ShadowY | AnimProperty::ShadowBlur | AnimProperty::ShadowColor => {
            let Effect::DropShadow { color, offset, radius, .. } = node.effects.get(first_drop_shadow(&node.effects)?)? else { return None };
            match property {
                AnimProperty::ShadowX => AnimValue::Scalar(offset.x),
                AnimProperty::ShadowY => AnimValue::Scalar(offset.y),
                AnimProperty::ShadowBlur => AnimValue::Scalar(*radius),
                _ => AnimValue::Color(*color),
            }
        }
        AnimProperty::BlurRadius => {
            let Effect::LayerBlur { radius, .. } = node.effects.get(first_layer_blur(&node.effects)?)? else { return None };
            AnimValue::Scalar(*radius)
        }
    })
}

/// Evaluates a node's keyframes and clips at `t_ms`.
///
/// Tracks give the base value at `t_ms` (the node's static value when a property has no track),
/// then every visible clip composes onto it: additive for position and rotation, multiplicative
/// for scale, size and opacity (see [`super::Clip`]). Times before 0 evaluate as 0.
pub fn evaluate_node(node: &Node, t_ms: f32) -> EvaluatedNode {
    let mut ev = EvaluatedNode::default();
    let m = &node.motion;
    if m.is_empty() {
        return ev;
    }
    let t = if t_ms.is_finite() { t_ms.max(0.0) } else { 0.0 };
    for tr in m.tracks.iter().filter(|tr| tr.visible) {
        if let Some(v) = tr.value_at(t) {
            ev.set(tr.property, v);
        }
    }
    for c in m.clips.iter().filter(|c| c.visible) {
        let v = c.value_at(t);
        match c.kind {
            ClipKind::Position => {
                let cur = ev.position.unwrap_or_else(|| base_position(node));
                ev.position = Some(Vec2::new(cur.x + v.x, cur.y + v.y));
            }
            ClipKind::Rotation => ev.rotation = Some(ev.rotation.unwrap_or_else(|| base_rotation_deg(node)) + v.x),
            ClipKind::Scale => {
                let cur = ev.scale.unwrap_or(Vec2::new(1.0, 1.0));
                ev.scale = Some(Vec2::new(cur.x * v.x, cur.y * v.y));
            }
            ClipKind::Size => {
                ev.width = Some(ev.width.unwrap_or(node.size.x) * v.x);
                ev.height = Some(ev.height.unwrap_or(node.size.y) * v.y);
            }
            ClipKind::Opacity => ev.opacity = Some((ev.opacity.unwrap_or(node.opacity) * v.x).clamp(0.0, 1.0)),
        }
    }
    ev
}

/// Value of `property` at `t_ms` including clips: the number the inspector shows at the playhead.
/// Falls back to the static value; `None` only when the node cannot animate the property.
pub fn value_at(node: &Node, property: AnimProperty, t_ms: f32) -> Option<AnimValue> {
    evaluate_node(node, t_ms).get(property).or_else(|| base_value(node, property))
}

/// Value of `property` at `t_ms` from its keyframe track alone (clips ignored), falling back to
/// the static value. This is what a new keyframe at the playhead should capture.
pub fn keyframed_value_at(node: &Node, property: AnimProperty, t_ms: f32) -> Option<AnimValue> {
    let t = if t_ms.is_finite() { t_ms.max(0.0) } else { 0.0 };
    node.motion.track(property).and_then(|tr| tr.value_at(t)).or_else(|| base_value(node, property))
}

/// Ids of the nodes below `page` (and the page itself) that have keyframes or clips, in paint order.
pub fn animated_nodes(doc: &Document, page: NodeId) -> Vec<NodeId> {
    let mut out = Vec::new();
    doc.walk(page, |n, _| {
        if !n.motion.is_empty() {
            out.push(n.id);
        }
    });
    out
}

/// Evaluated nodes of one page at one time, sorted by id.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MotionFrame {
    pub t_ms: f32,
    pub nodes: Vec<(NodeId, EvaluatedNode)>,
}

impl MotionFrame {
    pub fn get(&self, id: NodeId) -> Option<&EvaluatedNode> {
        self.nodes.binary_search_by_key(&id, |(i, _)| *i).ok().map(|i| &self.nodes[i].1)
    }

    /// Writes the frame onto `out`, a copy of `base`.
    pub fn apply(&self, base: &Document, out: &mut Document) {
        for (id, ev) in &self.nodes {
            if let (Some(b), Some(o)) = (base.nodes.get(id), out.nodes.get_mut(id)) {
                ev.apply_to(b, o);
            }
        }
    }
}

/// Evaluates every animated node of `page` at `t_ms` without touching `doc`.
pub fn evaluate_frame(doc: &Document, page: NodeId, t_ms: f32) -> MotionFrame {
    let mut nodes: Vec<(NodeId, EvaluatedNode)> = animated_nodes(doc, page)
        .into_iter()
        .filter_map(|id| doc.nodes.get(&id).map(|n| (id, evaluate_node(n, t_ms))))
        .filter(|(_, e)| !e.is_empty())
        .collect();
    nodes.sort_by_key(|(id, _)| *id);
    MotionFrame { t_ms, nodes }
}

/// A copy of `doc` with the animation of `page` evaluated at `t_ms` baked into the node
/// properties (animation data stays attached). This deep-clones the document; use
/// [`MotionPlayer`] when evaluating many frames.
pub fn evaluate(doc: &Document, page: NodeId, t_ms: f32) -> Document {
    let mut out = doc.clone();
    evaluate_frame(doc, page, t_ms).apply(doc, &mut out);
    out
}

/// Frame-by-frame playback over a scratch copy of the document.
///
/// Construction clones the document once; each [`MotionPlayer::seek`] evaluates the animated
/// nodes and rewrites only their animated properties (a few field writes per node, no allocation
/// per frame besides what the caller's renderer does), so hundreds of animated nodes evaluate in
/// well under a millisecond. Edits to the source document are *not* tracked: rebuild the player
/// ([`MotionPlayer::new`]) after any edit, and never feed edits made on the scratch document
/// back as commands (they contain animated values, not the stored ones).
pub struct MotionPlayer {
    scratch: Document,
    page: NodeId,
    ids: Vec<NodeId>,
}

impl MotionPlayer {
    pub fn new(doc: &Document, page: NodeId) -> Self {
        Self { scratch: doc.clone(), page, ids: animated_nodes(doc, page) }
    }

    pub fn page(&self) -> NodeId {
        self.page
    }

    /// Ids of the animated nodes the player updates.
    pub fn animated_ids(&self) -> &[NodeId] {
        &self.ids
    }

    /// The evaluated document as of the last [`seek`](Self::seek) (initially the unmodified copy).
    pub fn document(&self) -> &Document {
        &self.scratch
    }

    /// Evaluates the page at `t_ms` against `base` (the document the player was built from) and
    /// returns the evaluated scratch document.
    pub fn seek(&mut self, base: &Document, t_ms: f32) -> &Document {
        for id in &self.ids {
            if let (Some(b), Some(o)) = (base.nodes.get(id), self.scratch.nodes.get_mut(id)) {
                evaluate_node(b, t_ms).apply_to(b, o);
            }
        }
        &self.scratch
    }
}

#[cfg(test)]
mod tests {
    use super::super::clip::{Clip, ClipDirection, ClipMode, ClipRepeat};
    use super::super::easing::Ease;
    use super::*;
    use crate::{Node, PaintKind, Rect};

    fn doc_with(node: Node) -> (Document, NodeId, NodeId) {
        let mut d = Document::new("t");
        let page = d.add_page("P");
        let id = node.id;
        d.insert(page, node).unwrap();
        (d, page, id)
    }

    fn rect() -> Node {
        Node::rectangle(NodeId(10), Rect::new(100.0, 50.0, 80.0, 40.0))
    }

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    fn key(n: &mut Node, p: AnimProperty, t: u32, v: AnimValue) {
        n.motion.ensure_track(p).set_key(t, v, None);
    }

    #[test]
    fn untouched_nodes_evaluate_to_nothing() {
        let n = rect();
        assert!(evaluate_node(&n, 100.0).is_empty());
        assert_eq!(value_at(&n, AnimProperty::Opacity, 5.0), Some(AnimValue::Scalar(1.0)));
        assert_eq!(base_value(&n, AnimProperty::Position), Some(AnimValue::Vec2(Vec2::new(100.0, 50.0))));
    }

    #[test]
    fn position_track_moves_the_node() {
        let mut n = rect();
        key(&mut n, AnimProperty::Position, 0, AnimValue::Vec2(Vec2::new(100.0, 50.0)));
        key(&mut n, AnimProperty::Position, 1000, AnimValue::Vec2(Vec2::new(300.0, 50.0)));
        let (d, page, id) = doc_with(n);
        let e = evaluate(&d, page, 500.0);
        let t = e.get(id).unwrap().transform;
        assert!(near(t.tx, 200.0) && near(t.ty, 50.0));
        // The stored document is untouched.
        assert_eq!(d.get(id).unwrap().transform.tx, 100.0);
        // Past the end it holds the last key; before the start the first.
        assert!(near(evaluate(&d, page, 9000.0).get(id).unwrap().transform.tx, 300.0));
    }

    #[test]
    fn rotation_pivots_about_the_centre_and_position_is_independent() {
        let mut n = rect();
        key(&mut n, AnimProperty::Rotation, 0, AnimValue::Scalar(0.0));
        key(&mut n, AnimProperty::Rotation, 1000, AnimValue::Scalar(90.0));
        let (d, page, id) = doc_with(n);
        let e = evaluate(&d, page, 1000.0);
        let node = e.get(id).unwrap();
        // The centre (140, 70) stays put.
        let c = node.transform.apply(Vec2::new(40.0, 20.0));
        assert!(near(c.x, 140.0) && near(c.y, 70.0), "{c:?}");
        // 90 degrees counter-clockwise (Figma): local +x points up (negative y).
        let right = node.transform.apply(Vec2::new(80.0, 20.0));
        assert!(near(right.x, 140.0) && near(right.y, 30.0), "{right:?}");
        assert!(near(base_rotation_deg(node), 90.0));
    }

    #[test]
    fn scale_about_centre_and_mirroring() {
        let mut n = rect();
        key(&mut n, AnimProperty::Scale, 0, AnimValue::Vec2(Vec2::new(2.0, 1.0)));
        let (d, page, id) = doc_with(n);
        let e = evaluate(&d, page, 0.0);
        let t = e.get(id).unwrap().transform;
        let c = t.apply(Vec2::new(40.0, 20.0));
        assert!(near(c.x, 140.0) && near(c.y, 70.0));
        assert!(near(t.apply(Vec2::new(80.0, 20.0)).x, 220.0), "right edge doubles its distance from the centre");
        // Size is untouched by scale.
        assert_eq!(e.get(id).unwrap().size, Vec2::new(80.0, 40.0));
    }

    #[test]
    fn width_height_opacity_radius_stroke() {
        let mut n = rect();
        n.stroke.paints = vec![crate::Paint::solid(Color::BLACK)];
        key(&mut n, AnimProperty::Width, 0, AnimValue::Scalar(80.0));
        key(&mut n, AnimProperty::Width, 100, AnimValue::Scalar(160.0));
        key(&mut n, AnimProperty::Opacity, 0, AnimValue::Scalar(1.0));
        key(&mut n, AnimProperty::Opacity, 100, AnimValue::Scalar(0.0));
        key(&mut n, AnimProperty::CornerRadius, 0, AnimValue::Scalar(0.0));
        key(&mut n, AnimProperty::CornerRadius, 100, AnimValue::Scalar(20.0));
        key(&mut n, AnimProperty::StrokeWeight, 0, AnimValue::Scalar(1.0));
        key(&mut n, AnimProperty::StrokeWeight, 100, AnimValue::Scalar(5.0));
        key(&mut n, AnimProperty::StrokeColor, 0, AnimValue::Color(Color::BLACK));
        key(&mut n, AnimProperty::StrokeColor, 100, AnimValue::Color(Color::WHITE));
        key(&mut n, AnimProperty::FillColor, 0, AnimValue::Color(Color::BLACK));
        key(&mut n, AnimProperty::FillColor, 100, AnimValue::Color(Color::WHITE));
        let (d, page, id) = doc_with(n);
        let node = evaluate(&d, page, 50.0).get(id).unwrap().clone();
        assert!(near(node.size.x, 120.0) && near(node.size.y, 40.0));
        assert!(near(node.opacity, 0.5));
        assert_eq!(node.corner_radii, [10.0; 4]);
        assert!(near(node.stroke.weight, 3.0));
        let PaintKind::Solid(c) = node.fills[0].kind else { panic!() };
        assert!(near(c.r, 0.5));
        let PaintKind::Solid(c) = node.stroke.paints[0].kind else { panic!() };
        assert!(near(c.g, 0.5));
        // Unrotated node: x/y unchanged by the width change.
        assert!(near(node.transform.tx, 100.0) && near(node.transform.ty, 50.0));
    }

    #[test]
    fn color_track_without_solid_fill_is_ignored() {
        let mut n = rect();
        n.fills.clear();
        key(&mut n, AnimProperty::FillColor, 0, AnimValue::Color(Color::WHITE));
        assert_eq!(base_value(&n, AnimProperty::FillColor), None);
        let (d, page, id) = doc_with(n);
        assert!(evaluate(&d, page, 0.0).get(id).unwrap().fills.is_empty());
    }

    #[test]
    fn clips_compose_with_keyframes() {
        let mut n = rect();
        key(&mut n, AnimProperty::Position, 0, AnimValue::Vec2(Vec2::new(100.0, 50.0)));
        key(&mut n, AnimProperty::Position, 1000, AnimValue::Vec2(Vec2::new(300.0, 50.0)));
        key(&mut n, AnimProperty::Opacity, 0, AnimValue::Scalar(0.5));
        let slide = Clip::new(ClipKind::Position, ClipMode::In).dir(ClipDirection::Bottom).amount(100.0).delay(0).dur(1000).ease(Ease::Linear);
        let fade = Clip::new(ClipKind::Opacity, ClipMode::In).amount(0.0).dur(1000).ease(Ease::Linear);
        n.motion.clips = vec![slide, Clip { id: 1, ..fade }];
        // At t=500: keyframed position (200,50) + clip offset (0,50); opacity 0.5 * 0.5.
        let ev = evaluate_node(&n, 500.0);
        let p = ev.position.unwrap();
        assert!(near(p.x, 200.0) && near(p.y, 100.0), "{p:?}");
        assert!(near(ev.opacity.unwrap(), 0.25));
        // Clips are additive/multiplicative on top of the static value too, and hold afterwards.
        n.motion.tracks.clear();
        let ev = evaluate_node(&n, 5000.0);
        assert_eq!(ev.position, Some(Vec2::new(100.0, 50.0)));
        assert!(near(ev.opacity.unwrap(), 1.0));
        // Before the clip: held at its start state.
        let ev = evaluate_node(&n, 0.0);
        assert_eq!(ev.position, Some(Vec2::new(100.0, 150.0)));
        assert_eq!(ev.opacity, Some(0.0));
    }

    #[test]
    fn hidden_clips_do_nothing_and_scale_clips_multiply() {
        let mut n = rect();
        let mut a = Clip::new(ClipKind::Scale, ClipMode::Custom).custom(Vec2::new(2.0, 2.0), Vec2::new(2.0, 2.0)).dur(10);
        a.id = 0;
        let mut b = a.clone();
        b.id = 1;
        n.motion.clips = vec![a.clone(), b];
        assert_eq!(evaluate_node(&n, 5.0).scale, Some(Vec2::new(4.0, 4.0)));
        n.motion.clips[1].visible = false;
        assert_eq!(evaluate_node(&n, 5.0).scale, Some(Vec2::new(2.0, 2.0)));
        n.motion.clips[0].visible = false;
        assert!(evaluate_node(&n, 5.0).is_empty());
    }

    #[test]
    fn size_clip_scales_dimensions() {
        let mut n = rect();
        n.motion.clips = vec![Clip::new(ClipKind::Size, ClipMode::Out).amount(0.5).dir(ClipDirection::Horizontal).dur(100).ease(Ease::Linear)];
        let ev = evaluate_node(&n, 100.0);
        assert!(near(ev.width.unwrap(), 40.0) && near(ev.height.unwrap(), 40.0));
    }

    #[test]
    fn rotation_clip_adds_to_static_rotation() {
        let mut n = rect();
        n.transform = Affine::translate(100.0, 50.0) * Affine::rotate(-30f32.to_radians());
        assert!(near(base_rotation_deg(&n), 30.0));
        n.motion.clips = vec![Clip::new(ClipKind::Rotation, ClipMode::Custom).custom(Vec2::ZERO, Vec2::new(60.0, 0.0)).ease(Ease::Linear).dur(100)];
        let p0 = base_position(&n);
        let (d, page, id) = doc_with(n);
        let e = evaluate(&d, page, 100.0);
        assert!(near(base_rotation_deg(e.get(id).unwrap()), 90.0));
        // Layout position survives the rotation change.
        let p = base_position(e.get(id).unwrap());
        assert!(near(p.x, p0.x) && near(p.y, p0.y), "{p:?} vs {p0:?}");
    }

    #[test]
    fn evaluation_is_idempotent_and_does_not_accumulate() {
        let mut n = rect();
        key(&mut n, AnimProperty::Rotation, 0, AnimValue::Scalar(0.0));
        key(&mut n, AnimProperty::Rotation, 1000, AnimValue::Scalar(45.0));
        key(&mut n, AnimProperty::Scale, 0, AnimValue::Vec2(Vec2::new(1.0, 1.0)));
        key(&mut n, AnimProperty::Scale, 1000, AnimValue::Vec2(Vec2::new(2.0, 2.0)));
        let (d, page, id) = doc_with(n);
        let mut player = MotionPlayer::new(&d, page);
        assert_eq!(player.animated_ids(), &[id]);
        let at = |p: &mut MotionPlayer, t: f32| p.seek(&d, t).get(id).unwrap().transform;
        let a = at(&mut player, 500.0);
        let _ = at(&mut player, 900.0);
        let b = at(&mut player, 500.0);
        assert_eq!(a, b);
        assert_eq!(a, evaluate(&d, page, 500.0).get(id).unwrap().transform);
        assert_eq!(player.document().get(id).unwrap().transform, a);
    }

    #[test]
    fn value_at_variants() {
        let mut n = rect();
        key(&mut n, AnimProperty::Opacity, 0, AnimValue::Scalar(0.0));
        key(&mut n, AnimProperty::Opacity, 100, AnimValue::Scalar(1.0));
        n.motion.clips = vec![Clip::new(ClipKind::Opacity, ClipMode::Custom).custom(Vec2::new(0.5, 0.0), Vec2::new(0.5, 0.0)).dur(10)];
        assert_eq!(keyframed_value_at(&n, AnimProperty::Opacity, 100.0), Some(AnimValue::Scalar(1.0)));
        assert_eq!(value_at(&n, AnimProperty::Opacity, 100.0), Some(AnimValue::Scalar(0.5)));
        assert_eq!(value_at(&n, AnimProperty::Width, 0.0), Some(AnimValue::Scalar(80.0)));
        assert_eq!(keyframed_value_at(&n, AnimProperty::Position, 0.0), Some(AnimValue::Vec2(Vec2::new(100.0, 50.0))));
        // Negative / NaN times behave like 0.
        assert_eq!(keyframed_value_at(&n, AnimProperty::Opacity, -50.0), Some(AnimValue::Scalar(0.0)));
        assert!(evaluate_node(&n, f32::NAN).opacity.unwrap().is_finite());
    }

    #[test]
    fn infinite_clip_keeps_running_past_the_timeline() {
        let mut n = rect();
        n.motion.clips = vec![Clip::new(ClipKind::Rotation, ClipMode::Custom)
            .custom(Vec2::ZERO, Vec2::new(360.0, 0.0))
            .ease(Ease::Linear)
            .dur(1000)
            .repeat(ClipRepeat::Infinite)];
        let r = |t: f32| evaluate_node(&n, t).rotation.unwrap();
        assert!(near(r(250.0), 90.0) && near(r(10_250.0), 90.0));
    }

    /// Rough frame-time measurement for the report: `cargo test -p desigma-doc --release -- --nocapture bench`.
    #[test]
    fn bench_hundreds_of_animated_nodes() {
        let mut d = Document::new("bench");
        let page = d.add_page("P");
        for i in 0..500u64 {
            let mut n = Node::rectangle(NodeId(0), Rect::new((i % 25) as f32 * 40.0, (i / 25) as f32 * 40.0, 30.0, 30.0));
            n.id = d.alloc_id();
            key(&mut n, AnimProperty::Position, 0, AnimValue::Vec2(Vec2::new(0.0, 0.0)));
            key(&mut n, AnimProperty::Position, 1000, AnimValue::Vec2(Vec2::new(50.0, 80.0)));
            key(&mut n, AnimProperty::Rotation, 0, AnimValue::Scalar(0.0));
            key(&mut n, AnimProperty::Rotation, 1000, AnimValue::Scalar(180.0));
            key(&mut n, AnimProperty::Opacity, 0, AnimValue::Scalar(0.0));
            key(&mut n, AnimProperty::Opacity, 1000, AnimValue::Scalar(1.0));
            n.motion.tracks[0].keys[0].easing = Ease::SpringBouncy;
            n.motion.clips = vec![
                Clip::new(ClipKind::Position, ClipMode::In).amount(50.0).dur(600).ease(Ease::bezier(0.07, 0.97, 0.58, 1.0)),
                Clip::new(ClipKind::Scale, ClipMode::In).amount(0.5).dur(600),
            ];
            d.insert(page, n).unwrap();
        }
        let mut player = MotionPlayer::new(&d, page);
        let frames = 600;
        let start = std::time::Instant::now();
        for f in 0..frames {
            let t = (f as f32 / 60.0 * 1000.0) % 1000.0;
            std::hint::black_box(player.seek(&d, t));
        }
        let per_frame = start.elapsed().as_secs_f64() * 1000.0 / frames as f64;
        println!("MotionPlayer::seek, 500 animated nodes (3 tracks + 2 clips each): {per_frame:.4} ms/frame");
        assert!(per_frame < 8.0, "{per_frame} ms per frame");
        let start = std::time::Instant::now();
        let ev = evaluate(&d, page, 400.0);
        println!("evaluate() deep clone of the same document: {:.3} ms", start.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(ev.nodes.len(), d.nodes.len());
    }
}
