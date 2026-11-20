//! Playback: the canvas renders the page evaluated at the playhead.
//!
//! [`Playback`] owns a [`MotionPlayer`] (a scratch copy of the document whose animated nodes are
//! rewritten for each time) that is rebuilt whenever the stored document changes, and the clock
//! that advances the playhead while playing. The stored document is never touched: editing keeps
//! working on it, and the player's document is only used for painting.

pub mod export;
pub mod present;

use super::MotionUi;
use desigma_doc::motion::{MotionPlayer, TimelineSettings};
use desigma_doc::{Document, NodeId};

/// What the renderer was last asked to draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Shown {
    enabled: bool,
    doc_rev: u64,
    page: NodeId,
    t_ms: u32,
}

#[derive(Default)]
pub struct Playback {
    player: Option<MotionPlayer>,
    /// Document revision and page the player was built for.
    built: Option<(u64, NodeId)>,
    /// What was last handed to the renderer; a change bumps `render_rev`.
    shown: Option<Shown>,
    /// Revision handed to the render caches: changes whenever the pixels may change.
    render_rev: u64,
    /// Playback clock: egui time and playhead (ms) when the current run started.
    clock: Option<(f64, f32)>,
    /// Playhead the clock last wrote (a different `playhead_ms` means the user scrubbed).
    written: u32,
    /// Frames evaluated since start (tests and the perf overlay).
    pub frames: u64,
    /// Milliseconds the last evaluation took.
    pub last_eval_ms: f32,
    /// The "Export animation" block: remembered settings and the running job.
    pub export: export::AnimExportUi,
}

impl Playback {
    /// The document to paint: the evaluated page while Motion mode is on, else the stored one.
    pub fn document<'a>(&'a self, enabled: bool, stored: &'a Document) -> &'a Document {
        match (&self.player, enabled) {
            (Some(p), true) => p.document(),
            _ => stored,
        }
    }

    /// Drops the cached player (the document was replaced).
    pub fn reset(&mut self) {
        self.player = None;
        self.built = None;
        self.shown = None;
        self.clock = None;
    }

    /// Brings the evaluated document up to date for this frame and returns the revision to hand to
    /// the render caches. `doc_rev` is the document revision (it changes on every edit).
    pub fn prepare(&mut self, ui: &MotionUi, stored: &Document, page: NodeId, doc_rev: u64) -> u64 {
        let want = Shown { enabled: ui.enabled, doc_rev, page, t_ms: if ui.enabled { ui.playhead_ms } else { 0 } };
        if self.shown == Some(want) {
            return self.render_rev;
        }
        if ui.enabled {
            let start = std::time::Instant::now();
            if self.built != Some((doc_rev, page)) || self.player.is_none() {
                self.player = Some(MotionPlayer::new(stored, page));
                self.built = Some((doc_rev, page));
            }
            if let Some(p) = &mut self.player {
                p.seek(stored, ui.playhead_ms as f32);
            }
            self.frames += 1;
            self.last_eval_ms = start.elapsed().as_secs_f32() * 1000.0;
        } else if self.player.is_some() {
            // Motion mode is off: the scratch copy is dead weight.
            self.player = None;
            self.built = None;
            self.clock = None;
        }
        self.shown = Some(want);
        self.render_rev += 1;
        self.render_rev
    }

    /// Advances the playhead while `ui.playing`. `now` is the egui clock in seconds. Returns true
    /// while playback keeps running (the caller requests another repaint).
    pub fn tick(&mut self, ui: &mut MotionUi, timeline: TimelineSettings, now: f64) -> bool {
        if !ui.enabled || !ui.playing {
            self.clock = None;
            ui.playing = false;
            return false;
        }
        let duration = timeline.duration_ms;
        // The user dragged the playhead: restart the clock from there.
        if self.clock.is_some() && ui.playhead_ms != self.written {
            self.clock = None;
        }
        let (t0, from) = *self.clock.get_or_insert_with(|| {
            // Playing from the end of a non-looping timeline starts over.
            let start = if !timeline.looping && ui.playhead_ms >= duration { 0.0 } else { ui.playhead_ms as f32 };
            (now, start)
        });
        let elapsed = from + ((now - t0) * 1000.0) as f32;
        if timeline.finished(elapsed) {
            ui.playhead_ms = duration;
            self.written = duration;
            ui.playing = false;
            self.clock = None;
            return false;
        }
        ui.playhead_ms = timeline.playback_time(elapsed).round() as u32;
        self.written = ui.playhead_ms;
        true
    }
}

impl crate::app::App {
    /// Per-frame hook: advances the playhead while the timeline plays and keeps repainting.
    pub fn motion_tick(&mut self, ctx: &egui::Context) {
        self.poll_anim_export(ctx);
        if !self.motion.enabled {
            // timeline selections do not outlive Motion mode
            self.motion.selected_keys.clear();
            self.motion.selected_clip.take();
            std::mem::take(&mut self.motion.easing_target);
        }
        if !self.motion.enabled && !self.motion.playing {
            return;
        }
        let timeline = self.doc.timeline(self.page);
        // The playhead never leaves the timeline, even when its length was shortened.
        self.motion.playhead_ms = self.motion.playhead_ms.min(timeline.duration_ms);
        if self.canvas.motion.tick(&mut self.motion, timeline, self.now) {
            ctx.request_repaint();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use desigma_doc::motion::{AnimProperty, AnimValue, Ease};
    use desigma_doc::{Node, Rect};

    fn doc() -> (Document, NodeId, NodeId) {
        let mut d = Document::new("t");
        let page = d.add_page("P");
        let id = d.alloc_id();
        let mut n = Node::rectangle(id, Rect::new(0.0, 0.0, 10.0, 10.0));
        let op = n.motion.ensure_track(AnimProperty::Opacity);
        op.set_key(0, AnimValue::Scalar(0.0), Some(Ease::Linear));
        op.set_key(1000, AnimValue::Scalar(1.0), None);
        d.insert(page, n).unwrap();
        (d, page, id)
    }

    fn ui_playing(at: u32) -> MotionUi {
        MotionUi { enabled: true, playing: true, playhead_ms: at, ..Default::default() }
    }

    #[test]
    fn stops_at_the_end_when_not_looping() {
        let tl = TimelineSettings { duration_ms: 1000, fps: 60, looping: false };
        let mut pb = Playback::default();
        let mut ui = ui_playing(0);
        assert!(pb.tick(&mut ui, tl, 10.0));
        assert_eq!(ui.playhead_ms, 0);
        assert!(pb.tick(&mut ui, tl, 10.4));
        assert_eq!(ui.playhead_ms, 400);
        assert!(!pb.tick(&mut ui, tl, 11.5));
        assert_eq!((ui.playhead_ms, ui.playing), (1000, false));
        // Pressing play at the end starts over from 0.
        ui.playing = true;
        assert!(pb.tick(&mut ui, tl, 20.0));
        assert_eq!(ui.playhead_ms, 0);
    }

    #[test]
    fn loops_forever_when_looping() {
        let tl = TimelineSettings { duration_ms: 1000, fps: 60, looping: true };
        let mut pb = Playback::default();
        let mut ui = ui_playing(250);
        assert!(pb.tick(&mut ui, tl, 5.0));
        assert!(pb.tick(&mut ui, tl, 7.1));
        assert_eq!(ui.playhead_ms, 350);
        assert!(ui.playing);
    }

    #[test]
    fn scrubbing_while_playing_restarts_the_clock() {
        let tl = TimelineSettings { duration_ms: 1000, fps: 60, looping: false };
        let mut pb = Playback::default();
        let mut ui = ui_playing(0);
        pb.tick(&mut ui, tl, 0.0);
        pb.tick(&mut ui, tl, 0.2);
        assert_eq!(ui.playhead_ms, 200);
        ui.playhead_ms = 600; // user drags
        pb.tick(&mut ui, tl, 0.3);
        assert_eq!(ui.playhead_ms, 600);
        pb.tick(&mut ui, tl, 0.4);
        assert_eq!(ui.playhead_ms, 700);
    }

    #[test]
    fn not_playing_without_motion_mode() {
        let mut pb = Playback::default();
        let mut ui = MotionUi { enabled: false, playing: true, ..Default::default() };
        assert!(!pb.tick(&mut ui, TimelineSettings::default(), 1.0));
        assert!(!ui.playing);
    }

    #[test]
    fn evaluated_document_follows_the_playhead_and_the_edits() {
        let (mut d, page, id) = doc();
        let mut pb = Playback::default();
        let mut ui = MotionUi { enabled: true, playhead_ms: 500, ..Default::default() };
        let r1 = pb.prepare(&ui, &d, page, 1);
        assert!((pb.document(true, &d).get(id).unwrap().opacity - 0.5).abs() < 1e-3);
        assert_eq!(d.get(id).unwrap().opacity, 1.0, "the stored document is untouched");
        // Same inputs: same revision (no re-render).
        assert_eq!(pb.prepare(&ui, &d, page, 1), r1);
        // New time: new revision.
        ui.playhead_ms = 1000;
        let r2 = pb.prepare(&ui, &d, page, 1);
        assert_ne!(r2, r1);
        assert!((pb.document(true, &d).get(id).unwrap().opacity - 1.0).abs() < 1e-3);
        // A document edit rebuilds the player.
        d.get_mut(id).unwrap().name = "renamed".into();
        let r3 = pb.prepare(&ui, &d, page, 2);
        assert_ne!(r3, r2);
        assert_eq!(pb.document(true, &d).get(id).unwrap().name, "renamed");
        // Leaving Motion mode shows the stored document again.
        ui.enabled = false;
        let r4 = pb.prepare(&ui, &d, page, 2);
        assert_ne!(r4, r3);
        assert!(std::ptr::eq(pb.document(false, &d), &d));
    }
}
