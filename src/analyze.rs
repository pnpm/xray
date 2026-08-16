#[cfg(test)]
mod tests;

use crate::{
    classify::missing_package,
    manifest::Manifest,
    reachable::reachable_files,
    report::{Finding, PackageReport, Severity},
    scan::{self, Origin},
};
use anyhow::Result;
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    path::{Path, PathBuf},
};
use walkdir::WalkDir;

/// Which of a package's files to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Follow the package's own encapsulation: walk from the entry points when
    /// an `exports` map says which files are reachable, and read everything
    /// otherwise.
    Declared,
    /// Everything in the tarball, including tests, templates and examples whose
    /// dependencies belong to whoever runs them.
    EveryFile,
}

/// What one installed package reaches for but never declared, or `None` when it
/// declared everything its shipped files name.
pub fn analyze(package_dir: &Path, scope: Scope) -> Result<Option<PackageReport>> {
    let manifest = Manifest::read(package_dir)?;
    let declared: HashSet<&str> = manifest.reachable().collect();

    // An `exports` map is a package saying which files a consumer may reach;
    // without one, `require("pkg/anything")` resolves and every shipped file is
    // part of the surface — including the tests, whether the author meant that
    // or not.
    let encapsulated = !manifest.exports.is_null();
    let files: Vec<PathBuf> = match scope {
        Scope::Declared if encapsulated => {
            reachable_files(package_dir, &manifest).into_iter().collect()
        }
        _ => scannable_files(package_dir).collect(),
    };

    let mut referenced: BTreeSet<(scan::Requirement, Origin)> = BTreeSet::new();
    let mut unparsed = Vec::new();
    for file in files {
        let Ok(source) = std::fs::read_to_string(&file) else {
            continue;
        };
        match scan::specifiers(&source, &file) {
            Ok(found) => referenced.extend(found),
            Err(reason) => unparsed.push((file, reason)),
        }
    }
    for (file, reason) in &unparsed {
        eprintln!("warning: could not parse {}: {reason}", file.display());
    }

    // Classified per reference rather than per name: the same package can be
    // satisfied in a type position by `@types/` and still be missing at run
    // time, and it is the run-time reference that has to win.
    let mut missing: BTreeMap<String, Origin> = BTreeMap::new();
    for (requirement, origin) in &referenced {
        if let Some(name) = missing_package(requirement, *origin, &declared) {
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
        .filter(|path| scan::is_scannable(path))
}
