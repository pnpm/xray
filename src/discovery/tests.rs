#![cfg(unix)]

use crate::{discovery::installed_packages, fixtures::global_virtual_store};

#[test]
fn walks_from_the_project_into_every_store_entry() {
    let root = global_virtual_store();
    let found = installed_packages(&root.path().join("project")).unwrap();
    let names: Vec<_> =
        found.iter().map(|dir| dir.file_name().unwrap().to_str().unwrap()).collect();
    assert_eq!(names, ["app", "helper"]);
}

#[test]
fn a_project_with_nothing_installed_says_so() {
    let root = tempfile::tempdir().unwrap();
    let error = installed_packages(root.path()).unwrap_err().to_string();
    assert!(error.contains("no installed dependencies"), "{error}");
}
