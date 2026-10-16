use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum CityosBase {
    CanonicalGit { repository_id: String, commit: String },
    NativeOwner { owner: String, resource: String, revision: String },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CityosOccurrence {
    pub definition_id: String,
    pub source_node_id: String,
    pub instance_id: Option<String>,
    pub occurrence_id: String,
    pub view_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CityosBinding {
    pub schema_version: String,
    pub document_id: String,
    pub draft_id: String,
    pub base: CityosBase,
    pub semantic_base: Value,
    pub dependency_pins: Vec<String>,
    pub occurrences: BTreeMap<String, CityosOccurrence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exchange_v2: Option<Value>,
}
impl CityosBinding {
    pub fn validate(&self) -> Result<(), String> {
        if !["cityos.design-exchange.v1", "cityos.design-exchange.v2"].contains(&self.schema_version.as_str()) || self.document_id.is_empty() || self.draft_id.is_empty() {
            return Err("Unsupported or incomplete CityOS binding".into());
        }
        match &self.base {
            CityosBase::CanonicalGit { repository_id, commit }
                if repository_id.is_empty() || commit.len() != 40 || !commit.bytes().all(|b| b.is_ascii_hexdigit()) =>
            {
                return Err("Git drafts require a pinned canonical commit".into())
            }
            CityosBase::NativeOwner { owner, resource, revision } if owner.is_empty() || resource.is_empty() || revision.is_empty() => {
                return Err("Native drafts require an authorized owner revision".into())
            }
            _ => {}
        }
        if self.schema_version == "cityos.design-exchange.v2" {
            let draft = self.exchange_v2.as_ref().ok_or("Missing v2 exchange payload")?;
            let CityosBase::CanonicalGit { repository_id, commit } = &self.base else { return Err("Native v2 drafts are unqualified".into()) };
            if draft["schemaVersion"] != self.schema_version || draft["draftId"] != self.draft_id || draft["documentId"] != self.document_id ||
                draft["base"]["kind"] != "canonical-git" || draft["base"]["commit"] != *commit || draft["base"]["repositoryId"] != *repository_id ||
                draft["semanticBase"] != self.semantic_base || draft["draftRevision"].as_u64().is_none() || !draft["semanticDraft"].is_object() {
                return Err("Inconsistent v2 binding; trusted validation required before editing".into());
            }
        } else if self.exchange_v2.is_some() { return Err("V2 payload cannot be stored in a legacy binding".into()); }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::*;
    fn v2_document() -> Document {
        let mut doc=Document::new("bound v2");doc.add_page("P");
        let base=serde_json::json!({"id":"frame","revision":"1","root":{"props":{"elevation":"flat"}}});
        doc.cityos=Some(CityosBinding{schema_version:"cityos.design-exchange.v2".into(),document_id:"document".into(),draft_id:"draft".into(),
            base:CityosBase::CanonicalGit{repository_id:"repo".into(),commit:"a".repeat(40)},semantic_base:base.clone(),dependency_pins:vec![],occurrences:BTreeMap::new(),
            exchange_v2:Some(serde_json::json!({"schemaVersion":"cityos.design-exchange.v2","documentId":"document","draftId":"draft","draftRevision":0,
                "base":{"kind":"canonical-git","repositoryId":"repo","commit":"a".repeat(40)},"semanticBase":base,"semanticDraft":base,"semanticDigest":"base-digest"}))});doc
    }
    #[test] fn expanded_bindings_use_new_container_and_reject_legacy_relabeling(){
        let doc=v2_document();let saved=doc.to_json().unwrap();assert!(saved.contains("desigma.document.v3"));
        let reopened=Document::from_json(&saved).unwrap();assert_eq!(reopened.cityos.unwrap().exchange_v2,doc.cityos.unwrap().exchange_v2);
        assert!(Document::from_json(&saved.replace("desigma.document.v3","desigma.document.v2")).is_err());
    }
    #[test] fn bound_projection_commands_and_history_install_together_without_revision_reuse(){
        let mut doc=v2_document();let snapshot=doc.to_json().unwrap();let mut history=History::default();
        assert!(history.exec(&mut doc,Command::SetComments{comments:vec![]}).is_err());assert_eq!(doc.to_json().unwrap(),snapshot);
        let mut next=doc.clone();let payload=next.cityos.as_mut().unwrap().exchange_v2.as_mut().unwrap();
        payload["semanticDraft"]["root"]["props"]["elevation"]=serde_json::json!("resting");payload["semanticDigest"]=serde_json::json!("resting-digest");
        next.name="Changed projection".into();history.exec(&mut doc,Command::CityosInstall{document:Box::new(next)}).unwrap();
        assert_eq!(doc.cityos.as_ref().unwrap().exchange_v2.as_ref().unwrap()["draftRevision"],1);
        history.undo(&mut doc).unwrap();assert_eq!(doc.name,"bound v2");assert_eq!(doc.cityos.as_ref().unwrap().exchange_v2.as_ref().unwrap()["draftRevision"],2);
        history.redo(&mut doc).unwrap();assert_eq!(doc.name,"Changed projection");assert_eq!(doc.cityos.as_ref().unwrap().exchange_v2.as_ref().unwrap()["draftRevision"],3);
        let before=doc.to_json().unwrap();let mut wrong=doc.clone();wrong.cityos.as_mut().unwrap().semantic_base=serde_json::json!({});
        assert!(history.exec(&mut doc,Command::CityosInstall{document:Box::new(wrong)}).is_err());assert_eq!(doc.to_json().unwrap(),before);
    }
    #[test]
    fn failed_batch_and_failed_undo_leave_document_and_history_intact() {
        let mut doc = Document::new("test");
        let page = doc.add_page("P");
        let mut history = History::default();
        let before = doc.to_json().unwrap();
        assert!(history
            .exec(
                &mut doc,
                Command::Batch(vec![
                    Command::SetFlows { flows: vec![Flow { name: "changed".into(), page, start: page, description: String::new() }] },
                    Command::Remove { id: NodeId(999) }
                ])
            )
            .is_err());
        assert_eq!(doc.to_json().unwrap(), before);
        assert!(!history.can_undo());
        let id = doc.alloc_id();
        doc.insert(page, Node::rectangle(id, Rect::new(0., 0., 10., 10.))).unwrap();
        let node = doc.get(id).unwrap().clone();
        history.exec(&mut doc, Command::Update { node: Box::new(node) }).unwrap();
        doc.remove(id).unwrap();
        let snapshot = doc.to_json().unwrap();
        assert!(history.undo(&mut doc).is_err());
        assert!(history.can_undo());
        assert_eq!(doc.to_json().unwrap(), snapshot);
    }
    #[test]
    fn legacy_documents_read_but_unknown_containers_and_overlay_bases_are_rejected() {
        let doc = Document::new("legacy");
        let legacy = serde_json::to_string(&doc).unwrap();
        assert!(Document::from_json(&legacy).is_ok());
        let current = doc.to_json().unwrap();
        assert!(Document::from_json(&current.replace("desigma.document.v1", "desigma.document.v999")).is_err());
        assert!(serde_json::from_value::<CityosBase>(serde_json::json!({"kind":"local-overlay","commit":"abc"})).is_err());
    }
    #[test]
    fn bound_semantic_values_survive_native_save_and_reopen_unchanged() {
        let mut doc = Document::new("bound");
        let base =
            serde_json::json!({"schemaVersion":"cityos.experience-composition.v1","root":{"props":{"elevation":"flat","opacity":0.123456789},"slots":{}}});
        doc.cityos = Some(CityosBinding {
            schema_version: "cityos.design-exchange.v1".into(),
            document_id: "document-1".into(),
            draft_id: "draft-1".into(),
            base: CityosBase::CanonicalGit { repository_id: "repo".into(), commit: "a".repeat(40) },
            semantic_base: base.clone(),
            dependency_pins: vec!["b".repeat(64)],
            occurrences: BTreeMap::new(),
            exchange_v2: None,
        });
        let reopened = Document::from_json(&doc.to_json().unwrap()).unwrap();
        assert_eq!(reopened.cityos.unwrap().semantic_base, base);
        let container = doc.to_json().unwrap();
        assert!(serde_json::from_str::<Document>(&container).is_err(), "legacy flat readers cannot silently strip bindings");
    }
}
