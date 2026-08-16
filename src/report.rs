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
        return "No undeclared dependencies found in shipped declaration files.\n".to_string();
    }

    let mut out = String::new();
    for report in reports {
        let _ = writeln!(out, "{}", report.package);
        for finding in &report.findings {
            let note = match finding.severity {
                Severity::DevDependency => "declared as a devDependency",
                Severity::Undeclared => "not in the manifest at all",
            };
            let _ = writeln!(out, "  {} — {note}", finding.dependency);
        }
        out.push('\n');
    }
    let total: usize = reports.iter().map(|r| r.findings.len()).sum();
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
mod tests {
    use super::*;
    use yaml_rust2::YamlLoader;

    fn medplum() -> Vec<PackageReport> {
        vec![PackageReport {
            package: "@medplum/core@5.1.15".to_string(),
            findings: vec![Finding {
                dependency: "@medplum/fhirtypes".to_string(),
                declared_range: Some("5.1.15".to_string()),
                severity: Severity::DevDependency,
            }],
        }]
    }

    #[test]
    fn package_extensions_declare_an_optional_peer() {
        assert_eq!(
            as_package_extensions(&medplum()),
            "packageExtensions:\n  \
             '@medplum/core':\n    \
             peerDependencies:\n      \
             '@medplum/fhirtypes': '*'\n    \
             peerDependenciesMeta:\n      \
             '@medplum/fhirtypes':\n        \
             optional: true\n"
        );
    }

    /// The block is pasted into pnpm-workspace.yaml, so a quoting slip would
    /// hand the user a file their package manager cannot read.
    #[test]
    fn package_extensions_parse_as_yaml() {
        let rendered = as_package_extensions(&medplum());
        let parsed = YamlLoader::load_from_str(&rendered).expect("emitted invalid YAML");
        let extensions = &parsed[0]["packageExtensions"]["@medplum/core"];
        assert_eq!(extensions["peerDependencies"]["@medplum/fhirtypes"].as_str(), Some("*"));
        assert_eq!(
            extensions["peerDependenciesMeta"]["@medplum/fhirtypes"]["optional"].as_bool(),
            Some(true)
        );
    }

    #[test]
    fn the_version_is_stripped_from_a_scoped_package() {
        let rendered = as_package_extensions(&medplum());
        assert!(rendered.contains("'@medplum/core':"), "{rendered}");
        assert!(!rendered.contains("5.1.15'"), "{rendered}");
    }

    #[test]
    fn nothing_found_produces_no_block_to_paste() {
        assert_eq!(as_package_extensions(&[]), "");
        assert!(as_text(&[]).starts_with("No undeclared dependencies"));
    }

    #[test]
    fn text_separates_the_two_kinds_of_finding() {
        let reports = vec![PackageReport {
            package: "pkg@1.0.0".to_string(),
            findings: vec![
                Finding {
                    dependency: "known".to_string(),
                    declared_range: Some("^1".to_string()),
                    severity: Severity::DevDependency,
                },
                Finding {
                    dependency: "unknown".to_string(),
                    declared_range: None,
                    severity: Severity::Undeclared,
                },
            ],
        }];
        let rendered = as_text(&reports);
        assert!(rendered.contains("known — declared as a devDependency"), "{rendered}");
        assert!(rendered.contains("unknown — not in the manifest at all"), "{rendered}");
    }
}
