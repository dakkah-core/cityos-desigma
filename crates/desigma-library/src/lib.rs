//! The projects library: everything desigma keeps on disk between sessions.
//!
//! Layout under [`Library::root`]:
//!
//! ```text
//! projects/<project-id>/project.json        project name, folders, file index
//! projects/<project-id>/files/<file-id>.desigma
//! projects/<project-id>/thumbs/<file-id>.png
//! trash/<file-id>/...                       restorable until emptied by the user
//! versions/<file-id>/<n>.desigma, index.json  version history (`versions.rs`)
//! settings.json                             app settings kept by the editor (the author name for comments)
//! libraries/<library-id>/<version>.json     published library snapshots (`publish.rs`)
//! ```
//!
//! The modules are owned separately: `store` (projects, files, folders,
//! recents, thumbnails, trash), `publish` (shared libraries) and `versions`
//! (version history). Each adds `impl Library` blocks in its own file.

mod publish;
mod store;
mod versions;
mod submissions;
mod checkpoint;

pub use publish::*;
pub use store::*;
pub use versions::*;
pub use submissions::*;
pub use checkpoint::CheckpointSaveReceipt;

use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("{0}")]
    Invalid(String),
    #[error("Checkpoint baseline changed for {0}; reload/reconcile before saving")]
    CheckpointConflict(String),
    #[error("Checkpoint save for {file_id} requires reconciliation; save: {save_error}; recovery: {recovery_error}")]
    ReconciliationRequired { file_id: String, save_error: String, recovery_error: String },
}

pub type Result<T> = std::result::Result<T, LibraryError>;

pub(crate) fn validate_id(id: &str) -> Result<()> {
    if id.is_empty() || id.len() > 160 || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        return Err(LibraryError::Invalid("Invalid library identity".into()));
    }
    Ok(())
}

/// Handle to the on-disk library. Cheap to clone; all state lives on disk.
#[derive(Clone, Debug)]
pub struct Library {
    root: PathBuf,
}

impl Library {
    /// Opens (creating if needed) a library rooted at `root`.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        std::fs::create_dir_all(&root)?;
        let library=Self { root };
        let _guard = store::write_lock();
        library.recover_pending_saves()?;
        library.recover_pending_versions()?;
        Ok(library)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Joins a relative path under the root.
    pub fn path(&self, rel: impl AsRef<Path>) -> PathBuf {
        self.root.join(rel)
    }
}

/// Stable random-looking id for new projects, files and libraries.
pub fn new_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let t = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let c = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{:x}{:04x}", t, c & 0xffff)
}
