//! Bound undo/redo is semantic intent, validated by the same compiler as an edit.

use crate::draft::draft;
use crate::workspace::Workspace;
use crate::editor::EditorHost;
use desigma_doc::{CityosHistoryStep, Document};
use serde_json::{json, Value};

pub struct PendingHistory {
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

    pub fn install<H:EditorHost>(self, app:&mut H, document:Document)->Result<(),String> {
        self.validate(draft(&document).ok_or("Missing validated history binding")?)?;
        app.install_history(self.step,document)
    }

}

impl<H:EditorHost> Workspace<H> {
    pub fn dispatch_history_request(&mut self, app: &mut H) {
        if self.busy.is_some() {return;}
        let Some(step)=app.take_history_request() else {return;};
        if self.preview_only || !self.validated || self.recovery_pending || app.status().frozen
            || !app.history_step_current(&step) {
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

