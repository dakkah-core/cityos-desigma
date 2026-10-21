//! The bottom toolbar of a board: Move, Hand, the drawing tray (Marker / Highlighter / Washi tape /
//! Eraser), Sticky, Shape, Connector, Text, Section and Stamp, and a "more" button for the rest (Table), with
//! popovers for the brush, the shape, the stamp and the extra tools.

use super::{color32, color_of, rgb_of, ToolMenu};
use crate::app::App;
use crate::canvas::tools::Tool;
use crate::icons::{self, Icon};
use crate::panels::toolbar::tool_icon;
use crate::theme;
use crate::widgets;
use desigma_doc::{ShapeKind, WashiPattern, BRUSH_SIZES, STAMP_KINDS};
use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};

struct Slot {
    tools: &'static [Tool],
    /// A dropdown chevron next to the button.
    menu: Option<ToolMenu>,
    /// A divider before the slot.
    divider: bool,
    /// The "more" button: it opens a popover of extra tools instead of picking its tool.
    more: bool,
}

const SLOTS: [Slot; 11] = [
    Slot { tools: &[Tool::Move], menu: None, divider: false, more: false },
    Slot { tools: &[Tool::Hand], menu: None, divider: false, more: false },
    Slot { tools: &[Tool::Marker, Tool::Highlighter, Tool::Washi, Tool::Eraser], menu: Some(ToolMenu::Brush), divider: true, more: false },
    Slot { tools: &[Tool::Sticky], menu: Some(ToolMenu::Sticky), divider: false, more: false },
    Slot { tools: &[Tool::BoardShape], menu: Some(ToolMenu::Shape), divider: false, more: false },
    Slot { tools: &[Tool::Connector], menu: None, divider: false, more: false },
    Slot { tools: &[Tool::Text], menu: None, divider: false, more: false },
    Slot { tools: &[Tool::Section], menu: None, divider: false, more: false },
    Slot { tools: &[Tool::Stamp], menu: Some(ToolMenu::Stamp), divider: false, more: false },
    Slot { tools: &[Tool::Code], menu: None, divider: false, more: false },
    Slot { tools: &[Tool::Table], menu: None, divider: false, more: true },
];

const SLOT_GAP: f32 = 4.0;
const DIVIDER_GAP: f32 = 8.0;

fn slot_width(s: &Slot) -> f32 {
    theme::TOOL_BUTTON + if s.menu.is_some() { theme::TOOL_CHEVRON_W } else { 0.0 }
}

/// Total toolbar width.
pub fn toolbar_width() -> f32 {
    let mut w = theme::TOOLBELT_PADDING * 2.0;
    for (i, s) in SLOTS.iter().enumerate() {
        if i > 0 {
            w += SLOT_GAP;
        }
        if s.divider {
            w += DIVIDER_GAP * 2.0 + theme::BORDER_WIDTH;
        }
        w += slot_width(s);
    }
    w
}

pub fn toolbar_rect(screen: Rect) -> Rect {
    let w = toolbar_width();
    Rect::from_min_size(
        Pos2::new(screen.center().x - w / 2.0, screen.bottom() - theme::TOOLBELT_BOTTOM_MARGIN - theme::TOOLBELT_HEIGHT),
        Vec2::new(w, theme::TOOLBELT_HEIGHT),
    )
}

/// The tool a slot's button stands for right now.
fn shown_tool(app: &App, s: &Slot) -> Tool {
    if s.menu == Some(ToolMenu::Brush) {
        if app.tool.is_drawing() {
            app.tool
        } else {
            app.board.brush
        }
    } else {
        s.tools[0]
    }
}

/// Paints the toolbar and handles its buttons.
pub fn show(app: &mut App, ui: &mut Ui, screen: Rect) {
    let bar = toolbar_rect(screen);
    let p = ui.painter().clone();
    widgets::shadow(&p, bar, theme::RADIUS_LARGE, Vec2::new(0.0, 3.0), 8.0, theme::SHADOW_COLOR);
    widgets::shadow(&p, bar, theme::RADIUS_LARGE, Vec2::new(0.0, 1.0), 3.0, theme::SHADOW_COLOR_STRONG);
    p.rect_filled(bar, theme::cr(theme::RADIUS_LARGE), theme::BG_TOOLBAR);
    p.rect_stroke(bar.shrink(0.5), theme::cr(theme::RADIUS_LARGE), Stroke::new(1.0, theme::INNER_HIGHLIGHT), StrokeKind::Inside);
    ui.interact(bar, ui.id().with("board-toolbar-block"), Sense::click_and_drag());

    let y = bar.top() + (bar.height() - theme::TOOL_BUTTON) / 2.0;
    let mut x = bar.left() + theme::TOOLBELT_PADDING;
    for (i, s) in SLOTS.iter().enumerate() {
        if i > 0 {
            x += SLOT_GAP;
        }
        if s.divider {
            x += DIVIDER_GAP;
            widgets::vline(&p, x, bar.top() + 8.0, bar.bottom() - 8.0, theme::BORDER);
            x += DIVIDER_GAP + theme::BORDER_WIDTH;
        }
        let tool = shown_tool(app, s);
        let selected = s.tools.contains(&app.tool);
        let br = Rect::from_min_size(Pos2::new(x, y), Vec2::splat(theme::TOOL_BUTTON));
        let resp = ui.interact(br, ui.id().with(("board-tool", i)), Sense::click());
        let fill = if selected {
            theme::BG_TOOLBAR_SELECTED
        } else if resp.hovered() {
            theme::BG_TOOLBAR_HOVER
        } else {
            Color32::TRANSPARENT
        };
        if fill != Color32::TRANSPARENT {
            p.rect_filled(br, theme::cr(theme::RADIUS_MEDIUM), fill);
        }
        let icon = if s.more && !selected { Icon::Plus } else { tool_icon(tool) };
        icons::paint(&p, icon, br.center(), theme::TOOL_ICON_SIZE, if selected { Color32::WHITE } else { theme::ICON });
        if resp.clicked() {
            if s.more {
                let open = app.board.tool_menu.map(|m| m.0) == Some(ToolMenu::More);
                app.board.tool_menu = if open { None } else { Some((ToolMenu::More, br)) };
            } else {
                app.board.tool_menu = None;
                app.set_tool(tool);
            }
        }
        let tip = if s.more {
            "More".to_string()
        } else if tool.shortcut().is_empty() {
            tool.label().to_string()
        } else {
            format!("{}   {}", tool.label(), tool.shortcut())
        };
        resp.on_hover_text(tip);
        widgets::record(ui, &format!("board-tool-{tool:?}"), br);
        x += theme::TOOL_BUTTON;
        if let Some(menu) = s.menu {
            let cr = Rect::from_min_size(Pos2::new(x, y), Vec2::new(theme::TOOL_CHEVRON_W, theme::TOOL_BUTTON));
            let cresp = ui.interact(cr, ui.id().with(("board-chev", i)), Sense::click());
            let open = app.board.tool_menu.map(|m| m.0) == Some(menu);
            if open || cresp.hovered() {
                p.rect_filled(cr, theme::cr(theme::RADIUS_MEDIUM), theme::BG_TOOLBAR_HOVER);
            }
            icons::paint(
                &p,
                Icon::ChevronDown,
                Pos2::new(br.right() + theme::TOOL_CHEVRON_CENTER_DX, cr.center().y),
                theme::TOOL_CHEVRON_ICON,
                theme::ICON_SECONDARY,
            );
            if cresp.clicked() {
                app.board.tool_menu = if open { None } else { Some((menu, cr)) };
            }
            widgets::record(ui, &format!("board-chevron-{menu:?}"), cr);
            x += theme::TOOL_CHEVRON_W;
        }
    }
    popup(app, ui);
}

/// Cell size of the popover grids.
const CELL: f32 = 40.0;

/// The open popover above the toolbar.
fn popup(app: &mut App, ui: &mut Ui) {
    let Some((menu, anchor)) = app.board.tool_menu else { return };
    let ctx = ui.ctx().clone();
    let pad = 8.0;
    let (cols, rows) = match menu {
        ToolMenu::Brush => (4, 3),
        ToolMenu::Shape => (5, 2),
        ToolMenu::Stamp => (6, 2),
        ToolMenu::Sticky => (2, 1),
        ToolMenu::More => (1, 1),
    };
    let size = match menu {
        ToolMenu::Brush => brush_menu_size(app, pad),
        ToolMenu::More => Vec2::new(200.0 + pad * 2.0, MORE_TOOLS.len() as f32 * 32.0 + pad * 2.0),
        _ => Vec2::new(cols as f32 * CELL + pad * 2.0, rows as f32 * CELL + pad * 2.0),
    };
    let screen = ctx.content_rect();
    let pos = Pos2::new((anchor.center().x - size.x / 2.0).clamp(screen.left() + 8.0, screen.right() - size.x - 8.0), anchor.top() - 8.0 - size.y);
    let rect = Rect::from_min_size(pos, size);
    let mut picked: Option<Pick> = None;
    let area = egui::Area::new(egui::Id::new("board-tool-menu")).order(egui::Order::Foreground).fixed_pos(rect.min).show(&ctx, |ui| {
        ui.allocate_rect(rect, Sense::hover());
        let p = ui.painter().clone();
        widgets::menu_panel(&p, rect, theme::RADIUS_LARGE);
        let inner = rect.shrink(pad);
        match menu {
            ToolMenu::Brush => brush_menu(app, ui, inner, &mut picked),
            ToolMenu::More => more_menu(app, ui, inner, &mut picked),
            ToolMenu::Shape => {
                for (i, k) in ShapeKind::ALL.iter().enumerate() {
                    let cell = Rect::from_min_size(inner.min + Vec2::new((i % cols) as f32 * CELL, (i / cols) as f32 * CELL), Vec2::splat(CELL));
                    let sel = app.board.shape == *k;
                    let resp = ui.interact(cell, ui.id().with(("board-shape", i)), Sense::click());
                    cell_bg(&p, cell, sel, resp.hovered());
                    if let Some(t) = app.board.previews.shape(ui.ctx(), *k, color_of(0xE6E6E6), 24.0, 18.0) {
                        p.image(
                            t,
                            Rect::from_center_size(cell.center(), Vec2::new(24.0, 18.0)),
                            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    }
                    widgets::record(ui, &format!("board-shape-{k:?}"), cell);
                    if resp.on_hover_text(k.label()).clicked() {
                        picked = Some(Pick::Shape(*k));
                    }
                }
            }
            ToolMenu::Sticky => {
                for (i, (wide, icon, tip)) in [(false, Icon::Sticky, "Sticky note"), (true, Icon::WideSticky, "Wide sticky note")].into_iter().enumerate() {
                    let cell = Rect::from_min_size(inner.min + Vec2::new(i as f32 * CELL, 0.0), Vec2::splat(CELL));
                    let resp = ui.interact(cell, ui.id().with(("board-sticky-kind", i)), Sense::click());
                    cell_bg(&p, cell, app.board.wide == wide, resp.hovered());
                    widgets::paint_icon_in(&p, cell, icon, 20.0, theme::ICON);
                    widgets::record(ui, if wide { "board-sticky-wide" } else { "board-sticky-square" }, cell);
                    if resp.on_hover_text(tip).clicked() {
                        picked = Some(Pick::Sticky(wide));
                    }
                }
            }
            ToolMenu::Stamp => {
                for (i, k) in STAMP_KINDS.iter().enumerate() {
                    let cell = Rect::from_min_size(inner.min + Vec2::new((i % cols) as f32 * CELL, (i / cols) as f32 * CELL), Vec2::splat(CELL));
                    let sel = app.board.stamp == *k;
                    let resp = ui.interact(cell, ui.id().with(("board-stamp", i)), Sense::click());
                    cell_bg(&p, cell, sel, resp.hovered());
                    if let Some(t) = app.board.previews.stamp(ui.ctx(), k, 28.0) {
                        p.image(
                            t,
                            Rect::from_center_size(cell.center(), Vec2::splat(28.0)),
                            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    }
                    widgets::record(ui, &format!("board-stamp-{k}"), cell);
                    if resp.on_hover_text(desigma_doc::stamp_label(k)).clicked() {
                        picked = Some(Pick::Stamp((*k).to_string()));
                    }
                }
            }
        }
    });
    let layer = area.response.layer_id;
    let (pressed, ptr, esc) = ctx.input(|i| (i.pointer.any_pressed(), i.pointer.interact_pos(), i.key_pressed(egui::Key::Escape)));
    let other_popup = |p: Pos2| ctx.layer_id_at(p).is_some_and(|l| l.order == egui::Order::Foreground && l != layer);
    let outside = pressed && ptr.is_some_and(|p| !rect.contains(p) && !anchor.contains(p) && !other_popup(p));
    match picked {
        Some(Pick::Shape(k)) => {
            app.board.shape = k;
            app.board.tool_menu = None;
            app.set_tool(Tool::BoardShape);
        }
        Some(Pick::Stamp(k)) => {
            app.board.stamp = k;
            app.board.tool_menu = None;
            app.set_tool(Tool::Stamp);
        }
        Some(Pick::Sticky(wide)) => {
            app.board.wide = wide;
            app.board.tool_menu = None;
            app.set_tool(Tool::Sticky);
        }
        Some(Pick::Brush(t)) => {
            app.board.brush = t;
            app.set_tool(t);
        }
        Some(Pick::Ink(c)) => app.board.ink = color_of(c),
        Some(Pick::More(t)) => {
            app.board.tool_menu = None;
            app.set_tool(t);
        }
        Some(Pick::Size(i)) => app.board.size = i,
        Some(Pick::Pattern(p)) => app.board.pattern = p,
        None => {
            if outside || esc {
                app.board.tool_menu = None;
            }
        }
    }
}

enum Pick {
    More(Tool),
    Shape(ShapeKind),
    Stamp(String),
    Sticky(bool),
    Brush(Tool),
    Ink(u32),
    Size(usize),
    Pattern(WashiPattern),
}

fn cell_bg(p: &egui::Painter, cell: Rect, selected: bool, hover: bool) {
    let r = cell.shrink(2.0);
    if selected {
        p.rect_filled(r, theme::cr(theme::RADIUS_MEDIUM), theme::BG_SELECTED);
    } else if hover {
        p.rect_filled(r, theme::cr(theme::RADIUS_MEDIUM), theme::BG_HOVER);
    }
}

/// The tools in the "more" popover.
const MORE_TOOLS: [Tool; 1] = [Tool::Table];

/// One row per extra tool, with its icon, name and shortcut.
fn more_menu(app: &App, ui: &mut Ui, inner: Rect, picked: &mut Option<Pick>) {
    let p = ui.painter().clone();
    for (i, t) in MORE_TOOLS.into_iter().enumerate() {
        let row = Rect::from_min_size(inner.min + Vec2::new(0.0, i as f32 * 32.0), Vec2::new(inner.width(), 32.0));
        let resp = ui.interact(row, ui.id().with(("board-more", i)), Sense::click());
        if app.tool == t {
            p.rect_filled(row, theme::cr(theme::RADIUS_MEDIUM), theme::BG_SELECTED);
        } else if resp.hovered() {
            p.rect_filled(row, theme::cr(theme::RADIUS_MEDIUM), theme::BG_HOVER);
        }
        icons::paint(&p, tool_icon(t), Pos2::new(row.left() + 18.0, row.center().y), 16.0, theme::ICON);
        p.text(Pos2::new(row.left() + 36.0, row.center().y), Align2::LEFT_CENTER, t.label(), theme::font_body(), theme::TEXT);
        p.text(Pos2::new(row.right() - 8.0, row.center().y), Align2::RIGHT_CENTER, t.shortcut(), theme::font_body(), theme::TEXT_TERTIARY);
        widgets::record(ui, &format!("board-more-{t:?}"), row);
        if resp.clicked() {
            *picked = Some(Pick::More(t));
        }
    }
}

/// Tools listed in the drawing tray.
const TRAY_TOOLS: [Tool; 4] = [Tool::Marker, Tool::Highlighter, Tool::Washi, Tool::Eraser];
const TRAY_ROW: f32 = 32.0;
const INK_CELL: f32 = 26.0;
const OPTION_ROW: f32 = 32.0;

/// Whether the tray shows colours and thickness (everything but the eraser).
fn tray_has_ink(app: &App) -> bool {
    app.board.brush != Tool::Eraser
}

/// Size of the drawing tray: the tools, then the colour row, the thickness row and, for washi
/// tape, the pattern row.
fn brush_menu_size(app: &App, pad: f32) -> Vec2 {
    let mut h = TRAY_TOOLS.len() as f32 * TRAY_ROW + pad * 2.0;
    if tray_has_ink(app) {
        h += 6.0 + INK_CELL + 6.0 + OPTION_ROW;
        if app.board.brush == Tool::Washi {
            h += 6.0 + OPTION_ROW;
        }
    }
    Vec2::new(theme::BOARD_DRAW_INK.len() as f32 * INK_CELL + pad * 2.0, h)
}

/// A small sample of washi tape in `r`.
pub fn pattern_glyph(p: &egui::Painter, r: Rect, pattern: WashiPattern, base: Color32) {
    p.rect_filled(r, 2.0, base);
    let ink = Color32::from_white_alpha(150);
    let st = Stroke::new(1.2, ink);
    let p = p.with_clip_rect(r);
    match pattern {
        WashiPattern::Stripes => {
            let mut x = r.left() - r.height();
            while x < r.right() {
                p.line_segment([Pos2::new(x, r.bottom()), Pos2::new(x + r.height(), r.top())], Stroke::new(2.0, ink));
                x += 6.0;
            }
        }
        WashiPattern::Dots => {
            for (i, y) in [r.top() + 5.0, r.bottom() - 5.0].into_iter().enumerate() {
                let mut x = r.left() + 4.0 + i as f32 * 3.0;
                while x < r.right() {
                    p.circle_filled(Pos2::new(x, y), 1.4, ink);
                    x += 6.0;
                }
            }
        }
        WashiPattern::Grid => {
            let mut x = r.left() + 3.0;
            while x < r.right() {
                p.vline(x, r.y_range(), st);
                x += 6.0;
            }
            p.hline(r.x_range(), r.center().y, st);
        }
        WashiPattern::Zigzag => {
            for y in [r.top() + 5.0, r.bottom() - 5.0] {
                let pts: Vec<Pos2> = (0..8).map(|i| Pos2::new(r.left() + i as f32 * 4.0, y + if i % 2 == 0 { -2.0 } else { 2.0 })).collect();
                p.add(egui::Shape::line(pts, st));
            }
        }
        WashiPattern::Checks => {
            for i in 0..8 {
                for j in 0..3 {
                    if (i + j) % 2 == 0 {
                        p.rect_filled(Rect::from_min_size(Pos2::new(r.left() + i as f32 * 4.0, r.top() + j as f32 * 4.0), Vec2::splat(4.0)), 0.0, ink);
                    }
                }
            }
        }
    }
}

/// The drawing tray: tool rows, the ink colours, the thickness choices and the tape patterns.
fn brush_menu(app: &App, ui: &mut Ui, inner: Rect, picked: &mut Option<Pick>) {
    let p = ui.painter().clone();
    for (i, t) in TRAY_TOOLS.into_iter().enumerate() {
        let row = Rect::from_min_size(inner.min + Vec2::new(0.0, i as f32 * TRAY_ROW), Vec2::new(inner.width(), TRAY_ROW));
        let sel = app.board.brush == t;
        let resp = ui.interact(row, ui.id().with(("board-brush", i)), Sense::click());
        if sel {
            p.rect_filled(row, theme::cr(theme::RADIUS_MEDIUM), theme::BG_SELECTED);
        } else if resp.hovered() {
            p.rect_filled(row, theme::cr(theme::RADIUS_MEDIUM), theme::BG_HOVER);
        }
        icons::paint(&p, tool_icon(t), Pos2::new(row.left() + 18.0, row.center().y), 16.0, theme::ICON);
        p.text(Pos2::new(row.left() + 36.0, row.center().y), Align2::LEFT_CENTER, t.label(), theme::font_body(), theme::TEXT);
        p.text(Pos2::new(row.right() - 8.0, row.center().y), Align2::RIGHT_CENTER, t.shortcut(), theme::font_body(), theme::TEXT_TERTIARY);
        widgets::record(ui, &format!("board-brush-{t:?}"), row);
        if resp.clicked() {
            *picked = Some(Pick::Brush(t));
        }
    }
    if !tray_has_ink(app) {
        return;
    }
    let mut y = inner.top() + TRAY_TOOLS.len() as f32 * TRAY_ROW + 6.0;
    p.hline(inner.x_range(), y - 3.0, Stroke::new(1.0, theme::BORDER));
    for (i, c) in theme::BOARD_DRAW_INK.iter().enumerate() {
        let r = Rect::from_min_size(Pos2::new(inner.left() + i as f32 * INK_CELL, y), Vec2::splat(INK_CELL));
        let resp = ui.interact(r, ui.id().with(("board-ink", i)), Sense::click());
        let sel = rgb_of(app.board.ink) == *c;
        swatch(&p, r.center(), *c, sel, resp.hovered());
        widgets::record(ui, &format!("board-ink-{i}"), r);
        if resp.clicked() {
            *picked = Some(Pick::Ink(*c));
        }
    }
    y += INK_CELL + 6.0;
    // thickness: a dot per choice
    let cell_w = inner.width() / BRUSH_SIZES.len() as f32;
    for (i, k) in BRUSH_SIZES.iter().enumerate() {
        let r = Rect::from_min_size(Pos2::new(inner.left() + i as f32 * cell_w, y), Vec2::new(cell_w, OPTION_ROW));
        let resp = ui.interact(r, ui.id().with(("board-size", i)), Sense::click());
        cell_bg(&p, r, app.board.size == i, resp.hovered());
        p.circle_filled(r.center(), 2.0 + 3.0 * k, theme::ICON);
        widgets::record(ui, &format!("board-size-{i}"), r);
        if resp.on_hover_text(["Thin", "Medium", "Thick"][i]).clicked() {
            *picked = Some(Pick::Size(i));
        }
    }
    if app.board.brush == Tool::Washi {
        y += OPTION_ROW + 6.0;
        let cell_w = inner.width() / WashiPattern::ALL.len() as f32;
        let base = color32(app.board.ink);
        for (i, pat) in WashiPattern::ALL.iter().enumerate() {
            let r = Rect::from_min_size(Pos2::new(inner.left() + i as f32 * cell_w, y), Vec2::new(cell_w, OPTION_ROW));
            let resp = ui.interact(r, ui.id().with(("board-pattern", i)), Sense::click());
            cell_bg(&p, r, app.board.pattern == *pat, resp.hovered());
            pattern_glyph(&p, Rect::from_center_size(r.center(), Vec2::new(cell_w - 10.0, 18.0)), *pat, base);
            widgets::record(ui, &format!("board-pattern-{pat:?}"), r);
            if resp.on_hover_text(pat.label()).clicked() {
                *picked = Some(Pick::Pattern(*pat));
            }
        }
    }
}

/// A round colour swatch with a ring when selected.
pub fn swatch(p: &egui::Painter, c: Pos2, rgb: u32, selected: bool, hover: bool) {
    let r = theme::BOARD_SWATCH / 2.0;
    p.circle_filled(c, r, color32(color_of(rgb)));
    p.circle_stroke(c, r - 0.5, Stroke::new(1.0, theme::BORDER_TRANSLUCENT));
    if selected {
        p.circle_stroke(c, r + 3.0, Stroke::new(2.0, theme::BG_BRAND));
    } else if hover {
        p.circle_stroke(c, r + 3.0, Stroke::new(1.0, theme::TEXT_TERTIARY));
    }
}
