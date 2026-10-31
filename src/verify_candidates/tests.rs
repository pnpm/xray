use super::{collect, compose, has_extensions, Candidate};

#[test]
fn proposals_without_extension_entries_are_not_candidates() {
    assert!(!has_extensions(""));
    assert!(!has_extensions("packageExtensions:\n"));
    assert!(!has_extensions("packageExtensions: {}\n"));
    assert!(has_extensions(
        "packageExtensions:\n  'example@1.0.0':\n    dependencies:\n      dep: '*'\n"
    ));
}
use crate::{
    analyze::Scope,
    report::{Finding, PackageReport, Severity},
    scan::Origin,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

fn package(root: &Path, dir: &str, name: &str, source: &str, types: Option<&str>) -> PathBuf {
    let path = root.join(dir);
    std::fs::create_dir_all(&path).unwrap();
    let mut manifest =
        serde_json::json!({"name": name, "version": "1.0.0", "exports": "./index.js"});
    if types.is_some() {
        manifest["types"] = "./index.d.ts".into();
    }
    std::fs::write(path.join("package.json"), manifest.to_string()).unwrap();
    std::fs::write(path.join("index.js"), source).unwrap();
    if let Some(types) = types {
        std::fs::write(path.join("index.d.ts"), types).unwrap();
    }
    path
}

fn finding(name: &str) -> Finding {
    Finding {
        dependency: name.into(),
        declared_range: None,
        severity: Severity::Undeclared,
        origin: Origin::Runtime,
        evidence: BTreeSet::new(),
    }
}

#[test]
fn ranks_observed_before_type_and_other_candidates_and_honors_existing_decisions() {
    let root = tempfile::tempdir().unwrap();
    let consumer = package(
        root.path(),
        "node_modules/consumer",
        "consumer",
        "require('runtime');",
        Some("import type { T } from 'type-provider';"),
    );
    let packages = vec![
        consumer,
        package(root.path(), "node_modules/runtime", "runtime", "module.exports = 1;", None),
        package(
            root.path(),
            "node_modules/type-provider",
            "type-provider",
            "module.exports = 1;",
            Some("export type T = string;"),
        ),
        package(root.path(), "node_modules/observed", "observed", "module.exports = 1;", None),
    ];
    let observed = vec![PackageReport {
        package: "consumer@1.0.0".into(),
        findings: vec![finding("observed")],
    }];
    let candidates =
        collect(root.path(), root.path(), &packages, &observed, Scope::Declared, false).unwrap();
    let keys: Vec<_> = candidates.iter().map(|candidate| candidate.key.as_str()).collect();
    assert_eq!(
        keys,
        [
            "extension:consumer@1.0.0:observed",
            "extension:consumer@1.0.0:type-provider",
            "extension:consumer@1.0.0:runtime",
        ],
    );
    std::fs::write(
        root.path().join("pnpm-workspace.yaml"),
        "packageExtensions:\n  'consumer@1.0.0':\n    peerDependencies:\n      observed: '*'\n",
    )
    .unwrap();
    let remaining =
        collect(root.path(), root.path(), &packages, &observed, Scope::Declared, false).unwrap();
    assert!(!remaining.iter().any(|candidate| candidate.key.ends_with(":observed")));
}

#[test]
fn excludes_observed_and_static_findings_when_any_same_version_copy_is_incomplete() {
    let root = tempfile::tempdir().unwrap();
    let good = package(root.path(), "one/consumer", "consumer", "require('runtime');", None);
    let broken = package(root.path(), "two/consumer", "consumer", "export const = ;", None);
    let provider =
        package(root.path(), "node_modules/runtime", "runtime", "module.exports = 1;", None);
    let observed = vec![PackageReport {
        package: "consumer@1.0.0".into(),
        findings: vec![finding("runtime")],
    }];
    assert!(collect(
        root.path(),
        root.path(),
        &[good, broken, provider],
        &observed,
        Scope::Declared,
        false
    )
    .unwrap()
    .is_empty());
}

#[test]
fn composes_extensions_and_patches_but_rejects_conflicting_artifacts_or_keys() {
    let extension = Candidate {
        key: "extension:example@1.0.0:helper".into(),
        configuration:
            "packageExtensions:\n  'example@1.0.0':\n    peerDependencies:\n      helper: '*'\n"
                .into(),
        artifacts: BTreeMap::new(),
    };
    let patch = Candidate {
        key: "patch:example@1.0.0".into(),
        configuration: "patchedDependencies:\n  'example@1.0.0': patches/example.patch\n".into(),
        artifacts: BTreeMap::from([("patches/example.patch".into(), "patch text".into())]),
    };
    let plan = compose(&[extension.clone(), patch.clone()]).unwrap();
    let yaml = yaml_rust2::YamlLoader::load_from_str(&plan.configuration).unwrap();
    assert_eq!(
        yaml[0]["packageExtensions"]["example@1.0.0"]["peerDependencies"]["helper"].as_str(),
        Some("*"),
    );
    assert_eq!(
        yaml[0]["patchedDependencies"]["example@1.0.0"].as_str(),
        Some("patches/example.patch"),
    );
    assert_eq!(plan.artifacts[Path::new("patches/example.patch")], "patch text");
    let mut conflicting = patch.clone();
    conflicting.key = "patch:other".into();
    conflicting.artifacts.insert("patches/example.patch".into(), "different".into());
    assert!(compose(&[patch, conflicting]).is_err());
    let mut duplicate = extension.clone();
    duplicate.configuration.push_str("# different\n");
    assert!(compose(&[extension, duplicate]).is_err());
}
