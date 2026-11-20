//! The server's secret key: it fixes the endpoint id inside the join link, so the link survives
//! restarts. Anyone who reads the file can impersonate the server; it is created readable by the
//! owner only (Unix), and should be kept out of backups and repositories.

use std::{
    io::Write as _,
    path::{Path, PathBuf},
};

/// `.<stem>.desigma-key` next to the document.
pub fn default_key_path(doc: &Path) -> PathBuf {
    let stem = doc.file_stem().and_then(|s| s.to_str()).unwrap_or("document");
    doc.with_file_name(format!(".{stem}.desigma-key"))
}

/// Reads the key at `path`, or creates a new random one there. A file that exists but is not a
/// key is an error: replacing it silently would change the link.
pub fn load_or_create_key(path: &Path) -> Result<[u8; 32], String> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(text.trim()).ok_or_else(|| format!("{} is not a desigma key file (expected 64 hex digits)", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let key = desigma_net::generate_secret_key();
            create(path, &key).map_err(|e| format!("cannot write key file {}: {e}", path.display()))?;
            Ok(key)
        }
        Err(e) => Err(format!("cannot read key file {}: {e}", path.display())),
    }
}

fn create(path: &Path, key: &[u8; 32]) -> std::io::Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    f.write_all(format!("{}\n", hex(key)).as_bytes())
}

fn hex(b: &[u8; 32]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn parse(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 || !s.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip() {
        let mut k = [0u8; 32];
        for (i, b) in k.iter_mut().enumerate() {
            *b = (i * 8 + 1) as u8;
        }
        assert_eq!(parse(&hex(&k)), Some(k));
        assert_eq!(parse("zz"), None);
        assert_eq!(parse(&"g".repeat(64)), None);
    }

    #[test]
    fn default_path_is_hidden_next_to_the_file() {
        assert_eq!(default_key_path(Path::new("/srv/a/design.desigma")), Path::new("/srv/a/.design.desigma-key"));
    }
}
