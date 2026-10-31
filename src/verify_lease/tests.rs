use super::Lease;

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
