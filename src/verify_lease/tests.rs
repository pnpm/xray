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

#[cfg(unix)]
#[test]
fn dropping_guards_releases_locks_before_a_forked_child_execs() {
    fn with_unexecuted_child<Output>(operation: impl FnOnce() -> Output) -> Output {
        let mut descriptors = [0; 2];
        // SAFETY: descriptors points to two writable integers. The child calls
        // only async-signal-safe functions and exits without running Rust code.
        unsafe {
            assert_eq!(libc::pipe(descriptors.as_mut_ptr()), 0);
            let child = libc::fork();
            assert!(child >= 0, "fork failed");
            if child == 0 {
                libc::close(descriptors[1]);
                let mut byte = 0_u8;
                libc::read(descriptors[0], (&raw mut byte).cast(), 1);
                libc::_exit(0);
            }
            libc::close(descriptors[0]);
            let result = operation();
            let byte = 1_u8;
            libc::write(descriptors[1], (&raw const byte).cast(), 1);
            libc::close(descriptors[1]);
            let mut status = 0;
            assert_eq!(libc::waitpid(child, &raw mut status, 0), child);
            result
        }
    }

    let directory = tempfile::tempdir().unwrap();
    let lease = Lease::acquire(directory.path()).unwrap();
    let reacquired = with_unexecuted_child(|| {
        drop(lease);
        Lease::acquire(directory.path())
    });
    reacquired.unwrap().remove().unwrap();

    let state = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let lock = WorkspaceLock::acquire_in(state.path(), workspace.path()).unwrap();
    let reacquired = with_unexecuted_child(|| {
        drop(lock);
        WorkspaceLock::acquire_in(state.path(), workspace.path())
    });
    drop(reacquired.unwrap());
}
