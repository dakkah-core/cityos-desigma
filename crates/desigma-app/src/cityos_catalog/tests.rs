use super::*;
use serde_json::json;

#[test]
fn selected_workspace_requests_preserve_the_source_and_do_not_resume_unrelated_drafts() {
    let request = WorkspaceRequest::selected("config/experience/another-page.input.json", true);
    assert_eq!(request.source_path.as_deref(), Some("config/experience/another-page.input.json"));
    assert!(request.preview_only);
    assert!(!request.resume);
    assert!(request.resource_pin.is_none());
    let browse = WorkspaceRequest::browse();
    assert!(browse.source_path.is_none());
    assert!(!browse.resume);
}

#[test]
fn design_preview_requests_preserve_the_complete_captured_locator_without_draft_authority() {
    let pin = exact_specimen_profile("fixture.second", "2", "Second", "authoring-two", "web");
    let request = WorkspaceRequest::pinned_preview(pin.clone());
    assert!(request.preview_only && !request.resume);
    assert_eq!(request.source_path.as_deref(), pin["sourcePath"].as_str());
    assert_eq!(request.resource_pin, Some(pin));
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
    registration.semantic_id="fixture.context".into();
    registration.source_path="registrations.json".into(); registration.source_digest="c".repeat(64); registration.label="ContextCapsule".into();
    let mut specimen=json!({"entry":registration,"digest":registration.source_digest,"data":{"name":"ContextCapsule","publicExport":"ContextCapsule","sourcePath":"packages/ui/src/ContextCapsule.tsx"},"workspaceProfile":profile});
    assert!(support::actions(&registration,Some(&specimen),Some(&summary),"committed").actual_preview);
    specimen["data"]["publicExport"]=json!("UnrelatedExport");
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
    selected.source_digest = "b".repeat(64);
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

fn exact_specimen_profile(id: &str, version: &str, export: &str, authoring: &str, target: &str) -> Value {
    json!({"id":format!("{id}:{version}:{authoring}:{target}"),"kind":"registered-specimen",
        "resourceId":format!("{id}:{version}:{authoring}:{target}"),"sourcePath":"packages/runtime/src/shared.tsx",
        "sourcePaths":["packages/runtime/src/shared.tsx","packages/runtime/src/Receiver.tsx"],"publicExport":export,
        "componentId":id,"componentVersion":version,"profileRef":authoring,"targetProfileRef":target,
        "locator":{"componentId":id,"componentVersion":version,"profileRef":authoring,"targetProfileRef":target},
        "sourceDigest":"b".repeat(64),"implementationDigest":"b".repeat(64),
        "fixture":"registered-fixture","actualWebPreview":true,"editable":false,"ownerWrites":false,"qualification":"unqualified"})
}

#[test]
fn shared_modules_preserve_ambiguity_and_require_an_exact_export_before_opening() {
    let first = exact_specimen_profile("fixture.first", "1", "First", "authoring", "web");
    let second = exact_specimen_profile("fixture.second", "1", "Second", "authoring", "web");
    let summary = json!({"snapshot":{"mode":"committed","canonicalCommit":"a".repeat(40)},"workspaceProfiles":[first,second]});
    let mut selected = entry("shared"); selected.kind = "source-file".into(); selected.label = "First".into();
    selected.source_path = "packages/runtime/src/shared.tsx".into(); selected.source_digest = "b".repeat(64);
    let actions = support::actions(&selected, None, Some(&summary), "committed");
    assert!(!actions.actual_preview && !actions.editable && actions.source.is_none() && actions.inline_pin.is_none());
    assert!(actions.preview_reason.contains("Several registered definitions") && actions.edit_reason.contains("Select an exact"));
    assert_eq!(actions.preview_choices, vec![first, second.clone()]);
    assert_eq!(actions.capability_rows()[0].1, "Choose definition");
    assert!(support::actions(&selected, None, Some(&summary), "local").preview_choices.is_empty());
    selected.kind = "public-declaration".into(); selected.label = "Second".into();
    let exact = support::actions(&selected, None, Some(&summary), "committed");
    assert!(exact.actual_preview && exact.preview_here);
    assert!(exact.workspace_reason.as_deref().unwrap().contains("Registered defaults are temporary"));
    assert_eq!(exact.capability_rows()[1].1, "Unavailable", "Temporary defaults have no canonical authored draft binding");
    assert_eq!(exact.inline_pin.unwrap()["componentId"], "fixture.second");
    selected.label = "MissingExport".into();
    assert!(support::actions(&selected, None, Some(&summary), "committed").source.is_none());
}

#[test]
fn registered_fallback_checks_component_version_and_every_profile_target_claim() {
    let first = exact_specimen_profile("fixture.first", "1", "First", "authoring", "web");
    let summary = json!({"snapshot":{"mode":"committed","canonicalCommit":"a".repeat(40)},"workspaceProfiles":[
        exact_specimen_profile("fixture.first", "2", "First", "authoring", "web"),
        exact_specimen_profile("fixture.first", "1", "First", "other-authoring", "web"),
        exact_specimen_profile("fixture.first", "1", "First", "authoring", "native"),first]});
    let mut selected = entry("registered"); selected.kind = "registration".into(); selected.semantic_id = "fixture.first".into();
    selected.label = "First".into(); selected.source_path = "registry.json".into();
    selected.dependencies = vec!["packages/runtime/src/shared.tsx".into()];
    assert!(support::actions(&selected, None, Some(&summary), "committed").source.is_none());
    let data = json!({"id":"fixture.first","version":"1","sourcePath":"packages/runtime/src/shared.tsx",
        "locator":first["locator"]});
    let specimen = json!({"entry":selected,"digest":selected.source_digest,"data":data});
    let actions = support::actions(&selected, Some(&specimen), Some(&summary), "committed");
    assert!(actions.actual_preview); assert_eq!(actions.inline_pin.unwrap()["locator"], first["locator"]);
    for (key, value) in [("version", "9"), ("componentVersion", "9"), ("profileRef", "missing"),
        ("targetProfileRef", "missing"), ("componentId", "fixture.wrong"), ("id", "fixture.wrong"), ("publicExport", "WrongExport")] {
        let mut wrong = specimen.clone(); wrong["data"][key] = json!(value);
        let actions = support::actions(&selected, Some(&wrong), Some(&summary), "committed");
        assert!(!actions.actual_preview && actions.source.is_none(), "{key}");
    }
    let mut wrong = specimen.clone(); wrong["data"]["canvasLocator"] = first["locator"].clone();
    wrong["data"]["canvasLocator"]["componentVersion"] = json!("9");
    assert!(support::actions(&selected, Some(&wrong), Some(&summary), "committed").source.is_none());
    let versions = json!({"workspaceProfiles":[summary["workspaceProfiles"][0], first]});
    let without_version = json!({"entry":selected,"digest":selected.source_digest,
        "data":{"id":"fixture.first","sourcePath":"packages/runtime/src/shared.tsx","profileRef":"authoring","targetProfileRef":"web"}});
    let unresolved = support::actions(&selected, Some(&without_version), Some(&versions), "committed");
    assert!(unresolved.source.is_none() && unresolved.preview_reason.contains("Several registered definitions"));
    selected.semantic_id = "fixture.wrong".into();
    assert!(support::actions(&selected, None, Some(&summary), "committed").source.is_none());
}

#[test]
fn materialized_ambiguity_or_identity_failure_cannot_fall_back_to_a_summary_first_match() {
    let profile = exact_specimen_profile("fixture.first", "1", "First", "authoring", "web");
    let summary = json!({"workspaceProfiles":[profile]});
    let mut selected = entry("shared"); selected.kind = "source-file".into();
    selected.source_path = "packages/runtime/src/shared.tsx".into(); selected.source_digest = "b".repeat(64);
    for status in ["ambiguous", "unavailable"] {
        let specimen = json!({"entry":selected,"digest":selected.source_digest,"data":"captured bytes",
            "workspaceSelection":{"status":status,"reason":"Select the exact locator."}});
        let actions = support::actions(&selected, Some(&specimen), Some(&summary), "committed");
        assert!(actions.source.is_none() && !actions.actual_preview && actions.inline_pin.is_none());
        assert!(actions.preview_reason.contains("Select"));
    }
    let mut specimen = json!({"entry":selected,"digest":selected.source_digest,"data":{},"workspaceProfile":profile});
    specimen["workspaceProfile"]["sourceDigest"] = json!("c".repeat(64));
    assert!(support::actions(&selected, Some(&specimen), Some(&summary), "committed").source.is_none());
}

#[test]
fn missing_locator_identity_and_shared_helper_files_do_not_select_a_registered_leaf() {
    let profile = exact_specimen_profile("fixture.first", "1", "First", "authoring", "web");
    let mut selected = entry("helper"); selected.kind = "source-file".into(); selected.label = "First".into();
    selected.source_path = "packages/runtime/src/Receiver.tsx".into(); selected.source_digest = "b".repeat(64);
    let summary = json!({"workspaceProfiles":[profile]});
    assert!(support::actions(&selected, None, Some(&summary), "committed").source.is_none());
    selected.kind = "registration".into(); selected.source_path = "registry.json".into();
    selected.dependencies = vec!["packages/runtime/src/shared.tsx".into()];
    let specimen = json!({"entry":selected,"digest":selected.source_digest,"data":{"name":"First","sourcePath":"packages/runtime/src/shared.tsx"}});
    assert!(support::actions(&selected, Some(&specimen), Some(&summary), "committed").source.is_none());
    selected.kind = "source-file".into(); selected.source_path = "packages/runtime/src/shared.tsx".into();
    for key in ["componentId", "componentVersion", "profileRef", "targetProfileRef"] {
        let mut missing = summary.clone(); missing["workspaceProfiles"][0].as_object_mut().unwrap().remove(key);
        assert!(support::actions(&selected, None, Some(&missing), "committed").source.is_none(), "{key}");
    }
}

#[test]
fn unavailable_actual_adapter_keeps_the_selected_compositions_structure_canvas() {
    let mut selected = entry("structure"); selected.kind = "authored-composition".into();
    selected.source_path = "config/experience/another-page.input.json".into();
    let specimen = json!({"entry":selected,"digest":selected.source_digest,"data":{},
        "workspaceSelection":{"status":"unavailable","reason":"No renderer binding."}});
    let actions = support::actions(&selected, Some(&specimen), None, "committed");
    assert_eq!(actions.source.as_deref(), Some(selected.source_path.as_str()));
    assert!(!actions.actual_preview && !actions.editable);
    assert!(actions.preview_reason.contains("Authored structure"));
}

#[test]
fn selected_capability_labels_separate_preview_canvas_local_drafts_and_qualification_from_accounting() {
    assert_eq!(support::LEVEL_OVERVIEW_TITLE, "Source accounting by level");
    assert!(support::LEVEL_EDITING_TOTALS.contains("Not aggregated"));
    let mut selected = entry("accordion"); selected.kind = "public-declaration".into(); selected.label = "Accordion".into();
    selected.source_path = "packages/design-system/src/components/ui/accordion.tsx".into();
    let actions = support::actions(&selected, None, Some(&profile_summary()), "committed");
    let rows = actions.capability_rows();
    assert_eq!(rows.iter().map(|r| (r.0, r.1)).collect::<Vec<_>>(), vec![
        ("Actual preview", "Available"), ("Structure canvas", "Available"), ("Local copy", "Unavailable"),
        ("Local draft", "Available"), ("Renderer qualification", "Not established here"), ("Owner writes", "Separate workflow")]);
    assert!(!selected.editable && !selected.renderer_qualified && !selected.persistent_draft);
    let local = support::actions(&selected, None, Some(&profile_summary()), "local");
    assert!(local.inline_pin.is_none() && local.edit_reason.contains("Local-preview bytes are not adopted"));
    let (theme, specimen, snapshot) = editable_fixture();
    let summary = json!({"snapshot":snapshot});
    let theme_actions = support::actions(&theme, Some(&specimen), Some(&summary), "committed");
    let rows = theme_actions.capability_rows();
    assert_eq!(rows[2].1, "Available"); assert_eq!(rows[3].1, "Unavailable");
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
fn theme_detail_prioritizes_its_workflows_instead_of_unrelated_component_binding_errors() {
    let (selected, specimen, snapshot) = editable_fixture();
    let (mut browser, _, _) = fixture();
    browser.selected = Some(selected.clone());
    browser.specimen = Some(specimen.clone());
    browser.summary = Some(json!({"snapshot":snapshot}));
    let ctx = egui::Context::default();
    let mut output = None;
    for _ in 0..2 {
        output = Some(ctx.run_ui(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.,1000.))), ..Default::default() }, |ui| browser.show(ui, &ctx)));
    }
    let mut rendered = String::new();
    for shape in output.unwrap().shapes { shape_text(&shape.shape, &mut rendered); }
    for label in ["Token sheet", "Inspection canvas", "Local library", "Owner-bound draft", "Web-frame theme preview", "Create local theme library", "Preview token sheet"] {
        assert!(rendered.contains(label), "Missing theme workflow {label}: {rendered}");
    }
    for unrelated in ["Open actual preview", "Open on canvas", "No exact captured workspace binding", "No editable workspace adapter"] {
        assert!(!rendered.contains(unrelated), "Theme must not display an unrelated component action/error: {unrelated}");
    }
    let actions = support::actions(&selected, Some(&specimen), browser.summary.as_ref(), "committed");
    let rows = actions.capability_rows();
    assert_eq!(rows[0].1, "Available");
    assert_eq!(rows[1].1, "Available");
    assert_eq!(rows[2].1, "Available");
    assert_eq!(rows[3].1, "Unavailable", "A local theme library grants no owner binding");
    assert_eq!(rows[4].1, "Available");
    let stale = support::actions(&selected, None, browser.summary.as_ref(), "committed");
    assert_eq!(stale.capability_rows()[0].1, "Unavailable");
    assert_eq!(stale.capability_rows()[4].1, "Unavailable");
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
    browser.auto_refresh=false;
    browser.filters.search="context".into(); browser.filters_changed();
    browser.filters.search="context capsule".into(); browser.filters_changed();
    browser.filter_changed=Some(Instant::now()-Duration::from_millis(125));
    browser.tick();
    match requests.try_recv().expect("Search should dispatch within the 150 ms interaction budget") {
        Job::Query(_,filters,offset,binding)=>{assert_eq!(filters.search,"context capsule");assert_eq!(offset,0);assert_eq!(binding.generation,"first");},
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
    browser.catalog_binding = Some(CatalogBinding { config: browser.reader_config(), mode: browser.mode.clone(), generation: "first".into() });
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
fn catalog_refresh_filters_wait_for_cancellation_then_query_the_retained_generation() {
    let (mut browser, tx, requests) = fixture();
    // Local notifications can keep arriving while a debounced search is pending.
    browser.mode = "local".into();
    browser.summary.as_mut().unwrap()["snapshot"]["mode"] = json!("local");
    browser.catalog_binding.as_mut().unwrap().mode = "local".into();
    browser.refresh();
    let scan_epoch = browser.epoch;
    assert!(matches!(requests.try_recv(), Ok(Job::Scan(_, _, _, false))));
    browser.filters.search = "latest search".into();
    browser.filters_changed();
    // Keep the debounce pending deterministically, even on a slow test host.
    browser.filter_changed = Some(Instant::now() + Duration::from_secs(60));
    assert!(browser.cancelled.load(Ordering::Acquire));
    tx.send(Reply::SourceChanged).unwrap();
    tx.send(Reply::Scan(scan_epoch.wrapping_sub(1), Ok(json!({"generationId":"stale","snapshot":{"mode":"local"}})))).unwrap();
    browser.tick();
    assert!(browser.busy && browser.cancelled.load(Ordering::Acquire));
    assert_eq!(browser.epoch, scan_epoch);
    assert!(requests.try_recv().is_err());
    tx.send(Reply::Scan(scan_epoch, Err("Catalog request cancelled; previous generation retained".into()))).unwrap();
    browser.tick();
    assert!(!browser.busy && !browser.cancelled.load(Ordering::Acquire));
    assert!(requests.try_recv().is_err(), "Debounce must finish before another refresh or read");
    browser.filter_changed = Some(Instant::now() - FILTER_DEBOUNCE);
    browser.tick();
    match requests.try_recv().unwrap() {
        Job::Query(epoch, filters, 0, binding) => {
            assert_eq!(binding.generation, "first");
            assert_eq!(binding.mode, "local");
            assert_eq!(binding.config, browser.reader_config());
            assert_eq!(filters.search, "latest search");
            tx.send(Reply::Query(epoch, filters, 0, Ok(Page { entries: vec![entry("matching")], total: 1, next_offset: None }))).unwrap();
        }
        _ => panic!("Changed filters must precede another background scan"),
    }
    browser.poll();
    assert_eq!(browser.entries[0].id, "matching");
    assert_eq!(browser.displayed_filters, browser.filters);
    assert_eq!(browser.summary.as_ref().unwrap()["generationId"], "first");
    assert_eq!(browser.refresh_failures, 0);
    assert!(browser.error.is_none());
}

#[test]
fn catalog_refresh_latest_selection_materializes_only_after_scan_settlement() {
    let (mut browser, tx, requests) = fixture();
    browser.refresh();
    let scan_epoch = browser.epoch;
    assert!(matches!(requests.try_recv(), Ok(Job::Scan(..))));
    browser.select(entry("two"));
    browser.select(entry("three"));
    browser.tick();
    assert!(browser.busy && browser.pending_selection);
    assert!(browser.cancelled.load(Ordering::Acquire));
    assert!(requests.try_recv().is_err());
    tx.send(Reply::Scan(scan_epoch, Err("CityOS background work cancelled; previous generation retained".into()))).unwrap();
    browser.tick();
    match requests.try_recv().unwrap() {
        Job::Materialize(epoch, id, binding) => {
            assert_eq!(id, "three");
            assert_eq!(binding.generation, "first");
            assert!(!browser.cancelled.load(Ordering::Acquire));
            tx.send(Reply::Scan(scan_epoch, Ok(json!({"generationId":"late","snapshot":{"mode":"committed"}})))).unwrap();
            tx.send(Reply::Materialize(epoch, id, Ok(json!({"entry":entry("three"),"digest":"digest","data":{"$value":3}})))).unwrap();
        }
        _ => panic!("The latest selection must read the retained session"),
    }
    browser.poll();
    assert_eq!(browser.selected.as_ref().unwrap().id, "three");
    assert!(browser.specimen.is_some());
    assert_eq!(browser.summary.as_ref().unwrap()["generationId"], "first");
    assert!(!browser.busy && browser.error.is_none());
    assert!(requests.try_recv().is_err());
}

#[test]
fn catalog_refresh_initial_read_without_a_generation_is_not_cancelled_by_interaction() {
    for selection in [false, true] {
        let (mut browser, tx, requests) = fixture();
        let first = browser.summary.take().unwrap();
        browser.catalog_binding = None;
        browser.entries.clear();
        browser.refresh();
        assert_eq!(browser.activity, "Reading repository");
        assert!(matches!(requests.try_recv(), Ok(Job::Scan(_, _, _, true))));
        if selection { browser.select(entry("one")); }
        else { browser.filters.search = "initial search".into(); browser.filters_changed(); }
        browser.tick();
        assert!(browser.busy);
        assert!(!browser.cancelled.load(Ordering::Acquire));
        assert!(requests.try_recv().is_err());
        tx.send(Reply::Scan(browser.epoch, Ok(first))).unwrap();
        browser.tick();
        assert!(matches!(requests.try_recv(), Ok(Job::Query(_, _, 0, binding)) if binding.generation == "first"));
        assert!(browser.usable_generation().is_some());
    }
}

#[test]
fn catalog_refresh_retention_requires_cancellation_and_a_matching_last_good_session() {
    for message in ["Catalog request cancelled; previous generation retained", "CityOS background work cancelled; previous generation retained"] {
        let result = Err(message.to_string());
        assert!(retain_cancelled_refresh(&result, true, true, true));
        assert!(!retain_cancelled_refresh(&result, false, true, true));
        assert!(!retain_cancelled_refresh(&result, true, false, true));
        assert!(!retain_cancelled_refresh(&result, true, true, false));
    }
    for message in ["Reader timed out", "Catalog request cancelled but protocol failed", "GENERATION_OBSOLETE"] {
        assert!(!retain_cancelled_refresh(&Err(message.into()), true, true, true));
    }
    assert!(!retain_cancelled_refresh(&Ok(json!({})), true, true, true));
}

#[test]
fn catalog_refresh_changed_configuration_or_mode_cannot_reuse_a_displayed_generation() {
    for change in 0..7 {
        let (mut browser, _, requests) = fixture();
        match change {
            0 => browser.repository = "different-repository".into(),
            1 => browser.reader = "different-reader".into(),
            2 => browser.node = "different-node".into(),
            3 => browser.cache = Some(PathBuf::from("different-cache")),
            4 => browser.canonical_ref = Some("different-commit".into()),
            5 => browser.mode = "local".into(),
            _ => browser.summary.as_mut().unwrap()["generationId"] = json!("different-generation"),
        }
        assert!(browser.usable_generation().is_none());
        browser.query(0);
        browser.select(entry("one"));
        browser.tick();
        assert!(requests.try_recv().is_err());
        browser.refresh();
        if change==3 {
            assert!(matches!(requests.try_recv(), Ok(Job::Restore(..))));
            assert_eq!(browser.activity,"Restoring saved library");
        } else {
            assert!(matches!(requests.try_recv(), Ok(Job::Scan(_, _, _, true))));
            assert_eq!(browser.activity, "Reading repository");
        }
        browser.filters.search = "new".into();
        browser.filters_changed();
        assert!(!browser.cancelled.load(Ordering::Acquire));
    }
}

#[test]
fn initial_cache_recovery_is_separate_from_background_scan_and_queries_only_the_valid_generation() {
    let (mut browser,tx,requests)=fixture();
    let summary=browser.summary.take().unwrap();browser.catalog_binding=None;browser.entries.clear();browser.cache=Some("cache-fixture".into());
    browser.refresh();let epoch=browser.epoch;
    assert_eq!(browser.activity,"Restoring saved library");
    assert!(matches!(requests.try_recv(),Ok(Job::Restore(..))));
    browser.filters.search="requested during recovery".into();browser.filters_changed();browser.tick();
    assert!(!browser.cancelled.load(Ordering::Acquire));assert!(requests.try_recv().is_err());
    tx.send(Reply::Scan(epoch,Ok(summary))).unwrap();browser.poll();
    assert!(matches!(requests.try_recv(),Ok(Job::Query(_,filters,0,binding)) if filters.search=="requested during recovery" && binding.generation=="first"));
    assert!(browser.usable_generation().is_some());
}

#[test]
fn cache_only_misses_fall_back_once_but_protocol_failures_or_obsolete_contexts_cannot_start_a_scan() {
    for error in ["CACHE_RESTORE_UNAVAILABLE: missing cache","CACHE_RESTORE_UNAVAILABLE: reader capability unavailable","Malformed reader reply"] {
        let (mut browser,tx,requests)=fixture();browser.summary=None;browser.catalog_binding=None;browser.cache=Some("cache-fixture".into());
        browser.refresh();assert!(matches!(requests.try_recv(),Ok(Job::Restore(..))));
        tx.send(Reply::Scan(browser.epoch,Err(error.into()))).unwrap();browser.poll();
        if error.starts_with("CACHE_RESTORE_UNAVAILABLE") {
            assert_eq!(browser.activity,"Reading repository");assert!(matches!(requests.try_recv(),Ok(Job::Scan(_,_,_,true))));
        } else {assert!(requests.try_recv().is_err());assert!(browser.error.is_some());}
    }
    for obsolete in [true,false] {
        let (mut browser,tx,requests)=fixture();browser.summary=None;browser.catalog_binding=None;browser.cache=Some("cache-fixture".into());
        browser.refresh();let epoch=browser.epoch;assert!(matches!(requests.try_recv(),Ok(Job::Restore(..))));
        if obsolete {browser.mode="local".into();} else {browser.cancelled.store(true,Ordering::Release);}
        tx.send(Reply::Scan(epoch,Err("CACHE_RESTORE_UNAVAILABLE: incompatible cache".into()))).unwrap();browser.poll();
        assert!(requests.try_recv().is_err(),"A retired recovery cannot enqueue new source work");
    }
}

#[test]
fn catalog_refresh_config_change_during_scan_rejects_its_result_and_old_reads() {
    let (mut browser, tx, requests) = fixture();
    browser.refresh();
    assert!(matches!(requests.try_recv(), Ok(Job::Scan(..))));
    browser.reader = "new-reader".into();
    tx.send(Reply::Scan(browser.epoch, Ok(json!({"generationId":"wrong-config","snapshot":{"mode":"committed"}})))).unwrap();
    browser.poll();
    assert_eq!(browser.summary.as_ref().unwrap()["generationId"], "first");
    assert!(browser.usable_generation().is_none() && browser.error.is_some());
    browser.query(0);
    assert!(requests.try_recv().is_err());
}

#[test]
fn catalog_refresh_failure_preserves_display_and_explicit_refresh_reconnects_with_cache() {
    let (mut browser, tx, requests) = fixture();
    let previous = browser.summary.clone();
    browser.refresh();
    assert!(matches!(requests.try_recv(), Ok(Job::Scan(..))));
    browser.filters.search = "changed".into();
    browser.filters_changed();
    tx.send(Reply::Scan(browser.epoch, Err("Reader timed out".into()))).unwrap();
    browser.poll();
    assert_eq!(browser.summary, previous);
    assert_eq!(browser.entries[0].id, "one");
    assert!(browser.catalog_binding.is_none());
    assert_eq!(browser.refresh_failures, 1);
    browser.query(0);
    assert!(requests.try_recv().is_err());
    browser.refresh();
    assert!(matches!(requests.try_recv(), Ok(Job::Scan(_, _, _, true))));
    tx.send(Reply::Scan(browser.epoch, Ok(previous.unwrap()))).unwrap();
    browser.poll();
    assert!(matches!(requests.try_recv(), Ok(Job::Query(_, filters, 0, binding)) if filters.search == "changed" && binding.generation == "first"));
}

#[test]
fn failed_remote_refresh_recovered_from_cache_keeps_browsing_and_failure_backoff() {
    let (mut browser,tx,requests)=fixture();browser.auto_refresh=false;
    let mut recovered=browser.summary.clone().unwrap();
    recovered["restoredCache"]=json!(true);recovered["stale"]=json!(true);
    recovered["refreshError"]=json!("Remote unavailable");recovered["restoredAfterRefreshFailure"]=json!(true);
    browser.refresh();let epoch=browser.epoch;assert!(matches!(requests.try_recv(),Ok(Job::Scan(..))));
    browser.filters.level="L2".into();browser.filters_changed();
    tx.send(Reply::Scan(epoch,Ok(recovered))).unwrap();browser.poll();
    assert_eq!(browser.refresh_failures,1);assert_eq!(browser.summary.as_ref().unwrap()["generationId"],"first");
    assert!(matches!(requests.try_recv(),Ok(Job::Query(_,filters,0,binding)) if filters.level=="L2"&&binding.generation=="first"));
    tx.send(Reply::Query(browser.epoch,browser.filters.clone(),0,Ok(Page{entries:vec![entry("L2-match")],total:1,next_offset:None}))).unwrap();
    browser.poll();assert_eq!(browser.entries[0].id,"L2-match");assert_eq!(browser.displayed_filters,browser.filters);
    assert_eq!(browser.next_repaint_after(Instant::now()),None);
}

#[test]
fn an_unrecoverable_reader_shows_reconnect_instead_of_permanent_loading_or_mismatched_rows() {
    let (mut browser,_,_)=fixture();browser.auto_refresh=false;browser.catalog_binding=None;
    browser.filters.level="L2".into();browser.error=Some("Remote unavailable; saved cache is missing".into());
    let ctx=egui::Context::default();let size=egui::vec2(1440.,900.);
    catalog_frame(&mut browser,&ctx,size,vec![]);let output=catalog_frame(&mut browser,&ctx,size,vec![]);
    let mut text=String::new();for shape in &output.shapes {shape_text(&shape.shape,&mut text);}
    assert!(text.contains("Results unavailable")&&text.contains("Reconnect saved library"));
    assert!(!text.contains("Updating results")&&!text.contains("token:/colors/surface/page"));
    assert_eq!(browser.next_repaint_after(Instant::now()),None);
}

#[test]
fn remote_failure_recovery_only_restores_the_exact_generation_without_capturing_sources() {
    for outcome in ["healthy","generation-changed","mode-changed","old-reader"] {
        let root=std::env::temp_dir().join(format!("desigma-cache-reconnect-{}",uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();let fixture=CatalogReaderFixture(root.clone());
        let reader=root.join("reader.mjs");
        let script=r#"import readline from 'node:readline';const outcome=__OUTCOME__;let generation=null;
          const send=(r,body)=>process.stdout.write(JSON.stringify({protocolVersion:r.protocolVersion,requestId:r.requestId,sessionId:r.sessionId,generationId:generation,kind:'result',result:body})+'\n');
          readline.createInterface({input:process.stdin}).on('line',line=>{const r=JSON.parse(line);
            if(r.op==='Hello')return send(r,{capabilities:{cacheOnlyRestore:outcome!=='old-reader'}});
            if(r.op==='Scan'){if(r.cacheOnly!==true)process.exit(51);generation=outcome==='generation-changed'?'different':'first';return send(r,{generationId:generation,restoredCache:true,snapshot:{mode:outcome==='mode-changed'?'local':'committed'}});}
            if(r.op==='Query')return send(r,{entries:[],total:0,nextOffset:null});
            process.exit(52);});"#.replace("__OUTCOME__",&serde_json::to_string(outcome).unwrap());
        std::fs::write(&reader,script).unwrap();let marker=root.join("active.json");std::fs::write(&marker,b"immutable fixture pointer").unwrap();
        let binding=CatalogBinding {config:ReaderConfig{node:"node".into(),reader,repository:root.clone(),cache:Some(root.clone()),canonical_ref:None},
            mode:"committed".into(),generation:"first".into()};
        let result=recover_previous_catalog(&binding,Arc::new(AtomicBool::new(false)),"Remote unavailable");
        if outcome=="healthy" {
            let (mut client,summary)=result.unwrap();assert_eq!(summary["stale"],true);assert_eq!(summary["refreshError"],"Remote unavailable");
            assert_eq!(summary["restoredAfterRefreshFailure"],true);assert_eq!(client.generation.as_deref(),Some("first"));
            assert_eq!(client.query(&Filters::default(),0).unwrap().total,0);drop(client);
        } else {assert!(result.is_err(),"{outcome}");}
        assert_eq!(std::fs::read(&marker).unwrap(),b"immutable fixture pointer");drop(fixture);
    }
}

struct CatalogReaderFixture(PathBuf);
impl Drop for CatalogReaderFixture {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}

// A real Client against a small IPC peer exercises request/session/generation
// validation without a repository scan, cache publication, or the heavy-job gate.
fn cancelled_catalog_client(fence_mode: &str) -> (CatalogReaderFixture, Client, CatalogBinding, Arc<AtomicBool>, Value) {
    let directory = std::env::temp_dir().join(format!("desigma-catalog-refresh-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    let fixture = CatalogReaderFixture(directory.clone());
    let script = r#"
import readline from 'node:readline';
const fenceMode = __FENCE_MODE__;
const entry = __ENTRY__;
let generation = 'first', scans = 0, pending = null;
const send = (r, kind, body) => process.stdout.write(JSON.stringify({
    protocolVersion:r.protocolVersion, requestId:r.requestId, sessionId:r.sessionId,
    generationId:generation, kind, ...body
})+'\n');
readline.createInterface({input:process.stdin}).on('line', line => {
    const r = JSON.parse(line);
    if (r.op === 'Hello') { send(r, 'result', {result:{}}); return; }
    if (r.op === 'Scan') {
        if (++scans === 1) send(r, 'result', {result:{generationId:generation, snapshot:{mode:r.mode}, readerProcess:process.pid}});
        else pending = r;
        return;
    }
    if (r.op === 'Cancel') {
        if (r.targetRequestId === 'settled-refresh') {
            if (fenceMode === 'error') { send(r, 'error', {error:{message:'Cancellation fence failed'}}); return; }
            if (fenceMode === 'dead') { process.exit(0); return; }
            if (fenceMode === 'advanced') generation = 'unexpected-replacement';
        }
        const retired = pending; pending = null;
        send(r, 'result', {result:{cancelled:true}});
        // An obsolete scan frame must not update Client.generation or satisfy
        // the subsequent fence/read, even if it arrives after the Cancel ack.
        if (retired) setImmediate(() => send(retired, 'result', {result:{generationId:'obsolete', snapshot:{mode:'committed'}}}));
        return;
    }
    if (r.op === 'Query') {
        if (r.search !== 'new search') { send(r, 'error', {error:{message:'Wrong filters'}}); return; }
        send(r, 'result', {result:{entries:[entry], total:1, nextOffset:null}}); return;
    }
    if (r.op === 'Materialize') send(r, 'result', {result:{entry, digest:'digest', data:{selected:r.entryId}, readerProcess:process.pid, scans}});
});
"#;
    let script = script.replace("__FENCE_MODE__", &serde_json::to_string(fence_mode).unwrap())
        .replace("__ENTRY__", &serde_json::to_string(&entry("one")).unwrap());
    let reader = directory.join("reader.mjs");
    std::fs::write(&reader, script).unwrap();
    let config = ReaderConfig { node: "node".into(), reader, repository: directory, cache: None, canonical_ref: None };
    let cancelled = Arc::new(AtomicBool::new(false));
    let mut client = Client::connect_with_cancel(&config, cancelled.clone()).expect("IPC regression fixture requires Node on PATH");
    let summary = client.scan(&config, "committed").unwrap();
    cancelled.store(true, Ordering::Release);
    let result = client.scan(&config, "committed");
    assert!(retain_cancelled_refresh(&result, true, true, client.generation.is_some()));
    let binding = CatalogBinding { config, mode: "committed".into(), generation: "first".into() };
    (fixture, client, binding, cancelled, summary)
}

#[test]
fn catalog_refresh_retained_client_serves_query_and_materialize_from_the_previous_generation() {
    for materialize_first in [false, true] {
        let (_fixture, client, binding, cancelled, summary) = cancelled_catalog_client("healthy");
        let mut client = Some(client);
        let mut session = Some(binding.clone());
        let mut fence = true;
        // The cancellation request has returned; this is the UI's settle boundary.
        cancelled.store(false, Ordering::Release);
        let first_specimen = materialize_first.then(||
            read_catalog(&mut client, &mut session, &mut fence, &binding, |c| c.materialize("one")).unwrap());
        let filters = Filters { search: "new search".into(), ..Default::default() };
        let page = read_catalog(&mut client, &mut session, &mut fence, &binding, |c| c.query(&filters, 0)).unwrap();
        assert_eq!(page.entries[0].id, "one");
        assert!(!fence);
        let specimen = first_specimen.unwrap_or_else(||
            read_catalog(&mut client, &mut session, &mut fence, &binding, |c| c.materialize("one")).unwrap());
        assert_eq!(specimen["data"]["selected"], "one");
        assert_eq!(specimen["readerProcess"], summary["readerProcess"], "A healthy cancellation must keep the same Client process");
        assert_eq!(specimen["scans"], 2, "Reads must not launch a replacement scan");
        assert_eq!(client.as_ref().unwrap().generation.as_deref(), Some("first"));
        assert_eq!(session.as_ref(), Some(&binding));
        drop(client);
    }
}

#[test]
fn catalog_refresh_worker_read_rejects_mismatched_config_mode_and_generation() {
    for change in 0..4 {
        let (_fixture, client, binding, cancelled, _) = cancelled_catalog_client("healthy");
        let mut client = Some(client);
        let mut session = Some(binding.clone());
        let mut fence = true;
        let mut requested = binding;
        match change {
            0 => requested.config.repository = PathBuf::from("other-repository"),
            1 => requested.mode = "local".into(),
            2 => requested.generation = "other-generation".into(),
            _ => client.as_mut().unwrap().generation = Some("other-generation".into()),
        }
        cancelled.store(false, Ordering::Release);
        let mut read_called = false;
        assert!(read_catalog(&mut client, &mut session, &mut fence, &requested, |_| {
            read_called = true;
            Ok(())
        }).is_err());
        assert!(!read_called && client.is_none() && session.is_none() && !fence);
    }
}

#[test]
fn catalog_refresh_failed_cancel_fence_discards_session_and_preserves_display_for_explicit_recovery() {
    for mode in ["error", "dead", "advanced"] {
        let (_fixture, client, binding, cancelled, _) = cancelled_catalog_client(mode);
        let mut client = Some(client);
        let mut session = Some(binding.clone());
        let mut fence = true;
        cancelled.store(false, Ordering::Release);
        let mut read_called = false;
        let error = read_catalog(&mut client, &mut session, &mut fence, &binding, |_| {
            read_called = true;
            Ok(())
        }).unwrap_err();
        assert!(!read_called && client.is_none() && session.is_none() && !fence);
        assert!(error.contains("Use Refresh to reconnect from cache"));

        let (mut browser, tx, requests) = fixture();
        let previous = browser.summary.clone();
        browser.filters.search = "new search".into();
        browser.query(0);
        assert!(matches!(requests.try_recv(), Ok(Job::Query(..))));
        tx.send(Reply::Query(browser.epoch, browser.filters.clone(), 0, Err(error))).unwrap();
        browser.poll();
        assert_eq!(browser.summary, previous);
        assert_eq!(browser.entries[0].id, "one");
        assert!(browser.catalog_binding.is_none());
        assert!(browser.error.as_ref().unwrap().contains("Use Refresh to reconnect from cache"));
        browser.query(0);
        assert!(requests.try_recv().is_err(), "A failed fence must not be bypassed by another read");
        browser.refresh();
        assert!(matches!(requests.try_recv(), Ok(Job::Scan(_, _, _, true))));
        assert_eq!(browser.activity, "Reading repository");
    }
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
