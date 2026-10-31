//! Snapshot only files that Xray and pnpm may change during verified repair.
use anyhow::{bail, Context, Result};
use sha2::{Digest as _, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, Permissions},
    io::Write as _,
    path::{Component, Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq)]
enum FileState {
    Absent,
    Present { bytes: Vec<u8>, permissions: FilePermissions },
}

/// Compare and restore only permissions exposed by the platform. Windows
/// `Permissions` also retains unrelated file attributes from its source file.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FilePermissions {
    #[cfg(unix)]
    value: Permissions,
    #[cfg(not(unix))]
    readonly: bool,
}

impl FilePermissions {
    #[cfg(unix)]
    const fn from_permissions(permissions: Permissions) -> Self {
        Self { value: permissions }
    }

    #[cfg(not(unix))]
    fn from_permissions(permissions: Permissions) -> Self {
        Self { readonly: permissions.readonly() }
    }

    fn from_mode(mode: u32) -> Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            if mode > 0o177_777 {
                bail!("invalid recovery permissions");
            }
            Ok(Self { value: Permissions::from_mode(mode) })
        }
        #[cfg(not(unix))]
        {
            if mode > 1 {
                bail!("invalid recovery permissions");
            }
            Ok(Self { readonly: mode == 1 })
        }
    }

    fn mode(&self) -> u32 {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            self.value.mode()
        }
        #[cfg(not(unix))]
        {
            u32::from(self.readonly)
        }
    }

    fn apply(&self, file: &fs::File) -> Result<()> {
        #[cfg(unix)]
        {
            file.set_permissions(self.value.clone())?;
        }
        #[cfg(not(unix))]
        {
            let mut permissions = file.metadata()?.permissions();
            permissions.set_readonly(self.readonly);
            file.set_permissions(permissions)?;
        }
        Ok(())
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Manifest {
    schema_version: u8,
    root: PathBuf,
    files: Vec<Entry>,
    owned_blobs: BTreeSet<String>,
    #[serde(default)]
    layout: Option<crate::verify_layout::Layout>,
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    path: PathBuf,
    before: Option<Blob>,
    expected: Option<Blob>,
    lockfile: bool,
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Blob {
    file: String,
    mode: u32,
    sha256: String,
}

fn read_recovery_blob(
    directory: &Path,
    owned_blobs: &BTreeSet<String>,
    blob: Option<Blob>,
) -> Result<FileState> {
    let Some(blob) = blob else { return Ok(FileState::Absent) };
    if !owned_blobs.contains(&blob.file) {
        bail!("unowned recovery blob");
    }
    let name = Path::new(&blob.file);
    if !matches!(name.components().next(), Some(Component::Normal(_)))
        || name.components().count() != 1
        || !blob.file.starts_with("xray-blob-")
    {
        bail!("invalid recovery blob path");
    }
    let path = directory.join(name);
    let metadata = fs::symlink_metadata(&path)?;
    if !metadata.file_type().is_file() {
        bail!("recovery blob must be a regular file");
    }
    let bytes = fs::read(path)?;
    if format!("{:x}", Sha256::digest(&bytes)) != blob.sha256 {
        bail!("recovery blob checksum mismatch");
    }
    let permissions = FilePermissions::from_mode(blob.mode)?;
    Ok(FileState::Present { bytes, permissions })
}

struct TrackedFile {
    before: FileState,
    expected: FileState,
}

/// The last checkpoint is a trust boundary: call it only immediately after a
/// controlled operation, never after the user's verification command.
pub struct Snapshot {
    root: PathBuf,
    files: BTreeMap<PathBuf, TrackedFile>,
    lockfiles: BTreeSet<PathBuf>,
    owned_patch_dir: Option<same_file::Handle>,
    layout: Option<crate::verify_layout::Layout>,
}

impl Snapshot {
    /// Capture workspace settings and lockfiles, including separate importers.
    pub fn capture(root: &Path, importers: &[PathBuf]) -> Result<Self> {
        let root = root.canonicalize().context("resolving transaction root")?;
        let mut snapshot = Self {
            root: root.clone(),
            files: BTreeMap::new(),
            lockfiles: BTreeSet::new(),
            owned_patch_dir: None,
            layout: None,
        };
        snapshot.track(root.join("pnpm-workspace.yaml"))?;
        let mut projects = BTreeSet::from([root.clone()]);
        for importer in importers {
            let importer = importer.canonicalize().context("resolving transaction importer")?;
            if !importer.starts_with(&root) {
                bail!("transaction importer escapes workspace");
            }
            projects.insert(importer);
        }
        for project in projects {
            let path = project.join("pnpm-lock.yaml");
            snapshot.track(path.clone())?;
            snapshot.lockfiles.insert(path);
        }
        Ok(snapshot)
    }

    /// Record an artifact before Xray creates it. Only direct patch files are
    /// accepted; unrelated files and directories are never tracked or removed.
    pub fn track_artifact(&mut self, relative: &Path) -> Result<PathBuf> {
        let mut components = relative.components();
        if components.next() != Some(Component::Normal("patches".as_ref()))
            || !matches!(components.next(), Some(Component::Normal(_)))
            || components.next().is_some()
            || relative.extension().is_none_or(|extension| extension != "patch")
        {
            bail!("transaction artifact must be a direct patches/*.patch path");
        }
        let path = self.root.join(relative);
        self.track(path.clone())?;
        Ok(path)
    }

    /// Create the patch directory ourselves so rollback can remove it safely.
    pub fn ensure_patch_directory(&mut self) -> Result<()> {
        let path = self.root.join("patches");
        match fs::create_dir(&path) {
            Ok(()) => self.owned_patch_dir = Some(same_file::Handle::from_path(&path)?),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let metadata = fs::symlink_metadata(&path)?;
                if !metadata.is_dir() || metadata.file_type().is_symlink() {
                    bail!("patch directory is not a real directory");
                }
            }
            Err(error) => return Err(error.into()),
        }
        Ok(())
    }

    pub fn workspace_root(&self) -> &Path {
        &self.root
    }

    /// Record the original pnpm layout before persisting the journal.
    pub fn set_layout(&mut self, layout: crate::verify_layout::Layout) {
        self.layout = Some(layout);
    }

    /// Older journals lack a layout proof and cannot safely reinstall a graph.
    pub fn recovery_layout(&self) -> Result<&crate::verify_layout::Layout> {
        self.layout
            .as_ref()
            .context("recovery journal lacks the original pnpm layout; manual recovery is required")
    }

    /// All managed files, including generated artifacts.
    pub fn tracked_paths(&self) -> Vec<PathBuf> {
        self.files.keys().cloned().collect()
    }

    /// Paths a controlled pnpm install may update.
    pub fn lockfiles(&self) -> Vec<PathBuf> {
        self.lockfiles.iter().cloned().collect()
    }

    /// The canonical workspace settings path to checkpoint after Xray writes it.
    pub fn workspace_file(&self) -> PathBuf {
        self.root.join("pnpm-workspace.yaml")
    }

    /// Check for edits since the most recent controlled checkpoint.
    pub fn verify_unchanged(&self) -> Result<()> {
        self.check_expected()
    }

    /// Accept the current states of specified tracked files after a controlled
    /// mutation. All reads succeed before any expected state is updated.
    pub fn checkpoint(&mut self, paths: &[PathBuf]) -> Result<()> {
        let mut states = Vec::new();
        for path in paths {
            if !self.files.contains_key(path) {
                bail!("cannot checkpoint an untracked path: {}", path.display());
            }
            states.push((path.clone(), self.read_state(path)?));
        }
        for (path, state) in states {
            self.files.get_mut(&path).expect("tracked path was checked").expected = state;
        }
        Ok(())
    }

    /// Persist original bytes before any mutation. Keep this directory until
    /// the transaction succeeds or a complete rollback has been verified.
    pub fn save_recovery(&self) -> Result<PathBuf> {
        let directory = tempfile::Builder::new().prefix("xray-recovery-").tempdir()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(directory.path(), Permissions::from_mode(0o700))?;
        }
        #[cfg(windows)]
        protect_recovery_directory(directory.path())?;
        self.refresh_recovery(directory.path())?;
        Ok(directory.keep())
    }

    /// Publish a complete new manifest atomically; old blobs remain available
    /// if the process stops during an update.
    pub fn refresh_recovery(&self, directory: &Path) -> Result<()> {
        let manifest_path = directory.join("recovery.json");
        let mut owned: BTreeSet<String> = if manifest_path.exists() {
            let previous: serde_json::Value = serde_json::from_slice(&fs::read(&manifest_path)?)?;
            serde_json::from_value(
                previous.get("ownedBlobs").cloned().context("missing owned blobs")?,
            )?
        } else {
            BTreeSet::new()
        };
        let mut entries = Vec::new();
        for (path, tracked) in &self.files {
            let state = |value: &FileState| -> Result<serde_json::Value> {
                match value {
                    FileState::Absent => Ok(serde_json::Value::Null),
                    FileState::Present { bytes, permissions } => {
                        let mut file =
                            tempfile::Builder::new().prefix("xray-blob-").tempfile_in(directory)?;
                        file.write_all(bytes)?;
                        file.as_file().sync_all()?;
                        let (_, path) = file.keep()?;
                        let name = path
                            .file_name()
                            .and_then(|name| name.to_str())
                            .context("recovery blob has no name")?;
                        let mode = permissions.mode();
                        Ok(
                            serde_json::json!({ "file": name, "mode": mode, "sha256": format!("{:x}", Sha256::digest(bytes)) }),
                        )
                    }
                }
            };
            let before = state(&tracked.before)?;
            let expected = state(&tracked.expected)?;
            for value in [&before, &expected] {
                if let Some(name) = value.get("file").and_then(serde_json::Value::as_str) {
                    owned.insert(name.to_owned());
                }
            }
            entries.push(serde_json::json!({
                "path": path.strip_prefix(&self.root)?,
                "before": before,
                "expected": expected,
                "lockfile": self.lockfiles.contains(path),
            }));
        }
        let manifest = serde_json::json!({ "schemaVersion": 2, "root": self.root, "files": entries, "ownedBlobs": owned, "layout": self.layout });
        let mut file = tempfile::NamedTempFile::new_in(directory)?;
        serde_json::to_writer_pretty(&mut file, &manifest)?;
        file.as_file().sync_all()?;
        file.persist(directory.join("recovery.json"))?;
        Ok(())
    }

    /// Load a journal after a crash. Untrusted paths, blobs, and checksums are
    /// validated before the returned snapshot can touch the workspace.
    pub fn load_recovery(directory: &Path) -> Result<Self> {
        let metadata = fs::symlink_metadata(directory)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            bail!("recovery directory must be a real directory");
        }
        let manifest_path = directory.join("recovery.json");
        if !fs::symlink_metadata(&manifest_path)?.file_type().is_file() {
            bail!("recovery manifest must be a regular file");
        }
        let manifest: Manifest = serde_json::from_slice(&fs::read(manifest_path)?)?;
        if ![1, 2].contains(&manifest.schema_version) {
            bail!("unsupported recovery schema");
        }
        let root = manifest.root.canonicalize().context("resolving recovery workspace")?;
        let mut snapshot = Self {
            root: root.clone(),
            files: BTreeMap::new(),
            lockfiles: BTreeSet::new(),
            owned_patch_dir: None,
            layout: None,
        };
        if manifest.schema_version == 2 {
            snapshot.layout = manifest.layout;
        }
        for entry in manifest.files {
            let relative = &entry.path;
            if relative.components().any(|part| !matches!(part, Component::Normal(_)))
                || relative.as_os_str().is_empty()
            {
                bail!("invalid recovery path");
            }
            let is_workspace = relative == Path::new("pnpm-workspace.yaml");
            let is_lock = relative.file_name().is_some_and(|name| name == "pnpm-lock.yaml")
                && !relative
                    .components()
                    .any(|part| part == Component::Normal("node_modules".as_ref()));
            let mut components = relative.components();
            let is_patch = components.next() == Some(Component::Normal("patches".as_ref()))
                && matches!(components.next(), Some(Component::Normal(_)))
                && components.next().is_none()
                && relative.extension().is_some_and(|ext| ext == "patch");
            if !(is_workspace || is_lock || is_patch) || entry.lockfile != is_lock {
                bail!("invalid managed recovery path");
            }
            let path = root.join(relative);
            let tracked = TrackedFile {
                before: read_recovery_blob(directory, &manifest.owned_blobs, entry.before)?,
                expected: read_recovery_blob(directory, &manifest.owned_blobs, entry.expected)?,
            };
            if snapshot.files.insert(path.clone(), tracked).is_some() {
                bail!("duplicate recovery path");
            }
            if entry.lockfile {
                snapshot.lockfiles.insert(path);
            }
        }
        if !snapshot.files.contains_key(&snapshot.workspace_file())
            || !snapshot.lockfiles.contains(&root.join("pnpm-lock.yaml"))
        {
            bail!("recovery manifest lacks required workspace files");
        }
        Ok(snapshot)
    }

    /// Remove only blobs recorded in this journal, never unknown user files.
    pub fn remove_recovery(directory: &Path) -> Result<()> {
        // Validate every live reference and managed path before deleting anything.
        let _snapshot = Self::load_recovery(directory)?;
        let manifest_path = directory.join("recovery.json");
        let manifest: serde_json::Value = serde_json::from_slice(&fs::read(&manifest_path)?)?;
        let owned: BTreeSet<String> = serde_json::from_value(manifest["ownedBlobs"].clone())?;
        for name in &owned {
            let path = Path::new(name);
            if !name.starts_with("xray-blob-")
                || path.components().count() != 1
                || !matches!(path.components().next(), Some(Component::Normal(_)))
            {
                bail!("invalid recovery blob path");
            }
            let metadata = fs::symlink_metadata(directory.join(path))?;
            if !metadata.file_type().is_file() {
                bail!("recovery blob must be a regular file");
            }
        }
        for name in owned {
            fs::remove_file(directory.join(name))?;
        }
        fs::remove_file(manifest_path)?;
        match fs::remove_dir(directory) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::DirectoryNotEmpty => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    /// Refuse all changes if any tracked file differs from the last checkpoint.
    /// Restores each file atomically; an I/O failure can still interrupt a
    /// multi-file restore and is returned to the caller for recovery.
    pub fn restore(&mut self) -> Result<()> {
        self.check_expected()?;
        let mut staged = BTreeMap::new();
        for (path, tracked) in &self.files {
            if tracked.before == tracked.expected {
                continue;
            }
            if let FileState::Present { bytes, permissions } = &tracked.before {
                let parent = path.parent().context("transaction path has no parent")?;
                let mut temp = tempfile::NamedTempFile::new_in(parent)?;
                permissions.apply(temp.as_file())?;
                temp.write_all(bytes)?;
                temp.as_file().sync_all()?;
                staged.insert(path.clone(), temp);
            }
        }
        self.check_expected()?;
        for (path, tracked) in &self.files {
            if tracked.before == tracked.expected {
                continue;
            }
            if self.read_state(path)? != tracked.expected {
                bail!("transaction file changed during rollback: {}", path.display());
            }
            match &tracked.before {
                FileState::Absent => match fs::remove_file(path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(error).with_context(|| format!("removing {}", path.display()))
                    }
                },
                FileState::Present { permissions, .. } => {
                    let restored = staged
                        .remove(path)
                        .expect("restoration was staged")
                        .persist(path)
                        .with_context(|| format!("restoring {}", path.display()))?;
                    // On Windows tempfile::persist resets file attributes to
                    // NORMAL before renaming. Apply permissions to the open
                    // file handle so a path replacement cannot redirect this.
                    permissions
                        .apply(&restored)
                        .with_context(|| format!("restoring permissions on {}", path.display()))?;
                    restored.sync_all()?;
                }
            }
        }
        if let Some(original) = self.owned_patch_dir.take() {
            let path = self.root.join("patches");
            match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                    let same_directory = same_file::Handle::from_path(&path)? == original;
                    drop(original);
                    if same_directory {
                        match fs::remove_dir(path) {
                            Ok(()) => {}
                            Err(error) if error.kind() == std::io::ErrorKind::DirectoryNotEmpty => {
                            }
                            Err(error) => return Err(error.into()),
                        }
                    }
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    fn track(&mut self, path: PathBuf) -> Result<()> {
        if self.files.contains_key(&path) {
            return Ok(());
        }
        let state = self.read_state(&path)?;
        self.files.insert(path, TrackedFile { before: state.clone(), expected: state });
        Ok(())
    }

    fn check_expected(&self) -> Result<()> {
        for (path, tracked) in &self.files {
            if self.read_state(path)? != tracked.expected {
                bail!("transaction file changed since checkpoint: {}", path.display());
            }
        }
        Ok(())
    }

    fn read_state(&self, path: &Path) -> Result<FileState> {
        let relative =
            path.strip_prefix(&self.root).context("transaction path escapes workspace")?;
        let mut current = self.root.clone();
        for component in relative.components() {
            let Component::Normal(name) = component else {
                bail!("invalid transaction path");
            };
            current.push(name);
            match fs::symlink_metadata(&current) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    bail!("transaction path contains a symlink: {}", current.display())
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(FileState::Absent)
                }
                Err(error) => {
                    return Err(error).with_context(|| format!("checking {}", current.display()))
                }
            }
        }
        let metadata = fs::metadata(path)?;
        if !metadata.is_file() {
            bail!("transaction path is not a regular file: {}", path.display());
        }
        Ok(FileState::Present {
            bytes: fs::read(path)?,
            permissions: FilePermissions::from_permissions(metadata.permissions()),
        })
    }
}

#[cfg(test)]
#[path = "verify_transaction/tests.rs"]
mod tests;

#[cfg(windows)]
fn protect_recovery_directory(path: &Path) -> Result<()> {
    use std::{os::windows::ffi::OsStrExt as _, ptr};
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::{
            Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW, SetFileSecurityW,
            DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
        },
    };
    // The owner-rights SID grants full access to the creator and descendants;
    // P protects the DACL from inheriting access from a shared temp parent.
    let descriptor: Vec<u16> = "D:P(A;OICI;FA;;;OW)\0".encode_utf16().collect();
    let filename: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut security = ptr::null_mut();
    // SAFETY: all pointers refer to live, null-terminated buffers; Windows
    // allocates the returned security descriptor, which is freed below.
    unsafe {
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            descriptor.as_ptr(),
            1,
            &mut security,
            ptr::null_mut(),
        ) == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        let success = SetFileSecurityW(
            filename.as_ptr(),
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            security,
        );
        let error = std::io::Error::last_os_error();
        LocalFree(security);
        if success == 0 {
            return Err(error.into());
        }
    }
    Ok(())
}
