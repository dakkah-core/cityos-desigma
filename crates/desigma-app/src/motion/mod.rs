//! Motion (Beta) mode: UI state shared by the inspector, timeline and playback.

pub mod inspector;
pub mod playback;
pub mod timeline;

/// State of Motion mode shared between its panels. Created identically by the inspector, the
/// timeline and the playback parts so they merge cleanly.
#[derive(Default)]
pub struct MotionUi {
    pub enabled: bool,
    pub playhead_ms: u32,
    pub playing: bool,
    pub selected_keys: Vec<desigma_doc::motion::edit::KeyRef>,
    pub selected_clip: Option<(desigma_doc::NodeId, u32)>,
    /// A keyframe segment is selected: the inspector shows the Easing panel.
    pub easing_target: bool,
}
