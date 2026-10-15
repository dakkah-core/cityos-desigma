use super::*;
use serde_json::json;

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
    for expected in ["Composition structure", "Target rendering is not yet available", "Material", "Horizon", "flat", "split"] {
        assert!(text.contains(expected), "Missing {expected}: {text}");
    }
}
