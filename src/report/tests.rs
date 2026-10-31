use crate::report::{as_package_extensions, as_text, Finding, Origin, PackageReport, Severity};
use yaml_rust2::YamlLoader;

fn medplum() -> Vec<PackageReport> {
    vec![PackageReport {
        package: "@medplum/core@5.1.15".to_string(),
        findings: vec![Finding {
            deep_import_only: false,
            dependency: "@medplum/fhirtypes".to_string(),
            declared_range: Some("5.1.15".to_string()),
            severity: Severity::DevDependency,
            origin: Origin::Types,
            evidence: std::collections::BTreeSet::new(),
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
            deep_import_only: false,
            dependency: "another".into(),
            declared_range: None,
            severity: Severity::Undeclared,
            origin: Origin::Runtime,
            evidence: std::collections::BTreeSet::new(),
        }],
    });
    reports.push(PackageReport {
        package: "@medplum/core@6.0.0".into(),
        findings: vec![Finding {
            deep_import_only: false,
            dependency: "third".into(),
            declared_range: None,
            severity: Severity::Undeclared,
            origin: Origin::Runtime,
            evidence: std::collections::BTreeSet::new(),
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
fn packages_without_a_semver_version_get_no_extension() {
    let report = |package: &str| PackageReport {
        package: package.to_string(),
        findings: vec![Finding {
            deep_import_only: false,
            dependency: "missing".to_string(),
            declared_range: None,
            severity: Severity::Undeclared,
            origin: Origin::Runtime,
            evidence: std::collections::BTreeSet::new(),
        }],
    };
    let unversioned = vec![report("example@"), report("example@1.0"), report("@scope/pkg@")];
    assert_eq!(as_package_extensions(&unversioned), "");
    assert_eq!(
        crate::report::without_extension_selector(&unversioned),
        ["example@", "example@1.0", "@scope/pkg@"],
    );
    let mut mixed = unversioned;
    mixed.push(report("@scope/pkg@1.0.0-beta.1"));
    let rendered = as_package_extensions(&mixed);
    let documents = YamlLoader::load_from_str(&rendered).unwrap();
    let extensions = documents[0]["packageExtensions"].as_hash().unwrap();
    assert_eq!(extensions.len(), 1);
    assert!(rendered.contains("'@scope/pkg@1.0.0-beta.1':"));
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
                deep_import_only: false,
                dependency: "known".to_string(),
                declared_range: Some("^1".to_string()),
                severity: Severity::DevDependency,
                origin: Origin::Types,
                evidence: std::collections::BTreeSet::new(),
            },
            Finding {
                deep_import_only: false,
                dependency: "unknown".to_string(),
                declared_range: None,
                severity: Severity::Undeclared,
                origin: Origin::Runtime,
                evidence: std::collections::BTreeSet::new(),
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
                deep_import_only: false,
                dependency: "helper".into(),
                declared_range: Some(range.into()),
                severity: Severity::DevDependency,
                origin: Origin::Runtime,
                evidence: std::collections::BTreeSet::new(),
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
                deep_import_only: false,
                dependency: "helper".into(),
                declared_range: Some("^1".into()),
                severity: Severity::DevDependency,
                origin: Origin::Types,
                evidence: std::collections::BTreeSet::new(),
            }],
        },
        PackageReport {
            package: "example@1.0.0".into(),
            findings: vec![Finding {
                deep_import_only: false,
                dependency: "helper".into(),
                declared_range: Some("^1".into()),
                severity: Severity::DevDependency,
                origin: Origin::Runtime,
                evidence: std::collections::BTreeSet::new(),
            }],
        },
    ];
    let merged = crate::report::merge_reports(reports);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].findings.len(), 1);
    assert_eq!(merged[0].findings[0].origin, Origin::Both);
    assert_eq!(merged[0].findings[0].declared_range.as_deref(), Some("^1"));
}

#[test]
fn portable_ranges_follow_npm_semver_including_prereleases() {
    for range in ["^2.0.0-beta.1", ">=1.2 <3", "1.0.0 || ^2.0.0", "~1.2", "*"] {
        assert!(crate::report::registry_range(range), "{range}");
    }
    for range in [
        "",
        "v",
        "file:../helper",
        "workspace:^",
        "github:example/helper",
        "latest",
        "not a version",
    ] {
        assert!(!crate::report::registry_range(range), "{range}");
    }
}

#[test]
fn merging_peer_contexts_retains_deep_only_label_only_when_every_context_agrees() {
    let mut deep = medplum();
    deep[0].findings[0].deep_import_only = true;
    let merged = super::merge_reports([deep.clone(), deep.clone()].concat());
    assert!(merged[0].findings[0].deep_import_only);
    let merged = super::merge_reports([deep, medplum()].concat());
    assert!(!merged[0].findings[0].deep_import_only);
}
