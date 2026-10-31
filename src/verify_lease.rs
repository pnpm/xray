//! An exclusive operating-system lease for one recovery journal.
use anyhow::{bail, Context, Result};
use std::{
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
};

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
