#[cfg(test)]
mod tests;

use crate::{
    classify::{missing_package, types_package},
    manifest::Manifest,
    reachable::{reachable_files, reachable_files_for_repair},
    report::{Diagnostic, DiagnosticKind, Evidence, Finding, PackageReport, Severity},
    scan::{self, Origin},
};
use anyhow::{Context, Result};
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
#[cfg(test)]
pub fn analyze(package_dir: &Path, scope: Scope) -> Result<Option<PackageReport>> {
    let outcome = analyze_with_diagnostics(package_dir, scope)?;
    for diagnostic in &outcome.diagnostics {
        if diagnostic.kind == DiagnosticKind::ReadError {
            anyhow::bail!(
                "reading source {}: {}",
                package_dir.join(&diagnostic.file).display(),
                diagnostic.message
            );
        }
        eprintln!(
            "warning: could not parse {}: {}",
            package_dir.join(&diagnostic.file).display(),
            diagnostic.message
        );
    }
    Ok(outcome.report)
}

pub struct AnalysisOutcome {
    pub report: Option<PackageReport>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Analyze without printing diagnostics, including failures in otherwise clean packages.
pub fn analyze_with_diagnostics(package_dir: &Path, scope: Scope) -> Result<AnalysisOutcome> {
    analyze_inner(package_dir, scope, false)
}

/// Analyze a package for an automatic repair, rejecting incomplete scans.
pub fn analyze_for_repair(package_dir: &Path, scope: Scope) -> Result<Option<PackageReport>> {
    Ok(analyze_inner(package_dir, scope, true)?.report)
}

fn analyze_inner(package_dir: &Path, scope: Scope, strict: bool) -> Result<AnalysisOutcome> {
    let manifest = Manifest::read(package_dir)?;
    let declared: HashSet<&str> = manifest.reachable().collect();

    // An `exports` map is a package saying which files a consumer may reach;
    // without one, `require("pkg/anything")` resolves and every shipped file is
    // part of the surface — including the tests, whether the author meant that
    // or not.
    let encapsulated = manifest.is_encapsulated();
    let files: Vec<PathBuf> = match scope {
        Scope::Declared if strict => {
            reachable_files_for_repair(package_dir, &manifest)?.into_iter().collect()
        }
        Scope::Declared if encapsulated => {
            reachable_files(package_dir, &manifest).into_iter().collect()
        }
        _ => scannable_files(package_dir).collect(),
    };

    let mut missing: BTreeMap<String, (Origin, BTreeSet<Evidence>)> = BTreeMap::new();
    let canonical_root = package_dir.canonicalize()?;
    let mut diagnostics = Vec::new();
    for file in files {
        let canonical_file = file.canonicalize()?;
        let relative = canonical_file.strip_prefix(&canonical_root).unwrap_or(&canonical_file);
        let filename = relative.to_string_lossy().replace('\\', "/");
        let source = match std::fs::read_to_string(&file) {
            Ok(source) => source,
            Err(error) if strict => {
                return Err(error).with_context(|| format!("reading source {}", file.display()))
            }
            Err(error) => {
                diagnostics.push(Diagnostic {
                    package: manifest.id(),
                    file: filename,
                    message: error.to_string(),
                    kind: DiagnosticKind::ReadError,
                });
                continue;
            }
        };
        match scan::references(&source, &file) {
            Ok(found) => {
                let positions =
                    reference_positions(&source, found.iter().map(|(_, _, offset)| *offset));
                for (requirement, origin, offset) in found {
                    let specifier = match &requirement {
                        scan::Requirement::Module(specifier)
                        | scan::Requirement::TypesReference(specifier)
                        | scan::Requirement::PathReference(specifier) => specifier,
                    };
                    let (line, column) = positions[&offset];
                    let evidence = Evidence {
                        file: filename.clone(),
                        line,
                        column,
                        specifier: specifier.clone(),
                        origin,
                    };
                    let targets = if specifier.starts_with('#') {
                        manifest
                            .import_targets(specifier)
                            .into_iter()
                            .map(scan::Requirement::Module)
                            .collect::<Vec<_>>()
                    } else {
                        vec![requirement]
                    };
                    for target in &targets {
                        if let Some(mut name) = missing_package(target, origin, &declared) {
                            if origin == Origin::Types
                                && !manifest.dev_dependencies.contains_key(&name)
                            {
                                let type_name = types_package(&name);
                                if manifest.dev_dependencies.contains_key(&type_name) {
                                    name = type_name;
                                }
                            }
                            let (seen_origin, locations) =
                                missing.entry(name).or_insert_with(|| (origin, BTreeSet::new()));
                            *seen_origin = seen_origin.merged(origin);
                            locations.insert(evidence.clone());
                        }
                    }
                }
            }
            Err(reason) if strict => anyhow::bail!("could not parse {}: {reason}", file.display()),
            Err(reason) => diagnostics.push(Diagnostic {
                package: manifest.id(),
                file: filename,
                message: reason,
                kind: DiagnosticKind::ParseError,
            }),
        }
    }

    let report = build_report(&manifest, missing);
    Ok(AnalysisOutcome { report, diagnostics })
}

fn build_report(
    manifest: &Manifest,
    missing: BTreeMap<String, (Origin, BTreeSet<Evidence>)>,
) -> Option<PackageReport> {
    let findings: Vec<Finding> = missing
        .into_iter()
        .map(|(name, (origin, evidence))| Finding {
            declared_range: manifest.dev_dependencies.get(&name).cloned(),
            severity: if manifest.dev_dependencies.contains_key(&name) {
                Severity::DevDependency
            } else {
                Severity::Undeclared
            },
            dependency: name,
            origin,
            evidence,
        })
        .collect();

    (!findings.is_empty()).then(|| PackageReport { package: manifest.id(), findings })
}

/// Walk the source once, including for long minified lines with many references.
fn reference_positions(
    source: &str,
    offsets: impl Iterator<Item = u32>,
) -> BTreeMap<u32, (usize, usize)> {
    let offsets: BTreeSet<_> = offsets.collect();
    let mut chars = source.chars();
    let (mut byte, mut line, mut column) = (0, 1, 1);
    let mut positions = BTreeMap::new();
    for offset in offsets {
        while byte < offset as usize {
            let Some(character) = chars.next() else {
                break;
            };
            byte += character.len_utf8();
            if character == '\n' {
                line += 1;
                column = 1;
            } else {
                column += 1;
            }
        }
        positions.insert(offset, (line, column));
    }
    positions
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
