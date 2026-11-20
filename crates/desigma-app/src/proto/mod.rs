//! Prototype mode: editing interactions (Prototype tab, canvas noodles) and
//! the player behind the Present view.

pub mod easing;
pub mod geom;
pub mod player;
pub mod smart;

use crate::app::App;
use desigma_doc::*;
use player::top_level_frames;

/// Editing state of the Prototype tab and canvas.
#[derive(Default)]
pub struct ProtoState {
    /// Interaction being edited in the popover: (layer, reaction index).
    pub editing: Option<(NodeId, usize)>,
    /// Screen rect of the row that opened the popover (clicks on it do not close it).
    pub editing_anchor: Option<egui::Rect>,
    /// Popover height measured last frame (the background is drawn before the content).
    pub popover_h: f32,
    /// Flow whose name is being edited in the list.
    pub scroll: f32,
    pub content_h: f32,
    /// Hotspot drag started on the canvas: (source layer, pointer position).
    pub connect: Option<(NodeId, egui::Pos2)>,
    /// Layer whose hotspot the pointer is over.
    pub hot: Option<NodeId>,
    /// Deepest layer under the pointer while in prototype mode.
    pub hover: Option<NodeId>,
    pub present: Option<crate::present::PresentState>,
}

/// Device presets: name, width, height, corner radius. The chosen preset is stored in
/// `Document::prototype_device`; Present letterboxes the start frame into its size.
pub const DEVICES: &[(&str, f32, f32, f32)] = &[
    ("None", 0.0, 0.0, 0.0),
    ("iPhone 15", 393.0, 852.0, 48.0),
    ("iPhone 15 Pro Max", 430.0, 932.0, 52.0),
    ("Android Large", 360.0, 800.0, 32.0),
    ("iPad mini", 744.0, 1133.0, 28.0),
    ("MacBook Air", 1280.0, 832.0, 12.0),
    ("Desktop", 1440.0, 1024.0, 8.0),
    ("Presentation", 1920.0, 1080.0, 0.0),
];

/// Corner radius used for a stored device whose name is not a known preset.
const CUSTOM_DEVICE_RADIUS: f32 = 24.0;

/// Index of the stored device in [`DEVICES`] (0 = None). Unknown names map to `None`.
pub fn device_index(d: &PrototypeDevice) -> Option<usize> {
    match &d.preset {
        None => Some(0),
        Some(name) => DEVICES.iter().position(|p| p.0 == name && p.1 > 0.0),
    }
}

/// The stored device as `Command` payload for preset `i` (keeps the background).
pub fn device_for(i: usize, keep: &PrototypeDevice) -> PrototypeDevice {
    match DEVICES.get(i).filter(|d| d.1 > 0.0) {
        Some(d) => PrototypeDevice { preset: Some(d.0.to_string()), size: Some(Vec2::new(d.1, d.2)), background: keep.background },
        None => PrototypeDevice { preset: None, size: None, background: keep.background },
    }
}

/// Viewport size and corner radius to present in; `None` = no device frame.
pub fn device_viewport(d: &PrototypeDevice) -> Option<(Vec2, f32)> {
    d.preset.as_ref()?;
    let known = d.preset.as_ref().and_then(|n| DEVICES.iter().find(|p| p.0 == n && p.1 > 0.0));
    let size = d.size.or_else(|| known.map(|p| Vec2::new(p.1, p.2)))?;
    (size.x > 0.0 && size.y > 0.0).then(|| (size, known.map(|p| p.3).unwrap_or(CUSTOM_DEVICE_RADIUS)))
}

impl App {
    /// Persists the device as one undo step.
    pub fn set_prototype_device(&mut self, device: PrototypeDevice) {
        if device == self.doc.prototype_device {
            return;
        }
        if let Err(e) = self.hist.exec(&mut self.doc, Command::SetPrototypeDevice { device }) {
            self.show_toast(format!("Edit failed: {e}"), true);
        }
        self.changed();
    }
}

// ------------------------------------------------------------------ labels

pub const TRIGGERS: &[&str] =
    &["On click", "On drag", "While hovering", "While pressing", "Mouse enter", "Mouse leave", "Mouse down", "Mouse up", "After delay", "Key", "Gamepad"];

/// Controllers a gamepad trigger can be bound to, in menu order.
pub const GAMEPAD_DEVICES: &[(&str, GamepadDevice)] = &[
    ("Any controller", GamepadDevice::Generic),
    ("Xbox", GamepadDevice::Xbox),
    ("PlayStation", GamepadDevice::PlayStation),
    ("Switch Pro", GamepadDevice::SwitchPro),
];

pub fn gamepad_device_index(d: GamepadDevice) -> usize {
    GAMEPAD_DEVICES.iter().position(|(_, x)| *x == d).unwrap_or(0)
}

pub fn trigger_index(t: &Trigger) -> usize {
    match t {
        Trigger::OnClick => 0,
        Trigger::OnDrag => 1,
        Trigger::WhileHovering => 2,
        Trigger::WhilePressing => 3,
        Trigger::MouseEnter { .. } => 4,
        Trigger::MouseLeave { .. } => 5,
        Trigger::MouseDown { .. } => 6,
        Trigger::MouseUp { .. } => 7,
        Trigger::AfterDelay { .. } => 8,
        Trigger::KeyPress { .. } => 9,
        Trigger::GamepadPress { .. } => 10,
    }
}

/// A fresh trigger of the i-th kind, keeping the delay of `prev` when both have one.
pub fn trigger_of_index(i: usize, prev: &Trigger) -> Trigger {
    let d = trigger_delay(prev).unwrap_or(match i {
        8 => 800,
        _ => 0,
    });
    match i {
        0 => Trigger::OnClick,
        1 => Trigger::OnDrag,
        2 => Trigger::WhileHovering,
        3 => Trigger::WhilePressing,
        4 => Trigger::MouseEnter { delay_ms: d },
        5 => Trigger::MouseLeave { delay_ms: d },
        6 => Trigger::MouseDown { delay_ms: d },
        7 => Trigger::MouseUp { delay_ms: d },
        8 => Trigger::AfterDelay { delay_ms: d },
        9 => Trigger::KeyPress { keys: if let Trigger::KeyPress { keys } = prev { keys.clone() } else { "Space".into() } },
        _ => match prev {
            Trigger::GamepadPress { .. } => prev.clone(),
            _ => Trigger::GamepadPress { device: GamepadDevice::Generic, buttons: "A".into() },
        },
    }
}

pub fn trigger_delay(t: &Trigger) -> Option<u32> {
    match t {
        Trigger::MouseEnter { delay_ms }
        | Trigger::MouseLeave { delay_ms }
        | Trigger::MouseDown { delay_ms }
        | Trigger::MouseUp { delay_ms }
        | Trigger::AfterDelay { delay_ms } => Some(*delay_ms),
        _ => None,
    }
}

pub fn with_delay(t: &Trigger, ms: u32) -> Trigger {
    match t {
        Trigger::MouseEnter { .. } => Trigger::MouseEnter { delay_ms: ms },
        Trigger::MouseLeave { .. } => Trigger::MouseLeave { delay_ms: ms },
        Trigger::MouseDown { .. } => Trigger::MouseDown { delay_ms: ms },
        Trigger::MouseUp { .. } => Trigger::MouseUp { delay_ms: ms },
        Trigger::AfterDelay { .. } => Trigger::AfterDelay { delay_ms: ms },
        other => other.clone(),
    }
}

pub fn trigger_label(t: &Trigger) -> String {
    match t {
        Trigger::KeyPress { keys } if !keys.is_empty() => format!("Key {keys}"),
        Trigger::GamepadPress { buttons, .. } if !buttons.is_empty() => format!("Gamepad {buttons}"),
        Trigger::AfterDelay { delay_ms } => format!("After delay {delay_ms}ms"),
        other => TRIGGERS[trigger_index(other)].to_string(),
    }
}

pub const ACTIONS: &[&str] = &[
    "Navigate to",
    "Back",
    "Open overlay",
    "Swap overlay",
    "Close overlay",
    "Scroll to",
    "Open link",
    "Change to",
    "Set variable",
    "Set variable mode",
    "Conditional",
    "Control video",
    "Control animation",
];

pub fn action_index(a: &Action) -> usize {
    match a {
        Action::Navigate { .. } => 0,
        Action::Back => 1,
        Action::OpenOverlay { .. } => 2,
        Action::SwapOverlay { .. } => 3,
        Action::CloseOverlay => 4,
        Action::ScrollTo { .. } => 5,
        Action::OpenUrl { .. } => 6,
        Action::ChangeTo { .. } => 7,
        Action::SetVariable { .. } => 8,
        Action::SetVariableMode { .. } => 9,
        Action::Conditional { .. } => 10,
        Action::MediaControl { .. } => 11,
        Action::Animation { .. } => 12,
    }
}

pub fn action_dest(a: &Action) -> Option<NodeId> {
    match a {
        Action::Navigate { dest, .. }
        | Action::OpenOverlay { dest, .. }
        | Action::SwapOverlay { dest }
        | Action::ScrollTo { dest, .. }
        | Action::ChangeTo { dest }
        | Action::MediaControl { dest, .. } => Some(*dest),
        _ => None,
    }
}

/// A new action of the i-th kind, keeping the destination of `prev` when it has one.
/// `doc` supplies the first variable / collection / video for the variable and media actions.
pub fn action_of_index(doc: &Document, i: usize, prev: &Action, fallback_dest: NodeId) -> Action {
    let dest = action_dest(prev).unwrap_or(fallback_dest);
    match i {
        0 => Action::Navigate { dest, preserve_scroll: false },
        1 => Action::Back,
        2 => Action::OpenOverlay { dest, position: OverlayPosition::Center, offset: Vec2::ZERO, close_on_click_outside: true, background: None },
        3 => Action::SwapOverlay { dest },
        4 => Action::CloseOverlay,
        5 => Action::ScrollTo { dest, offset: Vec2::ZERO },
        6 => Action::OpenUrl { url: "https://".into(), new_tab: true },
        7 => Action::ChangeTo { dest },
        8 => default_set_variable(doc),
        9 => {
            let coll = doc.variable_collections.values().next();
            Action::SetVariableMode {
                collection: coll.map(|c| c.id.clone()).unwrap_or_default(),
                mode: coll.map(|c| c.default_mode.clone()).unwrap_or_default(),
            }
        }
        10 => Action::Conditional { branches: vec![(Expression::boolean(true), vec![Action::Back])], otherwise: Vec::new() },
        11 => Action::MediaControl { dest: video_layers(doc).first().copied().unwrap_or(dest), op: MediaOp::TogglePlay },
        _ => Action::Animation { dest: None, op: AnimationOp::RESUME },
    }
}

pub const ANIMATION_OPS: &[&str] = &["Play", "Play from start", "Pause", "Restart", "Toggle play/pause"];

pub fn animation_op_index(op: &AnimationOp) -> usize {
    match op {
        AnimationOp::Play { from_start: false } => 0,
        AnimationOp::Play { from_start: true } => 1,
        AnimationOp::Pause => 2,
        AnimationOp::Restart => 3,
        AnimationOp::TogglePlay => 4,
    }
}

pub fn animation_op_of_index(i: usize) -> AnimationOp {
    match i {
        0 => AnimationOp::Play { from_start: false },
        1 => AnimationOp::Play { from_start: true },
        2 => AnimationOp::Pause,
        3 => AnimationOp::Restart,
        _ => AnimationOp::TogglePlay,
    }
}

/// Frames (top-level, with their names) that have a Motion timeline worth controlling: any
/// layer inside animates.
pub fn animated_frames(doc: &Document, frames: &[NodeId]) -> Vec<NodeId> {
    frames
        .iter()
        .copied()
        .filter(|f| {
            let mut any = false;
            doc.walk(*f, |n, _| any |= !n.motion.is_empty());
            any
        })
        .collect()
}

/// "Set variable" on the first variable, with a value that suits its type.
pub fn default_set_variable(doc: &Document) -> Action {
    let first = doc.variable_collections.values().flat_map(|c| c.variables.iter()).find_map(|id| doc.variables.get(id));
    match first {
        Some(v) => Action::SetVariable { variable: v.id.clone(), value: default_value_for(v.kind, v, doc) },
        None => Action::SetVariable { variable: String::new(), value: Expression::number(0.0) },
    }
}

/// A starting expression for `kind`: "<variable> + 1" for numbers, a plain literal otherwise.
pub fn default_value_for(kind: VariableKind, v: &Variable, _doc: &Document) -> Expression {
    match kind {
        VariableKind::Number => Expression::binary(BinaryOp::Add, Expression::var(v.id.clone()), Expression::number(1.0)),
        VariableKind::String => Expression::text(""),
        VariableKind::Boolean => Expression::logical_not(Expression::var(v.id.clone())),
        VariableKind::Color => Expression::Literal(ExprValue::Color(Color::BLACK)),
    }
}

/// Layers with a video fill, in document order.
pub fn video_layers(doc: &Document) -> Vec<NodeId> {
    doc.nodes.values().filter(|n| !n.prototype.videos.is_empty()).map(|n| n.id).collect()
}

pub const MEDIA_OPS: &[&str] = &["Play", "Pause", "Toggle play/pause", "Mute", "Unmute", "Toggle mute", "Skip by", "Skip to time", "Loop", "Don't loop"];

pub fn media_op_index(op: &MediaOp) -> usize {
    match op {
        MediaOp::Play => 0,
        MediaOp::Pause => 1,
        MediaOp::TogglePlay => 2,
        MediaOp::Mute => 3,
        MediaOp::Unmute => 4,
        MediaOp::ToggleMute => 5,
        MediaOp::SkipBy(_) => 6,
        MediaOp::SeekTo(_) => 7,
        MediaOp::SetLoop(true) => 8,
        MediaOp::SetLoop(false) => 9,
    }
}

pub fn media_op_of_index(i: usize, prev: &MediaOp) -> MediaOp {
    let secs = match prev {
        MediaOp::SkipBy(s) | MediaOp::SeekTo(s) => *s,
        _ => 5.0,
    };
    match i {
        0 => MediaOp::Play,
        1 => MediaOp::Pause,
        2 => MediaOp::TogglePlay,
        3 => MediaOp::Mute,
        4 => MediaOp::Unmute,
        5 => MediaOp::ToggleMute,
        6 => MediaOp::SkipBy(secs),
        7 => MediaOp::SeekTo(secs.max(0.0)),
        8 => MediaOp::SetLoop(true),
        _ => MediaOp::SetLoop(false),
    }
}

/// Whether the action plays an animation (so the editor shows the transition).
pub fn action_animates(a: &Action) -> bool {
    match a {
        Action::Conditional { branches, otherwise } => {
            branches.iter().any(|(_, acts)| acts.iter().any(action_animates)) || otherwise.iter().any(action_animates)
        }
        Action::Back
        | Action::CloseOverlay
        | Action::OpenUrl { .. }
        | Action::SetVariable { .. }
        | Action::SetVariableMode { .. }
        | Action::MediaControl { .. }
        | Action::Animation { .. } => false,
        _ => true,
    }
}

/// Calls `f` on every action, descending into conditional branches.
pub fn walk_actions<'a>(actions: &'a [Action], f: &mut impl FnMut(&'a Action)) {
    for a in actions {
        f(a);
        if let Action::Conditional { branches, otherwise } = a {
            for (_, acts) in branches {
                walk_actions(acts, f);
            }
            walk_actions(otherwise, f);
        }
    }
}

pub fn action_label(doc: &Document, a: &Action) -> String {
    let name = |id: &NodeId| doc.get(*id).map(|n| n.name.clone()).unwrap_or_else(|| "(missing)".into());
    match a {
        Action::Navigate { dest, .. } => format!("Navigate to {}", name(dest)),
        Action::Back => "Back".into(),
        Action::OpenOverlay { dest, .. } => format!("Open overlay {}", name(dest)),
        Action::SwapOverlay { dest } => format!("Swap overlay {}", name(dest)),
        Action::CloseOverlay => "Close overlay".into(),
        Action::ScrollTo { dest, .. } => format!("Scroll to {}", name(dest)),
        Action::OpenUrl { url, .. } => format!("Open link {url}"),
        Action::ChangeTo { dest } => format!("Change to {}", name(dest)),
        Action::SetVariable { variable, value } => format!("Set {} = {}", variable_name(doc, variable), expression_text(doc, value)),
        Action::SetVariableMode { collection, mode } => {
            let c = doc.variable_collections.get(collection);
            let m = c.and_then(|c| c.modes.iter().find(|m| &m.id == mode)).map(|m| m.name.clone()).unwrap_or_else(|| "(missing)".into());
            format!("Set mode {}: {m}", c.map(|c| c.name.as_str()).unwrap_or("(missing)"))
        }
        Action::Conditional { branches, otherwise } => {
            let first = branches.first().map(|(c, _)| expression_text(doc, c)).unwrap_or_default();
            let tail = if otherwise.is_empty() { "" } else { ", else" };
            format!("If {first}{tail}")
        }
        Action::MediaControl { dest, op } => format!("{} {}", MEDIA_OPS[media_op_index(op)], name(dest)),
        Action::Animation { dest, op } => {
            let target = dest.map(|d| name(&d)).unwrap_or_else(|| "this frame".into());
            format!("{} animation of {target}", ANIMATION_OPS[animation_op_index(op)])
        }
    }
}

// ------------------------------------------------------------- expressions

pub fn variable_name(doc: &Document, id: &str) -> String {
    doc.variables.get(id).map(|v| v.name.clone()).unwrap_or_else(|| if id.is_empty() { "(none)".into() } else { "(missing)".into() })
}

/// Text form of an expression, with variable names.
pub fn expression_text(doc: &Document, e: &Expression) -> String {
    e.to_source(&|id| doc.variables.get(id).map(|v| v.name.clone()))
}

/// Finds a variable id from a name as typed in an expression: exact name or id, else a unique
/// case-insensitive name, else a unique last segment of a grouped name ("Cart/count" for count).
pub fn find_variable(doc: &Document, name: &str) -> Option<String> {
    if doc.variables.contains_key(name) {
        return Some(name.to_string());
    }
    let by = |pred: &dyn Fn(&Variable) -> bool| {
        let hits: Vec<&Variable> = doc.variables.values().filter(|v| pred(v)).collect();
        (hits.len() == 1).then(|| hits[0].id.clone())
    };
    if let Some(id) = doc.variables.values().find(|v| v.name == name).map(|v| v.id.clone()) {
        return Some(id);
    }
    by(&|v| v.name.eq_ignore_ascii_case(name)).or_else(|| by(&|v| v.name.rsplit('/').next().is_some_and(|last| last.eq_ignore_ascii_case(name))))
}

/// Parses and type-checks expression text typed in the editor. `expected` is the type the value
/// must have (the variable's type for Set variable, boolean for conditions).
pub fn parse_expression_text(doc: &Document, text: &str, expected: Option<VariableKind>) -> Result<Expression, String> {
    let e = parse_expression(text, &|n| find_variable(doc, n)).map_err(|e| e.to_string())?;
    let ty = e.type_of(&|id| doc.variables.get(id).map(|v| v.kind))?;
    match expected {
        // Anything can be stored in a text variable (numbers and booleans are written out).
        Some(VariableKind::String) | None => {}
        Some(k) if ExprType::from(k) != ty => return Err(format!("Expected a {} value, not {}", ExprType::from(k).name(), ty.name())),
        Some(_) => {}
    }
    Ok(e)
}

/// Same as [`parse_expression_text`] for a condition.
pub fn parse_condition_text(doc: &Document, text: &str) -> Result<Expression, String> {
    let e = parse_expression_text(doc, text, None)?;
    let ty = e.type_of(&|id| doc.variables.get(id).map(|v| v.kind))?;
    if ty != ExprType::Boolean {
        return Err(format!("A condition must be true or false, not {}", ty.name()));
    }
    Ok(e)
}

pub const ANIMATIONS: &[&str] = &["Instant", "Dissolve", "Smart animate", "Move in", "Move out", "Push", "Slide in", "Slide out"];

pub fn animation_index(t: &Option<Transition>) -> usize {
    match t.map(|t| t.kind) {
        None => 0,
        Some(TransitionKind::Dissolve) => 1,
        Some(TransitionKind::SmartAnimate) => 2,
        Some(TransitionKind::MoveIn(_)) => 3,
        Some(TransitionKind::MoveOut(_)) => 4,
        Some(TransitionKind::Push(_)) => 5,
        Some(TransitionKind::SlideIn(_)) => 6,
        Some(TransitionKind::SlideOut(_)) => 7,
    }
}

pub fn transition_direction(k: TransitionKind) -> Option<Direction> {
    match k {
        TransitionKind::MoveIn(d) | TransitionKind::MoveOut(d) | TransitionKind::Push(d) | TransitionKind::SlideIn(d) | TransitionKind::SlideOut(d) => Some(d),
        _ => None,
    }
}

pub fn with_direction(k: TransitionKind, d: Direction) -> TransitionKind {
    match k {
        TransitionKind::MoveIn(_) => TransitionKind::MoveIn(d),
        TransitionKind::MoveOut(_) => TransitionKind::MoveOut(d),
        TransitionKind::Push(_) => TransitionKind::Push(d),
        TransitionKind::SlideIn(_) => TransitionKind::SlideIn(d),
        TransitionKind::SlideOut(_) => TransitionKind::SlideOut(d),
        other => other,
    }
}

/// The transition for the i-th animation entry, keeping easing, duration and direction of `prev`.
pub fn transition_of_index(i: usize, prev: &Option<Transition>) -> Option<Transition> {
    let dir = prev.and_then(|t| transition_direction(t.kind)).unwrap_or(Direction::Left);
    let kind = match i {
        0 => return None,
        1 => TransitionKind::Dissolve,
        2 => TransitionKind::SmartAnimate,
        3 => TransitionKind::MoveIn(dir),
        4 => TransitionKind::MoveOut(dir),
        5 => TransitionKind::Push(dir),
        6 => TransitionKind::SlideIn(dir),
        _ => TransitionKind::SlideOut(dir),
    };
    Some(prev.map(|t| Transition { kind, ..t }).unwrap_or(Transition { kind, easing: Easing::EaseOut, duration_ms: 300 }))
}

pub fn direction_name(d: Direction) -> &'static str {
    match d {
        Direction::Left => "Left",
        Direction::Right => "Right",
        Direction::Top => "Top",
        Direction::Bottom => "Bottom",
    }
}

pub fn position_name(p: OverlayPosition) -> &'static str {
    match p {
        OverlayPosition::Center => "Center",
        OverlayPosition::TopLeft => "Top left",
        OverlayPosition::TopCenter => "Top center",
        OverlayPosition::TopRight => "Top right",
        OverlayPosition::BottomLeft => "Bottom left",
        OverlayPosition::BottomCenter => "Bottom center",
        OverlayPosition::BottomRight => "Bottom right",
        OverlayPosition::Manual => "Manual",
    }
}

pub const OVERFLOWS: &[(&str, Overflow)] = &[
    ("No scrolling", Overflow::None),
    ("Horizontal scrolling", Overflow::Horizontal),
    ("Vertical scrolling", Overflow::Vertical),
    ("Horizontal & vertical", Overflow::Both),
];

// ------------------------------------------------------------------ queries

/// Top-level frame (child of its page) that contains `id`, or `id` itself.
pub fn top_frame(doc: &Document, mut id: NodeId) -> Option<NodeId> {
    loop {
        let p = doc.get(id)?.parent?;
        if matches!(doc.get(p)?.kind, NodeKind::Page { .. }) {
            return Some(id);
        }
        id = p;
    }
}

/// Frames a reaction on `src` may target: the top-level frames of its page.
pub fn destination_frames(doc: &Document, src: NodeId) -> Vec<NodeId> {
    let Some(page) = doc.page_of(src) else { return Vec::new() };
    top_level_frames(doc, page)
}

/// (target frame, is overlay) pairs of every connection `id` draws a noodle to.
pub fn connections(doc: &Document, id: NodeId) -> Vec<NodeId> {
    let Some(n) = doc.get(id) else { return Vec::new() };
    let mut out = Vec::new();
    for r in &n.prototype.reactions {
        walk_actions(&r.actions, &mut |a| {
            if let Action::Navigate { dest, .. } | Action::OpenOverlay { dest, .. } | Action::SwapOverlay { dest } = a {
                if let Some(f) = top_frame(doc, *dest) {
                    if doc.page_of(f) == doc.page_of(id) && !out.contains(&f) {
                        out.push(f);
                    }
                }
            }
        });
    }
    out
}

pub fn has_interactions(doc: &Document, id: NodeId) -> bool {
    doc.get(id).is_some_and(|n| !n.prototype.reactions.is_empty())
}

/// Next flow name: "Flow N" with N one above the highest existing "Flow N".
pub fn next_flow_name(flows: &[Flow]) -> String {
    let max = flows.iter().filter_map(|f| f.name.strip_prefix("Flow ").and_then(|n| n.trim().parse::<u32>().ok())).max().unwrap_or(0);
    format!("Flow {}", max.max(flows.len() as u32) + 1)
}

// ------------------------------------------------------------------- edits

impl App {
    /// Edits the prototype settings of `ids` as one undo step.
    pub fn proto_edit(&mut self, ids: &[NodeId], mut f: impl FnMut(&mut PrototypeProps)) {
        self.edit_nodes(ids, |n| f(&mut n.prototype));
    }

    /// Replaces the flow list as one undo step.
    pub fn proto_set_flows(&mut self, flows: Vec<Flow>) {
        if flows == self.doc.flows {
            return;
        }
        if let Err(e) = self.hist.exec(&mut self.doc, Command::SetFlows { flows }) {
            self.show_toast(format!("Edit failed: {e}"), true);
        }
        self.changed();
    }

    pub fn proto_add_flow(&mut self, start: NodeId) {
        let Some(page) = self.doc.page_of(start) else { return };
        let mut flows = self.doc.flows.clone();
        if flows.iter().any(|f| f.start == start) {
            return;
        }
        let name = next_flow_name(&flows);
        flows.push(Flow { name, page, start, description: String::new() });
        self.proto_set_flows(flows);
    }

    /// Default reaction for the + button: On click -> navigate to the next frame.
    pub fn proto_default_reaction(&self, src: NodeId) -> Reaction {
        let frames = destination_frames(&self.doc, src);
        let own = top_frame(&self.doc, src);
        let dest =
            own.and_then(|o| frames.iter().position(|f| *f == o)).map(|i| frames[(i + 1) % frames.len()]).or_else(|| frames.first().copied()).unwrap_or(src);
        Reaction {
            trigger: Trigger::OnClick,
            actions: vec![Action::Navigate { dest, preserve_scroll: false }],
            transition: Some(Transition { kind: TransitionKind::Dissolve, easing: Easing::EaseOut, duration_ms: 300 }),
        }
    }

    /// Adds a reaction to `src` and opens it in the editor.
    pub fn proto_add_reaction(&mut self, src: NodeId) {
        let r = self.proto_default_reaction(src);
        let mut idx = 0;
        self.proto_edit(&[src], |p| {
            p.reactions.push(r.clone());
            idx = p.reactions.len() - 1;
        });
        self.proto.editing = Some((src, idx));
    }

    /// Drag-from-hotspot: On click -> Navigate to `dest_frame` (instant, Figma's default).
    pub fn proto_connect(&mut self, src: NodeId, dest_frame: NodeId) {
        let r = Reaction { trigger: Trigger::OnClick, actions: vec![Action::Navigate { dest: dest_frame, preserve_scroll: false }], transition: None };
        self.proto_edit(&[src], |p| p.reactions.push(r.clone()));
    }

    /// Opens the Present view from the selected frame, else the page's first flow, else the first frame.
    pub fn start_present(&mut self) {
        crate::decks::ui::flush_notes(self);
        let start = if self.is_deck() {
            self.active_slide()
        } else {
            self.selection
                .first()
                .and_then(|s| top_frame(&self.doc, *s))
                .or_else(|| self.doc.flows.iter().find(|f| f.page == self.page).map(|f| f.start))
                .or_else(|| top_level_frames(&self.doc, self.page).first().copied())
        };
        match start {
            Some(s) => self.proto.present = Some(crate::present::PresentState::new(&self.doc, s)),
            None => self.show_toast(if self.is_deck() { "Nothing to present: add a slide first" } else { "Nothing to present: add a frame first" }, true),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc3() -> (Document, NodeId, NodeId, NodeId, NodeId) {
        let mut d = Document::new("t");
        let page = d.add_page("p");
        let mut f = Vec::new();
        for i in 0..3 {
            let id = d.alloc_id();
            f.push(d.insert(page, Node::frame(id, Rect::new(i as f32 * 500.0, 0.0, 400.0, 800.0))).unwrap());
        }
        let id = d.alloc_id();
        let b = d.insert(f[0], Node::rectangle(id, Rect::new(0.0, 0.0, 10.0, 10.0))).unwrap();
        (d, f[0], f[1], f[2], b)
    }

    #[test]
    fn deck_presentation_starts_the_active_or_selected_slide() {
        let (mut doc, a, b, _, nested) = doc3();
        doc.kind = DocumentKind::Deck;
        for id in [a, b] {
            doc.get_mut(id).unwrap().slide = Some(SlideProps::default());
        }
        let mut app = App::new(doc);
        app.selection.clear();
        app.decks.active = Some(b);
        app.start_present();
        assert_eq!(app.proto.present.as_ref().unwrap().player.current, b);
        app.proto.present = None;
        app.selection = vec![nested];
        app.start_present();
        assert_eq!(app.proto.present.as_ref().unwrap().player.current, a);
    }

    #[test]
    fn trigger_round_trips_through_indexes() {
        for i in 0..TRIGGERS.len() {
            let t = trigger_of_index(i, &Trigger::OnClick);
            assert_eq!(trigger_index(&t), i);
        }
        let t = trigger_of_index(4, &Trigger::AfterDelay { delay_ms: 250 });
        assert_eq!(t, Trigger::MouseEnter { delay_ms: 250 }, "delay carries over");
        assert_eq!(trigger_of_index(8, &Trigger::OnClick), Trigger::AfterDelay { delay_ms: 800 });
        assert_eq!(with_delay(&t, 90), Trigger::MouseEnter { delay_ms: 90 });
        assert_eq!(with_delay(&Trigger::OnClick, 90), Trigger::OnClick);
    }

    #[test]
    fn action_round_trips_and_keeps_destination() {
        let prev = Action::Navigate { dest: NodeId(7), preserve_scroll: true };
        let doc = Document::new("t");
        for i in 0..ACTIONS.len() {
            assert_eq!(action_index(&action_of_index(&doc, i, &prev, NodeId(1))), i);
        }
        assert_eq!(action_dest(&action_of_index(&doc, 2, &prev, NodeId(1))), Some(NodeId(7)));
        assert_eq!(action_dest(&action_of_index(&doc, 0, &Action::Back, NodeId(1))), Some(NodeId(1)));
        assert!(!action_animates(&Action::Back) && action_animates(&prev));
    }

    #[test]
    fn animation_picker_keeps_duration_and_direction() {
        let prev = Some(Transition { kind: TransitionKind::Push(Direction::Top), easing: Easing::Linear, duration_ms: 777 });
        let t = transition_of_index(3, &prev).unwrap();
        assert_eq!(t.kind, TransitionKind::MoveIn(Direction::Top));
        assert_eq!((t.duration_ms, t.easing), (777, Easing::Linear));
        assert_eq!(transition_of_index(0, &prev), None);
        let fresh = transition_of_index(1, &None).unwrap();
        assert_eq!((fresh.kind, fresh.duration_ms), (TransitionKind::Dissolve, 300));
        for i in 0..ANIMATIONS.len() {
            assert_eq!(animation_index(&transition_of_index(i, &None)), i);
        }
        assert_eq!(with_direction(TransitionKind::Dissolve, Direction::Left), TransitionKind::Dissolve);
        assert_eq!(transition_direction(TransitionKind::SlideOut(Direction::Right)), Some(Direction::Right));
    }

    #[test]
    fn labels_name_destinations() {
        let (d, a, b, _c, _) = doc3();
        assert_eq!(action_label(&d, &Action::Navigate { dest: b, preserve_scroll: false }), "Navigate to Frame");
        assert_eq!(action_label(&d, &Action::Navigate { dest: NodeId(99), preserve_scroll: false }), "Navigate to (missing)");
        assert_eq!(trigger_label(&Trigger::AfterDelay { delay_ms: 5 }), "After delay 5ms");
        assert_eq!(trigger_label(&Trigger::KeyPress { keys: "K".into() }), "Key K");
        assert_eq!(trigger_label(&Trigger::WhileHovering), "While hovering");
        let _ = a;
    }

    #[test]
    fn connections_lists_destination_frames_once() {
        let (mut d, _a, b, c, btn) = doc3();
        let r = |dest| Reaction { trigger: Trigger::OnClick, actions: vec![Action::Navigate { dest, preserve_scroll: false }], transition: None };
        d.get_mut(btn).unwrap().prototype.reactions =
            vec![r(b), r(b), r(c), Reaction { trigger: Trigger::OnClick, actions: vec![Action::Back], transition: None }];
        assert_eq!(connections(&d, btn), vec![b, c]);
        assert!(has_interactions(&d, btn));
        assert!(connections(&d, b).is_empty());
    }

    fn vars_doc() -> Document {
        let mut d = Document::new("t");
        d.add_page("p");
        let mut s = VariableStore::default();
        let c = s.add_collection("Cart");
        s.add_variable(&c, "Counter/total", VariableKind::Number).unwrap();
        s.add_variable(&c, "name", VariableKind::String).unwrap();
        s.add_variable(&c, "ready", VariableKind::Boolean).unwrap();
        let _ = s.command().apply(&mut d);
        d
    }

    #[test]
    fn expression_text_resolves_names_ids_and_unique_short_names() {
        let d = vars_doc();
        let total = d.variables.values().find(|v| v.name == "Counter/total").unwrap().id.clone();
        assert_eq!(find_variable(&d, "Counter/total"), Some(total.clone()));
        assert_eq!(find_variable(&d, &total), Some(total.clone()), "ids work too");
        assert_eq!(find_variable(&d, "total"), Some(total.clone()), "last segment of a grouped name");
        assert_eq!(find_variable(&d, "NAME").as_deref(), d.variables.values().find(|v| v.name == "name").map(|v| v.id.as_str()));
        assert_eq!(find_variable(&d, "nope"), None);
        let e = parse_expression_text(&d, "{Counter/total} + $total * 2", None).unwrap();
        assert_eq!(expression_text(&d, &e), "{Counter/total} + {Counter/total} * 2");
        assert_eq!(variable_name(&d, &total), "Counter/total");
        assert_eq!(variable_name(&d, "x"), "(missing)");
        assert_eq!(variable_name(&d, ""), "(none)");
    }

    #[test]
    fn expression_text_is_checked_against_the_variable_type() {
        let d = vars_doc();
        let ok = |t: &str, k| parse_expression_text(&d, t, k).map(|_| ());
        assert!(ok("$total + 1", Some(VariableKind::Number)).is_ok());
        assert!(ok("\"x\"", Some(VariableKind::Number)).unwrap_err().contains("Expected a number"));
        assert!(ok("$ready", Some(VariableKind::Number)).is_err());
        assert!(ok("$total + 1", Some(VariableKind::String)).is_ok(), "numbers are written out into text variables");
        assert!(ok("true", Some(VariableKind::Boolean)).is_ok());
        assert!(ok("1 +", None).unwrap_err().contains("end of expression"));
        assert!(ok("$nope", None).unwrap_err().contains("Unknown variable"));
        assert!(parse_condition_text(&d, "$total > 3 && $ready").is_ok());
        assert!(parse_condition_text(&d, "$total + 3").unwrap_err().contains("true or false"));
    }

    #[test]
    fn new_actions_have_labels_and_defaults() {
        let d = vars_doc();
        let total = d.variables.values().find(|v| v.name == "Counter/total").unwrap().id.clone();
        let set = action_of_index(&d, 8, &Action::Back, NodeId(1));
        assert_eq!(action_label(&d, &set), "Set Counter/total = {Counter/total} + 1");
        let mode = action_of_index(&d, 9, &Action::Back, NodeId(1));
        assert!(action_label(&d, &mode).starts_with("Set mode Cart: "));
        let cond = action_of_index(&d, 10, &Action::Back, NodeId(1));
        assert_eq!(action_label(&d, &cond), "If true");
        let cond = Action::Conditional { branches: vec![(Expression::var(total), vec![])], otherwise: vec![Action::Back] };
        assert_eq!(action_label(&d, &cond), "If {Counter/total}, else");
        let media = action_of_index(&d, 11, &Action::Back, NodeId(5));
        assert_eq!(action_label(&d, &media), "Toggle play/pause (missing)");
        // nothing animates for variable and media actions; a conditional animates when a branch does
        assert!(!action_animates(&set) && !action_animates(&mode) && !action_animates(&media));
        let nav = Action::Navigate { dest: NodeId(1), preserve_scroll: false };
        let c2 = Action::Conditional { branches: vec![(Expression::boolean(true), vec![nav])], otherwise: vec![] };
        assert!(action_animates(&c2));
        // an empty document gives harmless placeholders
        let empty = Document::new("e");
        assert!(matches!(action_of_index(&empty, 8, &Action::Back, NodeId(1)), Action::SetVariable { variable, .. } if variable.is_empty()));
        assert!(matches!(action_of_index(&empty, 9, &Action::Back, NodeId(1)), Action::SetVariableMode { collection, .. } if collection.is_empty()));
    }

    #[test]
    fn media_ops_round_trip_through_the_menu_and_keep_seconds() {
        for i in 0..MEDIA_OPS.len() {
            assert_eq!(media_op_index(&media_op_of_index(i, &MediaOp::Play)), i);
        }
        assert_eq!(media_op_of_index(7, &MediaOp::SkipBy(12.0)), MediaOp::SeekTo(12.0));
        assert_eq!(media_op_of_index(6, &MediaOp::SeekTo(3.0)), MediaOp::SkipBy(3.0));
        assert_eq!(media_op_of_index(7, &MediaOp::SkipBy(-4.0)), MediaOp::SeekTo(0.0), "a time is never negative");
    }

    #[test]
    fn noodles_follow_navigation_inside_conditionals() {
        let (mut d, _a, b, c, btn) = doc3();
        let nav = |dest| Action::Navigate { dest, preserve_scroll: false };
        d.get_mut(btn).unwrap().prototype.reactions = vec![Reaction {
            trigger: Trigger::OnClick,
            actions: vec![Action::Conditional { branches: vec![(Expression::boolean(true), vec![nav(b)])], otherwise: vec![nav(c)] }],
            transition: None,
        }];
        assert_eq!(connections(&d, btn), vec![b, c]);
        let mut seen = 0;
        walk_actions(&d.get(btn).unwrap().prototype.reactions[0].actions, &mut |_| seen += 1);
        assert_eq!(seen, 3, "the conditional and both nested navigations");
    }

    #[test]
    fn gamepad_trigger_indexes_and_labels() {
        let t = trigger_of_index(10, &Trigger::OnClick);
        assert_eq!(t, Trigger::GamepadPress { device: GamepadDevice::Generic, buttons: "A".into() });
        assert_eq!(trigger_index(&t), 10);
        assert_eq!(trigger_label(&t), "Gamepad A");
        let keep = Trigger::GamepadPress { device: GamepadDevice::Xbox, buttons: "LB".into() };
        assert_eq!(trigger_of_index(10, &keep), keep);
        assert_eq!(gamepad_device_index(GamepadDevice::SwitchPro), 3);
    }

    #[test]
    fn flow_names_count_up() {
        let mk = |n: &str| Flow { name: n.into(), page: NodeId(1), start: NodeId(2), description: String::new() };
        assert_eq!(next_flow_name(&[]), "Flow 1");
        assert_eq!(next_flow_name(&[mk("Flow 1"), mk("Flow 4")]), "Flow 5");
        assert_eq!(next_flow_name(&[mk("Onboarding")]), "Flow 2");
    }

    #[test]
    fn default_reaction_targets_the_next_frame() {
        let (d, _a, b, _c, btn) = doc3();
        let app = App::new(d);
        let r = app.proto_default_reaction(btn);
        assert_eq!(r.trigger, Trigger::OnClick);
        assert_eq!(r.actions, vec![Action::Navigate { dest: b, preserve_scroll: false }]);
    }

    #[test]
    fn edits_are_single_undo_steps() {
        let (d, _a, b, c, btn) = doc3();
        let mut app = App::new(d);
        app.proto_add_reaction(btn);
        assert_eq!(app.doc.get(btn).unwrap().prototype.reactions.len(), 1);
        assert_eq!(app.proto.editing, Some((btn, 0)));
        app.proto_edit(&[btn], |p| p.reactions[0].actions = vec![Action::Navigate { dest: c, preserve_scroll: false }]);
        app.proto_connect(btn, b);
        assert_eq!(app.doc.get(btn).unwrap().prototype.reactions.len(), 2);
        app.undo();
        assert_eq!(app.doc.get(btn).unwrap().prototype.reactions.len(), 1);
        app.undo();
        assert!(matches!(app.doc.get(btn).unwrap().prototype.reactions[0].actions[0], Action::Navigate { dest, .. } if dest == b));
        app.undo();
        assert!(app.doc.get(btn).unwrap().prototype.reactions.is_empty());
        app.redo();
        assert_eq!(app.doc.get(btn).unwrap().prototype.reactions.len(), 1);
    }

    #[test]
    fn flows_are_undoable() {
        let (d, a, b, _c, _) = doc3();
        let mut app = App::new(d);
        app.proto_add_flow(a);
        app.proto_add_flow(a);
        assert_eq!(app.doc.flows.len(), 1, "a frame starts at most one flow");
        app.proto_add_flow(b);
        assert_eq!(app.doc.flows.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["Flow 1", "Flow 2"]);
        app.undo();
        assert_eq!(app.doc.flows.len(), 1);
        app.undo();
        assert!(app.doc.flows.is_empty());
        app.redo();
        assert_eq!(app.doc.flows[0].start, a);
    }
}
