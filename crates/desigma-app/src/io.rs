//! Opening and saving documents.

use desigma_doc::{Document, Vec2};
use std::path::{Path, PathBuf};

pub const NATIVE_EXT: &str = "desigma";

/// Loads a document from disk, dispatching on the file extension.
pub fn load_path(path: &Path) -> Result<Document, String> {
    load_path_observed(path).map(|(document, _)| document)
}

/// The observation belongs to the bytes parsed here, before view normalization.
pub fn load_path_observed(path: &Path) -> Result<(Document, String), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
    let fingerprint = desigma_library::payload_digest(&bytes);
    parse_path_bytes(path, &bytes).map(|document| (document, fingerprint))
}

fn parse_path_bytes(path: &Path, bytes: &[u8]) -> Result<Document, String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let text = || std::str::from_utf8(bytes).map_err(|e| format!("Cannot read {}: {e}", path.display()));
    let mut doc = match ext.as_str() {
        "fig" => desigma_fig::import_fig(bytes).map_err(|e| format!("Cannot import .fig: {e}"))?,
        "json" => {
            let text = text()?;
            match desigma_fig::import_rest_json(&text) {
                Ok(d) => d,
                Err(fig_err) => Document::from_json(&text).map_err(|e| format!("Not a Figma REST file ({fig_err}) nor a desigma file ({e})"))?,
            }
        }
        "svg" => {
            let text = text()?;
            let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("svg");
            let mut doc = Document::new(name);
            let page = doc.add_page("Page 1");
            desigma_svg::import_svg_named(&text, name, &mut doc, page, Vec2::ZERO).map_err(|e| format!("Cannot import SVG: {e}"))?;
            doc
        }
        _ => {
            let text = text()?;
            Document::from_json(&text).map_err(|e| format!("Invalid desigma file: {e}"))?
        }
    };
    if doc.name.is_empty() {
        doc.name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Untitled").to_string();
    }
    ensure_page(&mut doc);
    Ok(doc)
}

pub fn ensure_page(doc: &mut Document) {
    doc.pages.retain(|p| doc.nodes.contains_key(p));
    if doc.pages.is_empty() {
        doc.add_page("Page 1");
    }
}

/// Writes the native format; returns the final path (extension enforced).
pub fn save_path(doc: &Document, path: &Path) -> Result<PathBuf, String> {
    save_path_observed(doc, path).map(|(path, _)| path)
}

/// Explicit new/copy save, returning the exact written bytes' identity.
pub fn save_path_observed(doc: &Document, path: &Path) -> Result<(PathBuf, String), String> {
    save_path_inner(doc, path, None)
}

pub fn file_fingerprint(path: &Path) -> Result<String, String> {
    fingerprint_locked(path)
}

fn fingerprint_locked(path: &Path) -> Result<String, String> {
    std::fs::read(path)
        .map(|bytes| desigma_library::payload_digest(&bytes))
        .map_err(|error| format!("Cannot read saved generation {}: {error}", path.display()))
}

/// Cooperating Desigma processes serialize comparison and atomic replacement.
/// A conflict leaves both the local candidate and current disk bytes intact.
pub fn save_path_expected(doc: &Document, path: &Path, expected: &str) -> Result<(PathBuf, String), String> {
    save_path_inner(doc, path, Some(expected))
}

fn save_path_inner(doc: &Document, path: &Path, expected: Option<&str>) -> Result<(PathBuf, String), String> {
    let mut p = path.to_path_buf();
    if p.extension().is_none() {
        p.set_extension(NATIVE_EXT);
    }
    let _guard = external_lock(&p, true)?;
    if let Some(expected) = expected {
        if fingerprint_locked(&p)? != expected {
            return Err("The file changed on disk. Keep this draft and review the newer file before saving.".into());
        }
    }
    let json = doc.to_json().map_err(|e| format!("Cannot serialize: {e}"))?;
    desigma_library::write_atomic(&p, json.as_bytes()).map_err(|e| format!("Cannot write {}: {e}", p.display()))?;
    Ok((p, desigma_library::payload_digest(json.as_bytes())))
}

fn external_lock(path: &Path, create_parent: bool) -> Result<std::fs::File, String> {
    let parent = path.parent().filter(|parent| !parent.as_os_str().is_empty()).unwrap_or(Path::new("."));
    if create_parent {
        std::fs::create_dir_all(parent).map_err(|error| format!("Cannot create save directory: {error}"))?;
    }
    let parent = std::fs::canonicalize(parent).map_err(|error| format!("Cannot open save directory: {error}"))?;
    let name = path.file_name().ok_or("A document file name is required")?;
    let target = parent.join(name);
    match std::fs::symlink_metadata(&target) {
        Ok(metadata) if !metadata.is_file() || metadata.file_type().is_symlink() => return Err("A regular document file is required".into()),
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("Cannot inspect document file: {error}")),
    }
    let mut lock_name = std::ffi::OsString::from(".");
    lock_name.push(name);
    lock_name.push(".save.lock");
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(parent.join(lock_name))
        .map_err(|error| format!("Cannot coordinate document save: {error}"))?;
    match lock.try_lock() {
        Ok(()) => Ok(lock),
        Err(std::fs::TryLockError::WouldBlock) => Err("The document is busy in another Desigma process. Retry saving.".into()),
        Err(std::fs::TryLockError::Error(error)) => Err(format!("Cannot lock document save: {error}")),
    }
}

/// External replacement must never bypass a library checkpoint/index transaction.
pub(crate) fn inside_library_storage(path: &Path, root: &Path) -> Result<bool, String> {
    let root = std::fs::canonicalize(root).map_err(|error| format!("Cannot verify library storage: {error}"))?;
    let absolute = std::path::absolute(path).map_err(|error| format!("Cannot resolve save target: {error}"))?;
    let mut parent = absolute.as_path();
    loop {
        match std::fs::canonicalize(parent) {
            Ok(existing) => return Ok(existing.starts_with(&root)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                parent = parent.parent().ok_or("Cannot resolve save target ancestry")?;
            }
            Err(error) => return Err(format!("Cannot verify save target: {error}")),
        }
    }
}

pub fn pick_open() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .add_filter("Design files", &["fig", "json", "desigma", "svg"])
        .add_filter("Figma file", &["fig"])
        .add_filter("SVG", &["svg"])
        .add_filter("JSON", &["json"])
        .add_filter("desigma", &[NATIVE_EXT])
        .pick_file()
}

pub fn pick_save(default_name: &str) -> Option<PathBuf> {
    rfd::FileDialog::new().add_filter("desigma", &[NATIVE_EXT]).set_file_name(format!("{default_name}.{NATIVE_EXT}")).save_file()
}

pub fn pick_export(default_name: &str) -> Option<PathBuf> {
    rfd::FileDialog::new().add_filter("PNG", &["png"]).set_file_name(format!("{default_name}.png")).save_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use desigma_doc::{Node, Rect};

    struct ExternalTemp(PathBuf);
    impl ExternalTemp {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("desigma_external_{}_{}", std::process::id(), desigma_library::new_id()));
            std::fs::create_dir(&root).unwrap();
            Self(root)
        }
        fn file(&self) -> PathBuf {
            self.0.join("draft.desigma")
        }
    }
    impl Drop for ExternalTemp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn stale_external_save_preserves_the_newer_document_and_local_candidate() {
        let temp = ExternalTemp::new();
        let path = temp.file();
        save_path(&Document::new("base"), &path).unwrap();
        let (mut first, observation) = load_path_observed(&path).unwrap();
        let (mut second, same_observation) = load_path_observed(&path).unwrap();
        assert_eq!(observation, same_observation);
        second.name = "other writer".into();
        let (_, next) = save_path_expected(&second, &path, &same_observation).unwrap();
        let persisted = std::fs::read(&path).unwrap();
        first.name = "keep local edits".into();
        assert!(save_path_expected(&first, &path, &observation).unwrap_err().contains("changed on disk"));
        assert_eq!(std::fs::read(&path).unwrap(), persisted);
        assert_eq!(first.name, "keep local edits");
        assert_eq!(file_fingerprint(&path).unwrap(), next);
    }

    #[test]
    fn external_lock_child() {
        let Some(directory) = std::env::var_os("DESIGMA_EXTERNAL_LOCK_CHILD") else { return };
        let root = PathBuf::from(directory);
        let _guard = external_lock(&root.join("draft.desigma"), false).unwrap();
        std::fs::write(root.join("ready"), b"locked").unwrap();
        while !root.join("release").exists() {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn external_saves_fail_busy_across_processes_and_recover_after_exit() {
        let temp = ExternalTemp::new();
        let path = temp.file();
        save_path(&Document::new("base"), &path).unwrap();
        let fingerprint = file_fingerprint(&path).unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["io::tests::external_lock_child", "--exact", "--nocapture"])
            .env("DESIGMA_EXTERNAL_LOCK_CHILD", &temp.0)
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let started = std::time::Instant::now();
        while !temp.0.join("ready").exists() && started.elapsed().as_secs() < 10 {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let ready = temp.0.join("ready").exists();
        let busy = if ready { save_path_expected(&Document::new("candidate"), &path, &fingerprint).unwrap_err() } else { String::new() };
        // Always release/stop our own helper before asserting to avoid an orphan.
        child.kill().ok();
        child.wait().unwrap();
        assert!(ready, "child did not acquire the external file lock");
        assert!(busy.contains("busy"), "{busy}");
        assert_eq!(file_fingerprint(&path).unwrap(), fingerprint);
        save_path_expected(&Document::new("retry"), &path, &fingerprint).unwrap();
        assert_eq!(load_path(&path).unwrap().name, "retry");
    }

    #[cfg(windows)]
    #[test]
    fn windows_failed_external_replacement_preserves_bytes_and_can_retry() {
        use std::os::windows::fs::OpenOptionsExt;
        let temp = ExternalTemp::new();
        let path = temp.file();
        save_path(&Document::new("base"), &path).unwrap();
        let before = std::fs::read(&path).unwrap();
        let observation = file_fingerprint(&path).unwrap();
        let held = std::fs::OpenOptions::new().read(true).share_mode(3).open(&path).unwrap();
        let candidate = Document::new("retry candidate");
        assert!(save_path_expected(&candidate, &path, &observation).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        drop(held);
        save_path_expected(&candidate, &path, &observation).unwrap();
        assert_eq!(load_path(&path).unwrap().name, candidate.name);
    }

    fn tmp(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("desigma_test_{}_{name}", std::process::id()));
        p
    }

    #[test]
    fn native_roundtrip() {
        let mut d = Document::new("Saved");
        let page = d.add_page("P");
        let id = d.alloc_id();
        d.insert(page, Node::rectangle(id, Rect::new(1.0, 2.0, 3.0, 4.0))).unwrap();
        let p = save_path(&d, &tmp("a")).unwrap();
        assert_eq!(p.extension().unwrap(), NATIVE_EXT);
        let back = load_path(&p).unwrap();
        assert_eq!(back.name, "Saved");
        assert_eq!(back.nodes.len(), 2);
        std::fs::remove_file(p).ok();
    }

    #[test]
    fn json_falls_back_to_native() {
        let mut d = Document::new("J");
        d.add_page("P");
        let p = tmp("b.json");
        std::fs::write(&p, d.to_json().unwrap()).unwrap();
        let back = load_path(&p).unwrap();
        assert_eq!(back.name, "J");
        std::fs::remove_file(p).ok();
    }

    #[test]
    fn errors_are_readable() {
        let p = tmp("c.desigma");
        std::fs::write(&p, "not json").unwrap();
        assert!(load_path(&p).unwrap_err().contains("Invalid desigma file"));
        std::fs::remove_file(p).ok();
        assert!(load_path(Path::new("/definitely/missing.fig")).is_err());
    }

    #[test]
    fn svg_loads_as_a_document_named_after_the_file() {
        let p = tmp("icon.svg");
        std::fs::write(&p, r##"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><rect width="8" height="8" fill="#f00"/></svg>"##).unwrap();
        let d = load_path(&p).unwrap();
        std::fs::remove_file(&p).ok();
        assert!(d.name.ends_with("icon"));
        assert_eq!(d.pages.len(), 1);
        let page = d.get(d.pages[0]).unwrap();
        assert_eq!(page.name, "Page 1");
        assert_eq!(page.children.len(), 1);
        assert_eq!(d.get(page.children[0]).unwrap().name, d.name);
    }

    #[test]
    fn empty_doc_gets_a_page() {
        let mut d = Document::new("E");
        ensure_page(&mut d);
        assert_eq!(d.pages.len(), 1);
    }
}
