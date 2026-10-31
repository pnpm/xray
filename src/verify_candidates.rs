//! Immutable, individually testable repair proposals for the verified loop.
use crate::{
    analyze::{analyze_with_diagnostics, Scope},
    configuration, declaration_patch,
    manifest::Manifest,
    regenerate, repair, repair_plan,
    report::{self, PackageReport},
    scan::Origin,
    verify_types,
};
use anyhow::{bail, Result};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone)]
pub struct Candidate {
    pub key: String,
    pub configuration: String,
    pub artifacts: BTreeMap<PathBuf, String>,
}

/// Build candidates without modifying the workspace. A package version with
/// any incomplete peer context is excluded from extensions for that version.
pub fn collect(
    project: &Path,
    workspace_root: &Path,
    installed_packages: &[PathBuf],
    observed_reports: &[PackageReport],
    scope: Scope,
    regenerate_types: bool,
) -> Result<Vec<Candidate>> {
    let mut complete = BTreeSet::new();
    let mut incomplete = BTreeSet::new();
    let mut reports = Vec::new();
    for directory in installed_packages {
        let manifest = match Manifest::read(directory) {
            Ok(manifest) => manifest,
            Err(error) => {
                eprintln!("Skipping incomplete package {}: {error:#}", directory.display());
                continue;
            }
        };
        let package = manifest.id();
        match analyze_with_diagnostics(directory, scope) {
            Ok(outcome) if outcome.diagnostics.is_empty() => {
                complete.insert(package);
                if let Some(report) = outcome.report {
                    reports.push(report);
                }
            }
            Ok(outcome) => {
                eprintln!(
                    "Skipping incomplete package {}: {} diagnostics",
                    directory.display(),
                    outcome.diagnostics.len(),
                );
                incomplete.insert(package);
            }
            Err(error) => {
                eprintln!("Skipping incomplete package {}: {error:#}", directory.display());
                incomplete.insert(package);
            }
        }
    }
    let eligible = |package: &str| complete.contains(package) && !incomplete.contains(package);
    reports.retain(|report| eligible(&report.package));
    let mut observed = BTreeSet::new();
    for report in observed_reports.iter().filter(|report| eligible(&report.package)) {
        observed.extend(
            report
                .findings
                .iter()
                .map(|finding| (report.package.clone(), finding.dependency.clone())),
        );
        reports.push(report.clone());
    }
    let mut reports = report::merge_reports(reports);
    configuration::apply(workspace_root, &mut reports)?;
    let reports = repair_plan::select(&reports, installed_packages, workspace_root)?;
    let mut ranked = Vec::new();
    for report in reports {
        for finding in report.findings {
            let priority =
                if observed.contains(&(report.package.clone(), finding.dependency.clone())) {
                    0
                } else if finding.origin == Origin::Types {
                    2
                } else {
                    3
                };
            let key = format!("extension:{}:{}", report.package, finding.dependency);
            let single = PackageReport { package: report.package.clone(), findings: vec![finding] };
            let configuration = report::as_package_extensions(&[single]);
            if has_extensions(&configuration) {
                ranked
                    .push((priority, Candidate { key, configuration, artifacts: BTreeMap::new() }));
            }
        }
    }
    for candidate in verify_types::collect(project, installed_packages)? {
        if has_extensions(&candidate.configuration)
            && !incomplete
                .iter()
                .any(|package| candidate.key.starts_with(&format!("extension:{package}:")))
        {
            ranked.push((1, candidate));
        }
    }
    ranked.sort_by(|(left_priority, left), (right_priority, right)| {
        (left_priority, &left.key).cmp(&(right_priority, &right.key))
    });
    let mut candidates: Vec<_> = ranked.into_iter().map(|(_, candidate)| candidate).collect();
    if regenerate_types {
        let proposal = regenerate::propose(project, installed_packages)?;
        for message in proposal.messages {
            eprintln!("Declaration verification: {message}");
        }
        for patch in proposal.patches {
            let plan = declaration_patch::plan(std::slice::from_ref(&patch), installed_packages)?;
            if !plan.configuration.is_empty() {
                candidates.push(Candidate {
                    key: format!("patch:{}", patch.package),
                    configuration: plan.configuration,
                    artifacts: plan.artifacts,
                });
            }
        }
    }
    Ok(candidates)
}

/// A proposal that would write no package extension (for example for a
/// package whose version cannot be keyed) cannot change the result, so it
/// must not spend a verification attempt.
fn has_extensions(configuration: &str) -> bool {
    yaml_rust2::YamlLoader::load_from_str(configuration).is_ok_and(|documents| {
        documents
            .first()
            .and_then(|document| document["packageExtensions"].as_hash())
            .is_some_and(|extensions| !extensions.is_empty())
    })
}

/// Combine proposals using the same conflict checks as normal workspace edits.
pub fn compose(candidates: &[Candidate]) -> Result<declaration_patch::PatchPlan> {
    let mut configuration = String::new();
    let mut artifacts = BTreeMap::new();
    let mut keys: BTreeMap<&str, &Candidate> = BTreeMap::new();
    for candidate in candidates {
        if let Some(previous) = keys.insert(&candidate.key, candidate) {
            if previous.configuration != candidate.configuration
                || previous.artifacts != candidate.artifacts
            {
                bail!("conflicting verified candidates for {}", candidate.key);
            }
            continue;
        }
        configuration = repair::merge(&configuration, &candidate.configuration)?;
        for (path, content) in &candidate.artifacts {
            if let Some(existing) = artifacts.insert(path.clone(), content.clone()) {
                if existing != *content {
                    bail!("conflicting verified patch artifact {}", path.display());
                }
            }
        }
    }
    Ok(declaration_patch::PatchPlan { configuration, artifacts })
}

#[cfg(test)]
#[path = "verify_candidates/tests.rs"]
mod tests;
