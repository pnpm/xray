//! Observe failed Node module resolutions while a command runs.
use crate::{
    classify::package_name,
    manifest::Manifest,
    report::{Evidence, Finding, PackageReport, Severity},
    scan::Origin,
};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
};

#[derive(Deserialize)]
struct Event {
    specifier: String,
    parent: PathBuf,
}

pub struct TraceResult {
    pub status: ExitStatus,
    pub reports: Vec<PackageReport>,
}

/// Run a command with the Node preload active. Its streams and exit status are
/// unchanged. Every captured resolution failure may have been caught by code.
pub fn run(command: &[OsString], packages: &[PathBuf], project: &Path) -> Result<TraceResult> {
    let (program, args) = command.split_first().context("--trace needs a command after --")?;
    let temp = tempfile::tempdir().context("creating trace directory")?;
    let hook = temp.path().join("hook.mjs");
    let events = temp.path().join("events.jsonl");
    std::fs::write(&hook, include_str!("trace-hook.mjs"))?;
    std::fs::write(&events, "")?;
    let mut options = std::env::var_os("NODE_OPTIONS").unwrap_or_default();
    if !options.is_empty() {
        options.push(" ");
    }
    options.push(format!("--import={}", file_url(&hook)?));
    let status = Command::new(program)
        .args(args)
        .current_dir(project)
        .env("NODE_OPTIONS", options)
        .env("XRAY_TRACE_FILE", &events)
        .status()
        .context("starting traced command")?;
    let reports = reports_from_events(&std::fs::read_to_string(events)?, packages)?;
    Ok(TraceResult { status, reports })
}

// Escape spaces and non-ASCII bytes so NODE_OPTIONS doesn't split the URL.
fn file_url(path: &Path) -> Result<String> {
    let path = path.to_str().context("trace hook path is not valid UTF-8")?;
    let path = if cfg!(windows) { path.replace('\\', "/") } else { path.to_string() };
    let mut url = String::from("file://");
    if !path.starts_with('/') {
        url.push('/');
    }
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~:".contains(&byte) {
            url.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            let _ = write!(url, "%{byte:02X}");
        }
    }
    Ok(url)
}

fn reports_from_events(raw: &str, packages: &[PathBuf]) -> Result<Vec<PackageReport>> {
    let mut roots = Vec::new();
    for package in packages {
        let path = package.canonicalize()?;
        // Workspace packages resolve to their source directories, while an
        // installed package resides under a node_modules directory even in the
        // global virtual store. Never attribute workspace failures to packages.
        if !path.components().any(|component| component.as_os_str() == "node_modules") {
            continue;
        }
        let manifest = Manifest::read(&path)?;
        roots.push((path, manifest));
    }
    roots.sort_by_key(|(path, _)| std::cmp::Reverse(path.components().count()));
    let mut reports: BTreeMap<String, BTreeMap<String, Finding>> = BTreeMap::new();
    for line in raw.lines() {
        let event: Event = serde_json::from_str(line).context("reading Node trace event")?;
        let Some(name) = package_name(&event.specifier) else { continue };
        let Ok(parent) = event.parent.canonicalize() else { continue };
        let Some((root, manifest)) = roots.iter().find(|(path, _)| parent.starts_with(path)) else {
            continue;
        };
        // A declared package can still fail due to a missing subpath or install;
        // a phantom-dependency extension is not the right remedy for that.
        if manifest.reachable().any(|declared| declared == name) {
            continue;
        }
        let range = manifest.dev_dependencies.get(name).cloned();
        let severity = if range.is_some() { Severity::DevDependency } else { Severity::Undeclared };
        let finding =
            reports.entry(manifest.id()).or_default().entry(name.to_string()).or_insert_with(
                || Finding {
                    dependency: name.to_string(),
                    declared_range: range,
                    severity,
                    origin: Origin::Runtime,
                    evidence: BTreeSet::new(),
                },
            );
        let relative = parent.strip_prefix(root).unwrap_or(&parent).to_string_lossy().into_owned();
        finding.evidence.insert(Evidence {
            file: relative,
            line: 0,
            column: 0,
            specifier: event.specifier,
            origin: Origin::Runtime,
        });
    }
    Ok(reports
        .into_iter()
        .map(|(package, findings)| PackageReport {
            package,
            findings: findings.into_values().collect(),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observes_caught_dynamic_resolution_failures_from_installed_packages() {
        let supported = Command::new("node")
            .args([
                "-e",
                "process.exit(typeof require('node:module').registerHooks === 'function' ? 0 : 1)",
            ])
            .status()
            .is_ok_and(|status| status.success());
        if !supported {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let package = temp.path().join("node_modules/example");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(
            package.join("package.json"),
            r#"{"name":"example","version":"1.0.0","devDependencies":{"missing-runtime":"^2"}}"#,
        )
        .unwrap();
        std::fs::write(
            package.join("index.cjs"),
            "const dep = 'missing-' + 'runtime'; try { require(dep); } catch {}\n",
        )
        .unwrap();
        let result = run(
            &["node".into(), package.join("index.cjs").into_os_string()],
            &[package],
            temp.path(),
        )
        .unwrap();
        assert!(result.status.success());
        assert_eq!(result.reports.len(), 1);
        let finding = &result.reports[0].findings[0];
        assert_eq!(finding.dependency, "missing-runtime");
        assert_eq!(finding.declared_range.as_deref(), Some("^2"));
        assert_eq!(finding.evidence.first().unwrap().file, "index.cjs");
    }

    #[test]
    fn ignores_workspace_imports_and_declared_dependencies() {
        let temp = tempfile::tempdir().unwrap();
        let package = temp.path().join("node_modules/example");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(
            package.join("package.json"),
            r#"{"name":"example","version":"1.0.0","dependencies":{"declared":"1"}}"#,
        )
        .unwrap();
        let installed_file = package.join("index.js");
        let workspace_file = temp.path().join("app.js");
        std::fs::write(&installed_file, "").unwrap();
        std::fs::write(&workspace_file, "").unwrap();
        let raw = format!(
            "{}\n{}\n",
            serde_json::json!({"specifier":"declared", "parent":installed_file}),
            serde_json::json!({"specifier":"missing", "parent":workspace_file})
        );
        assert!(reports_from_events(&raw, &[package]).unwrap().is_empty());
    }
    #[test]
    fn preserves_nonzero_exit_status_and_observes_dynamic_esm_import() {
        if !Command::new("node")
            .args([
                "-e",
                "process.exit(typeof require('node:module').registerHooks === 'function' ? 0 : 1)",
            ])
            .status()
            .is_ok_and(|s| s.success())
        {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let package = temp.path().join("node_modules/example");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(package.join("package.json"), r#"{"name":"example","version":"1.0.0"}"#)
            .unwrap();
        std::fs::write(
            package.join("index.mjs"),
            "try { await import('missing-' + 'esm'); } catch {} process.exitCode = 7;\n",
        )
        .unwrap();
        let result = run(
            &["node".into(), package.join("index.mjs").into_os_string()],
            &[package],
            temp.path(),
        )
        .unwrap();
        assert_eq!(result.status.code(), Some(7));
        assert_eq!(result.reports[0].findings[0].dependency, "missing-esm");
    }
    #[test]
    fn child_command_runs_from_project_directory() {
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("marker");
        std::fs::write(marker, "").unwrap();
        let result = run(
            &[
                "node".into(),
                "-e".into(),
                "process.exit(require('fs').existsSync('marker') ? 0 : 8)".into(),
            ],
            &[],
            temp.path(),
        )
        .unwrap();
        assert!(result.status.success());
    }
}
