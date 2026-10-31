use super::test_helpers::analyze;
#[cfg(unix)]
use crate::{analyze::scannable_files, fixtures::global_virtual_store, report::Severity};

use crate::{analyze::Scope, scan::Origin};

#[cfg(unix)]
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

#[cfg(unix)]
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

#[cfg(unix)]
#[test]
fn a_package_that_declares_what_it_imports_is_not_reported() {
    let root = global_virtual_store();
    let helper = root.path().join("store/helper/node_modules/helper");
    assert!(analyze(&helper, Scope::Declared).unwrap().is_none());
}

#[cfg(unix)]
#[test]
fn files_bundled_under_node_modules_are_not_the_package_own() {
    let root = global_virtual_store();
    let app = root.path().join("store/app/node_modules/app");
    let scanned: Vec<_> = scannable_files(&app).collect();
    assert_eq!(scanned.len(), 4, "{scanned:?}");
    assert!(scanned.iter().all(|path| !path.to_string_lossy().contains("vendored")), "{scanned:?}");
}

#[cfg(unix)]
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

#[test]
fn boolean_bundle_dependencies_does_not_prevent_analysis() {
    use crate::fixtures::{manifest, write};

    for flag in ["true", "false"] {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("pkg");
        write(
            &dir.join("package.json"),
            &manifest(
                "pkg",
                &format!(r#","bundleDependencies":{flag},"dependencies":{{"declared":"1"}}"#),
            ),
        );
        write(&dir.join("index.js"), "require('declared'); require('missing');");
        let report = analyze(&dir, Scope::Declared).unwrap().unwrap();
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].dependency, "missing");
    }
}

#[test]
fn imports_map_targets_are_followed_and_external_dependencies_reported() {
    use crate::fixtures::{manifest, write};
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("pkg");
    write(
        &dir.join("package.json"),
        &manifest(
            "pkg",
            r##","exports":"./index.js","imports":{"#feature/*":{"node":"./lib/*.js","default":"fallback/*"},"#feature/hidden":null,"#unused":"unused-package"},"devDependencies":{"fallback":"^2"}"##,
        ),
    );
    write(&dir.join("index.js"), "require('#feature/active'); require('#feature/hidden');");
    write(&dir.join("lib/active.js"), "require('nested-missing');");
    write(&dir.join("lib/hidden.js"), "require('hidden-missing');");
    let report = analyze(&dir, Scope::Declared).unwrap().unwrap();
    let findings: Vec<_> = report
        .findings
        .iter()
        .map(|finding| (finding.dependency.as_str(), finding.origin))
        .collect();
    assert_eq!(findings, [("fallback", Origin::Runtime), ("nested-missing", Origin::Runtime)]);
    assert_eq!(report.findings[0].declared_range.as_deref(), Some("^2"));
}

#[test]
fn type_imports_keep_conditional_fallback_when_types_target_cannot_resolve() {
    use crate::fixtures::{manifest, write};
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("pkg");
    write(
        &dir.join("package.json"),
        &manifest(
            "pkg",
            r##","exports":"./index.js","types":"./index.d.ts","imports":{"#dependency":{"types":"./missing.d.ts","default":"fallback-package/subpath"}}"##,
        ),
    );
    write(&dir.join("index.js"), "module.exports = {};\n");
    write(
        &dir.join("index.d.ts"),
        "import type { T } from '#dependency'; export type Value = T;\n",
    );
    let report = crate::analyze::analyze_for_repair(&dir, Scope::Declared).unwrap().unwrap();
    assert_eq!(report.findings.len(), 1);
    assert_eq!(report.findings[0].dependency, "fallback-package");
    assert_eq!(report.findings[0].origin, Origin::Types);
    assert_eq!(report.findings[0].evidence.first().unwrap().specifier, "#dependency");
}

#[test]
fn browser_mappings_are_followed_only_for_reached_runtime_references() {
    use crate::fixtures::{manifest, write};
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("pkg");
    write(
        &dir.join("package.json"),
        &manifest(
            "pkg",
            r#","exports":"./index.js","types":"./types.d.ts","browser":{"crypto":"crypto-polyfill","./server.js":"./browser.js","types-only":"not-runtime","unused":"unused-polyfill"}"#,
        ),
    );
    write(&dir.join("index.js"), "require('crypto'); require('./server');");
    write(&dir.join("server.js"), "require('server-dependency');");
    write(&dir.join("browser.js"), "require('browser-dependency');");
    write(&dir.join("types.d.ts"), "import type { T } from 'types-only';");
    let report = crate::analyze::analyze_for_repair(&dir, Scope::Declared).unwrap().unwrap();
    let names: Vec<_> = report.findings.iter().map(|finding| finding.dependency.as_str()).collect();
    assert_eq!(names, ["browser-dependency", "crypto-polyfill", "server-dependency", "types-only"]);
    let evidence = report
        .findings
        .iter()
        .find(|finding| finding.dependency == "crypto-polyfill")
        .unwrap()
        .evidence
        .first()
        .unwrap();
    assert_eq!((evidence.file.as_str(), evidence.specifier.as_str()), ("index.js", "crypto"));
}

#[test]
fn browser_mapping_of_package_main_is_followed_for_repair() {
    use crate::fixtures::{manifest, write};
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("pkg");
    write(
        &dir.join("package.json"),
        &manifest(
            "pkg",
            r#","main":"server.js","types":"types.d.ts","browser":{"./server.js":"./browser.js","./unused.js":"./other.js","./types.d.ts":"./other.d.ts"}"#,
        ),
    );
    write(&dir.join("server.js"), "require('node-dependency');");
    write(&dir.join("browser.js"), "require('browser-dependency');");
    write(&dir.join("unused.js"), "module.exports = 1;");
    write(&dir.join("other.js"), "require('unused-dependency');");
    write(&dir.join("types.d.ts"), "export declare const value: string;");
    write(&dir.join("other.d.ts"), "import 'not-a-type-replacement';");
    let report = crate::analyze::analyze_for_repair(&dir, Scope::Declared).unwrap().unwrap();
    let names: Vec<_> = report.findings.iter().map(|finding| finding.dependency.as_str()).collect();
    assert_eq!(names, ["browser-dependency", "node-dependency"]);
    assert_eq!(report.findings[0].evidence.first().unwrap().file, "browser.js");
}

#[test]
fn findings_include_all_precise_source_references_and_import_aliases() {
    use crate::fixtures::{manifest, write};
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("pkg");
    write(
        &dir.join("package.json"),
        &manifest(
            "pkg",
            r##","exports":"./index.js","types":"./index.d.ts","imports":{"#dep":"missing"}"##,
        ),
    );
    let script = "import a from 'missing';\nrequire('#dep');\nimport('missing');\n";
    let types = "/// <reference types=\"missing\" />\nimport type { A } from 'missing';\nexport =       function entry(): void;\n";
    write(&dir.join("index.js"), script);
    write(&dir.join("index.d.ts"), types);
    let report = analyze(&dir, Scope::Declared).unwrap().unwrap();
    let missing = report.findings.iter().find(|finding| finding.dependency == "missing").unwrap();
    assert_eq!(missing.origin, Origin::Both);
    let locations: Vec<_> = missing
        .evidence
        .iter()
        .map(|item| {
            (item.file.as_str(), item.line, item.column, item.specifier.as_str(), item.origin)
        })
        .collect();
    assert_eq!(
        locations,
        [
            ("index.d.ts", 2, 24, "missing", Origin::Types),
            ("index.js", 1, 15, "missing", Origin::Runtime),
            ("index.js", 2, 9, "#dep", Origin::Runtime),
            ("index.js", 3, 8, "missing", Origin::Runtime),
        ],
    );
    let types_reference =
        report.findings.iter().find(|finding| finding.dependency == "@types/missing").unwrap();
    let reference = types_reference.evidence.first().unwrap();
    assert_eq!((reference.file.as_str(), reference.line, reference.column), ("index.d.ts", 1, 1));
    let json = crate::report::as_json(&[report], 1, &[]).unwrap();
    assert!(json.contains(r##""specifier": "#dep""##));
}

#[test]
fn types_only_import_uses_the_authors_dev_types_package() {
    use crate::fixtures::{manifest, write};
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("pkg");
    write(
        &dir.join("package.json"),
        &manifest("pkg", r#","devDependencies":{"@types/example":"^2"}"#),
    );
    write(&dir.join("index.d.ts"), "import type { A } from 'example';");
    let report = analyze(&dir, Scope::Declared).unwrap().unwrap();
    assert_eq!(report.findings[0].dependency, "@types/example");
    assert_eq!(report.findings[0].declared_range.as_deref(), Some("^2"));
    assert_eq!(report.findings[0].evidence.first().unwrap().specifier, "example");
}

#[test]
fn stylesheet_entries_follow_local_partials_and_report_external_packages() {
    use crate::fixtures::{manifest, write};
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("pkg");
    write(
        &dir.join("package.json"),
        &manifest(
            "pkg",
            r#","exports":{"./style":"./style.scss"},"sass":"./style.scss","dependencies":{"declared":"1"}"#,
        ),
    );
    write(&dir.join("style.scss"), "@use 'tokens';\n@forward '@scope/missing/mixins';\n@import 'legacy';\n@use '~webpack-package/item';\n");
    write(&dir.join("_tokens.scss"), "@use 'declared/item';\n@use 'nested-missing/item';\n");
    write(&dir.join("_legacy.import.scss"), "@use 'legacy-missing/item';\n");
    write(&dir.join("_legacy.scss"), "@use 'wrong-legacy/item';\n");
    let report = crate::analyze::analyze_for_repair(&dir, Scope::Declared).unwrap().unwrap();
    let names: Vec<_> = report.findings.iter().map(|finding| finding.dependency.as_str()).collect();
    assert_eq!(names, ["@scope/missing", "legacy-missing", "nested-missing", "webpack-package"]);
    let evidence = report.findings[0].evidence.first().unwrap();
    assert_eq!(
        (evidence.file.as_str(), evidence.line, evidence.specifier.as_str()),
        ("style.scss", 2, "@scope/missing/mixins"),
    );
}

#[test]
fn ambient_nodejs_namespace_does_not_suggest_an_ineffective_package_extension() {
    use crate::fixtures::{manifest, write};
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("pkg");
    write(
        &dir.join("package.json"),
        &manifest("pkg", r#","types":"index.d.ts","devDependencies":{"@types/node":"^20"}"#),
    );
    write(&dir.join("index.d.ts"), "export declare const output: NodeJS.WriteStream;\n");
    assert!(analyze(&dir, Scope::Declared).unwrap().is_none());
}

#[test]
fn repair_scans_public_stylesheet_subpaths_without_manifest_entry() {
    use crate::fixtures::{manifest, write};
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("pkg");
    write(&dir.join("package.json"), &manifest("pkg", ""));
    write(&dir.join("component.import.scss"), "@forward '@scope/missing/mixins';");
    let report = crate::analyze::analyze_for_repair(&dir, Scope::Declared).unwrap().unwrap();
    assert_eq!(report.findings[0].dependency, "@scope/missing");
}

#[test]
fn sass_package_importer_is_resolved_as_a_package() {
    use crate::fixtures::{manifest, write};
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("pkg");
    write(&dir.join("package.json"), &manifest("pkg", r#","sass":"./style.scss""#));
    write(&dir.join("style.scss"), "@use 'pkg:@scope/library/theme';\n@use 'sass:math';\n");
    let report = crate::analyze::analyze_for_repair(&dir, Scope::Declared).unwrap().unwrap();
    assert_eq!(report.findings.len(), 1);
    assert_eq!(report.findings[0].dependency, "@scope/library");
    assert_eq!(report.findings[0].evidence.first().unwrap().specifier, "pkg:@scope/library/theme");
}

#[test]
fn repair_rejects_unparsable_source_even_when_other_findings_exist() {
    use crate::fixtures::{manifest, write};
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("pkg");
    write(&dir.join("package.json"), &manifest("pkg", r#","exports":"./index.js""#));
    write(&dir.join("index.js"), "require('missing'); require('./broken.js');");
    write(&dir.join("broken.js"), "export const = ;");
    assert!(analyze(&dir, Scope::Declared).unwrap().is_some());
    for scope in [Scope::Declared, Scope::EveryFile] {
        let error = crate::analyze::analyze_for_repair(&dir, scope).unwrap_err().to_string();
        assert!(error.contains("broken.js"), "{error}");
    }
}

#[test]
fn incomplete_scan_returns_structured_diagnostics_without_a_finding() {
    use crate::fixtures::{manifest, write};
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("pkg");
    write(&dir.join("package.json"), &manifest("pkg", ""));
    write(&dir.join("broken.js"), "export const = ;");
    let outcome = crate::analyze::analyze_with_diagnostics(&dir, Scope::Declared).unwrap();
    assert!(outcome.report.is_none());
    assert_eq!(outcome.diagnostics.len(), 1);
    let diagnostic = &outcome.diagnostics[0];
    assert_eq!(diagnostic.package, "pkg@1.0.0");
    assert_eq!(diagnostic.file, "broken.js");
    assert_eq!(diagnostic.kind, crate::report::DiagnosticKind::ParseError);
    assert!(!diagnostic.message.is_empty());
}

#[test]
fn repair_without_exports_follows_entries_not_unrelated_source() {
    use crate::fixtures::{manifest, write};
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("pkg");
    write(&dir.join("package.json"), &manifest("pkg", r#","main":"./index.js""#));
    write(&dir.join("index.js"), "require('missing');");
    write(&dir.join("src/unrelated.js"), "export const = ;");
    let report = crate::analyze::analyze_for_repair(&dir, Scope::Declared).unwrap().unwrap();
    assert_eq!(report.findings[0].dependency, "missing");
    assert!(crate::analyze::analyze_for_repair(&dir, Scope::EveryFile).is_err());
    write(&dir.join("index.js"), "require('missing'); require('./src/unrelated.js');");
    assert!(crate::analyze::analyze_for_repair(&dir, Scope::Declared).is_err());
}

#[test]
fn repair_scans_flow_files_exported_by_wildcard() {
    use crate::fixtures::{manifest, write};
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("pkg");
    write(
        &dir.join("package.json"),
        &manifest("pkg", r#","exports":{".":"./index.js","./*":"./*"}"#),
    );
    write(&dir.join("index.js"), "module.exports = 1;");
    write(
        &dir.join("source.js"),
        "// @flow\nimport type { Foo } from 'missing';\nexport type X = {| +x: Foo |};",
    );
    let report = crate::analyze::analyze_for_repair(&dir, Scope::Declared).unwrap().unwrap();
    assert_eq!(report.findings[0].dependency, "missing");
    assert_eq!(report.findings[0].origin, Origin::Types);
}

#[test]
fn unreadable_source_cannot_be_reported_as_clean() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("package.json"), r#"{"name":"example","version":"1.0.0"}"#)
        .unwrap();
    std::fs::write(root.path().join("index.js"), [0xff, 0xfe]).unwrap();
    let error = analyze(root.path(), Scope::Declared).unwrap_err().to_string();
    assert!(error.contains("reading source"), "{error}");
    assert!(error.contains("index.js"), "{error}");
}
