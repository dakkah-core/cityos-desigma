//! What a release offers and which of it fits this machine: the asset names `release.yml` publishes, the
//! `SHA256SUMS` file that goes with them, and the hosts a download may come from.

use semver::Version;
use url::Url;

/// One file attached to a GitHub release.
#[derive(Clone, Debug, PartialEq)]
pub struct Asset {
    pub name: String,
    /// GitHub's `browser_download_url`.
    pub url: String,
    pub size: u64,
}

/// The operating system an update is for. Chosen at run time (`Os::current`), so the code for every system
/// is compiled and tested everywhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Windows,
    Mac,
    Linux,
}

impl Os {
    pub fn current() -> Os {
        if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::Mac
        } else {
            Os::Linux
        }
    }
}

/// How this copy of desigma was installed, which decides the asset and how it is put in place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Windows, installed by the Inno Setup wizard: the update runs the new setup silently.
    WindowsInstaller,
    /// Windows, unzipped somewhere: `desigma.exe` is swapped in place.
    WindowsPortable,
    /// macOS: the `desigma.app` bundle is replaced.
    MacApp,
    /// Linux, from the tarball (`install.sh` or run in place): the binary is replaced.
    LinuxTarball,
}

/// The file name `release.yml` publishes for `kind` at `version` (no leading `v`).
pub fn asset_name(kind: Kind, version: &Version) -> String {
    match kind {
        Kind::WindowsInstaller => format!("desigma-{version}-x86_64-windows-setup.exe"),
        Kind::WindowsPortable => format!("desigma-{version}-x86_64-pc-windows-msvc.zip"),
        Kind::MacApp => format!("desigma-{version}-macos-universal.tar.gz"),
        Kind::LinuxTarball => format!("desigma-{version}-x86_64-unknown-linux-gnu.tar.gz"),
    }
}

/// The release asset for `kind`, found by its exact name.
pub fn pick<'a>(assets: &'a [Asset], kind: Kind, version: &Version) -> Result<&'a Asset, String> {
    let name = asset_name(kind, version);
    assets.iter().find(|a| a.name == name).ok_or_else(|| format!("this release has no {name}"))
}

/// The release's checksum file.
pub fn sums(assets: &[Asset]) -> Result<&Asset, String> {
    assets.iter().find(|a| a.name == "SHA256SUMS").ok_or_else(|| "this release has no SHA256SUMS file".to_string())
}

/// The SHA-256 (lowercase hex) that `SHA256SUMS` lists for `name`. Lines look like `<64 hex>  <name>` (text
/// mode) or `<64 hex> *<name>` (binary mode). A missing entry, a malformed digest or two different digests for
/// one name are refused.
pub fn expected_digest(sums: &str, name: &str) -> Result<String, String> {
    let mut found: Option<String> = None;
    for line in sums.lines() {
        let Some((digest, file)) = line.trim().split_once(char::is_whitespace) else { continue };
        let file = file.trim_start().trim_start_matches('*');
        if file.strip_prefix("./").unwrap_or(file) != name {
            continue;
        }
        if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(format!("SHA256SUMS has a malformed entry for {name}"));
        }
        let digest = digest.to_ascii_lowercase();
        if found.as_ref().is_some_and(|d| *d != digest) {
            return Err(format!("SHA256SUMS lists {name} twice with different checksums"));
        }
        found = Some(digest);
    }
    found.ok_or_else(|| format!("SHA256SUMS has no entry for {name}"))
}

const REPO_DOWNLOADS: &str = "/deepfounder-ai/desigma/releases/download/";
/// GitHub's own hosts for release files: an asset URL on github.com redirects to one of these.
const CDN_HOSTS: [&str; 2] = ["objects.githubusercontent.com", "release-assets.githubusercontent.com"];

/// The URL of a release asset as the API lists it: https on github.com, below this repository's releases.
pub fn check_asset_url(url: &str) -> Result<Url, String> {
    check(url, false)
}

/// A URL a redirect may lead to: the same, or one of GitHub's release file hosts.
pub fn check_redirect_url(url: &str) -> Result<Url, String> {
    check(url, true)
}

fn check(url: &str, redirect: bool) -> Result<Url, String> {
    let refuse = || format!("refusing to download from {}", url.split(['?', '#']).next().unwrap_or(url));
    let parsed = Url::parse(url).map_err(|_| refuse())?;
    #[cfg(test)]
    if local_test_server(&parsed) {
        return Ok(parsed);
    }
    if parsed.scheme() != "https" || !parsed.username().is_empty() || parsed.password().is_some() || parsed.port().is_some() {
        return Err(refuse());
    }
    match parsed.host_str() {
        Some("github.com") if parsed.path().starts_with(REPO_DOWNLOADS) => Ok(parsed),
        Some(host) if redirect && CDN_HOSTS.contains(&host) => Ok(parsed),
        _ => Err(refuse()),
    }
}

/// Tests download from a `TcpListener` on localhost over plain http; production builds never do.
#[cfg(test)]
fn local_test_server(url: &Url) -> bool {
    url.scheme() == "http" && url.host_str() == Some("127.0.0.1")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    fn asset(name: &str) -> Asset {
        Asset { name: name.into(), url: format!("https://github.com/deepfounder-ai/desigma/releases/download/v1/{name}"), size: 1 }
    }

    #[test]
    fn each_kind_gets_its_own_file() {
        let version = v("0.1.0-alpha.8");
        let names = [
            (Kind::WindowsInstaller, "desigma-0.1.0-alpha.8-x86_64-windows-setup.exe"),
            (Kind::WindowsPortable, "desigma-0.1.0-alpha.8-x86_64-pc-windows-msvc.zip"),
            (Kind::MacApp, "desigma-0.1.0-alpha.8-macos-universal.tar.gz"),
            (Kind::LinuxTarball, "desigma-0.1.0-alpha.8-x86_64-unknown-linux-gnu.tar.gz"),
        ];
        let all: Vec<Asset> = names.iter().map(|(_, n)| asset(n)).chain([asset("desigma-0.1.0-alpha.8-macos.dmg"), asset("SHA256SUMS")]).collect();
        for (kind, name) in names {
            assert_eq!(asset_name(kind, &version), name);
            assert_eq!(pick(&all, kind, &version).unwrap().name, name);
        }
        assert_eq!(sums(&all).unwrap().name, "SHA256SUMS");
        // another version's file never matches, and a release without the file says which one is missing
        assert!(pick(&all, Kind::MacApp, &v("0.1.0-alpha.9")).unwrap_err().contains("macos-universal.tar.gz"));
        assert!(pick(&[], Kind::LinuxTarball, &version).is_err());
        assert!(sums(&all[..4]).is_err());
    }

    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB";

    #[test]
    fn checksum_lines_are_parsed() {
        let sums = format!("{A}  desigma-1-macos-universal.tar.gz\r\n{B} *desigma-1-x86_64-windows-setup.exe\n\nnot a checksum line\n{A}  ./desigma-1.zip\n");
        assert_eq!(expected_digest(&sums, "desigma-1-macos-universal.tar.gz").unwrap(), A);
        assert_eq!(expected_digest(&sums, "desigma-1-x86_64-windows-setup.exe").unwrap(), B.to_lowercase(), "binary mode, upper case");
        assert_eq!(expected_digest(&sums, "desigma-1.zip").unwrap(), A);
        assert!(expected_digest(&sums, "desigma-2.zip").unwrap_err().contains("no entry"));
        // a name that only ends the same way is another file
        assert!(expected_digest(&sums, "tar.gz").is_err());
    }

    #[test]
    fn bad_checksum_files_are_refused() {
        assert!(expected_digest("", "x").is_err());
        assert!(expected_digest("abc  x\n", "x").unwrap_err().contains("malformed"));
        assert!(expected_digest(&format!("{}g  x\n", &A[1..]), "x").unwrap_err().contains("malformed"));
        assert!(expected_digest(&format!("{A}  x\n{}  x\n", "b".repeat(64)), "x").unwrap_err().contains("twice"));
        assert_eq!(expected_digest(&format!("{A}  x\n{A}  x\n"), "x").unwrap(), A, "the same line twice is fine");
    }

    #[test]
    fn only_this_repositorys_release_files_may_be_fetched() {
        let ok = "https://github.com/deepfounder-ai/desigma/releases/download/v0.1.0-alpha.8/SHA256SUMS";
        assert!(check_asset_url(ok).is_ok());
        assert!(check_redirect_url(ok).is_ok());
        for bad in [
            "http://github.com/deepfounder-ai/desigma/releases/download/v1/x",
            "https://github.com/other/repo/releases/download/v1/x",
            "https://github.com/deepfounder-ai/desigma/archive/main.zip",
            "https://github.com/deepfounder-ai/desigma/releases/download/../../../../evil/x/releases/download/x",
            "https://github.com.evil.com/deepfounder-ai/desigma/releases/download/v1/x",
            "https://evil.com/github.com/deepfounder-ai/desigma/releases/download/v1/x",
            "https://user@github.com/deepfounder-ai/desigma/releases/download/v1/x",
            "https://github.com:8443/deepfounder-ai/desigma/releases/download/v1/x",
            "https://objects.githubusercontent.com/github-production-release-asset/1",
            "file:///etc/passwd",
            "not a url",
        ] {
            assert!(check_asset_url(bad).is_err(), "{bad}");
        }
        // the CDN hosts are for redirects only, and only over https on their own name
        for cdn in ["https://objects.githubusercontent.com/github-production-release-asset/1?X-Amz=1", "https://release-assets.githubusercontent.com/a/b"] {
            assert!(check_redirect_url(cdn).is_ok(), "{cdn}");
        }
        for bad in [
            "http://objects.githubusercontent.com/x",
            "https://objects.githubusercontent.com.evil.com/x",
            "https://githubusercontent.com/x",
            "https://raw.githubusercontent.com/deepfounder-ai/desigma/main/README.md",
            "https://example.com/x",
            "http://127.0.0.2:80/x",
        ] {
            let e = check_redirect_url(bad).unwrap_err();
            assert!(e.starts_with("refusing to download from"), "{bad}: {e}");
        }
    }

    #[test]
    fn query_strings_stay_out_of_the_error() {
        assert_eq!(check_redirect_url("https://example.com/x?token=secret").unwrap_err(), "refusing to download from https://example.com/x");
    }
}
