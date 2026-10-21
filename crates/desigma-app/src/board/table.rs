//! Tables on a board: hover and selection handles (the + buttons that add a row or column at a
//! border, the dividers dragged to resize), picking and editing cells, Tab between cells, and the
//! edits the contextual toolbar offers. The model and its edit operations live in
//! `desigma_doc::board_blocks` and `desigma_engine::table`.

use crate::app::App;
use crate::canvas::tools::Tool;
use crate::canvas::{interact, Interaction};
use crate::edit::Tx;
use crate::theme;
use crate::util::*;
use desigma_doc::*;
use desigma_engine::table as ops;
use egui::{Color32, Painter, Pos2, Rect as ERect, Stroke};

/// Distance of the + buttons from the table edge, in screen pixels.
pub const PLUS_OFFSET: f32 = 16.0;
/// Radius of an open + button.
pub const PLUS_RADIUS: f32 = 9.0;
/// How close (screen pixels) the pointer has to be to a + button or divider.
pub const HIT: f32 = 10.0;
const DIVIDER_HIT: f32 = 4.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Col,
    Row,
}

/// A column or row being resized by its divider. The transaction holds every preview step.
pub struct TableDrag {
    pub tx: Tx,
    pub id: NodeId,
    pub axis: Axis,
    /// The column (or row) whose size changes: the one left of (above) the divider.
    pub index: usize,
    /// Pointer position along the axis (table space) when the drag started, and the size then.
    pub start: f32,
    pub orig: f32,
}

fn props(app: &App, id: NodeId) -> Option<TableProps> {
    match &app.doc.get(id)?.kind {
        NodeKind::Table(t) => Some(t.clone()),
        _ => None,
    }
}

/// The table whose handles are shown: the selected one, else the one under the pointer.
pub fn handle_table(app: &App) -> Option<NodeId> {
    let is_table = |id: NodeId| matches!(app.doc.get(id).map(|n| &n.kind), Some(NodeKind::Table(_)));
    match app.selection.as_slice() {
        [one] if is_table(*one) => Some(*one),
        [] | [_] => app.board.hover_table.filter(|h| is_table(*h)),
        _ => None,
    }
}

/// Keeps `board.hover_table` on the table under the pointer, or on the one whose + buttons the
/// pointer is reaching for (the buttons sit outside the table).
pub fn update_hover(app: &mut App, origin: Pos2, page: Option<Vec2>) {
    let idle = matches!(app.canvas.inter, Interaction::None);
    let Some(page) = page.filter(|_| app.is_board() && app.tool == Tool::Move) else {
        if idle {
            app.board.hover_table = None;
        }
        return;
    };
    if !idle {
        return;
    }
    let is_table = |id: NodeId| matches!(app.doc.get(id).map(|n| &n.kind), Some(NodeKind::Table(_)));
    let under = interact::hit_under(app, page, false).filter(|id| is_table(*id));
    app.board.hover_table = under.or_else(|| {
        app.board.hover_table.filter(|t| is_table(*t)).filter(|t| {
            let r = world_aabb(&app.doc, *t);
            let m = (PLUS_OFFSET + HIT) / app.view.zoom;
            Rect::new(r.x - m, r.y - m, r.w + 2.0 * m, r.h + 2.0 * m).contains(page)
        })
    });
    let _ = origin;
}

/// Screen position of a table-space point.
fn screen(app: &App, origin: Pos2, id: NodeId, local: Vec2) -> Pos2 {
    app.view.to_screen(origin, app.doc.world_transform(id).apply(local))
}

/// The + buttons of table `id`: (axis, border index, screen position). A column button sits above
/// the top edge at every column border, a row button left of the left edge at every row border;
/// the border index is where a new column (row) would be inserted.
pub fn plus_handles(app: &App, origin: Pos2, id: NodeId) -> Vec<(Axis, usize, Pos2)> {
    let Some(t) = props(app, id) else { return Vec::new() };
    let off = PLUS_OFFSET / app.view.zoom;
    let mut v = Vec::new();
    for c in 0..=t.cols() {
        v.push((Axis::Col, c, screen(app, origin, id, Vec2::new(t.col_x(c), -off))));
    }
    for r in 0..=t.rows() {
        v.push((Axis::Row, r, screen(app, origin, id, Vec2::new(-off, t.row_y(r)))));
    }
    v
}

/// The divider (axis, index of the column or row it resizes) under screen point `pos`.
pub fn divider_at(app: &App, origin: Pos2, id: NodeId, pos: Pos2) -> Option<(Axis, usize)> {
    let t = props(app, id)?;
    let size = t.size();
    let near = |a: Pos2, b: Pos2| {
        let ab = b - a;
        let l2 = ab.length_sq().max(1e-6);
        let k = ((pos - a).dot(ab) / l2).clamp(0.0, 1.0);
        (a + ab * k - pos).length() <= DIVIDER_HIT
    };
    for c in 1..t.cols() {
        let x = t.col_x(c);
        if near(screen(app, origin, id, Vec2::new(x, 0.0)), screen(app, origin, id, Vec2::new(x, size.y))) {
            return Some((Axis::Col, c - 1));
        }
    }
    for r in 1..t.rows() {
        let y = t.row_y(r);
        if near(screen(app, origin, id, Vec2::new(0.0, y)), screen(app, origin, id, Vec2::new(size.x, y))) {
            return Some((Axis::Row, r - 1));
        }
    }
    None
}

/// The cell (id, row, col) of table `id` under page point `page`.
pub fn cell_at(app: &App, id: NodeId, page: Vec2) -> Option<(NodeId, usize, usize)> {
    let t = props(app, id)?;
    let local = app.doc.world_transform(id).invert()?.apply(page);
    let (r, c) = t.cell_at(local)?;
    Some((table_cell_node(&app.doc, id, r, c)?, r, c))
}

/// The active cell of the selected table (the one last edited or clicked), if still valid.
pub fn active_cell(app: &App) -> Option<(NodeId, usize, usize)> {
    let cell = app.board.cell?;
    let (table, r, c) = table_of_cell(&app.doc, cell)?;
    app.selection.contains(&table).then_some((table, r, c)).map(|(_, r, c)| (cell, r, c))
}

// ------------------------------------------------------------------- pointer

/// A press on a + button or divider of the table whose handles are shown. Returns true when the
/// press was taken.
pub fn press(app: &mut App, origin: Pos2, pos: Pos2, page: Vec2) -> bool {
    if app.tool != Tool::Move {
        return false;
    }
    let Some(id) = handle_table(app) else { return false };
    if app.doc.get(id).is_some_and(|n| n.locked) {
        return false;
    }
    if let Some((axis, at, _)) = plus_handles(app, origin, id).into_iter().find(|(_, _, p)| (*p - pos).length() <= HIT) {
        let mut tx = Tx::new();
        let done = match axis {
            Axis::Col => ops::insert_col(&mut app.doc, &mut tx, id, at),
            Axis::Row => ops::insert_row(&mut app.doc, &mut tx, id, at),
        };
        if done {
            app.run_tx(tx);
            app.selection = vec![id];
        } else {
            tx.cancel(&mut app.doc);
        }
        return true;
    }
    if let Some((axis, index)) = divider_at(app, origin, id, pos) {
        let Some(t) = props(app, id) else { return false };
        let Some(inv) = app.doc.world_transform(id).invert() else { return false };
        let local = inv.apply(page);
        let (start, orig) = match axis {
            Axis::Col => (local.x, t.col_widths[index]),
            Axis::Row => (local.y, t.row_heights[index]),
        };
        app.selection = vec![id];
        app.canvas.inter = Interaction::Table(TableDrag { tx: Tx::new(), id, axis, index, start, orig });
        return true;
    }
    false
}

/// Pointer moved to `page` while dragging a divider.
pub fn drag(app: &mut App, d: &mut TableDrag, page: Vec2) {
    d.tx.ed.revert(&mut app.doc);
    let Some(inv) = app.doc.world_transform(d.id).invert() else { return };
    let local = inv.apply(page);
    match d.axis {
        Axis::Col => ops::set_col_width(&mut app.doc, &mut d.tx, d.id, d.index, d.orig + (local.x - d.start)),
        Axis::Row => ops::set_row_height(&mut app.doc, &mut d.tx, d.id, d.index, d.orig + (local.y - d.start)),
    }
    crate::layout::relayout_up(&mut app.doc, &mut d.tx.ed, d.id);
    app.changed();
}

/// The pointer was released: the divider drag is one undo step.
pub fn release(app: &mut App, d: TableDrag) {
    app.run_tx(d.tx);
}

/// The cursor over a divider (resize) or a + button (pointer), if the pointer is on one.
pub fn cursor_at(app: &App, origin: Pos2, pos: Pos2) -> Option<egui::CursorIcon> {
    if app.tool != Tool::Move {
        return None;
    }
    let id = handle_table(app)?;
    if plus_handles(app, origin, id).iter().any(|(_, _, p)| (*p - pos).length() <= HIT) {
        return Some(egui::CursorIcon::PointingHand);
    }
    divider_at(app, origin, id, pos).map(|(axis, _)| if axis == Axis::Col { egui::CursorIcon::ResizeHorizontal } else { egui::CursorIcon::ResizeVertical })
}

// ------------------------------------------------------------------- editing

/// Starts typing into `cell` (selecting its table, remembering the cell as the active one).
pub fn edit_cell(app: &mut App, cell: NodeId) {
    interact::begin_text_edit(app, cell);
}

/// Double-click on table `id` at `page`: edit the cell under the pointer.
pub fn edit_at(app: &mut App, id: NodeId, page: Vec2) {
    let cell = cell_at(app, id, page).map(|c| c.0).or_else(|| table_cell_node(&app.doc, id, 0, 0));
    if let Some(c) = cell {
        edit_cell(app, c);
    }
}

/// A click on a cell of the already selected table makes it the active cell.
pub fn pick_cell(app: &mut App, id: NodeId, page: Vec2) {
    app.board.cell = cell_at(app, id, page).map(|c| c.0);
}

/// Tab (Shift+Tab) on a selected table: edit the cell after (before) the active one, or the first
/// (last) cell when none is active.
pub fn tab_selected(app: &mut App, forward: bool) -> bool {
    let [id] = app.selection.as_slice() else { return false };
    let id = *id;
    let Some(t) = props(app, id) else { return false };
    let n = t.rows() * t.cols();
    let target = match active_cell(app).and_then(|(c, ..)| ops::neighbour_cell(&app.doc, c, forward)) {
        Some(c) => Some(c),
        None if active_cell(app).is_some() => None,
        None => app.doc.get(id).and_then(|t| if forward { t.children.first() } else { t.children.get(n.saturating_sub(1)) }).copied(),
    };
    match target {
        Some(c) => {
            edit_cell(app, c);
            true
        }
        None => false,
    }
}

/// Tab (Shift+Tab) while typing in a cell: move to the next (previous) cell; Tab in the last cell
/// adds a row, like FigJam. Returns true when `cell` is a table cell.
pub fn tab_cell(app: &mut App, forward: bool) -> bool {
    let Some(cell) = app.canvas.text.as_ref().map(|s| s.id) else { return false };
    let Some((table, _, _)) = table_of_cell(&app.doc, cell) else { return false };
    match ops::neighbour_cell(&app.doc, cell, forward) {
        Some(next) => edit_cell(app, next),
        None if forward => {
            // commit the text, then grow the table in its own undo step
            interact::finish_text(app, true);
            let rows = props(app, table).map(|t| t.rows()).unwrap_or(0);
            let mut tx = Tx::new();
            if ops::insert_row(&mut app.doc, &mut tx, table, rows) {
                app.run_tx(tx);
                if let Some(c) = table_cell_node(&app.doc, table, rows, 0) {
                    edit_cell(app, c);
                }
            } else {
                tx.cancel(&mut app.doc);
            }
        }
        None => {}
    }
    true
}

/// The cells the contextual bar's fill applies to: the active cell, else every cell.
pub fn fill_targets(app: &App, table: NodeId) -> Vec<NodeId> {
    match active_cell(app) {
        Some((c, ..)) if app.doc.get(c).is_some_and(|n| n.parent == Some(table)) => vec![c],
        _ => app.doc.get(table).map(|n| n.children.clone()).unwrap_or_default(),
    }
}

/// Sets (or clears) the fill of the bar's target cells as one undo step.
pub fn set_fill(app: &mut App, table: NodeId, fill: Option<Color>) {
    let ids = fill_targets(app, table);
    app.edit_nodes(&ids, |n| {
        if let NodeKind::TableCell(c) = &mut n.kind {
            c.fill = fill;
        }
    });
}

/// The fill the bar shows: that of the active cell, else the first cell.
pub fn current_fill(app: &App, table: NodeId) -> Option<Color> {
    let id = fill_targets(app, table).first().copied()?;
    match &app.doc.get(id)?.kind {
        NodeKind::TableCell(c) => c.fill,
        _ => None,
    }
}

/// Adds a row or column after the active one (at the end when no cell is active).
pub fn add(app: &mut App, table: NodeId, axis: Axis) {
    let Some(t) = props(app, table) else { return };
    let cell = active_cell(app).map(|(_, r, c)| (r, c));
    let at = match (axis, cell) {
        (Axis::Row, Some((r, _))) => r + 1,
        (Axis::Col, Some((_, c))) => c + 1,
        (Axis::Row, None) => t.rows(),
        (Axis::Col, None) => t.cols(),
    };
    let mut tx = Tx::new();
    let done = match axis {
        Axis::Row => ops::insert_row(&mut app.doc, &mut tx, table, at),
        Axis::Col => ops::insert_col(&mut app.doc, &mut tx, table, at),
    };
    if done {
        app.run_tx(tx);
    } else {
        tx.cancel(&mut app.doc);
    }
}

/// Removes the active row or column (the last one when no cell is active).
pub fn remove(app: &mut App, table: NodeId, axis: Axis) {
    let Some(t) = props(app, table) else { return };
    let cell = active_cell(app).map(|(_, r, c)| (r, c));
    let at = match (axis, cell) {
        (Axis::Row, Some((r, _))) => r,
        (Axis::Col, Some((_, c))) => c,
        (Axis::Row, None) => t.rows().saturating_sub(1),
        (Axis::Col, None) => t.cols().saturating_sub(1),
    };
    let mut tx = Tx::new();
    let done = match axis {
        Axis::Row => ops::remove_row(&mut app.doc, &mut tx, table, at),
        Axis::Col => ops::remove_col(&mut app.doc, &mut tx, table, at),
    };
    if done {
        // the remembered cell may have gone with the row
        app.board.cell = None;
        app.run_tx(tx);
    } else {
        tx.cancel(&mut app.doc);
    }
}

// ------------------------------------------------------------------- painting

/// Handles of the hovered or selected table, the divider under the pointer and the active cell.
pub fn overlay(app: &App, p: &Painter, rect: ERect) {
    if let Some((cell, ..)) = active_cell(app) {
        if let Some(n) = app.doc.get(cell) {
            let w = app.doc.world_transform(cell);
            let corners = [v2(0.0, 0.0), v2(n.size.x, 0.0), v2(n.size.x, n.size.y), v2(0.0, n.size.y)].map(|c| app.view.to_screen(rect.min, w.apply(c)));
            p.add(egui::Shape::closed_line(corners.to_vec(), Stroke::new(2.0, theme::BG_BRAND)));
        }
    }
    let Some(id) = handle_table(app).filter(|_| matches!(app.canvas.inter, Interaction::None | Interaction::Table(_))) else { return };
    if app.tool != Tool::Move {
        return;
    }
    let ptr = app.canvas.pointer_page.map(|q| app.view.to_screen(rect.min, q));
    if let (Some(ptr), Some((axis, index))) = (ptr, ptr.and_then(|q| divider_at(app, rect.min, id, q))) {
        let Some(t) = props(app, id) else { return };
        let s = t.size();
        let (a, b) = match axis {
            Axis::Col => (v2(t.col_x(index + 1), 0.0), v2(t.col_x(index + 1), s.y)),
            Axis::Row => (v2(0.0, t.row_y(index + 1)), v2(s.x, t.row_y(index + 1))),
        };
        let _ = ptr;
        p.line_segment([screen(app, rect.min, id, a), screen(app, rect.min, id, b)], Stroke::new(2.0, theme::BG_BRAND));
    }
    for (_, _, at) in plus_handles(app, rect.min, id) {
        let hot = ptr.is_some_and(|q| (q - at).length() <= HIT);
        if hot {
            p.circle_filled(at, PLUS_RADIUS, Color32::WHITE);
            p.circle_stroke(at, PLUS_RADIUS, Stroke::new(1.5, theme::BG_BRAND));
            let k = 4.0;
            p.line_segment([at - egui::vec2(k, 0.0), at + egui::vec2(k, 0.0)], Stroke::new(1.5, theme::BG_BRAND));
            p.line_segment([at - egui::vec2(0.0, k), at + egui::vec2(0.0, k)], Stroke::new(1.5, theme::BG_BRAND));
        } else {
            p.circle_filled(at, 2.5, theme::BG_BRAND.gamma_multiply(0.55));
        }
    }
}
