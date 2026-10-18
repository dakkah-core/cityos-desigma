//! Generic content-addressed storage for trusted native extensions.
//!
//! This layer owns bytes and integrity only. Extensions own authorization,
//! retention roots, portable-export policy and their transaction state machines.
use crate::{write_atomic, Library, LibraryError, Result};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

pub fn content_digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// A namespace is one canonical path component below the selected library.
fn namespace(value: &str) -> Result<()> {
    let device = matches!(value, "con" | "prn" | "aux" | "nul")
        || value.strip_prefix("com").or_else(|| value.strip_prefix("lpt"))
            .is_some_and(|suffix| suffix.len() == 1 && matches!(suffix.as_bytes()[0], b'1'..=b'9'));
    if value.is_empty() || value.len() > 80
        || !value.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_'))
        || device || matches!(value, "projects" | "trash" | "versions" | "libraries" | "settings")
    {
        return Err(LibraryError::Invalid("Invalid extension payload namespace".into()));
    }
    Ok(())
}

fn digest(value: &str) -> Result<()> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(LibraryError::Invalid("Invalid payload digest".into()));
    }
    Ok(())
}

fn reject_link(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            #[cfg(windows)]
            let reparse = {
                use std::os::windows::fs::MetadataExt;
                metadata.file_attributes() & 0x400 != 0
            };
            #[cfg(not(windows))]
            let reparse = false;
            if metadata.file_type().is_symlink() || reparse {
                return Err(LibraryError::Invalid("Extension payload paths cannot contain links or reparse points".into()));
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// Cloneable byte-store handle, with no document or submission mutation API.
#[derive(Clone, Debug)]
pub struct PayloadStore {
    library_root: PathBuf,
    namespace: String,
}

impl Library {
    /// Does not create directories. Existing extension layouts remain stable:
    /// `<library>/<namespace>/payloads/<sha256>`.
    pub fn payloads(&self, name: &str) -> Result<PayloadStore> {
        namespace(name)?;
        Ok(PayloadStore { library_root: self.root().to_path_buf(), namespace: name.into() })
    }
}

impl PayloadStore {
    fn path(&self, hash: &str) -> Result<PathBuf> {
        digest(hash)?;
        let scope = self.library_root.join(&self.namespace);
        let directory = scope.join("payloads");
        let path = directory.join(hash);
        // The selected library root is the authority boundary; extension-owned
        // descendants cannot redirect byte reads or writes through a link.
        for candidate in [&scope, &directory, &path] { reject_link(candidate)?; }
        Ok(path)
    }

    pub fn retain(&self, bytes: &[u8]) -> Result<String> {
        let hash = content_digest(bytes);
        self.put(&hash, bytes)?;
        Ok(hash)
    }

    /// Verified objects are a no-op. A damaged object can only be repaired by
    /// supplying the exact bytes identified by the requested digest.
    pub fn put(&self, expected: &str, bytes: &[u8]) -> Result<()> {
        let path = self.path(expected)?;
        if content_digest(bytes) != expected {
            return Err(LibraryError::Invalid("Dependency digest mismatch".into()));
        }
        match self.verify(expected) {
            Ok(()) => return Ok(()),
            Err(LibraryError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {},
            Err(LibraryError::Invalid(_)) => {},
            Err(error) => return Err(error),
        }
        write_atomic(&path, bytes)?;
        self.verify(expected)
    }

    /// Verify without allocating the complete payload.
    pub fn verify(&self, hash: &str) -> Result<()> {
        let mut file = std::fs::File::open(self.path(hash)?)?;
        let mut digest = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 { break; }
            digest.update(&buffer[..read]);
        }
        if format!("{:x}", digest.finalize()) != hash {
            return Err(LibraryError::Invalid("Dependency digest mismatch".into()));
        }
        Ok(())
    }

    pub fn load(&self, hash: &str) -> Result<Vec<u8>> {
        let bytes = std::fs::read(self.path(hash)?)?;
        if content_digest(&bytes) != hash {
            return Err(LibraryError::Invalid("Dependency digest mismatch".into()));
        }
        Ok(bytes)
    }
}
