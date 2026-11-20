//! What a guest keeps between sessions (image bytes, by content digest) and what it asks the host
//! for (fonts it lacks).
//!
//! Image keys in a document (`img-3`) are only unique inside that document, so the cache is
//! addressed by the SHA-256 of the bytes. The host tells a joining guest each key's digest; the
//! guest asks only for the ones it cannot read from the cache, and checks what arrives against the
//! digest it was told.

use desigma_fonts::FontRequest;
use std::path::PathBuf;
use std::time::SystemTime;

/// Largest font file shared with a guest.
pub const MAX_FONT: usize = 20 * 1024 * 1024;
/// The image cache stays under this many bytes.
pub const IMAGE_CACHE_LIMIT: u64 = 500 * 1024 * 1024;

pub use desigma_collab::wire::{digest, FontKey};

fn valid_digest(d: &str) -> bool {
    d.len() == 64 && d.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The font a request names.
pub fn font_key(r: &FontRequest) -> FontKey {
    FontKey { family: r.family.clone(), weight: r.weight, italic: r.italic }
}

/// Where fonts come from on the host and where they go on the guest.
pub trait FontIo {
    /// The file of the installed font for `key`, if this machine has it.
    fn file(&self, key: &FontKey) -> Option<Vec<u8>>;
    /// Makes `bytes` available to text layout for this run of the app only.
    fn install(&self, bytes: Vec<u8>);
}

/// The real thing: installed fonts out, the renderer's font system in. Nothing is written to the
/// operating system.
pub struct SystemFonts;

impl FontIo for SystemFonts {
    fn file(&self, key: &FontKey) -> Option<Vec<u8>> {
        desigma_render::font_file(&key.family, key.weight, key.italic)
    }
    fn install(&self, bytes: Vec<u8>) {
        if looks_like_font(&bytes) {
            desigma_export::register_font(bytes.clone());
            desigma_render::register_font(bytes);
        }
    }
}

/// TrueType, OpenType or a collection: what the font database can load.
fn looks_like_font(b: &[u8]) -> bool {
    matches!(b.get(..4), Some([0, 1, 0, 0]) | Some(b"OTTO") | Some(b"true") | Some(b"ttcf"))
}

pub struct FontIoBox(pub Box<dyn FontIo>);

impl Default for FontIoBox {
    fn default() -> Self {
        Self(Box::new(SystemFonts))
    }
}

/// Image bytes by content digest in a directory, least recently used dropped first.
pub struct ImageCache {
    dir: PathBuf,
    limit: u64,
    /// Bytes on disk, counted once and kept up to date.
    total: Option<u64>,
}

impl Default for ImageCache {
    fn default() -> Self {
        Self::with_dir(desigma_fonts::app_cache_dir().join("collab-images"), IMAGE_CACHE_LIMIT)
    }
}

impl ImageCache {
    pub fn with_dir(dir: PathBuf, limit: u64) -> Self {
        Self { dir, limit, total: None }
    }

    fn path(&self, digest: &str) -> Option<PathBuf> {
        valid_digest(digest).then(|| self.dir.join(format!("{digest}.bin")))
    }

    /// The bytes for `digest`, if cached and intact. Counts as a use.
    pub fn get(&self, digest: &str) -> Option<Vec<u8>> {
        let path = self.path(digest)?;
        let bytes = std::fs::read(&path).ok()?;
        if self::digest(&bytes) != digest {
            let _ = std::fs::remove_file(&path);
            return None;
        }
        touch(&path);
        Some(bytes)
    }

    #[cfg(test)]
    pub fn dir(&self) -> &std::path::Path {
        &self.dir
    }

    #[cfg(test)]
    pub fn contains(&self, digest: &str) -> bool {
        self.path(digest).is_some_and(|p| p.is_file())
    }

    /// Stores `bytes` under their `digest` (the caller has checked it) and trims the cache.
    pub fn put(&mut self, digest: &str, bytes: &[u8]) {
        let Some(path) = self.path(digest) else { return };
        if bytes.is_empty() || bytes.len() as u64 > self.limit / 2 {
            return;
        }
        if path.is_file() {
            touch(&path);
            return;
        }
        if std::fs::create_dir_all(&self.dir).is_err() {
            return;
        }
        let total = self.total();
        let tmp = path.with_extension("part");
        if std::fs::write(&tmp, bytes).is_err() || std::fs::rename(&tmp, &path).is_err() {
            let _ = std::fs::remove_file(&tmp);
            return;
        }
        self.total = Some(total + bytes.len() as u64);
        if total + bytes.len() as u64 > self.limit {
            self.trim(&path);
        }
    }

    fn total(&mut self) -> u64 {
        if self.total.is_none() {
            self.total = Some(self.entries().iter().map(|e| e.1).sum());
        }
        self.total.unwrap_or(0)
    }

    /// `(path, size, last use)` of every cached file.
    fn entries(&self) -> Vec<(PathBuf, u64, SystemTime)> {
        let Ok(dir) = std::fs::read_dir(&self.dir) else { return Vec::new() };
        dir.filter_map(Result::ok)
            .filter_map(|e| {
                let m = e.metadata().ok()?;
                m.is_file().then(|| (e.path(), m.len(), m.modified().unwrap_or(SystemTime::UNIX_EPOCH)))
            })
            .collect()
    }

    /// Deletes the least recently used files (never `keep`) until the cache is at 90% of its limit.
    fn trim(&mut self, keep: &std::path::Path) {
        let mut all = self.entries();
        all.sort_by_key(|e| e.2);
        let mut total: u64 = all.iter().map(|e| e.1).sum();
        let goal = self.limit / 10 * 9;
        for (path, size, _) in all {
            if total <= goal {
                break;
            }
            if path != keep && std::fs::remove_file(&path).is_ok() {
                total -= size;
            }
        }
        self.total = Some(total);
    }
}

/// Marks a cached file as just used (its mtime orders the eviction).
fn touch(path: &std::path::Path) {
    if let Ok(f) = std::fs::OpenOptions::new().write(true).open(path) {
        let _ = f.set_modified(SystemTime::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("desigma-img-cache-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn digest_is_sha256_hex() {
        assert_eq!(digest(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn stores_reads_and_refuses_what_does_not_match() {
        let mut c = ImageCache::with_dir(dir("rw"), 1 << 20);
        let d = digest(b"png bytes");
        assert!(c.get(&d).is_none() && !c.contains(&d));
        c.put(&d, b"png bytes");
        assert_eq!(c.get(&d).as_deref(), Some(&b"png bytes"[..]));
        // a damaged file is dropped, not served
        std::fs::write(c.path(&d).unwrap(), b"tampered").unwrap();
        assert!(c.get(&d).is_none() && !c.contains(&d));
        // names that are not digests never touch the disk
        assert!(c.get("../../etc/passwd").is_none());
        c.put("img-1", b"x");
        assert!(c.path("img-1").is_none());
    }

    #[test]
    fn evicts_the_least_recently_used_to_stay_bounded() {
        let mut c = ImageCache::with_dir(dir("lru"), 1000);
        let blob = |n: u8| vec![n; 300];
        let ds: Vec<String> = (0..4).map(|n| digest(&blob(n))).collect();
        for (i, d) in ds.iter().take(3).enumerate() {
            c.put(d, &blob(i as u8));
            // distinct ages whatever the file system's clock granularity
            let f = std::fs::OpenOptions::new().write(true).open(c.path(d).unwrap()).unwrap();
            f.set_modified(SystemTime::now() - Duration::from_secs(100 - i as u64 * 10)).unwrap();
        }
        // reading the oldest makes it the newest
        assert!(c.get(&ds[0]).is_some());
        c.put(&ds[3], &blob(3));
        assert!(c.entries().iter().map(|e| e.1).sum::<u64>() <= 1000);
        assert!(c.contains(&ds[0]) && c.contains(&ds[3]), "recent entries survive");
        assert!(!c.contains(&ds[1]), "the least recently used went first");
        // one file bigger than half the budget is not kept at all
        let big = vec![1u8; 600];
        c.put(&digest(&big), &big);
        assert!(!c.contains(&digest(&big)));
    }

    #[test]
    fn a_fresh_cache_object_counts_what_is_already_there() {
        let d = dir("recount");
        let mut a = ImageCache::with_dir(d.clone(), 700);
        let (x, y, z) = (vec![1u8; 300], vec![2u8; 300], vec![3u8; 300]);
        a.put(&digest(&x), &x);
        a.put(&digest(&y), &y);
        let mut b = ImageCache::with_dir(d, 700);
        b.put(&digest(&z), &z);
        assert!(b.entries().iter().map(|e| e.1).sum::<u64>() <= 700);
        assert!(b.contains(&digest(&z)));
    }
}
