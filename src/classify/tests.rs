use crate::{
    classify::{missing_package, package_name, types_package},
    scan::Requirement,
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
fn a_scope_on_its_own_is_not_a_package() {
    assert_eq!(package_name("@scope"), None);
}

#[test]
fn maps_a_name_to_its_types_package() {
    assert_eq!(types_package("node"), "@types/node");
    assert_eq!(types_package("@medplum/core"), "@types/medplum__core");
}

#[test]
fn an_import_is_satisfied_by_the_types_package_that_declares_it() {
    let requirement = Requirement::Module("estree".to_string());
    assert_eq!(missing_package(&requirement, &declared(&["@types/estree"])), None);
    assert_eq!(missing_package(&requirement, &declared(&[])), Some("estree".to_string()));
}

#[test]
fn a_types_reference_is_satisfied_by_either_spelling() {
    let requirement = Requirement::TypesReference("node".to_string());
    assert_eq!(missing_package(&requirement, &declared(&["@types/node"])), None);
    assert_eq!(missing_package(&requirement, &declared(&["node"])), None);
}

#[test]
fn an_unsatisfied_types_reference_is_reported_under_the_types_name() {
    let requirement = Requirement::TypesReference("estree".to_string());
    assert_eq!(missing_package(&requirement, &declared(&[])), Some("@types/estree".to_string()));
}
