#[cfg(test)]
mod tests;

use crate::{
    classify::missing_package,
    manifest::Manifest,
    report::{Finding, PackageReport, Severity},
    scan,
};
use anyhow::Result;
use std::{
    collections::{BTreeSet, HashSet},
    path::{Path, PathBuf},
};
use walkdir::WalkDir;

/// What one installed package reaches for but never declared, or `None` when it
/// declared everything its declaration files name.
pub fn analyze(package_dir: &Path) -> Result<Option<PackageReport>> {
    let manifest = Manifest::read(package_dir)?;
    let declared: HashSet<&str> = manifest.reachable().collect();

    let mut referenced = BTreeSet::new();
    let mut unparsed = Vec::new();
    for file in declaration_files(package_dir) {
        let Ok(source) = std::fs::read_to_string(&file) else {
            continue;
        };
        match scan::specifiers(&source) {
            Ok(found) => referenced.extend(found),
            Err(reason) => unparsed.push((file, reason)),
        }
    }
    for (file, reason) in &unparsed {
        eprintln!("warning: could not parse {}: {reason}", file.display());
    }

    let findings: Vec<Finding> = referenced
        .iter()
        .filter_map(|requirement| missing_package(requirement, &declared))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|name| Finding {
            declared_range: manifest.dev_dependencies.get(&name).cloned(),
            severity: if manifest.dev_dependencies.contains_key(&name) {
                Severity::DevDependency
            } else {
                Severity::Undeclared
            },
            dependency: name,
        })
        .collect();

    if findings.is_empty() {
        return Ok(None);
    }
    Ok(Some(PackageReport { package: manifest.id(), findings }))
}

/// The declaration files a package ships. Anything under a nested
/// `node_modules` belongs to a bundled dependency, not to the package itself.
pub fn declaration_files(package_dir: &Path) -> impl Iterator<Item = PathBuf> {
    WalkDir::new(package_dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| entry.file_name() != "node_modules")
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(walkdir::DirEntry::into_path)
        .filter(|path| {
            let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
            name.ends_with(".d.ts") || name.ends_with(".d.mts") || name.ends_with(".d.cts")
        })
}
