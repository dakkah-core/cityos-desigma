//! Figma Motion (Beta) <-> `desigma_doc::motion`.
//!
//! `.fig` schema (see FIELDS.md, "Motion"):
//!
//! * An animated node carries `parameterConsumptionMap.entries[]` (`variableField` +
//!   `variableData`). A keyframed field is an `EXPRESSION` whose function is `KEYFRAME`; its
//!   arguments are the static base value and a `keyframeTrackParameterValue` listing the tracks:
//!   `MANUAL` (a track the user keyed by hand) or `ANIMATION_PRESET` (a track owned by one of the
//!   node's preset instances).
//! * `KEYFRAME_TRACK` nodes (on the internal-only canvas) own `KEYFRAME` nodes (`timelinePosition`
//!   in microseconds, `keyframeValue`, `easingData`).
//! * `animationPresets.presets[]` on the node lists its `ANIMATION_PRESET_INSTANCE` nodes
//!   (`timelineOffset` in microseconds, `componentPropAssignments`); each becomes a desigma clip.
//!
//! Everything is read tolerantly: unknown fields, properties and values are skipped and the rest
//! of the file imports normally.
//!
//! The result is attached to the REST-shaped node as `motion` (serde form of [`NodeMotion`], plus
//! a `relative` list of properties whose values are offsets from the node's static value), and to
//! pages as `motionTimeline`. `rest_import` and `export` read/write the same keys.

use crate::fig::guid_str;
use crate::rest_import::{farr, fnum, fstr};
use desigma_doc::motion::{AnimProperty, AnimValue, Clip, ClipDirection, ClipKind, ClipMode, ClipRepeat, Ease, NodeMotion, TimelineSettings, Track};
use desigma_doc::{Node, Vec2};
use serde_json::{json, Value};
use std::collections::HashMap;

// ------------------------------------------------------------------ small helpers

fn i64_of(v: &Value, k: &str) -> Option<i64> {
    let x = v.get(k)?;
    x.as_i64().or_else(|| x.as_f64().map(|f| f as i64)).or_else(|| x.as_str().and_then(|s| s.parse().ok()))
}

/// Microseconds -> whole milliseconds.
fn us_to_ms(us: i64) -> u32 {
    ((us.max(0) + 500) / 1000).min(u32::MAX as i64) as u32
}

fn secs_to_ms(s: f32) -> u32 {
    if s.is_finite() && s > 0.0 {
        (s * 1000.0).round() as u32
    } else {
        0
    }
}

/// Spring `mass`/`stiffness`/`damping` -> desigma's duration-based `bounce` (inverse of
/// `bounce_to_damping_ratio`): `zeta = damping / (2 sqrt(stiffness mass))`.
fn bounce_of(stiffness: f32, damping: f32, mass: f32) -> f32 {
    let zeta = damping / (2.0 * (stiffness.max(1e-3) * mass.max(1e-3)).sqrt());
    if zeta <= 1.0 {
        1.0 - zeta
    } else {
        1.0 / zeta - 1.0
    }
}

/// `EasingData` -> [`Ease`]. See FIELDS.md for the confidence of each mapping.
pub(crate) fn ease_of(ed: &Value) -> Option<Ease> {
    let ty = fstr(ed, "easingType")?;
    let ev = ed.get("easingValue");
    let bez = ev.and_then(|e| e.get("bezierEasing")).and_then(|b| Some([fnum(b, "p1x")?, fnum(b, "p1y")?, fnum(b, "p2x")?, fnum(b, "p2y")?]));
    let spring = ev
        .and_then(|e| e.get("springEasing"))
        .and_then(|s| Some(Ease::spring(bounce_of(fnum(s, "stiffness")?, fnum(s, "damping")?, fnum(s, "mass").unwrap_or(1.0)))));
    Some(match ty {
        "LINEAR" => Ease::Linear,
        "HOLD" => Ease::Hold,
        "IN_CUBIC" | "EASE_IN" => Ease::EaseIn,
        "OUT_CUBIC" => Ease::EaseOut,
        "INOUT_CUBIC" => Ease::EaseInOut,
        "IN_BACK_CUBIC" => Ease::EaseInBack,
        "OUT_BACK_CUBIC" => Ease::EaseOutBack,
        "INOUT_BACK_CUBIC" => Ease::EaseInOutBack,
        "CUSTOM_CUBIC" => bez.map(|b| Ease::bezier(b[0], b[1], b[2], b[3])).unwrap_or(Ease::EaseInOut),
        "GENTLE_SPRING" => spring.unwrap_or(Ease::SpringGentle),
        "SPRING" | "CUSTOM_SPRING" => spring.unwrap_or(Ease::spring(desigma_doc::motion::DEFAULT_BOUNCE)),
        "SPRING_PRESET_ONE" => spring.unwrap_or(Ease::SpringQuick),
        "SPRING_PRESET_TWO" => spring.unwrap_or(Ease::SpringBouncy),
        "SPRING_PRESET_THREE" => spring.unwrap_or(Ease::SpringSlow),
        _ => return None,
    })
}

// ------------------------------------------------------------------ raw tracks

/// One keyframe as stored: time, 1 or 2 components, easing of the segment that *ends* here.
#[derive(Clone, Copy)]
struct RawKey {
    t: u32,
    v: [f32; 2],
    ease: Ease,
}

/// A scalar track from `(time, value, incoming easing)`; desigma keeps the easing on the key that
/// *starts* a segment, so each key takes the easing of the next one.
fn to_track(property: AnimProperty, keys: &[(u32, AnimValue, Ease)]) -> Track {
    let mut t = Track::new(property);
    for (i, (ms, v, _)) in keys.iter().enumerate() {
        let ease = keys.get(i + 1).map(|k| k.2).unwrap_or(Ease::Linear);
        t.set_key(*ms, *v, Some(ease));
    }
    t
}

struct Index<'a> {
    nodes: HashMap<String, &'a Value>,
    /// Parent id -> `KEYFRAME_TRACK` ids.
    tracks: HashMap<String, Vec<String>>,
    /// Track id -> its `KEYFRAME` nodes.
    keyframes: HashMap<String, Vec<&'a Value>>,
    /// Code component library key -> (prop def id -> prop name).
    prop_names: HashMap<String, HashMap<String, String>>,
}

impl<'a> Index<'a> {
    fn new(all: &[&'a Value]) -> Self {
        let mut ix = Index { nodes: HashMap::new(), tracks: HashMap::new(), keyframes: HashMap::new(), prop_names: HashMap::new() };
        for &j in all {
            let Some(id) = j.get("guid").and_then(guid_str) else { continue };
            let parent = j.get("parentIndex").and_then(|p| p.get("guid")).and_then(guid_str);
            match fstr(j, "type") {
                Some("KEYFRAME_TRACK") => {
                    if let Some(p) = parent {
                        ix.tracks.entry(p).or_default().push(id.clone());
                    }
                }
                Some("KEYFRAME") => {
                    if let Some(p) = parent {
                        ix.keyframes.entry(p).or_default().push(j);
                    }
                }
                Some("CODE_COMPONENT") => {
                    if let Some(key) = fstr(j, "key") {
                        let names =
                            farr(j, "componentPropDefs").iter().filter_map(|d| Some((d.get("id").and_then(guid_str)?, fstr(d, "name")?.to_string()))).collect();
                        ix.prop_names.insert(key.to_string(), names);
                    }
                }
                _ => {}
            }
            ix.nodes.insert(id, j);
        }
        ix
    }

    /// Keyframes of a track, sorted by time. Values are scalar (`[x, _]`) or vector (`[x, y]`);
    /// other value types (colours, paints...) are not supported and give `None`.
    fn keys(&self, track: &str) -> Option<(Vec<RawKey>, bool)> {
        let kfs = self.keyframes.get(track)?;
        let mut out = Vec::new();
        let mut is_vec = false;
        for k in kfs {
            let val = k.get("keyframeValue")?.get("value")?;
            let v = if let Some(x) = val.get("floatValue").and_then(Value::as_f64) {
                [x as f32, 0.0]
            } else if let Some(vv) = val.get("vectorValue") {
                is_vec = true;
                [fnum(vv, "x")?, fnum(vv, "y")?]
            } else {
                return None;
            };
            let t = i64_of(k, "timelinePosition").map(us_to_ms).unwrap_or(0);
            let ease = k.get("easingData").and_then(ease_of).unwrap_or(Ease::Linear);
            out.push(RawKey { t, v, ease });
        }
        out.sort_by_key(|k| k.t);
        out.dedup_by_key(|k| k.t);
        (!out.is_empty()).then_some((out, is_vec))
    }
}

/// The track references of a `variableData` holding a `KEYFRAME` expression:
/// `(manual track ids, [(preset id, track id)])`.
fn track_refs(data: &Value) -> (Vec<String>, Vec<(String, String)>) {
    fn find(d: &Value, depth: usize) -> Option<&Value> {
        let value = d.get("value")?;
        if let Some(p) = value.get("keyframeTrackParameterValue") {
            return Some(p);
        }
        if depth > 4 {
            return None;
        }
        farr(value.get("expressionValue")?, "expressionArguments").iter().find_map(|a| find(a, depth + 1))
    }
    let (mut manual, mut presets) = (Vec::new(), Vec::new());
    if let Some(p) = find(data, 0) {
        for param in farr(p, "parameters") {
            let Some(v) = param.get("value") else { continue };
            if let Some(t) = v.get("manual").and_then(|m| m.get("keyframeTrackId")).and_then(|t| t.get("guid")).and_then(guid_str) {
                manual.push(t);
            } else if let Some(a) = v.get("animationPreset") {
                let pid = a.get("animationPresetId").and_then(|p| p.get("guid")).and_then(guid_str);
                let tid = a.get("keyframeTrackId").and_then(|p| p.get("guid")).and_then(guid_str);
                if let (Some(p), Some(t)) = (pid, tid) {
                    presets.push((p, t));
                }
            }
        }
    }
    (manual, presets)
}

// ------------------------------------------------------------------ manual tracks

/// Fields that animate a desigma property: `(property, component)`. Component 0/1 are the x/y
/// of a vector property, 2 means the field is the whole value (scalar or vector).
fn field_target(field: &str) -> Option<(AnimProperty, usize)> {
    Some(match field {
        "MOTION_TRANSLATION_XY" => (AnimProperty::Position, 2),
        "MOTION_TRANSLATION_X" => (AnimProperty::Position, 0),
        "MOTION_TRANSLATION_Y" => (AnimProperty::Position, 1),
        "MOTION_SCALE_XY" => (AnimProperty::Scale, 2),
        "MOTION_SCALE_X" => (AnimProperty::Scale, 0),
        "MOTION_SCALE_Y" => (AnimProperty::Scale, 1),
        "MOTION_ROTATION" => (AnimProperty::Rotation, 2),
        "OPACITY" => (AnimProperty::Opacity, 2),
        "WIDTH" => (AnimProperty::Width, 2),
        "HEIGHT" => (AnimProperty::Height, 2),
        "CORNER_RADIUS" => (AnimProperty::CornerRadius, 2),
        "STROKE_WEIGHT" => (AnimProperty::StrokeWeight, 2),
        _ => return None,
    })
}

/// File value -> desigma value (still an offset for the relative properties).
fn convert_value(p: AnimProperty, raw: f32) -> f32 {
    match p {
        // Radians, clockwise positive in the file; desigma: degrees, counter-clockwise positive.
        AnimProperty::Rotation => -raw.to_degrees() + 0.0,
        // Percent.
        AnimProperty::Opacity => raw / 100.0,
        _ => raw,
    }
}

fn is_relative(p: AnimProperty) -> bool {
    matches!(p, AnimProperty::Position | AnimProperty::Rotation)
}

fn manual_tracks(ix: &Index, fields: &[(String, String)]) -> Vec<Track> {
    // property -> [x channel, y channel, whole]
    let mut chans: HashMap<AnimProperty, [Option<Vec<RawKey>>; 3]> = HashMap::new();
    for (field, track) in fields {
        let Some((prop, comp)) = field_target(field) else { continue };
        let Some((keys, _)) = ix.keys(track) else { continue };
        chans.entry(prop).or_default()[comp] = Some(keys);
    }
    let mut out: Vec<Track> = Vec::new();
    for (prop, [x, y, whole]) in chans {
        let conv = |k: &RawKey, c: usize| convert_value(prop, k.v[c]);
        let scalar_keys =
            |keys: &[RawKey], c: usize| -> Vec<(u32, AnimValue, Ease)> { keys.iter().map(|k| (k.t, AnimValue::Scalar(conv(k, c)), k.ease)).collect() };
        if prop.kind() == desigma_doc::motion::ValueKind::Scalar {
            if let Some(w) = whole {
                out.push(to_track(prop, &scalar_keys(&w, 0)));
            }
            continue;
        }
        // Vector property.
        if let Some(w) = whole {
            let keys: Vec<(u32, AnimValue, Ease)> = w.iter().map(|k| (k.t, AnimValue::Vec2(Vec2::new(k.v[0], k.v[1])), k.ease)).collect();
            out.push(to_track(prop, &keys));
            continue;
        }
        let neutral = if prop == AnimProperty::Scale { 1.0 } else { 0.0 };
        let tx = x.as_ref().map(|k| to_track(AnimProperty::Width, &scalar_keys(k, 0)));
        let ty = y.as_ref().map(|k| to_track(AnimProperty::Width, &scalar_keys(k, 0)));
        let mut times: Vec<u32> = x.iter().chain(y.iter()).flatten().map(|k| k.t).collect();
        times.sort_unstable();
        times.dedup();
        let at = |t: &Option<Track>, ms: u32| t.as_ref().and_then(|t| t.value_at(ms as f32)).and_then(|v| v.scalar()).unwrap_or(neutral);
        let ease_at = |ms: u32| x.iter().chain(y.iter()).flatten().find(|k| k.t == ms).map(|k| k.ease).unwrap_or(Ease::Linear);
        let keys: Vec<(u32, AnimValue, Ease)> = times.iter().map(|&ms| (ms, AnimValue::Vec2(Vec2::new(at(&tx, ms), at(&ty, ms))), ease_at(ms))).collect();
        out.push(to_track(prop, &keys));
    }
    out.sort_by_key(|t| t.property);
    out
}

// ------------------------------------------------------------------ clips (animation presets)

struct PresetParams<'a> {
    by_name: HashMap<String, &'a Value>,
}

impl<'a> PresetParams<'a> {
    fn new(ix: &Index, inst: &'a Value) -> Self {
        let key = inst.get("backingCodeComponentId").and_then(|b| b.get("assetRef")).and_then(|a| fstr(a, "key"));
        let names = key.and_then(|k| ix.prop_names.get(k));
        let mut by_name = HashMap::new();
        if let Some(names) = names {
            for a in farr(inst, "componentPropAssignments") {
                if let Some(name) = a.get("defID").and_then(guid_str).and_then(|id| names.get(&id)) {
                    by_name.insert(name.clone(), a);
                }
            }
        }
        PresetParams { by_name }
    }

    fn text(&self, name: &str) -> Option<String> {
        let a = self.by_name.get(name)?;
        let t = a.get("value").and_then(|v| v.get("textValue")).or_else(|| a.get("varValue")?.get("value")?.get("textDataValue"))?;
        Some(fstr(t, "characters")?.to_string())
    }

    fn float(&self, name: &str) -> Option<f32> {
        let a = self.by_name.get(name)?;
        fnum(a.get("value")?, "floatValue").or_else(|| fnum(a.get("varValue")?.get("value")?, "floatValue"))
    }

    fn easing(&self, name: &str) -> Option<Ease> {
        let a = self.by_name.get(name)?;
        let ed = a.get("value").and_then(|v| v.get("easingData")).or_else(|| a.get("varValue")?.get("value")?.get("easingValue"))?;
        ease_of(ed)
    }
}

fn parse_direction(s: &str) -> Option<ClipDirection> {
    let k: String = s.chars().filter(|c| c.is_ascii_alphabetic()).collect::<String>().to_ascii_lowercase();
    Some(match k.as_str() {
        "top" | "up" => ClipDirection::Top,
        "bottom" | "down" => ClipDirection::Bottom,
        "left" => ClipDirection::Left,
        "right" => ClipDirection::Right,
        "topleft" => ClipDirection::TopLeft,
        "topright" => ClipDirection::TopRight,
        "bottomleft" => ClipDirection::BottomLeft,
        "bottomright" => ClipDirection::BottomRight,
        "clockwise" => ClipDirection::Clockwise,
        "counterclockwise" | "anticlockwise" => ClipDirection::CounterClockwise,
        "both" => ClipDirection::Both,
        "horizontal" => ClipDirection::Horizontal,
        "vertical" => ClipDirection::Vertical,
        _ => return None,
    })
}

/// Start and end value of a preset's tracks in the clip kind's value space; components missing
/// from the tracks stay at the kind's neutral value.
fn clip_values(ix: &Index, kind: ClipKind, fields: &[(String, String)]) -> Option<(Vec2, Vec2)> {
    let neutral = kind.neutral();
    let (mut a, mut b) = (neutral, neutral);
    let mut any = false;
    for (field, track) in fields {
        let Some((keys, _)) = ix.keys(track) else { continue };
        let (first, last) = (keys[0], keys[keys.len() - 1]);
        let (prop, comp) = match (kind, field.as_str()) {
            (ClipKind::Size, "WIDTH") => (AnimProperty::Width, 0),
            (ClipKind::Size, "HEIGHT") => (AnimProperty::Height, 1),
            (ClipKind::Size, _) => continue,
            (_, f) => match field_target(f) {
                Some((p, c)) if kind.properties().contains(&p) => (p, if c == 2 { 0 } else { c }),
                _ => continue,
            },
        };
        let conv = |k: &RawKey, c: usize| if prop == AnimProperty::Rotation { convert_value(prop, k.v[c]) } else { k.v[c] };
        let vector = field == "MOTION_TRANSLATION_XY" || field == "MOTION_SCALE_XY";
        let (fa, fb) = if vector { (Vec2::new(first.v[0], first.v[1]), Vec2::new(last.v[0], last.v[1])) } else { (Vec2::ZERO, Vec2::ZERO) };
        if vector {
            a = fa;
            b = fb;
        } else if comp == 0 {
            a.x = conv(&first, 0);
            b.x = conv(&last, 0);
        } else {
            a.y = conv(&first, 0);
            b.y = conv(&last, 0);
        }
        any = true;
    }
    any.then_some((a, b))
}

fn kind_of_fields(fields: &[(String, String)]) -> Option<ClipKind> {
    let has = |p: &str| fields.iter().any(|(f, _)| f.starts_with(p));
    if has("MOTION_TRANSLATION") {
        Some(ClipKind::Position)
    } else if has("MOTION_SCALE") {
        Some(ClipKind::Scale)
    } else if has("MOTION_ROTATION") {
        Some(ClipKind::Rotation)
    } else if fields.iter().any(|(f, _)| f == "WIDTH" || f == "HEIGHT") {
        Some(ClipKind::Size)
    } else if has("OPACITY") {
        Some(ClipKind::Opacity)
    } else {
        None
    }
}

fn near(a: Vec2, b: Vec2) -> bool {
    (a.x - b.x).abs() < 0.01 && (a.y - b.y).abs() < 0.01
}

fn build_clip(ix: &Index, inst: &Value, fields: &[(String, String)]) -> Option<Clip> {
    let kind = kind_of_fields(fields)?;
    let (start, end) = clip_values(ix, kind, fields)?;
    let params = PresetParams::new(ix, inst);
    let neutral = kind.neutral();
    let ty = params.text("type").map(|s| s.to_ascii_lowercase());
    let mode = match ty.as_deref() {
        Some(t) if t.ends_with("in") => ClipMode::In,
        Some(t) if t.ends_with("out") => ClipMode::Out,
        Some(_) => ClipMode::Custom,
        None if near(end, neutral) => ClipMode::In,
        None if near(start, neutral) => ClipMode::Out,
        None => ClipMode::Custom,
    };
    let away = if mode == ClipMode::Out { end } else { start };
    let mut clip = Clip::new(kind, mode);
    if mode != ClipMode::Custom {
        match kind {
            ClipKind::Position => {
                clip.amount = params.float("distance").unwrap_or_else(|| (away.x * away.x + away.y * away.y).sqrt());
                clip.direction = params.text("direction").and_then(|d| parse_direction(&d)).unwrap_or_else(|| {
                    ClipKind::Position
                        .directions()
                        .iter()
                        .copied()
                        .max_by(|a, b| {
                            let d = |c: &ClipDirection| c.vector().x * away.x + c.vector().y * away.y;
                            d(a).total_cmp(&d(b))
                        })
                        .unwrap_or(ClipDirection::Bottom)
                });
            }
            ClipKind::Scale | ClipKind::Size => {
                let a = (away.x - 1.0).abs() > 1e-4;
                let b = (away.y - 1.0).abs() > 1e-4;
                clip.direction = match (a, b) {
                    (true, false) => ClipDirection::Horizontal,
                    (false, true) => ClipDirection::Vertical,
                    _ => ClipDirection::Both,
                };
                clip.amount = if b && !a { away.y } else { away.x };
            }
            ClipKind::Rotation => {
                clip.amount = away.x.abs();
                let clockwise_in = mode == ClipMode::In && away.x >= 0.0;
                let clockwise_out = mode == ClipMode::Out && away.x < 0.0;
                clip.direction = params.text("direction").and_then(|d| parse_direction(&d)).unwrap_or(if clockwise_in || clockwise_out {
                    ClipDirection::Clockwise
                } else {
                    ClipDirection::CounterClockwise
                });
            }
            ClipKind::Opacity => clip.amount = away.x,
        }
        // The named parameters must reproduce the stored tracks, else keep the exact values.
        let (s, e) = clip.endpoints();
        if !near(s, start) || !near(e, end) {
            clip.mode = ClipMode::Custom;
        }
    }
    if clip.mode == ClipMode::Custom {
        clip.from = start;
        clip.to = end;
    }
    let delay_ms = i64_of(inst, "timelineOffset").map(us_to_ms).or_else(|| params.float("delay").map(secs_to_ms)).unwrap_or(0);
    let track_end = fields.iter().filter_map(|(_, t)| ix.keys(t)).filter_map(|(k, _)| k.last().map(|k| k.t)).max().unwrap_or(0);
    clip.delay_ms = delay_ms;
    clip.duration_ms = params.float("duration").map(secs_to_ms).filter(|d| *d > 0).unwrap_or(track_end).max(1);
    clip.easing =
        params.easing("easing").or_else(|| fields.iter().find_map(|(_, t)| ix.keys(t)).and_then(|(k, _)| k.last().map(|k| k.ease))).unwrap_or(Ease::EaseOut);
    // GUESS (no sample): a looping track makes a repeating clip, a boomerang playback style a
    // there-and-back one.
    if fields.iter().any(|(_, t)| ix.nodes.get(t).and_then(|n| fstr(n, "trackLoopMode")) == Some("LOOP")) {
        clip.repeat = ClipRepeat::Infinite;
    }
    if fstr(inst, "playbackStyle") == Some("BOOMERANG") {
        clip.yoyo = true;
    }
    clip.visible = inst.get("visible").and_then(Value::as_bool).unwrap_or(true);
    clip.sanitize();
    Some(clip)
}

// ------------------------------------------------------------------ extraction

/// Motion of every animated node: node id (`session:local`) -> `motion` JSON for the REST-shaped
/// node. `nodes` are all kiwi node changes (tracks and presets live on the internal canvas).
pub(crate) fn extract(nodes: &[&Value]) -> HashMap<String, Value> {
    let ix = Index::new(nodes);
    let mut out = HashMap::new();
    for &nc in nodes {
        let Some(id) = nc.get("guid").and_then(guid_str) else { continue };
        let entries = nc.get("parameterConsumptionMap").map(|m| farr(m, "entries")).unwrap_or(&[]);
        let preset_list = nc.get("animationPresets").map(|m| farr(m, "presets")).unwrap_or(&[]);
        if entries.is_empty() && preset_list.is_empty() {
            continue;
        }
        let mut manual: Vec<(String, String)> = Vec::new();
        // preset id -> (field, track)
        let mut by_preset: HashMap<String, Vec<(String, String)>> = HashMap::new();
        for e in entries {
            let (Some(field), Some(data)) = (fstr(e, "variableField"), e.get("variableData")) else { continue };
            let (m, p) = track_refs(data);
            manual.extend(m.into_iter().map(|t| (field.to_string(), t)));
            for (pid, tid) in p {
                by_preset.entry(pid).or_default().push((field.to_string(), tid));
            }
        }
        let mut motion = NodeMotion { tracks: manual_tracks(&ix, &manual), ..Default::default() };

        let mut presets: Vec<(&str, String)> = preset_list
            .iter()
            .filter_map(|p| Some((fstr(p, "sortPosition").unwrap_or(""), p.get("animationPresetId")?.get("guid").and_then(guid_str)?)))
            .collect();
        presets.sort();
        for (_, pid) in presets {
            let (Some(inst), Some(fields)) = (ix.nodes.get(&pid), by_preset.get(&pid)) else { continue };
            if let Some(mut clip) = build_clip(&ix, inst, fields) {
                clip.id = motion.clips.len() as u32;
                motion.clips.push(clip);
            }
        }
        if motion.is_empty() {
            continue;
        }
        let relative: Vec<&str> = motion.tracks.iter().filter(|t| is_relative(t.property)).map(|t| t.property.name()).collect();
        let mut v = serde_json::to_value(&motion).unwrap_or(Value::Null);
        if !relative.is_empty() {
            v["relative"] = json!(relative);
        }
        out.insert(id, v);
    }
    out
}

/// Timeline settings of a page (`CANVAS`), as `motionTimeline` JSON, when the file sets any.
///
/// SCHEMA-ONLY (the reference file leaves the defaults): `timelineDuration` (microseconds) on the
/// page, or the default entry of `timelineDefinitions` (`durationUs`, `playbackStyle`,
/// `keyframeSnappingFps`).
pub(crate) fn page_timeline(nc: &Value) -> Option<Value> {
    let mut s = TimelineSettings::default();
    let def = nc
        .get("timelineDefinitions")
        .map(|m| farr(m, "entries"))
        .unwrap_or(&[])
        .iter()
        .filter_map(|e| e.get("data"))
        .find(|d| d.get("defaultTimeline").and_then(Value::as_bool) == Some(true));
    if let Some(d) = def {
        if let Some(us) = i64_of(d, "durationUs") {
            s.duration_ms = us_to_ms(us);
        }
        s.looping = fstr(d, "playbackStyle") == Some("LOOP");
        if let Some(f) = d.get("keyframeSnappingFps") {
            let (n, dn) = (fnum(f, "numerator").unwrap_or(0.0), fnum(f, "denominator").unwrap_or(1.0));
            if n > 0.0 && dn > 0.0 {
                s.fps = (n / dn).round() as u32;
            }
        }
    }
    if let Some(us) = i64_of(nc, "timelineDuration") {
        s.duration_ms = us_to_ms(us);
    }
    if fstr(nc, "playbackStyle") == Some("LOOP") {
        s.looping = true;
    }
    let s = s.sanitized();
    (!s.is_default()).then(|| serde_json::to_value(s).unwrap_or(Value::Null))
}

/// Node types of the motion machinery; they are not canvas content.
pub(crate) fn is_motion_node(nc: &Value) -> bool {
    match fstr(nc, "type") {
        Some("KEYFRAME" | "KEYFRAME_TRACK" | "ANIMATION_PRESET_INSTANCE") => true,
        Some("CODE_COMPONENT") => fstr(nc, "codeObjectType") == Some("ANIMATION_PRESET"),
        _ => false,
    }
}

// ------------------------------------------------------------------ REST side

/// Reads a node's `motion` (see module docs). Values of the `relative` properties are offsets
/// from the node's static value and become absolute here.
pub(crate) fn parse_node_motion(v: &Value, node: &Node) -> NodeMotion {
    let mut m: NodeMotion = serde_json::from_value(v.clone()).unwrap_or_default();
    let relative: Vec<&str> = farr(v, "relative").iter().filter_map(Value::as_str).collect();
    for t in &mut m.tracks {
        if !relative.contains(&t.property.name()) {
            continue;
        }
        let base = desigma_doc::motion::base_value(node, t.property);
        for k in &mut t.keys {
            k.value = match (k.value, base) {
                (AnimValue::Vec2(d), Some(AnimValue::Vec2(b))) => AnimValue::Vec2(Vec2::new(b.x + d.x, b.y + d.y)),
                (AnimValue::Scalar(d), Some(AnimValue::Scalar(b))) => AnimValue::Scalar(b + d),
                (other, _) => other,
            };
        }
    }
    m.normalize();
    m
}

/// Adds `motion` (and `motionTimeline` for pages) to an exported REST node.
pub(crate) fn export_into(doc: &desigma_doc::Document, node: &Node, m: &mut serde_json::Map<String, Value>) {
    if !node.motion.is_empty() {
        if let Ok(v) = serde_json::to_value(&node.motion) {
            m.insert("motion".into(), v);
        }
    }
    if let Some(t) = doc.timelines.get(&node.id).filter(|t| !t.is_default()) {
        if let Ok(v) = serde_json::to_value(t) {
            m.insert("motionTimeline".into(), v);
        }
    }
}
