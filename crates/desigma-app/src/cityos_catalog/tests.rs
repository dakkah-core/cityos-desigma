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

fn profile_summary() -> Value {
    json!({"snapshot":{"mode":"committed","canonicalCommit":"a".repeat(40)},"workspaceProfiles":[
        {"sourcePath":"config/experience/living-canvas-frame/cityos-web.input.json","kind":"composition","fixture":"cityos-web-frame"},
        {"sourcePath":"packages/design-system/src/components/ui/accordion.tsx","publicExport":"Accordion","kind":"component-specimen","fixture":"cityos-accordion"}
    ]})
}

#[test]
fn registered_default_inline_preview_preserves_the_full_locator_and_requires_matching_implementation_bytes() {
    let mut selected=entry("context"); selected.kind="source-file".into();
    selected.source_path="packages/runtime/src/ContextCapsule.tsx".into(); selected.source_digest="b".repeat(64);
    let profile=json!({"kind":"registered-specimen","sourcePath":selected.source_path,"publicExport":"ContextCapsule",
        "sourceDigest":selected.source_digest,"implementationDigest":selected.source_digest,"sourcePaths":[selected.source_path,"packages/ui/src/ContextCapsule.tsx"],
        "componentId":"fixture.context","componentVersion":"1","profileRef":"fixture-profile","targetProfileRef":"web-fixture",
        "resourceId":"fixture-context-ar","fixture":"cityos-shared-leaf","actualWebPreview":true,"editable":false,
        "provenance":{"canonicalResource":false,"temporary":true,"locale":"ar","sourceCommit":"a".repeat(40)}});
    let mut summary=json!({"snapshot":{"mode":"committed","canonicalCommit":"a".repeat(40)},"workspaceProfiles":[profile]});
    let actions=support::actions(&selected,None,Some(&summary),"committed");
    assert!(actions.actual_preview); assert!(!actions.editable);
    assert_eq!(actions.inline_pin,Some(profile.clone()));
    assert!(support::actions(&selected,None,Some(&summary),"local").inline_pin.is_none());
    let mut registration=selected.clone(); registration.kind="registered-component".into();
    registration.source_path="registrations.json".into(); registration.source_digest="c".repeat(64); registration.label="ContextCapsule".into();
    let mut specimen=json!({"entry":registration,"digest":registration.source_digest,"data":{"name":"ContextCapsule","sourcePath":"packages/ui/src/ContextCapsule.tsx"},"workspaceProfile":profile});
    assert!(support::actions(&registration,Some(&specimen),Some(&summary),"committed").actual_preview);
    specimen["data"]["name"]=json!("UnrelatedExport"); registration.label="UnrelatedExport".into();
    assert!(!support::actions(&registration,Some(&specimen),Some(&summary),"committed").actual_preview);
    summary["workspaceProfiles"][0]["implementationDigest"]=json!("c".repeat(64));
    assert!(!support::actions(&selected,None,Some(&summary),"committed").actual_preview);
    summary["workspaceProfiles"][0]=profile;
    selected.source_digest="c".repeat(64);
    assert!(support::actions(&selected,None,Some(&summary),"committed").inline_pin.is_none());
}

#[test]
fn registered_selection_actions_exist_before_materialization_without_loading_an_unrelated_frame() {
    let summary = profile_summary();
    let mut selected = entry("frame");
    selected.kind = "authored-composition".into();
    selected.source_path = "config/experience/living-canvas-frame/cityos-web.input.json".into();
    let actions = support::actions(&selected, None, Some(&summary), "committed");
    assert_eq!(actions.source.as_deref(), Some(selected.source_path.as_str()));
    assert!(actions.actual_preview && actions.editable);
    assert!(actions.inline_pin.is_none(), "An unloaded selection has no captured renderer digest");
    selected.source_path = "config/experience/another-page.input.json".into();
    let actions = support::actions(&selected, None, Some(&summary), "committed");
    assert_eq!(actions.source.as_deref(), Some(selected.source_path.as_str()));
    assert!(!actions.actual_preview && !actions.editable);
    assert!(actions.preview_reason.contains("No actual renderer"));
    selected.kind = "registration".into();
    assert!(support::actions(&selected, None, Some(&summary), "committed").source.is_none());
}

#[test]
fn component_selection_uses_the_registered_export_and_rejects_other_exports_or_missing_module_identity() {
    let summary = profile_summary();
    let mut selected = entry("accordion");
    selected.kind = "public-declaration".into();
    selected.label = "Accordion".into();
    selected.source_path = "packages/design-system/src/components/ui/accordion.tsx".into();
    let actions = support::actions(&selected, None, Some(&summary), "committed");
    assert_eq!(actions.source.as_deref(), Some(selected.source_path.as_str()));
    assert!(actions.actual_preview && actions.editable);
    selected.label = "AccordionItem".into();
    assert!(support::actions(&selected, None, Some(&summary), "committed").source.is_none());
    selected.label = "Accordion".into();
    selected.source_path = "an-unrelated-file.json".into();
    let specimen = json!({"entry":selected,"digest":selected.source_digest,"data":{"name":"Accordion"},
        "workspaceProfile":{"sourcePath":"not-a-design-system-file.tsx","publicExport":"Accordion","fixture":"cityos-accordion"}});
    assert!(support::actions(&selected, Some(&specimen), None, "committed").source.is_none());
}

#[test]
fn registered_component_records_resolve_only_their_captured_module_and_export() {
    let summary = profile_summary();
    let mut selected = entry("accordion-record");
    selected.kind = "registered-component".into();
    selected.label = "Accordion".into();
    selected.dependencies = vec!["components/ui/accordion.tsx".into()];
    let unloaded = support::actions(&selected, None, Some(&summary), "committed");
    assert_eq!(unloaded.source.as_deref(), Some("packages/design-system/src/components/ui/accordion.tsx"));
    selected.dependencies.clear();
    let mut specimen = json!({"entry":selected,"digest":selected.source_digest,"data":{"name":"Accordion","file":"components/ui/accordion.tsx"},
        "workspaceProfile":summary["workspaceProfiles"][1]});
    let actions = support::actions(&selected, Some(&specimen), Some(&summary), "committed");
    assert_eq!(actions.source.as_deref(), Some("packages/design-system/src/components/ui/accordion.tsx"));
    specimen["data"]["file"] = json!("components/ui/another.tsx");
    assert!(support::actions(&selected, Some(&specimen), Some(&summary), "committed").source.is_none());
}

#[test]
fn inline_renderer_requires_matching_selection_and_exact_captured_commit_and_digest() {
    let summary = profile_summary();
    let mut selected = entry("frame");
    selected.kind = "authored-composition".into();
    selected.source_path = "config/experience/living-canvas-frame/cityos-web.input.json".into();
    let mut specimen = json!({"entry":selected,"digest":selected.source_digest,"data":{},"workspaceProfile":summary["workspaceProfiles"][0]});
    specimen["workspaceProfile"]["sourceDigest"] = json!("b".repeat(64));
    let actions = support::actions(&selected, Some(&specimen), Some(&summary), "committed");
    let pin = actions.inline_pin.unwrap();
    assert_eq!(pin["sourcePath"], selected.source_path);
    assert_eq!(pin["sourceDigest"], "b".repeat(64));
    for field in ["actualWebPreview", "editable"] { specimen["workspaceProfile"][field] = json!(false); }
    let actions = support::actions(&selected, Some(&specimen), Some(&summary), "committed");
    assert!(!actions.actual_preview && !actions.editable && actions.inline_pin.is_none());
    specimen["workspaceProfile"]["actualWebPreview"] = json!(true);
    specimen["workspaceProfile"]["sourceDigest"] = json!("short");
    assert!(support::actions(&selected, Some(&specimen), Some(&summary), "committed").inline_pin.is_none());
    specimen["workspaceProfile"]["sourceDigest"] = json!("b".repeat(64));
    specimen["entry"]["id"] = json!("other");
    assert!(support::actions(&selected, Some(&specimen), Some(&summary), "committed").inline_pin.is_none());
    specimen["entry"]["id"] = json!(selected.id);
    assert!(support::actions(&selected, Some(&specimen), Some(&summary), "local").inline_pin.is_none());
    let mut invalid_commit = summary.clone();
    invalid_commit["snapshot"]["canonicalCommit"] = json!("short");
    assert!(support::actions(&selected, Some(&specimen), Some(&invalid_commit), "committed").inline_pin.is_none());
}

#[test]
fn every_level_reports_captured_support_and_preserves_missing_coverage_as_unknown() {
    let summary = json!({"navigation":{"levels":[
        {"id":"L0","name":"Foundations","count":12,"temporaryViewCount":10,"rendererCount":0,"editableCount":0},
        {"id":"L9","name":"Frame","count":0}
    ]}});
    let levels = support::levels(Some(&summary));
    assert_eq!(levels.len(), 10);
    assert_eq!(levels[0].count, Some(12));
    assert_eq!(levels[0].specimens, Some(10));
    assert_eq!(levels[0].renderers, Some(0));
    assert_eq!(levels[0].editable, Some(0));
    assert_eq!(levels[9].count, Some(0));
    assert!(levels[9].editable.is_none());
    for (n, level) in levels.iter().enumerate().skip(1).take(8) {
        assert_eq!(level.id, format!("L{n}"));
        assert!(level.count.is_none() && level.renderers.is_none());
    }
    assert!(support::levels(None).iter().all(|l| l.count.is_none()));
}

#[test]
fn theme_actions_explain_unloaded_unselectable_stale_and_local_sources() {
    let (selected, mut specimen, snapshot) = editable_fixture();
    let summary = json!({"snapshot":snapshot});
    let unloaded = support::actions(&selected, None, Some(&summary), "committed");
    assert!(unloaded.copy_reason.unwrap().contains("Load the selected values"));
    assert!(unloaded.theme_reason.unwrap().contains("Load this theme"));
    let ready = support::actions(&selected, Some(&specimen), Some(&summary), "committed");
    assert!(ready.copy_reason.is_none() && ready.theme_reason.is_none());
    specimen["data"]["metadata"]["selectable"] = json!(false);
    assert!(support::actions(&selected, Some(&specimen), Some(&summary), "committed").theme_reason.unwrap().contains("non-selectable"));
    specimen["entry"]["id"] = json!("other");
    let stale = support::actions(&selected, Some(&specimen), Some(&summary), "committed");
    assert!(stale.copy_reason.unwrap().contains("selected source changed"));
    assert!(stale.theme_reason.unwrap().contains("no longer matches"));
    let local = support::actions(&selected, Some(&specimen), Some(&summary), "local");
    assert!(local.copy_reason.unwrap().contains("Switch to GitLab main"));
    assert!(local.theme_reason.unwrap().contains("Switch to GitLab main"));
}

#[test]
fn nonvisual_dependencies_never_offer_a_visual_fixture_even_when_profile_metadata_conflicts() {
    let mut selected = entry("provider");
    selected.kind = "nonvisual-dependency".into();
    let summary = json!({"workspaceProfiles":[{"sourcePath":selected.source_path,"kind":"composition","actualWebPreview":true,"editable":true}]});
    let actions = support::actions(&selected, None, Some(&summary), "committed");
    assert!(actions.source.is_none() && !actions.actual_preview && !actions.editable);
    assert!(actions.preview_reason.contains("no standalone visual appearance"));
}

#[test]
fn mismatched_materialization_identity_cannot_replace_the_selected_resource() {
    for mismatch in ["entry", "digest"] {
        let (mut browser, responses, _) = fixture();
        browser.select(entry("one"));
        browser.epoch = 4;
        browser.busy = true;
        let mut value = json!({"entry":entry("one"),"digest":"digest","data":{"$value":3}});
        if mismatch == "entry" { value["entry"]["id"] = json!("other"); } else { value["digest"] = json!("other"); }
        responses.send(Reply::Materialize(4, "one".into(), Ok(value))).unwrap();
        browser.poll();
        assert_eq!(browser.selected.as_ref().unwrap().id, "one");
        assert!(browser.specimen.is_none());
        assert!(browser.preview_error.as_ref().unwrap().contains("different source identity"));
    }
}

#[test]
fn navigation_history_restores_selection_filters_mode_and_detail_without_reusing_an_old_specimen() {
    let (mut browser, _, _) = fixture();
    browser.select(entry("one"));
    browser.detail_tab = DetailTab::Inspector;
    browser.theme_mode = "dark".into();
    browser.filters = Filters { collection:"themes".into(), ..Default::default() };
    browser.filters_changed();
    assert!(browser.selected.is_none());
    browser.navigate_history(false);
    assert_eq!(browser.filters.collection, "design");
    assert_eq!(browser.selected.as_ref().unwrap().id, "one");
    assert_eq!(browser.theme_mode, "dark");
    assert!(browser.detail_tab == DetailTab::Inspector);
    assert!(!browser.results_visible);
    assert!(browser.specimen.is_none() && browser.pending_selection);
    browser.navigate_history(true);
    assert_eq!(browser.filters.collection, "themes");
    assert!(browser.selected.is_none() && browser.results_visible);
    browser.filters.search = "new".into();
    browser.filters_changed();
    assert!(browser.navigation_forward.is_empty());
}

#[test]
fn typing_during_debounce_keeps_one_back_location_and_clears_stale_inline_status() {
    let (mut browser, _, _) = fixture();
    browser.inline_status = "Current · old selection".into();
    browser.filters.search = "a".into(); browser.filters_changed();
    browser.filters.search = "ab".into(); browser.filters_changed();
    assert_eq!(browser.navigation_back.len(), 1);
    assert!(browser.inline_status.is_empty());
    browser.navigate_history(false);
    assert!(browser.filters.search.is_empty());
}

#[test]
fn search_coalesces_keystrokes_and_dispatches_before_the_interaction_budget() {
    let (mut browser,_responses,requests)=fixture();
    browser.summary=Some(json!({})); browser.auto_refresh=false;
    browser.filters.search="context".into(); browser.filters_changed();
    browser.filters.search="context capsule".into(); browser.filters_changed();
    browser.filter_changed=Some(Instant::now()-Duration::from_millis(125));
    browser.tick();
    match requests.try_recv().expect("Search should dispatch within the 150 ms interaction budget") {
        Job::Query(_,filters,offset)=>{assert_eq!(filters.search,"context capsule");assert_eq!(offset,0);},
        _=>panic!("Only the final query should run"),
    }
    assert!(requests.try_recv().is_err());
}

fn catalog_frame(browser: &mut Browser, ctx: &egui::Context, size: egui::Vec2, events: Vec<egui::Event>) -> egui::FullOutput {
    ctx.run_ui(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)), events, ..Default::default() }, |ui| browser.show(ui, ctx))
}

fn click_catalog(browser: &mut Browser, ctx: &egui::Context, size: egui::Vec2, id: &str) {
    ctx.all_styles_mut(|style| style.animation_time = 0.0);
    catalog_frame(browser, ctx, size, vec![]);
    catalog_frame(browser, ctx, size, vec![]);
    let rect: egui::Rect = ctx.data(|data| data.get_temp(egui::Id::new(("test-rect", id.to_string())))).unwrap_or_else(|| panic!("Missing {id}"));
    let pos = rect.center();
    for pressed in [true, false] {
        catalog_frame(browser, ctx, size, vec![egui::Event::PointerMoved(pos), egui::Event::PointerButton {
            pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE,
        }]);
    }
}

#[test]
fn clicking_preview_before_materialization_requests_the_selected_resource() {
    let ctx = egui::Context::default();
    let (mut browser, _, _) = fixture();
    browser.summary = Some(profile_summary());
    let mut selected = entry("selected");
    selected.kind = "public-declaration".into();
    selected.label = "Accordion".into();
    selected.source_path = "packages/design-system/src/components/ui/accordion.tsx".into();
    browser.select(selected.clone());
    click_catalog(&mut browser, &ctx, egui::vec2(1600., 1000.), "catalog-preview-action");
    let request = browser.workspace_request.take().expect("Selected fixture should open before materialization");
    assert_eq!(request.source_path.as_deref(), Some(selected.source_path.as_str()));
    assert!(request.preview_only && !request.resume);
    assert!(browser.specimen.is_none());
}

#[test]
fn compact_results_return_keeps_selection_and_loaded_specimen() {
    let ctx = egui::Context::default();
    let (mut browser, _, _) = fixture();
    browser.select(entry("one"));
    browser.pending_selection = false;
    browser.specimen = Some(json!({"entry":entry("one"),"digest":"digest","data":{"$value":3}}));
    let captured = browser.specimen.clone();
    click_catalog(&mut browser, &ctx, egui::vec2(680., 700.), "catalog-results-back");
    assert!(browser.results_visible);
    assert_eq!(browser.selected.as_ref().unwrap().id, "one");
    assert_eq!(browser.specimen, captured);
    assert_eq!(browser.filters, browser.displayed_filters);
}

#[test]
fn list_and_tile_results_stay_in_the_viewport_at_compact_and_wide_sizes() {
    for size in [egui::vec2(360., 700.), egui::vec2(680., 600.), egui::vec2(900., 600.), egui::vec2(1600., 1000.)] {
        for tiles in [false, true] {
            let ctx = egui::Context::default();
            let (mut browser, _, _) = fixture();
            browser.visual_results = tiles;
            for _ in 0..2 { catalog_frame(&mut browser, &ctx, size, vec![]); }
            for name in ["catalog-search", "catalog-item-one", "catalog-back"] {
                let rect: egui::Rect = ctx.data(|data| data.get_temp(egui::Id::new(("test-rect", name.to_string())))).unwrap_or_else(|| panic!("Missing {name} at {size:?}"));
                assert!(rect.min.x >= 0. && rect.min.y >= 0. && rect.max.x <= size.x + 1. && rect.max.y <= size.y + 1., "{name} escaped {size:?} with tiles={tiles}: {rect:?}");
            }
        }
    }
}

#[test]
fn unloaded_selection_shows_disabled_actions_and_resource_specific_reason() {
    let ctx = egui::Context::default();
    let (mut browser, _, _) = fixture();
    let mut selected = entry("provider");
    selected.kind = "nonvisual-dependency".into();
    browser.select(selected);
    let output = catalog_frame(&mut browser, &ctx, egui::vec2(1600., 1000.), vec![]);
    let mut text = String::new();
    for shape in output.shapes { shape_text(&shape.shape, &mut text); }
    for expected in ["Open actual preview", "Open on canvas", "no standalone visual appearance", "No editable workspace adapter"] {
        assert!(text.contains(expected), "Missing {expected}: {text}");
    }
    click_catalog(&mut browser, &ctx, egui::vec2(1600., 1000.), "catalog-preview-action");
    assert!(browser.workspace_request.is_none());
}

#[test]
fn empty_theme_modes_and_compact_token_sheets_remain_browsable() {
    let ctx = egui::Context::default();
    let mut mode = "light".into();
    let _ = ctx.run_ui(Default::default(), |ui| assert!(preview::theme_sheet(ui, &json!({"modes":{}}), &mut mode)));
    let (_, specimen, _) = editable_fixture();
    let output = ctx.run_ui(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(360., 700.))), ..Default::default() }, |ui| {
        assert!(preview::theme_sheet(ui, &specimen["presentation"], &mut mode));
    });
    let mut text = String::new();
    for shape in output.shapes { shape_text(&shape.shape, &mut text); }
    assert!(text.contains("colors.primary") && text.contains("#123456"));
}

#[test]
fn failed_initial_capture_offers_recovery_instead_of_an_endless_loading_message() {
    let ctx = egui::Context::default();
    let (mut browser, _, _) = fixture();
    browser.summary = None;
    browser.error = Some("Reader executable is unavailable".into());
    let output = catalog_frame(&mut browser, &ctx, egui::vec2(900., 700.), vec![]);
    let mut text = String::new();
    for shape in output.shapes { shape_text(&shape.shape, &mut text); }
    assert!(text.contains("Repository capture is unavailable"));
    assert!(text.contains("Retry capture") && text.contains("Connection settings"));
}

#[test]
fn registered_product_hierarchy_exposes_only_the_captured_shell_route_and_page() {
    let ctx = egui::Context::default();
    let (mut browser, _, _) = fixture();
    browser.products_open = true;
    browser.filters = Filters { collection:"sources".into(), product:"alpha".into(), ..Default::default() };
    browser.displayed_filters = browser.filters.clone();
    browser.location_filters = browser.filters.clone();
    browser.summary.as_mut().unwrap()["navigation"]["products"] = json!([{
        "id":"alpha","name":"Alpha","targets":["web"],"shells":[{
            "id":"main","name":"Main shell","routes":[{
                "id":"overview","name":"Overview route","path":"/app/overview","pages":[{
                    "id":"overview-page","name":"Overview page","sourcePath":"apps/alpha/src/overview.tsx"
                }]
            }]
        }]
    }]);
    let size = egui::vec2(1600., 1200.);
    click_catalog(&mut browser, &ctx, size, "catalog-hierarchy-alpha/shells/0");
    click_catalog(&mut browser, &ctx, size, "catalog-hierarchy-alpha/shells/0/routes/0");
    click_catalog(&mut browser, &ctx, size, "catalog-hierarchy-alpha/shells/0/routes/0/pages/0");
    let output = catalog_frame(&mut browser, &ctx, size, vec![]);
    let mut text = String::new();
    for shape in output.shapes { shape_text(&shape.shape, &mut text); }
    assert!(text.contains("Page ·"));
    assert!(text.contains("apps/alpha/src/overview.tsx"));
    assert!(browser.workspace_request.is_none(), "Hierarchy browsing must not substitute a renderer");
    browser.summary.as_mut().unwrap()["navigation"]["products"][0]["shells"] = json!([]);
    let output = catalog_frame(&mut browser, &ctx, size, vec![]);
    let mut text = String::new();
    for shape in output.shapes { shape_text(&shape.shape, &mut text); }
    assert!(text.contains("No shell hierarchy is registered"));
    assert!(!text.contains("Overview page"));
}

#[test]
fn nested_target_composition_opens_its_exact_registered_source_without_a_frame_substitute() {
    let ctx = egui::Context::default();
    let (mut browser, _, _) = fixture();
    browser.products_open = true;
    browser.filters = Filters { collection:"sources".into(), product:"alpha".into(), ..Default::default() };
    browser.displayed_filters = browser.filters.clone();
    browser.location_filters = browser.filters.clone();
    let source = "config/experience/registered-page.input.json";
    browser.summary.as_mut().unwrap()["navigation"]["products"] = json!([{
        "id":"alpha","name":"Alpha","targets":[{"id":"web","kind":"target","shells":[{
            "id":"main","name":"Main shell","routes":[{"id":"overview","path":"/overview","compositions":[{
                "id":"composition.page","kind":"composition","sourcePath":source,"sourceDigest":"d".repeat(64)
            }]}]
        }]}]
    }]);
    let size = egui::vec2(1600., 1400.);
    for id in [
        "catalog-hierarchy-alpha/targets/0",
        "catalog-hierarchy-alpha/targets/0/shells/0",
        "catalog-hierarchy-alpha/targets/0/shells/0/routes/0",
        "catalog-hierarchy-alpha/targets/0/shells/0/routes/0/compositions/0",
        "catalog-hierarchy-open-alpha/targets/0/shells/0/routes/0/compositions/0",
    ] { click_catalog(&mut browser, &ctx, size, id); }
    let request = browser.workspace_request.take().expect("Registered composition action should open its selected workspace");
    assert_eq!(request.source_path.as_deref(), Some(source));
    assert!(request.preview_only && !request.resume);
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
    browser.results_visible = false;
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
fn idle_library_uses_a_five_second_guard_and_no_timer_when_paused_disconnected_or_hidden() {
    let (mut browser, _, _) = fixture();
    let now = Instant::now();
    browser.last_refresh = now;
    browser.last_interaction = now;
    assert_eq!(browser.next_repaint_after(now), Some(Duration::from_secs(5)));
    browser.auto_refresh = false;
    assert_eq!(browser.next_repaint_after(now), None);
    browser.auto_refresh = true;
    browser.summary = None;
    assert_eq!(browser.next_repaint_after(now), None);
    browser.busy = true;
    assert_eq!(browser.next_repaint_after(now), Some(Duration::from_millis(100)));
    browser.open = false;
    assert_eq!(browser.next_repaint_after(now), None);
}

#[test]
fn pending_library_work_keeps_fast_repaints_even_with_live_updates_paused() {
    let (mut browser, _, _) = fixture();
    let now = Instant::now();
    browser.auto_refresh = false;
    browser.busy = true;
    assert_eq!(browser.next_repaint_after(now), Some(Duration::from_millis(100)));
    browser.busy = false;
    browser.filter_changed = Some(now);
    assert_eq!(browser.next_repaint_after(now), Some(Duration::from_millis(75)));
    assert_eq!(browser.next_repaint_after(now+Duration::from_millis(50)), Some(Duration::from_millis(25)));
    browser.filter_changed = None;
    browser.pending_selection = true;
    assert_eq!(browser.next_repaint_after(now), Some(Duration::from_millis(100)));
    browser.pending_selection = false;
    assert_eq!(browser.next_repaint_after(now), None);
}

#[test]
fn idle_guard_does_not_delay_sixty_second_refresh_backoff_or_interaction_deadlines() {
    let (mut browser, _, _) = fixture();
    let now = Instant::now();
    browser.last_refresh = now - Duration::from_secs(59);
    browser.last_interaction = now - Duration::from_secs(20);
    assert_eq!(browser.next_repaint_after(now), Some(Duration::from_secs(1)));
    assert_eq!(browser.next_repaint_after(now + Duration::from_secs(1)), Some(Duration::ZERO));
    browser.refresh_failures = 1;
    assert_eq!(browser.next_repaint_after(now), Some(Duration::from_secs(5)));
    browser.last_refresh = now - Duration::from_secs(119);
    assert_eq!(browser.next_repaint_after(now), Some(Duration::from_secs(1)));
    browser.refresh_failures = 0;
    browser.last_refresh = now - Duration::from_secs(60);
    browser.last_interaction = now - Duration::from_secs(9);
    assert_eq!(browser.next_repaint_after(now), Some(Duration::from_secs(1)));
    browser.last_interaction = now - Duration::from_secs(10);
    assert_eq!(browser.next_repaint_after(now), Some(Duration::ZERO));
}

#[test]
fn reader_source_events_queue_then_wake_the_desktop_without_waiting_for_idle_repaint() {
    let (mut browser, responses, requests) = fixture();
    let ctx = egui::Context::default();
    for _ in 0..4 { let _ = ctx.run_ui(Default::default(), |_| {}); }
    let wakes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = wakes.clone();
    ctx.set_request_repaint_callback(move |_| { observed.fetch_add(1, Ordering::SeqCst); });
    *browser.repaint.lock().unwrap() = Some(ctx);
    browser.mode = "local".into();
    browser.selected = Some(entry("one"));
    assert!(send_reply(&responses, &browser.repaint, Reply::SourceChanged));
    assert!(wakes.load(Ordering::SeqCst) > 0);
    browser.tick();
    assert!(matches!(requests.try_recv(), Ok(Job::Scan(_, _, mode, _)) if mode == "local"));
    assert_eq!(browser.selected.as_ref().unwrap().id, "one");
    assert!(browser.workspace_request.is_none());
    *browser.repaint.lock().unwrap() = None;
    let count = wakes.load(Ordering::SeqCst);
    assert!(send_reply(&responses, &browser.repaint, Reply::SourceChanged));
    assert_eq!(wakes.load(Ordering::SeqCst), count);
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
fn theme_variables_open_the_collection_of_the_active_page(){
    let (entry,specimen,snapshot)=editable_fixture();let document=local_library::create(&entry,&specimen,&snapshot,"light").unwrap();
    let page=document.pages[0];let expected=document.get(page).unwrap().variables.modes.keys().next().unwrap().clone();
    let mut app=crate::app::App::new(document);app.page=page;app.vars.collection=None;app.open_variables();
    assert_eq!(app.vars.collection,Some(expected));
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
    assert!(text.contains("Open actual preview"));
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
