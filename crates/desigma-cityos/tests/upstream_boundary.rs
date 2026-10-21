//! The native CI lane verifies ancestry and declared core changes without a JS runtime.
use std::{collections::HashSet, path::PathBuf, process::Command};

#[test]
fn selected_upstream_and_all_native_core_changes_are_declared() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("config/upstream/integration.json")).unwrap()).unwrap();
    let upstream = manifest["upstream"]["commit"].as_str().unwrap();
    assert_eq!(upstream.len(), 40);
    assert!(upstream.bytes().all(|byte| byte.is_ascii_hexdigit()));
    let run = |args: &[&str]| {
        let result = Command::new("git").current_dir(&root).args(args).output().expect("Git is required for source qualification");
        assert!(result.status.success(), "upstream Git check failed; fetch the frozen source with full ancestry");
        result.stdout
    };
    run(&["merge-base", "--is-ancestor", upstream, "HEAD"]);
    let mut declared = HashSet::new();
    for change in manifest["changes"].as_array().unwrap() {
        for field in ["id", "purpose", "invariant"] { assert!(!change[field].as_str().unwrap().is_empty()); }
        assert!(!change["tests"].as_array().unwrap().is_empty());
        for test in change["tests"].as_array().unwrap() {
            assert!(root.join(test.as_str().unwrap()).is_file(), "declared regression source is missing");
        }
        for path in change["paths"].as_array().unwrap() {
            assert!(declared.insert(path.as_str().unwrap()), "ambiguous core change owner");
        }
    }
    let owned: Vec<_> = manifest["ownedDirectories"].as_array().unwrap().iter().map(|value| value.as_str().unwrap()).collect();
    let diff = run(&["diff", "--name-only", "-z", &format!("{upstream}...HEAD")]);
    for path in std::str::from_utf8(&diff).unwrap().split('\0').filter(|path| !path.is_empty()) {
        assert!(declared.contains(path) || owned.iter().any(|owner| path == *owner || path.starts_with(&format!("{owner}/"))), "undeclared upstream change: {path}");
    }
}
