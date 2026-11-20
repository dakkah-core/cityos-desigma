//! The "desigma X is available" banner and the update dialog: release notes, then Update now → progress → Restart to
//! update, with Skip this version, Later and a link to the release page.

use super::Install;
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
    let ready = matches!(app.updater.install, Install::Ready(_));
    let text = if ready { format!("desigma {} is ready to install", release.version) } else { format!("desigma {} is available", release.version) };
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

/// One line of the dialog body.
enum Row {
    Text(std::sync::Arc<egui::Galley>, f32),
    /// The progress bar: a fraction, or `None` for "working, no number".
    Bar(Option<f32>),
    /// "Open release page".
    Link,
}

const BAR_H: f32 = 6.0;
const LINK_H: f32 = 18.0;

/// "12.3 MB": download sizes are tens of megabytes.
pub fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
}

/// "12.3 of 45.6 MB", or just "12.3 MB" while the size is unknown.
pub fn progress_text(done: u64, total: Option<u64>) -> String {
    match total {
        Some(t) if t > 0 => format!("{} of {}", megabytes(done).trim_end_matches(" MB"), megabytes(t)),
        _ => megabytes(done),
    }
}

/// Failure messages start lower case ("the download was interrupted"); the dialog shows sentences.
fn sentence(message: &str) -> String {
    let mut chars = message.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

fn plain(p: &egui::Painter, text: impl Into<String>, font: FontId, color: Color32, gap: f32, width: f32) -> Row {
    Row::Text(p.layout(text.into(), font, color, width), gap)
}

/// The modal: version and, per step of "Update now", what is happening; a short excerpt of "What's Changed";
/// the release page link; then the buttons that fit the step.
fn dialog(app: &mut App, ctx: &Context) {
    if !app.updater.dialog {
        return;
    }
    let Some(release) = app.updater.available.clone() else {
        app.updater.dialog = false;
        return;
    };
    let install = app.updater.install.clone();
    let screen = ctx.content_rect();
    let text_w = W - PAD * 2.0;
    let measure = ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("updater-measure")));
    let m = &measure;
    let mut rows: Vec<Row> = Vec::new();
    match &install {
        Install::Idle => rows.push(plain(
            m,
            format!("You have {}. desigma can download this update and install it for you.", crate::VERSION),
            theme::font_body(),
            theme::TEXT_SECONDARY,
            14.0,
            text_w,
        )),
        Install::Downloading { done, total } => {
            rows.push(plain(m, "Downloading the update…", theme::font_body_strong(), theme::TEXT, 8.0, text_w));
            rows.push(Row::Bar(total.filter(|t| *t > 0).map(|t| (*done as f32 / t as f32).clamp(0.0, 1.0))));
            rows.push(plain(m, progress_text(*done, *total), theme::font_body(), theme::TEXT_SECONDARY, 14.0, text_w));
        }
        Install::Verifying | Install::Preparing => {
            let what = if matches!(install, Install::Verifying) { "Checking the download…" } else { "Preparing the update…" };
            rows.push(plain(m, what, theme::font_body_strong(), theme::TEXT, 8.0, text_w));
            rows.push(Row::Bar(None));
        }
        Install::Ready(_) => rows.push(plain(
            m,
            format!("desigma {} is downloaded and checked. Restart to finish updating. If you have unsaved changes you are asked first.", release.version),
            theme::font_body(),
            theme::TEXT_SECONDARY,
            14.0,
            text_w,
        )),
        Install::Failed(error) => {
            rows.push(plain(m, "The update didn't finish", theme::font_body_strong(), theme::DANGER, 4.0, text_w));
            rows.push(plain(m, sentence(error), theme::font_body(), theme::TEXT_SECONDARY, 6.0, text_w));
            rows.push(plain(m, "You can still get this version from the release page.", theme::font_body(), theme::TEXT_SECONDARY, 14.0, text_w));
        }
    }
    if !release.notes.is_empty() {
        rows.push(plain(m, "What's changed", theme::font_body_strong(), theme::TEXT, 6.0, text_w));
        rows.extend(release.notes.iter().take(MAX_NOTES).map(|n| plain(m, n.clone(), theme::font_body(), theme::TEXT_SECONDARY, 4.0, text_w)));
        if release.notes.len() > MAX_NOTES {
            rows.push(plain(
                m,
                format!("and {} more on the release page", release.notes.len() - MAX_NOTES),
                theme::font_body(),
                theme::TEXT_TERTIARY,
                4.0,
                text_w,
            ));
        }
    }
    rows.push(Row::Link);
    let heights: Vec<f32> = rows
        .iter()
        .map(|row| match row {
            Row::Text(galley, gap) => galley.size().y + gap,
            Row::Bar(_) => BAR_H + 10.0,
            Row::Link => LINK_H,
        })
        .collect();
    let body_h: f32 = heights.iter().sum();
    let rect = Rect::from_center_size(screen.center() - Vec2::new(0.0, 24.0), Vec2::new(W, PAD + 24.0 + 12.0 + body_h + 12.0 + BTN_H + PAD));

    // right-aligned buttons, the primary one first (rightmost); Skip this version sits on the left
    let (buttons, skip_offered): (Vec<(&str, &str, bool)>, bool) = match &install {
        Install::Idle => (vec![("updater-update", "Update now", true), ("updater-later", "Later", false)], true),
        Install::Downloading { .. } => (vec![("updater-cancel", "Cancel", false)], false),
        Install::Verifying | Install::Preparing => (vec![], false),
        Install::Ready(_) => (vec![("updater-restart", "Restart to update", true), ("updater-later", "Later", false)], false),
        Install::Failed(_) => (vec![("updater-retry", "Try again", true), ("updater-later", "Later", false)], true),
    };
    let (enter, esc) = ctx.input_mut(|i| (i.consume_key(Modifiers::NONE, Key::Enter), i.consume_key(Modifiers::NONE, Key::Escape)));
    // what Enter and Escape do depends on the step
    let mut clicked: Option<&str> = None;
    if enter {
        clicked = buttons.iter().find(|b| b.2).map(|b| b.0);
    }
    if esc {
        clicked = buttons.iter().find(|b| b.0 == "updater-later" || b.0 == "updater-cancel").map(|b| b.0);
    }
    let busy = install.busy();
    let mut page = false;
    let mut skip = false;
    egui::Area::new(Id::new("updater-dialog")).order(Order::Foreground).fixed_pos(screen.min).interactable(true).show(ctx, |ui| {
        let scrim = ui.allocate_rect(screen, Sense::click_and_drag());
        let p = ui.painter().clone();
        p.rect_filled(screen, 0.0, Color32::from_black_alpha(140));
        if scrim.clicked() && !busy && !rect.contains(scrim.interact_pointer_pos().unwrap_or(Pos2::ZERO)) {
            clicked = Some("updater-later");
        }
        widgets::menu_panel(&p, rect, theme::MENU_RADIUS);
        widgets::record(ui, "updater-dialog", rect);
        let x0 = rect.left() + PAD;
        let mut y = rect.top() + PAD;
        let title = if matches!(install, Install::Ready(_)) {
            format!("desigma {} is ready", release.version)
        } else {
            format!("desigma {} is available", release.version)
        };
        p.text(Pos2::new(x0, y + 12.0), Align2::LEFT_CENTER, title, theme::font_large_strong(), theme::TEXT);
        y += 24.0 + 12.0;
        for (row, h) in rows.into_iter().zip(heights) {
            match row {
                Row::Text(galley, _) => p.galley(Pos2::new(x0, y), galley, theme::TEXT_SECONDARY),
                Row::Bar(fraction) => {
                    let track = Rect::from_min_size(Pos2::new(x0, y), Vec2::new(text_w, BAR_H));
                    p.rect_filled(track, theme::cr(3), theme::BG_TERTIARY);
                    let fill = match fraction {
                        Some(f) => Rect::from_min_size(track.min, Vec2::new((track.width() * f).max(BAR_H), BAR_H)),
                        None => {
                            // no number to show: a block sliding back and forth
                            let t = ctx.input(|i| i.time) as f32;
                            let w = track.width() * 0.3;
                            let at = (1.0 - (t * 1.4).cos()) / 2.0 * (track.width() - w);
                            ctx.request_repaint_after(std::time::Duration::from_millis(33));
                            Rect::from_min_size(Pos2::new(track.left() + at, y), Vec2::new(w, BAR_H))
                        }
                    };
                    p.rect_filled(fill, theme::cr(3), theme::BG_BRAND);
                    widgets::record(ui, "updater-bar", track);
                }
                Row::Link => {
                    let label = "Open release page";
                    let w = widgets::text_width(&p, label, &theme::font_body());
                    let r = Rect::from_min_size(Pos2::new(x0, y), Vec2::new(w, LINK_H));
                    let resp = ui.interact(r, Id::new("updater-page"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
                    p.text(r.left_center(), Align2::LEFT_CENTER, label, theme::font_body(), theme::TEXT_BRAND);
                    if resp.hovered() {
                        let base = r.left_bottom() - Vec2::new(0.0, 2.0);
                        p.line_segment([base, base + Vec2::new(w, 0.0)], Stroke::new(1.0, theme::TEXT_BRAND));
                    }
                    widgets::record(ui, "updater-page", r);
                    page |= resp.clicked();
                }
            }
            y += h;
        }
        y = rect.bottom() - PAD - BTN_H;
        let w = |label| button_width(&p, label).max(64.0);
        let mut right = rect.right() - PAD;
        for (key, label, primary) in &buttons {
            let r = Rect::from_min_size(Pos2::new(right - w(*label), y), Vec2::new(w(*label), BTN_H));
            if button(ui, r, *key, label, *primary, true).clicked() {
                clicked = Some(*key);
            }
            widgets::record(ui, key, r);
            right = r.left() - 8.0;
        }
        if skip_offered {
            let r = Rect::from_min_size(Pos2::new(x0, y), Vec2::new(w("Skip this version"), BTN_H));
            skip = button(ui, r, "updater-skip", "Skip this version", false, true).clicked();
            widgets::record(ui, "updater-skip", r);
        }
    });
    if skip {
        app.skip_update();
        return;
    }
    if page {
        if let Err(e) = crate::home::open_link(&release.page) {
            app.show_toast(e, true);
        }
    }
    match clicked {
        Some("updater-update") => app.start_update(ctx),
        Some("updater-retry") => {
            app.updater.install = Install::Idle;
            app.start_update(ctx);
        }
        Some("updater-cancel") => app.cancel_update(),
        Some("updater-restart") => app.request_restart(),
        Some("updater-later") => {
            app.updater.dialog = false;
            app.updater.dismissed = true;
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_as_megabytes() {
        assert_eq!(megabytes(0), "0.0 MB");
        assert_eq!(megabytes(12_900_000), "12.3 MB");
        assert_eq!(progress_text(12_900_000, Some(47_800_000)), "12.3 of 45.6 MB");
        assert_eq!(progress_text(12_900_000, None), "12.3 MB");
        assert_eq!(progress_text(5, Some(0)), "0.0 MB");
    }

    #[test]
    fn errors_become_sentences() {
        assert_eq!(sentence("the download was interrupted"), "The download was interrupted");
        assert_eq!(sentence("GitHub answered 404"), "GitHub answered 404");
        assert_eq!(sentence(""), "");
    }
}
