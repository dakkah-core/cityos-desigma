//! Bound CityOS workspace: native projection and actual web renderer share one semantic draft.
mod bridge;
mod host;
use crate::app::App;
pub use bridge::Config;
use desigma_doc::{CityosBase, CityosBinding, CityosOccurrence, Color, Command, Document, Node, Paint, Rect, TextStyle};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

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
    expanded: bool,
    preview_visible: bool,
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
            expanded: false,
            preview_visible: true,
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
        }
    }
}

fn binding(draft: Value, projection: &Value) -> Result<Document, String> {
    if draft["schemaVersion"] != "cityos.design-exchange.v2"
        || !matches!(projection["schemaVersion"].as_str(), Some("cityos.desigma-canvas.v1" | "cityos.desigma-structure.v1"))
        || projection["draftId"] != draft["draftId"]
        || projection["draftRevision"] != draft["draftRevision"]
        || projection["semanticDigest"] != draft["semanticDigest"]
    {
        return Err("Projection/draft identity mismatch".into());
    }
    let string = |v: &Value, key: &str| v[key].as_str().map(str::to_owned).ok_or_else(|| format!("Missing {key}"));
    let mut doc = Document::new(draft["semanticDraft"]["id"].as_str().unwrap_or("CityOS design"));
    let page = doc.add_page("Bound canvas");
    let mut occurrences = BTreeMap::new();
    let mut ids = BTreeMap::new();
    for row in projection["nodes"].as_array().ok_or("Missing canvas projection")? {
        let id = doc.alloc_id();
        let key = string(row, "id")?;
        ids.insert(key.clone(), id);
        let number =
            |name: &str| row[name].as_f64().filter(|n| n.is_finite() && n.abs() < 10000.).map(|n| n as f32).ok_or_else(|| format!("Invalid projection {name}"));
        let parent = row["parent"].as_str().and_then(|p| ids.get(p)).copied().unwrap_or(page);
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
        self.theme_id = id;
        self.current = false;
        self.pending = Some(Instant::now());
    }
    pub fn begin(&mut self, config: Config, source: Option<String>, preview_only: bool) {
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
        self.send(json!({"op":"Resources"}));
    }
    fn open_resource(&mut self, resource: Value) {
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
        self.send(json!({"op":"Open","resource":resource}));
    }
    fn actual_preview(&self) -> bool {
        self.harness || self.resource.as_ref().is_some_and(|r| r["actualWebPreview"] == true)
    }
    fn install_staged(&mut self, app: &mut App) {
        if self.staged.is_none() {
            return;
        }
        if self.resource.as_ref().is_some_and(|r| r["references"].as_u64().unwrap_or(0) > 0) {
            self.error = Some("Reusable dependencies are unresolved. This file remains a temporary structure preview.".into());
            return;
        }
        // Previewing another file never replaces the editor document. An explicit
        // open must preserve the current document, including failed saves.
        if app.home.is_none() && (app.doc.cityos.is_some() || app.hist.can_undo()) {
            let saved = if app.lib_file.is_some() {
                app.save_library_file().map(|_| ())
            } else if let Some(path) = &app.file_path {
                crate::io::save_path(&app.doc, path).map(|_| ())
            } else {
                Err("Save the current draft before opening another file. Its preview remains available.".into())
            };
            if let Err(error) = saved {
                self.error = Some(error);
                return;
            }
        }
        let doc = self.staged.take().unwrap();
        app.replace_document(doc, None);
        app.home = None;
        self.preview_only = false;
        self.temporary_canvas = None;
        self.observed = None;
        self.error = None;
    }
    pub fn begin_harness(&mut self, config: Config) {
        self.host.reset();
        self.bridge = Some(bridge::Bridge::spawn(config.clone()));
        self.config = Some(config);
        self.busy = None;
        self.open = true;
        self.preview_only = true;
        self.harness = true;
        self.validated = false;
        self.preview = None;
        self.staged = None;
        self.error = None;
        self.send(json!({"op":"Harness"}));
    }
    pub fn resume(&mut self, config: Config, app: &App) {
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
    fn send(&mut self, value: Value) {
        if self.busy.is_some() {
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
        if bridge.jobs.send(value).is_err() {
            self.busy = None;
            self.error = Some("Exchange helper unavailable".into());
        }
    }
    fn active_draft(&self, app: &App) -> Option<Value> {
        if self.preview_only {
            self.staged.as_ref().and_then(draft).cloned()
        } else {
            draft(&app.doc).cloned()
        }
    }
    fn request_preview(&mut self, app: &App) {
        if let Some(draft) = self.active_draft(app) {
            self.current = false;
            self.send(json!({"op":"Preview","draft":draft,"scenario":self.scenario()}));
        }
    }
    fn scenario(&self) -> Value {
        json!({"target":"web","locale":self.locale,"theme":self.theme,"themeId":self.theme_id,
        "viewport":{"width":self.viewport.0,"height":self.viewport.1},"fixture":self.resource.as_ref().and_then(|r|r["fixture"].as_str()).unwrap_or("cityos-web-frame")})
    }
    fn poll(&mut self, app: &mut App) {
        let result = self.bridge.as_ref().and_then(|b| b.replies.try_recv().ok());
        if let Some(result) = result {
            let op = self.busy.take().unwrap_or_default();
            match result {
                Err(error) => {
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
                            self.resources = value["resources"].as_array().cloned().unwrap_or_default();
                            self.resource_diagnostics = value["diagnostics"].as_array().cloned().unwrap_or_default();
                            if let Some(source) = self.requested_source.take() {
                                if let Some(resource) = self.resources.iter().find(|r| r["sourcePath"] == source).cloned() {
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
                        }
                        "Open" | "Command" | "Project" => match binding(value["draft"].clone(), &value["projection"]) {
                            Err(e) => self.error = Some(e),
                            Ok(document) => {
                                self.resource = Some(value["resource"].clone());
                                if self.preview_only {
                                    self.temporary_canvas = Some(Box::new(App::new(document.clone())));
                                    self.staged = Some(document);
                                    if self.open_after_preview {
                                        self.open_after_preview = false;
                                        self.install_staged(app);
                                    }
                                } else if op == "Open" {
                                    app.replace_document(document, None);
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
                                    app.doc = document;
                                    app.changed();
                                }
                                self.validated = true;
                                self.observed = None;
                                self.pending = Some(Instant::now());
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
                        "Export" => {
                            self.compare = Some(serde_json::to_string_pretty(&value).unwrap_or_default());
                            self.compare_open = true;
                        }
                        _ => {}
                    }
                }
            }
        }
        for event in self.host.poll() {
            let matches =
                self.preview.as_ref().is_some_and(|p| {
                    ["sessionId", "requestId", "draftId", "draftRevision", "semanticDigest", "scenario"].iter().all(|key| p[*key] == event[*key])
                }) && self.active_draft(app).is_some_and(|d| {
                    d["draftId"] == event["draftId"] && d["draftRevision"] == event["draftRevision"] && d["semanticDigest"] == event["semanticDigest"]
                }) && event["scenario"]["locale"] == self.locale
                    && event["scenario"]["theme"] == self.theme;
            let matches = matches && event["scenario"] == self.scenario();
            if !matches {
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
                }
                Some("selection") => {
                    if let Some(node) = event["nodeId"].as_str() {
                        self.select(app, node);
                    }
                }
                Some("shortcut") if !self.preview_only && self.busy.is_none() => match event["key"].as_str() {
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
        if self.render_started.is_some_and(|at| at.elapsed() > Duration::from_secs(30)) {
            self.error = Some("Renderer did not acknowledge this revision; restart preview to retry".into());
            self.render_started = None;
            self.current = false;
        }
        let identity = if self.preview_only { self.staged.as_ref().and_then(revision) } else { revision(&app.doc) };
        if identity != self.observed {
            self.observed = identity;
            self.current = false;
            self.pending = Some(Instant::now());
        }
        if self.validated && self.actual_preview() && !self.themes_loaded && self.busy.is_none() {
            if let Some(d) = self.active_draft(app) {
                self.send(json!({"op":"Themes","draft":d}));
            }
        } else if self.validated && self.actual_preview() && self.pending.is_some_and(|at| at.elapsed() >= Duration::from_millis(250)) && self.busy.is_none() {
            self.pending = None;
            self.request_preview(app);
        }
        if !self.actual_preview() {
            self.pending = None;
        }
    }
    fn select(&mut self, app: &mut App, node: &str) {
        self.selected = node.into();
        if let Some(id) = app.doc.nodes.values().find(|n| n.source_id.as_deref() == Some(node)).map(|n| n.id) {
            app.select_only(id);
        }
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
        self.poll(app);
        let ctx = ui.ctx().clone();
        ctx.request_repaint_after(Duration::from_millis(if self.busy.is_some() || self.pending.is_some() || self.render_started.is_some() {
            100
        } else {
            1000
        }));
        if let Some(toast) = &app.toast {
            if app.now < toast.until {
                egui::Panel::bottom("cityos-workspace-save-status").show(ui, |ui| {
                    ui.colored_label(if toast.error { ui.visuals().error_fg_color } else { ui.visuals().text_color() }, &toast.text);
                });
            }
        }
        if !self.preview_only {
            app.autosave_tick(&ctx);
            if ctx.input(|i| i.viewport().close_requested()) {
                app.flush_autosave();
            }
        }
        if !ctx.egui_wants_keyboard_input() && self.busy.is_none() {
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
                if ui.button("← Design library").clicked() {
                    self.open = false;
                }
                ui.heading(if self.preview_only { "CityOS · Design preview" } else { "CityOS · Bound design workspace" });
                if ui.add_enabled(self.busy.is_none(), egui::Button::new("Choose design file…")).clicked() {
                    self.picker_open = !self.picker_open;
                    if self.resources.is_empty() {
                        self.send(json!({"op":"Resources"}));
                    }
                }
                if self.preview_only && !self.harness {
                    if ui
                        .add_enabled(
                            self.staged.is_some() && self.busy.is_none(),
                            egui::Button::new(if self.actual_preview() { "Open on canvas" } else { "Open protected canvas" }),
                        )
                        .clicked()
                    {
                        self.install_staged(app);
                    }
                    if app.home.is_none() && (app.doc.cityos.is_some() || app.hist.can_undo()) && ui.button("Save current draft").clicked() {
                        self.host.hide();
                        app.save(false);
                    }
                } else if !self.preview_only {
                    if ui.button("Save draft").clicked() {
                        self.host.hide();
                        app.save(false);
                    }
                    if ui.button("Open draft…").clicked() {
                        self.host.hide();
                        app.open_dialog();
                        // A loaded file never inherits the previous document's validation,
                        // preview receipts or outstanding bridge responses.
                        if let Some(config) = self.config.clone() {
                            self.resume(config, app);
                        }
                    }
                    if ui.add_enabled(self.validated && self.busy.is_none(), egui::Button::new("Compare / export")).clicked() {
                        if let Some(draft) = self.active_draft(app) {
                            self.send(json!({"op":"Export","draft":draft}));
                        }
                    }
                    if ui.add_enabled(self.validated && self.busy.is_none() && app.hist.can_undo(), egui::Button::new("Undo")).clicked() {
                        app.undo();
                    }
                    if ui.add_enabled(self.validated && self.busy.is_none() && app.hist.can_redo(), egui::Button::new("Redo")).clicked() {
                        app.redo();
                    }
                    ui.add_enabled(false, egui::Button::new("Submit proposal"))
                        .on_hover_text("Engineering owner publication and merge qualification remains required");
                }
                if ui.button("Reset panels").clicked() {
                    self.expanded = false;
                    self.preview_visible = true;
                    ctx.memory_mut(|m| m.data.clear());
                }
                if self.actual_preview() {
                    ui.checkbox(&mut self.preview_visible, "Web preview");
                    if ui.button(if self.expanded { "Canvas + preview" } else { "Larger preview" }).clicked() {
                        self.expanded = !self.expanded;
                    }
                }
            });
        });
        if let Some(error) = self.error.clone() {
            egui::Panel::top("cityos-workspace-error").show(ui, |ui| {
                ui.colored_label(ui.visuals().error_fg_color, &error);
            });
        }
        {
            egui::Panel::top("cityos-workspace-busy").exact_size(38.).show(ui, |ui| {
                ui.horizontal(|ui| {
                    if let Some(operation) = &self.busy {
                        ui.spinner();
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
                ui.small("Select to preview. Open on canvas when ready.");
                ui.text_edit_singleline(&mut self.resource_search);
                let search = self.resource_search.to_lowercase();
                let mut selected = None;
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for resource in &self.resources {
                        let path = resource["sourcePath"].as_str().unwrap_or("");
                        let label = resource["label"].as_str().unwrap_or(path);
                        if !format!("{path} {label}").to_lowercase().contains(&search) {
                            continue;
                        }
                        ui.add_enabled_ui(self.busy.is_none(), |ui| {
                            if ui.selectable_label(self.resource.as_ref().is_some_and(|r| r["sourcePath"] == path), label).on_hover_text(path).clicked() {
                                selected = Some(resource.clone());
                            }
                        });
                        ui.small(if resource["actualWebPreview"] == true {
                            "Actual web preview · canvas edit"
                        } else {
                            "Authored structure · renderer unavailable"
                        });
                        ui.separator();
                    }
                    for finding in &self.resource_diagnostics {
                        ui.colored_label(ui.visuals().error_fg_color, format!("{}: {}", finding["sourcePath"], finding["error"]));
                    }
                });
                if let Some(resource) = selected {
                    self.open_after_preview = false;
                    self.open_resource(resource);
                }
            });
        }
        if let Some(resource) = self.resource.clone() {
            egui::Panel::top("cityos-selected-resource").show(ui, |ui| {
                ui.strong(resource["resourceId"].as_str().unwrap_or("Selected design"));
                ui.small(resource["sourcePath"].as_str().unwrap_or(""));
                if let Some(commit) = resource["commit"].as_str() {
                    ui.small(format!("Canonical GitLab commit {}", &commit[..commit.len().min(12)]));
                }
                if let Some(messages) = resource["diagnostics"].as_array() {
                    for message in messages {
                        ui.label(message.as_str().unwrap_or(""));
                    }
                }
            });
        }
        let mut bounds = None;
        if (!self.expanded && !self.preview_only) || (self.preview_only && !self.actual_preview() && self.validated) {
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
                    ui.add_enabled_ui(self.validated&&self.busy.is_none()&&self.selected==field["nodeId"].as_str().unwrap_or(""),|ui|{
                        if let Some(mut boolean)=value.as_bool(){if ui.checkbox(&mut boolean,"Allow multiple open sections").changed(){value=json!(boolean);}}
                        else if let Some(values)=field["values"].as_array(){ui.horizontal_wrapped(|ui|{for option in values {ui.selectable_value(&mut value,option.clone(),option.as_str().unwrap_or(""));}});}
                    });
                    if value!=previous {self.send(json!({"op":"Command","command":{"draftId":d["draftId"],"expectedRevision":d["draftRevision"],"operationId":uuid::Uuid::new_v4().to_string(),"operation":"set-property","nodeId":field["nodeId"],"property":field["property"],"value":value},"draft":d}));}
                }}
                ui.small("Only the declared properties are editable. Slots and owner actions remain protected.");
                } else {ui.small("Protected source nodes. No editing adapter is enabled for this file.");}
                ui.separator();if let Some(d)=self.active_draft(app){ui.label(format!("Draft revision {}",d["draftRevision"]));if let Some(commit)=d["base"]["commit"].as_str(){ui.small(format!("Canonical base {}",&commit[..commit.len().min(12)]));}}
                ui.small("Canvas geometry is a design projection.");
            });
        }
        let show_side_preview = self.actual_preview() && self.preview_visible && !self.expanded && !self.preview_only;
        let show_full_preview = self.actual_preview() && (self.expanded || self.preview_only);
        let mut web_panel = |ui: &mut egui::Ui| {
            ui.heading("Actual web renderer");
            if let Some(p) = &self.preview {
                ui.label(format!("{} · revision {}", if self.current { "Current" } else { "Rendering" }, p["draftRevision"]));
                ui.small(format!("Rendered elevation: {}", p["observedElevation"].as_str().unwrap_or("pending")));
                ui.small(format!("Viewport: {} × {} CSS px", p["observedViewport"]["width"], p["observedViewport"]["height"]));
            } else {
                ui.label("Waiting for preview");
            }
            ui.small(if self.harness { "Inert embedded-host diagnostic · no repository code" } else { "Registered CityOS renderer · deterministic fixture · owner actions disabled" });
            let old = (self.locale.clone(), self.theme.clone(), self.theme_id.clone());
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
            ui.horizontal_wrapped(|ui| {
                ui.selectable_value(&mut self.locale, "ar".into(), "Arabic · RTL");
                ui.selectable_value(&mut self.locale, "en".into(), "English · LTR");
                ui.selectable_value(&mut self.theme, "light".into(), "Light");
                ui.selectable_value(&mut self.theme, "dark".into(), "Dark");
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
            if old != (self.locale.clone(), self.theme.clone(), self.theme_id.clone()) {
                self.current = false;
                self.pending = Some(Instant::now());
            }
            let rect = ui.available_rect_before_wrap();
            let viewport = (rect.width().round().clamp(240., 4096.) as u32, rect.height().round().clamp(240., 4096.) as u32);
            if viewport != self.panel_size {
                self.panel_size = viewport;
                self.viewport = viewport;
                self.current = false;
                self.pending = Some(Instant::now());
            }
            bounds = Some(rect);
        };
        if show_side_preview {
            egui::Panel::right("cityos-workspace-preview").default_size(560.).size_range(320.0..=1400.0).resizable(true).show(ui, &mut web_panel);
        }
        if show_full_preview {
            egui::CentralPanel::default().show(ui, &mut web_panel);
        } else {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.heading(if self.actual_preview() { "Native canvas" } else { "Protected composition structure" });
                if !self.validated {
                    if self.busy.is_some() {
                        ui.spinner();
                    }
                    ui.label(if self.busy.is_some() { "Loading selected source…" } else { "Choose a design file to preview before opening it on canvas." });
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
        if !self.preview_visible || !self.open {
            self.host.hide();
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
        } else if self.open && self.preview_visible && self.actual_preview() {
            if let (Some(rect), Some(url)) = (bounds, self.preview.as_ref().and_then(|p| p["url"].as_str())) {
                if let Err(error) = self.host.show(frame, url, rect, ctx.pixels_per_point()) {
                    self.error = Some(error);
                }
            }
        }
        if let Some(version) = &self.host.version {
            egui::Area::new("cityos-webview-version".into()).anchor(egui::Align2::RIGHT_BOTTOM, [-8., -4.]).show(&ctx, |ui| {
                ui.small(format!("WebView2 {version}"));
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
