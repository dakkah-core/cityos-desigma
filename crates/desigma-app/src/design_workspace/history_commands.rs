//! Bound undo/redo is semantic intent, validated by the same compiler as an edit.

use super::{draft, Workspace};
use crate::app::App;
use desigma_doc::{CityosHistoryStep, Document};
use serde_json::{json, Value};

pub(super) struct PendingHistory {
    step: CityosHistoryStep,
    command: Value,
}

fn immutable_draft(value: &Value) -> Value {
    let mut value=value.clone();
    if let Some(object)=value.as_object_mut() {
        for field in ["semanticDraft","semanticDigest","draftRevision","commandReceipts"] { object.remove(field); }
    }
    value
}

impl PendingHistory {
    fn prepare(step: CityosHistoryStep, resource: &Value) -> Result<Self,String> {
        let source=step.source();let target=step.target();
        if source["schemaVersion"]!="cityos.design-exchange.v3" || immutable_draft(source)!=immutable_draft(target) {
            return Err("This history entry requires a validated typed binding on the current immutable baseline.".into());
        }
        // Exactly one declared field may change. Historical snapshots are never
        // installed as a shortcut for unsupported structural or owner operations.
        let mut intent=None;
        for field in resource["fields"].as_array().into_iter().flatten() {
            let Some(pointer)=field["sourcePointer"].as_str() else {continue;};
            let (Some(previous),Some(next))=(source["semanticDraft"].pointer(pointer),target["semanticDraft"].pointer(pointer)) else {continue;};
            if previous==next {continue;}
            let mut candidate=source["semanticDraft"].clone();
            *candidate.pointer_mut(pointer).ok_or("History field is no longer present")?=next.clone();
            if candidate!=target["semanticDraft"] || intent.is_some() { return Err("This history operation is not supported by the current field adapter.".into()); }
            let operation=field["operation"].as_str().unwrap_or("set-property");
            if !matches!(operation,"set-property"|"set-token") || !field["nodeId"].is_string() || !field["property"].is_string() {
                return Err("This history field has no supported semantic operation.".into());
            }
            intent=Some(json!({"draftId":source["draftId"],"expectedRevision":source["draftRevision"],
                "operationId":uuid::Uuid::new_v4().to_string(),"operation":operation,
                "nodeId":field["nodeId"],"property":field["property"],"value":next}));
        }
        Ok(Self {step,command:intent.ok_or("This history entry has no supported semantic field change")?})
    }

    fn validate(&self, next: &Value) -> Result<(),String> {
        let source=self.step.source();
        let revision=source["draftRevision"].as_u64().and_then(|r|r.checked_add(1)).ok_or("Draft revision exhausted")?;
        if immutable_draft(next)!=immutable_draft(source) || next["draftRevision"].as_u64()!=Some(revision)
            || next["semanticDraft"]!=self.step.target()["semanticDraft"] {
            return Err("Obsolete or substituted history command response discarded".into());
        }
        let receipts=next["commandReceipts"].as_array().ok_or("Missing history command receipt")?;
        let receipt=receipts.last().ok_or("Missing history command receipt")?;
        let digest=receipt["requestDigest"].as_str().unwrap_or("");
        if receipt["operationId"]!=self.command["operationId"] || receipt["revision"].as_u64()!=Some(revision)
            || digest.len()!=64 || !digest.bytes().all(|byte|byte.is_ascii_hexdigit()) {
            return Err("History command receipt identity mismatch".into());
        }
        let mut expected=source["commandReceipts"].as_array().ok_or("Missing current receipt chain")?.clone();
        expected.push(receipt.clone());
        if expected.len()>256 {expected.drain(..expected.len()-256);}
        if *receipts!=expected {return Err("History command cannot replace the current receipt chain".into());}
        Ok(())
    }

    pub(super) fn install(self, app: &mut App, document: Document) -> Result<(),String> {
        self.validate(draft(&document).ok_or("Missing validated history binding")?)?;
        // History changes authored semantics and their projection, not local
        // annotations, styles, variables or the user's document metadata.
        let mut current=serde_json::to_value(&app.doc).map_err(|e|e.to_string())?;
        let projected=serde_json::to_value(document).map_err(|e|e.to_string())?;
        for field in ["nodes","pages","next_id","cityos"] {current[field]=projected[field].clone();}
        let document:Document=serde_json::from_value(current).map_err(|e|e.to_string())?;
        document.to_json().map_err(|e|e.to_string())?;
        app.hist.commit_cityos(&mut app.doc,self.step,document).map_err(|e|e.to_string())?;
        app.changed();app.sanitize();
        Ok(())
    }
}

impl Workspace {
    pub(super) fn dispatch_history_request(&mut self, app: &mut App) {
        if self.busy.is_some() {return;}
        let Some(step)=app.ui.cityos_history_request.take() else {return;};
        if self.preview_only || !self.validated || self.recovery_pending || app.cityos_submission_frozen()
            || !app.hist.cityos_step_current(&app.doc,&step) {
            self.error=Some("Bound history is unavailable until this draft is validated on its current baseline.".into());return;
        }
        let result=self.resource.as_ref().ok_or("The current field profile is unavailable".into())
            .and_then(|resource|PendingHistory::prepare(step,resource));
        match result {
            Err(error)=>self.error=Some(error),
            Ok(pending)=>{
                self.send(json!({"op":"Command","draft":pending.step.source(),"command":pending.command}));
                if self.busy.as_deref()==Some("Command") {self.history_command=Some(pending);}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;

    fn document() -> Document {
        let semantic=json!({"id":"frame","revision":"1","root":{"id":"material","props":{"elevation":"flat"}}});
        let value=json!({"schemaVersion":"cityos.design-exchange.v3","documentId":"document","draftId":"draft",
            "base":{"kind":"canonical-git","repositoryId":"repo","commit":"a".repeat(40)},
            "semanticContract":"cityos.experience-composition.v1","semanticBase":semantic,"semanticDraft":semantic,
            "draftRevision":0,"semanticDigest":"base","commandReceipts":[],"dependencyPins":[],"profileRef":"fixture-profile"});
        let mut doc=Document::new("Bound frame");doc.add_page("P");
        doc.cityos=Some(CityosBinding {schema_version:"cityos.design-exchange.v3".into(),document_id:"document".into(),draft_id:"draft".into(),
            base:CityosBase::CanonicalGit {repository_id:"repo".into(),commit:"a".repeat(40)},semantic_base:semantic,
            dependency_pins:vec![],occurrences:BTreeMap::new(),exchange_v2:None,exchange_v3:Some(value)});
        doc
    }

    fn edited_app() -> App {
        let mut app=App::new(document());app.home=None;
        let mut next=app.doc.clone();let value=next.cityos.as_mut().unwrap().exchange_mut().unwrap();
        value["semanticDraft"]["root"]["props"]["elevation"]=json!("resting");value["semanticDigest"]=json!("resting");
        value["commandReceipts"]=json!([{"operationId":"edit","requestDigest":"b".repeat(64),"revision":1}]);
        app.hist.exec(&mut app.doc,Command::CityosInstall {document:Box::new(next)}).unwrap();app
    }

    #[test]
    fn bound_undo_waits_for_trusted_validation_before_changing_document_or_history() {
        let mut app=edited_app();let document=app.doc.to_json().unwrap();let history=serde_json::to_value(&app.hist).unwrap();
        app.undo();
        assert_eq!(app.doc.to_json().unwrap(),document,"Bound undo must not install an unvalidated historical draft");
        assert_eq!(serde_json::to_value(&app.hist).unwrap(),history);
    }

    fn resource() -> Value {
        json!({"fixture":"history-test","fields":[{"nodeId":"material","property":"elevation",
            "operation":"set-property","sourcePointer":"/root/props/elevation"}]})
    }

    fn workspace() -> (Workspace,std::sync::mpsc::Receiver<Value>,std::sync::mpsc::Sender<Result<Value,String>>) {
        let (bridge,requests,responses)=super::super::bridge::Bridge::test_transport();
        let mut workspace=Workspace::default();workspace.bridge=Some(bridge);workspace.validated=true;
        workspace.themes_loaded=true;workspace.resource=Some(resource());
        (workspace,requests,responses)
    }

    // Protocol-faithful compiler fixture. Live compiler coverage is separate.
    fn reply(request: &Value) -> Value {
        let mut next=request["draft"].clone();let revision=next["draftRevision"].as_u64().unwrap()+1;
        next["semanticDraft"]["root"]["props"]["elevation"]=request["command"]["value"].clone();
        next["semanticDigest"]=json!(format!("revision-{revision}"));next["draftRevision"]=json!(revision);
        next["commandReceipts"].as_array_mut().unwrap().push(json!({"operationId":request["command"]["operationId"],"requestDigest":"c".repeat(64),"revision":revision}));
        json!({"draft":next,"resource":resource(),"projection":{"schemaVersion":"cityos.desigma-canvas.v1",
            "draftId":next["draftId"],"draftRevision":next["draftRevision"],"semanticDigest":next["semanticDigest"],"nodes":[]}})
    }

    #[test]
    fn undo_redo_are_fresh_commands_and_keep_the_cumulative_receipt_chain_and_stacks() {
        let mut app=edited_app();let (mut workspace,requests,responses)=workspace();
        app.doc.name="My annotated draft".into();
        app.undo();workspace.poll(&mut app);let undo=requests.try_recv().unwrap();
        assert_eq!(undo["op"],"Command");assert_eq!(undo["command"]["expectedRevision"],1);
        assert_eq!(undo["command"]["value"],"flat");assert_ne!(undo["command"]["operationId"],"edit");
        assert_eq!(draft(&app.doc).unwrap()["draftRevision"],1);
        responses.send(Ok(reply(&undo))).unwrap();workspace.poll(&mut app);
        let undone=draft(&app.doc).unwrap();assert_eq!(undone["draftRevision"],2);
        assert_eq!(undone["semanticDraft"]["root"]["props"]["elevation"],"flat");
        assert_eq!(undone["commandReceipts"].as_array().unwrap().len(),2);
        assert!(!app.hist.can_undo());assert!(app.hist.can_redo());
        let before=app.doc.to_json().unwrap();app.redo();assert_eq!(app.doc.to_json().unwrap(),before);
        workspace.poll(&mut app);let redo=requests.try_recv().unwrap();
        assert_eq!(redo["command"]["expectedRevision"],2);assert_eq!(redo["command"]["value"],"resting");
        assert_ne!(redo["command"]["operationId"],undo["command"]["operationId"]);
        responses.send(Ok(reply(&redo))).unwrap();workspace.poll(&mut app);
        let redone=draft(&app.doc).unwrap();assert_eq!(redone["draftRevision"],3);
        assert_eq!(redone["commandReceipts"].as_array().unwrap().len(),3);
        assert_eq!(redone["commandReceipts"][0]["operationId"],"edit");
        assert_eq!(app.doc.name,"My annotated draft");
        assert!(app.hist.can_undo());assert!(!app.hist.can_redo());
    }

    #[test]
    fn rejected_history_keeps_document_and_history_and_a_retry_gets_a_new_identity() {
        let mut app=edited_app();let (mut workspace,requests,responses)=workspace();
        let document=app.doc.to_json().unwrap();let history=serde_json::to_value(&app.hist).unwrap();
        app.undo();workspace.poll(&mut app);let first=requests.try_recv().unwrap();
        responses.send(Err("DESIGN_FIELD_UNSUPPORTED".into())).unwrap();workspace.poll(&mut app);
        assert_eq!(app.doc.to_json().unwrap(),document);assert_eq!(serde_json::to_value(&app.hist).unwrap(),history);
        app.undo();workspace.poll(&mut app);let retry=requests.try_recv().unwrap();
        assert_eq!(retry["draft"],first["draft"]);assert_ne!(retry["command"]["operationId"],first["command"]["operationId"]);
    }

    #[test]
    fn sealed_baselines_and_changed_revisions_fence_delayed_history_results() {
        for seal in [false,true] {
            let mut app=edited_app();let (mut workspace,requests,responses)=workspace();
            app.undo();workspace.poll(&mut app);let request=requests.try_recv().unwrap();
            if seal {app.hist.seal();} else {
                let mut newer=app.doc.clone();newer.cityos.as_mut().unwrap().exchange_mut().unwrap()["semanticDigest"]=json!("newer");
                app.hist.exec(&mut app.doc,Command::CityosInstall {document:Box::new(newer)}).unwrap();
            }
            let document=app.doc.to_json().unwrap();let history=serde_json::to_value(&app.hist).unwrap();
            responses.send(Ok(reply(&request))).unwrap();workspace.poll(&mut app);
            assert!(workspace.error.is_some());assert_eq!(app.doc.to_json().unwrap(),document);
            assert_eq!(serde_json::to_value(&app.hist).unwrap(),history);
        }
    }

    #[test]
    fn malformed_receipts_cannot_replace_the_live_chain_or_install_unrequested_semantics() {
        for fault in ["identity","chain","semantics","base"] {
            let mut app=edited_app();let (mut workspace,requests,responses)=workspace();
            app.undo();workspace.poll(&mut app);let request=requests.try_recv().unwrap();let mut result=reply(&request);
            match fault {
                "identity"=>{result["draft"]["commandReceipts"][1]["operationId"]=json!("wrong-command");},
                "chain"=>{result["draft"]["commandReceipts"].as_array_mut().unwrap().remove(0);},
                "semantics"=>{result["draft"]["semanticDraft"]["root"]["props"]["elevation"]=json!("floating");},
                _=>{result["draft"]["profileRef"]=json!("different-profile");},
            }
            let document=app.doc.to_json().unwrap();let history=serde_json::to_value(&app.hist).unwrap();
            responses.send(Ok(result)).unwrap();workspace.poll(&mut app);
            assert!(workspace.error.is_some(),"{fault}");assert_eq!(app.doc.to_json().unwrap(),document,"{fault}");
            assert_eq!(serde_json::to_value(&app.hist).unwrap(),history,"{fault}");
        }
    }

    #[test]
    fn unsupported_history_and_frozen_or_unvalidated_drafts_send_no_command() {
        for mode in ["structural","frozen","unvalidated"] {
            let mut app=edited_app();let (mut workspace,requests,_responses)=workspace();
            if mode=="structural" {app.doc.cityos.as_mut().unwrap().exchange_mut().unwrap()["semanticDraft"]["root"]["other"]=json!(true);}
            if mode=="frozen" {app.ui.cityos_frozen_draft=Some("draft".into());}
            if mode=="unvalidated" {workspace.validated=false;}
            let document=app.doc.to_json().unwrap();let history=serde_json::to_value(&app.hist).unwrap();
            app.undo();workspace.poll(&mut app);assert!(requests.try_recv().is_err(),"{mode}");
            assert_eq!(app.doc.to_json().unwrap(),document);assert_eq!(serde_json::to_value(&app.hist).unwrap(),history);
        }
    }
}
