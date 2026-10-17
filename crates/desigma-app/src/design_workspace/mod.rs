//! Bound CityOS workspace: native projection and actual web renderer share one semantic draft.
mod bridge;
mod host;
mod layout;
mod dependencies;
mod submission_recovery;
mod repaint;
use crate::app::App;
pub use bridge::Config;
#[cfg(test)]
use repaint::RENDER_ACK_TIMEOUT;
use desigma_doc::{CityosBase, CityosBinding, CityosOccurrence, Color, Command, Document, Node, Paint, Rect, TextStyle};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

struct DraftTab {
    document:Document,history:desigma_doc::History,path:Option<std::path::PathBuf>,file:Option<crate::home::LibFile>,
    autosave:crate::home::Autosave,page:desigma_doc::NodeId,resource:Option<Value>,locale:String,theme:String,theme_id:String,interactive:bool,selected:String,
}
#[derive(Clone, Copy, PartialEq)]
enum DependencyAction { Open, Resume, Reconcile, Accepted }

const SPECIMEN_CANVAS_REASON: &str = "Temporary registered defaults have no canonical document binding. Open on canvas and changes are unavailable.";

fn registered_availability_reason(resource: &Value) -> String {
    let reasons = resource["availability"]["preview"]["reasons"].as_array().into_iter().flatten()
        .filter_map(Value::as_str).map(|reason| match reason {
            "registered-fixture-props-unavailable" => "The component owner has not declared default fixture properties.",
            "owner-input-fixture-unavailable" => "This definition needs a declared owner-data fixture.",
            "registered-composition-inputs-required" => "Select an authored composition supplying its required slots.",
            "requires-registered-composition-root" => "Select the complete registered frame composition to inspect this member.",
            "renderer-adapter-unavailable" => "No actual-renderer adapter is registered for this definition.",
            "renderer-pins-unavailable" => "Install matching renderer dependencies to preview this definition.",
            "semantic-catalog-entry-unavailable" => "The registered semantic catalog entry is unavailable.",
            _ => reason,
        }).collect::<Vec<_>>();
    if reasons.is_empty() { "Select a canonical composition or an installed preview adapter for this definition.".into() }
    else { reasons.join(" ") }
}

#[derive(Clone)]
struct SpecimenRequest {
    locator: Value,
    scenario: Value,
    generation: u64,
}
impl SpecimenRequest {
    fn job(&self) -> Value {
        json!({"op":"FixturePreview","fixture":self.locator,"scenario":self.scenario})
    }
}

fn specimen_locator(resource: &Value) -> Option<Value> {
    if resource["kind"] != "registered-specimen" || resource["actualWebPreview"] != true
        || resource["editable"] != false || resource["provenance"]["canonicalResource"] != false {
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

fn semantic_change_label(change: &Value) -> String {
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
fn document_selection(document: &Document, requested: &str) -> String {
    let contains = |id: &str| document.nodes.values().any(|node| node.source_id.as_deref() == Some(id));
    if contains(requested) { return requested.into(); }
    draft(document).and_then(|value| value["semanticDraft"]["root"]["id"].as_str())
        .filter(|id| contains(id)).map(str::to_owned)
        .or_else(|| document.nodes.values().find_map(|node| node.source_id.clone())).unwrap_or_default()
}

fn resource_picker(ui: &mut egui::Ui, resources: &[Value], active: Option<&Value>, search: &str, busy: bool) -> Option<Value> {
    let search = search.to_lowercase();
    let rows: Vec<_> = resources.iter().filter(|resource| {
        search.is_empty() || format!("{} {} {} {} {}", resource["sourcePath"], resource["label"], resource["componentId"], resource["profileRef"], resource["targetProfileRef"])
            .to_lowercase().contains(&search)
    }).collect();
    ui.small(format!("{} designs", rows.len()));
    let mut selected = None;
    egui::ScrollArea::vertical().id_salt(("cityos-design-files", &search)).auto_shrink([false, false])
        .show_rows(ui, 68., rows.len(), |ui, range| {
            for index in range {
                let resource = rows[index];
                let path = resource["sourcePath"].as_str().unwrap_or("");
                let label = resource["label"].as_str().unwrap_or(path);
                let is_specimen = resource["kind"] == "registered-specimen";
                let row_selected = active.is_some_and(|current| if is_specimen {
                    specimen_locator(resource).is_some() && specimen_locator(current) == specimen_locator(resource)
                } else { current["kind"] != "registered-specimen" && current["sourcePath"] == path });
                let representation = if resource["actualWebPreview"] == true { "Actual web preview" }
                    else if is_specimen { "Preview unavailable" } else { "Authored structure" };
                let scope = if is_specimen { "Temporary defaults" } else { "Canonical file" };
                let detail = ["level", "target", "componentVersion"].into_iter().filter_map(|key|resource[key].as_str())
                    .filter(|value|!value.is_empty()).collect::<Vec<_>>().join(" · ");
                let detail = if detail.is_empty() { "Repository design".to_owned() } else { detail };
                let hint = format!("{}\n{}\n{}", path, resource["componentId"].as_str().unwrap_or(""),
                    resource["previewNotice"].as_str().unwrap_or("Select to preview before opening on canvas."));
                ui.add_enabled_ui(!busy && resource["openAvailable"] != false, |ui| {
                    ui.allocate_ui_with_layout(egui::vec2(ui.available_width(),68.),egui::Layout::top_down(egui::Align::Min),|ui|{
                        ui.spacing_mut().item_spacing.y=1.;
                        let response=ui.add_sized([ui.available_width(),30.],egui::Button::new(label).selected(row_selected).wrap_mode(egui::TextWrapMode::Truncate));
                        if response.on_hover_text(&hint).on_disabled_hover_text(&hint).clicked(){selected=Some(resource.clone());}
                        ui.add(egui::Label::new(egui::RichText::new(detail).small()).truncate());
                        ui.add(egui::Label::new(egui::RichText::new(format!("{representation} · {scope}")).small()).truncate());
                    });
                });
            }
        });
    selected
}

fn semantic_change_value_text(value: Option<&Value>, document: Option<&Value>) -> String {
    let Some(value) = value else { return "Unavailable (value not provided)".into(); };
    let reference = value.get("resource").is_some()
        && (value.get("nodeId").is_some() || value.get("pointer").is_some());
    let resolved = if reference {
        let resolved = document.and_then(|document| {
            let resource = value.get("resource")?;
            let id = resource.get("id")?.as_str()?;
            let revision = resource.get("revision")?.as_str()?;
            if document.get("id")?.as_str()? != id || document.get("revision")?.as_str()? != revision { return None; }
            document.pointer(value.get("pointer")?.as_str()?)
        });
        let Some(resolved) = resolved else { return format!("Unavailable (unresolved reference: {value})"); };
        resolved
    } else { value };
    resolved.as_str().map(str::to_owned).unwrap_or_else(|| resolved.to_string())
}

pub struct Workspace {
    pub open: bool,
    config: Option<Config>,
    bridge: Option<bridge::Bridge>,
    host: host::Host,
    busy: Option<String>,
    staged: Option<Document>,
    preview_only: bool,
    error: Option<String>,
    preview: Option<Value>,
    current: bool,
    render_started: Option<Instant>,
    locale: String,
    theme: String,
    theme_id: String,
    themes: Vec<Value>,
    themes_loaded: bool,
    selected: String,
    observed: Option<(String, u64)>,
    pending: Option<Instant>,
    compare: Option<String>,
    compare_open: bool,
    validated: bool,
    harness: bool,
    viewport: (u32, u32),
    panel_size: (u32, u32),
    resources: Vec<Value>,
    resource: Option<Value>,
    resource_diagnostics: Vec<Value>,
    picker_open: bool,
    resource_search: String,
    requested_source: Option<String>,
    open_after_preview: bool,
    temporary_canvas: Option<Box<App>>,
    tabs:BTreeMap<String,DraftTab>,
    changes_mode:bool,
    incoming:Option<Value>,
    inline_identity:Option<String>,
    requested_pin:Option<Value>,
    layout: layout::Layout,
    layout_loaded: bool,
    narrow_preview: bool,
    leave_to_library: bool,
    dependency_action: Option<DependencyAction>,
    dependency_draft: Option<Value>,
    reconciliation_bundle: Option<Value>,
    resolutions: BTreeMap<String, String>,
    recovery_pending: bool,
    interactive: bool,
    property_text: BTreeMap<String,String>,
    resume_document: Option<Document>,
    specimen_request: Option<SpecimenRequest>,
    local_specimen_request: Option<SpecimenRequest>,
    preview_generation: u64,
    submissions: submission_recovery::Recovery,
    accepted_bundle: Option<Value>,
}
impl Default for Workspace {
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
            requested_source: None,
            open_after_preview: false,
            temporary_canvas: None,
            tabs:BTreeMap::new(),
            changes_mode:false,
            incoming:None,
            inline_identity:None,
            requested_pin:None,
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
        }
    }
}

fn projection_document(projection: &Value, name: &str) -> Result<(Document,BTreeMap<String,CityosOccurrence>),String> {
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
fn binding(draft: Value, projection: &Value) -> Result<Document, String> {
    if !matches!(draft["schemaVersion"].as_str(),Some("cityos.design-exchange.v2" | "cityos.design-exchange.v3"))
        || !matches!(projection["schemaVersion"].as_str(), Some("cityos.desigma-canvas.v1" | "cityos.desigma-structure.v1"))
        || projection["draftId"] != draft["draftId"] || projection["draftRevision"] != draft["draftRevision"] || projection["semanticDigest"] != draft["semanticDigest"] {
        return Err("Projection/draft identity mismatch".into());
    }
    let string = |v: &Value, key: &str| v[key].as_str().map(str::to_owned).ok_or_else(|| format!("Missing {key}"));
    let (mut doc,occurrences)=projection_document(projection,draft["semanticDraft"]["id"].as_str().unwrap_or("CityOS design"))?;
    let pins = draft["dependencyPins"].as_array().ok_or("Missing dependency pins")?.iter().map(|p| string(p, "digest")).collect::<Result<Vec<_>, _>>()?;
    doc.cityos = Some(CityosBinding {
        schema_version: string(&draft,"schemaVersion")?,
        document_id: string(&draft, "documentId")?,
        draft_id: string(&draft, "draftId")?,
        base: CityosBase::CanonicalGit { repository_id: string(&draft["base"], "repositoryId")?, commit: string(&draft["base"], "commit")? },
        semantic_base: draft["semanticBase"].clone(),
        dependency_pins: pins,
        occurrences,
        exchange_v2: if draft["schemaVersion"]=="cityos.design-exchange.v2" {Some(draft.clone())} else {None},
        exchange_v3: if draft["schemaVersion"]=="cityos.design-exchange.v3" {Some(draft)} else {None},
    });
    doc.cityos.as_ref().unwrap().validate()?;
    Ok(doc)
}
fn draft(doc: &Document) -> Option<&Value> {
    doc.cityos.as_ref()?.exchange()
}
fn revision(doc: &Document) -> Option<(String, u64)> {
    let d = draft(doc)?;
    Some((d["draftId"].as_str()?.into(), d["draftRevision"].as_u64()?))
}

fn local_copy(document: &Document) -> Document {
    let mut copy = document.clone();
    copy.cityos = None;
    copy.name = format!("{} · local copy", copy.name);
    for node in copy.nodes.values_mut() { node.locked = false; node.source_id = None; }
    copy
}

/// Headless delivery check through the same native document and trusted bridge paths.
pub fn verify_roundtrip(config: Config, directory: &std::path::Path) -> Result<Value, String> {
    let bundle: Value =
        serde_json::from_slice(&std::fs::read(directory.join("canonical-bundle.json")).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let bridge = bridge::Bridge::spawn(config);
    let mut document = binding(bundle["draft"].clone(), &bundle["projection"])?;
    let path = crate::io::save_path(&document, &directory.join("native-flat.desigma"))?;
    document = crate::io::load_path(&path)?;
    let ask = |request: Value| -> Result<Value, String> {
        bridge.jobs.send(request).map_err(|e| e.to_string())?;
        bridge.replies.recv_timeout(Duration::from_secs(30)).map_err(|e| e.to_string())?
    };
    if ask(json!({"op":"Export","draft":draft(&document)}))?["operationCount"] != 0 {
        return Err("Initial roundtrip manufactured edits".into());
    }
    let d = draft(&document).ok_or("Missing binding")?.clone();
    let result = ask(
        json!({"op":"Command","draft":d,"command":{"draftId":d["draftId"],"expectedRevision":d["draftRevision"],"operationId":"native-roundtrip","operation":"set-property","nodeId":"material","property":"elevation","value":"resting"}}),
    )?;
    let mut history = desigma_doc::History::default();
    history
        .exec(&mut document, Command::CityosInstall { document: Box::new(binding(result["draft"].clone(), &result["projection"])?) })
        .map_err(|e| e.to_string())?;
    let path = crate::io::save_path(&document, &directory.join("native-resting.desigma"))?;
    let reopened = crate::io::load_path(&path)?;
    let exported = ask(json!({"op":"Export","draft":draft(&reopened)}))?;
    if exported["operationCount"] != 1
        || exported["changeset"]["changes"][0]["field"] != "elevation"
        || draft(&reopened).unwrap()["semanticDraft"]["root"]["slots"]["horizon"] != bundle["draft"]["semanticBase"]["root"]["slots"]["horizon"]
    {
        return Err("Unexpected semantic export".into());
    }
    history.undo(&mut document).map_err(|e| e.to_string())?;
    if ask(json!({"op":"Export","draft":draft(&document)}))?["operationCount"] != 0 {
        return Err("Undo manufactured edits".into());
    }
    Ok(
        json!({"nativeRoundtrip":true,"unchangedOperationCount":0,"editedOperationCount":1,"undoOperationCount":0,"canonicalBase":bundle["draft"]["base"],"embeddedRendererObserved":false}),
    )
}
impl Workspace {
    pub fn set_preview_theme(&mut self, id: String) {
        self.theme_id = if self.is_specimen() { String::new() } else { id };
        self.current = false;
        self.pending = Some(Instant::now());
    }
    pub fn begin(&mut self, config: Config, source: Option<String>, preview_only: bool) {
        self.begin_request(config,source,preview_only,None);
    }
    pub fn begin_pinned_preview(&mut self, config: Config, resource: Value) {
        let source = resource["sourcePath"].as_str().map(str::to_owned);
        self.begin_request(config, source, true, Some(resource));
    }
    fn begin_request(&mut self, config: Config, source: Option<String>, preview_only: bool,pin:Option<Value>) {
        self.host.configure_storage(&config.cache);
        self.invalidate_specimen_request();
        self.resume_document=None;
        self.property_text.clear();
        self.interactive=false;
        self.dependency_action=None; self.dependency_draft=None; self.reconciliation_bundle=None; self.resolutions.clear();
        self.leave_to_library = true;
        self.load_layout(&config);
        self.inline_identity=None;self.requested_pin=None;self.changes_mode=false;
        self.host.reset();
        self.bridge = Some(bridge::Bridge::spawn(config.clone()));
        self.config = Some(config);
        self.open = true;
        self.preview_only = true;
        self.open_after_preview = !preview_only;
        self.requested_source = source;
        self.picker_open = self.requested_source.is_none();
        self.resources.clear();
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
        if let Some(pin)=pin {
            self.requested_source=None;
            if pin["kind"] == "registered-specimen" { self.open_resource(pin); }
            else { self.send(json!({"op":"Open","resource":pin})); }
        }
        else{self.send(json!({"op":"Resources"}));}
    }
    fn open_resource(&mut self, resource: Value) {
        if self.busy.is_some() { return; }
        self.prepare_resource(resource.clone());
        if self.is_specimen() {
            self.open_after_preview = false;
            self.theme_id.clear();
            self.themes_loaded = true;
            self.validated = specimen_locator(&resource).is_some();
            if self.validated { self.request_specimen_preview(); }
            else { self.error = Some(registered_availability_reason(&resource)); }
        } else {
            self.send(json!({"op":"Open","resource":resource}));
        }
    }
    fn invalidate_specimen_request(&mut self) {
        self.preview_generation = self.preview_generation.wrapping_add(1);
        self.specimen_request = None;
        self.local_specimen_request = None;
    }
    fn is_specimen(&self) -> bool {
        self.resource.as_ref().is_some_and(|r| r["kind"] == "registered-specimen")
    }
    fn prepare_resource(&mut self, resource: Value) {
        self.invalidate_specimen_request();
        self.harness = false;
        self.error = None;
        self.property_text.clear();
        self.dependency_action = None;
        self.dependency_draft = None;
        self.reconciliation_bundle = None;
        self.resume_document = None;
        self.changes_mode=false;self.incoming=None;
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
    pub fn show_inline(&mut self,app:&mut App,frame:&eframe::Frame,config:Config,pin:Value,rect:egui::Rect,scale:f32,ctx:&egui::Context)->String {
        let key=serde_json::to_string(&pin).unwrap_or_default();
        if self.inline_identity.as_deref()!=Some(&key){
            let source=pin["sourcePath"].as_str().map(str::to_owned);
            if source.is_none() && pin["kind"] != "registered-specimen" { return "Preview source unavailable".into(); }
            self.begin_request(config,source,true,Some(pin));self.open=false;self.inline_identity=Some(key);
        }
        self.poll(app);
        if let Some(error)=&self.error{self.host.hide();return error.clone();}
        if let Some(url)=self.preview.as_ref().and_then(|p|p["url"].as_str()){
            if let Err(error)=self.host.show(frame,url,rect,scale,ctx){self.render_started=None;self.pending=None;self.error=Some(error.clone());return error;}
        }
        if self.current{format!("Current · revision {}",self.preview.as_ref().map(|p|p["draftRevision"].clone()).unwrap_or(Value::Null))}
        else if let Some(operation)=&self.busy{format!("{operation}…")}else{"Preparing actual preview…".into()}
    }
    pub fn inline_pending(&self)->bool {self.error.is_none()&&(self.busy.is_some()||self.pending.is_some()||self.render_started.is_some())}
    fn actual_preview(&self) -> bool {
        self.harness || self.resource.as_ref().is_some_and(|r| r["actualWebPreview"] == true)
    }
    fn save_before_switch(app:&mut App)->Result<(),String>{
        if app.home.is_some(){return Ok(());}
        if app.lib_file.is_some(){return app.save_library_file();}
        if let Some(path)=&app.file_path{return crate::io::save_path(&app.doc,path).map(|_|());}
        if app.doc.cityos.is_some(){
            return Self::save_to_draft_library(app);
        }
        if app.hist.can_undo(){return Err("Save your current design before opening a bound draft.".into());}Ok(())
    }
    fn open_desigma_files(&mut self, app: &mut App) -> Result<(), String> {
        if self.recovery_pending {
            return Err("Recover the pending document generation before leaving this draft.".into());
        }
        if self.busy.is_some() {
            return Err("Wait for the current CityOS operation before opening Desigma files.".into());
        }
        if app.home.is_none() {
            if app.lib.is_none() { return Err("The Desigma file library is unavailable. Save this draft before leaving.".into()); }
            Self::save_before_switch(app)?;
            app.go_home();
            if app.home.is_none() { return Err("Could not open Desigma files. The current draft is still open.".into()); }
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
    fn activate_resumed_document(app: &mut App, document: Document) {
        if !app.cityos_submission_frozen() { app.doc = document; app.changed(); }
        // Returning from the ordinary file library must restore editor state.
        // Leaving home active would make later save-before-switch checks skip
        // the bound document even after the projection is editable.
        app.home = None;
    }
    fn install_resumed_projection(&mut self, app: &mut App, document: Document) -> bool {
        if !self.require_dependencies(app, &document, DependencyAction::Resume) {
            self.resume_document = Some(document);
            self.validated = false;
            return false;
        }
        Self::activate_resumed_document(app, document);
        true
    }
    fn save_to_draft_library(app: &mut App) -> Result<(), String> {
        if app.lib_file.is_some() { return app.save_library_file(); }
        let lib=app.lib.clone().ok_or("The library is unavailable. Save this draft before switching.")?;
        let project=lib.list_projects().into_iter().find(|p|p.name=="CityOS drafts").map(Ok).unwrap_or_else(||lib.create_project("CityOS drafts")).map_err(|e|e.to_string())?;
        let file=lib.create_file_with_history(&project.id,None,&app.doc.name,&app.doc,&app.hist).map_err(|e|e.to_string())?;
        app.lib_file=Some(crate::home::LibFile{id:file.id,project_id:project.id,project:project.name,name:file.name});
        app.file_path=None;
        app.autosave=crate::home::Autosave{seen_rev:app.canvas.tiles.rev(),saved_hash:crate::home::doc_hash(&app.doc),..Default::default()};
        Ok(())
    }
    fn suspend_current(&mut self,app:&mut App)->Result<(),String>{
        let id=draft(&app.doc).and_then(|d|d["draftId"].as_str()).map(str::to_owned);
        if id.is_some()&&self.tabs.len()>=3{return Err("Four drafts are already loaded. Save and close an inactive tab before opening another.".into());}
        Self::save_before_switch(app)?;
        if let Some(id)=id{self.tabs.insert(id,DraftTab{document:app.doc.clone(),history:std::mem::take(&mut app.hist),path:app.file_path.clone(),file:app.lib_file.clone(),
            autosave:std::mem::take(&mut app.autosave),page:app.page,resource:self.resource.clone(),locale:self.locale.clone(),theme:self.theme.clone(),theme_id:self.theme_id.clone(),interactive:self.interactive,selected:self.selected.clone()});}
        Ok(())
    }
    fn activate_tab(&mut self,app:&mut App,id:&str){
        let Some(tab)=self.tabs.remove(id) else{return;};
        if let Err(error)=self.suspend_current(app){self.tabs.insert(id.into(),tab);self.error=Some(error);return;}
        if !app.replace_document(tab.document,tab.path){self.error=Some("Current draft could not be saved; tab switch cancelled".into());return;}app.hist=tab.history;app.lib_file=tab.file;app.autosave=tab.autosave;app.page=tab.page;app.home=None;
        self.resource=tab.resource;self.locale=tab.locale;self.theme=tab.theme;self.theme_id=tab.theme_id;
        self.interactive=tab.interactive;
        let selected=document_selection(&app.doc,&tab.selected); self.select(app,&selected);
        self.changes_mode=false;self.incoming=None;
        if let Some(config)=self.config.clone(){self.resume(config,app);}
    }
    fn install_staged(&mut self, app: &mut App) {
        if self.is_specimen() { self.error = Some(SPECIMEN_CANVAS_REASON.into()); return; }
        if self.staged.is_none() {
            return;
        }
        if self.resource.as_ref().is_some_and(|r| r["references"].as_u64().unwrap_or(0) > 0 && r["referenceClosure"]["complete"]!=true) {
            self.error = Some("Reusable dependencies are unresolved. This file remains a temporary structure preview.".into());
            return;
        }
        let candidate = self.staged.as_ref().unwrap().clone();
        if !self.require_dependencies(app, &candidate, DependencyAction::Open) { return; }
        // Previewing another file never replaces the editor document. An explicit
        // open must preserve the current document, including failed saves.
        if let Err(error)=self.suspend_current(app){self.error=Some(error);return;}
        let doc = self.staged.as_ref().unwrap().clone();
        if !app.replace_document(doc, None){self.error=Some("Current draft could not be saved; open cancelled".into());return;}
        self.staged=None;
        let selected=document_selection(&app.doc,""); self.select(app,&selected);
        app.home = None;
        self.preview_only = false;
        self.changes_mode=false;self.incoming=None;
        self.temporary_canvas = None;
        self.observed = None;
        self.error = None;
        if let Err(error)=Self::save_before_switch(app) { self.error=Some(error); }
    }
    fn create_local_copy(&mut self, app: &mut App) {
        if self.is_specimen() {
            if self.busy.is_some() || !self.validated { return; }
            let Some(request)=self.captured_specimen_request() else { return; };
            self.send(json!({"op":"Fixtures","fixture":request.locator,"locale":self.locale}));
            if self.busy.as_deref()==Some("Fixtures") { self.local_specimen_request=Some(request); }
            return;
        }
        let source = if self.preview_only { self.staged.as_ref() } else { Some(&app.doc) };
        let Some(document) = source.map(local_copy) else { return; };
        self.open_local_document(app,document);
    }
    fn open_local_document(&mut self, app: &mut App, document: Document) {
        if let Err(error) = self.suspend_current(app) { self.error = Some(error); return; }
        if !app.replace_document(document, None) { self.error = Some("Save failed; local copy was not opened".into()); return; }
        app.home = None;
        self.host.reset();
        self.bridge = None;
        self.leave_to_library = false;
        self.open = false;
        app.show_toast("Local Desigma copy · no CityOS submission binding", false);
    }
    fn local_specimen_document(&mut self, value: Value) -> Result<Document,String> {
        let request=self.local_specimen_request.take().ok_or("No explicit local-copy request is pending")?;
        let selected=self.resource.as_ref().ok_or("Selected registered definition unavailable")?;
        let resource=&value["resource"];
        if !self.preview_only || !self.is_specimen() || request.generation!=self.preview_generation
            || specimen_locator(selected).as_ref()!=Some(&request.locator) || specimen_locator(resource).as_ref()!=Some(&request.locator)
            || request.scenario["locale"]!=self.locale || value["locale"]!=self.locale || value["state"]!="projected"
            || value["ownerWrites"]!=false || value["canonicalResource"]!=false || value.get("draft").is_some()
            || value["provenance"]["temporary"]!=true || value["provenance"]["canonicalResource"]!=false
            || value["provenance"]["sourceCommit"]!=selected["provenance"]["sourceCommit"]
            || value["provenance"]["locale"]!=self.locale || resource["provenance"]!=value["provenance"]
            || value["draftRevision"]!=0 || value["draftId"]!=format!("registered-fixture:{}",resource["resourceId"].as_str().unwrap_or(""))
            || value["projection"]["schemaVersion"]!="cityos.desigma-canvas.v1"
            || value["projection"]["draftId"]!=value["draftId"] || value["projection"]["draftRevision"]!=0
            || value["projection"]["semanticDigest"]!=value["semanticDigest"]
            || value["projection"]["nodes"].as_array().is_none_or(|nodes|nodes.is_empty()) {
            return Err("Obsolete or substituted registered projection discarded; the current document was preserved".into());
        }
        let name=format!("{} · inspection",selected["label"].as_str().unwrap_or("Registered default"));
        let (document,_)=projection_document(&value["projection"],&name)?;
        Ok(local_copy(&document))
    }
    pub fn begin_harness(&mut self, config: Config) {
        self.host.configure_storage(&config.cache);
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
    pub fn resume(&mut self, config: Config, app: &App) {
        self.selected=document_selection(&app.doc,&self.selected);
        self.host.configure_storage(&config.cache);
        self.invalidate_specimen_request();
        self.resume_document=None;
        self.property_text.clear();
        self.dependency_action=None; self.dependency_draft=None; self.reconciliation_bundle=None; self.resolutions.clear();
        self.leave_to_library = true;
        self.load_layout(&config);
        self.inline_identity=None;self.requested_pin=None;
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
        if let Some(draft) = draft(&app.doc) {
            self.send(json!({"op":"Project","draft":draft}));
        }
    }
    pub fn hide(&self) {
        self.host.hide();
    }
    pub fn library_requested(&self) -> bool { self.leave_to_library }
    fn require_dependencies(&mut self, app: &App, document: &Document, action: DependencyAction) -> bool {
        let Some(library) = &app.lib else { self.error=Some("The dependency library is unavailable; this draft remains read-only".into()); return false; };
        if dependencies::complete(library, document) {
            if action==DependencyAction::Resume {if let Some(file)=&app.lib_file {if let Err(error)=library.load_checkpoint(&file.id){
                self.error=Some(format!("Retained history dependencies are unavailable; restore a portable checkpoint before editing: {error}"));return false;
            }}}
            return true;
        }
        let Some(draft) = draft(document).cloned() else { self.error=Some("Missing bound draft".into()); return false; };
        self.dependency_action=Some(action); self.dependency_draft=Some(draft.clone());
        self.send(json!({"op":"Dependencies","draft":draft}));
        false
    }
    fn install_reconciliation(&mut self, app: &mut App, value: Value) {
        let install = (|| -> Result<(), String> {
            if value["historyBoundary"] != true { return Err("Reconciliation did not establish a validated baseline boundary".into()); }
            let mut document = binding(value["draft"].clone(), &value["projection"])?;
            document.name = app.doc.name.clone();
            let previous = revision(&app.doc).ok_or("Current draft is unavailable")?;
            let next = revision(&document).ok_or("Incoming draft is unavailable")?;
            if previous.0 != next.0 || next.1 != previous.1 + 1 { return Err("Obsolete reconciliation discarded".into()); }
            if !self.require_dependencies(app, &document, DependencyAction::Reconcile) {
                self.reconciliation_bundle=Some(value.clone());
                return Ok(());
            }
            let library = app.lib.clone().ok_or("The draft library is unavailable")?;
            let file = app.lib_file.clone().ok_or("Save this draft to the library before applying incoming changes")?;
            // Archive the current undo stack, then persist the complete new state
            // before it becomes visible. A failed save leaves the old draft active.
            let mut checkpoint = library.load_checkpoint(&file.id).map_err(|e| e.to_string())?;
            checkpoint.document = app.doc.clone(); checkpoint.history = app.hist.clone();
            checkpoint.install_baseline(document.clone());
            let expected_checkpoint = checkpoint.to_json().map_err(|e|e.to_string())?;
            if let Err(error)=library.save_checkpoint(&file.id, &checkpoint) {
                // A durable journal can have committed despite an I/O error.
                // Recover/read back before either installing or retrying old state.
                match library.load_checkpoint_for_recovery(&file.id) {
                    Ok((recovered,missing)) if missing.is_empty() && recovered.to_json().is_ok_and(|text|text==expected_checkpoint) => {},
                    Ok((recovered,_)) if recovered.document.to_json().ok()==app.doc.to_json().ok() => return Err(format!("Incoming changes were not installed; current draft retained: {error}")),
                    _ => {
                        self.recovery_pending=true; self.validated=false;
                        self.reconciliation_bundle=Some(value.clone());
                        return Err("Document generation outcome is uncertain. Recover it before editing, saving or leaving.".into());
                    }
                }
            }
            app.doc = document; app.hist = checkpoint.history;
            app.history.reset(); app.changed();
            app.autosave.saved_hash=crate::home::doc_hash(&app.doc);
            app.autosave.changed_at=None;
            self.resource=Some(value["resource"].clone()); self.incoming=None; self.resolutions.clear();
            self.compare=None; self.current=false; self.observed=None; self.validated=true;
            self.pending=Some(Instant::now());
            self.recovery_pending=false; self.reconciliation_bundle=None;
            app.show_toast("Reviewed incoming changes saved. Earlier baseline history is retained for inspection.",false);
            Ok(())
        })();
        if let Err(error)=install { self.error=Some(error); self.bridge=None; self.busy=None; }
    }
    fn recover_reconciliation(&mut self, app: &mut App) {
        let result=(|| -> Result<(),String> {
            let expected=self.reconciliation_bundle.as_ref().ok_or("Missing recovery identity")?;
            let library=app.lib.as_ref().ok_or("Draft library unavailable")?;
            let file=app.lib_file.as_ref().ok_or("Draft file unavailable")?;
            let(checkpoint,missing)=library.load_checkpoint_for_recovery(&file.id).map_err(|e|e.to_string())?;
            if !missing.is_empty(){return Err("Recovery dependencies remain unavailable".into());}
            let recovered=draft(&checkpoint.document).ok_or("Recovery binding unavailable")?;
            if ["draftId","draftRevision","semanticDigest","base"].iter().all(|key|recovered[*key]==expected["draft"][*key]) {
                app.doc=checkpoint.document; app.hist=checkpoint.history; app.history.reset(); app.changed();
                app.autosave.saved_hash=crate::home::doc_hash(&app.doc); app.autosave.changed_at=None;
                self.resource=Some(expected["resource"].clone()); self.incoming=None;
            } else if checkpoint.document.to_json().ok()!=app.doc.to_json().ok(){return Err("Recovered generation differs from both known states; keep the draft frozen for inspection".into());}
            self.recovery_pending=false; self.reconciliation_bundle=None; self.validated=true;
            self.current=false;self.observed=None;self.pending=Some(Instant::now());self.error=None;
            Ok(())
        })();
        if let Err(error)=result{self.error=Some(error);}
    }
    fn recover_accepted(&mut self, app: &mut App) {
        if self.busy.is_some() { return; }
        let Some(record) = self.submissions.accepted().cloned() else { return; };
        self.submissions.requested = Some(record.command.operation_id.clone());
        match self.submissions.recover_existing(app, &record) {
            Ok(true) => {
                self.accepted_bundle=None; self.recovery_pending=false; self.validated=false;
                self.current=false; self.observed=None; self.error=None;
                if let Some(draft)=draft(&app.doc) { self.send(json!({"op":"Project","draft":draft})); }
            }
            Ok(false) => if let Some(draft)=self.active_draft(app) {
                self.send(json!({"op":"AcceptedProjection","draft":draft,"submission":record}));
            },
            Err(error) => { self.error=Some(error); self.recovery_pending=true; },
        }
    }
    fn install_accepted(&mut self, app: &mut App, value: Value) {
        let result=(|| -> Result<(),String> {
            let record=self.submissions.accepted().cloned().ok_or("Original accepted submission is unavailable")?;
            if value["operationId"]!=record.command.operation_id || value["requestDigest"]!=record.request_digest
                || value["authority"]!="projection-only" || value["ownerWrites"]!=false || value["historyBoundary"]!=true || value["operationCount"]!=0 {
                return Err("Accepted projection does not match the retained original operation".into());
            }
            let mut document=binding(value["draft"].clone(),&value["projection"])?;
            document.name=app.doc.name.clone();
            let previous=revision(&app.doc).ok_or("Original submitted draft unavailable")?;
            let next=revision(&document).ok_or("Accepted draft unavailable")?;
            if previous.0!=next.0 || next.1!=previous.1+1 { return Err("Obsolete accepted projection discarded".into()); }
            self.accepted_bundle=Some(value.clone());
            if !self.require_dependencies(app,&document,DependencyAction::Accepted) { return Ok(()); }
            self.submissions.install(app,&record,document)?;
            self.accepted_bundle=None;self.recovery_pending=false;self.validated=true;
            self.resource=Some(value["resource"].clone());self.incoming=None;self.resolutions.clear();
            self.compare=None;self.current=false;self.observed=None;self.pending=Some(Instant::now());self.error=None;
            app.show_toast("Accepted owner state synchronized and saved. Earlier baseline history is retained for inspection.",false);
            Ok(())
        })();
        if let Err(error)=result {
            self.error=Some(error);self.recovery_pending=true;self.validated=false;
            self.bridge=None;self.busy=None;
        }
    }
    fn load_layout(&mut self, config: &Config) {
        if self.layout_loaded { return; }
        match layout::Layout::load(&config.cache) {
            Ok(value) => self.layout = value,
            Err(error) => self.error = Some(error),
        }
        self.layout_loaded = true;
    }
    fn save_layout(&mut self) {
        if let Some(config) = &self.config {
            if let Err(error) = self.layout.save(&config.cache) { self.error = Some(error); }
        }
    }
    fn send(&mut self, value: Value) {
        if self.busy.is_some() {
            return;
        }
        if (self.submissions.error.is_some() || self.submissions.records.iter().any(desigma_library::SubmissionRecord::frozen))
            && matches!(value["op"].as_str(),Some("Command"|"Reconcile")) {
            self.error=Some("The immutable submitted draft stays frozen until its original owner outcome is resolved.".into());return;
        }
        if self.is_specimen() && !matches!(value["op"].as_str(), Some("FixturePreview" | "Fixtures" | "Resources")) {
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
        self.busy = value["op"].as_str().map(str::to_owned);
        if bridge.jobs.try_send(value).is_err() {
            self.busy = None;
            self.error = Some("Exchange helper unavailable".into());
        }
    }
    fn active_draft(&self, app: &App) -> Option<Value> {
        if self.is_specimen() { return None; }
        if self.preview_only {
            self.staged.as_ref().and_then(draft).cloned()
        } else {
            draft(&app.doc).cloned()
        }
    }
    fn request_preview(&mut self, app: &App) {
        if self.harness { self.send(json!({"op":"Harness"})); return; }
        if self.is_specimen() { self.request_specimen_preview(); return; }
        if let Some(draft) = self.active_draft(app) {
            self.current = false;
            self.send(json!({"op":"Preview","draft":draft,"scenario":self.scenario()}));
        }
    }
    fn captured_specimen_request(&self) -> Option<SpecimenRequest> {
        Some(SpecimenRequest { locator: specimen_locator(self.resource.as_ref()?)?,
            scenario: self.scenario(), generation: self.preview_generation })
    }
    fn request_specimen_preview(&mut self) {
        if self.busy.is_some() { self.current = false; self.pending = Some(Instant::now()); return; }
        self.invalidate_specimen_request();
        let Some(request) = self.captured_specimen_request() else { return; };
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
    fn accept_specimen_preview(&mut self, value: Value, operation: &str) -> bool {
        // The bridge checks the outer session/request envelope before returning.
        // Capture selection and scenario independently so even a correctly routed
        // reply cannot substitute another resource or an obsolete presentation.
        if operation != "FixturePreview" || self.busy.is_some() { return false; }
        let Some(request) = self.specimen_request.take() else { return false; };
        let Some(selected) = self.resource.as_ref() else { return false; };
        let resource = &value["resource"];
        let identity = ["sessionId", "requestId", "semanticDigest"].iter()
            .all(|key| value[*key].as_str().is_some_and(|s| !s.is_empty()));
        let matches = self.preview_only && self.is_specimen() && self.staged.is_none() && self.temporary_canvas.is_none()
            && request.generation == self.preview_generation && request.scenario == self.scenario()
            && specimen_locator(selected).as_ref() == Some(&request.locator)
            && specimen_locator(resource).as_ref() == Some(&request.locator)
            && resource["fixture"] == selected["fixture"] && value["scenario"] == request.scenario
            && value["canonicalResource"] == false && value["ownerWrites"] == false && value.get("draft").is_none()
            && value["provenance"]["canonicalResource"] == false && value["provenance"]["temporary"] == true
            && value["provenance"]["locale"] == self.locale && resource["provenance"] == value["provenance"]
            && value["provenance"]["sourceCommit"] == selected["provenance"]["sourceCommit"]
            && resource["resourceId"].as_str().is_some_and(|id| value["draftId"] == format!("registered-fixture:{id}"))
            && value["draftRevision"] == 0 && value["state"] == "prepared" && identity;
        if !matches { return false; }
        self.preview = Some(value);
        self.validated = true;
        self.current = false;
        self.render_started = Some(Instant::now());
        true
    }
    fn settle_specimen_preview(&mut self, value: Value) {
        // Panel reflow and scenario controls may change while the helper works.
        // Retire that response quietly and coalesce one replacement. Showing an
        // error here changes the panel height and can cause a resize/retry loop.
        let superseded = self.specimen_request.as_ref().is_some_and(|request| {
            self.preview_only && self.is_specimen()
                && request.generation == self.preview_generation
                && self.resource.as_ref().and_then(specimen_locator).as_ref() == Some(&request.locator)
                && request.scenario != self.scenario()
        });
        if self.accept_specimen_preview(value, "FixturePreview") { return; }
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
    fn matches_preview_event(&self, app: &App, event: &Value) -> bool {
        let receipt_matches = self.preview.as_ref().is_some_and(|p| {
            ["sessionId", "requestId", "draftId", "draftRevision", "semanticDigest", "scenario"].iter().all(|key| p[*key] == event[*key])
        }) && event["scenario"] == self.scenario();
        if self.is_specimen() {
            receipt_matches && self.preview_only && self.specimen_request.is_none() && self.busy.as_deref() != Some("FixturePreview")
                && self.preview.as_ref().is_some_and(|p| p["canonicalResource"] == false
                    && specimen_locator(&p["resource"]) == self.resource.as_ref().and_then(specimen_locator))
        } else {
            receipt_matches && self.active_draft(app).is_some_and(|d| d["draftId"] == event["draftId"]
                && d["draftRevision"] == event["draftRevision"] && d["semanticDigest"] == event["semanticDigest"])
        }
    }
    fn scenario(&self) -> Value {
        json!({"target":"web","locale":self.locale,"theme":self.theme,"themeId":self.theme_id,
        "viewport":{"width":self.viewport.0,"height":self.viewport.1},"fixture":self.resource.as_ref().and_then(|r|r["fixture"].as_str()).unwrap_or("cityos-web-frame"),
        "renderMode":if self.interactive && self.resource.as_ref().is_none_or(|r|r["fixture"]!="cityos-lower-level"){"interactive-fixture"}else{"studio-passive"},"motion":"reduce"})
    }
    fn poll(&mut self, app: &mut App) {
        let result = self.bridge.as_ref().and_then(|b| b.replies.try_recv().ok());
        if let Some(result) = result {
            let op = self.busy.take().unwrap_or_default();
            match result {
                Err(error) => {
                    if op == "FixturePreview" { self.specimen_request = None; }
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
                                if let Some(resource) = self.resources.iter().find(|r| r["sourcePath"] == source && r["openAvailable"]!=false).cloned() {
                                    let resource=self.requested_pin.take().unwrap_or(resource);
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
                          if op=="Command" && app.cityos_submission_frozen() {
                              self.error=Some("A delayed edit cannot replace the frozen submitted draft.".into());return;
                          }
                          match binding(value["draft"].clone(), &value["projection"]) {
                            Err(e) => self.error = Some(e),
                            Ok(mut document) => {
                                self.property_text.clear();
                                if op != "Open" && !self.preview_only { document.name=app.doc.name.clone(); }
                                self.incoming=None;
                                self.resource = Some(value["resource"].clone());
                                if self.preview_only {
                                    self.temporary_canvas = Some(Box::new(App::new(document.clone())));
                                    self.staged = Some(document);
                                    if self.open_after_preview {
                                        self.open_after_preview = false;
                                        self.install_staged(app);
                                    }
                                } else if op == "Open" {
                                    if !app.replace_document(document, None){self.error=Some("Current draft could not be saved; open cancelled".into());return;}
                                    app.home = None;
                                } else if op == "Command" {
                                    let previous = revision(&app.doc);
                                    let next = revision(&document);
                                    if !previous.zip(next).is_some_and(|((id, rev), (next_id, next_rev))| id == next_id && next_rev == rev + 1) {
                                        self.error = Some("Obsolete draft command response discarded".into());
                                        return;
                                    }
                                    match app.hist.exec(&mut app.doc, Command::CityosInstall { document: Box::new(document) }) {
                                        Ok(()) => app.changed(),
                                        Err(e) => self.error = Some(e.to_string()),
                                    }
                                } else {
                                    if !self.install_resumed_projection(app, document) { return; }
                                }
                                self.validated = true;
                                if !self.preview_only { let selected=document_selection(&app.doc,&self.selected); self.select(app,&selected); }
                                self.observed = None;
                                self.pending = Some(Instant::now());
                            }
                          }
                        },
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
                            Ok(document)=>self.open_local_document(app,document),
                            Err(error)=>self.error=Some(error),
                        },
                        "Export" => {
                            self.compare = Some(serde_json::to_string_pretty(&value).unwrap_or_default());
                            self.compare_open = !self.changes_mode;
                        }
                        "Incoming"=>{self.resolutions.clear();self.incoming=Some(value);},
                        "Dependencies" => {
                            let retained = self.config.as_ref().zip(app.lib.as_ref()).zip(self.dependency_draft.as_ref())
                                .ok_or_else(|| "Dependency context changed".to_owned())
                                .and_then(|((config, library), draft)| dependencies::retain(&config.cache, library, draft, &value));
                            match retained {
                                Err(error) => { self.error=Some(error); self.validated=false; },
                                Ok(()) => {
                                    self.dependency_draft=None;
                                    match self.dependency_action.take() {
                                        Some(DependencyAction::Open) => self.install_staged(app),
                                        Some(DependencyAction::Resume) => {
                                            let history=app.lib.as_ref().zip(app.lib_file.as_ref()).map(|(lib,file)|lib.load_checkpoint(&file.id));
                                            if let Some(Err(error))=history {self.validated=false;self.error=Some(format!("Retained history dependencies are unavailable. Restore a portable checkpoint before editing: {error}"));}
                                            else {
                                                if let Some(document)=self.resume_document.take(){Self::activate_resumed_document(app,document);}
                                                self.validated=true; self.observed=None; self.pending=Some(Instant::now());
                                            }
                                        },
                                        Some(DependencyAction::Reconcile) => if let Some(bundle)=self.reconciliation_bundle.take() { self.install_reconciliation(app,bundle); },
                                        Some(DependencyAction::Accepted) => if let Some(bundle)=self.accepted_bundle.take() { self.install_accepted(app,bundle); },
                                        None => {},
                                    }
                                }
                            }
                            if let Some(lease)=value["leaseId"].as_str() { self.send(json!({"op":"ReleaseDependencies","leaseId":lease})); }
                        },
                        "Reconcile" => if app.cityos_submission_frozen() {
                            self.error=Some("A submitted draft cannot adopt incoming changes until its original outcome is resolved.".into());
                        } else { self.install_reconciliation(app, value); },
                        "AcceptedProjection" => self.install_accepted(app, value),
                        _ => {}
                    }
                }
            }
        }
        for event in self.host.poll() {
            if !self.matches_preview_event(app, &event) {
                continue;
            }
            match event["kind"].as_str() {
                Some("rendered") => {
                    // A receipt identifies the observed CSS viewport as well as the
                    // requested scenario. Child-window DPI may map these differently.
                    // Retain the observation instead of recompiling into a size loop.
                    self.current = true;
                    if let Some(preview) = &mut self.preview {
                        preview["observedViewport"] = event["viewport"].clone();
                        preview["observedElevation"] = event["elevation"].clone();
                        preview["observedShadow"] = event["shadow"].clone();
                    }
                    if let Some(preview) = &mut self.preview {
                        preview["observedThemePrimary"] = event["themePrimary"].clone();
                        preview["observedThemePage"] = event["themePage"].clone();
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
                Some("shortcut") if !self.preview_only && self.busy.is_none() && !self.recovery_pending => match event["key"].as_str() {
                    Some("s") => {
                        self.host.hide();
                        app.save(false);
                    }
                    Some("z") if event["shift"] != true => app.undo(),
                    Some("z" | "y") => app.redo(),
                    _ => {}
                },
                _ => {}
            }
        }
        self.expire_render_acknowledgement(Instant::now());
        let identity = if self.preview_only { self.staged.as_ref().and_then(revision) } else { revision(&app.doc) };
        if identity != self.observed {
            self.observed = identity;
            self.current = false;
            self.pending = Some(Instant::now());
        }
        if self.validated && self.actual_preview() && !self.is_specimen() && !self.themes_loaded && self.busy.is_none() {
            if let Some(d) = self.active_draft(app) {
                self.send(json!({"op":"Themes","draft":d}));
            }
        } else if self.validated && self.actual_preview() && (self.preview_only || self.inline_identity.is_some() || self.layout.view != layout::View::Canvas) && self.pending.is_some_and(|at| at.elapsed() >= Duration::from_millis(250)) && self.busy.is_none() {
            self.pending = None;
            self.request_preview(app);
        }
        if self.harness || !self.actual_preview() {
            self.pending = None;
        }
    }
    fn select(&mut self, app: &mut App, node: &str) {
        if self.is_specimen() { return; }
        self.selected = node.into();
        if let Some(id) = app.doc.nodes.values().find(|n| n.source_id.as_deref() == Some(node)).map(|n| n.id) {
            app.select_only(id);
        }
    }
    fn capture_resources(&mut self, value: &Value) {
        self.resources = value["resources"].as_array().cloned().unwrap_or_default();
        self.resources.extend(value["specimens"].as_array().into_iter().flatten()
            .filter(|r| r["kind"] == "registered-specimen").cloned());
        // Metadata selection is independent of renderer availability. Keep every
        // registered definition visible without compiling its defaults eagerly.
        for selection in value["specimenSelections"].as_array().into_iter().flatten() {
            let id = selection["resourceId"].as_str().filter(|id| !id.is_empty());
            if id.is_none() { continue; }
            if let Some(resource) = self.resources.iter_mut().find(|r| r["kind"] == "registered-specimen" && r["resourceId"].as_str() == id) {
                for key in ["level", "target"] { if selection[key].is_string() { resource[key] = selection[key].clone(); } }
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
    fn choose_design_file(&mut self) {
        self.picker_open = self.changes_mode || !self.picker_open;
        self.changes_mode = false;
        if self.resources.is_empty() {
            self.send(json!({"op":"Resources"}));
        }
    }
    fn progress_repaint_delay(&self) -> Option<Duration> {
        self.progress_repaint_delay_at(Instant::now())
    }
    fn progress_repaint_delay_at(&self, now: Instant) -> Option<Duration> {
        let visible_pending = !self.harness && self.pending.is_some() && (self.preview_only || self.layout.view != layout::View::Canvas);
        repaint::progress_delay(now, self.busy.is_some(), visible_pending, self.render_started)
    }
    fn expire_render_acknowledgement(&mut self, now: Instant) {
        if repaint::acknowledgement_expired(now, self.render_started) {
            self.error = Some("Renderer did not acknowledge this revision; restart preview to retry".into());
            self.render_started = None;
            self.current = false;
        }
    }
    fn observe_preview_viewport(&mut self, viewport: (u32, u32)) {
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
    fn release_inactive_preview(&mut self) {
        if self.host.release() {
            // Keep the semantic draft and immutable prepared artifact. A new
            // child must acknowledge that exact receipt before becoming current.
            self.current = false;
            self.render_started = None;
        }
    }
    pub(crate) fn performance_state(&self) -> [bool; 5] {
        [self.busy.is_some(), self.pending.is_some(), self.render_started.is_some(), self.current, self.progress_repaint_delay().is_some()]
    }
    pub fn show(&mut self, app: &mut App, ui: &mut egui::Ui, frame: &eframe::Frame) {
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
        if let Some(delay) = self.progress_repaint_delay() { ctx.request_repaint_after(delay); }
        if let Some(toast) = &app.toast {
            if app.now < toast.until {
                ctx.request_repaint_after(Duration::from_secs_f64(toast.until - app.now));
                egui::Panel::bottom("cityos-workspace-save-status").show(ui, |ui| {
                    ui.colored_label(if toast.error { ui.visuals().error_fg_color } else { ui.visuals().text_color() }, &toast.text);
                });
            }
        }
        // `app` remains the user's persistent document while a temporary preview
        // lives in `staged`/`temporary_canvas`. Leaving must still save that draft.
        if !self.recovery_pending { app.autosave_tick(&ctx); }
        if ctx.input(|i| i.viewport().close_requested()) {
            if self.recovery_pending || !app.flush_autosave(){ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);}
        }
        if !ctx.egui_wants_keyboard_input() && self.busy.is_none() && !self.recovery_pending {
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::S)) && !self.preview_only {
                app.save(false);
            }
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z)) && !self.preview_only {
                app.undo();
            }
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::Z)) && !self.preview_only {
                app.redo();
            }
        }
        egui::Panel::top("cityos-workspace-actions").show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                let can_leave = !self.recovery_pending && self.busy.is_none();
                let files = ui.add_enabled(can_leave && (app.lib.is_some() || app.home.is_some()), egui::Button::new("Desigma files"))
                    .on_hover_text("Save the current draft and return to the Desigma file library")
                    .on_disabled_hover_text("Wait for the current operation, recover the draft, or attach a Desigma file library");
                crate::widgets::record(ui, "workspace-desigma-files", files.rect);
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
                if ui.add_enabled(self.busy.is_none()&&!self.recovery_pending, egui::Button::new("Choose design file…")).clicked() {
                    self.choose_design_file();
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
                    if app.home.is_none() && (app.doc.cityos.is_some() || app.hist.can_undo()) && ui.button("Save current draft").clicked() {
                        self.host.hide();
                        app.save(false);
                    }
                } else if !self.preview_only {
                    if ui.add_enabled(!self.recovery_pending,egui::Button::new("Save draft")).clicked() {
                        self.host.hide();
                        app.save(false);
                    }
                    ui.add_enabled_ui(!self.recovery_pending,|ui|{ui.menu_button("Draft actions", |ui| {
                    if app.lib_file.is_none() && ui.button("Save to draft library").clicked() {
                        ui.close();
                        match Self::save_to_draft_library(app) {
                            Ok(()) => app.show_toast("Draft and history saved to the CityOS draft library",false),
                            Err(error) => self.error=Some(error),
                        }
                    }
                    if ui.add_enabled(app.lib_file.is_some()&&self.validated,egui::Button::new("Export portable draft…")).clicked() {
                        ui.close();self.host.hide();
                        let export=(|| -> Result<(),String> {
                            Self::save_before_switch(app)?;
                            let library=app.lib.as_ref().ok_or("Draft library unavailable")?;
                            let file=app.lib_file.as_ref().ok_or("Save this draft to the library before exporting")?;
                            let payload=library.export_checkpoint(&file.id).map_err(|e|e.to_string())?;
                            if let Some(path)=rfd::FileDialog::new().add_filter("Portable Desigma checkpoint",&["json"]).set_file_name(format!("{}.portable.json",app.doc.name)).save_file() {
                                desigma_library::write_atomic(&path,payload.as_bytes()).map_err(|e|e.to_string())?;
                                app.show_toast("Portable draft exported with verified dependencies and retained history",false);
                            }
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
                    if ui.add_enabled(self.validated && self.busy.is_none() && app.hist.can_undo(), egui::Button::new("Undo")).clicked() {
                        ui.close();
                        app.undo();
                    }
                    if ui.add_enabled(self.validated && self.busy.is_none() && app.hist.can_redo(), egui::Button::new("Redo")).clicked() {
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
        });
        let active_id=draft(&app.doc).and_then(|d|d["draftId"].as_str()).map(str::to_owned);
        if active_id.is_some()||!self.tabs.is_empty(){
            let tabs:Vec<_>=self.tabs.iter().map(|(id,t)|(id.clone(),t.document.name.clone())).collect();
            let mut activate=None;let mut close=None;
            egui::Panel::top("cityos-draft-tabs").show(ui,|ui|{ui.horizontal_wrapped(|ui|{
                if active_id.is_some()&&ui.add_enabled(!self.recovery_pending,egui::Button::new(format!("{} · {}",app.doc.name,if app.autosave.pending(){"unsaved"}else{"local draft"})).selected(!self.preview_only)).clicked(){
                    if let Some(config)=self.config.clone(){self.resume(config,app);}
                }
                for(id,name)in tabs {if ui.add_enabled(self.busy.is_none()&&!self.recovery_pending,egui::Button::new(name)).clicked(){activate=Some(id.clone());}
                    if ui.small_button("×").on_hover_text("Close saved inactive draft").clicked(){close=Some(id);}}
                ui.small("One active canvas / renderer");
            });});
            if let Some(id)=close{self.tabs.remove(&id);}
            if let Some(id)=activate{self.activate_tab(app,&id);}
        }
        if self.changes_mode {
            if old_layout != self.layout { self.save_layout(); }
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
                                let eligible=incoming["reconcileAvailable"]==true && resolved && self.validated && !self.preview_only && self.busy.is_none() && app.lib_file.is_some();
                                if ui.add_enabled(eligible,egui::Button::new("Apply reviewed incoming changes")).clicked(){apply_incoming=true;}
                                if app.lib_file.is_none(){ui.small("Save this draft to the library before applying incoming changes.");}
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
                    } else {ui.label("Choose Compare local edits.");}
                });
                if apply_incoming {if let (Some(draft),Some(incoming))=(self.active_draft(app),self.incoming.clone()){
                    let resolutions:Vec<_>=self.resolutions.iter().filter(|(_,value)|!value.is_empty()).map(|(pointer,value)|json!({"sourcePointer":pointer,"resolution":value})).collect();
                    self.send(json!({"op":"Reconcile","expectedRevision":draft["draftRevision"],"draft":draft,"incoming":{"reconciliationId":incoming["reconciliationId"],"pin":incoming["pin"]},"resolutions":resolutions}));
                }}
            });return;
        }
        if let Some(error) = self.error.clone() {
            egui::Panel::top("cityos-workspace-error").show(ui, |ui| {
                ui.colored_label(ui.visuals().error_fg_color, &error);
                if self.recovery_pending && self.submissions.accepted().is_some() {
                    if ui.button("Recover accepted state").clicked(){self.recover_accepted(app);}
                } else if self.recovery_pending && ui.button("Recover document generation").clicked(){self.recover_reconciliation(app);}
                if !self.preview_only && !self.recovery_pending && self.busy.is_none() && ui.button("Retry draft validation").clicked() {
                    if let Some(config)=self.config.clone(){self.resume(config,app);}
                }
            });
        }
        {
            egui::Panel::top("cityos-workspace-busy").exact_size(38.).show(ui, |ui| {
                ui.horizontal(|ui| {
                    if let Some(operation) = &self.busy {
                        crate::cityos_jobs::progress_marker(ui);
                        ui.label(format!("{operation}…"));
                    } else {
                        ui.label(if self.harness {
                            "Embedded host diagnostic"
                        } else if self.preview_only {
                            "Temporary preview · current draft preserved"
                        } else {
                            "Canonical draft · local changes only"
                        });
                        if let Some(path) = &app.file_path {
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
                                ui.small(if finding["code"]=="PACKAGED_CANONICAL_REVISION" {format!("Packaged canonical revision {} · newer main {} needs a matching renderer",finding["commit"],finding["latestCommit"])}else{format!("{}: {}",finding["sourcePath"],finding["error"])});
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
                    ui.small(if self.is_specimen() { "Temporary registered defaults" } else if self.preview_only { "Temporary preview" } else { "Canonical bound draft" });
                    ui.menu_button("Source details", |ui| {
                        ui.set_max_width(600.);
                        if self.is_specimen() {
                            ui.label(SPECIMEN_CANVAS_REASON);
                            for key in ["componentId", "componentVersion", "profileRef", "targetProfileRef"] {
                                ui.label(format!("{key}: {}", resource[key].as_str().unwrap_or("unavailable")));
                            }
                            if let Some(commit) = resource["provenance"]["sourceCommit"].as_str() { ui.monospace(format!("Installed renderer source {commit}")); }
                        }
                        ui.label(resource["sourcePath"].as_str().unwrap_or(""));
                        if let Some(commit) = resource["commit"].as_str() { ui.monospace(format!("GitLab commit {commit}")); }
                        if resource["packagedRevision"]==true {ui.label("Pinned packaged revision. Newer main needs a matching renderer package.");}
                        if let Some(messages) = resource["diagnostics"].as_array() {
                            for message in messages { ui.label(message.as_str().unwrap_or("")); }
                        }
                    });
                    if let Some(d) = self.active_draft(app) { ui.small(format!("Draft revision {}", d["draftRevision"])); }
                    if self.is_specimen() { ui.small(SPECIMEN_CANVAS_REASON); }
                    if let Some(messages) = resource["diagnostics"].as_array() {
                        if !messages.is_empty() { ui.small(format!("{} source findings", messages.len())); }
                    }
                });
            });
        }
        let mut bounds = None;
        let narrow = ui.available_width() < 1100.;
        if narrow && !self.preview_only && self.actual_preview() && self.layout.view == layout::View::Split {
            egui::Panel::top("cityos-narrow-surfaces").show(ui, |ui| { ui.horizontal(|ui| {
                ui.selectable_value(&mut self.narrow_preview, false, "Canvas");
                ui.selectable_value(&mut self.narrow_preview, true, "Actual preview");
            }); });
        }
        let effective_view = self.layout.effective_view(ui.available_width(), self.narrow_preview);
        if self.layout.inspector && ((!self.preview_only && effective_view != layout::View::Preview) || (self.preview_only && !self.actual_preview() && self.validated)) {
            egui::Panel::left("cityos-workspace-properties").default_size(248.).size_range(200.0..=360.0).resizable(true).show(ui,|ui|{
                ui.heading("Layers & properties");
                fn layers(node:&Value,depth:usize,rows:&mut Vec<(String,String)>) {if depth>12{return;}if let Some(id)=node["id"].as_str(){rows.push((id.into(),format!("{}{}", "  ".repeat(depth),id)));}if let Some(slots)=node["slots"].as_object(){for children in slots.values(){if let Some(children)=children.as_array(){for child in children {layers(child,depth+1,rows);}}}}}
                let mut rows=vec![];if let Some(d)=self.active_draft(app){layers(&d["semanticDraft"]["root"],0,&mut rows);}
                for(node,label)in rows {if ui.selectable_label(self.selected==node,label).clicked(){if self.preview_only {self.selected=node.clone();if let Some(canvas)=self.temporary_canvas.as_mut(){if let Some(id)=canvas.doc.nodes.values().find(|n|n.source_id.as_deref()==Some(&node)).map(|n|n.id){canvas.select_only(id);}}}else{self.select(app,&node);}}}
                if self.resource.as_ref().is_some_and(|r|r["editable"]==true) {
                ui.separator();
                let fields=self.resource.as_ref().and_then(|r|r["fields"].as_array()).cloned().unwrap_or_default();
                for field in fields {if let Some(d)=self.active_draft(app){
                    let pointer=field["sourcePointer"].as_str().unwrap_or("").strip_prefix("/composition").unwrap_or(field["sourcePointer"].as_str().unwrap_or(""));
                    let previous=d["semanticDraft"].pointer(pointer).cloned().unwrap_or(Value::Null);let mut value=previous.clone();
                    ui.label(format!("{}.{}",field["nodeId"].as_str().unwrap_or(""),field["property"].as_str().unwrap_or("")));
                    if app.cityos_submission_frozen() { ui.small("Submitted draft frozen · inspect its original outcome in Changes."); }
                    ui.add_enabled_ui(self.validated&&!app.cityos_submission_frozen()&&self.busy.is_none()&&self.selected==field["nodeId"].as_str().unwrap_or(""),|ui|{
                        if let Some(mut boolean)=value.as_bool(){if ui.checkbox(&mut boolean,"Allow multiple open sections").changed(){value=json!(boolean);}}
                        else if let Some(values)=field["values"].as_array(){ui.horizontal_wrapped(|ui|{for option in values {ui.selectable_value(&mut value,option.clone(),option.as_str().unwrap_or(""));}});}
                        else if let Some(text)=previous.as_str() {
                            let key=format!("{}:{}:{}",d["draftId"],d["draftRevision"],pointer);
                            let edited=self.property_text.entry(key).or_insert_with(||text.into());
                            let response=ui.text_edit_singleline(edited);
                            if ui.button("Apply property").clicked() || response.lost_focus()&&ui.input(|i|i.key_pressed(egui::Key::Enter)) {
                                value=json!(edited.clone());
                            }
                            ui.small("Validated against the registered component contract before applying.");
                        }
                    });
                    if value!=previous {self.send(json!({"op":"Command","command":{"draftId":d["draftId"],"expectedRevision":d["draftRevision"],"operationId":uuid::Uuid::new_v4().to_string(),"operation":"set-property","nodeId":field["nodeId"],"property":field["property"],"value":value},"draft":d}));}
                }}
                ui.small("Only the declared properties are editable. Slots and owner actions remain protected.");
                } else {ui.small("Protected source nodes. No editing adapter is enabled for this file.");}
                ui.separator();if let Some(d)=self.active_draft(app){ui.label(format!("Draft revision {}",d["draftRevision"]));if let Some(commit)=d["base"]["commit"].as_str(){ui.small(format!("Canonical base {}",&commit[..commit.len().min(12)]));}}
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
                let status = if self.current { "Current" } else if self.error.is_some() { "Failed — previous preview retained" }
                    else if matches!(self.busy.as_deref(), Some("Preview" | "FixturePreview")) || self.render_started.is_some() { "Rendering" } else { "Out of date" };
                if self.is_specimen() { ui.label(format!("{status} · temporary registered defaults")); }
                else { ui.label(format!("{status} · rendered revision {}", p["draftRevision"])); }
                if p["scenario"]["fixture"]=="cityos-web-frame" {
                    ui.small(format!("Rendered elevation: {}", p["observedElevation"].as_str().unwrap_or("pending")));
                }
                ui.small(format!("Viewport: {} × {} CSS px", p["observedViewport"]["width"], p["observedViewport"]["height"]));
            } else {
                ui.label("Waiting for preview");
            }
            ui.small(if self.harness { "Inert embedded-host diagnostic · no repository code" } else { "Registered CityOS renderer · deterministic fixture · owner actions disabled" });
            let old = (self.locale.clone(), self.theme.clone(), self.theme_id.clone(), self.interactive);
            if self.harness {
                ui.small("Keyboard, clipboard and focus controls are inside the diagnostic page.");
            } else if self.is_specimen() {
                ui.small("Registered default theme · light/dark presentation only");
            } else { egui::ComboBox::from_id_salt("workspace-theme")
                .width(250.)
                .selected_text(self.themes.iter().find(|t| t["id"] == self.theme_id).and_then(|t| t["name"].as_str()).unwrap_or("Renderer default theme"))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.theme_id, String::new(), "Renderer default theme");
                    for item in &self.themes {
                        if let (Some(id), Some(name)) = (item["id"].as_str(), item["name"].as_str()) {
                            ui.selectable_value(&mut self.theme_id, id.into(), name);
                        }
                    }
                }); }
            ui.horizontal_wrapped(|ui| {
                if !self.harness {
                ui.selectable_value(&mut self.locale, "ar".into(), "Arabic · RTL");
                ui.selectable_value(&mut self.locale, "en".into(), "English · LTR");
                ui.selectable_value(&mut self.theme, "light".into(), "Light");
                ui.selectable_value(&mut self.theme, "dark".into(), "Dark");
                if self.resource.as_ref().is_none_or(|r|r["fixture"]!="cityos-lower-level") {
                    ui.checkbox(&mut self.interactive,"Interact with fixture").on_hover_text("Preview interactions stay local and do not edit the draft or invoke owner actions.");
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
            let panel = egui::Panel::right("cityos-workspace-preview").default_size(preview_width).size_range(280.0..=1400.0).resizable(true).show(ui, &mut web_panel);
            if !ctx.input(|i| i.pointer.any_down()) { resized_preview = Some(panel.response.rect.width().clamp(280., 1400.)); }
        }
        if show_full_preview {
            egui::CentralPanel::default().show(ui, &mut web_panel);
        } else {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.heading(if !self.validated { "Selected design" } else if self.actual_preview() { "Native canvas" } else { "Protected composition structure" });
                if !self.validated {
                    if self.busy.is_some() {
                        crate::cityos_jobs::progress_marker(ui);
                    }
                    if self.is_specimen() && self.busy.is_none() {
                        ui.label("Preview and canvas availability");
                        if let Some(resource) = &self.resource { ui.label(registered_availability_reason(resource)); }
                        ui.small("The selected definition is preserved. Your saved draft remains in its tab.");
                    } else {
                        ui.label(if self.busy.is_some() { "Loading selected source…" } else { "Choose a design file to preview before opening it on canvas." });
                    }
                    return;
                }
                let app = if self.preview_only {
                    let Some(canvas) = self.temporary_canvas.as_mut() else {
                        return;
                    };
                    canvas.as_mut()
                } else {
                    &mut *app
                };
                let rect = ui.available_rect_before_wrap();
                app.canvas_rect = rect;
                if !app.first_fit_done {
                    app.first_fit_done = true;
                    app.fit_all();
                }
                crate::canvas::tiles::paint(app, ui, rect);
                // Projection-only objects: pointer operations select/pan/zoom, never mutate authored structure.
                let response = ui.interact(rect, ui.id().with("cityos-native-canvas"), egui::Sense::click_and_drag());
                if response.dragged_by(egui::PointerButton::Middle) {
                    let delta = ctx.input(|i| i.pointer.delta());
                    app.view.offset.x -= delta.x / app.view.zoom;
                    app.view.offset.y -= delta.y / app.view.zoom;
                }
                if response.hovered() {
                    let zoom = ctx.input(|i| i.zoom_delta());
                    if zoom != 1. {
                        let position = ctx.input(|i| i.pointer.hover_pos()).unwrap_or(rect.center());
                        app.view.zoom_at(rect.min, position, app.view.zoom * zoom);
                    }
                }
                if response.clicked() {
                    if let Some(position) = response.interact_pointer_pos() {
                        let point = app.view.to_page(rect.min, position);
                        let picked = app
                            .doc
                            .nodes
                            .values()
                            .rev()
                            .find(|n| {
                                !app.doc.pages.contains(&n.id) && {
                                    let t = app.doc.world_transform(n.id);
                                    point.x >= t.tx && point.y >= t.ty && point.x <= t.tx + n.size.x && point.y <= t.ty + n.size.y
                                }
                            })
                            .and_then(|n| n.source_id.clone());
                        if let Some(node) = picked {
                            self.selected = node.clone();
                            if let Some(id) = app.doc.nodes.values().find(|n| n.source_id.as_deref() == Some(&node)).map(|n| n.id) {
                                app.select_only(id);
                            }
                        }
                    }
                }
                crate::canvas::draw::overlays(app, ui, rect);
            });
        }
        if (!show_side_preview && !show_full_preview) || !self.open {
            self.release_inactive_preview();
        }
        if self.compare_open {
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
        if let Some(width) = resized_preview { self.layout.preview_width = width; }
        if old_layout != self.layout { self.save_layout(); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct NavigationLibrary(std::path::PathBuf);
    impl NavigationLibrary {
        fn app() -> (Self, App, desigma_library::Library) {
            let root = std::env::temp_dir().join(format!("desigma-workspace-navigation-{}", uuid::Uuid::new_v4()));
            let library = desigma_library::Library::open(&root).unwrap();
            library.put_cityos_payload(&desigma_library::payload_digest(b"fixture"), b"fixture").unwrap();
            let mut app = App::new(tab_document("navigation"));
            app.home = None; app.lib = Some(library.clone());
            Workspace::save_to_draft_library(&mut app).unwrap();
            (Self(root), app, library)
        }
    }
    impl Drop for NavigationLibrary {
        fn drop(&mut self) {
            crate::home::thumb::finish_library_jobs(&self.0);
            let resolved = self.0.canonicalize().unwrap();
            assert_eq!(resolved.parent(), Some(std::env::temp_dir().canonicalize().unwrap().as_path()));
            std::fs::remove_dir_all(resolved).unwrap();
        }
    }
    #[test]
    fn desigma_files_navigation_saves_the_bound_draft_and_preserves_undo_and_bindings() {
        let (_directory, mut app, library) = NavigationLibrary::app();
        let id = app.lib_file.as_ref().unwrap().id.clone();
        let mut edited = app.doc.clone(); edited.name = "Edited before returning to files".into();
        app.hist.exec(&mut app.doc, Command::CityosInstall { document: Box::new(edited) }).unwrap();
        app.changed();
        let document = app.doc.to_json().unwrap();
        let history = serde_json::to_value(&app.hist).unwrap();
        let mut workspace = Workspace::default(); workspace.open = true;
        workspace.pending = Some(Instant::now()); workspace.preview = Some(json!({"draftRevision":0}));
        workspace.host.url = Some("http://127.0.0.1:1/fixture".into());
        workspace.open_desigma_files(&mut app).unwrap();
        assert!(!workspace.open && !workspace.library_requested());
        assert!(app.home.is_some() && app.doc.cityos.is_some());
        assert!(workspace.host.url.is_none() && workspace.preview.is_none() && workspace.pending.is_none());
        assert_eq!(app.doc.to_json().unwrap(), document);
        assert_eq!(serde_json::to_value(&app.hist).unwrap(), history);
        let checkpoint = library.load_checkpoint(&id).unwrap();
        assert_eq!(checkpoint.document.to_json().unwrap(), document);
        assert_eq!(serde_json::to_value(&checkpoint.history).unwrap(), history);
        assert_eq!(library.all_files().len(), 1);
    }
    #[test]
    fn desigma_files_navigation_keeps_the_current_workspace_when_saving_fails() {
        let (_directory, mut app, library) = NavigationLibrary::app();
        let file = app.lib_file.as_ref().unwrap().clone();
        let stored = library.load_checkpoint(&file.id).unwrap().to_json().unwrap();
        app.doc.name = "Unsaved navigation change".into(); app.changed();
        app.autosave.changed_at = Some(0.); app.autosave.confirmed = true;
        let document = app.doc.to_json().unwrap(); let hash = app.autosave.saved_hash;
        let blocker = library.path("projects").join(&file.project_id).join(".save.pending.json");
        std::fs::create_dir(&blocker).unwrap();
        let mut workspace = Workspace::default(); workspace.open = true;
        workspace.host.url = Some("http://127.0.0.1:1/fixture".into());
        assert!(workspace.open_desigma_files(&mut app).is_err());
        assert!(workspace.open && workspace.library_requested());
        assert!(app.home.is_none() && app.autosave.pending());
        assert!(workspace.host.url.is_some());
        assert_eq!(app.doc.to_json().unwrap(), document);
        assert_eq!(app.autosave.saved_hash, hash);
        assert_eq!(app.lib_file.as_ref().unwrap().id, file.id);
        std::fs::remove_dir(&blocker).unwrap();
        assert_eq!(library.load_checkpoint(&file.id).unwrap().to_json().unwrap(), stored);
    }
    #[test]
    fn desigma_files_navigation_discards_temporary_views_without_saving_them_or_resetting_home() {
        let (_directory, mut app, library) = NavigationLibrary::app();
        let file = app.lib_file.as_ref().unwrap().id.clone();
        let stored = library.load_checkpoint(&file).unwrap().to_json().unwrap();
        let document = app.doc.to_json().unwrap();
        let mut home = crate::home::Home::new(); home.search = "Retain my file filter".into(); home.scroll = 42.;
        app.home = Some(home);
        let mut workspace = Workspace::default(); workspace.open = true; workspace.preview_only = true;
        workspace.staged = Some(Document::new("Temporary preview"));
        workspace.temporary_canvas = Some(Box::new(App::new(Document::new("Temporary canvas"))));
        workspace.open_desigma_files(&mut app).unwrap();
        assert!(!workspace.open && !workspace.library_requested());
        assert!(workspace.staged.is_none() && workspace.temporary_canvas.is_none());
        assert_eq!(app.home.as_ref().unwrap().search, "Retain my file filter");
        assert_eq!(app.home.as_ref().unwrap().scroll, 42.);
        assert_eq!(app.doc.to_json().unwrap(), document);
        assert_eq!(library.load_checkpoint(&file).unwrap().to_json().unwrap(), stored);
        assert_eq!(library.all_files().len(), 1);
    }
    #[test]
    fn desigma_files_navigation_cannot_abandon_inflight_commands_or_uncertain_recovery() {
        let mut app = App::new(Document::new("Current")); app.home = None;
        let document = app.doc.to_json().unwrap();
        let mut workspace = Workspace::default(); workspace.open = true;
        workspace.busy = Some("Command".into());
        assert!(workspace.open_desigma_files(&mut app).unwrap_err().contains("current CityOS operation"));
        workspace.busy = None; workspace.recovery_pending = true;
        assert!(workspace.open_desigma_files(&mut app).unwrap_err().contains("Recover"));
        assert!(workspace.open && app.home.is_none());
        assert_eq!(app.doc.to_json().unwrap(), document);
    }
    #[test]
    fn resumed_projection_exits_file_home_only_after_the_retained_dependencies_validate() {
        let (_directory, mut app, library) = NavigationLibrary::app();
        app.home = Some(crate::home::Home::new());
        let original = app.doc.to_json().unwrap(); let history = serde_json::to_value(&app.hist).unwrap();
        let mut workspace = Workspace::default();
        let document = app.doc.clone();
        assert!(workspace.install_resumed_projection(&mut app, document));
        assert!(app.home.is_none());
        assert_eq!(app.doc.to_json().unwrap(), original);
        assert_eq!(serde_json::to_value(&app.hist).unwrap(), history);
        assert!(dependencies::complete(&library, &app.doc));
    }
    #[test]
    fn missing_resume_dependencies_preserve_the_file_home_and_original_document() {
        let root = std::env::temp_dir().join(format!("desigma-workspace-navigation-{}", uuid::Uuid::new_v4()));
        let _directory = NavigationLibrary(root.clone());
        let mut app = App::new(tab_document("missing-dependency"));
        app.lib = Some(desigma_library::Library::open(&root).unwrap());
        app.home = Some(crate::home::Home::new());
        let original = app.doc.to_json().unwrap(); let document = app.doc.clone();
        let mut workspace = Workspace::default();
        assert!(!workspace.install_resumed_projection(&mut app, document));
        assert!(app.home.is_some() && workspace.resume_document.is_some());
        assert!(!workspace.validated);
        assert_eq!(app.doc.to_json().unwrap(), original);
    }
    #[test]
    fn renderer_ack_wait_uses_its_original_deadline_without_a_repaint_heartbeat() {
        let mut workspace = Workspace::default();
        let started = Instant::now();
        workspace.render_started = Some(started);
        for seconds in [0, 1, 15, 29, 30, 31] {
            let elapsed = Duration::from_secs(seconds);
            assert_eq!(workspace.progress_repaint_delay_at(started + elapsed),
                Some(RENDER_ACK_TIMEOUT.saturating_sub(elapsed)));
            assert_eq!(workspace.render_started, Some(started), "An unrelated UI pass must not postpone the deadline");
        }
        // A matching IPC receipt clears this field in poll(), waking the UI
        // immediately through Host's existing IPC handler.
        workspace.render_started = None;
        workspace.current = true;
        assert!(workspace.progress_repaint_delay_at(started + Duration::from_secs(1)).is_none());
    }
    #[test]
    fn renderer_ack_wait_keeps_loading_and_pending_edit_progress_serviceable() {
        let mut workspace = Workspace::default();
        let started = Instant::now();
        workspace.render_started = Some(started);
        workspace.busy = Some("Preview".into());
        assert_eq!(workspace.progress_repaint_delay_at(started), Some(Duration::from_millis(100)));
        assert_eq!(workspace.progress_repaint_delay_at(started + Duration::from_millis(29_950)), Some(Duration::from_millis(50)));
        workspace.busy = None;
        workspace.pending = Some(started);
        workspace.preview_only = true;
        assert_eq!(workspace.progress_repaint_delay_at(started), Some(Duration::from_millis(100)));
        workspace.pending = None;
        workspace.harness = true;
        workspace.render_started = None;
        workspace.busy = Some("Harness".into());
        assert_eq!(workspace.progress_repaint_delay_at(started), Some(Duration::from_millis(100)), "Initial harness loading still polls its helper");
        workspace.busy = None;
        assert!(workspace.progress_repaint_delay_at(started).is_none());
    }
    #[test]
    fn renderer_ack_timeout_at_the_deadline_retains_the_artifact_and_pending_edit() {
        let mut workspace = Workspace::default();
        let started = Instant::now();
        let artifact = json!({"requestId":"retained-receipt","draftRevision":1,"semanticDigest":"unchanged"});
        workspace.preview = Some(artifact.clone());
        workspace.render_started = Some(started);
        workspace.pending = Some(started);
        workspace.current = false;
        workspace.expire_render_acknowledgement(started + RENDER_ACK_TIMEOUT - Duration::from_nanos(1));
        assert!(workspace.error.is_none());
        assert_eq!(workspace.render_started, Some(started));
        workspace.expire_render_acknowledgement(started + RENDER_ACK_TIMEOUT);
        assert!(workspace.error.as_deref().is_some_and(|error| error.contains("did not acknowledge")));
        assert!(workspace.render_started.is_none());
        assert!(!workspace.current);
        assert_eq!(workspace.preview, Some(artifact));
        assert_eq!(workspace.pending, Some(started), "A newer semantic revision is not discarded by a renderer timeout");
        let error = workspace.error.clone();
        workspace.expire_render_acknowledgement(started + RENDER_ACK_TIMEOUT + Duration::from_secs(1));
        assert_eq!(workspace.error, error, "Expiry is terminal without another deadline or retry");
    }
    #[test]
    fn releasing_inactive_preview_preserves_the_artifact_and_pending_semantic_revision() {
        let mut workspace = Workspace::default();
        workspace.layout.view = layout::View::Canvas;
        workspace.preview = Some(json!({"requestId":"retained-artifact","draftRevision":1}));
        workspace.host.url = Some("http://127.0.0.1:1/fixture".into());
        workspace.current = true;
        workspace.pending = Some(Instant::now());
        workspace.render_started = Some(Instant::now());
        workspace.release_inactive_preview();
        assert_eq!(workspace.preview, Some(json!({"requestId":"retained-artifact","draftRevision":1})));
        assert!(workspace.pending.is_some(), "A newer draft must still render when made visible");
        assert!(!workspace.current && workspace.render_started.is_none());
        assert!(workspace.progress_repaint_delay().is_none(), "An inactive renderer must not keep the canvas awake");
        workspace.release_inactive_preview();
        assert!(workspace.pending.is_some());
    }
    #[test]
    fn inert_host_resize_does_not_schedule_semantic_work_or_idle_repaints() {
        let mut workspace = Workspace::default();
        workspace.harness = true;
        workspace.preview_only = true;
        for viewport in [(800, 600), (900, 700), (900, 700)] {
            workspace.observe_preview_viewport(viewport);
            assert_eq!(workspace.panel_size, viewport);
            assert_eq!(workspace.viewport, viewport);
            assert!(workspace.pending.is_none());
            assert!(workspace.progress_repaint_delay().is_none());
        }
        // A retained pre-harness debounce must not keep the diagnostic awake.
        workspace.pending = Some(Instant::now());
        assert!(workspace.progress_repaint_delay().is_none());
        workspace.busy = Some("Harness".into());
        assert!(workspace.progress_repaint_delay().is_some(), "Loading remains responsive");
        workspace.busy = None;
        workspace.harness = false;
        workspace.pending = None;
        workspace.observe_preview_viewport((1000, 800));
        assert!(workspace.pending.is_some(), "Real previews still recompile for changed viewports");
        assert!(workspace.progress_repaint_delay().is_some());
    }
    #[test]
    fn current_workspace_is_event_driven_but_pending_work_keeps_progress_repaints() {
        let mut workspace = Workspace::default();
        assert!(workspace.progress_repaint_delay().is_none());
        workspace.pending = Some(Instant::now());
        workspace.layout.view = layout::View::Canvas;
        assert!(workspace.progress_repaint_delay().is_none(), "Hidden previews do not poll");
        workspace.preview_only = true;
        assert_eq!(workspace.progress_repaint_delay(), Some(Duration::from_millis(100)));
        workspace.preview_only = false;
        workspace.pending = None;
        workspace.render_started = Some(Instant::now());
        assert!(workspace.progress_repaint_delay().is_some(), "Renderer deadlines remain serviceable");
        workspace.render_started = None;
        workspace.busy = Some("Command".into());
        assert!(workspace.progress_repaint_delay().is_some());
        workspace.busy = None;
        assert!(workspace.progress_repaint_delay().is_none());
    }
    #[test]
    fn semantic_change_display_resolves_frame_references_and_rfc6901_pointers() {
        let base = json!({"id":"frame","revision":"1","root":{"slots":{"material":[{"props":{"elevation":"flat","a/b~c":"before"}}]}}});
        let candidate = json!({"id":"frame","revision":"2","root":{"slots":{"material":[{"props":{"elevation":"resting","a/b~c":"after"}}]}}});
        let change = json!({"nodeId":"material","field":"elevation",
            "before":{"resource":{"id":"frame","revision":"1"},"nodeId":"material","pointer":"/root/slots/material/0/props/elevation"},
            "after":{"resource":{"id":"frame","revision":"2"},"nodeId":"material","pointer":"/root/slots/material/0/props/elevation"}});
        let export = json!({"base":base,"candidate":candidate,"changeset":{"changes":[change]}});
        let original = export.clone();
        let change = &export["changeset"]["changes"][0];
        assert_eq!(semantic_change_label(change), "material.elevation");
        assert_eq!(semantic_change_value_text(change.get("before"), export.get("base")), "flat");
        assert_eq!(semantic_change_value_text(change.get("after"), export.get("candidate")), "resting");
        let escaped = json!({"resource":{"id":"frame","revision":"1"},"nodeId":"material","pointer":"/root/slots/material/0/props/a~1b~0c"});
        assert_eq!(semantic_change_value_text(Some(&escaped), export.get("base")), "before");
        assert_eq!(export, original, "Display must leave the complete raw export untouched");
    }
    #[test]
    fn semantic_change_display_preserves_accordion_scalar_values() {
        let change = json!({"sourcePointer":"/root/props/allowMultiple","before":false,"after":true});
        assert_eq!(semantic_change_label(&change), "/root/props/allowMultiple");
        assert_eq!(semantic_change_value_text(change.get("before"), None), "false");
        assert_eq!(semantic_change_value_text(change.get("after"), None), "true");
        assert_eq!(semantic_change_value_text(Some(&Value::Null), None), "null");
    }
    #[test]
    fn semantic_change_display_marks_missing_or_unknown_references_unavailable() {
        let document = json!({"id":"frame","revision":"1","root":{"props":{"elevation":"flat"}}});
        for reference in [
            json!({"resource":{"id":"frame","revision":"1"},"nodeId":"material","pointer":"/root/props/missing"}),
            json!({"resource":{"id":"frame","revision":"1"},"nodeId":"material"}),
            json!({"resource":{"id":"frame","revision":"1"},"nodeId":"material","pointer":"root/props/elevation"}),
            json!({"resource":{"id":"other","revision":"1"},"nodeId":"material","pointer":"/root/props/elevation"}),
            json!({"resource":{"id":"frame","revision":"2"},"nodeId":"material","pointer":"/root/props/elevation"}),
        ] {
            assert_eq!(semantic_change_value_text(Some(&reference), Some(&document)),
                format!("Unavailable (unresolved reference: {reference})"));
        }
        assert_eq!(semantic_change_value_text(None, Some(&document)), "Unavailable (value not provided)");
    }
    #[test]
    fn global_design_chooser_exits_changes_and_preserves_design_toggle() {
        for (changes_mode, picker_open, expected_open) in [
            (true, false, true),
            (true, true, true),
            (false, false, true),
            (false, true, false),
        ] {
            let mut workspace = Workspace::default();
            workspace.changes_mode = changes_mode;
            workspace.picker_open = picker_open;
            workspace.resources = vec![json!({"sourcePath":"selected.json"})];
            workspace.choose_design_file();
            assert!(!workspace.changes_mode, "The global chooser must reveal Design");
            assert_eq!(workspace.picker_open, expected_open, "changes={changes_mode}, picker={picker_open}");
            assert!(workspace.busy.is_none(), "Cached choices need no bridge request");
        }
    }
    fn registered_specimen(component: &str, locale: &str) -> Value {
        json!({"kind":"registered-specimen","resourceId":format!("web-fixture/{component}@1/{locale}"),
            "componentId":component,"componentVersion":"1","profileRef":"fixture-profile","targetProfileRef":"web-fixture",
            "label":component,"fixture":"cityos-lower-level","actualWebPreview":true,"editable":false,"openAvailable":true,
            "provenance":{"kind":"registered-defaults","canonicalResource":false,"temporary":true,"locale":locale,"sourceCommit":"a".repeat(40)}})
    }
    fn specimen_receipt(workspace: &Workspace) -> Value {
        let component = workspace.resource.as_ref().unwrap()["componentId"].as_str().unwrap();
        let resource = registered_specimen(component, &workspace.locale);
        json!({"sessionId":"session-1","requestId":"workspace-3","draftId":format!("registered-fixture:{}",resource["resourceId"].as_str().unwrap()),
            "draftRevision":0,"semanticDigest":"c".repeat(64),"scenario":workspace.scenario(),
            "url":"http://127.0.0.1:12345/fixture","resource":resource,"provenance":resource["provenance"],
            "canonicalResource":false,"ownerWrites":false,"state":"prepared"})
    }
    fn specimen_workspace() -> Workspace {
        let mut workspace = Workspace::default();
        workspace.prepare_resource(registered_specimen("fixture.component", "ar"));
        workspace.specimen_request = workspace.captured_specimen_request();
        workspace
    }
    #[test]
    fn registered_specimens_are_listed_even_when_no_canonical_resource_is_qualified() {
        let mut workspace = Workspace::default();
        let specimen = registered_specimen("fixture.component", "ar");
        workspace.capture_resources(&json!({"resources":[],"specimens":[specimen],"diagnostics":[{"code":"UNQUALIFIED"}]}));
        assert_eq!(workspace.resources, vec![specimen]);
        assert!(specimen_locator(&workspace.resources[0]).is_some());
    }
    #[test]
    fn unqualified_registered_definitions_remain_selectable_without_loading_another_design() {
        let mut workspace = Workspace::default();
        let specimen = registered_specimen("ready.component", "ar");
        let unavailable = json!({"resourceId":"unqualified.component", "componentId":"unqualified.component", "label":"Owner component",
            "availability":{"preview":{"status":"unavailable","reasons":["owner-input-fixture-unavailable"]}},
            "provenance":{"canonicalResource":false,"temporary":true}});
        workspace.capture_resources(&json!({"resources":[],"specimens":[specimen],"specimenSelections":[specimen,unavailable]}));
        assert_eq!(workspace.resources.len(), 2, "Qualified descriptors must not be duplicated");
        let selection = workspace.resources[1].clone();
        workspace.open_resource(selection.clone());
        assert_eq!(workspace.resource, Some(selection));
        assert!(workspace.preview_only && workspace.is_specimen());
        assert!(workspace.bridge.is_none() && workspace.busy.is_none());
        assert!(workspace.preview.is_none() && workspace.staged.is_none() && workspace.temporary_canvas.is_none());
        assert!(workspace.error.as_deref().unwrap().contains("owner-data fixture"));
    }
    #[test]
    fn temporary_registered_specimen_never_changes_document_history_saving_or_library() {
        let root = std::env::temp_dir().join(format!("desigma-specimen-{}", uuid::Uuid::new_v4()));
        let library = desigma_library::Library::open(&root).unwrap();
        library.put_cityos_payload(&desigma_library::payload_digest(b"fixture"), b"fixture").unwrap();
        let mut app = App::new(tab_document("persistent")); app.home = None; app.lib = Some(library.clone());
        let mut edited = app.doc.clone(); edited.name = "Saved draft with undo".into();
        app.hist.exec(&mut app.doc, Command::CityosInstall { document: Box::new(edited) }).unwrap();
        Workspace::save_to_draft_library(&mut app).unwrap();
        let file_id = app.lib_file.as_ref().unwrap().id.clone();
        let stored_before = library.load_checkpoint(&file_id).unwrap();
        app.doc.name = "Existing unsaved work".into(); app.changed();
        let before = app.doc.to_json().unwrap();
        let history = serde_json::to_value(&app.hist).unwrap();
        let pending = app.autosave.pending();
        let mut workspace = Workspace::default();
        // No configured helper is started in this state-transition unit test.
        workspace.open_resource(registered_specimen("fixture.component", "ar"));
        workspace.specimen_request = workspace.captured_specimen_request();
        let receipt = specimen_receipt(&workspace);
        assert!(workspace.accept_specimen_preview(receipt.clone(), "FixturePreview"));
        assert!(workspace.matches_preview_event(&app, &receipt));
        workspace.select(&mut app, "material");
        workspace.install_staged(&mut app);
        workspace.create_local_copy(&mut app);
        workspace.send(json!({"op":"Command"}));
        assert!(workspace.active_draft(&app).is_none());
        assert!(workspace.staged.is_none() && workspace.temporary_canvas.is_none());
        assert!(workspace.preview_only && !workspace.changes_mode && workspace.tabs.is_empty());
        assert!(workspace.error.as_ref().unwrap().contains("no canonical document binding"));
        assert_eq!(app.doc.to_json().unwrap(), before);
        assert_eq!(serde_json::to_value(&app.hist).unwrap(), history);
        assert_eq!(app.autosave.pending(), pending);
        assert_eq!(app.lib_file.as_ref().unwrap().id, file_id);
        let stored_after = library.load_checkpoint(&file_id).unwrap();
        assert_eq!(stored_after.document.to_json().unwrap(), stored_before.document.to_json().unwrap());
        assert_eq!(serde_json::to_value(&stored_after.history).unwrap(), serde_json::to_value(&stored_before.history).unwrap());
        crate::home::thumb::finish_library_jobs(&root);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn registered_specimen_receipts_reject_busy_obsolete_and_substituted_results() {
        for field in ["componentId", "componentVersion", "profileRef", "targetProfileRef"] {
            let mut workspace = specimen_workspace();
            let mut receipt = specimen_receipt(&workspace);
            receipt["resource"][field] = json!("other");
            assert!(!workspace.accept_specimen_preview(receipt, "FixturePreview"), "{field}");
            assert!(workspace.preview.is_none());
        }
        for change in ["selection", "generation", "locale", "theme", "viewport", "busy", "draft", "authority", "source"] {
            let mut workspace = specimen_workspace();
            let mut receipt = specimen_receipt(&workspace);
            match change {
                "selection" => workspace.resource = Some(registered_specimen("other.component", "ar")),
                "generation" => workspace.preview_generation += 1,
                "locale" => workspace.locale = "en".into(),
                "theme" => workspace.theme = "dark".into(),
                "viewport" => workspace.viewport.0 += 1,
                "busy" => workspace.busy = Some("FixturePreview".into()),
                "draft" => receipt["draft"] = json!({"draftId":"must-not-bind"}),
                "authority" => receipt["canonicalResource"] = json!(true),
                "source" => receipt["provenance"]["sourceCommit"] = json!("b".repeat(40)),
                _ => unreachable!(),
            }
            assert!(!workspace.accept_specimen_preview(receipt, "FixturePreview"), "{change}");
        }
        let mut workspace = specimen_workspace();
        let receipt = specimen_receipt(&workspace);
        assert!(!workspace.accept_specimen_preview(receipt.clone(), "Preview"));
        assert!(workspace.accept_specimen_preview(receipt.clone(), "FixturePreview"));
        assert!(!workspace.accept_specimen_preview(receipt, "FixturePreview"));
    }
    #[test]
    fn registered_specimen_panel_reflow_coalesces_without_an_error_resize_loop() {
        let mut workspace = specimen_workspace();
        let previous = specimen_receipt(&workspace);
        workspace.viewport = (1200, 640);
        workspace.settle_specimen_preview(previous);
        assert!(workspace.preview.is_none());
        assert!(workspace.error.is_none(), "Expected reflow must not alter the layout with an error row");
        assert!(workspace.pending.is_some());
        workspace.specimen_request = workspace.captured_specimen_request();
        workspace.pending = None;
        let current = specimen_receipt(&workspace);
        workspace.settle_specimen_preview(current);
        assert!(workspace.preview.is_some());
        assert!(workspace.pending.is_none() && workspace.error.is_none());
    }
    #[test]
    fn invalid_registered_specimen_receipt_never_retries_the_same_request() {
        let mut workspace = specimen_workspace();
        let mut invalid = specimen_receipt(&workspace);
        invalid["semanticDigest"] = json!("");
        workspace.pending = Some(Instant::now());
        workspace.settle_specimen_preview(invalid);
        assert!(workspace.error.is_some());
        assert!(workspace.pending.is_none() && workspace.specimen_request.is_none());
        assert!(workspace.preview.is_none() && workspace.render_started.is_none());
    }
    #[test]
    fn specimen_regeneration_retains_previous_receipt_and_panel_geometry_even_if_helper_is_unavailable() {
        let mut workspace = specimen_workspace();
        let receipt = specimen_receipt(&workspace);
        assert!(workspace.accept_specimen_preview(receipt.clone(), "FixturePreview"));
        workspace.host.url = receipt["url"].as_str().map(str::to_owned);
        workspace.viewport.1 += 24;
        workspace.request_specimen_preview();
        assert_eq!(workspace.preview, Some(receipt.clone()), "Status and viewport rows must remain present during regeneration");
        assert_eq!(workspace.host.url.as_deref(), receipt["url"].as_str());
        assert!(!workspace.current);
        assert!(workspace.error.is_some(), "This fixture deliberately has no configured helper");
    }
    #[test]
    #[ignore = "requires the matching packaged renderer; exercised by the Windows delivery check"]
    fn packaged_registered_specimens_pass_native_identity_checks_for_both_locales() {
        let directory = std::path::PathBuf::from(std::env::var("CITYOS_DESIGN_WORKSPACE_EVIDENCE").expect("evidence directory"));
        let package = std::path::PathBuf::from(std::env::var("CITYOS_DESIGN_WORKSPACE_PACKAGE").expect("package directory"));
        let bridge = bridge::Bridge::spawn(Config {
            node: std::env::var("CITYOS_DESIGN_WORKSPACE_NODE").expect("pinned node").into(),
            bridge: package.join("workspace.mjs"),
            repository: std::env::var("CITYOS_DESIGN_WORKSPACE_REPOSITORY").expect("canonical repository").into(),
            cache: directory.join("canonical-cache"),
        });
        bridge.jobs.send(json!({"op":"Resources"})).unwrap();
        let resources = bridge.replies.recv_timeout(Duration::from_secs(30)).unwrap().unwrap();
        let specimens = resources["specimens"].as_array().expect("registered specimens");
        assert!(!specimens.is_empty());
        let mut receipts = Vec::new();
        for selected in specimens {
            for locale in ["ar", "en"] {
                let mut workspace = Workspace::default();
                workspace.prepare_resource(selected.clone());
                workspace.locale = locale.into();
                workspace.specimen_request = workspace.captured_specimen_request();
                bridge.jobs.send(workspace.specimen_request.as_ref().unwrap().job()).unwrap();
                let receipt = bridge.replies.recv_timeout(Duration::from_secs(30)).unwrap().unwrap();
                workspace.settle_specimen_preview(receipt.clone());
                assert!(workspace.preview.is_some(), "{} / {locale}: {:?}", selected["resourceId"], workspace.error);
                assert!(workspace.pending.is_none());
                receipts.push(json!({"resourceId":receipt["resource"]["resourceId"],"locale":locale,
                    "scenario":receipt["scenario"],"artifactDigest":receipt["artifactDigest"],
                    "provenance":receipt["provenance"],"nativeIdentityAccepted":true,"embeddedRendererObserved":false}));
            }
        }
        std::fs::write(directory.join("native-specimen-receipts.json"), serde_json::to_vec_pretty(&receipts).unwrap()).unwrap();
    }
    #[test]
    fn registered_specimen_locale_regenerates_defaults_with_a_stable_four_field_locator() {
        let mut workspace = specimen_workspace();
        let original_locator = workspace.specimen_request.as_ref().unwrap().locator.clone();
        let old_receipt = specimen_receipt(&workspace);
        workspace.locale = "en".into();
        assert!(!workspace.accept_specimen_preview(old_receipt, "FixturePreview"));
        workspace.specimen_request = workspace.captured_specimen_request();
        let request = workspace.specimen_request.as_ref().unwrap();
        assert_eq!(request.locator, original_locator);
        assert_eq!(request.scenario["locale"], "en");
        assert_eq!(request.locator.as_object().unwrap().len(), 4);
        let job = request.job();
        assert_eq!(job["op"], "FixturePreview");
        assert_eq!(job["fixture"], original_locator);
        assert_eq!(job["scenario"]["locale"], "en");
        assert!(job.get("draft").is_none());
        let receipt = specimen_receipt(&workspace);
        assert!(receipt["resource"]["resourceId"].as_str().unwrap().ends_with("/en"));
        assert!(workspace.accept_specimen_preview(receipt, "FixturePreview"));
        assert!(workspace.resource.as_ref().unwrap()["resourceId"].as_str().unwrap().ends_with("/ar"));
        workspace.set_preview_theme("repository-theme-must-not-load".into());
        assert!(workspace.theme_id.is_empty());
        let mut app = App::new(Document::new("Underlying document"));
        workspace.pending = None; workspace.error = None; workspace.themes_loaded = false;
        workspace.poll(&mut app);
        assert!(workspace.busy.is_none() && workspace.error.is_none());
        assert!(workspace.themes.is_empty() && !workspace.themes_loaded);
    }
    #[test]
    fn registered_specimen_render_events_are_fenced_by_exact_request_session_and_scenario() {
        let mut workspace = specimen_workspace();
        let app = App::new(Document::new("Untouched"));
        let receipt = specimen_receipt(&workspace);
        assert!(workspace.accept_specimen_preview(receipt.clone(), "FixturePreview"));
        for key in ["sessionId", "requestId", "draftId", "draftRevision", "semanticDigest", "scenario"] {
            let mut stale = receipt.clone(); stale[key] = json!("obsolete");
            assert!(!workspace.matches_preview_event(&app, &stale), "{key}");
        }
        workspace.busy = Some("FixturePreview".into());
        assert!(!workspace.matches_preview_event(&app, &receipt));
        let selected = workspace.resource.clone();
        workspace.open_resource(registered_specimen("other.component", "ar"));
        assert_eq!(workspace.resource, selected, "Busy requests must retain the selected resource");
        workspace.busy = None;
        workspace.specimen_request = workspace.captured_specimen_request();
        assert!(!workspace.matches_preview_event(&app, &receipt));
        workspace.invalidate_specimen_request();
        workspace.resource = Some(registered_specimen("other.component", "ar"));
        assert!(!workspace.matches_preview_event(&app, &receipt));
    }
    fn tab_document(id:&str)->Document{
        tab_document_node(id,"material")
    }
    fn tab_document_node(id:&str,node:&str)->Document{
        let source_digest=desigma_library::payload_digest(b"fixture");
        let semantic=json!({"id":id,"revision":"1","root":{"id":node,"props":{}}});
        let draft=json!({"schemaVersion":"cityos.design-exchange.v3","semanticContract":"fixture.component","profileRef":"fixture","draftId":id,"documentId":id,
            "draftRevision":0,"base":{"kind":"canonical-git","repositoryId":"fixture","commit":"a".repeat(40),"sourcePath":"fixture.json","sourceDigest":source_digest},
            "semanticBase":semantic,"semanticDraft":semantic,"semanticDigest":"c".repeat(64),"dependencyPins":[{"path":"fixture.json","digest":source_digest}],"commandReceipts":[]});
        binding(draft,&json!({"schemaVersion":"cityos.desigma-canvas.v1","draftId":id,"draftRevision":0,"semanticDigest":"c".repeat(64),"nodes":[{"id":"native","sourceNodeId":node,"definitionId":"fixture","parent":null,"slot":null,"x":0,"y":0,"width":100,"height":100,"label":id,"fill":"#ffffff"}]})).unwrap()
    }
    #[test]
    fn explicit_local_copy_has_no_submission_binding_and_leaves_the_source_untouched() {
        let source = tab_document("canonical");
        let before = source.to_json().unwrap();
        let copy = local_copy(&source);
        assert!(copy.cityos.is_none());
        assert!(copy.nodes.values().all(|node| !node.locked && node.source_id.is_none()));
        assert_eq!(source.to_json().unwrap(), before);
        assert_eq!(copy.nodes.len(), source.nodes.len());
    }
    fn submitted_fixture() -> (std::path::PathBuf,App,desigma_library::SubmissionRecord) {
        let root=std::env::temp_dir().join(format!("desigma-submission-ui-{}",uuid::Uuid::new_v4()));
        let library=desigma_library::Library::open(&root).unwrap();
        library.retain_cityos_payload(b"fixture").unwrap();
        let mut app=App::new(tab_document("submitted"));app.lib=Some(library.clone());
        let mut edited=app.doc.clone();edited.name="Submitted design".into();
        app.hist.exec(&mut app.doc,Command::CityosInstall{document:Box::new(edited)}).unwrap();
        Workspace::save_to_draft_library(&mut app).unwrap();
        let binding=app.doc.cityos.as_ref().unwrap();let exchange=binding.exchange().unwrap();
        let command=desigma_library::SubmissionCommand{intent_id:"fixture-intent".into(),operation_id:"fixture-operation".into(),predecessor:None,
            owner:"fixture-owner".into(),resource:"submitted".into(),scope:BTreeMap::from([("tenantId".into(),"fixture-tenant".into()),("actorId".into(),"fixture-actor".into())]),
            base:binding.base.clone(),expected_revision:exchange["base"]["sourceDigest"].as_str().unwrap().into(),semantic_base:binding.semantic_base.clone(),
            submitted_snapshot:exchange["semanticDraft"].clone(),dependency_pins:binding.dependency_pins.clone()};
        let record=library.prepare_submission_for_file(&app.lib_file.as_ref().unwrap().id,command).unwrap();
        (root,app,record)
    }
    #[test]
    fn prepared_submissions_restore_freezing_and_block_semantic_and_undo_input_paths() {
        let(root,mut app,record)=submitted_fixture();let original=app.doc.to_json().unwrap();
        let mut workspace=Workspace::default();workspace.submissions.refresh_if_changed(&mut app);
        assert_eq!(workspace.submissions.records,vec![record]);assert!(app.cityos_submission_frozen());
        assert!(app.hist.can_undo());app.undo();app.redo();assert_eq!(app.doc.to_json().unwrap(),original);
        workspace.send(json!({"op":"Command"}));assert!(workspace.busy.is_none());
        assert!(workspace.error.as_ref().unwrap().contains("frozen"));
        app.lib=Some(desigma_library::Library::open(&root).unwrap());workspace.submissions=Default::default();
        workspace.submissions.refresh_if_changed(&mut app);assert!(app.cityos_submission_frozen());
        assert_eq!(app.doc.to_json().unwrap(),original);std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn accepted_generation_resumes_by_original_identity_and_seals_the_old_undo_baseline() {
        let(root,mut app,record)=submitted_fixture();let library=app.lib.as_ref().unwrap().clone();
        let mut accepted=app.doc.clone();
        let new_digest=library.retain_cityos_payload(b"accepted source").unwrap();
        let binding=accepted.cityos.as_mut().unwrap();binding.base=CityosBase::CanonicalGit{repository_id:"fixture".into(),commit:"b".repeat(40)};
        binding.semantic_base["revision"]=json!("accepted-concurrent-revision");binding.dependency_pins=vec![new_digest.clone()];
        let semantic=binding.semantic_base.clone();let value=binding.exchange_mut().unwrap();
        value["base"]["commit"]=json!("b".repeat(40));value["base"]["sourceDigest"]=json!(new_digest);
        value["dependencyPins"]=json!([{"path":"fixture.json","digest":new_digest}]);
        value["semanticBase"]=semantic.clone();value["semanticDraft"]=semantic.clone();
        value["draftRevision"]=json!(value["draftRevision"].as_u64().unwrap()+1);
        value["semanticDigest"]=json!(desigma_library::payload_digest(&serde_json::to_vec(&semantic).unwrap()));
        let accepted_record=library.record_submission_observation(desigma_library::SubmissionObservation{
            operation_id:record.command.operation_id.clone(),request_digest:record.request_digest.clone(),owner:record.command.owner.clone(),resource:record.command.resource.clone(),scope:record.command.scope.clone(),
            state:desigma_library::SubmissionState::AcceptedSyncPending,receipt_ref:"fixture-authenticated-owner-observation".into(),owner_revision:Some("b".repeat(40)),conclusively_ineligible:false,
            details:json!({"acceptedSemanticDigest":desigma_library::payload_digest(&serde_json::to_vec(&semantic).unwrap())})}).unwrap();
        let file=app.lib_file.as_ref().unwrap().id.clone();let mut checkpoint=library.load_checkpoint(&file).unwrap();checkpoint.install_baseline(accepted.clone());
        library.prepare_submission_sync(&file,&accepted_record,&checkpoint).unwrap();
        // Restart before any accepted projection is installed; recover the durable
        // generation without requesting a new proposal or owner operation.
        app.lib=Some(desigma_library::Library::open(&root).unwrap());
        let mut recovery=submission_recovery::Recovery::default();recovery.reload(&mut app);
        assert!(app.cityos_submission_frozen());assert!(recovery.recover_existing(&mut app,&accepted_record).unwrap());
        assert_eq!(app.doc.to_json().unwrap(),accepted.to_json().unwrap());assert!(!app.hist.can_undo());assert!(!app.cityos_submission_frozen());
        assert_eq!(draft(&app.doc).unwrap()["semanticBase"],draft(&app.doc).unwrap()["semanticDraft"]);
        let stored=app.lib.as_ref().unwrap().load_checkpoint(&file).unwrap();assert!(stored.retained[0].history.can_undo());
        assert_eq!(recovery.records[0].state,desigma_library::SubmissionState::Synchronized);
        let before=app.doc.to_json().unwrap();app.undo();assert_eq!(app.doc.to_json().unwrap(),before);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn unreadable_retained_submission_never_unlocks_the_current_draft() {
        let(root,mut app,record)=submitted_fixture();let library=app.lib.as_ref().unwrap();
        std::fs::write(library.path("cityos/submissions").join(format!("{}.json",record.command.operation_id)),b"incomplete receipt").unwrap();
        let mut workspace=Workspace::default();workspace.submissions.refresh_if_changed(&mut app);
        assert!(app.cityos_submission_frozen());assert!(workspace.submissions.error.is_some());
        let before=app.doc.to_json().unwrap();app.undo();workspace.send(json!({"op":"Command"}));
        assert_eq!(app.doc.to_json().unwrap(),before);assert!(workspace.busy.is_none());
        std::fs::remove_dir_all(root).unwrap();
    }
    fn specimen_projection(workspace: &Workspace) -> Value {
        let mut receipt=specimen_receipt(workspace);
        receipt["state"]=json!("projected");receipt["locale"]=json!(workspace.locale);
        receipt["projection"]=json!({"schemaVersion":"cityos.desigma-canvas.v1","draftId":receipt["draftId"],"draftRevision":0,
            "semanticDigest":receipt["semanticDigest"],"nodes":[{"id":"fixture-occurrence","sourceNodeId":"specimen-root","definitionId":"fixture.component",
                "parent":null,"slot":null,"x":0,"y":0,"width":360,"height":180,"label":"Declared registered default","fill":"#ffffff","protected":true}]});
        receipt
    }
    #[test]
    fn explicit_registered_local_copy_has_native_objects_and_no_canonical_or_submission_binding() {
        let(root,mut app,record)=submitted_fixture();let library=app.lib.as_ref().unwrap().clone();
        let file=app.lib_file.as_ref().unwrap().id.clone();let saved=library.load_checkpoint(&file).unwrap().to_json().unwrap();
        let count=library.all_files().len();let mut workspace=specimen_workspace();
        workspace.local_specimen_request=workspace.captured_specimen_request();
        let receipt=specimen_projection(&workspace);let document=workspace.local_specimen_document(receipt).unwrap();
        assert!(document.cityos.is_none());assert!(document.nodes.values().all(|node|!node.locked&&node.source_id.is_none()));
        assert!(document.nodes.values().any(|node|node.name=="Declared registered default"));
        assert_eq!(library.load_checkpoint(&file).unwrap().to_json().unwrap(),saved,"projection alone never persists a document");
        workspace.open_local_document(&mut app,document);
        assert!(!workspace.open);assert!(app.doc.cityos.is_none());assert!(app.lib_file.is_none());
        assert_eq!(library.all_files().len(),count);assert_eq!(library.load_submission(&record.command.operation_id).unwrap(),record);
        crate::home::thumb::finish_library_jobs(&root);std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn local_copy_rejects_substituted_or_stale_registered_projections_before_touching_the_document() {
        for variant in ["selection","commit","locale","digest","authority","parent","extent"] {
            let mut workspace=specimen_workspace();workspace.local_specimen_request=workspace.captured_specimen_request();
            let mut receipt=specimen_projection(&workspace);
            match variant {
                "selection"=>receipt["resource"]["componentId"]=json!("other"),
                "commit"=>receipt["provenance"]["sourceCommit"]=json!("d".repeat(40)),
                "locale"=>receipt["locale"]=json!("en"),
                "digest"=>receipt["projection"]["semanticDigest"]=json!("d".repeat(64)),
                "authority"=>receipt["ownerWrites"]=json!(true),
                "parent"=>receipt["projection"]["nodes"][0]["parent"]=json!("missing"),
                "extent"=>receipt["projection"]["nodes"][0]["width"]=json!(-1),
                _=>unreachable!(),
            }
            assert!(workspace.local_specimen_document(receipt).is_err(),"{variant}");
            assert!(workspace.preview_only);assert!(workspace.staged.is_none());assert!(workspace.tabs.is_empty());
        }
    }
    #[test]
    fn reviewed_baseline_is_durable_before_display_and_previous_undo_is_retained_inactively() {
        let root=std::env::temp_dir().join(format!("desigma-incoming-{}",uuid::Uuid::new_v4()));
        let library=desigma_library::Library::open(&root).unwrap();
        library.put_cityos_payload(&desigma_library::payload_digest(b"fixture"),b"fixture").unwrap();
        let mut app=App::new(tab_document("incoming")); app.lib=Some(library.clone()); app.home=None;
        let mut edited=app.doc.clone();edited.name="My draft".into();
        app.hist.exec(&mut app.doc,Command::CityosInstall{document:Box::new(edited)}).unwrap();
        Workspace::save_to_draft_library(&mut app).unwrap();
        let file=app.lib_file.as_ref().unwrap().id.clone();
        let mut candidate=draft(&app.doc).unwrap().clone();
        let next_revision=candidate["draftRevision"].as_u64().unwrap()+1;
        candidate["base"]["commit"]=json!("b".repeat(40));candidate["draftRevision"]=json!(next_revision);
        candidate["semanticDigest"]=json!("d".repeat(64));
        let bundle=json!({"draft":candidate,"historyBoundary":true,"resource":{"resourceId":"incoming"},
            "projection":{"schemaVersion":"cityos.desigma-canvas.v1","draftId":"incoming","draftRevision":next_revision,"semanticDigest":"d".repeat(64),
                "nodes":[{"id":"native","sourceNodeId":"material","definitionId":"fixture","parent":null,"x":0,"y":0,"width":100,"height":100,"label":"incoming","fill":"#ffffff"}]}});
        let mut workspace=Workspace::default();workspace.install_reconciliation(&mut app,bundle.clone());
        assert!(workspace.error.is_none(),"{:?}",workspace.error);assert!(!app.hist.can_undo());assert_eq!(app.doc.name,"My draft");
        let stored=library.load_checkpoint(&file).unwrap();
        assert_eq!(stored.document.to_json().unwrap(),app.doc.to_json().unwrap());
        assert_eq!(stored.retained.len(),1);assert!(stored.retained[0].history.can_undo());
        let before=app.doc.to_json().unwrap();workspace.install_reconciliation(&mut app,bundle);
        assert!(workspace.error.as_ref().unwrap().contains("Obsolete"));
        assert_eq!(app.doc.to_json().unwrap(),before);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn external_bound_autosave_persists_after_two_seconds_and_failed_close_keeps_dirty_state() {
        let root = std::env::temp_dir().join(format!("desigma-external-autosave-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let mut app = App::new(tab_document("external"));
        let path = root.join("draft.desigma");
        crate::io::save_path(&app.doc, &path).unwrap();
        app.file_path = Some(path.clone());
        app.home = None;
        app.autosave.saved_hash = crate::home::doc_hash(&app.doc);
        app.doc.name = "Edited external".into(); app.changed();
        let ctx = egui::Context::default();
        app.now = 1.; app.autosave_tick(&ctx);
        app.now = 1.5; app.autosave_tick(&ctx);
        assert_ne!(crate::io::load_path(&path).unwrap().name, app.doc.name);
        app.now = 3.1; app.autosave_tick(&ctx);
        assert_eq!(crate::io::load_path(&path).unwrap().name, "Edited external");
        assert!(!app.autosave.pending());
        let blocker = root.join("blocked"); std::fs::write(&blocker, b"file").unwrap();
        app.file_path = Some(blocker.join("draft.desigma"));
        app.doc.name = "Unsaved external".into(); app.changed();
        assert!(!app.flush_autosave()); assert!(app.autosave.pending());
        assert!(!app.replace_document(Document::new("Replacement"), None));
        assert_eq!(app.doc.name, "Unsaved external");
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn draft_tabs_save_before_switch_and_restore_undo_with_the_correct_library_file(){
        let root=std::env::temp_dir().join(format!("desigma-tabs-{}",uuid::Uuid::new_v4()));
        let mut app=App::new(tab_document("first"));app.lib=Some(desigma_library::Library::open(&root).unwrap());app.home=None;
        app.lib.as_ref().unwrap().put_cityos_payload(&desigma_library::payload_digest(b"fixture"),b"fixture").unwrap();
        let mut next=app.doc.clone();next.name="First edited".into();app.hist.exec(&mut app.doc,Command::CityosInstall{document:Box::new(next)}).unwrap();
        let mut workspace=Workspace::default();workspace.selected="material".into();workspace.suspend_current(&mut app).unwrap();let first_file=app.lib_file.clone().unwrap().id;
        app.replace_document(tab_document_node("second","accordion"),None);app.home=None;workspace.selected="accordion".into();
        workspace.activate_tab(&mut app,"first");assert_eq!(draft(&app.doc).unwrap()["draftId"],"first");assert_eq!(app.lib_file.as_ref().unwrap().id,first_file);assert!(app.hist.can_undo());
        assert_eq!(workspace.selected,"material");assert_eq!(workspace.tabs["second"].selected,"accordion");
        assert_ne!(workspace.tabs["second"].file.as_ref().unwrap().id,first_file);app.hist.undo(&mut app.doc).unwrap();
        assert_eq!(app.lib.as_ref().unwrap().load_file(&first_file).unwrap().name,"First edited");
        workspace.activate_tab(&mut app,"second");assert_eq!(workspace.selected,"accordion");
        crate::home::thumb::finish_library_jobs(&root);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn opening_a_different_projection_selects_its_own_root_without_authored_changes(){
        let root=std::env::temp_dir().join(format!("desigma-selection-{}",uuid::Uuid::new_v4()));
        let mut app=App::new(Document::new("Empty"));app.lib=Some(desigma_library::Library::open(&root).unwrap());app.home=None;
        app.lib.as_ref().unwrap().put_cityos_payload(&desigma_library::payload_digest(b"fixture"),b"fixture").unwrap();
        let document=tab_document_node("accordion-draft","accordion");let before=document.to_json().unwrap();
        let mut workspace=Workspace::default();workspace.selected="material".into();workspace.preview_only=true;workspace.staged=Some(document);
        workspace.install_staged(&mut app);
        assert!(workspace.error.is_none(),"{:?}",workspace.error);assert_eq!(workspace.selected,"accordion");
        assert_eq!(app.doc.to_json().unwrap(),before);assert!(!app.hist.can_undo());
        assert_eq!(document_selection(&app.doc,"stale-node"),"accordion");
        crate::home::thumb::finish_library_jobs(&root);std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn design_file_picker_only_builds_visible_rows_for_a_large_estate(){
        let resources:Vec<_>=(0..10_000).map(|index|json!({"label":format!("Design {index}"),"sourcePath":format!("designs/{index}.json"),"actualWebPreview":true})).collect();
        let ctx=egui::Context::default();
        let output=ctx.run_ui(egui::RawInput{screen_rect:Some(egui::Rect::from_min_size(egui::Pos2::ZERO,egui::vec2(320.,400.))),..Default::default()},|ui|{
            assert!(resource_picker(ui,&resources,None,"",false).is_none());
        });
        assert!(output.shapes.len()<150,"Offscreen design rows produced {} shapes",output.shapes.len());
    }
    #[test]
    fn preview_panel_geometry_stays_stable_across_render_status_changes(){
        for width in [1523.,1707.] {
            let ctx=egui::Context::default();let frame=eframe::Frame::_new_kittest();
            let mut app=App::new(tab_document_node("accordion-draft","accordion"));app.home=None;
            app.autosave.saved_hash=crate::home::doc_hash(&app.doc);
            let mut workspace=Workspace::default();workspace.open=true;workspace.validated=true;workspace.themes_loaded=true;
            workspace.observed=revision(&app.doc);workspace.resource=Some(json!({"label":"Accordion","actualWebPreview":true,"fixture":"cityos-accordion","editable":false}));
            workspace.preview=Some(json!({"draftRevision":0,"observedViewport":{"width":540,"height":608},"scenario":{"fixture":"cityos-accordion"}}));
            let mut sizes=Vec::new();
            for iteration in 0..20 {
                workspace.current=iteration%4==0;workspace.busy=if iteration%4==1 {Some("Preview".into())}else{None};
                workspace.render_started=if iteration%4==2 {Some(Instant::now())}else{None};workspace.pending=None;
                let _=ctx.run_ui(egui::RawInput{screen_rect:Some(egui::Rect::from_min_size(egui::Pos2::ZERO,egui::vec2(width,1019.))),..Default::default()},|ui|workspace.show(&mut app,ui,&frame));
                if iteration>=4 {sizes.push(workspace.panel_size);}
            }
            assert!(sizes.windows(2).all(|pair|pair[0]==pair[1]),"Preview geometry alternates at width {width}: {sizes:?}");
        }
    }
    #[test]
    fn failed_tab_save_preserves_the_current_document_and_history(){
        let mut app=App::new(tab_document("first"));app.home=None;app.lib=None;
        let mut next=app.doc.clone();next.name="Edited".into();app.hist.exec(&mut app.doc,Command::CityosInstall{document:Box::new(next)}).unwrap();
        let before=app.doc.to_json().unwrap();let mut workspace=Workspace::default();assert!(workspace.suspend_current(&mut app).is_err());
        assert_eq!(app.doc.to_json().unwrap(),before);assert!(app.hist.can_undo());assert!(workspace.tabs.is_empty());
    }
    #[test]
    #[ignore="requires the packaged canonical Accordion bundle; exercised by the Windows delivery check"]
    fn canonical_accordion_native_save_reopen_and_boolean_export(){
        let directory=std::path::PathBuf::from(std::env::var("CITYOS_DESIGN_WORKSPACE_EVIDENCE").unwrap());
        let bundle:Value=serde_json::from_slice(&std::fs::read(directory.join("accordion-bundle.json")).unwrap()).unwrap();
        let edited:Value=serde_json::from_slice(&std::fs::read(directory.join("accordion-edited-bundle.json")).unwrap()).unwrap();
        let document=binding(bundle["draft"].clone(),&bundle["projection"]).unwrap();let path=crate::io::save_path(&document,&directory.join("native-accordion.desigma")).unwrap();
        let mut reopened=crate::io::load_path(&path).unwrap();let mut history=desigma_doc::History::default();
        history.exec(&mut reopened,Command::CityosInstall{document:Box::new(binding(edited["draft"].clone(),&edited["projection"]).unwrap())}).unwrap();
        crate::io::save_path(&reopened,&path).unwrap();let reopened=crate::io::load_path(&path).unwrap();
        let bridge=bridge::Bridge::spawn(Config{node:std::env::var("CITYOS_DESIGN_WORKSPACE_NODE").unwrap().into(),bridge:std::path::PathBuf::from(std::env::var("CITYOS_DESIGN_WORKSPACE_PACKAGE").unwrap()).join("workspace.mjs"),
            repository:std::env::var("CITYOS_DESIGN_WORKSPACE_REPOSITORY").unwrap().into(),cache:directory.join("canonical-cache")});
        bridge.jobs.send(json!({"op":"Export","draft":draft(&reopened).unwrap()})).unwrap();
        let exported=bridge.replies.recv_timeout(Duration::from_secs(30)).unwrap().unwrap();assert_eq!(exported["operationCount"],1);assert_eq!(exported["ownerSubmission"],false);
        assert_eq!(exported["changeset"]["changes"][0]["sourcePointer"],"/root/props/allowMultiple");
    }
    #[test]
    fn resource_preview_never_replaces_the_current_document_or_starts_its_autosave() {
        let mut workspace = Workspace::default();
        let mut app = App::new(Document::new("Existing unsaved design"));
        let before = app.doc.to_json().unwrap();
        workspace.preview_only = true;
        workspace.staged = Some(Document::new("Preview of another file"));
        workspace.temporary_canvas = Some(Box::new(App::new(workspace.staged.as_ref().unwrap().clone())));
        workspace.resource = Some(json!({"references":1,"actualWebPreview":false}));
        workspace.install_staged(&mut app);
        assert_eq!(app.doc.to_json().unwrap(), before);
        assert!(workspace.staged.is_some());
        assert!(workspace.preview_only);
        assert!(!workspace.actual_preview());
        assert!(workspace.error.as_ref().unwrap().contains("unresolved"));
    }
    #[test]
    #[ignore = "requires packaged selected-resource evidence; exercised by the local delivery check"]
    fn selected_compositions_survive_native_save_reopen_and_zero_edit_export() {
        let directory = std::path::PathBuf::from(std::env::var("CITYOS_DESIGN_WORKSPACE_EVIDENCE").unwrap());
        let package = std::path::PathBuf::from(std::env::var("CITYOS_DESIGN_WORKSPACE_PACKAGE").unwrap());
        let bridge = bridge::Bridge::spawn(Config {
            node: std::env::var("CITYOS_DESIGN_WORKSPACE_NODE").unwrap().into(),
            bridge: package.join("workspace.mjs"),
            repository: std::env::var("CITYOS_DESIGN_WORKSPACE_REPOSITORY").unwrap().into(),
            cache: directory.join("canonical-cache"),
        });
        let mut count = 0;
        for entry in std::fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            if !path.file_name().unwrap().to_string_lossy().ends_with("-protected-bundle.json") {
                continue;
            }
            let bundle: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            let document = binding(bundle["draft"].clone(), &bundle["projection"]).unwrap();
            let saved = crate::io::save_path(&document, &path.with_extension("desigma")).unwrap();
            let reopened = crate::io::load_path(&saved).unwrap();
            assert_eq!(draft(&reopened).unwrap(), &bundle["draft"]);
            bridge.jobs.send(json!({"op":"Export","draft":draft(&reopened).unwrap()})).unwrap();
            assert_eq!(bridge.replies.recv_timeout(Duration::from_secs(30)).unwrap().unwrap()["operationCount"], 0);
            count += 1;
        }
        assert!(count > 1, "Requires multiple selected-resource fixtures");
    }
    #[test]
    fn presentation_scenarios_fence_theme_locale_and_viewport_changes() {
        let mut workspace = Workspace::default();
        let original = workspace.scenario();
        workspace.theme_id = "forest-green".into();
        assert_ne!(workspace.scenario(), original);
        workspace.theme_id.clear();
        workspace.viewport.0 += 1;
        assert_ne!(workspace.scenario(), original);
        workspace.viewport.0 -= 1;
        workspace.locale = "en".into();
        assert_ne!(workspace.scenario(), original);
    }
    #[test]
    fn projections_must_identify_the_exact_semantic_draft() {
        assert!(binding(json!({"schemaVersion":"cityos.design-exchange.v2","draftId":"a","semanticDigest":"x"}), &json!({"draftId":"b","semanticDigest":"x"}))
            .is_err());
    }
    #[test]
    fn reopening_cannot_reuse_prior_document_qualification_or_export() {
        let mut workspace = Workspace::default();
        workspace.validated = true;
        workspace.current = true;
        workspace.preview = Some(json!({"draftId":"previous","draftRevision":7}));
        workspace.observed = Some(("previous".into(), 7));
        workspace.pending = Some(Instant::now());
        workspace.render_started = Some(Instant::now());
        workspace.compare = Some("previous export".into());
        workspace.compare_open = true;
        workspace.busy = Some("Command".into());
        let app = App::new(Document::new("Unbound file"));
        workspace.resume(Config { node: "missing-node".into(), bridge: "missing-bridge".into(), repository: ".".into(), cache: ".".into() }, &app);
        assert!(!workspace.validated);
        assert!(!workspace.current);
        assert!(!workspace.compare_open);
        assert!(workspace.preview.is_none());
        assert!(workspace.observed.is_none());
        assert!(workspace.pending.is_none());
        assert!(workspace.render_started.is_none());
        assert!(workspace.compare.is_none());
        assert!(workspace.busy.is_none());
    }
    #[test]
    #[ignore = "requires the packaged canonical smoke bundle; exercised by the Windows delivery check"]
    fn canonical_frame_native_save_reopen_and_semantic_export() {
        let directory = std::path::PathBuf::from(std::env::var("CITYOS_DESIGN_WORKSPACE_EVIDENCE").expect("evidence directory"));
        let package = std::path::PathBuf::from(std::env::var("CITYOS_DESIGN_WORKSPACE_PACKAGE").expect("package directory"));
        let config = Config {
            node: std::path::PathBuf::from(std::env::var("CITYOS_DESIGN_WORKSPACE_NODE").expect("pinned node")),
            bridge: package.join("workspace.mjs"),
            repository: std::path::PathBuf::from(std::env::var("CITYOS_DESIGN_WORKSPACE_REPOSITORY").expect("canonical repository")),
            cache: directory.join("cache"),
        };
        let bridge = bridge::Bridge::spawn(config);
        let bundle: Value = serde_json::from_slice(&std::fs::read(directory.join("canonical-bundle.json")).unwrap()).unwrap();
        let document = binding(bundle["draft"].clone(), &bundle["projection"]).unwrap();
        let path = crate::io::save_path(&document, &directory.join("native-flat.desigma")).unwrap();
        let mut reopened = crate::io::load_path(&path).unwrap();
        bridge.jobs.send(json!({"op":"Export","draft":draft(&reopened).unwrap()})).unwrap();
        assert_eq!(bridge.replies.recv_timeout(Duration::from_secs(30)).unwrap().unwrap()["operationCount"], 0);
        let d = draft(&reopened).unwrap().clone();
        bridge.jobs.send(json!({"op":"Command","draft":d,"command":{"draftId":d["draftId"],"expectedRevision":d["draftRevision"],"operationId":"native-journey","operation":"set-property","nodeId":"material","property":"elevation","value":"resting"}})).unwrap();
        let result = bridge.replies.recv_timeout(Duration::from_secs(30)).unwrap().unwrap();
        let mut history = desigma_doc::History::default();
        history.exec(&mut reopened, Command::CityosInstall { document: Box::new(binding(result["draft"].clone(), &result["projection"]).unwrap()) }).unwrap();
        let path = crate::io::save_path(&reopened, &directory.join("native-resting.desigma")).unwrap();
        let reopened = crate::io::load_path(&path).unwrap();
        bridge.jobs.send(json!({"op":"Export","draft":draft(&reopened).unwrap()})).unwrap();
        let exported = bridge.replies.recv_timeout(Duration::from_secs(30)).unwrap().unwrap();
        assert_eq!(exported["operationCount"], 1);
        assert_eq!(exported["changeset"]["changes"][0]["field"], "elevation");
        assert_eq!(draft(&reopened).unwrap()["semanticDraft"]["root"]["slots"]["horizon"], bundle["draft"]["semanticBase"]["root"]["slots"]["horizon"]);
    }
}
