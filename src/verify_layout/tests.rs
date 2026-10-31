use super::{effective_included, install_args_for, matches_configuration, Layout};
use std::path::Path;

#[test]
fn reinstalls_reproduce_the_original_dependency_groups() {
    let temp = tempfile::tempdir().unwrap();
    let canonical = temp.path().canonicalize().unwrap();
    let root = canonical.as_path();
    let store = root.join("store");
    std::fs::create_dir_all(&store).unwrap();
    let base = format!(
        "nodeLinker: isolated\nstoreDir: '{}'\nvirtualStoreDir: .pnpm\npublicHoistPattern: []\n",
        store.display(),
    );
    metadata(root, &format!("{base}included:\n  dependencies: true\n  devDependencies: false\n  optionalDependencies: true\n"));
    let layout = Layout::capture(root).unwrap();
    assert_eq!(layout.install_args().unwrap(), ["--prod"]);
    let restored: Layout = serde_json::from_slice(&serde_json::to_vec(&layout).unwrap()).unwrap();
    assert_eq!(restored.install_args().unwrap(), ["--prod"]);
    // A full install over a production layout is a layout change.
    metadata(root, &format!("{base}included:\n  dependencies: true\n  devDependencies: true\n  optionalDependencies: true\n"));
    assert!(layout.verify_unchanged().is_err());
    metadata(root, &format!("{base}included:\n  dependencies: false\n  devDependencies: false\n  optionalDependencies: true\n"));
    assert!(Layout::capture(root).is_err());

    assert!(install_args_for(None).is_empty());
    assert!(install_args_for(Some((true, true, true))).is_empty());
    assert_eq!(install_args_for(Some((false, true, false))), ["--dev", "--no-optional"]);
    for included in [(true, false, true), (false, true, true), (true, true, false)] {
        let args = install_args_for(Some(included));
        assert_eq!(effective_included(&serde_json::json!({}), &args, false), included);
    }
    // Explicit flags override configuration; without them it applies.
    let production = serde_json::json!({"production": true});
    assert_eq!(effective_included(&production, &[], false), (true, false, true));
    assert_eq!(effective_included(&production, &["--dev"], false), (false, true, true));
    assert_eq!(effective_included(&serde_json::json!({}), &[], true), (true, false, true));
    assert_eq!(
        effective_included(&serde_json::json!({"optional": false}), &[], false),
        (true, true, false),
    );
}

#[test]
fn rejects_importers_installed_with_different_dependency_groups() {
    let temp = tempfile::tempdir().unwrap();
    let canonical = temp.path().canonicalize().unwrap();
    let root = canonical.as_path();
    let store = root.join("store");
    std::fs::create_dir_all(&store).unwrap();
    std::fs::write(root.join("pnpm-lock.yaml"), "importers:\n  .: {}\n  app: {}\n").unwrap();
    std::fs::create_dir_all(root.join("app")).unwrap();
    let base = format!(
        "nodeLinker: isolated\nstoreDir: '{}'\nvirtualStoreDir: .pnpm\npublicHoistPattern: []\n",
        store.display(),
    );
    let groups = |dev: bool| {
        format!("{base}included:\n  dependencies: true\n  devDependencies: {dev}\n  optionalDependencies: true\n")
    };
    metadata(root, &groups(true));
    metadata(&root.join("app"), &groups(false));
    assert!(Layout::capture(root).is_err());
    metadata(&root.join("app"), &groups(true));
    assert_eq!(Layout::capture(root).unwrap().install_args().unwrap(), Vec::<&str>::new());
}

fn metadata(root: &Path, content: &str) {
    let modules = root.join("node_modules");
    std::fs::create_dir_all(modules.join(".pnpm")).unwrap();
    std::fs::write(modules.join(".modules.yaml"), content).unwrap();
}

#[test]
fn detects_resolution_changes_but_not_build_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let canonical = temp.path().canonicalize().unwrap();
    let root = canonical.as_path();
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
    let canonical = temp.path().canonicalize().unwrap();
    assert!(Layout::capture(&canonical).is_err());
    metadata(&canonical, "nodeLinker: isolated\n");
    assert!(Layout::capture(&canonical).is_err());
}

#[test]
fn preflight_rejects_cli_only_global_and_hoist_settings() {
    let temp = tempfile::tempdir().unwrap();
    let canonical = temp.path().canonicalize().unwrap();
    let root = canonical.as_path();
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
    assert!(matches_configuration(signature, &effective, &store, root, 12).unwrap());
    assert!(!matches_configuration(
        signature,
        &serde_json::json!({"enableGlobalVirtualStore":true}),
        &store,
        root,
        12,
    )
    .unwrap());
    // Local default may not exist; either an error or a mismatch rejects it.
    assert!(!matches_configuration(
        signature,
        &serde_json::json!({"hoist":false}),
        &store,
        root,
        12
    )
    .unwrap_or(false));
}

#[test]
fn pnpm_nine_uses_its_default_public_hoist_patterns() {
    let temp = tempfile::tempdir().unwrap();
    let canonical = temp.path().canonicalize().unwrap();
    let root = canonical.as_path();
    let store = root.join("store");
    std::fs::create_dir_all(&store).unwrap();
    metadata(root, &format!("nodeLinker: isolated\nstoreDir: '{}'\nvirtualStoreDir: .pnpm\nhoistPattern: ['*']\npublicHoistPattern: ['*eslint*', '*prettier*']\n", store.display()));
    let layout = Layout::capture(root).unwrap();
    let signature = layout.modules.get(root).unwrap().as_ref().unwrap();
    assert!(matches_configuration(signature, &serde_json::json!({}), &store, root, 9).unwrap());
    assert!(!matches_configuration(signature, &serde_json::json!({}), &store, root, 10).unwrap());
}
