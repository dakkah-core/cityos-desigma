//! The clip settings popover: kind, Type (slide in / out / custom), Direction, the amount,
//! Delay, Duration and the easing editor.

use super::easing;
use super::*;
use crate::widgets::menu::dropdown;
use desigma_doc::motion::{ClipDirection, ClipKind};

const W: f32 = 240.0;
const ROW: f32 = 32.0;
const CONTROL_X: f32 = 108.0;
const CONTROL_W: f32 = 124.0;

fn amount_label(kind: ClipKind) -> &'static str {
    match kind {
        ClipKind::Position => "Distance",
        ClipKind::Scale => "Scale",
        ClipKind::Size => "Size",
        ClipKind::Rotation => "Angle",
        ClipKind::Opacity => "Opacity",
    }
}

/// Display factor and suffix of the amount of `kind`.
fn amount_unit(kind: ClipKind) -> (f32, &'static str) {
    match kind {
        ClipKind::Position => (1.0, ""),
        ClipKind::Rotation => (1.0, "°"),
        _ => (100.0, "%"),
    }
}

/// Height of the popover for `clip`.
fn height(clip: &Clip) -> f32 {
    let custom = clip.mode == ClipMode::Custom;
    let mut rows = 1; // type
    if custom {
        rows += 2;
    } else {
        if !clip.kind.directions().is_empty() {
            rows += 1;
        }
        rows += 1; // amount
    }
    40.0 + 8.0 + rows as f32 * ROW + 12.0 + 3.0 * ROW + 8.0 + 32.0 + easing::GRAPH_H + 8.0 + 24.0 + 12.0
}

fn label(ui: &Ui, rect: Rect, text: &str) {
    let font = theme::font_body();
    widgets::text_on_baseline(
        ui.painter(),
        rect.left() + 16.0,
        widgets::centered_baseline(Rect::from_min_size(rect.min, Vec2::new(rect.width(), 24.0)), &font),
        text,
        font,
        theme::TEXT_SECONDARY,
    );
}

pub(super) fn popover(app: &mut App, ctx: &egui::Context, region: Rect) {
    let Some((node, cid)) = app.motion.selected_clip else { return };
    let Some(clip) = app.doc.get(node).and_then(|n| n.motion.clip(cid)).cloned() else {
        app.motion.selected_clip = None;
        return;
    };
    let screen = ctx.content_rect();
    let h = height(&clip).min(screen.height() - 16.0);
    let anchor = app.motion_ins.clip_anchor;
    let x = (region.left() - 8.0 - W).max(screen.left() + 8.0);
    let y = anchor.map_or(region.top() + 100.0, |a| a.top() - 16.0).min(screen.bottom() - h - 8.0).max(screen.top() + 8.0);
    let rect = Rect::from_min_size(Pos2::new(x, y), Vec2::new(W, h));
    let mut new = clip.clone();
    let mut close = false;
    let mut live_ease: Option<Ease> = None;
    let outside = float(ctx, Id::new("mo-clip-pop"), rect, anchor, |ui| {
        // Header: kind dropdown + close.
        let kr = Rect::from_min_size(Pos2::new(rect.left() + 8.0, rect.top() + 8.0), Vec2::new(116.0, 24.0));
        widgets::record(ui, "mo-clip-kind", kr);
        let names: Vec<String> = ClipKind::ALL.iter().map(|k| k.name().to_string()).collect();
        if let Some(i) = dropdown(ui, kr, "mo-clip-kind", clip.kind.name(), &names) {
            let k = ClipKind::ALL[i];
            if k != clip.kind {
                let mut c = Clip::new(k, clip.mode);
                c.id = clip.id;
                c.delay_ms = clip.delay_ms;
                c.duration_ms = clip.duration_ms;
                c.easing = clip.easing;
                c.repeat = clip.repeat;
                c.yoyo = clip.yoyo;
                c.visible = clip.visible;
                new = c;
            }
        }
        let close_r = Rect::from_min_size(Pos2::new(rect.right() - 32.0, rect.top() + 8.0), Vec2::splat(24.0));
        widgets::record(ui, "mo-clip-close", close_r);
        if widgets::icon_button(ui, close_r, "mo-clip-close", Icon::Close, false, "").clicked() {
            close = true;
        }
        widgets::hline(ui.painter(), rect.left(), rect.right(), rect.top() + 39.0, theme::BORDER);

        let mut y = rect.top() + 48.0;
        let row = |ui: &mut Ui, y: &mut f32, name: &str| -> Rect {
            let line = Rect::from_min_size(Pos2::new(rect.left(), *y), Vec2::new(W, 24.0));
            label(ui, line, name);
            *y += ROW;
            Rect::from_min_size(Pos2::new(rect.left() + CONTROL_X, line.top()), Vec2::new(CONTROL_W, 24.0))
        };

        // Type.
        let r = row(ui, &mut y, "Type");
        widgets::record(ui, "mo-clip-type", r);
        let modes = [ClipMode::In, ClipMode::Out, ClipMode::Custom];
        let labels: Vec<String> = modes.iter().map(|m| m.label(clip.kind)).collect();
        if let Some(i) = dropdown(ui, r, "mo-clip-type", &clip.mode.label(clip.kind), &labels) {
            new.mode = modes[i];
        }
        if clip.mode == ClipMode::Custom {
            // From / To values.
            for (name, to) in [("From", false), ("To", true)] {
                let r = row(ui, &mut y, name);
                let v = if to { clip.to } else { clip.from };
                let vec2 = matches!(clip.kind, ClipKind::Position | ClipKind::Scale | ClipKind::Size);
                let (f, suffix) = amount_unit(clip.kind);
                let f = if clip.kind == ClipKind::Position { 1.0 } else { f };
                let suffix = if clip.kind == ClipKind::Position { "" } else { suffix };
                let set = |c: &mut Clip, nv: DVec2| {
                    if to {
                        c.to = nv;
                    } else {
                        c.from = nv;
                    }
                };
                if vec2 {
                    let a = Rect::from_min_size(r.min, Vec2::new(60.0, 24.0));
                    let b = Rect::from_min_size(Pos2::new(r.left() + 64.0, r.top()), Vec2::new(60.0, 24.0));
                    widgets::record(ui, &format!("mo-clip-{name}-x"), a);
                    if let Some(nv) = field::num_field(ui, a, ("mo-clip-v", name, 0), Prefix::Text("X"), Some((v.x * f * 100.0).round() / 100.0), suffix) {
                        set(&mut new, DVec2::new(nv / f, v.y));
                    }
                    if let Some(nv) = field::num_field(ui, b, ("mo-clip-v", name, 1), Prefix::Text("Y"), Some((v.y * f * 100.0).round() / 100.0), suffix) {
                        set(&mut new, DVec2::new(v.x, nv / f));
                    }
                } else if let Some(nv) = field::num_field(ui, r, ("mo-clip-v", name, 0), Prefix::None, Some((v.x * f * 100.0).round() / 100.0), suffix) {
                    set(&mut new, DVec2::new(nv / f, v.y));
                }
            }
        } else {
            // Direction.
            if !clip.kind.directions().is_empty() {
                let r = row(ui, &mut y, "Direction");
                widgets::record(ui, "mo-clip-dir", r);
                let dirs: &[ClipDirection] = clip.kind.directions();
                let labels: Vec<String> = dirs.iter().map(|d| d.label(clip.mode)).collect();
                let cur = dirs.iter().position(|d| *d == clip.direction).map_or_else(|| clip.direction.label(clip.mode), |i| labels[i].clone());
                if let Some(i) = dropdown(ui, r, "mo-clip-dir", &cur, &labels) {
                    new.direction = dirs[i];
                }
            }
            // Amount.
            let r = row(ui, &mut y, amount_label(clip.kind));
            widgets::record(ui, "mo-clip-amount", r);
            let (f, suffix) = amount_unit(clip.kind);
            if let Some(v) = field::num_field(ui, r, "mo-clip-amount", Prefix::None, Some((clip.amount * f * 100.0).round() / 100.0), suffix) {
                new.amount = v / f;
            }
        }
        y += 4.0;
        widgets::hline(ui.painter(), rect.left(), rect.right(), y, theme::BORDER);
        y += 8.0;

        // Delay and duration.
        let r = row(ui, &mut y, "Delay");
        widgets::record(ui, "mo-clip-delay", r);
        if let Some(v) = field::num_field(ui, r, "mo-clip-delay", Prefix::Icon(Icon::Clock), Some(clip.delay_ms as f32), "ms") {
            new.delay_ms = v.max(0.0).round() as u32;
        }
        let r = row(ui, &mut y, "Duration");
        widgets::record(ui, "mo-clip-duration", r);
        if let Some(v) = field::num_field(ui, r, "mo-clip-duration", Prefix::Icon(Icon::Clock), Some(clip.duration_ms as f32), "ms") {
            new.duration_ms = v.max(1.0).round() as u32;
        }

        // Easing.
        let line = Rect::from_min_size(Pos2::new(rect.left(), y), Vec2::new(W, 24.0));
        label(ui, line, "Easing");
        let dd = Rect::from_min_size(Pos2::new(rect.left() + CONTROL_X, y), Vec2::new(CONTROL_W, 24.0));
        if let Some(e) = easing::dropdown(app, ui, dd, "clip", clip.easing) {
            live_ease = Some(e);
        }
        y += 32.0;
        let (e, _) = easing::body(app, ui, Pos2::new(rect.left() + 8.0, y), W - 16.0, "clip", clip.easing);
        if e.is_some() {
            live_ease = e;
        }
    });
    if let Some(e) = live_ease {
        new.easing = e;
    }
    if new != clip {
        let cmd = edit::update_clip(&app.doc, node, new);
        run_all(app, vec![cmd]);
    }
    // Clicking the timeline (e.g. dragging this clip's bar) keeps the clip selected.
    let on_timeline = ctx.input(|i| i.pointer.press_origin()).zip(crate::motion::timeline::last_rect(ctx)).is_some_and(|(p, r)| r.contains(p));
    if close || (outside && !on_timeline) {
        app.motion.selected_clip = None;
        app.motion_ins.easing.menu = None;
    }
}
