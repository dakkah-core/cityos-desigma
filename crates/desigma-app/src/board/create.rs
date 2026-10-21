//! Creating board content: new board files, stickies, shapes, stamps, brush strokes, the
//! Tab-to-next-sticky shortcut, and the text frame the editor lays text out in.

use super::{color_of, rgb_of};
use crate::app::App;
use crate::canvas::tools::Tool;
use crate::edit::Tx;
use crate::theme;
use crate::util::*;
use desigma_doc::*;

/// An empty board file: one page on the (transparent) canvas colour, so the dots show through.
pub fn new_document(name: &str) -> Document {
    let mut d = Document::new(name);
    d.kind = DocumentKind::Board;
    let page = d.add_page("Board");
    if let Some(p) = d.get_mut(page) {
        p.kind = NodeKind::Page { background: Color { a: 0.0, ..color_of(theme::BOARD_BG) } };
    }
    d
}

/// The name shown on the author line of sticky notes: the signed-in user of the machine.
pub fn author_name() -> String {
    std::env::var("USERNAME").or_else(|_| std::env::var("USER")).ok().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "Me".to_string())
}

/// A fresh node for a board drawing tool (position and size are filled in by the caller);
/// `None` for tools that are not board item tools.
pub fn item(app: &mut App, tool: Tool) -> Option<Node> {
    let id = app.doc.alloc_id();
    let r = Rect::new(0.0, 0.0, 0.0, 0.0);
    Some(match tool {
        Tool::Sticky => {
            let mut n = Node::sticky(id, r, "");
            if let NodeKind::Sticky(s) = &mut n.kind {
                s.color = app.board.color;
                s.author = Some(author_name());
                s.wide = app.board.wide;
            }
            n.name = app.unique_name("Sticky note");
            n
        }
        Tool::Code => {
            let mut n = Node::code_block(id, r, app.board.lang, "");
            n.name = app.unique_name("Code block");
            n
        }
        Tool::BoardShape => {
            let mut n = Node::shape_with_text(id, r, app.board.shape, "");
            n.name = app.unique_name("Shape");
            n
        }
        Tool::Stamp => {
            let mut n = Node::stamp(id, Vec2::ZERO, &app.board.stamp, theme::BOARD_STAMP_SIZE);
            n.size = Vec2::ZERO;
            n.name = stamp_label(&app.board.stamp).to_string();
            n
        }
        _ => return None,
    })
}

/// Default box of a board item placed by a click at `at` (centred on the point).
pub fn click_rect(tool: Tool, at: Vec2, wide: bool) -> Rect {
    let (w, h) = match tool {
        Tool::Sticky if wide => (STICKY_WIDE_W, STICKY_WIDE_H),
        Tool::Sticky => (STICKY_SIZE, STICKY_SIZE),
        Tool::Code => (CODE_BLOCK_W, CODE_BLOCK_H),
        Tool::Stamp => (theme::BOARD_STAMP_SIZE, theme::BOARD_STAMP_SIZE),
        Tool::Table => (TABLE_DEFAULT_COLS as f32 * TABLE_COL_W, TABLE_DEFAULT_ROWS as f32 * TABLE_ROW_H),
        Tool::Section => (SECTION_DEFAULT_SIZE.x, SECTION_DEFAULT_SIZE.y),
        _ => (theme::BOARD_SHAPE_W, theme::BOARD_SHAPE_H),
    };
    Rect::new((at.x - w / 2.0).round(), (at.y - h / 2.0).round(), w, h)
}

/// Whether dragging out this tool keeps the box square.
pub fn keeps_square(tool: Tool, wide: bool) -> bool {
    matches!(tool, Tool::Stamp) || (tool == Tool::Sticky && !wide)
}

/// Whether a plain click with this tool places the item at its default size (rather than a small
/// default box at the click, which is what the design tools do).
pub fn click_places(tool: Tool, board: bool) -> bool {
    types_into(tool) || matches!(tool, Tool::Stamp | Tool::Table) || (board && tool == Tool::Section)
}

/// Whether a click with this tool starts typing into the new item.
pub fn types_into(tool: Tool) -> bool {
    matches!(tool, Tool::Sticky | Tool::BoardShape | Tool::Code)
}

/// Base width of a brush's stroke (thickness 1.0).
pub fn brush_base_width(tool: Tool) -> f32 {
    match tool {
        Tool::Highlighter => theme::BOARD_HIGHLIGHTER_WIDTH,
        Tool::Washi => theme::BOARD_WASHI_WIDTH,
        _ => theme::BOARD_MARKER_WIDTH,
    }
}

/// Stroke width of `tool` at thickness choice `size` (an index into [`BRUSH_SIZES`]).
pub fn brush_width(tool: Tool, size: usize) -> f32 {
    brush_base_width(tool) * BRUSH_SIZES[size.min(BRUSH_SIZES.len() - 1)]
}

/// Opacity of the ink of `tool`.
pub fn brush_alpha(tool: Tool) -> f32 {
    match tool {
        Tool::Highlighter => theme::BOARD_HIGHLIGHTER_ALPHA,
        Tool::Washi => theme::BOARD_WASHI_ALPHA,
        _ => 1.0,
    }
}

/// Styles a freshly drawn marker, highlighter or washi tape stroke. `size` picks the thickness
/// from [`BRUSH_SIZES`]; `pattern` is only used by washi tape.
pub fn brush_style(n: &mut Node, tool: Tool, ink: Color, size: usize, pattern: WashiPattern) {
    n.stroke.paints = vec![Paint::solid(ink.with_alpha(brush_alpha(tool)))];
    n.stroke.weight = brush_width(tool, size);
    n.stroke.cap = if tool == Tool::Washi { StrokeCap::None } else { StrokeCap::Round };
    n.stroke.join = StrokeJoin::Round;
    n.stroke.align = StrokeAlign::Center;
    n.fills.clear();
    let (brush, name) = match tool {
        Tool::Highlighter => (Brush::Highlighter, "Highlighter"),
        Tool::Washi => (Brush::Washi(pattern), "Washi tape"),
        _ => (Brush::Marker, "Marker"),
    };
    n.brush = Some(brush);
    // the highlighter darkens what it covers, like ink over print
    n.blend = if tool == Tool::Highlighter { BlendMode::Multiply } else { BlendMode::PassThrough };
    n.name = name.into();
}

/// Creates the sticky note to the right of the selected (or edited) one, same colour, and starts
/// typing into it. Returns false when no single sticky is selected.
pub fn tab_next(app: &mut App) -> bool {
    let cur = app.canvas.text.as_ref().map(|s| s.id).or_else(|| (app.selection.len() == 1).then(|| app.selection[0]));
    let Some(cur) = cur else { return false };
    let Some(n) = app.doc.get(cur) else { return false };
    let NodeKind::Sticky(src) = &n.kind else { return false };
    let (color, tpos, size, parent, wide) = (src.color, n.transform, n.size, n.parent, src.wide);
    let Some(parent) = parent else { return false };
    // commit the text being typed, then add the neighbour in its own undo step
    crate::canvas::interact::finish_text(app, true);
    let id = app.doc.alloc_id();
    let mut next = Node::sticky(id, Rect::new(0.0, 0.0, size.x, size.y), "");
    next.transform = Affine::translate(tpos.tx + size.x + theme::BOARD_STICKY_GAP, tpos.ty);
    next.name = app.unique_name("Sticky note");
    if let NodeKind::Sticky(s) = &mut next.kind {
        s.color = color;
        s.author = Some(author_name());
        s.wide = wide;
    }
    let index = app.doc.get(parent).map(|p| p.children.len()).unwrap_or(0);
    let mut tx = Tx::new();
    if tx.cmd(&mut app.doc, Command::Insert { parent, index, nodes: vec![next] }).is_err() {
        return false;
    }
    app.run_tx(tx);
    app.selection = vec![id];
    crate::canvas::interact::begin_text_edit(app, id);
    app.changed();
    true
}

/// Where the text of a node is laid out: its local origin and box, and the props to lay out with.
pub struct TextFrame {
    pub origin: Vec2,
    pub size: Vec2,
    pub props: TextProps,
}

/// The frame of the editable text of `n`: a text layer's own box, or the text area of a sticky,
/// shape or connector label (with the font size the renderer fits).
pub fn text_frame(doc: &Document, n: &Node) -> Option<TextFrame> {
    match &n.kind {
        NodeKind::Text(t) => Some(TextFrame { origin: Vec2::ZERO, size: n.size, props: t.clone() }),
        k if k.board_text().is_some() || matches!(k, NodeKind::Connector(_)) => {
            let f = desigma_render::board::text_frame(doc, n)?;
            Some(TextFrame { origin: v2(f.rect.x, f.rect.y), size: v2(f.rect.w, f.rect.h), props: f.props })
        }
        _ => None,
    }
}

/// Starts editing the label of connector `id`, adding an empty one first when it has none.
pub fn edit_label(app: &mut App, id: NodeId) {
    crate::canvas::interact::finish_text(app, true);
    let has = matches!(app.doc.get(id).map(|n| &n.kind), Some(NodeKind::Connector(c)) if c.label.is_some());
    if !has {
        app.edit_nodes(&[id], |n| {
            if let NodeKind::Connector(c) = &mut n.kind {
                c.label = Some(board_text(14.0, TextAlignH::Center, TextAlignV::Center));
            }
        });
    }
    crate::canvas::interact::begin_text_edit(app, id);
}

/// `0xRRGGBB` of a paint when it is a solid colour.
pub fn solid_rgb(p: &Paint) -> Option<u32> {
    match &p.kind {
        PaintKind::Solid(c) => Some(rgb_of(*c)),
        _ => None,
    }
}
