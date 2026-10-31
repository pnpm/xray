use super::Snapshot;
use std::{fs, path::Path};

#[test]
fn restores_workspace_and_separate_lockfiles_without_touching_unknown_files() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().canonicalize().unwrap();
    let project = directory.join("packages/app");
    fs::create_dir_all(&project).unwrap();
    let workspace = directory.join("pnpm-workspace.yaml");
    let root_lock = directory.join("pnpm-lock.yaml");
    let member_lock = project.join("pnpm-lock.yaml");
    fs::write(&workspace, "original settings\n").unwrap();
    fs::write(&root_lock, "original lock\n").unwrap();
    fs::write(project.join("package.json"), "user manifest\n").unwrap();
    let mut snapshot = Snapshot::capture(root.path(), std::slice::from_ref(&project)).unwrap();
    let patch = snapshot.track_artifact(Path::new("patches/example.patch")).unwrap();
    fs::create_dir_all(patch.parent().unwrap()).unwrap();
    fs::write(&patch, "generated patch").unwrap();
    fs::write(patch.parent().unwrap().join("user.patch"), "user data").unwrap();
    fs::write(&workspace, "changed settings\n").unwrap();
    snapshot.checkpoint(&[workspace.clone(), patch.clone()]).unwrap();
    fs::write(&root_lock, "changed root lock\n").unwrap();
    fs::write(&member_lock, "new member lock\n").unwrap();
    snapshot.checkpoint(&snapshot.lockfiles()).unwrap();
    snapshot.restore().unwrap();
    assert_eq!(fs::read(&workspace).unwrap(), b"original settings\n");
    assert_eq!(fs::read(&root_lock).unwrap(), b"original lock\n");
    assert!(!member_lock.exists());
    assert!(!patch.exists());
    assert_eq!(fs::read(patch.parent().unwrap().join("user.patch")).unwrap(), b"user data");
    assert_eq!(fs::read(project.join("package.json")).unwrap(), b"user manifest\n");
}

#[test]
fn refuses_rollback_if_any_tracked_file_changed_after_checkpoint() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().canonicalize().unwrap();
    let workspace = directory.join("pnpm-workspace.yaml");
    let lock = directory.join("pnpm-lock.yaml");
    fs::write(&workspace, "before").unwrap();
    fs::write(&lock, "lock-before").unwrap();
    let mut snapshot = Snapshot::capture(root.path(), &[]).unwrap();
    fs::write(&workspace, "tool-edit").unwrap();
    fs::write(&lock, "lock-after").unwrap();
    snapshot.checkpoint(&[workspace.clone(), lock.clone()]).unwrap();
    fs::write(&workspace, "user-edit").unwrap();
    assert!(snapshot.restore().is_err());
    assert_eq!(fs::read(&workspace).unwrap(), b"user-edit");
    assert_eq!(fs::read(&lock).unwrap(), b"lock-after");
}

#[test]
fn rejects_untracked_or_escaping_paths() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().canonicalize().unwrap();
    let mut snapshot = Snapshot::capture(root.path(), &[]).unwrap();
    for path in [
        "../outside.patch",
        "patches/nested/file.patch",
        "patches/../outside.patch",
        "patches/file.txt",
    ] {
        assert!(snapshot.track_artifact(Path::new(path)).is_err(), "{path}");
    }
    assert!(snapshot.checkpoint(&[directory.join("package.json")]).is_err());
    let outside = tempfile::tempdir().unwrap();
    assert!(Snapshot::capture(root.path(), &[outside.path().to_path_buf()]).is_err());
}

#[cfg(unix)]
#[test]
fn refuses_symlinked_managed_paths_and_preserves_permissions() {
    use std::os::unix::{fs::symlink, fs::PermissionsExt};
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().canonicalize().unwrap();
    let workspace = directory.join("pnpm-workspace.yaml");
    fs::write(&workspace, "before").unwrap();
    fs::set_permissions(&workspace, fs::Permissions::from_mode(0o640)).unwrap();
    let mut snapshot = Snapshot::capture(root.path(), &[]).unwrap();
    fs::write(&workspace, "after").unwrap();
    fs::set_permissions(&workspace, fs::Permissions::from_mode(0o600)).unwrap();
    snapshot.checkpoint(std::slice::from_ref(&workspace)).unwrap();
    snapshot.restore().unwrap();
    assert_eq!(fs::metadata(&workspace).unwrap().permissions().mode() & 0o777, 0o640);
    let outside = tempfile::tempdir().unwrap();
    symlink(outside.path(), directory.join("patches")).unwrap();
    assert!(snapshot.track_artifact(Path::new("patches/example.patch")).is_err());
}

#[cfg(unix)]
#[test]
fn recovery_contains_private_original_and_checkpointed_content() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().canonicalize().unwrap().join("pnpm-workspace.yaml");
    fs::write(&workspace, "original secret").unwrap();
    fs::set_permissions(&workspace, fs::Permissions::from_mode(0o640)).unwrap();
    let mut snapshot = Snapshot::capture(root.path(), &[]).unwrap();
    fs::write(&workspace, "changed secret").unwrap();
    snapshot.checkpoint(std::slice::from_ref(&workspace)).unwrap();
    let recovery = snapshot.save_recovery(&root.path().join("journals")).unwrap();
    assert_eq!(fs::metadata(&recovery).unwrap().permissions().mode() & 0o777, 0o700);
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(recovery.join("recovery.json")).unwrap()).unwrap();
    let entry = manifest["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["path"] == "pnpm-workspace.yaml")
        .unwrap();
    assert_eq!(entry["before"]["mode"].as_u64().unwrap() & 0o777, 0o640);
    for (field, content) in [("before", "original secret"), ("expected", "changed secret")] {
        let file = recovery.join(entry[field]["file"].as_str().unwrap());
        assert_eq!(fs::read_to_string(&file).unwrap(), content);
        assert_eq!(fs::metadata(file).unwrap().permissions().mode() & 0o777, 0o600);
    }
    assert_eq!(
        fs::metadata(recovery.join("recovery.json")).unwrap().permissions().mode() & 0o777,
        0o600,
    );
    fs::remove_dir_all(recovery).unwrap();
}

#[test]
fn removes_only_its_own_empty_patch_directory() {
    let root = tempfile::tempdir().unwrap();
    let mut snapshot = Snapshot::capture(root.path(), &[]).unwrap();
    let patch = snapshot.track_artifact(Path::new("patches/example.patch")).unwrap();
    snapshot.ensure_patch_directory().unwrap();
    fs::write(&patch, "generated").unwrap();
    snapshot.checkpoint(std::slice::from_ref(&patch)).unwrap();
    snapshot.restore().unwrap();
    assert!(!root.path().join("patches").exists());
    snapshot.checkpoint(std::slice::from_ref(&patch)).unwrap();

    snapshot.ensure_patch_directory().unwrap();
    fs::write(root.path().join("patches/user.txt"), "user").unwrap();
    snapshot.restore().unwrap();
    assert!(root.path().join("patches/user.txt").exists());
}

#[cfg(unix)]
#[test]
fn refreshing_recovery_preserves_original_and_updates_expected() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().canonicalize().unwrap().join("pnpm-workspace.yaml");
    fs::write(&workspace, "before").unwrap();
    let mut snapshot = Snapshot::capture(root.path(), &[]).unwrap();
    let recovery = snapshot.save_recovery(&root.path().join("journals")).unwrap();
    fs::write(&workspace, "after").unwrap();
    snapshot.checkpoint(std::slice::from_ref(&workspace)).unwrap();
    snapshot.refresh_recovery(&recovery).unwrap();
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(recovery.join("recovery.json")).unwrap()).unwrap();
    let entry = manifest["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["path"] == "pnpm-workspace.yaml")
        .unwrap();
    assert_eq!(
        fs::read_to_string(recovery.join(entry["before"]["file"].as_str().unwrap())).unwrap(),
        "before",
    );
    assert_eq!(
        fs::read_to_string(recovery.join(entry["expected"]["file"].as_str().unwrap())).unwrap(),
        "after",
    );
    fs::remove_dir_all(recovery).unwrap();
}

#[test]
fn recovery_roundtrip_and_safe_removal() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().canonicalize().unwrap().join("pnpm-workspace.yaml");
    fs::write(&workspace, "before").unwrap();
    let mut snapshot = Snapshot::capture(root.path(), &[]).unwrap();
    let recovery = snapshot.save_recovery(&root.path().join("journals")).unwrap();
    fs::write(&workspace, "after").unwrap();
    snapshot.checkpoint(std::slice::from_ref(&workspace)).unwrap();
    snapshot.refresh_recovery(&recovery).unwrap();
    let mut loaded = Snapshot::load_recovery(&recovery).unwrap();
    assert_eq!(loaded.workspace_root(), root.path().canonicalize().unwrap());
    loaded.restore().unwrap();
    assert_eq!(fs::read(&workspace).unwrap(), b"before");
    fs::write(recovery.join("notes"), "leave me").unwrap();
    Snapshot::remove_recovery(&recovery).unwrap();
    assert_eq!(fs::read(recovery.join("notes")).unwrap(), b"leave me");
    fs::remove_dir_all(recovery).unwrap();
}

#[test]
fn recovery_rejects_path_escape_and_corrupt_blob() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("pnpm-workspace.yaml"), "before").unwrap();
    let snapshot = Snapshot::capture(root.path(), &[]).unwrap();
    let recovery = snapshot.save_recovery(&root.path().join("journals")).unwrap();
    let manifest_path = recovery.join("recovery.json");
    let original = fs::read(&manifest_path).unwrap();
    let mut manifest: serde_json::Value = serde_json::from_slice(&original).unwrap();
    manifest["files"][0]["path"] = "../outside/pnpm-lock.yaml".into();
    fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert!(Snapshot::load_recovery(&recovery).is_err());
    manifest["files"][0]["path"] = "node_modules/example/pnpm-lock.yaml".into();
    fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert!(Snapshot::load_recovery(&recovery).is_err());
    fs::write(&manifest_path, &original).unwrap();
    let manifest: serde_json::Value = serde_json::from_slice(&original).unwrap();
    let blob = manifest["files"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|entry| entry["before"]["file"].as_str())
        .unwrap();
    fs::write(recovery.join(blob), "corrupted").unwrap();
    assert!(Snapshot::load_recovery(&recovery).is_err());
    fs::remove_dir_all(recovery).unwrap();
}

#[cfg(windows)]
#[test]
fn recovery_directory_has_a_protected_dacl() {
    use std::{os::windows::ffi::OsStrExt as _, ptr};
    use windows_sys::Win32::Security::{
        GetFileSecurityW, GetSecurityDescriptorControl, DACL_SECURITY_INFORMATION,
        SE_DACL_PROTECTED,
    };
    let root = tempfile::tempdir().unwrap();
    let snapshot = Snapshot::capture(root.path(), &[]).unwrap();
    let recovery = snapshot.save_recovery(&root.path().join("journals")).unwrap();
    let filename: Vec<u16> = recovery.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut required = 0;
    // SAFETY: the path is null terminated; the first call requests the required
    // buffer size and the second uses an initialized buffer of that size.
    unsafe {
        GetFileSecurityW(
            filename.as_ptr(),
            DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            0,
            &mut required,
        );
        assert!(required > 0);
        let mut descriptor = vec![0_u8; required as usize];
        assert_ne!(
            GetFileSecurityW(
                filename.as_ptr(),
                DACL_SECURITY_INFORMATION,
                descriptor.as_mut_ptr().cast(),
                required,
                &mut required
            ),
            0,
        );
        let mut control = 0;
        let mut revision = 0;
        assert_ne!(
            GetSecurityDescriptorControl(
                descriptor.as_mut_ptr().cast(),
                &mut control,
                &mut revision
            ),
            0,
        );
        assert_ne!(control & SE_DACL_PROTECTED, 0);
    }
    Snapshot::remove_recovery(&recovery).unwrap();
}

#[cfg(windows)]
#[test]
fn recovery_preserves_and_checks_windows_readonly_permission() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().canonicalize().unwrap().join("pnpm-workspace.yaml");
    fs::write(&path, "before").unwrap();
    let mut original_permissions = fs::metadata(&path).unwrap().permissions();
    original_permissions.set_readonly(true);
    fs::set_permissions(&path, original_permissions).unwrap();
    let mut snapshot = Snapshot::capture(root.path(), &[]).unwrap();
    let recovery = snapshot.save_recovery(&root.path().join("journals")).unwrap();
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_readonly(false);
    fs::set_permissions(&path, permissions).unwrap();
    fs::write(&path, "after").unwrap();
    snapshot.checkpoint(std::slice::from_ref(&path)).unwrap();
    snapshot.refresh_recovery(&recovery).unwrap();
    let mut loaded = Snapshot::load_recovery(&recovery).unwrap();
    let mut changed = fs::metadata(&path).unwrap().permissions();
    changed.set_readonly(true);
    fs::set_permissions(&path, changed).unwrap();
    assert!(loaded.restore().is_err(), "readonly changes must be detected");
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_readonly(false);
    fs::set_permissions(&path, permissions).unwrap();
    loaded.restore().unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"before");
    assert!(fs::metadata(&path).unwrap().permissions().readonly());
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_readonly(false);
    fs::set_permissions(&path, permissions).unwrap();
    Snapshot::remove_recovery(&recovery).unwrap();
}

#[test]
fn journal_roundtrips_layout_and_legacy_requires_manual_recovery() {
    let root = tempfile::tempdir().unwrap();
    let modules = root.path().join("node_modules");
    fs::create_dir_all(modules.join(".pnpm")).unwrap();
    let store = root.path().join("store");
    fs::create_dir_all(&store).unwrap();
    fs::write(modules.join(".modules.yaml"), format!("nodeLinker: isolated\nstoreDir: '{}'\nvirtualStoreDir: .pnpm\npublicHoistPattern: []\n", store.display())).unwrap();
    let layout = crate::verify_layout::Layout::capture(root.path()).unwrap();
    let mut snapshot = Snapshot::capture(root.path(), &[]).unwrap();
    snapshot.set_layout(layout);
    let recovery = snapshot.save_recovery(&root.path().join("journals")).unwrap();
    let loaded = Snapshot::load_recovery(&recovery).unwrap();
    loaded.recovery_layout().unwrap().validate_recovery(root.path()).unwrap();
    let manifest_path = recovery.join("recovery.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    manifest["schemaVersion"] = 1.into();
    manifest.as_object_mut().unwrap().remove("layout");
    fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let legacy = Snapshot::load_recovery(&recovery).unwrap();
    assert!(legacy.recovery_layout().is_err());
    Snapshot::remove_recovery(&recovery).unwrap();
}

fn blob_files(recovery: &Path) -> Vec<String> {
    let mut names: Vec<_> = fs::read_dir(recovery)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.starts_with("xray-blob-"))
        .collect();
    names.sort();
    names
}

#[test]
fn repeated_refreshes_store_each_content_once_and_prune_superseded_blobs() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().canonicalize().unwrap();
    let workspace = directory.join("pnpm-workspace.yaml");
    let lock = directory.join("pnpm-lock.yaml");
    fs::write(&workspace, "settings").unwrap();
    fs::write(&lock, "original lock").unwrap();
    let mut snapshot = Snapshot::capture(root.path(), &[]).unwrap();
    let recovery = snapshot.save_recovery(&root.path().join("journals")).unwrap();
    // Original and checkpointed content are identical: one blob each file.
    assert_eq!(blob_files(&recovery).len(), 2);
    for _ in 0..5 {
        snapshot.refresh_recovery(&recovery).unwrap();
    }
    assert_eq!(blob_files(&recovery).len(), 2);
    for round in 0..5 {
        fs::write(&lock, format!("lock {round}")).unwrap();
        snapshot.checkpoint(std::slice::from_ref(&lock)).unwrap();
        snapshot.refresh_recovery(&recovery).unwrap();
        // The settings, the original lock, and only the latest lock.
        assert_eq!(blob_files(&recovery).len(), 3);
    }
    let mut loaded = Snapshot::load_recovery(&recovery).unwrap();
    loaded.restore().unwrap();
    assert_eq!(fs::read(&lock).unwrap(), b"original lock");
    Snapshot::remove_recovery(&recovery).unwrap();
    assert!(!recovery.exists());
}

#[test]
fn recovery_accepts_original_content_and_any_lockfile_but_not_user_edits() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().canonicalize().unwrap();
    let workspace = directory.join("pnpm-workspace.yaml");
    let lock = directory.join("pnpm-lock.yaml");
    fs::write(&workspace, "original settings").unwrap();
    fs::write(&lock, "original lock").unwrap();
    let mut snapshot = Snapshot::capture(root.path(), &[]).unwrap();
    let recovery = snapshot.save_recovery(&root.path().join("journals")).unwrap();
    fs::write(&workspace, "proposal settings").unwrap();
    fs::write(&lock, "proposal lock").unwrap();
    snapshot.checkpoint(&[workspace.clone(), lock.clone()]).unwrap();
    snapshot.refresh_recovery(&recovery).unwrap();
    // A restore wrote the original settings, then the process was killed
    // while a pnpm install it started kept writing the lockfile.
    fs::write(&workspace, "original settings").unwrap();
    fs::write(&lock, "lock written by an orphaned install").unwrap();
    let mut loaded = Snapshot::load_recovery(&recovery).unwrap();
    assert!(loaded.restore().is_err(), "a rollback still requires the checkpoint");
    loaded.restore_for_recovery().unwrap();
    assert_eq!(fs::read(&workspace).unwrap(), b"original settings");
    assert_eq!(fs::read(&lock).unwrap(), b"original lock");

    fs::write(&workspace, "user edit").unwrap();
    let mut loaded = Snapshot::load_recovery(&recovery).unwrap();
    assert!(loaded.restore_for_recovery().is_err());
    assert_eq!(fs::read(&workspace).unwrap(), b"user edit");
    Snapshot::remove_recovery(&recovery).unwrap();
}

#[test]
fn recovery_removes_an_empty_patch_directory_created_before_a_crash() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().canonicalize().unwrap();
    let mut snapshot = Snapshot::capture(root.path(), &[]).unwrap();
    let patch = snapshot.track_artifact(Path::new("patches/example.patch")).unwrap();
    snapshot.ensure_patch_directory().unwrap();
    let recovery = snapshot.save_recovery(&root.path().join("journals")).unwrap();
    fs::write(&patch, "generated").unwrap();
    snapshot.checkpoint(std::slice::from_ref(&patch)).unwrap();
    snapshot.refresh_recovery(&recovery).unwrap();
    drop(snapshot);
    let mut loaded = Snapshot::load_recovery(&recovery).unwrap();
    loaded.restore_for_recovery().unwrap();
    assert!(!directory.join("patches").exists());

    // A directory that already existed is never removed.
    fs::create_dir(directory.join("patches")).unwrap();
    let mut snapshot = Snapshot::capture(root.path(), &[]).unwrap();
    snapshot.track_artifact(Path::new("patches/example.patch")).unwrap();
    snapshot.ensure_patch_directory().unwrap();
    snapshot.refresh_recovery(&recovery).unwrap();
    Snapshot::load_recovery(&recovery).unwrap().restore_for_recovery().unwrap();
    assert!(directory.join("patches").is_dir());
    Snapshot::remove_recovery(&recovery).unwrap();
}
