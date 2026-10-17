//! Presentation of captured capabilities. This module grants no owner authority.
use desigma_cityos_catalog::Entry;
use serde_json::{json, Value};

pub(super) struct Actions {
    pub foundation: bool,
    pub theme: bool,
    pub inspection_available: bool,
    pub source: Option<String>,
    pub actual_preview: bool,
    pub editable: bool,
    pub preview_reason: String,
    pub edit_reason: String,
    pub copy_reason: Option<String>,
    pub theme_reason: Option<String>,
    pub inline_pin: Option<Value>,
    pub preview_choices: Vec<Value>,
    pub preview_here: bool,
    pub workspace_reason: Option<String>,
}

pub(super) const LEVEL_OVERVIEW_TITLE: &str = "Source accounting by level";
pub(super) const LEVEL_EDITING_TOTALS: &str = "Per-level canvas and editing totals: Not aggregated";

impl Actions {
    pub(super) fn capability_rows(&self) -> Vec<(&'static str, &'static str, &str)> {
        let available = |value| if value { "Available" } else { "Unavailable" };
        if self.foundation {
            let mut rows = vec![
                ("Token sheet", available(self.inspection_available), "Captured values and swatches; a data-only representation."),
                ("Inspection canvas", available(self.inspection_available), "Temporary native inspection objects; no persistent document is created."),
                ("Local library", available(self.copy_reason.is_none()), self.copy_reason.as_deref().unwrap_or("Editable native variables and samples in an ordinary local document.")),
                ("Owner-bound draft", "Unavailable", "Owner theme editing requires a separately qualified Native owner. Local libraries can be edited and saved without that binding."),
            ];
            if self.theme {
                rows.push(("Web-frame theme preview", available(self.theme_reason.is_none()), self.theme_reason.as_deref().unwrap_or("Apply the captured canonical theme to the registered web-frame fixture as a preview scenario.")));
            }
            rows.push(("Renderer qualification", "Not established here", "A token sheet or local library does not establish target renderer qualification."));
            rows.push(("Owner writes", "Separate workflow", "Catalog observations grant no owner-write permission."));
            return rows;
        }
        vec![
            ("Actual preview", if self.preview_choices.is_empty() { available(self.actual_preview) } else { "Choose definition" }, &self.preview_reason),
            ("Structure canvas", available(self.source.is_some() && self.workspace_reason.is_none()), self.workspace_reason.as_deref().unwrap_or("A native inspection projection is separate from actual rendering and editing.")),
            ("Local copy", available(self.copy_reason.is_none()), self.copy_reason.as_deref().unwrap_or("Ordinary local design; no CityOS submission binding.")),
            ("Local draft", available(self.editable), &self.edit_reason),
            ("Renderer qualification", "Not established here", "Actual preview availability does not establish renderer qualification."),
            ("Owner writes", "Separate workflow", "Catalog observations grant no owner-write permission."),
        ]
    }
}

pub(super) fn matches(entry: &Entry, specimen: &Value) -> bool {
    specimen["entry"]["id"] == entry.id && specimen["digest"] == entry.source_digest
}

fn hex(value: &Value, length: usize) -> bool {
    value.as_str().is_some_and(|s| s.len() == length && s.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn profile_matches(profile: &Value, entry: &Entry, specimen: Option<&Value>) -> bool {
    if entry.kind == "nonvisual-dependency" { return false; }
    let registered = profile["kind"] == "registered-specimen";
    let data = specimen.map(|s| &s["data"]).unwrap_or(&Value::Null);
    let record = !matches!(entry.kind.as_str(), "source-file" | "public-declaration");
    if registered {
        for key in ["componentId", "componentVersion", "profileRef", "targetProfileRef"] {
            let Some(expected) = profile[key].as_str().filter(|s| !s.is_empty()) else { return false; };
            if profile.get("locator").is_some() && profile["locator"][key] != expected { return false; }
            let mut claims = vec![data.get(key), data["locator"].get(key), data["canvasLocator"].get(key)];
            if key == "componentId" {
                claims.push(data["component"].get("id"));
                if record { claims.push(data.get("id")); }
                if record && entry.semantic_id != expected { return false; }
            } else if key == "componentVersion" {
                claims.push(data.get("version")); claims.push(data["component"].get("version"));
            }
            if claims.into_iter().flatten().any(|value| value.as_str() != Some(expected)) { return false; }
        }
        if [data.get("publicExport"), data.get("exportName")].into_iter().flatten().any(|v| v != &profile["publicExport"]) { return false; }
    }
    // A bundled fixture never silently represents different committed bytes.
    if registered && (!hex(&profile["sourceDigest"],64)
        || profile["sourceDigest"] != profile["implementationDigest"]
        || (profile["sourcePath"] == entry.source_path && profile["sourceDigest"] != entry.source_digest)) { return false; }
    let Some(source) = profile["sourcePath"].as_str() else { return false; };
    if !registered && profile.get("sourceDigest").is_some() && source == entry.source_path
        && profile["sourceDigest"] != entry.source_digest { return false; }
    if let Some(export) = profile["publicExport"].as_str() {
        let named = data["name"] == export || data["publicExport"] == export || data["exportName"] == export
            || (entry.kind == "public-declaration" && entry.label == export)
            || (!registered && entry.label == export);
        let mut paths = vec![source];
        if registered { paths.extend(profile["sourcePaths"].as_array().into_iter().flatten().filter_map(Value::as_str)); }
        let declared = [data.get("sourcePath"), data.get("path"), data.get("file")];
        let declared_module = declared.iter().flatten().any(|value| paths.iter().any(|ref_path|
            value.as_str() == Some(*ref_path) || ref_path.split_once("/src/").is_some_and(|(_,relative)| value.as_str() == Some(relative))));
        if declared.iter().any(|v| v.is_some()) && !declared_module { return false; }
        let module = paths.contains(&entry.source_path.as_str()) || declared_module
            || (declared.iter().all(|v| v.is_none()) && paths.iter().any(|ref_path| entry.dependencies.iter().any(|d| d.as_str() == *ref_path)))
            || (!registered && declared.iter().all(|v| v.is_none()) && source.strip_prefix("packages/design-system/src/").is_some_and(|relative|
                entry.dependencies.iter().any(|dependency| dependency == relative)));
        let registered_id = registered && (record && entry.semantic_id == profile["componentId"].as_str().unwrap_or("")
            || [&data["componentId"], &data["component"]["id"], &data["locator"]["componentId"], &data["canvasLocator"]["componentId"]]
                .into_iter().any(|v| v == &profile["componentId"]));
        return module && (registered_id || (source == entry.source_path && (named || entry.kind == "source-file"))
            || (!registered && named));
    }
    source == entry.source_path && profile["kind"] == "composition"
}

pub(super) fn actions(entry: &Entry, specimen: Option<&Value>, summary: Option<&Value>, mode: &str) -> Actions {
    let captured = specimen;
    let specimen = specimen.filter(|s| matches(entry, s));
    let resolution = specimen.and_then(|s| s.get("workspaceSelection"));
    let mut candidates = Vec::new();
    for profile in summary.and_then(|s| s["workspaceProfiles"].as_array()).into_iter().flatten() {
        if profile_matches(profile, entry, specimen) && !candidates.contains(&profile) { candidates.push(profile); }
    }
    let ambiguous = resolution.is_some_and(|r| r["status"] == "ambiguous") || candidates.len() > 1;
    let unavailable = resolution.is_some_and(|r| r["status"] == "unavailable");
    let provided = specimen.and_then(|s| s.get("workspaceProfile")).filter(|p| !p.is_null());
    let profile = if ambiguous || unavailable { None }
        else if provided.is_some() { provided.filter(|p| profile_matches(p, entry, specimen)) }
        else { candidates.first().copied() };
    let selection_reason = if ambiguous {
        Some("Several registered definitions match this source. Select an exact component version, authoring profile and target, or its public export before opening.".to_owned())
    } else if unavailable || provided.is_some() && profile.is_none() {
        Some(resolution.and_then(|r| r["reason"].as_str()).unwrap_or("The captured selection does not match an installed component/version and renderer binding. Select an exact public export or locator.").to_owned())
    } else { None };
    // These two existing fixtures predate explicit capability fields. New adapters
    // must declare their capabilities; a composition alone is never a frame.
    let legacy_fixture = profile.is_some_and(|p| matches!(p["fixture"].as_str(), Some("cityos-web-frame" | "cityos-accordion")));
    let actual_preview = profile.is_some_and(|p| p["actualWebPreview"].as_bool().unwrap_or(legacy_fixture));
    let editable = profile.is_some_and(|p| p["editable"].as_bool().unwrap_or(legacy_fixture));
    let composition = entry.kind == "authored-composition"
        || specimen.is_some_and(|s| s["data"]["composition"]["schemaVersion"] == "cityos.experience-composition.v1");
    let blocked_selection = ambiguous || provided.is_some() && profile.is_none();
    let source = profile.and_then(|p| p["sourcePath"].as_str()).map(str::to_owned)
        .or_else(|| (composition && !blocked_selection).then(|| entry.source_path.clone()));
    let nonvisual = entry.kind == "nonvisual-dependency";
    let preview_reason = if nonvisual {
        "This code dependency has no standalone visual appearance. Inspect its consuming interface.".into()
    } else if let Some(reason) = selection_reason.as_ref().filter(|_| blocked_selection || !composition) {
        reason.clone()
    } else if actual_preview {
        if mode == "local" {
            "Actual preview opens corresponding canonical source. Local-overlay bytes remain a data preview.".into()
        } else {
            "Registered actual renderer fixture. Opening it verifies the selected source and package compatibility.".into()
        }
    } else if composition {
        "Authored structure is available. No actual renderer adapter is registered for this source.".into()
    } else if matches!(entry.kind.as_str(), "theme" | "token" | "design-token") {
        "Captured values and swatches are available after loading. They are data previews.".into()
    } else {
        "No actual renderer fixture is registered for this resource. Captured source and contract inspection remain available.".into()
    };
    let edit_reason = if let Some(reason) = &selection_reason {
        reason.clone()
    } else if editable && mode == "local" {
        "Open the corresponding canonical draft. Local-preview bytes are not adopted; submission requires its separate owner workflow.".into()
    } else if editable {
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
    let preview_choices = if ambiguous && mode == "committed" && snapshot["mode"] == "committed"
        && hex(&snapshot["canonicalCommit"], 40) {
        candidates.iter().filter(|p| p["kind"] == "registered-specimen" && p["actualWebPreview"] == true
            && hex(&p["sourceDigest"], 64)).map(|p| (**p).clone()).collect()
    } else { Vec::new() };
    // Both the inline preview and Design carry the full captured locator. A
    // registered default remains a temporary view until an explicit local copy.
    let preview_here = actual_preview && profile.is_some_and(|p| p["kind"] == "registered-specimen");
    let workspace_reason = preview_here.then(|| "Registered defaults are temporary. Open in Design to inspect them or create an editable local copy. A bound source draft requires canonical authoring input.".into());
    Actions { foundation: matches!(entry.kind.as_str(), "theme" | "token" | "design-token"), theme: entry.kind == "theme",
        inspection_available: specimen.is_some(), source, actual_preview, editable, preview_reason, edit_reason, copy_reason, theme_reason, inline_pin, preview_choices, preview_here, workspace_reason }
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
