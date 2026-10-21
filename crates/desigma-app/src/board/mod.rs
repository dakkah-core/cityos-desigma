//! Board (whiteboard) mode: a file whose [`DocumentKind`] is `Board` gets its own bottom toolbar,
//! a floating contextual toolbar instead of the right panel, a dotted canvas, stickies, shapes with
//! text, connectors that follow their nodes, and stamps. The items themselves are document kinds
//! (`desigma_doc::board`); everything here is editor behaviour.

pub mod bar;
pub mod canvas;
pub mod connect;
pub mod create;
pub mod demo;
pub mod erase;
pub mod preview;
pub mod section;
pub mod session;
pub mod table;
pub mod toolbar;

use crate::app::App;
use desigma_doc::{Color, DocumentKind, NodeId, NodeKind, ShapeKind};

/// Which popover of the bottom toolbar is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolMenu {
    Brush,
    Shape,
    Stamp,
    /// The "more" popover: tools that do not fit the bar (Table).
    More,
    Sticky,
}

/// Which popover of the contextual toolbar is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarMenu {
    Shape,
    StartCap,
    EndCap,
    /// The colour grid of a section.
    SectionColor,
    Lang,
}

/// Editor state of board mode.
pub struct BoardState {
    /// Colour of the next sticky note.
    pub color: Color,
    /// The Sticky tool draws wide notes.
    pub wide: bool,
    /// Language of the next code block.
    pub lang: desigma_doc::CodeLang,
    /// Votes per person the next voting session offers.
    pub votes_each: u32,
    /// Facilitation widgets: the vote setup popover and the results list are open.
    pub vote_menu: bool,
    /// The last code copied with the contextual bar's copy button.
    pub copied: Option<String>,
    pub results_open: bool,
    /// Shape the Shape tool draws.
    pub shape: ShapeKind,
    /// Stamp the Stamp tool places.
    pub stamp: String,
    /// Marker and connector ink.
    pub ink: Color,
    /// Marker, highlighter, washi tape or eraser: the brush the toolbar button shows.
    pub brush: crate::canvas::tools::Tool,
    /// Thickness of the next stroke: an index into `desigma_doc::BRUSH_SIZES`.
    pub size: usize,
    /// Pattern of the next washi tape.
    pub pattern: desigma_doc::WashiPattern,
    /// Open toolbar popover and the button it hangs off.
    pub tool_menu: Option<(ToolMenu, egui::Rect)>,
    /// Open contextual toolbar popover and its anchor.
    pub bar_menu: Option<(BarMenu, egui::Rect)>,
    /// Node whose magnet dots are shown.
    pub dots: Option<NodeId>,
    /// Node a connector being dragged would attach to.
    pub target: Option<NodeId>,
    /// Rendered previews of shapes and stamps for the popovers.
    pub previews: preview::Previews,
    /// Section whose title is being renamed in place, and the text typed so far.
    pub rename: Option<(NodeId, String)>,
    /// The table cell last edited or clicked: what the contextual bar's table controls act on.
    pub cell: Option<NodeId>,
    /// The table under the pointer (kept while the pointer reaches for its + buttons).
    pub hover_table: Option<NodeId>,
}

impl Default for BoardState {
    fn default() -> Self {
        Self {
            color: desigma_doc::STICKY_YELLOW,
            wide: false,
            lang: desigma_doc::CodeLang::Plain,
            votes_each: desigma_doc::VOTES_DEFAULT,
            vote_menu: false,
            copied: None,
            results_open: true,
            shape: ShapeKind::RoundedRect,
            stamp: "thumbs_up".into(),
            ink: color_of(crate::theme::BOARD_INK[0]),
            brush: crate::canvas::tools::Tool::Marker,
            size: 1,
            pattern: Default::default(),
            tool_menu: None,
            bar_menu: None,
            dots: None,
            target: None,
            previews: Default::default(),
            rename: None,
            cell: None,
            hover_table: None,
        }
    }
}

/// `0xRRGGBB` as a document colour.
pub fn color_of(c: u32) -> Color {
    Color::rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// A document colour as `0xRRGGBB` (alpha dropped).
pub fn rgb_of(c: Color) -> u32 {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
    (b(c.r) << 16) | (b(c.g) << 8) | b(c.b)
}

pub fn color32(c: Color) -> egui::Color32 {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    egui::Color32::from_rgb(b(c.r), b(c.g), b(c.b))
}

impl App {
    /// The open file is a board.
    pub fn is_board(&self) -> bool {
        self.doc.kind == DocumentKind::Board
    }
}

/// Kinds a connector can attach to.
pub fn connectable(kind: &NodeKind) -> bool {
    matches!(kind, NodeKind::Sticky(_) | NodeKind::ShapeWithText(_) | NodeKind::Stamp { .. } | NodeKind::Text(_) | NodeKind::CodeBlock(_))
}
