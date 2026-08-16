#![cfg(unix)]

use crate::{
    fixtures::{global_virtual_store, manifest, write},
    manifest::Manifest,
    reachable::reachable_files,
};

fn names(package_dir: &std::path::Path) -> Vec<String> {
    let manifest = Manifest::read(package_dir).unwrap();
    let package_dir = &package_dir.canonicalize().unwrap();
    reachable_files(package_dir, &manifest)
        .into_iter()
        .map(|path| path.strip_prefix(package_dir).unwrap().to_string_lossy().replace('\\', "/"))
        .collect()
}

#[test]
fn follows_relative_imports_out_of_the_entry_points() {
    let root = global_virtual_store();
    let app = root.path().join("store/app/node_modules/app");
    let mut found = names(&app);
    found.sort();
    assert_eq!(found, ["index.d.ts", "index.js", "lib/helper.js"]);
}

#[test]
fn a_package_with_no_entry_field_still_has_an_index() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("pkg");
    write(&dir.join("package.json"), &manifest("pkg", ""));
    write(&dir.join("index.js"), "require('something');\n");
    assert_eq!(names(&dir), ["index.js"]);
}

#[test]
fn an_entry_naming_a_directory_resolves_to_its_index() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("pkg");
    write(&dir.join("package.json"), &manifest("pkg", r#","main":"./lib""#));
    write(&dir.join("lib/index.js"), "module.exports = 1;\n");
    assert_eq!(names(&dir), ["lib/index.js"]);
}

/// A TypeScript package importing `./thing.js` means the source that compiles
/// to it, which is what the tarball holds when sources ship alongside.
#[test]
fn a_js_specifier_resolves_to_the_typescript_source_beside_it() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("pkg");
    write(&dir.join("package.json"), &manifest("pkg", r#","main":"index.ts""#));
    write(&dir.join("index.ts"), "export * from './thing.js';\n");
    write(&dir.join("thing.ts"), "export const a = 1;\n");
    let mut found = names(&dir);
    found.sort();
    assert_eq!(found, ["index.ts", "thing.ts"]);
}

#[test]
fn every_string_under_exports_is_an_entry() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("pkg");
    write(
        &dir.join("package.json"),
        &manifest("pkg", r#","exports":{".":{"import":"./esm.mjs","require":"./cjs.cjs"}}"#),
    );
    write(&dir.join("esm.mjs"), "export const a = 1;\n");
    write(&dir.join("cjs.cjs"), "module.exports = 1;\n");
    let mut found = names(&dir);
    found.sort();
    assert_eq!(found, ["cjs.cjs", "esm.mjs"]);
}

/// Two files importing each other is ordinary, and it used to walk forever:
/// joining `./a` onto a directory keeps the `.`, so the same file arrived under
/// a new path each lap and the visited set never recognised it. The walk ate
/// memory until the process died.
#[test]
fn a_cycle_between_two_files_terminates() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("pkg");
    write(&dir.join("package.json"), &manifest("pkg", r#","main":"./a.js""#));
    write(&dir.join("a.js"), "require('./b.js');\nrequire('phantom-a');\n");
    write(&dir.join("b.js"), "require('./a.js');\nrequire('phantom-b');\n");

    let mut found = names(&dir);
    found.sort();
    assert_eq!(found, ["a.js", "b.js"]);
}

#[test]
fn a_file_importing_itself_terminates() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("pkg");
    write(&dir.join("package.json"), &manifest("pkg", r#","main":"./self.js""#));
    write(&dir.join("self.js"), "require('./self.js');\n");
    assert_eq!(names(&dir), ["self.js"]);
}

/// Packages routinely put `"./package.json"` in `exports`. It is a real file
/// and a real entry, and it is not source.
#[test]
fn an_entry_that_is_not_source_is_not_scanned() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("pkg");
    write(
        &dir.join("package.json"),
        &manifest("pkg", r#","exports":{".":"./index.js","./package.json":"./package.json"}"#),
    );
    write(&dir.join("index.js"), "module.exports = 1;\n");
    assert_eq!(names(&dir), ["index.js"]);
}
