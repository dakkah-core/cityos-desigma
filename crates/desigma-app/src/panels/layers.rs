//! Layers tree: reflects the page's node tree; select, rename, toggle,
//! expand/collapse and drag to reorder/reparent.

use super::{clipped, scrollbar, wheel_scroll};
use crate::app::App;
use crate::icons::{self, Icon};
use crate::theme;
use crate::widgets::{self, field};
use desigma_doc::{Document, NodeId, NodeKind};
use egui::{Align2, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropKind {
    Before,
    After,
    Inside,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Drop {
    pub row: usize,
    pub kind: DropKind,
}

#[derive(Default)]
pub struct LayersState {
    pub expanded: HashSet<NodeId>,
    pub scroll: f32,
    pub page_scroll: f32,
    pub renaming: Option<NodeId>,
    pub renaming_page: Option<NodeId>,
    /// Right-clicked page and where its context menu opens.
    pub page_menu: Option<(NodeId, Pos2)>,
    pub page_drag: Option<NodeId>,
    /// Insertion slot (0..=pages) while dragging a page.
    pub page_drop: Option<usize>,
    pub rename_started: bool,
    pub rename_seen: bool,
    pub drag: Option<Vec<NodeId>>,
    pub drop: Option<Drop>,
    pub last_sel: Vec<NodeId>,
    pub anchor: Option<NodeId>,
    /// Name filter (the header's search icon): shown, its text, and a focus request.
    pub filter_open: bool,
    pub filter: String,
    pub filter_focus: bool,
}

impl LayersState {
    pub fn reset(&mut self) {
        *self = LayersState::default();
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Row {
    pub id: NodeId,
    pub depth: usize,
    pub has_children: bool,
}

/// Visible rows, top-most layer first (reverse z-order), honoring expansion.
pub fn flatten(doc: &Document, page: NodeId, expanded: &HashSet<NodeId>) -> Vec<Row> {
    fn rec(doc: &Document, id: NodeId, depth: usize, expanded: &HashSet<NodeId>, out: &mut Vec<Row>) {
        let Some(n) = doc.get(id) else { return };
        // the cells of a table are edited on the board, not listed as layers
        if matches!(n.kind, NodeKind::Table(_)) {
            return;
        }
        for c in n.children.iter().rev() {
            let Some(cn) = doc.get(*c) else { continue };
            let has = !cn.children.is_empty() && !matches!(cn.kind, NodeKind::Table(_));
            out.push(Row { id: *c, depth, has_children: has });
            if has && expanded.contains(c) {
                rec(doc, *c, depth + 1, expanded, out);
            }
        }
    }
    let mut out = Vec::new();
    rec(doc, page, 0, expanded, &mut out);
    out
}

/// Computes the drop target for a pointer at `y` (relative to the first row).
pub fn drop_for(y: f32, rows: &[Row], doc: &Document, dragged: &[NodeId]) -> Option<Drop> {
    if rows.is_empty() {
        return None;
    }
    let idx = (y / theme::LAYER_ROW_HEIGHT).floor();
    if idx < 0.0 {
        return Some(Drop { row: 0, kind: DropKind::Before });
    }
    let idx = idx as usize;
    if idx >= rows.len() {
        return Some(Drop { row: rows.len() - 1, kind: DropKind::After });
    }
    let row = rows[idx];
    if dragged.iter().any(|d| *d == row.id || doc.is_ancestor(*d, row.id)) {
        return None;
    }
    let rel = (y - idx as f32 * theme::LAYER_ROW_HEIGHT) / theme::LAYER_ROW_HEIGHT;
    let container = doc.get(row.id).map(|n| n.kind.is_container() && !matches!(n.kind, NodeKind::Instance { .. })).unwrap_or(false);
    let kind = if container {
        if rel < 0.25 {
            DropKind::Before
        } else if rel > 0.75 {
            DropKind::After
        } else {
            DropKind::Inside
        }
    } else if rel < 0.5 {
        DropKind::Before
    } else {
        DropKind::After
    };
    Some(Drop { row: idx, kind })
}

/// Resolves a drop to (parent, index) for `App::move_to_parent` (index counted
/// without the dragged nodes).
pub fn resolve_drop(doc: &Document, rows: &[Row], drop: Drop, dragged: &[NodeId]) -> Option<(NodeId, usize)> {
    let row = rows.get(drop.row)?;
    let (parent, mut index) = match drop.kind {
        DropKind::Inside => (row.id, doc.get(row.id)?.children.len()),
        DropKind::Before | DropKind::After => {
            let parent = doc.get(row.id)?.parent?;
            let pos = doc.get(parent)?.children.iter().position(|c| *c == row.id)?;
            // Layers are listed top-first, so "before" (visually above) means a higher z index.
            (parent, if drop.kind == DropKind::Before { pos + 1 } else { pos })
        }
    };
    let siblings = &doc.get(parent)?.children;
    let removed_before = dragged.iter().filter(|d| siblings.iter().position(|c| c == *d).map(|p| p < index).unwrap_or(false)).count();
    index = index.saturating_sub(removed_before);
    Some((parent, index))
}

pub(crate) fn kind_icon(k: &NodeKind) -> Icon {
    match k {
        NodeKind::Page { .. } => Icon::Page,
        NodeKind::Frame(_) => Icon::Frame,
        NodeKind::Group => Icon::Group,
        NodeKind::Section => Icon::Section,
        NodeKind::Rectangle => Icon::Rectangle,
        NodeKind::Ellipse(_) => Icon::Ellipse,
        NodeKind::Line => Icon::Line,
        NodeKind::Polygon { .. } => Icon::Polygon,
        NodeKind::Star { .. } => Icon::Star,
        NodeKind::Vector { .. } => Icon::Vector,
        NodeKind::Text(_) => Icon::Text,
        NodeKind::BooleanOp(_) => Icon::Boolean,
        NodeKind::Component(_) | NodeKind::ComponentSet(_) => Icon::Component,
        NodeKind::Instance { .. } => Icon::Instance,
        NodeKind::Sticky(_) => Icon::Sticky,
        NodeKind::Connector(_) => Icon::Connector,
        NodeKind::ShapeWithText(_) => Icon::Shapes,
        NodeKind::Stamp { .. } => Icon::Stamp,
        NodeKind::BoardSection(_) => Icon::Section,
        NodeKind::Table(_) => Icon::Table,
        NodeKind::TableCell(_) => Icon::Rectangle,
        NodeKind::CodeBlock(_) => Icon::Code,
        NodeKind::Unknown { .. } => Icon::Rectangle,
    }
}

fn is_component_like(n: &desigma_doc::Node) -> bool {
    // Components, instances and every layer inside an instance read as purple.
    matches!(n.kind, NodeKind::Component(_) | NodeKind::ComponentSet(_) | NodeKind::Instance { .. }) || n.component.source.is_some()
}

/// The rows to show: the whole tree, or only what matches the name filter.
pub(crate) fn visible_rows(app: &App) -> Vec<Row> {
    match crate::find::filter::matcher(&app.layers.filter).filter(|_| app.layers.filter_open) {
        Some(m) => crate::find::filter::rows(&app.doc, app.page, &m),
        None => flatten(&app.doc, app.page, &app.layers.expanded),
    }
}

pub fn show(app: &mut App, ui: &mut Ui, region: Rect) {
    // Reveal newly selected layers.
    if app.selection != app.layers.last_sel {
        let sel = app.selection.clone();
        for id in &sel {
            let mut cur = app.doc.get(*id).and_then(|n| n.parent);
            while let Some(p) = cur {
                app.layers.expanded.insert(p);
                cur = app.doc.get(p).and_then(|n| n.parent);
            }
        }
        let rows = visible_rows(app);
        if let Some(i) = sel.first().and_then(|s| rows.iter().position(|r| r.id == *s)) {
            let top = i as f32 * theme::LAYER_ROW_HEIGHT;
            if top < app.layers.scroll {
                app.layers.scroll = top;
            } else if top + theme::LAYER_ROW_HEIGHT > app.layers.scroll + region.height() {
                app.layers.scroll = top + theme::LAYER_ROW_HEIGHT - region.height();
            }
        }
        app.layers.last_sel = sel;
    }
    let rows = visible_rows(app);
    let filter = crate::find::filter::matcher(&app.layers.filter).filter(|_| app.layers.filter_open);
    let content_h = rows.len() as f32 * theme::LAYER_ROW_HEIGHT + 8.0;
    let mut scroll = app.layers.scroll;
    wheel_scroll(ui, region, &mut scroll, content_h);
    app.layers.scroll = scroll;
    app.hover_layer = None;

    let first = (scroll / theme::LAYER_ROW_HEIGHT).floor().max(0.0) as usize;
    let last = (((scroll + region.height()) / theme::LAYER_ROW_HEIGHT).ceil() as usize + 1).min(rows.len());
    let selected: HashSet<NodeId> = app.selection.iter().copied().collect();

    // Drag bookkeeping.
    let pointer = ui.input(|i| i.pointer.interact_pos());
    if let (Some(ids), Some(pos)) = (app.layers.drag.clone(), pointer) {
        app.layers.drop = if region.contains(pos) { drop_for(pos.y - region.top() + scroll, &rows, &app.doc, &ids) } else { None };
    }

    clipped(ui, region, |ui| {
        let p = ui.painter().clone();
        for (i, row) in rows.iter().enumerate().take(last).skip(first) {
            let Some(node) = app.doc.get(row.id).cloned() else { continue };
            let rr = Rect::from_min_size(
                Pos2::new(region.left(), region.top() + i as f32 * theme::LAYER_ROW_HEIGHT - scroll),
                Vec2::new(region.width(), theme::LAYER_ROW_HEIGHT),
            );
            let is_sel = selected.contains(&row.id);
            let under_sel = !is_sel && {
                let mut c = node.parent;
                let mut f = false;
                while let Some(pp) = c {
                    if selected.contains(&pp) {
                        f = true;
                        break;
                    }
                    c = app.doc.get(pp).and_then(|n| n.parent);
                }
                f
            };
            let resp = ui.interact(rr, ui.id().with(("layer", row.id.0)), Sense::click_and_drag());
            let row_hot = ui.rect_contains_pointer(rr);
            if resp.hovered() {
                app.hover_layer = Some(row.id);
            }
            if is_sel {
                p.rect_filled(rr, 0.0, if resp.hovered() { theme::BG_SELECTED_HOVER } else { theme::BG_SELECTED });
            } else if under_sel {
                p.rect_filled(rr, 0.0, theme::BG_SELECTED_SECONDARY);
            } else if resp.hovered() && app.layers.drag.is_none() {
                p.rect_stroke(rr.shrink2(Vec2::new(8.0, 0.0)), theme::cr(theme::RADIUS_MEDIUM), Stroke::new(1.0, theme::BORDER), StrokeKind::Inside);
            }

            let indent = row.depth as f32 * theme::LAYER_INDENT;
            let chev = Rect::from_min_size(Pos2::new(rr.left() + theme::LAYER_CHEVRON_PAD + indent, rr.top() + 4.0), Vec2::splat(theme::LAYER_CHEVRON_BOX));
            if row.has_children {
                let open = app.layers.expanded.contains(&row.id);
                let cr = ui.interact(chev, ui.id().with(("chev", row.id.0)), Sense::click());
                let col = if is_sel {
                    theme::ICON_SELECTED
                } else if cr.hovered() {
                    theme::ICON
                } else {
                    theme::ICON_TERTIARY
                };
                icons::paint(&p, if open { Icon::ChevronDown } else { Icon::ChevronRight }, chev.center(), 12.0, col);
                if cr.clicked() {
                    if open {
                        app.layers.expanded.remove(&row.id);
                    } else {
                        app.layers.expanded.insert(row.id);
                    }
                }
            }
            let comp = is_component_like(&node);
            let icon_c = Pos2::new(chev.right() + theme::LAYER_ICON_GAP + theme::LAYER_ICON_SIZE / 2.0, rr.center().y);
            let icon_col = if comp {
                theme::TEXT_COMPONENT
            } else if is_sel {
                theme::ICON_SELECTED
            } else {
                theme::ICON_TERTIARY
            };
            icons::paint(&p, kind_icon(&node.kind), icon_c, theme::LAYER_ICON_SIZE, icon_col);

            let text_left = icon_c.x + theme::LAYER_ICON_SIZE / 2.0 + 6.0;
            let toggles_w = theme::LAYER_TOGGLE_SIZE * 2.0 + 8.0;
            let text_right = rr.right() - toggles_w;
            let tcol = if !node.visible {
                theme::TEXT_TERTIARY
            } else if comp {
                theme::TEXT_COMPONENT
            } else {
                theme::TEXT
            };
            if app.layers.renaming == Some(row.id) {
                let id = ui.id().with(("layer-rename", row.id.0));
                let tr = Rect::from_min_max(Pos2::new(text_left - 4.0, rr.top() + 4.0), Pos2::new(rr.right() - 8.0, rr.bottom() - 4.0));
                p.rect_filled(tr, theme::cr(theme::RADIUS_SMALL), theme::BG);
                p.rect_stroke(tr, theme::cr(theme::RADIUS_SMALL), Stroke::new(1.0, theme::BORDER_SELECTED), StrokeKind::Inside);
                if !app.layers.rename_started {
                    ui.memory_mut(|m| m.request_focus(id));
                    app.layers.rename_started = true;
                    app.layers.rename_seen = false;
                }
                let res = field::edit_text(ui, tr, id, &node.name.clone(), theme::font_body(), theme::TEXT, 4.0);
                let focused = ui.memory(|m| m.has_focus(id));
                if focused {
                    app.layers.rename_seen = true;
                }
                if let Some(s) = res {
                    app.rename(row.id, &s);
                }
                if !focused && app.layers.rename_seen {
                    app.layers.renaming = None;
                    app.layers.rename_started = false;
                    app.layers.rename_seen = false;
                }
            } else {
                let shown = widgets::elide(&p, &node.name, &theme::font_body(), (text_right - text_left).max(10.0));
                if let Some(m) = &filter {
                    let ranges = crate::find::filter::highlights(m, &shown);
                    crate::find::filter::paint_highlights(&p, &shown, &ranges, text_left, rr, &theme::font_body(), text_right);
                }
                p.text(Pos2::new(text_left, rr.center().y), Align2::LEFT_CENTER, shown, theme::font_body(), tcol);
            }

            // Visibility / lock toggles (visible on hover or when active).
            let eye = Rect::from_min_size(Pos2::new(rr.right() - 8.0 - theme::LAYER_TOGGLE_SIZE, rr.top() + 4.0), Vec2::splat(theme::LAYER_TOGGLE_SIZE));
            let lock = eye.translate(Vec2::new(-theme::LAYER_TOGGLE_SIZE, 0.0));
            if row_hot || !node.visible {
                let er = ui.interact(eye, ui.id().with(("eye", row.id.0)), Sense::click());
                icons::paint(
                    &p,
                    if node.visible { Icon::Eye } else { Icon::EyeOff },
                    eye.center(),
                    14.0,
                    if er.hovered() { theme::ICON } else { theme::ICON_SECONDARY },
                );
                if er.clicked() {
                    app.toggle_visible(row.id);
                }
            }
            if row_hot || node.locked {
                let lr = ui.interact(lock, ui.id().with(("lock", row.id.0)), Sense::click());
                icons::paint(
                    &p,
                    if node.locked { Icon::Lock } else { Icon::Unlock },
                    lock.center(),
                    14.0,
                    if lr.hovered() { theme::ICON } else { theme::ICON_SECONDARY },
                );
                if lr.clicked() {
                    app.toggle_lock(row.id);
                }
            }

            if resp.secondary_clicked() {
                if !app.is_selected(row.id) {
                    app.select_only(row.id);
                }
                app.ui.ctx_menu = ui.input(|i| i.pointer.interact_pos());
            }

            // Selection clicks.
            let mods = ui.input(|i| i.modifiers);
            if resp.double_clicked() {
                app.layers.renaming = Some(row.id);
                app.layers.rename_started = false;
            } else if resp.clicked() && app.layers.renaming != Some(row.id) {
                if mods.shift {
                    let anchor = app.layers.anchor.and_then(|a| rows.iter().position(|r| r.id == a)).unwrap_or(i);
                    let (a, b) = (anchor.min(i), anchor.max(i));
                    app.selection = rows[a..=b].iter().map(|r| r.id).collect();
                } else if mods.command {
                    if let Some(pos) = app.selection.iter().position(|s| *s == row.id) {
                        app.selection.remove(pos);
                    } else {
                        app.selection.push(row.id);
                    }
                    app.layers.anchor = Some(row.id);
                } else {
                    app.select_only(row.id);
                    app.layers.anchor = Some(row.id);
                }
            }
            if resp.drag_started() && app.layers.renaming.is_none() && filter.is_none() {
                if !app.is_selected(row.id) {
                    app.select_only(row.id);
                }
                app.layers.drag = Some(app.top_selection());
            }
        }

        // Drop indicator.
        if let (Some(d), Some(_)) = (app.layers.drop, app.layers.drag.as_ref()) {
            if let Some(row) = rows.get(d.row) {
                let top = region.top() + d.row as f32 * theme::LAYER_ROW_HEIGHT - scroll;
                let indent = row.depth as f32 * theme::LAYER_INDENT + theme::LAYER_CHEVRON_PAD;
                match d.kind {
                    DropKind::Inside => {
                        let r = Rect::from_min_size(Pos2::new(region.left() + 4.0, top), Vec2::new(region.width() - 8.0, theme::LAYER_ROW_HEIGHT));
                        p.rect_stroke(r, theme::cr(theme::RADIUS_SMALL), Stroke::new(theme::LAYER_DROP_THICKNESS, theme::DROP_INDICATOR), StrokeKind::Inside);
                    }
                    k => {
                        let y = if k == DropKind::Before { top } else { top + theme::LAYER_ROW_HEIGHT };
                        let r = Rect::from_min_max(Pos2::new(region.left() + indent, y - 1.0), Pos2::new(region.right() - 8.0, y + 1.0));
                        p.rect_filled(r, 0.0, theme::DROP_INDICATOR);
                    }
                }
            }
        }
    });
    scrollbar(ui, region, scroll, content_h);

    // Finish drag.
    if app.layers.drag.is_some() && ui.input(|i| i.pointer.primary_released()) {
        let ids = app.layers.drag.take().unwrap_or_default();
        if let Some(d) = app.layers.drop.take() {
            if let Some((parent, index)) = resolve_drop(&app.doc, &rows, d, &ids) {
                app.move_to_parent(&ids, parent, index);
                app.layers.expanded.insert(parent);
            }
        }
        app.layers.drop = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use desigma_doc::{Node, Rect as DRect};

    fn sample() -> (Document, NodeId, NodeId, NodeId, NodeId) {
        let mut d = Document::new("t");
        let page = d.add_page("p");
        let f = d.alloc_id();
        let frame = d.insert(page, Node::frame(f, DRect::new(0.0, 0.0, 100.0, 100.0))).unwrap();
        let a = d.alloc_id();
        let a = d.insert(frame, Node::rectangle(a, DRect::new(0.0, 0.0, 10.0, 10.0))).unwrap();
        let b = d.alloc_id();
        let b = d.insert(page, Node::rectangle(b, DRect::new(0.0, 0.0, 10.0, 10.0))).unwrap();
        (d, page, frame, a, b)
    }

    #[test]
    fn flatten_lists_top_first_and_honors_expansion() {
        let (d, page, frame, a, b) = sample();
        let mut ex = HashSet::new();
        let rows = flatten(&d, page, &ex);
        assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), vec![b, frame]);
        ex.insert(frame);
        let rows = flatten(&d, page, &ex);
        assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), vec![b, frame, a]);
        assert_eq!(rows[2].depth, 1);
    }

    #[test]
    fn drop_zones() {
        let (d, page, frame, _a, b) = sample();
        let rows = flatten(&d, page, &HashSet::new());
        // row 0 = b (leaf): top half before, bottom half after
        assert_eq!(drop_for(5.0, &rows, &d, &[]), Some(Drop { row: 0, kind: DropKind::Before }));
        assert_eq!(drop_for(20.0, &rows, &d, &[]), Some(Drop { row: 0, kind: DropKind::After }));
        // row 1 = frame: middle is "inside"
        assert_eq!(drop_for(48.0, &rows, &d, &[]), Some(Drop { row: 1, kind: DropKind::Inside }));
        // can't drop onto itself
        assert_eq!(drop_for(48.0, &rows, &d, &[frame]), None);
        assert_eq!(drop_for(1000.0, &rows, &d, &[b]), Some(Drop { row: 1, kind: DropKind::After }));
    }

    #[test]
    fn resolve_inside_and_ordering() {
        let (d, page, frame, a, b) = sample();
        let rows = flatten(&d, page, &HashSet::new());
        let (p, i) = resolve_drop(&d, &rows, Drop { row: 1, kind: DropKind::Inside }, &[b]).unwrap();
        assert_eq!((p, i), (frame, 1));
        // dropping "before" the top row b puts the node above it in z-order
        let (p, i) = resolve_drop(&d, &rows, Drop { row: 0, kind: DropKind::Before }, &[a]).unwrap();
        assert_eq!(p, page);
        assert_eq!(i, 2);
        // moving b above the frame: dragged sibling before index is discounted
        let (p, i) = resolve_drop(&d, &rows, Drop { row: 1, kind: DropKind::Before }, &[b]).unwrap();
        assert_eq!((p, i), (page, 1));
    }
}
