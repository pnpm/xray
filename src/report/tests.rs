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
fn package_extensions_declare_an_optional_peer() {
    assert_eq!(
        as_package_extensions(&medplum()),
        "packageExtensions:\n  \
         '@medplum/core':\n    \
         peerDependencies:\n      \
         '@medplum/fhirtypes': '*'\n    \
         peerDependenciesMeta:\n      \
         '@medplum/fhirtypes':\n        \
         optional: true\n",
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
        Some(true),
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
