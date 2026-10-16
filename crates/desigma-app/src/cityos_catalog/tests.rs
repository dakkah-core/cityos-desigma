use super::*;
use serde_json::json;

#[test]
fn selected_workspace_requests_preserve_the_source_and_do_not_resume_unrelated_drafts() {
    let request = WorkspaceRequest::selected("config/experience/another-page.input.json", true);
    assert_eq!(request.source_path.as_deref(), Some("config/experience/another-page.input.json"));
    assert!(request.preview_only);
    assert!(!request.resume);
    let browse = WorkspaceRequest::browse();
    assert!(browse.source_path.is_none());
    assert!(!browse.resume);
}

fn entry(id: &str) -> Entry {
    serde_json::from_value(json!({"id":id,"semanticId":"token:/colors/surface/page","label":"token:/colors/surface/page","level":"L0","family":"foundations","kind":"token","owner":"design-system","package":"@cityos/design-tokens","product":null,"targets":["web"],"locales":["ar","en"],"sourcePath":"tokens.json","sourcePointer":"/colors/page","sourceDigest":"digest","observationId":"git:fixture","dependencies":[],"generationStatus":"not-applicable","classificationStatus":"classified","temporaryView":"token-sheet","persistentDraft":false,"editable":false,"rendererQualified":false,"diagnostics":[]})).unwrap()
}
fn fixture() -> (Browser, Sender<Reply>, Receiver<Job>) {
    let mut browser = Browser::new(None, None, None, None, None);
    let (jobs, requests) = mpsc::channel();
    let (responses, replies) = mpsc::channel();
    browser.jobs = jobs;
    browser.replies = replies;
    browser.open = true;
    browser.settings_open = false;
    browser.summary = Some(
        json!({"generationId":"first","snapshot":{"mode":"committed","canonicalCommit":"1234567890"},"entryCount":1,"navigation":{"levels":[{"id":"L0","name":"Foundation","count":1}],"products":[]}}),
    );
    browser.entries = vec![entry("one")];
    browser.total = 1;
    browser.displayed_filters = browser.filters.clone();
    (browser, responses, requests)
}
pub(crate) fn populated_browser() -> Browser {
    let (mut browser, _, _) = fixture();
    browser.selected = Some(entry("one"));
    browser.specimen = Some(json!({"entry":entry("one"),"data":{"$type":"color","$value":"#f8f8f3"}}));
    browser
}
#[test]
fn filter_change_cannot_install_old_results_or_old_preview() {
    let (mut browser, tx, _) = fixture();
    browser.epoch = 3;
    browser.busy = true;
    let old = browser.filters.clone();
    browser.filters.search = "new search".into();
    browser.filters_changed();
    tx.send(Reply::Query(3, old, 0, Ok(Page { entries: vec![entry("stale")], total: 1, next_offset: None }))).unwrap();
    tx.send(Reply::Materialize(2, "stale".into(), Ok(json!({"data":"stale"})))).unwrap();
    browser.poll();
    assert_eq!(browser.entries[0].id, "one");
    assert!(browser.selected.is_none());
    assert!(browser.specimen.is_none());
    assert_ne!(browser.filters, browser.displayed_filters);
}
#[test]
fn newer_selection_wins_when_materialization_is_in_flight() {
    let (mut browser, tx, _) = fixture();
    browser.epoch = 4;
    browser.busy = true;
    browser.select(entry("two"));
    tx.send(Reply::Materialize(4, "one".into(), Ok(json!({"data":"old"})))).unwrap();
    browser.poll();
    assert!(browser.specimen.is_none());
    assert!(browser.pending_selection);
    browser.tick();
    assert!(browser.busy);
    assert_eq!(browser.epoch, 5);
}
#[test]
fn pagination_appends_and_unchanged_refresh_preserves_selection() {
    let (mut browser, tx, _) = fixture();
    browser.select(entry("one"));
    browser.pending_selection = false;
    browser.epoch = 4;
    browser.busy = true;
    tx.send(Reply::Query(4, browser.filters.clone(), 1, Ok(Page { entries: vec![entry("two")], total: 2, next_offset: None }))).unwrap();
    browser.poll();
    assert_eq!(browser.entries.len(), 2);
    tx.send(Reply::Scan(4, Ok(browser.summary.clone().unwrap()))).unwrap();
    browser.poll();
    assert_eq!(browser.selected.as_ref().unwrap().id, "one");
    assert!(!browser.busy);
}
#[test]
fn source_mode_mismatch_keeps_the_actual_previous_snapshot() {
    let (mut browser, tx, _) = fixture();
    browser.mode = "local".into();
    browser.epoch = 6;
    browser.busy = true;
    tx.send(Reply::Scan(6, Ok(json!({"generationId":"wrong","snapshot":{"mode":"committed"}})))).unwrap();
    browser.poll();
    assert_eq!(browser.summary.as_ref().unwrap()["generationId"], "first");
    assert!(browser.error.is_some());
}
#[test]
fn token_previews_use_literal_values_and_keep_aliases_unresolved() {
    assert_eq!(preview::color(&json!("#f8f8f3")), Some(egui::Color32::from_rgb(248, 248, 243)));
    assert!(preview::color(&json!("{colors.primary}")).is_none());
    assert!(preview::color(&json!("#💚💚💚")).is_none());
    assert_eq!(preview::display_name(&entry("one")), "Colors surface page");
    let mut generated = entry("generated");
    generated.label = "auto.l2.control.packages-design-system".into();
    assert_eq!(preview::display_name(&generated), "Control");
    generated.label = "@{shellId=shell.mobile; name=; source=...}.shellId".into();
    assert_eq!(preview::display_name(&generated), "Shell mobile");
}
#[test]
#[cfg(windows)]
fn windows_catalog_font_fallback_contains_arabic_glyphs() {
    let ctx = egui::Context::default();
    crate::theme::install_fonts(&ctx);
    let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
        assert!(ui.fonts_mut(|fonts| fonts.has_glyphs(&egui::FontId::proportional(16.), "سيتي أو إس")));
    });
}

#[test]
fn failed_refresh_waits_after_completion_instead_of_entering_a_retry_loop() {
    let (mut browser, tx, requests) = fixture();
    browser.epoch = 6;
    browser.busy = true;
    browser.last_refresh = Instant::now() - Duration::from_secs(300);
    browser.last_interaction = Instant::now() - Duration::from_secs(300);
    tx.send(Reply::Scan(6, Err("Remote timed out".into()))).unwrap();
    browser.tick();
    assert!(!browser.busy);
    assert_eq!(browser.refresh_failures, 1);
    assert!(browser.last_refresh.elapsed() < Duration::from_secs(1));
    assert!(requests.try_recv().is_err());
    assert!(browser.summary.is_some());
}
#[test]
fn paused_catalog_does_not_schedule_background_scans() {
    let (mut browser, _, requests) = fixture();
    browser.last_refresh = Instant::now() - Duration::from_secs(3600);
    browser.last_interaction = Instant::now() - Duration::from_secs(3600);
    browser.source_changed = true;
    browser.mode = "local".into();
    browser.auto_refresh = false;
    browser.tick();
    assert!(!browser.busy);
    assert!(requests.try_recv().is_err());
    browser.auto_refresh = true;
    browser.tick();
    assert!(matches!(requests.try_recv(), Ok(Job::Scan(..))));
}
#[test]
fn inspection_canvas_has_real_native_nodes_and_no_editor_binding_or_history() {
    let e = entry("theme");
    let s = json!({"presentation":{"modes":{"light":[{"name":"colors.primary","resolvedValue":"#123456"}]}},"data":{}});
    let canvas = canvas::SpecimenCanvas::new(&e, &s, "light");
    assert!(canvas.document().cityos.is_none());
    assert!(canvas.document().nodes.values().any(|n| n.name == "colors.primary"));
    assert!(canvas.document().nodes.values().filter(|n| !canvas.document().pages.contains(&n.id)).all(|n| n.locked));
}
#[test]
fn catalog_layout_keeps_search_and_results_reachable_at_supported_sizes() {
    for size in [egui::vec2(1600.0, 1000.0), egui::vec2(900.0, 600.0), egui::vec2(680.0, 600.0)] {
        let ctx = egui::Context::default();
        let (mut browser, _, _) = fixture();
        let mut output = None;
        for _ in 0..2 {
            output = Some(ctx.run_ui(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)), ..Default::default() }, |ui| {
                browser.show(ui, &ctx)
            }));
        }
        for name in ["catalog-search", "catalog-item-one", "catalog-back"] {
            let rect: egui::Rect =
                ctx.data(|data| data.get_temp(egui::Id::new(("test-rect", name.to_string())))).unwrap_or_else(|| panic!("missing {name} at {size:?}"));
            assert!(rect.min.x >= 0.0 && rect.max.x <= size.x + 1.0 && rect.min.y >= 0.0 && rect.max.y <= size.y + 1.0, "{name} outside {size:?}: {rect:?}");
        }
        assert!(!output.unwrap().shapes.is_empty());
    }
}

fn shape_text(shape: &egui::epaint::Shape, output: &mut String) {
    match shape {
        egui::epaint::Shape::Text(text) => {
            output.push_str(&text.galley.job.text);
            output.push('\n');
        }
        egui::epaint::Shape::Vec(shapes) => {
            for shape in shapes {
                shape_text(shape, output);
            }
        }
        _ => {}
    }
}
fn has_fill(shape: &egui::epaint::Shape, expected: egui::Color32) -> bool {
    match shape {
        egui::epaint::Shape::Rect(rect) => rect.fill == expected,
        egui::epaint::Shape::Vec(shapes) => shapes.iter().any(|shape| has_fill(shape, expected)),
        _ => false,
    }
}
#[test]
fn rendered_token_data_preview_contains_its_real_swatch_and_literal_value() {
    let ctx = egui::Context::default();
    let token = entry("one");
    let output = ctx
        .run_ui(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 700.0))), ..Default::default() }, |ui| {
            preview::show(ui, &token, &json!({"$type":"color","$value":"#f8f8f3"}))
        });
    let mut text = String::new();
    for shape in &output.shapes {
        shape_text(&shape.shape, &mut text);
    }
    assert!(text.contains("#f8f8f3"));
    assert!(text.contains("Token data preview"));
    assert!(output.shapes.iter().any(|shape| has_fill(&shape.shape, egui::Color32::from_rgb(248, 248, 243))));
}
#[test]
fn composition_preview_exposes_captured_slots_and_props_without_claiming_target_rendering() {
    let ctx = egui::Context::default();
    let item = entry("composition");
    let data = json!({"composition":{"root":{"id":"frame","component":{"id":"cityos.l9.frame.v1"},"props":{},"slots":{"material":[{"id":"material","component":{"id":"cityos.l1.material.v1"},"props":{"elevation":"flat"},"slots":{}}],"horizon":[{"id":"horizon","props":{"copilotLayout":"split"},"slots":{}}]}}}});
    let output = ctx
        .run_ui(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 1000.0))), ..Default::default() }, |ui| {
            preview::show(ui, &item, &data)
        });
    let mut text = String::new();
    for shape in &output.shapes {
        shape_text(&shape.shape, &mut text);
    }
    for expected in ["Composition structure", "structure workspace preserves", "Material", "Horizon", "flat", "split"] {
        assert!(text.contains(expected), "Missing {expected}: {text}");
    }
}

fn editable_fixture() -> (Entry, Value, Value) {
    let mut e = entry("local-theme");
    e.kind = "theme".into();
    e.label = "Arctic fixture".into();
    e.source_digest = "d".repeat(64);
    let specimen = json!({"entry":e,"digest":e.source_digest,"data":{"metadata":{"id":"fixture"}},"presentation":{"modes":{
        "light":[{"name":"colors.primary","type":"color","value":"#123456","resolvedValue":"#123456"},
            {"name":"colors.link","type":"color","value":"{colors.primary}","resolvedValue":"#123456"},
            {"name":"spacing.small","type":"dimension","value":"0.5rem","resolvedValue":"0.5rem"}],
        "dark":[{"name":"colors.primary","type":"color","value":"#abcdef","resolvedValue":"#abcdef"}]}}});
    (e, specimen, json!({"mode":"committed","canonicalCommit":"a".repeat(40)}))
}
#[test]
fn explicit_native_theme_copy_has_editable_variables_aliases_samples_and_source_provenance() {
    use desigma_doc::*;
    let (e, specimen, snapshot) = editable_fixture();
    let mut doc = local_library::create(&e, &specimen, &snapshot, "light").unwrap();
    assert!(doc.cityos.is_none());
    assert_eq!(doc.variable_collections.len(), 2);
    assert_eq!(doc.variables.len(), 4);
    assert_eq!(doc.styles.len(), 3);
    assert_eq!(doc.get(doc.pages[0]).unwrap().name, "light");
    let collection = doc.variable_collections.values().find(|c| c.name.ends_with(" / light")).unwrap().clone();
    let primary = doc.variables.values().find(|v| v.collection == collection.id && v.name == "colors/primary").unwrap().id.clone();
    let alias = doc.variables.values().find(|v| v.collection == collection.id && v.name == "colors/link").unwrap();
    assert_eq!(alias.values[&collection.default_mode], VariableValue::Alias(primary.clone()));
    assert!(doc.variables.values().any(|v| v.values.values().any(|v| matches!(v,VariableValue::String(s) if s=="0.5rem"))));
    assert!(doc.nodes.values().all(|n| !n.locked));
    let sample = doc.nodes.values().find(|n| n.variables.bindings.get("fills/0") == Some(&primary)).unwrap().id;
    let before = doc.to_json().unwrap();
    let mut store = VariableStore::from_doc(&doc);
    assert!(store.set_value(&primary, &collection.default_mode, VariableValue::Color(Color::from_hex("#aabbcc").unwrap())));
    let mut history = History::default();
    history.exec(&mut doc, store.command()).unwrap();
    assert_eq!(doc.bound_value(sample, "fills/0"), Some(VariableValue::Color(Color::from_hex("#aabbcc").unwrap())));
    let reopened = Document::from_json(&doc.to_json().unwrap()).unwrap();
    assert_eq!(reopened.variables, doc.variables);
    assert!(reopened.cityos.is_none());
    assert!(reopened.variables[&primary].description.contains(&e.source_digest));
    history.undo(&mut doc).unwrap();
    assert_eq!(doc.to_json().unwrap(), before);
}
#[test]
fn native_import_blocks_local_overlays_stale_selection_unresolved_aliases_and_partial_failure() {
    let (e, specimen, mut snapshot) = editable_fixture();
    snapshot["mode"] = json!("local");
    assert!(local_library::create(&e, &specimen, &snapshot, "light").is_err());
    snapshot["mode"] = json!("committed");
    let mut stale = specimen.clone();
    stale["entry"]["id"] = json!("other");
    assert!(local_library::create(&e, &stale, &snapshot, "light").is_err());
    stale = specimen.clone();
    stale["digest"] = json!("old");
    assert!(local_library::create(&e, &stale, &snapshot, "light").is_err());
    stale = specimen.clone();
    stale["presentation"]["modes"]["light"][1]["resolvedValue"] = Value::Null;
    assert!(local_library::create(&e, &stale, &snapshot, "light").is_err());
    stale = specimen.clone();
    stale["presentation"]["modes"]["light"][1]["value"] = json!("{missing}");
    assert!(local_library::create(&e, &stale, &snapshot, "light").is_err());
}
#[test]
fn number_token_creates_a_real_editable_native_number_variable() {
    let mut e = entry("ratio");
    e.kind = "design-token".into();
    let specimen = json!({"entry":e,"digest":e.source_digest,"data":{"$type":"number","$value":3}});
    let doc = local_library::create(&e, &specimen, &json!({"mode":"committed","canonicalCommit":"b".repeat(40)}), "Value").unwrap();
    let variable = doc.variables.values().next().unwrap();
    assert_eq!(variable.kind, desigma_doc::VariableKind::Number);
    assert!(variable.values.values().any(|v| *v == desigma_doc::VariableValue::Number(3.)));
}
#[test]
fn supported_frame_specimen_links_to_actual_preview_instead_of_claiming_unavailability() {
    let mut e = entry("frame");
    e.source_path = "config/experience/living-canvas-frame/cityos-web.input.json".into();
    let ctx = egui::Context::default();
    let output = ctx.run_ui(Default::default(), |ui| preview::show(ui, &e, &json!({"root":{"id":"frame","slots":{}}})));
    let mut text = String::new();
    for shape in output.shapes {
        shape_text(&shape.shape, &mut text);
    }
    assert!(text.contains("Open actual web preview"));
    assert!(!text.contains("Target rendering is not yet available"));
}

#[test]
fn native_colors_preserve_transparency_and_reject_unsafe_css() {
    let color = local_library::parse_color("rgba(12, 24, 48, 0.15)").unwrap();
    assert_eq!(color.a, 0.15);
    assert_eq!(local_library::parse_color("#1234").unwrap().a, 68.0 / 255.0);
    for value in ["rgba(1,2,3,2)", "rgb(-1,0,0)", "rgba(NaN,0,0,1)", "var(--unresolved)", "url(file:///secret)"] {
        assert!(local_library::parse_color(value).is_none());
    }
}
#[test]
fn provider_inspector_explains_why_the_record_has_no_visual_preview() {
    let ctx = egui::Context::default();
    let output = ctx.run_ui(Default::default(), |ui| preview::show(ui, &entry("provider"), &json!({"reclassificationReason":"Context provider"})));
    let mut text = String::new();
    for shape in output.shapes {
        shape_text(&shape.shape, &mut text);
    }
    assert!(text.contains("no standalone visual appearance"));
    assert!(!text.contains("Component rendering is not yet available"));
}
#[test]
#[ignore = "requires captured repository theme fixtures; run by the local native-library delivery check"]
fn captured_repository_themes_create_editable_libraries_and_survive_save_reopen() {
    let directory = std::path::PathBuf::from(std::env::var("CITYOS_NATIVE_LIBRARY_EVIDENCE").unwrap());
    let fixture: Value = serde_json::from_slice(&std::fs::read(directory.join("fixtures.json")).unwrap()).unwrap();
    let mut reports = vec![];
    for value in fixture["themes"].as_array().unwrap() {
        let e: Entry = serde_json::from_value(value["entry"].clone()).unwrap();
        let document = local_library::create(&e, value, &fixture["snapshot"], "light").unwrap_or_else(|error| panic!("{}: {}", e.label, error));
        let rows = value["presentation"]["modes"].as_object().unwrap().values().map(|v| v.as_array().unwrap().len()).sum::<usize>();
        assert_eq!(document.variables.len(), rows);
        assert!(document.cityos.is_none());
        assert!(document.nodes.values().all(|n| !n.locked));
        let target = directory.join(format!("{}.desigma", e.semantic_id));
        let path = crate::io::save_path(&document, &target).unwrap();
        let reopened = crate::io::load_path(&path).unwrap();
        assert_eq!(reopened.variables, document.variables);
        assert_eq!(reopened.variable_collections, document.variable_collections);
        reports.push(json!({"id":e.semantic_id,"variables":rows,"collections":document.variable_collections.len(),"sourceDigest":e.source_digest,"saveReopen":true,"ownerWrites":false}));
    }
    assert!(reports.len() > 1);
    std::fs::write(directory.join("native-results.json"), serde_json::to_vec_pretty(&json!({"themes":reports,"snapshot":fixture["snapshot"]})).unwrap())
        .unwrap();
}
