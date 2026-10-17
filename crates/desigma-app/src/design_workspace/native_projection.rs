//! Bounded component visuals over semantic occurrences. Never imports browser geometry.
use desigma_doc::{CityosOccurrence, Color, Document, LineHeight, Node, NodeKind, Paint, Path, PathCmd, Rect,
    StrokeAlign, StrokeCap, StrokeJoin, TextAlignH, TextAlignV, TextAutoResize, TextDirection, TextStyle, Vec2};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

const MAX_NODES: usize = 256;
const MAX_TEXT: usize = 16 * 1024;
const MAX_COORDINATE: f32 = 10_000.;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Scene {
    schema_version: String,
    adapter: String,
    locale: String,
    direction: String,
    theme: String,
    viewport: Viewport,
    nodes: Vec<Primitive>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Viewport { width: f32, height: f32 }
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Primitive {
    id: String,
    parent: Option<String>,
    occurrence_id: String,
    kind: String,
    x: f32, y: f32, width: f32, height: f32,
    label: String,
    fill: Option<[f32; 4]>,
    stroke: Option<Outline>,
    corner_radius: Option<f32>,
    clips_content: Option<bool>,
    text: Option<Text>,
    path: Option<Vec<(String, f32, f32)>>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Outline { color: [f32; 4], width: f32, cap: Option<String>, join: Option<String> }
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Text {
    value: String, font_family: String, font_weight: u16, font_size: f32, line_height: f32,
    align: String, direction: String,
}
struct Occurrence { definition: String, source: String, parent: Option<String> }
fn identity(value: &str) -> bool {
    !value.is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
        && !value.contains(['/', '\\']) && !value.contains("..")
}
fn literal(value: &Value, key: &str) -> Result<String, String> {
    value[key].as_str().filter(|v| identity(v)).map(str::to_owned).ok_or_else(|| format!("Invalid scene {key}"))
}
fn finite(value: f32, min: f32, max: f32) -> bool { value.is_finite() && value >= min && value <= max }
fn color(value: [f32; 4]) -> Result<Color, String> {
    if !value.into_iter().all(|v| finite(v, 0., 1.)) { return Err("Invalid native scene RGBA paint".into()); }
    Ok(Color { r: value[0], g: value[1], b: value[2], a: value[3] })
}

/// Present scenes fail closed. Only their absence permits the legacy inspection projection.
pub(super) fn document(projection: &Value, name: &str) -> Result<(Document, BTreeMap<String, CityosOccurrence>), String> {
    let scene: Scene = serde_json::from_value(projection["scene"].clone()).map_err(|e| format!("Invalid native scene: {e}"))?;
    if scene.schema_version != "cityos.desigma-native-scene.v1" || scene.adapter != "cityos.accordion.visual.v1"
        || !matches!((scene.locale.as_str(), scene.direction.as_str()), ("ar", "rtl") | ("en", "ltr"))
        || !matches!(scene.theme.as_str(), "light" | "dark")
        || !finite(scene.viewport.width, 1., 8192.) || !finite(scene.viewport.height, 1., 8192.)
        || scene.nodes.is_empty() || scene.nodes.len() > MAX_NODES {
        return Err("Unsupported native scene, scenario or resource bounds".into());
    }
    let rows = projection["nodes"].as_array().filter(|rows| !rows.is_empty() && rows.len() <= MAX_NODES).ok_or("Missing semantic scene occurrences")?;
    let mut semantic = BTreeMap::new();
    for row in rows {
        let id = literal(row, "id")?;
        let occurrence = Occurrence { definition: literal(row, "definitionId")?, source: literal(row, "sourceNodeId")?,
            parent: match &row["parent"] { Value::Null => None, Value::String(parent) if semantic.contains_key(parent) => Some(parent.clone()),
                _ => return Err("Semantic scene parent must precede its child".into()) } };
        if row["protected"] != true || semantic.insert(id, occurrence).is_some() { return Err("Duplicate or unprotected semantic scene occurrence".into()); }
    }
    let mut doc = Document::new(name);
    let page = doc.add_page("Bound canvas");
    let mut ids = BTreeMap::new();
    let mut views = BTreeMap::new();
    let mut owners = BTreeMap::<String, String>::new();
    let mut seen = BTreeSet::new();
    let mut roots = 0;
    let mut text_bytes = 0;
    for row in scene.nodes {
        if !identity(&row.id) || ids.contains_key(&row.id) || row.label.is_empty() || row.label.len() > 4096
            || row.label.chars().any(|c| c == '\0')
            || !finite(row.x, -MAX_COORDINATE, MAX_COORDINATE) || !finite(row.y, -MAX_COORDINATE, MAX_COORDINATE)
            || !finite(row.width, 0.01, MAX_COORDINATE) || !finite(row.height, 0.01, MAX_COORDINATE)
            || row.width * row.height > 32_000_000. {
            return Err("Invalid or excessive native scene geometry/identity".into());
        }
        let owner = semantic.get(&row.occurrence_id).ok_or("Unknown native scene semantic occurrence")?;
        let parent = if let Some(parent) = &row.parent {
            let id = *ids.get(parent).ok_or("Native scene parent must precede its child")?;
            let parent_owner = owners.get(parent).ok_or("Missing native scene parent owner")?;
            if parent_owner != &row.occurrence_id && owner.parent.as_ref() != Some(parent_owner) {
                return Err("Native scene crosses a semantic owner boundary".into());
            }
            if !matches!(doc.get(id).map(|n| &n.kind), Some(NodeKind::Frame(_))) { return Err("Native scene children require a frame".into()); }
            id
        } else {
            roots += 1;
            if owner.parent.is_some() || roots > 1 { return Err("Native scene requires one semantic root".into()); }
            page
        };
        let id = doc.alloc_id();
        let bounds = Rect::new(row.x, row.y, row.width, row.height);
        let fill = row.fill.map(color).transpose()?;
        let mut node = match row.kind.as_str() {
            "frame" if row.text.is_none() && row.path.is_none() => {
                let radius = row.corner_radius.unwrap_or(0.);
                if !finite(radius, 0., row.width.min(row.height) / 2.) { return Err("Invalid native corner radius".into()); }
                let mut node = Node::frame(id, bounds);
                node.corner_radii = [radius; 4];
                node.kind.frame_props_mut().unwrap().clips_content = row.clips_content.unwrap_or(false);
                node.fills = fill.map(|c| vec![Paint::solid(c)]).unwrap_or_default();
                node
            }
            "text" if row.path.is_none() && row.corner_radius.is_none() && row.clips_content.is_none() && row.stroke.is_none() => {
                let text = row.text.ok_or("Missing native scene text")?;
                text_bytes += text.value.len();
                if text_bytes > MAX_TEXT || text.value.contains('\0') || text.font_family.is_empty() || text.font_family.len() > 128
                    || text.font_family.chars().any(char::is_control) || !(100..=900).contains(&text.font_weight)
                    || !finite(text.font_size, 4., 256.) || !finite(text.line_height, 4., 512.) || text.direction != scene.direction {
                    return Err("Invalid or excessive native scene typography".into());
                }
                let align = match text.align.as_str() { "left" => TextAlignH::Left, "right" => TextAlignH::Right, "center" => TextAlignH::Center,
                    _ => return Err("Unsupported native text alignment".into()) };
                let mut node = Node::text(id, bounds, text.value, TextStyle { font_family: text.font_family, font_weight: text.font_weight,
                    font_size: text.font_size, line_height: LineHeight::Pixels(text.line_height), align_h: align, align_v: TextAlignV::Center,
                    fills: vec![Paint::solid(fill.ok_or("Native scene text requires a paint")?)], ..Default::default() });
                if let NodeKind::Text(text) = &mut node.kind {
                    text.direction = if scene.direction == "rtl" { TextDirection::Rtl } else { TextDirection::Ltr };
                    text.auto_resize = TextAutoResize::Truncate;
                    text.max_lines = Some(1);
                }
                node
            }
            "path" if row.text.is_none() && row.corner_radius.is_none() && row.clips_content.is_none() => {
                let commands = row.path.ok_or("Missing native scene path")?;
                if commands.len() < 2 || commands.len() > 64 { return Err("Invalid native scene path bounds".into()); }
                let mut path = Path::default();
                for (index, (command, x, y)) in commands.into_iter().enumerate() {
                    if !finite(x, 0., row.width) || !finite(y, 0., row.height) { return Err("Native path escapes its bounds".into()); }
                    path.cmds.push(match command.as_str() {
                        "M" if index == 0 => PathCmd::MoveTo(Vec2::new(x, y)),
                        "L" if index > 0 => PathCmd::LineTo(Vec2::new(x, y)),
                        _ => return Err("Unsupported native path command".into()),
                    });
                }
                let mut node = Node::new(id, NodeKind::Vector { paths: vec![path], network: None });
                node.transform.tx = row.x; node.transform.ty = row.y; node.size = Vec2::new(row.width, row.height);
                node.fills = fill.map(|c| vec![Paint::solid(c)]).unwrap_or_default();
                node
            }
            _ => return Err("Unsupported or inconsistent native scene primitive".into()),
        };
        if let Some(stroke) = row.stroke {
            if !finite(stroke.width, 0.01, 32.) { return Err("Invalid native scene stroke".into()); }
            node.stroke.paints = vec![Paint::solid(color(stroke.color)?)];
            node.stroke.weight = stroke.width;
            node.stroke.align = StrokeAlign::Inside;
            node.stroke.cap = match stroke.cap.as_deref().unwrap_or("butt") { "butt" => StrokeCap::None, "round" => StrokeCap::Round,
                _ => return Err("Unsupported native stroke cap".into()) };
            node.stroke.join = match stroke.join.as_deref().unwrap_or("miter") { "miter" => StrokeJoin::Miter, "round" => StrokeJoin::Round,
                _ => return Err("Unsupported native stroke join".into()) };
            if matches!(node.kind, NodeKind::Vector { .. }) { node.stroke.align = StrokeAlign::Center; }
        }
        if parent != page {
            let transform = doc.world_transform(parent);
            node.transform.tx -= transform.tx; node.transform.ty -= transform.ty;
        }
        node.name = row.label;
        node.source_id = Some(owner.source.clone());
        node.locked = true;
        doc.insert(parent, node).map_err(|e| e.to_string())?;
        ids.insert(row.id.clone(), id);
        owners.insert(row.id.clone(), row.occurrence_id.clone());
        seen.insert(row.occurrence_id.clone());
        views.insert(id.0.to_string(), CityosOccurrence { definition_id: owner.definition.clone(), source_node_id: owner.source.clone(),
            instance_id: None, occurrence_id: row.occurrence_id, view_id: format!("native-canvas/{}", row.id) });
    }
    if seen.len() != semantic.len() { return Err("Native scene omitted a semantic occurrence".into()); }
    Ok((doc, views))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn projection() -> Value {
        json!({"nodes":[
            {"id":"draft:accordion","sourceNodeId":"accordion","definitionId":"Accordion","parent":null,"protected":true,"x":0,"y":0,"width":560,"height":139,"label":"Old inspection root","fill":"#ffffff"},
            {"id":"draft:first","sourceNodeId":"first","definitionId":"Accordion","parent":"draft:accordion","protected":true,"x":25,"y":25,"width":510,"height":44,"label":"Old inspection item","fill":"#ffffff"}],
            "scene":{"schemaVersion":"cityos.desigma-native-scene.v1","adapter":"cityos.accordion.visual.v1","locale":"ar","direction":"rtl","theme":"dark","viewport":{"width":560,"height":720},"nodes":[
                {"id":"root","occurrenceId":"draft:accordion","parent":null,"kind":"frame","x":0,"y":0,"width":560,"height":139,"label":"Accordion","fill":[0.1,0.1,0.1,1],"cornerRadius":8,"clipsContent":true,"stroke":{"color":[1,1,1,0.12],"width":1}},
                {"id":"trigger","occurrenceId":"draft:first","parent":"root","kind":"frame","x":25,"y":25,"width":510,"height":44,"label":"First trigger"},
                {"id":"label","occurrenceId":"draft:first","parent":"trigger","kind":"text","x":70,"y":37,"width":449,"height":20,"label":"First label","fill":[0.95,0.95,0.95,1],"text":{"value":"القسم الأول","fontFamily":"Inter","fontWeight":500,"fontSize":14,"lineHeight":20,"align":"right","direction":"rtl"}},
                {"id":"chevron","occurrenceId":"draft:first","parent":"trigger","kind":"path","x":41,"y":40,"width":14,"height":14,"label":"Collapsed indicator","stroke":{"color":[0.5,0.5,0.5,1],"width":1.5,"cap":"round","join":"round"},"path":[["M",3.5,5.5],["L",7,9],["L",10.5,5.5]]}
            ]}})
    }
    #[test]
    fn the_production_importer_materializes_visual_parts_without_inspection_captions() {
        let (doc, bindings) = super::super::projection_document(&projection(), "Accordion").unwrap();
        assert_eq!(bindings.len(), 4);
        let texts = doc.nodes.values().filter_map(|n| if let NodeKind::Text(t) = &n.kind { Some(t) } else { None }).collect::<Vec<_>>();
        assert_eq!(texts.len(), 1);
        assert_eq!(texts[0].content, "القسم الأول");
        assert_eq!(texts[0].style.font_size, 14.);
        assert_eq!(texts[0].style.line_height, LineHeight::Pixels(20.));
        assert_eq!(texts[0].direction, TextDirection::Rtl);
        assert!(doc.nodes.values().any(|n| matches!(n.kind, NodeKind::Vector { .. }) && n.stroke.weight == 1.5));
        assert!(doc.nodes.values().filter(|n| !doc.pages.contains(&n.id)).all(|n| n.locked));
        assert_eq!(bindings.values().filter(|v| v.source_node_id == "first").count(), 3);
        assert_eq!(bindings.values().map(|v| &v.view_id).collect::<BTreeSet<_>>().len(), 4);
        let root = doc.nodes.values().find(|n| n.source_id.as_deref() == Some("accordion")).unwrap();
        assert_eq!(root.corner_radii, [8.; 4]); assert!(root.kind.frame_props().unwrap().clips_content);
        let Paint { kind: desigma_doc::PaintKind::Solid(c), .. } = root.stroke.paints[0] else { panic!("RGBA stroke") };
        assert_eq!(c.a, 0.12);
        let reopened = Document::from_json(&doc.to_json().unwrap()).unwrap();
        assert_eq!(reopened.nodes.len(), doc.nodes.len());
    }
    #[test]
    fn invalid_scenes_never_fall_back_to_inspection_cards() {
        for case in ["schema", "adapter", "direction", "paint", "parent", "duplicate", "owner", "cross-owner", "radius", "path", "extent", "text", "missing-owner", "unknown-field"] {
            let mut p = projection();
            match case {
                "schema" => p["scene"]["schemaVersion"] = json!("future"),
                "adapter" => p["scene"]["adapter"] = json!("unqualified"),
                "direction" => p["scene"]["direction"] = json!("ltr"),
                "paint" => p["scene"]["nodes"][0]["fill"] = json!([0,0,0,2]),
                "parent" => p["scene"]["nodes"][1]["parent"] = json!("label"),
                "duplicate" => p["scene"]["nodes"][1]["id"] = json!("root"),
                "owner" => p["scene"]["nodes"][2]["occurrenceId"] = json!("foreign"),
                "cross-owner" => p["scene"]["nodes"][2]["occurrenceId"] = json!("draft:accordion"),
                "radius" => p["scene"]["nodes"][0]["cornerRadius"] = json!(300),
                "path" => p["scene"]["nodes"][3]["path"][1] = json!(["eval",7,9]),
                "extent" => p["scene"]["nodes"][0]["width"] = json!(1e30),
                "text" => p["scene"]["nodes"][2]["text"]["value"] = json!("x".repeat(MAX_TEXT + 1)),
                "missing-owner" => { p["scene"]["nodes"].as_array_mut().unwrap().truncate(1); },
                _ => p["scene"]["execute"] = json!("forbidden"),
            }
            assert!(super::super::projection_document(&p, "Rejected").is_err(), "{case}");
        }
    }
}
