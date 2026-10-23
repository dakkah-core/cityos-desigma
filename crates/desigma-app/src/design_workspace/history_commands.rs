#[cfg(test)]
mod tests {
    use super::super::*;

    fn document() -> Document {
        let semantic = json!({"id":"frame","revision":"1","root":{"id":"material","props":{"elevation":"flat"}}});
        let value = json!({"schemaVersion":"cityos.design-exchange.v3","documentId":"document","draftId":"draft",
            "base":{"kind":"canonical-git","repositoryId":"repo","commit":"a".repeat(40)},
            "semanticContract":"cityos.experience-composition.v1","semanticBase":semantic,"semanticDraft":semantic,
            "draftRevision":0,"semanticDigest":"base","commandReceipts":[],"dependencyPins":[],"profileRef":"fixture-profile"});
        let mut doc = Document::new("Bound frame");
        doc.add_page("P");
        doc.cityos = Some(CityosBinding {
            schema_version: "cityos.design-exchange.v3".into(),
            document_id: "document".into(),
            draft_id: "draft".into(),
            base: CityosBase::CanonicalGit { repository_id: "repo".into(), commit: "a".repeat(40) },
            semantic_base: semantic,
            dependency_pins: vec![],
            occurrences: BTreeMap::new(),
            exchange_v2: None,
            exchange_v3: Some(value),
        });
        doc
    }

    fn edited_app() -> App {
        let mut app = App::new(document());
        app.home = None;
        let mut next = app.doc.clone();
        let value = next.cityos.as_mut().unwrap().exchange_mut().unwrap();
        value["semanticDraft"]["root"]["props"]["elevation"] = json!("resting");
        value["semanticDigest"] = json!("resting");
        value["commandReceipts"] = json!([{"operationId":"edit","requestDigest":"b".repeat(64),"revision":1}]);
        app.hist.exec(&mut app.doc, Command::CityosInstall { document: Box::new(next) }).unwrap();
        app
    }

    #[test]
    fn bound_undo_waits_for_trusted_validation_before_changing_document_or_history() {
        let mut app = edited_app();
        let document = app.doc.to_json().unwrap();
        let history = serde_json::to_value(&app.hist).unwrap();
        app.undo();
        assert_eq!(app.doc.to_json().unwrap(), document, "Bound undo must not install an unvalidated historical draft");
        assert_eq!(serde_json::to_value(&app.hist).unwrap(), history);
    }

    fn resource() -> Value {
        json!({"fixture":"history-test","fields":[{"nodeId":"material","property":"elevation",
            "operation":"set-property","sourcePointer":"/root/props/elevation"}]})
    }

    fn workspace() -> (Workspace, std::sync::mpsc::Receiver<Value>, std::sync::mpsc::Sender<Result<Value, String>>) {
        let (bridge, requests, responses) = super::super::bridge::Bridge::test_transport();
        let mut workspace = Workspace {
            bridge: Some(bridge),
            validated: true,
            themes_loaded: true,
            resource: Some(resource()),
            ..Default::default()
        };
        (workspace, requests, responses)
    }

    // Protocol-faithful compiler fixture. Live compiler coverage is separate.
    fn reply(request: &Value) -> Value {
        let mut next = request["draft"].clone();
        let revision = next["draftRevision"].as_u64().unwrap() + 1;
        next["semanticDraft"]["root"]["props"]["elevation"] = request["command"]["value"].clone();
        next["semanticDigest"] = json!(format!("revision-{revision}"));
        next["draftRevision"] = json!(revision);
        next["commandReceipts"]
            .as_array_mut()
            .unwrap()
            .push(json!({"operationId":request["command"]["operationId"],"requestDigest":"c".repeat(64),"revision":revision}));
        json!({"draft":next,"resource":resource(),"projection":{"schemaVersion":"cityos.desigma-canvas.v1",
            "draftId":next["draftId"],"draftRevision":next["draftRevision"],"semanticDigest":next["semanticDigest"],"nodes":[]}})
    }

    #[test]
    fn undo_redo_are_fresh_commands_and_keep_the_cumulative_receipt_chain_and_stacks() {
        let mut app = edited_app();
        let (mut workspace, requests, responses) = workspace();
        app.doc.name = "My annotated draft".into();
        app.undo();
        workspace.poll(&mut app);
        let undo = requests.try_recv().unwrap();
        assert_eq!(undo["op"], "Command");
        assert_eq!(undo["command"]["expectedRevision"], 1);
        assert_eq!(undo["command"]["value"], "flat");
        assert_ne!(undo["command"]["operationId"], "edit");
        assert_eq!(draft(&app.doc).unwrap()["draftRevision"], 1);
        responses.send(Ok(reply(&undo))).unwrap();
        workspace.poll(&mut app);
        let undone = draft(&app.doc).unwrap();
        assert_eq!(undone["draftRevision"], 2);
        assert_eq!(undone["semanticDraft"]["root"]["props"]["elevation"], "flat");
        assert_eq!(undone["commandReceipts"].as_array().unwrap().len(), 2);
        assert!(!app.hist.can_undo());
        assert!(app.hist.can_redo());
        let before = app.doc.to_json().unwrap();
        app.redo();
        assert_eq!(app.doc.to_json().unwrap(), before);
        workspace.poll(&mut app);
        let redo = requests.try_recv().unwrap();
        assert_eq!(redo["command"]["expectedRevision"], 2);
        assert_eq!(redo["command"]["value"], "resting");
        assert_ne!(redo["command"]["operationId"], undo["command"]["operationId"]);
        responses.send(Ok(reply(&redo))).unwrap();
        workspace.poll(&mut app);
        let redone = draft(&app.doc).unwrap();
        assert_eq!(redone["draftRevision"], 3);
        assert_eq!(redone["commandReceipts"].as_array().unwrap().len(), 3);
        assert_eq!(redone["commandReceipts"][0]["operationId"], "edit");
        assert_eq!(app.doc.name, "My annotated draft");
        assert!(app.hist.can_undo());
        assert!(!app.hist.can_redo());
    }

    #[test]
    fn rejected_history_keeps_document_and_history_and_a_retry_gets_a_new_identity() {
        let mut app = edited_app();
        let (mut workspace, requests, responses) = workspace();
        let document = app.doc.to_json().unwrap();
        let history = serde_json::to_value(&app.hist).unwrap();
        app.undo();
        workspace.poll(&mut app);
        let first = requests.try_recv().unwrap();
        responses.send(Err("DESIGN_FIELD_UNSUPPORTED".into())).unwrap();
        workspace.poll(&mut app);
        assert_eq!(app.doc.to_json().unwrap(), document);
        assert_eq!(serde_json::to_value(&app.hist).unwrap(), history);
        app.undo();
        workspace.poll(&mut app);
        let retry = requests.try_recv().unwrap();
        assert_eq!(retry["draft"], first["draft"]);
        assert_ne!(retry["command"]["operationId"], first["command"]["operationId"]);
    }

    #[test]
    fn sealed_baselines_and_changed_revisions_fence_delayed_history_results() {
        for seal in [false, true] {
            let mut app = edited_app();
            let (mut workspace, requests, responses) = workspace();
            app.undo();
            workspace.poll(&mut app);
            let request = requests.try_recv().unwrap();
            if seal {
                app.hist.seal();
            } else {
                let mut newer = app.doc.clone();
                newer.cityos.as_mut().unwrap().exchange_mut().unwrap()["semanticDigest"] = json!("newer");
                app.hist.exec(&mut app.doc, Command::CityosInstall { document: Box::new(newer) }).unwrap();
            }
            let document = app.doc.to_json().unwrap();
            let history = serde_json::to_value(&app.hist).unwrap();
            responses.send(Ok(reply(&request))).unwrap();
            workspace.poll(&mut app);
            assert!(workspace.error.is_some());
            assert_eq!(app.doc.to_json().unwrap(), document);
            assert_eq!(serde_json::to_value(&app.hist).unwrap(), history);
        }
    }

    #[test]
    fn malformed_receipts_cannot_replace_the_live_chain_or_install_unrequested_semantics() {
        for fault in ["identity", "chain", "semantics", "base"] {
            let mut app = edited_app();
            let (mut workspace, requests, responses) = workspace();
            app.undo();
            workspace.poll(&mut app);
            let request = requests.try_recv().unwrap();
            let mut result = reply(&request);
            match fault {
                "identity" => {
                    result["draft"]["commandReceipts"][1]["operationId"] = json!("wrong-command");
                }
                "chain" => {
                    result["draft"]["commandReceipts"].as_array_mut().unwrap().remove(0);
                }
                "semantics" => {
                    result["draft"]["semanticDraft"]["root"]["props"]["elevation"] = json!("floating");
                }
                _ => {
                    result["draft"]["profileRef"] = json!("different-profile");
                }
            }
            let document = app.doc.to_json().unwrap();
            let history = serde_json::to_value(&app.hist).unwrap();
            responses.send(Ok(result)).unwrap();
            workspace.poll(&mut app);
            assert!(workspace.error.is_some(), "{fault}");
            assert_eq!(app.doc.to_json().unwrap(), document, "{fault}");
            assert_eq!(serde_json::to_value(&app.hist).unwrap(), history, "{fault}");
        }
    }

    #[test]
    fn unsupported_history_and_frozen_or_unvalidated_drafts_send_no_command() {
        for mode in ["structural", "frozen", "unvalidated"] {
            let mut app = edited_app();
            let (mut workspace, requests, _responses) = workspace();
            if mode == "structural" {
                app.doc.cityos.as_mut().unwrap().exchange_mut().unwrap()["semanticDraft"]["root"]["other"] = json!(true);
            }
            if mode == "frozen" {
                app.ui.cityos_frozen_draft = Some("draft".into());
            }
            if mode == "unvalidated" {
                workspace.validated = false;
            }
            let document = app.doc.to_json().unwrap();
            let history = serde_json::to_value(&app.hist).unwrap();
            app.undo();
            workspace.poll(&mut app);
            assert!(requests.try_recv().is_err(), "{mode}");
            assert_eq!(app.doc.to_json().unwrap(), document);
            assert_eq!(serde_json::to_value(&app.hist).unwrap(), history);
        }
    }
}
