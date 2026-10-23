//! Native inspector/history protocol regressions, not actual renderer tests.
//! Bridge replies and preview events are explicit fixtures. The fixed inspection
//! projection below neither renders a horizon nor applies semantic commands.
//! Persistence uses the real native save/load path; CMS owns schema/renderer proof.
use super::*;

const HORIZON: &str = "/root/slots/horizon/0/props/copilotLayout";
const MATERIAL: &str = "/root/slots/material/0";
// Authoritative CMS schema: packages/shared-experience-rendering/src/
// living-canvas-lower-level-composition.ts (LivingHorizonCompositionPropsV1 and
// validateLivingCanvasLowerLevelPropsV1), not a native semantic validator.
const LAYOUTS: [&str; 4] = ["conversation", "split", "canvas", "operations"];

fn resource() -> Value {
    // Material deliberately precedes horizon. Inspector routing must use the
    // declared owner, not a fixed field index or a hardcoded material node.
    json!({"fixture":"horizon-protocol-test","editable":true,"fields":[
        {"nodeId":"material","componentId":"cityos.l1.living-material.v1","componentVersion":"1.0.0",
            "property":"material","operation":"set-property","values":["ambient","glass","raised","trust"],
            "sourcePointer":"/root/slots/material/0/props/material"},
        {"nodeId":"horizon","componentId":"cityos.l7.living-horizon-region.v1","componentVersion":"1.0.0",
            "property":"copilotLayout","operation":"set-property","values":LAYOUTS,"sourcePointer":HORIZON}]})
}

fn horizon_field() -> Value {
    resource()["fields"].as_array().unwrap().iter().find(|field| field["nodeId"] == "horizon" && field["property"] == "copilotLayout").unwrap().clone()
}

fn composition_fixture(layout: &str) -> Value {
    json!({"schemaVersion":"cityos.experience-composition.v1","id":"horizon-protocol-test","revision":"1",
        "root":{"kind":"component","id":"frame","component":{"id":"cityos.l9.living-canvas-frame.v1","version":"1.0.0"},
            "props":{},"slots":{
                "material":[{"kind":"component","id":"material",
                    "component":{"id":"cityos.l1.living-material.v1","version":"1.0.0"},
                    "props":{"material":"glass","elevation":"resting"},"slots":{}}],
                "horizon":[{"kind":"component","id":"horizon",
                    "component":{"id":"cityos.l7.living-horizon-region.v1","version":"1.0.0"},
                    "props":{"copilotLayout":layout},"slots":{}}]}}})
}

fn projection_fixture(draft: &Value) -> Value {
    // Static protected inspection objects, identical for every layout. No DOM,
    // native scene, layout geometry, paints or component renderer is emulated.
    json!({"schemaVersion":"cityos.desigma-canvas.v1","draftId":draft["draftId"],
        "draftRevision":draft["draftRevision"],"semanticDigest":draft["semanticDigest"],"nodes":[
            {"id":"fixture-frame","sourceNodeId":"frame","definitionId":"cityos.l9.living-canvas-frame.v1",
                "parent":null,"slot":null,"x":0,"y":0,"width":200,"height":200,"label":"Frame protocol fixture","fill":"#ffffff"},
            {"id":"fixture-material","sourceNodeId":"material","definitionId":"cityos.l1.living-material.v1",
                "parent":"fixture-frame","slot":"material","x":0,"y":0,"width":100,"height":100,"label":"Material protocol fixture","fill":"#ffffff"},
            {"id":"fixture-horizon","sourceNodeId":"horizon","definitionId":"cityos.l7.living-horizon-region.v1",
                "parent":"fixture-frame","slot":"horizon","x":0,"y":100,"width":100,"height":100,"label":"Horizon protocol fixture","fill":"#ffffff"}]})
}

fn document(layout: &str) -> Document {
    let seed = super::tests::tab_document("horizon-protocol-test");
    let mut exchange = draft(&seed).unwrap().clone();
    exchange["semanticContract"] = json!("cityos.experience-composition.v1");
    exchange["semanticBase"] = composition_fixture(layout);
    exchange["semanticDraft"] = composition_fixture(layout);
    exchange["semanticDigest"] = json!(desigma_library::payload_digest(&serde_json::to_vec(&exchange["semanticDraft"]).unwrap()));
    let projection = projection_fixture(&exchange);
    binding(exchange, &projection).unwrap()
}

fn workspace() -> (Workspace, std::sync::mpsc::Receiver<Value>, std::sync::mpsc::Sender<Result<Value, String>>) {
    let (bridge, requests, responses) = bridge::Bridge::test_transport();
    (Workspace { bridge: Some(bridge), validated: true, themes_loaded: true, resource: Some(resource()), ..Default::default() }, requests, responses)
}

fn prepared(workspace: &Workspace, document: &Document) -> Value {
    let draft = draft(document).unwrap();
    json!({"sessionId":"horizon-session","requestId":"horizon-request","draftId":draft["draftId"],
        "draftRevision":draft["draftRevision"],"semanticDigest":draft["semanticDigest"],
        "scenario":workspace.scenario(),"state":"prepared"})
}

// Only frames an already-authored expected snapshot in the existing reply seam.
// It never interprets the command's owner, property, pointer or value, nor
// decides whether intent is valid. Real compiler acceptance is covered in CMS.
fn accepted_fixture(request: &Value, semantic_snapshot: Value) -> Value {
    let mut next = request["draft"].clone();
    let revision = next["draftRevision"].as_u64().unwrap() + 1;
    next["semanticDraft"] = semantic_snapshot;
    next["draftRevision"] = json!(revision);
    next["semanticDigest"] = json!(desigma_library::payload_digest(&serde_json::to_vec(&next["semanticDraft"]).unwrap()));
    next["commandReceipts"].as_array_mut().unwrap().push(json!({"operationId":request["command"]["operationId"],
        "requestDigest":desigma_library::payload_digest(&serde_json::to_vec(&request["command"]).unwrap()),"revision":revision}));
    json!({"draft":next,"resource":resource(),"projection":projection_fixture(&next)})
}

fn assert_intent(request: &Value, current: &Value, layout: &Value) {
    assert_eq!(request["op"], "Command");
    assert_eq!(&request["draft"], current);
    assert_eq!(request["command"]["draftId"], current["draftId"]);
    assert_eq!(request["command"]["expectedRevision"], current["draftRevision"]);
    assert_eq!(request["command"]["operation"], "set-property");
    assert_eq!(request["command"]["nodeId"], "horizon");
    assert_eq!(request["command"]["property"], "copilotLayout");
    assert_eq!(&request["command"]["value"], layout);
    assert!(!request["command"]["operationId"].as_str().unwrap().is_empty());
}

fn assert_preserved(document: &Document, original: &Value, layout: &str) {
    let current = draft(document).unwrap();
    assert_eq!(current["semanticDraft"].pointer(HORIZON).unwrap(), layout);
    assert_eq!(current["semanticDraft"].pointer(MATERIAL), original["semanticBase"].pointer(MATERIAL));
    for field in ["base", "semanticBase", "dependencyPins", "documentId", "profileRef", "semanticContract"] {
        assert_eq!(current[field], original[field], "{field}");
    }
    assert_eq!(document.cityos.as_ref().unwrap().semantic_base, original["semanticBase"]);
    for source in ["material", "horizon"] {
        let nodes: Vec<_> = document.nodes.values().filter(|node| node.source_id.as_deref() == Some(source)).collect();
        assert!(!nodes.is_empty(), "missing {source} occurrence");
        assert!(nodes.iter().all(|node| node.locked));
    }
}

fn unchanged_state(app: &App) -> (String, Value, Vec<desigma_doc::NodeId>, (u64, u32, bool)) {
    (
        app.doc.to_json().unwrap(),
        serde_json::to_value(&app.hist).unwrap(),
        app.selection.clone(),
        (app.autosave.saved_hash, app.autosave.saves, app.autosave.pending()),
    )
}

#[test]
fn selecting_horizon_dispatches_each_schema_layout_without_authoring_or_targeting_material() {
    let field = horizon_field();
    assert_eq!(field["componentId"], "cityos.l7.living-horizon-region.v1");
    assert_eq!(field["componentVersion"], "1.0.0");
    assert_eq!(field["sourcePointer"], HORIZON);
    assert_eq!(field["values"], json!(LAYOUTS));
    for layout in LAYOUTS {
        let mut app = App::new(document(if layout == "split" { "conversation" } else { "split" }));
        app.home = None;
        let (mut workspace, requests, _responses) = workspace();
        workspace.select(&mut app, "material");
        let mut event = prepared(&workspace, &app.doc);
        workspace.preview = Some(event.clone());
        event["kind"] = json!("selection");
        event["nodeId"] = json!("horizon");
        workspace.handle_preview_event(&mut app, &event);
        assert_eq!(workspace.selected, "horizon");
        assert_eq!(app.selection.len(), 1);
        assert_eq!(app.doc.get(app.selection[0]).unwrap().source_id.as_deref(), Some("horizon"));
        let before = unchanged_state(&app);
        workspace.send_property_command(&app, &field, json!(layout));
        assert_intent(&requests.try_recv().unwrap(), draft(&app.doc).unwrap(), &json!(layout));
        assert!(requests.try_recv().is_err());
        assert_eq!(unchanged_state(&app), before, "sending intent must wait for boundary validation");
    }
}

#[test]
fn blocked_horizon_controls_and_material_controls_on_horizon_cannot_mutate_or_dispatch() {
    for fault in ["busy", "unvalidated", "material-selected", "frame-selected", "frozen", "material-control"] {
        let mut app = App::new(document("split"));
        app.home = None;
        let (mut workspace, requests, _responses) = workspace();
        workspace.select(&mut app, "horizon");
        let mut field = horizon_field();
        match fault {
            "busy" => workspace.busy = Some("Command".into()),
            "unvalidated" => workspace.validated = false,
            "material-selected" => workspace.select(&mut app, "material"),
            "frame-selected" => workspace.select(&mut app, "frame"),
            "frozen" => app.ui.cityos_frozen_draft = Some(draft(&app.doc).unwrap()["draftId"].as_str().unwrap().into()),
            "material-control" => field = resource()["fields"][0].clone(),
            _ => unreachable!(),
        }
        let before = unchanged_state(&app);
        workspace.send_property_command(&app, &field, if fault == "material-control" { json!("trust") } else { json!("canvas") });
        assert!(requests.try_recv().is_err(), "{fault}");
        assert_eq!(unchanged_state(&app), before, "{fault}");
    }
}

#[test]
fn invalid_horizon_values_rejected_by_boundary_preserve_the_last_accepted_draft_and_history() {
    for invalid in [json!("compact"), Value::Null, json!(42), json!({"layout":"canvas"})] {
        let mut app = App::new(document("split"));
        app.home = None;
        let (mut workspace, requests, responses) = workspace();
        workspace.select(&mut app, "horizon");
        workspace.send_property_command(&app, &horizon_field(), json!("canvas"));
        let edit = requests.try_recv().unwrap();
        responses.send(Ok(accepted_fixture(&edit, composition_fixture("canvas")))).unwrap();
        workspace.poll(&mut app);
        assert!(workspace.error.is_none() && app.hist.can_undo());
        let before = unchanged_state(&app);
        workspace.send_property_command(&app, &horizon_field(), invalid.clone());
        let rejected = requests.try_recv().unwrap();
        assert_intent(&rejected, draft(&app.doc).unwrap(), &invalid);
        assert_eq!(unchanged_state(&app), before);
        // Invalid intent remains the compiler/profile's decision, not a local
        // mock validator. This explicit error fixture must never install state.
        responses.send(Err("DESIGN_FIELD_UNSUPPORTED".into())).unwrap();
        workspace.poll(&mut app);
        assert_eq!(workspace.error.as_deref(), Some("DESIGN_FIELD_UNSUPPORTED"));
        assert_eq!(unchanged_state(&app), before);
        assert!(app.hist.can_undo() && !app.hist.can_redo());
        assert!(requests.try_recv().is_err());
    }
}

#[test]
fn horizon_undo_redo_and_native_save_reopen_preserve_intent_material_and_immutable_base() {
    for layout in LAYOUTS {
        let baseline = if layout == "split" { "conversation" } else { "split" };
        let mut app = App::new(document(baseline));
        app.home = None;
        let (mut workspace, requests, responses) = workspace();
        workspace.select(&mut app, "horizon");
        let original = draft(&app.doc).unwrap().clone();
        let mut operation_ids = Vec::new();
        for (revision, expected) in [(1, layout), (2, baseline), (3, layout)] {
            let before = unchanged_state(&app);
            match revision {
                1 => workspace.send_property_command(&app, &horizon_field(), json!(layout)),
                2 => app.undo(),
                _ => app.redo(),
            }
            assert_eq!(unchanged_state(&app), before);
            workspace.poll(&mut app);
            let request = requests.try_recv().unwrap();
            assert_intent(&request, draft(&app.doc).unwrap(), &json!(expected));
            assert_eq!(request["command"]["expectedRevision"], revision - 1);
            assert_eq!(unchanged_state(&app), before, "history must wait for a fresh boundary reply");
            let operation_id = request["command"]["operationId"].clone();
            assert!(!operation_ids.contains(&operation_id));
            operation_ids.push(operation_id);
            responses.send(Ok(accepted_fixture(&request, composition_fixture(expected)))).unwrap();
            workspace.poll(&mut app);
            assert!(workspace.error.is_none(), "{:?}", workspace.error);
            assert_preserved(&app.doc, &original, expected);
            let current = draft(&app.doc).unwrap();
            assert_eq!(current["draftRevision"], revision);
            let receipts = current["commandReceipts"].as_array().unwrap();
            assert_eq!(receipts.len(), revision as usize);
            assert_eq!(receipts.iter().map(|receipt| receipt["operationId"].clone()).collect::<Vec<_>>(), operation_ids);
            assert_eq!(app.hist.can_undo(), revision != 2);
            assert_eq!(app.hist.can_redo(), revision == 2);
            assert_eq!(workspace.selected, "horizon");
            assert_eq!(app.selection.len(), 1);
            assert_eq!(app.doc.get(app.selection[0]).unwrap().source_id.as_deref(), Some("horizon"));

            let path = std::env::temp_dir().join(format!("desigma-horizon-{}.desigma", uuid::Uuid::new_v4()));
            let saved = crate::io::save_path(&app.doc, &path).unwrap();
            let reopened = crate::io::load_path(&saved);
            std::fs::remove_file(&saved).unwrap();
            let reopened = reopened.unwrap();
            assert_eq!(reopened.to_json().unwrap(), app.doc.to_json().unwrap());
            assert_eq!(draft(&reopened), Some(current));
            assert_preserved(&reopened, &original, expected);
        }
    }
}

fn rendered_fixture(receipt: &Value, layout: &str) -> Value {
    let mut event = receipt.clone();
    event["kind"] = json!("rendered");
    event["horizonLayout"] = json!(layout);
    event
}

fn assert_ignored(workspace: &mut Workspace, app: &mut App, event: &Value) {
    let before = unchanged_state(app);
    let preview = workspace.preview.clone();
    let current = workspace.current;
    let started = workspace.render_started;
    let error = workspace.error.clone();
    workspace.handle_preview_event(app, event);
    assert_eq!(unchanged_state(app), before);
    assert_eq!(workspace.preview, preview);
    assert_eq!(workspace.current, current);
    assert_eq!(workspace.render_started, started);
    assert_eq!(workspace.error, error);
}

#[test]
fn current_horizon_layout_observations_update_only_telemetry_without_authoring_intent() {
    for layout in LAYOUTS {
        let mut app = App::new(document("split"));
        app.home = None;
        let (mut workspace, requests, _responses) = workspace();
        let receipt = prepared(&workspace, &app.doc);
        workspace.preview = Some(receipt.clone());
        workspace.render_started = Some(Instant::now());
        let before = unchanged_state(&app);
        workspace.handle_preview_event(&mut app, &rendered_fixture(&receipt, layout));
        assert_eq!(workspace.preview.as_ref().unwrap()["observedHorizonLayout"], layout);
        assert!(workspace.current && workspace.render_started.is_none());
        assert_eq!(unchanged_state(&app), before);
        assert_eq!(draft(&app.doc).unwrap()["semanticDraft"].pointer(HORIZON).unwrap(), "split");
        assert!(requests.try_recv().is_err());
    }
}

#[test]
fn stale_horizon_layout_cannot_replace_current_observations_or_newer_native_intent() {
    let mut app = App::new(document("split"));
    app.home = None;
    let (mut workspace, requests, responses) = workspace();
    let receipt = prepared(&workspace, &app.doc);
    workspace.preview = Some(receipt.clone());
    workspace.handle_preview_event(&mut app, &rendered_fixture(&receipt, "split"));
    for key in ["sessionId", "requestId", "draftId", "draftRevision", "semanticDigest", "scenario"] {
        let mut stale = rendered_fixture(&receipt, "operations");
        stale[key] = match key {
            "draftRevision" => json!(1),
            "scenario" => json!({"locale":"en"}),
            _ => json!("obsolete"),
        };
        workspace.render_started = Some(Instant::now());
        assert_ignored(&mut workspace, &mut app, &stale);
    }
    // Even an event matching the old stored receipt is stale after a native edit.
    workspace.select(&mut app, "horizon");
    workspace.send_property_command(&app, &horizon_field(), json!("canvas"));
    let edit = requests.try_recv().unwrap();
    responses.send(Ok(accepted_fixture(&edit, composition_fixture("canvas")))).unwrap();
    workspace.poll(&mut app);
    assert!(workspace.error.is_none());
    let stale = rendered_fixture(&receipt, "operations");
    assert_ignored(&mut workspace, &mut app, &stale);
    let mut latest = prepared(&workspace, &app.doc);
    latest["requestId"] = json!("new-horizon-request");
    workspace.preview = Some(latest.clone());
    workspace.handle_preview_event(&mut app, &rendered_fixture(&latest, "canvas"));
    assert_eq!(workspace.preview.as_ref().unwrap()["observedHorizonLayout"], "canvas");
    assert_ignored(&mut workspace, &mut app, &stale);
    // A receipt that remains internally consistent cannot outlive live scenario drift.
    workspace.locale = "en".into();
    assert_ignored(&mut workspace, &mut app, &rendered_fixture(&latest, "operations"));
    assert_eq!(draft(&app.doc).unwrap()["semanticDraft"].pointer(HORIZON).unwrap(), "canvas");
}
