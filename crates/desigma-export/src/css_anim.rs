//! Dev Mode code for Motion animations: CSS `@keyframes`, SwiftUI `keyframeAnimator` and
//! Compose `keyframes` snippets for every animated layer under a node.
//!
//! The CSS mapping animates the *individual transform properties* (`translate`, `rotate`,
//! `scale`) relative to the layer's static place, so it layers on top of the position and
//! rotation [`css_for`](crate::css_for) already prints. One `@keyframes` rule is written per
//! animated property, which lets each property keep its own keyframe times and easing:
//!
//! * keyframe times become percentages of the timeline duration;
//! * the easing of a segment is the `animation-timing-function` of the keyframe it starts at:
//!   `cubic-bezier()` for curves, `step-end` for Hold and a sampled `linear()` for springs;
//! * properties driven by animation presets (clips) are baked into sampled keyframes
//!   (about every 1/30 s, at most 120), because clips compose with keyframes at runtime.

use crate::codegen::{kebab, Out};
use crate::util::{css_color, num, num2};
use desigma_doc::motion::{base_position, base_rotation_deg, value_at, AnimProperty, AnimValue, Ease, Track};
use desigma_doc::{Document, Node, NodeId, NodeKind};
use std::collections::BTreeSet;

/// CSS timing function of the segment that starts with `ease`.
pub fn timing_function(ease: &Ease) -> String {
    if matches!(ease, Ease::Hold) {
        return "step-end".into();
    }
    if matches!(ease, Ease::Linear) {
        return "linear".into();
    }
    if let Some([x1, y1, x2, y2]) = ease.control_points() {
        return format!("cubic-bezier({}, {}, {}, {})", num2(x1.clamp(0.0, 1.0)), num2(y1), num2(x2.clamp(0.0, 1.0)), num2(y2));
    }
    // springs have no closed form in CSS: sample the response curve
    let n = 30;
    let pts: Vec<String> = (0..=n).map(|i| num2(ease.ease(i as f32 / n as f32))).collect();
    format!("linear({})", pts.join(", "))
}

fn declaration(node: &Node, p: AnimProperty, v: &AnimValue) -> Option<String> {
    Some(match (p, v) {
        (AnimProperty::Position, AnimValue::Vec2(v)) => {
            let b = base_position(node);
            format!("translate: {}px {}px", num(v.x - b.x), num(v.y - b.y))
        }
        (AnimProperty::Scale, AnimValue::Vec2(v)) => format!("scale: {} {}", num2(v.x), num2(v.y)),
        (AnimProperty::Rotation, AnimValue::Scalar(d)) => format!("rotate: {}deg", num2(-(d - base_rotation_deg(node)))),
        (AnimProperty::Width, AnimValue::Scalar(v)) => format!("width: {}px", num(*v)),
        (AnimProperty::Height, AnimValue::Scalar(v)) => format!("height: {}px", num(*v)),
        (AnimProperty::Opacity, AnimValue::Scalar(v)) => format!("opacity: {}", num2(*v)),
        (AnimProperty::CornerRadius, AnimValue::Scalar(v)) => format!("border-radius: {}px", num(*v)),
        (AnimProperty::FillColor, AnimValue::Color(c)) => {
            let prop = if matches!(node.kind, NodeKind::Text(_)) { "color" } else { "background-color" };
            format!("{prop}: {}", css_color(c, 1.0))
        }
        (AnimProperty::StrokeColor, AnimValue::Color(c)) => format!("border-color: {}", css_color(c, 1.0)),
        (AnimProperty::StrokeWeight, AnimValue::Scalar(v)) => format!("border-width: {}px", num(*v)),
        (AnimProperty::BlurRadius, AnimValue::Scalar(v)) => format!("filter: blur({}px)", num(v.max(0.0))),
        // the shadow properties share one `box-shadow` rule: see `shadow_frames`
        _ => return None,
    })
}

fn pct(t_ms: u32, duration: u32) -> String {
    format!("{}%", num2((t_ms.min(duration) as f32 / duration.max(1) as f32) * 100.0))
}

/// One property's keyframes as `(time ms, value, easing of the segment that starts here)`.
fn frames(node: &Node, p: AnimProperty, duration: u32, baked: bool) -> Vec<(u32, AnimValue, Option<Ease>)> {
    if baked || node.motion.track(p).is_none() {
        let step = (duration / 120).max(33);
        let mut times: Vec<u32> = (0..duration).step_by(step as usize).collect();
        times.push(duration);
        return times.into_iter().filter_map(|t| value_at(node, p, t as f32).map(|v| (t, v, None))).collect();
    }
    let Some(tr): Option<&Track> = node.motion.track(p) else { return vec![] };
    let mut out: Vec<(u32, AnimValue, Option<Ease>)> = Vec::new();
    for (i, k) in tr.keys.iter().enumerate() {
        let timing = (i + 1 < tr.keys.len()).then_some(k.easing);
        out.push((k.time_ms.min(duration), k.value, timing));
    }
    if let Some(first) = out.first().cloned() {
        if first.0 > 0 {
            out.insert(0, (0, first.1, None));
        }
    }
    if let Some(last) = out.last().cloned() {
        if last.0 < duration {
            out.push((duration, last.1, None));
        }
    }
    // keys past the end collapse onto it: keep the first of equal times
    out.dedup_by_key(|f| f.0);
    out
}

const SHADOW_PROPS: [AnimProperty; 4] = [AnimProperty::ShadowX, AnimProperty::ShadowY, AnimProperty::ShadowBlur, AnimProperty::ShadowColor];

fn is_shadow(p: AnimProperty) -> bool {
    SHADOW_PROPS.contains(&p)
}

/// Keyframes of the `box-shadow` of the first drop shadow: CSS cannot animate the offset, blur
/// and colour of a shadow separately, so the keyframe times of all animated shadow properties are
/// merged and each frame prints the full shadow. The easing of a frame is the one of the first
/// animated shadow track that has a key there.
fn shadow_frames(node: &Node, duration: u32) -> Vec<(u32, String, Option<Ease>)> {
    let tracks: Vec<&Track> = SHADOW_PROPS.iter().filter_map(|p| node.motion.track(*p)).filter(|t| t.visible && !t.is_empty()).collect();
    let spread = match node.effects.iter().find(|e| matches!(e, desigma_doc::Effect::DropShadow { .. })) {
        Some(desigma_doc::Effect::DropShadow { spread, .. }) => *spread,
        _ => return vec![],
    };
    let mut times: BTreeSet<u32> = tracks.iter().flat_map(|t| t.keys.iter().map(|k| k.time_ms.min(duration))).collect();
    times.insert(0);
    times.insert(duration);
    let times: Vec<u32> = times.into_iter().collect();
    let mut out = Vec::new();
    for (i, t) in times.iter().enumerate() {
        let num_at = |p| value_at(node, p, *t as f32).and_then(|v| v.scalar()).unwrap_or(0.0);
        let col = value_at(node, AnimProperty::ShadowColor, *t as f32).and_then(|v| v.color()).unwrap_or(desigma_doc::Color::BLACK);
        let v = format!(
            "{}px {}px {}px {}px {}",
            num2(num_at(AnimProperty::ShadowX)),
            num2(num_at(AnimProperty::ShadowY)),
            num2(num_at(AnimProperty::ShadowBlur)),
            num2(spread),
            css_color(&col, 1.0)
        );
        let timing = (i + 1 < times.len()).then(|| tracks.iter().find_map(|tr| tr.key_at(*t).map(|k| k.easing)).unwrap_or(Ease::Linear));
        out.push((*t, v, timing));
    }
    out
}

fn animated_properties(node: &Node) -> Vec<(AnimProperty, bool)> {
    let mut clip_props: BTreeSet<AnimProperty> = BTreeSet::new();
    for c in node.motion.clips.iter().filter(|c| c.visible) {
        clip_props.extend(c.kind.properties().iter().copied());
    }
    let mut all: BTreeSet<AnimProperty> = node.motion.tracks.iter().filter(|t| !t.is_empty() && t.visible).map(|t| t.property).collect();
    all.extend(clip_props.iter().copied());
    all.into_iter().map(|p| (p, clip_props.contains(&p))).collect()
}

/// CSS animation for `node` and everything below it: `@keyframes` rules and the `animation`
/// declaration of each animated layer's class. Empty when nothing is animated.
pub fn css_animation_for(doc: &Document, node: NodeId) -> String {
    let Some(page) = doc.page_of(node).or(Some(node)).filter(|p| doc.get(*p).is_some()) else { return String::new() };
    let tl = doc.timeline(page);
    let mut layers: Vec<NodeId> = Vec::new();
    doc.walk(node, |n, _| {
        if !n.motion.is_empty() {
            layers.push(n.id);
        }
    });
    let mut out = Out::new("  ");
    let mut used: BTreeSet<String> = BTreeSet::new();
    for id in layers {
        let Some(n) = doc.get(id) else { continue };
        let props = animated_properties(n);
        let mut class = kebab(&n.name);
        if !used.insert(class.clone()) {
            let mut i = 2;
            while !used.insert(format!("{class}-{i}")) {
                i += 1;
            }
            class = format!("{class}-{i}");
        }
        let mut names: Vec<String> = Vec::new();
        let sh = shadow_frames(n, tl.duration_ms);
        if props.iter().any(|(p, _)| is_shadow(*p)) && sh.len() >= 2 {
            let name = format!("{class}-shadow");
            if !out.is_empty() {
                out.line(0, "");
            }
            out.line(0, format!("@keyframes {name} {{"));
            for (t, v, timing) in &sh {
                match timing.as_ref().map(timing_function) {
                    Some(f) => out.line(1, format!("{} {{ box-shadow: {v}; animation-timing-function: {f}; }}", pct(*t, tl.duration_ms))),
                    None => out.line(1, format!("{} {{ box-shadow: {v}; }}", pct(*t, tl.duration_ms))),
                }
            }
            out.line(0, "}");
            names.push(name);
        }
        for (p, baked) in props {
            if is_shadow(p) {
                continue;
            }
            let fr = frames(n, p, tl.duration_ms, baked);
            if fr.len() < 2 {
                continue;
            }
            let name = format!("{class}-{}", kebab(p.name()));
            if !out.is_empty() {
                out.line(0, "");
            }
            out.line(0, format!("@keyframes {name} {{"));
            for (t, v, timing) in &fr {
                let Some(d) = declaration(n, p, v) else { continue };
                match timing.as_ref().map(timing_function) {
                    Some(f) => out.line(1, format!("{} {{ {d}; animation-timing-function: {f}; }}", pct(*t, tl.duration_ms))),
                    None => out.line(1, format!("{} {{ {d}; }}", pct(*t, tl.duration_ms))),
                }
            }
            out.line(0, "}");
            names.push(name);
        }
        if names.is_empty() {
            continue;
        }
        let iter = if tl.looping { "infinite" } else { "1" };
        let list: Vec<String> = names.iter().map(|nm| format!("{nm} {}ms linear both {iter}", tl.duration_ms)).collect();
        out.line(0, "");
        out.line(0, format!(".{class} {{"));
        out.line(1, format!("animation: {};", list.join(", ")));
        out.line(0, "}");
    }
    out.finish().trim_start().to_string()
}

// ------------------------------------------------------------------- SwiftUI and Compose

/// One animated number of a layer: `x`, `y`, `scaleX`, `scaleY`, `rotation` or `opacity`.
struct Channel {
    key: &'static str,
    frames: Vec<(u32, f32, Option<Ease>)>,
}

/// The channels both native generators support, plus the property names they leave out
/// (size, corner radius, stroke weight and colours).
fn channels(node: &Node, duration: u32) -> (Vec<Channel>, Vec<&'static str>) {
    let mut out = Vec::new();
    let mut skipped = Vec::new();
    for (p, baked) in animated_properties(node) {
        let fr = frames(node, p, duration, baked);
        if fr.len() < 2 {
            continue;
        }
        let split = |f: &dyn Fn(&AnimValue) -> Option<f32>, key: &'static str, out: &mut Vec<Channel>| {
            out.push(Channel { key, frames: fr.iter().filter_map(|(t, v, e)| f(v).map(|x| (*t, x, *e))).collect() });
        };
        match p {
            AnimProperty::Position => {
                let b = base_position(node);
                split(&|v| v.vec2().map(|v| v.x - b.x), "x", &mut out);
                split(&|v| v.vec2().map(|v| v.y - b.y), "y", &mut out);
            }
            AnimProperty::Scale => {
                split(&|v| v.vec2().map(|v| v.x), "scaleX", &mut out);
                split(&|v| v.vec2().map(|v| v.y), "scaleY", &mut out);
            }
            AnimProperty::Rotation => {
                let b = base_rotation_deg(node);
                split(&|v| v.scalar().map(|d| -(d - b)), "rotation", &mut out);
            }
            AnimProperty::Opacity => split(&|v| v.scalar(), "opacity", &mut out),
            other => skipped.push(other.name()),
        }
    }
    (out, skipped)
}

fn secs(ms: u32) -> String {
    num(ms as f32 / 1000.0)
}

fn animated_layers(doc: &Document, node: NodeId) -> Vec<NodeId> {
    let mut layers: Vec<NodeId> = Vec::new();
    doc.walk(node, |n, _| {
        if !n.motion.is_empty() {
            layers.push(n.id);
        }
    });
    layers
}

/// SwiftUI (iOS 17+) `keyframeAnimator` for every animated layer under `node`.
/// Position, scale, rotation and opacity are generated; curves are approximated by
/// `CubicKeyframe`, springs by `SpringKeyframe`, Hold by a `MoveKeyframe`. Size, corner radius
/// and colour animations are listed in a comment.
pub fn swiftui_animation_for(doc: &Document, node: NodeId) -> String {
    let Some(page) = doc.page_of(node).or(Some(node)) else { return String::new() };
    let tl = doc.timeline(page);
    let mut out = Out::new("    ");
    for id in animated_layers(doc, node) {
        let Some(n) = doc.get(id) else { continue };
        let (chs, skipped) = channels(n, tl.duration_ms);
        if chs.is_empty() {
            continue;
        }
        let ty = format!("{}Animation", crate::codegen::pascal(&n.name));
        if !out.is_empty() {
            out.line(0, "");
        }
        out.line(0, format!("// {}", n.name));
        out.line(0, format!("struct {ty} {{"));
        for c in &chs {
            out.line(1, format!("var {} = {}", c.key, num(c.frames[0].1)));
        }
        out.line(0, "}");
        out.line(0, "");
        out.line(0, format!(".keyframeAnimator(initialValue: {ty}(), repeating: {}) {{ view, v in", tl.looping));
        out.line(1, "view");
        let has = |k: &str| chs.iter().any(|c| c.key == k);
        if has("x") {
            out.line(2, ".offset(x: v.x, y: v.y)");
        }
        if has("scaleX") {
            out.line(2, ".scaleEffect(x: v.scaleX, y: v.scaleY)");
        }
        if has("rotation") {
            out.line(2, ".rotationEffect(.degrees(v.rotation))");
        }
        if has("opacity") {
            out.line(2, ".opacity(v.opacity)");
        }
        out.line(0, "} keyframes: { _ in");
        for c in &chs {
            out.line(1, format!("KeyframeTrack(\\.{}) {{", c.key));
            out.line(2, format!("MoveKeyframe({})", num(c.frames[0].1)));
            for w in c.frames.windows(2) {
                let (t0, v0, e) = w[0];
                let (t1, v1, _) = w[1];
                let d = secs(t1 - t0);
                let e = e.unwrap_or(Ease::Linear);
                if matches!(e, Ease::Hold) {
                    out.line(2, format!("LinearKeyframe({}, duration: {d})", num(v0)));
                    out.line(2, format!("MoveKeyframe({})", num(v1)));
                } else if let Some(b) = e.bounce() {
                    out.line(2, format!("SpringKeyframe({}, duration: {d}, spring: Spring(duration: {d}, bounce: {}))", num(v1), num2(b)));
                } else if e.is_linear() {
                    out.line(2, format!("LinearKeyframe({}, duration: {d})", num(v1)));
                } else {
                    out.line(2, format!("CubicKeyframe({}, duration: {d}) // {}", num(v1), timing_function(&e)));
                }
            }
            out.line(1, "}");
        }
        out.line(0, "}");
        if !skipped.is_empty() {
            out.line(0, format!("// not generated: {}", skipped.join(", ")));
        }
    }
    out.finish()
}

fn kotlin_f(v: f32) -> String {
    format!("{}f", num2(v))
}

fn kotlin_easing(e: &Option<Ease>) -> String {
    match e {
        None | Some(Ease::Linear) => String::new(),
        Some(Ease::Hold) => " using Easing { f -> if (f < 1f) 0f else 1f }".into(),
        Some(e) => match e.control_points() {
            Some([a, b, c, d]) => {
                format!(" using CubicBezierEasing({}, {}, {}, {})", kotlin_f(a.clamp(0.0, 1.0)), kotlin_f(b), kotlin_f(c.clamp(0.0, 1.0)), kotlin_f(d))
            }
            // no spring easing inside `keyframes`: an overshooting curve stands in
            None => " using CubicBezierEasing(0.34f, 1.56f, 0.64f, 1f) // spring".into(),
        },
    }
}

/// Jetpack Compose animations (`keyframes { }` with `CubicBezierEasing`) for every animated layer
/// under `node`, plus the `graphicsLayer` that applies them. Size, corner radius and colour
/// animations are listed in a comment; springs are approximated by an overshooting curve.
pub fn compose_animation_for(doc: &Document, node: NodeId) -> String {
    let Some(page) = doc.page_of(node).or(Some(node)) else { return String::new() };
    let tl = doc.timeline(page);
    let mut out = Out::new("    ");
    for id in animated_layers(doc, node) {
        let Some(n) = doc.get(id) else { continue };
        let (chs, skipped) = channels(n, tl.duration_ms);
        if chs.is_empty() {
            continue;
        }
        let base = crate::codegen::camel(&n.name);
        if !out.is_empty() {
            out.line(0, "");
        }
        out.line(0, format!("// {}", n.name));
        if tl.looping {
            out.line(0, format!("val {base}Transition = rememberInfiniteTransition(label = \"{base}\")"));
        }
        for c in &chs {
            let var = format!("{base}{}", crate::codegen::pascal(c.key));
            let first = c.frames[0].1;
            let last = c.frames[c.frames.len() - 1].1;
            let spec = |out: &mut Out, depth: usize| {
                out.line(depth, "keyframes {");
                out.line(depth + 1, format!("durationMillis = {}", tl.duration_ms));
                for (t, v, e) in &c.frames {
                    out.line(depth + 1, format!("{} at {t}{}", kotlin_f(*v), kotlin_easing(e)));
                }
                out.line(depth, "}");
            };
            if tl.looping {
                out.line(0, format!("val {var} by {base}Transition.animateFloat("));
                out.line(1, format!("initialValue = {},", kotlin_f(first)));
                out.line(1, format!("targetValue = {},", kotlin_f(last)));
                out.line(1, "animationSpec = infiniteRepeatable(");
                out.line(2, "animation =");
                spec(&mut out, 3);
                out.line(2, ",");
                out.line(2, "repeatMode = RepeatMode.Restart");
                out.line(1, "),");
                out.line(1, format!("label = \"{}\"", c.key));
                out.line(0, ")");
            } else {
                out.line(0, format!("val {var} = remember {{ Animatable({}) }}", kotlin_f(first)));
                out.line(0, format!("LaunchedEffect(Unit) {{ {var}.animateTo({}, animationSpec =", kotlin_f(last)));
                spec(&mut out, 1);
                out.line(0, ") }");
            }
        }
        out.line(0, "Modifier.graphicsLayer {");
        for c in &chs {
            let var = format!("{base}{}", crate::codegen::pascal(c.key));
            let v = if tl.looping { var } else { format!("{var}.value") };
            match c.key {
                "x" => out.line(1, format!("translationX = {v}.dp.toPx()")),
                "y" => out.line(1, format!("translationY = {v}.dp.toPx()")),
                "scaleX" => out.line(1, format!("scaleX = {v}")),
                "scaleY" => out.line(1, format!("scaleY = {v}")),
                "rotation" => out.line(1, format!("rotationZ = {v}")),
                _ => out.line(1, format!("alpha = {v}")),
            }
        }
        out.line(0, "}");
        if !skipped.is_empty() {
            out.line(0, format!("// not generated: {}", skipped.join(", ")));
        }
    }
    out.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use desigma_doc::motion::{find_preset, TimelineSettings};
    use desigma_doc::{Color, Rect, Vec2};

    fn doc(looping: bool) -> (Document, NodeId, NodeId) {
        let mut d = Document::new("t");
        let page = d.add_page("P");
        let frame = d.alloc_id();
        d.insert(page, Node::frame(frame, Rect::new(0.0, 0.0, 200.0, 100.0))).unwrap();
        let id = d.alloc_id();
        let mut n = Node::rectangle(id, Rect::new(10.0, 20.0, 40.0, 40.0));
        n.name = "Card".into();
        let pos = n.motion.ensure_track(AnimProperty::Position);
        pos.set_key(0, AnimValue::Vec2(Vec2::new(10.0, 20.0)), Some(Ease::bezier(0.4, 0.0, 0.2, 1.0)));
        pos.set_key(1000, AnimValue::Vec2(Vec2::new(110.0, 20.0)), Some(Ease::Hold));
        pos.set_key(1500, AnimValue::Vec2(Vec2::new(110.0, 70.0)), None);
        let op = n.motion.ensure_track(AnimProperty::Opacity);
        op.set_key(500, AnimValue::Scalar(0.0), Some(Ease::spring(0.4)));
        op.set_key(1000, AnimValue::Scalar(1.0), None);
        let fill = n.motion.ensure_track(AnimProperty::FillColor);
        fill.set_key(0, AnimValue::Color(Color::rgb(255, 0, 0)), Some(Ease::Linear));
        fill.set_key(2000, AnimValue::Color(Color::rgb(0, 0, 255)), None);
        d.insert(frame, n).unwrap();
        d.timelines.insert(page, TimelineSettings { duration_ms: 2000, fps: 60, looping });
        (d, frame, id)
    }

    #[test]
    fn keyframes_percentages_easings_and_the_animation_declaration() {
        let (d, frame, _) = doc(true);
        let css = css_animation_for(&d, frame);
        assert!(css.contains("@keyframes card-position {"), "{css}");
        assert!(css.contains("0% { translate: 0px 0px; animation-timing-function: cubic-bezier(0.4, 0, 0.2, 1); }"), "{css}");
        assert!(css.contains("50% { translate: 100px 0px; animation-timing-function: step-end; }"), "{css}");
        assert!(css.contains("75% { translate: 100px 50px; }"), "{css}");
        assert!(css.contains("100% { translate: 100px 50px; }"), "held to the end of the timeline: {css}");
        assert!(css.contains("0% { opacity: 0; }"), "the hold before the first key: {css}");
        assert!(css.contains("25% { opacity: 0; animation-timing-function: linear("), "{css}");
        assert!(css.contains("100% { background-color: #00F; }"), "{css}");
        assert!(
            css.contains(
                "animation: card-position 2000ms linear both infinite, card-opacity 2000ms linear both infinite, card-fill 2000ms linear both infinite;"
            ),
            "{css}"
        );
        assert!(css.contains(".card {"));
        // a timeline that does not loop plays once
        let (d, frame, _) = doc(false);
        assert!(css_animation_for(&d, frame).contains("2000ms linear both 1"));
    }

    #[test]
    fn spring_is_sampled_into_linear_and_presets_are_baked() {
        let t = timing_function(&Ease::spring(0.4));
        assert!(t.starts_with("linear(0, ") && t.ends_with(", 1)"), "{t}");
        assert_eq!(t.matches(',').count(), 30, "31 samples: {t}");
        assert_eq!(timing_function(&Ease::EaseOut), "cubic-bezier(0, 0, 0.58, 1)");
        let mut d = Document::new("t");
        let page = d.add_page("P");
        let id = d.alloc_id();
        let mut n = Node::rectangle(id, Rect::new(0.0, 0.0, 10.0, 10.0));
        n.name = "Dot".into();
        n.motion.clips = find_preset("Pulse").unwrap().instantiate(0, 0);
        d.insert(page, n).unwrap();
        d.timelines.insert(page, TimelineSettings { duration_ms: 1000, fps: 60, looping: true });
        let css = css_animation_for(&d, id);
        assert!(css.contains("@keyframes dot-scale {"), "{css}");
        assert!(css.matches("% { scale:").count() > 10, "baked samples: {css}");
        assert!(css.contains("100% { scale:"));
    }

    #[test]
    fn swiftui_keyframe_animator() {
        let (d, frame, _) = doc(true);
        let sw = swiftui_animation_for(&d, frame);
        assert!(sw.contains("struct CardAnimation {"), "{sw}");
        assert!(sw.contains(".keyframeAnimator(initialValue: CardAnimation(), repeating: true)"), "{sw}");
        assert!(sw.contains(".offset(x: v.x, y: v.y)") && sw.contains(".opacity(v.opacity)"), "{sw}");
        assert!(sw.contains("KeyframeTrack(\\.x) {"), "{sw}");
        assert!(sw.contains("CubicKeyframe(100, duration: 1) // cubic-bezier(0.4, 0, 0.2, 1)"), "{sw}");
        assert!(sw.contains("LinearKeyframe(100, duration: 0.5)") && sw.contains("MoveKeyframe(100)"), "hold: {sw}");
        assert!(sw.contains("SpringKeyframe(1, duration: 0.5, spring: Spring(duration: 0.5, bounce: 0.4))"), "{sw}");
        assert!(sw.contains("// not generated: Fill"), "{sw}");
    }

    #[test]
    fn compose_keyframes() {
        let (d, frame, _) = doc(true);
        let kt = compose_animation_for(&d, frame);
        assert!(kt.contains("val cardTransition = rememberInfiniteTransition(label = \"card\")"), "{kt}");
        assert!(kt.contains("val cardX by cardTransition.animateFloat("), "{kt}");
        assert!(kt.contains("0f at 0 using CubicBezierEasing(0.4f, 0f, 0.2f, 1f)"), "{kt}");
        assert!(kt.contains("100f at 1000 using Easing { f -> if (f < 1f) 0f else 1f }"), "{kt}");
        assert!(kt.contains("translationX = cardX.dp.toPx()") && kt.contains("alpha = cardOpacity"), "{kt}");
        let (d, frame, _) = doc(false);
        let kt = compose_animation_for(&d, frame);
        assert!(kt.contains("Animatable(0f)") && kt.contains("animateTo("), "{kt}");
    }

    #[test]
    fn no_animation_no_output() {
        let mut d = Document::new("t");
        let page = d.add_page("P");
        let id = d.alloc_id();
        d.insert(page, Node::rectangle(id, Rect::new(0.0, 0.0, 10.0, 10.0))).unwrap();
        assert_eq!(css_animation_for(&d, id), "");
        assert_eq!(swiftui_animation_for(&d, id), "");
        assert_eq!(compose_animation_for(&d, id), "");
    }

    #[test]
    fn shadow_blur_and_hidden_tracks_in_css() {
        use desigma_doc::Effect;
        let (mut d, frame, id) = doc(true);
        let mut n = d.get(id).unwrap().clone();
        n.effects = vec![
            Effect::DropShadow { color: Color::rgba(0, 0, 0, 64), offset: Vec2::new(0.0, 4.0), radius: 4.0, spread: 2.0, visible: true, show_behind: false },
            Effect::LayerBlur { radius: 0.0, visible: true },
        ];
        let sx = n.motion.ensure_track(AnimProperty::ShadowX);
        sx.set_key(0, AnimValue::Scalar(0.0), Some(Ease::EaseOut));
        sx.set_key(1000, AnimValue::Scalar(10.0), None);
        let sb = n.motion.ensure_track(AnimProperty::ShadowBlur);
        sb.set_key(500, AnimValue::Scalar(20.0), None);
        let bl = n.motion.ensure_track(AnimProperty::BlurRadius);
        bl.set_key(0, AnimValue::Scalar(0.0), None);
        bl.set_key(1000, AnimValue::Scalar(8.0), None);
        *d.get_mut(id).unwrap() = n;
        let css = css_animation_for(&d, frame);
        assert!(css.contains("@keyframes card-shadow {"), "{css}");
        assert!(css.contains("0% { box-shadow: 0px 4px 20px 2px rgba(0, 0, 0, 0.25); animation-timing-function: cubic-bezier(0, 0, 0.58, 1); }"), "{css}");
        assert!(css.contains("25% { box-shadow: ") && css.contains("50% { box-shadow: 10px 4px 20px 2px"), "{css}");
        assert!(css.contains("@keyframes card-layer-blur {") && css.contains("50% { filter: blur(8px); }"), "{css}");
        assert!(css.contains("card-shadow 2000ms linear both infinite"), "{css}");
        // a hidden track leaves the export
        let mut n = d.get(id).unwrap().clone();
        n.motion.track_mut(AnimProperty::Opacity).unwrap().visible = false;
        n.motion.track_mut(AnimProperty::ShadowX).unwrap().visible = false;
        n.motion.track_mut(AnimProperty::ShadowBlur).unwrap().visible = false;
        *d.get_mut(id).unwrap() = n;
        let css = css_animation_for(&d, frame);
        assert!(!css.contains("card-opacity") && !css.contains("card-shadow"), "{css}");
        assert!(css.contains("card-position"), "{css}");
    }
}
