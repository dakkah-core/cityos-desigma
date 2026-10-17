//! Shared libraries on disk.
//!
//! ```text
//! libraries/<library-id>/<version>.json   one `LibrarySnapshot` per published version
//! libraries/<library-id>/index.json       name, versions with description and changelog
//! libraries/<library-id>/.publish.pending.json  durable publication commit intent
//! .publication.lock                     fail-fast OS lock for publication APIs
//! trash/libraries/<library-id>[-n]/       unpublished libraries (soft delete)
//! ```
//!
//! A file publishes under its own id, so republishing adds a version to the
//! same library.

use crate::store::remove_durable;
use crate::{write_atomic, Library, LibraryError, Result};
use desigma_doc::{diff_snapshots, snapshot, Document, LibraryAssetKind, LibrarySnapshot, PublishInfo, UpdateKind};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const INDEX: &str = "index.json";
const PENDING: &str = ".publish.pending.json";

/// A durable commit intent: once recorded, recovery rolls the entire publication
/// forward. Keep the exact predecessor bytes so replay cannot replace unrelated
/// metadata, a damaged historical index, or a newer publication.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingPublication {
    schema_version: String,
    previous_index: Option<String>,
    index: String,
    snapshot: String,
    digest: String,
}

impl PendingPublication {
    fn digest(&self) -> Result<String> {
        // Length-delimited JSON also protects the predecessor and metadata.
        Ok(crate::payload_digest(&serde_json::to_vec(&(&self.previous_index, &self.index, &self.snapshot))?))
    }
}

#[cfg(test)]
thread_local! {
    static PUBLISH_FAULT: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
    static PUBLISH_EXIT: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
    static PUBLISH_HOOK: std::cell::RefCell<Option<(&'static str, Box<dyn FnOnce()>)>> = const { std::cell::RefCell::new(None) };
}

fn publish_fault(_phase: &str) -> Result<()> {
    #[cfg(test)]
    {
        let hook = PUBLISH_HOOK.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.as_ref().is_some_and(|(phase, _)| *phase == _phase) { slot.take().map(|(_, hook)| hook) }
            else { None }
        });
        if let Some(hook) = hook { hook(); }
        if PUBLISH_EXIT.with(|point| point.get() == Some(_phase)) { std::process::exit(92); }
        if PUBLISH_FAULT.with(|point| {
            if point.get() == Some(_phase) { point.set(None); true } else { false }
        }) {
            return Err(std::io::Error::other(format!("Interrupted publication at {_phase}")).into());
        }
    }
    Ok(())
}

fn invalid_publication(message: &str) -> LibraryError {
    LibraryError::Invalid(format!("Invalid published library: {message}"))
}

fn optional_bytes(path: &Path) -> Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn validate_index(index: &LibraryIndex, id: &str) -> Result<()> {
    if index.id.is_empty() || safe_id(&index.id) != safe_id(id) || index.versions.is_empty() {
        return Err(invalid_publication("index identity or empty history"));
    }
    let mut previous = 0;
    for entry in &index.versions {
        if entry.version <= previous {
            return Err(invalid_publication("non-increasing version history"));
        }
        previous = entry.version;
    }
    Ok(())
}

fn read_snapshot(dir: &Path, id: &str, version: u32) -> Result<LibrarySnapshot> {
    let bytes = optional_bytes(&dir.join(format!("{version}.json")))?
        .ok_or_else(|| LibraryError::NotFound(format!("library {id} version {version}")))?;
    let snapshot: LibrarySnapshot = serde_json::from_slice(&bytes)?;
    if snapshot.library_id != id || snapshot.version != version {
        return Err(invalid_publication("snapshot identity does not match its index"));
    }
    Ok(snapshot)
}

/// Validate every referenced snapshot and reject unindexed versions. Never infer
/// descriptions, timestamps or source bindings from orphan snapshot files.
fn validate_history(dir: &Path, index: Option<&LibraryIndex>, pending: Option<u32>) -> Result<Option<LibrarySnapshot>> {
    let mut latest = None;
    let mut versions = std::collections::BTreeSet::new();
    if let Some(index) = index {
        for entry in &index.versions {
            latest = Some(read_snapshot(dir, &index.id, entry.version)?);
            versions.insert(entry.version);
        }
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        if let Some(version) = name.to_str().and_then(|name| name.strip_suffix(".json")).and_then(|name| name.parse::<u32>().ok()) {
            if !versions.contains(&version) && pending != Some(version) {
                return Err(invalid_publication("snapshot has no committed index entry"));
            }
        }
    }
    Ok(latest)
}

fn read_index(dir: &Path, id: &str) -> Result<LibraryIndex> {
    let bytes = optional_bytes(&dir.join(INDEX))?.ok_or_else(|| LibraryError::NotFound(format!("library {id}")))?;
    let index: LibraryIndex = serde_json::from_slice(&bytes)?;
    validate_index(&index, id)?;
    validate_history(dir, Some(&index), None)?;
    Ok(index)
}

fn changelog(previous: Option<&LibrarySnapshot>, next: &LibrarySnapshot) -> Vec<ChangeEntry> {
    diff_snapshots(previous, next).into_iter().map(|update| ChangeEntry {
        key: update.key,
        kind: update.kind,
        name: update.name,
        change: match update.change {
            UpdateKind::Added => "added",
            UpdateKind::Changed => "changed",
            UpdateKind::Removed => "removed",
        }.into(),
    }).collect()
}

fn update_index(index: &mut LibraryIndex, snapshot: &LibrarySnapshot, entry: VersionEntry) {
    index.name = snapshot.name.clone();
    index.versions.push(entry);
    index.components = snapshot.count(LibraryAssetKind::Component) + snapshot.count(LibraryAssetKind::ComponentSet);
    index.styles = snapshot.count(LibraryAssetKind::Style);
    index.variables = snapshot.count(LibraryAssetKind::Variable);
}

fn empty_index(id: &str) -> LibraryIndex {
    LibraryIndex {
        id: id.into(), name: String::new(), versions: Vec::new(), components: 0, styles: 0, variables: 0,
        source_path: None, source_file: None,
    }
}

/// The caller holds the root publication lock throughout validation and replay.
fn recover_publication(dir: &Path, id: &str) -> Result<()> {
    let Some(bytes) = optional_bytes(&dir.join(PENDING))? else { return Ok(()) };
    let pending: PendingPublication = serde_json::from_slice(&bytes)?;
    if pending.schema_version != "desigma.library-publication.v1" || pending.digest != pending.digest()? {
        return Err(invalid_publication("journal schema or digest"));
    }
    let next: LibraryIndex = serde_json::from_str(&pending.index)?;
    validate_index(&next, id)?;
    let snapshot: LibrarySnapshot = serde_json::from_str(&pending.snapshot)?;
    if snapshot.library_id != next.id || snapshot.name != next.name || snapshot.version != next.latest_version() {
        return Err(invalid_publication("journal snapshot identity"));
    }
    let previous: Option<LibraryIndex> = pending.previous_index.as_deref().map(serde_json::from_str).transpose()?;
    if let Some(previous) = &previous {
        validate_index(previous, id)?;
        if previous.id != next.id || snapshot.version <= previous.latest_version() {
            return Err(invalid_publication("journal predecessor identity or version"));
        }
    }
    let previous_snapshot = validate_history(dir, previous.as_ref(), Some(snapshot.version))?;
    let entry = next.latest().ok_or_else(|| invalid_publication("missing journal version"))?;
    if entry.changelog != changelog(previous_snapshot.as_ref(), &snapshot) {
        return Err(invalid_publication("journal changelog"));
    }
    let mut expected = previous.unwrap_or_else(|| empty_index(&next.id));
    update_index(&mut expected, &snapshot, entry.clone());
    if expected != next {
        return Err(invalid_publication("journal would replace historical metadata"));
    }
    let current = optional_bytes(&dir.join(INDEX))?;
    let is_committed = current.as_deref() == Some(pending.index.as_bytes());
    if !is_committed && current.as_deref() != pending.previous_index.as_deref().map(str::as_bytes) {
        return Err(invalid_publication("index differs from journal predecessor and commit"));
    }
    let snapshot_path = dir.join(format!("{}.json", snapshot.version));
    let existing_snapshot = optional_bytes(&snapshot_path)?;
    if existing_snapshot.as_deref().is_some_and(|bytes| bytes != pending.snapshot.as_bytes()) {
        return Err(invalid_publication("refusing to overwrite an existing snapshot"));
    }
    if is_committed && existing_snapshot.is_none() {
        return Err(invalid_publication("committed snapshot is missing"));
    }
    // All validation precedes the first target write. Replay is byte-idempotent.
    if existing_snapshot.is_none() { write_atomic(&snapshot_path, pending.snapshot.as_bytes())?; }
    publish_fault("snapshot")?;
    if !is_committed { write_atomic(&dir.join(INDEX), pending.index.as_bytes())?; }
    publish_fault("index")?;
    remove_durable(&dir.join(PENDING))?;
    publish_fault("completion")
}

/// One line of a version's changelog.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChangeEntry {
    pub key: String,
    pub kind: LibraryAssetKind,
    pub name: String,
    /// "added", "changed" or "removed".
    pub change: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VersionEntry {
    pub version: u32,
    pub description: String,
    /// RFC 3339, UTC.
    pub published_at: String,
    pub changelog: Vec<ChangeEntry>,
}

/// `index.json` of a published library.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LibraryIndex {
    pub id: String,
    pub name: String,
    pub versions: Vec<VersionEntry>,
    /// Asset counts of the latest version: components (incl. sets), styles, variables.
    #[serde(default)]
    pub components: usize,
    #[serde(default)]
    pub styles: usize,
    #[serde(default)]
    pub variables: usize,
    /// Path of the file this library was published from, when it is saved on
    /// this device (lets "Go to main component" open it).
    #[serde(default)]
    pub source_path: Option<String>,
    /// Id of the projects-library file this library was published from, when
    /// it lives in the same library root (opens through the home store).
    #[serde(default)]
    pub source_file: Option<String>,
}

impl LibraryIndex {
    pub fn latest(&self) -> Option<&VersionEntry> {
        self.versions.iter().max_by_key(|v| v.version)
    }

    pub fn latest_version(&self) -> u32 {
        self.latest().map(|v| v.version).unwrap_or(0)
    }
}

/// Characters allowed in a library id used as a directory name.
fn safe_id(s: &str) -> String {
    let id: String = s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
    if id.is_empty() {
        "library".into()
    } else {
        id
    }
}

/// Current UTC time as RFC 3339 ("2026-01-02T03:04:05Z").
pub fn now_rfc3339() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    // Civil-from-days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

impl Library {
    fn library_dir(&self, id: &str) -> PathBuf {
        self.path(format!("libraries/{}", safe_id(id)))
    }

    /// A stable lock outside movable library directories. OS locks release on
    /// process exit; try_lock never queues the UI behind a writer/recovery.
    /// All publication readers and mutators, including trash moves, participate.
    fn publication_lock(&self) -> Result<std::fs::File> {
        let file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false)
            .open(self.path(".publication.lock"))?;
        match file.try_lock() {
            Ok(()) => Ok(file),
            Err(std::fs::TryLockError::WouldBlock) => Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock, "Published library storage is busy; retry the operation",
            ).into()),
            Err(std::fs::TryLockError::Error(error)) => Err(error.into()),
        }
    }

    pub(crate) fn recover_pending_publications(&self) -> Result<()> {
        let _guard = self.publication_lock()?;
        let entries = match std::fs::read_dir(self.path("libraries")) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        for entry in entries {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                recover_publication(&entry.path(), &entry.file_name().to_string_lossy())?;
            }
        }
        Ok(())
    }

    /// Reads a complete, validated index, recovering any durable commit first.
    /// A busy writer or damaged entry is an error, never a partial catalogue.
    pub fn library_index(&self, id: &str) -> Result<LibraryIndex> {
        let _guard = self.publication_lock()?;
        let dir = self.library_dir(id);
        recover_publication(&dir, id)?;
        read_index(&dir, id)
    }

    /// Publishes `doc` as a new version of the library `file_id_or_name` (the
    /// id of the file, so the same file always updates the same library; an
    /// existing [`PublishInfo`] on the document wins). Returns the info to
    /// store in the file's library state.
    ///
    /// The durable journal commits the intent. An error after that point may
    /// still commit this version on recovery; read the index before retrying.
    /// Contention returns an I/O `WouldBlock` error without waiting.
    pub fn publish_library(&self, file_id_or_name: &str, doc: &Document, description: &str) -> Result<PublishInfo> {
        let _guard = self.publication_lock()?;
        let id = doc.library.published.as_ref().map(|p| p.library_id.clone()).filter(|s| !s.is_empty()).unwrap_or_else(|| safe_id(file_id_or_name));
        let dir = self.library_dir(&id);
        std::fs::create_dir_all(&dir)?;
        // Persist newly created directory links before the journal can commit.
        #[cfg(not(windows))]
        {
            std::fs::File::open(self.root())?.sync_all()?;
            std::fs::File::open(self.path("libraries"))?.sync_all()?;
        }
        recover_publication(&dir, &id)?;
        // Only an empty directory can safely start a new version sequence. An
        // unavailable index with persisted state must not overwrite snapshots.
        let mut index = match read_index(&dir, &id) {
            Ok(index) => index,
            Err(LibraryError::NotFound(_)) if std::fs::read_dir(&dir)?.next().transpose()?.is_none() => empty_index(&id),
            Err(error) => return Err(error),
        };
        // Legacy ids may have been mapped to a safe directory name. Preserve
        // their persisted identity when addressed through that directory alias.
        let id = index.id.clone();
        let previous_index = optional_bytes(&dir.join(INDEX))?.map(String::from_utf8)
            .transpose().map_err(|_| invalid_publication("index is not UTF-8"))?;
        let previous = if index.versions.is_empty() { None } else { Some(read_snapshot(&dir, &index.id, index.latest_version())?) };
        let version = index.latest_version().max(doc.library.published.as_ref().map(|p| p.version).unwrap_or(0))
            .checked_add(1).ok_or_else(|| invalid_publication("version sequence exhausted"))?;
        let name = if doc.name.is_empty() { file_id_or_name.to_string() } else { doc.name.clone() };
        let snap = snapshot(doc, &id, &name, version);
        let published_at = now_rfc3339();
        update_index(&mut index, &snap, VersionEntry {
            version, description: description.into(), published_at: published_at.clone(), changelog: changelog(previous.as_ref(), &snap),
        });
        let mut pending = PendingPublication {
            schema_version: "desigma.library-publication.v1".into(), previous_index,
            index: serde_json::to_string_pretty(&index)?, snapshot: serde_json::to_string(&snap)?, digest: String::new(),
        };
        pending.digest = pending.digest()?;
        publish_fault("before-journal")?;
        write_atomic(&dir.join(PENDING), &serde_json::to_vec(&pending)?)?;
        publish_fault("journal")?;
        recover_publication(&dir, &id)?;
        Ok(PublishInfo { library_id: id, name, version, description: description.into(), published_at })
    }

    /// Records where the library's source file lives on this device.
    pub fn set_library_source(&self, id: &str, path: Option<&str>) -> Result<()> {
        let _guard = self.publication_lock()?;
        let dir = self.library_dir(id);
        recover_publication(&dir, id)?;
        let mut index = read_index(&dir, id)?;
        index.source_path = path.map(String::from);
        write_atomic(&dir.join(INDEX), serde_json::to_string_pretty(&index)?.as_bytes())
    }

    /// Records the projects-library file the library was published from.
    pub fn set_library_source_file(&self, id: &str, file_id: Option<&str>) -> Result<()> {
        let _guard = self.publication_lock()?;
        let dir = self.library_dir(id);
        recover_publication(&dir, id)?;
        let mut index = read_index(&dir, id)?;
        index.source_file = file_id.map(String::from);
        write_atomic(&dir.join(INDEX), serde_json::to_string_pretty(&index)?.as_bytes())
    }

    /// Published libraries, by name. Unpublished (trashed) ones are not listed.
    pub fn list_libraries(&self) -> Result<Vec<LibraryIndex>> {
        let _guard = self.publication_lock()?;
        let mut out = Vec::new();
        let root = self.path("libraries");
        let entries = match std::fs::read_dir(root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(error) => return Err(error.into()),
        };
        for e in entries {
            let e = e?;
            if !e.file_type()?.is_dir() {
                continue;
            }
            let id = e.file_name();
            let id = id.to_string_lossy();
            recover_publication(&e.path(), &id)?;
            // A failed first preparation can leave an empty directory without
            // publishing anything. It must not hide other committed libraries.
            if std::fs::read_dir(e.path())?.next().transpose()?.is_none() { continue; }
            out.push(read_index(&e.path(), &id)?);
        }
        out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.id.cmp(&b.id)));
        Ok(out)
    }

    /// Loads one version of a library, the latest with `None`.
    pub fn load_snapshot(&self, id: &str, version: Option<u32>) -> Result<LibrarySnapshot> {
        let _guard = self.publication_lock()?;
        let dir = self.library_dir(id);
        recover_publication(&dir, id)?;
        let index = read_index(&dir, id)?;
        let version = version.unwrap_or_else(|| index.latest_version());
        if !index.versions.iter().any(|entry| entry.version == version) {
            return Err(LibraryError::NotFound(format!("library {id} version {version}")));
        }
        read_snapshot(&dir, &index.id, version)
    }

    /// Unpublishes a library: moves it to `trash/libraries/` (restorable with
    /// [`Library::restore_library`]). Files that enabled it keep their copies.
    pub fn unpublish(&self, id: &str) -> Result<()> {
        let _guard = self.publication_lock()?;
        let src = self.library_dir(id);
        recover_publication(&src, id)?;
        read_index(&src, id)?;
        if !src.is_dir() {
            return Err(LibraryError::NotFound(format!("library {id}")));
        }
        let trash = self.path("trash/libraries");
        std::fs::create_dir_all(&trash)?;
        let mut dst = trash.join(safe_id(id));
        let mut n = 1;
        while dst.exists() {
            dst = trash.join(format!("{}-{n}", safe_id(id)));
            n += 1;
        }
        std::fs::rename(src, dst)?;
        Ok(())
    }

    /// Restores the most recently unpublished copy of a library.
    pub fn restore_library(&self, id: &str) -> Result<()> {
        let _guard = self.publication_lock()?;
        let dst = self.library_dir(id);
        if dst.exists() {
            return Err(LibraryError::Invalid(format!("library {id} is already published")));
        }
        let trash = self.path("trash/libraries");
        let base = safe_id(id);
        let mut best = None;
        let mut n = 0;
        loop {
            let p = if n == 0 { trash.join(&base) } else { trash.join(format!("{base}-{n}")) };
            if p.exists() {
                best = Some(p);
            } else if n > 0 {
                break;
            }
            n += 1;
        }
        let src = best.ok_or_else(|| LibraryError::NotFound(format!("library {id} in trash")))?;
        recover_publication(&src, id)?;
        read_index(&src, id)?;
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(src, dst)?;
        Ok(())
    }
}

/// Publishes `doc` (see [`Library::publish_library`]).
pub fn publish(lib: &Library, file_id_or_name: &str, doc: &Document, description: &str) -> Result<PublishInfo> {
    lib.publish_library(file_id_or_name, doc, description)
}

#[cfg(test)]
mod tests {
    use super::*;
    use desigma_doc::*;
    use std::path::Path;

    fn tmp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("desigma-publish-{name}-{}", crate::new_id()));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    fn lib_doc() -> Document {
        let mut d = Document::new("Design System");
        let page = d.add_page("P");
        let id = d.alloc_id();
        let mut c = Node::new(id, NodeKind::Component(FrameProps::default()));
        c.name = "Button".into();
        c.size = Vec2::new(100.0, 40.0);
        let c = d.insert(page, c).unwrap();
        let tid = d.alloc_id();
        d.insert(c, Node::text(tid, Rect::new(0.0, 0.0, 40.0, 20.0), "Label", TextStyle::default())).unwrap();
        d.styles.insert("Brand".into(), Style::Fill(vec![Paint::solid(Color::rgb(255, 0, 0))]));
        d
    }

    struct PublishedRoot(PathBuf);

    impl Drop for PublishedRoot {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
    }

    fn seeded_publication() -> (PublishedRoot, Library, LibraryIndex, Vec<u8>, Vec<u8>) {
        let root = PublishedRoot(tmp("transaction"));
        let lib = Library::open(&root.0).unwrap();
        lib.publish_library("ds", &lib_doc(), "First description").unwrap();
        lib.publish_library("ds", &lib_doc(), "Second description").unwrap();
        lib.set_library_source("ds", Some("source.desigma")).unwrap();
        lib.set_library_source_file("ds", Some("source-file")).unwrap();
        let index = lib.library_index("ds").unwrap();
        let first = std::fs::read(lib.library_dir("ds").join("1.json")).unwrap();
        let second = std::fs::read(lib.library_dir("ds").join("2.json")).unwrap();
        (root, lib, index, first, second)
    }

    fn next_document() -> Document {
        let mut doc = lib_doc();
        doc.name = "Renamed Design System".into();
        doc.styles.insert("Brand".into(), Style::Fill(vec![Paint::solid(Color::rgb(0, 0, 255))]));
        doc
    }

    fn assert_recovered(lib: &Library, old: &LibraryIndex, first: &[u8], second: &[u8]) {
        let index = lib.library_index("ds").unwrap();
        assert_eq!(&index.versions[..2], old.versions.as_slice());
        assert_eq!(index.versions.len(), 3);
        assert_eq!(index.latest_version(), 3);
        assert_eq!(index.latest().unwrap().description, "Third description");
        assert_eq!(index.name, "Renamed Design System");
        assert_eq!(index.source_path, old.source_path);
        assert_eq!(index.source_file, old.source_file);
        assert_eq!((index.components, index.styles, index.variables), (1, 1, 0));
        assert_eq!(lib.load_snapshot("ds", None).unwrap().version, 3);
        let dir = lib.library_dir("ds");
        assert!(std::fs::read(dir.join("1.json")).unwrap().as_slice() == first, "version 1 bytes changed");
        assert!(std::fs::read(dir.join("2.json")).unwrap().as_slice() == second, "version 2 bytes changed");
        assert!(!dir.join(PENDING).exists());
    }

    #[test]
    fn publication_faults_recover_on_every_read_without_losing_metadata_or_reusing_versions() {
        for phase in ["before-journal", "journal", "snapshot", "index", "completion"] {
            for reader in ["open", "index", "latest", "explicit", "list"] {
                let (_root, lib, old, first, second) = seeded_publication();
                let index_bytes = std::fs::read(lib.library_dir("ds").join(INDEX)).unwrap();
                PUBLISH_FAULT.with(|point| point.set(Some(phase)));
                assert!(lib.publish_library("ds", &next_document(), "Third description").is_err(), "{phase}/{reader}");
                match reader {
                    "open" => { Library::open(lib.root()).unwrap(); }
                    "index" => { lib.library_index("ds").unwrap(); }
                    "latest" => { lib.load_snapshot("ds", None).unwrap(); }
                    "explicit" => { lib.load_snapshot("ds", Some(1)).unwrap(); }
                    "list" => { assert_eq!(lib.list_libraries().unwrap().len(), 1); }
                    _ => unreachable!(),
                }
                if phase == "before-journal" {
                    assert_eq!(lib.library_index("ds").unwrap(), old);
                    assert!(std::fs::read(lib.library_dir("ds").join(INDEX)).unwrap() == index_bytes);
                    assert!(!lib.library_dir("ds").join("3.json").exists());
                    assert_eq!(lib.publish_library("ds", &next_document(), "Third description").unwrap().version, 3);
                }
                assert_recovered(&lib, &old, &first, &second);
                let third = std::fs::read(lib.library_dir("ds").join("3.json")).unwrap();
                // A document without a saved PublishInfo must allocate version 4.
                assert_eq!(lib.publish_library("ds", &lib_doc(), "Fourth").unwrap().version, 4);
                assert!(std::fs::read(lib.library_dir("ds").join("3.json")).unwrap() == third, "{phase}/{reader}: version 3 overwritten");
            }
        }
    }

    #[test]
    fn first_publication_faults_commit_only_after_durable_journal() {
        for phase in ["before-journal", "journal", "snapshot", "index", "completion"] {
            let root = PublishedRoot(tmp("first-transaction"));
            let lib = Library::open(&root.0).unwrap();
            PUBLISH_FAULT.with(|point| point.set(Some(phase)));
            assert!(lib.publish_library("ds", &lib_doc(), "First description").is_err());
            let reopened = Library::open(&root.0).unwrap();
            if phase == "before-journal" {
                assert!(!reopened.library_dir("ds").join("1.json").exists());
                assert_eq!(reopened.publish_library("ds", &lib_doc(), "First description").unwrap().version, 1);
            }
            assert_eq!(reopened.library_index("ds").unwrap().latest_version(), 1);
            let first = std::fs::read(reopened.library_dir("ds").join("1.json")).unwrap();
            assert_eq!(reopened.publish_library("ds", &next_document(), "Second").unwrap().version, 2);
            assert!(std::fs::read(reopened.library_dir("ds").join("1.json")).unwrap() == first);
        }
    }

    #[test]
    fn failed_first_publication_before_journal_keeps_existing_library_listing_available() {
        let (_root, lib, old, first, second) = seeded_publication();
        PUBLISH_FAULT.with(|point| point.set(Some("before-journal")));
        assert!(lib.publish_library("uncommitted", &lib_doc(), "Not committed").is_err());
        assert_eq!(lib.list_libraries().unwrap(), vec![old.clone()]);
        assert!(std::fs::read(lib.library_dir("ds").join("1.json")).unwrap() == first);
        assert!(std::fs::read(lib.library_dir("ds").join("2.json")).unwrap() == second);
        assert_eq!(lib.publish_library("uncommitted", &lib_doc(), "First").unwrap().version, 1);
        assert_eq!(lib.list_libraries().unwrap().len(), 2);
    }

    #[test]
    #[ignore = "subprocess helper for publication_process_exits_recover_and_release_writer_lock"]
    fn publication_crash_worker() {
        let Some(root) = std::env::var_os("DESIGMA_PUBLICATION_TEST_ROOT") else { return };
        let lib = Library::open(root).unwrap();
        let phase = std::env::var("DESIGMA_PUBLICATION_TEST_PHASE").unwrap();
        if phase == "hold-lock" {
            let _guard = lib.publication_lock().unwrap();
            std::fs::write(lib.path("writer-ready"), b"ready").unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while !lib.path("release-writer").exists() {
                assert!(std::time::Instant::now() < deadline, "parent did not release writer");
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            std::process::exit(92);
        }
        let phase = match phase.as_str() {
            "before-journal" => "before-journal", "journal" => "journal", "snapshot" => "snapshot",
            "index" => "index", "completion" => "completion", _ => panic!("unexpected publication phase"),
        };
        PUBLISH_EXIT.with(|point| point.set(Some(phase)));
        lib.publish_library("ds", &next_document(), "Third description").unwrap();
        panic!("publication did not exit at {phase}");
    }

    fn crash_command(root: &Path, phase: &str) -> std::process::Command {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command.args(["--exact", "publish::tests::publication_crash_worker", "--ignored", "--test-threads=1"])
            .env("DESIGMA_PUBLICATION_TEST_ROOT", root).env("DESIGMA_PUBLICATION_TEST_PHASE", phase);
        command
    }

    #[test]
    fn publication_process_exits_recover_and_release_writer_lock() {
        for first_publish in [false, true] {
            for phase in ["before-journal", "journal", "snapshot", "index", "completion"] {
                let (root, lib, old, first, second) = seeded_publication();
                if first_publish { std::fs::remove_dir_all(lib.library_dir("ds")).unwrap(); }
                assert_eq!(crash_command(&root.0, phase).status().unwrap().code(), Some(92), "{phase}");
                let dir = lib.library_dir("ds");
                let expected = if first_publish { 1 } else { 3 };
                // Inspect actual on-disk boundary state before invoking recovery.
                assert_eq!(dir.join(PENDING).exists(), matches!(phase, "journal" | "snapshot" | "index"));
                assert_eq!(dir.join(format!("{expected}.json")).exists(), matches!(phase, "snapshot" | "index" | "completion"));
                let pending: Option<PendingPublication> = optional_bytes(&dir.join(PENDING)).unwrap()
                    .map(|bytes| serde_json::from_slice(&bytes).unwrap());
                let on_disk_index = optional_bytes(&dir.join(INDEX)).unwrap();
                if matches!(phase, "index" | "completion") {
                    assert_eq!(serde_json::from_slice::<LibraryIndex>(on_disk_index.as_deref().unwrap()).unwrap().latest_version(), expected);
                } else if first_publish { assert!(on_disk_index.is_none()); }
                else { assert_eq!(serde_json::from_slice::<LibraryIndex>(on_disk_index.as_deref().unwrap()).unwrap(), old); }
                let reopened = Library::open(&root.0).unwrap();
                if phase == "before-journal" {
                    assert_eq!(reopened.publish_library("ds", &next_document(), "Third description").unwrap().version, expected);
                }
                if first_publish {
                    assert_eq!(reopened.library_index("ds").unwrap().latest_version(), 1);
                    assert_eq!(reopened.load_snapshot("ds", Some(1)).unwrap().name, "Renamed Design System");
                } else { assert_recovered(&reopened, &old, &first, &second); }
                if let Some(pending) = pending {
                    assert!(std::fs::read(dir.join(INDEX)).unwrap() == pending.index.as_bytes(), "{phase}: committed index bytes changed");
                    assert!(std::fs::read(dir.join(format!("{expected}.json"))).unwrap() == pending.snapshot.as_bytes(), "{phase}: committed snapshot bytes changed");
                }
                let committed = std::fs::read(dir.join(format!("{expected}.json"))).unwrap();
                // Replay after a second restart, then publish without a hint.
                let reopened = Library::open(&root.0).unwrap();
                assert_eq!(reopened.publish_library("ds", &lib_doc(), "Next").unwrap().version, expected + 1);
                assert!(std::fs::read(dir.join(format!("{expected}.json"))).unwrap() == committed);
            }
        }
    }

    #[test]
    fn cross_process_writer_contention_returns_busy_and_exit_releases_lock() {
        let (root, lib, old, first, second) = seeded_publication();
        struct Worker(std::process::Child);
        impl Drop for Worker {
            fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); }
        }
        let mut child = Worker(crash_command(&root.0, "hold-lock").spawn().unwrap());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !lib.path("writer-ready").exists() {
            assert!(child.0.try_wait().unwrap().is_none(), "writer exited before locking");
            assert!(std::time::Instant::now() < deadline, "writer never acquired lock");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let busy = |error| assert!(matches!(error, LibraryError::Io(ref error) if error.kind() == std::io::ErrorKind::WouldBlock));
        busy(lib.publish_library("ds", &next_document(), "Third description").unwrap_err());
        busy(lib.library_index("ds").unwrap_err());
        busy(lib.list_libraries().unwrap_err());
        busy(lib.load_snapshot("ds", Some(1)).unwrap_err());
        busy(lib.set_library_source("ds", Some("wrong")).unwrap_err());
        busy(lib.set_library_source_file("ds", Some("wrong")).unwrap_err());
        busy(lib.unpublish("ds").unwrap_err());
        busy(lib.restore_library("ds").unwrap_err());
        busy(Library::open(&root.0).unwrap_err());
        std::fs::write(lib.path("release-writer"), b"release").unwrap();
        assert_eq!(child.0.wait().unwrap().code(), Some(92));
        assert_eq!(lib.library_index("ds").unwrap(), old);
        lib.publish_library("ds", &next_document(), "Third description").unwrap();
        assert_recovered(&lib, &old, &first, &second);
    }

    #[test]
    fn recovery_rejects_invalid_intent_conflicting_snapshots_and_changed_history_before_writes() {
        for damage in ["schema", "digest", "identity", "history", "source", "changelog", "counts", "snapshot", "index", "missing-index", "missing-history", "missing-committed-snapshot"] {
            let (_root, lib, _old, _first, _second) = seeded_publication();
            PUBLISH_FAULT.with(|point| point.set(Some(if damage == "missing-committed-snapshot" { "index" } else { "journal" })));
            assert!(lib.publish_library("ds", &next_document(), "Third description").is_err());
            let dir = lib.library_dir("ds");
            let path = dir.join(PENDING);
            let journal_bytes = std::fs::read(&path).unwrap();
            let mut journal: PendingPublication = serde_json::from_slice(&journal_bytes).unwrap();
            match damage {
                "schema" => journal.schema_version = "future".into(),
                "digest" => journal.digest = "invalid".into(),
                "snapshot" => std::fs::write(dir.join("3.json"), b"an existing version must never be replaced").unwrap(),
                "index" => std::fs::write(dir.join(INDEX), b"{truncated").unwrap(),
                "missing-index" => std::fs::remove_file(dir.join(INDEX)).unwrap(),
                "missing-history" => std::fs::remove_file(dir.join("1.json")).unwrap(),
                "missing-committed-snapshot" => std::fs::remove_file(dir.join("3.json")).unwrap(),
                _ => {
                    let mut next: LibraryIndex = serde_json::from_str(&journal.index).unwrap();
                    match damage {
                        "identity" => next.id = "other".into(),
                        "history" => next.versions[0].description = "lost history".into(),
                        "source" => next.source_file = None,
                        "changelog" => next.versions.last_mut().unwrap().changelog.clear(),
                        "counts" => next.styles += 1,
                        _ => unreachable!(),
                    }
                    journal.index = serde_json::to_string_pretty(&next).unwrap();
                    journal.digest = journal.digest().unwrap();
                }
            }
            std::fs::write(&path, serde_json::to_vec(&journal).unwrap()).unwrap();
            let before = publication_files(&dir);
            assert!(Library::open(lib.root()).is_err(), "{damage}");
            assert!(lib.library_index("ds").is_err(), "{damage}");
            assert!(lib.list_libraries().is_err(), "{damage}");
            assert!(lib.load_snapshot("ds", Some(1)).is_err(), "{damage}");
            assert!(lib.publish_library("ds", &lib_doc(), "blocked").is_err(), "{damage}");
            assert!(publication_files(&dir) == before, "{damage}: rejected recovery changed persisted bytes");
        }
    }

    fn publication_files(dir: &Path) -> std::collections::BTreeMap<std::ffi::OsString, Vec<u8>> {
        std::fs::read_dir(dir).unwrap().map(|entry| {
            let entry = entry.unwrap();
            (entry.file_name(), std::fs::read(entry.path()).unwrap())
        }).collect()
    }

    #[test]
    fn filesystem_index_failure_keeps_intent_and_snapshot_for_read_recovery() {
        let (_root, lib, old, first, second) = seeded_publication();
        let dir = lib.library_dir("ds");
        let index = std::fs::read(dir.join(INDEX)).unwrap();
        let blocked_dir = dir.clone();
        // Obstruct the actual index replacement after its predecessor has been
        // validated and the new snapshot is durable, forcing a real OS failure.
        PUBLISH_HOOK.with(|slot| *slot.borrow_mut() = Some(("snapshot", Box::new(move || {
            std::fs::remove_file(blocked_dir.join(INDEX)).unwrap();
            std::fs::create_dir(blocked_dir.join(INDEX)).unwrap();
        }))));
        assert!(matches!(lib.publish_library("ds", &next_document(), "Third description"), Err(LibraryError::Io(_))));
        let snapshot = std::fs::read(dir.join("3.json")).unwrap();
        let journal = std::fs::read(dir.join(PENDING)).unwrap();
        assert!(matches!(lib.list_libraries(), Err(LibraryError::Io(_))));
        assert!(std::fs::read(dir.join("3.json")).unwrap() == snapshot);
        assert!(std::fs::read(dir.join(PENDING)).unwrap() == journal);
        std::fs::remove_dir(dir.join(INDEX)).unwrap();
        std::fs::write(dir.join(INDEX), index).unwrap();
        assert_recovered(&lib, &old, &first, &second);
    }

    #[test]
    fn same_process_coordination_and_mutators_preserve_recovered_metadata() {
        let (_root, lib, old, first, second) = seeded_publication();
        let guard = lib.publication_lock().unwrap();
        let other = lib.clone();
        assert!(matches!(other.publish_library("ds", &next_document(), "blocked"),
            Err(LibraryError::Io(ref error)) if error.kind() == std::io::ErrorKind::WouldBlock));
        assert!(matches!(other.library_index("ds"),
            Err(LibraryError::Io(ref error)) if error.kind() == std::io::ErrorKind::WouldBlock));
        drop(guard);
        assert_eq!(lib.library_index("ds").unwrap(), old);
        PUBLISH_FAULT.with(|point| point.set(Some("journal")));
        assert!(lib.publish_library("ds", &next_document(), "Third description").is_err());
        PUBLISH_FAULT.with(|point| point.set(Some("snapshot")));
        assert!(lib.library_index("ds").is_err(), "a recovery failure must not return the old index");
        PUBLISH_FAULT.with(|point| point.set(Some("index")));
        assert!(lib.list_libraries().is_err(), "a recovery failure must not return a partial catalogue");
        // Mutators finish the pending publication before editing its metadata.
        lib.set_library_source("ds", Some("new-source.desigma")).unwrap();
        lib.set_library_source_file("ds", Some("new-source-file")).unwrap();
        lib.unpublish("ds").unwrap();
        assert!(lib.list_libraries().unwrap().is_empty());
        lib.restore_library("ds").unwrap();
        let mut expected = old;
        expected.source_path = Some("new-source.desigma".into());
        expected.source_file = Some("new-source-file".into());
        assert_recovered(&lib, &expected, &first, &second);
        assert_eq!(lib.publish_library("ds", &lib_doc(), "Fourth").unwrap().version, 4);
        let index = lib.library_index("ds").unwrap();
        assert_eq!(index.source_path, expected.source_path);
        assert_eq!(index.source_file, expected.source_file);
    }

    #[test]
    fn legacy_identity_alias_and_exhausted_version_hint_cannot_replace_existing_versions() {
        let root = PublishedRoot(tmp("legacy-identity"));
        let lib = Library::open(&root.0).unwrap();
        let mut doc = lib_doc();
        doc.library.published = Some(PublishInfo {
            library_id: "legacy/id".into(), name: doc.name.clone(), version: 0, description: String::new(), published_at: String::new(),
        });
        assert_eq!(lib.publish_library("ignored", &doc, "First").unwrap().version, 1);
        let second = lib.publish_library("legacy-id", &lib_doc(), "Second").unwrap();
        assert_eq!((second.library_id.as_str(), second.version), ("legacy/id", 2));
        assert_eq!(lib.load_snapshot("legacy-id", Some(2)).unwrap().library_id, "legacy/id");
        let before = publication_files(&lib.library_dir("legacy-id"));
        doc.library.published.as_mut().unwrap().version = u32::MAX;
        assert!(matches!(lib.publish_library("ignored", &doc, "exhausted"), Err(LibraryError::Invalid(_))));
        assert!(publication_files(&lib.library_dir("legacy-id")) == before, "overflow changed existing publication bytes");
    }

    #[test]
    fn publish_list_load_and_changelog() {
        let root = tmp("a");
        let lib = Library::open(&root).unwrap();
        let mut doc = lib_doc();
        let info = publish(&lib, "file-1", &doc, "First").unwrap();
        assert_eq!((info.library_id.as_str(), info.version), ("file-1", 1));
        assert!(info.published_at.ends_with('Z') && info.published_at.len() == 20);
        doc.library.published = Some(info.clone());
        let page = doc.pages[0];
        let comp = doc.get(page).unwrap().children[0];
        let mut n = doc.get(comp).unwrap().clone();
        n.size = Vec2::new(120.0, 40.0);
        Command::Update { node: Box::new(n) }.apply(&mut doc).unwrap();
        let info2 = publish(&lib, "ignored", &doc, "Wider button").unwrap();
        assert_eq!((info2.library_id.as_str(), info2.version), ("file-1", 2));

        let all = lib.list_libraries().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!((all[0].name.as_str(), all[0].latest_version(), all[0].components, all[0].styles), ("Design System", 2, 1, 1));
        let v1 = &all[0].versions[0];
        assert!(v1.changelog.iter().all(|c| c.change == "added") && v1.changelog.len() == 2);
        let v2 = &all[0].versions[1];
        assert_eq!(v2.description, "Wider button");
        assert_eq!(v2.changelog.len(), 1);
        assert_eq!((v2.changelog[0].name.as_str(), v2.changelog[0].change.as_str()), ("Button", "changed"));

        assert_eq!(lib.load_snapshot("file-1", None).unwrap().version, 2);
        assert_eq!(lib.load_snapshot("file-1", Some(1)).unwrap().version, 1);
        assert!(matches!(lib.load_snapshot("file-1", Some(9)), Err(LibraryError::NotFound(_))));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn unpublish_is_soft_delete() {
        let root = tmp("b");
        let lib = Library::open(&root).unwrap();
        publish(&lib, "f", &lib_doc(), "").unwrap();
        lib.unpublish("f").unwrap();
        assert!(lib.list_libraries().unwrap().is_empty());
        assert!(root.join("trash/libraries/f/index.json").exists());
        lib.restore_library("f").unwrap();
        assert_eq!(lib.list_libraries().unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(root);
    }

    /// Publish in one file, enable in another, edit, republish, update: instance
    /// overrides survive.
    #[test]
    fn round_trip_between_two_documents_preserves_overrides() {
        let root = tmp("c");
        let lib = Library::open(&root).unwrap();
        let mut a = lib_doc();
        let info = publish(&lib, "ds", &a, "v1").unwrap();
        a.library.published = Some(info);

        let snap = lib.load_snapshot("ds", None).unwrap();
        let mut b = Document::new("App");
        let page = b.add_page("Main");
        let key = snap.assets.iter().find(|x| x.name == "Button").unwrap().key.clone();
        let cmd = import_asset(&mut b, &snap, &key, Some(Placement { parent: page, at: Vec2::new(10.0, 10.0) }));
        cmd.apply(&mut b).unwrap();
        let inst = *b.get(page).unwrap().children.last().unwrap();
        let label = b.get(inst).unwrap().children[0];
        let mut n = b.get(label).unwrap().clone();
        if let NodeKind::Text(t) = &mut n.kind {
            t.content = "Pay".into();
        }
        n.component.overrides.insert(OverrideField::Characters);
        Command::Update { node: Box::new(n) }.apply(&mut b).unwrap();
        assert!(pending_updates(&b, &snap).is_empty());

        // The library author edits the component and republishes.
        let comp = a.get(a.pages[0]).unwrap().children[0];
        let mut cn = a.get(comp).unwrap().clone();
        cn.size = Vec2::new(160.0, 48.0);
        Command::Update { node: Box::new(cn) }.apply(&mut a).unwrap();
        let info = publish(&lib, "ds", &a, "wider").unwrap();
        assert_eq!(info.version, 2);

        let latest = lib.load_snapshot("ds", None).unwrap();
        let ups = pending_updates(&b, &latest);
        assert_eq!(ups.len(), 1);
        apply_updates(&mut b, &latest, &[ups[0].key.clone()]).apply(&mut b).unwrap();
        assert_eq!(b.get(inst).unwrap().size.x, 160.0);
        let NodeKind::Text(t) = &b.get(b.get(inst).unwrap().children[0]).unwrap().kind else { panic!() };
        assert_eq!(t.content, "Pay");
        assert_eq!(b.library.enabled[0].version, 2);
        // The consumer document survives a JSON round trip with its library state.
        let back = Document::from_json(&b.to_json().unwrap()).unwrap();
        assert_eq!(back.library, b.library);
        assert!(pending_updates(&back, &latest).is_empty());
        let _ = std::fs::remove_dir_all(root);
    }
}
