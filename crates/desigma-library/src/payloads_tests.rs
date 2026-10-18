use crate::{content_digest, payload_digest, Library, LibraryError};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

struct TempRoot(PathBuf);

impl TempRoot {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        loop {
            let path = std::env::temp_dir().join(format!(
                "desigma-payloads-tests-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed),
            ));
            // Never reuse or delete a directory left by another process/run.
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("create temporary payload fixture: {error}"),
            }
        }
    }

    fn library(&self) -> Library {
        // Traversal fixtures remain inside the directory this test created.
        Library::open(self.0.join("sandbox/library")).unwrap()
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        // This is the exact fresh directory claimed by create_dir above.
        let _ = fs::remove_dir_all(&self.0);
    }
}

type Tree = Vec<(PathBuf, Option<Vec<u8>>, SystemTime)>;

fn snapshot(root: &Path) -> Tree {
    fn visit(root: &Path, directory: &Path, tree: &mut Tree) {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            let metadata = fs::symlink_metadata(&path).unwrap();
            assert!(!metadata.file_type().is_symlink(), "snapshot must not follow links");
            let bytes = if metadata.is_dir() { None } else { Some(fs::read(&path).unwrap()) };
            tree.push((path.strip_prefix(root).unwrap().to_path_buf(), bytes, metadata.modified().unwrap()));
            if metadata.is_dir() {
                visit(root, &path, tree);
            }
        }
    }
    let mut tree = Vec::new();
    visit(root, root, &mut tree);
    tree.sort_by(|left, right| left.0.cmp(&right.0));
    tree
}

fn backdate(path: &Path) -> SystemTime {
    let file = fs::OpenOptions::new().write(true).open(path).unwrap();
    file.set_times(fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(946_684_800))).unwrap();
    drop(file);
    fs::metadata(path).unwrap().modified().unwrap()
}

#[test]
fn namespaces_isolate_identical_content_and_survive_reopen() {
    let temp = TempRoot::new();
    let library = temp.library();
    let before = snapshot(&temp.0);
    let first = library.payloads("extension-a_1").unwrap();
    let second = library.payloads("extension-b_2").unwrap();
    let longest = "x".repeat(80);
    let boundary = library.payloads(&longest).unwrap();
    assert_eq!(snapshot(&temp.0), before, "opening stores must be lazy");

    let bytes = b"identical bytes retained by independent extensions\0\xff";
    let hash = first.retain(bytes).unwrap();
    let after_first = snapshot(&temp.0);
    assert!(second.verify(&hash).is_err());
    assert!(second.load(&hash).is_err());
    assert!(boundary.load(&hash).is_err());
    assert_eq!(snapshot(&temp.0), after_first);
    assert_eq!(second.retain(bytes).unwrap(), hash);
    boundary.put(&hash, bytes).unwrap();

    let first_path = library.path("extension-a_1/payloads").join(&hash);
    fs::remove_file(&first_path).unwrap();
    assert!(first.verify(&hash).is_err());
    assert!(first.load(&hash).is_err(), "another namespace must not satisfy a missing object");
    assert_eq!(second.load(&hash).unwrap(), bytes);
    assert_eq!(boundary.load(&hash).unwrap(), bytes);
    first.put(&hash, bytes).unwrap();
    drop(library);

    let reopened = temp.library();
    for namespace in ["extension-a_1", "extension-b_2", longest.as_str()] {
        let store = reopened.payloads(namespace).unwrap();
        store.verify(&hash).unwrap();
        assert_eq!(store.load(&hash).unwrap(), bytes);
        assert_eq!(fs::read(reopened.path(namespace).join("payloads").join(&hash)).unwrap(), bytes);
    }
    assert!(!reopened.path("cityos").exists());
}

#[cfg(windows)]
#[test]
fn windows_extended_paths_support_atomic_payload_creation_and_repair() {
    use std::os::windows::ffi::OsStrExt;
    let temp = TempRoot::new();
    let mut root = temp.0.clone();
    for _ in 0..9 {
        root.push("nested-payload-directory");
    }
    let library = Library::open(&root).unwrap();
    let store = library.payloads("cityos").unwrap();
    let bytes = b"long paths must retain the same exact-byte payload identity";
    let digest = content_digest(bytes);
    let target = library.path("cityos/payloads").join(&digest);
    assert!(target.as_os_str().encode_wide().count() > 320);
    store.put(&digest, bytes).unwrap();
    assert_eq!(store.load(&digest).unwrap(), bytes);
    fs::write(&target, b"damaged payload").unwrap();
    assert!(store.verify(&digest).is_err());
    store.put(&digest, bytes).unwrap();
    let reopened = Library::open(&root).unwrap();
    assert_eq!(reopened.payloads("cityos").unwrap().load(&digest).unwrap(), bytes);
}

#[test]
fn invalid_namespaces_leave_filesystem_unchanged() {
    for populated in [false, true] {
        let temp = TempRoot::new();
        let library = temp.library();
        if populated {
            library.retain_cityos_payload(b"existing object").unwrap();
        }
        let mut invalid: Vec<String> = [
            "", ".", "..", "../escape", "..\\escape", "nested/name", "nested\\name",
            "CityOS", "a b", "a.b", "a:b", "a\nb", "a\0b", "é",
            "projects", "trash", "versions", "libraries", "settings",
            "con", "prn", "aux", "nul",
        ].into_iter().map(String::from).collect();
        invalid.push("x".repeat(81));
        invalid.push(temp.0.join("absolute-escape").to_str().unwrap().to_owned());
        for number in 1..=9 {
            invalid.push(format!("com{number}"));
            invalid.push(format!("lpt{number}"));
        }
        let before = snapshot(&temp.0);
        for namespace in invalid {
            assert!(matches!(library.payloads(&namespace), Err(LibraryError::Invalid(_))), "{namespace:?}");
            assert_eq!(snapshot(&temp.0), before, "namespace {namespace:?}");
        }
    }
}

#[test]
fn invalid_digests_leave_filesystem_unchanged() {
    for populated in [false, true] {
        let temp = TempRoot::new();
        let library = temp.library();
        let store = library.payloads("extension").unwrap();
        if populated {
            store.retain(b"existing generic object").unwrap();
            library.retain_cityos_payload(b"existing CityOS object").unwrap();
        }
        let mut invalid: Vec<String> = [
            "", ".", "..", "../escape", "..\\escape", "nested/name", "nested\\name", "a\0b",
        ].into_iter().map(String::from).collect();
        invalid.extend([
            "a".repeat(63), "a".repeat(65), "g".repeat(64),
            format!("{} ", "a".repeat(63)), format!("{}\n", "a".repeat(63)),
            format!("{}/", "a".repeat(63)), format!("{}\\", "a".repeat(63)),
            temp.0.join("absolute-digest").to_str().unwrap().to_owned(),
        ]);
        let before = snapshot(&temp.0);
        for hash in invalid {
            assert!(matches!(store.put(&hash, b"bytes"), Err(LibraryError::Invalid(_))), "{hash:?}");
            assert_eq!(snapshot(&temp.0), before);
            assert!(matches!(store.verify(&hash), Err(LibraryError::Invalid(_))), "{hash:?}");
            assert_eq!(snapshot(&temp.0), before);
            assert!(matches!(store.load(&hash), Err(LibraryError::Invalid(_))), "{hash:?}");
            assert_eq!(snapshot(&temp.0), before);
            assert!(matches!(library.put_cityos_payload(&hash, b"bytes"), Err(LibraryError::Invalid(_))));
            assert_eq!(snapshot(&temp.0), before);
            assert!(matches!(library.verify_cityos_payload(&hash), Err(LibraryError::Invalid(_))));
            assert_eq!(snapshot(&temp.0), before);
            assert!(matches!(library.load_cityos_payload(&hash), Err(LibraryError::Invalid(_))));
            assert_eq!(snapshot(&temp.0), before);
        }
    }
}

#[test]
fn content_digest_matches_known_sha256_vectors_and_legacy_digest() {
    let vectors: [(&[u8], &str); 3] = [
        (b"", "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"),
        (b"abc", "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"),
        (b"abc\n", "edeaaff3f1774ad2888673770c6d64097e391bc362d7d6fb34982ddf0efd18cb"),
    ];
    for (bytes, expected) in vectors {
        assert_eq!(content_digest(bytes), expected);
        assert_eq!(payload_digest(bytes), expected);
    }
}

#[test]
fn cityos_legacy_path_and_apis_interoperate_with_generic_store() {
    let temp = TempRoot::new();
    let library = temp.library();
    let directory = library.path("cityos/payloads");
    fs::create_dir_all(&directory).unwrap();
    let legacy_hash = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    fs::write(directory.join(legacy_hash), b"abc").unwrap();
    let modified = backdate(&directory.join(legacy_hash));
    let store = library.payloads("cityos").unwrap();
    store.verify(legacy_hash).unwrap();
    library.verify_cityos_payload(legacy_hash).unwrap();
    assert_eq!(store.load(legacy_hash).unwrap(), b"abc");
    assert_eq!(library.load_cityos_payload(legacy_hash).unwrap(), b"abc");
    store.put(legacy_hash, b"abc").unwrap();
    library.put_cityos_payload(legacy_hash, b"abc").unwrap();
    assert_eq!(fs::metadata(directory.join(legacy_hash)).unwrap().modified().unwrap(), modified);

    let legacy_bytes = b"legacy retain\0\xff\r\n";
    let legacy_pin = library.retain_cityos_payload(legacy_bytes).unwrap();
    assert_eq!(legacy_pin, content_digest(legacy_bytes));
    assert_eq!(legacy_pin, payload_digest(legacy_bytes));
    assert_eq!(store.load(&legacy_pin).unwrap(), legacy_bytes);
    store.verify(&legacy_pin).unwrap();

    let generic_bytes = b"{ \"value\": 1 }\r\n";
    let generic_pin = store.retain(generic_bytes).unwrap();
    assert_eq!(generic_pin, payload_digest(generic_bytes));
    assert_ne!(generic_pin, content_digest(b"{\"value\":1}"));
    assert_eq!(library.load_cityos_payload(&generic_pin).unwrap(), generic_bytes);
    library.verify_cityos_payload(&generic_pin).unwrap();

    let legacy_put = b"legacy explicit put";
    let legacy_put_pin = content_digest(legacy_put);
    library.put_cityos_payload(&legacy_put_pin, legacy_put).unwrap();
    assert_eq!(store.load(&legacy_put_pin).unwrap(), legacy_put);
    let generic_put = b"generic explicit put";
    let generic_put_pin = payload_digest(generic_put);
    store.put(&generic_put_pin, generic_put).unwrap();
    assert_eq!(library.load_cityos_payload(&generic_put_pin).unwrap(), generic_put);

    for (hash, bytes) in [
        (legacy_hash, b"abc".as_slice()),
        (legacy_pin.as_str(), legacy_bytes.as_slice()),
        (generic_pin.as_str(), generic_bytes.as_slice()),
        (legacy_put_pin.as_str(), legacy_put.as_slice()),
        (generic_put_pin.as_str(), generic_put.as_slice()),
    ] {
        assert_eq!(fs::read(directory.join(hash)).unwrap(), bytes);
    }
    assert_eq!(fs::read_dir(&directory).unwrap().count(), 5);
}

#[test]
fn valid_put_and_retain_preserve_object_modified_time() {
    let temp = TempRoot::new();
    let library = temp.library();
    let bytes: Vec<u8> = (0..(64 * 1024 + 17)).map(|index| (index % 251) as u8).collect();
    for namespace in ["extension", "cityos"] {
        let store = library.payloads(namespace).unwrap();
        let hash = store.retain(&bytes).unwrap();
        let path = library.path(namespace).join("payloads").join(&hash);
        // Backdating avoids sleeps and catches rewrites on coarse timestamp filesystems.
        let modified = backdate(&path);
        let before = snapshot(&temp.0);
        for _ in 0..3 {
            store.put(&hash, &bytes).unwrap();
            assert_eq!(store.retain(&bytes).unwrap(), hash);
            store.verify(&hash).unwrap();
            assert_eq!(store.load(&hash).unwrap(), bytes);
            if namespace == "cityos" {
                library.put_cityos_payload(&hash, &bytes).unwrap();
                assert_eq!(library.retain_cityos_payload(&bytes).unwrap(), hash);
            }
            assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);
            assert_eq!(snapshot(&temp.0), before);
        }
    }
}

#[test]
fn corruption_requires_exact_expected_bytes_and_mismatches_preserve_old_bytes() {
    let canonical: Vec<u8> = (0..(64 * 1024 + 17)).map(|index| (index % 251) as u8).collect();
    let hash = content_digest(&canonical);
    let mut changed_tail = canonical.clone();
    *changed_tail.last_mut().unwrap() ^= 1;
    for namespace in ["extension", "cityos"] {
        let temp = TempRoot::new();
        let library = temp.library();
        let store = library.payloads(namespace).unwrap();
        let before = snapshot(&temp.0);
        assert!(store.put(&hash, b"wrong initial bytes").is_err());
        assert_eq!(snapshot(&temp.0), before, "a mismatch must not create directories");
        store.put(&hash, &canonical).unwrap();
        let path = library.path(namespace).join("payloads").join(&hash);

        for corrupt in [b"".as_slice(), b"truncated".as_slice(), changed_tail.as_slice()] {
            fs::write(&path, corrupt).unwrap();
            backdate(&path);
            let damaged = snapshot(&temp.0);
            assert!(matches!(store.verify(&hash), Err(LibraryError::Invalid(_))));
            assert!(matches!(store.load(&hash), Err(LibraryError::Invalid(_))));
            assert_eq!(snapshot(&temp.0), damaged, "reads must not repair corruption");
            assert!(store.put(&hash, b"wrong repair").is_err());
            assert_eq!(fs::read(&path).unwrap(), corrupt);
            assert_eq!(snapshot(&temp.0), damaged);
            if namespace == "cityos" {
                assert!(library.verify_cityos_payload(&hash).is_err());
                assert!(library.load_cityos_payload(&hash).is_err());
                assert!(library.put_cityos_payload(&hash, b"wrong repair").is_err());
                assert_eq!(snapshot(&temp.0), damaged);
                library.put_cityos_payload(&hash, &canonical).unwrap();
            } else {
                store.put(&hash, &canonical).unwrap();
            }
            store.verify(&hash).unwrap();
            assert_eq!(store.load(&hash).unwrap(), canonical);
            assert_eq!(fs::read(&path).unwrap(), canonical);
            let repaired = snapshot(&temp.0);
            assert!(store.put(&hash, b"replacement must be rejected").is_err());
            assert_eq!(snapshot(&temp.0), repaired);
        }
    }
}

#[cfg(unix)]
#[test]
fn symlink_descendants_reject_reads_and_writes_without_touching_targets() {
    use std::os::unix::fs::symlink;

    for level in ["namespace", "payloads", "object"] {
        let temp = TempRoot::new();
        let library = temp.library();
        let bytes = b"valid bytes outside the selected library";
        let hash = content_digest(bytes);
        let outside = temp.0.join("outside");
        fs::create_dir_all(outside.join("payloads")).unwrap();
        let target = outside.join(&hash);
        let nested_target = outside.join("payloads").join(&hash);
        fs::write(&target, bytes).unwrap();
        fs::write(&nested_target, bytes).unwrap();
        backdate(&target);
        backdate(&nested_target);
        let before = snapshot(&outside);
        let scope = library.path("extension");
        let directory = scope.join("payloads");
        let link = match level {
            "namespace" => {
                symlink(&outside, &scope).unwrap();
                scope
            }
            "payloads" => {
                fs::create_dir(&scope).unwrap();
                symlink(&outside, &directory).unwrap();
                directory
            }
            _ => {
                fs::create_dir_all(&directory).unwrap();
                let object = directory.join(&hash);
                symlink(&target, &object).unwrap();
                object
            }
        };
        let destination = fs::read_link(&link).unwrap();
        let store = library.payloads("extension").unwrap();
        assert!(matches!(store.verify(&hash), Err(LibraryError::Invalid(_))), "{level}");
        assert!(matches!(store.load(&hash), Err(LibraryError::Invalid(_))), "{level}");
        assert!(matches!(store.put(&hash, bytes), Err(LibraryError::Invalid(_))), "{level}");
        assert!(matches!(store.retain(bytes), Err(LibraryError::Invalid(_))), "{level}");
        // A missing object must not be materialized through a linked directory either.
        if level != "object" {
            assert!(matches!(store.retain(b"new redirected object"), Err(LibraryError::Invalid(_))), "{level}");
        }
        assert_eq!(fs::read_link(&link).unwrap(), destination);
        assert_eq!(snapshot(&outside), before, "{level}");
        assert_eq!(fs::read(&target).unwrap(), bytes);
        assert_eq!(fs::read(&nested_target).unwrap(), bytes);
    }
}
