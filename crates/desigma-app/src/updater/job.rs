//! The background half of "Update now": checksums first, then the download, its verification and the unpacking,
//! reporting every step to the UI through a channel.

use super::assets::{self, Asset};
use super::download::{self, Fail, MAX_DOWNLOAD, MAX_SUMS};
use super::install::{self, Prepared, Target};
use super::{Install, Release};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;

/// Runs the whole update preparation. The last message sent is `Ready`, `Failed`, or `Idle` after Cancel.
pub fn run(release: &Release, target: &Target, cancel: &AtomicBool, send: &dyn Fn(Install)) {
    let work = install::work_dir();
    let outcome = prepare(release, target, &work, cancel, send);
    send(match outcome {
        Ok(prepared) => Install::Ready(prepared),
        Err(Fail::Cancelled) => Install::Idle,
        Err(Fail::Failed(e)) => Install::Failed(e),
    });
}

/// [`run`] on its own thread; `wake` asks the UI to repaint after each message.
pub fn spawn(release: Release, target: Target, cancel: std::sync::Arc<AtomicBool>, sender: Sender<Install>, wake: impl Fn() + Send + 'static) {
    std::thread::spawn(move || {
        run(&release, &target, &cancel, &|state| {
            let _ = sender.send(state);
            wake();
        });
    });
}

fn prepare(release: &Release, target: &Target, work: &Path, cancel: &AtomicBool, send: &dyn Fn(Install)) -> Result<Prepared, Fail> {
    let asset = assets::pick(&release.assets, target.kind, &release.version)?;
    let sums = assets::sums(&release.assets)?;
    // the checksum comes first: a release without one for this file is refused before 40 MB are fetched
    let expected = assets::expected_digest(&download::to_string(&sums.url, MAX_SUMS, cancel)?, &asset.name)?;
    if asset.size > MAX_DOWNLOAD {
        return Err(Fail::Failed(format!("the download is larger than the {} MB limit", MAX_DOWNLOAD >> 20)));
    }
    let _ = std::fs::remove_dir_all(work);
    std::fs::create_dir_all(work).map_err(|e| format!("cannot create {} ({e})", work.display()))?;
    let file = work.join(safe_name(asset)?);
    send(Install::Downloading { done: 0, total: Some(asset.size).filter(|s| *s > 0) });
    let digest = download::to_file(&asset.url, &file, MAX_DOWNLOAD, cancel, &mut |done, total| {
        send(Install::Downloading { done, total: total.or(Some(asset.size).filter(|s| *s > 0)) });
    })?;
    send(Install::Verifying);
    if digest != expected {
        let _ = std::fs::remove_file(&file);
        return Err(Fail::Failed(format!("the download does not match its checksum in SHA256SUMS (expected {}…, got {}…)", &expected[..12], &digest[..12])));
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(Fail::Cancelled);
    }
    send(Install::Preparing);
    Ok(install::prepare(target, &file, work)?)
}

/// The asset's name as a file name in the work folder: no folders, nothing odd.
fn safe_name(asset: &Asset) -> Result<&str, String> {
    let ok = !asset.name.is_empty() && asset.name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_')) && !asset.name.starts_with('.');
    if ok {
        Ok(&asset.name)
    } else {
        Err("the release lists a file with an unusable name".into())
    }
}

#[cfg(test)]
mod tests {
    use super::super::assets::Kind;
    use super::*;
    use semver::Version;
    use sha2::{Digest, Sha256};
    use std::io::{BufRead, Write};
    use std::sync::Mutex;

    fn hex(bytes: &[u8]) -> String {
        Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Serves `(path, body)` pairs over http; requests for anything else get 404.
    fn serve(files: Vec<(String, Vec<u8>)>, connections: usize) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for _ in 0..connections {
                let Ok((mut stream, _)) = listener.accept() else { return };
                let head: Vec<String> = std::io::BufReader::new(&stream).lines().map_while(Result::ok).take_while(|l| !l.is_empty()).collect();
                let path = head[0].split(' ').nth(1).unwrap_or("/").to_string();
                let response = match files.iter().find(|(p, _)| *p == path) {
                    Some((_, body)) => {
                        let mut r = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
                        r.extend_from_slice(body);
                        r
                    }
                    None => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
                };
                let _ = stream.write_all(&response);
            }
        });
        base
    }

    const V: &str = "99.0.0-alpha.1";

    /// The files a server offers for a release of `kind` carrying `data`, with `SHA256SUMS` listing `entry`.
    fn files(kind: Kind, data: &[u8], entry: Option<(&str, String)>) -> Vec<(String, Vec<u8>)> {
        let mut files = vec![(format!("/{}", assets::asset_name(kind, &Version::parse(V).unwrap())), data.to_vec())];
        if let Some((name, digest)) = entry {
            files.push((
                "/SHA256SUMS".into(),
                format!(
                    "{digest}  {name}
"
                )
                .into_bytes(),
            ));
        }
        files
    }

    /// The release whose assets are the served `files`.
    fn release(base: &str, files: &[(String, Vec<u8>)]) -> Release {
        let assets = files.iter().map(|(path, body)| Asset { name: path[1..].into(), url: format!("{base}{path}"), size: body.len() as u64 }).collect();
        Release { version: Version::parse(V).unwrap(), page: String::new(), notes: vec![], assets }
    }

    fn run_collect(release: &Release, target: &Target) -> Vec<Install> {
        let seen = Mutex::new(Vec::new());
        run(release, target, &AtomicBool::new(false), &|s| seen.lock().unwrap().push(s));
        seen.into_inner().unwrap()
    }

    /// Serialises the tests below: they share the work folder in the temp dir.
    static WORK: Mutex<()> = Mutex::new(());

    fn name_of(kind: Kind) -> String {
        assets::asset_name(kind, &Version::parse(V).unwrap())
    }

    fn windows_target() -> Target {
        Target { kind: Kind::WindowsInstaller, exe: std::env::temp_dir().join("desigma-test-never-touched").join("desigma.exe") }
    }

    #[test]
    fn a_verified_installer_download_ends_ready() {
        let _guard = WORK.lock().unwrap_or_else(|e| e.into_inner());
        let setup = b"MZ installer bytes".to_vec();
        let served = files(Kind::WindowsInstaller, &setup, Some((&name_of(Kind::WindowsInstaller), hex(&setup))));
        let release = release(&serve(served.clone(), 2), &served);
        let states = run_collect(&release, &windows_target());
        assert!(matches!(states.first(), Some(Install::Downloading { done: 0, .. })), "{states:?}");
        assert!(states.contains(&Install::Verifying) && states.contains(&Install::Preparing), "{states:?}");
        let Some(Install::Ready(Prepared::Setup { setup: path })) = states.last() else { panic!("{states:?}") };
        assert_eq!(std::fs::read(path).unwrap(), setup);
        let _ = std::fs::remove_dir_all(install::work_dir());
    }

    #[test]
    fn a_wrong_checksum_is_refused_and_the_file_deleted() {
        let _guard = WORK.lock().unwrap_or_else(|e| e.into_inner());
        let name = name_of(Kind::WindowsInstaller);
        let served = files(Kind::WindowsInstaller, b"MZ real bytes", Some((&name, hex(b"something else"))));
        let release = release(&serve(served.clone(), 2), &served);
        let states = run_collect(&release, &windows_target());
        let Some(Install::Failed(e)) = states.last() else { panic!("{states:?}") };
        assert!(e.contains("does not match its checksum"), "{e}");
        assert!(!install::work_dir().join(&name).exists());
        let _ = std::fs::remove_dir_all(install::work_dir());
    }

    #[test]
    fn a_missing_checksum_entry_is_refused_before_downloading() {
        let _guard = WORK.lock().unwrap_or_else(|e| e.into_inner());
        // SHA256SUMS lists another file only; the server answers one connection, so a second request would hang the test
        let served = files(Kind::MacApp, b"data", Some(("some-other-file", hex(b"x"))));
        let release = release(&serve(served.clone(), 1), &served);
        let target = Target { kind: Kind::MacApp, exe: "/Applications/desigma.app/Contents/MacOS/desigma".into() };
        let states = run_collect(&release, &target);
        assert_eq!(states, [Install::Failed(format!("SHA256SUMS has no entry for {}", name_of(Kind::MacApp)))]);
        // no SHA256SUMS asset at all
        let mut bare = release.clone();
        bare.assets.retain(|a| a.name != "SHA256SUMS");
        assert_eq!(run_collect(&bare, &target), [Install::Failed("this release has no SHA256SUMS file".into())]);
        // no asset for this install type
        let mut empty = release;
        empty.assets.retain(|a| a.name == "SHA256SUMS");
        assert!(matches!(run_collect(&empty, &target).as_slice(), [Install::Failed(e)] if e.contains("macos-universal.tar.gz")));
    }

    #[test]
    fn cancel_ends_idle() {
        let _guard = WORK.lock().unwrap_or_else(|e| e.into_inner());
        let served = files(Kind::WindowsInstaller, b"MZ", Some((&name_of(Kind::WindowsInstaller), hex(b"MZ"))));
        let release = release(&serve(served.clone(), 1), &served);
        let cancel = AtomicBool::new(false);
        let seen = Mutex::new(Vec::new());
        run(&release, &windows_target(), &cancel, &|s| {
            if matches!(s, Install::Downloading { .. }) {
                cancel.store(true, Ordering::Relaxed);
            }
            seen.lock().unwrap().push(s);
        });
        let seen = seen.into_inner().unwrap();
        assert_eq!(seen.last(), Some(&Install::Idle), "{seen:?}");
        let _ = std::fs::remove_dir_all(install::work_dir());
    }

    #[test]
    fn odd_asset_names_are_not_written_to_disk() {
        let asset = |n: &str| Asset { name: n.into(), url: String::new(), size: 0 };
        assert!(safe_name(&asset("desigma-1.0.0-alpha.1-x86_64-windows-setup.exe")).is_ok());
        for bad in ["", "../x", "a/b", "a\\b", ".hidden", "a b", "x:y"] {
            assert!(safe_name(&asset(bad)).is_err(), "{bad:?}");
        }
    }
}
