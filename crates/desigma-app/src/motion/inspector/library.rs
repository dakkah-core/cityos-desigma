//! The animation library popover: search, group filter, the 41 presets grouped by Basic /
//! Enter / Cycle / Exit with speed tags, and a live preview of the hovered preset.

use super::*;
use desigma_doc::motion::{Preset, PresetGroup, PRESETS};
use desigma_doc::{Node, Rect as DRect};

pub const FILTERS: [&str; 5] = ["All", "Basic", "Enter", "Cycle", "Exit"];
const W: f32 = 232.0;
const ROW_H: f32 = 24.0;
const HEAD_H: f32 = 24.0;

pub struct LibraryState {
    pub query: String,
    /// Index into [`FILTERS`].
    pub filter: usize,
    pub scroll: f32,
    /// Preset under the pointer (drives the live preview).
    pub hover: Option<&'static str>,
    /// First frame: focus the search field.
    fresh: u8,
}

impl LibraryState {
    pub fn new() -> Self {
        Self { query: String::new(), filter: 0, scroll: 0.0, hover: None, fresh: 4 }
    }
}

/// Presets matching `query` (name or group, case-insensitive) in group `filter` (0 = all),
/// grouped in library order; empty groups are left out.
pub fn visible(query: &str, filter: usize) -> Vec<(PresetGroup, Vec<&'static Preset>)> {
    let q = query.trim().to_lowercase();
    PresetGroup::ALL
        .iter()
        .filter(|g| filter == 0 || PresetGroup::ALL.get(filter - 1) == Some(g))
        .map(|g| {
            let rows: Vec<&'static Preset> = PRESETS
                .iter()
                .filter(|p| p.group == *g)
                .filter(|p| q.is_empty() || p.name.to_lowercase().contains(&q) || g.name().to_lowercase().contains(&q))
                .collect();
            (*g, rows)
        })
        .filter(|(_, rows)| !rows.is_empty())
        .collect()
}

fn preset_icon(p: &Preset) -> Icon {
    p.clips().first().map_or(Icon::MoveArrows, |c| clip_icon(c.kind))
}

pub(super) fn popover(app: &mut App, ctx: &egui::Context, region: Rect) {
    let Some(mut st) = app.motion_ins.library.take() else { return };
    let sel = targets(app);
    let start = app.motion.playhead_ms;
    let screen = ctx.content_rect();
    let h = 392.0f32.min(screen.height() - 16.0);
    let anchor = app.motion_ins.library_anchor;
    let x = (region.left() - 8.0 - W).max(screen.left() + 8.0);
    let y = (anchor.map_or(region.top() + 24.0, |a| a.top() - 8.0)).min(screen.bottom() - h - 8.0).max(screen.top() + 8.0);
    let rect = Rect::from_min_size(Pos2::new(x, y), Vec2::new(W, h));
    let mut apply: Option<&'static Preset> = None;
    let mut close = false;
    let mut add = false;
    let mut hover: Option<&'static str> = None;
    let mut hover_row: Option<Rect> = None;
    let outside = float(ctx, Id::new("mo-library"), rect, anchor, |ui| {
        float_title(ui, rect, "Animations");
        let close_r = Rect::from_min_size(Pos2::new(rect.right() - 32.0, rect.top() + 8.0), Vec2::splat(24.0));
        let plus_r = Rect::from_min_size(Pos2::new(rect.right() - 60.0, rect.top() + 8.0), Vec2::splat(24.0));
        widgets::record(ui, "mo-lib-add", plus_r);
        widgets::record(ui, "mo-lib-close", close_r);
        if widgets::icon_button(ui, plus_r, "mo-lib-add", Icon::Plus, false, "Add animation").clicked() {
            add = true;
        }
        if widgets::icon_button(ui, close_r, "mo-lib-close", Icon::Close, false, "").clicked() {
            close = true;
        }
        // Search field (live: the text being typed filters at once).
        let sr = Rect::from_min_size(Pos2::new(rect.left() + 8.0, rect.top() + 44.0), Vec2::new(W - 16.0, 24.0));
        widgets::record(ui, "mo-lib-search", sr);
        let sid = ui.id().with(("txt", "mo-lib-search"));
        if st.fresh > 0 {
            st.fresh -= 1;
            ui.memory_mut(|m| m.request_focus(sid));
        }
        field::field_bg(ui, sr, sid);
        icons::paint(ui.painter(), Icon::Search, Pos2::new(sr.left() + 14.0, sr.center().y), 14.0, theme::ICON_SECONDARY);
        let inner = Rect::from_min_max(Pos2::new(sr.left() + 26.0, sr.top()), sr.right_bottom());
        let hint = Some(("Search", theme::TEXT_TERTIARY));
        if let Some(s) = field::edit_text_hint(ui, inner, sid, &st.query, theme::font_body(), theme::TEXT, 0.0, hint) {
            st.query = s;
        }
        let focused = ui.memory(|m| m.has_focus(sid));
        let live = if focused { ui.data(|d| d.get_temp::<String>(sid)) } else { None };
        let query = live.unwrap_or_else(|| st.query.clone());
        // Group filter.
        let fr = Rect::from_min_size(Pos2::new(rect.left() + 8.0, rect.top() + 76.0), Vec2::new(W - 16.0, 24.0));
        widgets::record(ui, "mo-lib-filter", fr);
        let opts: Vec<String> = FILTERS.iter().map(|s| s.to_string()).collect();
        if let Some(i) = crate::widgets::menu::dropdown(ui, fr, "mo-lib-filter", FILTERS[st.filter.min(4)], &opts) {
            st.filter = i;
            st.scroll = 0.0;
        }
        widgets::hline(ui.painter(), rect.left(), rect.right(), rect.top() + 108.0, theme::BORDER);
        // List.
        let list = Rect::from_min_max(Pos2::new(rect.left(), rect.top() + 109.0), Pos2::new(rect.right(), rect.bottom() - 8.0));
        let groups = visible(&query, st.filter);
        let total: f32 = groups.iter().map(|(_, rows)| HEAD_H + 8.0 + rows.len() as f32 * ROW_H).sum::<f32>() + 8.0;
        let max_scroll = (total - list.height()).max(0.0);
        if ui.rect_contains_pointer(list) {
            st.scroll -= ui.input(|i| i.smooth_scroll_delta.y);
        }
        st.scroll = st.scroll.clamp(0.0, max_scroll);
        let p = ui.painter().with_clip_rect(list);
        let mut yy = list.top() + 8.0 - st.scroll;
        if groups.is_empty() {
            p.text(Pos2::new(rect.left() + 16.0, list.top() + 20.0), Align2::LEFT_CENTER, "No animations found", theme::font_body(), theme::TEXT_TERTIARY);
        }
        for (g, rows) in &groups {
            p.text(Pos2::new(rect.left() + 16.0, yy + HEAD_H / 2.0), Align2::LEFT_CENTER, g.name(), theme::font_body_strong(), theme::TEXT);
            yy += HEAD_H + 8.0;
            for pr in rows {
                let rr = Rect::from_min_size(Pos2::new(rect.left() + 8.0, yy), Vec2::new(W - 16.0, ROW_H));
                yy += ROW_H;
                if rr.bottom() < list.top() || rr.top() > list.bottom() {
                    continue;
                }
                widgets::record(ui, &format!("mo-lib:{}", pr.name), rr);
                let hit = rr.intersect(list);
                let resp = ui.interact(hit, ui.id().with(("mo-lib-row", pr.name)), Sense::click());
                if resp.hovered() {
                    p.rect_filled(rr, theme::cr(theme::RADIUS_MEDIUM), theme::BG_SECONDARY);
                    hover = Some(pr.name);
                    hover_row = Some(rr);
                }
                icons::paint(&p, preset_icon(pr), Pos2::new(rr.left() + 16.0, rr.center().y), 14.0, theme::ICON_SECONDARY);
                p.text(Pos2::new(rr.left() + 32.0, rr.center().y), Align2::LEFT_CENTER, pr.name, theme::font_body_strong(), theme::TEXT);
                if let Some(s) = pr.speed {
                    p.text(Pos2::new(rr.right() - 8.0, rr.center().y), Align2::RIGHT_CENTER, s.name(), theme::font_body(), theme::TEXT_TERTIARY);
                }
                if resp.clicked() {
                    apply = Some(pr);
                }
            }
        }
        // Live preview card beside the popover (painted by the popover itself so no extra
        // layer appears under the pointer).
        if let (Some(name), Some(row)) = (hover, hover_row) {
            if let Some(pr) = motion::find_preset(name) {
                let card = Rect::from_min_size(
                    Pos2::new((rect.left() - 8.0 - 120.0).max(8.0), row.top().min(screen.bottom() - 136.0).max(8.0)),
                    Vec2::new(120.0, 128.0),
                );
                let time = ui.ctx().input(|i| i.time);
                widgets::elevation_400(ui.painter(), card, theme::RADIUS_LARGE);
                ui.painter().rect_filled(card, theme::cr(theme::RADIUS_LARGE), theme::BG);
                ui.painter().rect_stroke(card, theme::cr(theme::RADIUS_LARGE), Stroke::new(1.0, theme::ELEVATION_400_RING), StrokeKind::Inside);
                preview(ui, card, pr, time);
                ui.ctx().request_repaint();
            }
        }
    });
    st.hover = hover;
    if let Some(pr) = apply {
        let cmd = edit::apply_preset(&app.doc, &sel, pr, start);
        run_all(app, vec![cmd]);
        close = true;
    }
    if add {
        add_default_clip(app, &sel);
        close = true;
    }
    if !(close || outside) {
        app.motion_ins.library = Some(st);
    }
}

/// Plays `preset` on a small square in `card`: the preset's clips evaluated like a layer would.
fn preview(ui: &mut Ui, card: Rect, preset: &Preset, time: f64) {
    let p = ui.painter();
    p.text(Pos2::new(card.left() + 12.0, card.top() + 14.0), Align2::LEFT_CENTER, preset.name, theme::font_body_strong(), theme::TEXT);
    let stage = Rect::from_min_max(Pos2::new(card.left() + 8.0, card.top() + 28.0), Pos2::new(card.right() - 8.0, card.bottom() - 8.0));
    p.rect_filled(stage, theme::cr(theme::RADIUS_MEDIUM), theme::BG_SECONDARY);
    let dur = preset.base_duration_ms() as f32;
    let period = if preset.loops() { 2000.0 } else { dur + 800.0 };
    let t = ((time * 1000.0) as f32).rem_euclid(period);
    let mut node = Node::rectangle(NodeId(1), DRect::new(0.0, 0.0, 24.0, 24.0));
    node.motion.clips = preset.clips();
    let ev = motion::evaluate_node(&node, t);
    let off = ev.position.unwrap_or(DVec2::ZERO);
    let k = 0.3;
    let c = stage.center() + Vec2::new(off.x * k, off.y * k);
    let sc = ev.scale.unwrap_or(DVec2::new(1.0, 1.0));
    let (hw, hh) = (ev.width.unwrap_or(24.0) * sc.x * 0.5, ev.height.unwrap_or(24.0) * sc.y * 0.5);
    let th = -ev.rotation.unwrap_or(0.0).to_radians();
    let (s, co) = th.sin_cos();
    let pts: Vec<Pos2> = [(-hw, -hh), (hw, -hh), (hw, hh), (-hw, hh)].iter().map(|(x, y)| Pos2::new(c.x + x * co - y * s, c.y + x * s + y * co)).collect();
    let alpha = ev.opacity.unwrap_or(1.0).clamp(0.0, 1.0);
    let clipped = p.with_clip_rect(stage);
    clipped.add(Shape::convex_polygon(pts, theme::CANVAS_SELECTION.gamma_multiply(alpha), Stroke::NONE));
}
