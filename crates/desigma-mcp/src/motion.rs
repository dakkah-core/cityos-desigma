//! Motion tools: keyframes, animation presets, timeline settings and animation export.
//!
//! The editing tools run on any [`Host`](crate::engine::Host) (the MCP file session or the live
//! editor) through the undoable commands of `desigma_doc::motion::edit`. Export renders with
//! `desigma_export::anim` and returns bytes; the transports write them after their own path
//! checks, exactly as `desigma_export_image` does.

use crate::engine::Host;
use desigma_doc::motion::{self, edit, AnimProperty, AnimValue, Ease, TimelineSettings, PRESETS};
use desigma_doc::{Color, NodeId, NodeKind, Vec2};
use desigma_export::anim::{export_animation, has_motion, AnimExport, AnimFormat, Animation};
use serde::Deserialize;
use serde_json::{json, Value};

/// Tools handled by [`call`].
pub const EDIT_TOOLS: [&str; 3] = ["desigma_add_keyframe", "desigma_apply_animation_preset", "desigma_set_timeline"];

/// Most pixels (width x height x frames) one export may render.
pub const MAX_EXPORT_PIXELS: u64 = 1_500_000_000;
/// Most frames one export may have.
pub const MAX_EXPORT_FRAMES: usize = 1800;

const PROPERTIES: [(&str, AnimProperty); 15] = [
    ("position", AnimProperty::Position),
    ("scale", AnimProperty::Scale),
    ("rotation", AnimProperty::Rotation),
    ("width", AnimProperty::Width),
    ("height", AnimProperty::Height),
    ("opacity", AnimProperty::Opacity),
    ("corner_radius", AnimProperty::CornerRadius),
    ("fill", AnimProperty::FillColor),
    ("stroke", AnimProperty::StrokeColor),
    ("stroke_weight", AnimProperty::StrokeWeight),
    ("shadow_x", AnimProperty::ShadowX),
    ("shadow_y", AnimProperty::ShadowY),
    ("shadow_blur", AnimProperty::ShadowBlur),
    ("shadow_color", AnimProperty::ShadowColor),
    ("blur_radius", AnimProperty::BlurRadius),
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyframeArgs {
    node_id: u64,
    property: String,
    time_ms: u32,
    /// Number, `[x, y]` or `"#RRGGBB"`; omitted keeps the property's current look at that time.
    value: Option<Value>,
    /// A preset name ("linear", "ease in and out", "hold", "bouncy", ...).
    easing: Option<String>,
    /// `[x1, y1, x2, y2]` custom cubic bezier (overrides `easing`).
    bezier: Option<[f32; 4]>,
    /// Custom spring bounce, about -0.8..0.9 (overrides `easing`).
    spring_bounce: Option<f32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PresetArgs {
    node_ids: Vec<u64>,
    preset: String,
    #[serde(default)]
    start_ms: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TimelineArgs {
    page_id: Option<u64>,
    duration_ms: Option<u32>,
    fps: Option<u32>,
    #[serde(rename = "loop")]
    looping: Option<bool>,
}

/// Arguments of `desigma_export_animation`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportAnimArgs {
    pub node_id: u64,
    pub format: String,
    pub fps: Option<u32>,
    #[serde(default = "one")]
    pub scale: f32,
    pub path: String,
}

fn one() -> f32 {
    1.0
}

fn parse<T: serde::de::DeserializeOwned>(args: Value) -> Result<T, String> {
    serde_json::from_value(args).map_err(|e| format!("Invalid arguments: {e}"))
}

fn layer<H: Host>(host: &H, id: u64) -> Result<NodeId, String> {
    if id == 0 {
        return Err("node IDs must be positive integers".into());
    }
    match host.doc().get(NodeId(id)) {
        None => Err(format!("node {id} not found")),
        Some(n) if matches!(n.kind, NodeKind::Page { .. }) => Err("pages cannot be animated; animate a layer".into()),
        Some(n) => Ok(n.id),
    }
}

fn easing_by_name(name: &str) -> Result<Ease, String> {
    let norm = |s: &str| s.to_ascii_lowercase().replace(['_', '-'], " ");
    let want = norm(name);
    Ease::PRESETS
        .iter()
        .find(|(n, _)| norm(n) == want)
        .map(|(_, e)| *e)
        .ok_or_else(|| format!("unknown easing '{name}' (one of: {})", Ease::PRESETS.iter().map(|(n, _)| n.to_lowercase()).collect::<Vec<_>>().join(", ")))
}

fn value_for(property: AnimProperty, v: &Value) -> Result<AnimValue, String> {
    let num = |v: &Value| v.as_f64().filter(|x| x.is_finite() && x.abs() < 1e7).map(|x| x as f32).ok_or_else(|| "value numbers must be finite".to_string());
    match property.kind() {
        motion::ValueKind::Scalar => Ok(AnimValue::Scalar(num(v)?)),
        motion::ValueKind::Vec2 => {
            let a = v.as_array().filter(|a| a.len() == 2).ok_or("this property needs a value like [x, y]")?;
            Ok(AnimValue::Vec2(Vec2::new(num(&a[0])?, num(&a[1])?)))
        }
        motion::ValueKind::Color => {
            let s = v.as_str().ok_or("colour properties need a \"#RRGGBB\" value")?;
            Ok(AnimValue::Color(Color::from_hex(s).ok_or("colour must be #RGB, #RRGGBB or #RRGGBBAA")?))
        }
    }
}

fn property(name: &str) -> Result<AnimProperty, String> {
    PROPERTIES
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, p)| *p)
        .ok_or_else(|| format!("unknown property '{name}' (one of: {})", PROPERTIES.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ")))
}

fn timeline_json(s: &TimelineSettings) -> Value {
    json!({"duration_ms":s.duration_ms,"fps":s.fps,"loop":s.looping})
}

/// Runs one of [`EDIT_TOOLS`].
pub fn call<H: Host>(host: &mut H, name: &str, args: Value) -> Result<Value, String> {
    match name {
        "desigma_add_keyframe" => {
            let a: KeyframeArgs = parse(args)?;
            if a.time_ms > TimelineSettings::MAX_DURATION_MS {
                return Err(format!("time_ms must be at most {}", TimelineSettings::MAX_DURATION_MS));
            }
            let node = layer(host, a.node_id)?;
            let prop = property(&a.property)?;
            let value = a.value.as_ref().map(|v| value_for(prop, v)).transpose()?;
            let easing = if let Some(c) = a.bezier {
                if c.iter().any(|v| !v.is_finite()) {
                    return Err("bezier values must be finite".into());
                }
                Some(Ease::bezier(c[0], c[1], c[2], c[3]))
            } else if let Some(b) = a.spring_bounce {
                if !b.is_finite() {
                    return Err("spring_bounce must be finite".into());
                }
                Some(Ease::spring(b))
            } else {
                a.easing.as_deref().map(easing_by_name).transpose()?
            };
            let cmd = edit::add_keyframe(host.doc(), node, prop, a.time_ms, value, easing).map_err(|e| e.to_string())?;
            host.exec(cmd)?;
            let track = host.doc().get(node).and_then(|n| n.motion.track(prop)).map(|t| t.keys.len()).unwrap_or(0);
            Ok(json!({"node_id":node,"property":a.property,"time_ms":a.time_ms,"keyframes_in_track":track}))
        }
        "desigma_apply_animation_preset" => {
            let a: PresetArgs = parse(args)?;
            if a.node_ids.is_empty() || a.node_ids.len() > 100 {
                return Err("node_ids must list 1..100 layers".into());
            }
            if a.start_ms > TimelineSettings::MAX_DURATION_MS {
                return Err(format!("start_ms must be at most {}", TimelineSettings::MAX_DURATION_MS));
            }
            let preset = motion::find_preset(&a.preset).ok_or_else(|| format!("unknown preset '{}' (see the tool description for the list)", a.preset))?;
            let mut ids: Vec<NodeId> = Vec::new();
            for id in a.node_ids {
                let n = layer(host, id)?;
                if !ids.contains(&n) {
                    ids.push(n);
                }
            }
            let cmd = edit::apply_preset(host.doc(), &ids, preset, a.start_ms).map_err(|e| e.to_string())?;
            host.exec(cmd)?;
            Ok(json!({"preset":preset.name,"node_ids":ids,"start_ms":a.start_ms,"clips_added_per_node":preset.clips().len()}))
        }
        "desigma_set_timeline" => {
            let a: TimelineArgs = parse(args)?;
            if a.duration_ms.is_none() && a.fps.is_none() && a.looping.is_none() {
                return Err("give at least one of duration_ms, fps, loop".into());
            }
            let page = match a.page_id {
                Some(id) => {
                    let p = NodeId(id);
                    if !host.doc().get(p).is_some_and(|n| matches!(n.kind, NodeKind::Page { .. })) {
                        return Err(format!("{id} is not a page"));
                    }
                    p
                }
                None => *host.doc().pages.first().ok_or("the document has no pages")?,
            };
            let cur = host.doc().timeline(page);
            let want = TimelineSettings {
                duration_ms: a.duration_ms.unwrap_or(cur.duration_ms),
                fps: a.fps.unwrap_or(cur.fps),
                looping: a.looping.unwrap_or(cur.looping),
            };
            if want.duration_ms == 0 || want.duration_ms > TimelineSettings::MAX_DURATION_MS {
                return Err(format!("duration_ms must be 1..{}", TimelineSettings::MAX_DURATION_MS));
            }
            if want.fps == 0 || want.fps > TimelineSettings::MAX_FPS {
                return Err(format!("fps must be 1..{}", TimelineSettings::MAX_FPS));
            }
            if want != cur {
                let cmd = edit::set_timeline(host.doc(), page, want).map_err(|e| e.to_string())?;
                host.exec(cmd)?;
            }
            Ok(json!({"page_id":page,"timeline":timeline_json(&host.doc().timeline(page)),"changed":want != cur}))
        }
        _ => Err(format!("unknown tool: {name}")),
    }
}

/// File extension each export format is written with.
pub fn extension(format: &str) -> Result<&'static str, String> {
    Ok(AnimFormat::parse(format).ok_or("format must be gif, webm or png (a .zip of frames)")?.extension())
}

/// Renders the animation of a layer to GIF / WebM bytes or a zip of PNG frames. Returns the bytes
/// and a summary (`width`, `height`, `frames`, `duration_ms`, `fps`).
pub fn export<H: Host>(host: &H, a: &ExportAnimArgs) -> Result<(Vec<u8>, Value), String> {
    if a.node_id == 0 || host.doc().get(NodeId(a.node_id)).is_none() {
        return Err(format!("node {} not found", a.node_id));
    }
    let id = NodeId(a.node_id);
    let format = AnimFormat::parse(&a.format).ok_or("format must be gif, webm or png (a .zip of frames)")?;
    if !(0.25..=4.0).contains(&a.scale) {
        return Err("scale must be between 0.25 and 4".into());
    }
    if let Some(f) = a.fps {
        if f == 0 || f > format.max_fps() {
            return Err(format!("fps must be 1..{} for {}", format.max_fps(), format.label()));
        }
    }
    // variable bindings may resize layers: measure the resolved document
    let doc = desigma_doc::apply_bound_variables(host.doc());
    if !has_motion(&doc, id) {
        return Err("nothing is animated on that layer's page: add keyframes or an animation preset first".into());
    }
    let opts = AnimExport { format, fps: a.fps.unwrap_or(0), scale: a.scale, ..Default::default() };
    let probe = Animation::new(&doc, id, a.scale).map_err(|e| e.to_string())?;
    let (w, h) = probe.size();
    let fps = opts.effective_fps(&probe.timeline());
    let frames = probe.frame_times(fps).len();
    if frames > MAX_EXPORT_FRAMES {
        return Err(format!("{frames} frames exceed the {MAX_EXPORT_FRAMES}-frame limit; lower fps or shorten the timeline"));
    }
    if u64::from(w) * u64::from(h) * frames as u64 > MAX_EXPORT_PIXELS {
        return Err("export is too large (pixels x frames); lower the scale, fps or duration".into());
    }
    let name = doc.get(id).map(|n| n.name.clone()).unwrap_or_default();
    let bytes = export_animation(&doc, id, &opts, &name, None).map_err(|e| e.to_string())?;
    let tl = probe.timeline();
    Ok((bytes, json!({"width":w,"height":h,"frames":frames,"fps":fps,"duration_ms":tl.duration_ms,"loop":tl.looping,"format":format.label()})))
}

/// The preset names, for the tool schema.
pub fn preset_names() -> Vec<&'static str> {
    PRESETS.iter().map(|p| p.name).collect()
}

/// Tool descriptors (same shape as `engine::tools`).
pub fn tools() -> Vec<Value> {
    let id = json!({"type":"integer","minimum":1,"description":"Desigma numeric node ID; obtain from list_nodes/get_document."});
    let tool = |name: &str, description: String, properties: Value, required: Vec<&str>, read_only: bool, idempotent: bool| {
        json!({"name":name,"description":description,
            "inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false},
            "annotations":{"readOnlyHint":read_only,"destructiveHint":!read_only,"idempotentHint":idempotent,"openWorldHint":false}})
    };
    vec![
        tool(
            "desigma_add_keyframe",
            "Add (or update) a keyframe of an animatable property of one layer at time_ms on its page's Motion timeline. Omit value to keep the property's current look at that time. easing/bezier/spring_bounce set the easing of the segment that STARTS at this keyframe. Supports undo.".into(),
            json!({
                "node_id":id,
                "property":{"type":"string","enum":PROPERTIES.iter().map(|(n,_)|*n).collect::<Vec<_>>()},
                "time_ms":{"type":"integer","minimum":0,"maximum":TimelineSettings::MAX_DURATION_MS},
                "value":{"description":"Number (rotation in degrees counter-clockwise, opacity 0..1, size and stroke weight in px), [x, y] for position (px) and scale (1 = 100%), or \"#RRGGBB\" for fill and stroke.","anyOf":[{"type":"number"},{"type":"array","items":{"type":"number"},"minItems":2,"maxItems":2},{"type":"string"}]},
                "easing":{"type":"string","description":"One of: hold, linear, ease in, ease out, ease in and out, ease in back, ease out back, ease in and out back, gentle, quick, bouncy, slow."},
                "bezier":{"type":"array","items":{"type":"number"},"minItems":4,"maxItems":4,"description":"CSS-style [x1, y1, x2, y2]."},
                "spring_bounce":{"type":"number","minimum":-0.8,"maximum":0.9}
            }),
            vec!["node_id", "property", "time_ms"],
            false,
            true,
        ),
        tool(
            "desigma_apply_animation_preset",
            format!("Apply a library animation preset (enter, exit, cycle or basic motion) to one or more layers, starting at start_ms. Supports undo. Presets: {}.", preset_names().join(", ")),
            json!({"node_ids":{"type":"array","items":id,"minItems":1,"maxItems":100},"preset":{"type":"string","enum":preset_names()},"start_ms":{"type":"integer","minimum":0,"default":0}}),
            vec!["node_ids", "preset"],
            false,
            false,
        ),
        tool(
            "desigma_set_timeline",
            "Set the Motion timeline of a page: duration_ms, fps and whether playback loops (also the loop flag of exported GIFs). Defaults to the first page; unspecified fields keep their value. Supports undo.".into(),
            json!({"page_id":id,"duration_ms":{"type":"integer","minimum":1,"maximum":TimelineSettings::MAX_DURATION_MS},"fps":{"type":"integer","minimum":1,"maximum":TimelineSettings::MAX_FPS},"loop":{"type":"boolean"}}),
            vec![],
            false,
            true,
        ),
        tool(
            "desigma_export_animation",
            format!("Render the animation of one layer (usually a frame) to a file inside the workspace: gif (path ends .gif), webm (AV1, .webm) or png (a .zip of numbered PNG frames, .zip). At most {MAX_EXPORT_FRAMES} frames; GIF is limited to 50 fps. Overwrites an existing file of the same kind."),
            json!({"node_id":id,"format":{"type":"string","enum":["gif","webm","png"]},"fps":{"type":"integer","minimum":1,"maximum":120,"description":"Defaults to the timeline's fps."},"scale":{"type":"number","minimum":0.25,"maximum":4,"default":1},"path":{"type":"string","description":"Relative path inside the workspace ending in .gif, .webm or .zip; parent directory must exist."}}),
            vec!["node_id", "format", "path"],
            false,
            true,
        ),
    ]
}
