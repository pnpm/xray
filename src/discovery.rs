#[cfg(test)]
mod tests;

use anyhow::{bail, Context, Result};
use std::{
    collections::{BTreeSet, HashSet},
    path::{Path, PathBuf},
};

/// Every package reachable from the project, found by walking `node_modules`
/// directories through their real paths. A package's own dependencies sit in the
/// same directory it does, so following each one to where it really lives and
/// reading that directory again covers the whole graph — under a project-local
/// virtual store and a global one alike.
pub fn installed_packages(root: &Path) -> Result<Vec<PathBuf>> {
    let root_modules = root.join("node_modules");
    let projects = crate::workspace::projects(root)?;
    if !projects.iter().any(|project| project.join("node_modules").is_dir()) {
        bail!(
            "{} has no installed dependencies — run an install first, or pass --package",
            root.display(),
        );
    }

    let canonical_root = root.canonicalize()?;
    let source_boundary = canonical_root
        .ancestors()
        .find(|directory| directory.join("pnpm-workspace.yaml").is_file())
        .unwrap_or(&canonical_root)
        .to_path_buf();
    let mut workspace_roots = HashSet::from([canonical_root]);
    let mut queue = vec![root_modules.clone(), root_modules.join(".pnpm/node_modules")];
    // Workspace-only dependencies need not be reachable from the root package.
    for project in projects {
        queue.push(project.join("node_modules"));
        workspace_roots.insert(project);
    }
    let mut visited_dirs = HashSet::new();
    let mut packages = BTreeSet::new();

    while let Some(dir) = queue.pop() {
        let dir = match dir.canonicalize() {
            Ok(directory) => directory,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("resolving dependency directory {}", dir.display()))
            }
        };
        if !dir.is_dir() || !visited_dirs.insert(dir.clone()) {
            continue;
        }
        for package_dir in package_dirs_in(&dir)? {
            let Some(real) = installed_package(&package_dir)? else { continue };
            // Always traverse a workspace member's own dependency links.
            queue.push(real.join("node_modules"));
            let authored = real.strip_prefix(&source_boundary).is_ok_and(|relative| {
                !relative.components().any(|part| part.as_os_str() == "node_modules")
            });
            if !workspace_roots.contains(&real) && !authored {
                if let Some(modules_dir) = nearest_modules_dir(&real) {
                    queue.push(modules_dir);
                }
                packages.insert(real);
            }
        }
    }
    Ok(packages.into_iter().collect())
}

fn installed_package(path: &Path) -> Result<Option<PathBuf>> {
    let real = path.canonicalize().with_context(|| {
        format!("resolving installed package {} (check for a broken link)", path.display())
    })?;
    if !real.is_dir() {
        return Ok(None);
    }
    match std::fs::metadata(real.join("package.json")) {
        Ok(metadata) if metadata.is_file() => Ok(Some(real)),
        Ok(_) => Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => {
            Err(error).with_context(|| format!("reading package metadata in {}", real.display()))
        }
    }
}

fn package_dirs_in(modules_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut packages = Vec::new();
    for entry in std::fs::read_dir(modules_dir)
        .with_context(|| format!("reading {}", modules_dir.display()))?
    {
        let entry = entry.with_context(|| format!("listing {}", modules_dir.display()))?;
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        if name.to_string_lossy().starts_with('@') {
            if !entry.path().is_dir() {
                continue;
            }
            for scoped in std::fs::read_dir(entry.path())
                .with_context(|| format!("reading scope {}", entry.path().display()))?
            {
                let scoped =
                    scoped.with_context(|| format!("listing scope {}", entry.path().display()))?;
                if !scoped.file_name().to_string_lossy().starts_with('.') {
                    packages.push(scoped.path());
                }
            }
        } else {
            packages.push(entry.path());
        }
    }
    Ok(packages)
}

fn nearest_modules_dir(package_dir: &Path) -> Option<PathBuf> {
    package_dir
        .ancestors()
        .find(|ancestor| ancestor.file_name().is_some_and(|name| name == "node_modules"))
        .map(Path::to_path_buf)
}
