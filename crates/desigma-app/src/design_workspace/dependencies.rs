//! Copy only verified, digest-addressed application artifacts into retention storage.
use desigma_doc::Document;
use desigma_library::Library;
use serde_json::Value;
use std::path::Path;

pub fn complete(library: &Library, document: &Document) -> bool {
    document.cityos.as_ref().is_some_and(|binding| binding.dependency_pins.iter()
        .all(|digest| library.load_cityos_payload(digest).is_ok()))
}

pub fn retain(cache: &Path, library: &Library, draft: &Value, receipt: &Value) -> Result<(), String> {
    if ["draftId", "draftRevision", "semanticDigest"].iter().any(|key| draft[*key] != receipt[*key]) {
        return Err("Obsolete dependency receipt discarded".into());
    }
    let pins = draft["dependencyPins"].as_array().ok_or("Missing dependency pins")?;
    let artifacts = receipt["artifacts"].as_array().ok_or("Missing dependency artifacts")?;
    let expected: std::collections::BTreeSet<_> = pins.iter().map(|pin| pin["digest"].as_str().ok_or("Invalid dependency pin"))
        .collect::<Result<_, _>>()?;
    if expected.len() != artifacts.len() { return Err("Incomplete dependency closure".into()); }
    let directory = cache.join("workspace-dependencies").canonicalize().map_err(|e| e.to_string())?;
    let cache = cache.canonicalize().map_err(|e| e.to_string())?;
    if directory.parent() != Some(cache.as_path()) { return Err("Dependency artifact directory escapes the application cache".into()); }
    let mut observed = std::collections::BTreeSet::new();
    for artifact in artifacts {
        let digest = artifact["digest"].as_str().filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or("Invalid dependency identity")?;
        if !observed.insert(digest) || !pins.iter().any(|pin| pin["digest"] == digest) {
            return Err("Unexpected or duplicate dependency artifact".into());
        }
        let file = directory.join(digest);
        let metadata = std::fs::symlink_metadata(&file).map_err(|e| e.to_string())?;
        let resolved = file.canonicalize().map_err(|e| e.to_string())?;
        if !metadata.is_file() || metadata.file_type().is_symlink() || resolved.parent() != Some(directory.as_path()) {
            return Err("Unsafe dependency artifact path".into());
        }
        if metadata.len() > 32 * 1024 * 1024 || artifact["size"].as_u64() != Some(metadata.len()) {
            return Err("Dependency artifact exceeds its bound or changed during capture".into());
        }
        use std::io::Read;
        let mut bytes = Vec::new();
        std::fs::File::open(&file).map_err(|e| e.to_string())?.take(32 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        if bytes.len() as u64 != metadata.len() { return Err("Dependency artifact changed during retention".into()); }
        library.put_cityos_payload(digest, &bytes).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn dependencies_require_matching_revision_exact_membership_and_verified_bytes() {
        let root = std::env::temp_dir().join(format!("desigma-dependencies-{}", uuid::Uuid::new_v4()));
        let cache = root.join("cache"); let directory = cache.join("workspace-dependencies");
        std::fs::create_dir_all(&directory).unwrap();
        let library = Library::open(root.join("library")).unwrap();
        let hash = desigma_library::payload_digest(b"captured");
        std::fs::write(directory.join(&hash), b"captured").unwrap();
        let draft = json!({"draftId":"d","draftRevision":1,"semanticDigest":"semantic","dependencyPins":[{"digest":hash}]});
        let mut receipt = json!({"draftId":"d","draftRevision":1,"semanticDigest":"semantic","artifacts":[{"digest":hash,"size":8}]});
        retain(&cache, &library, &draft, &receipt).unwrap();
        assert_eq!(library.load_cityos_payload(&hash).unwrap(), b"captured");
        receipt["draftRevision"] = json!(0); assert!(retain(&cache, &library, &draft, &receipt).is_err());
        receipt["draftRevision"] = json!(1);
        std::fs::write(directory.join(&hash), b"tampered").unwrap();
        assert!(retain(&cache, &library, &draft, &receipt).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
