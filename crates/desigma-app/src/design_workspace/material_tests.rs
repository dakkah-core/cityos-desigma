//! Native workspace regressions using the existing in-memory bridge transport.
//! Events and command replies below are explicit protocol fixtures, not renderer
//! observations or compiler/package qualification. No renderer is implemented here.
use super::*;

const MATERIAL: &str = "/root/slots/material/0/props/material";

fn resource() -> Value {
    // Mirrors the dynamic V3 field metadata, including elevation-first ordering.
    // This fixture deliberately has no actual-renderer/native-scene capability.
    json!({"fixture":"material-protocol-test","editable":true,"fields":[
        {"nodeId":"material","property":"elevation","operation":"set-property",
            "sourcePointer":"/root/slots/material/0/props/elevation","values":["flat","resting","floating"]},
        {"nodeId":"material","property":"material","operation":"set-property",
            "sourcePointer":MATERIAL,"values":["ambient","glass","raised","trust"]}]})
}

fn document(material: &str) -> Document {
    let mut document = super::tests::tab_document("material-protocol-test");
    let semantic = json!({"id":"material-protocol-test","revision":"1","root":{"id":"frame","slots":{
        "material":[{"id":"material","component":{"id":"cityos.l1.living-material.v1","version":"1.0.0"},
            "props":{"material":material,"elevation":"flat"}}]}}});
    let binding = document.cityos.as_mut().unwrap();
    binding.semantic_base = semantic.clone();
    let exchange = binding.exchange_mut().unwrap();
    exchange["semanticBase"] = semantic.clone();
    exchange["semanticDraft"] = semantic;
    exchange["semanticDigest"] = json!(desigma_library::payload_digest(&serde_json::to_vec(&exchange["semanticDraft"]).unwrap()));
    document
}

fn workspace() -> (Workspace, std::sync::mpsc::Receiver<Value>, std::sync::mpsc::Sender<Result<Value, String>>) {
    let (bridge, requests, responses) = bridge::Bridge::test_transport();
    let workspace = Workspace { bridge:Some(bridge), validated:true, themes_loaded:true,
        resource:Some(resource()), ..Default::default() };
    (workspace, requests, responses)
}

fn prepared(workspace: &Workspace, document: &Document) -> Value {
    let draft = draft(document).unwrap();
    json!({"sessionId":"material-session","requestId":"material-request","draftId":draft["draftId"],
        "draftRevision":draft["draftRevision"],"semanticDigest":draft["semanticDigest"],
        "scenario":workspace.scenario(),"state":"prepared"})
}

fn rendered(receipt: &Value, material: &str) -> Value {
    let mut event = receipt.clone();
    event["kind"] = json!("rendered");
    event["viewport"] = json!({"width":560,"height":720});
    event["elevation"] = json!("flat");
    event["shadow"] = json!("none");
    event["themePrimary"] = json!("#123456");
    event["themePage"] = json!("#ffffff");
    event["material"] = json!(material);
    event["background"] = json!("rgba(255, 255, 255, 0.8)");
    event["border"] = json!("1px solid rgb(20, 30, 40)");
    event["backdrop"] = json!(if material == "glass" { "blur(12px)" } else { "none" });
    event
}

fn assert_ignored(workspace: &mut Workspace, app: &mut App, event: &Value) {
    let preview = workspace.preview.clone();
    let current = workspace.current;
    let started = workspace.render_started;
    let error = workspace.error.clone();
    let document = app.doc.to_json().unwrap();
    let history = serde_json::to_value(&app.hist).unwrap();
    let selection = app.selection.clone();
    let saved = (app.autosave.saved_hash, app.autosave.saves, app.autosave.pending());
    workspace.handle_preview_event(app, event);
    assert_eq!(workspace.preview, preview);
    assert_eq!(workspace.current, current);
    assert_eq!(workspace.render_started, started);
    assert_eq!(workspace.error, error);
    assert_eq!(app.doc.to_json().unwrap(), document);
    assert_eq!(serde_json::to_value(&app.hist).unwrap(), history);
    assert_eq!(app.selection, selection);
    assert_eq!((app.autosave.saved_hash, app.autosave.saves, app.autosave.pending()), saved);
}

#[test]
fn current_material_observations_update_telemetry_without_authoring_native_paints_or_history() {
    for material in ["ambient", "glass", "raised", "trust"] {
        let mut app = App::new(document("ambient")); app.home = None;
        let (mut workspace, _requests, _responses) = workspace();
        let receipt = prepared(&workspace, &app.doc);
        workspace.preview = Some(receipt.clone());
        workspace.render_started = Some(Instant::now());
        let before = app.doc.to_json().unwrap();
        let history = serde_json::to_value(&app.hist).unwrap();
        let saved = (app.autosave.saved_hash, app.autosave.saves, app.autosave.pending());
        let event = rendered(&receipt, material);
        workspace.handle_preview_event(&mut app, &event);
        assert!(workspace.current && workspace.render_started.is_none());
        let preview = workspace.preview.as_ref().unwrap();
        for (observed, field) in [("observedMaterial", "material"), ("observedBackground", "background"),
            ("observedBorder", "border"), ("observedBackdrop", "backdrop"), ("observedElevation", "elevation"),
            ("observedViewport", "viewport"), ("observedShadow", "shadow"),
            ("observedThemePrimary", "themePrimary"), ("observedThemePage", "themePage")] {
            assert_eq!(preview[observed], event[field], "{observed}");
        }
        assert_eq!(app.doc.to_json().unwrap(), before, "CSS observations cannot author a native material");
        assert_eq!(serde_json::to_value(&app.hist).unwrap(), history);
        assert_eq!((app.autosave.saved_hash, app.autosave.saves, app.autosave.pending()), saved);
        assert_eq!(draft(&app.doc).unwrap()["semanticDraft"].pointer(MATERIAL).unwrap(), "ambient");
    }
}

#[test]
fn stale_rendered_material_cannot_cross_any_receipt_fence_or_replace_previous_observations() {
    for current in [false, true] {
        for key in ["sessionId", "requestId", "draftId", "draftRevision", "semanticDigest", "scenario"] {
            let mut app = App::new(document("ambient")); app.home = None;
            let (mut workspace, _requests, _responses) = workspace();
            let receipt = prepared(&workspace, &app.doc);
            let accepted = rendered(&receipt, "ambient");
            workspace.preview = Some(receipt.clone());
            workspace.handle_preview_event(&mut app, &accepted);
            workspace.current = current;
            workspace.render_started = Some(Instant::now());
            let mut stale = rendered(&receipt, "glass");
            stale[key] = match key {
                "draftRevision" => json!(1),
                "scenario" => json!({"locale":"en"}),
                _ => json!("obsolete"),
            };
            assert_ignored(&mut workspace, &mut app, &stale);
        }
    }
    // Compare the entire scenario, including fixed policy fields and dimensions.
    for (pointer, value) in [("/target", json!("native")), ("/locale", json!("en")),
        ("/theme", json!("dark")), ("/themeId", json!("other-theme")),
        ("/viewport/width", json!(561)), ("/viewport/height", json!(721)),
        ("/fixture", json!("other-fixture")), ("/renderMode", json!("interactive-fixture")),
        ("/motion", json!("full"))] {
        let mut app = App::new(document("ambient")); app.home = None;
        let (mut workspace, _requests, _responses) = workspace();
        let receipt = prepared(&workspace, &app.doc);
        workspace.preview = Some(receipt.clone());
        workspace.render_started = Some(Instant::now());
        let mut event = rendered(&receipt, "glass");
        *event["scenario"].pointer_mut(pointer).unwrap() = value;
        assert_ignored(&mut workspace, &mut app, &event);
    }
}

#[test]
fn matching_missing_empty_or_malformed_identities_do_not_acknowledge_material() {
    for key in ["sessionId", "requestId", "draftId", "draftRevision", "semanticDigest", "scenario"] {
        for bad in [Value::Null, json!(""), json!({})] {
            let mut app = App::new(document("ambient")); app.home = None;
            let (mut workspace, _requests, _responses) = workspace();
            let mut receipt = prepared(&workspace, &app.doc);
            receipt[key] = bad;
            workspace.preview = Some(receipt.clone());
            workspace.render_started = Some(Instant::now());
            let event = rendered(&receipt, "glass");
            assert_ignored(&mut workspace, &mut app, &event);
            // A missing field on both sides must not pass through null == null.
            receipt.as_object_mut().unwrap().remove(key);
            workspace.preview = Some(receipt.clone());
            assert_ignored(&mut workspace, &mut app, &rendered(&receipt, "glass"));
        }
    }
}

#[test]
fn material_observations_are_fenced_by_live_scenario_and_active_document_scope() {
    for change in ["locale", "theme", "themeId", "viewport", "fixture", "renderMode", "draft", "revision", "digest"] {
        let mut app = App::new(document("ambient")); app.home = None;
        let (mut workspace, _requests, _responses) = workspace();
        let receipt = prepared(&workspace, &app.doc);
        workspace.preview = Some(receipt.clone());
        workspace.render_started = Some(Instant::now());
        match change {
            "locale" => workspace.locale = "en".into(),
            "theme" => workspace.theme = "dark".into(),
            "themeId" => workspace.theme_id = "another-theme".into(),
            "viewport" => workspace.viewport.0 += 1,
            "fixture" => workspace.resource.as_mut().unwrap()["fixture"] = json!("another-fixture"),
            "renderMode" => workspace.interactive = true,
            "draft" => app.doc = super::tests::tab_document("another-draft"),
            "revision" => app.doc.cityos.as_mut().unwrap().exchange_mut().unwrap()["draftRevision"] = json!(1),
            "digest" => app.doc.cityos.as_mut().unwrap().exchange_mut().unwrap()["semanticDigest"] = json!("d".repeat(64)),
            _ => unreachable!(),
        }
        assert_ignored(&mut workspace, &mut app, &rendered(&receipt, "glass"));
    }
    // A temporary canonical preview belongs to staged, never the saved app tab.
    let mut app = App::new(super::tests::tab_document("saved-tab")); app.home = None;
    let (mut workspace, _requests, _responses) = workspace();
    workspace.preview_only = true;
    workspace.staged = Some(document("trust"));
    let receipt = prepared(&workspace, workspace.staged.as_ref().unwrap());
    workspace.preview = Some(receipt.clone());
    let before = app.doc.to_json().unwrap();
    workspace.handle_preview_event(&mut app, &rendered(&receipt, "trust"));
    assert!(workspace.current);
    assert_eq!(app.doc.to_json().unwrap(), before);
    workspace.staged = None;
    workspace.current = false;
    assert_ignored(&mut workspace, &mut app, &rendered(&receipt, "glass"));
}

#[test]
fn registered_specimen_material_observations_remain_inside_their_temporary_scope() {
    for fault in ["busy", "pending", "locator", "canonical", "scope"] {
        let mut app = App::new(document("ambient")); app.home = None;
        let mut workspace = super::tests::specimen_workspace();
        let receipt = super::tests::specimen_receipt(&workspace);
        assert!(workspace.accept_specimen_preview(receipt.clone(), "FixturePreview"));
        let before = app.doc.to_json().unwrap();
        workspace.handle_preview_event(&mut app, &rendered(&receipt, "raised"));
        assert!(workspace.current);
        assert_eq!(workspace.preview.as_ref().unwrap()["observedMaterial"], "raised");
        assert_eq!(app.doc.to_json().unwrap(), before);
        match fault {
            "busy" => workspace.busy = Some("FixturePreview".into()),
            "pending" => workspace.specimen_request = workspace.captured_specimen_request(),
            "locator" => workspace.resource.as_mut().unwrap()["profileRef"] = json!("other-profile"),
            "canonical" => workspace.preview.as_mut().unwrap()["canonicalResource"] = json!(true),
            "scope" => workspace.preview_only = false,
            _ => unreachable!(),
        }
        assert_ignored(&mut workspace, &mut app, &rendered(&receipt, "glass"));
    }
}

// A protocol-only Command response, following the existing history tests' seam.
// The fixed protected node is an inspection fixture, not a native material renderer.
fn command_reply(request: &Value, resource: &Value) -> Value {
    let mut next = request["draft"].clone();
    let field = resource["fields"].as_array().unwrap().iter().find(|field|
        field["nodeId"] == request["command"]["nodeId"] && field["property"] == request["command"]["property"]).unwrap();
    *next["semanticDraft"].pointer_mut(field["sourcePointer"].as_str().unwrap()).unwrap() = request["command"]["value"].clone();
    let revision = next["draftRevision"].as_u64().unwrap() + 1;
    next["draftRevision"] = json!(revision);
    next["semanticDigest"] = json!(desigma_library::payload_digest(&serde_json::to_vec(&next["semanticDraft"]).unwrap()));
    next["commandReceipts"].as_array_mut().unwrap().push(json!({"operationId":request["command"]["operationId"],
        "requestDigest":desigma_library::payload_digest(&serde_json::to_vec(&request["command"]).unwrap()),"revision":revision}));
    json!({"draft":next,"resource":resource,"projection":{"schemaVersion":"cityos.desigma-canvas.v1",
        "draftId":next["draftId"],"draftRevision":revision,"semanticDigest":next["semanticDigest"],"nodes":[
            {"id":"native","sourceNodeId":"material","definitionId":"fixture","parent":null,"slot":null,
                "x":0,"y":0,"width":100,"height":100,"label":"Protected protocol fixture","fill":"#ffffff"}]}})
}

#[test]
fn dynamic_material_enum_dispatches_intent_and_undo_redo_wait_for_fresh_boundary_replies() {
    let resource = resource();
    let field = resource["fields"].as_array().unwrap().iter().find(|field| field["property"] == "material").unwrap();
    assert_eq!(resource["fields"][0]["property"], "elevation");
    for option in field["values"].as_array().unwrap() {
        let original = if option.as_str() == Some("ambient") { "trust" } else { "ambient" };
        let mut app = App::new(document(original)); app.home = None;
        let (mut workspace, requests, responses) = workspace();
        let before = app.doc.to_json().unwrap();
        let history = serde_json::to_value(&app.hist).unwrap();
        workspace.send_property_command(&app, field, option.clone());
        let edit = requests.try_recv().unwrap();
        assert_eq!(edit["op"], "Command");
        assert_eq!(edit["draft"], draft(&app.doc).unwrap().clone());
        assert_eq!(edit["command"]["draftId"], edit["draft"]["draftId"]);
        assert_eq!(edit["command"]["expectedRevision"], 0);
        assert_eq!(edit["command"]["operation"], "set-property");
        assert_eq!(edit["command"]["nodeId"], "material");
        assert_eq!(edit["command"]["property"], "material");
        assert_eq!(&edit["command"]["value"], option);
        assert_eq!(app.doc.to_json().unwrap(), before);
        assert_eq!(serde_json::to_value(&app.hist).unwrap(), history);
        responses.send(Ok(command_reply(&edit, &resource))).unwrap(); workspace.poll(&mut app);
        assert!(workspace.error.is_none(), "{:?}", workspace.error);
        assert_eq!(draft(&app.doc).unwrap()["semanticDraft"].pointer(MATERIAL).unwrap(), option);
        assert_eq!(draft(&app.doc).unwrap()["semanticBase"].pointer(MATERIAL).unwrap(), original);
        assert!(app.hist.can_undo() && !app.hist.can_redo());
        for (redo, expected, revision) in [(false, json!(original), 2), (true, option.clone(), 3)] {
            let before = app.doc.to_json().unwrap();
            let history = serde_json::to_value(&app.hist).unwrap();
            if redo { app.redo(); } else { app.undo(); }
            assert_eq!(app.doc.to_json().unwrap(), before);
            workspace.poll(&mut app);
            let request = requests.try_recv().unwrap();
            assert_eq!(app.doc.to_json().unwrap(), before);
            assert_eq!(serde_json::to_value(&app.hist).unwrap(), history);
            assert_eq!(request["op"], "Command");
            assert_eq!(request["command"]["property"], "material");
            assert_eq!(request["command"]["value"], expected);
            assert_eq!(request["command"]["expectedRevision"], revision - 1);
            assert_ne!(request["command"]["operationId"], edit["command"]["operationId"]);
            responses.send(Ok(command_reply(&request, &resource))).unwrap(); workspace.poll(&mut app);
            assert!(workspace.error.is_none(), "{:?}", workspace.error);
            let next = draft(&app.doc).unwrap();
            assert_eq!(next["draftRevision"], revision);
            assert_eq!(next["semanticDraft"].pointer(MATERIAL).unwrap(), &expected);
            assert_eq!(next["commandReceipts"].as_array().unwrap().len(), revision as usize);
            assert_eq!(next["commandReceipts"][0]["operationId"], edit["command"]["operationId"]);
            assert_eq!(app.hist.can_undo(), redo);
            assert_eq!(app.hist.can_redo(), !redo);
        }
    }
}

#[test]
fn blocked_or_rejected_material_edits_leave_the_bound_document_and_history_unchanged() {
    let resource = resource();
    let field = &resource["fields"][1];
    for fault in ["busy", "unvalidated", "selection", "frozen", "boundary-rejection"] {
        let mut app = App::new(document("ambient")); app.home = None;
        let (mut workspace, requests, responses) = workspace();
        match fault {
            "busy" => workspace.busy = Some("Command".into()),
            "unvalidated" => workspace.validated = false,
            "selection" => workspace.selected = "frame".into(),
            "frozen" => app.ui.cityos_frozen_draft = Some(draft(&app.doc).unwrap()["draftId"].as_str().unwrap().into()),
            _ => {},
        }
        let before = app.doc.to_json().unwrap();
        let history = serde_json::to_value(&app.hist).unwrap();
        workspace.send_property_command(&app, field, json!("glass"));
        if fault == "boundary-rejection" {
            assert_eq!(requests.try_recv().unwrap()["command"]["value"], "glass");
            responses.send(Err("DESIGN_FIELD_UNSUPPORTED".into())).unwrap(); workspace.poll(&mut app);
            assert_eq!(workspace.error.as_deref(), Some("DESIGN_FIELD_UNSUPPORTED"));
        } else { assert!(requests.try_recv().is_err(), "{fault}"); }
        assert_eq!(app.doc.to_json().unwrap(), before, "{fault}");
        assert_eq!(serde_json::to_value(&app.hist).unwrap(), history, "{fault}");
    }
}

#[test]
fn native_save_reopen_preserves_material_intent_baseline_and_binding() {
    let resource = resource();
    let field = &resource["fields"][1];
    for material in field["values"].as_array().unwrap() {
        let original = if material.as_str() == Some("ambient") { "trust" } else { "ambient" };
        let mut app = App::new(document(original)); app.home = None;
        let (mut workspace, requests, responses) = workspace();
        workspace.send_property_command(&app, field, material.clone());
        let request = requests.try_recv().unwrap();
        responses.send(Ok(command_reply(&request, &resource))).unwrap(); workspace.poll(&mut app);
        assert!(workspace.error.is_none());
        let path = std::env::temp_dir().join(format!("desigma-material-{}.desigma", uuid::Uuid::new_v4()));
        let saved = crate::io::save_path(&app.doc, &path).unwrap();
        let reopened = crate::io::load_path(&saved).unwrap();
        std::fs::remove_file(&saved).unwrap();
        assert_eq!(draft(&reopened), draft(&app.doc));
        assert_eq!(draft(&reopened).unwrap()["semanticDraft"].pointer(MATERIAL).unwrap(), material);
        assert_eq!(draft(&reopened).unwrap()["semanticBase"].pointer(MATERIAL).unwrap(), original);
        assert_eq!(serde_json::to_value(&reopened.cityos).unwrap(), serde_json::to_value(&app.doc.cityos).unwrap());
        assert!(reopened.nodes.values().filter(|node| node.source_id.is_some()).all(|node| node.locked));
    }
}
