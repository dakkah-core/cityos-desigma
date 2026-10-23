//! Bound CityOS workspace: native projection and actual web renderer share one semantic draft.
use crate::editor::{BaselineInstall, BaselineRecovery, EditorHost, EditorSession};
use crate::workspace_support::{history_commands, presentation, submission_recovery};
use crate::{bridge, host, layout, projection as native_projection, repaint};
pub use bridge::Config;
use desigma_doc::{CityosOccurrence, Document};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

pub struct DraftTab<S> {
    pub session: Option<S>,
    pub resource: Option<Value>,
    pub locale: String,
    pub theme: String,
    pub theme_id: String,
    pub interactive: bool,
    pub selected: String,
}
#[derive(Clone, Copy, PartialEq)]
pub enum DependencyAction {
    Open,
    Resume,
    Reconcile,
    Accepted,
}

pub const SPECIMEN_CANVAS_REASON: &str = "Temporary registered defaults have no canonical document binding. Open on canvas and changes are unavailable.";

pub fn registered_availability_reason(resource: &Value) -> String {
    let reasons = resource["availability"]["preview"]["reasons"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|reason| match reason {
            "registered-fixture-props-unavailable" => "The component owner has not declared default fixture properties.",
            "owner-input-fixture-unavailable" => "This definition needs a declared owner-data fixture.",
            "registered-composition-inputs-required" => "Select an authored composition supplying its required slots.",
            "requires-registered-composition-root" => "Select the complete registered frame composition to inspect this member.",
            "renderer-adapter-unavailable" => "No actual-renderer adapter is registered for this definition.",
            "renderer-pins-unavailable" => "Install matching renderer dependencies to preview this definition.",
            "semantic-catalog-entry-unavailable" => "The registered semantic catalog entry is unavailable.",
            _ => reason,
        })
        .collect::<Vec<_>>();
    if reasons.is_empty() {
        "Select a canonical composition or an installed preview adapter for this definition.".into()
    } else {
        reasons.join(" ")
    }
}

#[derive(Clone)]
pub struct SpecimenRequest {
    pub locator: Value,
    pub scenario: Value,
    pub generation: u64,
}
impl SpecimenRequest {
    pub fn job(&self) -> Value {
        json!({"op":"FixturePreview","fixture":self.locator,"scenario":self.scenario})
    }
}

pub fn specimen_locator(resource: &Value) -> Option<Value> {
    if resource["kind"] != "registered-specimen"
        || resource["actualWebPreview"] != true
        || resource["editable"] != false
        || resource["provenance"]["canonicalResource"] != false
    {
        return None;
    }
    let mut locator = serde_json::Map::new();
    for key in ["componentId", "componentVersion", "profileRef", "targetProfileRef"] {
        let value = resource[key].as_str().filter(|s| !s.is_empty())?;
        locator.insert(key.into(), json!(value));
    }
    resource["fixture"].as_str().filter(|s| !s.is_empty())?;
    Some(Value::Object(locator))
}

pub fn semantic_change_label(change: &Value) -> String {
    let field = change.get("field").and_then(Value::as_str).filter(|s| !s.is_empty());
    let node = change.get("nodeId").and_then(Value::as_str).filter(|s| !s.is_empty());
    match (node, field) {
        (Some(node), Some(field)) => format!("{node}.{field}"),
        (_, Some(field)) => field.into(),
        _ => change.get("sourcePointer").and_then(Value::as_str).unwrap_or("Semantic change").into(),
    }
}

// Selection is view state. Resolve it against the new projection rather than
// carrying a semantic node name from an unrelated document into its inspector.
pub fn document_selection(document: &Document, requested: &str) -> String {
    let contains = |id: &str| document.nodes.values().any(|node| node.source_id.as_deref() == Some(id));
    if contains(requested) {
        return requested.into();
    }
    draft(document)
        .and_then(|value| value["semanticDraft"]["root"]["id"].as_str())
        .filter(|id| contains(id))
        .map(str::to_owned)
        .or_else(|| document.nodes.values().find_map(|node| node.source_id.clone()))
        .unwrap_or_default()
}

pub fn resource_picker(ui: &mut egui::Ui, resources: &[Value], active: Option<&Value>, search: &str, busy: bool) -> Option<Value> {
    let search = search.to_lowercase();
    let rows: Vec<_> = resources
        .iter()
        .filter(|resource| {
            search.is_empty()
                || format!(
                    "{} {} {} {} {}",
                    resource["sourcePath"], resource["label"], resource["componentId"], resource["profileRef"], resource["targetProfileRef"]
                )
                .to_lowercase()
                .contains(&search)
        })
        .collect();
    ui.small(format!("{} designs", rows.len()));
    let mut selected = None;
    egui::ScrollArea::vertical().id_salt(("cityos-design-files", &search)).auto_shrink([false, false]).show_rows(ui, 68., rows.len(), |ui, range| {
        for index in range {
            let resource = rows[index];
            let path = resource["sourcePath"].as_str().unwrap_or("");
            let label = resource["label"].as_str().unwrap_or(path);
            let is_specimen = resource["kind"] == "registered-specimen";
            let row_selected = active.is_some_and(|current| {
                if is_specimen {
                    specimen_locator(resource).is_some() && specimen_locator(current) == specimen_locator(resource)
                } else {
                    current["kind"] != "registered-specimen" && current["sourcePath"] == path
                }
            });
            let representation = if resource["actualWebPreview"] == true {
                "Actual web preview"
            } else if is_specimen {
                "Preview unavailable"
            } else {
                "Authored structure"
            };
            let scope = if is_specimen { "Temporary defaults" } else { "Canonical file" };
            let detail = ["level", "target", "componentVersion"]
                .into_iter()
                .filter_map(|key| resource[key].as_str())
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
                .join(" · ");
            let detail = if detail.is_empty() { "Repository design".to_owned() } else { detail };
            let hint = format!(
                "{}\n{}\n{}",
                path,
                resource["componentId"].as_str().unwrap_or(""),
                resource["previewNotice"].as_str().unwrap_or("Select to preview before opening on canvas.")
            );
            ui.add_enabled_ui(!busy && resource["openAvailable"] != false, |ui| {
                ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), 68.), egui::Layout::top_down(egui::Align::Min), |ui| {
                    ui.spacing_mut().item_spacing.y = 1.;
                    let response =
                        ui.add_sized([ui.available_width(), 30.], egui::Button::new(label).selected(row_selected).wrap_mode(egui::TextWrapMode::Truncate));
                    if response.on_hover_text(&hint).on_disabled_hover_text(&hint).clicked() {
                        selected = Some(resource.clone());
                    }
                    ui.add(egui::Label::new(egui::RichText::new(detail).small()).truncate());
                    ui.add(egui::Label::new(egui::RichText::new(format!("{representation} · {scope}")).small()).truncate());
                });
            });
        }
    });
    selected
}

pub fn semantic_change_value_text(value: Option<&Value>, document: Option<&Value>) -> String {
    let Some(value) = value else {
        return "Unavailable (value not provided)".into();
    };
    let reference = value.get("resource").is_some() && (value.get("nodeId").is_some() || value.get("pointer").is_some());
    let resolved = if reference {
        let resolved = document.and_then(|document| {
            let resource = value.get("resource")?;
            let id = resource.get("id")?.as_str()?;
            let revision = resource.get("revision")?.as_str()?;
            if document.get("id")?.as_str()? != id || document.get("revision")?.as_str()? != revision {
                return None;
            }
            document.pointer(value.get("pointer")?.as_str()?)
        });
        let Some(resolved) = resolved else {
            return format!("Unavailable (unresolved reference: {value})");
        };
        resolved
    } else {
        value
    };
    resolved.as_str().map(str::to_owned).unwrap_or_else(|| resolved.to_string())
}

macro_rules! workspace_type { ($visibility:vis) => {
pub struct Workspace<H: EditorHost> {
    pub open: bool,
    $visibility config: Option<Config>,
    $visibility bridge: Option<bridge::Bridge>,
    $visibility host: host::Host,
    $visibility busy: Option<String>,
    $visibility staged: Option<Document>,
    $visibility preview_only: bool,
    $visibility error: Option<String>,
    $visibility preview: Option<Value>,
    $visibility current: bool,
    $visibility render_started: Option<Instant>,
    $visibility locale: String,
    $visibility theme: String,
    $visibility theme_id: String,
    $visibility themes: Vec<Value>,
    $visibility themes_loaded: bool,
    $visibility selected: String,
    $visibility observed: Option<(String, u64)>,
    $visibility pending: Option<Instant>,
    $visibility compare: Option<String>,
    $visibility compare_open: bool,
    $visibility validated: bool,
    $visibility harness: bool,
    $visibility viewport: (u32, u32),
    $visibility panel_size: (u32, u32),
    $visibility resources: Vec<Value>,
    $visibility resource: Option<Value>,
    $visibility resource_diagnostics: Vec<Value>,
    $visibility picker_open: bool,
    $visibility resource_search: String,
    $visibility coverage: crate::workspace_support::coverage::Coverage,
    $visibility requested_source: Option<String>,
    $visibility open_after_preview: bool,
    $visibility temporary_canvas: Option<H::Canvas>,
    $visibility tabs:BTreeMap<String,DraftTab<H::Session>>,
    $visibility changes_mode:bool,
    $visibility incoming:Option<Value>,
    $visibility inline_identity:Option<String>,
    $visibility requested_pin:Option<Value>,
    $visibility layout: layout::Layout,
    $visibility layout_loaded: bool,
    $visibility narrow_preview: bool,
    $visibility leave_to_library: bool,
    $visibility dependency_action: Option<DependencyAction>,
    $visibility dependency_draft: Option<Value>,
    $visibility reconciliation_bundle: Option<Value>,
    $visibility resolutions: BTreeMap<String, String>,
    $visibility recovery_pending: bool,
    $visibility interactive: bool,
    $visibility property_text: BTreeMap<String,String>,
    $visibility resume_document: Option<Document>,
    $visibility specimen_request: Option<SpecimenRequest>,
    $visibility local_specimen_request: Option<SpecimenRequest>,
    $visibility preview_generation: u64,
    $visibility submissions: submission_recovery::Recovery,
    $visibility accepted_bundle: Option<Value>,
    $visibility sent_presentation: Option<Value>,
    $visibility projection_attempted: Option<Value>,
    $visibility canvas_presentation: Option<Value>,
    $visibility resume_presentation: Option<Value>,
    $visibility native_fit: H::CanvasState,
    $visibility baseline_recovery: Option<BaselineRecovery>,
    $visibility history_command: Option<history_commands::PendingHistory>,
}
}; }
#[cfg(feature = "test-support")]
workspace_type!(pub);
#[cfg(not(feature = "test-support"))]
workspace_type!(pub(crate));

impl<H: EditorHost> Default for Workspace<H> {
    fn default() -> Self {
        Self {
            open: false,
            config: None,
            bridge: None,
            host: Default::default(),
            busy: None,
            staged: None,
            preview_only: false,
            error: None,
            preview: None,
            current: false,
            render_started: None,
            locale: "ar".into(),
            theme: "light".into(),
            theme_id: String::new(),
            themes: vec![],
            themes_loaded: false,
            selected: "material".into(),
            observed: None,
            pending: None,
            compare: None,
            compare_open: false,
            validated: false,
            harness: false,
            viewport: (560, 720),
            panel_size: (0, 0),
            resources: vec![],
            resource: None,
            resource_diagnostics: vec![],
            picker_open: false,
            resource_search: String::new(),
            coverage: Default::default(),
            requested_source: None,
            open_after_preview: false,
            temporary_canvas: None,
            tabs: BTreeMap::new(),
            changes_mode: false,
            incoming: None,
            inline_identity: None,
            requested_pin: None,
            layout: Default::default(),
            layout_loaded: false,
            narrow_preview: false,
            leave_to_library: true,
            dependency_action: None,
            dependency_draft: None,
            reconciliation_bundle: None,
            resolutions: BTreeMap::new(),
            recovery_pending: false,
            interactive: false,
            property_text: BTreeMap::new(),
            resume_document: None,
            specimen_request: None,
            local_specimen_request: None,
            preview_generation: 0,
            submissions: Default::default(),
            accepted_bundle: None,
            sent_presentation: None,
            projection_attempted: None,
            canvas_presentation: None,
            resume_presentation: None,
            native_fit: Default::default(),
            baseline_recovery: None,
            history_command: None,
        }
    }
}

pub fn projection_document(projection: &Value, name: &str) -> Result<(Document, BTreeMap<String, CityosOccurrence>), String> {
    native_projection::projection_document(projection, name)
}
use crate::draft::{binding, draft, local_copy, revision};

impl<H: EditorHost> Workspace<H> {
    pub fn coverage_open(&self) -> bool {
        self.coverage.open
    }
    /// Catalog accounting is a separate observation; it cannot modify a draft or capabilities.
    pub fn set_catalog_coverage(&mut self, observation: Option<crate::catalog::CoverageObservation>) {
        self.coverage.catalog = observation;
    }
    pub fn set_preview_theme(&mut self, id: String) {
        self.theme_id = if self.is_specimen() { String::new() } else { id };
        self.current = false;
        self.pending = Some(Instant::now());
    }
    pub fn begin(&mut self, config: Config, source: Option<String>, preview_only: bool) {
        self.begin_request(config, source, preview_only, None);
    }
    pub fn begin_pinned_preview(&mut self, config: Config, resource: Value) {
        let source = resource["sourcePath"].as_str().map(str::to_owned);
        self.begin_request(config, source, true, Some(resource));
    }
    pub fn begin_request(&mut self, config: Config, source: Option<String>, preview_only: bool, pin: Option<Value>) {
        if self.recovery_pending {
            self.error = Some("Recover the pending document generation before opening another workspace.".into());
            return;
        }
        self.host.configure_storage(&config.cache);
        self.invalidate_specimen_request();
        self.resume_document = None;
        self.sent_presentation = None;
        self.history_command = None;
        self.projection_attempted = None;
        self.canvas_presentation = None;
        self.resume_presentation = None;
        self.property_text.clear();
        self.interactive = false;
        self.dependency_action = None;
        self.dependency_draft = None;
        self.reconciliation_bundle = None;
        self.resolutions.clear();
        self.leave_to_library = true;
        self.load_layout(&config);
        self.inline_identity = None;
        self.requested_pin = None;
        self.changes_mode = false;
        self.host.reset();
        self.bridge = Some(bridge::Bridge::spawn(config.clone()));
        self.config = Some(config);
        self.open = true;
        self.preview_only = true;
        self.open_after_preview = !preview_only;
        self.requested_source = source;
        self.picker_open = self.requested_source.is_none();
        self.resources.clear();
        self.coverage = Default::default();
        self.resource = None;
        self.resource_diagnostics.clear();
        self.temporary_canvas = None;
        self.staged = None;
        self.observed = None;
        self.preview = None;
        self.error = None;
        self.validated = false;
        self.harness = false;
        self.themes.clear();
        self.themes_loaded = false;
        self.busy = None;
        self.pending = None;
        self.current = false;
        self.render_started = None;
        self.compare = None;
        self.compare_open = false;
        if let Some(pin) = pin {
            self.requested_source = None;
            if pin["kind"] == "registered-specimen" {
                self.open_resource(pin);
            } else {
                self.send(json!({"op":"Open","resource":pin}));
            }
        } else {
            self.send(json!({"op":"Resources"}));
        }
    }
    pub fn open_resource(&mut self, resource: Value) {
        if self.busy.is_some() {
            return;
        }
        self.prepare_resource(resource.clone());
        if self.is_specimen() {
            self.open_after_preview = false;
            self.theme_id.clear();
            self.themes_loaded = true;
            self.validated = specimen_locator(&resource).is_some();
            if self.validated {
                self.request_specimen_preview();
            } else {
                self.error = Some(registered_availability_reason(&resource));
            }
        } else {
            self.send(json!({"op":"Open","resource":resource}));
        }
    }
    pub fn invalidate_specimen_request(&mut self) {
        self.preview_generation = self.preview_generation.wrapping_add(1);
        self.specimen_request = None;
        self.local_specimen_request = None;
    }
    pub fn is_specimen(&self) -> bool {
        self.resource.as_ref().is_some_and(|r| r["kind"] == "registered-specimen")
    }
    pub fn prepare_resource(&mut self, resource: Value) {
        self.invalidate_specimen_request();
        self.harness = false;
        self.error = None;
        self.property_text.clear();
        self.dependency_action = None;
        self.dependency_draft = None;
        self.reconciliation_bundle = None;
        self.resume_document = None;
        self.history_command = None;
        self.projection_attempted = None;
        self.canvas_presentation = None;
        self.resume_presentation = None;
        self.changes_mode = false;
        self.incoming = None;
        self.host.reset();
        self.preview_only = true;
        self.validated = false;
        self.current = false;
        self.pending = None;
        self.render_started = None;
        self.observed = None;
        self.preview = None;
        self.staged = None;
        self.temporary_canvas = None;
        self.compare = None;
        self.compare_open = false;
        self.resource = Some(resource.clone());
        self.themes.clear();
        self.themes_loaded = false;
    }
    #[expect(clippy::too_many_arguments, reason = "Preserve the existing native host presentation contract during extension extraction")]
    pub fn show_inline(&mut self, app: &mut H, frame: &eframe::Frame, config: Config, pin: Value, rect: egui::Rect, scale: f32, ctx: &egui::Context) -> String {
        let key = serde_json::to_string(&pin).unwrap_or_default();
        if self.inline_identity.as_deref() != Some(&key) {
            let source = pin["sourcePath"].as_str().map(str::to_owned);
            if source.is_none() && pin["kind"] != "registered-specimen" {
                return "Preview source unavailable".into();
            }
            self.begin_request(config, source, true, Some(pin));
            self.open = false;
            self.inline_identity = Some(key);
        }
        self.poll(app);
        if let Some(error) = &self.error {
            self.host.hide();
            return error.clone();
        }
        if let Some(url) = self.preview.as_ref().and_then(|p| p["url"].as_str()) {
            if let Err(error) = self.host.show(frame, url, rect, scale, ctx) {
                self.render_started = None;
                self.pending = None;
                self.error = Some(error.clone());
                return error;
            }
        }
        if self.current {
            format!("Current · revision {}", self.preview.as_ref().map(|p| p["draftRevision"].clone()).unwrap_or(Value::Null))
        } else if let Some(operation) = &self.busy {
            format!("{operation}…")
        } else {
            "Preparing actual preview…".into()
        }
    }
    pub fn inline_pending(&self) -> bool {
        self.error.is_none() && (self.busy.is_some() || self.pending.is_some() || self.render_started.is_some())
    }
    pub fn actual_preview(&self) -> bool {
        self.harness || self.resource.as_ref().is_some_and(|r| r["actualWebPreview"] == true)
    }
    pub fn save_before_switch(app: &mut H) -> Result<(), String> {
        app.save_before_switch()
    }
    pub fn open_desigma_files(&mut self, app: &mut H) -> Result<(), String> {
        if self.recovery_pending {
            return Err("Recover the pending document generation before leaving this draft.".into());
        }
        if self.busy.is_some() {
            return Err("Wait for the current CityOS operation before opening Desigma files.".into());
        }
        if !app.status().home {
            if !app.status().library_available {
                return Err("The Desigma file library is unavailable. Save this draft before leaving.".into());
            }
            Self::save_before_switch(app)?;
            app.open_files()?;
        }
        self.leave_to_library = false;
        self.open = false;
        self.host.release();
        self.bridge = None;
        self.invalidate_specimen_request();
        self.preview = None;
        self.temporary_canvas = None;
        self.staged = None;
        self.pending = None;
        self.render_started = None;
        self.current = false;
        self.inline_identity = None;
        self.error = None;
        Ok(())
    }
    pub fn activate_resumed_document(app: &mut H, document: Document) -> Result<(), String> {
        app.install_presentation(document)?;
        app.enter_document();
        Ok(())
    }
    pub fn install_resumed_projection(&mut self, app: &mut H, document: Document) -> bool {
        if !self.require_dependencies(app, &document, DependencyAction::Resume) {
            self.resume_document = Some(document);
            self.validated = false;
            return false;
        }
        if let Err(error) = Self::activate_resumed_document(app, document) {
            self.error = Some(error);
            self.validated = false;
            return false;
        }
        true
    }
    pub fn save_to_draft_library(app: &mut H) -> Result<(), String> {
        app.save_to_draft_library()
    }
    pub fn prepare_switch(&self, app: &mut H, replacing_tab: bool) -> Result<(), String> {
        if self.recovery_pending || self.busy.is_some() || app.status().history_pending {
            return Err("Finish the pending CityOS operation before switching drafts.".into());
        }
        let id = draft(app.document()).and_then(|d| d["draftId"].as_str()).map(str::to_owned);
        if id.is_some() && self.tabs.len().saturating_sub(usize::from(replacing_tab)) >= 3 {
            return Err("Four drafts are already loaded. Save and close an inactive tab before opening another.".into());
        }
        Self::save_before_switch(app)
    }
    pub fn retained_tab(&self, session: H::Session) -> DraftTab<H::Session> {
        DraftTab {
            session: Some(session),
            resource: self.resource.clone(),
            locale: self.locale.clone(),
            theme: self.theme.clone(),
            theme_id: self.theme_id.clone(),
            interactive: self.interactive,
            selected: self.selected.clone(),
        }
    }
    pub fn replace_active(&mut self, app: &mut H, document: Document, path: Option<std::path::PathBuf>) -> Result<(), String> {
        self.prepare_switch(app, false)?;
        let id = draft(app.document()).and_then(|d| d["draftId"].as_str()).map(str::to_owned);
        let previous = app.replace_session(document, path)?;
        if let Some(id) = id {
            self.tabs.insert(id, self.retained_tab(previous));
        }
        Ok(())
    }
    pub fn activate_tab(&mut self, app: &mut H, id: &str) {
        if !self.tabs.contains_key(id) {
            return;
        }
        if let Err(error) = self.prepare_switch(app, true) {
            self.error = Some(error);
            return;
        }
        self.activate_prepared_tab(app, id);
    }
    pub fn activate_prepared_tab(&mut self, app: &mut H, id: &str) {
        let previous_id = draft(app.document()).and_then(|d| d["draftId"].as_str()).map(str::to_owned);
        let previous = app.swap_session(&mut self.tabs.get_mut(id).unwrap().session);
        let previous = match previous {
            Ok(session) => session,
            Err(error) => {
                self.error = Some(error);
                return;
            }
        };
        let tab = self.tabs.remove(id).expect("Incoming tab is removed only after its successful transfer");
        if let Some(previous_id) = previous_id {
            self.tabs.insert(previous_id, self.retained_tab(previous));
        }
        self.resource = tab.resource;
        self.locale = tab.locale;
        self.theme = tab.theme;
        self.theme_id = tab.theme_id;
        self.interactive = tab.interactive;
        let selected = document_selection(app.document(), &tab.selected);
        self.select(app, &selected);
        self.changes_mode = false;
        self.incoming = None;
        if let Some(config) = self.config.clone() {
            self.resume(config, app);
        }
    }
    pub fn install_staged(&mut self, app: &mut H) {
        if self.is_specimen() {
            self.error = Some(SPECIMEN_CANVAS_REASON.into());
            return;
        }
        if self.staged.is_none() {
            return;
        }
        if self.resource.as_ref().is_some_and(|r| r["references"].as_u64().unwrap_or(0) > 0 && r["referenceClosure"]["complete"] != true) {
            self.error = Some("Reusable dependencies are unresolved. This file remains a temporary structure preview.".into());
            return;
        }
        let candidate = self.staged.as_ref().unwrap().clone();
        if !self.require_dependencies(app, &candidate, DependencyAction::Open) {
            return;
        }
        // Previewing another file never replaces the editor document. An explicit
        // open must preserve the current document, including failed saves.
        let doc = self.staged.as_ref().unwrap().clone();
        if let Err(error) = self.replace_active(app, doc, None) {
            self.error = Some(error);
            return;
        }
        self.staged = None;
        let selected = document_selection(app.document(), "");
        self.select(app, &selected);
        app.enter_document();
        self.preview_only = false;
        self.changes_mode = false;
        self.incoming = None;
        self.temporary_canvas = None;
        self.observed = None;
        self.error = None;
        if let Err(error) = Self::save_before_switch(app) {
            self.error = Some(error);
        }
    }
    pub fn create_local_copy(&mut self, app: &mut H) {
        if self.is_specimen() {
            if self.busy.is_some() || !self.validated {
                return;
            }
            let Some(request) = self.captured_specimen_request() else {
                return;
            };
            self.send(json!({"op":"Fixtures","fixture":request.locator,"locale":self.locale}));
            if self.busy.as_deref() == Some("Fixtures") {
                self.local_specimen_request = Some(request);
            }
            return;
        }
        let source = if self.preview_only { self.staged.as_ref() } else { Some(app.document()) };
        let Some(document) = source.map(local_copy) else {
            return;
        };
        self.open_local_document(app, document);
    }
    pub fn open_local_document(&mut self, app: &mut H, document: Document) {
        if let Err(error) = self.replace_active(app, document, None) {
            self.error = Some(error);
            return;
        }
        app.enter_document();
        self.host.reset();
        self.bridge = None;
        self.leave_to_library = false;
        self.open = false;
        app.toast("Local Desigma copy · no CityOS submission binding", false);
    }
    pub fn local_specimen_document(&mut self, value: Value) -> Result<Document, String> {
        let request = self.local_specimen_request.take().ok_or("No explicit local-copy request is pending")?;
        let selected = self.resource.as_ref().ok_or("Selected registered definition unavailable")?;
        let resource = &value["resource"];
        if !self.preview_only
            || !self.is_specimen()
            || request.generation != self.preview_generation
            || specimen_locator(selected).as_ref() != Some(&request.locator)
            || specimen_locator(resource).as_ref() != Some(&request.locator)
            || request.scenario["locale"] != self.locale
            || value["locale"] != self.locale
            || value["state"] != "projected"
            || value["ownerWrites"] != false
            || value["canonicalResource"] != false
            || value.get("draft").is_some()
            || value["provenance"]["temporary"] != true
            || value["provenance"]["canonicalResource"] != false
            || value["provenance"]["sourceCommit"] != selected["provenance"]["sourceCommit"]
            || value["provenance"]["locale"] != self.locale
            || resource["provenance"] != value["provenance"]
            || value["draftRevision"] != 0
            || value["draftId"] != format!("registered-fixture:{}", resource["resourceId"].as_str().unwrap_or(""))
            || value["projection"]["schemaVersion"] != "cityos.desigma-canvas.v1"
            || value["projection"]["draftId"] != value["draftId"]
            || value["projection"]["draftRevision"] != 0
            || value["projection"]["semanticDigest"] != value["semanticDigest"]
            || value["projection"]["nodes"].as_array().is_none_or(|nodes| nodes.is_empty())
        {
            return Err("Obsolete or substituted registered projection discarded; the current document was preserved".into());
        }
        let name = format!("{} · inspection", selected["label"].as_str().unwrap_or("Registered default"));
        let (document, _) = projection_document(&value["projection"], &name)?;
        Ok(local_copy(&document))
    }
    pub fn begin_harness(&mut self, config: Config) {
        if self.recovery_pending {
            self.error = Some("Recover the pending document generation before opening the preview diagnostic.".into());
            return;
        }
        self.host.configure_storage(&config.cache);
        self.coverage = Default::default();
        self.invalidate_specimen_request();
        self.resource = None;
        self.host.reset();
        self.bridge = Some(bridge::Bridge::spawn(config.clone()));
        self.config = Some(config);
        self.busy = None;
        self.open = true;
        self.preview_only = true;
        self.harness = true;
        self.validated = false;
        self.pending = None;
        self.render_started = None;
        self.observed = None;
        self.preview = None;
        self.staged = None;
        self.error = None;
        self.send(json!({"op":"Harness"}));
    }
    pub fn resume(&mut self, config: Config, app: &H) {
        if self.recovery_pending {
            self.error = Some("Recover the pending document generation before resuming a workspace.".into());
            return;
        }
        self.selected = document_selection(app.document(), &self.selected);
        self.coverage = Default::default();
        self.host.configure_storage(&config.cache);
        self.invalidate_specimen_request();
        self.resume_document = None;
        self.sent_presentation = None;
        self.history_command = None;
        self.projection_attempted = None;
        self.canvas_presentation = None;
        self.resume_presentation = None;
        self.property_text.clear();
        self.dependency_action = None;
        self.dependency_draft = None;
        self.reconciliation_bundle = None;
        self.resolutions.clear();
        self.leave_to_library = true;
        self.load_layout(&config);
        self.inline_identity = None;
        self.requested_pin = None;
        self.host.reset();
        self.config = Some(config);
        self.bridge = None;
        self.busy = None;
        self.open = true;
        self.preview_only = false;
        self.themes.clear();
        self.themes_loaded = false;
        self.preview = None;
        self.observed = None;
        self.validated = false;
        self.current = false;
        self.render_started = None;
        self.pending = None;
        self.error = None;
        self.compare = None;
        self.compare_open = false;
        self.resource = None;
        self.temporary_canvas = None;
        self.picker_open = false;
        self.harness = false;
        if let Some(draft) = draft(app.document()) {
            self.send(json!({"op":"Project","draft":draft}));
        }
    }
    pub fn hide(&self) {
        self.host.hide();
    }
    pub fn library_requested(&self) -> bool {
        self.leave_to_library
    }
    pub fn require_dependencies(&mut self, app: &H, document: &Document, action: DependencyAction) -> bool {
        match app.dependencies_complete(document, action == DependencyAction::Resume) {
            Ok(true) => return true,
            Err(error) => {
                self.error = Some(error);
                return false;
            }
            Ok(false) => {}
        }
        let Some(draft) = draft(document).cloned() else {
            self.error = Some("Missing bound draft".into());
            return false;
        };
        self.dependency_action = Some(action);
        self.dependency_draft = Some(draft.clone());
        self.send(json!({"op":"Dependencies","draft":draft}));
        false
    }
    pub fn install_reconciliation(&mut self, app: &mut H, value: Value) {
        let install = (|| -> Result<(), String> {
            if value["historyBoundary"] != true {
                return Err("Reconciliation did not establish a validated baseline boundary".into());
            }
            let mut document = binding(value["draft"].clone(), &value["projection"])?;
            document.name = app.document().name.clone();
            let previous = revision(app.document()).ok_or("Current draft is unavailable")?;
            let next = revision(&document).ok_or("Incoming draft is unavailable")?;
            if previous.0 != next.0 || next.1 != previous.1 + 1 {
                return Err("Obsolete reconciliation discarded".into());
            }
            if !self.require_dependencies(app, &document, DependencyAction::Reconcile) {
                self.reconciliation_bundle = Some(value.clone());
                return Ok(());
            }
            match app.install_reviewed_baseline(document)? {
                BaselineInstall::Installed => {}
                BaselineInstall::Uncertain { recovery, message } => {
                    self.baseline_recovery = Some(recovery);
                    self.recovery_pending = true;
                    self.validated = false;
                    self.reconciliation_bundle = Some(value.clone());
                    return Err(message);
                }
            }
            self.resource = Some(value["resource"].clone());
            self.incoming = None;
            self.resolutions.clear();
            self.compare = None;
            self.current = false;
            self.observed = None;
            self.validated = true;
            self.pending = Some(Instant::now());
            self.recovery_pending = false;
            self.reconciliation_bundle = None;
            app.toast("Reviewed incoming changes saved. Earlier baseline history is retained for inspection.", false);
            Ok(())
        })();
        if let Err(error) = install {
            self.error = Some(error);
            self.bridge = None;
            self.busy = None;
        }
    }
    pub fn recover_reconciliation(&mut self, app: &mut H) {
        let result = (|| -> Result<(), String> {
            let expected = self.reconciliation_bundle.as_ref().ok_or("Missing recovery bundle")?;
            let identity = self.baseline_recovery.as_ref().ok_or("Missing durable recovery identity")?;
            if app.recover_reviewed_baseline(identity)? {
                self.resource = Some(expected["resource"].clone());
                self.incoming = None;
            }
            self.recovery_pending = false;
            self.baseline_recovery = None;
            self.reconciliation_bundle = None;
            self.validated = true;
            self.current = false;
            self.observed = None;
            self.pending = Some(Instant::now());
            self.error = None;
            Ok(())
        })();
        if let Err(error) = result {
            self.error = Some(error);
        }
    }
    pub fn recover_accepted(&mut self, app: &mut H) {
        if self.busy.is_some() {
            return;
        }
        let Some(record) = self.submissions.accepted().cloned() else {
            return;
        };
        self.submissions.requested = Some(record.command.operation_id.clone());
        match self.submissions.recover_existing(app, &record) {
            Ok(true) => {
                self.accepted_bundle = None;
                self.recovery_pending = false;
                self.validated = false;
                self.current = false;
                self.observed = None;
                self.error = None;
                if let Some(draft) = draft(app.document()) {
                    self.send(json!({"op":"Project","draft":draft}));
                }
            }
            Ok(false) => {
                if let Some(draft) = self.active_draft(app) {
                    self.send(json!({"op":"AcceptedProjection","draft":draft,"submission":record}));
                }
            }
            Err(error) => {
                self.error = Some(error);
                self.recovery_pending = true;
            }
        }
    }
    pub fn install_accepted(&mut self, app: &mut H, value: Value) {
        let result = (|| -> Result<(), String> {
            let record = self.submissions.accepted().cloned().ok_or("Original accepted submission is unavailable")?;
            if value["operationId"] != record.command.operation_id
                || value["requestDigest"] != record.request_digest
                || value["authority"] != "projection-only"
                || value["ownerWrites"] != false
                || value["historyBoundary"] != true
                || value["operationCount"] != 0
            {
                return Err("Accepted projection does not match the retained original operation".into());
            }
            let mut document = binding(value["draft"].clone(), &value["projection"])?;
            document.name = app.document().name.clone();
            let previous = revision(app.document()).ok_or("Original submitted draft unavailable")?;
            let next = revision(&document).ok_or("Accepted draft unavailable")?;
            if previous.0 != next.0 || next.1 != previous.1 + 1 {
                return Err("Obsolete accepted projection discarded".into());
            }
            self.accepted_bundle = Some(value.clone());
            if !self.require_dependencies(app, &document, DependencyAction::Accepted) {
                return Ok(());
            }
            self.submissions.install(app, &record, document)?;
            self.accepted_bundle = None;
            self.recovery_pending = false;
            self.validated = true;
            self.resource = Some(value["resource"].clone());
            self.incoming = None;
            self.resolutions.clear();
            self.compare = None;
            self.current = false;
            self.observed = None;
            self.pending = Some(Instant::now());
            self.error = None;
            app.toast("Accepted owner state synchronized and saved. Earlier baseline history is retained for inspection.", false);
            Ok(())
        })();
        if let Err(error) = result {
            self.error = Some(error);
            self.recovery_pending = true;
            self.validated = false;
            self.bridge = None;
            self.busy = None;
        }
    }
    pub fn load_layout(&mut self, config: &Config) {
        if self.layout_loaded {
            return;
        }
        match layout::Layout::load(&config.cache) {
            Ok(value) => self.layout = value,
            Err(error) => self.error = Some(error),
        }
        self.layout_loaded = true;
    }
    pub fn save_layout(&mut self) {
        if let Some(config) = &self.config {
            if let Err(error) = self.layout.save(&config.cache) {
                self.error = Some(error);
            }
        }
    }
    pub fn send(&mut self, mut value: Value) {
        if self.busy.is_some() {
            return;
        }
        // An uncertain reviewed baseline cannot start another workflow. Original
        // accepted-submission readback must remain available for its recovery.
        let recovery_read = value["op"] == "ReleaseDependencies"
            || self.baseline_recovery.is_none()
                && self.submissions.accepted().is_some()
                && matches!(value["op"].as_str(), Some("AcceptedProjection" | "Dependencies"));
        if self.recovery_pending && !recovery_read {
            self.error = Some("Recover the pending document generation before requesting more workspace operations.".into());
            return;
        }
        if (self.submissions.error.is_some() || self.submissions.records.iter().any(desigma_library::SubmissionRecord::frozen))
            && matches!(value["op"].as_str(), Some("Command" | "Reconcile"))
        {
            self.error = Some("The immutable submitted draft stays frozen until its original owner outcome is resolved.".into());
            return;
        }
        if self.is_specimen() && !matches!(value["op"].as_str(), Some("FixturePreview" | "Fixtures" | "Resources" | "SemanticCoverage")) {
            self.error = Some(SPECIMEN_CANVAS_REASON.into());
            return;
        }
        if self.bridge.is_none() {
            if let Some(config) = &self.config {
                self.bridge = Some(bridge::Bridge::spawn(config.clone()));
            }
        }
        let Some(bridge) = &self.bridge else {
            self.error = Some("Choose a canonical design file to configure the bridge".into());
            return;
        };
        if matches!(value["op"].as_str(), Some("Open" | "Command" | "Project")) {
            let presentation = self.presentation();
            value["presentation"] = presentation.clone();
            self.sent_presentation = Some(presentation.clone());
            if value["op"] == "Project" {
                self.projection_attempted = Some(presentation::key(&value["draft"], &presentation));
            }
        }
        self.busy = value["op"].as_str().map(str::to_owned);
        if bridge.jobs.try_send(value).is_err() {
            self.busy = None;
            self.error = Some("Exchange helper unavailable".into());
        }
    }
    pub fn active_draft(&self, app: &H) -> Option<Value> {
        if self.is_specimen() {
            return None;
        }
        if self.preview_only {
            self.staged.as_ref().and_then(draft).cloned()
        } else {
            draft(app.document()).cloned()
        }
    }
    pub fn send_property_command(&mut self, app: &H, field: &Value, value: Value) {
        if !self.validated || app.status().frozen || self.busy.is_some() || self.selected != field["nodeId"].as_str().unwrap_or("") {
            return;
        }
        let Some(draft) = self.active_draft(app) else {
            return;
        };
        // Values and operations remain subject to the trusted profile/compiler.
        // The inspector only sends intent; it never installs local semantics.
        self.send(json!({"op":"Command","command":{"draftId":draft["draftId"],
            "expectedRevision":draft["draftRevision"],"operationId":uuid::Uuid::new_v4().to_string(),
            "operation":"set-property","nodeId":field["nodeId"],"property":field["property"],"value":value},"draft":draft}));
    }
    pub fn request_preview(&mut self, app: &H) {
        if self.harness {
            self.send(json!({"op":"Harness"}));
            return;
        }
        if self.is_specimen() {
            self.request_specimen_preview();
            return;
        }
        if let Some(draft) = self.active_draft(app) {
            self.current = false;
            self.send(json!({"op":"Preview","draft":draft,"scenario":self.scenario()}));
        }
    }
    pub fn captured_specimen_request(&self) -> Option<SpecimenRequest> {
        Some(SpecimenRequest { locator: specimen_locator(self.resource.as_ref()?)?, scenario: self.scenario(), generation: self.preview_generation })
    }
    pub fn request_specimen_preview(&mut self) {
        if self.busy.is_some() {
            self.current = false;
            self.pending = Some(Instant::now());
            return;
        }
        self.invalidate_specimen_request();
        let Some(request) = self.captured_specimen_request() else {
            return;
        };
        self.current = false;
        // Keep the previous receipt and view while regenerating. Removing its
        // status/viewport rows changes panel height, invalidates the captured
        // viewport and creates an endless resize -> request -> resize cycle.
        // prepare_resource clears it when the selected definition changes.
        self.render_started = None;
        self.pending = None;
        self.send(request.job());
        self.specimen_request = (self.busy.as_deref() == Some("FixturePreview")).then_some(request);
    }
    pub fn accept_specimen_preview(&mut self, value: Value, operation: &str) -> bool {
        // The bridge checks the outer session/request envelope before returning.
        // Capture selection and scenario independently so even a correctly routed
        // reply cannot substitute another resource or an obsolete presentation.
        if operation != "FixturePreview" || self.busy.is_some() {
            return false;
        }
        let Some(request) = self.specimen_request.take() else {
            return false;
        };
        let Some(selected) = self.resource.as_ref() else {
            return false;
        };
        let resource = &value["resource"];
        let identity = ["sessionId", "requestId", "semanticDigest"].iter().all(|key| value[*key].as_str().is_some_and(|s| !s.is_empty()));
        let matches = self.preview_only
            && self.is_specimen()
            && self.staged.is_none()
            && self.temporary_canvas.is_none()
            && request.generation == self.preview_generation
            && request.scenario == self.scenario()
            && specimen_locator(selected).as_ref() == Some(&request.locator)
            && specimen_locator(resource).as_ref() == Some(&request.locator)
            && resource["fixture"] == selected["fixture"]
            && value["scenario"] == request.scenario
            && value["canonicalResource"] == false
            && value["ownerWrites"] == false
            && value.get("draft").is_none()
            && value["provenance"]["canonicalResource"] == false
            && value["provenance"]["temporary"] == true
            && value["provenance"]["locale"] == self.locale
            && resource["provenance"] == value["provenance"]
            && value["provenance"]["sourceCommit"] == selected["provenance"]["sourceCommit"]
            && resource["resourceId"].as_str().is_some_and(|id| value["draftId"] == format!("registered-fixture:{id}"))
            && value["draftRevision"] == 0
            && value["state"] == "prepared"
            && identity;
        if !matches {
            return false;
        }
        self.preview = Some(value);
        self.validated = true;
        self.current = false;
        self.render_started = Some(Instant::now());
        true
    }
    pub fn settle_specimen_preview(&mut self, value: Value) {
        // Panel reflow and scenario controls may change while the helper works.
        // Retire that response quietly and coalesce one replacement. Showing an
        // error here changes the panel height and can cause a resize/retry loop.
        let superseded = self.specimen_request.as_ref().is_some_and(|request| {
            self.preview_only
                && self.is_specimen()
                && request.generation == self.preview_generation
                && self.resource.as_ref().and_then(specimen_locator).as_ref() == Some(&request.locator)
                && request.scenario != self.scenario()
        });
        if self.accept_specimen_preview(value, "FixturePreview") {
            return;
        }
        self.current = false;
        if superseded {
            self.error = None;
            self.pending = Some(Instant::now());
        } else {
            // A malformed or substituted receipt is terminal. An automatic
            // retry of the same request cannot repair its identity.
            self.pending = None;
            self.error = Some("Obsolete or mismatched registered specimen preview discarded. Select the design or restart preview to retry.".into());
        }
    }
    pub fn matches_preview_event(&self, app: &H, event: &Value) -> bool {
        let receipt_matches = self.preview.as_ref().is_some_and(|p| {
            ["sessionId", "requestId", "draftId", "semanticDigest"]
                .iter()
                .all(|key| p[*key].as_str().is_some_and(|value| !value.is_empty()) && p[*key] == event[*key])
                && p["draftRevision"].as_u64().is_some()
                && p["draftRevision"] == event["draftRevision"]
                && p["scenario"] == event["scenario"]
        }) && event["scenario"] == self.scenario();
        if self.is_specimen() {
            receipt_matches
                && self.preview_only
                && self.specimen_request.is_none()
                && self.busy.as_deref() != Some("FixturePreview")
                && self
                    .preview
                    .as_ref()
                    .is_some_and(|p| p["canonicalResource"] == false && specimen_locator(&p["resource"]) == self.resource.as_ref().and_then(specimen_locator))
        } else {
            receipt_matches
                && self.active_draft(app).is_some_and(|d| {
                    d["draftId"] == event["draftId"] && d["draftRevision"] == event["draftRevision"] && d["semanticDigest"] == event["semanticDigest"]
                })
        }
    }
    pub fn scenario(&self) -> Value {
        json!({"target":"web","locale":self.locale,"theme":self.theme,"themeId":self.theme_id,
        "viewport":{"width":self.viewport.0,"height":self.viewport.1},"fixture":self.resource.as_ref().and_then(|r|r["fixture"].as_str()).unwrap_or("cityos-web-frame"),
        "renderMode":if self.interactive && self.resource.as_ref().is_none_or(|r|r["fixture"]!="cityos-lower-level"){"interactive-fixture"}else{"studio-passive"},"motion":"reduce"})
    }
    pub fn handle_preview_event(&mut self, app: &mut H, event: &Value) {
        if !self.matches_preview_event(app, event) {
            return;
        }
        match event["kind"].as_str() {
            Some("rendered") => {
                // Keep renderer observations behind the full draft/request/scenario
                // fence. They describe rendered CSS, never authored native paints.
                self.current = true;
                if let Some(preview) = &mut self.preview {
                    for (observed, field) in [
                        ("observedViewport", "viewport"),
                        ("observedElevation", "elevation"),
                        ("observedShadow", "shadow"),
                        ("observedThemePrimary", "themePrimary"),
                        ("observedThemePage", "themePage"),
                        ("observedMaterial", "material"),
                        ("observedBackground", "background"),
                        ("observedBorder", "border"),
                        ("observedBackdrop", "backdrop"),
                        ("observedHorizonLayout", "horizonLayout"),
                    ] {
                        preview[observed] = event[field].clone();
                    }
                }
                self.render_started = None;
            }
            Some("failed") => {
                self.current = false;
                self.error = Some("Renderer failed; draft retained".into());
                self.render_started = None;
            }
            Some("selection") => {
                if let Some(node) = event["nodeId"].as_str() {
                    self.select(app, node);
                }
            }
            Some("focus-editor") if self.open && matches!(event["key"].as_str(), Some("F6" | "Escape")) => {
                if let Some(url) = self.preview.as_ref().and_then(|preview| preview["url"].as_str()) {
                    if let Err(error) = self.host.focus_parent(url) {
                        self.error = Some(error);
                    }
                }
            }
            Some("shortcut") if !self.preview_only && self.busy.is_none() && !self.recovery_pending => match event["key"].as_str() {
                Some("s") => {
                    self.host.hide();
                    app.save();
                }
                Some("z") if event["shift"] != true => app.undo(),
                Some("z" | "y") => app.redo(),
                _ => {}
            },
            _ => {}
        }
    }
    pub fn poll(&mut self, app: &mut H) {
        self.dispatch_history_request(app);
        let result = self.bridge.as_ref().and_then(|b| b.replies.try_recv().ok());
        if let Some(result) = result {
            let op = self.busy.take().unwrap_or_default();
            if op == "SemanticCoverage" {
                self.coverage.receive(result);
                return;
            }
            let presentation = self.sent_presentation.take();
            let history_command = if op == "Command" { self.history_command.take() } else { None };
            match result {
                Err(error) => {
                    if op == "FixturePreview" {
                        self.specimen_request = None;
                    }
                    if op == "Themes" {
                        self.themes_loaded = true;
                    }
                    self.error = Some(error);
                    self.current = false;
                }
                Ok(value) => {
                    self.error = None;
                    match op.as_str() {
                        "Resources" => {
                            self.capture_resources(&value);
                            self.resource_diagnostics = value["diagnostics"].as_array().cloned().unwrap_or_default();
                            if let Some(source) = self.requested_source.take() {
                                if let Some(resource) = self.resources.iter().find(|r| r["sourcePath"] == source && r["openAvailable"] != false).cloned() {
                                    let resource = self.requested_pin.take().unwrap_or(resource);
                                    self.open_resource(resource);
                                } else {
                                    self.error = Some(format!("No canonical composition found at {source}. No substitute was opened."));
                                    self.picker_open = true;
                                }
                            }
                        }
                        "Themes" => {
                            self.themes = value["themes"].as_array().cloned().unwrap_or_default();
                            self.themes_loaded = true;
                        }
                        "Harness" => {
                            self.preview = Some(value);
                            self.current = false;
                            self.pending = None;
                            self.render_started = None;
                        }
                        "Open" | "Command" | "Project" => {
                            if op == "Project" && !self.project_response_current(app, &value, presentation.as_ref()) {
                                self.pending = Some(Instant::now());
                                return;
                            }
                            if op == "Command" && app.status().frozen {
                                self.error = Some("A delayed edit cannot replace the frozen submitted draft.".into());
                                return;
                            }
                            if presentation::scene_resource(&value["resource"]) && !presentation::scene_matches(&value["projection"], presentation.as_ref()) {
                                self.error = Some("Native scene does not match the requested presentation; previous canvas retained".into());
                                return;
                            }
                            match binding(value["draft"].clone(), &value["projection"]) {
                                Err(e) => self.error = Some(e),
                                Ok(mut document) => {
                                    let projected_key = value["projection"]
                                        .get("scene")
                                        .map(|_| presentation::key(&value["draft"], presentation.as_ref().unwrap_or(&Value::Null)));
                                    self.property_text.clear();
                                    if op != "Open" && !self.preview_only {
                                        document.name = app.document().name.clone();
                                    }
                                    if op != "Project" {
                                        self.incoming = None;
                                    }
                                    self.resource = Some(value["resource"].clone());
                                    if self.preview_only {
                                        self.temporary_canvas = Some(H::temporary_canvas(document.clone()));
                                        self.staged = Some(document);
                                        if self.open_after_preview {
                                            self.open_after_preview = false;
                                            self.install_staged(app);
                                        }
                                    } else if op == "Open" {
                                        if let Err(error) = self.replace_active(app, document, None) {
                                            self.error = Some(error);
                                            return;
                                        }
                                        app.enter_document();
                                    } else if op == "Command" {
                                        let previous = revision(app.document());
                                        let next = revision(&document);
                                        if !previous.zip(next).is_some_and(|((id, rev), (next_id, next_rev))| id == next_id && next_rev == rev + 1) {
                                            self.error = Some("Obsolete draft command response discarded".into());
                                            return;
                                        }
                                        if let Some(history_command) = history_command {
                                            if let Err(error) = history_command.install(app, document) {
                                                self.error = Some(error);
                                                return;
                                            }
                                        } else if let Err(error) = app.install_edit(document) {
                                            self.error = Some(error);
                                            return;
                                        }
                                    } else if !self.install_resumed_projection(app, document) {
                                        self.resume_presentation = projected_key;
                                        return;
                                    }
                                    if let Some(key) = projected_key {
                                        self.projection_attempted = Some(key.clone());
                                        self.canvas_presentation = Some(key);
                                    }
                                    self.validated = true;
                                    if !self.preview_only {
                                        let selected = document_selection(app.document(), &self.selected);
                                        self.select(app, &selected);
                                    }
                                    self.observed = None;
                                    self.pending = Some(Instant::now());
                                    if op == "Command" {
                                        // Undo/redo can run while Changes is visible.
                                        // An export belongs to the preceding revision;
                                        // retire it atomically with the installed edit.
                                        self.compare = None;
                                        self.compare_open = false;
                                        if self.changes_mode {
                                            if let Some(draft) = self.active_draft(app) {
                                                self.send(json!({"op":"Export","draft":draft}));
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        "Preview" => {
                            let current = self.active_draft(app);
                            if current.as_ref().is_some_and(|d| {
                                d["draftId"] == value["draftId"]
                                    && d["draftRevision"] == value["draftRevision"]
                                    && d["semanticDigest"] == value["semanticDigest"]
                                    && value["scenario"] == self.scenario()
                            }) {
                                self.preview = Some(value);
                                self.current = false;
                                self.render_started = Some(Instant::now());
                            }
                        }
                        "FixturePreview" => {
                            self.settle_specimen_preview(value);
                        }
                        "Fixtures" => match self.local_specimen_document(value) {
                            Ok(document) => self.open_local_document(app, document),
                            Err(error) => self.error = Some(error),
                        },
                        "Export" => {
                            self.compare = Some(serde_json::to_string_pretty(&value).unwrap_or_default());
                            self.compare_open = !self.changes_mode;
                        }
                        "Incoming" => {
                            self.resolutions.clear();
                            self.incoming = Some(value);
                        }
                        "Dependencies" => {
                            let retained = self
                                .config
                                .as_ref()
                                .zip(self.dependency_draft.as_ref())
                                .ok_or_else(|| "Dependency context changed".to_owned())
                                .and_then(|(config, draft)| app.retain_dependencies(&config.cache, draft, &value));
                            match retained {
                                Err(error) => {
                                    self.error = Some(error);
                                    self.validated = false;
                                }
                                Ok(()) => {
                                    self.dependency_draft = None;
                                    match self.dependency_action.take() {
                                        Some(DependencyAction::Open) => self.install_staged(app),
                                        Some(DependencyAction::Resume) => {
                                            let history = app.validate_history_dependencies();
                                            if let Err(error) = history {
                                                self.validated = false;
                                                self.error = Some(format!(
                                                    "Retained history dependencies are unavailable. Restore a portable checkpoint before editing: {error}"
                                                ));
                                            } else {
                                                if let Some(document) = self.resume_document.take() {
                                                    let key = self.resume_presentation.take();
                                                    if self.has_native_scene()
                                                        && key.as_ref()
                                                            != self
                                                                .active_draft(app)
                                                                .as_ref()
                                                                .map(|draft| presentation::key(draft, &self.presentation()))
                                                                .as_ref()
                                                    {
                                                        self.validated = false;
                                                        self.pending = Some(Instant::now());
                                                    } else if let Err(error) = Self::activate_resumed_document(app, document) {
                                                        self.error = Some(error);
                                                        self.validated = false;
                                                    } else {
                                                        if let Some(key) = key {
                                                            self.projection_attempted = Some(key.clone());
                                                            self.canvas_presentation = Some(key);
                                                        }
                                                        self.validated = true;
                                                    }
                                                }
                                                self.observed = None;
                                                self.pending = Some(Instant::now());
                                            }
                                        }
                                        Some(DependencyAction::Reconcile) => {
                                            if let Some(bundle) = self.reconciliation_bundle.take() {
                                                self.install_reconciliation(app, bundle);
                                            }
                                        }
                                        Some(DependencyAction::Accepted) => {
                                            if let Some(bundle) = self.accepted_bundle.take() {
                                                self.install_accepted(app, bundle);
                                            }
                                        }
                                        None => {}
                                    }
                                }
                            }
                            if let Some(lease) = value["leaseId"].as_str() {
                                self.send(json!({"op":"ReleaseDependencies","leaseId":lease}));
                            }
                        }
                        "Reconcile" => {
                            if app.status().frozen {
                                self.error = Some("A submitted draft cannot adopt incoming changes until its original outcome is resolved.".into());
                            } else {
                                self.install_reconciliation(app, value);
                            }
                        }
                        "AcceptedProjection" => self.install_accepted(app, value),
                        _ => {}
                    }
                }
            }
        }
        for event in self.host.poll() {
            self.handle_preview_event(app, &event);
        }
        self.dispatch_history_request(app);
        if !self.preview_work_visible() {
            self.release_inactive_preview();
        }
        self.expire_render_acknowledgement(Instant::now());
        let identity = if self.preview_only { self.staged.as_ref().and_then(revision) } else { revision(app.document()) };
        if identity != self.observed {
            self.observed = identity;
            self.current = false;
            self.pending = Some(Instant::now());
        }
        if self.refresh_native_presentation(app) {
            // Projection and renderer are serialized over the same semantic draft.
        } else if self.validated && self.actual_preview() && !self.is_specimen() && !self.themes_loaded && self.busy.is_none() {
            if let Some(d) = self.active_draft(app) {
                self.send(json!({"op":"Themes","draft":d}));
            }
        } else if self.validated
            && self.actual_preview()
            && self.preview_work_visible()
            && self.pending.is_some_and(|at| at.elapsed() >= Duration::from_millis(250))
            && self.busy.is_none()
        {
            self.pending = None;
            self.request_preview(app);
        }
        if self.harness || !self.actual_preview() {
            self.pending = None;
        }
    }
    pub fn select(&mut self, app: &mut H, node: &str) {
        if self.is_specimen() {
            return;
        }
        self.selected = node.into();
        app.select_source(node);
    }
    pub fn capture_resources(&mut self, value: &Value) {
        self.resources = value["resources"].as_array().cloned().unwrap_or_default();
        self.resources.extend(value["specimens"].as_array().into_iter().flatten().filter(|r| r["kind"] == "registered-specimen").cloned());
        // Metadata selection is independent of renderer availability. Keep every
        // registered definition visible without compiling its defaults eagerly.
        for selection in value["specimenSelections"].as_array().into_iter().flatten() {
            let id = selection["resourceId"].as_str().filter(|id| !id.is_empty());
            if id.is_none() {
                continue;
            }
            if let Some(resource) = self.resources.iter_mut().find(|r| r["kind"] == "registered-specimen" && r["resourceId"].as_str() == id) {
                for key in ["level", "target"] {
                    if selection[key].is_string() {
                        resource[key] = selection[key].clone();
                    }
                }
                continue;
            }
            let mut resource = selection.clone();
            resource["kind"] = json!("registered-specimen");
            resource["actualWebPreview"] = json!(false);
            resource["editable"] = json!(false);
            resource["fixture"] = selection["previewAdapter"].clone();
            resource["previewNotice"] = json!(registered_availability_reason(selection));
            self.resources.push(resource);
        }
    }
    pub fn request_coverage(&mut self, refresh: bool) {
        self.coverage.open = true;
        if self.busy.is_some() || self.recovery_pending || (!refresh && self.coverage.report.is_some()) {
            return;
        }
        self.send(json!({"op":"SemanticCoverage","summaryOnly":true,"locale":self.locale}));
    }
    pub fn choose_design_file(&mut self) {
        self.picker_open = self.changes_mode || !self.picker_open;
        self.changes_mode = false;
        if self.resources.is_empty() {
            self.send(json!({"op":"Resources"}));
        }
    }
    pub fn progress_repaint_delay(&self) -> Option<Duration> {
        self.progress_repaint_delay_at(Instant::now())
    }
    fn preview_work_visible(&self) -> bool {
        !self.changes_mode && (self.preview_only || self.inline_identity.is_some() || self.layout.view != layout::View::Canvas)
    }
    pub fn progress_repaint_delay_at(&self, now: Instant) -> Option<Duration> {
        let visible_pending = !self.harness && self.pending.is_some() && self.preview_work_visible();
        repaint::progress_delay(now, self.busy.is_some(), visible_pending, self.render_started)
    }
    pub fn expire_render_acknowledgement(&mut self, now: Instant) {
        if repaint::acknowledgement_expired(now, self.render_started) {
            self.error = Some("Renderer did not acknowledge this revision; restart preview to retry".into());
            self.render_started = None;
            self.current = false;
        }
    }
    pub fn observe_preview_viewport(&mut self, viewport: (u32, u32)) {
        if viewport != self.panel_size {
            self.panel_size = viewport;
            self.viewport = viewport;
            // The inert harness has no semantic revision to compile. Its child
            // bounds are updated by Host::show without scheduling draft work.
            if !self.harness {
                self.current = false;
                self.pending = Some(Instant::now());
            }
        }
    }
    pub fn release_inactive_preview(&mut self) {
        self.host.release();
        // A prepared response can arrive before any child exists (for example
        // reopening on Changes). No hidden receipt can await a renderer ACK.
        // Retain the artifact and pending draft for its next visible activation.
        self.current = false;
        self.render_started = None;
    }
    pub fn performance_state(&self) -> [bool; 5] {
        [self.busy.is_some(), self.pending.is_some(), self.render_started.is_some(), self.current, self.progress_repaint_delay().is_some()]
    }
    pub fn show(&mut self, app: &mut H, ui: &mut egui::Ui, frame: &eframe::Frame) {
        ui.spacing_mut().item_spacing = egui::vec2(10., 8.);
        ui.spacing_mut().button_padding = egui::vec2(12., 8.);
        ui.spacing_mut().interact_size.y = 34.;
        for (style, size) in [
            (egui::TextStyle::Body, 15.),
            (egui::TextStyle::Button, 15.),
            (egui::TextStyle::Small, 13.),
            (egui::TextStyle::Heading, 20.),
            (egui::TextStyle::Monospace, 13.),
        ] {
            ui.style_mut().text_styles.insert(style, egui::FontId::proportional(size));
        }
        self.submissions.refresh_if_changed(app);
        self.poll(app);
        if !self.preview_only && self.validated && self.busy.is_none() && self.submissions.accepted().is_some() && self.submissions.requested.is_none() {
            self.recover_accepted(app);
        }
        let ctx = ui.ctx().clone();
        let old_layout = self.layout.clone();
        // Input and WebView IPC wake the window independently. A current draft
        // does not need an idle heartbeat that redraws the entire canvas.
        // A transient notice must not resize the WebView or invalidate a
        // projection. Reserve one bounded status row in every workspace state.
        egui::Panel::bottom("cityos-workspace-save-status").exact_size(24.).show(ui, |ui| {
            let status = app.status();
            let toast = status.toast.as_ref();
            if let Some(toast) = toast {
                ctx.request_repaint_after(toast.remaining);
            }
            let message = toast.map(|toast| toast.text.as_str()).unwrap_or_else(|| {
                if self.preview_only {
                    "Temporary preview"
                } else if status.frozen {
                    "Submitted draft · frozen"
                } else if status.save_failures {
                    "Save needs attention · local changes retained"
                } else if status.dirty {
                    "Unsaved local changes"
                } else if status.library_file || status.external_path.is_some() {
                    "Saved draft"
                } else {
                    "Local draft · not saved yet"
                }
            });
            let color = if toast.is_some_and(|toast| toast.error) { ui.visuals().error_fg_color } else { ui.visuals().text_color() };
            ui.add(egui::Label::new(egui::RichText::new(message).color(color)).truncate());
        });
        // `app` remains the user's persistent document while a temporary preview
        // lives in `staged`/`temporary_canvas`. Leaving must still save that draft.
        if !self.recovery_pending {
            app.autosave_tick(&ctx);
        }
        if ctx.input(|i| i.viewport().close_requested()) && (self.recovery_pending || !app.flush_autosave()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
        if !ctx.egui_wants_keyboard_input() && self.busy.is_none() && !self.recovery_pending {
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::S)) && !self.preview_only {
                app.save();
            }
            if !self.preview_only {
                match ctx.input_mut(history_commands::take_shortcut) {
                    Some(history_commands::Shortcut::Undo) => app.undo(),
                    Some(history_commands::Shortcut::Redo) => app.redo(),
                    None => {}
                }
            }
        }
        egui::Panel::top("cityos-workspace-actions").show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                let can_leave = !self.recovery_pending && self.busy.is_none();
                let files = ui.add_enabled(can_leave && (app.status().library_available || app.status().home), egui::Button::new("Desigma files"))
                    .on_hover_text("Save the current draft and return to the Desigma file library")
                    .on_disabled_hover_text("Wait for the current operation, recover the draft, or attach a Desigma file library");
                H::record_widget(ui, "workspace-desigma-files", files.rect);
                if files.clicked() {
                    if let Err(error) = self.open_desigma_files(app) { self.error = Some(error); }
                }
                if ui.add_enabled(can_leave,egui::Button::new("Library"))
                    .on_hover_text("Browse the CityOS library")
                    .on_disabled_hover_text("Wait for the current operation or recover the draft before leaving").clicked() {
                    self.open = false;
                }
                if ui.selectable_label(!self.changes_mode,"Design").clicked(){self.changes_mode=false;}
                if ui.add_enabled(self.validated&&!self.is_specimen()&&self.busy.is_none(),egui::Button::new("Changes").selected(self.changes_mode))
                    .on_disabled_hover_text(if self.is_specimen() { SPECIMEN_CANVAS_REASON } else { "Wait for a canonical draft to load" }).clicked(){
                    self.changes_mode=true;self.compare_open=false;
                    if let Some(draft)=self.active_draft(app){self.send(json!({"op":"Export","draft":draft}));}
                }
                ui.strong(if self.preview_only { "Design preview" } else { "CityOS workspace" });
                H::workspace_tools(ui);
                if ui.add_enabled(self.busy.is_none()&&!self.recovery_pending, egui::Button::new("Choose design file…")).clicked() {
                    self.choose_design_file();
                }
                if ui.add_enabled(self.busy.is_none()&&!self.recovery_pending, egui::Button::new("L0–L9 coverage"))
                    .on_hover_text("Inspect installed preview, canvas and editing support at every level").clicked() {
                    self.request_coverage(false);
                }
                if self.preview_only && !self.harness {
                    if ui
                        .add_enabled(
                            !self.is_specimen() && self.staged.is_some() && self.busy.is_none(),
                            egui::Button::new(if self.actual_preview() { "Open on canvas" } else { "Open protected canvas" }),
                        )
                        .on_disabled_hover_text(if self.is_specimen() { SPECIMEN_CANVAS_REASON } else { "Wait for the selected source to load" })
                        .clicked()
                    {
                        self.install_staged(app);
                    }
                    if !app.status().home && (app.document().cityos.is_some() || app.status().can_undo) && ui.button("Save current draft").clicked() {
                        self.host.hide();
                        app.save();
                    }
                } else if !self.preview_only {
                    if ui.add_enabled(!self.recovery_pending,egui::Button::new("Save draft")).clicked() {
                        self.host.hide();
                        app.save();
                    }
                    ui.add_enabled_ui(!self.recovery_pending,|ui|{ui.menu_button("Draft actions", |ui| {
                    if !app.status().library_file && ui.button("Save to draft library").clicked() {
                        ui.close();
                        match Self::save_to_draft_library(app) {
                            Ok(()) => app.toast("Draft and history saved to the CityOS draft library",false),
                            Err(error) => self.error=Some(error),
                        }
                    }
                    if ui.add_enabled(app.status().library_file&&self.validated,egui::Button::new("Export portable draft…")).clicked() {
                        ui.close();self.host.hide();
                        let export=(|| -> Result<(),String> {
                            Self::save_before_switch(app)?;
                            app.export_portable()?;
                            Ok(())
                        })();
                        if let Err(error)=export{self.error=Some(error);}
                    }
                    if ui.button("Open saved draft…").clicked() {
                        ui.close();
                        self.host.hide();
                        app.open_dialog();
                        // A loaded file never inherits the previous document's validation,
                        // preview receipts or outstanding bridge responses.
                        if let Some(config) = self.config.clone() {
                            self.resume(config, app);
                        }
                    }
                    if ui.add_enabled(self.validated && self.busy.is_none(), egui::Button::new("Compare / export")).clicked() {
                        ui.close();
                        if let Some(draft) = self.active_draft(app) {
                            self.send(json!({"op":"Export","draft":draft}));
                        }
                    }
                    if ui.add_enabled(self.validated && self.busy.is_none() && app.status().can_undo, egui::Button::new("Undo")).clicked() {
                        ui.close();
                        app.undo();
                    }
                    if ui.add_enabled(self.validated && self.busy.is_none() && app.status().can_redo, egui::Button::new("Redo")).clicked() {
                        ui.close();
                        app.redo();
                    }
                    ui.add_enabled(false, egui::Button::new("Submit proposal"))
                        .on_hover_text("Engineering owner publication and merge qualification remains required");
                    });});
                }
                if ui.button("Reset panels").clicked() {
                    self.layout = Default::default();
                    self.narrow_preview = false;
                    // Keep focus, catalog navigation and unrelated editor preferences.
                    ctx.data_mut(|data| {
                        data.remove::<egui::containers::panel::PanelState>(egui::Id::new("cityos-workspace-properties"));
                        data.remove::<egui::containers::panel::PanelState>(egui::Id::new("cityos-workspace-preview"));
                    });
                }
                if self.actual_preview() && !self.preview_only {
                    ui.selectable_value(&mut self.layout.view, layout::View::Canvas, "Canvas");
                    ui.selectable_value(&mut self.layout.view, layout::View::Split, "Split");
                    ui.selectable_value(&mut self.layout.view, layout::View::Preview, "Preview");
                }
                if ui.add_enabled(self.validated && self.busy.is_none(), egui::Button::new("Create local copy"))
                    .on_disabled_hover_text("Wait for the selected preview or projection to load")
                    .on_hover_text("Create an ordinary editable copy of the native inspection layout. It has no CityOS proposal binding; actual target rendering qualifies separately.").clicked() {
                    self.create_local_copy(app);
                }
                if !self.preview_only { ui.toggle_value(&mut self.layout.inspector, "Properties"); }
            });
            H::workspace_tools_feedback(ui);
        });
        let active_id = draft(app.document()).and_then(|d| d["draftId"].as_str()).map(str::to_owned);
        if active_id.is_some() || !self.tabs.is_empty() {
            let tabs: Vec<_> = self.tabs.iter().filter_map(|(id, t)| t.session.as_ref().map(|session| (id.clone(), session.name().to_owned()))).collect();
            let mut activate = None;
            let mut close = None;
            egui::Panel::top("cityos-draft-tabs").show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if active_id.is_some()
                        && ui
                            .add_enabled(
                                !self.recovery_pending,
                                egui::Button::new(format!("{} · {}", app.document().name, if app.status().dirty { "unsaved" } else { "local draft" }))
                                    .selected(!self.preview_only),
                            )
                            .clicked()
                    {
                        if let Some(config) = self.config.clone() {
                            self.resume(config, app);
                        }
                    }
                    for (id, name) in tabs {
                        if ui.add_enabled(self.busy.is_none() && !self.recovery_pending, egui::Button::new(name)).clicked() {
                            activate = Some(id.clone());
                        }
                        if ui.small_button("×").on_hover_text("Close saved inactive draft").clicked() {
                            close = Some(id);
                        }
                    }
                    ui.small("One active canvas / renderer");
                });
            });
            if let Some(id) = close {
                self.tabs.remove(&id);
            }
            if let Some(id) = activate {
                self.activate_tab(app, &id);
            }
        }
        if self.changes_mode {
            if old_layout != self.layout {
                self.save_layout();
            }
            self.release_inactive_preview();
            egui::CentralPanel::default().show(ui,|ui|{
                ui.heading("Changes");ui.label("Review incoming source changes before updating this draft's pinned baseline.");
                if let Some(error)=&self.error{ui.colored_label(ui.visuals().error_fg_color,error);}
                if self.recovery_pending && self.accepted_bundle.is_none() && self.submissions.accepted().is_none()
                    && ui.button("Recover document generation").clicked(){self.recover_reconciliation(app);}
                if ui.button("Refresh local submission status").clicked(){self.submissions.reload(app);}
                if let Some(error)=&self.submissions.error { ui.colored_label(ui.visuals().error_fg_color,error); }
                for record in &self.submissions.records {
                    ui.group(|ui|{
                        ui.strong(submission_recovery::state_label(&record.state));
                        ui.label(format!("{} · {}",record.command.owner,record.command.operation_id));
                        ui.small(format!("Original request {}",record.request_digest));
                    });
                }
                if self.submissions.accepted().is_some() && ui.add_enabled(self.busy.is_none(),egui::Button::new("Recover accepted state")).clicked(){self.recover_accepted(app);}
                ui.horizontal_wrapped(|ui|{
                    if ui.add_enabled(self.validated&&self.busy.is_none(),egui::Button::new("Review incoming changes")).clicked(){if let Some(draft)=self.active_draft(app){self.send(json!({"op":"Incoming","draft":draft}));}}
                    if ui.add_enabled(self.validated&&self.busy.is_none(),egui::Button::new("Compare local edits")).clicked(){if let Some(draft)=self.active_draft(app){self.send(json!({"op":"Export","draft":draft}));}}
                    ui.add_enabled(false,egui::Button::new("Submit proposal")).on_hover_text("Engineering owner acceptance and recovery qualification is required.");
                    ui.add_enabled(false,egui::Button::new("Check outcome")).on_hover_text("No qualified owner submission adapter is installed.");
                });
                let mut apply_incoming=false;
                egui::ScrollArea::vertical().id_salt("cityos-changes-content").show(ui,|ui|{
                    if let Some(incoming)=&self.incoming {
                        ui.heading("Incoming source");
                        match incoming["status"].as_str() {
                            Some("current") => { ui.label("This draft already uses the current source baseline."); },
                            Some("unavailable") => { ui.colored_label(ui.visuals().error_fg_color,incoming["reason"].as_str().unwrap_or("Incoming source unavailable")); },
                            _ => {
                                if let Some(commit)=incoming["candidate"]["base"]["commit"].as_str(){ui.label(format!("Incoming canonical revision {}",&commit[..commit.len().min(12)]));}
                                let changes=incoming["changes"].as_array().cloned().unwrap_or_default();
                                ui.label(format!("{} incoming semantic changes",changes.len()));
                                for change in changes {ui.label(change["sourcePointer"].as_str().unwrap_or("Source change"));}
                                let conflicts=incoming["conflicts"].as_array().cloned().unwrap_or_default();
                                for conflict in &conflicts {
                                    let pointer=conflict["sourcePointer"].as_str().unwrap_or("");
                                    ui.group(|ui| {
                                        ui.strong(pointer);
                                        ui.label(format!("Your value: {}",conflict["local"]));
                                        ui.label(format!("Incoming value: {}",conflict["incoming"]));
                                        let decision=self.resolutions.entry(pointer.into()).or_default();
                                        ui.horizontal(|ui|{ui.selectable_value(decision,"local".into(),"Keep mine");ui.selectable_value(decision,"incoming".into(),"Use incoming");});
                                    });
                                }
                                let resolved=conflicts.iter().all(|c|c["sourcePointer"].as_str().is_some_and(|p|self.resolutions.get(p).is_some_and(|s|!s.is_empty())));
                                let eligible=incoming["reconcileAvailable"]==true && resolved && self.validated && !self.preview_only && self.busy.is_none() && app.status().library_file;
                                if ui.add_enabled(eligible,egui::Button::new("Apply reviewed incoming changes")).clicked(){apply_incoming=true;}
                                if !app.status().library_file{ui.small("Save this draft to the library before applying incoming changes.");}
                                else if !resolved{ui.small("Choose a value for each conflict before applying.");}
                            },
                        }
                        ui.collapsing("Source and reconciliation details",|ui|{ui.monospace(serde_json::to_string_pretty(incoming).unwrap_or_default());});
                    }
                    ui.heading("Semantic edits");
                    if let Some(compare)=&self.compare {
                        if ui.button("Copy semantic export").clicked(){ctx.copy_text(compare.clone());}
                        if let Ok(export)=serde_json::from_str::<Value>(compare) {
                            ui.label(format!("{} authored operations",export["operationCount"]));
                            if let Some(changes)=export["changeset"]["changes"].as_array() {for change in changes {
                                ui.group(|ui| {
                                    ui.strong(semantic_change_label(change));
                                    ui.label(format!("{} → {}",
                                        semantic_change_value_text(change.get("before"), export.get("base")),
                                        semantic_change_value_text(change.get("after"), export.get("candidate"))));
                                });
                            }}
                            ui.collapsing("Complete semantic export",|ui|{ui.monospace(compare);});
                        } else {ui.monospace(compare);}
                    } else {ui.label(if self.busy.as_deref() == Some("Export") { "Comparing current draft…" } else { "Choose Compare local edits." });}
                });
                if apply_incoming {if let (Some(draft),Some(incoming))=(self.active_draft(app),self.incoming.clone()){
                    let resolutions:Vec<_>=self.resolutions.iter().filter(|(_,value)|!value.is_empty()).map(|(pointer,value)|json!({"sourcePointer":pointer,"resolution":value})).collect();
                    self.send(json!({"op":"Reconcile","expectedRevision":draft["draftRevision"],"draft":draft,"incoming":{"reconciliationId":incoming["reconciliationId"],"pin":incoming["pin"]},"resolutions":resolutions}));
                }}
            });
            if let Some(delay) = self.progress_repaint_delay() {
                ctx.request_repaint_after(delay);
            }
            return;
        }
        if let Some(error) = self.error.clone() {
            egui::Panel::top("cityos-workspace-error").show(ui, |ui| {
                ui.colored_label(ui.visuals().error_fg_color, &error);
                if self.recovery_pending && self.submissions.accepted().is_some() {
                    if ui.button("Recover accepted state").clicked() {
                        self.recover_accepted(app);
                    }
                } else if self.recovery_pending && ui.button("Recover document generation").clicked() {
                    self.recover_reconciliation(app);
                }
                if !self.preview_only && !self.recovery_pending && self.busy.is_none() && ui.button("Retry draft validation").clicked() {
                    if let Some(config) = self.config.clone() {
                        self.resume(config, app);
                    }
                }
            });
        }
        {
            egui::Panel::top("cityos-workspace-busy").exact_size(38.).show(ui, |ui| {
                ui.horizontal(|ui| {
                    if let Some(operation) = &self.busy {
                        crate::jobs::progress_marker(ui);
                        ui.label(format!("{operation}…"));
                    } else {
                        ui.label(if self.harness {
                            "Embedded host diagnostic"
                        } else if self.preview_only {
                            "Temporary preview · current draft preserved"
                        } else {
                            "Canonical draft · local changes only"
                        });
                        if let Some(path) = &app.status().external_path {
                            ui.small(path.display().to_string());
                        }
                    }
                });
            });
        }
        if self.picker_open {
            egui::Panel::left("cityos-design-file-picker").default_size(310.).size_range(240.0..=460.0).resizable(true).show(ui, |ui| {
                ui.heading("Design files");
                ui.small("Select to preview. Canonical files can open on canvas; registered defaults stay temporary.");
                ui.text_edit_singleline(&mut self.resource_search);
                let selected = resource_picker(ui, &self.resources, self.resource.as_ref(), &self.resource_search, self.busy.is_some());
                if !self.resource_diagnostics.is_empty() {
                    ui.collapsing(format!("{} discovery findings", self.resource_diagnostics.len()), |ui| {
                        egui::ScrollArea::vertical().max_height(120.).show(ui, |ui| {
                            for finding in &self.resource_diagnostics {
                                ui.small(if finding["code"] == "PACKAGED_CANONICAL_REVISION" {
                                    format!(
                                        "Packaged canonical revision {} · newer main {} needs a matching renderer",
                                        finding["commit"], finding["latestCommit"]
                                    )
                                } else {
                                    format!("{}: {}", finding["sourcePath"], finding["error"])
                                });
                            }
                        });
                    });
                }
                if let Some(resource) = selected {
                    self.open_after_preview = false;
                    self.open_resource(resource);
                }
            });
        }
        if let Some(resource) = self.resource.clone() {
            egui::Panel::top("cityos-selected-resource").show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.strong(resource["label"].as_str().or_else(|| resource["resourceId"].as_str()).unwrap_or("Selected design"));
                    ui.small(if self.is_specimen() {
                        "Temporary registered defaults"
                    } else if self.preview_only {
                        "Temporary preview"
                    } else {
                        "Canonical bound draft"
                    });
                    ui.menu_button("Source details", |ui| {
                        ui.set_max_width(600.);
                        if self.is_specimen() {
                            ui.label(SPECIMEN_CANVAS_REASON);
                            for key in ["componentId", "componentVersion", "profileRef", "targetProfileRef"] {
                                ui.label(format!("{key}: {}", resource[key].as_str().unwrap_or("unavailable")));
                            }
                            if let Some(commit) = resource["provenance"]["sourceCommit"].as_str() {
                                ui.monospace(format!("Installed renderer source {commit}"));
                            }
                        }
                        ui.label(resource["sourcePath"].as_str().unwrap_or(""));
                        if let Some(commit) = resource["commit"].as_str() {
                            ui.monospace(format!("GitLab commit {commit}"));
                        }
                        if resource["packagedRevision"] == true {
                            ui.label("Pinned packaged revision. Newer main needs a matching renderer package.");
                        }
                        if let Some(messages) = resource["diagnostics"].as_array() {
                            for message in messages {
                                ui.label(message.as_str().unwrap_or(""));
                            }
                        }
                    });
                    if let Some(d) = self.active_draft(app) {
                        ui.small(format!("Draft revision {}", d["draftRevision"]));
                    }
                    if self.is_specimen() {
                        ui.small(SPECIMEN_CANVAS_REASON);
                    }
                    if let Some(messages) = resource["diagnostics"].as_array() {
                        if !messages.is_empty() {
                            ui.small(format!("{} source findings", messages.len()));
                        }
                    }
                });
            });
        }
        let mut bounds = None;
        let narrow = ui.available_width() < 1100.;
        if narrow && !self.preview_only && self.actual_preview() && self.layout.view == layout::View::Split {
            egui::Panel::top("cityos-narrow-surfaces").show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.narrow_preview, false, "Canvas");
                    ui.selectable_value(&mut self.narrow_preview, true, "Actual preview");
                });
            });
        }
        let effective_view = self.layout.effective_view(ui.available_width(), self.narrow_preview);
        if self.layout.inspector
            && ((!self.preview_only && effective_view != layout::View::Preview) || (self.preview_only && !self.actual_preview() && self.validated))
        {
            egui::Panel::left("cityos-workspace-properties").default_size(248.).size_range(200.0..=360.0).resizable(true).show(ui, |ui| {
                ui.heading("Layers & properties");
                fn layers(node: &Value, depth: usize, rows: &mut Vec<(String, String)>) {
                    if depth > 12 {
                        return;
                    }
                    if let Some(id) = node["id"].as_str() {
                        rows.push((id.into(), format!("{}{}", "  ".repeat(depth), id)));
                    }
                    if let Some(slots) = node["slots"].as_object() {
                        for children in slots.values() {
                            if let Some(children) = children.as_array() {
                                for child in children {
                                    layers(child, depth + 1, rows);
                                }
                            }
                        }
                    }
                }
                let mut rows = vec![];
                if let Some(d) = self.active_draft(app) {
                    layers(&d["semanticDraft"]["root"], 0, &mut rows);
                }
                for (node, label) in rows {
                    if ui.selectable_label(self.selected == node, label).clicked() {
                        if self.preview_only {
                            self.selected = node.clone();
                            if let Some(canvas) = self.temporary_canvas.as_mut() {
                                H::select_temporary_source(canvas, &node);
                            }
                        } else {
                            self.select(app, &node);
                        }
                    }
                }
                if self.resource.as_ref().is_some_and(|r| r["editable"] == true) {
                    ui.separator();
                    let fields = self.resource.as_ref().and_then(|r| r["fields"].as_array()).cloned().unwrap_or_default();
                    for field in fields {
                        if let Some(d) = self.active_draft(app) {
                            let pointer = field["sourcePointer"]
                                .as_str()
                                .unwrap_or("")
                                .strip_prefix("/composition")
                                .unwrap_or(field["sourcePointer"].as_str().unwrap_or(""));
                            let previous = d["semanticDraft"].pointer(pointer).cloned().unwrap_or(Value::Null);
                            let mut value = previous.clone();
                            ui.label(format!("{}.{}", field["nodeId"].as_str().unwrap_or(""), field["property"].as_str().unwrap_or("")));
                            if app.status().frozen {
                                ui.small("Submitted draft frozen · inspect its original outcome in Changes.");
                            }
                            ui.add_enabled_ui(
                                self.validated && !app.status().frozen && self.busy.is_none() && self.selected == field["nodeId"].as_str().unwrap_or(""),
                                |ui| {
                                    if let Some(mut boolean) = value.as_bool() {
                                        if ui.checkbox(&mut boolean, "Allow multiple open sections").changed() {
                                            value = json!(boolean);
                                        }
                                    } else if let Some(values) = field["values"].as_array() {
                                        ui.horizontal_wrapped(|ui| {
                                            for option in values {
                                                ui.selectable_value(&mut value, option.clone(), option.as_str().unwrap_or(""));
                                            }
                                        });
                                    } else if let Some(text) = previous.as_str() {
                                        let key = format!("{}:{}:{}", d["draftId"], d["draftRevision"], pointer);
                                        let edited = self.property_text.entry(key).or_insert_with(|| text.into());
                                        let response = ui.text_edit_singleline(edited);
                                        if ui.button("Apply property").clicked() || response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                            value = json!(edited.clone());
                                        }
                                        ui.small("Validated against the registered component contract before applying.");
                                    }
                                },
                            );
                            if value != previous {
                                self.send_property_command(app, &field, value);
                            }
                        }
                    }
                    ui.small("Only the declared properties are editable. Slots and owner actions remain protected.");
                } else {
                    ui.small("Protected source nodes. No editing adapter is enabled for this file.");
                }
                ui.separator();
                if let Some(d) = self.active_draft(app) {
                    ui.label(format!("Draft revision {}", d["draftRevision"]));
                    if let Some(commit) = d["base"]["commit"].as_str() {
                        ui.small(format!("Canonical base {}", &commit[..commit.len().min(12)]));
                    }
                }
                ui.small("Canvas geometry is a design projection.");
            });
        }
        let show_side_preview = self.actual_preview() && effective_view == layout::View::Split && !self.preview_only;
        let show_full_preview = self.actual_preview() && (effective_view == layout::View::Preview || self.preview_only);
        let preview_width = self.layout.preview_width;
        let mut resized_preview = None;
        let mut web_panel = |ui: &mut egui::Ui| {
            ui.heading(if self.harness { "Embedded host diagnostic" } else { "Actual web renderer" });
            if self.harness {
                ui.label("Inert application content · no semantic draft or renderer qualification");
            } else if let Some(p) = &self.preview {
                let status = if self.current {
                    "Current"
                } else if self.error.is_some() {
                    "Failed — previous preview retained"
                } else if matches!(self.busy.as_deref(), Some("Preview" | "FixturePreview")) || self.render_started.is_some() {
                    "Rendering"
                } else {
                    "Out of date"
                };
                if self.is_specimen() {
                    ui.label(format!("{status} · temporary registered defaults"));
                } else {
                    ui.label(format!("{status} · rendered revision {}", p["draftRevision"]));
                }
                if p["scenario"]["fixture"] == "cityos-web-frame" {
                    ui.small(format!("Rendered elevation: {}", p["observedElevation"].as_str().unwrap_or("pending")));
                }
                ui.small(format!("Viewport: {} × {} CSS px", p["observedViewport"]["width"], p["observedViewport"]["height"]));
            } else {
                ui.label("Waiting for preview");
            }
            ui.small(if self.harness {
                "Inert embedded-host diagnostic · no repository code"
            } else {
                "Registered CityOS renderer · deterministic fixture · owner actions disabled"
            });
            if !self.harness {
                ui.small("F6 returns to the editor · Escape after closing preview controls");
            }
            let old = (self.locale.clone(), self.theme.clone(), self.theme_id.clone(), self.interactive);
            if self.harness {
                ui.small("Keyboard, clipboard and focus controls are inside the diagnostic page.");
            } else if self.is_specimen() {
                ui.small("Registered default theme · light/dark presentation only");
            } else {
                egui::ComboBox::from_id_salt("workspace-theme")
                    .width(250.)
                    .selected_text(self.themes.iter().find(|t| t["id"] == self.theme_id).and_then(|t| t["name"].as_str()).unwrap_or("Renderer default theme"))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.theme_id, String::new(), "Renderer default theme");
                        for item in &self.themes {
                            if let (Some(id), Some(name)) = (item["id"].as_str(), item["name"].as_str()) {
                                ui.selectable_value(&mut self.theme_id, id.into(), name);
                            }
                        }
                    });
            }
            ui.horizontal_wrapped(|ui| {
                if !self.harness {
                    ui.selectable_value(&mut self.locale, "ar".into(), "Arabic · RTL");
                    ui.selectable_value(&mut self.locale, "en".into(), "English · LTR");
                    ui.selectable_value(&mut self.theme, "light".into(), "Light");
                    ui.selectable_value(&mut self.theme, "dark".into(), "Dark");
                    if self.resource.as_ref().is_none_or(|r| r["fixture"] != "cityos-lower-level") {
                        ui.checkbox(&mut self.interactive, "Interact with fixture")
                            .on_hover_text("Preview interactions stay local and do not edit the draft or invoke owner actions.");
                    }
                }
                if ui.button("Restart preview").clicked() {
                    self.host.reset();
                    self.request_preview(app);
                }
                #[cfg(feature = "open-url")]
                if ui.button("Companion browser").clicked() {
                    if let Some(url) = self.preview.as_ref().and_then(|p| p["url"].as_str()) {
                        let _ = open::that(url);
                    }
                }
            });
            if old != (self.locale.clone(), self.theme.clone(), self.theme_id.clone(), self.interactive) {
                self.current = false;
                self.pending = Some(Instant::now());
            }
            let rect = ui.available_rect_before_wrap();
            let viewport = (rect.width().round().clamp(240., 4096.) as u32, rect.height().round().clamp(240., 4096.) as u32);
            self.observe_preview_viewport(viewport);
            bounds = Some(rect);
        };
        if show_side_preview {
            let panel =
                egui::Panel::right("cityos-workspace-preview").default_size(preview_width).size_range(280.0..=1400.0).resizable(true).show(ui, &mut web_panel);
            if !ctx.input(|i| i.pointer.any_down()) {
                resized_preview = Some(panel.response.rect.width().clamp(280., 1400.));
            }
        }
        if show_full_preview {
            egui::CentralPanel::default().show(ui, &mut web_panel);
        } else {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.heading(if !self.validated {
                    "Selected design"
                } else if self.actual_preview() {
                    "Native canvas"
                } else {
                    "Protected composition structure"
                });
                if self.resource.as_ref().is_some_and(|resource| resource["fixture"] == "cityos-web-frame") {
                    ui.small("Material visual projection · frame and horizon interiors remain protected proxies.");
                }
                if let Some(notice) = self.native_theme_notice() {
                    ui.small(notice);
                }
                if self.has_native_scene() && !self.native_presentation_current(app) {
                    ui.small("Canvas presentation out of date · previous view retained");
                }
                if !self.validated {
                    if self.busy.is_some() {
                        crate::jobs::progress_marker(ui);
                    }
                    if self.is_specimen() && self.busy.is_none() {
                        ui.label("Preview and canvas availability");
                        if let Some(resource) = &self.resource {
                            ui.label(registered_availability_reason(resource));
                        }
                        ui.small("The selected definition is preserved. Your saved draft remains in its tab.");
                    } else {
                        ui.label(if self.busy.is_some() {
                            "Loading selected source…"
                        } else {
                            "Choose a design file to preview before opening it on canvas."
                        });
                    }
                    return;
                }
                let native_scene = self.has_native_scene();
                let picked = if self.preview_only {
                    self.temporary_canvas.as_mut().and_then(|canvas| H::draw_temporary_canvas(canvas, ui, native_scene, &mut self.native_fit))
                } else {
                    app.draw_canvas(ui, native_scene, &mut self.native_fit)
                };
                if let Some(node) = picked {
                    self.selected = node;
                }
            });
        }
        if (!show_side_preview && !show_full_preview) || !self.open {
            self.release_inactive_preview();
        }
        if self.coverage.open {
            self.host.hide();
            if self.coverage.show(&ctx, self.busy.is_some() || self.recovery_pending) {
                self.request_coverage(true);
            }
        } else if self.compare_open {
            self.host.hide();
            egui::Window::new("Semantic changes · local export").open(&mut self.compare_open).default_size([720., 520.]).show(&ctx, |ui| {
                if let Some(text) = &self.compare {
                    if ui.button("Copy export").clicked() {
                        ctx.copy_text(text.clone());
                    }
                    egui::ScrollArea::both().show(ui, |ui| {
                        ui.monospace(text);
                    });
                }
            });
        } else if egui::Popup::is_any_open(&ctx) {
            self.host.hide();
        } else if self.open && (show_side_preview || show_full_preview) && self.actual_preview() {
            if let (Some(rect), Some(url)) = (bounds, self.preview.as_ref().and_then(|p| p["url"].as_str())) {
                let activating = self.host.url.as_deref() != Some(url);
                if let Err(error) = self.host.show(frame, url, rect, ctx.pixels_per_point(), &ctx) {
                    self.error = Some(error);
                } else if activating && !self.harness {
                    self.current = false;
                    self.render_started = Some(Instant::now());
                    ctx.request_repaint();
                }
            }
        }
        if let Some(version) = &self.host.version {
            egui::Area::new("cityos-webview-version".into()).anchor(egui::Align2::RIGHT_BOTTOM, [-8., -4.]).show(&ctx, |ui| {
                ui.small(format!("WebView2 {version}"));
            });
        }
        if let Some(width) = resized_preview {
            self.layout.preview_width = width;
        }
        if old_layout != self.layout {
            self.save_layout();
        }
        if app.status().history_pending {
            self.dispatch_history_request(app);
            ctx.request_repaint();
        }
        // Scenario, viewport and command changes can be discovered while laying
        // out this frame. Schedule their bounded work after those observations.
        if let Some(delay) = self.progress_repaint_delay() {
            ctx.request_repaint_after(delay);
        }
    }
}
