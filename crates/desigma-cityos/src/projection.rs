//! Native scene dispatch and the generic inspection projection.
mod native_projection;

use desigma_doc::{CityosOccurrence, Color, Document, Node, Paint, Rect, TextStyle};
use serde_json::Value;
use std::collections::BTreeMap;

pub fn projection_document(projection: &Value, name: &str) -> Result<(Document,BTreeMap<String,CityosOccurrence>),String> {
    if projection.get("scene").is_some() { return native_projection::document(projection, name); }
    let string = |v: &Value, key: &str| v[key].as_str().map(str::to_owned).ok_or_else(|| format!("Missing {key}"));
    let mut doc = Document::new(name);
    let page = doc.add_page("Bound canvas");
    let mut occurrences = BTreeMap::new();
    let mut ids = BTreeMap::new();
    for row in projection["nodes"].as_array().ok_or("Missing canvas projection")? {
        let id = doc.alloc_id();
        let key = string(row, "id")?;
        if key.is_empty() || ids.contains_key(&key) { return Err("Duplicate or empty native occurrence identity".into()); }
        ids.insert(key.clone(), id);
        let number =
            |name: &str| row[name].as_f64().filter(|n| n.is_finite() && n.abs() < 10000.).map(|n| n as f32).ok_or_else(|| format!("Invalid projection {name}"));
        let parent = match row.get("parent") {
            None | Some(Value::Null) => page,
            Some(Value::String(parent)) => *ids.get(parent).filter(|parent| **parent != id).ok_or("Native projection parent must precede its child")?,
            _ => return Err("Invalid native projection parent".into()),
        };
        if number("width")?<=0. || number("height")?<=0. { return Err("Invalid native projection extent".into()); }
        let mut node = Node::frame(id, Rect::new(number("x")?, number("y")?, number("width")?, number("height")?));
        // Adapter coordinates are page-space. Native nodes retain the named-slot parent relation.
        if parent != page {
            let transform = doc.world_transform(parent);
            node.transform.tx -= transform.tx;
            node.transform.ty -= transform.ty;
        }
        node.name = string(row, "label")?;
        node.source_id = Some(string(row, "sourceNodeId")?);
        node.locked = true;
        node.fills = vec![Paint::solid(row["fill"].as_str().and_then(Color::from_hex).ok_or("Unsupported canonical projection paint")?)];
        node.corner_radii = [12.; 4];
        doc.insert(parent, node).map_err(|e| e.to_string())?;
        let text_id = doc.alloc_id();
        let mut caption = Node::text(
            text_id,
            Rect::new(12., 12., (number("width")? - 24.).max(20.), 44.),
            row["label"].as_str().unwrap_or("Source node"),
            TextStyle { font_size: 16., ..Default::default() },
        );
        if let desigma_doc::NodeKind::Text(text) = &mut caption.kind {
            // Inspection labels are derived captions, not unconstrained authored
            // text. Keep long bilingual summaries inside their native object.
            text.auto_resize = desigma_doc::TextAutoResize::Truncate;
            text.max_lines = Some(2);
        }
        caption.locked = true;
        doc.insert(id, caption).map_err(|e| e.to_string())?;
        occurrences.insert(
            id.0.to_string(),
            CityosOccurrence {
                definition_id: string(row, "definitionId")?,
                source_node_id: string(row, "sourceNodeId")?,
                instance_id: None,
                occurrence_id: key,
                view_id: "native-canvas".into(),
            },
        );
    }
    Ok((doc,occurrences))
}
