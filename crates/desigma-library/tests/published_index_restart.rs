use desigma_doc::{Color, Document, Paint, Style};
use desigma_library::Library;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

const LIBRARY_ID: &str = "persisted-library";
const WORKER: &str = "published_index_restart_worker";

struct TestRoot(PathBuf);

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn document(name: &str, color: Color) -> Document {
    let mut document = Document::new(name);
    document.styles.insert("Brand".into(), Style::Fill(vec![Paint::solid(color)]));
    document
}

fn published_files(root: &Path) -> BTreeMap<OsString, Vec<u8>> {
    std::fs::read_dir(root.join("libraries").join(LIBRARY_ID))
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (entry.file_name(), std::fs::read(entry.path()).unwrap())
        })
        .collect()
}

fn restart(root: &Path, mode: &str) -> std::process::ExitStatus {
    std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", WORKER, "--ignored", "--test-threads=1"])
        .env("DESIGMA_LIBRARY_INDEX_TEST_ROOT", root)
        .env("DESIGMA_LIBRARY_INDEX_TEST_MODE", mode)
        .status()
        .unwrap()
}

#[test]
#[ignore = "subprocess helper for unavailable_published_index_after_restart_preserves_snapshots_and_recovers_after_repair"]
fn published_index_restart_worker() {
    let Some(root) = std::env::var_os("DESIGMA_LIBRARY_INDEX_TEST_ROOT") else { return };
    let mode = std::env::var("DESIGMA_LIBRARY_INDEX_TEST_MODE").unwrap();
    let library = Library::open(root).unwrap();
    // No PublishInfo hint: the durable index must remain the version authority
    // even if the publishing document was not saved after the previous publish.
    let candidate = document("Third", Color::rgb(0, 255, 0));
    match mode.as_str() {
        "reject" => {
            assert!(library.library_index(LIBRARY_ID).is_err());
            assert!(library.list_libraries().is_err(), "a damaged library must not disappear from a partial catalogue");
            assert!(library.load_snapshot(LIBRARY_ID, Some(1)).is_err(), "explicit reads must validate the publication index");
            assert!(library.publish_library(LIBRARY_ID, &candidate, "Third").is_err(), "an unavailable existing index must not restart the version sequence");
        }
        "retry" => assert_eq!(library.publish_library(LIBRARY_ID, &candidate, "Third").unwrap().version, 3),
        _ => panic!("Unexpected index restart worker mode"),
    }
}

#[test]
fn unavailable_published_index_after_restart_preserves_snapshots_and_recovers_after_repair() {
    for damage in ["truncated", "missing", "invalid-utf8"] {
        let root = TestRoot(std::env::temp_dir().join(format!("desigma-published-index-{}-{}", std::process::id(), desigma_library::new_id())));
        let library = Library::open(&root.0).unwrap();
        assert_eq!(library.publish_library(LIBRARY_ID, &document("First", Color::rgb(255, 0, 0)), "First").unwrap().version, 1);
        assert_eq!(library.publish_library(LIBRARY_ID, &document("Second", Color::rgb(0, 0, 255)), "Second").unwrap().version, 2);
        library.set_library_source(LIBRARY_ID, Some("source.desigma")).unwrap();
        library.set_library_source_file(LIBRARY_ID, Some("source-file")).unwrap();
        let original_index = library.library_index(LIBRARY_ID).unwrap();
        let original_files = published_files(&root.0);
        let index_path = root.0.join("libraries").join(LIBRARY_ID).join("index.json");
        let index_bytes = std::fs::read(&index_path).unwrap();
        drop(library);

        // Real persisted damage, followed by a replacement process. No injected
        // error, cache or Library handle can supply the missing version history.
        match damage {
            "truncated" => std::fs::write(&index_path, b"{\"versions\":[").unwrap(),
            "missing" => std::fs::remove_file(&index_path).unwrap(),
            "invalid-utf8" => std::fs::write(&index_path, [0xff, 0xfe]).unwrap(),
            _ => unreachable!(),
        }
        let damaged_files = published_files(&root.0);
        let result = restart(&root.0, "reject");
        assert!(published_files(&root.0) == damaged_files, "{damage}: failed publication changed durable file bytes");
        assert!(result.success(), "{damage}: replacement process must reject publication");

        // Restoring the exact saved index is an explicit repair, not automatic
        // reconstruction that could silently lose descriptions/source bindings.
        std::fs::write(&index_path, &index_bytes).unwrap();
        assert!(restart(&root.0, "retry").success(), "{damage}: publication must resume after repair");
        let reopened = Library::open(&root.0).unwrap();
        let recovered = reopened.library_index(LIBRARY_ID).unwrap();
        assert_eq!(recovered.versions.len(), 3);
        assert_eq!(&recovered.versions[..2], original_index.versions.as_slice());
        assert_eq!(recovered.source_path, original_index.source_path);
        assert_eq!(recovered.source_file, original_index.source_file);
        assert_eq!(recovered.latest_version(), 3);
        assert_eq!(recovered.latest().unwrap().description, "Third");
        assert_eq!(reopened.load_snapshot(LIBRARY_ID, None).unwrap().version, 3);
        let repaired_files = published_files(&root.0);
        assert_eq!(repaired_files.len(), original_files.len() + 1);
        for (name, bytes) in &original_files {
            if name.to_str() != Some("index.json") {
                assert!(repaired_files.get(name) == Some(bytes), "{damage}: old snapshot {name:?} changed after retry");
            }
        }
    }
}

#[test]
fn legacy_layout_remains_readable_and_preserves_metadata_when_next_publication_commits() {
    let root = TestRoot(std::env::temp_dir().join(format!("desigma-legacy-publication-{}-{}", std::process::id(), desigma_library::new_id())));
    let dir = root.0.join("libraries").join(LIBRARY_ID);
    std::fs::create_dir_all(&dir).unwrap();
    let source = document("Legacy", Color::rgb(255, 0, 0));
    // Independent legacy fixture: no journal, lock, or new publisher involved.
    let snapshot = desigma_doc::snapshot(&source, LIBRARY_ID, "Legacy", 7);
    let snapshot_bytes = serde_json::to_vec(&snapshot).unwrap();
    let index = serde_json::json!({
        "id": LIBRARY_ID, "name": "Legacy",
        "versions": [{ "version": 7, "description": "Retained description", "published_at": "2025-01-02T03:04:05Z", "changelog": [] }],
        "source_path": "legacy-source.desigma", "source_file": "legacy-source-file"
    });
    // Legacy optional asset counts deliberately absent.
    let index_bytes = serde_json::to_vec(&index).unwrap();
    std::fs::write(dir.join("7.json"), &snapshot_bytes).unwrap();
    std::fs::write(dir.join("index.json"), &index_bytes).unwrap();
    let lib = Library::open(&root.0).unwrap();
    let before = lib.library_index(LIBRARY_ID).unwrap();
    assert_eq!(lib.list_libraries().unwrap(), vec![before.clone()]);
    assert_eq!(lib.load_snapshot(LIBRARY_ID, None).unwrap().version, 7);
    assert!(std::fs::read(dir.join("index.json")).unwrap() == index_bytes, "reading legacy data rewrote its index");
    assert_eq!(lib.publish_library(LIBRARY_ID, &document("Next", Color::rgb(0, 0, 255)), "Next").unwrap().version, 8);
    let after = Library::open(&root.0).unwrap().library_index(LIBRARY_ID).unwrap();
    assert_eq!(&after.versions[..1], before.versions.as_slice());
    assert_eq!(after.source_path, before.source_path);
    assert_eq!(after.source_file, before.source_file);
    assert_eq!(after.styles, 1);
    assert!(std::fs::read(dir.join("7.json")).unwrap() == snapshot_bytes, "legacy version 7 bytes changed");
}

#[test]
fn valid_json_with_invalid_history_or_orphan_snapshot_is_never_exposed_or_overwritten() {
    for damage in ["duplicate-version", "identity", "missing-snapshot", "snapshot-identity", "orphan"] {
        let root = TestRoot(std::env::temp_dir().join(format!("desigma-publication-history-{}-{}", std::process::id(), desigma_library::new_id())));
        let lib = Library::open(&root.0).unwrap();
        lib.publish_library(LIBRARY_ID, &document("First", Color::rgb(255, 0, 0)), "First").unwrap();
        let dir = root.0.join("libraries").join(LIBRARY_ID);
        match damage {
            "duplicate-version" | "identity" => {
                let path = dir.join("index.json");
                let mut index: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                if damage == "identity" {
                    index["id"] = "other-library".into();
                } else {
                    let entry = index["versions"][0].clone();
                    index["versions"].as_array_mut().unwrap().push(entry);
                }
                std::fs::write(path, serde_json::to_vec(&index).unwrap()).unwrap();
            }
            "missing-snapshot" => std::fs::remove_file(dir.join("1.json")).unwrap(),
            "snapshot-identity" => {
                let path = dir.join("1.json");
                let mut snapshot: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                snapshot["version"] = 9.into();
                std::fs::write(path, serde_json::to_vec(&snapshot).unwrap()).unwrap();
            }
            "orphan" => std::fs::write(dir.join("2.json"), b"orphan snapshot bytes").unwrap(),
            _ => unreachable!(),
        }
        let before = published_files(&root.0);
        let reopened = Library::open(&root.0).unwrap();
        assert!(reopened.library_index(LIBRARY_ID).is_err(), "{damage}");
        assert!(reopened.list_libraries().is_err(), "{damage}");
        assert!(reopened.load_snapshot(LIBRARY_ID, Some(1)).is_err(), "{damage}");
        assert!(reopened.publish_library(LIBRARY_ID, &document("Next", Color::rgb(0, 0, 255)), "Next").is_err(), "{damage}");
        assert!(published_files(&root.0) == before, "{damage}: rejected operation changed durable bytes");
    }
}
