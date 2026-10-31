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
            Requirement::TypesReference(_)
            | Requirement::PathReference(_)
            | Requirement::EsmRelative(_)
            | Requirement::Style(_, _) => None,
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
fn recoverable_parser_diagnostics_are_not_reported_as_complete() {
    for source in ["const x; require('dependency');", "export const = ;"] {
        assert!(specifiers(source, Path::new("index.js")).is_err(), "{source}");
    }
}

#[test]
fn commonjs_top_level_return_still_exposes_imports() {
    let source = "if (!globalThis.Feature) return; require('needed');";
    for name in ["index.js", "index.cjs"] {
        assert_eq!(script(source, name), ["needed"]);
    }
    assert!(specifiers(source, Path::new("index.mjs")).is_err());
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

#[test]
fn parses_flow_imports_and_tracks_original_utf8_positions() {
    let source = "// @flow\nconst emoji = '🍀';\nimport type {A} from 'typed';\nimport value from 'runtime';\nexport * from 'exported';\nrequire('required');\n";
    let found = crate::scan::references(source, Path::new("index.js")).unwrap();
    for (specifier, origin) in [
        ("typed", Origin::Types),
        ("runtime", Origin::Runtime),
        ("exported", Origin::Runtime),
        ("required", Origin::Runtime),
    ] {
        let position = u32::try_from(source.find(&format!("'{specifier}'")).unwrap()).unwrap();
        assert!(
            found.contains(&(Requirement::Module(specifier.to_string()), origin, position)),
            "{specifier}: {found:?}",
        );
    }
}

#[test]
fn recognizes_flow_pragmas_in_block_comments() {
    for comment in ["/** @flow */", "/**\n * @flow strict\n */"] {
        let source =
            format!("{comment}\nimport type {{A}} from 'typed';\nexport type X = {{| +x: A |}};");
        assert!(crate::scan::references(&source, Path::new("index.js")).unwrap().contains(&(
            Requirement::Module("typed".into()),
            Origin::Types,
            u32::try_from(source.find("'typed'").unwrap()).unwrap()
        )));
    }
}

#[test]
fn flow_pragma_does_not_override_typescript_parser() {
    let source = "// @flow\nimport type { A } from 'types';\nconst value = 1 satisfies number;";
    for name in ["index.ts", "index.tsx"] {
        assert!(
            origins(source, name).contains(&(Requirement::Module("types".into()), Origin::Types)),
        );
    }
}

#[test]
fn parses_unmarked_flow_after_javascript_parser_fails() {
    let source = "import type {Thing} from 'types-package';\nexport default function clone(value: Thing): Thing { return value; }";
    let found = origins(source, "index.js");
    assert!(found.contains(&(Requirement::Module("types-package".into()), Origin::Types)));
}

#[test]
fn flow_shadowed_require_is_scoped_and_syntax_errors_are_not_silenced() {
    let source = "// @flow\nrequire('outer'); function f(require) { require('hidden'); } { require('also-hidden'); let require = custom; } function g() { require('hoisted'); { var require = custom; } }";
    assert_eq!(script(source, "index.js"), ["outer"]);
    assert!(crate::scan::references("// @flow\nexport const = ;", Path::new("index.js")).is_err());
    assert!(crate::scan::references(
        "// @flow\nconst require = createRequire(url); require('x');",
        Path::new("index.js")
    )
    .is_err());
}

fn script(source: &str, name: &str) -> Vec<String> {
    specifiers(source, Path::new(name))
        .unwrap()
        .into_iter()
        .filter_map(|(requirement, _origin)| match requirement {
            Requirement::Module(specifier) => Some(specifier),
            Requirement::TypesReference(_)
            | Requirement::PathReference(_)
            | Requirement::EsmRelative(_)
            | Requirement::Style(_, _) => None,
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
fn constant_commonjs_aliases_follow_bindings_without_guessing_mutable_or_shadowed_values() {
    let source = "const load = require; const again = load; const resolve = require.resolve; const also = resolve; again('loaded'); also('resolved'); let mutable = require; mutable('not-proven'); function f(require) { const load = require; load('shadowed'); }";
    assert_eq!(script(source, "index.cjs"), ["loaded", "resolved"]);
    let flow = format!("// @flow\n{source}");
    assert_eq!(script(&flow, "index.js"), ["loaded", "resolved"]);
    let reversed = "function f() { const again = load; again('nested'); } const load = require;";
    assert_eq!(script(reversed, "index.js"), ["nested"]);
    let computed = "const resolve = require['resolve']; resolve('computed');";
    assert_eq!(script(computed, "index.js"), ["computed"]);
}

#[test]
fn custom_resolution_roots_are_not_attributed_to_the_calling_package() {
    let source = r"require.resolve('default'); require.resolve('local', {paths: [__dirname]}); require.resolve('empty', {}); require.resolve('foreign', {paths: [otherDir]}); const resolve = require.resolve; resolve('aliased-foreign', {paths: [otherDir]}); const load = require; load.resolve('aliased-local', {paths: [__dirname]}); require.resolve('opaque', options); function f(__dirname) { require.resolve('shadowed-dir', {paths: [__dirname]}); }";
    let expected = ["aliased-local", "default", "empty", "local"];
    assert_eq!(script(source, "index.cjs"), expected);
    assert_eq!(script(&format!("// @flow\n{source}"), "index.js"), expected);
}

#[test]
fn node_custom_resolve_roots_can_reach_another_packages_dependency() {
    let root = tempfile::tempdir().unwrap();
    let foreign = root.path().join("foreign");
    let dependency = foreign.join("node_modules/foreign-only");
    std::fs::create_dir_all(&dependency).unwrap();
    std::fs::write(dependency.join("index.js"), "module.exports = 1;").unwrap();
    let foreign_path = serde_json::to_string(&foreign.to_string_lossy()).unwrap();
    let source = format!("require.resolve('foreign-only', {{paths: [{foreign_path}]}});\n");
    let entry = root.path().join("caller/entry.cjs");
    std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
    std::fs::write(&entry, &source).unwrap();
    let output = std::process::Command::new("node").arg(&entry).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(crate::scan::references(&source, &entry).unwrap().is_empty());
}

#[test]
fn commonjs_aliases_match_node_runtime() {
    let root = tempfile::tempdir().unwrap();
    let package = root.path().join("node_modules/example");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(package.join("package.json"), r#"{"name":"example","main":"index.js"}"#)
        .unwrap();
    std::fs::write(package.join("index.js"), "module.exports = 42;").unwrap();
    let source = "const load = require; const again = load; const resolve = require.resolve; if (again('example') !== 42 || !resolve('example')) process.exit(1);";
    let entry = root.path().join("entry.cjs");
    std::fs::write(&entry, source).unwrap();
    assert!(std::process::Command::new("node").arg(entry).status().unwrap().success());
    assert_eq!(script(source, "entry.cjs"), ["example"]);
}

#[test]
fn module_require_is_recognized_only_for_the_unshadowed_commonjs_binding() {
    let source = "module.require('one'); module['require']('two'); function f(module) { module.require('hidden'); } const x = {require: () => {}}; x.require('ignored');";
    assert_eq!(script(source, "index.cjs"), ["one", "two"]);
    let references = crate::scan::references(source, Path::new("index.cjs")).unwrap();
    let expected = (
        Requirement::Module("one".into()),
        Origin::Runtime,
        u32::try_from(source.find("'one'").unwrap()).unwrap(),
    );
    assert!(references.contains(&expected));
}

#[test]
fn import_meta_resolve_detects_only_intrinsic_literal_calls() {
    let source = "import.meta.resolve('example'); import.meta['resolve']('second'); import.meta.resolve(variable); other.meta.resolve('ignored');";
    assert_eq!(script(source, "index.mjs"), ["example", "second"]);
    let references = crate::scan::references(source, Path::new("index.mjs")).unwrap();
    let offset = u32::try_from(source.find("'example'").unwrap()).unwrap();
    assert!(references.contains(&(Requirement::Module("example".into()), Origin::Runtime, offset)));
    let flow = "// @flow\nimport.meta.resolve('example'); import.meta['resolve']('second'); other.meta.resolve('ignored');";
    assert_eq!(script(flow, "index.js"), ["example", "second"]);
}

#[test]
fn import_meta_resolve_literal_works_in_node() {
    let root = tempfile::tempdir().unwrap();
    let package = root.path().join("node_modules/example");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(package.join("package.json"), r#"{"name":"example","main":"index.js"}"#)
        .unwrap();
    std::fs::write(package.join("index.js"), "module.exports = 1;").unwrap();
    let entry = root.path().join("index.mjs");
    std::fs::write(&entry, "console.log(import.meta.resolve('example'));").unwrap();
    let output = std::process::Command::new("node").arg(entry).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(String::from_utf8_lossy(&output.stdout).contains("example/index.js"));
}

#[test]
fn flow_module_require_respects_lexical_shadowing() {
    let source = "// @flow\nmodule.require('outer'); module['require']('computed'); function f(module) { module.require('hidden'); } class C { f(module) { module.require('class-hidden'); } }";
    assert_eq!(script(source, "index.js"), ["computed", "outer"]);
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
fn constant_aliases_of_created_require_are_followed() {
    let source = "import { createRequire } from 'node:module'; const load = createRequire(import.meta.url); const again = load; const resolve = again.resolve; again('loaded'); resolve('resolved');";
    assert_eq!(script(source, "index.mjs"), ["loaded", "node:module", "resolved"]);
}

#[test]
fn create_require_foreign_or_opaque_bases_are_not_attributed_to_caller() {
    let source = "import { createRequire } from 'node:module'; const foreign = createRequire('/other/consumer.cjs'); foreign('not-ours'); const unknown = createRequire(process.cwd() + '/other.cjs'); const alias = unknown; alias.resolve('also-not-ours'); function f(__filename) { const load = createRequire(__filename); load('shadowed-base'); } const local = createRequire(import.meta['url']); local('ours');";
    assert_eq!(script(source, "index.mjs"), ["node:module", "ours"]);
}

#[test]
fn create_require_resolves_relative_to_its_foreign_base_in_node() {
    let root = tempfile::tempdir().unwrap();
    let foreign = root.path().join("foreign");
    let package = foreign.join("node_modules/foreign-only");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(package.join("index.js"), "module.exports = 42;").unwrap();
    let base = serde_json::to_string(&foreign.join("consumer.cjs").to_string_lossy()).unwrap();
    let source = format!("import {{ createRequire }} from 'node:module'; const load = createRequire({base}); if (load('foreign-only') !== 42) process.exit(1);\n");
    let entry = root.path().join("caller.mjs");
    std::fs::write(&entry, &source).unwrap();
    let output = std::process::Command::new("node").arg(&entry).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(script(&source, "caller.mjs"), ["node:module"]);
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
fn triple_slash_directives_are_only_recognized_before_statements() {
    let source = "/// description <reference types=\"fake\" />\n/// <reference types=\"real\" />\nexport {};\n/// <reference types=\"late\" />\n";
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
fn triple_slash_references_are_read_from_typescript_but_not_javascript() {
    let source = "/// <reference types=\"node\" />\nmodule.exports = {};\n";
    assert!(specifiers(source, Path::new("index.js")).unwrap().is_empty());
    for extension in ["ts", "tsx", "mts", "cts"] {
        let path = format!("index.{extension}");
        assert!(specifiers(source, Path::new(&path))
            .unwrap()
            .contains(&(Requirement::TypesReference("node".into()), Origin::Types)));
    }
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
            Requirement::TypesReference(_)
            | Requirement::PathReference(_)
            | Requirement::EsmRelative(_)
            | Requirement::Style(_, _) => None,
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
