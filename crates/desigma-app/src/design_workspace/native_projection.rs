//! Bounded component visuals over semantic occurrences. Never imports browser geometry.
use desigma_doc::{CityosOccurrence, Color, Document, Effect, LineHeight, Node, NodeKind, Paint, Path, PathCmd, Rect,
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
    effects: Option<Vec<DropShadow>>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DropShadow {
    kind: String,
    color: [f32; 4],
    offset: [f32; 2],
    radius: f32,
    spread: f32,
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
    // ExperienceComposition identifiers permit slash, colon and dot. These
    // references are data keys and never enter filesystem path resolution.
    value.len() <= 512 && value.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
        && value.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b':' | b'/' | b'-'))
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
    let material_scene = scene.adapter == "cityos.living-canvas-frame-material.native-scene.v1";
    let lower_level_scene = scene.adapter == "cityos.living-canvas-lower-level.native-scene.v1";
    if scene.schema_version != "cityos.desigma-native-scene.v1" || !(material_scene || lower_level_scene || scene.adapter == "cityos.accordion.visual.v1")
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
    // Standalone L1/L7 definitions retain their authored root. They cannot gain
    // a synthetic surface parent, additional owner interiors or another shape.
    if lower_level_scene {
        let root = semantic.values().next().ok_or("Missing standalone native scene owner")?;
        if semantic.len() != 1 || root.parent.is_some()
            || !matches!(root.definition.as_str(), "cityos.l1.living-material.v1" | "cityos.l7.living-horizon-region.v1")
            || scene.nodes.len() != 1 || scene.nodes[0].parent.is_some() || scene.nodes[0].kind != "frame" {
            return Err("Standalone native scene requires its one registered authored root".into());
        }
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
        if let Some(effects) = &row.effects {
            if !(material_scene || lower_level_scene) || row.kind != "frame" || owner.definition != "cityos.l1.living-material.v1" || effects.len() > 4 {
                return Err("Unsupported native scene effects or material owner".into());
            }
        }
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
        for effect in row.effects.unwrap_or_default() {
            if effect.kind != "dropShadow" || !effect.offset.into_iter().all(|v| finite(v, -256., 256.))
                || !finite(effect.radius, 0., 256.) || !finite(effect.spread, -256., 256.) {
                return Err("Invalid native material drop shadow".into());
            }
            node.effects.push(Effect::DropShadow { color: color(effect.color)?, offset: Vec2::new(effect.offset[0], effect.offset[1]),
                radius: effect.radius, spread: effect.spread, visible: true, show_behind: false });
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
    fn material_projection() -> Value {
        let mut p = projection();
        p["nodes"][0]["definitionId"] = json!("cityos.l9.living-canvas-frame.v1");
        p["nodes"][1]["definitionId"] = json!("cityos.l1.living-material.v1");
        p["nodes"][1]["sourceNodeId"] = json!("material");
        p["nodes"][1]["slot"] = json!("material");
        let mut horizon = p["nodes"][1].clone();
        horizon["id"] = json!("draft:horizon"); horizon["sourceNodeId"] = json!("horizon");
        horizon["definitionId"] = json!("cityos.l7.living-horizon.v1"); horizon["slot"] = json!("horizon");
        p["nodes"].as_array_mut().unwrap().push(horizon);
        p["scene"]["adapter"] = json!("cityos.living-canvas-frame-material.native-scene.v1");
        p["scene"]["nodes"].as_array_mut().unwrap().truncate(2);
        p["scene"]["nodes"][1]["fill"] = json!([32. / 255., 62. / 255., 53. / 255., 1]);
        p["scene"]["nodes"][1]["effects"] = json!([
            {"kind":"dropShadow","color":[0,0,0,0.35],"offset":[0,10],"radius":15,"spread":-3},
            {"kind":"dropShadow","color":[0,0,0,0.20],"offset":[0,4],"radius":6,"spread":-4}]);
        p["scene"]["nodes"].as_array_mut().unwrap().push(json!({"id":"horizon-proxy","occurrenceId":"draft:horizon",
            "parent":"root","kind":"frame","x":50,"y":80,"width":460,"height":30,"label":"Protected horizon proxy"}));
        p
    }
    #[test]
    fn material_shadows_survive_native_save_and_reopen_without_reparenting_semantic_siblings() {
        let p = material_projection();
        let (doc, bindings) = super::super::projection_document(&p, "Frame material").unwrap();
        let material = doc.nodes.values().find(|n| n.source_id.as_deref() == Some("material")).unwrap();
        let horizon = doc.nodes.values().find(|n| n.source_id.as_deref() == Some("horizon")).unwrap();
        assert_eq!(material.parent, horizon.parent);
        assert_eq!(material.corner_radii, [0.; 4]); assert!(material.locked && horizon.locked);
        assert_eq!(material.effects, vec![
            Effect::DropShadow {color:Color {r:0.,g:0.,b:0.,a:0.35},offset:Vec2::new(0.,10.),radius:15.,spread:-3.,visible:true,show_behind:false},
            Effect::DropShadow {color:Color {r:0.,g:0.,b:0.,a:0.20},offset:Vec2::new(0.,4.),radius:6.,spread:-4.,visible:true,show_behind:false}]);
        assert_eq!(bindings.values().map(|o|o.occurrence_id.as_str()).collect::<BTreeSet<_>>(),
            BTreeSet::from(["draft:accordion","draft:first","draft:horizon"]));
        assert!(!doc.nodes.values().any(|n| matches!(n.kind, NodeKind::Text(_))));
        let reopened = Document::from_json(&doc.to_json().unwrap()).unwrap();
        assert_eq!(reopened.get(material.id).unwrap().effects, material.effects);
        let mut flat = p; flat["scene"]["nodes"][1]["effects"] = json!([]);
        let (doc, _) = super::super::projection_document(&flat, "Flat").unwrap();
        assert!(doc.nodes.values().all(|n|n.effects.is_empty()));
    }
    fn lower_level_projection(horizon: bool) -> Value {
        let p = material_projection();
        let mut semantic = p["nodes"][if horizon { 2 } else { 1 }].clone();
        semantic["parent"] = Value::Null;
        if horizon { semantic["definitionId"] = json!("cityos.l7.living-horizon-region.v1"); }
        let mut primitive = p["scene"]["nodes"][if horizon { 2 } else { 1 }].clone();
        primitive["parent"] = Value::Null;
        let mut result = json!({"nodes":[semantic],"scene":p["scene"]});
        result["scene"]["adapter"] = json!("cityos.living-canvas-lower-level.native-scene.v1");
        result["scene"]["nodes"] = json!([primitive]);
        result
    }
    #[test]
    fn standalone_material_and_horizon_keep_their_authored_root_and_view_identity_on_reopen() {
        for horizon in [false, true] {
            let p = lower_level_projection(horizon);
            let (doc, bindings) = super::super::projection_document(&p, "Standalone").unwrap();
            let source = if horizon { "horizon" } else { "material" };
            let root = doc.nodes.values().find(|node| node.source_id.as_deref() == Some(source)).unwrap();
            assert_eq!(root.parent, Some(doc.pages[0]));
            assert!(root.locked);
            assert_eq!(bindings.len(), 1);
            let binding = &bindings[&root.id.0.to_string()];
            assert_eq!(binding.source_node_id, source);
            assert_eq!(binding.definition_id, if horizon { "cityos.l7.living-horizon-region.v1" } else { "cityos.l1.living-material.v1" });
            assert_eq!(binding.occurrence_id, p["nodes"][0]["id"].as_str().unwrap());
            assert_eq!(binding.view_id, format!("native-canvas/{}", p["scene"]["nodes"][0]["id"].as_str().unwrap()));
            assert_eq!(root.effects.len(), if horizon { 0 } else { 2 });
            let reopened = Document::from_json(&doc.to_json().unwrap()).unwrap();
            assert_eq!(reopened.get(root.id).unwrap().effects, root.effects);
            assert_eq!(reopened.nodes.len(), doc.nodes.len());
        }
    }
    #[test]
    fn canonical_source_identifiers_with_slashes_and_dots_survive_the_native_importer() {
        let mut p = lower_level_projection(false);
        p["nodes"][0]["id"] = json!("draft:authored/root:material.v1");
        p["nodes"][0]["sourceNodeId"] = json!("authored/root:material.v1");
        p["scene"]["nodes"][0]["occurrenceId"] = p["nodes"][0]["id"].clone();
        p["scene"]["nodes"][0]["id"] = json!("draft:scene:authored/root:material.v1");
        let (doc, bindings) = super::super::projection_document(&p, "Authored identity").unwrap();
        let occurrence = bindings.values().next().unwrap();
        assert_eq!(occurrence.source_node_id, "authored/root:material.v1");
        assert_eq!(occurrence.occurrence_id, "draft:authored/root:material.v1");
        assert_eq!(occurrence.view_id, "native-canvas/draft:scene:authored/root:material.v1");
        assert!(doc.nodes.values().any(|node| node.source_id.as_deref() == Some("authored/root:material.v1")));
        // Identifiers are bounded data, never filesystem paths. Retain the
        // canonical alphabet while rejecting controls, backslashes and spaces.
        for invalid in ["root\0child", "root\\child", "root child", "root\nchild"] {
            let mut invalid_projection = p.clone();
            invalid_projection["nodes"][0]["sourceNodeId"] = json!(invalid);
            assert!(super::super::projection_document(&invalid_projection, "Invalid identity").is_err());
        }
    }
    #[test]
    fn standalone_scenes_reject_synthetic_parents_foreign_owners_interiors_and_horizon_effects() {
        for case in ["surface", "foreign", "unregistered-horizon-alias", "semantic-parent", "extra-occurrence", "extra-view", "view-parent", "primitive", "unknown-adapter", "horizon-effects"] {
            let mut p = lower_level_projection(case == "horizon-effects");
            match case {
                "surface" => p["nodes"][0]["definitionId"] = json!("cityos.l9.living-canvas-frame.v1"),
                "foreign" => p["nodes"][0]["definitionId"] = json!("owner.foreign"),
                "unregistered-horizon-alias" => p["nodes"][0]["definitionId"] = json!("cityos.l7.living-horizon.v1"),
                "semantic-parent" => p["nodes"][0]["parent"] = json!("invented-surface"),
                "extra-occurrence" => { let mut row = p["nodes"][0].clone(); row["id"] = json!("other"); p["nodes"].as_array_mut().unwrap().push(row); },
                "extra-view" => { let mut row = p["scene"]["nodes"][0].clone(); row["id"] = json!("interior"); p["scene"]["nodes"].as_array_mut().unwrap().push(row); },
                "view-parent" => p["scene"]["nodes"][0]["parent"] = json!("invented-surface"),
                "primitive" => p["scene"]["nodes"][0]["kind"] = json!("path"),
                "unknown-adapter" => p["scene"]["adapter"] = json!("cityos.living-canvas-lower-level.native-scene.future"),
                _ => p["scene"]["nodes"][0]["effects"] = json!([]),
            }
            assert!(super::super::projection_document(&p, "Rejected standalone").is_err(), "{case}");
        }
    }
    #[test]
    fn material_effects_reject_unknown_owners_adapters_shapes_and_unbounded_payloads() {
        for case in ["adapter","owner","primitive","kind","paint","offset","radius","spread","count","unknown-field"] {
            let mut p = material_projection();
            match case {
                "adapter" => p["scene"]["adapter"] = json!("cityos.accordion.visual.v1"),
                "owner" => p["nodes"][1]["definitionId"] = json!("cityos.l7.living-horizon.v1"),
                "primitive" => p["scene"]["nodes"][1]["kind"] = json!("text"),
                "kind" => p["scene"]["nodes"][1]["effects"][0]["kind"] = json!("backgroundBlur"),
                "paint" => p["scene"]["nodes"][1]["effects"][0]["color"] = json!([0,0,0,1.1]),
                "offset" => p["scene"]["nodes"][1]["effects"][0]["offset"] = json!([0,257]),
                "radius" => p["scene"]["nodes"][1]["effects"][0]["radius"] = json!(-1),
                "spread" => p["scene"]["nodes"][1]["effects"][0]["spread"] = json!(-257),
                "count" => {let effect=p["scene"]["nodes"][1]["effects"][0].clone();p["scene"]["nodes"][1]["effects"]=json!(vec![effect;5]);},
                _ => p["scene"]["nodes"][1]["effects"][0]["execute"] = json!("forbidden"),
            }
            assert!(super::super::projection_document(&p,"Rejected material").is_err(),"{case}");
        }
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
