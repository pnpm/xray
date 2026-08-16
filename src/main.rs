mod manifest;
mod report;
mod scan;

use anyhow::{bail, Result};
use clap::Parser;
use manifest::Manifest;
use report::{Finding, PackageReport, Severity};
use scan::Requirement;
use std::{
    collections::{BTreeSet, HashSet},
    path::{Path, PathBuf},
};
use walkdir::WalkDir;

/// Packages published to npm routinely import things they never declared. Under
/// a hoisted `node_modules` those imports resolve by accident; under a strict or
/// global virtual store they do not. This finds them before your users do.
#[derive(Debug, Parser)]
#[command(name = "xray", version)]
struct Args {
    /// A project whose installed tree should be scanned, or a single package
    /// directory when `--package` is given.
    #[arg(default_value = ".")]
    path: PathBuf,

    /// Treat `path` as one package directory instead of a project to walk.
    #[arg(long)]
    package: bool,

    /// Report the findings as JSON.
    #[arg(long)]
    json: bool,

    /// Emit a `packageExtensions` block that declares every finding as an
    /// optional peer dependency, ready to paste into pnpm-workspace.yaml.
    #[arg(long)]
    package_extensions: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let packages =
        if args.package { vec![args.path.clone()] } else { installed_packages(&args.path)? };

    let mut reports: Vec<PackageReport> =
        packages.iter().filter_map(|dir| analyze(dir).transpose()).collect::<Result<_>>()?;
    reports.sort_by(|a, b| a.package.cmp(&b.package));

    let rendered = if args.json {
        report::as_json(&reports)?
    } else if args.package_extensions {
        report::as_package_extensions(&reports)
    } else {
        report::as_text(&reports)
    };
    print!("{rendered}");

    Ok(())
}

fn analyze(package_dir: &Path) -> Result<Option<PackageReport>> {
    let manifest = Manifest::read(package_dir)?;
    let declared: HashSet<&str> = manifest.reachable().collect();

    let mut referenced = BTreeSet::new();
    let mut unparsed = Vec::new();
    for file in declaration_files(package_dir) {
        let Ok(source) = std::fs::read_to_string(&file) else {
            continue;
        };
        match scan::specifiers(&source) {
            Ok(found) => referenced.extend(found),
            Err(reason) => unparsed.push((file, reason)),
        }
    }
    for (file, reason) in &unparsed {
        eprintln!("warning: could not parse {}: {reason}", file.display());
    }

    let findings: Vec<Finding> = referenced
        .iter()
        .filter_map(|requirement| missing_package(requirement, &declared))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|name| Finding {
            declared_range: manifest.dev_dependencies.get(&name).cloned(),
            severity: if manifest.dev_dependencies.contains_key(&name) {
                Severity::DevDependency
            } else {
                Severity::Undeclared
            },
            dependency: name,
        })
        .collect();

    if findings.is_empty() {
        return Ok(None);
    }
    Ok(Some(PackageReport { package: manifest.id(), findings }))
}

fn declaration_files(package_dir: &Path) -> impl Iterator<Item = PathBuf> {
    WalkDir::new(package_dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| entry.file_name() != "node_modules")
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.into_path())
        .filter(|path| {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            name.ends_with(".d.ts") || name.ends_with(".d.mts") || name.ends_with(".d.cts")
        })
}

/// The package that has to be installed for a requirement to resolve, or `None`
/// when the package already declared something that satisfies it.
fn missing_package(requirement: &Requirement, declared: &HashSet<&str>) -> Option<String> {
    match requirement {
        // A bare specifier can also be satisfied by a types package that declares
        // the module ambiently, the way `@types/estree` declares `estree`.
        Requirement::Module(specifier) => {
            let name = package_name(specifier)?;
            let satisfied =
                declared.contains(name) || declared.contains(types_package(name).as_str());
            (!satisfied).then(|| name.to_string())
        }
        // `/// <reference types="x" />` is satisfied by `@types/x` or by `x`
        // itself when that package ships its own declarations.
        Requirement::TypesReference(name) => {
            let types_package = types_package(name);
            let satisfied =
                declared.contains(name.as_str()) || declared.contains(types_package.as_str());
            (!satisfied).then_some(types_package)
        }
    }
}

fn types_package(name: &str) -> String {
    match name.strip_prefix('@') {
        Some(scoped) => format!("@types/{}", scoped.replacen('/', "__", 1)),
        None => format!("@types/{name}"),
    }
}

/// The package a specifier belongs to, or `None` when nothing needs to be
/// installed for it to resolve: relative paths, subpath imports, protocol URLs
/// and Node builtins.
fn package_name(specifier: &str) -> Option<&str> {
    if specifier.starts_with(['.', '/', '#']) || specifier.is_empty() {
        return None;
    }
    if let Some((scheme, _)) = specifier.split_once(':') {
        if !scheme.starts_with('@') {
            return None;
        }
    }

    let mut segments = specifier.split('/');
    let first = segments.next()?;
    let name = if first.starts_with('@') {
        let scope_end = first.len() + 1 + segments.next()?.len();
        &specifier[..scope_end]
    } else {
        first
    };

    if NODE_BUILTINS.contains(&name) {
        return None;
    }
    Some(name)
}

const NODE_BUILTINS: &[&str] = &[
    "assert",
    "async_hooks",
    "buffer",
    "child_process",
    "cluster",
    "console",
    "constants",
    "crypto",
    "dgram",
    "diagnostics_channel",
    "dns",
    "domain",
    "events",
    "fs",
    "http",
    "http2",
    "https",
    "inspector",
    "module",
    "net",
    "os",
    "path",
    "perf_hooks",
    "process",
    "punycode",
    "querystring",
    "readline",
    "repl",
    "sea",
    "sqlite",
    "stream",
    "string_decoder",
    "sys",
    "test",
    "timers",
    "tls",
    "trace_events",
    "tty",
    "url",
    "util",
    "v8",
    "vm",
    "wasi",
    "worker_threads",
    "zlib",
];

/// Every package reachable from the project, found by walking `node_modules`
/// directories through their real paths. A package's own dependencies sit in the
/// same directory it does, so following each one to where it really lives and
/// reading that directory again covers the whole graph — under a project-local
/// virtual store and a global one alike.
fn installed_packages(root: &Path) -> Result<Vec<PathBuf>> {
    let root_modules = root.join("node_modules");
    if !root_modules.is_dir() {
        bail!(
            "{} has no installed dependencies — run an install first, or pass --package",
            root.display()
        );
    }

    let mut queue = vec![root_modules.clone(), root_modules.join(".pnpm/node_modules")];
    let mut visited_dirs = HashSet::new();
    let mut packages = BTreeSet::new();

    while let Some(dir) = queue.pop() {
        let Ok(dir) = dir.canonicalize() else { continue };
        if !dir.is_dir() || !visited_dirs.insert(dir.clone()) {
            continue;
        }
        for package_dir in package_dirs_in(&dir) {
            let Ok(real) = package_dir.canonicalize() else { continue };
            if !real.join("package.json").is_file() {
                continue;
            }
            if let Some(modules_dir) = nearest_modules_dir(&real) {
                queue.push(modules_dir);
            }
            packages.insert(real);
        }
    }
    Ok(packages.into_iter().collect())
}

fn package_dirs_in(modules_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(modules_dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name() != ".pnpm" && entry.file_name() != ".bin")
        .flat_map(|entry| {
            if entry.file_name().to_string_lossy().starts_with('@') {
                std::fs::read_dir(entry.path())
                    .map(|scoped| scoped.filter_map(Result::ok).map(|e| e.path()).collect())
                    .unwrap_or_default()
            } else {
                vec![entry.path()]
            }
        })
        .collect()
}

fn nearest_modules_dir(package_dir: &Path) -> Option<PathBuf> {
    package_dir
        .ancestors()
        .find(|ancestor| ancestor.file_name().is_some_and(|name| name == "node_modules"))
        .map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn manifest(name: &str, extra: &str) -> String {
        format!(r#"{{"name":"{name}","version":"1.0.0"{extra}}}"#)
    }

    /// A project whose packages really live elsewhere, the shape a global
    /// virtual store produces: `node_modules` holds links, and each store entry
    /// holds the package next to the dependencies it may reach.
    #[cfg(unix)]
    fn global_virtual_store() -> tempfile::TempDir {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        let store = root.path().join("store");

        let app = store.join("app/node_modules/app");
        write(&app.join("package.json"), &manifest("app", r#","devDependencies":{"ghost":"^1"}"#));
        write(&app.join("index.d.ts"), "import type { G } from 'ghost';\nexport type { G };\n");
        // Bundled copies must not be mistaken for the package's own sources.
        write(&app.join("node_modules/vendored/index.d.ts"), "import 'not-yours';\n");

        let helper = store.join("helper/node_modules/helper");
        write(&helper.join("package.json"), &manifest("helper", ""));
        write(&helper.join("index.d.ts"), "export declare const ok: boolean;\n");

        symlink(&helper, store.join("app/node_modules/helper")).unwrap();
        fs::create_dir_all(project.join("node_modules")).unwrap();
        symlink(&app, project.join("node_modules/app")).unwrap();

        root
    }

    #[cfg(unix)]
    #[test]
    fn walks_from_the_project_into_every_store_entry() {
        let root = global_virtual_store();
        let found = installed_packages(&root.path().join("project")).unwrap();
        let names: Vec<_> =
            found.iter().map(|dir| dir.file_name().unwrap().to_str().unwrap()).collect();
        assert_eq!(names, ["app", "helper"]);
    }

    #[cfg(unix)]
    #[test]
    fn reports_a_dev_dependency_that_leaks_into_declarations() {
        let root = global_virtual_store();
        let app = root.path().join("store/app/node_modules/app");
        let report = analyze(&app).unwrap().expect("app should have a finding");
        assert_eq!(report.package, "app@1.0.0");
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].dependency, "ghost");
        assert_eq!(report.findings[0].severity, Severity::DevDependency);
        assert_eq!(report.findings[0].declared_range.as_deref(), Some("^1"));
    }

    #[cfg(unix)]
    #[test]
    fn a_package_that_declares_what_it_imports_is_not_reported() {
        let root = global_virtual_store();
        let helper = root.path().join("store/helper/node_modules/helper");
        assert!(analyze(&helper).unwrap().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn declarations_bundled_under_node_modules_are_not_the_package_own() {
        let root = global_virtual_store();
        let app = root.path().join("store/app/node_modules/app");
        let scanned: Vec<_> = declaration_files(&app).collect();
        assert_eq!(scanned.len(), 1, "{scanned:?}");
        assert!(scanned[0].ends_with("app/index.d.ts"), "{scanned:?}");
    }

    fn declared<'a>(names: &'a [&'a str]) -> HashSet<&'a str> {
        names.iter().copied().collect()
    }

    #[test]
    fn takes_the_package_out_of_a_specifier() {
        assert_eq!(package_name("lodash"), Some("lodash"));
        assert_eq!(package_name("lodash/fp"), Some("lodash"));
        assert_eq!(package_name("@medplum/core"), Some("@medplum/core"));
        assert_eq!(package_name("pdfmake/interfaces"), Some("pdfmake"));
        assert_eq!(package_name("@scope/name/deep/path"), Some("@scope/name"));
    }

    #[test]
    fn ignores_specifiers_that_need_nothing_installed() {
        for specifier in [
            "./relative",
            "../up",
            "/absolute",
            "#subpath-import",
            "node:fs",
            "fs",
            "worker_threads",
            "data:text/javascript,export{}",
            "https://example.com/mod.js",
            "",
        ] {
            assert_eq!(package_name(specifier), None, "{specifier} should need no package");
        }
    }

    #[test]
    fn a_scope_on_its_own_is_not_a_package() {
        assert_eq!(package_name("@scope"), None);
    }

    #[test]
    fn maps_a_name_to_its_types_package() {
        assert_eq!(types_package("node"), "@types/node");
        assert_eq!(types_package("@medplum/core"), "@types/medplum__core");
    }

    #[test]
    fn an_import_is_satisfied_by_the_types_package_that_declares_it() {
        let requirement = Requirement::Module("estree".to_string());
        assert_eq!(missing_package(&requirement, &declared(&["@types/estree"])), None);
        assert_eq!(missing_package(&requirement, &declared(&[])), Some("estree".to_string()));
    }

    #[test]
    fn a_types_reference_is_satisfied_by_either_spelling() {
        let requirement = Requirement::TypesReference("node".to_string());
        assert_eq!(missing_package(&requirement, &declared(&["@types/node"])), None);
        assert_eq!(missing_package(&requirement, &declared(&["node"])), None);
    }

    #[test]
    fn an_unsatisfied_types_reference_is_reported_under_the_types_name() {
        let requirement = Requirement::TypesReference("estree".to_string());
        assert_eq!(
            missing_package(&requirement, &declared(&[])),
            Some("@types/estree".to_string())
        );
    }
}
