//! The easing editor: preset dropdown and menu, Curve | Spring tabs, the dark graph with
//! draggable bezier handles (or the spring response) and the values field. The Easing panel
//! (a keyframe segment is selected) and the clip popover both use the pieces.

use super::*;
use crate::widgets::field::Prefix;

/// Height of the graph box.
pub const GRAPH_H: f32 = 144.0;
const PLOT: f32 = 80.0;

#[derive(Default)]
pub struct EasingState {
    /// Which editor's preset menu is open ("panel" or "clip").
    pub menu: Option<&'static str>,
    /// The menu shows the saved presets ("Libraries" tab).
    pub libraries: bool,
}

const CURVE_PRESETS: [Ease; 8] =
    [Ease::Hold, Ease::Linear, Ease::EaseIn, Ease::EaseOut, Ease::EaseInOut, Ease::EaseInBack, Ease::EaseOutBack, Ease::EaseInOutBack];
const SPRING_PRESETS: [Ease; 4] = [Ease::SpringGentle, Ease::SpringQuick, Ease::SpringBouncy, Ease::SpringSlow];

fn ease_close(a: Ease, b: Ease) -> bool {
    match (a.control_points(), b.control_points()) {
        (Some(x), Some(y)) => a.family() == b.family() && x.iter().zip(y.iter()).all(|(p, q)| (p - q).abs() < 1e-3),
        _ => a == b,
    }
}

/// Applies a new easing to the selected keyframe segments.
pub(super) fn apply_keys(app: &mut App, keys: &[KeyRef], e: Ease) {
    let cmd = edit::set_segment_easing(&app.doc, keys, e);
    run_all(app, vec![cmd]);
}

// ---------------------------------------------------------------- Easing panel

/// The Easing panel: replaces the inspector body while a keyframe segment is selected.
pub(super) fn panel(app: &mut App, ui: &mut Ui, region: Rect) {
    let keys = app.motion.selected_keys.clone();
    let ease = keys
        .iter()
        .find_map(|k| app.doc.get(k.node).and_then(|n| n.motion.track(k.property)).and_then(|t| t.key_at(k.time_ms)).map(|kf| kf.easing))
        .unwrap_or_default();
    let head = Rect::from_min_size(region.min, Vec2::new(region.width(), 40.0));
    let back = Rect::from_min_size(Pos2::new(region.left() + 8.0, region.top() + 8.0), Vec2::splat(24.0));
    widgets::record(ui, "mo-ease-back", back);
    if widgets::icon_button(ui, back, "mo-ease-back", Icon::ChevronLeft, false, "Back").clicked() {
        app.motion.easing_target = false;
        app.motion_ins.easing.menu = None;
    }
    ui.painter().text(head.center(), Align2::CENTER_CENTER, "Easing", theme::font_body_strong(), theme::TEXT);
    let x = region.left() - theme::BORDER_WIDTH + theme::SECTION_CONTENT_X;
    let dd = Rect::from_min_size(Pos2::new(x, region.top() + 48.0), Vec2::new(theme::SECTION_CONTENT_W, 24.0));
    let mut new = dropdown(app, ui, dd, "panel", ease);
    let (e2, _) = body(app, ui, Pos2::new(x, dd.bottom() + 8.0), theme::SECTION_CONTENT_W, "panel", ease);
    new = e2.or(new);
    if let Some(e) = new {
        apply_keys(app, &keys, e);
    }
}

// ---------------------------------------------------------------- dropdown + preset menu

/// Draws a small curve glyph of `ease` in `rect`.
pub(super) fn paint_glyph(p: &Painter, rect: Rect, ease: Ease, col: Color32) {
    let (lo, hi) = ease.value_range();
    let span = (hi - lo).max(1e-3);
    let pts: Vec<Pos2> =
        ease.sample(14).iter().map(|q| Pos2::new(rect.left() + q[0] * rect.width(), rect.bottom() - (q[1] - lo) / span * rect.height())).collect();
    p.add(Shape::line(pts, Stroke::new(1.25, col)));
}

fn glyph_rect(c: Pos2) -> Rect {
    Rect::from_center_size(c, Vec2::splat(12.0))
}

/// The preset dropdown button (shows the easing's name); opens the preset menu.
pub(super) fn dropdown(app: &mut App, ui: &mut Ui, rect: Rect, id: &'static str, ease: Ease) -> Option<Ease> {
    widgets::record(ui, &format!("mo-ease-dd:{id}"), rect);
    let open = app.motion_ins.easing.menu == Some(id);
    let resp = ui.interact(rect, ui.id().with(("mo-ease-dd", id)), Sense::click());
    {
        let p = ui.painter();
        p.rect_filled(rect, theme::cr(theme::RADIUS_MEDIUM), theme::BG_SECONDARY);
        if resp.hovered() || open {
            p.rect_stroke(
                rect,
                theme::cr(theme::RADIUS_MEDIUM),
                Stroke::new(1.0, if open { theme::BORDER_SELECTED } else { theme::BORDER }),
                StrokeKind::Inside,
            );
        }
        paint_glyph(p, glyph_rect(Pos2::new(rect.left() + 14.0, rect.center().y)), ease, theme::TEXT_SECONDARY);
        let font = theme::font_menu();
        let name = widgets::elide(p, ease.name(), &font, rect.width() - 52.0);
        p.text(Pos2::new(rect.left() + 28.0, rect.center().y), Align2::LEFT_CENTER, name, font, theme::TEXT);
        icons::paint(p, Icon::ChevronDown, Pos2::new(rect.right() - 12.0, rect.center().y), 12.0, theme::ICON_SECONDARY);
    }
    if resp.clicked() {
        let st = &mut app.motion_ins.easing;
        st.menu = if open { None } else { Some(id) };
        st.libraries = false;
    }
    if app.motion_ins.easing.menu == Some(id) {
        return menu(app, ui.ctx(), rect, id, ease);
    }
    None
}

fn menu(app: &mut App, ctx: &egui::Context, anchor: Rect, id: &'static str, ease: Ease) -> Option<Ease> {
    let libraries = app.motion_ins.easing.libraries;
    let spring = ease.is_spring();
    let mut rows: Vec<(String, Ease)> = if libraries {
        app.motion_ins.saved_eases.clone()
    } else if spring {
        let mut r: Vec<(String, Ease)> = SPRING_PRESETS.iter().map(|e| (e.name().to_string(), *e)).collect();
        let custom = if matches!(ease, Ease::CustomSpring { .. }) {
            ease
        } else if spring {
            ease.to_custom()
        } else {
            Ease::spring(motion::DEFAULT_BOUNCE)
        };
        r.push(("Custom spring".into(), custom));
        r
    } else {
        let mut r: Vec<(String, Ease)> = CURVE_PRESETS.iter().map(|e| (e.name().to_string(), *e)).collect();
        let custom = match ease {
            Ease::CustomBezier(_) => ease,
            Ease::Hold => Ease::bezier(0.0, 0.0, 1.0, 1.0),
            e => e.to_custom(),
        };
        r.push(("Custom bezier".into(), custom));
        r
    };
    rows.truncate(40);
    let w = 200.0;
    let list_h = (rows.len().max(1) as f32 * 24.0).min(300.0);
    let h = 40.0 + list_h + 8.0;
    let screen = ctx.content_rect();
    let x = (anchor.left() - 8.0 - w).max(screen.left() + 8.0);
    let y = anchor.top().min(screen.bottom() - h - 8.0).max(screen.top() + 8.0);
    let rect = Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, h));
    let mut picked: Option<Ease> = None;
    let mut new_libraries = libraries;
    let mut close = false;
    let mut save = false;
    let outside = float(ctx, Id::new(("mo-ease-menu", id)), rect, Some(anchor), |ui| {
        // Tabs: Presets | Libraries, then + and the close button.
        let mut tx = rect.left() + 8.0;
        for (i, label) in ["Presets", "Libraries"].iter().enumerate() {
            let tw = widgets::text_width(ui.painter(), label, &theme::font_body_strong()) + 16.0;
            let tr = Rect::from_min_size(Pos2::new(tx, rect.top() + 8.0), Vec2::new(tw, 24.0));
            widgets::record(ui, &format!("mo-ease-menu-tab:{label}"), tr);
            let resp = ui.interact(tr, ui.id().with(("mo-ease-menu-tab", i)), Sense::click());
            let sel = (i == 1) == libraries;
            if sel {
                ui.painter().rect_filled(tr, theme::cr(theme::RADIUS_MEDIUM), theme::BG_SECONDARY);
            } else if resp.hovered() {
                ui.painter().rect_filled(tr, theme::cr(theme::RADIUS_MEDIUM), theme::BG_HOVER);
            }
            let font = if sel { theme::font_body_strong() } else { theme::font_body() };
            ui.painter().text(tr.center(), Align2::CENTER_CENTER, label, font, if sel { theme::TEXT } else { theme::TEXT_SECONDARY });
            if resp.clicked() {
                new_libraries = i == 1;
            }
            tx += tw + 2.0;
        }
        let close_r = Rect::from_min_size(Pos2::new(rect.right() - 32.0, rect.top() + 8.0), Vec2::splat(24.0));
        let plus_r = Rect::from_min_size(Pos2::new(rect.right() - 60.0, rect.top() + 8.0), Vec2::splat(24.0));
        widgets::record(ui, "mo-ease-menu-save", plus_r);
        widgets::record(ui, "mo-ease-menu-close", close_r);
        if widgets::icon_button(ui, plus_r, "mo-ease-menu-save", Icon::Plus, false, "Save as preset").clicked() {
            save = true;
        }
        if widgets::icon_button(ui, close_r, "mo-ease-menu-close", Icon::Close, false, "").clicked() {
            close = true;
        }
        widgets::hline(ui.painter(), rect.left(), rect.right(), rect.top() + 39.0, theme::BORDER);
        let list = Rect::from_min_size(Pos2::new(rect.left() + 8.0, rect.top() + 40.0), Vec2::new(w - 16.0, list_h));
        if rows.is_empty() {
            ui.painter().text(
                Pos2::new(list.left() + 8.0, list.top() + 12.0),
                Align2::LEFT_CENTER,
                "No saved presets",
                theme::font_body(),
                theme::TEXT_TERTIARY,
            );
        }
        for (i, (name, e)) in rows.iter().enumerate() {
            let rr = Rect::from_min_size(Pos2::new(list.left(), list.top() + i as f32 * 24.0), Vec2::new(list.width(), 24.0));
            if rr.bottom() > list.bottom() + 0.5 {
                break;
            }
            widgets::record(ui, &format!("mo-ease-row:{name}"), rr);
            let resp = ui.interact(rr, ui.id().with(("mo-ease-row", i, libraries)), Sense::click());
            let selected = if libraries { ease_close(*e, ease) } else { e.name() == ease.name() };
            let p = ui.painter();
            if selected {
                p.rect_filled(rr, theme::cr(theme::RADIUS_MEDIUM), theme::BG_SELECTED);
            } else if resp.hovered() {
                p.rect_filled(rr, theme::cr(theme::RADIUS_MEDIUM), theme::BG_HOVER);
            }
            paint_glyph(p, glyph_rect(Pos2::new(rr.left() + 14.0, rr.center().y)), *e, theme::TEXT_SECONDARY);
            let shown = widgets::elide(p, name, &theme::font_body_strong(), rr.width() - 36.0);
            p.text(Pos2::new(rr.left() + 30.0, rr.center().y), Align2::LEFT_CENTER, shown, theme::font_body_strong(), theme::TEXT);
            if resp.clicked() {
                picked = Some(*e);
            }
        }
    });
    app.motion_ins.easing.libraries = new_libraries;
    if save {
        let n = app.motion_ins.saved_eases.len() + 1;
        app.motion_ins.saved_eases.push((format!("Custom {n}"), ease.to_custom()));
        app.motion_ins.easing.libraries = true;
    }
    if close || outside || picked.is_some() {
        app.motion_ins.easing.menu = None;
    }
    picked
}

// ---------------------------------------------------------------- tabs, graph, values

/// Curve | Spring tabs, the graph and the values field below `origin`. Returns the new easing
/// when the user changed it, and the height used.
pub(super) fn body(app: &mut App, ui: &mut Ui, origin: Pos2, w: f32, id: &'static str, ease: Ease) -> (Option<Ease>, f32) {
    let _ = app;
    let mut out = None;
    let mut y = origin.y;
    let spring = ease.is_spring();

    // Tabs.
    let tabs = Rect::from_min_size(Pos2::new(origin.x, y), Vec2::new(w, 24.0));
    ui.painter().rect_filled(tabs, theme::cr(theme::RADIUS_MEDIUM), theme::BG_SECONDARY);
    for (i, label) in ["Curve", "Spring"].iter().enumerate() {
        let tr = Rect::from_min_size(Pos2::new(tabs.left() + 2.0 + i as f32 * (w - 4.0) / 2.0, tabs.top() + 2.0), Vec2::new((w - 4.0) / 2.0, 20.0));
        widgets::record(ui, &format!("mo-ease-tab:{label}:{id}"), tr);
        let resp = ui.interact(tr, ui.id().with(("mo-ease-tab", id, i)), Sense::click());
        let sel = (i == 1) == spring;
        if sel {
            ui.painter().rect_filled(tr, theme::cr(theme::RADIUS_MEDIUM - 1), theme::BG_TERTIARY);
        } else if resp.hovered() {
            ui.painter().rect_filled(tr, theme::cr(theme::RADIUS_MEDIUM - 1), theme::BG_TERTIARY.gamma_multiply(0.5));
        }
        ui.painter().text(tr.center(), Align2::CENTER_CENTER, label, theme::font_body(), if sel { theme::TEXT } else { theme::TEXT_SECONDARY });
        if resp.clicked() && !sel {
            out = Some(default_ease(i == 1));
        }
    }
    y += 32.0;

    // Graph.
    let g = Rect::from_min_size(Pos2::new(origin.x, y), Vec2::new(w, GRAPH_H));
    if let Some(e) = graph(ui, g, id, ease) {
        out = Some(e);
    }
    y += GRAPH_H + 8.0;

    // Values.
    let vr = Rect::from_min_size(Pos2::new(origin.x, y), Vec2::new(w, 24.0));
    if spring {
        let bounce = ease.bounce().unwrap_or(motion::DEFAULT_BOUNCE);
        widgets::record(ui, &format!("mo-ease-bounce:{id}"), vr);
        if let Some(v) = field::num_field(ui, vr, ("mo-ease-bounce", id), Prefix::Icon(Icon::Spacing), Some((bounce * 100.0).round() / 100.0), "") {
            out = Some(Ease::spring(v));
        }
        y += 24.0;
    } else if let Some(c) = ease.control_points() {
        let fr = Rect::from_min_size(vr.min, Vec2::new(w - 28.0, 24.0));
        widgets::record(ui, &format!("mo-ease-values:{id}"), fr);
        let txt = c.iter().map(|v| fmt_num((v * 100.0).round() / 100.0)).collect::<Vec<_>>().join(", ");
        let fid = ui.id().with(("mo-ease-values", id));
        field::field_bg(ui, fr, fid);
        icons::paint(ui.painter(), Icon::Spacing, Pos2::new(fr.left() + 14.0, fr.center().y), 12.0, theme::TEXT_SECONDARY);
        let inner = Rect::from_min_max(Pos2::new(fr.left() + 24.0, fr.top()), fr.right_bottom());
        if let Some(s) = field::edit_text(ui, inner, fid, &txt, theme::font_body(), theme::TEXT, 0.0) {
            if let Some(e) = parse_values(&s) {
                out = Some(e);
            }
        }
        let rr = Rect::from_min_size(Pos2::new(vr.right() - 24.0, vr.top()), Vec2::splat(24.0));
        widgets::record(ui, &format!("mo-ease-reset:{id}"), rr);
        if widgets::icon_button(ui, rr, ("mo-ease-reset", id), Icon::Reset, false, "Reset").clicked() {
            out = Some(Ease::Linear);
        }
        y += 24.0;
    }
    (out, y - origin.y)
}

/// "x1, y1, x2, y2" to an easing: a known preset when it matches one, else a custom bezier.
pub(super) fn parse_values(s: &str) -> Option<Ease> {
    let nums: Vec<f32> = s.split([',', ' ', ';']).filter(|t| !t.trim().is_empty()).map(|t| t.trim().parse::<f32>()).collect::<Result<_, _>>().ok()?;
    if nums.len() != 4 || nums.iter().any(|v| !v.is_finite()) {
        return None;
    }
    let e = Ease::bezier(nums[0], nums[1], nums[2], nums[3]);
    let c = e.control_points()?;
    for (_, p) in Ease::PRESETS.iter() {
        if let Some(pc) = p.control_points() {
            if pc.iter().zip(c.iter()).all(|(a, b)| (a - b).abs() < 1e-4) {
                return Some(*p);
            }
        }
    }
    Some(e)
}

fn graph(ui: &mut Ui, rect: Rect, id: &'static str, ease: Ease) -> Option<Ease> {
    widgets::record(ui, &format!("mo-ease-graph:{id}"), rect);
    ui.painter().rect_filled(rect, theme::cr(theme::RADIUS_MEDIUM), theme::BG_SECONDARY);
    if ease.is_spring() {
        spring_graph(ui, rect, id, ease)
    } else {
        curve_graph(ui, rect, id, ease)
    }
}

fn handle_dot(p: &Painter, c: Pos2, active: bool) {
    p.circle_filled(c, 4.5, theme::BG_SECONDARY);
    p.circle_stroke(c, 4.0, Stroke::new(1.5, theme::CANVAS_SELECTION));
    if active {
        p.circle_filled(c, 2.0, theme::CANVAS_SELECTION);
    }
}

fn curve_graph(ui: &mut Ui, rect: Rect, id: &'static str, ease: Ease) -> Option<Ease> {
    let plot = Rect::from_min_size(Pos2::new(rect.center().x - PLOT / 2.0, rect.top() + (rect.height() - PLOT) / 2.0), Vec2::splat(PLOT));
    widgets::record(ui, &format!("mo-ease-plot:{id}"), plot);
    let to_s = |x: f32, y: f32| Pos2::new(plot.left() + x * PLOT, plot.bottom() - y * PLOT);
    let p = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    // Start and end value guides.
    for v in [0.0, 1.0] {
        p.line_segment([to_s(0.0, v), to_s(1.0, v)], Stroke::new(1.0, theme::TEXT_TERTIARY));
    }
    let c = ease.control_points().filter(|_| !matches!(ease, Ease::Hold));
    if let Some(c) = c {
        p.line_segment([to_s(0.0, 0.0), to_s(c[0], c[1])], Stroke::new(1.0, theme::CANVAS_SELECTION));
        p.line_segment([to_s(1.0, 1.0), to_s(c[2], c[3])], Stroke::new(1.0, theme::CANVAS_SELECTION));
    }
    let pts: Vec<Pos2> = ease.sample(64).iter().map(|q| to_s(q[0], q[1])).collect();
    p.add(Shape::line(pts, Stroke::new(1.5, theme::TEXT)));
    let mut out = None;
    if let Some(c) = c {
        for i in 0..2 {
            let pos = to_s(c[2 * i], c[2 * i + 1]);
            let hr = Rect::from_center_size(pos, Vec2::splat(16.0));
            widgets::record(ui, &format!("mo-ease-h{}:{id}", i + 1), hr);
            let resp = ui.interact(hr, ui.id().with(("mo-ease-h", id, i)), Sense::drag());
            if resp.hovered() || resp.dragged() {
                ui.ctx().set_cursor_icon(if resp.dragged() { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab });
            }
            if resp.drag_started() {
                // The first frame only arms the drag; edits follow once the app has snapshotted.
                mark_drag(ui);
            } else if resp.dragged() {
                mark_drag(ui);
                if let Some(pt) = resp.interact_pointer_pos() {
                    let x = (((pt.x - plot.left()) / PLOT).clamp(0.0, 1.0) * 100.0).round() / 100.0;
                    let y = (((plot.bottom() - pt.y) / PLOT).clamp(-0.4, 1.4) * 100.0).round() / 100.0;
                    let mut nc = c;
                    nc[2 * i] = x;
                    nc[2 * i + 1] = y;
                    out = Some(Ease::bezier(nc[0], nc[1], nc[2], nc[3]));
                }
            }
            handle_dot(&p, pos, resp.hovered() || resp.dragged());
        }
    }
    out
}

fn spring_graph(ui: &mut Ui, rect: Rect, id: &'static str, ease: Ease) -> Option<Ease> {
    let bounce = ease.bounce().unwrap_or(motion::DEFAULT_BOUNCE);
    let (vmin, vmax) = ease.value_range();
    let (lo, hi) = (vmin.min(0.0), vmax.max(1.0));
    let plot = Rect::from_min_max(Pos2::new(rect.left() + 20.0, rect.top() + 28.0), Pos2::new(rect.right() - 20.0, rect.bottom() - 28.0));
    widgets::record(ui, &format!("mo-ease-plot:{id}"), plot);
    let to_s = |t: f32, v: f32| Pos2::new(plot.left() + t * plot.width(), plot.bottom() - (v - lo) / (hi - lo).max(1e-3) * plot.height());
    let p = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    for v in [0.0, 1.0] {
        p.line_segment([to_s(0.0, v), to_s(1.0, v)], Stroke::new(1.0, theme::TEXT_TERTIARY));
    }
    let samples = ease.sample(96);
    p.add(Shape::line(samples.iter().map(|q| to_s(q[0], q[1])).collect(), Stroke::new(1.5, theme::TEXT)));
    // The handle sits on the overshoot peak (or the end of the curve when there is none).
    let (mut pt, mut pv) = (1.0f32, 1.0f32);
    for q in &samples {
        if q[1] > pv + 1e-4 {
            pt = q[0];
            pv = q[1];
        }
    }
    let pos = to_s(pt, pv);
    let hr = Rect::from_center_size(pos, Vec2::splat(16.0));
    widgets::record(ui, &format!("mo-ease-spring-handle:{id}"), hr);
    let hid = ui.id().with(("mo-ease-sh", id));
    let resp = ui.interact(hr, hid, Sense::drag());
    let mut out = None;
    if resp.hovered() || resp.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }
    if resp.drag_started() {
        mark_drag(ui);
        let y0 = resp.interact_pointer_pos().map_or(pos.y, |q| q.y);
        ui.ctx().data_mut(|d| d.insert_temp(hid, (bounce, y0)));
    } else if resp.dragged() {
        mark_drag(ui);
        if let (Some(q), Some((b0, y0))) = (resp.interact_pointer_pos(), ui.ctx().data(|d| d.get_temp::<(f32, f32)>(hid))) {
            let b = ((b0 + (y0 - q.y) / 80.0) * 100.0).round() / 100.0;
            out = Some(Ease::spring(b));
        }
    }
    handle_dot(&p, pos, resp.hovered() || resp.dragged());
    out
}
