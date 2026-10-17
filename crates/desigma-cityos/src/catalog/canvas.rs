//! Builds temporary native inspection objects without editor or persistence state.
use desigma_cityos_catalog::Entry;
use desigma_doc::{Color, Document, Node, Paint, Rect, TextStyle};
use serde_json::Value;

fn label(doc: &mut Document, parent: desigma_doc::NodeId, text: &str, x: f32, y: f32, width: f32, size: f32) {
    let id = doc.alloc_id();
    let mut node = Node::text(id, Rect::new(x, y, width, 48.), text, TextStyle { font_size: size, ..Default::default() });
    node.locked = true;
    doc.insert(parent, node).expect("valid specimen parent");
}
fn rows(value: &Value, prefix: &str, out: &mut Vec<(String, String)>, depth: usize) {
    if out.len() >= 240 || depth > 8 {
        return;
    }
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                rows(value, &if prefix.is_empty() { key.clone() } else { format!("{prefix}.{key}") }, out, depth + 1);
            }
        }
        Value::Array(items) => {
            for (i, value) in items.iter().take(48).enumerate() {
                rows(value, &format!("{prefix}[{i}]"), out, depth + 1);
            }
        }
        _ => out.push((prefix.into(), value.as_str().map(str::to_owned).unwrap_or_else(|| value.to_string()))),
    }
}
/// Project the captured specimen into a detached, locked native document.
pub fn build_document(entry: &Entry, specimen: &Value, mode: &str) -> Document {
    let mut doc = Document::new("Temporary CityOS specimen");
    let page = doc.add_page("Inspection canvas");
    label(&mut doc, page, &entry.label, 0., 0., 1080., 28.);
    label(&mut doc, page, "Temporary native objects · inspection only · no document or owner edits", 0., 54., 1080., 16.);
    let tokens = specimen["presentation"]["modes"][mode].as_array();
    if let Some(tokens) = tokens {
        for (i, row) in tokens.iter().enumerate() {
            let x = (i % 4) as f32 * 280.;
            let y = 124. + (i / 4) as f32 * 150.;
            if let Some(value) = row["resolvedValue"].as_str().and_then(Color::from_hex) {
                let id = doc.alloc_id();
                let mut swatch = Node::rectangle(id, Rect::new(x, y, 250., 64.));
                swatch.name = row["name"].as_str().unwrap_or("Token").into();
                swatch.locked = true;
                swatch.fills = vec![Paint::solid(value)];
                doc.insert(page, swatch).expect("valid swatch parent");
            }
            label(&mut doc, page, row["name"].as_str().unwrap_or("Token"), x, y + 72., 260., 13.);
            let text = row["resolvedValue"].as_str().unwrap_or("Unresolved / structured value");
            label(&mut doc, page, text, x, y + 100., 260., 15.);
        }
    } else {
        let mut fields = vec![];
        rows(&specimen["data"], "", &mut fields, 0);
        if fields.is_empty() {
            fields.push(("Source".into(), entry.source_path.clone()));
        }
        for (i, (key, value)) in fields.iter().enumerate() {
            let y = 120. + i as f32 * 56.;
            label(&mut doc, page, key, 0., y, 480., 14.);
            let display = value.chars().take(160).collect::<String>();
            label(&mut doc, page, &display, 500., y, 600., 14.);
        }
        if fields.len() >= 240 {
            label(&mut doc, page, "Canvas shows the first 240 fields. The complete payload remains in Raw data.", 0., 120. + 240. * 56., 1100., 16.);
        }
    }
    doc
}
