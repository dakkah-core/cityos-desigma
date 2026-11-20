//! Animation clips: Figma's animation presets are stacks of these.
//!
//! A clip animates one *kind* of change (position, scale, rotation, size, opacity) relative to
//! whatever the node would otherwise look like at that time. Evaluation is
//! `base value (static or keyframed) -> compose clips`:
//!
//! | kind     | neutral | composition with the base / other clips |
//! |----------|---------|------------------------------------------|
//! | Position | (0, 0)  | additive offset in px                    |
//! | Rotation | 0       | additive offset in degrees               |
//! | Scale    | (1, 1)  | multiplies the scale factor              |
//! | Size     | (1, 1)  | multiplies width and height              |
//! | Opacity  | 1       | multiplies opacity                       |
//!
//! Composition is commutative, so clip order never matters. Before its delay a clip holds its
//! start state (so an "Enter" clip keeps the node off-screen/invisible until it starts); after
//! its last cycle it holds the end state (an "Exit" clip leaves the node gone; an "Enter" clip
//! leaves it untouched).

use super::easing::Ease;
use super::value::{lerp_vec2, AnimProperty};
use crate::Vec2;
use serde::{Deserialize, Serialize};

/// What a clip changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ClipKind {
    Position,
    Scale,
    Rotation,
    /// Width and height (does not scale children or strokes like [`ClipKind::Scale`] does not
    /// either; it simply multiplies `size`).
    Size,
    Opacity,
}

impl ClipKind {
    pub const ALL: [ClipKind; 5] = [ClipKind::Position, ClipKind::Scale, ClipKind::Rotation, ClipKind::Size, ClipKind::Opacity];

    pub fn name(&self) -> &'static str {
        match self {
            ClipKind::Position => "Position",
            ClipKind::Scale => "Scale",
            ClipKind::Rotation => "Rotation",
            ClipKind::Size => "Size",
            ClipKind::Opacity => "Opacity",
        }
    }

    /// Animatable properties this kind of clip drives.
    pub fn properties(&self) -> &'static [AnimProperty] {
        match self {
            ClipKind::Position => &[AnimProperty::Position],
            ClipKind::Scale => &[AnimProperty::Scale],
            ClipKind::Rotation => &[AnimProperty::Rotation],
            ClipKind::Size => &[AnimProperty::Width, AnimProperty::Height],
            ClipKind::Opacity => &[AnimProperty::Opacity],
        }
    }

    /// Value that leaves the node unchanged (`.x` only for scalar kinds).
    pub fn neutral(&self) -> Vec2 {
        match self {
            ClipKind::Position | ClipKind::Rotation => Vec2::ZERO,
            ClipKind::Scale | ClipKind::Size => Vec2::new(1.0, 1.0),
            ClipKind::Opacity => Vec2::new(1.0, 0.0),
        }
    }

    /// Directions the inspector offers for this kind.
    pub fn directions(&self) -> &'static [ClipDirection] {
        use ClipDirection::*;
        match self {
            ClipKind::Position => &[Top, Bottom, Left, Right, TopLeft, TopRight, BottomLeft, BottomRight],
            ClipKind::Rotation => &[Clockwise, CounterClockwise],
            ClipKind::Scale | ClipKind::Size => &[Both, Horizontal, Vertical],
            ClipKind::Opacity => &[],
        }
    }

    pub fn default_direction(&self) -> ClipDirection {
        match self {
            ClipKind::Position => ClipDirection::Bottom,
            ClipKind::Rotation => ClipDirection::Clockwise,
            _ => ClipDirection::Both,
        }
    }

    /// What `amount` means: distance in px (Position), start/end factor (Scale, Size), degrees
    /// (Rotation), start/end opacity (Opacity).
    pub fn default_amount(&self) -> f32 {
        match self {
            ClipKind::Position => 100.0,
            ClipKind::Scale | ClipKind::Size => 0.5,
            ClipKind::Rotation => 90.0,
            ClipKind::Opacity => 0.0,
        }
    }
}

/// The "Type" dropdown of a clip.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ClipMode {
    /// Slide in: starts away from the node's normal state and settles into it.
    #[default]
    In,
    /// Slide out: starts at the normal state and moves away.
    Out,
    /// Free `from` / `to` values.
    Custom,
}

impl ClipMode {
    /// Label for a kind, e.g. "Slide in" for Position, "Scale in" for Scale.
    pub fn label(&self, kind: ClipKind) -> String {
        let verb = match kind {
            ClipKind::Position => "Slide",
            ClipKind::Scale => "Scale",
            ClipKind::Rotation => "Rotate",
            ClipKind::Size => "Resize",
            ClipKind::Opacity => "Fade",
        };
        match self {
            ClipMode::In => format!("{verb} in"),
            ClipMode::Out => format!("{verb} out"),
            ClipMode::Custom => "Custom".to_string(),
        }
    }
}

/// Where a clip comes from / goes to, or how it turns. Meaning depends on the kind; see
/// [`ClipKind::directions`] for the valid ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ClipDirection {
    Top,
    Bottom,
    Left,
    Right,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    Clockwise,
    CounterClockwise,
    /// Scale and size: both axes.
    Both,
    /// Scale and size: width only.
    Horizontal,
    /// Scale and size: height only.
    Vertical,
}

impl ClipDirection {
    /// Unit vector in y-down parent space for the position directions.
    pub fn vector(&self) -> Vec2 {
        const D: f32 = std::f32::consts::FRAC_1_SQRT_2;
        match self {
            ClipDirection::Top => Vec2::new(0.0, -1.0),
            ClipDirection::Bottom => Vec2::new(0.0, 1.0),
            ClipDirection::Left => Vec2::new(-1.0, 0.0),
            ClipDirection::Right => Vec2::new(1.0, 0.0),
            ClipDirection::TopLeft => Vec2::new(-D, -D),
            ClipDirection::TopRight => Vec2::new(D, -D),
            ClipDirection::BottomLeft => Vec2::new(-D, D),
            ClipDirection::BottomRight => Vec2::new(D, D),
            _ => Vec2::ZERO,
        }
    }

    /// Axis mask for scale / size: which axes the amount applies to.
    fn axes(&self) -> Vec2 {
        match self {
            ClipDirection::Horizontal => Vec2::new(1.0, 0.0),
            ClipDirection::Vertical => Vec2::new(0.0, 1.0),
            _ => Vec2::new(1.0, 1.0),
        }
    }

    /// Dropdown label: "From bottom" / "To bottom", "Clockwise", "Horizontal".
    pub fn label(&self, mode: ClipMode) -> String {
        let lead = if mode == ClipMode::Out { "To" } else { "From" };
        let side = |s: &str| format!("{lead} {s}");
        match self {
            ClipDirection::Top => side("top"),
            ClipDirection::Bottom => side("bottom"),
            ClipDirection::Left => side("left"),
            ClipDirection::Right => side("right"),
            ClipDirection::TopLeft => side("top left"),
            ClipDirection::TopRight => side("top right"),
            ClipDirection::BottomLeft => side("bottom left"),
            ClipDirection::BottomRight => side("bottom right"),
            ClipDirection::Clockwise => "Clockwise".into(),
            ClipDirection::CounterClockwise => "Counter-clockwise".into(),
            ClipDirection::Both => "Both".into(),
            ClipDirection::Horizontal => "Horizontal".into(),
            ClipDirection::Vertical => "Vertical".into(),
        }
    }
}

/// How often a clip's cycle runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClipRepeat {
    #[default]
    Once,
    /// Run this many cycles (1 = once).
    Times(u32),
    /// Keep cycling until the end of the timeline (and beyond, when the playhead goes there).
    Infinite,
}

fn yes() -> bool {
    true
}
fn is_yes(v: &bool) -> bool {
    *v
}
fn is_no(v: &bool) -> bool {
    !*v
}
fn is_zero(v: &Vec2) -> bool {
    v.x == 0.0 && v.y == 0.0
}
fn is_once(r: &ClipRepeat) -> bool {
    *r == ClipRepeat::Once
}
fn default_duration() -> u32 {
    500
}

/// One clip on a node's timeline (a "Position", "Scale", ... bar).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    /// Unique among the node's clips; stable across edits so the UI can keep a selection.
    pub id: u32,
    pub kind: ClipKind,
    /// Slide in / Slide out / Custom.
    #[serde(default)]
    pub mode: ClipMode,
    #[serde(default = "default_direction")]
    pub direction: ClipDirection,
    /// Distance (px), scale or size factor, degrees, or opacity; see [`ClipKind::default_amount`].
    /// Ignored in Custom mode.
    #[serde(default)]
    pub amount: f32,
    /// Custom mode start value (`.x` only for rotation and opacity).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub from: Vec2,
    /// Custom mode end value.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub to: Vec2,
    /// When the clip starts, in ms on the timeline ("Delay").
    #[serde(default)]
    pub delay_ms: u32,
    /// Length of one cycle in ms.
    #[serde(default = "default_duration")]
    pub duration_ms: u32,
    #[serde(default)]
    pub easing: Ease,
    #[serde(default, skip_serializing_if = "is_once")]
    pub repeat: ClipRepeat,
    /// Odd cycles play backwards, so repeating clips go there and back (Pulse, Float).
    #[serde(default, skip_serializing_if = "is_no")]
    pub yoyo: bool,
    /// The eye toggle: hidden clips contribute nothing.
    #[serde(default = "yes", skip_serializing_if = "is_yes")]
    pub visible: bool,
}

fn default_direction() -> ClipDirection {
    ClipDirection::Bottom
}

impl Clip {
    /// A clip of `kind` with the kind's default direction/amount, 500ms, ease out, at t = 0.
    pub fn new(kind: ClipKind, mode: ClipMode) -> Self {
        Self {
            id: 0,
            kind,
            mode,
            direction: kind.default_direction(),
            amount: kind.default_amount(),
            from: kind.neutral(),
            to: kind.neutral(),
            delay_ms: 0,
            duration_ms: 500,
            easing: Ease::EaseOut,
            repeat: ClipRepeat::Once,
            yoyo: false,
            visible: true,
        }
    }

    // Builder helpers, mostly for the preset table.
    pub fn dir(mut self, d: ClipDirection) -> Self {
        self.direction = d;
        self
    }
    pub fn amount(mut self, a: f32) -> Self {
        self.amount = a;
        self
    }
    pub fn custom(mut self, from: Vec2, to: Vec2) -> Self {
        self.mode = ClipMode::Custom;
        self.from = from;
        self.to = to;
        self
    }
    pub fn delay(mut self, ms: u32) -> Self {
        self.delay_ms = ms;
        self
    }
    pub fn dur(mut self, ms: u32) -> Self {
        self.duration_ms = ms;
        self
    }
    pub fn ease(mut self, e: Ease) -> Self {
        self.easing = e;
        self
    }
    pub fn repeat(mut self, r: ClipRepeat) -> Self {
        self.repeat = r;
        self
    }
    pub fn yoyo(mut self) -> Self {
        self.yoyo = true;
        self
    }

    /// Clamps fields to usable values after loading or UI edits.
    pub fn sanitize(&mut self) {
        self.duration_ms = self.duration_ms.max(1);
        if !self.amount.is_finite() {
            self.amount = self.kind.default_amount();
        }
        for v in [&mut self.from, &mut self.to] {
            if !v.x.is_finite() || !v.y.is_finite() {
                *v = self.kind.neutral();
            }
        }
        self.easing = self.easing.sanitized();
        if let ClipRepeat::Times(0) = self.repeat {
            self.repeat = ClipRepeat::Once;
        }
    }

    /// Start and end value of one cycle in the kind's own value space (see [`ClipKind`] for the
    /// neutral value of each kind).
    ///
    /// * Position: offset in px. "In from bottom, 116": starts `(0, +116)` and ends `(0, 0)`;
    ///   "Out to bottom" starts `(0, 0)` and ends `(0, +116)`.
    /// * Scale / Size: factor. In goes `amount -> 1`, Out `1 -> amount` (per axis mask).
    /// * Rotation: degrees, counter-clockwise positive like [`AnimProperty::Rotation`]. "Clockwise
    ///   90 in": starts at `+90` and turns clockwise to `0`; Out turns `0 -> -90`.
    /// * Opacity: In goes `amount -> 1`, Out `1 -> amount`.
    /// * Custom mode: `from` and `to` as stored.
    pub fn endpoints(&self) -> (Vec2, Vec2) {
        let n = self.kind.neutral();
        if self.mode == ClipMode::Custom {
            return (self.from, self.to);
        }
        if self.kind == ClipKind::Rotation {
            // Clockwise rotation decreases the (counter-clockwise positive) angle. Going out turns
            // away from rest in the named direction; coming in starts on the opposite side so
            // the turn keeps its direction.
            let turn = if self.direction == ClipDirection::CounterClockwise { self.amount } else { -self.amount };
            return if self.mode == ClipMode::In { (Vec2::new(-turn, 0.0), n) } else { (n, Vec2::new(turn, 0.0)) };
        }
        // Position moves to/from the named side; scale, size and opacity shrink or fade towards
        // `amount`.
        let away = match self.kind {
            ClipKind::Position => {
                let v = self.direction.vector();
                Vec2::new(v.x * self.amount, v.y * self.amount)
            }
            ClipKind::Scale | ClipKind::Size => {
                let ax = self.direction.axes();
                Vec2::new(1.0 + (self.amount - 1.0) * ax.x, 1.0 + (self.amount - 1.0) * ax.y)
            }
            _ => Vec2::new(self.amount, 0.0),
        };
        if self.mode == ClipMode::In {
            (away, n)
        } else {
            (n, away)
        }
    }

    /// Total length in ms of all cycles, `None` for an infinite clip.
    pub fn total_ms(&self) -> Option<u32> {
        let d = self.duration_ms.max(1);
        match self.repeat {
            ClipRepeat::Once => Some(d),
            ClipRepeat::Times(n) => Some(d.saturating_mul(n.max(1))),
            ClipRepeat::Infinite => None,
        }
    }

    /// End of the clip on the timeline; infinite clips end with the timeline.
    pub fn end_ms(&self, timeline_duration_ms: u32) -> u32 {
        match self.total_ms() {
            Some(total) => self.delay_ms.saturating_add(total),
            None => timeline_duration_ms.max(self.delay_ms.saturating_add(self.duration_ms.max(1))),
        }
    }

    /// Eased progress through the clip at timeline time `t_ms`: 0 = start value, 1 = end value
    /// (may over/undershoot with back and spring easings). Holds the start before the delay and
    /// the final cycle's end state afterwards.
    pub fn progress_at(&self, t_ms: f32) -> f32 {
        let local = t_ms - self.delay_ms as f32;
        if local.is_nan() || local <= 0.0 {
            return 0.0;
        }
        let n = local / self.duration_ms.max(1) as f32;
        let cycles = match self.repeat {
            ClipRepeat::Once => Some(1u32),
            ClipRepeat::Times(k) => Some(k.max(1)),
            ClipRepeat::Infinite => None,
        };
        if let Some(k) = cycles {
            if n >= k as f32 {
                let reversed = self.yoyo && (k - 1) % 2 == 1;
                return if reversed { 0.0 } else { 1.0 };
            }
        }
        let idx = n.floor();
        let frac = n - idx;
        let reversed = self.yoyo && (idx as u64) % 2 == 1;
        self.easing.ease(if reversed { 1.0 - frac } else { frac })
    }

    /// The clip's value (kind's own space, see [`Clip::endpoints`]) at timeline time `t_ms`.
    pub fn value_at(&self, t_ms: f32) -> Vec2 {
        let (a, b) = self.endpoints();
        lerp_vec2(a, b, self.progress_at(t_ms))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn slide_in_from_bottom_offsets_y() {
        let c = Clip::new(ClipKind::Position, ClipMode::In).dir(ClipDirection::Bottom).amount(116.0).delay(997).dur(300).ease(Ease::Linear);
        assert_eq!(c.endpoints(), (Vec2::new(0.0, 116.0), Vec2::ZERO));
        // Held at the start before the delay, at rest after the clip.
        assert_eq!(c.value_at(0.0), Vec2::new(0.0, 116.0));
        assert_eq!(c.value_at(997.0), Vec2::new(0.0, 116.0));
        assert!(close(c.value_at(997.0 + 150.0).y, 58.0));
        assert_eq!(c.value_at(1297.0), Vec2::ZERO);
        assert_eq!(c.value_at(5000.0), Vec2::ZERO);
    }

    #[test]
    fn slide_out_ends_away() {
        let c = Clip::new(ClipKind::Position, ClipMode::Out).dir(ClipDirection::Left).amount(80.0).dur(200).ease(Ease::Linear);
        assert_eq!(c.value_at(0.0), Vec2::ZERO);
        assert_eq!(c.value_at(1000.0), Vec2::new(-80.0, 0.0));
    }

    #[test]
    fn diagonal_direction_has_the_given_length() {
        let c = Clip::new(ClipKind::Position, ClipMode::In).dir(ClipDirection::TopRight).amount(100.0);
        let (from, _) = c.endpoints();
        assert!(close((from.x * from.x + from.y * from.y).sqrt(), 100.0));
        assert!(from.x > 0.0 && from.y < 0.0);
    }

    #[test]
    fn scale_opacity_rotation_endpoints() {
        let s = Clip::new(ClipKind::Scale, ClipMode::In).amount(0.5);
        assert_eq!(s.endpoints(), (Vec2::new(0.5, 0.5), Vec2::new(1.0, 1.0)));
        let h = Clip::new(ClipKind::Size, ClipMode::Out).amount(2.0).dir(ClipDirection::Horizontal);
        assert_eq!(h.endpoints(), (Vec2::new(1.0, 1.0), Vec2::new(2.0, 1.0)));
        let o = Clip::new(ClipKind::Opacity, ClipMode::In).amount(0.0);
        assert_eq!(o.endpoints(), (Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0)));
        // Clockwise in: angle (ccw positive) falls from +90 to 0.
        let r = Clip::new(ClipKind::Rotation, ClipMode::In).amount(90.0).dir(ClipDirection::Clockwise);
        assert_eq!(r.endpoints(), (Vec2::new(90.0, 0.0), Vec2::ZERO));
        let r = Clip::new(ClipKind::Rotation, ClipMode::Out).amount(90.0).dir(ClipDirection::Clockwise);
        assert_eq!(r.endpoints(), (Vec2::ZERO, Vec2::new(-90.0, 0.0)));
        let r = Clip::new(ClipKind::Rotation, ClipMode::Out).amount(45.0).dir(ClipDirection::CounterClockwise);
        assert_eq!(r.endpoints(), (Vec2::ZERO, Vec2::new(45.0, 0.0)));
    }

    #[test]
    fn custom_uses_from_and_to() {
        let c = Clip::new(ClipKind::Opacity, ClipMode::Custom).custom(Vec2::new(0.2, 0.0), Vec2::new(0.8, 0.0)).ease(Ease::Linear).dur(100);
        assert!(close(c.value_at(50.0).x, 0.5));
        assert_eq!(c.value_at(0.0).x, 0.2);
        assert_eq!(c.value_at(500.0).x, 0.8);
    }

    #[test]
    fn repeat_and_yoyo() {
        let c = Clip::new(ClipKind::Scale, ClipMode::Custom)
            .custom(Vec2::new(1.0, 1.0), Vec2::new(2.0, 2.0))
            .ease(Ease::Linear)
            .dur(100)
            .repeat(ClipRepeat::Infinite)
            .yoyo();
        assert!(close(c.value_at(50.0).x, 1.5));
        assert!(close(c.value_at(150.0).x, 1.5), "second cycle runs backwards, at 1.5 mid way");
        assert!(close(c.value_at(110.0).x, 1.9));
        assert!(close(c.value_at(210.0).x, 1.1));
        assert!(close(c.value_at(100_050.0).x, 1.5));
        let two = c.clone().repeat(ClipRepeat::Times(2));
        // Even number of yoyo cycles ends where it began.
        assert_eq!(two.progress_at(10_000.0), 0.0);
        let three = c.repeat(ClipRepeat::Times(3));
        assert_eq!(three.progress_at(10_000.0), 1.0);
        let plain =
            Clip::new(ClipKind::Rotation, ClipMode::Custom).custom(Vec2::ZERO, Vec2::new(360.0, 0.0)).ease(Ease::Linear).dur(100).repeat(ClipRepeat::Times(2));
        assert!(close(plain.value_at(150.0).x, 180.0));
        assert_eq!(plain.value_at(1000.0).x, 360.0);
    }

    #[test]
    fn span_of_clips() {
        let c = Clip::new(ClipKind::Opacity, ClipMode::In).delay(200).dur(300);
        assert_eq!(c.end_ms(2000), 500);
        assert_eq!(c.clone().repeat(ClipRepeat::Times(3)).end_ms(2000), 1100);
        assert_eq!(c.clone().repeat(ClipRepeat::Infinite).end_ms(2000), 2000);
        assert_eq!(c.repeat(ClipRepeat::Infinite).end_ms(100), 500);
    }

    #[test]
    fn sanitize_fixes_bad_values() {
        let mut c = Clip::new(ClipKind::Position, ClipMode::In);
        c.duration_ms = 0;
        c.amount = f32::NAN;
        c.repeat = ClipRepeat::Times(0);
        c.from = Vec2::new(f32::INFINITY, 0.0);
        c.sanitize();
        assert_eq!(c.duration_ms, 1);
        assert_eq!(c.amount, 100.0);
        assert_eq!(c.repeat, ClipRepeat::Once);
        assert_eq!(c.from, Vec2::ZERO);
    }

    #[test]
    fn labels() {
        assert_eq!(ClipMode::In.label(ClipKind::Position), "Slide in");
        assert_eq!(ClipDirection::Bottom.label(ClipMode::In), "From bottom");
        assert_eq!(ClipDirection::Bottom.label(ClipMode::Out), "To bottom");
    }
}
