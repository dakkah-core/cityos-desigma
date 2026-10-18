//! Board sections and tables.
//!
//! A *section* is a named, coloured region of the board. It is a container: the nodes inside
//! are its children, dragging it moves them, resizing it leaves them where they are. Its
//! title is the node name; the colour and the "contents hidden" switch live in
//! [`SectionProps`].
//!
//! A *table* is a grid of text cells. The table node holds the column widths and row
//! heights ([`TableProps`]); every cell is a child [`NodeKind::TableCell`] in row-major
//! order, carrying its own text and fill. The cell nodes' boxes are derived from the grid
//! ([`sync_table`]), so text editing and hit testing can treat a cell like any other text
//! carrying node.

use crate::*;
use serde::{Deserialize, Serialize};

// ------------------------------------------------------------------- sections

/// Section corner radius.
pub const SECTION_RADIUS: f32 = 12.0;
/// Height of the title pill that sits on the top left corner of a section.
pub const SECTION_PILL_H: f32 = 28.0;
/// Gap between the title pill and the section.
pub const SECTION_PILL_GAP: f32 = 6.0;
/// How far above its box a section draws (title pill and gap).
pub const SECTION_TITLE_REACH: f32 = SECTION_PILL_H + SECTION_PILL_GAP + 2.0;
/// Horizontal padding inside the title pill.
pub const SECTION_PILL_PAD: f32 = 10.0;
/// Title text size.
pub const SECTION_TITLE_SIZE: f32 = 14.0;
/// Opacity of the section fill.
pub const SECTION_FILL_ALPHA: f32 = 0.5;
/// Default size of a section placed with a click.
pub const SECTION_DEFAULT_SIZE: Vec2 = Vec2 { x: 640.0, y: 420.0 };

/// FigJam's section colours: eight hues and a light variant of each.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SectionColor {
    #[default]
    Gray,
    Red,
    Orange,
    Yellow,
    Green,
    Blue,
    Violet,
    Pink,
    LightGray,
    LightRed,
    LightOrange,
    LightYellow,
    LightGreen,
    LightBlue,
    LightViolet,
    LightPink,
}

impl SectionColor {
    pub const ALL: [SectionColor; 16] = [
        SectionColor::Gray,
        SectionColor::Red,
        SectionColor::Orange,
        SectionColor::Yellow,
        SectionColor::Green,
        SectionColor::Blue,
        SectionColor::Violet,
        SectionColor::Pink,
        SectionColor::LightGray,
        SectionColor::LightRed,
        SectionColor::LightOrange,
        SectionColor::LightYellow,
        SectionColor::LightGreen,
        SectionColor::LightBlue,
        SectionColor::LightViolet,
        SectionColor::LightPink,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SectionColor::Gray => "Gray",
            SectionColor::Red => "Red",
            SectionColor::Orange => "Orange",
            SectionColor::Yellow => "Yellow",
            SectionColor::Green => "Green",
            SectionColor::Blue => "Blue",
            SectionColor::Violet => "Violet",
            SectionColor::Pink => "Pink",
            SectionColor::LightGray => "Light gray",
            SectionColor::LightRed => "Light red",
            SectionColor::LightOrange => "Light orange",
            SectionColor::LightYellow => "Light yellow",
            SectionColor::LightGreen => "Light green",
            SectionColor::LightBlue => "Light blue",
            SectionColor::LightViolet => "Light violet",
            SectionColor::LightPink => "Light pink",
        }
    }

    pub fn is_light(self) -> bool {
        (self as usize) >= 8
    }

    /// `0xRRGGBB` of the colour: the title pill, and the fill before it is made translucent.
    pub fn rgb(self) -> u32 {
        match self {
            SectionColor::Gray => 0xBDBDBD,
            SectionColor::Red => 0xFF9C8F,
            SectionColor::Orange => 0xFFB86B,
            SectionColor::Yellow => 0xFFD84D,
            SectionColor::Green => 0x7FD99A,
            SectionColor::Blue => 0x7DB8FF,
            SectionColor::Violet => 0xB69BFF,
            SectionColor::Pink => 0xF59AD8,
            SectionColor::LightGray => 0xE6E6E6,
            SectionColor::LightRed => 0xFFD1CB,
            SectionColor::LightOrange => 0xFFDDB3,
            SectionColor::LightYellow => 0xFFEE9E,
            SectionColor::LightGreen => 0xBDEFCB,
            SectionColor::LightBlue => 0xBCDAFF,
            SectionColor::LightViolet => 0xDACCFF,
            SectionColor::LightPink => 0xFAC9EB,
        }
    }

    /// The colour as a document colour (opaque).
    pub fn color(self) -> Color {
        let c = self.rgb();
        Color::rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
    }

    /// The translucent fill of the section body.
    pub fn fill(self) -> Color {
        self.color().with_alpha(SECTION_FILL_ALPHA)
    }

    /// The colour closest to `c` (used to map imported fills).
    pub fn nearest(c: Color) -> SectionColor {
        let d = |a: Color| (a.r - c.r).powi(2) + (a.g - c.g).powi(2) + (a.b - c.b).powi(2);
        SectionColor::ALL.iter().copied().min_by(|a, b| d(a.color()).partial_cmp(&d(b.color())).unwrap_or(std::cmp::Ordering::Equal)).unwrap_or_default()
    }
}

/// Props of a board section; the title is the node name.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SectionProps {
    pub color: SectionColor,
    /// The nodes inside are not drawn (nor hit) while this is set.
    pub contents_hidden: bool,
}

impl Node {
    /// A board section at `r`, named `title`.
    pub fn board_section(id: NodeId, r: Rect, title: impl Into<String>) -> Self {
        let mut n = Self::new(id, NodeKind::BoardSection(SectionProps::default()));
        n.name = title.into();
        n.transform = Affine::translate(r.x, r.y);
        n.size = Vec2::new(r.w, r.h);
        n
    }
}

/// The children of a section that are drawn and hit (none while its contents are hidden).
pub fn section_visible_children(n: &Node) -> &[NodeId] {
    match &n.kind {
        NodeKind::BoardSection(s) if s.contents_hidden => &[],
        _ => &n.children,
    }
}

// --------------------------------------------------------------------- tables

/// Default column width.
pub const TABLE_COL_W: f32 = 200.0;
/// Default row height.
pub const TABLE_ROW_H: f32 = 56.0;
/// Narrowest a column gets when dragged or scaled.
pub const TABLE_MIN_COL_W: f32 = 48.0;
/// Shortest a row gets when dragged or scaled.
pub const TABLE_MIN_ROW_H: f32 = 32.0;
/// FigJam's default table: two columns, three rows.
pub const TABLE_DEFAULT_COLS: usize = 2;
pub const TABLE_DEFAULT_ROWS: usize = 3;
/// Padding between a cell's border and its text.
pub const TABLE_CELL_PAD: f32 = 10.0;
/// Text size of a cell (it shrinks to fit like a sticky note's).
pub const TABLE_FONT: f32 = 16.0;
/// Grid line colour.
pub const TABLE_LINE: Color = Color { r: 0.769, g: 0.769, b: 0.769, a: 1.0 };

/// A table: widths of its columns and heights of its rows, in order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TableProps {
    pub col_widths: Vec<f32>,
    pub row_heights: Vec<f32>,
}

impl Default for TableProps {
    fn default() -> Self {
        Self::new(TABLE_DEFAULT_COLS, TABLE_DEFAULT_ROWS)
    }
}

impl TableProps {
    pub fn new(cols: usize, rows: usize) -> Self {
        Self { col_widths: vec![TABLE_COL_W; cols.max(1)], row_heights: vec![TABLE_ROW_H; rows.max(1)] }
    }

    pub fn cols(&self) -> usize {
        self.col_widths.len()
    }

    pub fn rows(&self) -> usize {
        self.row_heights.len()
    }

    pub fn size(&self) -> Vec2 {
        Vec2::new(self.col_widths.iter().sum(), self.row_heights.iter().sum())
    }

    /// Left edge of column `c` (`c == cols()` is the right edge of the table).
    pub fn col_x(&self, c: usize) -> f32 {
        self.col_widths.iter().take(c).sum()
    }

    /// Top edge of row `r` (`r == rows()` is the bottom edge of the table).
    pub fn row_y(&self, r: usize) -> f32 {
        self.row_heights.iter().take(r).sum()
    }

    /// The box of the cell at (`row`, `col`).
    pub fn cell_rect(&self, row: usize, col: usize) -> Rect {
        Rect::new(self.col_x(col), self.row_y(row), self.col_widths.get(col).copied().unwrap_or(0.0), self.row_heights.get(row).copied().unwrap_or(0.0))
    }

    /// Index of a cell in the table's child list.
    pub fn cell_index(&self, row: usize, col: usize) -> usize {
        row * self.cols() + col
    }

    /// (row, col) of the child at `index`.
    pub fn cell_pos(&self, index: usize) -> (usize, usize) {
        (index / self.cols().max(1), index % self.cols().max(1))
    }

    /// The (row, col) under `p` (table-local), if inside the table.
    pub fn cell_at(&self, p: Vec2) -> Option<(usize, usize)> {
        let s = self.size();
        if p.x < 0.0 || p.y < 0.0 || p.x > s.x || p.y > s.y {
            return None;
        }
        let find = |sizes: &[f32], v: f32| {
            let mut acc = 0.0;
            for (i, w) in sizes.iter().enumerate() {
                acc += w;
                if v < acc {
                    return i;
                }
            }
            sizes.len().saturating_sub(1)
        };
        Some((find(&self.row_heights, p.y), find(&self.col_widths, p.x)))
    }

    /// Scales the grid to `size` (widths and heights keep their proportions, within the minimums).
    pub fn scale_to(&mut self, size: Vec2) {
        fn scale(v: &mut [f32], total: f32, min: f32) {
            let sum: f32 = v.iter().sum();
            if sum <= 0.0 || v.is_empty() {
                return;
            }
            let k = total.max(min * v.len() as f32) / sum;
            for x in v.iter_mut() {
                *x = (*x * k).max(min);
            }
        }
        scale(&mut self.col_widths, size.x, TABLE_MIN_COL_W);
        scale(&mut self.row_heights, size.y, TABLE_MIN_ROW_H);
    }
}

impl Node {
    pub fn table_cell(id: NodeId) -> Self {
        let mut n = Self::new(id, NodeKind::TableCell(TableCellProps::default()));
        n.name = "Cell".into();
        n
    }
}

/// Props of one table cell.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TableCellProps {
    /// Background of the cell; `None` leaves it white.
    pub fill: Option<Color>,
    pub text: TextProps,
}

impl Default for TableCellProps {
    fn default() -> Self {
        Self { fill: None, text: board_text(TABLE_FONT, TextAlignH::Left, TextAlignV::Center) }
    }
}

/// A new table's nodes (the table first, then its cells row by row), ready for `Command::Insert`.
pub fn table_nodes(doc: &mut Document, at: Vec2, cols: usize, rows: usize) -> Vec<Node> {
    let props = TableProps::new(cols, rows);
    let id = doc.alloc_id();
    let mut table = Node::new(id, NodeKind::Table(props.clone()));
    table.name = "Table".into();
    table.transform = Affine::translate(at.x, at.y);
    table.size = props.size();
    let mut cells = Vec::new();
    for r in 0..props.rows() {
        for c in 0..props.cols() {
            let cid = doc.alloc_id();
            let mut cell = Node::table_cell(cid);
            cell.parent = Some(id);
            let b = props.cell_rect(r, c);
            cell.transform = Affine::translate(b.x, b.y);
            cell.size = Vec2::new(b.w, b.h);
            table.children.push(cid);
            cells.push(cell);
        }
    }
    let mut out = vec![table];
    out.extend(cells);
    out
}

/// Where each cell of table `id` belongs: (cell, box), in child order. `None` when `id` is no table.
pub fn table_layout(doc: &Document, id: NodeId) -> Option<Vec<(NodeId, Rect)>> {
    let n = doc.get(id)?;
    let NodeKind::Table(t) = &n.kind else { return None };
    Some(
        n.children
            .iter()
            .enumerate()
            .map(|(i, c)| {
                (*c, {
                    let (r, col) = t.cell_pos(i);
                    t.cell_rect(r, col)
                })
            })
            .collect(),
    )
}

/// Sets the boxes of table `id`'s cells (and its own size) from its grid.
pub fn sync_table(doc: &mut Document, id: NodeId) {
    let Some(size) = doc.get(id).and_then(|n| if let NodeKind::Table(t) = &n.kind { Some(t.size()) } else { None }) else { return };
    let Some(layout) = table_layout(doc, id) else { return };
    if let Some(n) = doc.get_mut(id) {
        n.size = size;
    }
    for (cid, r) in layout {
        if let Some(c) = doc.get_mut(cid) {
            c.transform = Affine::translate(r.x, r.y);
            c.size = Vec2::new(r.w, r.h);
        }
    }
}

/// The table a cell belongs to and the cell's (row, col), if `cell` is a table cell.
pub fn table_of_cell(doc: &Document, cell: NodeId) -> Option<(NodeId, usize, usize)> {
    let n = doc.get(cell)?;
    if !matches!(n.kind, NodeKind::TableCell(_)) {
        return None;
    }
    let parent = n.parent?;
    let p = doc.get(parent)?;
    let NodeKind::Table(t) = &p.kind else { return None };
    let i = p.children.iter().position(|c| *c == cell)?;
    let (r, c) = t.cell_pos(i);
    Some((parent, r, c))
}

/// The cell node at (`row`, `col`) of table `id`.
pub fn table_cell_node(doc: &Document, id: NodeId, row: usize, col: usize) -> Option<NodeId> {
    let n = doc.get(id)?;
    let NodeKind::Table(t) = &n.kind else { return None };
    if row >= t.rows() || col >= t.cols() {
        return None;
    }
    n.children.get(t.cell_index(row, col)).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc_with_table(cols: usize, rows: usize) -> (Document, NodeId, NodeId) {
        let mut d = Document::new("t");
        d.kind = DocumentKind::Board;
        let page = d.add_page("p");
        let nodes = table_nodes(&mut d, Vec2::new(10.0, 20.0), cols, rows);
        let id = nodes[0].id;
        Command::Insert { parent: page, index: 0, nodes }.apply(&mut d).unwrap();
        (d, page, id)
    }

    #[test]
    fn default_table_is_two_columns_by_three_rows() {
        let (d, _, t) = doc_with_table(TABLE_DEFAULT_COLS, TABLE_DEFAULT_ROWS);
        let n = d.get(t).unwrap();
        assert_eq!(n.children.len(), 6);
        assert_eq!(n.size, Vec2::new(2.0 * TABLE_COL_W, 3.0 * TABLE_ROW_H));
        // cells sit in row-major order on the grid
        let lay = table_layout(&d, t).unwrap();
        assert_eq!(lay[1].1, Rect::new(TABLE_COL_W, 0.0, TABLE_COL_W, TABLE_ROW_H));
        assert_eq!(lay[2].1, Rect::new(0.0, TABLE_ROW_H, TABLE_COL_W, TABLE_ROW_H));
        let c = n.children[3];
        assert_eq!(table_of_cell(&d, c), Some((t, 1, 1)));
        assert_eq!(table_cell_node(&d, t, 1, 1), Some(c));
        assert_eq!(table_cell_node(&d, t, 3, 0), None);
        assert_eq!(d.get(c).unwrap().size, Vec2::new(TABLE_COL_W, TABLE_ROW_H));
    }

    #[test]
    fn grid_geometry_and_hit_cells() {
        let mut p = TableProps::new(3, 2);
        p.col_widths = vec![100.0, 50.0, 80.0];
        p.row_heights = vec![40.0, 60.0];
        assert_eq!(p.size(), Vec2::new(230.0, 100.0));
        assert_eq!(p.col_x(2), 150.0);
        assert_eq!(p.row_y(1), 40.0);
        assert_eq!(p.cell_rect(1, 2), Rect::new(150.0, 40.0, 80.0, 60.0));
        assert_eq!(p.cell_at(Vec2::new(120.0, 10.0)), Some((0, 1)));
        assert_eq!(p.cell_at(Vec2::new(229.0, 99.0)), Some((1, 2)));
        assert_eq!(p.cell_at(Vec2::new(-1.0, 10.0)), None);
        assert_eq!(p.cell_pos(p.cell_index(1, 2)), (1, 2));
    }

    #[test]
    fn scaling_keeps_proportions_within_minimums() {
        let mut p = TableProps::new(2, 2);
        p.col_widths = vec![100.0, 300.0];
        p.scale_to(Vec2::new(200.0, 2.0 * TABLE_ROW_H));
        assert!((p.col_widths[0] - 50.0).abs() < 0.01 && (p.col_widths[1] - 150.0).abs() < 0.01, "{:?}", p.col_widths);
        // never below the minimum, however small the drag
        p.scale_to(Vec2::new(1.0, 1.0));
        assert!(p.col_widths.iter().all(|w| *w >= TABLE_MIN_COL_W));
        assert!(p.row_heights.iter().all(|h| *h >= TABLE_MIN_ROW_H));
    }

    #[test]
    fn sync_places_cells_after_the_grid_changes() {
        let (mut d, _, t) = doc_with_table(2, 2);
        if let Some(NodeKind::Table(p)) = d.get_mut(t).map(|n| &mut n.kind) {
            p.col_widths[0] = 90.0;
            p.row_heights[1] = 70.0;
        }
        sync_table(&mut d, t);
        assert_eq!(d.get(t).unwrap().size, Vec2::new(90.0 + TABLE_COL_W, TABLE_ROW_H + 70.0));
        let last = d.get(t).unwrap().children[3];
        let c = d.get(last).unwrap();
        assert_eq!((c.transform.tx, c.transform.ty, c.size), (90.0, TABLE_ROW_H, Vec2::new(TABLE_COL_W, 70.0)));
    }

    #[test]
    fn sections_and_tables_roundtrip_through_json_with_defaults() {
        let (mut d, page, t) = doc_with_table(2, 2);
        let sid = d.alloc_id();
        let mut s = Node::board_section(sid, Rect::new(0.0, 0.0, 300.0, 200.0), "Ideas");
        if let NodeKind::BoardSection(p) = &mut s.kind {
            p.color = SectionColor::LightBlue;
            p.contents_hidden = true;
        }
        d.insert(page, s).unwrap();
        let back = Document::from_json(&d.to_json().unwrap()).unwrap();
        assert_eq!(back.get(sid), d.get(sid));
        assert_eq!(back.get(t), d.get(t));
        // props written by an older build (missing fields) read with their defaults
        let p: SectionProps = serde_json::from_str("{}").unwrap();
        assert_eq!(p, SectionProps::default());
        let tp: TableProps = serde_json::from_str("{}").unwrap();
        assert_eq!((tp.cols(), tp.rows()), (TABLE_DEFAULT_COLS, TABLE_DEFAULT_ROWS));
        let cp: TableCellProps = serde_json::from_str("{}").unwrap();
        assert!(cp.fill.is_none());
    }

    #[test]
    fn section_colours_cover_hues_and_light_variants() {
        assert_eq!(SectionColor::ALL.len(), 16);
        assert!(SectionColor::ALL.iter().filter(|c| c.is_light()).count() == 8);
        let mut seen = std::collections::HashSet::new();
        for c in SectionColor::ALL {
            assert!(seen.insert(c.rgb()), "{c:?} repeats a colour");
            assert!(c.fill().a < 1.0, "section fills are translucent");
        }
        assert_eq!(SectionColor::nearest(SectionColor::Violet.color()), SectionColor::Violet);
        assert_eq!(SectionColor::nearest(SectionColor::LightGreen.color()), SectionColor::LightGreen);
    }

    #[test]
    fn hidden_contents_hide_the_children_from_drawing() {
        let (mut d, page, _) = doc_with_table(1, 1);
        let sid = d.alloc_id();
        let sec = d.insert(page, Node::board_section(sid, Rect::new(0.0, 0.0, 300.0, 200.0), "S")).unwrap();
        let rid = d.alloc_id();
        d.insert(sec, Node::rectangle(rid, Rect::new(10.0, 10.0, 20.0, 20.0))).unwrap();
        assert_eq!(section_visible_children(d.get(sec).unwrap()), &[rid]);
        if let Some(NodeKind::BoardSection(p)) = d.get_mut(sec).map(|n| &mut n.kind) {
            p.contents_hidden = true;
        }
        assert!(section_visible_children(d.get(sec).unwrap()).is_empty());
    }
}
