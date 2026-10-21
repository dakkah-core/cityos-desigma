//! The eraser: dragging it over strokes marks everything it touches, and letting go deletes the
//! marked strokes in one undo step. Only drawn strokes (marker, highlighter, washi tape) are erased.

use crate::app::App;
use crate::canvas::{EraseState, Interaction};
use crate::edit::Tx;
use crate::theme;
use crate::util::*;
use desigma_doc::{brush_hits, Command, NodeId, Vec2};

/// Eraser radius on the page at the current zoom (a screen size, so it stays usable zoomed out).
pub fn radius(app: &App) -> f32 {
    theme::BOARD_ERASER_RADIUS / app.view.zoom.max(1e-3)
}

fn mark(app: &App, e: &mut EraseState, at: Vec2) {
    // sample along the way so a fast swipe cannot skip a thin stroke
    let r = radius(app);
    let from = e.last;
    let steps = ((dist(from, at) / (r * 0.5)).ceil() as usize).max(1);
    for i in 1..=steps {
        let t = i as f32 / steps as f32;
        let p = v2(from.x + (at.x - from.x) * t, from.y + (at.y - from.y) * t);
        for id in brush_hits(&app.doc, app.page, p, r) {
            if !e.hits.contains(&id) {
                e.hits.push(id);
            }
        }
    }
    e.last = at;
}

/// Pointer down with the eraser at `page`.
pub fn press(app: &mut App, page: Vec2) {
    app.clear_selection();
    let mut e = EraseState { hits: Vec::new(), last: page };
    mark(app, &mut e, page);
    app.canvas.inter = Interaction::Erase(e);
    app.changed();
}

/// Pointer moved while erasing.
pub fn drag(app: &mut App, e: &mut EraseState, at: Vec2) {
    mark(app, e, at);
    app.changed();
}

/// Pointer released: delete what was touched.
pub fn release(app: &mut App, e: EraseState) {
    if e.hits.is_empty() {
        return;
    }
    let mut tx = Tx::new();
    for id in e.hits {
        let _ = tx.cmd(&mut app.doc, Command::Remove { id });
    }
    app.run_tx(tx);
    app.sanitize();
}

/// Ids the running erase would delete (for the canvas overlay).
pub fn pending(app: &App) -> &[NodeId] {
    match &app.canvas.inter {
        Interaction::Erase(e) => &e.hits,
        _ => &[],
    }
}

/// Paints the eraser disc at the pointer and a red wash over the strokes about to go.
pub fn overlay(app: &App, p: &egui::Painter, rect: egui::Rect) {
    let to_s = |v: Vec2| app.view.to_screen(rect.min, v);
    for id in pending(app) {
        let bb = world_aabb(&app.doc, *id);
        let r = egui::Rect::from_two_pos(to_s(v2(bb.x, bb.y)), to_s(v2(bb.x + bb.w, bb.y + bb.h))).expand(2.0);
        p.rect_filled(r, 2.0, egui::Color32::from_rgba_unmultiplied(0xF2, 0x48, 0x22, 0x26));
        p.rect_stroke(r, 2.0, egui::Stroke::new(1.0, egui::Color32::from_rgb(0xF2, 0x48, 0x22)), egui::StrokeKind::Outside);
    }
    if app.tool == crate::canvas::tools::Tool::Eraser {
        if let Some(at) = app.canvas.pointer_page {
            let c = to_s(at);
            p.circle_filled(c, theme::BOARD_ERASER_RADIUS, egui::Color32::from_white_alpha(70));
            p.circle_stroke(c, theme::BOARD_ERASER_RADIUS, egui::Stroke::new(1.5, theme::TEXT));
        }
    }
}
