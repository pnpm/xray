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
    add_installed_aliases(project, packages, &mut providers)?;

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

/// An npm alias is available under its importer key, not its manifest name.
/// Only accept an alias when the link resolves to a discovered installed
/// package whose actual name and version agree with the importer's request.
fn add_installed_aliases(
    root: &Path,
    packages: &[PathBuf],
    providers: &mut BTreeMap<String, Provider>,
) -> Result<()> {
    let installed: BTreeSet<PathBuf> =
        packages.iter().map(|package| package.canonicalize()).collect::<std::io::Result<_>>()?;
    for importer in crate::workspace::projects(root)? {
        if !importer.join("package.json").is_file() {
            continue;
        }
        let manifest = Manifest::read(&importer)?;
        for (alias, request) in manifest
            .dependencies
            .iter()
            .chain(manifest.dev_dependencies.iter())
            .chain(manifest.optional_dependencies.iter())
            .chain(manifest.peer_dependencies.iter())
        {
            if !valid_package_name(alias) {
                continue;
            }
            let Some((name, range)) = npm_alias(request) else {
                continue;
            };
            let path = importer.join("node_modules").join(alias);
            let Ok(actual) = path.canonicalize() else {
                continue;
            };
            if !installed.contains(&actual) {
                continue;
            }
            let provider = Manifest::read(&actual)?;
            let Ok(version) = node_semver::Version::parse(&provider.version) else {
                continue;
            };
            // The emitted optional peer is `*`; avoid guessing whether a
            // prerelease provider will satisfy it across pnpm configurations.
            if !version.pre_release.is_empty()
                || provider.name != name
                || range.as_ref().is_some_and(|range| !range.satisfies(&version))
            {
                continue;
            }
            let typed = has_types(&provider, &actual);
            let entry = providers.entry(alias.clone()).or_default();
            entry.versions.insert(provider.version);
            entry.typed |= typed;
        }
    }
    Ok(())
}

fn npm_alias(request: &str) -> Option<(&str, Option<node_semver::Range>)> {
    let target = request.strip_prefix("npm:")?;
    let separator = target.rfind('@').filter(|index| *index > 0);
    let (name, range) = match separator {
        Some(index) => (&target[..index], Some(target[index + 1..].parse().ok()?)),
        None => (target, None),
    };
    valid_package_name(name).then_some((name, range))
}

fn valid_package_name(name: &str) -> bool {
    let valid_segment = |segment: &str| {
        !segment.is_empty()
            && segment != "."
            && segment != ".."
            && segment.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"-._~".contains(&byte))
    };
    if let Some(scoped) = name.strip_prefix('@') {
        scoped
            .split_once('/')
            .is_some_and(|(scope, package)| valid_segment(scope) && valid_segment(package))
    } else {
        valid_segment(name)
    }
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
        || [
            "index.d.ts",
            "index.d.mts",
            "index.d.cts",
            "index.ts",
            "index.mts",
            "index.cts",
            "index.tsx",
        ]
        .iter()
        .any(|entry| path.join(entry).is_file())
        || manifest.types_versions.as_object().is_some_and(|versions| !versions.is_empty())
        || has_declaration(&manifest.exports)
        || manifest.entry_paths().iter().any(|entry| {
            let target = path.join(&entry.path);
            if matches!(
                target.extension().and_then(|extension| extension.to_str()),
                Some("ts" | "tsx" | "mts" | "cts"),
            ) {
                return true;
            }
            [(".js", ".d.ts"), (".mjs", ".d.mts"), (".cjs", ".d.cts")].iter().any(
                |(extension, declaration)| {
                    entry
                        .path
                        .strip_suffix(extension)
                        .is_some_and(|stem| path.join(format!("{stem}{declaration}")).is_file())
                },
            )
        })
}

// Do not turn a malformed or non-concrete manifest version into an install range.
fn exact_version(version: &str) -> bool {
    node_semver::Version::parse(version).is_ok()
}

#[cfg(test)]
mod tests;
