//! Prototype interactions (Figma "Prototype" tab): triggers, actions and
//! transitions attached to nodes, plus flow starting points per page.

use crate::{NodeId, Vec2};
use serde::{Deserialize, Serialize};

// The variable expression language (parser, evaluator, limits) lives in its own file.
#[path = "prototype_expr.rs"]
mod expr;
pub use expr::*;

/// Input device of a key/gamepad trigger (Figma's `device`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum GamepadDevice {
    Xbox,
    PlayStation,
    SwitchPro,
    /// Any other controller (standard mapping).
    #[default]
    Generic,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Trigger {
    OnClick,
    OnDrag,
    WhileHovering,
    WhilePressing,
    MouseEnter {
        delay_ms: u32,
    },
    MouseLeave {
        delay_ms: u32,
    },
    MouseDown {
        delay_ms: u32,
    },
    MouseUp {
        delay_ms: u32,
    },
    AfterDelay {
        delay_ms: u32,
    },
    /// Keyboard press, e.g. "Space", "Ctrl+K" (modifiers joined with `+`, any order).
    KeyPress {
        keys: String,
    },
    /// Gamepad button press: `buttons` joined with `+` ("A", "DPadUp", "LB+A").
    GamepadPress {
        device: GamepadDevice,
        buttons: String,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    #[default]
    Left,
    Right,
    Top,
    Bottom,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Easing {
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
    EaseInBack,
    EaseOutBack,
    EaseInOutBack,
    /// CSS-style cubic-bezier control points.
    Bezier([f32; 4]),
    Spring {
        mass: f32,
        stiffness: f32,
        damping: f32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransitionKind {
    Dissolve,
    SmartAnimate,
    MoveIn(Direction),
    MoveOut(Direction),
    Push(Direction),
    SlideIn(Direction),
    SlideOut(Direction),
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transition {
    pub kind: TransitionKind,
    pub easing: Easing,
    pub duration_ms: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum OverlayPosition {
    #[default]
    Center,
    TopLeft,
    TopCenter,
    TopRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
    /// Relative to the triggering layer, offset by `Overlay::offset`.
    Manual,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Action {
    /// Go to another top-level frame. `preserve_scroll` keeps scroll position.
    Navigate {
        dest: NodeId,
        preserve_scroll: bool,
    },
    OpenOverlay {
        dest: NodeId,
        position: OverlayPosition,
        offset: Vec2,
        close_on_click_outside: bool,
        background: Option<crate::Color>,
    },
    SwapOverlay {
        dest: NodeId,
    },
    CloseOverlay,
    Back,
    /// Scroll a scrollable ancestor so `dest` is visible.
    ScrollTo {
        dest: NodeId,
        offset: Vec2,
    },
    OpenUrl {
        url: String,
        new_tab: bool,
    },
    /// Change an instance to another variant of its component set.
    ChangeTo {
        dest: NodeId,
    },
    /// Set a variable (by id) to the value of an expression. Present only:
    /// the document is never changed.
    SetVariable {
        variable: String,
        value: Expression,
    },
    /// Switch a variable collection (by id) to one of its modes (by id) for the
    /// rest of the Present session.
    SetVariableMode {
        collection: String,
        mode: String,
    },
    /// Run the actions of the first branch whose condition holds, else the
    /// `otherwise` actions ("If / Else if / Else").
    Conditional {
        branches: Vec<(Expression, Vec<Action>)>,
        otherwise: Vec<Action>,
    },
    /// Control video playback on `dest` (a layer with a [`VideoFill`]).
    MediaControl {
        dest: NodeId,
        op: MediaOp,
    },
    /// Control the Motion timeline of a frame (Present only). `dest` is the frame to animate,
    /// or any layer in it; `None` means the frame of the layer that triggers the action. A frame
    /// that is the target of any such action waits (shows the timeline's start) until it is
    /// played; other animated frames play when they appear.
    Animation {
        #[serde(default)]
        dest: Option<NodeId>,
        #[serde(default)]
        op: AnimationOp,
    },
}

/// What an [`Action::Animation`] does to a frame's timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnimationOp {
    /// Play; `from_start` rewinds first, otherwise it resumes (a finished one-shot starts over).
    Play {
        #[serde(default)]
        from_start: bool,
    },
    /// Hold the timeline where it is.
    Pause,
    /// Rewind to the start and play.
    Restart,
    /// Pause when playing, resume otherwise.
    TogglePlay,
}

impl Default for AnimationOp {
    fn default() -> Self {
        Self::RESUME
    }
}

impl AnimationOp {
    /// `Play { from_start: false }`, the default.
    pub const RESUME: AnimationOp = AnimationOp::Play { from_start: false };
}

/// Playback control of a video layer (Figma's media actions plus loop).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum MediaOp {
    Play,
    Pause,
    TogglePlay,
    Mute,
    Unmute,
    ToggleMute,
    /// Skip forward (positive) or backward (negative) by seconds.
    SkipBy(f32),
    SeekTo(f32),
    SetLoop(bool),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reaction {
    pub trigger: Trigger,
    pub actions: Vec<Action>,
    /// `None` means instant.
    pub transition: Option<Transition>,
}

/// A prototype flow: a named starting frame on a page.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Flow {
    pub name: String,
    pub page: NodeId,
    pub start: NodeId,
    pub description: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Overflow {
    #[default]
    None,
    Horizontal,
    Vertical,
    Both,
}

/// A video fill, stored as a sidecar of an ordinary image paint.
///
/// Decision: no new `PaintKind` variant. Adding one would touch every renderer
/// (CPU, GPU, SVG/CSS export, .fig export), and a video only plays in Present.
/// Instead the node keeps a normal `PaintKind::Image` paint at `paint` whose
/// image is the poster frame (so every renderer and exporter shows it), and
/// this record says that paint is really a video. The video bytes live in
/// `Document::images` under `source` (the images map is persisted, copied by
/// clipboard/library and embedded in files like any other blob).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VideoFill {
    /// Index into the node's `fills` of the poster image paint.
    pub paint: usize,
    /// Key of the video file bytes in `Document::images`.
    pub source: String,
    pub autoplay: bool,
    #[serde(rename = "loop")]
    pub looping: bool,
    pub muted: bool,
}

/// Container of video bytes by magic number, without decoding anything.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoContainer {
    Mp4,
    WebM,
}

impl VideoContainer {
    /// Sniffs MP4 (`ftyp` box) and WebM/Matroska (EBML header).
    pub fn sniff(bytes: &[u8]) -> Option<Self> {
        if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
            Some(VideoContainer::Mp4)
        } else if bytes.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
            Some(VideoContainer::WebM)
        } else {
            None
        }
    }
}

/// Prototype settings for a frame.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PrototypeProps {
    pub reactions: Vec<Reaction>,
    /// Scroll behaviour when content overflows (frames only).
    pub overflow: Overflow,
    /// Children that stay fixed while the frame scrolls.
    pub fixed_children: Vec<NodeId>,
    /// Video fills of this layer (prototype-only playback).
    #[serde(default)]
    pub videos: Vec<VideoFill>,
}

impl PrototypeProps {
    pub fn is_empty(&self) -> bool {
        self.reactions.is_empty() && self.overflow == Overflow::None && self.fixed_children.is_empty() && self.videos.is_empty()
    }
}

#[cfg(test)]
mod animation_tests {
    use super::*;

    #[test]
    fn animation_actions_round_trip_and_default() {
        let r = Reaction {
            trigger: Trigger::WhileHovering,
            actions: vec![
                Action::Animation { dest: None, op: AnimationOp::Pause },
                Action::Animation { dest: Some(NodeId(7)), op: AnimationOp::Play { from_start: true } },
                Action::Animation { dest: None, op: AnimationOp::Restart },
                Action::Animation { dest: None, op: AnimationOp::TogglePlay },
            ],
            transition: None,
        };
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(serde_json::from_str::<Reaction>(&json).unwrap(), r);
        // fields can be left out: resume the frame's own animation
        let a: Action = serde_json::from_str(r#"{"Animation":{}}"#).unwrap();
        assert_eq!(a, Action::Animation { dest: None, op: AnimationOp::RESUME });
        let a: Action = serde_json::from_str(r#"{"Animation":{"op":{"Play":{}}}}"#).unwrap();
        assert_eq!(a, Action::Animation { dest: None, op: AnimationOp::RESUME });
    }
}
