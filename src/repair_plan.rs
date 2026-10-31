//! Select extension entries supported by the installed dependency graph.

use crate::{
    classify::types_package,
    manifest::Manifest,
    report::{registry_range, PackageReport},
    scan::Origin,
};
use anyhow::Result;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

/// Limit automatic repairs to author-provided ranges and installed providers.
/// A `@types` mapping uses an exact version only when all installed copies agree.
pub fn select(
    reports: &[PackageReport],
    packages: &[PathBuf],
    project: &Path,
) -> Result<Vec<PackageReport>> {
    let direct = direct_types_providers(project)?;
    let mut providers: BTreeMap<String, Provider> = BTreeMap::new();
    for path in packages {
        let manifest = Manifest::read(path)?;
        if !manifest.name.is_empty() && !manifest.version.is_empty() {
            let typed = has_types(&manifest, path);
            let provider = providers.entry(manifest.name).or_default();
            provider.versions.insert(manifest.version);
            provider.typed |= typed;
        }
    }

    let mut selected = Vec::new();
    for report in reports {
        let mut findings = Vec::new();
        for finding in &report.findings {
            let mut finding = finding.clone();
            if finding.origin == Origin::Types
                && finding.declared_range.as_deref().is_some_and(registry_range)
            {
                findings.push(finding);
                continue;
            }
            let provider = providers.get(&finding.dependency);
            if finding.origin == Origin::Types
                && !finding.dependency.starts_with("@types/")
                && provider.is_none_or(|provider| !provider.typed)
            {
                let name = types_package(&finding.dependency);
                if let Some(types) = providers.get(&name) {
                    let direct_versions = direct.get(&name);
                    let versions = match direct_versions {
                        Some(versions) => versions,
                        None => &types.versions,
                    };
                    if versions.len() == 1 {
                        let version = versions.first().expect("one version exists");
                        if types.versions.contains(version) && exact_version(version) {
                            finding.dependency = name;
                            finding.declared_range = Some(version.clone());
                            findings.push(finding);
                            continue;
                        }
                    }
                    // An ambiguous version is not safe to infer from.
                    continue;
                }
            }
            if provider.is_some() {
                // For a runtime or unknown dependency, preserve optional peer
                // behavior; its dev range is not necessarily a consumer range.
                finding.declared_range = None;
                findings.push(finding);
            }
        }
        if !findings.is_empty() {
            selected.push(PackageReport { package: report.package.clone(), findings });
        }
    }
    Ok(selected)
}

// Prefer a provider explicitly declared by a workspace importer. If importers
// declare different versions there is no single safe answer for a global fix.
fn direct_types_providers(root: &Path) -> Result<BTreeMap<String, BTreeSet<String>>> {
    let mut importers = BTreeSet::from([root.to_path_buf()]);
    let lock = root.join("pnpm-lock.yaml");
    if lock.is_file() {
        let documents = yaml_rust2::YamlLoader::load_from_str(&std::fs::read_to_string(lock)?)?;
        let canonical_root = root.canonicalize()?;
        if let Some(map) = documents.first().and_then(|doc| doc["importers"].as_hash()) {
            for name in map.keys().filter_map(yaml_rust2::Yaml::as_str) {
                if let Ok(path) = root.join(name).canonicalize() {
                    if path.starts_with(&canonical_root) {
                        importers.insert(path);
                    }
                }
            }
        }
    }
    let mut direct = BTreeMap::<String, BTreeSet<String>>::new();
    for importer in importers {
        if !importer.join("package.json").is_file() {
            continue;
        }
        let manifest = Manifest::read(&importer)?;
        for name in manifest
            .dependencies
            .keys()
            .chain(manifest.dev_dependencies.keys())
            .chain(manifest.optional_dependencies.keys())
            .chain(manifest.peer_dependencies.keys())
            .filter(|name| name.starts_with("@types/"))
        {
            let provider_dir = importer.join("node_modules").join(name);
            if provider_dir.join("package.json").is_file() {
                let provider = Manifest::read(&provider_dir)?;
                if provider.name == *name {
                    direct.entry(name.clone()).or_default().insert(provider.version);
                }
            }
        }
    }
    Ok(direct)
}

#[derive(Default)]
struct Provider {
    versions: BTreeSet<String>,
    // If any installed variant advertises types, do not infer that the package
    // is untyped: different releases can have different declaration surfaces.
    typed: bool,
}

fn has_types(manifest: &Manifest, path: &Path) -> bool {
    fn has_declaration(value: &serde_json::Value) -> bool {
        match value {
            serde_json::Value::String(target) => {
                target.ends_with(".d.ts")
                    || target.ends_with(".d.mts")
                    || target.ends_with(".d.cts")
            }
            serde_json::Value::Array(values) => values.iter().any(has_declaration),
            serde_json::Value::Object(values) => {
                values.iter().any(|(key, value)| key == "types" || has_declaration(value))
            }
            _ => false,
        }
    }
    manifest.types.is_string()
        || manifest.typings.is_string()
        || path.join("index.d.ts").is_file()
        || has_declaration(&manifest.exports)
}

// Do not turn a malformed or non-concrete manifest version into an install range.
fn exact_version(version: &str) -> bool {
    node_semver::Version::parse(version).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::{Evidence, Finding, Severity};
    use std::collections::BTreeSet;

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
        ];
        let reports = vec![PackageReport {
            package: "consumer@1.0.0".into(),
            findings: vec![
                finding("estree", Origin::Types, None),
                finding("pug", Origin::Runtime, Some("^3")),
                finding("uninstalled-template", Origin::Runtime, None),
                finding("authored", Origin::Types, Some("^2.0.0")),
            ],
        }];
        let result = select(&reports, &packages, root.path()).unwrap();
        let found = &result[0].findings;
        assert_eq!(found.len(), 3);
        assert_eq!(
            (&*found[0].dependency, found[0].declared_range.as_deref()),
            ("@types/estree", Some("1.0.8"))
        );
        assert_eq!(found[0].evidence.first().unwrap().specifier, "estree");
        assert_eq!(found[1].dependency, "pug");
        assert!(found[1].declared_range.is_none());
        assert_eq!(found[2].declared_range.as_deref(), Some("^2.0.0"));
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
}
