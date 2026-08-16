#![cfg(unix)]

use crate::{
    analyze::{analyze, scannable_files},
    fixtures::global_virtual_store,
    report::{Origin, Severity},
};

#[test]
fn reports_a_dev_dependency_that_leaks_into_declarations() {
    let root = global_virtual_store();
    let app = root.path().join("store/app/node_modules/app");
    let report = analyze(&app).unwrap().expect("app should have a finding");
    assert_eq!(report.package, "app@1.0.0");
    let ghost = report.findings.iter().find(|f| f.dependency == "ghost").expect("ghost");
    assert_eq!(ghost.severity, Severity::DevDependency);
    assert_eq!(ghost.declared_range.as_deref(), Some("^1"));
    assert_eq!(ghost.origin, Origin::Types);
}

#[test]
fn reports_a_dependency_required_from_executable_code() {
    let root = global_virtual_store();
    let app = root.path().join("store/app/node_modules/app");
    let report = analyze(&app).unwrap().expect("app should have findings");
    let runtime =
        report.findings.iter().find(|f| f.dependency == "runtime-ghost").expect("runtime-ghost");
    assert_eq!(runtime.severity, Severity::DevDependency);
    assert_eq!(runtime.origin, Origin::Runtime);
}

#[test]
fn a_package_that_declares_what_it_imports_is_not_reported() {
    let root = global_virtual_store();
    let helper = root.path().join("store/helper/node_modules/helper");
    assert!(analyze(&helper).unwrap().is_none());
}

#[test]
fn files_bundled_under_node_modules_are_not_the_package_own() {
    let root = global_virtual_store();
    let app = root.path().join("store/app/node_modules/app");
    let scanned: Vec<_> = scannable_files(&app).collect();
    assert_eq!(scanned.len(), 2, "{scanned:?}");
    assert!(scanned.iter().all(|path| !path.to_string_lossy().contains("vendored")), "{scanned:?}");
}
