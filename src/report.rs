use crate::scan::Origin;
use anyhow::Result;
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageReport {
    pub package: String,
    pub findings: Vec<Finding>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub dependency: String,
    /// The range the package pinned in its own devDependencies, when it has one.
    /// It is the closest thing to an author-sanctioned version for the peer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub declared_range: Option<String>,
    pub severity: Severity,
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

pub fn as_json(reports: &[PackageReport]) -> Result<String> {
    Ok(format!("{}\n", serde_json::to_string_pretty(reports)?))
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
        }
        out.push('\n');
    }
    let total: usize = reports.iter().map(|report| report.findings.len()).sum();
    let _ = writeln!(out, "{total} undeclared dependencies across {} packages.", reports.len());
    out
}

pub fn as_package_extensions(reports: &[PackageReport]) -> String {
    if reports.is_empty() {
        return String::new();
    }

    // The same version can occur more than once with different peer contexts.
    // Merge those reports, and keep versions separate so a fix for one release
    // does not silently change every other release of the package.
    let mut extensions: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for report in reports {
        extensions
            .entry(&report.package)
            .or_default()
            .extend(report.findings.iter().map(|finding| finding.dependency.as_str()));
    }
    let mut out = String::from("packageExtensions:\n");
    for (package, dependencies) in extensions {
        let package = package.replace('\'', "''");
        let _ = writeln!(out, "  '{package}':");
        let _ = writeln!(out, "    peerDependencies:");
        for dependency in &dependencies {
            let dependency = dependency.replace('\'', "''");
            let _ = writeln!(out, "      '{dependency}': '*'");
        }
        let _ = writeln!(out, "    peerDependenciesMeta:");
        for dependency in &dependencies {
            let dependency = dependency.replace('\'', "''");
            let _ = writeln!(out, "      '{dependency}':");
            let _ = writeln!(out, "        optional: true");
        }
    }
    out
}

#[cfg(test)]
mod tests;
