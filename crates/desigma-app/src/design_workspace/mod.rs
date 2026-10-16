//! Bound CityOS workspace: native projection and actual web renderer share one semantic draft.
mod bridge;
mod host;
use crate::app::App;
pub use bridge::Config;
use desigma_doc::{CityosBase, CityosBinding, CityosOccurrence, Color, Command, Document, Node, Paint, Rect};
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
    selected: String,
    observed: Option<(String, u64)>,
    pending: Option<Instant>,
    compare: Option<String>,
    compare_open: bool,
    expanded: bool,
    preview_visible: bool,
    validated: bool,
    harness: bool,
    viewport: (u32,u32),
    panel_size: (u32,u32),
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
            selected: "material".into(),
            observed: None,
            pending: None,
            compare: None,
            compare_open: false,
            expanded: false,
            preview_visible: true,
            validated: false,
            harness: false,
            viewport:(560,720),
            panel_size:(0,0),
        }
    }
}

fn binding(draft: Value, projection: &Value) -> Result<Document, String> {
    if draft["schemaVersion"] != "cityos.design-exchange.v2"
        || projection["draftId"] != draft["draftId"]
        || projection["semanticDigest"] != draft["semanticDigest"]
    {
        return Err("Projection/draft identity mismatch".into());
    }
    let string = |v: &Value, key: &str| v[key].as_str().map(str::to_owned).ok_or_else(|| format!("Missing {key}"));
    let mut doc = Document::new("CityOS web frame");
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
        schema_version: "cityos.design-exchange.v2".into(),
        document_id: string(&draft, "documentId")?,
        draft_id: string(&draft, "draftId")?,
        base: CityosBase::CanonicalGit { repository_id: string(&draft["base"], "repositoryId")?, commit: string(&draft["base"], "commit")? },
        semantic_base: draft["semanticBase"].clone(),
        dependency_pins: pins,
        occurrences,
        exchange_v2: Some(draft),
    });
    doc.cityos.as_ref().unwrap().validate()?;
    Ok(doc)
}
fn draft(doc: &Document) -> Option<&Value> {
    doc.cityos.as_ref()?.exchange_v2.as_ref()
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
    pub fn begin(&mut self, config: Config, preview_only: bool) {
        self.host.reset();
        self.bridge = Some(bridge::Bridge::spawn(config.clone()));
        self.config = Some(config);
        self.open = true;
        self.preview_only = preview_only;
        self.staged = None;
        self.observed = None;
        self.preview = None;
        self.error = None;
        self.validated = false;
        self.harness = false;
        self.send(json!({"op":"Open"}));
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
        self.preview = None;
        self.observed = None;
        self.validated = false;
        self.current = false;
        self.render_started = None;
        self.pending = None;
        self.error = None;
        self.compare = None;
        self.compare_open = false;
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
            self.error = Some("Open a canonical frame from the catalog to configure the bridge".into());
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
            self.send(json!({"op":"Preview","draft":draft,"scenario":{
                "target":"web","locale":self.locale,"theme":self.theme,"viewport":{"width":self.viewport.0,"height":self.viewport.1},"fixture":"cityos-web-frame"}}));
        }
    }
    fn poll(&mut self, app: &mut App) {
        let result = self.bridge.as_ref().and_then(|b| b.replies.try_recv().ok());
        if let Some(result) = result {
            let op = self.busy.take().unwrap_or_default();
            match result {
                Err(error) => {
                    self.error = Some(error);
                    self.current = false;
                }
                Ok(value) => {
                    self.error = None;
                    match op.as_str() {
                        "Harness" => {
                            self.preview = Some(value);
                            self.current = false;
                        }
                        "Open" | "Command" | "Project" => match binding(value["draft"].clone(), &value["projection"]) {
                            Err(e) => self.error = Some(e),
                            Ok(document) => {
                                if self.preview_only {
                                    self.staged = Some(document);
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
            if !matches {
                continue;
            }
            match event["kind"].as_str() {
                Some("rendered") => {
                    // A receipt identifies the observed CSS viewport as well as the
                    // requested scenario. Child-window DPI may map these differently.
                    // Retain the observation instead of recompiling into a size loop.
                    self.current = true;
                    if let Some(preview)=&mut self.preview {preview["observedViewport"]=event["viewport"].clone();preview["observedElevation"]=event["elevation"].clone();preview["observedShadow"]=event["shadow"].clone();}
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
        if self.validated && self.pending.is_some_and(|at| at.elapsed() >= Duration::from_millis(250)) && self.busy.is_none() {
            self.pending = None;
            self.request_preview(app);
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
        ctx.request_repaint_after(Duration::from_millis(50));
        if let Some(toast)=&app.toast {if app.now<toast.until {egui::Panel::bottom("cityos-workspace-save-status").show(ui,|ui|{
            ui.colored_label(if toast.error {ui.visuals().error_fg_color}else{ui.visuals().text_color()},&toast.text);
        });}}
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
                ui.heading(if self.preview_only { "CityOS · Actual web preview" } else { "CityOS · Bound design workspace" });
                if self.preview_only && !self.harness {
                    if ui.add_enabled(self.staged.is_some(), egui::Button::new("Open on canvas")).clicked() {
                        if let Some(doc) = self.staged.take() {
                            app.replace_document(doc, None);
                            app.home = None;
                            self.preview_only = false;
                            self.observed = None;
                        }
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
                ui.checkbox(&mut self.preview_visible, "Web preview");
                if ui.button(if self.expanded { "Canvas + preview" } else { "Larger preview" }).clicked() {
                    self.expanded = !self.expanded;
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
                    if let Some(operation)=&self.busy {ui.spinner();ui.label(format!("{operation}…"));}
                    else {ui.label(if self.harness {"Embedded host diagnostic"} else {"Canonical draft · local changes only"});
                        if let Some(path)=&app.file_path {ui.small(path.display().to_string());}
                    }
                });
            });
        }
        let mut bounds = None;
        if !self.expanded && !self.preview_only {
            egui::Panel::left("cityos-workspace-properties").default_size(248.).size_range(200.0..=360.0).resizable(true).show(ui,|ui|{
                ui.heading("Layers & properties");
                for(node,label)in [("frame","L9 · CityOS web frame"),("material","L1 · Material"),("horizon","L7 · Horizon")]{if ui.selectable_label(self.selected==node,label).clicked(){self.select(app,node);}}
                ui.separator();ui.label("material.elevation");
                let d=self.active_draft(app);let elevation=d.as_ref().and_then(|d|d["semanticDraft"]["root"]["slots"]["material"][0]["props"]["elevation"].as_str()).unwrap_or("flat").to_string();
                let mut value=elevation.clone();ui.add_enabled_ui(self.validated && self.busy.is_none() && self.selected=="material",|ui|{
                    ui.horizontal_wrapped(|ui|{for v in ["flat","resting","floating"]{ui.selectable_value(&mut value,v.into(),v);}});});
                if value!=elevation {if let Some(draft)=d {self.send(json!({"op":"Command","command":{"draftId":draft["draftId"],"expectedRevision":draft["draftRevision"],"operationId":uuid::Uuid::new_v4().to_string(),"operation":"set-property","nodeId":"material","property":"elevation","value":value},"draft":draft}));}}
                ui.small("Material kind, horizon layout, slots and owner actions are protected.");
                ui.separator();if let Some(d)=self.active_draft(app){ui.label(format!("Draft revision {}",d["draftRevision"]));if let Some(commit)=d["base"]["commit"].as_str(){ui.small(format!("Canonical base {}",&commit[..commit.len().min(12)]));}}
                ui.small("Canvas geometry is a design projection.");
            });
        }
        let show_side_preview = self.preview_visible && !self.expanded && !self.preview_only;
        let show_full_preview = self.expanded || self.preview_only;
        let mut web_panel = |ui: &mut egui::Ui| {
            ui.heading("Actual web renderer");
            if let Some(p) = &self.preview {
                ui.label(format!("{} · revision {}", if self.current { "Current" } else { "Rendering" }, p["draftRevision"]));
                ui.small(format!("Rendered elevation: {}",p["observedElevation"].as_str().unwrap_or("pending")));
                ui.small(format!("Viewport: {} × {} CSS px",p["observedViewport"]["width"],p["observedViewport"]["height"]));
            } else {ui.label("Waiting for preview");}
            ui.small(if self.harness { "Inert embedded-host diagnostic · no repository code" } else { "CityOS web-frame fixture · owner actions disabled" });
            let old = (self.locale.clone(), self.theme.clone());
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
            if old != (self.locale.clone(), self.theme.clone()) {
                self.current = false;
                self.pending = Some(Instant::now());
            }
            let rect=ui.available_rect_before_wrap();
            let viewport=(rect.width().round().clamp(240.,4096.) as u32,rect.height().round().clamp(240.,4096.) as u32);
            if viewport!=self.panel_size {self.panel_size=viewport;self.viewport=viewport;self.current=false;self.pending=Some(Instant::now());}
            bounds = Some(rect);
        };
        if show_side_preview {
            egui::Panel::right("cityos-workspace-preview").default_size(560.).size_range(320.0..=1400.0).resizable(true).show(ui, &mut web_panel);
        }
        if show_full_preview {
            egui::CentralPanel::default().show(ui, &mut web_panel);
        } else {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.heading("Native canvas");
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
                            self.select(app, &node);
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
        } else if self.open && self.preview_visible {
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
        workspace.resume(Config {
            node: "missing-node".into(), bridge: "missing-bridge".into(),
            repository: ".".into(), cache: ".".into(),
        }, &app);
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
