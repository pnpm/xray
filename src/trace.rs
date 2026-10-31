//! Observe failed Node module resolutions while a command runs.
use crate::{
    classify::package_name,
    manifest::Manifest,
    report::{Diagnostic, DiagnosticKind, Evidence, Finding, PackageReport, Severity},
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
    #[serde(default)]
    code: String,
    #[serde(default)]
    message: String,
}

pub struct TraceResult {
    pub status: ExitStatus,
    pub reports: Vec<PackageReport>,
    pub diagnostics: Vec<Diagnostic>,
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
    let (reports, diagnostics) = reports_from_events(&std::fs::read_to_string(events)?, packages)?;
    Ok(TraceResult { status, reports, diagnostics })
}

// Escape spaces and non-ASCII bytes so NODE_OPTIONS doesn't split the URL.
fn file_url(path: &Path) -> Result<String> {
    let path = path.to_str().context("trace hook path is not valid UTF-8")?;
    file_url_for_platform(path, cfg!(windows))
}

fn file_url_for_platform(path: &str, windows: bool) -> Result<String> {
    let normalized = if windows {
        if let Some(unc) = path.strip_prefix(r"\\?\UNC\") {
            format!("//{}", unc.replace('\\', "/"))
        } else {
            path.strip_prefix(r"\\?\").unwrap_or(path).replace('\\', "/")
        }
    } else {
        path.to_string()
    };
    let (mut url, rest) = if windows && normalized.starts_with("//") {
        (String::from("file://"), &normalized[2..])
    } else if windows
        && normalized.as_bytes().get(1) == Some(&b':')
        && normalized.as_bytes().get(2) == Some(&b'/')
    {
        (String::from("file:///"), normalized.as_str())
    } else if !windows && normalized.starts_with('/') {
        (String::from("file://"), normalized.as_str())
    } else {
        anyhow::bail!("trace hook path must be absolute");
    };
    for byte in rest.bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~:".contains(&byte) {
            url.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            let _ = write!(url, "%{byte:02X}");
        }
    }
    Ok(url)
}

fn reports_from_events(
    raw: &str,
    packages: &[PathBuf],
) -> Result<(Vec<PackageReport>, Vec<Diagnostic>)> {
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
    let mut diagnostics = BTreeMap::new();
    for line in raw.lines() {
        let event: Event = serde_json::from_str(line).context("reading Node trace event")?;
        let Ok(parent) = event.parent.canonicalize() else { continue };
        let Some((root, manifest)) = roots.iter().find(|(path, _)| parent.starts_with(path)) else {
            continue;
        };
        let relative =
            parent.strip_prefix(root).unwrap_or(&parent).to_string_lossy().replace('\\', "/");
        let name = package_name(&event.specifier);
        let declared = name.is_some_and(|name| manifest.reachable().any(|dep| dep == name));
        let missing = event.code.is_empty()
            || matches!(event.code.as_str(), "MODULE_NOT_FOUND" | "ERR_MODULE_NOT_FOUND");
        if !missing || declared || event.specifier.starts_with('#') {
            let message = format!(
                "{}: {} ({})",
                event.code,
                event.specifier,
                event.message.lines().next().unwrap_or(""),
            );
            let diagnostic = Diagnostic {
                package: manifest.id(),
                file: relative,
                message,
                kind: DiagnosticKind::ResolutionError,
            };
            diagnostics.insert(
                (diagnostic.package.clone(), diagnostic.file.clone(), diagnostic.message.clone()),
                diagnostic,
            );
            continue;
        }
        let Some(name) = name else { continue };
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
        finding.evidence.insert(Evidence {
            file: relative,
            line: 0,
            column: 0,
            specifier: event.specifier,
            origin: Origin::Runtime,
        });
    }
    let reports = reports
        .into_iter()
        .map(|(package, findings)| PackageReport {
            package,
            findings: findings.into_values().collect(),
        })
        .collect();
    Ok((reports, diagnostics.into_values().collect()))
}

#[cfg(test)]
mod tests;
