//! Thin native editor adapter for the CityOS extension.
mod bridge;
mod host;
mod layout;
mod dependencies;
mod submission_recovery;
mod repaint;
mod native_projection;
mod presentation;
mod history_commands;
mod session;
mod editor_host;
#[cfg(test)] mod atlas_tests;
#[cfg(test)] mod material_tests;
#[cfg(test)] mod horizon_tests;
use crate::app::App;
use desigma_doc::{CityosBase,CityosBinding,CityosOccurrence,Command,Document};
use serde_json::{json,Value};
use std::collections::BTreeMap;
use std::time::{Duration,Instant};
use desigma_cityos::draft::{binding,draft,revision,local_copy};
pub use desigma_cityos::bridge::Config;
pub type Workspace = desigma_cityos::workspace::Workspace<App>;
#[cfg(test)] use desigma_cityos::workspace::*;
#[cfg(test)] use repaint::RENDER_ACK_TIMEOUT;
/// Headless delivery check through the same native document and trusted bridge paths.
pub fn verify_roundtrip(config: Config, directory: &std::path::Path) -> Result<Value, String> {
    let bundle: Value =
        serde_json::from_slice(&std::fs::read(directory.join("canonical-bundle.json")).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let bridge = bridge::Bridge::spawn(config.clone());
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
    // Exercise the same native host and conditional checkpoint save used by the
    // desktop, including exact retained dependencies and reopened Undo/Redo.
    let library = desigma_library::Library::open(directory.join("draft-library")).map_err(|e| e.to_string())?;
    let retained = ask(json!({"op":"Dependencies","draft":draft(&document)}))?;
    dependencies::retain(&config.cache, &library, draft(&document).ok_or("Missing binding")?, &retained)?;
    let mut app = App::new(document);
    app.lib = Some(library.clone()); app.home = None;
    <App as desigma_cityos::editor::EditorHost>::save_to_draft_library(&mut app)?;
    <App as desigma_cityos::editor::EditorHost>::install_edit(&mut app, binding(result["draft"].clone(), &result["projection"])?)?;
    if !dependencies::complete(&library, &app.doc) { return Err("Edited dependency closure is incomplete".into()); }
    app.save_library_file()?;
    let file = app.lib_file.as_ref().ok_or("Missing saved draft")?.id.clone();
    fn reopen(library: &desigma_library::Library, file: &str) -> Result<App, String> {
        let mut reopened = App::new(desigma_doc::Document::new("Unopened checkpoint"));
        reopened.lib = Some(library.clone());
        reopened.open_library_file(file);
        if reopened.lib_file.as_ref().is_none_or(|opened| opened.id != file) {
            return Err("Native host failed to reopen the saved checkpoint".into());
        }
        Ok(reopened)
    }
    app = reopen(&library, &file)?;
    if !app.hist.can_undo() { return Err("Reopening lost the saved undo history".into()); }
    document = app.doc.clone();
    let path = crate::io::save_path(&document, &directory.join("native-resting.desigma"))?;
    let reopened = crate::io::load_path(&path)?;
    let exported = ask(json!({"op":"Export","draft":draft(&reopened)}))?;
    if exported["operationCount"] != 1
        || exported["changeset"]["changes"][0]["field"] != "elevation"
        || draft(&reopened).unwrap()["semanticDraft"]["root"]["slots"]["horizon"] != bundle["draft"]["semanticBase"]["root"]["slots"]["horizon"]
    {
        return Err("Unexpected semantic export".into());
    }
    app.hist.undo(&mut app.doc).map_err(|e| e.to_string())?;
    app.save_library_file()?;
    app = reopen(&library, &file)?;
    if !app.hist.can_redo() { return Err("Reopening lost the saved redo history".into()); }
    if ask(json!({"op":"Export","draft":draft(&app.doc)}))?["operationCount"] != 0 {
        return Err("Undo manufactured edits".into());
    }
    crate::home::thumb::cancel_library_jobs(library.root())?;
    Ok(
        json!({"nativeRoundtrip":true,"nativeApplicationHost":true,"checkpointHistoryRestored":true,"unchangedOperationCount":0,"editedOperationCount":1,"undoOperationCount":0,"canonicalBase":bundle["draft"]["base"],"embeddedRendererObserved":false}),
    )
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
    pub(super) fn specimen_receipt(workspace: &Workspace) -> Value {
        let component = workspace.resource.as_ref().unwrap()["componentId"].as_str().unwrap();
        let resource = registered_specimen(component, &workspace.locale);
        json!({"sessionId":"session-1","requestId":"workspace-3","draftId":format!("registered-fixture:{}",resource["resourceId"].as_str().unwrap()),
            "draftRevision":0,"semanticDigest":"c".repeat(64),"scenario":workspace.scenario(),
            "url":"http://127.0.0.1:12345/fixture","resource":resource,"provenance":resource["provenance"],
            "canonicalResource":false,"ownerWrites":false,"state":"prepared"})
    }
    pub(super) fn specimen_workspace() -> Workspace {
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
                workspace.local_specimen_request = workspace.captured_specimen_request();
                bridge.jobs.send(json!({"op":"Fixtures","fixture":workspace.local_specimen_request.as_ref().unwrap().locator,"locale":locale})).unwrap();
                let projected = bridge.replies.recv_timeout(Duration::from_secs(30)).unwrap().unwrap();
                let copy = workspace.local_specimen_document(projected).unwrap_or_else(|error|
                    panic!("{} / {locale}: {error}", selected["resourceId"]));
                assert!(copy.cityos.is_none(), "Explicit copies never gain submission authority");
                assert!(copy.nodes.values().all(|node| !node.locked && node.source_id.is_none()));
                receipts.push(json!({"resourceId":receipt["resource"]["resourceId"],"locale":locale,
                    "scenario":receipt["scenario"],"artifactDigest":receipt["artifactDigest"],
                    "provenance":receipt["provenance"],"nativeIdentityAccepted":true,"localCopyIdentityAccepted":true,"embeddedRendererObserved":false}));
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
    pub(super) fn tab_document(id:&str)->Document{
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
    fn long_projection_captions_remain_inside_native_object_bounds() {
        let workspace = specimen_workspace();
        let mut receipt = specimen_projection(&workspace);
        let label = "كبسولة السياق · Context capsule · state: unavailable · ".repeat(20);
        receipt["projection"]["nodes"][0]["label"] = json!(label);
        let (document, _) = projection_document(&receipt["projection"], "Inspection").unwrap();
        let caption = document.nodes.values().find(|node| matches!(node.kind, desigma_doc::NodeKind::Text(_))).unwrap();
        let desigma_doc::NodeKind::Text(text) = &caption.kind else { unreachable!() };
        assert_eq!(text.content, label, "Caption fitting cannot change source content");
        let outlines = desigma_render::text_outlines(text, caption.size);
        assert!(!outlines.is_empty());
        for outline in outlines {
            let ink = outline.path.bounds();
            assert!(ink.left() >= -1. && ink.right() <= caption.size.x + 1., "Caption overflow: {ink:?}");
            assert!(ink.top() >= -1. && ink.bottom() <= caption.size.y + 1.);
        }
        assert_eq!(crate::layout::fit_text(text, caption.size), caption.size, "Editing must retain the bounded box");
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
        let (document, fingerprint) = crate::io::load_path_observed(&path).unwrap();
        app.doc = document;
        app.file_path = Some(path.clone());
        app.home = None;
        app.autosave = crate::home::Autosave { seen_rev: app.canvas.tiles.rev(), saved_hash: crate::home::doc_hash(&app.doc),
            external_fingerprint: Some(fingerprint), ..Default::default() };
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
        app.autosave.external_fingerprint = None; // This intentionally invalid target has never been observed.
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
        let mut workspace=Workspace::default();workspace.selected="material".into();workspace.replace_active(&mut app,tab_document_node("second","accordion"),None).unwrap();
        let first_file=workspace.tabs["first"].session.as_ref().unwrap().file_id().unwrap().to_owned();
        app.home=None;workspace.selected="accordion".into();
        workspace.activate_tab(&mut app,"first");assert_eq!(draft(&app.doc).unwrap()["draftId"],"first");assert_eq!(app.lib_file.as_ref().unwrap().id,first_file);assert!(app.hist.can_undo());
        assert_eq!(workspace.selected,"material");assert_eq!(workspace.tabs["second"].selected,"accordion");
        assert_ne!(workspace.tabs["second"].session.as_ref().unwrap().file_id().unwrap(),first_file);app.hist.undo(&mut app.doc).unwrap();
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
                app.toast=if iteration%4==3 {Some(crate::app::Toast {text:"Save failed: the current draft remains local and can be retried without discarding changes.".into(),error:true,until:100.})} else {None};
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
        let before=app.doc.to_json().unwrap();let workspace=Workspace::default();assert!(workspace.prepare_switch(&mut app,false).is_err());
        assert_eq!(app.doc.to_json().unwrap(),before);assert!(app.hist.can_undo());assert!(workspace.tabs.is_empty());
    }
    #[test]
    fn unsaved_ordinary_notes_are_finalized_before_the_workspace_allows_a_switch(){
        let mut app=App::new(crate::decks::new_document("Unsaved ordinary design"));app.home=None;
        let slide=app.deck_slides()[0];app.decks.notes_for=Some(slide);app.decks.notes_draft="Keep these pending notes".into();
        assert!(!app.hist.can_undo());
        assert!(Workspace::save_before_switch(&mut app).is_err(),"Pending ordinary edits require a save before replacement");
        assert_eq!(app.doc.get(slide).unwrap().slide.as_ref().unwrap().notes,"Keep these pending notes");
        assert!(app.hist.can_undo());
    }
    #[test]
    fn external_ordinary_notes_are_finalized_before_the_workspace_saves_a_switch(){
        let root=std::env::temp_dir().join(format!("desigma-switch-notes-{}",uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let mut app=App::new(crate::decks::new_document("External ordinary design"));app.home=None;
        let path=root.join("ordinary.desigma");crate::io::save_path(&app.doc,&path).unwrap();
        let (document,fingerprint)=crate::io::load_path_observed(&path).unwrap();app.doc=document;
        app.file_path=Some(path.clone());
        app.autosave=crate::home::Autosave { seen_rev:app.canvas.tiles.rev(),saved_hash:crate::home::doc_hash(&app.doc),
            external_fingerprint:Some(fingerprint),..Default::default() };
        let slide=app.deck_slides()[0];app.decks.notes_for=Some(slide);app.decks.notes_draft="Persist these pending notes".into();
        Workspace::save_before_switch(&mut app).unwrap();
        let saved=crate::io::load_path(&path).unwrap();
        assert_eq!(saved.get(slide).unwrap().slide.as_ref().unwrap().notes,"Persist these pending notes");
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn failure_after_switch_preparation_keeps_the_incoming_tab_and_active_history(){
        let root=std::env::temp_dir().join(format!("desigma-final-switch-{}",uuid::Uuid::new_v4()));
        let mut app=App::new(tab_document("first"));app.home=None;
        app.lib=Some(desigma_library::Library::open(&root).unwrap());
        app.lib.as_ref().unwrap().put_cityos_payload(&desigma_library::payload_digest(b"fixture"),b"fixture").unwrap();
        let mut workspace=Workspace::default();
        workspace.replace_active(&mut app,tab_document("second"),None).unwrap();
        workspace.prepare_switch(&mut app,true).unwrap();
        crate::home::thumb::finish_library_jobs(&root);
        let manifest=root.join("projects").join(&app.lib_file.as_ref().unwrap().project_id).join("project.json");
        let manifest_bytes=std::fs::read(&manifest).unwrap();
        let mut edited=app.doc.clone();edited.name="Second remains unsaved".into();
        app.hist.exec(&mut app.doc,Command::CityosInstall{document:Box::new(edited)}).unwrap();app.changed();
        std::fs::write(&manifest,b"{invalid manifest").unwrap();
        let before=app.doc.to_json().unwrap();let history=serde_json::to_value(&app.hist).unwrap();
        let target=workspace.tabs["first"].session.as_ref().unwrap().name().to_owned();
        workspace.activate_prepared_tab(&mut app,"first");
        assert!(workspace.error.is_some());assert_eq!(app.doc.to_json().unwrap(),before);
        assert_eq!(serde_json::to_value(&app.hist).unwrap(),history);assert!(app.hist.can_undo());
        assert_eq!(workspace.tabs["first"].session.as_ref().unwrap().name(),target);
        assert!(!workspace.tabs.contains_key("second"));assert!(app.autosave.pending());
        std::fs::write(manifest,manifest_bytes).unwrap();
        crate::home::thumb::finish_library_jobs(&root);std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    #[ignore="requires the packaged canonical Accordion bundle; exercised by the Windows delivery check"]
    fn canonical_accordion_native_save_reopen_and_boolean_export(){
        let directory=std::path::PathBuf::from(std::env::var("CITYOS_DESIGN_WORKSPACE_EVIDENCE").unwrap());
        let bundle:Value=serde_json::from_slice(&std::fs::read(directory.join("accordion-bundle.json")).unwrap()).unwrap();
        let edited:Value=serde_json::from_slice(&std::fs::read(directory.join("accordion-edited-bundle.json")).unwrap()).unwrap();
        let document=binding(bundle["draft"].clone(),&bundle["projection"]).unwrap();
        assert_eq!(document.cityos.as_ref().unwrap().occurrences.len(),9,"Requires actual native visual parts");
        assert_eq!(document.nodes.values().filter(|node|matches!(node.kind,desigma_doc::NodeKind::Text(_))).count(),2);
        assert_eq!(document.nodes.values().filter(|node|matches!(node.kind,desigma_doc::NodeKind::Vector {..})).count(),2);
        for node in document.nodes.values().filter(|node|!document.pages.contains(&node.id)) {assert!(node.locked);}
        let path=crate::io::save_path(&document,&directory.join("native-accordion.desigma")).unwrap();
        let mut reopened=crate::io::load_path(&path).unwrap();let mut history=desigma_doc::History::default();
        history.exec(&mut reopened,Command::CityosInstall{document:Box::new(binding(edited["draft"].clone(),&edited["projection"]).unwrap())}).unwrap();
        crate::io::save_path(&reopened,&path).unwrap();let reopened=crate::io::load_path(&path).unwrap();
        let bridge=bridge::Bridge::spawn(Config{node:std::env::var("CITYOS_DESIGN_WORKSPACE_NODE").unwrap().into(),bridge:std::path::PathBuf::from(std::env::var("CITYOS_DESIGN_WORKSPACE_PACKAGE").unwrap()).join("workspace.mjs"),
            repository:std::env::var("CITYOS_DESIGN_WORKSPACE_REPOSITORY").unwrap().into(),cache:directory.join("canonical-cache")});
        bridge.jobs.send(json!({"op":"Export","draft":draft(&reopened).unwrap()})).unwrap();
        let exported=bridge.replies.recv_timeout(Duration::from_secs(30)).unwrap().unwrap();assert_eq!(exported["operationCount"],1);assert_eq!(exported["ownerSubmission"],false);
        assert_eq!(exported["changeset"]["changes"][0]["sourcePointer"],"/root/props/allowMultiple");
        bridge.jobs.send(json!({"op":"Export","draft":draft(&document).unwrap()})).unwrap();
        assert_eq!(bridge.replies.recv_timeout(Duration::from_secs(30)).unwrap().unwrap()["operationCount"],0);
        let d=draft(&reopened).unwrap();
        bridge.jobs.send(json!({"op":"Command","draft":d,"command":{"draftId":d["draftId"],"expectedRevision":d["draftRevision"],
            "operationId":"native-accordion-return","operation":"set-property","nodeId":"accordion","property":"allowMultiple","value":false}})).unwrap();
        let returned=bridge.replies.recv_timeout(Duration::from_secs(30)).unwrap().unwrap();
        assert_eq!(returned["draft"]["semanticDraft"]["root"]["items"],bundle["draft"]["semanticBase"]["root"]["items"]);
        let returned=binding(returned["draft"].clone(),&returned["projection"]).unwrap();
        let path=crate::io::save_path(&returned,&directory.join("native-accordion-returned.desigma")).unwrap();let returned=crate::io::load_path(&path).unwrap();
        bridge.jobs.send(json!({"op":"Export","draft":draft(&returned).unwrap()})).unwrap();
        assert_eq!(bridge.replies.recv_timeout(Duration::from_secs(30)).unwrap().unwrap()["operationCount"],0);
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
