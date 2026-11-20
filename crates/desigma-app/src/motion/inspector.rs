//! Motion mode inspector (the right panel while Motion is on): Animations, Transform, Layout,
//! Appearance, Fill and the static Stroke / Effects / Export sections, every animatable field
//! with its keyframe diamond; plus the Easing panel, the animation library popover, the clip
//! settings popover and the canvas motion-path overlay.
//!
//! Values shown are the ones at the playhead (`value_at`, clips included). Editing a field goes
//! through [`edit::auto_key`] using the keyframed value as the baseline: a property that already
//! has keyframes gets a keyframe at the playhead, any other property changes statically.

mod clip;
mod easing;
mod library;
mod overlay;

#[cfg(test)]
pub use library::visible as library_visible;
pub use overlay::{commit_move, paint_overlay};
#[cfg(test)]
pub use overlay::{gizmo_centre, path_point};

use crate::app::App;
use crate::icons::{self, Icon};
use crate::panels::properties::{self, row_end, section_sep, sublabel, title, Pane};
use crate::panels::{clipped, effects, export, scrollbar, stroke, wheel_scroll};
use crate::theme;
use crate::widgets::field::{self, Prefix};
use crate::widgets::{self, fmt_num};
use desigma_doc::motion::edit::{self, KeyRef};
use desigma_doc::motion::{self, AnimProperty, AnimValue, Clip, ClipKind, ClipMode, Ease};
use desigma_doc::{Color, Command, NodeId, NodeKind, Vec2 as DVec2};
use egui::{Align2, Color32, Id, Painter, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, Vec2};

/// Inspector-owned UI state (the shared flags live in [`super::MotionUi`]).
#[derive(Default)]
pub struct InspectorState {
    pub scroll: f32,
    pub content_h: f32,
    /// Easing editor: open preset menu, active drag.
    pub easing: easing::EasingState,
    /// The animation library popover while open.
    pub library: Option<library::LibraryState>,
    /// Where the clip popover is anchored (the clip row last clicked).
    pub clip_anchor: Option<Rect>,
    /// The presets button of the Animations section (anchor of the library popover).
    pub library_anchor: Option<Rect>,
    /// Custom easing presets saved with the "+" of the preset menu (kept per app session).
    pub saved_eases: Vec<(String, Ease)>,
    /// The scale lock of the Transform section is off (on by default, as in Figma): with it on,
    /// changing one scale axis scales the other proportionally.
    pub scale_unlocked: bool,
    /// The transform gizmo on the canvas is hidden (the crosshair button of the Transform header).
    pub gizmo_hidden: bool,
}

impl InspectorState {
    /// Whether the proportional scale lock is on.
    pub fn scale_locked(&self) -> bool {
        !self.scale_unlocked
    }
}

// ---------------------------------------------------------------- public entry points

/// Opens the Easing panel for the segments that start at `keys`.
pub fn open_easing(app: &mut App, keys: Vec<KeyRef>) {
    app.motion.easing_target = !keys.is_empty();
    app.motion.selected_keys = keys;
    app.motion_ins.easing.menu = None;
}

/// Opens the animation library popover.
pub fn open_library(app: &mut App) {
    app.motion_ins.library = Some(library::LibraryState::new());
}

/// Opens the settings popover of a clip.
pub fn open_clip(app: &mut App, node: NodeId, clip: u32) {
    app.motion.selected_clip = Some((node, clip));
    app.motion_ins.easing.menu = None;
}

// ---------------------------------------------------------------- shared helpers

/// Applies a command as one undo step (or live while a scrub / drag is in progress, which the
/// app folds into a single step when the pointer is released).
pub(super) fn run(app: &mut App, cmd: Command) -> bool {
    if app.history.viewing.is_some() {
        app.show_toast("Viewing an old version is read-only", true);
        return false;
    }
    if app.scrub.is_some() {
        let ok = cmd.apply(&mut app.doc).is_ok();
        app.changed();
        return ok;
    }
    match app.hist.exec(&mut app.doc, cmd) {
        Ok(()) => {
            app.changed();
            app.sanitize();
            true
        }
        Err(e) => {
            app.show_toast(format!("Edit failed: {e}"), true);
            false
        }
    }
}

/// Runs the commands that could be built (errors are skipped) as one undo step.
pub(super) fn run_all(app: &mut App, cmds: Vec<Result<Command, desigma_doc::DocError>>) -> bool {
    let mut ok: Vec<Command> = cmds.into_iter().flatten().collect();
    match ok.len() {
        0 => false,
        1 => run(app, ok.remove(0)),
        _ => run(app, Command::Batch(ok)),
    }
}

/// Marks a continuous edit (drag) as in progress so the app folds it into one undo step.
pub(super) fn mark_drag(ui: &Ui) {
    ui.ctx().data_mut(|d| d.insert_temp(Id::new(field::SCRUB_FLAG), true));
}

fn playhead(app: &App) -> f32 {
    app.motion.playhead_ms as f32
}

/// Selected nodes that can carry motion.
fn targets(app: &App) -> Vec<NodeId> {
    app.selection.iter().copied().filter(|id| app.doc.get(*id).is_some_and(|n| !matches!(n.kind, NodeKind::Page { .. }))).collect()
}

/// Values of `prop` at the playhead (clips included) for every node.
pub(crate) fn shown(app: &App, sel: &[NodeId], prop: AnimProperty) -> Vec<AnimValue> {
    let t = playhead(app);
    sel.iter().filter_map(|id| app.doc.get(*id)).filter_map(|n| motion::value_at(n, prop, t)).collect()
}

/// Component `idx` of the values when they all agree.
fn common_comp(vals: &[AnimValue], idx: usize) -> Option<f32> {
    let xs: Vec<f32> = vals
        .iter()
        .filter_map(|v| match (idx, v) {
            (0, AnimValue::Scalar(s)) => Some(*s),
            (0, AnimValue::Vec2(v)) => Some(v.x),
            (1, AnimValue::Vec2(v)) => Some(v.y),
            _ => None,
        })
        .collect();
    let first = *xs.first()?;
    xs.iter().all(|x| (x - first).abs() < 1e-3).then_some(first)
}

/// Sets `prop` on every node: `f` maps the keyframed baseline at the playhead to the new value.
pub(crate) fn edit_prop(app: &mut App, sel: &[NodeId], prop: AnimProperty, f: impl Fn(AnimValue) -> AnimValue) {
    let t = playhead(app);
    let cmds: Vec<_> = sel
        .iter()
        .filter_map(|id| {
            let n = app.doc.get(*id)?;
            let cur = motion::keyframed_value_at(n, prop, t)?;
            Some(edit::auto_key(&app.doc, *id, prop, f(cur), t))
        })
        .collect();
    run_all(app, cmds);
}

fn set_comp(app: &mut App, sel: &[NodeId], prop: AnimProperty, idx: usize, v: f32) {
    let v = match prop {
        AnimProperty::Opacity => v.clamp(0.0, 1.0),
        AnimProperty::CornerRadius | AnimProperty::StrokeWeight => v.max(0.0),
        AnimProperty::Width | AnimProperty::Height => v.max(1.0),
        _ => v,
    };
    let locked = prop == AnimProperty::Scale && app.motion_ins.scale_locked();
    edit_prop(app, sel, prop, |cur| match cur {
        AnimValue::Vec2(c) if locked => AnimValue::Vec2(proportional(c, idx, v)),
        AnimValue::Vec2(c) => AnimValue::Vec2(if idx == 0 { DVec2::new(v, c.y) } else { DVec2::new(c.x, v) }),
        AnimValue::Scalar(_) => AnimValue::Scalar(v),
        other => other,
    });
}

/// The scale `cur` after setting component `idx` to `v` with the proportional lock on: the other
/// axis keeps its ratio to the edited one (both become `v` when the edited axis was 0).
pub fn proportional(cur: DVec2, idx: usize, v: f32) -> DVec2 {
    let (edited, other) = if idx == 0 { (cur.x, cur.y) } else { (cur.y, cur.x) };
    let o = if edited.abs() < 1e-6 { v } else { other * v / edited };
    if idx == 0 {
        DVec2::new(v, o)
    } else {
        DVec2::new(o, v)
    }
}

/// Whether every selected node has a keyframe of `prop` at the playhead (and at least one does).
fn keyed_at_playhead(app: &App, sel: &[NodeId], prop: AnimProperty) -> bool {
    let t = edit::to_ms(playhead(app));
    !sel.is_empty() && sel.iter().all(|id| app.doc.get(*id).is_some_and(|n| n.motion.track(prop).is_some_and(|tr| tr.key_at(t).is_some())))
}

fn can_animate(app: &App, sel: &[NodeId], prop: AnimProperty) -> bool {
    !sel.is_empty() && sel.iter().all(|id| app.doc.get(*id).is_some_and(|n| motion::base_value(n, prop).is_some()))
}

/// The diamond: adds or removes a keyframe of `prop` at the playhead on every selected node.
fn toggle_key(app: &mut App, sel: &[NodeId], prop: AnimProperty) {
    let t = edit::to_ms(playhead(app));
    let has = keyed_at_playhead(app, sel, prop);
    let cmds: Vec<_> = sel
        .iter()
        .filter(|id| {
            let k = app.doc.get(**id).is_some_and(|n| n.motion.track(prop).is_some_and(|tr| tr.key_at(t).is_some()));
            k == has
        })
        .map(|id| edit::toggle_keyframe(&app.doc, *id, prop, t))
        .collect();
    run_all(app, cmds);
}

// ---------------------------------------------------------------- painting

pub(super) fn diamond_shape(c: Pos2, r: f32) -> Vec<Pos2> {
    vec![Pos2::new(c.x, c.y - r), Pos2::new(c.x + r, c.y), Pos2::new(c.x, c.y + r), Pos2::new(c.x - r, c.y)]
}

/// A diamond glyph: outlined, or filled when `filled`.
pub(super) fn paint_diamond(p: &Painter, c: Pos2, r: f32, filled: bool, col: Color32) {
    let pts = diamond_shape(c, r);
    if filled {
        p.add(Shape::convex_polygon(pts, col, Stroke::NONE));
    } else {
        p.add(Shape::closed_line(pts, Stroke::new(1.25, col)));
    }
}

/// The keyframe toggle next to a field. Returns true when clicked.
fn key_button(ui: &mut Ui, rect: Rect, prop: AnimProperty, on: bool, enabled: bool, key: &str) -> bool {
    widgets::record(ui, &format!("mo-key:{key}"), rect);
    let resp = ui.interact(rect, ui.id().with(("mo-key", key)), if enabled { Sense::click() } else { Sense::hover() });
    let hovered = enabled && resp.hovered();
    let p = ui.painter();
    if on {
        p.rect_filled(rect, theme::cr(theme::RADIUS_MEDIUM), theme::BG_SELECTED);
    } else if hovered {
        p.rect_filled(rect, theme::cr(theme::RADIUS_MEDIUM), theme::BG_HOVER);
    }
    let col = if on {
        theme::CANVAS_SELECTION
    } else if !enabled {
        theme::TEXT_DISABLED.gamma_multiply(0.6)
    } else if hovered {
        theme::TEXT
    } else {
        theme::TEXT_SECONDARY
    };
    paint_diamond(p, rect.center(), 4.5, on, col);
    if enabled {
        let tip = if on { "Remove keyframe" } else { "Add keyframe" };
        resp.on_hover_text(format!("{tip} ({})", prop.name())).clicked()
    } else {
        false
    }
}

/// Field + keyframe diamond for one scalar (or one component of a pair) of `prop`.
#[allow(clippy::too_many_arguments)]
fn num_cell(app: &mut App, ui: &mut Ui, sel: &[NodeId], prop: AnimProperty, idx: usize, rect: Rect, key: &'static str, prefix: Prefix) {
    let f = prop.display_factor();
    let vals = shown(app, sel, prop);
    let value = common_comp(&vals, idx).map(|v| if prop == AnimProperty::Opacity { (v * f).round() } else { (v * f * 100.0).round() / 100.0 });
    let suffix = match prop {
        AnimProperty::Opacity | AnimProperty::Scale => "%",
        AnimProperty::Rotation => "°",
        _ => "",
    };
    if let Some(v) = field::num_field(ui, rect, key, prefix, value, suffix) {
        set_comp(app, sel, prop, idx, v / f);
    }
}

pub(crate) fn diamond_for(app: &mut App, ui: &mut Ui, sel: &[NodeId], prop: AnimProperty, rect: Rect) {
    let on = keyed_at_playhead(app, sel, prop);
    let enabled = can_animate(app, sel, prop);
    if key_button(ui, rect, prop, on, enabled, prop.name()) {
        toggle_key(app, sel, prop);
    }
}

// ---------------------------------------------------------------- panel

pub fn show(app: &mut App, ui: &mut Ui, region: Rect) {
    let ctx = ui.ctx().clone();
    // Keep the shared flags consistent with the document.
    app.motion.selected_keys.retain(|k| app.doc.get(k.node).is_some_and(|n| n.motion.track(k.property).is_some_and(|t| t.key_at(k.time_ms).is_some())));
    if app.motion.selected_keys.is_empty() {
        app.motion.easing_target = false;
    }
    if let Some((n, c)) = app.motion.selected_clip {
        if app.doc.get(n).is_none_or(|nd| nd.motion.clip(c).is_none()) {
            app.motion.selected_clip = None;
        }
    }

    if app.motion.easing_target {
        easing::panel(app, ui, region);
    } else if app.selection.is_empty() {
        crate::panels::properties::show(app, ui, region);
    } else {
        let mut scroll = app.motion_ins.scroll;
        wheel_scroll(ui, region, &mut scroll, app.motion_ins.content_h);
        app.motion_ins.scroll = scroll;
        let sel = targets(app);
        let end = clipped(ui, region, |ui| {
            let mut pane = Pane { x0: region.left() - theme::BORDER_WIDTH, oy: region.top() - scroll, y: 0.0 };
            sections(app, ui, &mut pane, &sel, region);
            pane.y + 24.0
        });
        app.motion_ins.content_h = end;
        scrollbar(ui, region, scroll, end);
        properties::picker_popup(app, ui, region);
        effects::popover_ui(app, ui, region);
    }
    library::popover(app, &ctx, region);
    clip::popover(app, &ctx, region);
}

fn sections(app: &mut App, ui: &mut Ui, pane: &mut Pane, sel: &[NodeId], region: Rect) {
    let Some(n0) = sel.first().and_then(|id| app.doc.get(*id)).cloned() else { return };
    let multi = sel.len() > 1;

    // ---- header: name + quick toggles
    pane.y += theme::RIGHT_SELECTION_HEADER_Y - theme::RIGHT_CONTENT_TOP;
    let name_r = pane.rect(theme::RIGHT_NAME_FIELD_X - 1.0, 0.0, theme::RIGHT_NAME_FIELD_W, theme::FIELD_HEIGHT);
    if multi {
        ui.painter().text(
            Pos2::new(name_r.left() + 8.0, name_r.center().y),
            Align2::LEFT_CENTER,
            format!("{} layers", sel.len()),
            theme::font_large(),
            theme::TEXT,
        );
    } else {
        let id = ui.id().with("mo-sel-name");
        if ui.rect_contains_pointer(name_r) || ui.memory(|m| m.has_focus(id)) {
            let col = if ui.memory(|m| m.has_focus(id)) { theme::BORDER_SELECTED } else { theme::BORDER };
            ui.painter().rect_stroke(name_r, theme::cr(theme::RADIUS_MEDIUM), Stroke::new(1.0, col), StrokeKind::Inside);
        }
        if let Some(s) = field::edit_text(ui, name_r, id, &n0.name, theme::font_large(), theme::TEXT, 8.0) {
            app.rename(sel[0], &s);
        }
    }
    let btns = [
        (Icon::Mask, "Use as mask", n0.is_mask),
        (if n0.locked { Icon::Lock } else { Icon::Unlock }, "Lock", n0.locked),
        (if n0.visible { Icon::Eye } else { Icon::EyeOff }, "Hide / show", !n0.visible),
    ];
    for (i, (icon, tip, on)) in btns.iter().enumerate() {
        let r = pane.rect(theme::RIGHT_NAME_ACTION_X[i] - 1.0, 0.0, theme::PANEL_ACTION_BUTTON, theme::PANEL_ACTION_BUTTON);
        if widgets::icon_button(ui, r, ("mo-hdr", i), *icon, *on, tip).clicked() {
            match i {
                0 => app.edit_nodes(sel, |n| n.is_mask = !on),
                1 => app.edit_nodes(sel, |n| n.locked = !on),
                _ => app.edit_nodes(sel, |n| n.visible = !n.visible),
            }
        }
    }
    pane.y += theme::FIELD_HEIGHT + 8.0;
    section_sep(ui, pane, false);

    animations_section(app, ui, pane, sel, region);

    // ---- Transform
    section_sep(ui, pane, false);
    title(ui, pane, "Transform", None, false);
    {
        // the crosshair of the header shows / hides the transform gizmo on the canvas
        let r = pane.rect(theme::SECTION_TRAILING_X, -theme::SECTION_TITLE_HEIGHT + 8.0, theme::PANEL_ACTION_BUTTON, theme::PANEL_ACTION_BUTTON);
        widgets::record(ui, "mo-gizmo", r);
        let on = !app.motion_ins.gizmo_hidden;
        if widgets::icon_button(ui, r, "mo-gizmo", Icon::Crosshair, on, if on { "Hide transform gizmo" } else { "Show transform gizmo" }).clicked() {
            app.motion_ins.gizmo_hidden = on;
        }
    }
    sublabel(ui, pane, "Position");
    let (x_r, y_r, d_r) = (pane.rect(17.0, 0.0, 68.0, 24.0), pane.rect(89.0, 0.0, 68.0, 24.0), pane.rect(161.0, 0.0, 24.0, 24.0));
    num_cell(app, ui, sel, AnimProperty::Position, 0, x_r, "mo-x", Prefix::Text("X"));
    num_cell(app, ui, sel, AnimProperty::Position, 1, y_r, "mo-y", Prefix::Text("Y"));
    diamond_for(app, ui, sel, AnimProperty::Position, d_r);
    row_end(pane);
    sublabel(ui, pane, "Scale");
    let (x_r, y_r, d_r) = (pane.rect(17.0, 0.0, 68.0, 24.0), pane.rect(89.0, 0.0, 68.0, 24.0), pane.rect(161.0, 0.0, 24.0, 24.0));
    num_cell(app, ui, sel, AnimProperty::Scale, 0, x_r, "mo-sx", Prefix::Icon(Icon::PaddingH));
    num_cell(app, ui, sel, AnimProperty::Scale, 1, y_r, "mo-sy", Prefix::Icon(Icon::PaddingV));
    diamond_for(app, ui, sel, AnimProperty::Scale, d_r);
    {
        let r = pane.rect(theme::SECTION_TRAILING_X, 0.0, theme::PANEL_ACTION_BUTTON, theme::PANEL_ACTION_BUTTON);
        widgets::record(ui, "mo-scale-lock", r);
        let on = app.motion_ins.scale_locked();
        let (icon, tip) = if on { (Icon::Link, "Unlock proportions") } else { (Icon::Unlink, "Lock proportions") };
        if widgets::icon_button(ui, r, "mo-scale-lock", icon, on, tip).clicked() {
            app.motion_ins.scale_unlocked = on;
        }
    }
    row_end(pane);
    sublabel(ui, pane, "Rotation");
    let (r_r, d_r) = (pane.rect(17.0, 0.0, 60.0, 24.0), pane.rect(79.0, 0.0, 24.0, 24.0));
    num_cell(app, ui, sel, AnimProperty::Rotation, 0, r_r, "mo-rot", Prefix::Icon(Icon::Rotation));
    diamond_for(app, ui, sel, AnimProperty::Rotation, d_r);
    row_end(pane);
    pane.y += 4.0;

    // ---- Layout
    section_sep(ui, pane, false);
    title(ui, pane, "Layout", None, false);
    sublabel(ui, pane, "Dimensions");
    let (w_r, wd_r) = (pane.rect(17.0, 0.0, 60.0, 24.0), pane.rect(79.0, 0.0, 24.0, 24.0));
    let (h_r, hd_r) = (pane.rect(113.0, 0.0, 60.0, 24.0), pane.rect(175.0, 0.0, 24.0, 24.0));
    num_cell(app, ui, sel, AnimProperty::Width, 0, w_r, "mo-w", Prefix::Text("W"));
    diamond_for(app, ui, sel, AnimProperty::Width, wd_r);
    num_cell(app, ui, sel, AnimProperty::Height, 0, h_r, "mo-h", Prefix::Text("H"));
    diamond_for(app, ui, sel, AnimProperty::Height, hd_r);
    row_end(pane);
    pane.y += 4.0;

    // ---- Appearance
    section_sep(ui, pane, false);
    title(ui, pane, "Appearance", None, false);
    let has_radius = !matches!(n0.kind, NodeKind::Text(_) | NodeKind::Line | NodeKind::Group | NodeKind::Vector { .. });
    sublabel_row(ui, pane, &[(17.0, "Opacity"), (113.0, if has_radius { "Corner radius" } else { "" })]);
    let (o_r, od_r) = (pane.rect(17.0, 0.0, 60.0, 24.0), pane.rect(79.0, 0.0, 24.0, 24.0));
    num_cell(app, ui, sel, AnimProperty::Opacity, 0, o_r, "mo-op", Prefix::Icon(Icon::Opacity));
    diamond_for(app, ui, sel, AnimProperty::Opacity, od_r);
    if has_radius {
        let (c_r, cd_r) = (pane.rect(113.0, 0.0, 60.0, 24.0), pane.rect(175.0, 0.0, 24.0, 24.0));
        num_cell(app, ui, sel, AnimProperty::CornerRadius, 0, c_r, "mo-radius", Prefix::Icon(Icon::Radius));
        diamond_for(app, ui, sel, AnimProperty::CornerRadius, cd_r);
    }
    row_end(pane);
    pane.y += 4.0;

    // ---- Fill
    if !matches!(n0.kind, NodeKind::Text(_)) && !matches!(n0.kind, NodeKind::Line) {
        section_sep(ui, pane, false);
        if properties::paint_title(app, ui, pane, sel, "Fill", false, n0.fills.is_empty()) {
            let paint = desigma_doc::Paint::solid(Color::from_hex(&format!("{:06X}", theme::DEFAULT_SHAPE_FILL)).unwrap_or(Color::WHITE));
            app.edit_nodes(sel, |n| n.fills.push(paint.clone()));
        }
        // every row is the shared paint row; the first solid one carries the colour diamond
        properties::paint_rows(app, ui, pane, sel, &n0.fills, false);
        if !n0.fills.is_empty() {
            pane.y += 8.0;
        }
    }

    // ---- Stroke / Effects / Export (static)
    section_sep(ui, pane, false);
    stroke::section(app, ui, pane, sel, &n0);
    section_sep(ui, pane, false);
    effects::section(app, ui, pane, sel, &n0.effects);
    section_sep(ui, pane, false);
    pane.y += export::section(app, ui, pane.x0, pane.oy + pane.y, sel);
}

/// Small labels above a row of cells.
fn sublabel_row(ui: &Ui, pane: &mut Pane, labels: &[(f32, &str)]) {
    for (x, text) in labels {
        let r = pane.rect(*x, 0.0, 90.0, theme::SUBLABEL_HEIGHT);
        ui.painter().text(Pos2::new(r.left(), r.center().y), Align2::LEFT_CENTER, text, theme::font_small_medium(), theme::TEXT_SECONDARY);
    }
    pane.y += theme::SUBLABEL_HEIGHT + 4.0;
}

// ---------------------------------------------------------------- Animations section

pub(super) fn clip_icon(kind: ClipKind) -> Icon {
    match kind {
        ClipKind::Position => Icon::MoveArrows,
        ClipKind::Scale => Icon::Maximize,
        ClipKind::Rotation => Icon::Rotation,
        ClipKind::Size => Icon::Scale,
        ClipKind::Opacity => Icon::Opacity,
    }
}

fn animations_section(app: &mut App, ui: &mut Ui, pane: &mut Pane, sel: &[NodeId], region: Rect) {
    title(ui, pane, "Animations", None, false);
    let y = -theme::SECTION_TITLE_HEIGHT + 8.0;
    let plus = pane.rect(theme::SECTION_TRAILING_X, y, theme::PANEL_ACTION_BUTTON, theme::PANEL_ACTION_BUTTON);
    let lib = pane.rect(theme::SECTION_TRAILING_X - 28.0, y, theme::PANEL_ACTION_BUTTON, theme::PANEL_ACTION_BUTTON);
    widgets::record(ui, "mo-presets", lib);
    app.motion_ins.library_anchor = Some(lib);
    widgets::record(ui, "mo-add-clip", plus);
    let lib_open = app.motion_ins.library.is_some();
    if widgets::icon_button(ui, lib, "mo-presets", Icon::GridView, lib_open, "Animation presets").clicked() {
        if lib_open {
            app.motion_ins.library = None;
        } else {
            open_library(app);
        }
    }
    if widgets::icon_button(ui, plus, "mo-add-clip", Icon::Plus, false, "Add animation").clicked() {
        add_default_clip(app, sel);
    }
    let Some(node0) = sel.first().copied() else { return };
    let clips: Vec<Clip> = app.doc.get(node0).map(|n| n.motion.clips.clone()).unwrap_or_default();
    if clips.is_empty() {
        return;
    }
    pane.y += 0.0;
    for c in &clips {
        let row = pane.rect(theme::SWATCH_ROW_X, 0.0, 156.0, theme::FIELD_HEIGHT);
        widgets::record(ui, &format!("mo-clip:{}", c.id), row);
        let resp = ui.interact(row, ui.id().with(("mo-clip", c.id)), Sense::click());
        let selected = app.motion.selected_clip == Some((node0, c.id));
        {
            let p = ui.painter();
            p.rect_filled(row, theme::cr(theme::RADIUS_MEDIUM), if resp.hovered() { theme::BG_TERTIARY } else { theme::BG_SECONDARY });
            if selected {
                p.rect_stroke(row, theme::cr(theme::RADIUS_MEDIUM), Stroke::new(1.0, theme::BORDER_SELECTED), StrokeKind::Inside);
            }
            let dim = if c.visible { 1.0 } else { 0.45 };
            icons::paint(p, clip_icon(c.kind), Pos2::new(row.left() + 16.0, row.center().y), 14.0, theme::ICON_SECONDARY.gamma_multiply(dim));
            p.text(Pos2::new(row.left() + 32.0, row.center().y), Align2::LEFT_CENTER, c.kind.name(), theme::font_body(), theme::TEXT.gamma_multiply(dim));
        }
        let eye_r = pane.rect(theme::RIGHT_NAME_ACTION_X[1] - 1.0 + 4.0, 0.0, theme::PANEL_ACTION_BUTTON, theme::PANEL_ACTION_BUTTON);
        let minus_r = pane.rect(theme::SECTION_TRAILING_X, 0.0, theme::PANEL_ACTION_BUTTON, theme::PANEL_ACTION_BUTTON);
        widgets::record(ui, &format!("mo-clip-eye:{}", c.id), eye_r);
        widgets::record(ui, &format!("mo-clip-del:{}", c.id), minus_r);
        let mut consumed = false;
        if widgets::icon_button(ui, eye_r, ("mo-clip-eye", c.id), if c.visible { Icon::Eye } else { Icon::EyeOff }, false, "Toggle animation visibility")
            .clicked()
        {
            let mut nc = c.clone();
            nc.visible = !nc.visible;
            run_all(app, vec![edit::update_clip(&app.doc, node0, nc)]);
            consumed = true;
        }
        if widgets::icon_button(ui, minus_r, ("mo-clip-del", c.id), Icon::Minus, false, "Remove animation").clicked() {
            let cmd = edit::remove_clip(&app.doc, node0, c.id);
            run_all(app, vec![cmd]);
            if app.motion.selected_clip == Some((node0, c.id)) {
                app.motion.selected_clip = None;
            }
            consumed = true;
        }
        if resp.clicked() && !consumed {
            if selected {
                app.motion.selected_clip = None;
            } else {
                app.motion_ins.clip_anchor = Some(row);
                open_clip(app, node0, c.id);
            }
        }
        pane.y += theme::FIELD_HEIGHT + 4.0;
    }
    pane.y += 4.0;
    let _ = region;
}

/// "+" of the Animations section: a default Position clip at the playhead on every selected node.
fn add_default_clip(app: &mut App, sel: &[NodeId]) {
    let start = app.motion.playhead_ms;
    let mut cmds = Vec::new();
    let mut last = None;
    for id in sel {
        let clip = Clip::new(ClipKind::Position, ClipMode::In).delay(start);
        match edit::add_clip(&app.doc, *id, clip) {
            Ok((c, cid)) => {
                cmds.push(Ok(c));
                last = Some((*id, cid));
            }
            Err(e) => cmds.push(Err(e)),
        }
    }
    if run_all(app, cmds) {
        if let Some((id, cid)) = last {
            if let Some(r) = app.motion_ins.clip_anchor {
                app.motion_ins.clip_anchor = Some(r);
            }
            open_clip(app, id, cid);
        }
    }
}

// ---------------------------------------------------------------- floating panels

/// A floating panel (library, clip settings, preset menu): elevation shadow, dark body, ring.
/// Returns true when a press landed outside `rect` (and outside `anchor` and other popups).
pub(super) fn float(ctx: &egui::Context, id: Id, rect: Rect, anchor: Option<Rect>, body: impl FnOnce(&mut Ui)) -> bool {
    let area = egui::Area::new(id).order(egui::Order::Foreground).fixed_pos(rect.min).show(ctx, |ui| {
        ui.allocate_rect(rect, Sense::hover());
        {
            let p = ui.painter();
            widgets::elevation_400(p, rect, theme::RADIUS_LARGE);
            p.rect_filled(rect, theme::cr(theme::RADIUS_LARGE), theme::BG);
            p.rect_stroke(rect, theme::cr(theme::RADIUS_LARGE), Stroke::new(1.0, theme::ELEVATION_400_RING), StrokeKind::Inside);
        }
        body(ui);
    });
    let layer = area.response.layer_id;
    let (pressed, ptr) = ctx.input(|i| (i.pointer.any_pressed(), i.pointer.interact_pos()));
    let other_popup = |p: Pos2| ctx.layer_id_at(p).is_some_and(|l| l.order == egui::Order::Foreground && l != layer);
    pressed && ptr.is_some_and(|p| !rect.contains(p) && !anchor.is_some_and(|a| a.contains(p)) && !other_popup(p))
}

/// Header row of a floating panel: bold title at the left, `trailing` buttons from the right.
pub(super) fn float_title(ui: &mut Ui, rect: Rect, text: &str) -> Rect {
    let tr = Rect::from_min_size(rect.min, Vec2::new(rect.width(), 40.0));
    let font = theme::font_body_strong();
    widgets::text_on_baseline(ui.painter(), rect.left() + 16.0, widgets::centered_baseline(tr, &font), text, font, theme::TEXT);
    tr
}

pub(super) fn default_ease(family_spring: bool) -> Ease {
    if family_spring {
        Ease::spring(motion::DEFAULT_BOUNCE)
    } else {
        Ease::Linear
    }
}

/// Screenshot presets (`--panel motion-*`): a layer with a keyframed move, selected in Motion
/// mode, with the easing panel, the library or a clip popover open.
pub fn preset(app: &mut App, name: &str) {
    use desigma_doc::{Node, Rect as DRect};
    let page = app.page;
    let id = app.doc.alloc_id();
    let mut n = Node::rectangle(id, DRect::new(300.0, 200.0, 188.0, 188.0));
    n.name = "Rectangle".into();
    let pos = n.motion.ensure_track(AnimProperty::Position);
    pos.set_key(0, AnimValue::Vec2(DVec2::new(300.0, 200.0)), Some(Ease::bezier(0.07, 0.97, 0.58, 1.0)));
    pos.set_key(1000, AnimValue::Vec2(DVec2::new(500.0, 95.0)), None);
    let rot = n.motion.ensure_track(AnimProperty::Rotation);
    rot.set_key(0, AnimValue::Scalar(0.0), None);
    rot.set_key(1000, AnimValue::Scalar(90.0), None);
    if name == "motion-clip" || name == "motion-library" {
        n.motion.clips = motion::find_preset("Burst").map(|p| p.instantiate(997, 0)).unwrap_or_default();
    }
    if app.doc.insert(page, n).is_err() {
        return;
    }
    app.selection = vec![id];
    app.motion.enabled = true;
    app.motion.playhead_ms = 1000;
    app.view.offset = DVec2::new(200.0, 0.0);
    match name {
        "motion-easing" => open_easing(app, vec![KeyRef::new(id, AnimProperty::Position, 0)]),
        "motion-library" => open_library(app),
        "motion-clip" => open_clip(app, id, 0),
        _ => {}
    }
}
