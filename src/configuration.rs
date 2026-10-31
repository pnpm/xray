//! Account for dependency declarations supplied by workspace package extensions.
use crate::{classify::types_package, report::PackageReport, scan::Origin};
use anyhow::{bail, Context, Result};
use node_semver::{Range, Version};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};
use yaml_rust2::{yaml::Hash, Yaml, YamlLoader};

/// Resolve the settings destination when invoked from a workspace member.
pub fn workspace_root(project: &Path) -> Result<PathBuf> {
    let project =
        project.canonicalize().with_context(|| format!("resolving {}", project.display()))?;
    for parent in project.ancestors() {
        if parent.join("pnpm-workspace.yaml").is_file() {
            return Ok(parent.to_path_buf());
        }
    }
    Ok(project)
}

pub fn apply(project: &Path, reports: &mut Vec<PackageReport>) -> Result<()> {
    let path = project.join("pnpm-workspace.yaml");
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    apply_text(&raw, reports).with_context(|| format!("reading extensions in {}", path.display()))
}

fn apply_text(raw: &str, reports: &mut Vec<PackageReport>) -> Result<()> {
    let documents = YamlLoader::load_from_str(raw)?;
    if documents.len() > 1 {
        bail!("workspace settings must contain one document");
    }
    let Some(document) = documents.first() else { return Ok(()) };
    let extensions = &document["packageExtensions"];
    if matches!(extensions, Yaml::Null | Yaml::BadValue) {
        return Ok(());
    }
    let extensions = extensions.as_hash().context("packageExtensions must be a mapping")?;
    validate_extensions(extensions)?;
    for report in reports.iter_mut() {
        let Some((name, version)) = report.package.rsplit_once('@') else { continue };
        let Ok(version) = version.parse::<Version>() else { continue };
        let mut declared = BTreeSet::new();
        for (selector, extension) in extensions {
            let Some(selector) = selector.as_str() else { continue };
            if !matches_selector(selector, name, &version) {
                continue;
            }
            for field in ["dependencies", "optionalDependencies", "peerDependencies"] {
                if let Some(dependencies) = extension[field].as_hash() {
                    declared.extend(dependencies.keys().filter_map(Yaml::as_str));
                }
            }
        }
        report.findings.retain(|finding| {
            !(declared.contains(finding.dependency.as_str())
                || finding.origin == Origin::Types
                    && declared.contains(types_package(&finding.dependency).as_str()))
        });
    }
    reports.retain(|report| !report.findings.is_empty());
    Ok(())
}

fn validate_extensions(extensions: &Hash) -> Result<()> {
    for (selector, extension) in extensions {
        let selector = selector.as_str().context("package extension selectors must be strings")?;
        if let Some((name, range)) = selector.rsplit_once('@').filter(|(name, _)| !name.is_empty())
        {
            range
                .parse::<Range>()
                .with_context(|| format!("invalid extension selector {name}@{range}"))?;
        }
        let settings = extension
            .as_hash()
            .with_context(|| format!("extension {selector} must be a mapping"))?;
        for field in ["dependencies", "optionalDependencies", "peerDependencies"] {
            if let Some(value) = settings.get(&Yaml::String(field.into())) {
                let values = value
                    .as_hash()
                    .with_context(|| format!("{selector}.{field} must be a mapping"))?;
                for (dependency, range) in values {
                    if dependency.as_str().is_none() || range.as_str().is_none() {
                        bail!("{selector}.{field} must map dependency names to string ranges");
                    }
                }
            }
        }
    }
    Ok(())
}

fn matches_selector(selector: &str, name: &str, version: &Version) -> bool {
    if selector == name {
        return true;
    }
    let Some((package, range)) = selector.rsplit_once('@') else { return false };
    package == name && range.parse::<Range>().is_ok_and(|range| version.satisfies(&range))
}

#[cfg(test)]
mod tests;
