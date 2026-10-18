//! Board table editing: adding and removing rows and columns, resizing them, and keeping the
//! cell nodes on the grid. Everything goes through a [`Tx`], so one gesture is one undo step.

use crate::edit::{Edit, Tx};
use crate::util::v2;
use desigma_doc::*;

/// Writes the boxes of table `id`'s cells and its own size from the grid, through `ed`.
pub fn sync_table(doc: &mut Document, ed: &mut Edit, id: NodeId) {
    let Some(layout) = table_layout(doc, id) else { return };
    let Some(size) = doc.get(id).and_then(|n| if let NodeKind::Table(t) = &n.kind { Some(t.size()) } else { None }) else { return };
    if doc.get(id).is_some_and(|n| n.size != size) {
        if let Some(n) = ed.node(doc, id) {
            n.size = size;
        }
    }
    for (cid, r) in layout {
        let off = doc.get(cid).is_some_and(|c| {
            (c.transform.tx - r.x).abs() > 1e-3 || (c.transform.ty - r.y).abs() > 1e-3 || (c.size.x - r.w).abs() > 1e-3 || (c.size.y - r.h).abs() > 1e-3
        });
        if off {
            if let Some(c) = ed.node(doc, cid) {
                c.transform = Affine::translate(r.x, r.y);
                c.size = v2(r.w, r.h);
            }
        }
    }
}

/// Scales the grid of table `id` to `size` (after the table was resized by its handles).
pub fn scale_table(doc: &mut Document, ed: &mut Edit, id: NodeId, size: Vec2) {
    if let Some(n) = ed.node(doc, id) {
        if let NodeKind::Table(t) = &mut n.kind {
            t.scale_to(size);
        }
    }
    sync_table(doc, ed, id);
}

fn props(doc: &Document, id: NodeId) -> Option<TableProps> {
    match &doc.get(id)?.kind {
        NodeKind::Table(t) => Some(t.clone()),
        _ => None,
    }
}

fn set_props(doc: &mut Document, tx: &mut Tx, id: NodeId, p: TableProps) {
    if let Some(n) = tx.ed.node(doc, id) {
        n.kind = NodeKind::Table(p);
    }
}

fn new_cell(doc: &mut Document, like: Option<NodeId>) -> Node {
    let id = doc.alloc_id();
    let mut cell = Node::table_cell(id);
    // a new cell takes the look of the cell it grows next to (fill and text style), but no text
    if let Some(NodeKind::TableCell(src)) = like.and_then(|l| doc.get(l)).map(|n| &n.kind) {
        if let NodeKind::TableCell(c) = &mut cell.kind {
            c.fill = src.fill;
            c.text.style = src.text.style.clone();
        }
    }
    cell
}

/// Inserts a row before row `at` (`at == rows` appends). Returns false when `id` is no table.
pub fn insert_row(doc: &mut Document, tx: &mut Tx, id: NodeId, at: usize) -> bool {
    // Reject before new_cell allocates any IDs or a transaction mutates the tree.
    if doc.is_managed() {
        return false;
    }
    let Some(mut p) = props(doc, id) else { return false };
    let at = at.min(p.rows());
    let cols = p.cols();
    let template = at.checked_sub(1).or(Some(at)).filter(|r| *r < p.rows());
    let h = template.map(|r| p.row_heights[r]).unwrap_or(TABLE_ROW_H);
    for c in 0..cols {
        let like = template.and_then(|r| table_cell_node(doc, id, r, c));
        let cell = new_cell(doc, like);
        let index = at * cols + c;
        if tx.cmd(doc, Command::Insert { parent: id, index, nodes: vec![cell] }).is_err() {
            return false;
        }
    }
    p.row_heights.insert(at, h);
    set_props(doc, tx, id, p);
    sync_table(doc, &mut tx.ed, id);
    true
}

/// Inserts a column before column `at` (`at == cols` appends).
pub fn insert_col(doc: &mut Document, tx: &mut Tx, id: NodeId, at: usize) -> bool {
    // Reject before new_cell allocates any IDs or a transaction mutates the tree.
    if doc.is_managed() {
        return false;
    }
    let Some(mut p) = props(doc, id) else { return false };
    let at = at.min(p.cols());
    let (cols, rows) = (p.cols(), p.rows());
    let template = at.checked_sub(1).or(Some(at)).filter(|c| *c < cols);
    let w = template.map(|c| p.col_widths[c]).unwrap_or(TABLE_COL_W);
    for r in 0..rows {
        let like = template.and_then(|c| table_cell_node(doc, id, r, c));
        let cell = new_cell(doc, like);
        // earlier rows already have one more cell
        let index = r * (cols + 1) + at;
        if tx.cmd(doc, Command::Insert { parent: id, index, nodes: vec![cell] }).is_err() {
            return false;
        }
    }
    p.col_widths.insert(at, w);
    set_props(doc, tx, id, p);
    sync_table(doc, &mut tx.ed, id);
    true
}

/// Removes row `row`; a table keeps at least one row.
pub fn remove_row(doc: &mut Document, tx: &mut Tx, id: NodeId, row: usize) -> bool {
    let Some(mut p) = props(doc, id) else { return false };
    if p.rows() <= 1 || row >= p.rows() {
        return false;
    }
    let cells: Vec<NodeId> = (0..p.cols()).filter_map(|c| table_cell_node(doc, id, row, c)).collect();
    for c in cells {
        if tx.cmd(doc, Command::Remove { id: c }).is_err() {
            return false;
        }
    }
    p.row_heights.remove(row);
    set_props(doc, tx, id, p);
    sync_table(doc, &mut tx.ed, id);
    true
}

/// Removes column `col`; a table keeps at least one column.
pub fn remove_col(doc: &mut Document, tx: &mut Tx, id: NodeId, col: usize) -> bool {
    let Some(mut p) = props(doc, id) else { return false };
    if p.cols() <= 1 || col >= p.cols() {
        return false;
    }
    let cells: Vec<NodeId> = (0..p.rows()).filter_map(|r| table_cell_node(doc, id, r, col)).collect();
    for c in cells {
        if tx.cmd(doc, Command::Remove { id: c }).is_err() {
            return false;
        }
    }
    p.col_widths.remove(col);
    set_props(doc, tx, id, p);
    sync_table(doc, &mut tx.ed, id);
    true
}

/// Sets the width of column `col` (clamped to the minimum); the table grows or shrinks with it.
pub fn set_col_width(doc: &mut Document, tx: &mut Tx, id: NodeId, col: usize, w: f32) {
    let Some(mut p) = props(doc, id) else { return };
    if let Some(slot) = p.col_widths.get_mut(col) {
        *slot = w.max(TABLE_MIN_COL_W);
        set_props(doc, tx, id, p);
        sync_table(doc, &mut tx.ed, id);
    }
}

/// Sets the height of row `row` (clamped to the minimum).
pub fn set_row_height(doc: &mut Document, tx: &mut Tx, id: NodeId, row: usize, h: f32) {
    let Some(mut p) = props(doc, id) else { return };
    if let Some(slot) = p.row_heights.get_mut(row) {
        *slot = h.max(TABLE_MIN_ROW_H);
        set_props(doc, tx, id, p);
        sync_table(doc, &mut tx.ed, id);
    }
}

/// The cell after (`forward`) or before `cell` in reading order, if any.
pub fn neighbour_cell(doc: &Document, cell: NodeId, forward: bool) -> Option<NodeId> {
    let (table, r, c) = table_of_cell(doc, cell)?;
    let n = doc.get(table)?;
    let NodeKind::Table(t) = &n.kind else { return None };
    let i = t.cell_index(r, c);
    let j = if forward { i + 1 } else { i.checked_sub(1)? };
    n.children.get(j).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(cols: usize, rows: usize) -> (Document, NodeId) {
        let mut d = Document::new("t");
        d.kind = DocumentKind::Board;
        let page = d.add_page("p");
        let nodes = table_nodes(&mut d, Vec2::ZERO, cols, rows);
        let id = nodes[0].id;
        Command::Insert { parent: page, index: 0, nodes }.apply(&mut d).unwrap();
        (d, id)
    }

    fn set_text(d: &mut Document, t: NodeId, r: usize, c: usize, s: &str) {
        let id = table_cell_node(d, t, r, c).unwrap();
        if let NodeKind::TableCell(cell) = &mut d.get_mut(id).unwrap().kind {
            cell.text.content = s.into();
        }
    }

    fn text_at(d: &Document, t: NodeId, r: usize, c: usize) -> String {
        match &d.get(table_cell_node(d, t, r, c).unwrap()).unwrap().kind {
            NodeKind::TableCell(cell) => cell.text.content.clone(),
            _ => panic!(),
        }
    }

    #[test]
    fn inserting_a_row_in_the_middle_keeps_every_other_cell_where_it_was() {
        let (mut d, t) = table(2, 3);
        for r in 0..3 {
            for c in 0..2 {
                set_text(&mut d, t, r, c, &format!("{r}{c}"));
            }
        }
        let mut tx = Tx::new();
        assert!(insert_row(&mut d, &mut tx, t, 1));
        assert_eq!(d.get(t).unwrap().children.len(), 8);
        assert_eq!(text_at(&d, t, 0, 1), "01");
        assert_eq!(text_at(&d, t, 1, 0), "");
        assert_eq!(text_at(&d, t, 2, 0), "10");
        assert_eq!(text_at(&d, t, 3, 1), "21");
        assert_eq!(d.get(t).unwrap().size.y, 4.0 * TABLE_ROW_H);
        // one undo step brings the old table back
        let mut h = History::default();
        assert!(tx.commit(&mut d, &mut h).unwrap());
        assert!(h.undo(&mut d).unwrap());
        assert_eq!(d.get(t).unwrap().children.len(), 6);
        assert_eq!(text_at(&d, t, 1, 0), "10");
        assert_eq!(d.get(t).unwrap().size.y, 3.0 * TABLE_ROW_H);
    }

    #[test]
    fn inserting_and_removing_columns_shifts_the_right_cells() {
        let (mut d, t) = table(2, 2);
        for r in 0..2 {
            for c in 0..2 {
                set_text(&mut d, t, r, c, &format!("{r}{c}"));
            }
        }
        let mut tx = Tx::new();
        assert!(insert_col(&mut d, &mut tx, t, 1));
        assert_eq!((text_at(&d, t, 0, 0), text_at(&d, t, 0, 1), text_at(&d, t, 0, 2)), ("00".into(), String::new(), "01".into()));
        assert_eq!((text_at(&d, t, 1, 0), text_at(&d, t, 1, 1), text_at(&d, t, 1, 2)), ("10".into(), String::new(), "11".into()));
        assert_eq!(d.get(t).unwrap().size.x, 3.0 * TABLE_COL_W);
        assert!(remove_col(&mut d, &mut tx, t, 0));
        assert_eq!((text_at(&d, t, 0, 0), text_at(&d, t, 0, 1)), (String::new(), "01".into()));
        assert_eq!(d.get(t).unwrap().children.len(), 4);
        // cells follow the grid after every change
        let c = d.get(t).unwrap().children[3];
        assert_eq!(d.get(c).unwrap().transform.tx, TABLE_COL_W);
    }

    #[test]
    fn a_table_keeps_one_row_and_one_column() {
        let (mut d, t) = table(1, 1);
        let mut tx = Tx::new();
        assert!(!remove_row(&mut d, &mut tx, t, 0));
        assert!(!remove_col(&mut d, &mut tx, t, 0));
        assert_eq!(d.get(t).unwrap().children.len(), 1);
    }

    #[test]
    fn dragging_a_divider_resizes_one_column_within_the_minimum() {
        let (mut d, t) = table(2, 2);
        let mut tx = Tx::new();
        set_col_width(&mut d, &mut tx, t, 0, 300.0);
        assert_eq!(d.get(t).unwrap().size.x, 300.0 + TABLE_COL_W);
        set_col_width(&mut d, &mut tx, t, 0, 3.0);
        assert_eq!(d.get(t).unwrap().size.x, TABLE_MIN_COL_W + TABLE_COL_W);
        set_row_height(&mut d, &mut tx, t, 1, 120.0);
        assert_eq!(d.get(t).unwrap().size.y, TABLE_ROW_H + 120.0);
        let last = d.get(t).unwrap().children[3];
        assert_eq!(d.get(last).unwrap().size, Vec2::new(TABLE_COL_W, 120.0));
    }

    #[test]
    fn new_cells_copy_the_fill_of_their_neighbour() {
        let (mut d, t) = table(2, 2);
        let c = table_cell_node(&d, t, 1, 0).unwrap();
        if let NodeKind::TableCell(cell) = &mut d.get_mut(c).unwrap().kind {
            cell.fill = Some(Color::rgb(0xFF, 0xE5, 0x8F));
        }
        let mut tx = Tx::new();
        insert_row(&mut d, &mut tx, t, 2);
        let fresh = table_cell_node(&d, t, 2, 0).unwrap();
        assert!(matches!(&d.get(fresh).unwrap().kind, NodeKind::TableCell(TableCellProps { fill: Some(_), .. })));
    }

    #[test]
    fn reading_order_neighbours() {
        let (d, t) = table(2, 2);
        let a = table_cell_node(&d, t, 0, 1).unwrap();
        assert_eq!(neighbour_cell(&d, a, true), table_cell_node(&d, t, 1, 0));
        assert_eq!(neighbour_cell(&d, a, false), table_cell_node(&d, t, 0, 0));
        let last = table_cell_node(&d, t, 1, 1).unwrap();
        assert_eq!(neighbour_cell(&d, last, true), None);
        let first = table_cell_node(&d, t, 0, 0).unwrap();
        assert_eq!(neighbour_cell(&d, first, false), None);
    }
}
