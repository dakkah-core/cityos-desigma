//! Motion: keyframe animation (Figma "Motion (Beta)" mode).
//!
//! The animation model, evaluation and edit helpers, with no UI:
//!
//! * [`TimelineSettings`]: duration, fps and loop, stored **per page** in
//!   [`Document::timelines`](crate::Document::timelines) (Figma keeps one timeline per page, and a
//!   page is the unit that is played and exported).
//! * [`NodeMotion`] (on [`Node::motion`](crate::Node::motion)): keyframe [`Track`]s, one per
//!   [`AnimProperty`], plus [`Clip`]s (the animation presets).
//! * [`Ease`]: hold, linear, bezier and spring easing for the segment that starts at a keyframe.
//! * [`PRESETS`]: the 41 library presets as clip lists.
//! * [`evaluate_node`] / [`evaluate_frame`] / [`evaluate`] / [`MotionPlayer`]: resolve everything
//!   at a time `t` without mutating the stored document.
//! * [`edit`]: functions that build undoable [`Command`](crate::Command)s (add / move / stretch /
//!   delete keyframes, easing, clips, presets, auto-key, timeline settings).
//!
//! Times are milliseconds. Keyframes sit on whole milliseconds; the playhead may be fractional.

mod clip;
mod easing;
pub mod edit;
mod eval;
mod presets;
mod track;
mod value;

pub use clip::*;
pub use easing::*;
pub use eval::*;
pub use presets::*;
pub use track::*;
pub use value::*;

use serde::{Deserialize, Serialize};

fn default_duration() -> u32 {
    2000
}
fn default_fps() -> u32 {
    60
}

/// Timeline settings of one page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimelineSettings {
    /// Length of the timeline in ms (the second number in the timeline header). Default 2000.
    #[serde(default = "default_duration")]
    pub duration_ms: u32,
    /// Frames per second used for stepping, snapping and export. Default 60.
    #[serde(default = "default_fps")]
    pub fps: u32,
    /// Whether playback loops (the Loop button).
    #[serde(default, rename = "loop")]
    pub looping: bool,
}

impl Default for TimelineSettings {
    fn default() -> Self {
        Self { duration_ms: default_duration(), fps: default_fps(), looping: false }
    }
}

impl TimelineSettings {
    pub const MAX_DURATION_MS: u32 = 10 * 60 * 1000;
    pub const MAX_FPS: u32 = 240;

    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Clamps to a usable range: duration 1ms ..= 10min, fps 1 ..= 240.
    pub fn sanitized(mut self) -> Self {
        self.duration_ms = self.duration_ms.clamp(1, Self::MAX_DURATION_MS);
        self.fps = self.fps.clamp(1, Self::MAX_FPS);
        self
    }

    /// Length of one frame in ms.
    pub fn frame_ms(&self) -> f32 {
        1000.0 / self.fps.max(1) as f32
    }

    /// Number of frames of a render of the whole timeline (at least 1).
    pub fn frame_count(&self) -> u32 {
        ((self.duration_ms as f64 * self.fps.max(1) as f64 / 1000.0).ceil() as u32).max(1)
    }

    /// Timeline time of frame `index`.
    pub fn frame_time_ms(&self, index: u32) -> f32 {
        index as f32 * self.frame_ms()
    }

    /// Nearest frame boundary to `t_ms`.
    pub fn snap_to_frame(&self, t_ms: f32) -> f32 {
        let f = self.frame_ms();
        (t_ms / f).round() * f
    }

    /// Maps a free-running playback clock to timeline time: wraps around the duration when
    /// looping, otherwise stops at the end.
    pub fn playback_time(&self, elapsed_ms: f32) -> f32 {
        let d = self.duration_ms.max(1) as f32;
        if self.looping {
            elapsed_ms.rem_euclid(d)
        } else {
            elapsed_ms.clamp(0.0, d)
        }
    }

    /// Whether non-looping playback has reached the end at `elapsed_ms`.
    pub fn finished(&self, elapsed_ms: f32) -> bool {
        !self.looping && elapsed_ms >= self.duration_ms as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_figma() {
        let s = TimelineSettings::default();
        assert_eq!((s.duration_ms, s.fps, s.looping), (2000, 60, false));
        assert!(s.is_default());
        assert_eq!(s.frame_count(), 120);
        assert!((s.frame_ms() - 16.6667).abs() < 1e-3);
    }

    #[test]
    fn serde_defaults_and_loop_name() {
        let s: TimelineSettings = serde_json::from_str("{}").unwrap();
        assert!(s.is_default());
        let s: TimelineSettings = serde_json::from_str(r#"{"loop":true,"duration_ms":500}"#).unwrap();
        assert!(s.looping && s.duration_ms == 500 && s.fps == 60);
        assert!(serde_json::to_string(&s).unwrap().contains("\"loop\":true"));
    }

    #[test]
    fn sanitize_and_playback_time() {
        let s = TimelineSettings { duration_ms: 0, fps: 0, looping: false }.sanitized();
        assert_eq!((s.duration_ms, s.fps), (1, 1));
        let s = TimelineSettings { duration_ms: 1000, fps: 30, looping: true };
        assert!((s.playback_time(2500.0) - 500.0).abs() < 1e-3);
        assert!(!s.finished(5000.0));
        let s = TimelineSettings { looping: false, ..s };
        assert_eq!(s.playback_time(2500.0), 1000.0);
        assert!(s.finished(1000.0));
        assert_eq!(s.snap_to_frame(40.0), 33.333332);
        assert_eq!(s.frame_count(), 30);
    }

    use crate::{Color, Document, History, Node, NodeId, Rect, Vec2};

    fn animated_doc() -> (Document, NodeId, NodeId) {
        let mut d = Document::new("anim");
        let page = d.add_page("P");
        let id = d.alloc_id();
        let mut n = Node::rectangle(id, Rect::new(10.0, 20.0, 100.0, 50.0));
        let pos = n.motion.ensure_track(AnimProperty::Position);
        pos.set_key(0, AnimValue::Vec2(Vec2::new(10.0, 20.0)), Some(Ease::bezier(0.07, 0.97, 0.58, 1.0)));
        pos.set_key(1500, AnimValue::Vec2(Vec2::new(300.0, 20.0)), None);
        let fill = n.motion.ensure_track(AnimProperty::FillColor);
        fill.set_key(0, AnimValue::Color(Color::rgb(255, 0, 0)), Some(Ease::spring(0.4)));
        fill.set_key(1000, AnimValue::Color(Color::rgb(0, 0, 255)), Some(Ease::Hold));
        n.motion.clips = find_preset("Pulse").unwrap().instantiate(250, 0);
        d.insert(page, n).unwrap();
        d.timelines.insert(page, TimelineSettings { duration_ms: 4000, fps: 30, looping: true });
        (d, page, id)
    }

    #[test]
    fn document_roundtrip_keeps_all_animation_data() {
        let (d, page, id) = animated_doc();
        let json = d.to_json().unwrap();
        let back = Document::from_json(&json).unwrap();
        assert_eq!(back.get(id).unwrap().motion, d.get(id).unwrap().motion);
        assert_eq!(back.timeline(page), d.timeline(page));
        assert_eq!(back.to_json().unwrap(), json, "stable on a second pass");
    }

    #[test]
    fn plain_documents_serialize_without_animation_fields() {
        let mut d = Document::new("plain");
        let page = d.add_page("P");
        let id = d.alloc_id();
        d.insert(page, Node::rectangle(id, Rect::new(0.0, 0.0, 5.0, 5.0))).unwrap();
        let json = d.to_json().unwrap();
        assert!(!json.contains("motion") && !json.contains("timelines") && !json.contains("loop"), "old files stay byte-identical in shape");
        // Files written before motion existed (no fields) load with defaults.
        let back = Document::from_json(&json).unwrap();
        assert!(back.get(id).unwrap().motion.is_empty());
        assert!(back.timeline(page).is_default());
        // Setting the timeline back to defaults leaves no trace either.
        let mut h = History::default();
        let mut d2 = d.clone();
        let c = edit::set_timeline(&d2, page, TimelineSettings { looping: true, ..Default::default() }).unwrap();
        h.exec(&mut d2, c).unwrap();
        let c = edit::set_timeline(&d2, page, TimelineSettings::default()).unwrap();
        h.exec(&mut d2, c).unwrap();
        assert_eq!(d2.to_json().unwrap(), json);
    }

    #[test]
    fn loading_repairs_hand_edited_motion() {
        let (d, page, id) = animated_doc();
        let mut v: serde_json::Value = serde_json::from_str(&d.to_json().unwrap()).unwrap();
        let node = &mut v["nodes"][id.0.to_string()];
        node["motion"]["tracks"][0]["keys"].as_array_mut().unwrap().reverse();
        v["timelines"][page.0.to_string()]["fps"] = serde_json::json!(100_000);
        let back = Document::from_json(&v.to_string()).unwrap();
        let t = back.get(id).unwrap().motion.track(AnimProperty::Position).unwrap();
        assert_eq!(t.keys.iter().map(|k| k.time_ms).collect::<Vec<_>>(), vec![0, 1500]);
        assert_eq!(back.timeline(page).fps, TimelineSettings::MAX_FPS);
    }

    #[test]
    fn evaluating_a_loaded_document_matches_the_original() {
        let (d, page, _) = animated_doc();
        let back = Document::from_json(&d.to_json().unwrap()).unwrap();
        for t in [0.0, 400.0, 1200.0, 3000.0] {
            let a = evaluate_frame(&d, page, t);
            let b = evaluate_frame(&back, page, t);
            assert_eq!(a, b);
        }
    }
}
