//! Observe failed Node module resolutions while a command runs.
use crate::{
    classify::package_name,
    manifest::Manifest,
    report::{Diagnostic, DiagnosticKind, Evidence, Finding, PackageReport, Severity},
    scan::Origin,
};
use anyhow::{Context, Result};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    io::Write as _,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

#[derive(Deserialize)]
struct Event {
    specifier: String,
    parent: PathBuf,
    #[serde(default)]
    code: String,
    #[serde(default)]
    message: String,
    #[serde(default, rename = "customResolution")]
    custom_resolution: bool,
}

pub struct TraceResult {
    pub status: ExitStatus,
    pub reports: Vec<PackageReport>,
    pub diagnostics: Vec<Diagnostic>,
}

struct TraceOwner {
    root: PathBuf,
    manifest: Manifest,
    installed: bool,
}

/// Preserve ordinary exit codes and use the shell convention for Unix signals.
pub fn status_code(status: ExitStatus) -> u8 {
    if let Some(code) = status.code() {
        return u8::try_from(code).unwrap_or(1);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt as _;
        if let Some(signal) = status.signal() {
            return u8::try_from(128 + signal).unwrap_or(1);
        }
    }
    1
}

/// The command's own status, or the shell's interrupt code when xray was
/// interrupted and the command still reported success.
pub fn exit_code(status: ExitStatus, interrupted: bool) -> u8 {
    if interrupted && status.success() {
        130
    } else {
        status_code(status)
    }
}

/// Run a command in the foreground for plain `--trace`. The terminal delivers
/// Ctrl-C to the command as well, so on `cancelled` it is given time to shut
/// down on its own before it is killed, and the observations made until then
/// are still returned.
pub fn run_interactive(
    command: &[OsString],
    packages: &[PathBuf],
    project: &Path,
    cancelled: &AtomicBool,
) -> Result<TraceResult> {
    run_inner(command, packages, project, Wait::Foreground(cancelled))
}

/// Run a command in its own process group, killed as soon as `cancelled` is set.
pub fn run_cancellable(
    command: &[OsString],
    packages: &[PathBuf],
    project: &Path,
    cancelled: &AtomicBool,
) -> Result<TraceResult> {
    run_inner(command, packages, project, Wait::Group(cancelled))
}

#[derive(Clone, Copy)]
enum Wait<'a> {
    Foreground(&'a AtomicBool),
    Group(&'a AtomicBool),
}

/// How long an interrupted foreground command may take to exit by itself.
const INTERRUPT_GRACE: Duration = Duration::from_secs(10);

fn run_inner(
    command: &[OsString],
    packages: &[PathBuf],
    project: &Path,
    wait: Wait<'_>,
) -> Result<TraceResult> {
    let (program, args) = command.split_first().context("--trace needs a command after --")?;
    let temp = tempfile::tempdir().context("creating trace directory")?;
    let hook = install_hook()?;
    let events = temp.path().join("events.jsonl");
    std::fs::write(&events, "")?;
    let options = node_options(std::env::var_os("NODE_OPTIONS"), &file_url(&hook)?);
    let mut workspaces = Vec::new();
    for workspace in crate::workspace::projects(project)? {
        if workspace.join("package.json").is_file() {
            workspaces.push(workspace);
        }
    }
    let owners = trace_owners(packages, &workspaces)?;
    let mut child = crate::process::command(program);
    child
        .args(args)
        .current_dir(project)
        .env("NODE_OPTIONS", options)
        .env("XRAY_TRACE_FILE", &events);
    let status = match wait {
        Wait::Group(flag) => crate::verify_signals::run(&mut child, flag)?,
        Wait::Foreground(flag) => wait_foreground(&mut child, flag, INTERRUPT_GRACE)?,
    };
    let observations = reports_from_events(&std::fs::read(events)?, &owners);
    if observations.skipped > 0 {
        eprintln!(
            "xray: skipped {} unreadable trace events; observations may be incomplete.",
            observations.skipped,
        );
    }
    Ok(TraceResult { status, reports: observations.reports, diagnostics: observations.diagnostics })
}

/// Keep the caller's Node options and add the preload after them.
fn node_options(existing: Option<OsString>, hook: &str) -> OsString {
    let mut options = existing.unwrap_or_default();
    if !options.is_empty() {
        options.push(" ");
    }
    options.push(format!("--import={hook}"));
    options
}

fn wait_foreground(
    command: &mut Command,
    cancelled: &AtomicBool,
    grace: Duration,
) -> Result<ExitStatus> {
    let mut child = command.spawn().context("starting traced command")?;
    let mut deadline = None;
    loop {
        if let Some(status) = child.try_wait().context("waiting for traced command")? {
            return Ok(status);
        }
        if cancelled.load(Ordering::SeqCst) {
            let deadline = *deadline.get_or_insert_with(|| Instant::now() + grace);
            if Instant::now() >= deadline {
                // The command may exit concurrently. Always reap it.
                if let Err(error) = child.kill() {
                    if child.try_wait()?.is_none() {
                        return Err(error).context("stopping traced command");
                    }
                }
                return child.wait().context("reaping traced command");
            }
        }
        thread::sleep(Duration::from_millis(25));
    }
}

/// Write the preload to a content-addressed path that is never removed. Node
/// processes that outlive the trace, such as build daemons, keep the inherited
/// `NODE_OPTIONS`, and every Node process they start must still find the hook.
fn install_hook() -> Result<PathBuf> {
    install_hook_in(&cache_directory().join("xray"))
}

fn install_hook_in(directory: &Path) -> Result<PathBuf> {
    use std::fmt::Write as _;
    const HOOK: &str = include_str!("trace-hook.mjs");
    let digest = Sha256::digest(HOOK.as_bytes());
    let name = digest.iter().take(8).fold(String::new(), |mut name, byte| {
        let _ = write!(name, "{byte:02x}");
        name
    });
    std::fs::create_dir_all(directory)
        .with_context(|| format!("creating {}", directory.display()))?;
    let hook = directory.join(format!("trace-hook-{name}.mjs"));
    if std::fs::read_to_string(&hook).is_ok_and(|current| current == HOOK) {
        return Ok(hook);
    }
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    temporary.write_all(HOOK.as_bytes())?;
    temporary.as_file().sync_all()?;
    temporary.persist(&hook).with_context(|| format!("writing {}", hook.display()))?;
    Ok(hook)
}

fn cache_directory() -> PathBuf {
    let from_env =
        |name: &str| std::env::var_os(name).map(PathBuf::from).filter(|path| path.is_absolute());
    if cfg!(windows) {
        if let Some(local) = from_env("LOCALAPPDATA") {
            return local;
        }
    } else if let Some(cache) = from_env("XDG_CACHE_HOME") {
        return cache;
    } else if let Some(home) = from_env("HOME") {
        return if cfg!(target_os = "macos") {
            home.join("Library/Caches")
        } else {
            home.join(".cache")
        };
    }
    std::env::temp_dir()
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

fn trace_owners(packages: &[PathBuf], workspaces: &[PathBuf]) -> Result<Vec<TraceOwner>> {
    let mut roots = Vec::new();
    for package in packages {
        let path = package.canonicalize()?;
        let installed = path.components().any(|component| component.as_os_str() == "node_modules");
        let manifest = Manifest::read(&path)?;
        roots.push(TraceOwner { root: path, manifest, installed });
    }
    for workspace in workspaces {
        let path = workspace.canonicalize()?;
        let manifest = Manifest::read(&path)?;
        roots.retain(|existing| existing.root != path);
        roots.push(TraceOwner { root: path, manifest, installed: false });
    }
    roots.sort_by_key(|owner| std::cmp::Reverse(owner.root.components().count()));
    Ok(roots)
}

struct Observations {
    reports: Vec<PackageReport>,
    diagnostics: Vec<Diagnostic>,
    /// Lines that were not a well-formed UTF-8 JSON event, for example one cut
    /// short by a killed process or holding an unpaired surrogate escape.
    skipped: usize,
}

fn reports_from_events(raw: &[u8], roots: &[TraceOwner]) -> Observations {
    let mut reports: BTreeMap<String, BTreeMap<String, Finding>> = BTreeMap::new();
    let mut diagnostics = BTreeMap::new();
    let mut skipped = 0;
    for line in raw.split(|byte| *byte == b'\n') {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let Ok(event) = serde_json::from_slice::<Event>(line) else {
            skipped += 1;
            continue;
        };
        let Ok(parent) = event.parent.canonicalize() else { continue };
        let Some(TraceOwner { root, manifest, installed }) = roots.iter().find(|owner| {
            parent.strip_prefix(&owner.root).is_ok_and(|relative| {
                owner.installed
                    || !relative.components().any(|part| part.as_os_str() == "node_modules")
            })
        }) else {
            continue;
        };
        let relative =
            parent.strip_prefix(root).unwrap_or(&parent).to_string_lossy().replace('\\', "/");
        let name = package_name(&event.specifier);
        let declared = name.is_some_and(|name| manifest.reachable().any(|dep| dep == name));
        let missing = event.code.is_empty()
            || matches!(event.code.as_str(), "MODULE_NOT_FOUND" | "ERR_MODULE_NOT_FOUND");
        if !missing
            || declared
            || event.specifier.starts_with('#')
            || !installed
            || event.custom_resolution
        {
            let context = if !installed {
                "Workspace source; package extensions do not repair workspace manifests. "
            } else if event.custom_resolution {
                "Explicit resolver options; caller dependency ownership is uncertain. "
            } else {
                ""
            };
            let message = format!(
                "{context}{}: {} ({})",
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
                    deep_import_only: false,
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
    Observations { reports, diagnostics: diagnostics.into_values().collect(), skipped }
}

#[cfg(test)]
mod tests;
