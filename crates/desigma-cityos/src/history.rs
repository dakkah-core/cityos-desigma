//! CityOS semantic history policy over the editor's generic stack transaction.
use desigma_doc::{Command, DocError, Document, History, HistoryTransition};
use serde_json::Value;

/// A prepared semantic intent is memory-only and cannot confer owner authority.
#[derive(Clone, Debug)]
pub struct CityosHistoryStep {
    transition: HistoryTransition,
    source: Value,
    source_binding: Value,
}

impl CityosHistoryStep {
    pub fn source(&self) -> &Value { &self.source }
    pub fn target(&self) -> &Value {
        match self.transition.command() {
            Command::CityosInstall { document } => document.cityos.as_ref().unwrap().exchange().unwrap(),
            _ => unreachable!("only validated bound snapshots can prepare semantic history"),
        }
    }
}

pub trait CityosHistory {
    fn prepare_cityos(&self, document: &Document, redo: bool) -> Result<Option<CityosHistoryStep>, DocError>;
    fn cityos_step_current(&self, document: &Document, step: &CityosHistoryStep) -> bool;
    fn commit_cityos(&mut self, document: &mut Document, step: CityosHistoryStep, validated: Document) -> Result<(), DocError>;
}

impl CityosHistory for History {
    fn prepare_cityos(&self, doc: &Document, redo: bool) -> Result<Option<CityosHistoryStep>, DocError> {
        let Some(transition) = self.prepare_transition(redo) else { return Ok(None) };
        let Command::CityosInstall { document } = transition.command() else { return Err(DocError::CityosProtected) };
        let source = doc.cityos.as_ref().ok_or(DocError::CityosProtected)?;
        let target = document.cityos.as_ref().ok_or(DocError::CityosProtected)?;
        if source.validate().is_err() || target.validate().is_err() || source.base != target.base
            || source.semantic_base != target.semantic_base || source.draft_id != target.draft_id
            || source.document_id != target.document_id {
            return Err(DocError::CityosProtected);
        }
        let source_binding = serde_json::to_value(source).map_err(|_| DocError::CityosProtected)?;
        let source = source.exchange().ok_or(DocError::CityosProtected)?.clone();
        target.exchange().ok_or(DocError::CityosProtected)?;
        Ok(Some(CityosHistoryStep { transition, source, source_binding }))
    }

    fn cityos_step_current(&self, doc: &Document, step: &CityosHistoryStep) -> bool {
        self.transition_current(&step.transition)
            && doc.cityos.as_ref().and_then(|binding| serde_json::to_value(binding).ok()).as_ref() == Some(&step.source_binding)
    }

    fn commit_cityos(&mut self, doc: &mut Document, step: CityosHistoryStep, validated: Document) -> Result<(), DocError> {
        if !self.cityos_step_current(doc, &step) { return Err(DocError::CityosProtected) }
        let next = validated.cityos.as_ref().and_then(|binding| binding.exchange()).ok_or(DocError::CityosProtected)?;
        let revision = step.source["draftRevision"].as_u64().and_then(|r| r.checked_add(1)).ok_or(DocError::CityosProtected)?;
        if next["draftRevision"].as_u64() != Some(revision) || next["semanticDraft"] != step.target()["semanticDraft"] {
            return Err(DocError::CityosProtected);
        }
        self.commit_transition(doc, step.transition, Command::CityosInstall { document: Box::new(validated) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use desigma_doc::{CityosBase, CityosBinding};
    use serde_json::json;
    use std::collections::BTreeMap;

    fn fixture() -> (Document, History) {
        let mut document = Document::new("bound");
        document.add_page("P");
        let base = json!({"id":"frame","root":{"props":{"elevation":"flat"}}});
        document.cityos = Some(CityosBinding {
            schema_version: "cityos.design-exchange.v2".into(), document_id: "document".into(), draft_id: "draft".into(),
            base: CityosBase::CanonicalGit { repository_id: "repo".into(), commit: "a".repeat(40) },
            semantic_base: base.clone(), dependency_pins: vec![], occurrences: BTreeMap::new(), exchange_v3: None,
            exchange_v2: Some(json!({"schemaVersion":"cityos.design-exchange.v2","documentId":"document","draftId":"draft",
                "base":{"kind":"canonical-git","repositoryId":"repo","commit":"a".repeat(40)},
                "semanticBase":base,"semanticDraft":base,"draftRevision":0,"semanticDigest":"base"})),
        });
        let mut history = History::default();
        let mut edited = document.clone();
        let payload = edited.cityos.as_mut().unwrap().exchange_mut().unwrap();
        payload["semanticDraft"]["root"]["props"]["elevation"] = json!("resting");
        payload["semanticDigest"] = json!("edited");
        history.exec(&mut document, Command::CityosInstall { document: Box::new(edited) }).unwrap();
        (document, history)
    }

    fn validated(doc: &Document, step: &CityosHistoryStep) -> Document {
        let mut next = doc.clone();
        let payload = next.cityos.as_mut().unwrap().exchange_mut().unwrap();
        payload["semanticDraft"] = step.target()["semanticDraft"].clone();
        payload["draftRevision"] = json!(step.source()["draftRevision"].as_u64().unwrap() + 1);
        payload["semanticDigest"] = json!("validated");
        next
    }

    #[test]
    fn prepared_history_commits_a_new_revision_and_retains_redo() {
        let (mut doc, mut history) = fixture();
        let step = history.prepare_cityos(&doc, false).unwrap().unwrap();
        let next = validated(&doc, &step);
        history.commit_cityos(&mut doc, step, next).unwrap();
        assert_eq!(doc.cityos.as_ref().unwrap().exchange().unwrap()["draftRevision"], 2);
        assert!(!history.can_undo());
        assert!(history.can_redo());
        let redo = history.prepare_cityos(&doc, true).unwrap().unwrap();
        let next = validated(&doc, &redo);
        history.commit_cityos(&mut doc, redo, next).unwrap();
        assert_eq!(doc.cityos.as_ref().unwrap().exchange().unwrap()["semanticDraft"]["root"]["props"]["elevation"], "resting");
        assert_eq!(doc.cityos.as_ref().unwrap().exchange().unwrap()["draftRevision"], 3);
    }

    #[test]
    fn changed_dependency_binding_fences_a_prepared_history_response() {
        let (mut doc, mut history) = fixture();
        let step = history.prepare_cityos(&doc, false).unwrap().unwrap();
        let next = validated(&doc, &step);
        doc.cityos.as_mut().unwrap().dependency_pins.push("b".repeat(64));
        let before = doc.to_json().unwrap();
        assert!(!history.cityos_step_current(&doc, &step));
        assert!(history.commit_cityos(&mut doc, step, next).is_err());
        assert_eq!(doc.to_json().unwrap(), before);
        assert!(history.can_undo());
    }

    #[test]
    fn rejected_semantic_response_preserves_document_and_history() {
        let (mut doc, mut history) = fixture();
        let step = history.prepare_cityos(&doc, false).unwrap().unwrap();
        let mut next = validated(&doc, &step);
        next.cityos.as_mut().unwrap().exchange_mut().unwrap()["semanticDraft"]["unrelated"] = json!(true);
        let before = doc.to_json().unwrap();
        assert!(history.commit_cityos(&mut doc, step, next).is_err());
        assert_eq!(doc.to_json().unwrap(), before);
        assert!(history.can_undo());
        assert!(!history.can_redo());
    }
}
