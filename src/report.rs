use crate::scan::Origin;
use anyhow::Result;
use serde::Serialize;
use std::fmt::Write as _;

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

    let mut out = String::from("packageExtensions:\n");
    for report in reports {
        let (name, _) = report.package.rsplit_once('@').unwrap_or((&report.package, ""));
        let _ = writeln!(out, "  '{name}':");
        let _ = writeln!(out, "    peerDependencies:");
        for finding in &report.findings {
            let _ = writeln!(out, "      '{}': '*'", finding.dependency);
        }
        let _ = writeln!(out, "    peerDependenciesMeta:");
        for finding in &report.findings {
            let _ = writeln!(out, "      '{}':", finding.dependency);
            let _ = writeln!(out, "        optional: true");
        }
    }
    out
}

#[cfg(test)]
mod tests;
