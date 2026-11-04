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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exchange_v3: Option<Value>,
}
impl CityosBinding {
    pub fn exchange(&self) -> Option<&Value> {
        self.exchange_v3.as_ref().or(self.exchange_v2.as_ref())
    }
    pub fn exchange_mut(&mut self) -> Option<&mut Value> {
        if self.exchange_v3.is_some() {
            self.exchange_v3.as_mut()
        } else {
            self.exchange_v2.as_mut()
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        if !["cityos.design-exchange.v1", "cityos.design-exchange.v2", "cityos.design-exchange.v3"].contains(&self.schema_version.as_str())
            || self.document_id.is_empty()
            || self.draft_id.is_empty()
        {
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
        if self.schema_version == "cityos.design-exchange.v2" || self.schema_version == "cityos.design-exchange.v3" {
            let v3 = self.schema_version == "cityos.design-exchange.v3";
            if (v3 && (self.exchange_v3.is_none() || self.exchange_v2.is_some())) || (!v3 && (self.exchange_v2.is_none() || self.exchange_v3.is_some())) {
                return Err("Exchange payload version mismatch".into());
            }
            let draft = self.exchange().ok_or("Missing exchange payload")?;
            let CityosBase::CanonicalGit { repository_id, commit } = &self.base else { return Err("Native v2 drafts are unqualified".into()) };
            if draft["schemaVersion"] != self.schema_version
                || draft["draftId"] != self.draft_id
                || draft["documentId"] != self.document_id
                || draft["base"]["kind"] != "canonical-git"
                || draft["base"]["commit"] != *commit
                || draft["base"]["repositoryId"] != *repository_id
                || draft["semanticBase"] != self.semantic_base
                || draft["draftRevision"].as_u64().is_none()
                || !draft["semanticDraft"].is_object()
            {
                return Err("Inconsistent v2 binding; trusted validation required before editing".into());
            }
            if v3 && (!draft["semanticContract"].is_string() || !draft["commandReceipts"].is_array()) {
                return Err("Incomplete typed exchange payload".into());
            }
        } else if self.exchange_v2.is_some() || self.exchange_v3.is_some() {
            return Err("Expanded payload cannot be stored in a legacy binding".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn binding(version: u8) -> CityosBinding {
        let schema = format!("cityos.design-exchange.v{version}");
        let base = json!({"root": {"opacity": 0.123456789}});
        let mut exchange = json!({
            "schemaVersion": schema, "documentId": "document", "draftId": "draft",
            "draftRevision": 0, "base": {"kind": "canonical-git", "repositoryId": "repo", "commit": "a".repeat(40)},
            "semanticBase": base.clone(), "semanticDraft": {"root": {}},
        });
        if version == 3 {
            exchange["semanticContract"] = json!("cityos.experience-composition.v1");
            exchange["commandReceipts"] = json!([]);
        }
        CityosBinding {
            schema_version: format!("cityos.design-exchange.v{version}"),
            document_id: "document".into(), draft_id: "draft".into(),
            base: CityosBase::CanonicalGit { repository_id: "repo".into(), commit: "a".repeat(40) },
            semantic_base: base, dependency_pins: vec!["pin".into()],
            occurrences: BTreeMap::from([("node".into(), CityosOccurrence {
                definition_id: "definition".into(), source_node_id: "source".into(), instance_id: None,
                occurrence_id: "occurrence".into(), view_id: "view".into(),
            })]),
            exchange_v2: (version == 2).then(|| exchange.clone()),
            exchange_v3: (version == 3).then_some(exchange),
        }
    }

    #[test]
    fn bindings_preserve_wire_shape_and_validate_across_versions() {
        for version in 1..=3 {
            let original = binding(version);
            original.validate().unwrap();
            let wire = serde_json::to_value(&original).unwrap();
            assert_eq!(wire["base"], json!({"kind": "canonical-git", "repository_id": "repo", "commit": "a".repeat(40)}));
            assert_eq!(wire["occurrences"]["node"]["instance_id"], Value::Null);
            assert_eq!(wire.get("exchange_v2").is_some(), version == 2);
            assert_eq!(wire.get("exchange_v3").is_some(), version == 3);
            let restored: CityosBinding = serde_json::from_value(wire.clone()).unwrap();
            restored.validate().unwrap();
            assert_eq!(serde_json::to_value(restored).unwrap(), wire);
        }
    }

    #[test]
    fn rejects_unpinned_bases_payload_mismatches_and_incomplete_typed_exchange() {
        let mut value = binding(1);
        value.base = CityosBase::CanonicalGit { repository_id: "repo".into(), commit: "not-a-commit".into() };
        assert!(value.validate().is_err());
        value.base = CityosBase::NativeOwner { owner: "owner".into(), resource: "resource".into(), revision: "1".into() };
        assert!(value.validate().is_ok());
        value.exchange_v2 = Some(json!({}));
        assert!(value.validate().is_err());
        assert!(serde_json::from_value::<CityosBase>(json!({"kind": "local-overlay"})).is_err());

        let mut value = binding(2);
        value.exchange_mut().unwrap()["documentId"] = json!("other");
        assert!(value.validate().is_err());
        let mut value = binding(2);
        value.base = CityosBase::NativeOwner { owner: "owner".into(), resource: "resource".into(), revision: "1".into() };
        assert!(value.validate().is_err());
        let mut value = binding(3);
        value.exchange_mut().unwrap()["commandReceipts"] = Value::Null;
        assert!(value.validate().is_err());
        let mut value = binding(3);
        value.exchange_v2 = Some(json!({}));
        assert!(value.validate().is_err());
    }
}

