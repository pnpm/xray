use super::{verified_patches, BridgeFile, BridgeResult};
use std::collections::BTreeMap;

fn item(root: &std::path::Path, name: &str, original: &str, replacement: &str) -> BridgeFile {
    let file = root.join(name);
    std::fs::write(&file, original).unwrap();
    BridgeFile {
        root: root.to_path_buf(),
        file,
        original: original.into(),
        replacement: replacement.into(),
    }
}
fn result(files: Vec<BridgeFile>) -> BridgeResult {
    BridgeResult { compiler: "5.9.3".into(), files, messages: Vec::new() }
}

#[test]
fn validates_declaration_paths_changes_and_uniqueness() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let roots = BTreeMap::from([(root.clone(), "fixture@1.0.0".into())]);
    let valid = item(&root, "index.d.mts", "invalid", "valid");
    let patches = verified_patches(result(vec![valid]), &roots).unwrap();
    assert_eq!(patches[0].files[0].path, std::path::Path::new("index.d.mts"));
    assert!(verified_patches(result(vec![item(&root, "index.js", "old", "new")]), &roots).is_err());
    assert!(
        verified_patches(result(vec![item(&root, "same.d.ts", "same", "same")]), &roots).is_err(),
    );
    let first = item(&root, "dup.d.ts", "old", "new");
    let second = item(&root, "dup.d.ts", "old", "new");
    assert!(verified_patches(result(vec![first, second]), &roots).is_err());
}

#[test]
fn refuses_nested_file_attributed_to_outer_package() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let child = root.join("node_modules/child");
    std::fs::create_dir_all(&child).unwrap();
    let child = child.canonicalize().unwrap();
    let roots = BTreeMap::from([(root.clone(), "outer@1".into()), (child, "inner@1".into())]);
    let file = root.join("node_modules/child/index.d.ts");
    std::fs::write(&file, "old").unwrap();
    let wrong = BridgeFile { root, file, original: "old".into(), replacement: "new".into() };
    assert!(verified_patches(result(vec![wrong]), &roots).is_err());
}

#[cfg(windows)]
#[test]
fn accepts_node_nonverbatim_spelling_for_a_canonical_windows_root() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let roots = BTreeMap::from([(root.clone(), "fixture@1".into())]);
    let valid = item(&root, "index.d.ts", "old", "new");
    let strip = |path: &std::path::Path| {
        std::path::PathBuf::from(
            path.to_str().unwrap().strip_prefix(r"\\?\").unwrap_or(path.to_str().unwrap()),
        )
    };
    let node_file = BridgeFile { root: strip(&valid.root), file: strip(&valid.file), ..valid };
    assert!(verified_patches(result(vec![node_file]), &roots).is_ok());
}

#[cfg(unix)]
#[test]
fn rejects_a_declaration_file_reached_through_a_symlink() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let roots = BTreeMap::from([(root.clone(), "fixture@1".into())]);
    std::fs::write(root.join("actual.d.ts"), "old").unwrap();
    symlink("actual.d.ts", root.join("index.d.ts")).unwrap();
    let file = BridgeFile {
        root: root.clone(),
        file: root.join("index.d.ts"),
        original: "old".into(),
        replacement: "new".into(),
    };
    assert!(verified_patches(result(vec![file]), &roots).is_err());
}
