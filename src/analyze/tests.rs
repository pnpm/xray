#![cfg(unix)]

use crate::{
    analyze::{analyze, declaration_files},
    fixtures::global_virtual_store,
    report::Severity,
};

#[test]
fn reports_a_dev_dependency_that_leaks_into_declarations() {
    let root = global_virtual_store();
    let app = root.path().join("store/app/node_modules/app");
    let report = analyze(&app).unwrap().expect("app should have a finding");
    assert_eq!(report.package, "app@1.0.0");
    assert_eq!(report.findings.len(), 1);
    assert_eq!(report.findings[0].dependency, "ghost");
    assert_eq!(report.findings[0].severity, Severity::DevDependency);
    assert_eq!(report.findings[0].declared_range.as_deref(), Some("^1"));
}

#[test]
fn a_package_that_declares_what_it_imports_is_not_reported() {
    let root = global_virtual_store();
    let helper = root.path().join("store/helper/node_modules/helper");
    assert!(analyze(&helper).unwrap().is_none());
}

#[test]
fn declarations_bundled_under_node_modules_are_not_the_package_own() {
    let root = global_virtual_store();
    let app = root.path().join("store/app/node_modules/app");
    let scanned: Vec<_> = declaration_files(&app).collect();
    assert_eq!(scanned.len(), 1, "{scanned:?}");
    assert!(scanned[0].ends_with("app/index.d.ts"), "{scanned:?}");
}
