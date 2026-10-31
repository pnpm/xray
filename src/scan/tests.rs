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
    let source = "export const = ;";
    assert!(specifiers(source, Path::new("index.d.ts")).is_err());
}

#[test]
fn reads_declaration_with_export_assignment_function_signature() {
    let source =
        "import type { T } from 'needed';\nexport =       function remapping(input: T): T;\n";
    assert_eq!(script(source, "index.d.cts"), ["needed"]);
}

#[test]
fn reads_jsx_in_js_files() {
    let source = "import Widget from 'widget'; export default () => <Widget />;";
    assert_eq!(script(source, "index.js"), ["widget"]);
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
fn a_local_binding_named_require_is_not_a_node_import() {
    let source = r"require('global'); function f(require) { return require('shadowed'); } { const require = x; require('also-shadowed'); }";
    assert_eq!(script(source, "index.js"), ["global"]);
}

#[test]
fn create_require_bindings_are_node_imports() {
    let source = r"import { createRequire } from 'node:module'; const require = createRequire(import.meta.url); require('loaded'); require.resolve('resolved');";
    assert_eq!(script(source, "index.mjs"), ["loaded", "node:module", "resolved"]);
}

#[test]
fn aliases_of_create_require_and_its_result_are_node_imports() {
    for source in [
        "import { createRequire as makeLoader } from 'node:module'; const load = makeLoader(import.meta.url); load('loaded');",
        "const { createRequire: makeLoader } = require('node:module'); const load = makeLoader(__filename); load('loaded');",
        "import * as mod from 'node:module'; const load = mod.createRequire(import.meta.url); load('loaded');",
        "const mod = require('node:module'); const load = mod.createRequire(__filename); load('loaded');",
        "const load = require('node:module').createRequire(__filename); load('loaded');",
    ] {
        assert_eq!(script(source, "index.js"), ["loaded", "node:module"]);
    }
}

#[test]
fn unrelated_factory_named_create_require_is_not_a_node_loader() {
    let source = "import { createRequire } from 'unrelated'; const load = createRequire(); load('not-a-dependency');";
    assert_eq!(script(source, "index.js"), ["unrelated"]);
    let shadowed = "function f(require) { const {createRequire} = require('node:module'); const load = createRequire(); load('not-a-dependency'); }";
    assert!(script(shadowed, "index.js").is_empty());
}

#[test]
fn triple_slash_text_inside_a_block_comment_is_not_a_reference() {
    let source =
        "/*\n/// <reference types=\"fake\" />\n*/\n/// <reference types=\"real\" />\nexport {};";
    assert_eq!(found(source), [Requirement::TypesReference("real".to_string())]);
}

#[test]
fn normalization_does_not_scan_comment_content_as_code() {
    let source = "/*\nexport = function pretend(): void;\nimport 'fake';\n*/\nimport 'real';\nexport = function actual(): void;";
    assert_eq!(script(source, "index.d.cts"), ["real"]);
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

/// `import { type A }` is elided by default but survives under
/// `verbatimModuleSyntax`, where TypeScript emits `import {} from "pkg"` and the
/// module really is loaded. Since the consumer's compiler options are unknowable
/// from the package, the runtime reading is the safe one: it cannot hide a
/// dependency the program turns out to need. Only the statement-level `import
/// type`, which is erased under every option, counts as type-only.
#[test]
fn inline_type_specifiers_are_read_as_runtime() {
    let statement_level = origins("import type { A } from 'erased';", "index.ts");
    assert_eq!(statement_level[0].1, Origin::Types);

    for source in [
        "import { type B } from 'kept';",
        "import { type C, D } from 'kept';",
        "export { type F } from 'kept';",
    ] {
        let found = origins(source, "index.ts");
        assert!(
            found.iter().all(|(_, origin)| *origin == Origin::Runtime),
            "{source} should stay runtime: {found:?}",
        );
    }
}
