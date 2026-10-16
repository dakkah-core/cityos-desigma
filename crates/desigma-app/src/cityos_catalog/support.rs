//! Presentation of captured capabilities. This module grants no owner authority.
use desigma_cityos_catalog::Entry;
use serde_json::{json, Value};

pub(super) struct Actions {
    pub source: Option<String>,
    pub actual_preview: bool,
    pub editable: bool,
    pub preview_reason: String,
    pub edit_reason: String,
    pub copy_reason: Option<String>,
    pub theme_reason: Option<String>,
    pub inline_pin: Option<Value>,
}

pub(super) fn matches(entry: &Entry, specimen: &Value) -> bool {
    specimen["entry"]["id"] == entry.id && specimen["digest"] == entry.source_digest
}

fn hex(value: &Value, length: usize) -> bool {
    value.as_str().is_some_and(|s| s.len() == length && s.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn profile_matches(profile: &Value, entry: &Entry, specimen: Option<&Value>) -> bool {
    if entry.kind == "nonvisual-dependency" { return false; }
    // A bundled fixture never silently represents different committed bytes.
    if profile["kind"] == "registered-specimen" && (!hex(&profile["sourceDigest"],64)
        || profile["sourceDigest"] != profile["implementationDigest"]
        || (profile["sourcePath"] == entry.source_path && profile["sourceDigest"] != entry.source_digest)) { return false; }
    let Some(source) = profile["sourcePath"].as_str() else { return false; };
    let data = specimen.map(|s| &s["data"]);
    if let Some(export) = profile["publicExport"].as_str() {
        let named = entry.label == export || data.is_some_and(|d| d["name"] == export);
        let module = source == entry.source_path
            || entry.dependencies.iter().any(|dependency| dependency == source)
            || (profile["kind"] == "registered-specimen" && profile["sourcePaths"].as_array().is_some_and(|paths| paths.iter().any(|ref_path| {
                ref_path.as_str().is_some_and(|ref_path| ref_path == entry.source_path || entry.dependencies.iter().any(|dependency| dependency == ref_path)
                    || data.is_some_and(|d| d["sourcePath"] == ref_path || ref_path.split_once("/src/").is_some_and(|(_,relative)| d["file"] == relative)))
            })))
            || source.strip_prefix("packages/design-system/src/").is_some_and(|relative|
                entry.dependencies.iter().any(|dependency| dependency == relative)
                    || data.is_some_and(|d| d["file"].as_str() == Some(relative)));
        return module && (named || (source == entry.source_path && entry.kind == "source-file"));
    }
    source == entry.source_path && profile["kind"] == "composition"
}

pub(super) fn actions(entry: &Entry, specimen: Option<&Value>, summary: Option<&Value>, mode: &str) -> Actions {
    let captured = specimen;
    let specimen = specimen.filter(|s| matches(entry, s));
    let profile = specimen.and_then(|s| s.get("workspaceProfile")).filter(|p| profile_matches(p, entry, specimen))
        .or_else(|| summary.and_then(|s| s["workspaceProfiles"].as_array())
            .and_then(|profiles| profiles.iter().find(|p| profile_matches(p, entry, specimen))));
    // These two existing fixtures predate explicit capability fields. New adapters
    // must declare their capabilities; a composition alone is never a frame.
    let legacy_fixture = profile.is_some_and(|p| matches!(p["fixture"].as_str(), Some("cityos-web-frame" | "cityos-accordion")));
    let actual_preview = profile.is_some_and(|p| p["actualWebPreview"].as_bool().unwrap_or(legacy_fixture));
    let editable = profile.is_some_and(|p| p["editable"].as_bool().unwrap_or(legacy_fixture));
    let composition = entry.kind == "authored-composition"
        || specimen.is_some_and(|s| s["data"]["composition"]["schemaVersion"] == "cityos.experience-composition.v1");
    let source = profile.and_then(|p| p["sourcePath"].as_str()).map(str::to_owned)
        .or_else(|| composition.then(|| entry.source_path.clone()));
    let nonvisual = entry.kind == "nonvisual-dependency";
    let preview_reason = if nonvisual {
        "This code dependency has no standalone visual appearance. Inspect its consuming interface.".into()
    } else if actual_preview {
        "Registered actual renderer fixture. Opening it verifies the selected source and package compatibility.".into()
    } else if composition {
        "Authored structure is available. No actual renderer adapter is registered for this source.".into()
    } else if matches!(entry.kind.as_str(), "theme" | "token" | "design-token") {
        "Captured values and swatches are available after loading. They are data previews.".into()
    } else {
        "No actual renderer fixture is registered for this resource. Captured source and contract inspection remain available.".into()
    };
    let edit_reason = if editable {
        "The registered adapter supports a local draft. Source submission requires its separate owner workflow.".into()
    } else {
        "No editable workspace adapter is registered for this source.".into()
    };
    let snapshot = summary.map(|s| &s["snapshot"]).unwrap_or(&Value::Null);
    let copy_reason = if mode != "committed" {
        Some("Switch to GitLab main before creating an editable local copy.".into())
    } else {
        super::local_library::unavailable_reason(entry, captured, snapshot)
    };
    let theme_reason = if entry.kind != "theme" {
        Some("Select a theme to use this workflow.".into())
    } else if mode != "committed" || snapshot["mode"] != "committed" {
        Some("Switch to GitLab main to preview the captured canonical theme on the web frame.".into())
    } else if captured.is_some() && specimen.is_none() {
        Some("The captured theme no longer matches this selection. Select it again before previewing.".into())
    } else if specimen.is_none() {
        Some("Load this theme before choosing its preview workflow.".into())
    } else if specimen.is_some_and(|s| s["data"]["metadata"]["selectable"] == false) {
        Some("This theme is marked as non-selectable by its source metadata.".into())
    } else {
        None
    };
    let inline_pin = profile.filter(|p| actual_preview && mode == "committed" && snapshot["mode"] == "committed"
        && hex(&p["sourceDigest"], 64) && hex(&snapshot["canonicalCommit"], 40))
        .map(|p| if p["kind"] == "registered-specimen" { p.clone() }
            else { json!({"sourcePath":p["sourcePath"],"sourceDigest":p["sourceDigest"],"commit":snapshot["canonicalCommit"]}) });
    Actions { source, actual_preview, editable, preview_reason, edit_reason, copy_reason, theme_reason, inline_pin }
}

pub(super) struct Level {
    pub id: String,
    pub name: String,
    pub count: Option<u64>,
    pub specimens: Option<u64>,
    pub renderers: Option<u64>,
    pub editable: Option<u64>,
}

pub(super) fn levels(summary: Option<&Value>) -> Vec<Level> {
    (0..=9).map(|n| {
        let id = format!("L{n}");
        let row = summary.and_then(|s| s["navigation"]["levels"].as_array())
            .and_then(|rows| rows.iter().find(|row| row["id"] == id));
        Level {
            id,
            name: row.and_then(|r| r["name"].as_str()).unwrap_or("Not reported in this snapshot").into(),
            count: row.and_then(|r| r["count"].as_u64()),
            specimens: row.and_then(|r| r["temporaryViewCount"].as_u64()),
            renderers: row.and_then(|r| r["rendererCount"].as_u64()),
            editable: row.and_then(|r| r["editableCount"].as_u64()),
        }
    }).collect()
}
