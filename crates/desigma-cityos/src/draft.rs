//! Semantic bound-document construction; no editor or owner writer.
use desigma_doc::{CityosBase, CityosBinding, Document};
use serde_json::Value;
use crate::projection::projection_document;

pub fn binding(draft: Value, projection: &Value) -> Result<Document, String> {
    if !matches!(draft["schemaVersion"].as_str(),Some("cityos.design-exchange.v2" | "cityos.design-exchange.v3"))
        || !matches!(projection["schemaVersion"].as_str(), Some("cityos.desigma-canvas.v1" | "cityos.desigma-structure.v1"))
        || projection["draftId"] != draft["draftId"] || projection["draftRevision"] != draft["draftRevision"] || projection["semanticDigest"] != draft["semanticDigest"] {
        return Err("Projection/draft identity mismatch".into());
    }
    let string = |v: &Value, key: &str| v[key].as_str().map(str::to_owned).ok_or_else(|| format!("Missing {key}"));
    let (mut doc,occurrences)=projection_document(projection,draft["semanticDraft"]["id"].as_str().unwrap_or("CityOS design"))?;
    let pins = draft["dependencyPins"].as_array().ok_or("Missing dependency pins")?.iter().map(|p| string(p, "digest")).collect::<Result<Vec<_>, _>>()?;
    doc.cityos = Some(CityosBinding {
        schema_version: string(&draft,"schemaVersion")?,
        document_id: string(&draft, "documentId")?,
        draft_id: string(&draft, "draftId")?,
        base: CityosBase::CanonicalGit { repository_id: string(&draft["base"], "repositoryId")?, commit: string(&draft["base"], "commit")? },
        semantic_base: draft["semanticBase"].clone(),
        dependency_pins: pins,
        occurrences,
        exchange_v2: if draft["schemaVersion"]=="cityos.design-exchange.v2" {Some(draft.clone())} else {None},
        exchange_v3: if draft["schemaVersion"]=="cityos.design-exchange.v3" {Some(draft)} else {None},
    });
    doc.cityos.as_ref().unwrap().validate()?;
    Ok(doc)
}
pub fn draft(doc: &Document) -> Option<&Value> {
    doc.cityos.as_ref()?.exchange()
}
pub fn revision(doc: &Document) -> Option<(String, u64)> {
    let d = draft(doc)?;
    Some((d["draftId"].as_str()?.into(), d["draftRevision"].as_u64()?))
}

pub fn local_copy(document: &Document) -> Document {
    let mut copy = document.clone();
    copy.cityos = None;
    copy.name = format!("{} · local copy", copy.name);
    for node in copy.nodes.values_mut() { node.locked = false; node.source_id = None; }
    copy
}


