use super::{Lease, WorkspaceLock};

#[test]
fn one_verification_or_recovery_per_workspace() {
    let state = tempfile::tempdir().unwrap();
    let locks = state.path().join("nested/locks");
    let workspace = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let first = WorkspaceLock::acquire_in(&locks, workspace.path()).unwrap();
    // The same workspace through a different spelling is still the same lock.
    let alias = workspace.path().join(".");
    let error = WorkspaceLock::acquire_in(&locks, &alias).err().unwrap();
    assert!(error.to_string().contains("running"), "{error:#}");
    let unrelated = WorkspaceLock::acquire_in(&locks, other.path()).unwrap();
    drop(first);
    let again = WorkspaceLock::acquire_in(&locks, workspace.path()).unwrap();
    drop((again, unrelated));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&locks).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }
}

#[test]
fn exclusive_until_drop_and_reusable_afterwards() {
    let dir = tempfile::tempdir().unwrap();
    let first = Lease::acquire(dir.path()).unwrap();
    assert!(Lease::acquire(dir.path()).is_err());
    drop(first);
    let second = Lease::acquire(dir.path()).unwrap();
    second.remove().unwrap();
    assert!(!dir.path().join(".xray-lock").exists());
}

#[test]
fn replacement_file_is_not_removed() {
    let dir = tempfile::tempdir().unwrap();
    let lease = Lease::acquire(dir.path()).unwrap();
    let path = dir.path().join(".xray-lock");
    std::fs::rename(&path, dir.path().join("old-lock")).unwrap();
    std::fs::write(&path, "unrelated").unwrap();
    assert!(lease.remove().is_err());
    assert_eq!(std::fs::read_to_string(path).unwrap(), "unrelated");
}

#[cfg(unix)]
#[test]
fn rejects_symlinks_without_modifying_target() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target");
    std::fs::write(&target, "untouched").unwrap();
    symlink(&target, dir.path().join(".xray-lock")).unwrap();
    assert!(Lease::acquire(dir.path()).is_err());
    assert_eq!(std::fs::read_to_string(target).unwrap(), "untouched");
}
