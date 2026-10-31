use crate::{
    classify::{missing_package, package_name, types_package},
    scan::{Origin, Requirement},
};
use std::collections::HashSet;

fn declared<'a>(names: &'a [&'a str]) -> HashSet<&'a str> {
    names.iter().copied().collect()
}

#[test]
fn takes_the_package_out_of_a_specifier() {
    assert_eq!(package_name("lodash"), Some("lodash"));
    assert_eq!(package_name("lodash/fp"), Some("lodash"));
    assert_eq!(package_name("@medplum/core"), Some("@medplum/core"));
    assert_eq!(package_name("pdfmake/interfaces"), Some("pdfmake"));
    assert_eq!(package_name("@scope/name/deep/path"), Some("@scope/name"));
}

#[test]
fn ignores_specifiers_that_need_nothing_installed() {
    for specifier in [
        "./relative",
        "../up",
        "/absolute",
        "#subpath-import",
        "node:fs",
        "fs",
        "worker_threads",
        "data:text/javascript,export{}",
        "https://example.com/mod.js",
        "",
    ] {
        assert_eq!(package_name(specifier), None, "{specifier} should need no package");
    }
}

#[test]
fn builtin_subpaths_are_only_exempt_when_node_recognizes_the_full_specifier() {
    for specifier in ["fs/promises", "path/posix", "assert/strict", "_stream_readable"] {
        assert_eq!(package_name(specifier), None, "{specifier}");
    }
    assert_eq!(package_name("fs/custom"), Some("fs"));
    assert_eq!(package_name("path/custom"), Some("path"));

    let root = tempfile::tempdir().unwrap();
    let package = root.path().join("node_modules/fs");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(package.join("custom.js"), "module.exports = 42;").unwrap();
    let output = std::process::Command::new("node")
        .arg("-e")
        .arg("if (require('fs/custom') !== 42) process.exit(1)")
        .current_dir(root.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
}

#[test]
fn node_builtin_names_are_not_builtin_sass_modules() {
    let requirement = Requirement::Style("events".into(), crate::style::StyleKind::Use);
    assert_eq!(
        missing_package(&requirement, Origin::Runtime, &declared(&[])),
        Some("events".into()),
    );
    let node = Requirement::Module("events".into());
    assert_eq!(missing_package(&node, Origin::Runtime, &declared(&[])), None);
}

#[test]
fn a_scope_on_its_own_is_not_a_package() {
    assert_eq!(package_name("@scope"), None);
}

#[test]
fn prefix_only_node_builtins_do_not_mask_bare_npm_packages() {
    for name in ["test", "sqlite", "sea"] {
        assert_eq!(package_name(name), Some(name));
        assert_eq!(package_name(&format!("node:{name}")), None);
    }
}

#[test]
fn maps_a_name_to_its_types_package() {
    assert_eq!(types_package("node"), "@types/node");
    assert_eq!(types_package("@medplum/core"), "@types/medplum__core");
}

#[test]
fn an_import_is_satisfied_by_the_types_package_that_declares_it() {
    let requirement = Requirement::Module("estree".to_string());
    assert_eq!(missing_package(&requirement, Origin::Types, &declared(&["@types/estree"])), None);
    assert_eq!(
        missing_package(&requirement, Origin::Types, &declared(&[])),
        Some("estree".to_string()),
    );
}

#[test]
fn a_types_reference_is_satisfied_by_either_spelling() {
    let requirement = Requirement::TypesReference("node".to_string());
    assert_eq!(missing_package(&requirement, Origin::Types, &declared(&["@types/node"])), None);
    assert_eq!(missing_package(&requirement, Origin::Types, &declared(&["node"])), None);
}

#[test]
fn an_unsatisfied_types_reference_is_reported_under_the_types_name() {
    let requirement = Requirement::TypesReference("estree".to_string());
    assert_eq!(
        missing_package(&requirement, Origin::Types, &declared(&[])),
        Some("@types/estree".to_string()),
    );
}

#[test]
fn a_types_package_does_not_satisfy_a_runtime_reference() {
    // `@types/lodash` carries no implementation, so requiring `lodash` at run
    // time still needs `lodash` even though the type position was satisfied.
    let requirement = Requirement::Module("lodash".to_string());
    assert_eq!(missing_package(&requirement, Origin::Types, &declared(&["@types/lodash"])), None);
    assert_eq!(
        missing_package(&requirement, Origin::Runtime, &declared(&["@types/lodash"])),
        Some("lodash".to_string()),
    );
    assert_eq!(missing_package(&requirement, Origin::Runtime, &declared(&["lodash"])), None);
}
