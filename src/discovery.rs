#[cfg(test)]
mod tests;

use anyhow::{bail, Context, Result};
use std::{
    collections::{BTreeSet, HashSet},
    path::{Path, PathBuf},
};
use yaml_rust2::YamlLoader;

/// Every package reachable from the project, found by walking `node_modules`
/// directories through their real paths. A package's own dependencies sit in the
/// same directory it does, so following each one to where it really lives and
/// reading that directory again covers the whole graph — under a project-local
/// virtual store and a global one alike.
pub fn installed_packages(root: &Path) -> Result<Vec<PathBuf>> {
    let root_modules = root.join("node_modules");
    if !root_modules.is_dir() {
        bail!(
            "{} has no installed dependencies — run an install first, or pass --package",
            root.display(),
        );
    }

    let mut queue = vec![root_modules.clone(), root_modules.join(".pnpm/node_modules")];
    // With hoisting disabled, workspace-only dependencies need not be reachable
    // from the root package. The installed lockfile lists every importer.
    let lockfile = root.join("pnpm-lock.yaml");
    if lockfile.is_file() {
        let raw = std::fs::read_to_string(&lockfile)
            .with_context(|| format!("reading {}", lockfile.display()))?;
        let documents = YamlLoader::load_from_str(&raw)
            .with_context(|| format!("parsing {}", lockfile.display()))?;
        let canonical_root = root.canonicalize()?;
        if let Some(importers) = documents.first().and_then(|doc| doc["importers"].as_hash()) {
            for importer in importers.keys().filter_map(yaml_rust2::Yaml::as_str) {
                let Ok(project) = root.join(importer).canonicalize() else { continue };
                if project.starts_with(&canonical_root) {
                    queue.push(project.join("node_modules"));
                }
            }
        }
    }
    let mut visited_dirs = HashSet::new();
    let mut packages = BTreeSet::new();

    while let Some(dir) = queue.pop() {
        let Ok(dir) = dir.canonicalize() else { continue };
        if !dir.is_dir() || !visited_dirs.insert(dir.clone()) {
            continue;
        }
        for package_dir in package_dirs_in(&dir) {
            let Ok(real) = package_dir.canonicalize() else { continue };
            if !real.join("package.json").is_file() {
                continue;
            }
            // npm and other layouts can put dependencies inside the package
            // itself rather than next to it in a virtual-store entry.
            queue.push(real.join("node_modules"));
            if let Some(modules_dir) = nearest_modules_dir(&real) {
                queue.push(modules_dir);
            }
            packages.insert(real);
        }
    }
    Ok(packages.into_iter().collect())
}

fn package_dirs_in(modules_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(modules_dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name() != ".pnpm" && entry.file_name() != ".bin")
        .flat_map(|entry| {
            if entry.file_name().to_string_lossy().starts_with('@') {
                std::fs::read_dir(entry.path())
                    .map(|scoped| scoped.filter_map(Result::ok).map(|entry| entry.path()).collect())
                    .unwrap_or_default()
            } else {
                vec![entry.path()]
            }
        })
        .collect()
}

fn nearest_modules_dir(package_dir: &Path) -> Option<PathBuf> {
    package_dir
        .ancestors()
        .find(|ancestor| ancestor.file_name().is_some_and(|name| name == "node_modules"))
        .map(Path::to_path_buf)
}
