//! Other people on the canvas: their selections and cursors, in their colour, as in Figma.

use super::presence::color_of;
use crate::app::App;
use crate::theme;
use desigma_doc::{NodeId, Vec2 as DV};
use egui::{Align2, Color32, Pos2, Rect, Shape, Stroke, Ui, Vec2};

/// The classic pointer arrow, tip at the origin (in points).
const ARROW: [(f32, f32); 7] = [(0.0, 0.0), (0.0, 16.0), (4.2, 12.4), (7.0, 18.6), (9.6, 17.4), (6.8, 11.2), (12.0, 11.2)];

pub fn overlays(app: &App, ui: &Ui, rect: Rect) {
    if app.collab.remotes.is_empty() {
        return;
    }
    let p = ui.painter().with_clip_rect(rect);
    // selections first, so a cursor is never hidden by an outline
    for (peer, r) in &app.collab.remotes {
        if r.p.page != app.page.0 {
            continue;
        }
        let col = color_of(*peer);
        for id in r.p.sel.iter().map(|i| NodeId(*i)) {
            if app.doc.get(id).is_some() && app.doc.page_of(id) == Some(app.page) {
                crate::canvas::draw::outline(app, &p, rect, id, col, theme::CANVAS_OUTLINE_WIDTH);
            }
        }
    }
    for (peer, r) in &app.collab.remotes {
        if r.p.page != app.page.0 {
            continue;
        }
        let Some(c) = r.shown else { continue };
        let at = app.view.to_screen(rect.min, DV::new(c[0], c[1]));
        if !rect.expand(40.0).contains(at) {
            continue;
        }
        cursor(&p, at, color_of(*peer), &r.p.name);
    }
}

/// Arrow plus a rounded name pill, the pointer tip at `at`.
fn cursor(p: &egui::Painter, at: Pos2, col: Color32, name: &str) {
    let pts: Vec<Pos2> = ARROW.iter().map(|(x, y)| at + Vec2::new(*x, *y)).collect();
    // the arrow is concave: fill it as triangles, then outline it
    let mut mesh = egui::Mesh::default();
    for q in &pts {
        mesh.colored_vertex(*q, col);
    }
    for t in [[0, 1, 2], [0, 2, 5], [0, 5, 6], [2, 3, 4], [2, 4, 5]] {
        mesh.add_triangle(t[0], t[1], t[2]);
    }
    p.add(Shape::mesh(mesh));
    p.add(Shape::closed_line(pts, Stroke::new(1.2, Color32::WHITE)));
    let font = theme::font_small_medium();
    let w = crate::widgets::text_width(p, name, &font) + 14.0;
    let pill = Rect::from_min_size(at + Vec2::new(11.0, 17.0), Vec2::new(w, 20.0));
    p.rect_filled(pill, theme::cr(10), col);
    p.text(pill.center(), Align2::CENTER_CENTER, name, font, Color32::WHITE);
}
