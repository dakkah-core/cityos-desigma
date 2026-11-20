//! A small floating pill while the agent bridge is on, with an Off button.

use crate::app::App;
use crate::comments::ui::button;
use crate::{theme, widgets};
use egui::{Align2, Color32, Context, Id, LayerId, Order, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};

pub fn show(app: &mut App, ctx: &Context) {
    let Some(bridge) = &app.bridge.bridge else { return };
    if app.home.is_some() || app.proto.present.is_some() {
        return;
    }
    let last = bridge.last_call.lock().ok().and_then(|l| l.clone());
    let text = match &last {
        Some(name) => format!("Agent · {}", name.trim_start_matches("desigma_")),
        None => "Agent bridge on".to_string(),
    };
    let tip = format!("desigma-mcp can edit this document (port {}). Click Off to stop.", bridge.port());
    let screen = ctx.content_rect();
    let measure = ctx.layer_painter(LayerId::new(Order::Tooltip, Id::new("bridge-measure")));
    let size = Vec2::new(widgets::text_width(&measure, &text, &theme::font_body()) + 86.0, 32.0);
    let r = Rect::from_min_size(Pos2::new(screen.right() - size.x - 16.0, screen.top() + 56.0), size);
    let mut off = false;
    egui::Area::new(Id::new("bridge-pill")).order(Order::Tooltip).fixed_pos(r.min).constrain(false).show(ctx, |ui| {
        ui.allocate_rect(r, Sense::hover()).on_hover_text(&tip);
        let p = ui.painter().clone();
        widgets::elevation_400(&p, r, theme::MENU_RADIUS);
        p.rect_filled(r, theme::cr(theme::MENU_RADIUS), theme::BG_MENU);
        p.rect_stroke(r, theme::cr(theme::MENU_RADIUS), Stroke::new(1.0, theme::ELEVATION_400_RING), StrokeKind::Inside);
        widgets::record(ui, "bridge-pill", r);
        p.circle_filled(Pos2::new(r.left() + 14.0, r.center().y), 4.0, Color32::from_rgb(0x3D, 0xD6, 0x8C));
        p.text(Pos2::new(r.left() + 26.0, r.center().y), Align2::LEFT_CENTER, &text, theme::font_body(), Color32::WHITE);
        let ob = Rect::from_center_size(Pos2::new(r.right() - 30.0, r.center().y), Vec2::new(44.0, 22.0));
        off = button(ui, ob, "bridge-off", "Off", false, true).clicked();
        widgets::record(ui, "bridge-off", ob);
    });
    if off {
        app.toggle_agent_bridge(ctx);
    }
}
