use crate::scan::{specifiers, Requirement};

fn found(source: &str) -> Vec<Requirement> {
    specifiers(source).unwrap().into_iter().collect()
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
    assert!(specifiers(source).is_err());
}
