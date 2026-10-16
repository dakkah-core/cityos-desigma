//! Version history of library files.
//!
//! Layout under [`Library::root`]:
//!
//! ```text
//! versions/<file-id>/<n>.desigma   a full snapshot of the document
//! versions/<file-id>/index.json    [{n, label, description, auto, created, size}]
//! ```
//!
//! Versions are either *named* (saved by the user, "Save to version history…")
//! or *auto* (saved while editing and when closing a file). Named versions are
//! always kept; auto versions are thinned by [`retention`] every time a new
//! auto version is written. Deleting a version moves it to the trash folder,
//! it is never hard-deleted except by [`Library::empty_trash`].
//!
//! The index and snapshots are written atomically, so a crash mid-write never
//! leaves a truncated file. A missing or damaged index is rebuilt from the
//! snapshot files.

use crate::store::{now_ms, remove_durable, write_atomic};
use crate::{Library, LibraryError, Result};
use desigma_doc::Document;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

const INDEX: &str = "index.json";
const PENDING: &str = ".version.pending.json";
const SEQUENCE: &str = ".sequence.json";
const HOUR: u64 = 3_600_000;
const DAY: u64 = 24 * HOUR;
/// Auto versions older than this are dropped.
pub const AUTO_MAX_AGE: u64 = 30 * DAY;

/// One saved snapshot of a file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileVersion {
    /// Sequence number, 1-based and never reused.
    pub n: u32,
    /// Name given by the user; `None` for plain auto versions.
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// Saved automatically (autosave, close, before a restore).
    #[serde(default)]
    pub auto: bool,
    /// Milliseconds since the Unix epoch.
    pub created: u64,
    /// Size of the snapshot in bytes.
    #[serde(default)]
    pub size: u64,
}

impl FileVersion {
    /// Kept forever: saved by the user, or an auto version that was later named.
    pub fn is_named(&self) -> bool {
        !self.auto || self.label.as_deref().is_some_and(|l| !l.trim().is_empty())
    }
}

static LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn lock() -> MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingVersion {
    schema_version: String,
    version: FileVersion,
    document: String,
    entries: Vec<FileVersion>,
    prune: Vec<u32>,
}

#[cfg(test)]
thread_local! {
    static VERSION_FAULT: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
}

fn version_fault(_phase: &str) -> Result<()> {
    #[cfg(test)]
    if VERSION_FAULT.with(|fault| {
        if fault.get() == Some(_phase) { fault.set(None); true } else { false }
    }) { return Err(std::io::Error::other(format!("Interrupted version at {_phase}")).into()); }
    Ok(())
}

/// Which auto versions the retention policy no longer keeps.
///
/// - the last hour: every auto version is kept;
/// - up to one day old: the newest auto version of each clock hour;
/// - up to 30 days old: the newest auto version of each calendar day (UTC);
/// - older than that: dropped.
///
/// Named versions are never returned. Order of `entries` does not matter.
pub fn retention(entries: &[FileVersion], now: u64) -> Vec<u32> {
    use std::collections::BTreeMap;
    let mut newest: BTreeMap<(u8, u64), (u64, u32)> = BTreeMap::new();
    let mut drop = Vec::new();
    for e in entries.iter().filter(|e| !e.is_named()) {
        let age = now.saturating_sub(e.created);
        let bucket = if age < HOUR {
            continue;
        } else if age < DAY {
            (0u8, e.created / HOUR)
        } else if age < AUTO_MAX_AGE {
            (1u8, e.created / DAY)
        } else {
            drop.push(e.n);
            continue;
        };
        let slot = newest.entry(bucket).or_insert((e.created, e.n));
        if (e.created, e.n) > *slot {
            drop.push(slot.1);
            *slot = (e.created, e.n);
        } else if (e.created, e.n) < *slot {
            drop.push(e.n);
        }
    }
    drop.sort_unstable();
    drop.dedup();
    drop
}

impl Library {
    fn versions_dir(&self, file: &str) -> PathBuf {
        self.path("versions").join(file)
    }

    fn version_path(&self, file: &str, n: u32) -> PathBuf {
        self.versions_dir(file).join(format!("{n}.desigma"))
    }

    fn version_metadata_path(&self, file: &str, n: u32) -> PathBuf {
        self.versions_dir(file).join(format!("{n}.meta.json"))
    }

    fn version_sequence(&self, file: &str) -> Result<u32> {
        match std::fs::read(self.versions_dir(file).join(SEQUENCE)) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
            Err(e) => Err(e.into()),
        }
    }

    fn recover_version_save(&self, file: &str) -> Result<()> {
        crate::validate_id(file)?;
        let path = self.versions_dir(file).join(PENDING);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        let pending: PendingVersion = serde_json::from_slice(&bytes)?;
        Document::from_json(&pending.document)?;
        let mut ids = std::collections::BTreeSet::new();
        if pending.schema_version != "desigma.version-save.v1" || pending.version.n == 0
            || pending.version.size != pending.document.len() as u64
            || !pending.entries.iter().any(|entry| entry == &pending.version)
            || pending.entries.iter().any(|entry| entry.n == 0 || !ids.insert(entry.n) || pending.prune.contains(&entry.n))
        {
            return Err(LibraryError::Invalid("Invalid pending version save".into()));
        }
        let sequence = self.version_sequence(file)?.max(pending.version.n);
        write_atomic(&self.versions_dir(file).join(SEQUENCE), &serde_json::to_vec(&sequence)?)?;
        write_atomic(&self.version_path(file, pending.version.n), pending.document.as_bytes())?;
        version_fault("snapshot")?;
        write_atomic(&self.version_metadata_path(file, pending.version.n), &serde_json::to_vec(&pending.version)?)?;
        version_fault("metadata")?;
        self.write_index(file, &pending.entries)?;
        version_fault("index")?;
        // Retention only runs after the replacement index is durable, while the
        // journal still makes an interrupted prune resumable.
        for n in pending.prune {
            remove_durable(&self.version_path(file, n))?;
            remove_durable(&self.version_metadata_path(file, n))?;
        }
        version_fault("prune")?;
        remove_durable(&path)
    }

    pub(crate) fn recover_pending_versions(&self) -> Result<()> {
        let _guard = lock();
        let entries = match std::fs::read_dir(self.path("versions")) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        for entry in entries {
            let entry = entry?;
            if entry.file_type()?.is_dir() { self.recover_version_save(&entry.file_name().to_string_lossy())?; }
        }
        Ok(())
    }

    fn read_index(&self, file: &str) -> Result<Vec<FileVersion>> {
        crate::validate_id(file)?;
        self.recover_version_save(file)?;
        let dir = self.versions_dir(file);
        let indexed = match std::fs::read(dir.join(INDEX)) {
            Ok(bytes) => serde_json::from_slice::<Vec<FileVersion>>(&bytes).unwrap_or_default(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e.into()),
        };
        let rd = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut out = Vec::new();
        // Reconcile even a syntactically valid index with orphan snapshots left
        // by older writers. Per-snapshot metadata preserves named retention.
        for entry in rd {
            let entry = entry?;
            let Some(n) = entry.file_name().to_str().and_then(|s| s.strip_suffix(".desigma")).and_then(|s| s.parse::<u32>().ok()) else { continue };
            if n == 0 { continue; }
            let metadata = entry.metadata()?;
            let mut version = match std::fs::read(self.version_metadata_path(file, n)) {
                Ok(bytes) => {
                    let version: FileVersion = serde_json::from_slice(&bytes)?;
                    if version.n != n || version.size != metadata.len() {
                        return Err(LibraryError::Invalid("Version metadata/snapshot mismatch".into()));
                    }
                    version
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => indexed.iter().find(|v| v.n == n).cloned().unwrap_or(FileVersion {
                    n, label: None, description: None, auto: true,
                    created: metadata.modified()?.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64,
                    size: metadata.len(),
                }),
                Err(e) => return Err(e.into()),
            };
            version.size = metadata.len();
            out.push(version);
        }
        Ok(out)
    }

    fn write_index(&self, file: &str, entries: &[FileVersion]) -> Result<()> {
        write_atomic(&self.versions_dir(file).join(INDEX), serde_json::to_string_pretty(entries)?.as_bytes())
    }

    /// Saves a snapshot of `doc` as a new version of `file_id`.
    ///
    /// `label` names the version; `auto` marks it as automatic. Saving an auto version
    /// also applies the retention policy.
    pub fn save_version(&self, file_id: &str, doc: &Document, label: Option<String>, auto: bool) -> Result<FileVersion> {
        self.save_version_described(file_id, doc, label, None, auto)
    }

    /// [`Library::save_version`] with a description (the "Save to version history" dialog).
    pub fn save_version_described(&self, file_id: &str, doc: &Document, label: Option<String>, description: Option<String>, auto: bool) -> Result<FileVersion> {
        self.save_version_at(file_id, doc, label, description, auto, now_ms())
    }

    /// Like `save_version_described` with an explicit clock (tests of the retention policy).
    pub fn save_version_at(
        &self,
        file_id: &str,
        doc: &Document,
        label: Option<String>,
        description: Option<String>,
        auto: bool,
        now: u64,
    ) -> Result<FileVersion> {
        crate::validate_id(file_id)?;
        let _g = lock();
        let mut entries = self.read_index(file_id)?;
        let clean = |s: Option<String>| s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        let json = doc.to_json()?;
        Document::from_json(&json)?;
        let on_disk_max = std::fs::read_dir(self.versions_dir(file_id))
            .map(|rd| rd.flatten().filter_map(|e| e.file_name().to_str()?.strip_suffix(".desigma")?.parse::<u32>().ok()).max().unwrap_or(0))
            .unwrap_or(0);
        let trashed_max = std::fs::read_dir(self.path("trash"))
            .map(|rd| rd.flatten().filter_map(|e| e.file_name().to_str()?.strip_prefix(&format!("{file_id}-v"))?.parse::<u32>().ok()).max().unwrap_or(0))
            .unwrap_or(0);
        let n = entries.iter().map(|e| e.n).max().unwrap_or(0).max(on_disk_max).max(trashed_max).max(self.version_sequence(file_id)?)
            .checked_add(1).ok_or_else(|| LibraryError::Invalid("Version sequence exhausted".into()))?;
        let v = FileVersion { n, label: clean(label), description: clean(description), auto, created: now, size: json.len() as u64 };
        entries.push(v.clone());
        let prune: Vec<_> = if auto { retention(&entries, now).into_iter().filter(|old| *old != n).collect() } else { Vec::new() };
        entries.retain(|e| !prune.contains(&e.n));
        let pending = PendingVersion { schema_version: "desigma.version-save.v1".into(), version: v.clone(), document: json, entries, prune };
        write_atomic(&self.versions_dir(file_id).join(PENDING), &serde_json::to_vec(&pending)?)?;
        version_fault("journal")?;
        self.recover_version_save(file_id)?;
        Ok(v)
    }

    /// The versions of a file, newest first.
    pub fn list_versions(&self, file_id: &str) -> Vec<FileVersion> {
        self.try_list_versions(file_id).unwrap_or_default()
    }

    /// Error-preserving history discovery for app reconciliation. The legacy
    /// `list_versions` convenience API cannot distinguish unavailable from empty.
    pub fn try_list_versions(&self, file_id: &str) -> Result<Vec<FileVersion>> {
        let _g = lock();
        let mut v = self.read_index(file_id)?;
        v.retain(|e| self.version_path(file_id, e.n).exists());
        v.sort_by(|a, b| b.created.cmp(&a.created).then(b.n.cmp(&a.n)));
        Ok(v)
    }

    pub fn load_version(&self, file_id: &str, n: u32) -> Result<Document> {
        crate::validate_id(file_id)?;
        let _guard = lock();
        self.recover_version_save(file_id)?;
        let text = std::fs::read_to_string(self.version_path(file_id, n)).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                LibraryError::NotFound(format!("version {n} of {file_id}"))
            } else {
                e.into()
            }
        })?;
        Ok(Document::from_json(&text)?)
    }

    /// Gives a version a name and/or description (an empty name clears it).
    pub fn rename_version(&self, file_id: &str, n: u32, label: Option<String>, description: Option<String>) -> Result<()> {
        let _g = lock();
        let mut entries = self.read_index(file_id)?;
        let e = entries.iter_mut().find(|e| e.n == n).ok_or_else(|| LibraryError::NotFound(format!("version {n} of {file_id}")))?;
        let clean = |s: Option<String>| s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        e.label = clean(label);
        if description.is_some() {
            e.description = clean(description);
        }
        write_atomic(&self.version_metadata_path(file_id, n), &serde_json::to_vec(e)?)?;
        self.write_index(file_id, &entries)
    }

    /// Moves a version to the trash (`trash/<file-id>-v<n>/`) and drops it from the index.
    pub fn delete_version(&self, file_id: &str, n: u32) -> Result<()> {
        let _g = lock();
        let mut entries = self.read_index(file_id)?;
        let i = entries.iter().position(|e| e.n == n).ok_or_else(|| LibraryError::NotFound(format!("version {n} of {file_id}")))?;
        let dir = self.path("trash").join(format!("{file_id}-v{n}"));
        std::fs::create_dir_all(&dir)?;
        write_atomic(&dir.join("version.json"), serde_json::to_string_pretty(&entries[i])?.as_bytes())?;
        let src = self.version_path(file_id, n);
        if src.exists() {
            std::fs::rename(src, dir.join("version.desigma"))?;
        }
        remove_durable(&self.version_metadata_path(file_id, n))?;
        entries.remove(i);
        self.write_index(file_id, &entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use desigma_doc::{Node, Rect};

    struct Tmp(PathBuf);
    impl Tmp {
        fn new() -> Self {
            static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let p = std::env::temp_dir().join(format!("desigma_versions_{}_{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            Self(p)
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn doc_with(rects: usize) -> Document {
        let mut d = Document::new("v");
        let page = d.add_page("Page 1");
        for _ in 0..rects {
            let id = d.alloc_id();
            d.insert(page, Node::rectangle(id, Rect::new(0.0, 0.0, 10.0, 10.0))).unwrap();
        }
        d
    }

    fn v(n: u32, age: u64, auto: bool, label: Option<&str>, now: u64) -> FileVersion {
        FileVersion { n, label: label.map(str::to_string), description: None, auto, created: now - age, size: 1 }
    }

    const NOW: u64 = 100 * DAY + 13 * HOUR + 30 * 60_000;

    #[test]
    fn interrupted_version_save_replays_metadata_index_and_retention_on_restart() {
        for phase in ["journal", "snapshot", "metadata", "index", "prune"] {
            let temp = Tmp::new();
            let library = Library::open(&temp.0).unwrap();
            library.save_version_at("f", &doc_with(1), None, None, true, NOW - 40 * DAY).unwrap();
            let named = library.save_version_at("f", &doc_with(2), Some("Keep".into()), Some("description".into()), false, NOW - 40 * DAY).unwrap();
            VERSION_FAULT.with(|fault| fault.set(Some(phase)));
            assert!(library.save_version_at("f", &doc_with(3), Some("Recovered".into()), Some("complete metadata".into()), true, NOW).is_err(), "{phase}");
            drop(library);
            let reopened = Library::open(&temp.0).unwrap();
            let versions = reopened.try_list_versions("f").unwrap();
            assert_eq!(versions.iter().map(|v| v.n).collect::<Vec<_>>(), [3, 2], "{phase}");
            assert_eq!(versions[0].label.as_deref(), Some("Recovered"));
            assert_eq!(versions[0].description.as_deref(), Some("complete metadata"));
            assert_eq!(versions[1], named);
            assert_eq!(reopened.load_version("f", 3).unwrap().nodes.len(), 4);
            assert!(!reopened.version_path("f", 1).exists());
            assert!(!reopened.versions_dir("f").join(PENDING).exists());
            drop(reopened);
            assert_eq!(Library::open(&temp.0).unwrap().try_list_versions("f").unwrap(), versions, "idempotent restart");
        }
    }

    #[test]
    fn valid_stale_index_and_damaged_index_recover_named_metadata_without_retention_loss() {
        let temp = Tmp::new();
        let library = Library::open(&temp.0).unwrap();
        let version = library.save_version_at("f", &doc_with(1), Some("Release".into()), Some("do not prune".into()), false, NOW - 40 * DAY).unwrap();
        for index in ["[]", "{truncated"] {
            std::fs::write(library.versions_dir("f").join(INDEX), index).unwrap();
            assert_eq!(library.try_list_versions("f").unwrap(), [version.clone()]);
        }
        library.save_version_at("f", &doc_with(2), None, None, true, NOW).unwrap();
        assert!(library.version_path("f", version.n).exists());
        assert_eq!(library.try_list_versions("f").unwrap()[1], version);
    }

    #[test]
    fn version_high_water_mark_survives_empty_trash_and_reports_exhaustion() {
        let temp = Tmp::new();
        let library = Library::open(&temp.0).unwrap();
        library.save_version("f", &doc_with(1), None, false).unwrap();
        library.delete_version("f", 1).unwrap();
        library.empty_trash().unwrap();
        let reopened = Library::open(&temp.0).unwrap();
        assert_eq!(reopened.save_version("f", &doc_with(2), None, false).unwrap().n, 2);
        std::fs::write(reopened.versions_dir("f").join(SEQUENCE), u32::MAX.to_string()).unwrap();
        assert!(reopened.save_version("f", &doc_with(3), None, false).is_err());
        assert_eq!(reopened.load_version("f", 2).unwrap().nodes.len(), 3);
        assert!(!reopened.versions_dir("f").join(PENDING).exists());
    }

    #[test]
    fn malformed_version_intent_is_not_overwritten_or_misreported_as_empty() {
        let temp = Tmp::new();
        let library = Library::open(&temp.0).unwrap();
        library.save_version("f", &doc_with(1), Some("saved".into()), false).unwrap();
        let pending = library.versions_dir("f").join(PENDING);
        std::fs::write(&pending, "{truncated").unwrap();
        assert!(Library::open(&temp.0).is_err());
        assert!(library.try_list_versions("f").is_err());
        assert!(library.save_version("f", &doc_with(2), None, true).is_err());
        assert_eq!(std::fs::read_to_string(pending).unwrap(), "{truncated");
        assert!(library.version_path("f", 1).exists());
    }

    #[test]
    fn history_identifiers_cannot_escape_the_versions_root() {
        let temp = Tmp::new();
        let library = Library::open(&temp.0).unwrap();
        for id in ["..", "../escape", "x/y", "x\\y", ""] {
            assert!(library.save_version(id, &doc_with(1), None, false).is_err());
            assert!(library.try_list_versions(id).is_err());
            assert!(library.load_version(id, 1).is_err());
            assert!(library.rename_version(id, 1, None, None).is_err());
            assert!(library.delete_version(id, 1).is_err());
        }
    }

    #[test]
    fn saves_lists_and_loads_versions() {
        let t = Tmp::new();
        let lib = Library::open(&t.0).unwrap();
        let a = lib.save_version("f1", &doc_with(1), None, true).unwrap();
        let b = lib.save_version("f1", &doc_with(3), Some("  Milestone ".into()), false).unwrap();
        assert_eq!((a.n, b.n), (1, 2));
        assert_eq!(b.label.as_deref(), Some("Milestone"));
        assert!(b.size > a.size);
        let list = lib.list_versions("f1");
        assert_eq!(list.iter().map(|e| e.n).collect::<Vec<_>>(), [2, 1], "newest first");
        assert_eq!(lib.load_version("f1", 1).unwrap().nodes.len(), 2);
        assert_eq!(lib.load_version("f1", 2).unwrap().nodes.len(), 4);
        assert!(lib.load_version("f1", 9).is_err());
        assert!(lib.list_versions("other").is_empty());
        assert!(t.0.join("versions/f1/index.json").exists() && t.0.join("versions/f1/2.desigma").exists());
        assert!(!t.0.join("versions/f1/index.json.tmp").exists(), "atomic writes leave no temp file");
    }

    #[test]
    fn index_has_the_documented_fields() {
        let t = Tmp::new();
        let lib = Library::open(&t.0).unwrap();
        lib.save_version("f", &doc_with(1), Some("x".into()), false).unwrap();
        let text = std::fs::read_to_string(t.0.join("versions/f/index.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        let e = &json[0];
        assert_eq!((e["n"].as_u64(), e["label"].as_str(), e["auto"].as_bool()), (Some(1), Some("x"), Some(false)));
        assert!(e["created"].as_u64().unwrap() > 0 && e["size"].as_u64().unwrap() > 0);
    }

    #[test]
    fn delete_moves_to_trash_and_numbers_are_not_reused() {
        let t = Tmp::new();
        let lib = Library::open(&t.0).unwrap();
        lib.save_version("f", &doc_with(1), None, true).unwrap();
        lib.save_version("f", &doc_with(2), None, true).unwrap();
        lib.delete_version("f", 2).unwrap();
        assert_eq!(lib.list_versions("f").len(), 1);
        assert!(!t.0.join("versions/f/2.desigma").exists());
        assert!(t.0.join("trash/f-v2/version.desigma").exists(), "restorable from the trash");
        assert!(lib.delete_version("f", 2).is_err());
        let next = lib.save_version("f", &doc_with(1), None, true).unwrap();
        assert_eq!(next.n, 3);
        assert!(lib.list_trash().is_empty(), "version leftovers are not listed as files");
    }

    #[test]
    fn rename_names_a_version_and_keeps_it() {
        let t = Tmp::new();
        let lib = Library::open(&t.0).unwrap();
        lib.save_version("f", &doc_with(1), None, true).unwrap();
        assert!(!lib.list_versions("f")[0].is_named());
        lib.rename_version("f", 1, Some("Final".into()), Some("shipped".into())).unwrap();
        let e = &lib.list_versions("f")[0];
        assert!(e.is_named());
        assert_eq!((e.label.as_deref(), e.description.as_deref()), (Some("Final"), Some("shipped")));
        lib.rename_version("f", 1, Some("  ".into()), None).unwrap();
        assert!(!lib.list_versions("f")[0].is_named());
    }

    #[test]
    fn a_damaged_index_is_rebuilt_from_the_snapshots() {
        let t = Tmp::new();
        let lib = Library::open(&t.0).unwrap();
        lib.save_version("f", &doc_with(1), Some("a".into()), false).unwrap();
        lib.save_version("f", &doc_with(2), None, true).unwrap();
        std::fs::write(t.0.join("versions/f/index.json"), "{ nope").unwrap();
        let list = lib.list_versions("f");
        assert_eq!(list.len(), 2);
        assert_eq!(lib.save_version("f", &doc_with(1), None, true).unwrap().n, 3);
    }

    #[test]
    fn retention_keeps_everything_in_the_last_hour() {
        let es: Vec<_> = (1..=10).map(|i| v(i, i as u64 * 5 * 60_000, true, None, NOW)).collect();
        assert!(es.iter().all(|e| NOW - e.created < HOUR));
        assert!(retention(&es, NOW).is_empty());
    }

    #[test]
    fn retention_thins_to_one_per_hour_for_a_day() {
        // three auto versions inside the same clock hour 3h ago, plus one in another hour
        let base = (NOW / HOUR - 3) * HOUR;
        let es = vec![
            FileVersion { n: 1, label: None, description: None, auto: true, created: base + 5 * 60_000, size: 1 },
            FileVersion { n: 2, label: None, description: None, auto: true, created: base + 20 * 60_000, size: 1 },
            FileVersion { n: 3, label: None, description: None, auto: true, created: base + 50 * 60_000, size: 1 },
            FileVersion { n: 4, label: None, description: None, auto: true, created: base - HOUR + 10 * 60_000, size: 1 },
        ];
        assert_eq!(retention(&es, NOW), vec![1, 2], "the newest of the hour survives");
    }

    #[test]
    fn retention_thins_to_one_per_day_for_a_month_and_drops_older() {
        let day = (NOW / DAY - 5) * DAY;
        let es = vec![
            FileVersion { n: 1, label: None, description: None, auto: true, created: day + HOUR, size: 1 },
            FileVersion { n: 2, label: None, description: None, auto: true, created: day + 9 * HOUR, size: 1 },
            FileVersion { n: 3, label: None, description: None, auto: true, created: day - DAY + HOUR, size: 1 },
            v(4, 31 * DAY, true, None, NOW),
        ];
        assert_eq!(retention(&es, NOW), vec![1, 4]);
    }

    #[test]
    fn named_versions_are_always_kept() {
        let es = vec![
            v(1, 40 * DAY, false, Some("Release"), NOW),
            v(2, 40 * DAY, true, Some("Renamed auto"), NOW),
            v(3, 41 * DAY, false, None, NOW),
            v(4, 40 * DAY, true, None, NOW),
        ];
        assert_eq!(retention(&es, NOW), vec![4]);
    }

    #[test]
    fn saving_an_auto_version_prunes_old_ones_on_disk() {
        let t = Tmp::new();
        let lib = Library::open(&t.0).unwrap();
        let now = NOW;
        lib.save_version_at("f", &doc_with(1), None, None, true, now - 40 * DAY).unwrap();
        lib.save_version_at("f", &doc_with(1), Some("Keep".into()), None, false, now - 40 * DAY).unwrap();
        lib.save_version_at("f", &doc_with(1), None, None, true, now - 5 * 60_000).unwrap();
        let ns: Vec<_> = lib.list_versions("f").iter().map(|e| e.n).collect();
        assert_eq!(ns, [3, 2], "the 40 day old auto version is gone");
        assert!(!t.0.join("versions/f/1.desigma").exists());
        assert!(t.0.join("versions/f/2.desigma").exists());
    }
}
