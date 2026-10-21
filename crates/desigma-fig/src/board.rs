//! FigJam (board) node types in REST JSON: `STICKY`, `SHAPE_WITH_TEXT`, `CONNECTOR`,
//! `STAMP`, `SECTION`, `TABLE` and `TABLE_CELL` and the `editorType: "figjam"` file marker,
//! mapped to and from the board kinds of the document model. In a design file `SECTION` stays
//! the plain design section.

use crate::export::{n, paints, v2};
use crate::rest_import::{farr, fbool, fnum, fstr, parse_paints, vec2};
use desigma_doc::*;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

/// Whether a file (or the node tree under it) comes from FigJam.
pub(crate) fn is_figjam(root: &Value) -> bool {
    fstr(root, "editorType").is_some_and(|t| t.eq_ignore_ascii_case("figjam"))
}

fn ink() -> Vec<Paint> {
    vec![Paint::solid(Color::rgb(0x1E, 0x1E, 0x1E))]
}

/// The text of a board node: `characters` (or `text.characters`) and, when present, the type style.
fn board_text_props(v: &Value, base: TextProps) -> TextProps {
    let mut t = base;
    let chars = fstr(v, "characters").or_else(|| v.get("text").and_then(|x| fstr(x, "characters")));
    if let Some(c) = chars {
        t.content = c.to_string();
    }
    if let Some(st) = v.get("style").or_else(|| v.get("text").and_then(|x| x.get("style"))) {
        if let Some(fs) = fnum(st, "fontSize").filter(|f| *f > 0.0) {
            t.style.font_size = fs;
        }
        if let Some(f) = fstr(st, "fontFamily") {
            t.style.font_family = f.to_string();
        }
        if let Some(w) = fnum(st, "fontWeight") {
            t.style.font_weight = w as u16;
        }
        match fstr(st, "textAlignHorizontal") {
            Some("CENTER") => t.style.align_h = TextAlignH::Center,
            Some("RIGHT") => t.style.align_h = TextAlignH::Right,
            Some("LEFT") => t.style.align_h = TextAlignH::Left,
            _ => {}
        }
        match fstr(st, "textAlignVertical") {
            Some("CENTER") => t.style.align_v = TextAlignV::Center,
            Some("BOTTOM") => t.style.align_v = TextAlignV::Bottom,
            Some("TOP") => t.style.align_v = TextAlignV::Top,
            _ => {}
        }
    }
    t
}

fn shape_from_name(s: &str) -> ShapeKind {
    match s {
        "ROUNDED_RECTANGLE" => ShapeKind::RoundedRect,
        "ELLIPSE" => ShapeKind::Ellipse,
        "DIAMOND" => ShapeKind::Diamond,
        "TRIANGLE_UP" => ShapeKind::Triangle,
        "TRIANGLE_DOWN" => ShapeKind::TriangleDown,
        "PARALLELOGRAM_RIGHT" | "PARALLELOGRAM_LEFT" => ShapeKind::Parallelogram,
        "STAR" => ShapeKind::Star,
        "HEXAGON" => ShapeKind::Hexagon,
        "PENTAGON" => ShapeKind::Pentagon,
        _ => ShapeKind::Square,
    }
}

fn shape_name(k: ShapeKind) -> &'static str {
    match k {
        ShapeKind::Square => "SQUARE",
        ShapeKind::RoundedRect => "ROUNDED_RECTANGLE",
        ShapeKind::Ellipse => "ELLIPSE",
        ShapeKind::Diamond => "DIAMOND",
        ShapeKind::Triangle => "TRIANGLE_UP",
        ShapeKind::TriangleDown => "TRIANGLE_DOWN",
        ShapeKind::Parallelogram => "PARALLELOGRAM_RIGHT",
        ShapeKind::Star => "STAR",
        ShapeKind::Hexagon => "HEXAGON",
        ShapeKind::Pentagon => "PENTAGON",
    }
}

fn cap_from_name(s: Option<&str>) -> ConnectorCap {
    match s {
        Some("LINE_ARROW") | Some("ARROW_LINES") => ConnectorCap::Arrow,
        Some("TRIANGLE_ARROW") | Some("TRIANGLE_FILLED") | Some("ARROW_EQUILATERAL") => ConnectorCap::Triangle,
        Some("CIRCLE_FILLED") => ConnectorCap::Circle,
        Some("DIAMOND_FILLED") => ConnectorCap::Diamond,
        _ => ConnectorCap::None,
    }
}

fn cap_name(c: ConnectorCap) -> &'static str {
    match c {
        ConnectorCap::None => "NONE",
        ConnectorCap::Arrow => "LINE_ARROW",
        ConnectorCap::Triangle => "TRIANGLE_FILLED",
        ConnectorCap::Circle => "CIRCLE_FILLED",
        ConnectorCap::Diamond => "DIAMOND_FILLED",
    }
}

fn magnet_from_name(s: Option<&str>) -> Magnet {
    match s {
        Some("TOP") => Magnet::Top,
        Some("RIGHT") => Magnet::Right,
        Some("BOTTOM") => Magnet::Bottom,
        Some("LEFT") => Magnet::Left,
        Some("CENTER") => Magnet::Center,
        _ => Magnet::Auto,
    }
}

fn magnet_name(m: Magnet) -> &'static str {
    match m {
        Magnet::Auto => "AUTO",
        Magnet::Top => "TOP",
        Magnet::Right => "RIGHT",
        Magnet::Bottom => "BOTTOM",
        Magnet::Left => "LEFT",
        Magnet::Center => "CENTER",
    }
}

/// Stamp kind from the REST node: an explicit `stampType` wins, otherwise the layer name.
fn stamp_kind(v: &Value) -> String {
    let hint = fstr(v, "stampType").or_else(|| fstr(v, "stampSubType")).or_else(|| fstr(v, "name")).unwrap_or("").to_ascii_lowercase();
    let known = [
        ("heart", "heart"),
        ("star", "star"),
        ("question", "question"),
        ("thumb", "thumbs_up"),
        ("like", "thumbs_up"),
        ("plus", "plus_one"),
        ("+1", "plus_one"),
    ];
    known.iter().find(|(k, _)| hint.contains(k)).map(|(_, k)| (*k).to_string()).unwrap_or_else(|| "thumbs_up".to_string())
}

/// Board node kind for a REST `type`, or `None` for every other type. `origin` is the
/// page-space position of the parent, to turn absolute connector points into parent space.
pub(crate) fn kind_from_json(ty: &str, v: &Value, origin: Vec2) -> Option<NodeKind> {
    Some(match ty {
        "STICKY" => {
            let mut p = StickyProps::default();
            if let Some(c) = parse_paints(v.get("fills")).iter().find_map(|f| match &f.kind {
                PaintKind::Solid(c) if f.visible => Some(*c),
                _ => None,
            }) {
                p.color = c.with_alpha(1.0);
            }
            p.text = board_text_props(v, p.text);
            p.show_author = fbool(v, "authorVisible").unwrap_or(false);
            p.author = fstr(v, "authorName").map(String::from).filter(|s| !s.is_empty());
            NodeKind::Sticky(p)
        }
        "SHAPE_WITH_TEXT" => {
            let mut p = ShapeWithTextProps { shape: fstr(v, "shapeType").map(shape_from_name).unwrap_or_default(), ..Default::default() };
            p.text = board_text_props(v, p.text);
            NodeKind::ShapeWithText(p)
        }
        "STAMP" => NodeKind::Stamp { kind: stamp_kind(v) },
        "SECTION" => {
            let color = parse_paints(v.get("fills")).iter().find_map(|f| match &f.kind {
                PaintKind::Solid(c) if f.visible => Some(SectionColor::nearest(*c)),
                _ => None,
            });
            NodeKind::BoardSection(SectionProps { color: color.unwrap_or_default(), contents_hidden: fbool(v, "sectionContentsHidden").unwrap_or(false) })
        }
        "TABLE" => NodeKind::Table(table_props(v)),
        "TABLE_CELL" => {
            let fill = parse_paints(v.get("fills")).iter().find_map(|f| match &f.kind {
                PaintKind::Solid(c) if f.visible => Some(*c),
                _ => None,
            });
            let mut p = TableCellProps { fill: fill.filter(|c| c.a > 0.0), ..Default::default() };
            p.text = board_text_props(v, p.text);
            NodeKind::TableCell(p)
        }
        "CONNECTOR" => {
            let end = |key: &str| -> ConnectorEnd {
                let e = v.get(key);
                let pos = e.and_then(|e| e.get("position")).and_then(vec2).unwrap_or(Vec2::ZERO);
                ConnectorEnd {
                    node: None,
                    magnet: magnet_from_name(e.and_then(|e| fstr(e, "magnet"))),
                    // free ends are absolute; the link pass replaces attached ones
                    point: Vec2::new(pos.x - origin.x, pos.y - origin.y),
                }
            };
            let mut p = ConnectorProps {
                start: end("connectorStart"),
                end: end("connectorEnd"),
                line: match fstr(v, "connectorLineType") {
                    Some("STRAIGHT") => ConnectorLine::Straight,
                    Some("CURVED") => ConnectorLine::Curved,
                    _ => ConnectorLine::Elbowed,
                },
                start_cap: cap_from_name(fstr(v, "connectorStartStrokeCap")),
                end_cap: cap_from_name(fstr(v, "connectorEndStrokeCap")),
                label: None,
            };
            let label = board_text_props(v, board_text(14.0, TextAlignH::Center, TextAlignV::Center));
            if !label.content.is_empty() {
                p.label = Some(label);
            }
            NodeKind::Connector(p)
        }
        _ => return None,
    })
}

fn node_size(v: &Value) -> Option<Vec2> {
    v.get("size").and_then(vec2).or_else(|| v.get("absoluteBoundingBox").and_then(|b| Some(Vec2::new(fnum(b, "width")?, fnum(b, "height")?))))
}

fn index_of(v: &Value, key: &str) -> Option<usize> {
    fnum(v, key).filter(|f| *f >= 0.0).map(|f| f as usize)
}

/// The cells of a REST `TABLE` in row-major order (by `rowIndex` / `columnIndex` when they are given).
pub(crate) fn table_children(v: &Value) -> Vec<&Value> {
    let mut cells: Vec<&Value> = farr(v, "children").iter().collect();
    if cells.iter().all(|c| index_of(c, "rowIndex").is_some() && index_of(c, "columnIndex").is_some()) {
        cells.sort_by_key(|c| (index_of(c, "rowIndex"), index_of(c, "columnIndex")));
    }
    cells
}

/// Grid of a REST `TABLE`: its size in rows and columns, and the cell sizes where given.
fn table_props(v: &Value) -> TableProps {
    let cells = table_children(v);
    let max_idx = |key: &str| cells.iter().filter_map(|c| index_of(c, key)).max().map(|m| m + 1);
    let cols = index_of(v, "numColumns").or_else(|| max_idx("columnIndex")).unwrap_or(1).max(1);
    let rows = index_of(v, "numRows").or_else(|| max_idx("rowIndex")).unwrap_or_else(|| cells.len().div_ceil(cols)).max(1);
    let mut p = TableProps::new(cols, rows);
    for (i, c) in cells.iter().enumerate() {
        let (r, col) = (index_of(c, "rowIndex").unwrap_or(i / cols), index_of(c, "columnIndex").unwrap_or(i % cols));
        if let Some(s) = node_size(c).filter(|s| s.x > 0.0 && s.y > 0.0) {
            if col < cols && r < rows {
                if r == 0 {
                    p.col_widths[col] = s.x.max(TABLE_MIN_COL_W);
                }
                if col == 0 {
                    p.row_heights[r] = s.y.max(TABLE_MIN_ROW_H);
                }
            }
        }
    }
    p
}

/// After the cells of a table were imported: pads missing cells, drops extras and puts every
/// cell on the grid.
pub(crate) fn finish_table(doc: &mut Document, id: NodeId) {
    let Some(want) = doc.get(id).and_then(|n| if let NodeKind::Table(t) = &n.kind { Some(t.rows() * t.cols()) } else { None }) else { return };
    let have = doc.get(id).map(|n| n.children.len()).unwrap_or(0);
    for extra in doc.get(id).map(|n| n.children.iter().skip(want).copied().collect::<Vec<_>>()).unwrap_or_default() {
        let _ = doc.remove(extra);
    }
    for _ in have..want {
        let cid = doc.alloc_id();
        let _ = doc.insert(id, Node::table_cell(cid));
    }
    sync_table(doc, id);
}

/// Fix-ups after the generic node fields were parsed.
pub(crate) fn finish_node(n: &mut Node) {
    match &mut n.kind {
        NodeKind::Sticky(_) => {
            // the sticky color lives in its props
            n.fills.clear();
            n.stroke.paints.clear();
            n.stroke.weight = 0.0;
        }
        NodeKind::Connector(_) => {
            if n.stroke.paints.is_empty() {
                n.stroke.paints = ink();
            }
            if n.stroke.weight <= 0.0 {
                n.stroke.weight = 2.0;
            }
            n.stroke.align = StrokeAlign::Center;
            n.stroke.join = StrokeJoin::Round;
            n.stroke.cap = StrokeCap::Round;
        }
        NodeKind::BoardSection(_) | NodeKind::TableCell(_) | NodeKind::Table(_) => {
            // the colour lives in the props
            n.fills.clear();
            n.stroke.paints.clear();
            n.stroke.weight = 0.0;
        }
        NodeKind::ShapeWithText(_) => {
            if n.stroke.paints.is_empty() {
                n.stroke.paints = ink();
                n.stroke.weight = n.stroke.weight.max(2.0);
                n.stroke.align = StrokeAlign::Center;
            }
        }
        _ => {}
    }
}

/// Attaches connector ends to the nodes they name (`endpointNodeId` is a REST id) and
/// places every connector on its route. Walks the REST tree below `root`.
pub(crate) fn link_connectors(doc: &mut Document, ids: &BTreeMap<String, NodeId>, root: &Value) {
    fn rec(doc: &mut Document, ids: &BTreeMap<String, NodeId>, v: &Value, out: &mut Vec<NodeId>) {
        if fstr(v, "type") == Some("CONNECTOR") {
            if let Some(&cid) = fstr(v, "id").and_then(|s| ids.get(s)) {
                let target = |key: &str| v.get(key).and_then(|e| fstr(e, "endpointNodeId")).and_then(|s| ids.get(s)).copied();
                let (a, b) = (target("connectorStart"), target("connectorEnd"));
                if let Some(NodeKind::Connector(c)) = doc.get_mut(cid).map(|n| &mut n.kind) {
                    c.start.node = a;
                    c.end.node = b;
                }
                out.push(cid);
            }
        }
        for c in farr(v, "children") {
            rec(doc, ids, c, out);
        }
    }
    let mut connectors = Vec::new();
    rec(doc, ids, root, &mut connectors);
    for c in connectors {
        sync_connector(doc, c);
    }
}

/// REST `type` of a board kind.
pub(crate) fn type_name(k: &NodeKind) -> Option<&'static str> {
    match k {
        NodeKind::Sticky(_) => Some("STICKY"),
        NodeKind::Connector(_) => Some("CONNECTOR"),
        NodeKind::ShapeWithText(_) => Some("SHAPE_WITH_TEXT"),
        NodeKind::Stamp { .. } => Some("STAMP"),
        NodeKind::BoardSection(_) => Some("SECTION"),
        NodeKind::Table(_) => Some("TABLE"),
        NodeKind::TableCell(_) => Some("TABLE_CELL"),
        NodeKind::CodeBlock(_) => Some("CODE_BLOCK"),
        _ => None,
    }
}

fn text_fields(m: &mut Map<String, Value>, t: &TextProps) {
    m.insert("characters".into(), t.content.clone().into());
    let s = &t.style;
    m.insert(
        "style".into(),
        json!({
            "fontFamily": s.font_family,
            "fontWeight": s.font_weight,
            "fontSize": n(s.font_size),
            "textAlignHorizontal": match s.align_h { TextAlignH::Left => "LEFT", TextAlignH::Center => "CENTER", TextAlignH::Right => "RIGHT", TextAlignH::Justified => "JUSTIFIED" },
            "textAlignVertical": match s.align_v { TextAlignV::Top => "TOP", TextAlignV::Center => "CENTER", TextAlignV::Bottom => "BOTTOM" },
        }),
    );
}

/// Board-specific fields of an exported node. `origin` is the page-space position of the
/// parent (connector points are written in page space); `id_of` maps a node to its REST id.
pub(crate) fn export_fields(node: &Node, origin: Vec2, m: &mut Map<String, Value>, id_of: &dyn Fn(NodeId) -> String) {
    match &node.kind {
        NodeKind::Sticky(s) => {
            m.insert("fills".into(), paints(&[Paint::solid(s.color)]));
            m.insert("authorVisible".into(), s.show_author.into());
            if let Some(a) = &s.author {
                m.insert("authorName".into(), a.clone().into());
            }
            text_fields(m, &s.text);
        }
        NodeKind::ShapeWithText(s) => {
            m.insert("shapeType".into(), shape_name(s.shape).into());
            text_fields(m, &s.text);
        }
        NodeKind::Stamp { kind } => {
            m.insert("stampType".into(), kind.clone().into());
        }
        NodeKind::BoardSection(s) => {
            m.insert("fills".into(), paints(&[Paint::solid(s.color.color())]));
            m.insert("sectionContentsHidden".into(), s.contents_hidden.into());
        }
        NodeKind::Table(t) => {
            m.insert("numRows".into(), t.rows().into());
            m.insert("numColumns".into(), t.cols().into());
        }
        NodeKind::TableCell(c) => {
            m.insert("fills".into(), paints(&c.fill.map(Paint::solid).into_iter().collect::<Vec<_>>()));
            text_fields(m, &c.text);
        }
        NodeKind::CodeBlock(c) => {
            m.insert("code".into(), c.text.content.clone().into());
            m.insert("codeLanguage".into(), c.lang.label().into());
        }
        NodeKind::Connector(c) => {
            let end = |e: &ConnectorEnd| {
                let mut o = Map::new();
                if let Some(t) = e.node {
                    o.insert("endpointNodeId".into(), id_of(t).into());
                }
                o.insert("position".into(), v2(Vec2::new(e.point.x + origin.x, e.point.y + origin.y)));
                o.insert("magnet".into(), magnet_name(e.magnet).into());
                Value::Object(o)
            };
            m.insert("connectorStart".into(), end(&c.start));
            m.insert("connectorEnd".into(), end(&c.end));
            m.insert(
                "connectorLineType".into(),
                match c.line {
                    ConnectorLine::Elbowed => "ELBOWED",
                    ConnectorLine::Straight => "STRAIGHT",
                    ConnectorLine::Curved => "CURVED",
                }
                .into(),
            );
            m.insert("connectorStartStrokeCap".into(), cap_name(c.start_cap).into());
            m.insert("connectorEndStrokeCap".into(), cap_name(c.end_cap).into());
            if let Some(l) = &c.label {
                text_fields(m, l);
            }
        }
        _ => {}
    }
}
