#[cfg(test)]
mod tests;

use crate::{
    classify::missing_package,
    manifest::Manifest,
    report::{Finding, Origin, PackageReport, Severity},
    scan,
};
use anyhow::Result;
use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
};
use walkdir::WalkDir;

/// Extensions worth reading: what the package ships as behaviour, and what it
/// ships as types.
const SCANNED_EXTENSIONS: &[&str] = &[".js", ".mjs", ".cjs", ".jsx", ".ts", ".mts", ".cts", ".tsx"];

/// What one installed package reaches for but never declared, or `None` when it
/// declared everything its shipped files name.
pub fn analyze(package_dir: &Path) -> Result<Option<PackageReport>> {
    let manifest = Manifest::read(package_dir)?;
    let declared: HashSet<&str> = manifest.reachable().collect();

    let mut referenced: BTreeMap<scan::Requirement, Origin> = BTreeMap::new();
    let mut unparsed = Vec::new();
    for file in scannable_files(package_dir) {
        let Ok(source) = std::fs::read_to_string(&file) else {
            continue;
        };
        let origin = if scan::is_declaration(&file) { Origin::Types } else { Origin::Runtime };
        match scan::specifiers(&source, &file) {
            Ok(found) => {
                for requirement in found {
                    referenced
                        .entry(requirement)
                        .and_modify(|seen| *seen = seen.merged(origin))
                        .or_insert(origin);
                }
            }
            Err(reason) => unparsed.push((file, reason)),
        }
    }
    for (file, reason) in &unparsed {
        eprintln!("warning: could not parse {}: {reason}", file.display());
    }

    let mut missing: BTreeMap<String, Origin> = BTreeMap::new();
    for (requirement, origin) in &referenced {
        if let Some(name) = missing_package(requirement, &declared) {
            missing.entry(name).and_modify(|seen| *seen = seen.merged(*origin)).or_insert(*origin);
        }
    }

    let findings: Vec<Finding> = missing
        .into_iter()
        .map(|(name, origin)| Finding {
            declared_range: manifest.dev_dependencies.get(&name).cloned(),
            severity: if manifest.dev_dependencies.contains_key(&name) {
                Severity::DevDependency
            } else {
                Severity::Undeclared
            },
            dependency: name,
            origin,
        })
        .collect();

    if findings.is_empty() {
        return Ok(None);
    }
    Ok(Some(PackageReport { package: manifest.id(), findings }))
}

/// The files a package ships. Anything under a nested `node_modules` belongs to
/// a bundled dependency, not to the package itself.
pub fn scannable_files(package_dir: &Path) -> impl Iterator<Item = PathBuf> {
    WalkDir::new(package_dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| entry.file_name() != "node_modules")
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(walkdir::DirEntry::into_path)
        .filter(|path| {
            let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
            SCANNED_EXTENSIONS.iter().any(|extension| name.ends_with(extension))
        })
}
