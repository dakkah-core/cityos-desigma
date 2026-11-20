//! Undoable animation edits.
//!
//! Each function reads the document and returns a [`Command`] (nothing is mutated); run it with
//! [`History::exec`](crate::History::exec) or [`Command::apply`]. Motion edits compile to
//! [`Command::SetNodeMotion`] (a snapshot swap of one node's tracks and clips) or
//! [`Command::SetTimeline`], so every helper is exactly one undo step, even when it touches many
//! nodes (those are wrapped in a [`Command::Batch`]).
//!
//! Functions that change what a selection refers to (move, stretch) also return the updated
//! [`KeyRef`]s so the UI can keep the selection.
//!
//! Always pass the *stored* document, never an evaluated copy from [`MotionPlayer`]: commands
//! built from animated values would bake them into the static properties.

use super::clip::Clip;
use super::easing::Ease;
use super::eval::{base_value, keyframed_value_at, EvaluatedNode};
use super::presets::Preset;
use super::track::NodeMotion;
use super::value::{AnimProperty, AnimValue};
use super::TimelineSettings;
use crate::{Command, DocError, Document, NodeId};
use std::collections::{BTreeMap, BTreeSet};

/// Identifies one keyframe: node, property and time (unique per track).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KeyRef {
    pub node: NodeId,
    pub property: AnimProperty,
    pub time_ms: u32,
}

impl KeyRef {
    pub fn new(node: NodeId, property: AnimProperty, time_ms: u32) -> Self {
        Self { node, property, time_ms }
    }
}

/// Rounds a fractional playhead time to the millisecond keyframes live on.
pub fn to_ms(t_ms: f32) -> u32 {
    if t_ms.is_finite() {
        t_ms.round().clamp(0.0, u32::MAX as f32) as u32
    } else {
        0
    }
}

fn nothing(what: &'static str) -> DocError {
    DocError::InvalidMotion(what)
}

fn combine(mut cmds: Vec<Command>) -> Result<Command, DocError> {
    match cmds.len() {
        0 => Err(nothing("nothing to change")),
        1 => Ok(cmds.remove(0)),
        _ => Ok(Command::Batch(cmds)),
    }
}

/// `SetNodeMotion` for `node` after running `f` on a copy of its motion; `f` returns whether it
/// changed anything.
fn edit_motion(doc: &Document, node: NodeId, f: impl FnOnce(&mut NodeMotion) -> bool) -> Result<Option<Command>, DocError> {
    let mut motion = doc.get(node).ok_or(DocError::NotFound(node))?.motion.clone();
    if !f(&mut motion) {
        return Ok(None);
    }
    motion.normalize();
    Ok(Some(Command::SetNodeMotion { node, motion: Box::new(motion) }))
}

fn single(doc: &Document, node: NodeId, f: impl FnOnce(&mut NodeMotion) -> bool) -> Result<Command, DocError> {
    edit_motion(doc, node, f)?.ok_or_else(|| nothing("nothing to change"))
}

fn group_by_node(keys: &[KeyRef]) -> BTreeMap<NodeId, Vec<KeyRef>> {
    let mut m: BTreeMap<NodeId, Vec<KeyRef>> = BTreeMap::new();
    for k in keys {
        m.entry(k.node).or_default().push(*k);
    }
    m
}

// ---- keyframes --------------------------------------------------------------------------------

/// Adds a keyframe of `property` at `time_ms`, or updates the one that is there.
///
/// `value` defaults to what the property is at that time (its keyframed value, else the static
/// one), so a bare "add keyframe" at the playhead keeps the look. `easing` of the segment that
/// starts at the new keyframe defaults to the easing of the segment it splits (linear for the
/// first keyframe), and an existing keyframe keeps its easing unless given. Creates the track when
/// the property had none. Fails when the node cannot animate the property (colour without a solid
/// paint) or `value` has the wrong shape.
pub fn add_keyframe(
    doc: &Document,
    node: NodeId,
    property: AnimProperty,
    time_ms: u32,
    value: Option<AnimValue>,
    easing: Option<Ease>,
) -> Result<Command, DocError> {
    let n = doc.get(node).ok_or(DocError::NotFound(node))?;
    let value = match value {
        Some(v) => v,
        None => keyframed_value_at(n, property, time_ms as f32).ok_or_else(|| nothing("property cannot be animated on this node"))?,
    };
    if !property.accepts(&value) || !value.is_finite() {
        return Err(nothing("value does not fit the property"));
    }
    single(doc, node, |m| {
        m.ensure_track(property).set_key(time_ms, value, easing.map(|e| e.sanitized()));
        true
    })
}

/// The keyframe diamond toggle: removes the keyframe at `time_ms` if there is one, else adds one
/// holding the current value.
pub fn toggle_keyframe(doc: &Document, node: NodeId, property: AnimProperty, time_ms: u32) -> Result<Command, DocError> {
    let has = doc.get(node).ok_or(DocError::NotFound(node))?.motion.track(property).is_some_and(|t| t.key_at(time_ms).is_some());
    if has {
        remove_keyframes(doc, &[KeyRef::new(node, property, time_ms)])
    } else {
        add_keyframe(doc, node, property, time_ms, None, None)
    }
}

/// Removes keyframes (any number, any nodes). Tracks left empty disappear.
pub fn remove_keyframes(doc: &Document, keys: &[KeyRef]) -> Result<Command, DocError> {
    let mut cmds = Vec::new();
    for (node, ks) in group_by_node(keys) {
        let c = edit_motion(doc, node, |m| {
            let mut changed = false;
            for k in &ks {
                changed |= m.track_mut(k.property).and_then(|t| t.remove_key(k.time_ms)).is_some();
            }
            changed
        })?;
        cmds.extend(c);
    }
    combine(cmds)
}

/// Deletes a whole track (the property row's keyframes).
pub fn remove_track(doc: &Document, node: NodeId, property: AnimProperty) -> Result<Command, DocError> {
    single(doc, node, |m| {
        let n = m.tracks.len();
        m.tracks.retain(|t| t.property != property);
        m.tracks.len() != n
    })
}

/// Shows or hides a track (the eye of its timeline row). A hidden track keeps its keyframes but
/// is ignored when evaluating.
pub fn set_track_visible(doc: &Document, node: NodeId, property: AnimProperty, visible: bool) -> Result<Command, DocError> {
    single(doc, node, |m| match m.track_mut(property) {
        Some(t) if t.visible != visible => {
            t.visible = visible;
            true
        }
        _ => false,
    })
}

/// Moves everything a layer animates (all keyframes of every track and all clips) by `delta_ms`
/// as one undo step: dragging the layer's summary bar. The delta is limited so nothing goes
/// below 0.
pub fn shift_node_motion(doc: &Document, node: NodeId, delta_ms: i64) -> Result<Command, DocError> {
    let m = &doc.get(node).ok_or(DocError::NotFound(node))?.motion;
    let earliest = m.tracks.iter().filter_map(|t| t.start_ms()).chain(m.clips.iter().map(|c| c.delay_ms)).min().ok_or_else(|| nothing("nothing animated"))?;
    let delta = delta_ms.max(-(earliest as i64));
    single(doc, node, |m| {
        if delta == 0 {
            return false;
        }
        let mv = |t: u32| (t as i64 + delta).clamp(0, u32::MAX as i64) as u32;
        for t in &mut m.tracks {
            for k in &mut t.keys {
                k.time_ms = mv(k.time_ms);
            }
        }
        for c in &mut m.clips {
            c.delay_ms = mv(c.delay_ms);
        }
        true
    })
}

/// Rebuilds the selected keyframes at new times given by `map`; unselected keyframes at a target
/// time are replaced. Returns the command and the moved keyframes' new refs.
fn retime(doc: &Document, keys: &[KeyRef], map: impl Fn(u32) -> u32) -> Result<(Command, Vec<KeyRef>), DocError> {
    let mut cmds = Vec::new();
    let mut out = BTreeSet::new();
    for (node, ks) in group_by_node(keys) {
        let c = edit_motion(doc, node, |m| {
            let mut changed = false;
            let props: BTreeSet<AnimProperty> = ks.iter().map(|k| k.property).collect();
            for prop in props {
                let times: BTreeSet<u32> = ks.iter().filter(|k| k.property == prop).map(|k| k.time_ms).collect();
                let Some(track) = m.track_mut(prop) else { continue };
                let mut moved = Vec::new();
                track.keys.retain(|k| {
                    if times.contains(&k.time_ms) {
                        moved.push(*k);
                        false
                    } else {
                        true
                    }
                });
                // Moved keys go last so they win ties with unselected keyframes.
                for mut k in moved {
                    let t = map(k.time_ms);
                    changed |= t != k.time_ms;
                    k.time_ms = t;
                    out.insert(KeyRef::new(node, prop, t));
                    track.keys.push(k);
                }
            }
            changed
        })?;
        cmds.extend(c);
    }
    Ok((combine(cmds)?, out.into_iter().collect()))
}

/// Moves keyframes by `delta_ms` (negative = earlier), keeping their relative spacing. The
/// delta is limited so no selected keyframe goes below 0. Unselected keyframes at a destination
/// time are replaced by the moved one. Returns the command and the new refs.
pub fn move_keyframes(doc: &Document, keys: &[KeyRef], delta_ms: i64) -> Result<(Command, Vec<KeyRef>), DocError> {
    let min_t = keys.iter().map(|k| k.time_ms).min().ok_or_else(|| nothing("no keyframes selected"))?;
    let delta = delta_ms.max(-(min_t as i64));
    retime(doc, keys, |t| (t as i64 + delta).clamp(0, u32::MAX as i64) as u32)
}

/// Time-stretches a group of keyframes about `pivot_ms`: `t' = pivot + (t - pivot) * factor`
/// (rounded to ms, never below 0). A factor of 2 doubles all distances to the pivot; the usual
/// pivot is the selection's earliest keyframe (or the playhead). Keyframes that collide after
/// rounding merge into one.
pub fn stretch_keyframes(doc: &Document, keys: &[KeyRef], pivot_ms: f32, factor: f32) -> Result<(Command, Vec<KeyRef>), DocError> {
    if !factor.is_finite() || factor <= 0.0 || !pivot_ms.is_finite() {
        return Err(nothing("invalid stretch"));
    }
    retime(doc, keys, |t| to_ms(pivot_ms + (t as f32 - pivot_ms) * factor))
}

/// Sets the value of one keyframe.
pub fn set_keyframe_value(doc: &Document, key: KeyRef, value: AnimValue) -> Result<Command, DocError> {
    if !key.property.accepts(&value) || !value.is_finite() {
        return Err(nothing("value does not fit the property"));
    }
    single(doc, key.node, |m| match m.track_mut(key.property).and_then(|t| t.key_index(key.time_ms).map(|i| (t, i))) {
        Some((t, i)) => {
            let changed = t.keys[i].value != value;
            t.keys[i].value = value;
            changed
        }
        None => false,
    })
}

/// Sets the easing of the segments that start at the given keyframes (several at once: the
/// "select the segments, pick an easing" flow). The last keyframe of a track has no segment but
/// still stores the easing.
pub fn set_segment_easing(doc: &Document, keys: &[KeyRef], easing: Ease) -> Result<Command, DocError> {
    let easing = easing.sanitized();
    let mut cmds = Vec::new();
    for (node, ks) in group_by_node(keys) {
        let c = edit_motion(doc, node, |m| {
            let mut changed = false;
            for k in &ks {
                if let Some(t) = m.track_mut(k.property) {
                    if let Some(i) = t.key_index(k.time_ms) {
                        changed |= t.keys[i].easing != easing;
                        t.keys[i].easing = easing;
                    }
                }
            }
            changed
        })?;
        cmds.extend(c);
    }
    combine(cmds)
}

// ---- clips ------------------------------------------------------------------------------------

/// Appends a clip to a node. Its id is replaced by a fresh one, returned with the command.
pub fn add_clip(doc: &Document, node: NodeId, mut clip: Clip) -> Result<(Command, u32), DocError> {
    let id = doc.get(node).ok_or(DocError::NotFound(node))?.motion.next_clip_id();
    clip.id = id;
    clip.sanitize();
    let cmd = single(doc, node, |m| {
        m.clips.push(clip);
        true
    })?;
    Ok((cmd, id))
}

pub fn remove_clip(doc: &Document, node: NodeId, clip_id: u32) -> Result<Command, DocError> {
    single(doc, node, |m| {
        let n = m.clips.len();
        m.clips.retain(|c| c.id != clip_id);
        m.clips.len() != n
    })
}

/// Replaces the clip with the same id (settings popover edits, trimming, moving, the eye toggle).
pub fn update_clip(doc: &Document, node: NodeId, mut clip: Clip) -> Result<Command, DocError> {
    clip.sanitize();
    single(doc, node, |m| match m.clips.iter_mut().find(|c| c.id == clip.id) {
        Some(c) if *c != clip => {
            *c = clip;
            true
        }
        _ => false,
    })
}

/// Applies a library preset to one or more nodes, starting at `start_ms`: the preset's clips are
/// appended to each node. One undo step for all nodes.
pub fn apply_preset(doc: &Document, nodes: &[NodeId], preset: &Preset, start_ms: u32) -> Result<Command, DocError> {
    let mut cmds = Vec::new();
    for &node in nodes {
        let first = doc.get(node).ok_or(DocError::NotFound(node))?.motion.next_clip_id();
        let clips = preset.instantiate(start_ms, first);
        cmds.extend(edit_motion(doc, node, |m| {
            m.clips.extend(clips);
            true
        })?);
    }
    combine(cmds)
}

// ---- timeline ---------------------------------------------------------------------------------

/// Sets the timeline settings of a page (values are clamped to valid ranges).
pub fn set_timeline(doc: &Document, page: NodeId, settings: TimelineSettings) -> Result<Command, DocError> {
    if doc.get(page).is_none() {
        return Err(DocError::NotFound(page));
    }
    let settings = settings.sanitized();
    if doc.timeline(page) == settings {
        return Err(nothing("nothing to change"));
    }
    Ok(Command::SetTimeline { page, settings })
}

// ---- inspector --------------------------------------------------------------------------------

/// A plain, un-animated property edit as an `Update` command: writes `value` into the node's
/// static property.
pub fn set_static_value(doc: &Document, node: NodeId, property: AnimProperty, value: AnimValue) -> Result<Command, DocError> {
    let base = doc.get(node).ok_or(DocError::NotFound(node))?;
    if !property.accepts(&value) || !value.is_finite() {
        return Err(nothing("value does not fit the property"));
    }
    if base_value(base, property).is_none() {
        return Err(nothing("property cannot be animated on this node"));
    }
    let mut ev = EvaluatedNode::default();
    ev.set(property, value);
    let mut out = base.clone();
    ev.apply_to(base, &mut out);
    Ok(Command::Update { node: Box::new(out) })
}

/// Auto-key: the edit the inspector makes when a field changes at playhead time `t_ms`.
///
/// If the node already has a keyframe track for `property`, the keyframe at `t_ms` is updated or
/// inserted (so editing a value away from existing keyframes adds a keyframe, as in Figma);
/// otherwise the static property changes and no animation is created (use [`add_keyframe`] /
/// [`toggle_keyframe`] to start animating a property).
pub fn auto_key(doc: &Document, node: NodeId, property: AnimProperty, value: AnimValue, t_ms: f32) -> Result<Command, DocError> {
    let n = doc.get(node).ok_or(DocError::NotFound(node))?;
    if n.motion.track(property).is_some() {
        add_keyframe(doc, node, property, to_ms(t_ms), Some(value), None)
    } else {
        set_static_value(doc, node, property, value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motion::{find_preset, ClipKind, ClipMode, ClipRepeat};
    use crate::{History, Node, Rect, Vec2};

    fn setup() -> (Document, NodeId, NodeId, NodeId) {
        let mut d = Document::new("t");
        let page = d.add_page("P");
        let a = d.alloc_id();
        d.insert(page, Node::rectangle(a, Rect::new(10.0, 20.0, 100.0, 50.0))).unwrap();
        let b = d.alloc_id();
        d.insert(page, Node::rectangle(b, Rect::new(0.0, 0.0, 10.0, 10.0))).unwrap();
        (d, page, a, b)
    }

    fn s(v: f32) -> AnimValue {
        AnimValue::Scalar(v)
    }

    fn times(d: &Document, n: NodeId, p: AnimProperty) -> Vec<u32> {
        d.get(n).unwrap().motion.track(p).map(|t| t.keys.iter().map(|k| k.time_ms).collect()).unwrap_or_default()
    }

    /// Runs `cmd`, checks undo restores the exact node motion and redo reapplies it; leaves `d`
    /// unchanged and returns the documents before and after.
    fn roundtrip(orig: &Document, cmd: Command) -> (Document, Document) {
        let mut doc = orig.clone();
        let d = &mut doc;
        let before = d.clone();
        let mut h = History::default();
        h.exec(d, cmd).unwrap();
        let after = d.clone();
        h.undo(d).unwrap();
        for (id, n) in &before.nodes {
            assert_eq!(&d.nodes[id], n, "undo restores {id:?}");
        }
        assert_eq!(d.timelines, before.timelines);
        h.redo(d).unwrap();
        for (id, n) in &after.nodes {
            assert_eq!(&d.nodes[id], n, "redo reapplies {id:?}");
        }
        assert_eq!(d.timelines, after.timelines);
        h.undo(d).unwrap();
        (before, after)
    }

    #[test]
    fn add_keyframe_creates_track_with_current_value() {
        let (mut d, _, a, _) = setup();
        let cmd = add_keyframe(&d, a, AnimProperty::Opacity, 300, None, None).unwrap();
        let (_, after) = roundtrip(&d, cmd);
        let t = after.get(a).unwrap().motion.track(AnimProperty::Opacity).unwrap();
        assert_eq!(t.keys.len(), 1);
        assert_eq!((t.keys[0].time_ms, t.keys[0].value), (300, s(1.0)));
        // Position captures the layout position.
        let cmd = add_keyframe(&d, a, AnimProperty::Position, 0, None, None).unwrap();
        d = roundtrip(&d, cmd).1;
        assert_eq!(d.get(a).unwrap().motion.track(AnimProperty::Position).unwrap().keys[0].value, AnimValue::Vec2(Vec2::new(10.0, 20.0)));
    }

    #[test]
    fn add_keyframe_validates() {
        let (mut d, _, a, _) = setup();
        assert!(add_keyframe(&d, a, AnimProperty::Opacity, 0, Some(AnimValue::Vec2(Vec2::ZERO)), None).is_err());
        assert!(add_keyframe(&d, a, AnimProperty::Opacity, 0, Some(s(f32::NAN)), None).is_err());
        assert!(add_keyframe(&d, NodeId(999), AnimProperty::Opacity, 0, None, None).is_err());
        d.get_mut(a).unwrap().fills.clear();
        assert!(add_keyframe(&d, a, AnimProperty::FillColor, 0, None, None).is_err());
    }

    #[test]
    fn add_over_existing_updates_in_place() {
        let (mut d, _, a, _) = setup();
        let c = add_keyframe(&d, a, AnimProperty::Opacity, 100, Some(s(0.2)), Some(Ease::EaseOut)).unwrap();
        d = roundtrip(&d, c).1;
        let c = add_keyframe(&d, a, AnimProperty::Opacity, 100, Some(s(0.9)), None).unwrap();
        d = roundtrip(&d, c).1;
        let t = d.get(a).unwrap().motion.track(AnimProperty::Opacity).unwrap();
        assert_eq!(t.keys.len(), 1);
        assert_eq!((t.keys[0].value, t.keys[0].easing), (s(0.9), Ease::EaseOut));
    }

    #[test]
    fn toggle_adds_then_removes_and_drops_empty_track() {
        let (mut d, _, a, _) = setup();
        let c = toggle_keyframe(&d, a, AnimProperty::Rotation, 500).unwrap();
        d = roundtrip(&d, c).1;
        assert_eq!(times(&d, a, AnimProperty::Rotation), vec![500]);
        let c = toggle_keyframe(&d, a, AnimProperty::Rotation, 500).unwrap();
        d = roundtrip(&d, c).1;
        assert!(d.get(a).unwrap().motion.is_empty());
    }

    fn keyed(d: &mut Document, n: NodeId, p: AnimProperty, ks: &[(u32, f32)]) {
        for (t, v) in ks {
            let c = add_keyframe(d, n, p, *t, Some(s(*v)), None).unwrap();
            c.apply(d).unwrap();
        }
    }

    #[test]
    fn remove_multiple_keyframes_across_nodes() {
        let (mut d, _, a, b) = setup();
        keyed(&mut d, a, AnimProperty::Opacity, &[(0, 0.0), (500, 0.5), (1000, 1.0)]);
        keyed(&mut d, b, AnimProperty::Opacity, &[(0, 0.0), (1000, 1.0)]);
        let cmd = remove_keyframes(
            &d,
            &[KeyRef::new(a, AnimProperty::Opacity, 500), KeyRef::new(b, AnimProperty::Opacity, 0), KeyRef::new(b, AnimProperty::Opacity, 1000)],
        )
        .unwrap();
        assert!(matches!(cmd, Command::Batch(_)));
        let (_, after) = roundtrip(&d, cmd);
        assert_eq!(times(&after, a, AnimProperty::Opacity), vec![0, 1000]);
        assert!(after.get(b).unwrap().motion.is_empty());
        assert!(remove_keyframes(&d, &[KeyRef::new(a, AnimProperty::Opacity, 123)]).is_err());
    }

    #[test]
    fn move_keyframes_group_clamps_at_zero_and_replaces_collisions() {
        let (mut d, _, a, _) = setup();
        keyed(&mut d, a, AnimProperty::Opacity, &[(100, 0.0), (300, 0.5), (700, 1.0)]);
        keyed(&mut d, a, AnimProperty::Rotation, &[(100, 0.0), (300, 90.0)]);
        let sel = [KeyRef::new(a, AnimProperty::Opacity, 100), KeyRef::new(a, AnimProperty::Opacity, 300), KeyRef::new(a, AnimProperty::Rotation, 100)];
        let (cmd, new) = move_keyframes(&d, &sel, 400).unwrap();
        let (_, after) = roundtrip(&d, cmd);
        assert_eq!(times(&after, a, AnimProperty::Opacity), vec![500, 700], "300 -> 700 replaces the key at 700");
        assert_eq!(after.get(a).unwrap().motion.track(AnimProperty::Opacity).unwrap().key_at(700).unwrap().value, s(0.5));
        assert_eq!(times(&after, a, AnimProperty::Rotation), vec![300, 500], "rotation key 100 -> 500; the unselected one at 300 stays");
        assert_eq!(
            new,
            vec![KeyRef::new(a, AnimProperty::Rotation, 500), KeyRef::new(a, AnimProperty::Opacity, 500), KeyRef::new(a, AnimProperty::Opacity, 700)]
        );
        // Moving earlier than 0 clamps the whole group (spacing preserved).
        let (cmd, new) = move_keyframes(&d, &sel[..2], -5000).unwrap();
        let (_, after) = roundtrip(&d, cmd);
        assert_eq!(times(&after, a, AnimProperty::Opacity), vec![0, 200, 700]);
        assert_eq!(new.len(), 2);
        assert!(move_keyframes(&d, &[], 5).is_err());
    }

    #[test]
    fn stretch_keyframes_scales_about_pivot() {
        let (mut d, _, a, b) = setup();
        keyed(&mut d, a, AnimProperty::Opacity, &[(100, 0.0), (300, 0.5), (700, 1.0)]);
        keyed(&mut d, b, AnimProperty::Width, &[(100, 5.0), (500, 9.0)]);
        let sel = [
            KeyRef::new(a, AnimProperty::Opacity, 100),
            KeyRef::new(a, AnimProperty::Opacity, 300),
            KeyRef::new(a, AnimProperty::Opacity, 700),
            KeyRef::new(b, AnimProperty::Width, 100),
            KeyRef::new(b, AnimProperty::Width, 500),
        ];
        let (cmd, new) = stretch_keyframes(&d, &sel, 100.0, 2.0).unwrap();
        let (_, after) = roundtrip(&d, cmd);
        assert_eq!(times(&after, a, AnimProperty::Opacity), vec![100, 500, 1300]);
        assert_eq!(times(&after, b, AnimProperty::Width), vec![100, 900]);
        assert_eq!(new.len(), 5);
        // Shrinking works too, values stay attached to their keyframes.
        let (cmd, _) = stretch_keyframes(&d, &sel[..3], 100.0, 0.5).unwrap();
        let (_, after) = roundtrip(&d, cmd);
        assert_eq!(times(&after, a, AnimProperty::Opacity), vec![100, 200, 400]);
        assert_eq!(after.get(a).unwrap().motion.track(AnimProperty::Opacity).unwrap().key_at(200).unwrap().value, s(0.5));
        assert!(stretch_keyframes(&d, &sel, 100.0, 0.0).is_err());
        assert!(stretch_keyframes(&d, &sel, 100.0, 1.0).is_err(), "no-op reports nothing to change");
    }

    #[test]
    fn set_value_and_segment_easing() {
        let (mut d, _, a, _) = setup();
        keyed(&mut d, a, AnimProperty::Opacity, &[(0, 0.0), (500, 1.0), (900, 0.0)]);
        let k0 = KeyRef::new(a, AnimProperty::Opacity, 0);
        let k1 = KeyRef::new(a, AnimProperty::Opacity, 500);
        let (_, after) = roundtrip(&d, set_keyframe_value(&d, k1, s(0.7)).unwrap());
        assert_eq!(after.get(a).unwrap().motion.track(AnimProperty::Opacity).unwrap().key_at(500).unwrap().value, s(0.7));
        assert!(set_keyframe_value(&d, k1, AnimValue::Vec2(Vec2::ZERO)).is_err());
        assert!(set_keyframe_value(&d, KeyRef::new(a, AnimProperty::Opacity, 42), s(0.1)).is_err());
        let easing = Ease::bezier(0.07, 0.97, 0.58, 1.0);
        let (_, after) = roundtrip(&d, set_segment_easing(&d, &[k0, k1], easing).unwrap());
        let t = after.get(a).unwrap().motion.track(AnimProperty::Opacity).unwrap();
        assert_eq!((t.keys[0].easing, t.keys[1].easing, t.keys[2].easing), (easing, easing, Ease::Linear));
    }

    #[test]
    fn clip_add_update_remove() {
        let (mut d, _, a, _) = setup();
        let (cmd, id0) = add_clip(&d, a, Clip::new(ClipKind::Position, ClipMode::In)).unwrap();
        d = roundtrip(&d, cmd).1;
        let (cmd, id1) = add_clip(&d, a, Clip::new(ClipKind::Opacity, ClipMode::In).delay(200)).unwrap();
        d = roundtrip(&d, cmd).1;
        assert_ne!(id0, id1);
        let mut c = d.get(a).unwrap().motion.clip(id1).unwrap().clone();
        c.delay_ms = 900;
        c.repeat = ClipRepeat::Infinite;
        c.visible = false;
        d = roundtrip(&d, update_clip(&d, a, c.clone()).unwrap()).1;
        assert_eq!(d.get(a).unwrap().motion.clip(id1).unwrap(), &c);
        assert!(update_clip(&d, a, c).is_err(), "unchanged clip is a no-op");
        d = roundtrip(&d, remove_clip(&d, a, id0).unwrap()).1;
        assert_eq!(d.get(a).unwrap().motion.clips.len(), 1);
        assert!(remove_clip(&d, a, 77).is_err());
    }

    #[test]
    fn apply_preset_is_one_undo_step_over_many_nodes() {
        let (mut d, _, a, b) = setup();
        let burst = find_preset("Burst").unwrap();
        let cmd = apply_preset(&d, &[a, b], burst, 997).unwrap();
        let mut h = History::default();
        h.exec(&mut d, cmd).unwrap();
        for n in [a, b] {
            let m = &d.get(n).unwrap().motion;
            assert_eq!(m.clips.len(), 4);
            assert!(m.clips.iter().all(|c| c.delay_ms == 997));
        }
        // Applying again stacks with fresh ids.
        let again = apply_preset(&d, &[a], burst, 0).unwrap();
        h.exec(&mut d, again).unwrap();
        let ids: BTreeSet<u32> = d.get(a).unwrap().motion.clips.iter().map(|c| c.id).collect();
        assert_eq!(ids.len(), 8);
        assert!(h.undo(&mut d).unwrap());
        assert_eq!(d.get(a).unwrap().motion.clips.len(), 4);
        assert!(h.undo(&mut d).unwrap());
        assert!(d.get(a).unwrap().motion.is_empty() && d.get(b).unwrap().motion.is_empty());
        assert!(!h.undo(&mut d).unwrap(), "exactly one step per apply");
        h.redo(&mut d).unwrap();
        assert_eq!(d.get(b).unwrap().motion.clips.len(), 4);
    }

    #[test]
    fn timeline_settings_command() {
        let (mut d, page, _, _) = setup();
        assert!(d.timelines.is_empty());
        let cmd = set_timeline(&d, page, TimelineSettings { duration_ms: 5000, fps: 30, looping: true }).unwrap();
        let (_, after) = roundtrip(&d, cmd);
        assert_eq!(after.timeline(page), TimelineSettings { duration_ms: 5000, fps: 30, looping: true });
        assert_eq!(after.timelines.len(), 1);
        d = after;
        // Back to defaults removes the entry again; no-ops are rejected.
        let back = set_timeline(&d, page, TimelineSettings::default()).unwrap();
        let (_, after) = roundtrip(&d, back);
        assert!(after.timelines.is_empty());
        assert!(set_timeline(&d, page, TimelineSettings { duration_ms: 5000, fps: 30, looping: true }).is_err());
        // Out-of-range values are clamped.
        let cmd = set_timeline(&d, page, TimelineSettings { duration_ms: 0, fps: 1000, looping: false }).unwrap();
        cmd.apply(&mut d).unwrap();
        assert_eq!((d.timeline(page).duration_ms, d.timeline(page).fps), (1, 240));
        assert!(set_timeline(&d, NodeId(999), TimelineSettings::default()).is_err());
    }

    #[test]
    fn auto_key_inserts_when_animated_and_edits_statically_otherwise() {
        let (mut d, _, a, _) = setup();
        // No track: plain property edit through Update.
        let cmd = auto_key(&d, a, AnimProperty::Opacity, s(0.25), 400.0).unwrap();
        assert!(matches!(cmd, Command::Update { .. }));
        let (_, after) = roundtrip(&d, cmd);
        assert_eq!(after.get(a).unwrap().opacity, 0.25);
        assert!(after.get(a).unwrap().motion.is_empty());
        // With a track: keyframes at the playhead.
        keyed(&mut d, a, AnimProperty::Opacity, &[(0, 0.0)]);
        let cmd = auto_key(&d, a, AnimProperty::Opacity, s(1.0), 1000.4).unwrap();
        assert!(matches!(cmd, Command::SetNodeMotion { .. }));
        let (_, after) = roundtrip(&d, cmd);
        assert_eq!(times(&after, a, AnimProperty::Opacity), vec![0, 1000]);
        assert_eq!(after.get(a).unwrap().opacity, 1.0, "static value untouched");
        // Editing at an existing key updates it.
        d = after;
        let (_, after) = roundtrip(&d, auto_key(&d, a, AnimProperty::Opacity, s(0.4), 0.0).unwrap());
        assert_eq!(after.get(a).unwrap().motion.track(AnimProperty::Opacity).unwrap().key_at(0).unwrap().value, s(0.4));
    }

    #[test]
    fn static_edits_of_transform_properties() {
        let (d, _, a, _) = setup();
        let (_, after) = roundtrip(&d, set_static_value(&d, a, AnimProperty::Position, AnimValue::Vec2(Vec2::new(50.0, 60.0))).unwrap());
        let n = after.get(a).unwrap();
        assert!((n.transform.tx - 50.0).abs() < 1e-4 && (n.transform.ty - 60.0).abs() < 1e-4);
        let (_, after) = roundtrip(&d, set_static_value(&d, a, AnimProperty::Rotation, s(90.0)).unwrap());
        assert!((crate::motion::base_rotation_deg(after.get(a).unwrap()) - 90.0).abs() < 1e-3);
        let (_, after) = roundtrip(&d, set_static_value(&d, a, AnimProperty::Width, s(200.0)).unwrap());
        assert_eq!(after.get(a).unwrap().size.x, 200.0);
        let (_, after) = roundtrip(&d, set_static_value(&d, a, AnimProperty::FillColor, AnimValue::Color(crate::Color::BLACK)).unwrap());
        assert!(matches!(after.get(a).unwrap().fills[0].kind, crate::PaintKind::Solid(c) if c == crate::Color::BLACK));
    }

    #[test]
    fn remove_track_and_node_removal_undo_keeps_motion() {
        let (mut d, _, a, _) = setup();
        keyed(&mut d, a, AnimProperty::Opacity, &[(0, 0.0), (100, 1.0)]);
        keyed(&mut d, a, AnimProperty::Width, &[(0, 10.0)]);
        let (_, after) = roundtrip(&d, remove_track(&d, a, AnimProperty::Opacity).unwrap());
        assert!(after.get(a).unwrap().motion.track(AnimProperty::Opacity).is_none());
        assert!(remove_track(&d, a, AnimProperty::Height).is_err());
        // Deleting and restoring the node brings its animation back.
        let mut h = History::default();
        h.exec(&mut d, Command::Remove { id: a }).unwrap();
        h.undo(&mut d).unwrap();
        assert_eq!(d.get(a).unwrap().motion.tracks.len(), 2);
    }

    #[test]
    fn hiding_a_track_is_one_undo_step_and_survives_serde() {
        let (mut d, _, a, _) = setup();
        let mut h = History::default();
        for (t, v) in [(0, 0.0), (1000, 1.0)] {
            let c = add_keyframe(&d, a, AnimProperty::Opacity, t, Some(s(v)), None).unwrap();
            h.exec(&mut d, c).unwrap();
        }
        assert!(d.get(a).unwrap().motion.track(AnimProperty::Opacity).unwrap().visible);
        let c = set_track_visible(&d, a, AnimProperty::Opacity, false).unwrap();
        let (_, after) = roundtrip(&d, c.clone());
        let n = after.get(a).unwrap();
        assert!(!n.motion.track(AnimProperty::Opacity).unwrap().visible);
        assert_eq!(n.motion.track(AnimProperty::Opacity).unwrap().keys.len(), 2, "keys are kept");
        assert!(crate::motion::evaluate_node(n, 500.0).opacity.is_none(), "hidden tracks are not evaluated");
        assert!(!n.motion.animates(AnimProperty::Opacity));
        let back = Document::from_json(&after.to_json().unwrap()).unwrap();
        assert!(!back.get(a).unwrap().motion.track(AnimProperty::Opacity).unwrap().visible);
        // visible tracks serialize without the flag
        assert!(
            !d.to_json().unwrap().contains("visible\":true,\"keys")
                && !d.get(a).map(|n| serde_json::to_string(&n.motion).unwrap()).unwrap().contains("visible")
        );
        assert!(set_track_visible(&d, a, AnimProperty::Opacity, true).is_err(), "no-op is an error");
        assert!(set_track_visible(&d, a, AnimProperty::Width, false).is_err());
    }

    #[test]
    fn shifting_a_layer_moves_keys_and_clips_together() {
        let (mut d, _, a, _) = setup();
        let mut h = History::default();
        let c = add_keyframe(&d, a, AnimProperty::Opacity, 100, Some(s(0.0)), None).unwrap();
        h.exec(&mut d, c).unwrap();
        let c = add_keyframe(&d, a, AnimProperty::Opacity, 600, Some(s(1.0)), None).unwrap();
        h.exec(&mut d, c).unwrap();
        let c = add_keyframe(&d, a, AnimProperty::Rotation, 300, Some(s(0.0)), None).unwrap();
        h.exec(&mut d, c).unwrap();
        let (c, _) = add_clip(&d, a, Clip::new(ClipKind::Position, ClipMode::In).delay(250)).unwrap();
        h.exec(&mut d, c).unwrap();
        let before = d.get(a).unwrap().motion.span_ms(2000).unwrap();
        let cmd = shift_node_motion(&d, a, 400).unwrap();
        let (_, after) = roundtrip(&d, cmd);
        let m = &after.get(a).unwrap().motion;
        assert_eq!(times(&after, a, AnimProperty::Opacity), vec![500, 1000]);
        assert_eq!(times(&after, a, AnimProperty::Rotation), vec![700]);
        assert_eq!(m.clips[0].delay_ms, 650);
        let span = m.span_ms(2000).unwrap();
        assert_eq!(span.0, before.0 + 400);
        // limited so nothing goes below 0
        let (_, after) = roundtrip(&d, shift_node_motion(&d, a, -5000).unwrap());
        assert_eq!(times(&after, a, AnimProperty::Opacity), vec![0, 500]);
        assert_eq!(after.get(a).unwrap().motion.clips[0].delay_ms, 150);
        assert!(shift_node_motion(&d, NodeId(9999), 5).is_err());
        let (d2, _, _, b) = setup();
        assert!(shift_node_motion(&d2, b, 5).is_err(), "nothing animated");
    }

    #[test]
    fn effect_properties_animate_the_first_shadow_and_blur() {
        use crate::{Color, Effect};
        let (mut d, _, a, b) = setup();
        let mut n = d.get(a).unwrap().clone();
        n.effects = vec![
            Effect::LayerBlur { radius: 2.0, visible: true },
            Effect::DropShadow { color: Color::rgba(0, 0, 0, 64), offset: Vec2::new(0.0, 4.0), radius: 4.0, spread: 1.0, visible: true, show_behind: false },
            Effect::DropShadow { color: Color::BLACK, offset: Vec2::new(9.0, 9.0), radius: 9.0, spread: 0.0, visible: true, show_behind: false },
        ];
        *d.get_mut(a).unwrap() = n;
        assert_eq!(base_value(d.get(a).unwrap(), AnimProperty::ShadowX), Some(s(0.0)));
        assert_eq!(base_value(d.get(a).unwrap(), AnimProperty::ShadowY), Some(s(4.0)));
        assert_eq!(base_value(d.get(a).unwrap(), AnimProperty::ShadowBlur), Some(s(4.0)));
        assert_eq!(base_value(d.get(a).unwrap(), AnimProperty::BlurRadius), Some(s(2.0)));
        assert!(base_value(d.get(b).unwrap(), AnimProperty::ShadowX).is_none(), "no shadow, nothing to animate");
        assert!(base_value(d.get(b).unwrap(), AnimProperty::BlurRadius).is_none());
        let mut h = History::default();
        for (p, t, v) in [
            (AnimProperty::ShadowX, 0, s(0.0)),
            (AnimProperty::ShadowX, 1000, s(20.0)),
            (AnimProperty::ShadowBlur, 0, s(0.0)),
            (AnimProperty::ShadowBlur, 1000, s(30.0)),
            (AnimProperty::ShadowColor, 0, AnimValue::Color(Color::rgb(255, 0, 0))),
            (AnimProperty::ShadowColor, 1000, AnimValue::Color(Color::rgb(0, 0, 255))),
            (AnimProperty::BlurRadius, 0, s(0.0)),
            (AnimProperty::BlurRadius, 1000, s(10.0)),
        ] {
            let c = add_keyframe(&d, a, p, t, Some(v), None).unwrap();
            h.exec(&mut d, c).unwrap();
        }
        let ev = crate::motion::evaluate(&d, d.page_of(a).unwrap(), 500.0);
        let fx = &ev.get(a).unwrap().effects;
        let Effect::DropShadow { offset, radius, color, spread, .. } = &fx[1] else { panic!() };
        assert_eq!((offset.x, offset.y, *radius, *spread), (10.0, 4.0, 15.0, 1.0));
        assert!((color.r - 0.5).abs() < 0.02 && (color.b - 0.5).abs() < 0.02);
        let Effect::DropShadow { offset, radius, .. } = &fx[2] else { panic!() };
        assert_eq!((offset.x, *radius), (9.0, 9.0), "only the first shadow animates");
        let Effect::LayerBlur { radius, .. } = &fx[0] else { panic!() };
        assert_eq!(*radius, 5.0);
        // editing without a track writes the static effect
        let c = auto_key(&d, b, AnimProperty::ShadowX, s(3.0), 0.0);
        assert!(c.is_err(), "node b has no shadow");
        let back = Document::from_json(&d.to_json().unwrap()).unwrap();
        assert_eq!(back.get(a).unwrap().motion, d.get(a).unwrap().motion);
    }
}
