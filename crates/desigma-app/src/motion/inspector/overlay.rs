//! Canvas side of Motion mode: the dotted motion path of a layer whose position is keyframed,
//! the transform gizmo at its current position, and dragging a layer on the canvas, which
//! keys Position at the playhead.

use super::*;
use crate::canvas::MoveState;
use crate::util::parent_world;
use desigma_doc::Document;

/// Pink (Y) and green (X) axis colours of the gizmo.
const AXIS_Y: Color32 = theme::GUIDE_COLOR;
const AXIS_X: Color32 = Color32::from_rgb(0x3D, 0xD6, 0x8C);
const RING_R: f32 = 18.0;

/// Page-space centre of `id` in `doc`, which must be the evaluated (drawn) document: the node's
/// animated size, rotation, scale and its parents' animation are all part of the result.
pub fn node_centre(doc: &Document, id: NodeId) -> Option<DVec2> {
    let n = doc.get(id)?;
    Some(doc.world_transform(id).apply(DVec2::new(n.size.x * 0.5, n.size.y * 0.5)))
}

/// Screen position of the transform gizmo of `id`: the centre of the layer as drawn at the
/// playhead (keyframes and clips included).
pub fn gizmo_centre(app: &App, rect: Rect, id: NodeId) -> Option<Pos2> {
    Some(app.view.to_screen(rect.min, node_centre(app.shown_doc(), id)?))
}

/// Screen position of the centre of `id` when its layout position is `pos`: the same space as
/// [`gizmo_centre`] (the drawn document's parent chain and size).
fn centre_at(app: &App, rect: Rect, id: NodeId, pos: DVec2) -> Option<Pos2> {
    let doc = app.shown_doc();
    let n = doc.get(id)?;
    let c = DVec2::new(pos.x + n.size.x * 0.5, pos.y + n.size.y * 0.5);
    Some(app.view.to_screen(rect.min, parent_world(doc, id).apply(c)))
}

/// Screen position of the centre of `id` along its motion path at time `t_ms` (the keyframed
/// Position, no clips), in the same space as [`gizmo_centre`].
pub fn path_point(app: &App, rect: Rect, id: NodeId, t_ms: f32) -> Option<Pos2> {
    let v = app.doc.get(id)?.motion.track(AnimProperty::Position)?.value_at(t_ms)?.vec2()?;
    centre_at(app, rect, id, v)
}

fn arrow(p: &Painter, from: Pos2, to: Pos2, col: Color32) {
    p.line_segment([from, to], Stroke::new(1.5, col));
    let d = (to - from).normalized();
    let n = Vec2::new(-d.y, d.x);
    let head = [to, to - d * 6.0 + n * 3.5, to - d * 6.0 - n * 3.5];
    p.add(Shape::convex_polygon(head.to_vec(), col, Stroke::NONE));
}

/// Paints the motion path and gizmo of every selected layer with a keyframed position.
pub fn paint_overlay(app: &App, ui: &Ui, rect: Rect) {
    if !app.motion.enabled {
        return;
    }
    let p = ui.painter().with_clip_rect(rect);
    let _playing = app.motion.playing;
    let frame = app.doc.timeline(app.page).frame_ms().max(1.0);
    let now = playhead(app);
    for id in &app.selection {
        let Some(n) = app.doc.get(*id) else { continue };
        let Some(track) = n.motion.track(AnimProperty::Position).filter(|t| !t.is_empty()) else { continue };
        let (Some(t0), Some(t1)) = (track.start_ms(), track.end_ms()) else { continue };
        let at = |t: f32| path_point(app, rect, *id, t);
        // The path: a thin line with a dot per frame (thinned while zoomed out).
        let mut pts = Vec::new();
        let mut t = t0 as f32;
        while t < t1 as f32 {
            pts.extend(at(t));
            t += frame;
        }
        pts.extend(at(t1 as f32));
        if pts.len() >= 2 {
            p.add(Shape::line(pts.clone(), Stroke::new(1.0, theme::CANVAS_SELECTION)));
            let mut last: Option<Pos2> = None;
            for q in &pts {
                if last.is_none_or(|l| l.distance(*q) >= 6.0) {
                    p.circle_filled(*q, 1.6, theme::CANVAS_SELECTION);
                    last = Some(*q);
                }
            }
        }
        // Keyframes: diamonds, filled at the playhead.
        for k in &track.keys {
            let Some(c) = at(k.time_ms as f32) else { continue };
            let on = k.time_ms == edit::to_ms(now);
            let fill = if on { theme::CANVAS_SELECTION } else { theme::CANVAS_HANDLE_FILL };
            p.add(Shape::convex_polygon(diamond_shape(c, 5.0), fill, Stroke::new(1.25, theme::CANVAS_SELECTION)));
        }
        // Gizmo on the layer's centre now (clips included).
        if let Some(c) = gizmo_centre(app, rect, *id).filter(|_| !app.motion_ins.gizmo_hidden) {
            p.circle_filled(c, RING_R, theme::CANVAS_SELECTION.gamma_multiply(0.12));
            p.circle_stroke(c, RING_R, Stroke::new(1.5, theme::CANVAS_SELECTION));
            arrow(&p, c + Vec2::new(0.0, -RING_R), c + Vec2::new(0.0, -RING_R - 22.0), AXIS_Y);
            arrow(&p, c + Vec2::new(-RING_R, 0.0), c + Vec2::new(-RING_R - 22.0, 0.0), AXIS_X);
            p.add(Shape::convex_polygon(diamond_shape(c, 4.5), theme::CANVAS_HANDLE_FILL, Stroke::new(1.25, theme::CANVAS_SELECTION)));
        }
    }
}

/// Finishes a canvas drag in Motion mode: the drag's static move is thrown away and the same
/// offset is applied through auto-key, so a layer with a keyframed position gets a keyframe at
/// the playhead and any other layer simply moves.
pub fn commit_move(app: &mut App, m: MoveState) {
    let news: Vec<(NodeId, DVec2)> = m.ids.iter().filter_map(|id| app.doc.get(*id).map(|n| (*id, motion::base_position(n)))).collect();
    m.tx.cancel(&mut app.doc);
    let t = playhead(app);
    let mut cmds = Vec::new();
    for (id, np) in news {
        let Some(n) = app.doc.get(id) else { continue };
        let op = motion::base_position(n);
        let (dx, dy) = (np.x - op.x, np.y - op.y);
        if dx.abs() < 1e-4 && dy.abs() < 1e-4 {
            continue;
        }
        let cur = motion::keyframed_value_at(n, AnimProperty::Position, t).and_then(|v| v.vec2()).unwrap_or(op);
        cmds.push(edit::auto_key(&app.doc, id, AnimProperty::Position, AnimValue::Vec2(DVec2::new(cur.x + dx, cur.y + dy)), t));
    }
    run_all(app, cmds);
    app.changed();
}
