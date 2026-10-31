use super::{matches_configuration, Layout};
use std::path::Path;

fn metadata(root: &Path, content: &str) {
    let modules = root.join("node_modules");
    std::fs::create_dir_all(modules.join(".pnpm")).unwrap();
    std::fs::write(modules.join(".modules.yaml"), content).unwrap();
}

#[test]
fn detects_resolution_changes_but_not_build_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let store = root.join("store");
    std::fs::create_dir_all(&store).unwrap();
    let first = format!("nodeLinker: isolated\nstoreDir: '{}'\nvirtualStoreDir: .pnpm\npublicHoistPattern: []\npendingBuilds: [first]\n", store.display());
    metadata(root, &first);
    let layout = Layout::capture(root).unwrap();
    let saved = serde_json::to_vec(&layout).unwrap();
    let restored: Layout = serde_json::from_slice(&saved).unwrap();
    restored.validate_recovery(root).unwrap();
    let outside = tempfile::tempdir().unwrap();
    assert!(restored.validate_recovery(outside.path()).is_err());
    metadata(root, &first.replace("first", "second"));
    layout.verify_unchanged().unwrap();
    metadata(root, &first.replace("publicHoistPattern: []", "publicHoistPattern: ['*']"));
    assert!(layout.verify_unchanged().is_err());
    metadata(root, &first);
    std::fs::create_dir_all(store.join("links")).unwrap();
    metadata(
        root,
        &first.replace(
            "virtualStoreDir: .pnpm",
            &format!("virtualStoreDir: '{}'", store.join("links").display()),
        ),
    );
    assert!(layout.verify_unchanged().is_err());
}

#[test]
fn refuses_missing_or_invalid_metadata() {
    let temp = tempfile::tempdir().unwrap();
    assert!(Layout::capture(temp.path()).is_err());
    metadata(temp.path(), "nodeLinker: isolated\n");
    assert!(Layout::capture(temp.path()).is_err());
}

#[test]
fn preflight_rejects_cli_only_global_and_hoist_settings() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let store = root.join("store");
    std::fs::create_dir_all(store.join("links")).unwrap();
    let content = format!(
        "nodeLinker: isolated\nstoreDir: '{}'\nvirtualStoreDir: '{}'\npublicHoistPattern: []\n",
        store.display(),
        store.join("links").display(),
    );
    metadata(root, &content);
    let layout = Layout::capture(root).unwrap();
    let signature = layout.modules.get(root).unwrap().as_ref().unwrap();
    let effective = serde_json::json!({"enableGlobalVirtualStore":true,"hoist":false});
    assert!(matches_configuration(signature, &effective, &store, root).unwrap());
    assert!(!matches_configuration(
        signature,
        &serde_json::json!({"enableGlobalVirtualStore":true}),
        &store,
        root
    )
    .unwrap());
    // Local default may not exist; either an error or a mismatch rejects it.
    assert!(!matches_configuration(signature, &serde_json::json!({"hoist":false}), &store, root)
        .unwrap_or(false));
}
