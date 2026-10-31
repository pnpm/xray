use super::{direct_types_providers, exact_version, has_types, select};
use crate::{
    manifest::Manifest,
    report::{Evidence, Finding, PackageReport, Severity},
    scan::Origin,
};
use std::{collections::BTreeSet, path::PathBuf};

fn package(root: &std::path::Path, dir: &str, name: &str, version: &str) -> PathBuf {
    let path = root.join(dir);
    std::fs::create_dir_all(&path).unwrap();
    std::fs::write(
        path.join("package.json"),
        serde_json::json!({"name": name, "version": version}).to_string(),
    )
    .unwrap();
    path
}

fn finding(name: &str, origin: Origin, range: Option<&str>) -> Finding {
    Finding {
        dependency: name.into(),
        declared_range: range.map(str::to_string),
        severity: Severity::Undeclared,
        origin,
        evidence: BTreeSet::from([Evidence {
            file: "index.d.ts".into(),
            line: 1,
            column: 1,
            specifier: name.into(),
            origin,
        }]),
    }
}

#[test]
fn selects_installed_providers_and_unique_types_while_preserving_evidence() {
    let root = tempfile::tempdir().unwrap();
    let packages = vec![
        package(root.path(), "types", "@types/estree", "1.0.8"),
        package(root.path(), "pug", "pug", "3.0.4"),
        package(root.path(), "node-types", "@types/node", "24.19.2"),
    ];
    let reports = vec![PackageReport {
        package: "consumer@1.0.0".into(),
        findings: vec![
            finding("estree", Origin::Types, None),
            finding("pug", Origin::Runtime, Some("^3")),
            finding("uninstalled-template", Origin::Runtime, None),
            finding("authored", Origin::Types, Some("^2.0.0")),
            finding("@types/node", Origin::Types, Some("^20.0.0")),
        ],
    }];
    let result = select(&reports, &packages, root.path()).unwrap();
    let found = &result[0].findings;
    assert_eq!(found.len(), 4);
    assert_eq!(
        (&*found[0].dependency, found[0].declared_range.as_deref()),
        ("@types/estree", Some("1.0.8")),
    );
    assert_eq!(found[0].evidence.first().unwrap().specifier, "estree");
    assert_eq!(found[1].dependency, "pug");
    assert!(found[1].declared_range.is_none());
    assert_eq!(found[2].declared_range.as_deref(), Some("^2.0.0"));
    assert_eq!(found[3].dependency, "@types/node");
    assert!(found[3].declared_range.is_none());
}

#[test]
fn does_not_guess_when_types_versions_disagree_or_provider_is_absent() {
    let root = tempfile::tempdir().unwrap();
    let packages = vec![
        package(root.path(), "one", "@types/estree", "1.0.8"),
        package(root.path(), "two", "@types/estree", "1.0.9"),
    ];
    let reports = vec![PackageReport {
        package: "consumer@1.0.0".into(),
        findings: vec![
            finding("estree", Origin::Types, None),
            finding("alias", Origin::Runtime, None),
        ],
    }];
    assert!(select(&reports, &packages, root.path()).unwrap().is_empty());
}

#[test]
fn selects_only_verified_installed_npm_aliases() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("package.json"), r#"{"dependencies":{"alias-provider":"npm:real-provider@^7.0.0","wrong-name":"npm:expected@^1","wrong-range":"npm:real-provider@^8","pre-release":"npm:preview@*"}}"#).unwrap();
    let packages = vec![
        package(root.path(), "node_modules/alias-provider", "real-provider", "7.0.0"),
        package(root.path(), "node_modules/wrong-name", "other", "1.0.0"),
        package(root.path(), "node_modules/wrong-range", "real-provider", "7.0.0"),
        package(root.path(), "node_modules/pre-release", "preview", "1.0.0-beta.1"),
    ];
    let reports = vec![PackageReport {
        package: "consumer@1.0.0".into(),
        findings: ["alias-provider", "wrong-name", "wrong-range", "pre-release"]
            .into_iter()
            .map(|name| finding(name, Origin::Runtime, None))
            .collect(),
    }];
    let selected = select(&reports, &packages, root.path()).unwrap();
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].findings.len(), 1);
    assert_eq!(selected[0].findings[0].dependency, "alias-provider");
    assert_eq!(selected[0].findings[0].declared_range, None);
    assert!(select(&reports, &packages[1..], root.path()).unwrap().is_empty());
}

#[test]
fn npm_alias_parser_accepts_scopes_and_rejects_nonportable_specs() {
    let (name, range) = super::npm_alias("npm:@scope/real@^2.0.0").unwrap();
    assert_eq!(name, "@scope/real");
    assert!(range.unwrap().satisfies(&node_semver::Version::parse("2.1.0").unwrap()));
    assert_eq!(super::npm_alias("npm:@scope/real").unwrap().0, "@scope/real");
    assert!(super::npm_alias("npm:real@latest").is_none());
    assert!(super::npm_alias("npm:../outside@1").is_none());
}

#[test]
fn selects_verified_prerelease_alias_with_matching_exact_request() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("package.json"),
        r#"{"dependencies":{"alias-provider":"npm:real-provider@2.0.0-rc.24"}}"#,
    )
    .unwrap();
    let provider =
        package(root.path(), "node_modules/alias-provider", "real-provider", "2.0.0-rc.24");
    let reports = vec![PackageReport {
        package: "consumer@1.0.0".into(),
        findings: vec![finding("alias-provider", Origin::Runtime, None)],
    }];
    let selected = select(&reports, &[provider], root.path()).unwrap();
    assert_eq!(selected[0].findings[0].dependency, "alias-provider");
    assert!(crate::report::as_package_extensions(&selected).contains("'alias-provider': '*'"));
}
#[test]
fn chooses_types_for_untyped_provider_but_not_one_that_ships_types() {
    let root = tempfile::tempdir().unwrap();
    let untyped = package(root.path(), "lodash", "lodash", "4.17.21");
    let typed = package(root.path(), "typed", "typed", "1.0.0");
    std::fs::write(typed.join("index.d.ts"), "export {};\n").unwrap();
    let packages = vec![
        untyped,
        typed,
        package(root.path(), "lodash-types", "@types/lodash", "4.17.0"),
        package(root.path(), "typed-types", "@types/typed", "1.0.0"),
        package(root.path(), "scoped-types", "@types/example__widget", "2.1.0"),
    ];
    let reports = vec![PackageReport {
        package: "consumer@1.0.0".into(),
        findings: vec![
            finding("lodash", Origin::Types, None),
            finding("typed", Origin::Types, None),
            finding("@example/widget", Origin::Types, None),
        ],
    }];
    let result = select(&reports, &packages, root.path()).unwrap();
    let found = &result[0].findings;
    assert_eq!(found[0].dependency, "@types/lodash");
    assert_eq!(found[0].declared_range.as_deref(), Some("4.17.0"));
    assert_eq!(found[1].dependency, "typed");
    assert!(found[1].declared_range.is_none());
    assert_eq!(found[2].dependency, "@types/example__widget");
}

#[test]
fn detects_exported_type_conditions() {
    let root = tempfile::tempdir().unwrap();
    let path = package(root.path(), "typed", "typed", "1.0.0");
    std::fs::write(path.join("package.json"), r#"{ "name": "typed", "version": "1.0.0", "exports": { ".": { "types": "./types/index.d.ts", "default": "./index.js" } } }"#).unwrap();
    assert!(has_types(&Manifest::read(&path).unwrap(), &path));
}
#[test]
fn direct_workspace_provider_disambiguates_but_conflicting_importers_do_not() {
    let root = tempfile::tempdir().unwrap();
    let importer = root.path().join("packages/app");
    std::fs::create_dir_all(importer.join("node_modules/@types/estree")).unwrap();
    std::fs::write(
        root.path().join("pnpm-lock.yaml"),
        "lockfileVersion: '9.0'\nimporters:\n  packages/app: {}\n",
    )
    .unwrap();
    std::fs::write(
        importer.join("package.json"),
        r#"{"name":"app","version":"1.0.0","devDependencies":{"@types/estree":"1.0.8"}}"#,
    )
    .unwrap();
    std::fs::write(
        importer.join("node_modules/@types/estree/package.json"),
        r#"{"name":"@types/estree","version":"1.0.8"}"#,
    )
    .unwrap();
    let packages = vec![
        package(root.path(), "one", "@types/estree", "1.0.8"),
        package(root.path(), "two", "@types/estree", "1.0.9"),
    ];
    let reports = vec![PackageReport {
        package: "consumer@1.0.0".into(),
        findings: vec![finding("estree", Origin::Types, None)],
    }];
    let found = select(&reports, &packages, root.path()).unwrap();
    assert_eq!(found[0].findings[0].declared_range.as_deref(), Some("1.0.8"));
    let other = root.path().join("packages/other");
    std::fs::create_dir_all(other.join("node_modules/@types/estree")).unwrap();
    std::fs::write(
        other.join("package.json"),
        r#"{"name":"other","version":"1.0.0","dependencies":{"@types/estree":"1.0.9"}}"#,
    )
    .unwrap();
    std::fs::write(
        other.join("node_modules/@types/estree/package.json"),
        r#"{"name":"@types/estree","version":"1.0.9"}"#,
    )
    .unwrap();
    std::fs::write(
        root.path().join("pnpm-lock.yaml"),
        "lockfileVersion: '9.0'\nimporters:\n  packages/app: {}\n  packages/other: {}\n",
    )
    .unwrap();
    assert!(select(&reports, &packages, root.path()).unwrap().is_empty());
}
#[test]
fn permits_concrete_prerelease_but_not_a_range() {
    assert!(exact_version("1.2.3-beta.1"));
    assert!(!exact_version("^1.2.3"));
    assert!(!exact_version("*"));
}

#[test]
fn installed_prerelease_runtime_provider_remains_an_optional_peer() {
    let root = tempfile::tempdir().unwrap();
    let provider = package(root.path(), "provider", "runtime-provider", "2.0.0-rc.24");
    let reports = vec![PackageReport {
        package: "consumer@1.0.0".into(),
        findings: vec![finding("runtime-provider", Origin::Runtime, None)],
    }];
    let selected = select(&reports, &[provider], root.path()).unwrap();
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].findings[0].dependency, "runtime-provider");
    assert!(selected[0].findings[0].declared_range.is_none());
    let rendered = crate::report::as_package_extensions(&selected);
    assert!(rendered.contains("'runtime-provider': '*'"));
    assert!(rendered.contains("optional: true"));
}

#[test]
fn separate_lockfile_workspaces_supply_direct_type_providers() {
    let root = tempfile::tempdir().unwrap();
    let member = root.path().join("packages/member");
    std::fs::create_dir_all(&member).unwrap();
    std::fs::write(
        root.path().join("pnpm-workspace.yaml"),
        "packages: [packages/*]\nsharedWorkspaceLockfile: false\n",
    )
    .unwrap();
    std::fs::write(member.join("package.json"), r#"{"dependencies":{"@types/estree":"1.0.8"}}"#)
        .unwrap();
    package(&member, "node_modules/@types/estree", "@types/estree", "1.0.8");
    let providers = direct_types_providers(root.path()).unwrap();
    assert_eq!(providers["@types/estree"], BTreeSet::from(["1.0.8".into()]));
}

#[test]
fn source_and_companion_type_surfaces_are_not_replaced_by_ambient_types() {
    let root = tempfile::tempdir().unwrap();
    for (index, extra, file) in [
        (0, serde_json::json!({"main":"dist/main.mjs"}), "dist/main.d.mts"),
        (1, serde_json::json!({"exports":"./source.ts"}), "source.ts"),
        (2, serde_json::json!({"typesVersions":{"*":{"*":["types/*"]}}}), "types/index.d.ts"),
    ] {
        let path = root.path().join(index.to_string());
        let file_path = path.join(file);
        std::fs::create_dir_all(file_path.parent().unwrap()).unwrap();
        std::fs::write(file_path, "export {};").unwrap();
        let mut manifest = serde_json::json!({"name":"typed","version":"1.0.0"});
        manifest.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        std::fs::write(path.join("package.json"), manifest.to_string()).unwrap();
        assert!(has_types(&Manifest::read(&path).unwrap(), &path));
    }
}
