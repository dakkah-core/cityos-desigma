//! Native document and history checkpoints. These carry local state, never owner authority.
use crate::{Document, History};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub(crate) mod native_box {
    use crate::Document;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(document: &Document, serializer: S) -> Result<S::Ok, S::Error> {
        let text = document.to_json().map_err(serde::ser::Error::custom)?;
        let value: serde_json::Value = serde_json::from_str(&text).map_err(serde::ser::Error::custom)?;
        value.serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Box<Document>, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        Document::from_json(&value.to_string()).map(Box::new).map_err(serde::de::Error::custom)
    }
}

/// Inactive history retained across an accepted/incoming owner baseline.
/// It remains available for inspection/export, but is never an active undo stack.
#[derive(Clone, Debug)]
pub struct RetainedHistory {
    pub document: Document,
    pub history: History,
}

#[derive(Clone, Debug)]
pub struct DocumentCheckpoint {
    pub document: Document,
    pub history: History,
    pub retained: Vec<RetainedHistory>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StateWire {
    document: serde_json::Value,
    history: History,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckpointWire {
    #[serde(rename = "schemaVersion")]
    schema_version: String,
    current: StateWire,
    retained: Vec<StateWire>,
}

fn state_to_wire(document: &Document, history: &History) -> serde_json::Result<StateWire> {
    for retained in history.retained_documents() {
        retained.to_json()?; // Validate semantic bindings inside both history stacks.
    }
    Ok(StateWire { document: serde_json::from_str(&document.to_json()?)?, history: history.clone() })
}

fn state_from_wire(state: StateWire) -> serde_json::Result<RetainedHistory> {
    for document in state.history.retained_documents() {
        document.to_json()?;
    }
    Ok(RetainedHistory { document: Document::from_json(&state.document.to_string())?, history: state.history })
}

impl DocumentCheckpoint {
    pub fn new(document: Document, history: History) -> Self {
        Self { document, history, retained: Vec::new() }
    }

    /// The caller must validate/authorize the incoming baseline before using this
    /// local hook. Persist the candidate checkpoint before installing it in the UI.
    pub fn install_baseline(&mut self, document: Document) {
        self.retained.push(RetainedHistory {
            document: std::mem::replace(&mut self.document, document),
            history: self.history.seal(),
        });
    }

    /// All payload roots, including documents reachable only from undo/redo or
    /// inactive baseline history. No garbage-collection authority is implied.
    pub fn dependency_pins(&self) -> Vec<String> {
        let mut roots = BTreeSet::new();
        let states = std::iter::once((&self.document, &self.history))
            .chain(self.retained.iter().map(|s| (&s.document, &s.history)));
        for (document, history) in states {
            for document in std::iter::once(document).chain(history.retained_documents()) {
                if let Some(binding) = &document.cityos {
                    roots.extend(binding.dependency_pins.iter().cloned());
                    // Expanded exchanges carry their own explicit pins. Retain
                    // both projections; a stale native wrapper must not drop a
                    // payload required by the versioned semantic exchange.
                    if let Some(pins) = binding.exchange().and_then(|value| value["dependencyPins"].as_array()) {
                        roots.extend(pins.iter().filter_map(|pin| pin["digest"].as_str()).map(str::to_owned));
                    }
                }
            }
        }
        roots.into_iter().collect()
    }

    pub fn to_json(&self) -> serde_json::Result<String> {
        let retained = self.retained.iter().map(|s| state_to_wire(&s.document, &s.history)).collect::<serde_json::Result<_>>()?;
        serde_json::to_string(&CheckpointWire {
            schema_version: "desigma.checkpoint.v1".into(),
            current: state_to_wire(&self.document, &self.history)?,
            retained,
        })
    }

    pub fn from_json(text: &str) -> serde_json::Result<Self> {
        let wire: CheckpointWire = serde_json::from_str(text)?;
        if wire.schema_version != "desigma.checkpoint.v1" {
            return Err(<serde_json::Error as serde::de::Error>::custom("Unsupported history checkpoint version"));
        }
        let current = state_from_wire(wire.current)?;
        let retained = wire.retained.into_iter().map(state_from_wire).collect::<serde_json::Result<_>>()?;
        Ok(Self { document: current.document, history: current.history, retained })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Command, Node, Rect};

    #[test]
    fn typed_history_roundtrip_preserves_redo_only_dependencies_and_revision_progress() {
        use crate::{CityosBase, CityosBinding};
        fn document(hash: &str) -> Document {
            let mut document = Document::new(hash);
            let semantic = serde_json::json!({"root":{"props":{"elevation":"flat"}}});
            document.cityos = Some(CityosBinding {
                schema_version: "cityos.design-exchange.v3".into(), document_id: "d".into(), draft_id: "draft".into(),
                base: CityosBase::CanonicalGit { repository_id: "repo".into(), commit: "a".repeat(40) },
                semantic_base: semantic.clone(), dependency_pins: vec![hash.into()], occurrences: Default::default(), exchange_v2: None,
                exchange_v3: Some(serde_json::json!({
                    "schemaVersion":"cityos.design-exchange.v3", "semanticContract":"fixture", "documentId":"d", "draftId":"draft", "draftRevision":0,
                    "base":{"kind":"canonical-git", "repositoryId":"repo", "commit":"a".repeat(40)},
                    "semanticBase":semantic, "semanticDraft":semantic, "commandReceipts":[], "dependencyPins":[{"digest":hash}]
                })),
            });
            document
        }
        let first_hash = "b".repeat(64); let second_hash = "c".repeat(64);
        let mut current = document(&first_hash);
        let mut history = History::default();
        history.exec(&mut current, Command::CityosInstall { document: Box::new(document(&second_hash)) }).unwrap();
        history.undo(&mut current).unwrap();
        let checkpoint = DocumentCheckpoint::new(current, history);
        assert_eq!(checkpoint.dependency_pins(), [first_hash, second_hash.clone()]);
        let mut reopened = DocumentCheckpoint::from_json(&checkpoint.to_json().unwrap()).unwrap();
        reopened.history.redo(&mut reopened.document).unwrap();
        assert_eq!(reopened.document.name, second_hash);
        assert_eq!(reopened.document.cityos.as_ref().unwrap().exchange().unwrap()["draftRevision"], 3);
        let mut future: serde_json::Value = serde_json::from_str(&checkpoint.to_json().unwrap()).unwrap();
        future["current"]["history"]["redo"][0]["CityosInstall"]["document"]["schemaVersion"] = serde_json::json!("desigma.document.v999");
        assert!(DocumentCheckpoint::from_json(&future.to_string()).is_err());
    }

    #[test]
    fn checkpoint_restarts_with_both_history_stacks_and_embedded_images() {
        let mut document = Document::new("checkpoint");
        let page = document.add_page("P");
        document.images.insert("image".into(), vec![0, 1, 255]);
        let id = document.alloc_id();
        document.insert(page, Node::rectangle(id, Rect::new(0., 0., 1., 1.))).unwrap();
        let mut history = History::default();
        for name in ["first", "second"] {
            let mut node = document.get(id).unwrap().clone();
            node.name = name.into();
            history.exec(&mut document, Command::Update { node: Box::new(node) }).unwrap();
        }
        history.undo(&mut document).unwrap();
        let checkpoint = DocumentCheckpoint::new(document, history);
        let mut back = DocumentCheckpoint::from_json(&checkpoint.to_json().unwrap()).unwrap();
        assert!(back.history.can_undo() && back.history.can_redo());
        assert_eq!(back.document.images["image"], [0, 1, 255]);
        back.history.redo(&mut back.document).unwrap();
        assert_eq!(back.document.get(id).unwrap().name, "second");
        back.history.undo(&mut back.document).unwrap();
        back.history.undo(&mut back.document).unwrap();
        assert_eq!(back.document.get(id).unwrap().name, "Rectangle");
    }

    #[test]
    fn baseline_retains_history_but_cannot_undo_across_it_after_restart() {
        let mut document = Document::new("local");
        let mut history = History::default();
        history.exec(&mut document, Command::SetComments { comments: vec![] }).unwrap();
        let mut checkpoint = DocumentCheckpoint::new(document, history);
        checkpoint.install_baseline(Document::new("incoming"));
        let mut back = DocumentCheckpoint::from_json(&checkpoint.to_json().unwrap()).unwrap();
        assert!(!back.history.undo(&mut back.document).unwrap());
        assert!(!back.history.can_redo());
        assert_eq!(back.document.name, "incoming");
        assert_eq!(back.retained[0].document.name, "local");
        assert!(back.retained[0].history.can_undo());
    }

    #[test]
    fn checkpoint_rejects_future_schema_and_invalid_nested_binding() {
        let checkpoint = DocumentCheckpoint::new(Document::new("t"), History::default());
        let text = checkpoint.to_json().unwrap();
        assert!(DocumentCheckpoint::from_json(&text.replace("desigma.checkpoint.v1", "desigma.checkpoint.v99")).is_err());
        let mut wire: serde_json::Value = serde_json::from_str(&text).unwrap();
        wire["current"]["history"]["undo"] = serde_json::json!([{"CityosInstall":{"document":{
            "name":"bad", "nodes":{}, "pages":[], "styles":{}, "images":{}, "next_id":0,
            "cityos":{"schema_version":"unknown", "document_id":"d", "draft_id":"d",
                "base":{"kind":"native-owner", "owner":"o", "resource":"r", "revision":"1"},
                "semantic_base":{}, "dependency_pins":[], "occurrences":{}}
        }}}]);
        assert!(DocumentCheckpoint::from_json(&wire.to_string()).is_err());
    }
}
