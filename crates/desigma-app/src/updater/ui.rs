//! The "desigma X is available" banner and the update dialog (release notes, Download, Skip this version, Later).

use crate::app::App;
use crate::comments::ui::{button, button_width};
use crate::icons::{self, Icon};
use crate::{theme, widgets};
use egui::{Align2, Color32, Context, FontId, Id, Key, LayerId, Modifiers, Order, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};

const W: f32 = 440.0;
const PAD: f32 = 20.0;
const BTN_H: f32 = 32.0;
/// Release-note lines shown in the dialog; the rest are only counted.
const MAX_NOTES: usize = 8;

/// Draws the banner and the dialog. Call once per frame on the home screen and in the editor.
pub fn show(app: &mut App, ctx: &Context) {
    banner(app, ctx);
    dialog(app, ctx);
}

/// A floating pill at the top of the window, like the library updates banner (and below it when both show).
fn banner(app: &mut App, ctx: &Context) {
    let Some(release) = &app.updater.available else { return };
    if app.updater.dismissed || app.updater.dialog {
        return;
    }
    let text = format!("desigma {} is available", release.version);
    let screen = ctx.content_rect();
    let below_libraries = app.home.is_none() && app.update_banner_visible();
    let measure = ctx.layer_painter(LayerId::new(Order::Tooltip, Id::new("updater-measure")));
    let size = Vec2::new(widgets::text_width(&measure, &text, &theme::font_body()) + 158.0, 40.0);
    let r = Rect::from_center_size(Pos2::new(screen.center().x, screen.top() + if below_libraries { 120.0 } else { 72.0 }), size);
    let (mut view, mut dismiss) = (false, false);
    egui::Area::new(Id::new("updater-banner")).order(Order::Tooltip).fixed_pos(r.min).constrain(false).show(ctx, |ui| {
        ui.allocate_rect(r, Sense::hover());
        let p = ui.painter().clone();
        widgets::elevation_400(&p, r, theme::MENU_RADIUS);
        p.rect_filled(r, theme::cr(theme::MENU_RADIUS), theme::BG_MENU);
        p.rect_stroke(r, theme::cr(theme::MENU_RADIUS), Stroke::new(1.0, theme::ELEVATION_400_RING), StrokeKind::Inside);
        widgets::record(ui, "updater-banner", r);
        icons::paint(&p, Icon::Export, Pos2::new(r.left() + 22.0, r.center().y), 16.0, theme::ICON_SELECTED);
        p.text(Pos2::new(r.left() + 40.0, r.center().y), Align2::LEFT_CENTER, &text, theme::font_body(), Color32::WHITE);
        let vb = Rect::from_center_size(Pos2::new(r.right() - 76.0, r.center().y), Vec2::new(64.0, 26.0));
        view = button(ui, vb, "updater-view", "View", true, true).clicked();
        widgets::record(ui, "updater-view", vb);
        let xb = Rect::from_center_size(Pos2::new(r.right() - 22.0, r.center().y), Vec2::splat(24.0));
        let xr = ui.interact(xb, Id::new("updater-dismiss"), Sense::click()).on_hover_text("Hide until the next launch");
        widgets::button_bg(&p, xb, &xr, false, theme::cr(theme::RADIUS_MEDIUM));
        widgets::record(ui, "updater-dismiss", xb);
        let c = xb.center();
        let st = Stroke::new(1.2, theme::ICON_SECONDARY);
        p.line_segment([c + Vec2::new(-3.5, -3.5), c + Vec2::new(3.5, 3.5)], st);
        p.line_segment([c + Vec2::new(-3.5, 3.5), c + Vec2::new(3.5, -3.5)], st);
        dismiss = xr.clicked();
    });
    if view {
        app.updater.dialog = true;
    } else if dismiss {
        app.updater.dismissed = true;
    }
}

/// The modal: version, a plain-text excerpt of "What's Changed", then Skip this version · Later · Download.
fn dialog(app: &mut App, ctx: &Context) {
    if !app.updater.dialog {
        return;
    }
    let Some(release) = app.updater.available.clone() else {
        app.updater.dialog = false;
        return;
    };
    let screen = ctx.content_rect();
    let text_w = W - PAD * 2.0;
    // (text, font, colour, space after)
    let mut rows: Vec<(String, FontId, Color32, f32)> =
        vec![(format!("You have {}. Download opens the release page on GitHub.", crate::VERSION), theme::font_body(), theme::TEXT_SECONDARY, 14.0)];
    if !release.notes.is_empty() {
        rows.push(("What's changed".into(), theme::font_body_strong(), theme::TEXT, 6.0));
        rows.extend(release.notes.iter().take(MAX_NOTES).map(|n| (n.clone(), theme::font_body(), theme::TEXT_SECONDARY, 4.0)));
        if release.notes.len() > MAX_NOTES {
            rows.push((format!("and {} more on the release page", release.notes.len() - MAX_NOTES), theme::font_body(), theme::TEXT_TERTIARY, 4.0));
        }
    }
    let measure = ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("updater-measure")));
    let galleys: Vec<_> = rows.into_iter().map(|(t, f, c, gap)| (measure.layout(t, f, c, text_w), gap)).collect();
    let body_h: f32 = galleys.iter().map(|(g, gap)| g.size().y + gap).sum();
    let rect = Rect::from_center_size(screen.center() - Vec2::new(0.0, 24.0), Vec2::new(W, PAD + 24.0 + 12.0 + body_h + 12.0 + BTN_H + PAD));

    let (enter, esc) = ctx.input_mut(|i| (i.consume_key(Modifiers::NONE, Key::Enter), i.consume_key(Modifiers::NONE, Key::Escape)));
    let (mut download, mut skip, mut later) = (enter, false, esc);
    egui::Area::new(Id::new("updater-dialog")).order(Order::Foreground).fixed_pos(screen.min).interactable(true).show(ctx, |ui| {
        let scrim = ui.allocate_rect(screen, Sense::click_and_drag());
        let p = ui.painter().clone();
        p.rect_filled(screen, 0.0, Color32::from_black_alpha(140));
        if scrim.clicked() && !rect.contains(scrim.interact_pointer_pos().unwrap_or(Pos2::ZERO)) {
            later = true;
        }
        widgets::menu_panel(&p, rect, theme::MENU_RADIUS);
        widgets::record(ui, "updater-dialog", rect);
        let x0 = rect.left() + PAD;
        let mut y = rect.top() + PAD;
        let title = format!("desigma {} is available", release.version);
        p.text(Pos2::new(x0, y + 12.0), Align2::LEFT_CENTER, title, theme::font_large_strong(), theme::TEXT);
        y += 24.0 + 12.0;
        for (galley, gap) in galleys {
            let h = galley.size().y;
            p.galley(Pos2::new(x0, y), galley, theme::TEXT_SECONDARY);
            y += h + gap;
        }
        y = rect.bottom() - PAD - BTN_H;
        let w = |label| button_width(&p, label).max(64.0);
        let ok = Rect::from_min_size(Pos2::new(rect.right() - PAD - w("Download"), y), Vec2::new(w("Download"), BTN_H));
        let later_r = Rect::from_min_size(Pos2::new(ok.left() - 8.0 - w("Later"), y), Vec2::new(w("Later"), BTN_H));
        let skip_r = Rect::from_min_size(Pos2::new(x0, y), Vec2::new(w("Skip this version"), BTN_H));
        for (r, key, label, primary, hit) in [
            (ok, "updater-download", "Download", true, &mut download),
            (later_r, "updater-later", "Later", false, &mut later),
            (skip_r, "updater-skip", "Skip this version", false, &mut skip),
        ] {
            *hit |= button(ui, r, key, label, primary, true).clicked();
            widgets::record(ui, key, r);
        }
    });
    if skip {
        app.skip_update();
        return;
    }
    if download {
        if let Err(e) = crate::home::open_link(&release.page) {
            app.show_toast(e, true);
        }
    }
    if download || later {
        app.updater.dialog = false;
        app.updater.dismissed = true;
    }
}
