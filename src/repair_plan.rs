//! Select extension entries supported by the installed dependency graph.

use crate::{
    classify::types_package,
    manifest::Manifest,
    report::{registry_range, PackageReport},
    scan::Origin,
};
use anyhow::Result;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

/// Limit automatic repairs to author-provided ranges and installed providers.
/// A `@types` mapping uses an exact version only when all installed copies agree.
pub fn select(
    reports: &[PackageReport],
    packages: &[PathBuf],
    project: &Path,
) -> Result<Vec<PackageReport>> {
    let direct = direct_types_providers(project)?;
    let mut providers: BTreeMap<String, Provider> = BTreeMap::new();
    for path in packages {
        let manifest = Manifest::read(path)?;
        if !manifest.name.is_empty() && !manifest.version.is_empty() {
            let typed = has_types(&manifest, path);
            let provider = providers.entry(manifest.name).or_default();
            provider.versions.insert(manifest.version);
            provider.typed |= typed;
        }
    }

    let mut selected = Vec::new();
    for report in reports {
        let mut findings = Vec::new();
        for finding in &report.findings {
            let mut finding = finding.clone();
            if finding.origin == Origin::Types
                && !providers.contains_key(&finding.dependency)
                && finding.declared_range.as_deref().is_some_and(registry_range)
            {
                findings.push(finding);
                continue;
            }
            let provider = providers.get(&finding.dependency);
            if finding.origin == Origin::Types
                && !finding.dependency.starts_with("@types/")
                && provider.is_none_or(|provider| !provider.typed)
            {
                let name = types_package(&finding.dependency);
                if let Some(types) = providers.get(&name) {
                    let direct_versions = direct.get(&name);
                    let versions = match direct_versions {
                        Some(versions) => versions,
                        None => &types.versions,
                    };
                    if versions.len() == 1 {
                        let version = versions.first().expect("one version exists");
                        if types.versions.contains(version) && exact_version(version) {
                            finding.dependency = name;
                            finding.declared_range = Some(version.clone());
                            findings.push(finding);
                            continue;
                        }
                    }
                    // An ambiguous version is not safe to infer from.
                    continue;
                }
            }
            if provider.is_some() {
                // For a runtime or unknown dependency, preserve optional peer
                // behavior; its dev range is not necessarily a consumer range.
                finding.declared_range = None;
                findings.push(finding);
            }
        }
        if !findings.is_empty() {
            selected.push(PackageReport { package: report.package.clone(), findings });
        }
    }
    Ok(selected)
}

// Prefer a provider explicitly declared by a workspace importer. If importers
// declare different versions there is no single safe answer for a global fix.
fn direct_types_providers(root: &Path) -> Result<BTreeMap<String, BTreeSet<String>>> {
    let importers = crate::workspace::projects(root)?;
    let mut direct = BTreeMap::<String, BTreeSet<String>>::new();
    for importer in importers {
        if !importer.join("package.json").is_file() {
            continue;
        }
        let manifest = Manifest::read(&importer)?;
        for name in manifest
            .dependencies
            .keys()
            .chain(manifest.dev_dependencies.keys())
            .chain(manifest.optional_dependencies.keys())
            .chain(manifest.peer_dependencies.keys())
            .filter(|name| name.starts_with("@types/"))
        {
            let provider_dir = importer.join("node_modules").join(name);
            if provider_dir.join("package.json").is_file() {
                let provider = Manifest::read(&provider_dir)?;
                if provider.name == *name {
                    direct.entry(name.clone()).or_default().insert(provider.version);
                }
            }
        }
    }
    Ok(direct)
}

#[derive(Default)]
struct Provider {
    versions: BTreeSet<String>,
    // If any installed variant advertises types, do not infer that the package
    // is untyped: different releases can have different declaration surfaces.
    typed: bool,
}

fn has_types(manifest: &Manifest, path: &Path) -> bool {
    fn has_declaration(value: &serde_json::Value) -> bool {
        match value {
            serde_json::Value::String(target) => {
                target.ends_with(".d.ts")
                    || target.ends_with(".d.mts")
                    || target.ends_with(".d.cts")
            }
            serde_json::Value::Array(values) => values.iter().any(has_declaration),
            serde_json::Value::Object(values) => {
                values.iter().any(|(key, value)| key == "types" || has_declaration(value))
            }
            _ => false,
        }
    }
    manifest.types.is_string()
        || manifest.typings.is_string()
        || path.join("index.d.ts").is_file()
        || has_declaration(&manifest.exports)
}

// Do not turn a malformed or non-concrete manifest version into an install range.
fn exact_version(version: &str) -> bool {
    node_semver::Version::parse(version).is_ok()
}

#[cfg(test)]
mod tests;
