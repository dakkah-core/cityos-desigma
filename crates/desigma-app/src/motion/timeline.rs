//! The Motion timeline panel (Figma "Motion (Beta)"): header controls, ruler and playhead, layer /
//! property / clip rows, keyframe diamonds with selection, dragging and easing-segment selection.
//!
//! The panel docks under the canvas ([`split`] carves its rect out of the canvas area). All of its
//! own UI state (height, zoom, scroll, time format, folded layers, the drag in progress, the
//! keyframe clipboard) lives in [`TimelineUi`], kept in egui's temp data so it does not touch the
//! shared [`MotionUi`](super::MotionUi) contract. Edits go through `desigma_doc::motion::edit`
//! helpers (or snapshot `SetNodeMotion` commands built the same way), one undo step per gesture.

use crate::app::App;
use crate::icons::{self, Icon};
use crate::panels::layers::kind_icon;
use crate::theme;
use crate::widgets::{self, field};
use desigma_doc::motion::edit::{self, KeyRef};
use desigma_doc::motion::{self, AnimProperty, AnimValue, Clip, ClipRepeat, Ease, TimelineSettings};
use desigma_doc::{Command, Document, NodeId};
use egui::{Align2, Color32, Context, Id, Key, Painter, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, Vec2};
use std::collections::BTreeSet;

// ---- metrics ----------------------------------------------------------------------------------

/// Header row (controls + ruler).
pub const HEADER_H: f32 = 34.0;
/// Height of a layer / clip / property row.
pub const ROW_H: f32 = 17.0;
/// Width of the layer column.
pub const LEFT_W: f32 = 205.0;
/// Space between the left edge of the track area and t = 0.
pub const TIME_PAD: f32 = 13.0;
const DEFAULT_H: f32 = 220.0;
const MIN_H: f32 = HEADER_H + ROW_H * 3.0;
const DIAMOND_R: f32 = 4.5;
const KEY_HIT: f32 = 6.0;
const EDGE_HIT: f32 = 5.0;
const DRAG_SLOP: f32 = 3.0;
const MIN_ZOOM: f32 = 0.05;
const MAX_ZOOM: f32 = 200.0;

const PLAYHEAD: Color32 = theme::CANVAS_SELECTION;
const SUMMARY: Color32 = theme::BG_BRAND;
const CLIP_FILL: Color32 = theme::rgba(0x0C8CE9, 0x5C);
const OUTSIDE: Color32 = theme::rgba(0x000000, 0x4D);
const ROW_SELECTED: Color32 = theme::BG_SELECTED;

// ---- state ------------------------------------------------------------------------------------

/// How times are written in the header fields (the "Change time format" button cycles them).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeFormat {
    Ms,
    Seconds,
    Frames,
}

impl TimeFormat {
    fn next(self) -> Self {
        match self {
            TimeFormat::Ms => TimeFormat::Seconds,
            TimeFormat::Seconds => TimeFormat::Frames,
            TimeFormat::Frames => TimeFormat::Ms,
        }
    }
    fn unit(self) -> &'static str {
        match self {
            TimeFormat::Ms => "ms",
            TimeFormat::Seconds => "s",
            TimeFormat::Frames => "f",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum ClipEdge {
    Move,
    TrimStart,
    TrimEnd,
}

#[derive(Clone, Debug)]
enum Drag {
    /// Scrubbing the playhead on the ruler.
    Scrub,
    /// Dragging the end of the timeline (new duration in ms).
    Duration { new_ms: u32 },
    /// Moving keyframes (or duplicating them with Alt): `anchor` is the grabbed key's time.
    Keys { origin: Pos2, anchor: u32, active: bool, dup: bool, delta: i64, base: Vec<KeyRef>, shifted: bool, pressed: KeyRef },
    /// Alt-dragging the edge of a selection to time-stretch it about `pivot`.
    Stretch { origin: Pos2, pivot: u32, edge: u32, factor: f32, active: bool, base: Vec<KeyRef> },
    /// Moving or trimming a clip bar.
    Clip { node: NodeId, edge: ClipEdge, origin: Pos2, orig: Clip, cur: Clip, active: bool },
    /// Dragging the blue summary bar of a layer: everything it animates moves by `delta` ms. `lo`
    /// is the start of the bar when the drag began.
    Summary { node: NodeId, origin: Pos2, lo: u32, active: bool, delta: i64 },
    /// Rubber-band selection in the track area.
    Marquee { origin: Pos2, cur: Pos2, base: Vec<KeyRef> },
    /// Resizing the panel by its top edge.
    Resize { origin_y: f32, h0: f32 },
}

/// A copied keyframe: where it was, relative to the earliest copied key.
#[derive(Clone, Debug)]
struct CopiedKey {
    node: NodeId,
    property: AnimProperty,
    rel_ms: u32,
    value: AnimValue,
    easing: Ease,
}

/// UI state of the timeline panel (not part of the document).
#[derive(Clone)]
pub struct TimelineUi {
    /// Height of the expanded panel in px.
    pub height: f32,
    /// "Collapse Motion timeline": only the header row stays.
    pub collapsed: bool,
    /// Zoom factor; 1 fits the timeline duration into the panel.
    pub zoom: f32,
    pub scroll_x: f32,
    pub scroll_y: f32,
    pub format: TimeFormat,
    /// Layers whose property rows are folded.
    pub folded: BTreeSet<NodeId>,
    drag: Option<Drag>,
    clipboard: Vec<CopiedKey>,
    /// The last copy was keyframes (Ctrl+V pastes keyframes instead of layers).
    keys_active: bool,
}

impl Default for TimelineUi {
    fn default() -> Self {
        Self {
            height: DEFAULT_H,
            collapsed: false,
            zoom: 1.0,
            scroll_x: 0.0,
            scroll_y: 0.0,
            format: TimeFormat::Ms,
            folded: BTreeSet::new(),
            drag: None,
            clipboard: Vec::new(),
            keys_active: false,
        }
    }
}

fn state_id() -> Id {
    Id::new("desigma-motion-timeline")
}

pub fn load(ctx: &Context) -> TimelineUi {
    ctx.data(|d| d.get_temp::<TimelineUi>(state_id())).unwrap_or_default()
}

fn save(ctx: &Context, st: TimelineUi) {
    ctx.data_mut(|d| d.insert_temp(state_id(), st));
}

/// Height the panel takes now.
pub fn panel_height(st: &TimelineUi, canvas_h: f32) -> f32 {
    if st.collapsed {
        HEADER_H + 1.0
    } else {
        st.height.clamp(MIN_H, (canvas_h * 0.7).max(MIN_H))
    }
}

/// Splits the canvas area into the part the canvas keeps and the timeline panel under it.
pub fn split(ctx: &Context, area: Rect) -> (Rect, Rect) {
    let st = load(ctx);
    let h = panel_height(&st, area.height()).min(area.height());
    let top = area.bottom() - h;
    (Rect::from_min_max(area.min, Pos2::new(area.right(), top)), Rect::from_min_max(Pos2::new(area.left(), top), area.max))
}

// ---- time helpers -----------------------------------------------------------------------------

pub fn fmt_time(ms: u32, fmt: TimeFormat, fps: u32) -> String {
    match fmt {
        TimeFormat::Ms => format!("{ms:04}"),
        TimeFormat::Seconds => format!("{:.3}", ms as f32 / 1000.0),
        TimeFormat::Frames => format!("{}", (ms as f32 * fps as f32 / 1000.0).round() as u32),
    }
}

pub fn parse_time(s: &str, fmt: TimeFormat, fps: u32) -> Option<u32> {
    let s = s.trim().trim_end_matches(|c: char| c.is_alphabetic()).trim();
    let v: f32 = s.parse().ok()?;
    if !v.is_finite() || v < 0.0 {
        return None;
    }
    Some(match fmt {
        TimeFormat::Ms => v.round() as u32,
        TimeFormat::Seconds => (v * 1000.0).round() as u32,
        TimeFormat::Frames => (v * 1000.0 / fps.max(1) as f32).round() as u32,
    })
}

/// Ruler label: whole seconds as "1s", anything else as the milliseconds past the last second
/// ("500ms"), as Figma writes them.
pub fn ruler_label(ms: u32) -> String {
    if ms.is_multiple_of(1000) {
        format!("{}s", ms / 1000)
    } else {
        format!("{}ms", ms % 1000)
    }
}

/// Distance between labelled ticks for `pxms` pixels per millisecond (labels stay >= 100 px apart).
pub fn tick_step(pxms: f32) -> u32 {
    const STEPS: [u32; 18] = [1, 2, 5, 10, 20, 50, 100, 200, 250, 500, 1000, 2000, 5000, 10_000, 30_000, 60_000, 120_000, 600_000];
    STEPS.iter().copied().find(|s| *s as f32 * pxms >= 100.0).unwrap_or(600_000)
}

/// Text of a property's value as the row shows it ("200, -105", "90°", "30%", "D9D9D9").
pub fn value_text(property: AnimProperty, v: AnimValue) -> String {
    let f = property.display_factor();
    let unit = match property {
        AnimProperty::Opacity | AnimProperty::Scale => "%",
        AnimProperty::Rotation => "°",
        _ => "",
    };
    match v {
        AnimValue::Scalar(x) => format!("{}{unit}", widgets::fmt_num(x * f)),
        AnimValue::Vec2(v) => format!("{}, {}{unit}", widgets::fmt_num(v.x * f), widgets::fmt_num(v.y * f)),
        AnimValue::Color(c) => c.to_hex(),
    }
}

// ---- geometry ---------------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Geo {
    tl: Rect,
    /// Ruler (header row right of the layer column).
    ruler: Rect,
    /// Layer column body.
    left: Rect,
    /// Track area body.
    tracks: Rect,
    pxms: f32,
    scroll_x: f32,
    scroll_y: f32,
    duration: u32,
    fps: u32,
}

impl Geo {
    fn x(&self, t_ms: f32) -> f32 {
        self.ruler.left() + TIME_PAD + t_ms * self.pxms - self.scroll_x
    }
    fn t(&self, x: f32) -> f32 {
        (x - self.ruler.left() - TIME_PAD + self.scroll_x) / self.pxms
    }
    fn row_top(&self, i: usize) -> f32 {
        self.tracks.top() + i as f32 * ROW_H - self.scroll_y
    }
    fn row_at(&self, y: f32) -> Option<usize> {
        let r = ((y - self.tracks.top() + self.scroll_y) / ROW_H).floor();
        (r >= 0.0).then_some(r as usize)
    }
    fn snap(&self, t: f32) -> u32 {
        edit::to_ms(TimelineSettings { fps: self.fps, ..Default::default() }.snap_to_frame(t.max(0.0)))
    }
    fn end_x(&self) -> f32 {
        self.x(self.duration as f32)
    }
    fn help(&self) -> Rect {
        Rect::from_min_size(self.tl.right_bottom() - Vec2::new(12.0 + 24.0, 12.0 + 24.0), Vec2::splat(24.0))
    }
    fn pill(&self) -> Rect {
        let h = self.help();
        Rect::from_min_max(Pos2::new(h.left() - 8.0 - 56.0, h.top()), Pos2::new(h.left() - 8.0, h.bottom()))
    }
}

fn geometry(st: &TimelineUi, rect: Rect, doc: &Document, page: NodeId) -> Geo {
    let tl = doc.timeline(page);
    let ruler = Rect::from_min_max(Pos2::new(rect.left() + LEFT_W, rect.top()), Pos2::new(rect.right(), rect.top() + HEADER_H));
    let body_top = rect.top() + HEADER_H + 1.0;
    let left = Rect::from_min_max(Pos2::new(rect.left(), body_top), Pos2::new(rect.left() + LEFT_W, rect.bottom()));
    let tracks = Rect::from_min_max(Pos2::new(rect.left() + LEFT_W, body_top), rect.max);
    let fit = ((ruler.width() - TIME_PAD - 84.0).max(40.0)) / tl.duration_ms.max(1) as f32;
    Geo { tl: rect, ruler, left, tracks, pxms: fit * st.zoom, scroll_x: st.scroll_x, scroll_y: st.scroll_y, duration: tl.duration_ms, fps: tl.fps }
}

// ---- rows -------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Row {
    Layer(NodeId),
    Clip(NodeId, u32),
    Prop(NodeId, AnimProperty),
}

/// Nodes shown in the timeline: every node of the page with animation, plus the selected ones.
fn timeline_nodes(app: &App) -> Vec<NodeId> {
    let mut nodes = motion::animated_nodes(&app.doc, app.page);
    nodes.retain(|n| *n != app.page);
    for s in &app.selection {
        if !nodes.contains(s) && app.doc.get(*s).is_some() {
            nodes.push(*s);
        }
    }
    nodes
}

fn build_rows(app: &App, st: &TimelineUi) -> Vec<Row> {
    let mut rows = Vec::new();
    for id in timeline_nodes(app) {
        rows.push(Row::Layer(id));
        if st.folded.contains(&id) {
            continue;
        }
        let Some(n) = app.doc.get(id) else { continue };
        rows.extend(n.motion.clips.iter().map(|c| Row::Clip(id, c.id)));
        rows.extend(n.motion.tracks.iter().filter(|t| !t.is_empty()).map(|t| Row::Prop(id, t.property)));
    }
    rows
}

fn content_h(rows: &[Row]) -> f32 {
    rows.len() as f32 * ROW_H
}

// ---- document edits ---------------------------------------------------------------------------

/// Runs one command as one undo step; false (with a toast) when it fails.
fn exec(app: &mut App, cmd: Command) -> bool {
    if app.history.viewing.is_some() {
        app.show_toast("Viewing an old version is read-only", true);
        return false;
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

fn batch(mut cmds: Vec<Command>) -> Option<Command> {
    match cmds.len() {
        0 => None,
        1 => cmds.pop(),
        _ => Some(Command::Batch(cmds)),
    }
}

/// Inserts keyframes (value + easing at absolute times), one `SetNodeMotion` per node.
fn insert_keys(doc: &Document, items: &[(NodeId, AnimProperty, u32, AnimValue, Ease)]) -> Option<(Command, Vec<KeyRef>)> {
    let nodes: BTreeSet<NodeId> = items.iter().map(|i| i.0).collect();
    let mut cmds = Vec::new();
    let mut refs = Vec::new();
    for node in nodes {
        let n = doc.get(node)?;
        let mut m = n.motion.clone();
        let mut changed = false;
        for (_, p, t, v, e) in items.iter().filter(|i| i.0 == node) {
            if motion::base_value(n, *p).is_none() || !p.accepts(v) {
                continue;
            }
            m.ensure_track(*p).set_key(*t, *v, Some(*e));
            refs.push(KeyRef::new(node, *p, *t));
            changed = true;
        }
        if changed {
            m.normalize();
            cmds.push(Command::SetNodeMotion { node, motion: Box::new(m) });
        }
    }
    Some((batch(cmds)?, refs))
}

/// The header's add-keyframe button: keys at the playhead for the selected layers' animated
/// properties (or Position when they have none).
pub fn add_keyframes_at_playhead(app: &mut App) -> bool {
    let t = app.motion.playhead_ms;
    let mut nodes: Vec<NodeId> = app.selection.iter().copied().filter(|n| app.doc.get(*n).is_some()).collect();
    if nodes.is_empty() {
        nodes = app.motion.selected_keys.iter().map(|k| k.node).collect::<BTreeSet<_>>().into_iter().collect();
    }
    if nodes.is_empty() {
        app.show_toast("Select a layer to add a keyframe", false);
        return false;
    }
    let mut items = Vec::new();
    for node in nodes {
        let Some(n) = app.doc.get(node) else { continue };
        let mut props: Vec<AnimProperty> = n.motion.tracks.iter().map(|t| t.property).collect();
        if props.is_empty() {
            props.push(AnimProperty::Position);
        }
        for p in props {
            if let Some(v) = motion::keyframed_value_at(n, p, t as f32) {
                let ease = n.motion.track(p).and_then(|tr| tr.key_at(t)).map(|k| k.easing);
                items.push((
                    node,
                    p,
                    t,
                    v,
                    ease.unwrap_or_else(|| n.motion.track(p).and_then(|tr| tr.segment_start(t as f32).map(|i| tr.keys[i].easing)).unwrap_or(Ease::Linear)),
                ));
            }
        }
    }
    let Some((cmd, refs)) = insert_keys(&app.doc, &items) else { return false };
    if exec(app, cmd) {
        app.motion.selected_keys = refs;
        app.motion.easing_target = false;
        true
    } else {
        false
    }
}

fn delete_selection(app: &mut App) -> bool {
    if !app.motion.selected_keys.is_empty() {
        let keys = std::mem::take(&mut app.motion.selected_keys);
        app.motion.easing_target = false;
        if let Ok(cmd) = edit::remove_keyframes(&app.doc, &keys) {
            return exec(app, cmd);
        }
        return false;
    }
    if let Some((node, id)) = app.motion.selected_clip.take() {
        if let Ok(cmd) = edit::remove_clip(&app.doc, node, id) {
            return exec(app, cmd);
        }
    }
    false
}

fn copy_selected(app: &App) -> Vec<CopiedKey> {
    let mut out = Vec::new();
    for k in &app.motion.selected_keys {
        let Some(key) = app.doc.get(k.node).and_then(|n| n.motion.track(k.property)).and_then(|t| t.key_at(k.time_ms)) else { continue };
        out.push(CopiedKey { node: k.node, property: k.property, rel_ms: k.time_ms, value: key.value, easing: key.easing });
    }
    let min = out.iter().map(|c| c.rel_ms).min().unwrap_or(0);
    for c in &mut out {
        c.rel_ms -= min;
    }
    out
}

/// Ctrl+C with keyframes selected: copies them (returns true so layers are not copied).
pub fn copy_keys(app: &mut App, ctx: &Context) -> bool {
    if !app.motion.enabled {
        return false;
    }
    let mut st = load(ctx);
    let ok = !app.motion.selected_keys.is_empty();
    if ok {
        st.clipboard = copy_selected(app);
    }
    st.keys_active = ok;
    save(ctx, st);
    ok
}

/// Ctrl+V after copying keyframes: pastes them at the playhead (onto the selected layer when a
/// single layer was copied, else onto the layers they came from).
pub fn paste_keys(app: &mut App, ctx: &Context) -> bool {
    if !app.motion.enabled {
        return false;
    }
    let st = load(ctx);
    if !st.keys_active || st.clipboard.is_empty() {
        return false;
    }
    let sources: BTreeSet<NodeId> = st.clipboard.iter().map(|c| c.node).collect();
    let target = (sources.len() == 1 && app.selection.len() == 1).then(|| app.selection[0]);
    let t0 = app.motion.playhead_ms;
    let items: Vec<_> = st.clipboard.iter().map(|c| (target.unwrap_or(c.node), c.property, t0 + c.rel_ms, c.value, c.easing)).collect();
    if let Some((cmd, refs)) = insert_keys(&app.doc, &items) {
        if exec(app, cmd) {
            app.motion.selected_keys = refs;
            app.motion.easing_target = false;
        }
    }
    true
}

/// `--motion`: enters Motion mode and animates the selected layer (position, rotation and opacity
/// keys at 0 and 1s plus a Burst preset) with the playhead near the second key, for screenshots.
pub fn open_demo(app: &mut App) {
    app.motion.enabled = true;
    app.ui.right_tab = Some(crate::app::RightTab::Design);
    // a document that is already animated (the built-in onboarding demo) is shown as it is
    if open_animated(app) {
        return;
    }
    let Some(id) = app.selection.first().copied().or_else(|| app.doc.get(app.page).and_then(|p| p.children.first().copied())) else { return };
    app.select_only(id);
    let Some(n) = app.doc.get(id) else { return };
    let pos = motion::base_position(n);
    let steps = [
        (AnimProperty::Position, 0, None),
        (AnimProperty::Position, 1000, Some(AnimValue::Vec2(desigma_doc::Vec2::new(pos.x + 200.0, pos.y - 105.0)))),
        (AnimProperty::Rotation, 0, None),
        (AnimProperty::Rotation, 1000, Some(AnimValue::Scalar(90.0))),
        (AnimProperty::Opacity, 0, None),
        (AnimProperty::Opacity, 1000, Some(AnimValue::Scalar(0.3))),
    ];
    for (p, t, v) in steps {
        if let Ok(cmd) = edit::add_keyframe(&app.doc, id, p, t, v, None) {
            exec(app, cmd);
        }
    }
    if let Some(preset) = motion::find_preset("Burst") {
        if let Ok(cmd) = edit::apply_preset(&app.doc, &[id], preset, 1000) {
            exec(app, cmd);
        }
    }
    app.motion.playhead_ms = 997;
}

/// Motion mode on a document that already has animation: its first animated page, the first
/// animated top-level frame selected, the playhead where most of the animation is on screen.
fn open_animated(app: &mut App) -> bool {
    let animated = |app: &App, root: NodeId| {
        let mut any = false;
        app.doc.walk(root, |n, _| any |= !n.motion.is_empty());
        any
    };
    let selected = app.selection.first().copied().and_then(|id| app.doc.page_of(id)).filter(|p| animated(app, *p));
    let Some(page) = selected.or_else(|| app.doc.pages.iter().copied().find(|p| animated(app, *p))) else { return false };
    app.page = page;
    if app.selection.is_empty() || app.doc.page_of(app.selection[0]) != Some(page) {
        let frame = app.doc.get(page).map(|p| p.children.clone()).unwrap_or_default().into_iter().find(|f| animated(app, *f));
        if let Some(f) = frame {
            app.select_only(f);
        }
    }
    let tl = app.doc.timeline(page);
    // late enough that every entrance has landed and the floating cycles are moving
    app.motion.playhead_ms = (tl.duration_ms as f32 * 0.7) as u32;
    true
}

// ---- keyboard ---------------------------------------------------------------------------------

/// Timeline shortcuts; true when the key was used. Called before the other canvas shortcuts while
/// Motion mode is on.
pub fn key(app: &mut App, key: Key, m: egui::Modifiers) -> bool {
    if m.command {
        return false;
    }
    let tl = app.doc.timeline(app.page);
    let t = app.motion.playhead_ms;
    match key {
        Key::Space if !m.shift && !m.alt => {
            toggle_play(app);
            true
        }
        Key::Comma | Key::Period => {
            let fwd = key == Key::Period;
            if m.shift {
                if let Some(k) = neighbour_key(app, t, fwd) {
                    seek(app, k);
                }
            } else {
                // step from the nearest frame so scrubbed times land on frame boundaries
                let step = tl.frame_ms();
                let idx = (t as f32 / step).round() as i64 + if fwd { 1 } else { -1 };
                seek(app, edit::to_ms(idx.max(0) as f32 * step));
            }
            true
        }
        Key::Home => {
            seek(app, 0);
            true
        }
        Key::End => {
            seek(app, tl.duration_ms);
            true
        }
        Key::Delete | Key::Backspace if !app.motion.selected_keys.is_empty() || app.motion.selected_clip.is_some() => {
            delete_selection(app);
            true
        }
        Key::Escape if !app.motion.selected_keys.is_empty() || app.motion.selected_clip.is_some() => {
            app.motion.selected_keys.clear();
            app.motion.selected_clip = None;
            app.motion.easing_target = false;
            true
        }
        _ => false,
    }
}

/// Nearest keyframe time before / after `t` over the selected layers (all animated layers when
/// nothing is selected).
fn neighbour_key(app: &App, t: u32, forward: bool) -> Option<u32> {
    let nodes = if app.selection.is_empty() { motion::animated_nodes(&app.doc, app.page) } else { app.selection.clone() };
    let times = nodes.iter().filter_map(|n| app.doc.get(*n)).filter_map(|n| if forward { n.motion.next_key_time(t) } else { n.motion.prev_key_time(t) });
    if forward {
        times.min()
    } else {
        times.max()
    }
}

fn seek(app: &mut App, ms: u32) {
    let d = app.doc.timeline(app.page).duration_ms;
    app.motion.playhead_ms = ms.min(d);
}

fn toggle_play(app: &mut App) {
    let tl = app.doc.timeline(app.page);
    if app.motion.playing {
        app.motion.playing = false;
    } else {
        if app.motion.playhead_ms >= tl.duration_ms && !tl.looping {
            app.motion.playhead_ms = 0;
        }
        app.motion.playing = true;
    }
}

// ---- painting helpers -------------------------------------------------------------------------

#[cfg(test)]
fn rec(ui: &Ui, name: impl FnOnce() -> String, rect: Rect) {
    widgets::record(ui, &name(), rect);
}
#[cfg(not(test))]
#[inline]
fn rec(_ui: &Ui, _name: impl FnOnce() -> String, _rect: Rect) {}

fn diamond(p: &Painter, c: Pos2, r: f32, fill: Color32, outline: Color32) {
    let pts = vec![Pos2::new(c.x, c.y - r), Pos2::new(c.x + r, c.y), Pos2::new(c.x, c.y + r), Pos2::new(c.x - r, c.y)];
    p.add(Shape::convex_polygon(pts, fill, Stroke::new(1.0, outline)));
}

fn mini_curve(p: &Painter, c: Pos2, ease: &Ease, col: Color32) {
    let (lo, hi) = ease.value_range();
    let span = (hi - lo).max(1e-3);
    let (w, h) = (10.0, 7.0);
    let pts: Vec<Pos2> = ease.sample(14).iter().map(|q| Pos2::new(c.x - w / 2.0 + q[0] * w, c.y + h / 2.0 - (q[1] - lo) / span * h)).collect();
    p.add(Shape::line(pts, Stroke::new(1.0, col)));
}

fn text(p: &Painter, pos: Pos2, align: Align2, s: &str, col: Color32) {
    p.text(pos, align, s, theme::font_body(), col);
}

// ---- panel ------------------------------------------------------------------------------------

/// Where the timeline panel was last drawn, so popovers elsewhere can treat clicks on it as inside.
pub fn last_rect(ctx: &egui::Context) -> Option<Rect> {
    ctx.data(|d| d.get_temp(egui::Id::new("motion-timeline-rect")))
}

pub fn show(app: &mut App, ui: &mut Ui, rect: Rect) {
    let ctx = ui.ctx().clone();
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("motion-timeline-rect"), rect));
    let mut st = load(&ctx);
    // the playhead is advanced by `playback::Playback::tick` (one clock for editing and export)
    sanitize(app);
    let tl = app.doc.timeline(app.page);
    if app.motion.playhead_ms > tl.duration_ms {
        app.motion.playhead_ms = tl.duration_ms;
    }
    let rows = build_rows(app, &st);
    let mut g = geometry(&st, rect, &app.doc, app.page);
    // clamp scrolling to the content
    let extent = content_extent(app, tl.duration_ms) as f32;
    let max_x = (TIME_PAD + extent * g.pxms + 40.0 - g.ruler.width()).max(0.0);
    st.scroll_x = st.scroll_x.clamp(0.0, max_x);
    let max_y = (content_h(&rows) - g.tracks.height()).max(0.0);
    st.scroll_y = st.scroll_y.clamp(0.0, max_y);
    g.scroll_x = st.scroll_x;
    g.scroll_y = st.scroll_y;

    let p = ui.painter().clone();
    p.rect_filled(rect, 0.0, theme::BG);
    rec(ui, || "motion-timeline".into(), rect);
    rec(ui, || "motion-ruler".into(), g.ruler);
    rec(ui, || "motion-scale".into(), Rect::from_min_size(Pos2::new(g.x(0.0), rect.top()), Vec2::new(g.pxms * 1000.0, 1.0)));

    // wheel / pinch over the panel
    wheel(ui, &mut st, &mut g, rect, extent);
    // resize by the top edge
    resize_handle(ui, &mut st, rect);

    if !st.collapsed {
        input(app, ui, &mut st, &g, &rows);
    } else {
        ruler_input(app, ui, &mut st, &g);
    }
    // the input may have changed the document, selection or rows
    let rows = build_rows(app, &st);
    let g = Geo { scroll_y: st.scroll_y, scroll_x: st.scroll_x, ..g };

    if !st.collapsed {
        paint_tracks(app, ui, &st, &g, &rows);
    }
    paint_ruler(app, ui, &st, &g);
    if !st.collapsed {
        left_column(app, ui, &mut st, &g, &rows);
    }
    header(app, ui, &mut st, &g, &rows);
    paint_playhead(app, ui, &st, &g);
    if !st.collapsed {
        let mut g2 = g;
        zoom_pill(ui, &mut st, &mut g2);
    }
    // top border
    widgets::hline(&p, rect.left(), rect.right(), rect.top(), theme::BORDER);
    save(&ctx, st);
}

/// Drops selections that no longer exist (after undo, delete, ...) and clears keyframe / clip
/// selection when the canvas is clicked.
fn sanitize(app: &mut App) {
    let doc = &app.doc;
    app.motion.selected_keys.retain(|k| doc.get(k.node).and_then(|n| n.motion.track(k.property)).is_some_and(|t| t.key_at(k.time_ms).is_some()));
    if app.motion.selected_keys.is_empty() {
        app.motion.easing_target = false;
    }
    if let Some((n, c)) = app.motion.selected_clip {
        if doc.get(n).is_none_or(|nd| nd.motion.clip(c).is_none()) {
            app.motion.selected_clip = None;
        }
    }
}

/// Latest time anything on the page is at (keyframes, clips) or the duration.
fn content_extent(app: &App, duration: u32) -> u32 {
    let mut e = duration;
    for id in motion::animated_nodes(&app.doc, app.page) {
        if let Some((_, hi)) = app.doc.get(id).and_then(|n| n.motion.span_ms(duration)) {
            e = e.max(hi);
        }
    }
    e
}

fn wheel(ui: &Ui, st: &mut TimelineUi, g: &mut Geo, rect: Rect, extent: f32) {
    let Some(pos) = ui.input(|i| i.pointer.latest_pos()).filter(|p| rect.contains(*p)) else { return };
    if widgets::pointer_over_popup(ui.ctx()) {
        return;
    }
    let (zoom, scroll, shift) = ui.input(|i| (i.zoom_delta(), i.smooth_scroll_delta, i.modifiers.shift));
    if (zoom - 1.0).abs() > 1e-4 && g.tracks.contains(pos) {
        zoom_about(st, g, pos.x, st.zoom * zoom);
    }
    let dx = scroll.x + if shift { scroll.y } else { 0.0 };
    let dy = if shift { 0.0 } else { scroll.y };
    if dx != 0.0 && (g.ruler.contains(pos) || g.tracks.contains(pos)) {
        st.scroll_x = (st.scroll_x - dx).max(0.0);
    }
    if dy != 0.0 && (g.left.contains(pos) || g.tracks.contains(pos)) {
        st.scroll_y = (st.scroll_y - dy).max(0.0);
    }
    let _ = extent;
}

/// Zooms keeping the time under screen x `anchor_x` in place.
fn zoom_about(st: &mut TimelineUi, g: &mut Geo, anchor_x: f32, new_zoom: f32) {
    let z = new_zoom.clamp(MIN_ZOOM, MAX_ZOOM);
    let t = g.t(anchor_x);
    let fit = g.pxms / st.zoom.max(1e-6);
    st.zoom = z;
    g.pxms = fit * z;
    st.scroll_x = (TIME_PAD + t * g.pxms - (anchor_x - g.ruler.left())).max(0.0);
    g.scroll_x = st.scroll_x;
}

fn resize_handle(ui: &mut Ui, st: &mut TimelineUi, rect: Rect) {
    if st.collapsed {
        return;
    }
    let strip = Rect::from_min_max(Pos2::new(rect.left(), rect.top() - 2.0), Pos2::new(rect.right(), rect.top() + 3.0));
    let r = ui.interact(strip, ui.id().with("motion-resize"), Sense::drag());
    if r.hovered() || r.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }
    if r.drag_started() {
        st.drag = Some(Drag::Resize { origin_y: ui.input(|i| i.pointer.press_origin().map_or(rect.top(), |p| p.y)), h0: rect.height() });
    }
    if let Some(Drag::Resize { origin_y, h0 }) = st.drag {
        if r.dragged() {
            if let Some(p) = ui.input(|i| i.pointer.latest_pos()) {
                st.height = (h0 + origin_y - p.y).max(MIN_H);
            }
        }
        if r.drag_stopped() {
            st.drag = None;
        }
    }
}

// ---- header -----------------------------------------------------------------------------------

fn hbtn(ui: &mut Ui, rect: Rect, key: &str, icon: Icon, selected: bool, tip: &str) -> bool {
    rec(ui, || format!("motion-{key}"), rect);
    widgets::icon_button(ui, rect, ("motion-btn", key), icon, selected, tip).clicked()
}

fn header(app: &mut App, ui: &mut Ui, st: &mut TimelineUi, g: &Geo, rows: &[Row]) {
    let p = ui.painter().clone();
    let top = g.tl.top();
    let cy = top + 17.0;
    let tl = app.doc.timeline(app.page);
    let at = |cx: f32, w: f32, h: f32| Rect::from_center_size(Pos2::new(g.tl.left() + cx, cy), Vec2::new(w, h));

    // play / pause
    let playing = app.motion.playing;
    if hbtn(ui, at(15.0, 22.0, 24.0), "play", if playing { Icon::Pause } else { Icon::Play }, false, if playing { "Pause   Space" } else { "Play   Space" }) {
        toggle_play(app);
    }
    // add keyframe
    if hbtn(ui, at(38.0, 22.0, 24.0), "add-key", Icon::Instance, false, "Add keyframe") {
        add_keyframes_at_playhead(app);
    }
    // current time and duration fields
    let fmt = st.format;
    let cur = at(68.0, 38.0, 20.0);
    let dur = at(108.0, 38.0, 20.0);
    rec(ui, || "motion-time".into(), cur);
    rec(ui, || "motion-duration".into(), dur);
    let shown = fmt_time(app.motion.playhead_ms, fmt, tl.fps);
    if let Some(s) = time_field(ui, cur, "motion-time-field", &shown) {
        if let Some(ms) = parse_time(&s, fmt, tl.fps) {
            seek(app, ms);
            app.motion.playing = false;
        }
    }
    let shown = fmt_time(tl.duration_ms, fmt, tl.fps);
    if let Some(s) = time_field(ui, dur, "motion-duration-field", &shown) {
        if let Some(ms) = parse_time(&s, fmt, tl.fps) {
            set_duration(app, ms);
        }
    }
    // time format
    let fr = at(137.0, 20.0, 20.0);
    rec(ui, || "motion-format".into(), fr);
    let fresp = ui.interact(fr, ui.id().with("motion-format"), Sense::click());
    widgets::button_bg(&p, fr, &fresp, false, theme::cr(theme::RADIUS_MEDIUM));
    text(&p, fr.center(), Align2::CENTER_CENTER, st.format.unit(), theme::TEXT_SECONDARY);
    if fresp.on_hover_text("Change time format").clicked() {
        st.format = st.format.next();
    }
    // loop
    if hbtn(ui, at(160.0, 22.0, 24.0), "loop", Icon::Repeat, tl.looping, "Loop") {
        if let Ok(cmd) = edit::set_timeline(&app.doc, app.page, TimelineSettings { looping: !tl.looping, ..tl }) {
            exec(app, cmd);
        }
    }
    // collapse / expand layers
    let any_open = rows.iter().any(|r| matches!(r, Row::Prop(..) | Row::Clip(..)));
    if hbtn(ui, at(190.0, 22.0, 24.0), "fold-layers", Icon::ChevronsDownUp, false, "Collapse layers") {
        if any_open {
            st.folded.extend(timeline_nodes(app));
        } else {
            st.folded.clear();
        }
    }
    // export the animation of the selected frame (or the page)
    let eb = Rect::from_center_size(Pos2::new(g.tl.right() - 46.0, cy), Vec2::new(24.0, 24.0));
    if hbtn(ui, eb, "export", Icon::Export, app.canvas.motion.export.running(), "Export animation") {
        export_from_timeline(app);
    }
    // collapse the whole panel
    let cb = Rect::from_center_size(Pos2::new(g.tl.right() - 18.0, cy), Vec2::new(24.0, 24.0));
    if hbtn(
        ui,
        cb,
        "collapse",
        if st.collapsed { Icon::ChevronRight } else { Icon::ChevronDown },
        false,
        if st.collapsed { "Expand Motion timeline" } else { "Collapse Motion timeline" },
    ) {
        st.collapsed = !st.collapsed;
    }
    // separator under the header
    widgets::hline(&p, g.tl.left(), g.tl.right(), top + HEADER_H, theme::BORDER);
    widgets::vline(&p, g.tl.left() + LEFT_W, top + 1.0, g.tl.bottom(), theme::BORDER);
}

/// The layer the timeline's export button animates: the top-level frame holding the selection,
/// else the page.
pub fn export_target(app: &App) -> NodeId {
    let page = app.page;
    for id in &app.selection {
        let mut cur = *id;
        while let Some(parent) = app.doc.get(cur).and_then(|n| n.parent) {
            if parent == page {
                return cur;
            }
            cur = parent;
        }
    }
    page
}

/// The export button of the timeline header: opens the animation export (file dialog, format,
/// fps and scale as remembered by the inspector's Animation block) for [`export_target`]. Says so
/// when nothing is animated. Returns whether an export was offered.
pub fn export_from_timeline(app: &mut App) -> bool {
    let node = export_target(app);
    if !app.has_motion_export(node) {
        app.show_toast("Nothing to export yet: add keyframes or an animation first", false);
        return false;
    }
    app.export_animation_dialog(node);
    true
}

fn time_field(ui: &mut Ui, rect: Rect, key: &str, shown: &str) -> Option<String> {
    let id = ui.id().with(key);
    field::field_bg(ui, rect, id);
    field::edit_text(ui, rect, id, shown, theme::font_body(), theme::TEXT, 4.0)
}

/// Changes the page's timeline duration (one undo step); the playhead stays inside.
fn set_duration(app: &mut App, ms: u32) -> bool {
    let tl = app.doc.timeline(app.page);
    let Ok(cmd) = edit::set_timeline(&app.doc, app.page, TimelineSettings { duration_ms: ms, ..tl }) else { return false };
    let ok = exec(app, cmd);
    seek(app, app.motion.playhead_ms);
    ok
}

// ---- ruler and playhead -----------------------------------------------------------------------

fn paint_ruler(app: &App, ui: &Ui, st: &TimelineUi, g: &Geo) {
    let p = ui.painter().with_clip_rect(g.ruler);
    let step = tick_step(g.pxms);
    let minor = if step.is_multiple_of(5) && step >= 5 {
        step / 5
    } else if step.is_multiple_of(2) {
        step / 2
    } else {
        step
    };
    let end = g.t(g.ruler.right()).max(0.0) as u32 + step;
    let start = (g.t(g.ruler.left()).max(0.0) as u32 / minor.max(1)) * minor.max(1);
    let base = g.ruler.bottom();
    let mut t = start;
    let dur = if let Some(Drag::Duration { new_ms }) = &st.drag { *new_ms } else { g.duration };
    while t <= end.min(dur.max(g.duration) + step) {
        let x = g.x(t as f32);
        let major = t.is_multiple_of(step);
        if x >= g.ruler.left() && x <= g.ruler.right() {
            let h = if major { 6.0 } else { 3.0 };
            widgets::vline(&p, x.round() - 0.5, base - h, base, if major { theme::TEXT_TERTIARY } else { theme::BORDER });
            if major {
                text(&p, Pos2::new(x + 3.0, g.ruler.top() + 17.0), Align2::LEFT_CENTER, &ruler_label(t), theme::TEXT_SECONDARY);
            }
        }
        t += minor.max(1);
    }
    let _ = app;
}

fn paint_playhead(app: &App, ui: &Ui, st: &TimelineUi, g: &Geo) {
    let x = g.x(app.motion.playhead_ms as f32);
    let clip = Rect::from_min_max(Pos2::new(g.ruler.left(), g.tl.top()), g.tl.max);
    if x < clip.left() - 8.0 || x > clip.right() + 8.0 {
        return;
    }
    let p = ui.painter().with_clip_rect(clip);
    let y0 = g.tl.top() + 6.0;
    let y1 = if st.collapsed { g.ruler.bottom() } else { g.tl.bottom() };
    p.line_segment([Pos2::new(x, y0), Pos2::new(x, y1)], Stroke::new(1.0, PLAYHEAD));
    p.circle_filled(Pos2::new(x, y0 + 1.0), 5.5, PLAYHEAD);
    rec(ui, || "motion-playhead".into(), Rect::from_center_size(Pos2::new(x, y0), Vec2::splat(12.0)));
}

/// Scrubbing and dragging the end of the timeline in the ruler (also while the panel is collapsed).
fn ruler_input(app: &mut App, ui: &mut Ui, st: &mut TimelineUi, g: &Geo) {
    let (pressed, down, pos) = ui.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_down(), i.pointer.latest_pos()));
    let Some(pos) = pos else { return };
    let over = g.ruler.contains(pos) && pos.y > g.tl.top() + 3.0 && !widgets::pointer_over_popup(ui.ctx());
    if over && near_end(g, pos.x) && st.drag.is_none() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    if pressed && over && st.drag.is_none() {
        // the knob of the collapse button area stays clickable
        st.drag = Some(if near_end(g, pos.x) { Drag::Duration { new_ms: g.duration } } else { Drag::Scrub });
        app.motion.playing = false;
    }
    drive_simple_drags(app, ui, st, g, pos, down);
}

fn near_end(g: &Geo, x: f32) -> bool {
    (x - g.end_x()).abs() <= 3.0
}

fn drive_simple_drags(app: &mut App, ui: &Ui, st: &mut TimelineUi, g: &Geo, pos: Pos2, down: bool) {
    match st.drag.clone() {
        Some(Drag::Scrub) => {
            if down {
                let t = g.snap(g.t(pos.x));
                app.motion.playhead_ms = t.min(g.duration);
            } else {
                st.drag = None;
            }
        }
        Some(Drag::Duration { .. }) => {
            if down {
                let t = g.snap(g.t(pos.x)).clamp(1, TimelineSettings::MAX_DURATION_MS);
                st.drag = Some(Drag::Duration { new_ms: t });
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            } else if let Some(Drag::Duration { new_ms }) = st.drag.take() {
                set_duration(app, new_ms);
            }
        }
        _ => {}
    }
}

// ---- left column ------------------------------------------------------------------------------

enum Act {
    Select(NodeId),
    Fold(NodeId),
    Visible(NodeId),
    Seek(u32),
    ToggleKey(NodeId, AnimProperty),
    TrackVisible(NodeId, AnimProperty),
    SelectClip(NodeId, u32),
    ClipVisible(NodeId, u32),
}

fn left_column(app: &mut App, ui: &mut Ui, st: &mut TimelineUi, g: &Geo, rows: &[Row]) {
    let t = app.motion.playhead_ms;
    let mut acts: Vec<Act> = Vec::new();
    let sel_clip = app.motion.selected_clip;
    crate::panels::clipped(ui, g.left, |ui| {
        let p = ui.painter().clone();
        for (i, row) in rows.iter().enumerate() {
            let top = g.row_top(i);
            if top + ROW_H < g.left.top() || top > g.left.bottom() {
                continue;
            }
            let rr = Rect::from_min_size(Pos2::new(g.left.left(), top), Vec2::new(LEFT_W, ROW_H));
            let cy = rr.center().y;
            match *row {
                Row::Layer(id) => {
                    let Some(n) = app.doc.get(id) else { continue };
                    let selected = app.is_selected(id);
                    if selected {
                        p.rect_filled(rr, 0.0, ROW_SELECTED);
                    }
                    rec(ui, || format!("motion-layer-{}", id.0), rr);
                    let resp = ui.interact(rr, ui.id().with(("motion-layer", id.0)), Sense::click());
                    if resp.clicked() {
                        acts.push(Act::Select(id));
                    }
                    let has_children = !n.motion.is_empty();
                    if has_children {
                        let ar = Rect::from_center_size(Pos2::new(rr.left() + 9.0, cy), Vec2::splat(14.0));
                        let folded = st.folded.contains(&id);
                        icons::paint(&p, if folded { Icon::ChevronRight } else { Icon::ChevronDown }, ar.center(), 10.0, theme::ICON_SECONDARY);
                        rec(ui, || format!("motion-fold-{}", id.0), ar);
                        if ui.interact(ar, ui.id().with(("motion-fold", id.0)), Sense::click()).clicked() {
                            acts.push(Act::Fold(id));
                        }
                    }
                    icons::paint(
                        &p,
                        kind_icon(&n.kind),
                        Pos2::new(rr.left() + 27.0, cy),
                        12.0,
                        if selected { theme::ICON_SELECTED } else { theme::ICON_SECONDARY },
                    );
                    let name = widgets::elide(&p, &n.name, &theme::font_body(), LEFT_W - 39.0 - 28.0);
                    text(&p, Pos2::new(rr.left() + 39.0, cy), Align2::LEFT_CENTER, &name, if n.visible { theme::TEXT } else { theme::TEXT_TERTIARY });
                    let eye = Rect::from_center_size(Pos2::new(rr.right() - 15.0, cy), Vec2::splat(16.0));
                    if ui.rect_contains_pointer(rr) || !n.visible {
                        let er = ui.interact(eye, ui.id().with(("motion-eye", id.0)), Sense::click());
                        icons::paint(
                            &p,
                            if n.visible { Icon::Eye } else { Icon::EyeOff },
                            eye.center(),
                            12.0,
                            if er.hovered() { theme::ICON } else { theme::ICON_SECONDARY },
                        );
                        if er.on_hover_text("Toggle layer visibility").clicked() {
                            acts.push(Act::Visible(id));
                        }
                    }
                }
                Row::Clip(id, cid) => {
                    let Some(c) = app.doc.get(id).and_then(|n| n.motion.clip(cid)) else { continue };
                    let selected = sel_clip == Some((id, cid));
                    if selected {
                        p.rect_filled(rr, 0.0, theme::BG_SELECTED_SECONDARY);
                    }
                    let resp = ui.interact(rr, ui.id().with(("motion-clip-row", id.0, cid)), Sense::click());
                    if resp.clicked() {
                        acts.push(Act::SelectClip(id, cid));
                    }
                    text(
                        &p,
                        Pos2::new(rr.left() + 39.0, cy),
                        Align2::LEFT_CENTER,
                        c.kind.name(),
                        if c.visible { theme::TEXT_SECONDARY } else { theme::TEXT_TERTIARY },
                    );
                    let eye = Rect::from_center_size(Pos2::new(rr.right() - 15.0, cy), Vec2::splat(16.0));
                    if ui.rect_contains_pointer(rr) || !c.visible {
                        let er = ui.interact(eye, ui.id().with(("motion-clip-eye", id.0, cid)), Sense::click());
                        icons::paint(
                            &p,
                            if c.visible { Icon::Eye } else { Icon::EyeOff },
                            eye.center(),
                            12.0,
                            if er.hovered() { theme::ICON } else { theme::ICON_SECONDARY },
                        );
                        if er.on_hover_text("Toggle animation visibility").clicked() {
                            acts.push(Act::ClipVisible(id, cid));
                        }
                    }
                }
                Row::Prop(id, prop) => {
                    let Some(n) = app.doc.get(id) else { continue };
                    let Some(track) = n.motion.track(prop) else { continue };
                    let resp = ui.interact(rr, ui.id().with(("motion-prop-row", id.0, prop as u8)), Sense::click());
                    if resp.clicked() && !app.is_selected(id) {
                        acts.push(Act::Select(id));
                    }
                    let live = track.visible;
                    text(
                        &p,
                        Pos2::new(rr.left() + 39.0, cy),
                        Align2::LEFT_CENTER,
                        prop.name(),
                        if live { theme::TEXT_SECONDARY } else { theme::TEXT_TERTIARY },
                    );
                    let prev = track.prev_key_time(t);
                    let next = track.next_key_time(t);
                    let on_key = track.key_at(t).is_some();
                    let pr = Rect::from_center_size(Pos2::new(rr.left() + 95.0, cy), Vec2::new(14.0, ROW_H));
                    let dr = Rect::from_center_size(Pos2::new(rr.left() + 109.0, cy), Vec2::new(14.0, ROW_H));
                    let nr = Rect::from_center_size(Pos2::new(rr.left() + 123.0, cy), Vec2::new(14.0, ROW_H));
                    rec(ui, || format!("motion-prev-{}-{}", id.0, prop.name()), pr);
                    rec(ui, || format!("motion-diamond-{}-{}", id.0, prop.name()), dr);
                    rec(ui, || format!("motion-next-{}-{}", id.0, prop.name()), nr);
                    let nav = |ui: &mut Ui, r: Rect, enabled: bool, icon: Icon, tag: &str, tip: &str| {
                        let rs = ui.interact(r, ui.id().with(("motion-nav", id.0, prop as u8, tag)), Sense::click());
                        icons::paint(
                            ui.painter(),
                            icon,
                            r.center(),
                            10.0,
                            if !enabled {
                                theme::ICON_TERTIARY
                            } else if rs.hovered() {
                                theme::ICON
                            } else {
                                theme::ICON_SECONDARY
                            },
                        );
                        rs.on_hover_text(tip).clicked() && enabled
                    };
                    if nav(ui, pr, prev.is_some(), Icon::ChevronLeft, "prev", "Previous keyframe") {
                        acts.push(Act::Seek(prev.unwrap_or(t)));
                    }
                    let dresp = ui.interact(dr, ui.id().with(("motion-diamond", id.0, prop as u8)), Sense::click());
                    let dcol = if on_key {
                        PLAYHEAD
                    } else if dresp.hovered() {
                        theme::ICON
                    } else {
                        theme::ICON_SECONDARY
                    };
                    diamond(&p, dr.center(), 3.5, if on_key { PLAYHEAD } else { Color32::TRANSPARENT }, dcol);
                    if dresp.on_hover_text("Add or remove keyframe").clicked() {
                        acts.push(Act::ToggleKey(id, prop));
                    }
                    if nav(ui, nr, next.is_some(), Icon::ChevronRight, "next", "Next keyframe") {
                        acts.push(Act::Seek(next.unwrap_or(t)));
                    }
                    if let Some(v) = motion::value_at(n, prop, t as f32) {
                        let s = widgets::elide(&p, &value_text(prop, v), &theme::font_body(), LEFT_W - 137.0 - 4.0);
                        text(&p, Pos2::new(rr.left() + 137.0, cy), Align2::LEFT_CENTER, &s, if live { theme::TEXT_SECONDARY } else { theme::TEXT_TERTIARY });
                    }
                    // "Toggle animation visibility": shown on hover (over the value) or when off
                    let eye = Rect::from_center_size(Pos2::new(rr.right() - 15.0, cy), Vec2::splat(16.0));
                    if ui.rect_contains_pointer(rr) || !live {
                        let er = ui.interact(eye, ui.id().with(("motion-prop-eye", id.0, prop as u8)), Sense::click());
                        rec(ui, || format!("motion-eye-{}-{}", id.0, prop.name()), eye);
                        p.rect_filled(eye.expand(2.0), 0.0, if app.is_selected(id) { ROW_SELECTED } else { theme::BG });
                        icons::paint(
                            &p,
                            if live { Icon::Eye } else { Icon::EyeOff },
                            eye.center(),
                            12.0,
                            if er.hovered() { theme::ICON } else { theme::ICON_SECONDARY },
                        );
                        if er.on_hover_text("Toggle animation visibility").clicked() {
                            acts.push(Act::TrackVisible(id, prop));
                        }
                    }
                }
            }
        }
    });
    for a in acts {
        match a {
            Act::Select(id) => {
                app.select_only(id);
                app.motion.selected_clip = None;
            }
            Act::Fold(id) => {
                if !st.folded.remove(&id) {
                    st.folded.insert(id);
                }
            }
            Act::Visible(id) => app.toggle_visible(id),
            Act::Seek(ms) => seek(app, ms),
            Act::ToggleKey(id, prop) => {
                if let Ok(cmd) = edit::toggle_keyframe(&app.doc, id, prop, t) {
                    exec(app, cmd);
                }
            }
            Act::TrackVisible(id, prop) => {
                let visible = app.doc.get(id).and_then(|n| n.motion.track(prop)).is_none_or(|t| t.visible);
                if let Ok(cmd) = edit::set_track_visible(&app.doc, id, prop, !visible) {
                    exec(app, cmd);
                }
            }
            Act::SelectClip(id, cid) => {
                app.select_only(id);
                app.motion.selected_clip = Some((id, cid));
                app.motion.selected_keys.clear();
                app.motion.easing_target = false;
            }
            Act::ClipVisible(id, cid) => {
                if let Some(mut c) = app.doc.get(id).and_then(|n| n.motion.clip(cid)).cloned() {
                    c.visible = !c.visible;
                    if let Ok(cmd) = edit::update_clip(&app.doc, id, c) {
                        exec(app, cmd);
                    }
                }
            }
        }
    }
}

// ---- track area: painting ---------------------------------------------------------------------

/// Where the key at `k` is drawn while a drag is in progress.
fn shown_time(drag: &Option<Drag>, k: &KeyRef) -> u32 {
    match drag {
        Some(Drag::Summary { active: true, node, delta, .. }) if *node == k.node => (k.time_ms as i64 + delta).max(0) as u32,
        Some(Drag::Keys { active: true, dup: false, delta, base, .. }) if base.contains(k) => (k.time_ms as i64 + delta).max(0) as u32,
        Some(Drag::Stretch { active: true, pivot, factor, base, .. }) if base.contains(k) => {
            edit::to_ms(*pivot as f32 + (k.time_ms as f32 - *pivot as f32) * factor)
        }
        _ => k.time_ms,
    }
}

fn paint_tracks(app: &App, ui: &Ui, st: &TimelineUi, g: &Geo, rows: &[Row]) {
    let clip_rect = g.tracks;
    let p = ui.painter().with_clip_rect(clip_rect);
    // the area past the duration is darker
    let dur = if let Some(Drag::Duration { new_ms }) = &st.drag { *new_ms } else { g.duration };
    let end_x = g.x(dur as f32);
    if end_x < clip_rect.right() {
        p.rect_filled(Rect::from_min_max(Pos2::new(end_x.max(clip_rect.left()), clip_rect.top()), clip_rect.max), 0.0, OUTSIDE);
    }
    let t = app.motion.playhead_ms;
    // selected layer rows span the whole width
    for (i, row) in rows.iter().enumerate() {
        let top = g.row_top(i);
        if top + ROW_H < clip_rect.top() || top > clip_rect.bottom() {
            continue;
        }
        let rr = Rect::from_min_size(Pos2::new(clip_rect.left(), top), Vec2::new(clip_rect.width(), ROW_H));
        let cy = rr.center().y;
        match *row {
            Row::Layer(id) => {
                let Some(n) = app.doc.get(id) else { continue };
                if app.is_selected(id) {
                    p.rect_filled(rr, 0.0, ROW_SELECTED);
                }
                if let Some((lo, hi)) = n.motion.span_ms(g.duration) {
                    let shift = match &st.drag {
                        Some(Drag::Summary { active: true, node, delta, .. }) if *node == id => *delta,
                        _ => 0,
                    };
                    let (lo, hi) = ((lo as i64 + shift).max(0) as u32, (hi as i64 + shift).max(0) as u32);
                    rec(
                        ui,
                        || format!("motion-summary-{}", id.0),
                        Rect::from_min_max(Pos2::new(g.x(lo as f32), cy - 6.0), Pos2::new(g.x(hi as f32), cy + 6.0)),
                    );
                    let bar = Rect::from_min_max(Pos2::new(g.x(lo as f32), cy - 6.0), Pos2::new(g.x(hi as f32).max(g.x(lo as f32) + 3.0), cy + 6.0));
                    p.rect_filled(bar, theme::cr(2), SUMMARY);
                    for x in [bar.left() + 3.0, bar.right() - 3.0] {
                        widgets::vline(&p, x, cy - 3.0, cy + 3.0, theme::rgba(0xFFFFFF, 0xB2));
                    }
                }
            }
            Row::Clip(id, cid) => {
                let Some(c0) = app.doc.get(id).and_then(|n| n.motion.clip(cid)) else { continue };
                let moved;
                let c = match &st.drag {
                    Some(Drag::Clip { node, cur, .. }) if *node == id && cur.id == cid => cur,
                    Some(Drag::Summary { active: true, node, delta, .. }) if *node == id => {
                        moved = Clip { delay_ms: (c0.delay_ms as i64 + delta).max(0) as u32, ..c0.clone() };
                        &moved
                    }
                    _ => c0,
                };
                let x0 = g.x(c.delay_ms as f32);
                let x1 = g.x(c.end_ms(g.duration) as f32).max(x0 + 6.0);
                let bar = Rect::from_min_max(Pos2::new(x0, cy - 6.5), Pos2::new(x1, cy + 6.5));
                rec(ui, || format!("motion-clip-{}-{}", id.0, cid), bar);
                let sel = app.motion.selected_clip == Some((id, cid));
                let alpha = if c.visible { 1.0 } else { 0.4 };
                p.rect_filled(bar, theme::cr(4), CLIP_FILL.gamma_multiply(alpha));
                p.rect_stroke(
                    bar,
                    theme::cr(4),
                    Stroke::new(if sel { 1.5 } else { 1.0 }, if sel { Color32::WHITE } else { PLAYHEAD }.gamma_multiply(alpha)),
                    StrokeKind::Inside,
                );
                let label = widgets::elide(&p, c.kind.name(), &theme::font_small_medium(), (bar.width() - 14.0).max(0.0));
                p.text(bar.center(), Align2::CENTER_CENTER, label, theme::font_small_medium(), Color32::WHITE.gamma_multiply(alpha));
                for x in [bar.left() + 3.0, bar.right() - 3.0] {
                    widgets::vline(&p, x, cy - 3.0, cy + 3.0, theme::rgba(0xFFFFFF, 0xB2).gamma_multiply(alpha));
                }
            }
            Row::Prop(id, prop) => {
                let Some(track) = app.doc.get(id).and_then(|n| n.motion.track(prop)) else { continue };
                // a hidden track is drawn dimmed
                let dim = if track.visible { 1.0 } else { 0.35 };
                let mut keys: Vec<(KeyRef, u32)> = track.keys.iter().map(|k| (KeyRef::new(id, prop, k.time_ms), 0)).collect();
                for k in &mut keys {
                    k.1 = shown_time(&st.drag, &k.0);
                }
                let mut ordered = keys.clone();
                ordered.sort_by_key(|k| k.1);
                // lines (and the selected segment's easing glyph)
                for w in ordered.windows(2) {
                    let (xa, xb) = (g.x(w[0].1 as f32), g.x(w[1].1 as f32));
                    let seg_sel = app.motion.easing_target && app.motion.selected_keys.contains(&w[0].0);
                    p.line_segment([Pos2::new(xa, cy), Pos2::new(xb, cy)], Stroke::new(if seg_sel { 2.0 } else { 1.0 }, PLAYHEAD.gamma_multiply(dim)));
                    rec(
                        ui,
                        || format!("motion-seg-{}-{}-{}", id.0, prop.name(), w[0].0.time_ms),
                        Rect::from_center_size(Pos2::new((xa + xb) / 2.0, cy), Vec2::new((xb - xa).max(2.0), ROW_H)),
                    );
                    if seg_sel {
                        let ease = track.key_at(w[0].0.time_ms).map(|k| k.easing).unwrap_or_default();
                        let c = Pos2::new((xa + xb) / 2.0, cy);
                        let gr = Rect::from_center_size(c, Vec2::new(16.0, 12.0));
                        if xb - xa > 24.0 {
                            p.rect_filled(gr, theme::cr(3), theme::BG);
                            p.rect_stroke(gr, theme::cr(3), Stroke::new(1.0, PLAYHEAD), StrokeKind::Inside);
                            mini_curve(&p, c, &ease, theme::TEXT);
                        }
                    }
                }
                for (k, shown) in &keys {
                    let c = Pos2::new(g.x(*shown as f32), cy);
                    let sel = app.motion.selected_keys.contains(k);
                    let hot = *shown == t;
                    diamond(
                        &p,
                        c,
                        DIAMOND_R,
                        if sel || hot { PLAYHEAD } else { theme::BG }.gamma_multiply(dim),
                        if sel || hot { PLAYHEAD } else { theme::TEXT_SECONDARY }.gamma_multiply(dim),
                    );
                    rec(
                        ui,
                        || format!("motion-key-{}-{}-{}", id.0, prop.name(), k.time_ms),
                        Rect::from_center_size(Pos2::new(g.x(k.time_ms as f32), cy), Vec2::splat(KEY_HIT * 2.0)),
                    );
                }
                // ghosts of an Alt-drag duplicate
                if let Some(Drag::Keys { active: true, dup: true, delta, base, .. }) = &st.drag {
                    for k in base.iter().filter(|k| k.node == id && k.property == prop) {
                        let tt = (k.time_ms as i64 + delta).max(0) as f32;
                        diamond(&p, Pos2::new(g.x(tt), cy), DIAMOND_R, PLAYHEAD.gamma_multiply(0.45), PLAYHEAD);
                    }
                }
            }
        }
    }
    // marquee
    if let Some(Drag::Marquee { origin, cur, .. }) = &st.drag {
        let r = Rect::from_two_pos(*origin, *cur);
        p.rect_filled(r, 0.0, theme::CANVAS_MARQUEE_FILL);
        p.rect_stroke(r, 0.0, Stroke::new(1.0, theme::CANVAS_SELECTION), StrokeKind::Inside);
    }
    // stretch / duration cursors
    if matches!(st.drag, Some(Drag::Stretch { .. })) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    // the end of the timeline
    widgets::vline(&p, end_x.round() - 0.5, clip_rect.top(), clip_rect.bottom(), theme::rgba(0xFFFFFF, 0x1A));
}

fn zoom_pill(ui: &mut Ui, st: &mut TimelineUi, g: &mut Geo) {
    let pill = g.pill();
    let p = ui.painter().clone();
    let help = g.help();
    widgets::elevation_shadow(&p, pill, 12);
    p.rect_filled(pill, theme::cr(12), theme::BG_MENU);
    p.rect_stroke(pill, theme::cr(12), Stroke::new(1.0, theme::ELEVATION_400_RING), StrokeKind::Inside);
    p.circle_filled(help.center(), 12.0, theme::BG_MENU);
    p.circle_stroke(help.center(), 11.5, Stroke::new(1.0, theme::ELEVATION_400_RING));
    let out = Rect::from_min_size(pill.min, Vec2::new(28.0, 24.0));
    let inn = Rect::from_min_size(Pos2::new(pill.right() - 28.0, pill.top()), Vec2::new(28.0, 24.0));
    let center_x = g.tracks.center().x;
    for (r, icon, key, tip, f) in [(out, Icon::Minus, "zoom-out", "Zoom out timeline", 1.0 / 1.4), (inn, Icon::Plus, "zoom-in", "Zoom in timeline", 1.4)] {
        rec(ui, || format!("motion-{key}"), r);
        let resp = ui.interact(r, ui.id().with(("motion-btn", key)), Sense::click());
        icons::paint(&p, icon, r.center(), 14.0, if resp.hovered() { theme::ICON } else { theme::ICON_SECONDARY });
        if resp.on_hover_text(tip).clicked() {
            zoom_about(st, g, center_x, st.zoom * f);
        }
    }
    icons::paint(&p, Icon::Help, help.center(), 14.0, theme::ICON_SECONDARY);
    let hr = ui.interact(help, ui.id().with("motion-help"), Sense::click());
    hr.on_hover_text("Space play, , . step a frame, Shift+, . previous / next keyframe, Home / End");
}

// ---- track area: input ------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum Hit {
    Key(KeyRef),
    /// The segment starting at this keyframe.
    Segment(KeyRef),
    ClipBody(NodeId, u32),
    ClipEdge(NodeId, u32, bool),
    Summary(NodeId),
    None,
}

fn hit_test(app: &App, g: &Geo, rows: &[Row], pos: Pos2) -> Hit {
    let Some(ri) = g.row_at(pos.y) else { return Hit::None };
    let Some(row) = rows.get(ri) else { return Hit::None };
    match *row {
        Row::Prop(id, prop) => {
            let Some(track) = app.doc.get(id).and_then(|n| n.motion.track(prop)) else { return Hit::None };
            if let Some(k) = track.keys.iter().min_by(|a, b| (g.x(a.time_ms as f32) - pos.x).abs().total_cmp(&(g.x(b.time_ms as f32) - pos.x).abs())) {
                if (g.x(k.time_ms as f32) - pos.x).abs() <= KEY_HIT {
                    return Hit::Key(KeyRef::new(id, prop, k.time_ms));
                }
            }
            for w in track.keys.windows(2) {
                if pos.x > g.x(w[0].time_ms as f32) && pos.x < g.x(w[1].time_ms as f32) {
                    return Hit::Segment(KeyRef::new(id, prop, w[0].time_ms));
                }
            }
            Hit::None
        }
        Row::Clip(id, cid) => {
            let Some(c) = app.doc.get(id).and_then(|n| n.motion.clip(cid)) else { return Hit::None };
            let (x0, x1) = (g.x(c.delay_ms as f32), g.x(c.end_ms(g.duration) as f32).max(g.x(c.delay_ms as f32) + 6.0));
            if pos.x < x0 || pos.x > x1 {
                Hit::None
            } else if pos.x - x0 <= EDGE_HIT {
                Hit::ClipEdge(id, cid, false)
            } else if x1 - pos.x <= EDGE_HIT {
                Hit::ClipEdge(id, cid, true)
            } else {
                Hit::ClipBody(id, cid)
            }
        }
        Row::Layer(id) => match app.doc.get(id).and_then(|n| n.motion.span_ms(g.duration)) {
            Some((lo, hi)) if pos.x >= g.x(lo as f32) && pos.x <= g.x(hi as f32) => Hit::Summary(id),
            _ => Hit::None,
        },
    }
}

fn input(app: &mut App, ui: &mut Ui, st: &mut TimelineUi, g: &Geo, rows: &[Row]) {
    let (pressed, down, pos, mods, dbl) = ui.input(|i| {
        (
            i.pointer.primary_pressed(),
            i.pointer.primary_down(),
            i.pointer.latest_pos(),
            i.modifiers,
            i.pointer.button_double_clicked(egui::PointerButton::Primary),
        )
    });
    let Some(pos) = pos else {
        if !down {
            st.drag = None;
        }
        return;
    };
    let popup = widgets::pointer_over_popup(ui.ctx());
    let in_ruler = g.ruler.contains(pos) && pos.y > g.tl.top() + 3.0;
    let in_tracks = g.tracks.contains(pos) && !g.pill().union(g.help()).expand(4.0).contains(pos);

    // a press on the canvas drops keyframe / clip selection
    if pressed && app.canvas_rect.contains(pos) && !popup {
        app.motion.selected_keys.clear();
        app.motion.selected_clip = None;
        app.motion.easing_target = false;
    }

    // cursors
    if st.drag.is_none() && !popup {
        if (in_ruler || in_tracks) && near_end(g, pos.x) && (in_ruler || hit_test(app, g, rows, pos).is_none_hit()) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        } else if in_tracks {
            if let Hit::ClipEdge(..) = hit_test(app, g, rows, pos) {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            }
        }
    }

    if pressed && st.drag.is_none() && !popup {
        if in_ruler {
            st.drag = Some(if near_end(g, pos.x) { Drag::Duration { new_ms: g.duration } } else { Drag::Scrub });
            app.motion.playing = false;
        } else if in_tracks {
            press(app, st, g, rows, pos, mods);
        }
    }

    // double click on a keyframe: jump the playhead to it
    if dbl && in_tracks && !popup {
        if let Hit::Key(k) = hit_test(app, g, rows, pos) {
            seek(app, k.time_ms);
        }
    }

    match st.drag.clone() {
        Some(Drag::Scrub) | Some(Drag::Duration { .. }) => drive_simple_drags(app, ui, st, g, pos, down),
        Some(d) => {
            if down {
                update_drag(app, st, g, rows, pos, d);
            } else {
                finish_drag(app, st, g, pos, mods);
            }
        }
        None => {}
    }

    // keep dragging near the edges: auto-scroll
    if down
        && matches!(
            st.drag,
            Some(Drag::Keys { .. }) | Some(Drag::Marquee { .. }) | Some(Drag::Clip { .. }) | Some(Drag::Stretch { .. }) | Some(Drag::Summary { .. })
        )
    {
        if pos.x > g.tracks.right() - 12.0 {
            st.scroll_x += 6.0;
        } else if pos.x < g.tracks.left() + 12.0 {
            st.scroll_x = (st.scroll_x - 6.0).max(0.0);
        }
        ui.ctx().request_repaint();
    }
}

impl Hit {
    fn is_none_hit(&self) -> bool {
        matches!(self, Hit::None)
    }
}

fn press(app: &mut App, st: &mut TimelineUi, g: &Geo, rows: &[Row], pos: Pos2, mods: egui::Modifiers) {
    let additive = mods.shift || mods.command;
    match hit_test(app, g, rows, pos) {
        Hit::Key(k) => {
            app.motion.selected_clip = None;
            app.motion.easing_target = false;
            if !app.is_selected(k.node) && !additive {
                app.select_only(k.node);
            }
            if additive {
                if let Some(i) = app.motion.selected_keys.iter().position(|s| *s == k) {
                    app.motion.selected_keys.remove(i);
                    return;
                }
                app.motion.selected_keys.push(k);
            } else if !app.motion.selected_keys.contains(&k) {
                app.motion.selected_keys = vec![k];
            }
            let base = app.motion.selected_keys.clone();
            let (lo, hi) = (base.iter().map(|b| b.time_ms).min().unwrap_or(0), base.iter().map(|b| b.time_ms).max().unwrap_or(0));
            if mods.alt && lo != hi && (k.time_ms == lo || k.time_ms == hi) {
                // Alt on a selection's edge: time-stretch about the other edge
                let (pivot, edge) = if k.time_ms == hi { (lo, hi) } else { (hi, lo) };
                st.drag = Some(Drag::Stretch { origin: pos, pivot, edge, factor: 1.0, active: false, base });
            } else {
                st.drag = Some(Drag::Keys { origin: pos, anchor: k.time_ms, active: false, dup: mods.alt, delta: 0, base, shifted: additive, pressed: k });
            }
        }
        Hit::Segment(k) => {
            app.motion.selected_keys = vec![k];
            app.motion.selected_clip = None;
            app.motion.easing_target = true;
            if !app.is_selected(k.node) {
                app.select_only(k.node);
            }
        }
        h @ (Hit::ClipBody(..) | Hit::ClipEdge(..)) => {
            let (n, c, edge) = match h {
                Hit::ClipEdge(n, c, true) => (n, c, ClipEdge::TrimEnd),
                Hit::ClipEdge(n, c, false) => (n, c, ClipEdge::TrimStart),
                Hit::ClipBody(n, c) => (n, c, ClipEdge::Move),
                _ => return,
            };
            app.select_only(n);
            app.motion.selected_clip = Some((n, c));
            app.motion.selected_keys.clear();
            app.motion.easing_target = false;
            let Some(orig) = app.doc.get(n).and_then(|nd| nd.motion.clip(c)).cloned() else { return };
            st.drag = Some(Drag::Clip { node: n, edge, origin: pos, cur: orig.clone(), orig, active: false });
        }
        Hit::Summary(n) => {
            app.select_only(n);
            app.motion.selected_clip = None;
            app.motion.selected_keys.clear();
            app.motion.easing_target = false;
            if let Some((lo, _)) = app.doc.get(n).and_then(|nd| nd.motion.span_ms(g.duration)) {
                st.drag = Some(Drag::Summary { node: n, origin: pos, lo, active: false, delta: 0 });
            }
        }
        _ => {
            if near_end(g, pos.x) {
                st.drag = Some(Drag::Duration { new_ms: g.duration });
                return;
            }
            let base = if additive { app.motion.selected_keys.clone() } else { Vec::new() };
            if !additive {
                app.motion.selected_keys.clear();
                app.motion.selected_clip = None;
                app.motion.easing_target = false;
            }
            st.drag = Some(Drag::Marquee { origin: pos, cur: pos, base });
        }
    }
}

fn update_drag(app: &mut App, st: &mut TimelineUi, g: &Geo, rows: &[Row], pos: Pos2, d: Drag) {
    match d {
        Drag::Keys { origin, anchor, active, dup, base, shifted, pressed, .. } => {
            let active = active || (pos - origin).length() > DRAG_SLOP;
            let raw = anchor as f32 + (pos.x - origin.x) / g.pxms;
            let min_t = base.iter().map(|k| k.time_ms).min().unwrap_or(0) as i64;
            let delta = (g.snap(raw) as i64 - anchor as i64).max(-min_t);
            st.drag = Some(Drag::Keys { origin, anchor, active, dup, delta, base, shifted, pressed });
        }
        Drag::Stretch { origin, pivot, edge, active, base, .. } => {
            let active = active || (pos - origin).length() > DRAG_SLOP;
            let new_edge = g.snap(g.t(pos.x)) as f32;
            let denom = edge as f32 - pivot as f32;
            let factor = if denom.abs() < 1.0 { 1.0 } else { ((new_edge - pivot as f32) / denom).max(0.02) };
            st.drag = Some(Drag::Stretch { origin, pivot, edge, factor, active, base });
        }
        Drag::Summary { node, origin, lo, active, .. } => {
            let active = active || (pos - origin).length() > DRAG_SLOP;
            let delta = (g.snap(lo as f32 + (pos.x - origin.x) / g.pxms) as i64 - lo as i64).max(-(lo as i64));
            st.drag = Some(Drag::Summary { node, origin, lo, active, delta });
        }
        Drag::Clip { node, edge, origin, orig, active, .. } => {
            let active = active || (pos - origin).length() > DRAG_SLOP;
            let dx = (pos.x - origin.x) / g.pxms;
            let cur = clip_dragged(&orig, &edge, dx, g);
            st.drag = Some(Drag::Clip { node, edge, origin, orig, cur, active });
        }
        Drag::Marquee { origin, base, .. } => {
            let r = Rect::from_two_pos(origin, pos);
            let mut sel = base.clone();
            for (i, row) in rows.iter().enumerate() {
                let cy = g.row_top(i) + ROW_H / 2.0;
                if cy < r.top() - ROW_H / 2.0 || cy > r.bottom() + ROW_H / 2.0 {
                    continue;
                }
                if let Row::Prop(id, prop) = *row {
                    if let Some(tr) = app.doc.get(id).and_then(|n| n.motion.track(prop)) {
                        for k in &tr.keys {
                            let c = Pos2::new(g.x(k.time_ms as f32), cy);
                            if r.contains(c) {
                                let kr = KeyRef::new(id, prop, k.time_ms);
                                if !sel.contains(&kr) {
                                    sel.push(kr);
                                }
                            }
                        }
                    }
                }
            }
            app.motion.selected_keys = sel;
            app.motion.easing_target = false;
            st.drag = Some(Drag::Marquee { origin, cur: pos, base });
        }
        _ => {}
    }
}

/// The clip as moved / trimmed by `dx` ms from its state at the start of the drag.
fn clip_dragged(orig: &Clip, edge: &ClipEdge, dx: f32, g: &Geo) -> Clip {
    let mut c = orig.clone();
    let d0 = orig.duration_ms.max(1) as f32;
    let cycles = match orig.repeat {
        ClipRepeat::Times(n) => n.max(1) as f32,
        _ => 1.0,
    };
    let total0 = d0 * cycles;
    match edge {
        ClipEdge::Move => c.delay_ms = g.snap(orig.delay_ms as f32 + dx),
        ClipEdge::TrimStart => {
            let end = orig.delay_ms as f32 + total0;
            let start = (g.snap(orig.delay_ms as f32 + dx) as f32).min(end - cycles);
            c.delay_ms = start.max(0.0) as u32;
            c.duration_ms = (((end - c.delay_ms as f32) / cycles).round() as u32).max(1);
        }
        ClipEdge::TrimEnd => {
            let end = (g.snap(orig.delay_ms as f32 + total0 + dx) as f32).max(orig.delay_ms as f32 + cycles);
            c.duration_ms = (((end - orig.delay_ms as f32) / cycles).round() as u32).max(1);
        }
    }
    c
}

fn finish_drag(app: &mut App, st: &mut TimelineUi, _g: &Geo, _pos: Pos2, mods: egui::Modifiers) {
    let Some(d) = st.drag.take() else { return };
    match d {
        Drag::Keys { active, dup, delta, base, shifted, pressed, .. } => {
            if !active {
                // a plain click on one of several selected keys narrows the selection to it
                if !shifted && base.len() > 1 && !mods.alt {
                    app.motion.selected_keys = vec![pressed];
                }
                return;
            }
            if delta == 0 {
                return;
            }
            if dup {
                let items: Vec<_> = base
                    .iter()
                    .filter_map(|k| {
                        let key = app.doc.get(k.node)?.motion.track(k.property)?.key_at(k.time_ms)?;
                        Some((k.node, k.property, (k.time_ms as i64 + delta).max(0) as u32, key.value, key.easing))
                    })
                    .collect();
                if let Some((cmd, refs)) = insert_keys(&app.doc, &items) {
                    if exec(app, cmd) {
                        app.motion.selected_keys = refs;
                    }
                }
            } else if let Ok((cmd, refs)) = edit::move_keyframes(&app.doc, &base, delta) {
                if exec(app, cmd) {
                    app.motion.selected_keys = refs;
                }
            }
        }
        Drag::Stretch { active, pivot, factor, base, .. } => {
            if active && (factor - 1.0).abs() > 1e-4 {
                if let Ok((cmd, refs)) = edit::stretch_keyframes(&app.doc, &base, pivot as f32, factor) {
                    if exec(app, cmd) {
                        app.motion.selected_keys = refs;
                    }
                }
            }
        }
        Drag::Summary { node, active, delta, .. } => {
            if active && delta != 0 {
                if let Ok(cmd) = edit::shift_node_motion(&app.doc, node, delta) {
                    exec(app, cmd);
                }
            }
        }
        Drag::Clip { node, active, cur, orig, .. } => {
            if active && cur != orig {
                if let Ok(cmd) = edit::update_clip(&app.doc, node, cur) {
                    exec(app, cmd);
                }
            }
        }
        _ => {}
    }
}

// ---- tests ------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_formats_roundtrip() {
        assert_eq!(fmt_time(997, TimeFormat::Ms, 60), "0997");
        assert_eq!(fmt_time(2000, TimeFormat::Ms, 60), "2000");
        assert_eq!(fmt_time(1500, TimeFormat::Seconds, 60), "1.500");
        assert_eq!(fmt_time(1000, TimeFormat::Frames, 60), "60");
        assert_eq!(parse_time("997", TimeFormat::Ms, 60), Some(997));
        assert_eq!(parse_time("1.5", TimeFormat::Seconds, 60), Some(1500));
        assert_eq!(parse_time("30", TimeFormat::Frames, 60), Some(500));
        assert_eq!(parse_time("250ms", TimeFormat::Ms, 60), Some(250));
        assert_eq!(parse_time("abc", TimeFormat::Ms, 60), None);
        assert_eq!(parse_time("-3", TimeFormat::Ms, 60), None);
    }

    #[test]
    fn ruler_labels_match_figma() {
        let l: Vec<String> = [0, 500, 1000, 1500, 2000].iter().map(|t| ruler_label(*t)).collect();
        assert_eq!(l, ["0s", "500ms", "1s", "500ms", "2s"]);
    }

    #[test]
    fn tick_step_grows_when_zoomed_out() {
        assert_eq!(tick_step(0.27), 500);
        assert_eq!(tick_step(0.13), 1000);
        assert_eq!(tick_step(2.0), 50);
        assert!(tick_step(0.001) >= 60_000);
    }

    #[test]
    fn values_read_like_figma() {
        assert_eq!(value_text(AnimProperty::Position, AnimValue::Vec2(desigma_doc::Vec2::new(200.0, -105.0))), "200, -105");
        assert_eq!(value_text(AnimProperty::Rotation, AnimValue::Scalar(90.0)), "90°");
        assert_eq!(value_text(AnimProperty::Opacity, AnimValue::Scalar(0.3)), "30%");
        assert_eq!(value_text(AnimProperty::FillColor, AnimValue::Color(desigma_doc::Color::rgb(0xD9, 0xD9, 0xD9))), "D9D9D9");
    }

    #[test]
    fn panel_height_is_clamped() {
        let mut st = TimelineUi::default();
        assert_eq!(panel_height(&st, 1000.0), DEFAULT_H);
        st.height = 10.0;
        assert_eq!(panel_height(&st, 1000.0), MIN_H);
        st.height = 5000.0;
        assert_eq!(panel_height(&st, 1000.0), 700.0);
        st.collapsed = true;
        assert_eq!(panel_height(&st, 1000.0), HEADER_H + 1.0);
    }
}
