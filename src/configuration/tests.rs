use super::apply_text;
use crate::report::{Finding, Severity};
use crate::{report::PackageReport, scan::Origin};
use std::collections::BTreeSet;

fn report(origin: Origin) -> Vec<PackageReport> {
    vec![PackageReport {
        package: "@example/consumer@1.2.3".into(),
        findings: vec![Finding {
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
                "packageExtensions:\n  '{selector}':\n    dependencies:\n      helper: '^1'\n"
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
