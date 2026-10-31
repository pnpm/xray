//! Discover installed type providers using the consumer's TypeScript resolver.
use crate::{
    manifest::Manifest,
    report::{self, Finding, PackageReport, Severity},
    scan::Origin,
    verify_candidates::Candidate,
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Serialize)]
struct Package<'a> {
    root: &'a Path,
    name: &'a str,
    version: &'a str,
    declared: Vec<&'a str>,
}

#[derive(Deserialize)]
struct Match {
    owner: PathBuf,
    provider: PathBuf,
    kind: String,
}

/// Return candidates only for compiler diagnostics resolved by an already
/// installed, unambiguous type package. This function does not alter disk.
pub fn collect(project: &Path, packages: &[PathBuf]) -> Result<Vec<Candidate>> {
    if !project.join("tsconfig.json").is_file() {
        return Ok(Vec::new());
    }
    let project = project.canonicalize()?;
    let mut manifests = BTreeMap::new();
    for package in packages {
        let path = package.canonicalize()?;
        if !path.components().any(|component| component.as_os_str() == "node_modules") {
            continue;
        }
        if let Ok(manifest) = Manifest::read(&path) {
            manifests.insert(path, manifest);
        }
    }
    let list: Vec<_> = manifests
        .iter()
        .map(|(root, manifest)| Package {
            root,
            name: &manifest.name,
            version: &manifest.version,
            declared: manifest.reachable().collect(),
        })
        .collect();
    let temp = tempfile::tempdir()?;
    let bridge = temp.path().join("bridge.cjs");
    let roots = temp.path().join("packages.json");
    std::fs::write(&bridge, include_str!("verify-types-bridge.cjs"))?;
    std::fs::write(temp.path().join("regenerate-paths.cjs"), include_str!("regenerate-paths.cjs"))?;
    std::fs::write(&roots, serde_json::to_vec(&list)?)?;
    let output = match Command::new("node")
        .arg(bridge)
        .arg(&project)
        .arg(roots)
        .current_dir(&project)
        .output()
    {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).context("starting TypeScript type-provider check"),
    };
    anyhow::ensure!(
        output.status.success(),
        "TypeScript type-provider check failed: {}",
        String::from_utf8_lossy(&output.stderr),
    );
    let matches: Vec<Match> = serde_json::from_slice(&output.stdout)
        .context("reading TypeScript type-provider results")?;
    let mut candidates = BTreeMap::new();
    for item in matches {
        let (Ok(owner), Ok(provider)) = (item.owner.canonicalize(), item.provider.canonicalize())
        else {
            continue;
        };
        let (Some(owner), Some(provider)) = (manifests.get(&owner), manifests.get(&provider))
        else {
            continue;
        };
        if !matches!(item.kind.as_str(), "types" | "module")
            || (item.kind == "types" && !provider.name.starts_with("@types/"))
            || owner.reachable().any(|name| name == provider.name)
            || node_semver::Version::parse(&provider.version).is_err()
            || node_semver::Version::parse(&owner.version).is_err()
        {
            continue;
        }
        let key = format!("extension:{}:{}", owner.id(), provider.name);
        let report = PackageReport {
            package: owner.id(),
            findings: vec![Finding {
                dependency: provider.name.clone(),
                declared_range: (item.kind == "types").then(|| provider.version.clone()),
                severity: Severity::Undeclared,
                origin: Origin::Types,
                evidence: BTreeSet::new(),
            }],
        };
        candidates.insert(
            key.clone(),
            Candidate {
                key,
                configuration: report::as_package_extensions(&[report]),
                artifacts: BTreeMap::new(),
            },
        );
    }
    Ok(candidates.into_values().collect())
}

#[cfg(test)]
#[path = "verify_types/tests.rs"]
mod tests;
