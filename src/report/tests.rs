use crate::report::{as_package_extensions, as_text, Finding, Origin, PackageReport, Severity};
use yaml_rust2::YamlLoader;

fn medplum() -> Vec<PackageReport> {
    vec![PackageReport {
        package: "@medplum/core@5.1.15".to_string(),
        findings: vec![Finding {
            dependency: "@medplum/fhirtypes".to_string(),
            declared_range: Some("5.1.15".to_string()),
            severity: Severity::DevDependency,
            origin: Origin::Types,
        }],
    }]
}

#[test]
fn package_extensions_install_the_author_declared_dependency() {
    assert_eq!(
        as_package_extensions(&medplum()),
        "packageExtensions:\n  \
         '@medplum/core@5.1.15':\n    \
         dependencies:\n      \
         '@medplum/fhirtypes': '5.1.15'\n",
    );
}

/// The block is pasted into pnpm-workspace.yaml, so a quoting slip would
/// hand the user a file their package manager cannot read.
#[test]
fn package_extensions_parse_as_yaml() {
    let rendered = as_package_extensions(&medplum());
    let parsed = YamlLoader::load_from_str(&rendered).expect("emitted invalid YAML");
    let extensions = &parsed[0]["packageExtensions"]["@medplum/core@5.1.15"];
    assert_eq!(extensions["dependencies"]["@medplum/fhirtypes"].as_str(), Some("5.1.15"));
}

#[test]
fn versions_are_separate_and_repeated_peer_contexts_are_merged() {
    let mut reports = medplum();
    reports.push(PackageReport {
        package: "@medplum/core@5.1.15".into(),
        findings: vec![Finding {
            dependency: "another".into(),
            declared_range: None,
            severity: Severity::Undeclared,
            origin: Origin::Runtime,
        }],
    });
    reports.push(PackageReport {
        package: "@medplum/core@6.0.0".into(),
        findings: vec![Finding {
            dependency: "third".into(),
            declared_range: None,
            severity: Severity::Undeclared,
            origin: Origin::Runtime,
        }],
    });
    let rendered = as_package_extensions(&reports);
    let parsed = YamlLoader::load_from_str(&rendered).expect("valid YAML");
    let extensions = &parsed[0]["packageExtensions"];
    assert_eq!(
        extensions["@medplum/core@5.1.15"]["peerDependencies"]["another"].as_str(),
        Some("*"),
    );
    assert_eq!(
        extensions["@medplum/core@5.1.15"]["dependencies"]["@medplum/fhirtypes"].as_str(),
        Some("5.1.15"),
    );
    assert_eq!(extensions["@medplum/core@6.0.0"]["peerDependencies"]["third"].as_str(), Some("*"));
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
                origin: Origin::Types,
            },
            Finding {
                dependency: "unknown".to_string(),
                declared_range: None,
                severity: Severity::Undeclared,
                origin: Origin::Runtime,
            },
        ],
    }];
    let rendered = as_text(&reports);
    assert!(rendered.contains("known — declared as a devDependency, used in types"), "{rendered}");
    assert!(rendered.contains("unknown — not in the manifest at all, used in code"), "{rendered}");
}

#[test]
fn publisher_local_ranges_are_not_copied_to_consumers() {
    for range in
        ["workspace:*", "file:../helper", "link:../helper", "github:example/helper", "../helper"]
    {
        let reports = vec![PackageReport {
            package: "example@1.0.0".into(),
            findings: vec![Finding {
                dependency: "helper".into(),
                declared_range: Some(range.into()),
                severity: Severity::DevDependency,
                origin: Origin::Runtime,
            }],
        }];
        let rendered = as_package_extensions(&reports);
        let parsed = YamlLoader::load_from_str(&rendered).unwrap();
        assert_eq!(
            parsed[0]["packageExtensions"]["example@1.0.0"]["peerDependencies"]["helper"].as_str(),
            Some("*"),
        );
        assert!(!rendered.contains(range));
    }
}

#[test]
fn peer_context_duplicates_merge_origins_without_losing_findings() {
    let reports = vec![
        PackageReport {
            package: "example@1.0.0".into(),
            findings: vec![Finding {
                dependency: "helper".into(),
                declared_range: Some("^1".into()),
                severity: Severity::DevDependency,
                origin: Origin::Types,
            }],
        },
        PackageReport {
            package: "example@1.0.0".into(),
            findings: vec![Finding {
                dependency: "helper".into(),
                declared_range: Some("^1".into()),
                severity: Severity::DevDependency,
                origin: Origin::Runtime,
            }],
        },
    ];
    let merged = crate::report::merge_reports(reports);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].findings.len(), 1);
    assert_eq!(merged[0].findings[0].origin, Origin::Both);
    assert_eq!(merged[0].findings[0].declared_range.as_deref(), Some("^1"));
}
