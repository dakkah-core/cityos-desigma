//! Tests of actual editor variables, installed fonts and native file IO.
use super::Browser;
use desigma_cityos::catalog::local_library;
use desigma_cityos_catalog::Entry;
use serde_json::{json, Value};

pub(crate) fn idle_browser() -> Browser {
    Browser(desigma_cityos::catalog::test_support::idle_browser(super::style::services()))
}
pub(crate) fn populated_browser() -> Browser {
    Browser(desigma_cityos::catalog::test_support::populated_browser(super::style::services()))
}

fn entry(id: &str) -> Entry {
    serde_json::from_value(json!({"id":id,"semanticId":"token:/colors/surface/page","label":"token:/colors/surface/page","level":"L0","family":"foundations","kind":"token","owner":"design-system","package":"@cityos/design-tokens","product":null,"targets":["web"],"locales":["ar","en"],"sourcePath":"tokens.json","sourcePointer":"/colors/page","sourceDigest":"digest","observationId":"git:fixture","dependencies":[],"generationStatus":"not-applicable","classificationStatus":"classified","temporaryView":"token-sheet","persistentDraft":false,"editable":false,"rendererQualified":false,"diagnostics":[]})).unwrap()
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
#[cfg(windows)]
fn windows_catalog_font_fallback_contains_arabic_glyphs() {
    let ctx = egui::Context::default();
    crate::theme::install_fonts(&ctx);
    let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
        assert!(ui.fonts_mut(|fonts| fonts.has_glyphs(&egui::FontId::proportional(16.), "سيتي أو إس")));
    });
}

#[test]
fn theme_variables_open_the_collection_of_the_active_page(){
    let (entry,specimen,snapshot)=editable_fixture();let document=local_library::create(&entry,&specimen,&snapshot,"light").unwrap();
    let page=document.pages[0];let expected=document.get(page).unwrap().variables.modes.keys().next().unwrap().clone();
    let mut app=crate::app::App::new(document);app.page=page;app.vars.collection=None;app.open_variables();
    assert_eq!(app.vars.collection,Some(expected));
}

struct LocalThemeMode {
    page: desigma_doc::NodeId,
    collection: String,
    mode: String,
    primary: String,
    sample: desigma_doc::NodeId,
}

fn local_theme_mode(document: &desigma_doc::Document, name: &str) -> LocalThemeMode {
    let page = document.pages.iter().copied().find(|page| document.get(*page).unwrap().name == name).unwrap();
    let collection = document.get(page).unwrap().variables.modes.keys().next().unwrap().clone();
    let mode = document.variable_collections[&collection].default_mode.clone();
    let primary = document.variables.values().find(|variable| variable.collection == collection && variable.name == "colors/primary").unwrap().id.clone();
    let sample = document.nodes.values().find(|node| document.page_of(node.id) == Some(page) && node.variables.bindings.get("fills/0") == Some(&primary)).unwrap().id;
    LocalThemeMode { page, collection, mode, primary, sample }
}

// Use the actual sidebar interaction so a future distinction between automatic
// and deliberate selection is exercised without assigning VarsState directly.
fn select_local_theme_collection(app: &mut crate::app::App, collection: &str) {
    let ctx = egui::Context::default();
    crate::theme::install_fonts(&ctx);
    let size = egui::vec2(1200., 900.);
    let name = format!("vars-coll:{}", app.doc.variable_collections[collection].name);
    let mut frame = |events| {
        let _ = ctx.run_ui(egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)), events, ..Default::default()
        }, |ui| {
            let content = ui.max_rect();
            crate::variables::view::show(app, ui, content);
        });
    };
    frame(vec![]);
    frame(vec![]);
    let rect: egui::Rect = ctx.data(|data| data.get_temp(egui::Id::new(("test-rect", name)))).expect("Theme collection sidebar row");
    let pos = rect.center();
    for pressed in [true, false] {
        frame(vec![egui::Event::PointerMoved(pos), egui::Event::PointerButton {
            pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE,
        }]);
    }
    assert_eq!(app.vars.collection.as_deref(), Some(collection));
}

#[test]
fn theme_variables_follow_automatic_collection_light_dark_light_and_edit_only_that_mode() {
    use desigma_doc::{Color, VariableValue};
    let (entry, specimen, snapshot) = editable_fixture();
    let document = local_library::create(&entry, &specimen, &snapshot, "light").unwrap();
    let light = local_theme_mode(&document, "light");
    let dark = local_theme_mode(&document, "dark");
    let mut app = crate::app::App::new(document);

    for (active, other, hex) in [(&light, &dark, "#334455"), (&dark, &light, "#556677"), (&light, &dark, "#778899")] {
        app.close_variables();
        app.switch_page(active.page);
        app.open_variables();
        assert!(app.vars.open);
        assert_eq!(app.vars.collection.as_deref(), Some(active.collection.as_str()), "Automatic collection must follow the active theme page after reopening");
        let other_values = app.doc.variables[&other.primary].values.clone();
        let other_sample = app.doc.bound_value(other.sample, "fills/0");

        // Resolve the edit through the selected collection, as the panel does.
        let selected = &app.doc.variable_collections[app.vars.collection.as_ref().unwrap()];
        let mode = selected.default_mode.clone();
        let primary = selected.variables.iter().find(|id| app.doc.variables[*id].name == "colors/primary").unwrap().clone();
        let expected = VariableValue::Color(Color::from_hex(hex).unwrap());
        assert!(app.edit_variables(|store| {
            assert!(store.set_value(&primary, &mode, expected.clone()));
            (vec![], vec![])
        }));
        assert_eq!(app.doc.variables[&active.primary].values[&active.mode], expected);
        assert_eq!(app.doc.bound_value(active.sample, "fills/0"), Some(expected));
        assert_eq!(app.doc.variables[&other.primary].values, other_values, "Editing one theme mode must preserve the other mode");
        assert_eq!(app.doc.bound_value(other.sample, "fills/0"), other_sample);
        assert!(app.doc.cityos.is_none(), "Local theme edits must not acquire an owner binding");
    }
}

#[test]
fn theme_variables_preserve_deliberate_collection_selection_when_reopening_the_same_page() {
    let (entry, specimen, snapshot) = editable_fixture();
    let document = local_library::create(&entry, &specimen, &snapshot, "light").unwrap();
    let light = local_theme_mode(&document, "light");
    let dark = local_theme_mode(&document, "dark");
    let mut app = crate::app::App::new(document);
    app.open_variables();
    assert_eq!(app.vars.collection.as_deref(), Some(light.collection.as_str()));
    let before = app.doc.to_json().unwrap();
    select_local_theme_collection(&mut app, &dark.collection);
    app.close_variables();
    app.switch_page(light.page);
    app.open_variables();
    assert_eq!(app.page, light.page);
    assert_eq!(app.vars.collection.as_deref(), Some(dark.collection.as_str()), "An explicit collection choice survives reopening on the same page");
    assert_eq!(app.doc.to_json().unwrap(), before, "Browsing collections must not edit the local theme");
}

#[test]
fn theme_variables_discard_a_removed_manual_collection_and_handle_no_collections() {
    let (entry, specimen, snapshot) = editable_fixture();
    let document = local_library::create(&entry, &specimen, &snapshot, "light").unwrap();
    let light = local_theme_mode(&document, "light");
    let dark = local_theme_mode(&document, "dark");
    let mut app = crate::app::App::new(document);
    app.open_variables();
    select_local_theme_collection(&mut app, &dark.collection);
    app.close_variables();
    assert!(app.edit_variables(|store| (store.delete_collection(&dark.collection), vec![dark.collection.clone()])));
    app.open_variables();
    assert_eq!(app.vars.collection.as_deref(), Some(light.collection.as_str()), "Removed manual selection must fall back to the live page collection");
    app.close_variables();
    assert!(app.edit_variables(|store| (store.delete_collection(&light.collection), vec![light.collection.clone()])));
    app.open_variables();
    assert!(app.doc.variable_collections.is_empty());
    assert!(app.vars.collection.is_none(), "A removed collection must not survive when no collections remain");
}

#[test]
fn theme_variables_do_not_carry_manual_selection_into_a_replacement_document_with_reused_ids() {
    let (entry, specimen, snapshot) = editable_fixture();
    let document = local_library::create(&entry, &specimen, &snapshot, "light").unwrap();
    let light = local_theme_mode(&document, "light");
    let dark = local_theme_mode(&document, "dark");
    // Reopening a saved copy can reuse the name, page ids and collection ids.
    // Its editor session must not inherit the previous document's manual choice.
    let replacement = document.clone();
    let mut app = crate::app::App::new(document);
    app.open_variables();
    select_local_theme_collection(&mut app, &dark.collection);
    app.close_variables();
    assert!(app.replace_document(replacement, None));
    assert_eq!(app.page, light.page);
    app.open_variables();
    assert_eq!(app.vars.collection.as_deref(), Some(light.collection.as_str()), "Page and collection ids alone cannot identify the previous document session");

    let fresh = local_library::create(&entry, &specimen, &snapshot, "dark").unwrap();
    let fresh_dark = local_theme_mode(&fresh, "dark");
    assert_eq!(fresh_dark.page, light.page, "Independently generated documents reuse page ids");
    app.close_variables();
    assert!(app.replace_document(fresh, None));
    app.open_variables();
    assert_eq!(app.vars.collection.as_deref(), Some(fresh_dark.collection.as_str()));
    assert!(!app.doc.variable_collections.contains_key(&light.collection));
    assert!(!app.doc.variable_collections.contains_key(&dark.collection));
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
