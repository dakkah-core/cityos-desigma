//! Existing portable-checkpoint v1 codec. Parsing grants no owner authority.
use crate::{payload_digest, LibraryError, Result};
use desigma_doc::{CityosBase, Document, DocumentCheckpoint};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::path::Path;

/// Resource controls, not performance claims. Oversized files fail explicitly.
pub const MAX_PORTABLE_CHECKPOINT_BYTES: usize = 128 * 1024 * 1024;
const MAX_PAYLOAD_BYTES: usize = 32 * 1024 * 1024;
const MAX_PAYLOADS: usize = 4096;
const MAX_PAYLOAD_TOTAL: usize = 512 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PortableWire {
    #[serde(rename = "schemaVersion")]
    schema_version: String,
    checkpoint: String,
    #[serde(deserialize_with = "read_payloads")]
    payloads: BTreeMap<String, Vec<u8>>,
}

fn read_payloads<'de, D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<BTreeMap<String, Vec<u8>>, D::Error> {
    struct Payloads;
    impl<'de> serde::de::Visitor<'de> for Payloads {
        type Value = BTreeMap<String, Vec<u8>>;
        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result { formatter.write_str("unique digest-verified dependency payloads") }
        fn visit_map<M: serde::de::MapAccess<'de>>(self, mut map: M) -> std::result::Result<Self::Value, M::Error> {
            let mut payloads = BTreeMap::new(); let mut total = 0usize;
            while let Some((hash, bytes)) = map.next_entry::<String, Vec<u8>>()? {
                total = total.checked_add(bytes.len()).ok_or_else(|| serde::de::Error::custom("Portable payload byte limit"))?;
                if bytes.len() > MAX_PAYLOAD_BYTES || total > MAX_PAYLOAD_TOTAL || payloads.len() >= MAX_PAYLOADS {
                    return Err(serde::de::Error::custom("Portable payload resource limit"));
                }
                if payload_digest(&bytes) != hash || payloads.insert(hash, bytes).is_some() {
                    return Err(serde::de::Error::custom("Portable payload digest mismatch or duplicate identity"));
                }
            }
            Ok(payloads)
        }
    }
    deserializer.deserialize_map(Payloads)
}

pub(crate) fn check_policy(checkpoint: &DocumentCheckpoint) -> Result<()> {
    let states = std::iter::once((&checkpoint.document, &checkpoint.history))
        .chain(checkpoint.retained.iter().map(|state| (&state.document, &state.history)));
    for (document, history) in states {
        for document in std::iter::once(document).chain(history.retained_documents()) {
            if document.cityos.as_ref().is_some_and(|binding| matches!(binding.base, CityosBase::NativeOwner { .. })) {
                return Err(LibraryError::Invalid("Native owner portable retention/export policy is not qualified".into()));
            }
        }
    }
    Ok(())
}

/// Validated payloads are private so an import cannot replace their bytes after
/// admission. The native workspace must still validate bindings and owner scope.
pub struct VerifiedPortableCheckpoint {
    checkpoint: DocumentCheckpoint,
    payloads: BTreeMap<String, Vec<u8>>,
}
impl std::fmt::Debug for VerifiedPortableCheckpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VerifiedPortableCheckpoint").field("dependencies", &self.payloads.len()).finish_non_exhaustive()
    }
}
impl VerifiedPortableCheckpoint {
    pub fn parse(text: &str) -> Result<Self> {
        if text.len() > MAX_PORTABLE_CHECKPOINT_BYTES { return Err(LibraryError::Invalid("Portable checkpoint exceeds the 128 MiB file limit".into())); }
        let wire: PortableWire = serde_json::from_str(text)?;
        if wire.schema_version != "desigma.portable-checkpoint.v1" { return Err(LibraryError::Invalid("Unsupported portable checkpoint version".into())); }
        let checkpoint = DocumentCheckpoint::from_json(&wire.checkpoint)?;
        let roots: BTreeSet<_> = checkpoint.dependency_pins().into_iter().collect();
        if roots != wire.payloads.keys().cloned().collect() { return Err(LibraryError::Invalid("Portable dependency closure mismatch".into())); }
        check_policy(&checkpoint)?;
        Ok(Self { checkpoint, payloads: wire.payloads })
    }
    /// JSON inputs are bounded before allocation, including a growing-file race.
    /// Unknown JSON formats remain on their existing native/Figma import path.
    pub fn read(path: &Path) -> Result<Option<Self>> {
        if !path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("json")) { return Ok(None); }
        let file = std::fs::File::open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() > MAX_PORTABLE_CHECKPOINT_BYTES as u64 {
            return Err(LibraryError::Invalid("JSON import exceeds the 128 MiB file limit or is not a regular file".into()));
        }
        let mut text = String::new();
        file.take(MAX_PORTABLE_CHECKPOINT_BYTES as u64 + 1).read_to_string(&mut text)?;
        if text.len() > MAX_PORTABLE_CHECKPOINT_BYTES { return Err(LibraryError::Invalid("JSON import changed beyond the 128 MiB file limit".into())); }
        // Serde also accepts structs as positional arrays; only objects carry
        // a portable header. Still reject malformed JSON before falling back.
        if !text.trim_start().starts_with('{') {
            serde_json::from_str::<serde::de::IgnoredAny>(&text)?;
            return Ok(None);
        }
        #[derive(Deserialize)]
        struct Header { #[serde(rename = "schemaVersion", default)] schema_version: serde_json::Value }
        let header: Header = serde_json::from_str(&text)?;
        if header.schema_version.as_str().is_some_and(|version| version.starts_with("desigma.portable-checkpoint.")) {
            Self::parse(&text).map(Some)
        } else { Ok(None) }
    }
    pub fn document(&self) -> &Document { &self.checkpoint.document }
    pub(crate) fn into_parts(self) -> (DocumentCheckpoint, BTreeMap<String, Vec<u8>>) { (self.checkpoint, self.payloads) }
}

pub(crate) fn encode(checkpoint: &DocumentCheckpoint, payloads: BTreeMap<String, Vec<u8>>) -> Result<String> {
    check_policy(checkpoint)?;
    if payloads.len() > MAX_PAYLOADS || payloads.values().any(|bytes| bytes.len() > MAX_PAYLOAD_BYTES)
        || payloads.values().map(Vec::len).sum::<usize>() > MAX_PAYLOAD_TOTAL {
        return Err(LibraryError::Invalid("Portable payload resource limit".into()));
    }
    struct Bounded { bytes: Vec<u8>, exceeded: bool }
    impl Write for Bounded {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > MAX_PORTABLE_CHECKPOINT_BYTES.saturating_sub(self.bytes.len()) {
                self.exceeded = true; return Err(std::io::Error::other("Portable checkpoint exceeds the 128 MiB file limit"));
            }
            self.bytes.extend_from_slice(bytes); Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }
    let wire = PortableWire { schema_version: "desigma.portable-checkpoint.v1".into(), checkpoint: checkpoint.to_json()?, payloads };
    let mut output = Bounded { bytes: Vec::new(), exceeded: false };
    if let Err(error) = serde_json::to_writer(&mut output, &wire) {
        if output.exceeded { return Err(LibraryError::Invalid("Portable checkpoint exceeds the 128 MiB file limit".into())); }
        return Err(error.into());
    }
    String::from_utf8(output.bytes).map_err(|error| LibraryError::Invalid(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use desigma_doc::History;
    use std::path::PathBuf;

    struct JsonFile(PathBuf);
    impl JsonFile {
        fn new(text: &str) -> Self {
            let path = std::env::temp_dir().join(format!("desigma_portable_{}_{}.json", std::process::id(), crate::new_id()));
            std::fs::write(&path, text).unwrap();
            Self(path)
        }
    }
    impl Drop for JsonFile {
        fn drop(&mut self) { let _ = std::fs::remove_file(&self.0); }
    }

    fn portable_json() -> serde_json::Value {
        let checkpoint = DocumentCheckpoint::new(Document::new("Portable draft"), History::default());
        serde_json::from_str(&encode(&checkpoint, BTreeMap::new()).unwrap()).unwrap()
    }

    #[test]
    fn read_leaves_unrelated_json_on_the_existing_import_path() {
        for text in [
            "[]", "[1, true, null]", r#"["desigma.portable-checkpoint.v1"]"#,
            r#"["desigma.portable-checkpoint.v2"]"#,
            r#"[{"schemaVersion":"desigma.portable-checkpoint.v1"}]"#,
            "{}", r#"{"schemaVersion":"figma.v1"}"#, r#"{"document":{"id":"root"}}"#,
            "null", "true", "42", r#""desigma.portable-checkpoint.v1""#,
        ] {
            let file = JsonFile::new(text);
            assert!(VerifiedPortableCheckpoint::read(&file.0).unwrap().is_none(), "{text}");
        }
    }

    #[test]
    fn read_leaves_non_string_schema_versions_on_the_existing_import_path() {
        for version in ["null", "true", "false", "1", "1.5", "[]", "{}", r#"["desigma.portable-checkpoint.v1"]"#] {
            let text = format!(r#"{{"schemaVersion":{version}}}"#);
            let file = JsonFile::new(&text);
            assert!(VerifiedPortableCheckpoint::read(&file.0).unwrap().is_none(), "{text}");
        }
    }

    #[test]
    fn read_validates_and_returns_the_exact_portable_format() {
        let file = JsonFile::new(&portable_json().to_string());
        let portable = VerifiedPortableCheckpoint::read(&file.0).unwrap().expect("portable checkpoint");
        assert_eq!(portable.document().name, "Portable draft");
        let (checkpoint, payloads) = portable.into_parts();
        assert!(checkpoint.dependency_pins().is_empty());
        assert!(payloads.is_empty());
    }

    #[test]
    fn read_rejects_invalid_exact_portable_formats_instead_of_falling_back() {
        for fault in ["missing", "checkpoint", "digest", "closure", "unknown"] {
            let mut value = portable_json();
            match fault {
                "missing" => { value.as_object_mut().unwrap().remove("checkpoint"); }
                "checkpoint" => value["checkpoint"] = serde_json::json!("{}"),
                "digest" => value["payloads"]["invalid-digest"] = serde_json::json!([1]),
                "closure" => value["payloads"][payload_digest(b"extra")] = serde_json::json!(b"extra".to_vec()),
                _ => value["unknown"] = serde_json::json!(true),
            }
            let file = JsonFile::new(&value.to_string());
            assert!(VerifiedPortableCheckpoint::read(&file.0).is_err(), "{fault}");
        }
    }

    #[test]
    fn read_explicitly_rejects_future_portable_versions() {
        for version in ["desigma.portable-checkpoint.v2", "desigma.portable-checkpoint.v999"] {
            let mut value = portable_json();
            value["schemaVersion"] = serde_json::json!(version);
            let file = JsonFile::new(&value.to_string());
            assert!(matches!(VerifiedPortableCheckpoint::read(&file.0), Err(LibraryError::Invalid(message))
                if message == "Unsupported portable checkpoint version"), "{version}");
        }
    }

    #[test]
    fn read_still_rejects_malformed_json_and_duplicate_headers() {
        for text in ["[", "{", r#"{"schemaVersion":null,"schemaVersion":"desigma.portable-checkpoint.v1"}"#] {
            let file = JsonFile::new(text);
            assert!(VerifiedPortableCheckpoint::read(&file.0).is_err(), "{text}");
        }
    }
}
