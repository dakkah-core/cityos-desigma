//! Keyframe tracks and the per-node motion container.

use super::clip::{Clip, ClipRepeat};
use super::easing::Ease;
use super::value::{AnimProperty, AnimValue};
use serde::{Deserialize, Serialize};

/// One keyframe: a value at a time, plus the easing of the segment that *starts* here (the
/// segment up to the next keyframe). The easing of a track's last keyframe is unused.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Keyframe {
    /// Position on the timeline in milliseconds.
    pub time_ms: u32,
    pub value: AnimValue,
    /// Easing of the segment from this keyframe to the next.
    #[serde(default, skip_serializing_if = "Ease::is_linear")]
    pub easing: Ease,
}

/// All keyframes of one property of one node, sorted by time with unique times.
///
/// Mutate through [`Track::set_key`] / [`Track::remove_key`] to keep the invariant; code that
/// edits `keys` directly must call [`Track::normalize`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub property: AnimProperty,
    pub keys: Vec<Keyframe>,
    /// The eye of the timeline's property row ("Toggle animation visibility"): a hidden track
    /// keeps its keyframes but is ignored by evaluation, playback and export.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub visible: bool,
}

fn default_true() -> bool {
    true
}

fn is_true(v: &bool) -> bool {
    *v
}

impl Track {
    pub fn new(property: AnimProperty) -> Self {
        Self { property, keys: Vec::new(), visible: true }
    }

    /// Sorts keys by time, dropping all but the last of keys sharing a time, and drops keys whose
    /// value has the wrong shape or is not finite.
    pub fn normalize(&mut self) {
        let prop = self.property;
        self.keys.retain(|k| prop.accepts(&k.value) && k.value.is_finite());
        // Stable sort keeps insertion order for equal times, so the last duplicate wins.
        self.keys.sort_by_key(|k| k.time_ms);
        let mut out: Vec<Keyframe> = Vec::with_capacity(self.keys.len());
        for k in self.keys.drain(..) {
            match out.last_mut() {
                Some(l) if l.time_ms == k.time_ms => *l = k,
                _ => out.push(k),
            }
        }
        self.keys = out;
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Index of the keyframe exactly at `time_ms`.
    pub fn key_index(&self, time_ms: u32) -> Option<usize> {
        self.keys.binary_search_by_key(&time_ms, |k| k.time_ms).ok()
    }

    /// The keyframe exactly at `time_ms`.
    pub fn key_at(&self, time_ms: u32) -> Option<&Keyframe> {
        self.key_index(time_ms).map(|i| &self.keys[i])
    }

    /// The keyframe closest to `time_ms` within `tolerance_ms` (for hit-testing the playhead).
    pub fn key_near(&self, time_ms: u32, tolerance_ms: u32) -> Option<&Keyframe> {
        self.keys.iter().filter(|k| k.time_ms.abs_diff(time_ms) <= tolerance_ms).min_by_key(|k| k.time_ms.abs_diff(time_ms))
    }

    /// Inserts a keyframe, or replaces the one at the same time. When a new keyframe lands inside
    /// a segment and `easing` is `None`, it inherits the easing of the segment it splits so the
    /// existing curve keeps its shape; otherwise `easing` (or linear) is used. Replacing keeps the
    /// existing easing unless one is given.
    pub fn set_key(&mut self, time_ms: u32, value: AnimValue, easing: Option<Ease>) {
        match self.keys.binary_search_by_key(&time_ms, |k| k.time_ms) {
            Ok(i) => {
                self.keys[i].value = value;
                if let Some(e) = easing {
                    self.keys[i].easing = e;
                }
            }
            Err(i) => {
                let inherited = if i > 0 { self.keys[i - 1].easing } else { Ease::Linear };
                self.keys.insert(i, Keyframe { time_ms, value, easing: easing.unwrap_or(inherited) });
            }
        }
    }

    /// Removes the keyframe at `time_ms`, returning it.
    pub fn remove_key(&mut self, time_ms: u32) -> Option<Keyframe> {
        self.key_index(time_ms).map(|i| self.keys.remove(i))
    }

    pub fn start_ms(&self) -> Option<u32> {
        self.keys.first().map(|k| k.time_ms)
    }

    pub fn end_ms(&self) -> Option<u32> {
        self.keys.last().map(|k| k.time_ms)
    }

    /// Time of the closest keyframe strictly before `time_ms`.
    pub fn prev_key_time(&self, time_ms: u32) -> Option<u32> {
        self.keys.iter().rev().find(|k| k.time_ms < time_ms).map(|k| k.time_ms)
    }

    /// Time of the closest keyframe strictly after `time_ms`.
    pub fn next_key_time(&self, time_ms: u32) -> Option<u32> {
        self.keys.iter().find(|k| k.time_ms > time_ms).map(|k| k.time_ms)
    }

    /// Index of the keyframe starting the segment that contains `t_ms` (the last key at or before
    /// `t_ms`); `None` before the first keyframe.
    pub fn segment_start(&self, t_ms: f32) -> Option<usize> {
        let n = self.keys.partition_point(|k| (k.time_ms as f32) <= t_ms);
        n.checked_sub(1)
    }

    /// Keyframed value at `t_ms`: the first value before the first key, the last value after the
    /// last key, eased interpolation in between. `None` for an empty track.
    pub fn value_at(&self, t_ms: f32) -> Option<AnimValue> {
        let first = self.keys.first()?;
        let Some(i) = self.segment_start(t_ms) else { return Some(first.value) };
        let a = &self.keys[i];
        let Some(b) = self.keys.get(i + 1) else { return Some(a.value) };
        let span = (b.time_ms - a.time_ms) as f32;
        let p = a.easing.ease((t_ms - a.time_ms as f32) / span);
        Some(AnimValue::lerp(&a.value, &b.value, p))
    }
}

/// Animation data of one node: keyframe tracks plus clips. Empty for most nodes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NodeMotion {
    /// One track per property at most, in creation order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tracks: Vec<Track>,
    /// Clips (animation presets), in creation order; composition is order independent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clips: Vec<Clip>,
}

impl NodeMotion {
    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty() && self.clips.is_empty()
    }

    pub fn track(&self, property: AnimProperty) -> Option<&Track> {
        self.tracks.iter().find(|t| t.property == property)
    }

    pub fn track_mut(&mut self, property: AnimProperty) -> Option<&mut Track> {
        self.tracks.iter_mut().find(|t| t.property == property)
    }

    /// The track for `property`, created empty when missing.
    pub fn ensure_track(&mut self, property: AnimProperty) -> &mut Track {
        let i = match self.tracks.iter().position(|t| t.property == property) {
            Some(i) => i,
            None => {
                self.tracks.push(Track::new(property));
                self.tracks.len() - 1
            }
        };
        &mut self.tracks[i]
    }

    pub fn clip(&self, id: u32) -> Option<&Clip> {
        self.clips.iter().find(|c| c.id == id)
    }

    /// A clip id not used by any clip of this node.
    pub fn next_clip_id(&self) -> u32 {
        self.clips.iter().map(|c| c.id + 1).max().unwrap_or(0)
    }

    /// Restores invariants after loading or bulk edits: normalised tracks, no empty tracks, no
    /// duplicate track properties (their keys merge, later keys win), unique clip ids.
    pub fn normalize(&mut self) {
        let mut merged: Vec<Track> = Vec::new();
        for t in self.tracks.drain(..) {
            match merged.iter_mut().find(|m| m.property == t.property) {
                Some(m) => {
                    m.keys.extend(t.keys);
                    m.visible &= t.visible;
                }
                None => merged.push(t),
            }
        }
        for t in &mut merged {
            t.normalize();
        }
        merged.retain(|t| !t.is_empty());
        self.tracks = merged;
        let mut seen = std::collections::BTreeSet::new();
        let mut next = self.clips.iter().map(|c| c.id + 1).max().unwrap_or(0);
        for c in &mut self.clips {
            if !seen.insert(c.id) {
                c.id = next;
                next += 1;
                seen.insert(c.id);
            }
            c.sanitize();
        }
    }

    /// Sorted, deduplicated times of every keyframe across all tracks (snap targets, "next
    /// keyframe" for the whole layer).
    pub fn key_times(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self.tracks.iter().flat_map(|t| t.keys.iter().map(|k| k.time_ms)).collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// Closest keyframe time across all tracks strictly before `time_ms`.
    pub fn prev_key_time(&self, time_ms: u32) -> Option<u32> {
        self.tracks.iter().filter_map(|t| t.prev_key_time(time_ms)).max()
    }

    /// Closest keyframe time across all tracks strictly after `time_ms`.
    pub fn next_key_time(&self, time_ms: u32) -> Option<u32> {
        self.tracks.iter().filter_map(|t| t.next_key_time(time_ms)).min()
    }

    /// Time range covered by this node's animation, for the blue summary bar of its layer row:
    /// first keyframe or clip start to last keyframe or clip end. Clips that repeat forever end at
    /// `timeline_duration_ms`. `None` when nothing is animated.
    pub fn span_ms(&self, timeline_duration_ms: u32) -> Option<(u32, u32)> {
        let mut lo = u32::MAX;
        let mut hi = 0u32;
        let mut any = false;
        for t in &self.tracks {
            if let (Some(a), Some(b)) = (t.start_ms(), t.end_ms()) {
                lo = lo.min(a);
                hi = hi.max(b);
                any = true;
            }
        }
        for c in self.clips.iter().filter(|c| c.visible) {
            lo = lo.min(c.delay_ms);
            hi = hi.max(c.end_ms(timeline_duration_ms));
            any = true;
        }
        any.then_some((lo, hi))
    }

    /// The first track-less hint for UI: does this node animate `property` at all, via a keyframe
    /// track or a visible clip that touches it?
    pub fn animates(&self, property: AnimProperty) -> bool {
        self.track(property).is_some_and(|t| t.visible) || self.clips.iter().any(|c| c.visible && c.kind.properties().contains(&property))
    }

    /// Whether any clip repeats forever (the node animates until the timeline end).
    pub fn has_infinite_clip(&self) -> bool {
        self.clips.iter().any(|c| c.visible && c.repeat == ClipRepeat::Infinite)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: f32) -> AnimValue {
        AnimValue::Scalar(v)
    }

    fn track(keys: &[(u32, f32)]) -> Track {
        let mut t = Track::new(AnimProperty::Opacity);
        for (ms, v) in keys {
            t.set_key(*ms, s(*v), None);
        }
        t
    }

    #[test]
    fn keys_stay_sorted_and_unique() {
        let t = track(&[(500, 0.5), (0, 0.0), (1000, 1.0), (500, 0.7)]);
        let times: Vec<u32> = t.keys.iter().map(|k| k.time_ms).collect();
        assert_eq!(times, vec![0, 500, 1000]);
        assert_eq!(t.key_at(500).unwrap().value, s(0.7));
    }

    #[test]
    fn value_at_holds_outside_and_interpolates_inside() {
        let t = track(&[(100, 0.0), (300, 1.0)]);
        assert_eq!(t.value_at(0.0), Some(s(0.0)));
        assert_eq!(t.value_at(100.0), Some(s(0.0)));
        assert_eq!(t.value_at(200.0), Some(s(0.5)));
        assert_eq!(t.value_at(300.0), Some(s(1.0)));
        assert_eq!(t.value_at(900.0), Some(s(1.0)));
        assert_eq!(Track::new(AnimProperty::Opacity).value_at(5.0), None);
    }

    #[test]
    fn single_key_is_constant() {
        let t = track(&[(400, 0.3)]);
        assert_eq!(t.value_at(0.0), Some(s(0.3)));
        assert_eq!(t.value_at(4000.0), Some(s(0.3)));
    }

    #[test]
    fn segment_easing_belongs_to_the_starting_key() {
        let mut t = track(&[(0, 0.0), (1000, 1.0), (2000, 0.0)]);
        t.keys[0].easing = Ease::Hold;
        // First segment holds, second is linear.
        assert_eq!(t.value_at(500.0), Some(s(0.0)));
        assert_eq!(t.value_at(1500.0), Some(s(0.5)));
        t.keys[0].easing = Ease::EaseIn;
        let AnimValue::Scalar(v) = t.value_at(500.0).unwrap() else { panic!() };
        assert!(v < 0.5);
    }

    #[test]
    fn new_key_inherits_easing_of_split_segment() {
        let mut t = track(&[(0, 0.0), (1000, 1.0)]);
        t.keys[0].easing = Ease::EaseOut;
        t.set_key(400, s(0.9), None);
        assert_eq!(t.key_at(400).unwrap().easing, Ease::EaseOut);
        t.set_key(400, s(0.8), Some(Ease::Hold));
        assert_eq!(t.key_at(400).unwrap().easing, Ease::Hold);
        t.set_key(400, s(0.7), None);
        assert_eq!(t.key_at(400).unwrap().easing, Ease::Hold, "replace keeps easing");
    }

    #[test]
    fn navigation_helpers() {
        let t = track(&[(100, 0.0), (300, 1.0), (700, 0.5)]);
        assert_eq!(t.prev_key_time(300), Some(100));
        assert_eq!(t.prev_key_time(100), None);
        assert_eq!(t.next_key_time(300), Some(700));
        assert_eq!(t.next_key_time(700), None);
        assert_eq!(t.key_near(310, 20).map(|k| k.time_ms), Some(300));
        assert!(t.key_near(500, 20).is_none());
        assert_eq!(t.segment_start(50.0), None);
        assert_eq!(t.segment_start(300.0), Some(1));
    }

    #[test]
    fn node_motion_span_and_navigation() {
        let mut m = NodeMotion::default();
        assert_eq!(m.span_ms(2000), None);
        *m.ensure_track(AnimProperty::Opacity) = track(&[(100, 0.0), (900, 1.0)]);
        let mut p = Track::new(AnimProperty::Rotation);
        p.set_key(300, s(0.0), None);
        p.set_key(1200, s(90.0), None);
        m.tracks.push(p);
        assert_eq!(m.span_ms(2000), Some((100, 1200)));
        assert_eq!(m.key_times(), vec![100, 300, 900, 1200]);
        assert_eq!(m.next_key_time(300), Some(900));
        assert_eq!(m.prev_key_time(300), Some(100));
        assert!(m.animates(AnimProperty::Rotation) && !m.animates(AnimProperty::Width));
    }

    #[test]
    fn normalize_repairs_hand_edited_data() {
        let mut m = NodeMotion::default();
        m.tracks.push(Track {
            property: AnimProperty::Opacity,
            keys: vec![
                Keyframe { time_ms: 500, value: s(1.0), easing: Ease::Linear },
                Keyframe { time_ms: 0, value: s(0.0), easing: Ease::Linear },
                Keyframe { time_ms: 500, value: s(0.5), easing: Ease::Linear },
                Keyframe { time_ms: 700, value: AnimValue::Vec2(crate::Vec2::ZERO), easing: Ease::Linear },
                Keyframe { time_ms: 800, value: s(f32::NAN), easing: Ease::Linear },
            ],
            visible: true,
        });
        m.tracks.push(Track::new(AnimProperty::Width));
        m.tracks.push(Track { property: AnimProperty::Opacity, keys: vec![Keyframe { time_ms: 900, value: s(0.1), easing: Ease::Linear }], visible: true });
        m.normalize();
        assert_eq!(m.tracks.len(), 1);
        let times: Vec<u32> = m.tracks[0].keys.iter().map(|k| k.time_ms).collect();
        assert_eq!(times, vec![0, 500, 900]);
        assert_eq!(m.tracks[0].key_at(500).unwrap().value, s(0.5));
    }
}
