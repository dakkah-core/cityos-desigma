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
}
impl CityosBinding {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != "cityos.design-exchange.v1" || self.document_id.is_empty() || self.draft_id.is_empty() {
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
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::*;
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
        });
        let reopened = Document::from_json(&doc.to_json().unwrap()).unwrap();
        assert_eq!(reopened.cityos.unwrap().semantic_base, base);
        let container = doc.to_json().unwrap();
        assert!(serde_json::from_str::<Document>(&container).is_err(), "legacy flat readers cannot silently strip bindings");
    }
}
