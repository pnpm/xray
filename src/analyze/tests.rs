#![cfg(unix)]

use crate::{
    analyze::{analyze, scannable_files, Scope},
    fixtures::global_virtual_store,
    report::Severity,
    scan::Origin,
};

#[test]
fn reports_a_dev_dependency_that_leaks_into_declarations() {
    let root = global_virtual_store();
    let app = root.path().join("store/app/node_modules/app");
    let report = analyze(&app, Scope::Declared).unwrap().expect("app should have a finding");
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
    let report = analyze(&app, Scope::Declared).unwrap().expect("app should have findings");
    let runtime =
        report.findings.iter().find(|f| f.dependency == "runtime-ghost").expect("runtime-ghost");
    assert_eq!(runtime.severity, Severity::DevDependency);
    assert_eq!(runtime.origin, Origin::Runtime);
}

#[test]
fn a_package_that_declares_what_it_imports_is_not_reported() {
    let root = global_virtual_store();
    let helper = root.path().join("store/helper/node_modules/helper");
    assert!(analyze(&helper, Scope::Declared).unwrap().is_none());
}

#[test]
fn files_bundled_under_node_modules_are_not_the_package_own() {
    let root = global_virtual_store();
    let app = root.path().join("store/app/node_modules/app");
    let scanned: Vec<_> = scannable_files(&app).collect();
    assert_eq!(scanned.len(), 4, "{scanned:?}");
    assert!(scanned.iter().all(|path| !path.to_string_lossy().contains("vendored")), "{scanned:?}");
}

#[test]
fn a_file_nothing_reaches_is_not_an_encapsulated_package_business() {
    let root = global_virtual_store();
    let app = root.path().join("store/app/node_modules/app");

    let reachable = analyze(&app, Scope::Declared).unwrap().expect("findings");
    let names: Vec<_> = reachable.findings.iter().map(|f| f.dependency.as_str()).collect();
    assert!(names.contains(&"reached-ghost"), "{names:?}");
    assert!(!names.contains(&"test-only-ghost"), "{names:?}");

    let everything = analyze(&app, Scope::EveryFile).unwrap().expect("findings");
    let names: Vec<_> = everything.findings.iter().map(|f| f.dependency.as_str()).collect();
    assert!(names.contains(&"test-only-ghost"), "--all-files should still find it: {names:?}");
}

/// Without an `exports` map, `require("pkg/test/spec")` resolves, so every
/// shipped file is part of the surface whether the author meant it or not.
#[test]
fn a_package_without_exports_has_no_unreachable_files() {
    use crate::fixtures::{manifest, write};

    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("pkg");
    write(&dir.join("package.json"), &manifest("pkg", r#","main":"index.js""#));
    write(&dir.join("index.js"), "module.exports = 1;\n");
    write(&dir.join("test/spec.js"), "require('only-in-tests');\n");

    let report = analyze(&dir, Scope::Declared).unwrap().expect("findings");
    let names: Vec<_> = report.findings.iter().map(|f| f.dependency.as_str()).collect();
    assert_eq!(names, ["only-in-tests"]);
}
