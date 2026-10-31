use super::{apply_text, workspace_root};
use crate::{
    report::{Finding, PackageReport, Severity},
    scan::Origin,
};
use std::collections::BTreeSet;

fn report(origin: Origin) -> Vec<PackageReport> {
    vec![PackageReport {
        package: "@example/consumer@1.2.3".into(),
        findings: vec![Finding {
            deep_import_only: false,
            dependency: "helper".into(),
            declared_range: None,
            severity: Severity::Undeclared,
            origin,
            evidence: BTreeSet::new(),
        }],
    }]
}

#[test]
fn honors_version_scoped_and_broad_configured_extensions() {
    for selector in ["@example/consumer@1.2.3", "@example/consumer@^1", "@example/consumer"] {
        let mut reports = report(Origin::Runtime);
        apply_text(
            &format!(
                "packageExtensions:\n  '{selector}':\n    dependencies:\n      helper: '^1'\n",
            ),
            &mut reports,
        )
        .unwrap();
        assert!(reports.is_empty(), "{selector}");
    }
    let mut reports = report(Origin::Runtime);
    apply_text(
        "packageExtensions:\n  '@example/consumer@^2':\n    dependencies:\n      helper: '^1'\n",
        &mut reports,
    )
    .unwrap();
    assert_eq!(reports.len(), 1);
}

#[test]
fn configured_types_satisfy_only_erased_imports() {
    let yaml = "packageExtensions:\n  '@example/consumer':\n    peerDependencies:\n      '@types/helper': '*'\n";
    let mut types = report(Origin::Types);
    apply_text(yaml, &mut types).unwrap();
    assert!(types.is_empty());
    for origin in [Origin::Runtime, Origin::Both] {
        let mut reports = report(origin);
        apply_text(yaml, &mut reports).unwrap();
        assert_eq!(reports.len(), 1);
    }
}

#[test]
fn workspace_members_resolve_the_nearest_settings_file() {
    let root = tempfile::tempdir().unwrap();
    let member = root.path().join("packages/member");
    std::fs::create_dir_all(&member).unwrap();
    std::fs::write(root.path().join("pnpm-workspace.yaml"), "packages: [packages/*]\n").unwrap();
    assert_eq!(workspace_root(&member).unwrap(), root.path().canonicalize().unwrap());
    std::fs::write(member.join("pnpm-workspace.yaml"), "packages: []\n").unwrap();
    assert_eq!(workspace_root(&member).unwrap(), member.canonicalize().unwrap());
}

#[test]
fn invalid_extension_settings_cannot_hide_findings() {
    for settings in [
        "packageExtensions: { '@example/consumer': { dependencies: { helper: false } } }",
        "packageExtensions: { '@example/consumer': { dependencies: [] } }",
        "packageExtensions: { '@example/consumer': false }",
        "packageExtensions: { '@example/consumer@not-a-range': {} }",
    ] {
        let mut reports = report(Origin::Runtime);
        assert!(apply_text(settings, &mut reports).is_err(), "{settings}");
        assert_eq!(reports.len(), 1);
    }
}

#[test]
fn selectors_match_pnpm_prerelease_behavior() {
    let version = node_semver::Version::parse("1.0.0-beta.1").unwrap();
    assert!(super::matches_selector("example", "example", &version));
    assert!(!super::matches_selector("example@*", "example", &version));
    assert!(!super::matches_selector("example@^1.0.0", "example", &version));
    assert!(super::matches_selector("example@1.0.0-beta.1", "example", &version));
    assert!(super::matches_selector("example@>=1.0.0-0 <2", "example", &version));
}
