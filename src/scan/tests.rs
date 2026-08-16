use crate::scan::{specifiers, Origin, Requirement};
use std::path::Path;

fn found(source: &str) -> Vec<Requirement> {
    specifiers(source, Path::new("index.d.ts"))
        .unwrap()
        .into_iter()
        .map(|(requirement, _)| requirement)
        .collect()
}

fn origins(source: &str, name: &str) -> Vec<(Requirement, Origin)> {
    specifiers(source, Path::new(name)).unwrap().into_iter().collect()
}

fn modules(source: &str) -> Vec<String> {
    found(source)
        .into_iter()
        .filter_map(|requirement| match requirement {
            Requirement::Module(specifier) => Some(specifier),
            Requirement::TypesReference(_) => None,
        })
        .collect()
}

#[test]
fn collects_every_module_position() {
    let source = r"
        import type { A } from 'static-import';
        export { B } from 'export-from';
        export * from 'export-all';
        import x = require('require-import');
        declare const c: import('type-position').C;
        declare function load(): Promise<typeof import('dynamic-import')>;
    ";
    assert_eq!(
        modules(source),
        [
            "dynamic-import",
            "export-all",
            "export-from",
            "require-import",
            "static-import",
            "type-position",
        ],
    );
}

#[test]
fn collects_nested_type_position_imports() {
    let source = "declare const a: import('outer').A<import('inner').B>;";
    assert_eq!(modules(source), ["inner", "outer"]);
}

#[test]
fn ignores_specifiers_inside_comments() {
    let source = "/** @example import { readJson } from '@medplum/definitions'; */\nexport {};";
    assert!(modules(source).is_empty());
}

#[test]
fn collects_triple_slash_type_references() {
    let source = "/// <reference types=\"node\" />\nexport {};";
    assert_eq!(found(source), [Requirement::TypesReference("node".to_string())]);
}

#[test]
fn reports_files_it_cannot_parse() {
    let source = "export =       function broken(): void;";
    assert!(specifiers(source, Path::new("index.d.ts")).is_err());
}

fn script(source: &str, name: &str) -> Vec<String> {
    specifiers(source, Path::new(name))
        .unwrap()
        .into_iter()
        .filter_map(|(requirement, _origin)| match requirement {
            Requirement::Module(specifier) => Some(specifier),
            Requirement::TypesReference(_) => None,
        })
        .collect()
}

#[test]
fn collects_require_calls() {
    let source = r"
        const a = require('plain-require');
        const b = require.resolve('resolved-require');
        function lazy() { return require('lazy-require'); }
    ";
    assert_eq!(script(source, "index.js"), ["lazy-require", "plain-require", "resolved-require"]);
}

#[test]
fn a_guarded_require_still_counts() {
    // An optional integration is a dependency the package failed to declare;
    // reporting it as an optional peer is exactly right.
    let source = r"try { module.exports = require('optional-thing') } catch {}";
    assert_eq!(script(source, "index.js"), ["optional-thing"]);
}

#[test]
fn a_computed_require_names_no_package() {
    let source = r"const name = process.env.PLUGIN; module.exports = require(name);";
    assert!(script(source, "index.js").is_empty());
}

#[test]
fn a_local_binding_named_require_is_still_read_as_one() {
    // Distinguishing a shadowed `require` needs scope analysis the scanner does
    // not do; the cost is a rare extra finding, not a missed one.
    let source = r"function f(require) { return require('shadowed'); }";
    assert_eq!(script(source, "index.js"), ["shadowed"]);
}

#[test]
fn esm_shipped_under_a_cjs_extension_still_parses() {
    let source = r"import x from 'esm-under-js';
export default x;";
    assert_eq!(script(source, "index.js"), ["esm-under-js"]);
    assert_eq!(script(source, "index.cjs"), ["esm-under-js"]);
}

#[test]
fn triple_slash_references_are_only_read_from_declarations() {
    let source = "/// <reference types=\"node\" />\nmodule.exports = {};\n";
    assert!(specifiers(source, Path::new("index.js")).unwrap().is_empty());
}

#[test]
fn erased_positions_are_type_only_wherever_they_appear() {
    let source = r"
        import type { A } from 'type-import';
        export type { B } from 'type-export';
        import { C } from 'value-import';
        declare const d: import('type-position').D;
    ";
    let by_name: Vec<_> = origins(source, "index.ts")
        .into_iter()
        .filter_map(|(requirement, origin)| match requirement {
            Requirement::Module(specifier) => Some((specifier, origin)),
            Requirement::TypesReference(_) => None,
        })
        .collect();
    assert_eq!(
        by_name,
        [
            ("type-export".to_string(), Origin::Types),
            ("type-import".to_string(), Origin::Types),
            ("type-position".to_string(), Origin::Types),
            ("value-import".to_string(), Origin::Runtime),
        ],
    );
}

#[test]
fn everything_in_a_declaration_file_is_type_only() {
    let source = r"import { A } from 'value-looking-import';
export { A };";
    let origins = origins(source, "index.d.ts");
    assert!(origins.iter().all(|(_, origin)| *origin == Origin::Types), "{origins:?}");
}
