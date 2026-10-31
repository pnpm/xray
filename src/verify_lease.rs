//! Exclusive operating-system leases for recovery journals and workspaces,
//! and the persistent directory that holds them.
use anyhow::{bail, Context, Result};
use sha2::{Digest as _, Sha256};
use std::{
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
};

/// A per-user directory that survives reboots. The system temp directory is
/// often a tmpfs, which would lose a recovery journal exactly when a crash
/// makes it necessary. `XDG_STATE_HOME` is honoured on every platform.
pub fn state_directory() -> Result<PathBuf> {
    let absolute =
        |name: &str| std::env::var_os(name).map(PathBuf::from).filter(|path| path.is_absolute());
    if let Some(state) = absolute("XDG_STATE_HOME") {
        return Ok(state.join("xray"));
    }
    #[cfg(windows)]
    let base = absolute("LOCALAPPDATA");
    #[cfg(target_os = "macos")]
    let base = absolute("HOME").map(|home| home.join("Library/Application Support"));
    #[cfg(not(any(windows, target_os = "macos")))]
    let base = absolute("HOME").map(|home| home.join(".local/state"));
    base.map(|base| base.join("xray")).context(
        "cannot locate a persistent state directory for the recovery journal; set XDG_STATE_HOME",
    )
}

/// Create a private directory and its missing parents, then make the new
/// entries durable so a journal created inside it survives a crash.
pub fn create_private_directory(path: &Path) -> Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder.create(path).with_context(|| format!("creating {}", path.display()))?;
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        bail!("{} must be a real directory", path.display());
    }
    sync_directory(path)?;
    // Ancestors may be unreadable (search-only); their entries are best effort.
    for ancestor in path.ancestors().skip(1) {
        if sync_directory(ancestor).is_err() {
            break;
        }
    }
    Ok(())
}

/// Make renames and creations inside `directory` durable. Windows offers no
/// directory handle sync through the standard library and needs none for
/// NTFS metadata journaling.
pub fn sync_directory(directory: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(directory)
        .and_then(|file| file.sync_all())
        .with_context(|| format!("syncing directory {}", directory.display()))?;
    #[cfg(not(unix))]
    let _ = directory;
    Ok(())
}

/// Serializes verification and recovery on one workspace. The lock file is
/// keyed by the canonical workspace root and is never removed, so two runs
/// always contend for the same file.
pub struct WorkspaceLock {
    _file: File,
}

impl WorkspaceLock {
    pub fn acquire(workspace: &Path) -> Result<Self> {
        Self::acquire_in(&state_directory()?.join("locks"), workspace)
    }

    pub fn acquire_in(directory: &Path, workspace: &Path) -> Result<Self> {
        let root = workspace.canonicalize().context("resolving workspace to lock")?;
        create_private_directory(directory)?;
        let key = Sha256::digest(root.as_os_str().as_encoded_bytes());
        let path = directory.join(format!("{key:x}.lock"));
        reject_symlink(&path)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let file = options.open(&path).context("opening workspace lock")?;
        file.try_lock().map_err(|_| {
            anyhow::anyhow!(
                "another xray verification or recovery is running for {}",
                root.display()
            )
        })?;
        Ok(Self { _file: file })
    }
}

pub struct Lease {
    file: File,
    path: PathBuf,
    identity: same_file::Handle,
}

impl Lease {
    /// Acquire without waiting. A stale lock file is reusable after a crash.
    pub fn acquire(journal: &Path) -> Result<Self> {
        let directory = std::fs::symlink_metadata(journal)?;
        if !directory.is_dir() || directory.file_type().is_symlink() {
            bail!("recovery journal must be a real directory");
        }
        let path = journal.join(".xray-lock");
        reject_symlink(&path)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let file = options.open(&path).context("opening recovery journal lease")?;
        let identity = same_file::Handle::from_file(file.try_clone()?)?;
        reject_symlink(&path)?;
        if same_file::Handle::from_path(&path)? != identity {
            bail!("recovery journal lease changed while opening");
        }
        file.try_lock().context("recovery journal is already in use")?;
        reject_symlink(&path)?;
        if same_file::Handle::from_path(&path)? != identity {
            bail!("recovery journal lease changed while locking");
        }
        Ok(Self { file, path, identity })
    }

    /// Remove only the same lock file, while the operating-system lock is held.
    pub fn remove(self) -> Result<()> {
        reject_symlink(&self.path)?;
        if same_file::Handle::from_path(&self.path)? != self.identity {
            bail!("recovery journal lease was replaced; leaving it untouched");
        }
        std::fs::remove_file(&self.path).context("removing recovery journal lease")?;
        drop(self.file);
        Ok(())
    }
}

fn reject_symlink(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            bail!("recovery journal lease is a symlink")
        }
        Ok(metadata) if !metadata.is_file() => {
            bail!("recovery journal lease is not a regular file")
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("checking recovery journal lease"),
    }
}

#[cfg(test)]
#[path = "verify_lease/tests.rs"]
mod tests;
