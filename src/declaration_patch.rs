//! Persist compiler-verified declaration changes as pnpm patches.
use crate::{manifest::Manifest, regenerate::DeclarationPatch, repair};
use anyhow::{bail, Context, Result};
use std::{
    collections::BTreeMap,
    fmt::Write as _,
    io::Write as _,
    path::{Component, Path, PathBuf},
};

pub struct PatchPlan {
    pub configuration: String,
    pub artifacts: BTreeMap<PathBuf, String>,
}

pub fn plan(patches: &[DeclarationPatch], packages: &[PathBuf]) -> Result<PatchPlan> {
    if patches.is_empty() {
        return Ok(PatchPlan { configuration: String::new(), artifacts: BTreeMap::new() });
    }
    let mut roots: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    for package in packages {
        roots.entry(Manifest::read(package)?.id()).or_default().push(package.canonicalize()?);
    }
    let mut artifacts = BTreeMap::new();
    let mut configuration = String::new();
    for patch in patches {
        let package_roots =
            roots.get(&patch.package).context("verified package is no longer installed")?;
        let filename = format!("{}.patch", patch.package.replace('/', "+"));
        if !filename
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "@._+-".contains(character))
        {
            bail!("unsupported patch identity {}", patch.package);
        }
        let mut content = String::new();
        for file in &patch.files {
            if file.path.components().any(|component| !matches!(component, Component::Normal(_))) {
                bail!("declaration patch contains a path outside its package");
            }
            if file.path.components().any(|component| component.as_os_str() == "node_modules") {
                bail!("declaration patch belongs to a nested dependency");
            }
            let path =
                file.path.to_str().context("declaration path is not UTF-8")?.replace('\\', "/");
            if !path
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "/@._+-".contains(character))
                || ![".d.ts", ".d.mts", ".d.cts"].iter().any(|extension| path.ends_with(extension))
            {
                bail!("unsupported declaration patch path {path}");
            }
            if file.original == file.replacement {
                continue;
            }
            for root in package_roots {
                let actual = root.join(&file.path).canonicalize()?;
                if actual != root.join(&file.path)
                    || std::fs::read_to_string(&actual)? != file.original
                {
                    bail!("{} changed after declaration verification", patch.package);
                }
            }
            content.push_str(&diff(&path, &file.original, &file.replacement));
        }
        if content.is_empty() {
            continue;
        }
        let relative = Path::new("patches").join(&filename);
        if let Some(existing) = artifacts.insert(relative, content.clone()) {
            if existing != content {
                bail!("conflicting verified patches for {}", patch.package);
            }
        }
        if configuration.is_empty() {
            configuration.push_str("patchedDependencies:\n");
        }
        let _ = writeln!(
            configuration,
            "  '{}': 'patches/{filename}'",
            patch.package.replace('\'', "''"),
        );
    }
    Ok(PatchPlan { configuration, artifacts })
}

fn diff(path: &str, original: &str, replacement: &str) -> String {
    let old_count = original.lines().count();
    let new_count = replacement.lines().count();
    let old_start = usize::from(old_count > 0);
    let new_start = usize::from(new_count > 0);
    let mut output = format!("diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ -{old_start},{old_count} +{new_start},{new_count} @@\n");
    for (prefix, text) in [('-', original), ('+', replacement)] {
        for line in text.split_inclusive('\n') {
            output.push(prefix);
            output.push_str(line);
            if !line.ends_with('\n') {
                output.push_str("\n\\ No newline at end of file\n");
            }
        }
    }
    output
}

/// Validate a proposed edit without creating any files. Dry runs and writes
/// use the same checks, including existing patch content and symlink checks.
pub fn validate(workspace: &Path, proposed: &str, plan: &PatchPlan) -> Result<()> {
    preflight(workspace, proposed, plan).map(|_| ())
}

fn preflight(workspace: &Path, proposed: &str, plan: &PatchPlan) -> Result<(String, PathBuf)> {
    let original = match std::fs::read_to_string(workspace) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.into()),
    };
    repair::merge(&original, proposed)?;
    let root = workspace.parent().context("workspace has no parent")?.canonicalize()?;
    let patch_dir = root.join("patches");
    if !plan.artifacts.is_empty()
        && std::fs::symlink_metadata(&patch_dir)
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        bail!("workspace patch directory must not be a symlink");
    }
    for (relative, content) in &plan.artifacts {
        if relative.parent() != Some(Path::new("patches"))
            || relative.components().any(|component| !matches!(component, Component::Normal(_)))
        {
            bail!("patch artifact escapes the workspace patch directory");
        }
        let path = root.join(relative);
        if std::fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            bail!("patch artifact must not be a symlink: {}", path.display());
        }
        if path.exists()
            && (!path.canonicalize()?.starts_with(&root)
                || std::fs::read_to_string(&path)? != *content)
        {
            bail!("existing patch {} conflicts with declaration repair", path.display());
        }
    }
    Ok((original, root))
}

/// Existing patch files are immutable unless their contents already match.
pub fn apply(workspace: &Path, proposed: &str, plan: &PatchPlan) -> Result<bool> {
    let (original, root) = preflight(workspace, proposed, plan)?;
    let mut created = Vec::new();
    let applied = (|| -> Result<bool> {
        for (relative, content) in &plan.artifacts {
            let path = root.join(relative);
            if path.exists() {
                continue;
            }
            let directory = path.parent().context("patch has no parent")?;
            std::fs::create_dir_all(directory)?;
            if !directory.canonicalize()?.starts_with(&root) {
                bail!("patch directory escapes workspace");
            }
            let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
            temporary.write_all(content.as_bytes())?;
            temporary.as_file().sync_all()?;
            temporary
                .persist_noclobber(&path)
                .with_context(|| format!("writing {}", path.display()))?;
            created.push((path, content));
        }
        let current = match std::fs::read_to_string(workspace) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(error.into()),
        };
        if current != original {
            bail!("{} changed during repair; rerun xray", workspace.display());
        }
        Ok(repair::apply(workspace, proposed)? || !created.is_empty())
    })();
    match applied {
        Ok(changed) => Ok(changed),
        Err(error) => {
            let mut failures = Vec::new();
            for (path, content) in created {
                let unchanged = std::fs::symlink_metadata(&path)
                    .is_ok_and(|metadata| !metadata.file_type().is_symlink())
                    && std::fs::read_to_string(&path).is_ok_and(|current| current == *content);
                if !unchanged {
                    failures.push(format!("{} changed during rollback", path.display()));
                } else if let Err(rollback_error) = std::fs::remove_file(&path) {
                    failures.push(format!("{}: {rollback_error}", path.display()));
                }
            }
            if !failures.is_empty() {
                bail!("{error}; could not roll back patch artifacts: {}", failures.join(", "));
            }
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests;
