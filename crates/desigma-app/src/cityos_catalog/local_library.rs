//! Explicit native copies for local design work. These documents have no CityOS write binding.
use desigma_cityos_catalog::Entry;
use desigma_doc::{Color, Document, Node, Paint, Rect, Style, StyleMeta, TextStyle, Variable, VariableCollection, VariableKind, VariableMode, VariableValue};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub(super) fn parse_color(value: &str) -> Option<Color> {
    if let Some(hex) = value.strip_prefix('#') {
        let expanded = if hex.len() == 4 { hex.chars().flat_map(|c| [c, c]).collect::<String>() } else { hex.into() };
        return Color::from_hex(&expanded);
    }
    if value == "transparent" {
        return Some(Color::TRANSPARENT);
    }
    let (body, alpha) = if let Some(v) = value.strip_prefix("rgba(").and_then(|v| v.strip_suffix(')')) {
        (v, true)
    } else {
        (value.strip_prefix("rgb(")?.strip_suffix(')')?, false)
    };
    let fields = body.split(',').map(|v| v.trim().parse::<f32>().ok()).collect::<Option<Vec<_>>>()?;
    if fields.len() != if alpha { 4 } else { 3 } || fields[..3].iter().any(|v| !v.is_finite() || !(0.0..=255.0).contains(v)) {
        return None;
    }
    let a = if alpha { fields[3] } else { 1. };
    if !a.is_finite() || !(0.0..=1.0).contains(&a) {
        return None;
    }
    Some(Color { r: fields[0] / 255., g: fields[1] / 255., b: fields[2] / 255., a })
}

fn text(doc: &mut Document, page: desigma_doc::NodeId, value: &str, x: f32, y: f32, width: f32, size: f32) -> Result<desigma_doc::NodeId, String> {
    let id = doc.alloc_id();
    let node = Node::text(id, Rect::new(x, y, width, 48.), value, TextStyle { font_size: size, ..Default::default() });
    doc.insert(page, node).map_err(|e| e.to_string())?;
    Ok(id)
}
fn literal(row: &Value) -> Result<(VariableKind, VariableValue), String> {
    let value = &row["resolvedValue"];
    match row["type"].as_str().unwrap_or("token") {
        "color" => {
            return value
                .as_str()
                .and_then(parse_color)
                .map(|v| (VariableKind::Color, VariableValue::Color(v)))
                .ok_or_else(|| "This color format is not supported by the native variable adapter.".into())
        }
        _ => {}
    }
    match value {
        Value::String(v) => Ok((VariableKind::String, VariableValue::String(v.clone()))),
        Value::Bool(v) => Ok((VariableKind::Boolean, VariableValue::Boolean(*v))),
        Value::Number(v) => v
            .as_f64()
            .map(|v| v as f32)
            .filter(|v| v.is_finite())
            .map(|v| (VariableKind::Number, VariableValue::Number(v)))
            .ok_or_else(|| "Numeric token exceeds the native variable range.".into()),
        _ => Err("Unresolved aliases or structured token values require another adapter; no partial library was created.".into()),
    }
}

pub(super) fn create(entry: &Entry, specimen: &Value, snapshot: &Value, preferred_mode: &str) -> Result<Document, String> {
    if snapshot["mode"] != "committed" || !snapshot["canonicalCommit"].as_str().is_some_and(|s| s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit())) {
        return Err("Create local libraries from committed source. Refresh GitLab main first; local overlays remain previews.".into());
    }
    if specimen["entry"]["id"] != entry.id || specimen["digest"] != entry.source_digest {
        return Err("The selected source changed. Select it again before creating a local library.".into());
    }
    if !matches!(entry.kind.as_str(), "theme" | "token" | "design-token") {
        return Err("This source has no native token-library adapter.".into());
    }
    let data = &specimen["data"];
    let modes: BTreeMap<String, Vec<Value>> = if entry.kind == "theme" {
        let modes = specimen["presentation"]["modes"].as_object().ok_or("Theme tokens have not loaded yet.")?;
        modes.iter().map(|(key, rows)| Ok((key.clone(), rows.as_array().ok_or("Invalid theme rows")?.clone()))).collect::<Result<_, String>>()?
    } else if let Some(value) = data.get("$value").or_else(|| data.get("value")) {
        if value.as_str().is_some_and(|s| s.starts_with('{') && s.ends_with('}')) {
            return Err("Load the complete theme to resolve this token alias.".into());
        }
        BTreeMap::from([(
            "Value".into(),
            vec![
                json!({"name":entry.label,"type":data.get("$type").or_else(||data.get("type")).and_then(Value::as_str).unwrap_or("token"),"value":value,"resolvedValue":value}),
            ],
        )])
    } else {
        return Err("This item is a source declaration. It does not have a native variable adapter.".into());
    };
    let total = modes.values().map(Vec::len).sum::<usize>();
    if total == 0 || total > 4096 || modes.len() > 16 {
        return Err("Local token libraries support 1–4096 values per selected source.".into());
    }
    let source = json!({"kind":"local-design-copy","sourcePath":entry.source_path,"sourcePointer":entry.source_pointer,
        "sourceDigest":entry.source_digest,"canonicalCommit":snapshot["canonicalCommit"],"ownerWrites":false});
    let mut doc = Document::new(format!("{} · local design library", entry.label));
    let mut modes = modes.into_iter().collect::<Vec<_>>();
    modes.sort_by_key(|(mode, _)| (mode != preferred_mode, mode.clone()));
    for (mode, rows) in modes {
        if rows.is_empty() {
            continue;
        }
        let page = doc.add_page(&mode);
        text(&mut doc, page, &format!("{} · {}", entry.label, mode), 0., 0., 1100., 28.)?;
        text(&mut doc, page, "Editable local variables and samples · source changes are not submitted", 0., 52., 1100., 15.)?;
        // Partial dark-mode exports remain partial. Separate collections avoid
        // inventing inheritance or default values that the source did not declare.
        let collection_id = uuid::Uuid::new_v4().to_string();
        let mode_id = uuid::Uuid::new_v4().to_string();
        let mut ids = BTreeMap::new();
        let mut collection = VariableCollection {
            id: collection_id.clone(),
            name: format!("{} / {}", entry.label, mode),
            modes: vec![VariableMode { id: mode_id.clone(), name: mode.clone() }],
            default_mode: mode_id.clone(),
            variables: vec![],
            key: None,
        };
        for row in &rows {
            let name = row["name"].as_str().ok_or("Token name is missing")?;
            if ids.contains_key(name) {
                return Err("Duplicate token names require source reconciliation.".into());
            }
            let (kind, value) = literal(row)?;
            let id = uuid::Uuid::new_v4().to_string();
            ids.insert(name.to_owned(), id.clone());
            collection.variables.push(id.clone());
            doc.variables.insert(
                id.clone(),
                Variable {
                    id,
                    name: name.replace('.', "/"),
                    collection: collection_id.clone(),
                    kind,
                    values: BTreeMap::from([(mode_id.clone(), value)]),
                    description: json!({"source":source,"mode":mode,"original":row}).to_string(),
                    scopes: vec![],
                    hidden_from_publishing: false,
                    key: None,
                },
            );
        }
        for (index, row) in rows.iter().enumerate() {
            let name = row["name"].as_str().unwrap();
            let id = ids.get(name).unwrap();
            if let Some(alias) = row["value"].as_str().and_then(|s| s.strip_prefix('{')).and_then(|s| s.strip_suffix('}')) {
                let target = ids.get(alias).ok_or("Alias target is not in the captured mode; import stopped.")?;
                if doc.variables[target].kind != doc.variables[id].kind {
                    return Err("Alias kinds do not match; import stopped.".into());
                }
                doc.variables.get_mut(id).unwrap().values.insert(mode_id.clone(), VariableValue::Alias(target.clone()));
            }
            let x = (index % 4) as f32 * 300.;
            let y = 120. + (index / 4) as f32 * 164.;
            if let Some(color) = row["resolvedValue"].as_str().and_then(parse_color).filter(|_| doc.variables[id].kind == VariableKind::Color) {
                let node_id = doc.alloc_id();
                let mut node = Node::rectangle(node_id, Rect::new(x, y, 264., 72.));
                node.name = name.into();
                node.fills = vec![Paint::solid(color)];
                node.variables.bindings.insert("fills/0".into(), id.clone());
                doc.insert(page, node).map_err(|e| e.to_string())?;
                let style_id = uuid::Uuid::new_v4().to_string();
                doc.styles.insert(style_id.clone(), Style::Fill(vec![Paint::solid(color)]));
                doc.style_meta
                    .insert(style_id, StyleMeta { name: format!("{mode}/{}", name.replace('.', "/")), description: source.to_string(), ..Default::default() });
            }
            text(&mut doc, page, name, x, y + 80., 284., 14.)?;
            let display = row["resolvedValue"].as_str().map(str::to_owned).unwrap_or_else(|| row["resolvedValue"].to_string());
            let text_id = text(&mut doc, page, &display, x, y + 111., 284., 15.)?;
            if doc.variables[id].kind == VariableKind::String {
                doc.get_mut(text_id).unwrap().variables.bindings.insert("characters".into(), id.clone());
            }
        }
        let variables = collection.variables.clone();
        doc.variable_collections.insert(collection_id, collection);
        for id in variables {
            if doc.resolve_variable(page, &id).is_none() {
                return Err("A native alias cannot be resolved; import stopped.".into());
            }
        }
    }
    let page = doc.add_page("Source & provenance");
    text(&mut doc, page, "Local design copy · no CityOS submission capability", 0., 0., 1200., 24.)?;
    let id = doc.alloc_id();
    let raw = serde_json::to_string_pretty(&json!({"provenance":source,"capturedData":data})).map_err(|e| e.to_string())?;
    let mut node = Node::text(id, Rect::new(0., 72., 1200., 2400.), &raw, TextStyle { font_size: 13., ..Default::default() });
    node.name = "Captured source and provenance".into();
    doc.insert(page, node).map_err(|e| e.to_string())?;
    Ok(doc)
}
