use crate::scan::Origin;
use anyhow::Result;
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageReport {
    pub package: String,
    pub findings: Vec<Finding>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub package: String,
    pub file: String,
    pub message: String,
    pub kind: DiagnosticKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
#[expect(clippy::enum_variant_names, reason = "diagnostic names are part of the JSON schema")]
pub enum DiagnosticKind {
    ParseError,
    ReadError,
    AnalysisError,
    ResolutionError,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub dependency: String,
    /// The range the package pinned in its own devDependencies, when it has one.
    /// It is the closest thing to an author-sanctioned version for the peer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub declared_range: Option<String>,
    pub severity: Severity,
    pub origin: Origin,
    pub evidence: BTreeSet<Evidence>,
}

/// A source reference, using package-relative paths and one-based positions.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Evidence {
    pub file: String,
    #[serde(skip_serializing_if = "is_unknown_position")]
    pub line: usize,
    #[serde(skip_serializing_if = "is_unknown_position")]
    pub column: usize,
    pub specifier: String,
    pub origin: Origin,
}

/// A dependency the package build-depends on but ships references to is a far
/// stronger signal than one that appears nowhere in its manifest, which is more
/// often a bundler artifact or an optional integration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Severity {
    DevDependency,
    Undeclared,
}

/// Collapse repeated installations (for example different peer contexts) while
/// retaining every dependency and both runtime/type origins.
pub fn merge_reports(reports: Vec<PackageReport>) -> Vec<PackageReport> {
    let mut packages: BTreeMap<String, BTreeMap<String, Finding>> = BTreeMap::new();
    for report in reports {
        let dependencies = packages.entry(report.package).or_default();
        for finding in report.findings {
            if let Some(existing) = dependencies.get_mut(&finding.dependency) {
                existing.origin = existing.origin.merged(finding.origin);
                existing.evidence.extend(finding.evidence);
                if existing.declared_range != finding.declared_range {
                    existing.declared_range = None;
                }
            } else {
                dependencies.insert(finding.dependency.clone(), finding);
            }
        }
    }
    packages
        .into_iter()
        .map(|(package, findings)| PackageReport {
            package,
            findings: findings.into_values().collect(),
        })
        .collect()
}

pub fn as_json(
    reports: &[PackageReport],
    packages_scanned: usize,
    diagnostics: &[Diagnostic],
) -> Result<String> {
    let result = serde_json::json!({
        "schemaVersion": 1,
        "packagesScanned": packages_scanned,
        "packages": reports,
        "diagnostics": diagnostics,
        "complete": diagnostics.is_empty(),
    });
    Ok(format!("{}\n", serde_json::to_string_pretty(&result)?))
}

pub fn as_text(reports: &[PackageReport]) -> String {
    if reports.is_empty() {
        return "No undeclared dependencies found in shipped files.\n".to_string();
    }

    let mut out = String::new();
    for report in reports {
        let _ = writeln!(out, "{}", report.package);
        for finding in &report.findings {
            let note = match finding.severity {
                Severity::DevDependency => "declared as a devDependency",
                Severity::Undeclared => "not in the manifest at all",
            };
            let where_from = match finding.origin {
                Origin::Runtime => "code",
                Origin::Types => "types",
                Origin::Both => "code and types",
            };
            let _ = writeln!(out, "  {} — {note}, used in {where_from}", finding.dependency);
            for evidence in &finding.evidence {
                if evidence.line == 0 {
                    let _ = writeln!(out, "    {} ({})", evidence.file, evidence.specifier);
                    continue;
                }
                let _ = writeln!(
                    out,
                    "    {}:{}:{} ({})",
                    evidence.file, evidence.line, evidence.column, evidence.specifier,
                );
            }
        }
        out.push('\n');
    }
    let total: usize = reports.iter().map(|report| report.findings.len()).sum();
    let _ = writeln!(out, "{total} undeclared dependencies across {} packages.", reports.len());
    out
}

/// Whether a report's package id can become a selector for exactly one
/// release. A missing version would select every release (`name@`), and a
/// version that is not semver (`1.0`) would be read by pnpm as a range.
pub fn has_extension_selector(report: &PackageReport) -> bool {
    report.package.rsplit_once('@').is_some_and(|(name, version)| {
        !name.is_empty() && node_semver::Version::parse(version).is_ok()
    })
}

/// Packages left out of `as_package_extensions` because their installed
/// version cannot select a single release.
pub fn without_extension_selector(reports: &[PackageReport]) -> Vec<&str> {
    reports
        .iter()
        .filter(|report| !has_extension_selector(report))
        .map(|report| report.package.as_str())
        .collect()
}

pub fn as_package_extensions(reports: &[PackageReport]) -> String {
    // The same version can occur more than once with different peer contexts.
    // Merge those reports, and keep versions separate so a fix for one release
    // does not silently change every other release of the package.
    let mut extensions: BTreeMap<&str, BTreeMap<&str, Option<&str>>> = BTreeMap::new();
    for report in reports.iter().filter(|report| has_extension_selector(report)) {
        let dependencies = extensions.entry(&report.package).or_default();
        for finding in &report.findings {
            let range = finding
                .declared_range
                .as_deref()
                .filter(|range| finding.origin == Origin::Types && registry_range(range));
            dependencies
                .entry(&finding.dependency)
                .and_modify(|existing| {
                    if *existing != range {
                        *existing = None;
                    }
                })
                .or_insert(range);
        }
    }
    if extensions.is_empty() {
        return String::new();
    }
    let mut out = String::from("packageExtensions:\n");
    for (package, dependencies) in extensions {
        let package = package.replace('\'', "''");
        let _ = writeln!(out, "  '{package}':");
        if dependencies.values().any(Option::is_some) {
            let _ = writeln!(out, "    dependencies:");
            for (dependency, range) in &dependencies {
                if let Some(range) = range {
                    let dependency = dependency.replace('\'', "''");
                    let range = range.replace('\'', "''");
                    let _ = writeln!(out, "      '{dependency}': '{range}'");
                }
            }
        }
        if dependencies.values().any(Option::is_none) {
            let _ = writeln!(out, "    peerDependencies:");
            for (dependency, range) in &dependencies {
                if range.is_none() {
                    let dependency = dependency.replace('\'', "''");
                    let _ = writeln!(out, "      '{dependency}': '*'");
                }
            }
            let _ = writeln!(out, "    peerDependenciesMeta:");
            for (dependency, range) in &dependencies {
                if range.is_none() {
                    let dependency = dependency.replace('\'', "''");
                    let _ = writeln!(out, "      '{dependency}':");
                    let _ = writeln!(out, "        optional: true");
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests;

/// Only portable registry ranges can be copied from a publisher's build setup.
/// Local paths and workspace protocols refer to the publisher's checkout.
pub fn registry_range(range: &str) -> bool {
    !range.trim().is_empty() && range.parse::<node_semver::Range>().is_ok()
}

#[expect(clippy::trivially_copy_pass_by_ref, reason = "serde skip predicates receive a reference")]
const fn is_unknown_position(position: &usize) -> bool {
    *position == 0
}
