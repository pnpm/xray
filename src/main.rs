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
