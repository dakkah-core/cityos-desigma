//! Sections on a board: a freshly drawn section adopts what it covers, the contextual bar sets its
//! colour, renames it and hides or shows its contents, and the title pill is renamed in place.

use crate::app::App;
use crate::edit::Tx;
use crate::theme;
use crate::util::*;
use crate::widgets;
use desigma_doc::*;
use egui::{Id, Pos2, Rect as ERect, Stroke, StrokeKind, Ui};

/// Moves the nodes that lie entirely inside the new section `id` (siblings of it) into it, keeping
/// their place on the board. Connectors stay where they are: they follow their ends.
pub fn adopt_inside(app: &mut App, tx: &mut Tx, id: NodeId) {
    let Some(parent) = app.doc.get(id).and_then(|n| n.parent) else { return };
    let area = world_aabb(&app.doc, id);
    let Some(inv) = app.doc.world_transform(id).invert() else { return };
    let siblings: Vec<NodeId> = app.doc.get(parent).map(|p| p.children.clone()).unwrap_or_default();
    for s in siblings {
        if s == id {
            continue;
        }
        let Some(n) = app.doc.get(s) else { continue };
        if !n.visible || n.locked || matches!(n.kind, NodeKind::Connector(_) | NodeKind::BoardSection(_) | NodeKind::Page { .. }) {
            continue;
        }
        let bb = world_aabb(&app.doc, s);
        let inside = bb.x >= area.x && bb.y >= area.y && bb.x + bb.w <= area.x + area.w && bb.y + bb.h <= area.y + area.h;
        if !inside {
            continue;
        }
        let world = app.doc.world_transform(s);
        let index = app.doc.get(id).map(|p| p.children.len()).unwrap_or(0);
        if tx.cmd(&mut app.doc, Command::Reparent { id: s, parent: id, index }).is_ok() {
            if let Some(n) = tx.ed.node(&mut app.doc, s) {
                n.transform = inv * world;
            }
        }
    }
}

/// Sets the colour of the sections among `ids` (one undo step).
pub fn set_color(app: &mut App, ids: &[NodeId], color: SectionColor) {
    app.edit_nodes(ids, |n| {
        if let NodeKind::BoardSection(s) = &mut n.kind {
            s.color = color;
        }
    });
}

/// Hides or shows the contents of the sections among `ids` (one undo step).
pub fn set_contents_hidden(app: &mut App, ids: &[NodeId], hidden: bool) {
    app.edit_nodes(ids, |n| {
        if let NodeKind::BoardSection(s) = &mut n.kind {
            s.contents_hidden = hidden;
        }
    });
    // what was inside is no longer there to be selected
    if hidden {
        let doc = &app.doc;
        let hide: Vec<NodeId> = app.selection.iter().copied().filter(|s| ids.iter().any(|i| doc.is_ancestor(*i, *s))).collect();
        app.selection.retain(|s| !hide.contains(s));
    }
}

/// Opens the in-place rename of section `id`.
pub fn begin_rename(app: &mut App, id: NodeId) {
    if let Some(n) = app.doc.get(id) {
        app.board.rename = Some((id, n.name.clone()));
    }
}

/// The in-place title editor above the section's pill; Enter or a click elsewhere keeps the new
/// name (one undo step), Escape drops it.
pub fn rename_ui(app: &mut App, ui: &mut Ui, canvas: ERect) {
    let Some((id, mut text)) = app.board.rename.clone() else { return };
    let Some(n) = app.doc.get(id).filter(|n| matches!(n.kind, NodeKind::BoardSection(_))) else {
        app.board.rename = None;
        return;
    };
    let pill = desigma_render::board::section_title_rect(n);
    let cur_name = n.name.clone();
    let w = app.doc.world_transform(id);
    let a = app.view.to_screen(canvas.min, w.apply(v2(pill.x, pill.y)));
    let z = app.view.zoom;
    let size = egui::vec2(((pill.w.max(120.0)) * z).clamp(140.0, 360.0), (pill.h * z).clamp(26.0, 40.0));
    let r = ERect::from_min_size(Pos2::new(a.x, a.y), size);
    let eid = Id::new("board-section-rename");
    let mut commit = false;
    let mut cancel = false;
    let p = ui.painter().clone();
    p.rect_filled(r, theme::cr(theme::RADIUS_MEDIUM), theme::BG);
    p.rect_stroke(r, theme::cr(theme::RADIUS_MEDIUM), Stroke::new(2.0, theme::BG_BRAND), StrokeKind::Inside);
    let resp = ui.put(r.shrink2(egui::vec2(8.0, 2.0)), egui::TextEdit::singleline(&mut text).id(eid).frame(egui::Frame::NONE).font(theme::font_body_strong()));
    widgets::record(ui, "board-section-rename", r);
    if !resp.has_focus() && !resp.lost_focus() {
        resp.request_focus();
    }
    let (enter, esc, pressed_out) = ui.input(|i| {
        (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape), i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|q| !r.contains(q)))
    });
    if esc {
        cancel = true;
    } else if enter || pressed_out {
        commit = true;
    }
    if cancel {
        app.board.rename = None;
    } else if commit {
        app.board.rename = None;
        let name = text.trim().to_string();
        if !name.is_empty() && name != cur_name {
            app.edit_nodes(&[id], |n| n.name = name.clone());
        }
    } else {
        app.board.rename = Some((id, text));
    }
}
