//! Projects, folders and files on disk, plus trash, recents, search and thumbnails.
//!
//! Everything here is plain `std::fs` and JSON; there is no database and no
//! account. A project is a directory (`projects/<id>/`) whose `project.json`
//! holds the project name, its folder tree and the index of its files. File
//! contents live next to it as `files/<file-id>.desigma`.
//!
//! Nothing is hard-deleted except by [`Library::empty_trash`]: deleting a file
//! or a project moves it to `trash/<id>/`, from where it can be restored.

use crate::{new_id, Library, LibraryError, Result};
use desigma_doc::{Document, DocumentCheckpoint, DocumentKind, Vec2};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

/// Name of the project created on first launch.
pub const SEED_PROJECT: &str = "Getting started";
const SEED_MARKER: &str = ".seeded";
const MANIFEST: &str = "project.json";
const TRASH_MANIFEST: &str = "trash.json";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Folder {
    pub id: String,
    pub name: String,
    /// `None` for a folder at the top of the project.
    pub parent: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub folders: Vec<Folder>,
    /// Milliseconds since the Unix epoch.
    pub created: u64,
    pub modified: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileEntry {
    pub id: String,
    pub project: String,
    pub folder: Option<String>,
    pub name: String,
    pub created: u64,
    pub modified: u64,
    pub opened: u64,
    #[serde(default)]
    pub starred: bool,
    /// PNG thumbnail, relative to the library root (resolve with [`Library::thumbnail_path`]).
    #[serde(default)]
    pub thumbnail: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum TrashKind {
    File,
    Project,
}

/// Something in the trash, restorable until the trash is emptied.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrashItem {
    pub id: String,
    pub kind: TrashKind,
    pub name: String,
    /// Milliseconds since the Unix epoch.
    pub trashed: u64,
    /// For files: the project they were deleted from.
    pub project: Option<String>,
    pub project_name: Option<String>,
    pub entry: Option<FileEntry>,
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    #[serde(flatten)]
    project: Project,
    #[serde(default)]
    files: Vec<FileEntry>,
}

/// Milliseconds since the epoch, strictly increasing within the process so
/// that "last opened" ordering is stable even for operations in the same ms.
pub fn now_ms() -> u64 {
    static LAST: AtomicU64 = AtomicU64::new(0);
    let t = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    let mut prev = LAST.load(Ordering::Relaxed);
    loop {
        let next = t.max(prev + 1);
        match LAST.compare_exchange_weak(prev, next, Ordering::SeqCst, Ordering::Relaxed) {
            Ok(_) => return next,
            Err(p) => prev = p,
        }
    }
}

static WRITE_LOCK: Mutex<()> = Mutex::new(());
static RECOVERY_LOCK: Mutex<()> = Mutex::new(());

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingSave {
    #[serde(default)]
    schema_version: Option<String>,
    file_id: String,
    document: String,
    manifest: Manifest,
    #[serde(default)]
    checkpoint: Option<String>,
    #[serde(default)]
    payloads: BTreeMap<String, Vec<u8>>,
    /// Durable CAS references plus any new objects carried by `payloads`.
    /// Absent only in legacy journals that predate explicit reference closure.
    #[serde(default)]
    required_pins: Option<Vec<String>>,
}

/// Release the OS handle before admitting the next process-local writer.
pub(crate) struct StorageGuard {
    _file: std::fs::File,
    _process: MutexGuard<'static, ()>,
}

/// Lock order: submission mutex (when needed), process writer, root OS lock,
/// then versions/recovery mutexes. Internal `_locked` helpers never reacquire
/// this guard. The stable file is outside project/version/trash directories.
pub(crate) fn write_lock(library: &Library) -> Result<StorageGuard> {
    let process = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // Do not create the parent: a late thumbnail must not resurrect a removed root.
    let file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(library.path(".storage.lock"))?;
    match file.try_lock() {
        Ok(()) => Ok(StorageGuard { _file: file, _process: process }),
        Err(std::fs::TryLockError::WouldBlock) => Err(LibraryError::StorageBusy),
        Err(std::fs::TryLockError::Error(error)) => Err(error.into()),
    }
}

/// Writes `bytes` to `path` via a temporary file in the same directory, so a
/// crash mid-write never leaves a truncated file behind.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    write_atomic_existing_parent(path, bytes)
}

fn write_atomic_existing_parent(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".tmp-{}", new_id()));
    let tmp = PathBuf::from(tmp);
    let res = (|| {
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        replace_atomic(&tmp, path)?;
        #[cfg(not(windows))]
        if let Some(parent) = path.parent() {
            std::fs::File::open(parent)?.sync_all()?;
        }
        Ok::<(), std::io::Error>(())
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    Ok(res?)
}

#[cfg(not(windows))]
fn replace_atomic(source: &Path, target: &Path) -> std::io::Result<()> {
    std::fs::rename(source, target)
}

#[cfg(windows)]
fn replace_atomic(source: &Path, target: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn MoveFileExW(existing: *const u16, replacement: *const u16, flags: u32) -> i32;
    }
    let wide = |path: &Path| -> std::io::Result<Vec<u16>> {
        use std::path::{Component, Prefix};
        if path.as_os_str().encode_wide().any(|unit| unit == 0) {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "NUL in persistence path"));
        }
        // std::fs handles long paths when creating the temporary file. Use the
        // same extended path form for the native replacement call, including
        // when only the unique temporary suffix crosses MAX_PATH.
        let absolute = std::path::absolute(path)?;
        let units: Vec<_> = absolute.as_os_str().encode_wide().collect();
        let mut value: Vec<u16> = match absolute.components().next() {
            Some(Component::Prefix(prefix)) => match prefix.kind() {
                Prefix::VerbatimDisk(_) | Prefix::VerbatimUNC(_, _) => units,
                Prefix::Disk(_) => r"\\?\".encode_utf16().chain(units).collect(),
                Prefix::UNC(_, _) => r"\\?\UNC\".encode_utf16().chain(units.into_iter().skip(2)).collect(),
                _ => return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Filesystem persistence path required")),
            },
            _ => return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Absolute persistence path required")),
        };
        value.push(0);
        Ok(value)
    };
    let source = wide(source)?;
    let target = wide(target)?;
    // Same-directory temps keep the move on one volume. Request durable replace
    // rather than relying on std::fs::rename's non-write-through Windows flags.
    // SAFETY: Both buffers are valid, NUL-terminated UTF-16 for this synchronous
    // call, remain alive, and are not retained by MoveFileExW.
    let moved = unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), 0x1 | 0x8) };
    if moved == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Persist deletion ordering as well as rename ordering on platforms that expose
/// directory fsync. Windows replacements request write-through; an interrupted
/// journal deletion may still require an idempotent replay after restart.
pub(crate) fn remove_durable(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => {
            #[cfg(not(windows))]
            if let Some(parent) = path.parent() {
                std::fs::File::open(parent)?.sync_all()?;
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
thread_local! {
    static SAVE_FAULT: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
    static SAVE_FAULT_REPEATS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static SAVE_EXIT: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
    static SAVE_HOOK: std::cell::RefCell<Option<SaveHook>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
type SaveHook = (&'static str, Box<dyn FnOnce()>);

#[cfg(test)]
pub(crate) fn inject_save_fault(phase: &'static str, repeats: usize) {
    SAVE_FAULT.with(|fault| fault.set(Some(phase)));
    SAVE_FAULT_REPEATS.with(|remaining| remaining.set(repeats));
}

#[cfg(test)]
pub(crate) fn exit_during_save(phase: &'static str) {
    SAVE_EXIT.with(|fault| fault.set(Some(phase)));
}

fn save_fault(_phase: &str) -> Result<()> {
    #[cfg(test)]
    if let Some(hook) = SAVE_HOOK.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.as_ref().is_some_and(|(phase, _)| *phase == _phase) {
            slot.take()
        } else {
            None
        }
    }) {
        hook.1();
    }
    #[cfg(test)]
    if SAVE_EXIT.with(|fault| fault.get() == Some(_phase)) {
        std::process::exit(92);
    }
    #[cfg(test)]
    if SAVE_FAULT.with(|fault| {
        if fault.get() == Some(_phase) {
            SAVE_FAULT_REPEATS.with(|remaining| {
                if remaining.get() == 0 {
                    fault.set(None);
                } else {
                    remaining.set(remaining.get() - 1);
                }
            });
            true
        } else {
            false
        }
    }) {
        return Err(std::io::Error::other(format!("Interrupted save at {_phase}")).into());
    }
    Ok(())
}

/// Loads a document from any importable file: `.desigma` / `.json` (native or
/// Figma REST), `.fig` or `.svg`.
pub fn load_external(path: &Path) -> Result<Document> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let invalid = |m: String| LibraryError::Invalid(m);
    let mut doc = match ext.as_str() {
        "fig" => desigma_fig::import_fig(&std::fs::read(path)?).map_err(|e| invalid(format!("Cannot import .fig: {e}")))?,
        "svg" => {
            let text = std::fs::read_to_string(path)?;
            let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("svg");
            let mut doc = Document::new(name);
            let page = doc.add_page("Page 1");
            desigma_svg::import_svg_named(&text, name, &mut doc, page, Vec2::ZERO).map_err(|e| invalid(format!("Cannot import SVG: {e}")))?;
            doc
        }
        "json" => {
            let text = std::fs::read_to_string(path)?;
            match desigma_fig::import_rest_json(&text) {
                Ok(d) => d,
                Err(fig) => Document::from_json(&text).map_err(|e| invalid(format!("Not a Figma REST file ({fig}) nor a desigma file ({e})")))?,
            }
        }
        _ => Document::from_json(&std::fs::read_to_string(path)?).map_err(|e| invalid(format!("Invalid desigma file: {e}")))?,
    };
    if doc.name.is_empty() {
        doc.name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Untitled").to_string();
    }
    doc.pages.retain(|p| doc.nodes.contains_key(p));
    if doc.pages.is_empty() {
        doc.add_page("Page 1");
    }
    Ok(doc)
}

fn clean_name(name: &str, what: &str) -> Result<String> {
    let n = name.trim();
    if n.is_empty() {
        return Err(LibraryError::Invalid(format!("The {what} name cannot be empty")));
    }
    Ok(n.to_string())
}

impl Library {
    /// `$DESIGMA_LIBRARY` if set (`$DESIGMA_LIBRARY_DIR` is accepted as a
    /// deprecated alias), otherwise `<data dir>/desigma/library`.
    pub fn default_root() -> PathBuf {
        for var in ["DESIGMA_LIBRARY", "DESIGMA_LIBRARY_DIR"] {
            if let Some(p) = std::env::var_os(var).filter(|p| !p.is_empty()) {
                return PathBuf::from(p);
            }
        }
        dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("desigma").join("library")
    }

    fn project_dir(&self, id: &str) -> PathBuf {
        self.path("projects").join(id)
    }
    fn file_path(&self, project: &str, file: &str) -> PathBuf {
        self.project_dir(project).join("files").join(format!("{file}.desigma"))
    }
    fn thumb_rel(project: &str, file: &str) -> PathBuf {
        PathBuf::from("projects").join(project).join("thumbs").join(format!("{file}.png"))
    }
    fn trash_dir(&self, id: &str) -> PathBuf {
        self.path("trash").join(id)
    }

    /// Caller holds the root/process storage guard, including during recovery.
    fn load_manifest_locked(&self, id: &str) -> Result<Manifest> {
        let _recovery = RECOVERY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        self.recover_project_save(id)?;
        let p = self.project_dir(id).join(MANIFEST);
        let text = std::fs::read_to_string(&p).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                LibraryError::NotFound(format!("project {id}"))
            } else {
                e.into()
            }
        })?;
        Ok(serde_json::from_str(&text)?)
    }

    fn save_manifest(&self, m: &Manifest) -> Result<()> {
        let p = self.project_dir(&m.project.id).join(MANIFEST);
        write_atomic(&p, serde_json::to_string_pretty(m)?.as_bytes())
    }

    fn recover_project_save(&self, project: &str) -> Result<()> {
        crate::validate_id(project)?;
        let journal = self.project_dir(project).join(".save.pending.json");
        let text = match std::fs::read_to_string(&journal) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        let pending: PendingSave = serde_json::from_str(&text)?;
        if pending.schema_version.as_deref().is_some_and(|version| !["desigma.file-save.v2", "desigma.file-save.v3"].contains(&version)) {
            return Err(LibraryError::Invalid("Unsupported pending save version".into()));
        }
        crate::validate_id(&pending.file_id)?;
        let mut ids = BTreeSet::new();
        if pending.manifest.project.id != project || !pending.manifest.files.iter().any(|f| f.id == pending.file_id) {
            return Err(LibraryError::Invalid("Invalid pending document save".into()));
        }
        for file in &pending.manifest.files {
            crate::validate_id(&file.id)?;
            if file.project != project || !ids.insert(&file.id) {
                return Err(LibraryError::Invalid("Invalid pending manifest identity".into()));
            }
        }
        let document = Document::from_json(&pending.document)?;
        let checkpoint = pending.checkpoint.as_deref().map(DocumentCheckpoint::from_json).transpose()?;
        if let Some(checkpoint) = &checkpoint {
            if checkpoint.document.to_json()? != document.to_json()? {
                return Err(LibraryError::Invalid("Pending checkpoint/document mismatch".into()));
            }
        }
        let roots: BTreeSet<_> = checkpoint
            .as_ref()
            .map(DocumentCheckpoint::dependency_pins)
            .unwrap_or_else(|| DocumentCheckpoint::new(document.clone(), desigma_doc::History::default()).dependency_pins())
            .into_iter()
            .collect();
        let required: BTreeSet<_> = pending.required_pins.as_ref().map(|pins| pins.iter().cloned().collect()).unwrap_or_else(|| {
            if checkpoint.is_some() {
                roots.clone()
            } else {
                pending.payloads.keys().cloned().collect()
            }
        });
        if (pending.schema_version.as_deref() == Some("desigma.file-save.v3") && pending.required_pins.is_none())
            || !required.is_subset(&roots)
            || (checkpoint.is_some() && required != roots)
        {
            return Err(LibraryError::Invalid("Pending dependency reference closure mismatch".into()));
        }
        // Validate the entire transaction before changing any target. Old journals
        // may refer to payloads already durable in the library.
        for (hash, bytes) in &pending.payloads {
            if crate::payload_digest(bytes) != *hash || !required.contains(hash) {
                return Err(LibraryError::Invalid("Invalid pending dependency payload".into()));
            }
        }
        for hash in &required {
            if !pending.payloads.contains_key(hash) {
                self.verify_cityos_payload(hash)?;
            }
        }
        for (hash, bytes) in &pending.payloads {
            self.put_cityos_payload(hash, bytes)?;
        }
        save_fault("payloads")?;
        write_atomic(&self.file_path(project, &pending.file_id), pending.document.as_bytes())?;
        save_fault("document")?;
        let checkpoint_path = self.checkpoint_path(&pending.file_id)?;
        if let Some(checkpoint) = &pending.checkpoint {
            write_atomic(&checkpoint_path, checkpoint.as_bytes())?;
        } else {
            remove_durable(&checkpoint_path)?;
        }
        save_fault("checkpoint")?;
        self.save_manifest(&pending.manifest)?;
        save_fault("manifest")?;
        remove_durable(&journal)?;
        save_fault("complete")?;
        Ok(())
    }

    fn commit_document(&self, file_id: &str, doc: &Document, manifest: Manifest) -> Result<()> {
        // Legacy native saves preserve unresolved pins (including remote-only
        // drafts). Complete checkpoints/export deliberately require their bytes.
        self.commit_document_state(file_id, doc, None, BTreeMap::new(), manifest)
    }

    fn commit_document_state(
        &self,
        file_id: &str,
        doc: &Document,
        checkpoint: Option<String>,
        payloads: BTreeMap<String, Vec<u8>>,
        manifest: Manifest,
    ) -> Result<()> {
        let _recovery = RECOVERY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let project = manifest.project.id.clone();
        crate::validate_id(&project)?;
        crate::validate_id(file_id)?;
        // Never replace an earlier durable intent with a later save after an I/O
        // error. Recovery completes it first; the caller retries from fresh state.
        if self.project_dir(&project).join(".save.pending.json").exists() {
            return Err(LibraryError::Invalid("A pending save must be recovered before retrying".into()));
        }
        let document = doc.to_json()?;
        Document::from_json(&document)?;
        let state = checkpoint.as_deref().map(DocumentCheckpoint::from_json).transpose()?;
        let roots: BTreeSet<_> = state
            .as_ref()
            .map(DocumentCheckpoint::dependency_pins)
            .unwrap_or_else(|| DocumentCheckpoint::new(doc.clone(), desigma_doc::History::default()).dependency_pins())
            .into_iter()
            .collect();
        if payloads.iter().any(|(hash, bytes)| !roots.contains(hash) || crate::payload_digest(bytes) != *hash) {
            return Err(LibraryError::Invalid("Checkpoint dependency closure mismatch".into()));
        }
        let mut supplied = payloads;
        let mut payloads = BTreeMap::new();
        let mut required_pins = Vec::new();
        for hash in roots {
            match self.verify_cityos_payload(&hash) {
                Ok(()) => {
                    required_pins.push(hash);
                }
                Err(LibraryError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                    if let Some(bytes) = supplied.remove(&hash) {
                        payloads.insert(hash.clone(), bytes);
                        required_pins.push(hash);
                    } else if state.is_some() {
                        return Err(error.into());
                    }
                }
                Err(error @ LibraryError::Invalid(_)) => {
                    // A verified portable copy can repair a corrupt CAS object.
                    // Retain its bytes in the intent before touching the object,
                    // just as for a missing dependency; ordinary saves still fail.
                    if let Some(bytes) = supplied.remove(&hash) {
                        payloads.insert(hash.clone(), bytes);
                        required_pins.push(hash);
                    } else {
                        return Err(error);
                    }
                }
                Err(error) => return Err(error),
            }
        }
        drop(supplied);
        let pending = PendingSave {
            schema_version: Some("desigma.file-save.v3".into()),
            file_id: file_id.into(),
            document,
            manifest,
            checkpoint,
            payloads,
            required_pins: Some(required_pins),
        };
        save_fault("before-journal")?;
        write_atomic(&self.project_dir(&project).join(".save.pending.json"), serde_json::to_string(&pending)?.as_bytes())?;
        save_fault("journal")?;
        self.recover_project_save(&project)
    }

    /// Saves the complete native document, undo/redo, inactive baseline histories,
    /// dependency closure and project manifest under one replayable durable intent.
    /// Existing verified CAS objects are references; missing or corrupt objects
    /// carry verified replacement bytes when supplied by a portable import.
    /// On an I/O error after intent creation, reopen/reload to reconcile before
    /// deciding whether the old or new candidate is current. No owner action occurs.
    pub fn save_checkpoint(&self, id: &str, checkpoint: &DocumentCheckpoint) -> Result<()> {
        self.save_checkpoint_payloads(id, checkpoint, BTreeMap::new())
    }

    pub(crate) fn save_checkpoint_payloads(&self, id: &str, checkpoint: &DocumentCheckpoint, payloads: BTreeMap<String, Vec<u8>>) -> Result<()> {
        let _guard = write_lock(self)?;
        self.save_checkpoint_payloads_locked(id, checkpoint, payloads)
    }

    pub(crate) fn save_checkpoint_payloads_locked(&self, id: &str, checkpoint: &DocumentCheckpoint, payloads: BTreeMap<String, Vec<u8>>) -> Result<()> {
        crate::validate_id(id)?;
        let checkpoint_json = checkpoint.to_json()?;
        DocumentCheckpoint::from_json(&checkpoint_json)?;
        let roots: BTreeSet<_> = checkpoint.dependency_pins().into_iter().collect();
        if payloads.iter().any(|(hash, bytes)| !roots.contains(hash) || crate::payload_digest(bytes) != *hash) {
            return Err(LibraryError::Invalid("Checkpoint dependency closure mismatch".into()));
        }
        self.check_checkpoint_lineage_locked(id, checkpoint)?;
        let (mut manifest, i) = self.find_file_locked(id)?;
        let now = now_ms();
        manifest.files[i].modified = now;
        manifest.project.modified = now;
        self.commit_document_state(id, &checkpoint.document, Some(checkpoint_json), payloads, manifest)
    }

    pub(crate) fn recover_pending_saves_locked(&self) -> Result<()> {
        let projects = self.path("projects");
        if !projects.exists() {
            return Ok(());
        }
        for entry in std::fs::read_dir(projects)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                let id = entry.file_name().to_string_lossy().into_owned();
                let _guard = RECOVERY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
                self.recover_project_save(&id)?;
            }
        }
        Ok(())
    }

    /// Finds the manifest holding `file` and the file's index in it.
    fn find_file_locked(&self, file: &str) -> Result<(Manifest, usize)> {
        for p in self.list_projects_locked()? {
            let m = self.load_manifest_locked(&p.id)?;
            if let Some(i) = m.files.iter().position(|f| f.id == file) {
                return Ok((m, i));
            }
        }
        Err(LibraryError::NotFound(format!("file {file}")))
    }

    // ------------------------------------------------------------ projects

    /// All projects, oldest first. Use `try_list_projects` to distinguish busy
    /// or damaged storage from an empty library (legacy convenience API).
    pub fn list_projects(&self) -> Vec<Project> {
        self.try_list_projects().unwrap_or_default()
    }

    /// Recover and read a consistent project catalogue, preserving busy/errors.
    pub fn try_list_projects(&self) -> Result<Vec<Project>> {
        let _guard = write_lock(self)?;
        self.list_projects_locked()
    }

    fn list_projects_locked(&self) -> Result<Vec<Project>> {
        let rd = match std::fs::read_dir(self.path("projects")) {
            Ok(rd) => rd,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut out = Vec::new();
        for entry in rd {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                let id = entry.file_name().to_string_lossy().into_owned();
                out.push(self.load_manifest_locked(&id)?.project);
            }
        }
        out.sort_by(|a, b| a.created.cmp(&b.created).then_with(|| a.name.cmp(&b.name)));
        Ok(out)
    }

    pub fn project(&self, id: &str) -> Result<Project> {
        let _guard = write_lock(self)?;
        Ok(self.load_manifest_locked(id)?.project)
    }

    pub fn create_project(&self, name: &str) -> Result<Project> {
        let _guard = write_lock(self)?;
        self.create_project_locked(name)
    }

    fn create_project_locked(&self, name: &str) -> Result<Project> {
        let name = clean_name(name, "project")?;
        let now = now_ms();
        let project = Project { id: new_id(), name, folders: vec![], created: now, modified: now };
        self.save_manifest(&Manifest { project: project.clone(), files: vec![] })?;
        Ok(project)
    }

    pub fn rename_project(&self, id: &str, name: &str) -> Result<()> {
        let _guard = write_lock(self)?;
        let name = clean_name(name, "project")?;
        let mut m = self.load_manifest_locked(id)?;
        m.project.name = name;
        m.project.modified = now_ms();
        self.save_manifest(&m)
    }

    /// Moves the whole project to the trash (it can be restored).
    pub fn delete_project(&self, id: &str) -> Result<()> {
        let _guard = write_lock(self)?;
        let m = self.load_manifest_locked(id)?;
        let dir = self.trash_dir(id);
        std::fs::create_dir_all(&dir)?;
        let item = TrashItem {
            id: id.to_string(),
            kind: TrashKind::Project,
            name: m.project.name.clone(),
            trashed: now_ms(),
            project: None,
            project_name: None,
            entry: None,
        };
        write_atomic(&dir.join(TRASH_MANIFEST), serde_json::to_string_pretty(&item)?.as_bytes())?;
        let dest = dir.join("project");
        let _ = std::fs::remove_dir_all(&dest);
        std::fs::rename(self.project_dir(id), dest)?;
        Ok(())
    }

    // ------------------------------------------------------------ folders

    pub fn create_folder(&self, project: &str, parent: Option<&str>, name: &str) -> Result<Folder> {
        let _guard = write_lock(self)?;
        let name = clean_name(name, "folder")?;
        let mut m = self.load_manifest_locked(project)?;
        if let Some(p) = parent {
            if !m.project.folders.iter().any(|f| f.id == p) {
                return Err(LibraryError::NotFound(format!("folder {p}")));
            }
        }
        let folder = Folder { id: new_id(), name, parent: parent.map(str::to_string) };
        m.project.folders.push(folder.clone());
        m.project.modified = now_ms();
        self.save_manifest(&m)?;
        Ok(folder)
    }

    pub fn rename_folder(&self, project: &str, folder: &str, name: &str) -> Result<()> {
        let _guard = write_lock(self)?;
        let name = clean_name(name, "folder")?;
        let mut m = self.load_manifest_locked(project)?;
        let f = m.project.folders.iter_mut().find(|f| f.id == folder).ok_or_else(|| LibraryError::NotFound(format!("folder {folder}")))?;
        f.name = name;
        self.save_manifest(&m)
    }

    /// Re-parents a folder (`None` = top level). A folder cannot move into itself or its own subtree.
    pub fn move_folder(&self, project: &str, folder: &str, new_parent: Option<&str>) -> Result<()> {
        let _guard = write_lock(self)?;
        let mut m = self.load_manifest_locked(project)?;
        if !m.project.folders.iter().any(|f| f.id == folder) {
            return Err(LibraryError::NotFound(format!("folder {folder}")));
        }
        let mut cur = new_parent.map(str::to_string);
        while let Some(c) = cur {
            if c == folder {
                return Err(LibraryError::Invalid("A folder cannot be moved into itself".into()));
            }
            cur = m.project.folders.iter().find(|f| f.id == c).and_then(|f| f.parent.clone());
        }
        if let Some(p) = new_parent {
            if !m.project.folders.iter().any(|f| f.id == p) {
                return Err(LibraryError::NotFound(format!("folder {p}")));
            }
        }
        if let Some(f) = m.project.folders.iter_mut().find(|f| f.id == folder) {
            f.parent = new_parent.map(str::to_string);
        }
        self.save_manifest(&m)
    }

    // ------------------------------------------------------------ files

    /// Every file in every project. Use `try_all_files` to preserve busy/errors.
    pub fn all_files(&self) -> Vec<FileEntry> {
        self.try_all_files().unwrap_or_default()
    }

    pub fn try_all_files(&self) -> Result<Vec<FileEntry>> {
        let _guard = write_lock(self)?;
        let mut files = Vec::new();
        for project in self.list_projects_locked()? {
            files.extend(self.load_manifest_locked(&project.id)?.files);
        }
        Ok(files)
    }

    /// Files of one project; with `folder` only that folder's direct children (`Some(None)` = top level).
    pub fn files_in(&self, project: &str, folder: Option<Option<&str>>) -> Result<Vec<FileEntry>> {
        let _guard = write_lock(self)?;
        let m = self.load_manifest_locked(project)?;
        Ok(m.files.into_iter().filter(|f| folder.is_none_or(|want| f.folder.as_deref() == want)).collect())
    }

    pub fn file(&self, id: &str) -> Result<FileEntry> {
        let _guard = write_lock(self)?;
        let (m, i) = self.find_file_locked(id)?;
        Ok(m.files[i].clone())
    }

    /// Resolve a selected path to its existing managed identity, without loading
    /// it as an external document. Missing or unindexed files return `None`.
    /// Recovery and catalogue inspection retain the same root/process guard.
    pub fn file_at_path(&self, path: &Path) -> Result<Option<FileEntry>> {
        let _guard = write_lock(self)?;
        let selected = match std::fs::canonicalize(path) {
            Ok(path) => path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if !selected.is_file() {
            return Err(LibraryError::Invalid("A regular document file is required".into()));
        }
        let root = std::fs::canonicalize(self.root())?;
        let entries = match std::fs::read_dir(self.path("projects")) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let mut found = None;
        let mut ids = BTreeSet::new();
        for entry in entries {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_symlink() {
                return Err(LibraryError::Invalid("Managed project paths cannot be links".into()));
            }
            if !kind.is_dir() {
                continue;
            }
            let project = entry.file_name().into_string().map_err(|_| LibraryError::Invalid("Invalid project path".into()))?;
            crate::validate_id(&project)?;
            let manifest = self.load_manifest_locked(&project)?;
            if manifest.project.id != project {
                return Err(LibraryError::Invalid("Project manifest identity does not match its path".into()));
            }
            for file in manifest.files {
                crate::validate_id(&file.id)?;
                if file.project != project || !ids.insert(file.id.clone()) {
                    return Err(LibraryError::Invalid("Invalid or duplicate managed file identity".into()));
                }
                let owned = match std::fs::canonicalize(self.file_path(&project, &file.id)) {
                    Ok(path) => path,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(error.into()),
                };
                if !owned.starts_with(root.join("projects").join(&project).join("files")) || !owned.is_file() {
                    return Err(LibraryError::Invalid("Managed document path escapes its project files".into()));
                }
                if selected == owned || cfg!(windows) && selected.to_string_lossy().eq_ignore_ascii_case(&owned.to_string_lossy()) {
                    if found.is_some() {
                        return Err(LibraryError::Invalid("Selected path has multiple managed identities".into()));
                    }
                    found = Some(file);
                }
            }
        }
        Ok(found)
    }

    pub fn create_file(&self, project: &str, folder: Option<&str>, name: &str, doc: &Document) -> Result<FileEntry> {
        self.create_file_state(project, folder, name, doc, None)
    }

    /// Creates the file and its first complete history checkpoint in the same
    /// document/manifest transaction; no intermediate history-free file is needed.
    pub fn create_file_with_history(
        &self,
        project: &str,
        folder: Option<&str>,
        name: &str,
        doc: &Document,
        history: &desigma_doc::History,
    ) -> Result<FileEntry> {
        self.create_file_checkpoint(project, folder, name, &DocumentCheckpoint::new(doc.clone(), history.clone()))
    }

    /// Creates a file including any histories retained across earlier baselines.
    pub fn create_file_checkpoint(&self, project: &str, folder: Option<&str>, name: &str, checkpoint: &DocumentCheckpoint) -> Result<FileEntry> {
        let text = checkpoint.to_json()?;
        DocumentCheckpoint::from_json(&text)?;
        self.create_file_state(project, folder, name, &checkpoint.document, Some((text, BTreeMap::new())))
    }

    pub(crate) fn create_file_state(
        &self,
        project: &str,
        folder: Option<&str>,
        name: &str,
        doc: &Document,
        checkpoint: Option<(String, BTreeMap<String, Vec<u8>>)>,
    ) -> Result<FileEntry> {
        let _guard = write_lock(self)?;
        self.create_file_state_locked(project, folder, name, doc, checkpoint)
    }

    pub(crate) fn create_file_state_locked(
        &self,
        project: &str,
        folder: Option<&str>,
        name: &str,
        doc: &Document,
        checkpoint: Option<(String, BTreeMap<String, Vec<u8>>)>,
    ) -> Result<FileEntry> {
        let name = clean_name(name, "file")?;
        let mut m = self.load_manifest_locked(project)?;
        if let Some(f) = folder {
            if !m.project.folders.iter().any(|x| x.id == f) {
                return Err(LibraryError::NotFound(format!("folder {f}")));
            }
        }
        let now = now_ms();
        let entry = FileEntry {
            id: new_id(),
            project: project.to_string(),
            folder: folder.map(str::to_string),
            name,
            created: now,
            modified: now,
            opened: now,
            starred: false,
            thumbnail: None,
        };
        m.files.push(entry.clone());
        m.project.modified = now;
        if let Some((text, payloads)) = checkpoint {
            self.commit_document_state(&entry.id, doc, Some(text), payloads, m)?;
        } else {
            self.commit_document(&entry.id, doc, m)?;
        }
        Ok(entry)
    }

    /// Overwrites the file's contents atomically and bumps its modified time.
    pub fn save_file(&self, id: &str, doc: &Document) -> Result<()> {
        let _guard = write_lock(self)?;
        if self.load_checkpoint_state_locked(id)?.history.baseline_id().is_some() {
            return Err(LibraryError::CheckpointConflict(id.into()));
        }
        let (mut m, i) = self.find_file_locked(id)?;
        let now = now_ms();
        m.files[i].modified = now;
        m.project.modified = now;
        self.commit_document(id, doc, m)
    }

    /// Whether the file is a design file or a board, read from the document itself (the manifest
    /// has no field for it). Parses the file without building a document, so it is cheap enough
    /// to call once per file while listing.
    pub fn file_kind(&self, id: &str) -> Result<DocumentKind> {
        let _guard = write_lock(self)?;
        #[derive(Deserialize)]
        struct Probe {
            #[serde(default)]
            kind: DocumentKind,
        }
        let (m, _) = self.find_file_locked(id)?;
        let bytes = std::fs::read(self.file_path(&m.project.id, id))?;
        if serde_json::from_slice::<serde_json::Value>(&bytes)?.get("schemaVersion").is_some() {
            return Ok(Document::from_json(&String::from_utf8_lossy(&bytes))?.kind);
        }
        Ok(serde_json::from_slice::<Probe>(&bytes)?.kind)
    }

    pub fn load_file(&self, id: &str) -> Result<Document> {
        let _guard = write_lock(self)?;
        self.load_file_locked(id)
    }

    pub(crate) fn load_file_locked(&self, id: &str) -> Result<Document> {
        let (m, _) = self.find_file_locked(id)?;
        let text = std::fs::read_to_string(self.file_path(&m.project.id, id))?;
        Ok(Document::from_json(&text)?)
    }

    fn update_file(&self, id: &str, f: impl FnOnce(&mut FileEntry)) -> Result<()> {
        let _guard = write_lock(self)?;
        let (mut m, i) = self.find_file_locked(id)?;
        f(&mut m.files[i]);
        self.save_manifest(&m)
    }

    pub fn rename_file(&self, id: &str, name: &str) -> Result<()> {
        let name = clean_name(name, "file")?;
        self.update_file(id, |e| e.name = name)
    }

    /// Records that the file was just opened (drives "Recents").
    pub fn mark_opened(&self, id: &str) -> Result<()> {
        self.update_file(id, |e| e.opened = now_ms())
    }

    pub fn star(&self, id: &str, starred: bool) -> Result<()> {
        self.update_file(id, |e| e.starred = starred)
    }

    /// Duplicate an ordinary checkpoint. Bound CityOS identities require an
    /// explicit successor contract and cannot become eligible through a copy.
    pub fn duplicate_file(&self, id: &str) -> Result<FileEntry> {
        self.duplicate_file_with_policy(&crate::UNMANAGED_STORAGE_POLICY, id)
    }
    pub fn duplicate_file_with_policy(&self, policy: &dyn crate::CheckpointStoragePolicy, id: &str) -> Result<FileEntry> {
        let _submissions = crate::submissions::SUBMISSION_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let _guard = write_lock(self)?;
        let (m, i) = self.find_file_locked(id)?;
        let src = m.files[i].clone();
        let checkpoint = self.load_checkpoint_locked(id)?;
        let states =
            std::iter::once((&checkpoint.document, &checkpoint.history)).chain(checkpoint.retained.iter().map(|state| (&state.document, &state.history)));
        if states.flat_map(|(document, history)| std::iter::once(document).chain(history.retained_documents())).any(|document| document.is_managed()) {
            return Err(LibraryError::Invalid("Bound CityOS documents cannot be duplicated without an explicit successor contract".into()));
        }
        self.ensure_portable_submission_safe_locked(policy, Some(id), &checkpoint)?;
        let mut new = self.create_file_state_locked(
            &src.project,
            src.folder.as_deref(),
            &format!("{} (copy)", src.name),
            &checkpoint.document,
            Some((checkpoint.to_json()?, BTreeMap::new())),
        )?;
        // The document, both history stacks and retained archives are durable
        // before best-effort presentation work. Thumbnail failure cannot undo or
        // orphan that primary generation or replace its manifest with an old one.
        if let Some(t) = &src.thumbnail {
            if let Ok(png) = std::fs::read(self.path(t)) {
                if self.set_thumbnail_locked(&new.id, &png, || {}).is_ok() {
                    new.thumbnail = Some(Self::thumb_rel(&src.project, &new.id));
                }
            }
        }
        Ok(new)
    }

    /// Moves a file to another project and/or folder.
    pub fn move_file(&self, id: &str, project: &str, folder: Option<&str>) -> Result<()> {
        let _guard = write_lock(self)?;
        let (mut src, i) = self.find_file_locked(id)?;
        if src.project.id == project {
            if let Some(f) = folder {
                if !src.project.folders.iter().any(|x| x.id == f) {
                    return Err(LibraryError::NotFound(format!("folder {f}")));
                }
            }
            src.files[i].folder = folder.map(str::to_string);
            return self.save_manifest(&src);
        }
        let mut dst = self.load_manifest_locked(project)?;
        if let Some(f) = folder {
            if !dst.project.folders.iter().any(|x| x.id == f) {
                return Err(LibraryError::NotFound(format!("folder {f}")));
            }
        }
        let mut entry = src.files.remove(i);
        let from = entry.project.clone();
        if let Some(parent) = self.file_path(project, id).parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(self.file_path(&from, id), self.file_path(project, id))?;
        if let Some(old) = entry.thumbnail.take() {
            let rel = Self::thumb_rel(project, id);
            if let Some(parent) = self.path(&rel).parent() {
                std::fs::create_dir_all(parent)?;
            }
            if std::fs::rename(self.path(old), self.path(&rel)).is_ok() {
                entry.thumbnail = Some(rel);
            }
        }
        entry.project = project.to_string();
        entry.folder = folder.map(str::to_string);
        dst.files.push(entry);
        let now = now_ms();
        dst.project.modified = now;
        src.project.modified = now;
        self.save_manifest(&dst)?;
        self.save_manifest(&src)
    }

    /// Stores a PNG thumbnail for the file.
    pub fn set_thumbnail(&self, id: &str, png: &[u8]) -> Result<()> {
        self.set_thumbnail_before_write(id, png, || {})
    }

    fn set_thumbnail_before_write(&self, id: &str, png: &[u8], before_write: impl FnOnce()) -> Result<()> {
        let _guard = write_lock(self)?;
        self.set_thumbnail_locked(id, png, before_write)
    }

    fn set_thumbnail_locked(&self, id: &str, png: &[u8], before_write: impl FnOnce()) -> Result<()> {
        let (mut m, i) = self.find_file_locked(id)?;
        before_write();
        let rel = Self::thumb_rel(&m.project.id, id);
        // A delayed renderer must not recreate a deleted/moved project from its
        // stale manifest. Create only the immediate thumbnail directory, then
        // use writes that require the project directory to remain present.
        let path = self.path(&rel);
        if let Some(parent) = path.parent() {
            match std::fs::create_dir(parent) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e.into()),
            }
        }
        write_atomic_existing_parent(&path, png)?;
        m.files[i].thumbnail = Some(rel);
        write_atomic_existing_parent(&self.project_dir(&m.project.id).join(MANIFEST), &serde_json::to_vec_pretty(&m)?)
    }

    /// Absolute path of an entry's thumbnail, if it has one on disk.
    pub fn thumbnail_path(&self, entry: &FileEntry) -> Option<PathBuf> {
        entry.thumbnail.as_ref().map(|r| self.path(r)).filter(|p| p.exists())
    }

    /// Copies a `.desigma` file, or converts a `.fig` / `.json` / `.svg`, into a project.
    pub fn import_file(&self, path: &Path, project: &str, folder: Option<&str>) -> Result<FileEntry> {
        self.import_file_with_policy(&crate::UNMANAGED_STORAGE_POLICY, path, project, folder)
    }
    pub fn import_file_with_policy(&self, policy: &dyn crate::CheckpointStoragePolicy, path: &Path, project: &str, folder: Option<&str>) -> Result<FileEntry> {
        if let Some(portable) = crate::VerifiedPortableCheckpoint::read_with_policy(policy, path)? {
            return self.import_checkpoint_as_new_with_policy(policy, project, folder, portable);
        }
        let doc = load_external(path)?;
        let stem = path.file_stem().and_then(|s| s.to_str()).filter(|s| !s.is_empty());
        let name = if !doc.name.is_empty() { doc.name.clone() } else { stem.unwrap_or("Imported").to_string() };
        self.create_file(project, folder, &name, &doc)
    }

    // ------------------------------------------------------------ recents / search

    /// Most recently opened files first.
    pub fn recents(&self, limit: usize) -> Vec<FileEntry> {
        let mut v = self.all_files();
        v.sort_by(|a, b| b.opened.cmp(&a.opened).then_with(|| b.modified.cmp(&a.modified)));
        v.truncate(limit);
        v
    }

    pub fn starred(&self) -> Vec<FileEntry> {
        let mut v: Vec<FileEntry> = self.all_files().into_iter().filter(|f| f.starred).collect();
        v.sort_by_key(|f| std::cmp::Reverse(f.opened));
        v
    }

    /// Case-insensitive substring match over file names, most recently modified first.
    pub fn search(&self, query: &str) -> Vec<FileEntry> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return vec![];
        }
        let mut v: Vec<FileEntry> = self.all_files().into_iter().filter(|f| f.name.to_lowercase().contains(&q)).collect();
        v.sort_by_key(|f| std::cmp::Reverse(f.modified));
        v
    }

    // ------------------------------------------------------------ trash

    /// Moves a file to the trash.
    pub fn trash_file(&self, id: &str) -> Result<()> {
        let _guard = write_lock(self)?;
        let (mut m, i) = self.find_file_locked(id)?;
        let entry = m.files[i].clone();
        let dir = self.trash_dir(id);
        std::fs::create_dir_all(&dir)?;
        let item = TrashItem {
            id: id.to_string(),
            kind: TrashKind::File,
            name: entry.name.clone(),
            trashed: now_ms(),
            project: Some(m.project.id.clone()),
            project_name: Some(m.project.name.clone()),
            entry: Some(entry.clone()),
        };
        write_atomic(&dir.join(TRASH_MANIFEST), serde_json::to_string_pretty(&item)?.as_bytes())?;
        std::fs::rename(self.file_path(&m.project.id, id), dir.join("file.desigma"))?;
        if let Some(t) = &entry.thumbnail {
            let _ = std::fs::rename(self.path(t), dir.join("thumb.png"));
        }
        m.files.remove(i);
        m.project.modified = now_ms();
        self.save_manifest(&m)
    }

    /// Everything in the trash. Use `try_list_trash` to preserve busy/errors.
    pub fn list_trash(&self) -> Vec<TrashItem> {
        self.try_list_trash().unwrap_or_default()
    }

    pub fn try_list_trash(&self) -> Result<Vec<TrashItem>> {
        let _guard = write_lock(self)?;
        self.list_trash_locked()
    }

    fn list_trash_locked(&self) -> Result<Vec<TrashItem>> {
        let rd = match std::fs::read_dir(self.path("trash")) {
            Ok(rd) => rd,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut v: Vec<TrashItem> =
            rd.flatten().filter_map(|e| std::fs::read_to_string(e.path().join(TRASH_MANIFEST)).ok()).filter_map(|t| serde_json::from_str(&t).ok()).collect();
        v.sort_by_key(|t| std::cmp::Reverse(t.trashed));
        Ok(v)
    }

    /// Restores a trashed file into its original project (and folder, if it still exists).
    pub fn restore_file(&self, id: &str) -> Result<FileEntry> {
        let _guard = write_lock(self)?;
        self.restore_file_locked(id)
    }

    fn restore_file_locked(&self, id: &str) -> Result<FileEntry> {
        let dir = self.trash_dir(id);
        let item: TrashItem =
            serde_json::from_str(&std::fs::read_to_string(dir.join(TRASH_MANIFEST)).map_err(|_| LibraryError::NotFound(format!("trashed file {id}")))?)?;
        let (Some(project), Some(mut entry)) = (item.project.clone(), item.entry.clone()) else {
            return Err(LibraryError::Invalid("Not a trashed file".into()));
        };
        let mut m = self.load_manifest_locked(&project).map_err(|_| {
            LibraryError::Invalid(format!("Its project \"{}\" is not available; restore the project first", item.project_name.unwrap_or_default()))
        })?;
        if entry.folder.as_ref().is_some_and(|f| !m.project.folders.iter().any(|x| &x.id == f)) {
            entry.folder = None;
        }
        if let Some(parent) = self.file_path(&project, id).parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(dir.join("file.desigma"), self.file_path(&project, id))?;
        entry.thumbnail = None;
        if dir.join("thumb.png").exists() {
            let rel = Self::thumb_rel(&project, id);
            if let Some(parent) = self.path(&rel).parent() {
                std::fs::create_dir_all(parent)?;
            }
            if std::fs::rename(dir.join("thumb.png"), self.path(&rel)).is_ok() {
                entry.thumbnail = Some(rel);
            }
        }
        m.files.push(entry.clone());
        m.project.modified = now_ms();
        self.save_manifest(&m)?;
        let _ = std::fs::remove_dir_all(dir);
        Ok(entry)
    }

    pub fn restore_project(&self, id: &str) -> Result<Project> {
        let _guard = write_lock(self)?;
        self.restore_project_locked(id)
    }

    fn restore_project_locked(&self, id: &str) -> Result<Project> {
        let dir = self.trash_dir(id);
        if !dir.join("project").exists() {
            return Err(LibraryError::NotFound(format!("trashed project {id}")));
        }
        if self.project_dir(id).exists() {
            return Err(LibraryError::Invalid("A project with this id already exists".into()));
        }
        std::fs::create_dir_all(self.path("projects"))?;
        std::fs::rename(dir.join("project"), self.project_dir(id))?;
        let _ = std::fs::remove_dir_all(dir);
        Ok(self.load_manifest_locked(id)?.project)
    }

    /// Restores whatever the trash item `id` is.
    pub fn restore(&self, id: &str) -> Result<()> {
        let _guard = write_lock(self)?;
        match self.list_trash_locked()?.into_iter().find(|t| t.id == id) {
            Some(t) if t.kind == TrashKind::Project => self.restore_project_locked(id).map(|_| ()),
            Some(_) => self.restore_file_locked(id).map(|_| ()),
            None => Err(LibraryError::NotFound(format!("trash item {id}"))),
        }
    }

    /// Permanently deletes everything in the trash. This is the only hard delete in the library.
    pub fn empty_trash(&self) -> Result<()> {
        let _guard = write_lock(self)?;
        let _versions = crate::versions::lock();
        let dir = self.path("trash");
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        Ok(())
    }

    // ------------------------------------------------------------ first run

    /// On the very first launch creates the "Getting started" project holding `demo()`.
    /// Returns the created file; later launches (even after the project was
    /// deleted) do nothing.
    pub fn seed_first_run(&self, demo: impl FnOnce() -> Document) -> Result<Option<FileEntry>> {
        let _guard = write_lock(self)?;
        let marker = self.path(SEED_MARKER);
        if marker.exists() || !self.list_projects_locked()?.is_empty() {
            return Ok(None);
        }
        let project = self.create_project_locked(SEED_PROJECT)?;
        let mut doc = demo();
        doc.name = "Welcome".into();
        let entry = self.create_file_state_locked(&project.id, None, "Welcome", &doc, None)?;
        write_atomic(&marker, b"1")?;
        Ok(Some(entry))
    }
}

/// Shared subprocess barriers for storage regressions. Readiness, rather than a
/// scheduling delay, proves that the child is inside the transaction under test.
#[cfg(test)]
pub(crate) mod storage_test {
    use super::*;
    use std::time::{Duration, Instant};

    pub(crate) struct Worker {
        child: std::process::Child,
        root: PathBuf,
    }

    impl Worker {
        pub(crate) fn start(root: &Path, worker: &str, mode: &str, file: &str) -> Self {
            let child = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", worker, "--ignored", "--test-threads=1"])
                .env("DESIGMA_STORAGE_TEST_ROOT", root)
                .env("DESIGMA_STORAGE_TEST_MODE", mode)
                .env("DESIGMA_STORAGE_TEST_FILE", file)
                .spawn()
                .unwrap();
            let mut worker = Self { child, root: root.to_owned() };
            let deadline = Instant::now() + Duration::from_secs(10);
            while !root.join("storage-test-ready").exists() {
                assert!(worker.child.try_wait().unwrap().is_none(), "storage worker exited before its barrier");
                assert!(Instant::now() < deadline, "storage worker did not reach its barrier");
                std::thread::sleep(Duration::from_millis(10));
            }
            worker
        }

        pub(crate) fn release_and_wait(&mut self, code: i32) {
            std::fs::write(self.root.join("storage-test-release"), b"release").unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if let Some(status) = self.child.try_wait().unwrap() {
                    assert_eq!(status.code(), Some(code));
                    return;
                }
                assert!(Instant::now() < deadline, "storage worker did not exit after release");
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    impl Drop for Worker {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    pub(crate) fn pause(root: &Path) {
        std::fs::write(root.join("storage-test-ready"), b"ready").unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        while !root.join("storage-test-release").exists() {
            assert!(Instant::now() < deadline, "parent did not release storage worker");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    pub(crate) fn busy<T>(result: Result<T>) {
        match result {
            Err(LibraryError::StorageBusy) => {}
            Err(error) => panic!("expected explicit storage contention, got {error}"),
            Ok(_) => panic!("operation entered another process's storage transaction"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use desigma_doc::{Node, Rect};

    #[test]
    #[ignore = "subprocess helper for cross_process_checkpoint_storage_exclusion"]
    fn checkpoint_storage_worker() {
        let Some(root) = std::env::var_os("DESIGMA_STORAGE_TEST_ROOT") else { return };
        let file = std::env::var("DESIGMA_STORAGE_TEST_FILE").unwrap();
        let mode = std::env::var("DESIGMA_STORAGE_TEST_MODE").unwrap();
        let library = Library::open(root).unwrap();
        let (phase, crash) = match mode.as_str() {
            "before-journal" => ("before-journal", false),
            "journal" => ("journal", true),
            "document" => ("document", true),
            _ => panic!("unexpected storage worker mode"),
        };
        let mut checkpoint = library.load_checkpoint(&file).unwrap();
        checkpoint.document.name = "child committed".into();
        checkpoint.history.exec(&mut checkpoint.document, desigma_doc::Command::SetComments { comments: vec![] }).unwrap();
        let barrier = library.root().to_owned();
        SAVE_HOOK.with(|slot| {
            *slot.borrow_mut() = Some((
                phase,
                Box::new(move || {
                    storage_test::pause(&barrier);
                    if crash {
                        std::process::exit(94);
                    }
                }),
            ))
        });
        library.save_checkpoint_reconciled(&file, &checkpoint).unwrap();
        std::process::exit(93);
    }

    #[test]
    fn cross_process_checkpoint_storage_exclusion() {
        for mode in ["before-journal", "journal", "document"] {
            let (temp, library) = Tmp::new();
            let project = library.create_project("P").unwrap();
            let checkpoint = DocumentCheckpoint::new(doc("old", 1), desigma_doc::History::default());
            let file = library.create_file_checkpoint(&project.id, None, "F", &checkpoint).unwrap();
            let version = library.save_version(&file.id, &checkpoint.document, Some("Keep".into()), false).unwrap();
            let mut worker = storage_test::Worker::start(&temp.0, "store::tests::checkpoint_storage_worker", mode, &file.id);
            let paths = [
                library.project_dir(&project.id).join(MANIFEST),
                library.file_path(&project.id, &file.id),
                library.checkpoint_path(&file.id).unwrap(),
                library.project_dir(&project.id).join(".save.pending.json"),
            ];
            let before: Vec<_> = paths.iter().map(std::fs::read).collect();
            let fingerprint = crate::checkpoint_fingerprint(&checkpoint).unwrap();
            storage_test::busy(library.save_file_with_history_expected(&file.id, &checkpoint.document, &checkpoint.history, &fingerprint));
            storage_test::busy(library.save_checkpoint_reconciled_expected(&file.id, &checkpoint, &fingerprint));
            storage_test::busy(library.create_folder(&project.id, None, "Concurrent folder"));
            storage_test::busy(library.duplicate_file(&file.id));
            storage_test::busy(library.create_file(&project.id, None, "Concurrent file", &doc("other", 0)));
            storage_test::busy(library.star(&file.id, true));
            storage_test::busy(library.set_thumbnail(&file.id, b"png"));
            storage_test::busy(library.load_file(&file.id));
            storage_test::busy(library.load_checkpoint(&file.id));
            storage_test::busy(library.load_checkpoint_for_recovery(&file.id));
            storage_test::busy(library.files_in(&project.id, None));
            storage_test::busy(library.file(&file.id));
            storage_test::busy(library.file_at_path(&library.file_path(&project.id, &file.id)));
            storage_test::busy(library.project(&project.id));
            storage_test::busy(library.try_list_projects());
            storage_test::busy(library.try_all_files());
            storage_test::busy(library.try_list_trash());
            storage_test::busy(library.export_checkpoint_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &file.id));
            storage_test::busy(library.retention_roots_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY));
            storage_test::busy(library.try_list_versions(&file.id));
            storage_test::busy(library.load_version(&file.id, version.n));
            storage_test::busy(library.save_version(&file.id, &checkpoint.document, None, false));
            storage_test::busy(Library::open(&temp.0));
            for (path, bytes) in paths.iter().zip(before) {
                match bytes {
                    Ok(bytes) => assert_eq!(std::fs::read(path).unwrap(), bytes, "busy operation changed {}", path.display()),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => assert!(!path.exists()),
                    Err(error) => panic!("cannot snapshot {}: {error}", path.display()),
                }
            }
            worker.release_and_wait(if mode == "before-journal" { 93 } else { 94 });
            let reopened = Library::open(&temp.0).unwrap();
            let saved = reopened.load_checkpoint(&file.id).unwrap();
            assert_eq!(saved.document.name, "child committed");
            assert!(saved.history.can_undo());
            reopened.create_folder(&project.id, None, "Concurrent folder").unwrap();
            reopened.star(&file.id, true).unwrap();
            assert_eq!(reopened.project(&project.id).unwrap().folders.len(), 1);
            assert!(reopened.file(&file.id).unwrap().starred);
            assert_eq!(reopened.load_checkpoint(&file.id).unwrap().to_json().unwrap(), saved.to_json().unwrap());
            assert!(!library.project_dir(&project.id).join(".save.pending.json").exists());
        }
    }

    struct Tmp(PathBuf);
    impl Tmp {
        fn new() -> (Self, Library) {
            let p = std::env::temp_dir().join(format!("desigma_lib_{}_{}", std::process::id(), new_id()));
            let lib = Library::open(&p).unwrap();
            (Tmp(p), lib)
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn doc(name: &str, rects: usize) -> Document {
        let mut d = Document::new(name);
        let page = d.add_page("P");
        for i in 0..rects {
            let id = d.alloc_id();
            d.insert(page, Node::rectangle(id, Rect::new(i as f32, 0.0, 10.0, 10.0))).unwrap();
        }
        d
    }

    fn cas_checkpoint(pins: Vec<String>) -> DocumentCheckpoint {
        let mut document = doc("CAS checkpoint", 1);
        document.cityos = Some(desigma_doc::CityosBinding {
            schema_version: "cityos.design-exchange.v1".into(),
            document_id: "d".into(),
            draft_id: "draft".into(),
            base: desigma_doc::CityosBase::CanonicalGit { repository_id: "repo".into(), commit: "a".repeat(40) },
            semantic_base: serde_json::json!({}),
            dependency_pins: pins,
            occurrences: BTreeMap::new(),
            exchange_v2: None,
            exchange_v3: None,
        });
        DocumentCheckpoint::new(document, desigma_doc::History::default())
    }

    #[test]
    fn file_at_path_resolves_owned_identity_and_leaves_unindexed_native_paths_external() {
        let (temp, library) = Tmp::new();
        let loose = temp.0.join("loose.desigma");
        std::fs::write(&loose, doc("unindexed", 0).to_json().unwrap()).unwrap();
        assert!(library.file_at_path(&loose).unwrap().is_none(), "an existing root need not contain any managed files");
        assert!(library.file_at_path(&temp.0.join("missing.desigma")).unwrap().is_none());
        let project = library.create_project("P").unwrap();
        let file = library.create_file_checkpoint(&project.id, None, "Managed", &cas_checkpoint(Vec::new())).unwrap();
        let native = library.file_path(&project.id, &file.id);
        let alias = native.parent().unwrap().join("..").join("files").join(native.file_name().unwrap());
        let original = std::fs::read(library.checkpoint_path(&file.id).unwrap()).unwrap();
        assert_eq!(library.file_at_path(&native).unwrap(), Some(file.clone()));
        assert_eq!(library.file_at_path(&alias).unwrap(), Some(file.clone()));
        assert!(library.file_at_path(&loose).unwrap().is_none());
        assert_eq!(std::fs::read(library.checkpoint_path(&file.id).unwrap()).unwrap(), original);
        // Malformed manifest IDs must error rather than turning a managed open
        // into an external load through a path assembled from untrusted fields.
        let manifest = library.project_dir(&project.id).join(MANIFEST);
        let mut value: serde_json::Value = serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
        value["files"][0]["id"] = serde_json::json!("../escape");
        std::fs::write(&manifest, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(matches!(library.file_at_path(&native), Err(LibraryError::Invalid(_))));
    }

    fn ordinary_duplicate_checkpoint() -> DocumentCheckpoint {
        let mut document = doc("previous baseline", 1);
        document.images.insert("archived image".into(), vec![9, 8, 7]);
        let mut history = desigma_doc::History::default();
        let id = document.nodes[&document.pages[0]].children[0];
        let mut node = document.get(id).unwrap().clone();
        node.name = "archived edit".into();
        history.exec(&mut document, desigma_doc::Command::Update { node: Box::new(node) }).unwrap();
        let mut checkpoint = DocumentCheckpoint::new(document, history);
        checkpoint.install_baseline(doc("current baseline", 1));
        checkpoint.document.images.insert("current image".into(), vec![1, 2, 3]);
        let id = checkpoint.document.nodes[&checkpoint.document.pages[0]].children[0];
        for name in ["first edit", "second edit"] {
            let mut node = checkpoint.document.get(id).unwrap().clone();
            node.name = name.into();
            checkpoint.history.exec(&mut checkpoint.document, desigma_doc::Command::Update { node: Box::new(node) }).unwrap();
        }
        checkpoint.history.undo(&mut checkpoint.document).unwrap();
        checkpoint
    }

    #[test]
    fn duplicate_preserves_complete_ordinary_checkpoint_metadata_and_thumbnail_after_reopen() {
        let (temp, library) = Tmp::new();
        let project = library.create_project("P").unwrap();
        let folder = library.create_folder(&project.id, None, "Folder").unwrap();
        let checkpoint = ordinary_duplicate_checkpoint();
        let source = library.create_file_checkpoint(&project.id, Some(&folder.id), "Source", &checkpoint).unwrap();
        library.star(&source.id, true).unwrap();
        library.set_thumbnail(&source.id, b"source png").unwrap();
        let document_bytes = std::fs::read(library.file_path(&project.id, &source.id)).unwrap();
        let checkpoint_bytes = std::fs::read(library.checkpoint_path(&source.id).unwrap()).unwrap();
        let copy = library.duplicate_file(&source.id).unwrap();
        assert_ne!(copy.id, source.id);
        assert_eq!(copy.name, "Source (copy)");
        assert_eq!(copy.project, project.id);
        assert_eq!(copy.folder, Some(folder.id));
        assert!(!copy.starred);
        assert!(copy.created > source.created);
        assert_eq!(copy.created, copy.modified);
        assert_eq!(copy.created, copy.opened);
        assert_eq!(std::fs::read(library.thumbnail_path(&copy).unwrap()).unwrap(), b"source png");
        drop(library);
        let reopened = Library::open(&temp.0).unwrap();
        let mut restored = reopened.load_checkpoint(&copy.id).unwrap();
        assert_eq!(restored.to_json().unwrap(), checkpoint.to_json().unwrap());
        assert!(restored.history.baseline_id().is_some());
        assert!(restored.history.can_undo() && restored.history.can_redo());
        assert_eq!(restored.retained.len(), 1);
        assert!(restored.retained[0].history.can_undo());
        assert_eq!(restored.retained[0].document.images["archived image"], [9, 8, 7]);
        let id = restored.document.nodes[&restored.document.pages[0]].children[0];
        restored.history.redo(&mut restored.document).unwrap();
        assert_eq!(restored.document.get(id).unwrap().name, "second edit");
        restored.history.undo(&mut restored.document).unwrap();
        assert_eq!(restored.document.get(id).unwrap().name, "first edit");
        assert_eq!(std::fs::read(reopened.file_path(&project.id, &source.id)).unwrap(), document_bytes);
        assert_eq!(std::fs::read(reopened.checkpoint_path(&source.id).unwrap()).unwrap(), checkpoint_bytes);
        assert!(reopened.file(&source.id).unwrap().starred);
    }

    #[test]
    fn duplicate_missing_thumbnail_does_not_discard_the_durable_primary_checkpoint() {
        let (_temp, library) = Tmp::new();
        let project = library.create_project("P").unwrap();
        let checkpoint = ordinary_duplicate_checkpoint();
        let source = library.create_file_checkpoint(&project.id, None, "Source", &checkpoint).unwrap();
        library.set_thumbnail(&source.id, b"png").unwrap();
        let source = library.file(&source.id).unwrap();
        std::fs::remove_file(library.thumbnail_path(&source).unwrap()).unwrap();
        let copy = library.duplicate_file(&source.id).unwrap();
        assert!(copy.thumbnail.is_none());
        assert_eq!(library.load_checkpoint(&copy.id).unwrap().to_json().unwrap(), checkpoint.to_json().unwrap());
        assert_eq!(library.file(&copy.id).unwrap(), copy);
        assert_eq!(library.files_in(&project.id, None).unwrap().len(), 2);
    }

    #[test]
    fn duplicate_checkpoint_faults_recover_zero_or_one_complete_copy_at_every_save_boundary() {
        for phase in ["before-journal", "journal", "payloads", "document", "checkpoint", "manifest", "complete"] {
            let (temp, library) = Tmp::new();
            let project = library.create_project("P").unwrap();
            let checkpoint = ordinary_duplicate_checkpoint();
            let source = library.create_file_checkpoint(&project.id, None, "Source", &checkpoint).unwrap();
            let document_bytes = std::fs::read(library.file_path(&project.id, &source.id)).unwrap();
            let checkpoint_bytes = std::fs::read(library.checkpoint_path(&source.id).unwrap()).unwrap();
            inject_save_fault(phase, 0);
            assert!(library.duplicate_file(&source.id).is_err(), "{phase}");
            drop(library);
            let reopened = Library::open(&temp.0).unwrap();
            let copies: Vec<_> = reopened.files_in(&project.id, None).unwrap().into_iter().filter(|file| file.id != source.id).collect();
            if phase == "before-journal" {
                assert!(copies.is_empty());
            } else {
                assert_eq!(copies.len(), 1, "{phase}");
                assert_eq!(copies[0].name, "Source (copy)");
                assert_eq!(reopened.load_checkpoint(&copies[0].id).unwrap().to_json().unwrap(), checkpoint.to_json().unwrap(), "{phase}");
                assert_eq!(Library::open(&temp.0).unwrap().load_checkpoint(&copies[0].id).unwrap().to_json().unwrap(), checkpoint.to_json().unwrap());
            }
            assert_eq!(std::fs::read(reopened.file_path(&project.id, &source.id)).unwrap(), document_bytes);
            assert_eq!(std::fs::read(reopened.checkpoint_path(&source.id).unwrap()).unwrap(), checkpoint_bytes);
            assert!(!reopened.project_dir(&project.id).join(".save.pending.json").exists());
        }
    }

    #[test]
    fn duplicate_rejects_bound_current_retained_undo_and_redo_documents_before_creating_a_copy() {
        for slot in ["current", "retained", "undo", "redo", "retained-undo"] {
            let (_temp, library) = Tmp::new();
            let project = library.create_project("P").unwrap();
            let mut checkpoint = ordinary_duplicate_checkpoint();
            let bound = cas_checkpoint(Vec::new()).document;
            match slot {
                "current" => {
                    checkpoint.document = bound;
                    checkpoint.history = desigma_doc::History::default();
                }
                "retained" => {
                    checkpoint.retained[0].document = bound;
                    checkpoint.retained[0].history = desigma_doc::History::default();
                }
                _ => {
                    let target = if slot == "retained-undo" { &mut checkpoint.retained[0].history } else { &mut checkpoint.history };
                    let mut history = serde_json::to_value(&*target).unwrap();
                    history[if slot == "redo" { "redo" } else { "undo" }]
                        .as_array_mut()
                        .unwrap()
                        .push(serde_json::to_value(desigma_doc::Command::CityosInstall { document: Box::new(bound) }).unwrap());
                    *target = serde_json::from_value(history).unwrap();
                }
            }
            let source = library.create_file_checkpoint(&project.id, None, "Bound source", &checkpoint).unwrap();
            let manifest_path = library.project_dir(&project.id).join(MANIFEST);
            let before = std::fs::read(&manifest_path).unwrap();
            let original = std::fs::read(library.checkpoint_path(&source.id).unwrap()).unwrap();
            assert!(matches!(library.duplicate_file(&source.id), Err(LibraryError::Invalid(ref message)) if message.contains("successor contract")), "{slot}");
            assert_eq!(std::fs::read(&manifest_path).unwrap(), before);
            assert_eq!(std::fs::read(library.checkpoint_path(&source.id).unwrap()).unwrap(), original);
            assert_eq!(library.files_in(&project.id, None).unwrap().len(), 1);
            assert!(!library.project_dir(&project.id).join(".save.pending.json").exists());
        }
    }

    #[test]
    fn duplicate_cannot_shed_a_frozen_legacy_submission_file_association() {
        let (_temp, library) = Tmp::new();
        let project = library.create_project("P").unwrap();
        let source = library.create_file_checkpoint(&project.id, None, "Source", &ordinary_duplicate_checkpoint()).unwrap();
        let record = library
            .prepare_submission_with_policy(
                &crate::storage_policy::tests::BOUND_FIXTURE_POLICY,
                crate::SubmissionCommand {
                    intent_id: "duplicate-intent".into(),
                    operation_id: "duplicate-operation".into(),
                    predecessor: None,
                    owner: "experience".into(),
                    resource: "frame".into(),
                    scope: BTreeMap::from([("tenantId".into(), "t".into()), ("actorId".into(), "a".into())]),
                    base: desigma_doc::CityosBase::CanonicalGit { repository_id: "repo".into(), commit: "a".repeat(40) },
                    expected_revision: "a".repeat(40),
                    semantic_base: serde_json::json!({}),
                    submitted_snapshot: serde_json::json!({}),
                    dependency_pins: vec![],
                },
            )
            .unwrap();
        // Independent legacy association fixture: no submitted checkpoint was
        // retained by that format, but its original file must remain frozen.
        let none: Option<String> = None;
        let digest = crate::payload_digest(
            &serde_json::to_vec(&("cityos.submission-file.v1", &source.id, &none, &none, &record.command.operation_id, &record.request_digest)).unwrap(),
        );
        let mut stored = serde_json::to_value(&record).unwrap();
        stored["local_file"] = serde_json::json!({"file_id":source.id,"document_id":null,"draft_id":null,"digest":digest});
        let path = library.submission_path(&record.command.operation_id).unwrap();
        let original = serde_json::to_vec(&stored).unwrap();
        std::fs::write(&path, &original).unwrap();
        assert_eq!(library.document_submissions_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &source.id).unwrap(), [record]);
        assert!(
            matches!(library.duplicate_file_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &source.id), Err(LibraryError::Invalid(ref message)) if message.contains("Unresolved submission"))
        );
        assert_eq!(std::fs::read(path).unwrap(), original);
        assert_eq!(library.files_in(&project.id, None).unwrap().len(), 1);
    }

    fn backdate_cas(library: &Library, hash: &str) -> SystemTime {
        let path = library.path("cityos/payloads").join(hash);
        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_times(std::fs::FileTimes::new().set_modified(UNIX_EPOCH + std::time::Duration::from_secs(946_684_800))).unwrap();
        drop(file);
        std::fs::metadata(path).unwrap().modified().unwrap()
    }

    #[test]
    fn cas_checkpoint_refs_preserve_payload_mtime_on_put_autosave_and_restart() {
        let (temp, library) = Tmp::new();
        let project = library.create_project("P").unwrap();
        let file = library.create_file(&project.id, None, "F", &doc("old", 1)).unwrap();
        let bytes = b"already durable immutable payload";
        let hash = library.retain_cityos_payload(bytes).unwrap();
        let path = library.path("cityos/payloads").join(&hash);
        let modified = backdate_cas(&library, &hash);
        let mut checkpoint = cas_checkpoint(vec![hash.clone()]);
        for i in 0..3 {
            library.put_cityos_payload(&hash, bytes).unwrap();
            checkpoint.document.name = format!("save {i}");
            library.save_checkpoint(&file.id, &checkpoint).unwrap();
            library.save_file_with_history(&file.id, &checkpoint.document, &checkpoint.history).unwrap();
            assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), modified);
        }
        SAVE_FAULT.with(|fault| fault.set(Some("journal")));
        assert!(library.save_checkpoint(&file.id, &checkpoint).is_err());
        let journal = library.project_dir(&project.id).join(".save.pending.json");
        let pending: PendingSave = serde_json::from_slice(&std::fs::read(&journal).unwrap()).unwrap();
        assert_eq!(pending.schema_version.as_deref(), Some("desigma.file-save.v3"));
        assert!(pending.payloads.is_empty(), "existing immutable bytes must not be copied into autosave journals");
        assert_eq!(pending.required_pins, Some(vec![hash.clone()]));
        drop(library);
        let reopened = Library::open(&temp.0).unwrap();
        assert_eq!(reopened.load_checkpoint(&file.id).unwrap().to_json().unwrap(), checkpoint.to_json().unwrap());
        assert_eq!(reopened.load_cityos_payload(&hash).unwrap(), bytes);
        assert_eq!(std::fs::metadata(path).unwrap().modified().unwrap(), modified);
        assert!(!journal.exists());
    }

    #[test]
    fn cas_import_journals_only_missing_bytes_and_replay_preserves_materialized_objects() {
        for phase in ["journal", "payloads", "document"] {
            let (temp, library) = Tmp::new();
            let project = library.create_project("P").unwrap();
            let file = library.create_file(&project.id, None, "F", &doc("old", 1)).unwrap();
            let existing = b"existing dependency";
            let incoming = b"new imported dependency";
            let first = library.retain_cityos_payload(existing).unwrap();
            let second = crate::payload_digest(incoming);
            let first_modified = backdate_cas(&library, &first);
            let checkpoint = cas_checkpoint(vec![first.clone(), second.clone()]);
            let portable = serde_json::json!({
                "schemaVersion":"desigma.portable-checkpoint.v1", "checkpoint":checkpoint.to_json().unwrap(),
                "payloads":BTreeMap::from([(first.clone(), existing.to_vec()), (second.clone(), incoming.to_vec())])
            });
            SAVE_FAULT.with(|fault| fault.set(Some(phase)));
            assert!(library.import_checkpoint_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &file.id, &portable.to_string()).is_err());
            let journal = library.project_dir(&project.id).join(".save.pending.json");
            let pending: PendingSave = serde_json::from_slice(&std::fs::read(&journal).unwrap()).unwrap();
            assert_eq!(pending.payloads, BTreeMap::from([(second.clone(), incoming.to_vec())]));
            assert_eq!(pending.required_pins.unwrap().into_iter().collect::<BTreeSet<_>>(), BTreeSet::from([first.clone(), second.clone()]));
            let second_modified = (phase != "journal").then(|| backdate_cas(&library, &second));
            drop(library);
            let reopened = Library::open(&temp.0).unwrap();
            assert_eq!(reopened.load_checkpoint(&file.id).unwrap().to_json().unwrap(), checkpoint.to_json().unwrap());
            assert_eq!(reopened.load_cityos_payload(&second).unwrap(), incoming);
            assert_eq!(std::fs::metadata(reopened.path("cityos/payloads").join(&first)).unwrap().modified().unwrap(), first_modified);
            if let Some(modified) = second_modified {
                assert_eq!(
                    std::fs::metadata(reopened.path("cityos/payloads").join(&second)).unwrap().modified().unwrap(),
                    modified,
                    "replay must not rewrite an imported object already materialized before the interruption"
                );
            }
            assert!(!journal.exists());
        }
    }

    #[test]
    fn cas_portable_repair_journals_verified_bytes_and_preserves_unresolved_submissions() {
        let (_source_dir, source) = Tmp::new();
        let source_project = source.create_project("Source").unwrap();
        let bytes = b"verified portable replacement";
        let hash = source.retain_cityos_payload(bytes).unwrap();
        let checkpoint = cas_checkpoint(vec![hash.clone()]);
        let source_file = source.create_file_checkpoint(&source_project.id, None, "F", &checkpoint).unwrap();
        let portable = source.export_checkpoint_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &source_file.id).unwrap();

        for phase in ["before-journal", "journal", "payloads"] {
            let (temp, library) = Tmp::new();
            let project = library.create_project("Target").unwrap();
            let file = library.create_file(&project.id, None, "F", &doc("old", 1)).unwrap();
            library.put_cityos_payload(&hash, bytes).unwrap();
            let submission_only = library.retain_cityos_payload(b"unresolved submission only").unwrap();
            let command = crate::SubmissionCommand {
                intent_id: "repair-intent".into(),
                operation_id: "repair-operation".into(),
                predecessor: None,
                owner: "experience".into(),
                resource: "frame".into(),
                scope: BTreeMap::from([("tenantId".into(), "t".into()), ("actorId".into(), "a".into())]),
                base: desigma_doc::CityosBase::CanonicalGit { repository_id: "repo".into(), commit: "a".repeat(40) },
                expected_revision: "a".repeat(40),
                semantic_base: serde_json::json!({"elevation":"flat"}),
                submitted_snapshot: serde_json::json!({"elevation":"resting"}),
                dependency_pins: vec![hash.clone(), submission_only.clone()],
            };
            let original = library.prepare_submission_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, command.clone()).unwrap();
            let submission_path = library.submission_path(&command.operation_id).unwrap();
            let original_record = std::fs::read(&submission_path).unwrap();
            let document_path = library.file_path(&project.id, &file.id);
            let original_document = std::fs::read(&document_path).unwrap();
            let manifest_path = library.project_dir(&project.id).join(MANIFEST);
            let original_manifest = std::fs::read(&manifest_path).unwrap();
            let journal = library.project_dir(&project.id).join(".save.pending.json");
            let payload_path = library.path("cityos/payloads").join(&hash);
            std::fs::write(&payload_path, b"corrupt local object").unwrap();
            std::fs::remove_file(library.path("cityos/payloads").join(&submission_only)).unwrap();

            // An incomplete/unverified package cannot repair or erase anything.
            for fault in ["missing", "corrupt"] {
                let mut invalid: serde_json::Value = serde_json::from_str(&portable).unwrap();
                if fault == "missing" {
                    invalid["payloads"].as_object_mut().unwrap().remove(&hash);
                } else {
                    invalid["payloads"][&hash] = serde_json::json!([0]);
                }
                assert!(
                    library.import_checkpoint_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &file.id, &invalid.to_string()).is_err(),
                    "{phase}/{fault}"
                );
                assert_eq!(std::fs::read(&payload_path).unwrap(), b"corrupt local object");
                assert_eq!(std::fs::read(&document_path).unwrap(), original_document);
                assert_eq!(std::fs::read(&manifest_path).unwrap(), original_manifest);
                assert_eq!(std::fs::read(&submission_path).unwrap(), original_record);
                assert!(!journal.exists());
                assert!(!library.checkpoint_path(&file.id).unwrap().exists());
            }
            assert!(library.save_checkpoint(&file.id, &checkpoint).is_err(), "ordinary saves still require verified CAS bytes");
            let mut dispatches = 0;
            assert!(library
                .dispatch_prepared_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, command.clone(), |_| {
                    dispatches += 1;
                    Ok(())
                })
                .is_err());
            assert_eq!(dispatches, 0);
            assert_eq!(library.prepare_submission_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, command.clone()).unwrap(), original);
            let expected_roots = BTreeSet::from([hash.clone(), submission_only.clone()]);
            assert_eq!(
                library.retention_roots_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY).unwrap().into_iter().collect::<BTreeSet<_>>(),
                expected_roots
            );

            SAVE_FAULT.with(|fault| fault.set(Some(phase)));
            assert!(library.import_checkpoint_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &file.id, &portable).is_err(), "{phase}");
            assert_eq!(SAVE_FAULT.with(|fault| fault.get()), None, "must reach the {phase} boundary");
            assert_eq!(std::fs::read(&document_path).unwrap(), original_document);
            assert_eq!(std::fs::read(&manifest_path).unwrap(), original_manifest);
            assert_eq!(std::fs::read(&submission_path).unwrap(), original_record);
            assert!(!library.checkpoint_path(&file.id).unwrap().exists());
            if phase == "before-journal" {
                assert!(!journal.exists());
            } else {
                let pending: PendingSave = serde_json::from_slice(&std::fs::read(&journal).unwrap()).unwrap();
                assert_eq!(pending.payloads, BTreeMap::from([(hash.clone(), bytes.to_vec())]));
                assert_eq!(pending.required_pins, Some(vec![hash.clone()]));
            }
            let expected_bytes: &[u8] = if phase == "payloads" { bytes } else { b"corrupt local object" };
            assert_eq!(std::fs::read(&payload_path).unwrap(), expected_bytes);
            let repaired_mtime = (phase == "payloads").then(|| backdate_cas(&library, &hash));
            drop(library);

            let reopened = Library::open(&temp.0).unwrap();
            if phase == "before-journal" {
                assert_eq!(std::fs::read(&document_path).unwrap(), original_document);
                reopened.import_checkpoint_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &file.id, &portable).unwrap();
            }
            assert_eq!(reopened.load_checkpoint(&file.id).unwrap().to_json().unwrap(), checkpoint.to_json().unwrap());
            assert_eq!(reopened.load_cityos_payload(&hash).unwrap(), bytes);
            if let Some(modified) = repaired_mtime {
                assert_eq!(std::fs::metadata(&payload_path).unwrap().modified().unwrap(), modified);
            }
            assert!(!journal.exists());
            assert_eq!(std::fs::read(&submission_path).unwrap(), original_record);
            assert_eq!(reopened.load_submission_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &command.operation_id).unwrap(), original);
            assert!(original.frozen());
            assert_eq!(
                reopened.retention_roots_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY).unwrap().into_iter().collect::<BTreeSet<_>>(),
                expected_roots
            );
            assert!(reopened.load_cityos_payload(&submission_only).is_err());
            assert!(reopened
                .dispatch_prepared_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, command.clone(), |_| {
                    dispatches += 1;
                    Ok(())
                })
                .is_err());
            assert_eq!(dispatches, 0, "repair must not bypass the still-missing submission dependency");
        }
    }

    #[test]
    fn cas_required_refs_fail_closed_before_recovery_writes_when_missing_corrupt_or_omitted() {
        for fault in ["missing", "corrupt", "omitted"] {
            let (_temp, library) = Tmp::new();
            let project = library.create_project("P").unwrap();
            let file = library.create_file(&project.id, None, "F", &doc("old", 1)).unwrap();
            let bytes = b"durable reference";
            let hash = library.retain_cityos_payload(bytes).unwrap();
            let checkpoint = cas_checkpoint(vec![hash.clone()]);
            let document_path = library.file_path(&project.id, &file.id);
            let old_document = std::fs::read(&document_path).unwrap();
            SAVE_FAULT.with(|point| point.set(Some("journal")));
            assert!(library.save_checkpoint(&file.id, &checkpoint).is_err());
            let journal = library.project_dir(&project.id).join(".save.pending.json");
            let original_journal = std::fs::read(&journal).unwrap();
            let payload_path = library.path("cityos/payloads").join(&hash);
            match fault {
                "missing" => std::fs::remove_file(&payload_path).unwrap(),
                "corrupt" => std::fs::write(&payload_path, b"corrupt bytes").unwrap(),
                _ => {
                    let mut pending: PendingSave = serde_json::from_slice(&original_journal).unwrap();
                    pending.required_pins = Some(vec![]);
                    std::fs::write(&journal, serde_json::to_vec(&pending).unwrap()).unwrap();
                }
            }
            assert!(Library::open(library.root()).is_err(), "{fault}");
            assert_eq!(std::fs::read(&document_path).unwrap(), old_document, "{fault}");
            assert!(journal.exists());
            library.put_cityos_payload(&hash, bytes).unwrap();
            std::fs::write(&journal, original_journal).unwrap();
            let reopened = Library::open(library.root()).unwrap();
            assert_eq!(reopened.load_checkpoint(&file.id).unwrap().to_json().unwrap(), checkpoint.to_json().unwrap());
        }
    }

    #[test]
    fn project_crud_and_trash() {
        let (_t, lib) = Tmp::new();
        let a = lib.create_project("  Alpha ").unwrap();
        assert_eq!(a.name, "Alpha");
        let b = lib.create_project("Beta").unwrap();
        assert_eq!(lib.list_projects().iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Alpha", "Beta"]);
        lib.rename_project(&a.id, "Alpha 2").unwrap();
        assert_eq!(lib.project(&a.id).unwrap().name, "Alpha 2");
        assert!(lib.create_project("   ").is_err());
        let f = lib.create_file(&b.id, None, "Keep", &doc("Keep", 1)).unwrap();
        lib.delete_project(&b.id).unwrap();
        assert_eq!(lib.list_projects().len(), 1);
        assert!(lib.load_file(&f.id).is_err());
        let trash = lib.list_trash();
        assert_eq!(trash.len(), 1);
        assert_eq!((trash[0].kind.clone(), trash[0].name.as_str()), (TrashKind::Project, "Beta"));
        lib.restore(&b.id).unwrap();
        assert_eq!(lib.list_projects().len(), 2);
        assert_eq!(lib.load_file(&f.id).unwrap().nodes.len(), 2);
        assert!(lib.list_trash().is_empty());
    }

    #[test]
    fn folders_nest_rename_and_refuse_cycles() {
        let (_t, lib) = Tmp::new();
        let p = lib.create_project("P").unwrap();
        let a = lib.create_folder(&p.id, None, "A").unwrap();
        let b = lib.create_folder(&p.id, Some(&a.id), "B").unwrap();
        let c = lib.create_folder(&p.id, Some(&b.id), "C").unwrap();
        lib.rename_folder(&p.id, &a.id, "Alpha").unwrap();
        let folders = lib.project(&p.id).unwrap().folders;
        assert_eq!(folders.iter().find(|f| f.id == a.id).unwrap().name, "Alpha");
        assert_eq!(folders.iter().find(|f| f.id == c.id).unwrap().parent.as_deref(), Some(b.id.as_str()));
        assert!(lib.move_folder(&p.id, &a.id, Some(&c.id)).is_err(), "into own subtree");
        assert!(lib.move_folder(&p.id, &a.id, Some(&a.id)).is_err(), "into itself");
        lib.move_folder(&p.id, &c.id, None).unwrap();
        assert_eq!(lib.project(&p.id).unwrap().folders.iter().find(|f| f.id == c.id).unwrap().parent, None);
        assert!(lib.create_folder(&p.id, Some("nope"), "X").is_err());
    }

    #[test]
    fn file_crud_roundtrip() {
        let (_t, lib) = Tmp::new();
        let p = lib.create_project("P").unwrap();
        let fo = lib.create_folder(&p.id, None, "F").unwrap();
        let e = lib.create_file(&p.id, Some(&fo.id), "Home", &doc("Home", 2)).unwrap();
        assert_eq!(lib.load_file(&e.id).unwrap().nodes.len(), 3);
        lib.save_file(&e.id, &doc("Home", 4)).unwrap();
        assert_eq!(lib.load_file(&e.id).unwrap().nodes.len(), 5);
        assert!(lib.file(&e.id).unwrap().modified > e.modified);
        lib.rename_file(&e.id, "Landing").unwrap();
        assert_eq!(lib.file(&e.id).unwrap().name, "Landing");
        assert_eq!(lib.files_in(&p.id, Some(Some(&fo.id))).unwrap().len(), 1);
        assert_eq!(lib.files_in(&p.id, Some(None)).unwrap().len(), 0);
        let d = lib.duplicate_file(&e.id).unwrap();
        assert_ne!(d.id, e.id);
        assert_eq!(d.name, "Landing (copy)");
        assert_eq!(lib.load_file(&d.id).unwrap().nodes.len(), 5);
        // move to another project drops the folder and moves the data
        let q = lib.create_project("Q").unwrap();
        lib.move_file(&d.id, &q.id, None).unwrap();
        assert_eq!(lib.file(&d.id).unwrap().project, q.id);
        assert_eq!(lib.files_in(&q.id, None).unwrap().len(), 1);
        assert_eq!(lib.files_in(&p.id, None).unwrap().len(), 1);
        assert_eq!(lib.load_file(&d.id).unwrap().nodes.len(), 5);
        assert!(lib.move_file(&d.id, &q.id, Some("missing")).is_err());
        assert!(lib.load_file("missing").is_err());
    }

    #[test]
    fn trash_restore_and_empty() {
        let (_t, lib) = Tmp::new();
        let p = lib.create_project("P").unwrap();
        let fo = lib.create_folder(&p.id, None, "F").unwrap();
        let e = lib.create_file(&p.id, Some(&fo.id), "Gone", &doc("Gone", 1)).unwrap();
        lib.set_thumbnail(&e.id, b"png").unwrap();
        lib.trash_file(&e.id).unwrap();
        assert!(lib.all_files().is_empty());
        assert!(lib.search("gone").is_empty());
        let t = lib.list_trash();
        assert_eq!((t.len(), t[0].kind.clone(), t[0].name.as_str()), (1, TrashKind::File, "Gone"));
        assert_eq!(t[0].project_name.as_deref(), Some("P"));
        let back = lib.restore_file(&e.id).unwrap();
        assert_eq!(back.folder.as_deref(), Some(fo.id.as_str()));
        assert_eq!(lib.load_file(&e.id).unwrap().nodes.len(), 2);
        assert!(lib.thumbnail_path(&lib.file(&e.id).unwrap()).is_some(), "thumbnail travels with the file");
        assert!(lib.list_trash().is_empty());
        // trash again, then empty: the only hard delete
        lib.trash_file(&e.id).unwrap();
        assert!(lib.trash_dir(&e.id).join("file.desigma").exists(), "trashing never deletes");
        lib.empty_trash().unwrap();
        assert!(lib.list_trash().is_empty());
        assert!(lib.restore_file(&e.id).is_err());
        assert!(lib.all_files().is_empty());
        lib.empty_trash().unwrap();
    }

    #[test]
    fn restoring_a_file_of_a_trashed_project_asks_for_the_project_first() {
        let (_t, lib) = Tmp::new();
        let p = lib.create_project("P").unwrap();
        let e = lib.create_file(&p.id, None, "F", &doc("F", 0)).unwrap();
        lib.trash_file(&e.id).unwrap();
        lib.delete_project(&p.id).unwrap();
        assert!(lib.restore_file(&e.id).is_err());
        lib.restore_project(&p.id).unwrap();
        lib.restore_file(&e.id).unwrap();
        assert_eq!(lib.all_files().len(), 1);
    }

    #[test]
    fn atomic_save_leaves_no_temp_file_and_survives_a_stale_one() {
        let (_t, lib) = Tmp::new();
        let p = lib.create_project("P").unwrap();
        let e = lib.create_file(&p.id, None, "A", &doc("A", 1)).unwrap();
        let path = lib.file_path(&p.id, &e.id);
        // a leftover temp file from a crashed write must not corrupt the real one
        std::fs::write(path.with_extension("desigma.tmp"), "garbage").unwrap();
        lib.save_file(&e.id, &doc("A", 3)).unwrap();
        assert_eq!(lib.load_file(&e.id).unwrap().nodes.len(), 4);
        let names: Vec<_> = std::fs::read_dir(path.parent().unwrap()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert!(names.contains(&format!("{}.desigma", e.id)));
        assert!(names.contains(&format!("{}.desigma.tmp", e.id)));
        assert!(names.iter().all(|name| !name.contains(".tmp-")));
        assert_eq!(std::fs::read_to_string(path.with_extension("desigma.tmp")).unwrap(), "garbage");
    }

    #[test]
    fn interrupted_document_manifest_commit_recovers_on_reopen() {
        let (_t, lib) = Tmp::new();
        let project = lib.create_project("P").unwrap();
        let file = lib.create_file(&project.id, None, "File", &doc("before", 1)).unwrap();
        let mut manifest = {
            let _guard = write_lock(&lib).unwrap();
            lib.load_manifest_locked(&project.id).unwrap()
        };
        manifest.files[0].modified = now_ms();
        let pending = PendingSave {
            schema_version: None,
            file_id: file.id.clone(),
            document: doc("accepted-new", 2).to_json().unwrap(),
            manifest,
            checkpoint: None,
            payloads: BTreeMap::new(),
            required_pins: None,
        };
        write_atomic(&lib.project_dir(&project.id).join(".save.pending.json"), serde_json::to_string(&pending).unwrap().as_bytes()).unwrap();
        let reopened = Library::open(lib.root()).unwrap();
        assert_eq!(reopened.load_file(&file.id).unwrap().name, "accepted-new");
        assert!(!lib.project_dir(&project.id).join(".save.pending.json").exists());
    }

    #[test]
    fn reconciled_checkpoint_save_recovers_each_phase_and_fences_stale_autosaves() {
        for phase in ["before-journal", "journal", "payloads", "document", "checkpoint", "manifest", "complete"] {
            let (temp, library) = Tmp::new();
            let project = library.create_project("P").unwrap();
            let mut old_document = doc("old UI", 1);
            let mut old_history = desigma_doc::History::default();
            old_history.exec(&mut old_document, desigma_doc::Command::SetComments { comments: vec![] }).unwrap();
            let file = library.create_file_with_history(&project.id, None, "F", &old_document, &old_history).unwrap();
            let old = library.load_checkpoint(&file.id).unwrap();
            let mut incoming = old.clone();
            incoming.install_baseline(doc("incoming baseline", 2));
            SAVE_FAULT.with(|fault| fault.set(Some(phase)));
            let result = library.save_checkpoint_reconciled(&file.id, &incoming);
            if phase == "before-journal" {
                assert!(result.is_err());
                assert_eq!(library.load_checkpoint(&file.id).unwrap().to_json().unwrap(), old.to_json().unwrap());
                library.save_file_with_history(&file.id, &old_document, &old_history).unwrap();
                continue;
            }
            let receipt = result.unwrap();
            assert!(receipt.recovered_after_error, "{phase}");
            assert_eq!(receipt.checkpoint.to_json().unwrap(), incoming.to_json().unwrap());
            assert!(matches!(library.save_file_with_history(&file.id, &old_document, &old_history), Err(LibraryError::CheckpointConflict(_))));
            assert!(matches!(library.save_file(&file.id, &old_document), Err(LibraryError::CheckpointConflict(_))));
            assert!(matches!(library.save_checkpoint(&file.id, &old), Err(LibraryError::CheckpointConflict(_))));
            assert_eq!(library.load_checkpoint(&file.id).unwrap().to_json().unwrap(), incoming.to_json().unwrap());
            drop(library);
            let reopened = Library::open(&temp.0).unwrap();
            assert_eq!(reopened.load_checkpoint(&file.id).unwrap().history.baseline_id(), incoming.history.baseline_id());
            let mut current = receipt.checkpoint;
            current.history.exec(&mut current.document, desigma_doc::Command::SetComments { comments: vec![] }).unwrap();
            reopened.save_file_with_history(&file.id, &current.document, &current.history).unwrap();
            assert!(reopened.load_checkpoint(&file.id).unwrap().history.can_undo());
        }
    }

    #[test]
    fn unresolved_checkpoint_save_keeps_intent_and_blocks_old_ui_after_later_recovery() {
        let (_temp, library) = Tmp::new();
        let project = library.create_project("P").unwrap();
        let file = library.create_file(&project.id, None, "F", &doc("old", 1)).unwrap();
        let old = library.load_checkpoint(&file.id).unwrap();
        let mut incoming = old.clone();
        incoming.install_baseline(doc("incoming", 2));
        SAVE_FAULT.with(|fault| fault.set(Some("document")));
        SAVE_FAULT_REPEATS.with(|remaining| remaining.set(1));
        assert!(matches!(library.save_checkpoint_reconciled(&file.id, &incoming), Err(LibraryError::ReconciliationRequired { .. })));
        assert!(library.project_dir(&project.id).join(".save.pending.json").exists());
        assert!(matches!(library.save_file_with_history(&file.id, &old.document, &old.history), Err(LibraryError::CheckpointConflict(_))));
        assert_eq!(library.load_checkpoint(&file.id).unwrap().to_json().unwrap(), incoming.to_json().unwrap());
    }

    #[test]
    fn legacy_error_after_journal_cannot_be_followed_by_a_stale_history_save() {
        let (_temp, library) = Tmp::new();
        let project = library.create_project("P").unwrap();
        let file = library.create_file(&project.id, None, "F", &doc("old", 1)).unwrap();
        let old = library.load_checkpoint(&file.id).unwrap();
        let mut incoming = old.clone();
        incoming.install_baseline(doc("incoming", 2));
        SAVE_FAULT.with(|fault| fault.set(Some("document")));
        assert!(library.save_checkpoint(&file.id, &incoming).is_err());
        assert!(matches!(library.save_file_with_history(&file.id, &old.document, &old.history), Err(LibraryError::CheckpointConflict(_))));
        assert_eq!(library.load_checkpoint(&file.id).unwrap().to_json().unwrap(), incoming.to_json().unwrap());
    }

    #[test]
    fn identical_content_baseline_install_still_fences_old_history_and_receipt_retries() {
        let (_temp, library) = Tmp::new();
        let project = library.create_project("P").unwrap();
        let file = library.create_file(&project.id, None, "F", &doc("same", 1)).unwrap();
        let old = library.load_checkpoint(&file.id).unwrap();
        let mut incoming = old.clone();
        incoming.install_baseline(old.document.clone());
        let receipt = library.save_checkpoint_reconciled(&file.id, &incoming).unwrap();
        assert!(!receipt.recovered_after_error);
        assert_ne!(receipt.checkpoint.history.baseline_id(), old.history.baseline_id());
        assert!(matches!(library.save_file_with_history(&file.id, &old.document, &old.history), Err(LibraryError::CheckpointConflict(_))));
        let retry = library.save_checkpoint_reconciled(&file.id, &incoming).unwrap();
        assert_eq!(retry.checkpoint.retained.len(), 1);
        assert_eq!(retry.checkpoint.history.baseline_id(), receipt.checkpoint.history.baseline_id());
    }

    #[test]
    #[ignore = "worker invoked by checkpoint_recovery_survives_process_exit"]
    fn checkpoint_interruption_worker() {
        let Ok(root) = std::env::var("DESIGMA_PERSISTENCE_TEST_ROOT") else { return };
        let file = std::env::var("DESIGMA_PERSISTENCE_TEST_FILE").unwrap();
        let phase = std::env::var("DESIGMA_PERSISTENCE_TEST_PHASE").unwrap();
        let phase = match phase.as_str() {
            "document" => "document",
            "manifest" => "manifest",
            _ => panic!("Unexpected worker phase"),
        };
        let library = Library::open(root).unwrap();
        let mut document = library.load_file(&file).unwrap();
        document.name = "from child".into();
        document.images.insert("child asset".into(), vec![9, 8, 7]);
        let mut history = desigma_doc::History::default();
        history.exec(&mut document, desigma_doc::Command::SetComments { comments: vec![] }).unwrap();
        SAVE_FAULT.with(|fault| fault.set(Some(phase)));
        assert!(library.save_checkpoint(&file, &DocumentCheckpoint::new(document, history)).is_err());
        // No Library/fixture teardown or process-local state survives this exit.
        std::process::exit(91);
    }

    #[test]
    fn checkpoint_recovery_survives_process_exit() {
        for phase in ["document", "manifest"] {
            let (temp, library) = Tmp::new();
            let project = library.create_project("P").unwrap();
            let file = library.create_file(&project.id, None, "F", &doc("old", 1)).unwrap();
            drop(library);
            let result = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "store::tests::checkpoint_interruption_worker", "--ignored"])
                .env("DESIGMA_PERSISTENCE_TEST_ROOT", &temp.0)
                .env("DESIGMA_PERSISTENCE_TEST_FILE", &file.id)
                .env("DESIGMA_PERSISTENCE_TEST_PHASE", phase)
                .status()
                .unwrap();
            assert_eq!(result.code(), Some(91), "worker must exit after the injected interruption");
            let reopened = Library::open(&temp.0).unwrap();
            let checkpoint = reopened.load_checkpoint(&file.id).unwrap();
            assert_eq!(checkpoint.document.name, "from child");
            assert_eq!(checkpoint.document.images["child asset"], [9, 8, 7]);
            assert!(checkpoint.history.can_undo());
            assert!(!reopened.project_dir(&project.id).join(".save.pending.json").exists());
        }
    }

    #[test]
    fn interrupted_checkpoint_creation_recovers_the_file_entry_and_initial_history() {
        for phase in ["before-journal", "journal", "document", "checkpoint", "manifest"] {
            let (temp, library) = Tmp::new();
            let project = library.create_project("P").unwrap();
            let mut document = doc("created", 2);
            let mut history = desigma_doc::History::default();
            history.exec(&mut document, desigma_doc::Command::SetComments { comments: vec![] }).unwrap();
            let checkpoint = DocumentCheckpoint::new(document, history);
            SAVE_FAULT.with(|fault| fault.set(Some(phase)));
            assert!(library.create_file_checkpoint(&project.id, None, "Created", &checkpoint).is_err());
            drop(library);
            let reopened = Library::open(&temp.0).unwrap();
            let files = reopened.files_in(&project.id, None).unwrap();
            if phase == "before-journal" {
                assert!(files.is_empty());
            } else {
                assert_eq!(files.len(), 1);
                assert_eq!(reopened.load_checkpoint(&files[0].id).unwrap().to_json().unwrap(), checkpoint.to_json().unwrap());
            }
        }
    }

    #[test]
    fn checkpoint_interruptions_recover_document_history_dependencies_and_manifest() {
        use desigma_doc::{Command, History, RetainedHistory};
        for phase in ["before-journal", "journal", "payloads", "document", "checkpoint", "manifest", "complete"] {
            let (temp, library) = Tmp::new();
            let project = library.create_project("P").unwrap();
            let file = library.create_file(&project.id, None, "F", &doc("old", 1)).unwrap();
            let mut document = doc("new", 2);
            document.images.insert("image".into(), vec![1, 3, 5]);
            let node_id = document.nodes[&document.pages[0]].children[0];
            let mut history = History::default();
            for name in ["first", "second"] {
                let mut node = document.get(node_id).unwrap().clone();
                node.name = name.into();
                history.exec(&mut document, Command::Update { node: Box::new(node) }).unwrap();
            }
            history.undo(&mut document).unwrap();
            let mut checkpoint = DocumentCheckpoint::new(document, history);
            let hash = crate::payload_digest(b"history-only dependency");
            let mut retained = doc("previous baseline", 0);
            retained.cityos = Some(desigma_doc::CityosBinding {
                schema_version: "cityos.design-exchange.v1".into(),
                document_id: "d".into(),
                draft_id: "d".into(),
                base: desigma_doc::CityosBase::CanonicalGit { repository_id: "r".into(), commit: "a".repeat(40) },
                semantic_base: serde_json::json!({}),
                dependency_pins: vec![hash.clone()],
                occurrences: BTreeMap::new(),
                exchange_v2: None,
                exchange_v3: None,
            });
            checkpoint.retained.push(RetainedHistory { document: retained, history: History::default() });
            let payloads = BTreeMap::from([(hash.clone(), b"history-only dependency".to_vec())]);
            let old_modified = library.file(&file.id).unwrap().modified;
            SAVE_FAULT.with(|fault| fault.set(Some(phase)));
            assert!(library.save_checkpoint_payloads(&file.id, &checkpoint, payloads).is_err(), "{phase}");
            drop(library);
            let reopened = Library::open(&temp.0).unwrap();
            let mut back = reopened.load_checkpoint(&file.id).unwrap();
            if phase == "before-journal" {
                assert_eq!(back.document.name, "old");
                assert_eq!(reopened.file(&file.id).unwrap().modified, old_modified);
                assert!(!reopened.path("cityos/payloads").exists());
            } else {
                assert_eq!(back.to_json().unwrap(), checkpoint.to_json().unwrap(), "{phase}");
                assert!(reopened.file(&file.id).unwrap().modified > old_modified);
                assert_eq!(reopened.load_cityos_payload(&hash).unwrap(), b"history-only dependency");
                back.history.redo(&mut back.document).unwrap();
                assert_eq!(back.document.get(node_id).unwrap().name, "second");
                back.history.undo(&mut back.document).unwrap();
                assert_eq!(back.document.get(node_id).unwrap().name, "first");
            }
            assert!(!reopened.project_dir(&project.id).join(".save.pending.json").exists());
            drop(reopened);
            assert!(Library::open(&temp.0).unwrap().load_checkpoint(&file.id).is_ok(), "idempotent restart at {phase}");
        }
    }

    #[test]
    fn invalid_recovery_payloads_are_preserved_without_partial_target_writes() {
        let (_temp, library) = Tmp::new();
        let project = library.create_project("P").unwrap();
        let file = library.create_file(&project.id, None, "F", &doc("old", 1)).unwrap();
        let path = library.file_path(&project.id, &file.id);
        let original = std::fs::read(&path).unwrap();
        let journal = library.project_dir(&project.id).join(".save.pending.json");
        for fault in ["checkpoint", "dependency", "identity", "manifest", "future"] {
            let mut manifest = {
                let _guard = write_lock(&library).unwrap();
                library.load_manifest_locked(&project.id).unwrap()
            };
            if fault == "manifest" {
                manifest.files[0].project = "another-project".into();
            }
            let pending = PendingSave {
                schema_version: Some(if fault == "future" { "future".into() } else { "desigma.file-save.v2".into() }),
                required_pins: None,
                file_id: if fault == "identity" { "..".into() } else { file.id.clone() },
                document: doc("new", 2).to_json().unwrap(),
                manifest,
                checkpoint: (fault == "checkpoint").then(|| DocumentCheckpoint::new(doc("wrong", 1), desigma_doc::History::default()).to_json().unwrap()),
                payloads: if fault == "dependency" { BTreeMap::from([("a".repeat(64), vec![1])]) } else { BTreeMap::new() },
            };
            std::fs::write(&journal, serde_json::to_vec(&pending).unwrap()).unwrap();
            assert!(Library::open(library.root()).is_err(), "{fault}");
            assert_eq!(std::fs::read(&path).unwrap(), original, "{fault}");
            assert!(journal.exists(), "retain failed intent for recovery");
            std::fs::remove_file(&journal).unwrap();
        }
    }

    #[test]
    fn folder_manifest_edits_and_checkpoint_saves_do_not_overwrite_each_other() {
        let (_temp, library) = Tmp::new();
        let project = library.create_project("P").unwrap();
        let file = library.create_file(&project.id, None, "F", &doc("old", 1)).unwrap();
        let background = library.clone();
        let project_id = project.id.clone();
        let thread = std::thread::spawn(move || {
            for i in 0..8 {
                background.create_folder(&project_id, None, &format!("Folder {i}")).unwrap();
            }
        });
        for i in 0..8 {
            library.save_checkpoint(&file.id, &DocumentCheckpoint::new(doc(&format!("save {i}"), 1), desigma_doc::History::default())).unwrap();
        }
        thread.join().unwrap();
        let reopened = Library::open(library.root()).unwrap();
        assert_eq!(reopened.project(&project.id).unwrap().folders.len(), 8);
        assert_eq!(reopened.load_checkpoint(&file.id).unwrap().document.name, "save 7");
    }

    #[test]
    fn thumbnail_and_checkpoint_writers_keep_the_same_manifest_after_restart() {
        let (_temp, library) = Tmp::new();
        let project = library.create_project("P").unwrap();
        let file = library.create_file(&project.id, None, "F", &doc("old", 1)).unwrap();
        let background = library.clone();
        let id = file.id.clone();
        let thread = std::thread::spawn(move || background.set_thumbnail(&id, b"png"));
        library.save_checkpoint(&file.id, &DocumentCheckpoint::new(doc("new", 2), desigma_doc::History::default())).unwrap();
        thread.join().unwrap().unwrap();
        let reopened = Library::open(library.root()).unwrap();
        assert_eq!(reopened.load_checkpoint(&file.id).unwrap().document.name, "new");
        assert!(reopened.thumbnail_path(&reopened.file(&file.id).unwrap()).is_some());
    }

    #[test]
    fn late_thumbnail_after_root_removal_does_not_recreate_a_project_or_manifest() {
        let (temp, library) = Tmp::new();
        let project = library.create_project("P").unwrap();
        let file = library.create_file(&project.id, None, "F", &doc("old", 1)).unwrap();
        std::fs::remove_dir_all(&temp.0).unwrap();
        assert!(library.set_thumbnail(&file.id, b"png").is_err());
        assert!(!temp.0.exists());
    }

    #[test]
    fn thumbnail_root_removed_after_manifest_read_is_not_resurrected() {
        let (temp, library) = Tmp::new();
        let project = library.create_project("P").unwrap();
        let file = library.create_file(&project.id, None, "F", &doc("old", 1)).unwrap();
        assert!(library.set_thumbnail_before_write(&file.id, b"png", || std::fs::remove_dir_all(&temp.0).unwrap()).is_err());
        assert!(!temp.0.exists());
    }

    #[test]
    fn file_kind_rejects_unknown_containers_instead_of_defaulting_to_a_design() {
        let (_t, lib) = Tmp::new();
        let project = lib.create_project("P").unwrap();
        let file = lib.create_file(&project.id, None, "File", &doc("valid", 1)).unwrap();
        let path = lib.file_path(&project.id, &file.id);
        let mut value: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        value["schemaVersion"] = serde_json::json!("desigma.document.v999");
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(lib.file_kind(&file.id).is_err());
        assert!(lib.load_file(&file.id).is_err());
    }

    #[test]
    fn search_and_recents_order() {
        let (_t, lib) = Tmp::new();
        let p = lib.create_project("P").unwrap();
        let a = lib.create_file(&p.id, None, "Login screen", &doc("a", 0)).unwrap();
        let b = lib.create_file(&p.id, None, "Dashboard", &doc("b", 0)).unwrap();
        let c = lib.create_file(&p.id, None, "Login flow", &doc("c", 0)).unwrap();
        assert_eq!(lib.recents(10).iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), [&c.id, &b.id, &a.id]);
        lib.mark_opened(&a.id).unwrap();
        assert_eq!(lib.recents(2).iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), [&a.id, &c.id]);
        lib.save_file(&b.id, &doc("b", 1)).unwrap();
        let hits = lib.search("LOGIN");
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().all(|f| f.name.contains("Login")));
        assert!(lib.search("").is_empty() && lib.search("zzz").is_empty());
        lib.star(&b.id, true).unwrap();
        assert_eq!(lib.starred().iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), [&b.id]);
        lib.star(&b.id, false).unwrap();
        assert!(lib.starred().is_empty());
    }

    #[test]
    fn thumbnails_are_stored_and_follow_moves() {
        let (_t, lib) = Tmp::new();
        let p = lib.create_project("P").unwrap();
        let q = lib.create_project("Q").unwrap();
        let e = lib.create_file(&p.id, None, "A", &doc("A", 0)).unwrap();
        assert!(lib.thumbnail_path(&e).is_none());
        lib.set_thumbnail(&e.id, &[1, 2, 3]).unwrap();
        let e2 = lib.file(&e.id).unwrap();
        assert_eq!(std::fs::read(lib.thumbnail_path(&e2).unwrap()).unwrap(), vec![1, 2, 3]);
        let dup = lib.duplicate_file(&e.id).unwrap();
        assert!(lib.thumbnail_path(&dup).is_some());
        lib.move_file(&e.id, &q.id, None).unwrap();
        let moved = lib.file(&e.id).unwrap();
        assert_eq!(std::fs::read(lib.thumbnail_path(&moved).unwrap()).unwrap(), vec![1, 2, 3]);
    }

    #[test]
    fn import_native_json_and_svg() {
        let (t, lib) = Tmp::new();
        let p = lib.create_project("P").unwrap();
        let src = t.0.join("incoming.desigma");
        std::fs::write(&src, doc("From disk", 2).to_json().unwrap()).unwrap();
        let e = lib.import_file_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &src, &p.id, None).unwrap();
        assert_eq!(e.name, "From disk");
        assert_eq!(lib.load_file(&e.id).unwrap().nodes.len(), 3);
        let svg = t.0.join("icon.svg");
        std::fs::write(&svg, r##"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><rect width="8" height="8" fill="#f00"/></svg>"##).unwrap();
        let s = lib.import_file_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &svg, &p.id, None).unwrap();
        assert_eq!(s.name, "icon");
        assert_eq!(lib.load_file(&s.id).unwrap().pages.len(), 1);
        let bad = t.0.join("bad.desigma");
        std::fs::write(&bad, "nope").unwrap();
        assert!(lib.import_file_with_policy(&crate::storage_policy::tests::BOUND_FIXTURE_POLICY, &bad, &p.id, None).is_err());
        assert_eq!(lib.all_files().len(), 2, "failed imports leave nothing behind");
    }

    #[test]
    fn first_run_seeds_once() {
        let (_t, lib) = Tmp::new();
        let e = lib.seed_first_run(|| doc("demo", 1)).unwrap().unwrap();
        let projects = lib.list_projects();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].name, SEED_PROJECT);
        assert_eq!(lib.load_file(&e.id).unwrap().nodes.len(), 2);
        assert!(lib.seed_first_run(|| doc("demo", 1)).unwrap().is_none());
        lib.delete_project(&projects[0].id).unwrap();
        assert!(lib.seed_first_run(|| doc("demo", 1)).unwrap().is_none(), "a deleted seed does not come back");
    }

    #[test]
    fn default_root_honours_the_env_override() {
        // Only this test touches the variable.
        std::env::set_var("DESIGMA_LIBRARY", "/tmp/somewhere-else");
        assert_eq!(Library::default_root(), PathBuf::from("/tmp/somewhere-else"));
        // The old name still works, but the new one wins.
        std::env::set_var("DESIGMA_LIBRARY_DIR", "/tmp/old-name");
        assert_eq!(Library::default_root(), PathBuf::from("/tmp/somewhere-else"));
        std::env::remove_var("DESIGMA_LIBRARY");
        assert_eq!(Library::default_root(), PathBuf::from("/tmp/old-name"));
        std::env::remove_var("DESIGMA_LIBRARY_DIR");
        assert!(Library::default_root().ends_with("desigma/library") || Library::default_root().ends_with("desigma\\library"));
    }

    #[test]
    fn times_are_strictly_increasing() {
        let a = now_ms();
        let b = now_ms();
        assert!(b > a);
    }
}
