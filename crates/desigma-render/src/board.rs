//! Board items (stickies, connectors, shapes with text, stamps) are drawn by
//! *lowering* them to ordinary nodes: a body (rectangle, ellipse or vector with the
//! item's fills, stroke and effects) plus synthetic children (text, end caps, label,
//! stamp glyph layers). Both renderers and the exporters run the same lowering, so
//! they all draw the same pixels.
//!
//! The synthetic children live in a per-thread registry under ids that cannot clash
//! with document ids ([`is_synthetic`]); look nodes up through [`get`].

use crate::text::text_geometry;
use desigma_doc as d;
use desigma_doc::{
    Affine, ArcData, Color, ConnectorCap, Document, Effect, Node, NodeId, NodeKind, Paint, Path, PathCmd, Rect, SectionProps, ShapeKind, StampLayer,
    StickyProps, StrokeAlign, StrokeCap, StrokeJoin, TableProps, TextAlignH, TextAlignV, TextAutoResize, TextProps, Vec2,
};
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

const SYNTH_BIT: u64 = 1 << 62;

thread_local! {
    static SYNTH: RefCell<HashMap<NodeId, Rc<Node>>> = RefCell::new(HashMap::new());
}

/// Id of the `slot`-th (0..16) synthetic child of the board item `base`.
pub fn synth_id(base: NodeId, slot: u64) -> NodeId {
    NodeId(SYNTH_BIT | (base.0 << 4) | (slot & 15))
}

/// Whether `id` names a lowered child rather than a document node.
pub fn is_synthetic(id: NodeId) -> bool {
    id.0 & SYNTH_BIT != 0
}

/// A document node, or a synthetic child registered by the last [`lower`] on this thread.
pub fn get(doc: &Document, id: NodeId) -> Option<Cow<'_, Node>> {
    if is_synthetic(id) {
        SYNTH.with(|s| s.borrow().get(&id).map(|n| Cow::Owned((**n).clone())))
    } else {
        doc.get(id).map(Cow::Borrowed)
    }
}

/// Whether `n` is drawn as its lowered form: a board item, a text layer with a highlight
/// background (its own fill) on a board, or any item that has votes.
pub fn lowers(doc: &Document, n: &Node) -> bool {
    n.kind.is_board_item()
        || (doc.kind == d::DocumentKind::Board
            && matches!(n.kind, NodeKind::Text(_))
            && (n.fills.iter().any(|p| p.visible) || doc.board_session.vote.as_ref().is_some_and(|v| v.count(n.id) > 0)))
}

/// Largest extent (local units) the shadow of a sticky note reaches past its box.
pub fn sticky_shadow_reach(size: Vec2) -> f32 {
    let m = size.x.min(size.y);
    m * 0.025 + m * 0.06 * 1.6 + 4.0
}

fn solid(c: Color) -> Vec<Paint> {
    vec![Paint::solid(c)]
}

fn child(base: &Node, slot: u64, kind: NodeKind, r: Rect) -> Node {
    let mut n = Node::new(synth_id(base.id, slot), kind);
    n.parent = Some(base.id);
    n.transform = Affine::translate(r.x, r.y);
    n.size = Vec2::new(r.w, r.h);
    n.name = String::new();
    n
}

fn scaled(paths: &[Path], w: f32, h: f32) -> Vec<Path> {
    let s = |p: Vec2| Vec2::new(p.x * w, p.y * h);
    paths
        .iter()
        .map(|p| Path {
            cmds: p
                .cmds
                .iter()
                .map(|c| match *c {
                    PathCmd::MoveTo(a) => PathCmd::MoveTo(s(a)),
                    PathCmd::LineTo(a) => PathCmd::LineTo(s(a)),
                    PathCmd::QuadTo(a, b) => PathCmd::QuadTo(s(a), s(b)),
                    PathCmd::CubicTo(a, b, c) => PathCmd::CubicTo(s(a), s(b), s(c)),
                    PathCmd::Close => PathCmd::Close,
                })
                .collect(),
            fill_rule: p.fill_rule,
        })
        .collect()
}

fn vector(paths: Vec<Path>) -> NodeKind {
    NodeKind::Vector { paths, network: None }
}

/// Where the text of a board item is laid out (node-local) and the props it is laid out with:
/// for stickies and shapes the font size already shrunk to fit; for connector labels the
/// text box inside the label background. The editor uses this to place its caret exactly
/// where the renderer draws the glyphs.
#[derive(Clone, Debug, PartialEq)]
pub struct TextFrame {
    pub rect: Rect,
    pub props: TextProps,
}

/// `text` fitted into `bx`: the base size shrinks until the wrapped text fits the height.
fn fit_into(text: &TextProps, bx: Rect, max: f32, min: f32) -> TextFrame {
    let mut props = text.clone();
    props.auto_resize = TextAutoResize::None;
    if !text.content.is_empty() && bx.w >= 1.0 && bx.h >= 1.0 {
        let size = Vec2::new(bx.w, bx.h);
        let fs = d::fit_font_size(max, min, |s| {
            props.style.font_size = s;
            text_geometry(&props, size).height <= bx.h + 0.5
        });
        props.style.font_size = fs;
    }
    TextFrame { rect: bx, props }
}

fn fitted_text(base: &Node, slot: u64, f: TextFrame) -> Option<Node> {
    if f.props.content.is_empty() || f.rect.w < 1.0 || f.rect.h < 1.0 {
        return None;
    }
    Some(child(base, slot, NodeKind::Text(f.props), f.rect))
}

fn shape_text_box(shape: ShapeKind, size: Vec2) -> Rect {
    let (w, h) = (size.x, size.y);
    let (fx, fy, fw, fh) = shape.text_box();
    let inset = (w.min(h) * 0.04).min(10.0);
    Rect::new(w * fx + inset, h * fy + inset, (w * fw - 2.0 * inset).max(1.0), (h * fh - 2.0 * inset).max(1.0))
}

/// Background box and text box of a connector label centered on `mid`, and the props to lay it out with.
fn label_frame(mid: Vec2, label: &TextProps) -> (Rect, TextFrame) {
    let mut props = label.clone();
    props.auto_resize = TextAutoResize::WidthAndHeight;
    let fs = label.style.font_size.max(1.0);
    let (tw, th) = if label.content.is_empty() {
        (fs * 3.0, fs * 1.3)
    } else {
        let g = text_geometry(&props, Vec2::ZERO);
        (g.width.max(1.0), g.height.max(1.0))
    };
    let (px, py) = (fs * 0.6, fs * 0.3);
    let bg = Rect::new((mid.x - tw / 2.0 - px).round(), (mid.y - th / 2.0 - py).round(), tw + 2.0 * px, th + 2.0 * py);
    (bg, TextFrame { rect: Rect::new(bg.x + px, bg.y + py, tw, th), props })
}

/// Route of connector `n` relative to the node, the way it is drawn.
fn local_route(doc: &Document, n: &Node) -> Option<d::Route> {
    let NodeKind::Connector(c) = &n.kind else { return None };
    let route = d::connector_route(doc, n.id).or_else(|| {
        let (a, b) = d::resolve_anchors((None, c.start.magnet, c.start.point), (None, c.end.magnet, c.end.point));
        Some(d::route(c.line, &a, &b))
    })?;
    Some(route.translated(Vec2::new(-n.transform.tx, -n.transform.ty)))
}

/// The text frame of a sticky, shape or connector label; `None` for every other node.
pub fn text_frame(doc: &Document, n: &Node) -> Option<TextFrame> {
    match &n.kind {
        NodeKind::Sticky(s) => {
            let lay = d::sticky_layout(n.size, s.show_author && s.author.is_some());
            Some(fit_into(&s.text, lay.text_box, s.text.style.font_size, d::STICKY_FONT_MIN))
        }
        NodeKind::ShapeWithText(s) => Some(fit_into(&s.text, shape_text_box(s.shape, n.size), s.text.style.font_size, d::STICKY_FONT_MIN)),
        NodeKind::Connector(c) => {
            let label = c.label.as_ref()?;
            let route = local_route(doc, n)?;
            Some(label_frame(route.midpoint(), label).1)
        }
        NodeKind::TableCell(c) => Some(fit_into(&c.text, cell_text_box(n.size), c.text.style.font_size, d::STICKY_FONT_MIN)),
        NodeKind::CodeBlock(c) => Some(TextFrame { rect: c.layout(n.size).text_box, props: c.styled_text() }),
        _ => None,
    }
}

/// The text box inside a table cell of `size`.
fn cell_text_box(size: Vec2) -> Rect {
    let p = d::TABLE_CELL_PAD;
    Rect::new(p, p, (size.x - 2.0 * p).max(1.0), (size.y - 2.0 * p).max(1.0))
}

/// The title pill of a section, in the section's local space (above its top left corner).
/// The pill hugs the title, within the section's width.
pub fn section_title_rect(n: &Node) -> Rect {
    let props = section_title_props(&n.name);
    let tw = if n.name.is_empty() { 0.0 } else { text_geometry(&props, Vec2::ZERO).width.max(1.0) };
    let w = (tw + 2.0 * d::SECTION_PILL_PAD).max(48.0).min(n.size.x.max(48.0));
    Rect::new(0.0, -(d::SECTION_PILL_H + d::SECTION_PILL_GAP), w, d::SECTION_PILL_H)
}

fn section_title_props(title: &str) -> TextProps {
    let mut t = d::board_text(d::SECTION_TITLE_SIZE, TextAlignH::Left, TextAlignV::Center);
    t.content = title.to_string();
    t.style.font_weight = 600;
    t.auto_resize = TextAutoResize::WidthAndHeight;
    t
}

/// Lowers `n` (a board item) to a body node with synthetic children, or `None` for other kinds.
pub fn lower(doc: &Document, n: &Node) -> Option<Node> {
    let mut extras: Vec<Node> = Vec::new();
    let mut body = n.clone();
    body.children.clear();
    body.is_mask = false;
    match &n.kind {
        NodeKind::Sticky(s) => lower_sticky(n, s, &mut body, &mut extras),
        NodeKind::ShapeWithText(s) => {
            let (w, h) = (n.size.x, n.size.y);
            body.kind = match s.shape {
                ShapeKind::Square => NodeKind::Rectangle,
                ShapeKind::RoundedRect => {
                    body.corner_radii = [w.min(h) * 0.2; 4];
                    NodeKind::Rectangle
                }
                ShapeKind::Ellipse => NodeKind::Ellipse(ArcData::default()),
                k => vector(k.path(w, h).into_iter().collect()),
            };
            if s.shape == ShapeKind::Square {
                body.corner_radii = [0.0; 4];
            }
            if body.stroke.join == StrokeJoin::Miter && !matches!(body.kind, NodeKind::Rectangle) {
                body.stroke.join = StrokeJoin::Round;
            }
            if !matches!(body.kind, NodeKind::Rectangle) {
                body.stroke.align = StrokeAlign::Center;
            }
            let bx = shape_text_box(s.shape, n.size);
            extras.extend(fitted_text(n, 0, fit_into(&s.text, bx, s.text.style.font_size, d::STICKY_FONT_MIN)));
        }
        NodeKind::Stamp { kind } => {
            let layers = d::stamp_layers(kind);
            let (w, h) = (n.size.x, n.size.y);
            body.stroke.paints.clear();
            body.stroke.weight = 0.0;
            let mut it = layers.into_iter();
            if let Some(first) = it.next() {
                body.kind = vector(scaled(&first.paths, w, h));
                body.fills = first.fill.map(solid).unwrap_or_default();
            }
            for (i, l) in it.enumerate() {
                extras.push(stamp_layer(n, 6 + i as u64, &l, w, h));
            }
        }
        NodeKind::CodeBlock(c) => lower_code(n, c, &mut body, &mut extras),
        NodeKind::Connector(_) => lower_connector(doc, n, &mut body, &mut extras)?,
        NodeKind::BoardSection(s) => lower_section(n, s, &mut body, &mut extras),
        NodeKind::Table(t) => lower_table(doc, n, t, &mut body, &mut extras),
        NodeKind::Text(t) if lowers(doc, n) => {
            // a highlight background: the text layer's own fill behind its text
            body.kind = NodeKind::Rectangle;
            body.corner_radii = [(t.style.font_size * 0.2).min(n.size.y / 2.0).max(0.0); 4];
            body.stroke.weight = 0.0;
            body.stroke.paints.clear();
            extras.push(child(n, 0, NodeKind::Text(t.clone()), Rect::new(0.0, 0.0, n.size.x, n.size.y)));
        }
        _ => return None,
    }
    if !matches!(n.kind, NodeKind::Connector(_)) {
        if let Some(count) = doc.board_session.vote.as_ref().map(|v| v.count(n.id)).filter(|c| *c > 0) {
            vote_badge(n, count, &mut extras);
        }
    }
    // a section keeps its real (visible) children under the title pill
    body.children = if matches!(n.kind, NodeKind::BoardSection(_)) { d::section_visible_children(n).to_vec() } else { Vec::new() };
    body.children.extend(extras.iter().map(|e| e.id));
    SYNTH.with(|s| {
        let mut s = s.borrow_mut();
        // ids of deleted items are never reused: forget their children when the table gets big
        if s.len() > 50_000 {
            s.clear();
        }
        for e in extras {
            s.insert(e.id, Rc::new(e));
        }
    });
    Some(body)
}

/// A copy of `doc` in which every board item is replaced by its lowered form (the body node
/// plus its synthetic children as real nodes), for exporters that walk the node tree
/// themselves. `None` when the document has no board items.
pub fn lowered_document(doc: &Document) -> Option<Document> {
    let items: Vec<NodeId> = doc.nodes.values().filter(|n| lowers(doc, n)).map(|n| n.id).collect();
    if items.is_empty() {
        return None;
    }
    let mut out = doc.clone();
    for id in items {
        let Some(n) = doc.get(id) else { continue };
        let Some(body) = lower(doc, n) else { continue };
        for c in body.children.iter().filter(|c| is_synthetic(**c)) {
            if let Some(k) = get(doc, *c) {
                out.nodes.insert(*c, k.into_owned());
            }
        }
        // keep the document's own tree links
        let mut body = body;
        body.parent = n.parent;
        out.nodes.insert(id, body);
    }
    Some(out)
}

fn lower_sticky(n: &Node, s: &StickyProps, body: &mut Node, extras: &mut Vec<Node>) {
    let lay = d::sticky_layout(n.size, s.show_author && s.author.is_some());
    let m = n.size.x.min(n.size.y);
    body.kind = NodeKind::Rectangle;
    body.corner_radii = [lay.corner; 4];
    body.corner_smoothing = 0.0;
    body.fills = solid(s.color);
    // soft shadow: a wide faint one and a tight contact one, then the user's own effects
    let mut effects = vec![
        Effect::DropShadow {
            color: Color { a: 0.18, ..Color::BLACK },
            offset: Vec2::new(0.0, m * 0.025),
            radius: m * 0.06,
            spread: 0.0,
            visible: true,
            show_behind: false,
        },
        Effect::DropShadow {
            color: Color { a: 0.12, ..Color::BLACK },
            offset: Vec2::new(0.0, 1.0),
            radius: 2.0,
            spread: 0.0,
            visible: true,
            show_behind: false,
        },
    ];
    effects.extend(n.effects.iter().cloned());
    body.effects = effects;
    extras.extend(fitted_text(n, 0, fit_into(&s.text, lay.text_box, s.text.style.font_size, d::STICKY_FONT_MIN)));
    if let (Some(author), Some(bx)) = (&s.author, lay.author_box) {
        let mut t = d::board_text(lay.author_size, TextAlignH::Left, TextAlignV::Bottom);
        t.content = author.clone();
        t.style.font_weight = 400;
        t.style.fills = solid(Color { a: 0.55, ..Color::rgb(0x1E, 0x1E, 0x1E) });
        extras.push(child(n, 1, NodeKind::Text(t), bx));
    }
}

fn lower_section(n: &Node, s: &SectionProps, body: &mut Node, extras: &mut Vec<Node>) {
    body.kind = NodeKind::Rectangle;
    body.corner_radii = [d::SECTION_RADIUS; 4];
    body.corner_smoothing = 0.0;
    body.fills = solid(s.color.fill());
    body.stroke.paints.clear();
    body.stroke.weight = 0.0;
    // the title pill sits on the top left corner, outside the box
    let pill = section_title_rect(n);
    let mut bg = child(n, 0, NodeKind::Rectangle, pill);
    bg.fills = solid(s.color.color());
    bg.corner_radii = [pill.h / 2.0; 4];
    bg.stroke.weight = 0.0;
    extras.push(bg);
    if !n.name.is_empty() {
        let mut t = section_title_props(&n.name);
        t.auto_resize = TextAutoResize::Truncate;
        t.max_lines = Some(1);
        let bx = Rect::new(pill.x + d::SECTION_PILL_PAD, pill.y, (pill.w - 2.0 * d::SECTION_PILL_PAD).max(1.0), pill.h);
        extras.push(child(n, 1, NodeKind::Text(t), bx));
    }
}

fn lower_table(doc: &Document, n: &Node, t: &TableProps, body: &mut Node, extras: &mut Vec<Node>) {
    body.kind = NodeKind::Rectangle;
    body.corner_radii = [0.0; 4];
    body.fills = solid(Color::WHITE);
    body.stroke.paints = solid(d::TABLE_LINE);
    body.stroke.weight = 1.0;
    body.stroke.align = StrokeAlign::Inside;
    let size = t.size();
    body.size = size;
    // cell fills first, then the grid, then the text
    let mut texts = Vec::new();
    for (i, cid) in n.children.iter().enumerate() {
        let Some(cell) = doc.get(*cid) else { continue };
        let NodeKind::TableCell(c) = &cell.kind else { continue };
        let (r, col) = t.cell_pos(i);
        let rect = t.cell_rect(r, col);
        if let Some(fill) = c.fill {
            let mut f = child(cell, 1, NodeKind::Rectangle, rect);
            f.parent = Some(n.id);
            f.fills = solid(fill);
            f.stroke.weight = 0.0;
            extras.push(f);
        }
        if let Some(mut tx) = fitted_text(cell, 0, fit_into(&c.text, cell_text_box(Vec2::new(rect.w, rect.h)), c.text.style.font_size, d::STICKY_FONT_MIN)) {
            tx.parent = Some(n.id);
            tx.transform = Affine::translate(rect.x + tx.transform.tx, rect.y + tx.transform.ty);
            texts.push(tx);
        }
    }
    let mut cmds = Vec::new();
    for c in 1..t.cols() {
        let x = t.col_x(c);
        cmds.push(PathCmd::MoveTo(Vec2::new(x, 0.0)));
        cmds.push(PathCmd::LineTo(Vec2::new(x, size.y)));
    }
    for r in 1..t.rows() {
        let y = t.row_y(r);
        cmds.push(PathCmd::MoveTo(Vec2::new(0.0, y)));
        cmds.push(PathCmd::LineTo(Vec2::new(size.x, y)));
    }
    if !cmds.is_empty() {
        let mut g = child(n, 0, vector(vec![Path { cmds, fill_rule: d::FillRule::NonZero }]), Rect::new(0.0, 0.0, size.x, size.y));
        g.fills.clear();
        g.stroke.paints = solid(d::TABLE_LINE);
        g.stroke.weight = 1.0;
        g.stroke.align = StrokeAlign::Center;
        extras.push(g);
    }
    extras.extend(texts);
}

/// Colour of vote dots.
pub const VOTE_COLOR: Color = Color { r: 0.592, g: 0.278, b: 1.0, a: 1.0 };

/// The vote dot in the top right corner of `n` with the number of votes on it.
fn vote_badge(n: &Node, count: u32, extras: &mut Vec<Node>) {
    let r = (n.size.x.min(n.size.y) * 0.09).clamp(10.0, 16.0);
    let c = Vec2::new((n.size.x - r - 4.0).max(r), r + 4.0);
    let bx = Rect::new(c.x - r, c.y - r, 2.0 * r, 2.0 * r);
    let mut dot = child(n, 14, NodeKind::Ellipse(ArcData::default()), bx);
    dot.fills = solid(VOTE_COLOR);
    dot.stroke.paints = solid(Color::WHITE);
    dot.stroke.weight = 1.5;
    dot.stroke.align = StrokeAlign::Center;
    extras.push(dot);
    let mut t = d::board_text(r * 1.15, TextAlignH::Center, TextAlignV::Center);
    t.content = count.to_string();
    t.style.font_weight = 700;
    t.style.fills = solid(Color::WHITE);
    extras.push(child(n, 15, NodeKind::Text(t), bx));
}

/// A code block: a clipping rounded frame in the theme's colours, the line numbers and the code.
fn lower_code(n: &Node, c: &d::CodeBlockProps, body: &mut Node, extras: &mut Vec<Node>) {
    let lay = c.layout(n.size);
    body.kind = NodeKind::Frame(d::FrameProps { clips_content: true, ..Default::default() });
    body.corner_radii = [lay.corner; 4];
    body.fills = solid(c.theme.background());
    if c.theme == d::CodeTheme::Light {
        body.stroke.paints = solid(Color { a: 0.12, ..Color::BLACK });
        body.stroke.weight = 1.0;
        body.stroke.align = StrokeAlign::Inside;
    } else {
        body.stroke.paints.clear();
        body.stroke.weight = 0.0;
    }
    body.effects = vec![Effect::DropShadow {
        color: Color { a: 0.16, ..Color::BLACK },
        offset: Vec2::new(0.0, 2.0),
        radius: 6.0,
        spread: 0.0,
        visible: true,
        show_behind: false,
    }];
    body.effects.extend(n.effects.iter().cloned());
    if !c.text.content.is_empty() {
        extras.push(child(n, 0, NodeKind::Text(c.styled_text()), lay.text_box));
    }
    if let Some(g) = lay.gutter {
        let mut t = c.gutter_text();
        t.auto_resize = TextAutoResize::None;
        extras.push(child(n, 1, NodeKind::Text(t), g));
    }
}

fn stamp_layer(base: &Node, slot: u64, l: &StampLayer, w: f32, h: f32) -> Node {
    let mut c = child(base, slot, vector(scaled(&l.paths, w, h)), Rect::new(0.0, 0.0, w, h));
    c.fills = l.fill.map(solid).unwrap_or_default();
    if let Some((color, weight)) = l.stroke {
        c.stroke.paints = solid(color);
        c.stroke.weight = weight * w.min(h);
        c.stroke.align = StrokeAlign::Center;
        c.stroke.cap = StrokeCap::Round;
        c.stroke.join = StrokeJoin::Round;
    } else {
        c.stroke.weight = 0.0;
    }
    c
}

fn lower_connector(doc: &Document, n: &Node, body: &mut Node, extras: &mut Vec<Node>) -> Option<()> {
    let NodeKind::Connector(c) = &n.kind else { return None };
    // the route from the live endpoints, relative to this node
    let route = local_route(doc, n)?;
    body.kind = vector(vec![route.path()]);
    body.fills.clear();
    body.corner_radii = [0.0; 4];
    body.stroke.align = StrokeAlign::Center;
    let weight = n.stroke.weight.max(0.5);
    let ink = n.stroke.paints.clone();
    let full = Rect::new(0.0, 0.0, n.size.x, n.size.y);
    // the line stops at the base of a filled cap so a wide cap is not struck through
    for (slot, cap, tip, dir) in [(2u64, c.start_cap, route.start(), neg(route.start_tangent())), (3u64, c.end_cap, route.end(), route.end_tangent())] {
        let Some(path) = d::cap_path(cap, tip, dir, weight) else { continue };
        let mut k = child(n, slot, vector(vec![path]), full);
        match cap {
            ConnectorCap::Arrow => {
                k.stroke = n.stroke.clone();
                k.stroke.dashes.clear();
                k.stroke.cap = StrokeCap::Round;
                k.stroke.join = StrokeJoin::Round;
                k.fills.clear();
            }
            _ => {
                k.fills = ink.clone();
                k.stroke.weight = 0.0;
                k.stroke.paints.clear();
            }
        }
        extras.push(k);
    }
    if let Some(label) = c.label.as_ref().filter(|l| !l.content.is_empty()) {
        let (bg, f) = label_frame(route.midpoint(), label);
        let fs = label.style.font_size.max(1.0);
        let mut b = child(n, 4, NodeKind::Rectangle, bg);
        b.fills = solid(Color::WHITE);
        b.corner_radii = [(fs * 0.4).min(bg.h / 2.0); 4];
        b.stroke.weight = 0.0;
        extras.push(b);
        extras.push(child(n, 5, NodeKind::Text(f.props), f.rect));
    }
    Some(())
}

fn neg(v: Vec2) -> Vec2 {
    Vec2::new(-v.x, -v.y)
}

#[cfg(test)]
mod tests {
    use super::*;
    use desigma_doc::{ConnectorEnd, ConnectorProps, Magnet, StickyProps};

    fn doc_with(f: impl FnOnce(&mut Document, NodeId)) -> Document {
        let mut doc = Document::new("t");
        let page = doc.add_page("p");
        f(&mut doc, page);
        doc
    }

    #[test]
    fn sticky_lowers_to_rect_shadow_and_fitted_text() {
        let mut ids = (NodeId(0), NodeId(0));
        let doc = doc_with(|d, page| {
            let a = d.alloc_id();
            let short = d.insert(page, Node::sticky(a, Rect::new(0.0, 0.0, 240.0, 240.0), "Hi")).unwrap();
            let b = d.alloc_id();
            let long = d.insert(page, Node::sticky(b, Rect::new(300.0, 0.0, 240.0, 240.0), "word ".repeat(80))).unwrap();
            ids = (short, long);
        });
        let short = lower(&doc, doc.get(ids.0).unwrap()).unwrap();
        let long = lower(&doc, doc.get(ids.1).unwrap()).unwrap();
        assert!(matches!(short.kind, NodeKind::Rectangle));
        assert!(short.corner_radii[0] > 0.0);
        assert_eq!(short.effects.iter().filter(|e| matches!(e, Effect::DropShadow { .. })).count(), 2);
        let size_of = |b: &Node| match &*get(&doc, b.children[0]).unwrap() {
            Node { kind: NodeKind::Text(t), .. } => t.style.font_size,
            _ => panic!("text child expected"),
        };
        let (s, l) = (size_of(&short), size_of(&long));
        assert_eq!(s, d::STICKY_FONT_MAX, "short text keeps the full size");
        assert!(l < s * 0.7, "long text shrinks: {l} vs {s}");
        // and the shrunk text really fits the box
        let NodeKind::Text(t) = &get(&doc, long.children[0]).unwrap().kind.clone() else { panic!() };
        let tb = get(&doc, long.children[0]).unwrap().size;
        assert!(text_geometry(t, tb).height <= tb.y + 0.5);
    }

    #[test]
    fn empty_sticky_has_no_text_and_author_adds_a_line() {
        let mut id = NodeId(0);
        let doc = doc_with(|d, page| {
            let a = d.alloc_id();
            id = d.insert(page, Node::sticky(a, Rect::new(0.0, 0.0, 200.0, 200.0), "")).unwrap();
            if let NodeKind::Sticky(StickyProps { author, show_author, .. }) = &mut d.get_mut(id).unwrap().kind {
                *author = Some("Kir".into());
                *show_author = true;
            }
        });
        let b = lower(&doc, doc.get(id).unwrap()).unwrap();
        assert_eq!(b.children.len(), 1, "only the author line");
        assert!(matches!(&get(&doc, b.children[0]).unwrap().kind, NodeKind::Text(t) if t.content == "Kir"));
    }

    #[test]
    fn connector_lowers_to_path_caps_and_label() {
        let mut cid = NodeId(0);
        let doc = doc_with(|d, page| {
            let a = d.alloc_id();
            let a = d.insert(page, Node::sticky(a, Rect::new(0.0, 0.0, 100.0, 100.0), "a")).unwrap();
            let b = d.alloc_id();
            let b = d.insert(page, Node::sticky(b, Rect::new(300.0, 0.0, 100.0, 100.0), "b")).unwrap();
            let id = d.alloc_id();
            let mut props =
                ConnectorProps { start: ConnectorEnd::attached(a, Magnet::Auto), end: ConnectorEnd::attached(b, Magnet::Auto), ..Default::default() };
            props.start_cap = ConnectorCap::Circle;
            props.end_cap = ConnectorCap::Triangle;
            let mut label = d::board_text(14.0, TextAlignH::Center, TextAlignV::Center);
            label.content = "yes".into();
            props.label = Some(label);
            cid = d.insert(page, Node::connector(id, props)).unwrap();
            d::sync_connector(d, cid);
        });
        let body = lower(&doc, doc.get(cid).unwrap()).unwrap();
        assert!(matches!(body.kind, NodeKind::Vector { .. }));
        assert!(body.fills.is_empty());
        // two caps, label background and label text
        assert_eq!(body.children.len(), 4);
        assert!(body.children.iter().all(|c| is_synthetic(*c) && get(&doc, *c).is_some()));
    }

    #[test]
    fn stamps_and_shapes_lower() {
        let mut ids = Vec::new();
        let doc = doc_with(|d, page| {
            for k in d::STAMP_KINDS {
                let id = d.alloc_id();
                ids.push(d.insert(page, Node::stamp(id, Vec2::ZERO, k, 64.0)).unwrap());
            }
            for k in ShapeKind::ALL {
                let id = d.alloc_id();
                ids.push(d.insert(page, Node::shape_with_text(id, Rect::new(0.0, 0.0, 120.0, 80.0), k, "Text")).unwrap());
            }
        });
        for id in ids {
            let b = lower(&doc, doc.get(id).unwrap()).unwrap();
            assert!(!matches!(b.kind, NodeKind::Sticky(_) | NodeKind::Stamp { .. } | NodeKind::ShapeWithText(_)));
        }
    }
}
