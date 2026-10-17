//! Data presentations only. No repository code, target renderer or editor document is used.
use super::ui::chip;
use super::CatalogUiServices;
use desigma_cityos_catalog::Entry;
use egui::{Color32, Frame, RichText, Ui};
use serde_json::Value;

pub(super) fn humanize(value: &str) -> String {
    let mut text = String::new();
    let mut previous_lower = false;
    for ch in value.chars() {
        if matches!(ch, '.' | '_' | '-' | '/') {
            if !text.ends_with(' ') {
                text.push(' ');
            }
            previous_lower = false;
        } else {
            if ch.is_uppercase() && previous_lower {
                text.push(' ');
            }
            text.push(ch);
            previous_lower = ch.is_lowercase();
        }
    }
    let text = text.trim();
    let mut chars = text.chars();
    chars.next().map(|first| first.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default()
}
pub(super) fn display_name(entry: &Entry) -> String {
    let label = entry.label.trim_start_matches("token:/");
    if let Some(rest) = label.strip_prefix("auto.") {
        if let Some(name) = rest.split('.').nth(1) {
            return humanize(name);
        }
    }
    if label.starts_with("@{") {
        if let Some(id) = label.split("shellId=").nth(1).and_then(|text| text.split(';').next()) {
            return humanize(id);
        }
    }
    if entry.kind == "source-file" || entry.kind == "public-declaration" || entry.kind == "package-export" {
        return label.to_string();
    }
    let label = label.strip_prefix("cityos.").unwrap_or(label);
    let label = label.split_once('.').filter(|(prefix, _)| prefix.starts_with('l') && prefix[1..].parse::<u8>().is_ok()).map(|(_, rest)| rest).unwrap_or(label);
    let label = label.strip_suffix(".v1").unwrap_or(label);
    humanize(label)
}
pub(super) fn kind_name(kind: &str) -> &str {
    match kind {
        "token" | "design-token" => "Token",
        "theme" => "Theme",
        "registered-component" => "Component",
        "registered-artifact" | "registration" | "artifact" => "Definition",
        "authored-composition" => "Composition",
        "authored-node" => "Composition node",
        "product" => "Product",
        "source-file" => "Source file",
        "public-declaration" => "Export",
        "source-consumer" => "Consumer",
        "nonvisual-dependency" => "Code dependency",
        "package-export" => "Package export",
        "asset" => "Asset",
        _ => "Source record",
    }
}
pub(super) fn source_text(specimen: &Value) -> String {
    match &specimen["data"] {
        Value::String(text) => text.clone(),
        data => serde_json::to_string_pretty(data).unwrap_or_default(),
    }
}
pub(super) fn color(value: &Value) -> Option<Color32> {
    let color = super::local_library::parse_color(value.as_str()?)?;
    let channel = |v: f32| (v * 255.).round() as u8;
    Some(Color32::from_rgba_unmultiplied(channel(color.r), channel(color.g), channel(color.b), channel(color.a)))
}
fn compact(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(items) if items.iter().all(|v| !v.is_object() && !v.is_array()) => items.iter().map(compact).collect::<Vec<_>>().join(", "),
        Value::Array(items) => format!("{} items", items.len()),
        Value::Object(fields) => format!("{} fields", fields.len()),
        _ => value.to_string(),
    }
}
fn property(ui: &mut Ui, services: CatalogUiServices, key: &str, value: &Value) {
    let theme = services.theme();
    ui.label(RichText::new(humanize(key)).color(theme.text_secondary));
    ui.label(compact(value));
    ui.end_row();
}
fn structured(ui: &mut Ui, services: CatalogUiServices, value: &Value, path: &str, depth: usize) {
    if depth > 8 {
        ui.small("More detail is available in Raw data.");
        return;
    }
    match value {
        Value::Object(fields) => {
            egui::Grid::new(("catalog-properties", path)).num_columns(2).spacing(egui::vec2(20.0, 10.0)).show(ui, |ui| {
                for (key, value) in fields {
                    if !value.is_object() && (!value.is_array() || value.as_array().is_some_and(|items| items.iter().all(|v| !v.is_object() && !v.is_array())))
                    {
                        property(ui, services, key, value);
                    }
                }
            });
            for (key, value) in fields {
                if value.is_object() || value.as_array().is_some_and(|items| items.iter().any(|v| v.is_object() || v.is_array())) {
                    egui::CollapsingHeader::new(format!("{} · {}", humanize(key), compact(value)))
                        .id_salt((path, key))
                        .show(ui, |ui| structured(ui, services, value, &format!("{path}/{key}"), depth + 1));
                }
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter().take(100).enumerate() {
                egui::CollapsingHeader::new(
                    item["name"].as_str().or_else(|| item["id"].as_str()).map(String::from).unwrap_or_else(|| format!("Item {}", index + 1)),
                )
                .id_salt((path, index))
                .show(ui, |ui| structured(ui, services, item, &format!("{path}/{index}"), depth + 1));
            }
            if items.len() > 100 {
                ui.small(format!("{} more items in Raw data", items.len() - 100));
            }
        }
        _ => {
            ui.label(compact(value));
        }
    }
}
fn composition_node(ui: &mut Ui, services: CatalogUiServices, node: &Value, path: &str, depth: usize, budget: &mut usize) {
    let theme = services.theme();
    if depth > 16 || *budget == 0 {
        ui.small("Additional structure is available in Raw data.");
        return;
    }
    *budget -= 1;
    let id = node["id"].as_str().unwrap_or("Unnamed node");
    Frame::new().fill(theme.bg).corner_radius(theme.catalog_radius).inner_margin(12).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.strong(humanize(id));
            if let Some(component) = node["component"]["id"].as_str() {
                chip(ui, services, component);
            }
        });
        if let Some(props) = node["props"].as_object() {
            if !props.is_empty() {
                egui::Grid::new((path, "props")).num_columns(2).show(ui, |ui| {
                    for (key, value) in props {
                        property(ui, services, key, value);
                    }
                });
            }
        }
        if let Some(slots) = node["slots"].as_object() {
            for (slot, children) in slots {
                ui.add_space(8.0);
                ui.label(RichText::new(format!("Slot · {}", humanize(slot))).small().color(theme.text_secondary));
                ui.indent((path, slot), |ui| {
                    if let Some(children) = children.as_array() {
                        for (index, child) in children.iter().enumerate() {
                            composition_node(ui, services, child, &format!("{path}/{slot}/{index}"), depth + 1, budget);
                        }
                    }
                });
            }
        }
    });
}
pub(super) fn show(ui: &mut Ui, services: CatalogUiServices, entry: &Entry, data: &Value) {
    let theme = services.theme();
    let token_value = data.get("$value").or_else(|| data.get("value"));
    if let Some(value) = token_value {
        let token_type = data["$type"].as_str().or_else(|| data["type"].as_str()).unwrap_or("Token");
        ui.label(RichText::new(humanize(token_type)).small().color(theme.text_secondary));
        ui.add_space(12.0);
        if let Some(color) = color(value) {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width().min(600.0), 160.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, theme.catalog_radius, theme.bg);
            ui.painter().rect_filled(rect, theme.catalog_radius, color);
            ui.add_space(12.0);
        }
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(compact(value)).size(theme.catalog_title).monospace());
            if ui.button("Copy value").clicked() {
                ui.ctx().copy_text(compact(value));
            }
        });
        if value.as_str().is_some_and(|v| v.starts_with('{')) {
            ui.label("Alias · referenced token values have not been resolved in this view.");
        }
        if let Some(description) = data["$description"].as_str().or_else(|| data["description"].as_str()) {
            ui.label(description);
        }
        ui.add_space(16.0);
        ui.small("Token data preview");
    } else if data["composition"]["root"].is_object() || data["root"].is_object() || (data["kind"] == "component" && data["id"].is_string()) {
        ui.strong("Composition structure");
        ui.label(
            RichText::new(if entry.source_path == "config/experience/living-canvas-frame/cityos-web.input.json" {
                "Authored structure. Use Open actual preview above to render this frame, or Open on canvas to edit supported properties."
            } else {
                "Authored structure. The structure workspace preserves these nodes. The selected workspace checks registered renderer and editing support for this source."
            })
            .color(theme.text_secondary),
        );
        ui.add_space(12.0);
        let root = if data["composition"]["root"].is_object() {
            &data["composition"]["root"]
        } else if data["root"].is_object() {
            &data["root"]
        } else {
            data
        };
        composition_node(ui, services, root, &entry.id, 0, &mut 120);
    } else if data.is_object() || data.is_array() {
        ui.strong("Contract overview");
        ui.label(RichText::new(if nonvisual(data) {"This is a code dependency, such as a context provider or utility. It has no standalone visual appearance; preview the component or page that consumes it."}
            else {"Source declaration only. This item still needs a renderer fixture and a native editing adapter."}).color(theme.text_secondary));
        ui.add_space(16.0);
        structured(ui, services, data, &entry.id, 0);
    } else {
        ui.strong("Source available");
        ui.label("This item has no visual data preview yet. Open Raw data to read its captured source.");
        ui.add_space(16.0);
        inspector(ui, services, entry);
    }
}
fn nonvisual(data: &Value) -> bool {
    data["reclassificationReason"]
        .as_str()
        .is_some_and(|s| ["context provider", "non-visual utility", "nonvisual utility"].contains(&s.to_lowercase().as_str()))
        || matches!(data["kind"].as_str(), Some("provider" | "context" | "utility"))
}
pub(super) fn theme_sheet(ui: &mut Ui, presentation: &Value, mode: &mut String) -> bool {
    let Some(modes) = presentation["modes"].as_object() else {
        return false;
    };
    if modes.is_empty() { ui.label("No theme modes were captured for this source."); return true; }
    if !modes.contains_key(mode.as_str()) {
        *mode = modes.keys().next().cloned().unwrap_or_default();
    }
    ui.strong("Theme token sheet");
    ui.small("Captured repository theme · aliases resolved within the selected mode");
    let rows = modes[mode.as_str()].as_array();
    if let Some(rows) = rows {
        if ui.available_width() < 540.0 {
            for row in rows {
                Frame::new().inner_margin(egui::Margin::symmetric(0, 8)).show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        let (rect, _) = ui.allocate_exact_size(egui::vec2(36., 24.), egui::Sense::hover());
                        if let Some(value) = color(&row["resolvedValue"]) { ui.painter().rect_filled(rect, 4, value); }
                        ui.label(row["name"].as_str().unwrap_or(""));
                    });
                    ui.label(RichText::new(if row["resolvedValue"].is_null() { "Unresolved alias".into() } else { compact(&row["resolvedValue"]) }).monospace());
                });
            }
            return true;
        }
        egui::Grid::new("theme-token-sheet").num_columns(3).spacing(egui::vec2(16., 8.)).striped(true).show(ui, |ui| {
            for row in rows {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(36., 24.), egui::Sense::hover());
                if let Some(value) = color(&row["resolvedValue"]) {
                    ui.painter().rect_filled(rect, 4, value);
                }
                ui.label(row["name"].as_str().unwrap_or(""));
                ui.monospace(if row["resolvedValue"].is_null() { "Unresolved alias".into() } else { compact(&row["resolvedValue"]) });
                ui.end_row();
            }
        });
    }
    true
}
pub(super) fn inspector(ui: &mut Ui, services: CatalogUiServices, entry: &Entry) {
    let theme = services.theme();
    ui.strong("Source");
    ui.label(&entry.source_path);
    if !entry.source_pointer.is_empty() {
        ui.monospace(&entry.source_pointer);
    }
    if ui.button("Copy source reference").clicked() {
        ui.ctx().copy_text(format!("{}#{}", entry.source_path, entry.source_pointer));
    }
    ui.add_space(16.0);
    let rows = [
        ("Owner", entry.owner.as_str()),
        ("Package", entry.package.as_deref().unwrap_or("Unassigned")),
        ("Classification", entry.classification_status.as_str()),
        ("Generated output", if entry.generation_status == "observed" { "Observed · freshness unverified" } else { "Not applicable" }),
        ("Persistent draft", if entry.persistent_draft { "Supported" } else { "Unavailable" }),
        ("Editing", if entry.editable { "Supported" } else { "Unavailable" }),
        ("Target renderer", if entry.renderer_qualified { "Qualified" } else { "Unqualified" }),
    ];
    if ui.available_width() < 540.0 {
        for (key, value) in rows {
            ui.label(RichText::new(key).small().color(theme.text_secondary));
            ui.label(value);
        }
    } else {
        egui::Grid::new(("catalog-support", &entry.id)).num_columns(2).spacing(egui::vec2(20.0, 10.0)).show(ui, |ui| {
            for (key, value) in rows {
                ui.label(RichText::new(key).color(theme.text_secondary));
                ui.label(value);
                ui.end_row();
            }
        });
    }
    if !entry.targets.is_empty() {
        ui.add_space(16.0);
        ui.strong("Declared targets");
        ui.horizontal_wrapped(|ui| {
            for target in &entry.targets {
                chip(ui, services, target);
            }
        });
    }
    if !entry.locales.is_empty() {
        ui.label(format!("Locales · {}", entry.locales.join(", ")));
    }
    ui.add_space(16.0);
    ui.collapsing(format!("Dependencies ({})", entry.dependencies.len()), |ui| {
        for dependency in &entry.dependencies {
            ui.label(dependency);
        }
    });
    if !entry.diagnostics.is_empty() {
        ui.collapsing(format!("Diagnostics ({})", entry.diagnostics.len()), |ui| {
            for diagnostic in &entry.diagnostics {
                ui.label(diagnostic["message"].as_str().unwrap_or("Source diagnostic"));
            }
        });
    }
    ui.collapsing("Provenance", |ui| {
        for (label, value) in [("Semantic ID", &entry.semantic_id), ("Observation", &entry.observation_id), ("Source digest", &entry.source_digest)] {
            ui.strong(label);
            ui.label(value);
        }
    });
}
