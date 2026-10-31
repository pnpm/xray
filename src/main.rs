mod analyze;
mod classify;
mod configuration;
mod declaration_patch;
mod discovery;
mod manifest;
mod process;
mod reachable;
mod regenerate;
mod repair;
mod repair_conflicts;
mod repair_plan;
mod report;
mod scan;
mod style;
mod trace;
mod verify;
mod verify_candidates;
mod verify_lease;
mod verify_signals;
mod verify_transaction;
mod verify_types;
mod workspace;

#[cfg(test)]
mod fixtures;

use crate::{
    analyze::{analyze_for_repair, analyze_with_diagnostics, Scope},
    discovery::installed_packages,
    report::{Diagnostic, DiagnosticKind, PackageReport},
};
use anyhow::Result;
use clap::{ArgGroup, Parser};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::ExitCode,
};

/// Packages published to npm routinely import things they never declared. Under
/// a hoisted `node_modules` those imports resolve by accident; under a strict or
/// global virtual store they do not. This finds them before your users do.
#[derive(Debug, Parser)]
#[command(name = "xray", version)]
#[command(group(ArgGroup::new("execution").args(["trace", "verify"])))]
#[command(group(ArgGroup::new("repair_action").args(["fix", "verify"]).multiple(true)))]
#[expect(
    clippy::struct_excessive_bools,
    reason = "a flag is a bool; the lint is aimed at domain types, not an argument struct"
)]
struct Args {
    /// A project whose installed tree should be scanned, or a single package
    /// directory when `--package` is given.
    #[arg(default_value = ".")]
    path: PathBuf,

    /// Treat `path` as one package directory instead of a project to walk.
    #[arg(long)]
    package: bool,

    /// Report the findings as JSON.
    #[arg(long, conflicts_with = "package_extensions")]
    json: bool,

    /// Exit 1 when undeclared dependencies remain, or 2 for an incomplete scan.
    #[arg(long, conflicts_with_all = ["fix", "trace", "verify"])]
    check: bool,

    /// Emit version-scoped package extensions using publisher dependency ranges
    /// when available, and optional peers otherwise.
    #[arg(long)]
    package_extensions: bool,

    /// Read every file in the package, even when its `exports` map says which
    /// files a consumer may reach. Finds more, including what tests and
    /// generator templates import on their own behalf.
    #[arg(long)]
    all_files: bool,

    /// Apply selected package extensions to pnpm-workspace.yaml. Run an install
    /// afterward to update the lockfile and dependency links.
    #[arg(long, conflicts_with_all = ["package", "json", "package_extensions"])]
    fix: bool,

    /// Print selected repairs without changing workspace settings.
    #[arg(long, requires = "fix")]
    dry_run: bool,

    /// Propose pnpm patches for malformed declarations only when the local
    /// TypeScript compiler verifies the full project with no remaining errors.
    #[arg(long, requires = "repair_action", conflicts_with = "trace")]
    regenerate_types: bool,

    /// Repair and reinstall until the command succeeds, minimize the retained
    /// changes, and restore the original configuration and graph on failure.
    /// The command may run repeatedly; installation follows existing pnpm policy.
    #[arg(long, requires = "command", conflicts_with_all = ["package", "json", "package_extensions", "dry_run"])]
    verify: bool,

    /// Maximum verification attempts after the baseline command.
    #[arg(long, requires = "verify", default_value_t = 32, value_parser = clap::value_parser!(u16).range(1..=256))]
    verify_max_attempts: u16,

    /// Maximum distinct proposals considered during a verified repair.
    #[arg(long, requires = "verify", default_value_t = 32, value_parser = clap::value_parser!(u16).range(1..=256))]
    verify_max_candidates: u16,

    /// Restore a retained verification journal for the selected workspace.
    #[arg(long, conflicts_with_all = ["verify", "trace", "fix", "dry_run", "regenerate_types", "package", "json", "package_extensions", "check", "all_files", "command"])]
    recover: Option<PathBuf>,

    /// Observe failed Node module resolutions while running a command after --.
    /// Requires Node 22.15+; observations may be caught optional integrations.
    #[arg(long, requires = "command", conflicts_with_all = ["package", "json", "package_extensions", "all_files"])]
    trace: bool,

    /// Command and arguments to execute with --trace or --verify.
    #[arg(last = true, requires = "execution")]
    command: Vec<OsString>,
}

fn main() -> Result<ExitCode> {
    let args = Args::parse();
    if let Some(journal) = &args.recover {
        return verify::recover(&args.path, journal);
    }
    let packages =
        if args.package { vec![args.path.clone()] } else { installed_packages(&args.path)? };
    let workspace =
        if args.package { args.path.clone() } else { configuration::workspace_root(&args.path)? };
    let providers = if !args.package
        && workspace != args.path.canonicalize()?
        && workspace::projects(&workspace)?
            .iter()
            .any(|project| project.join("node_modules").is_dir())
    {
        installed_packages(&workspace)?
    } else {
        packages.clone()
    };

    if args.verify {
        return run_verified(&args, &workspace);
    }
    if args.trace {
        return run_trace(&args, &workspace, &providers);
    }

    let scope = if args.all_files { Scope::EveryFile } else { Scope::Declared };
    let mut reports = Vec::new();
    let mut diagnostics = Vec::new();
    for directory in &packages {
        if args.fix || args.package_extensions && !args.package {
            match analyze_for_repair(directory, scope) {
                Ok(Some(report)) => reports.push(report),
                Ok(None) => {}
                Err(error) => diagnostics.push(Diagnostic {
                    package: directory.display().to_string(),
                    file: directory.display().to_string(),
                    message: format!("{error:#}"),
                    kind: DiagnosticKind::AnalysisError,
                }),
            }
        } else {
            match analyze_with_diagnostics(directory, scope) {
                Ok(outcome) => {
                    if let Some(report) = outcome.report {
                        reports.push(report);
                    }
                    diagnostics.extend(outcome.diagnostics);
                }
                Err(error) => diagnostics.push(Diagnostic {
                    package: directory.display().to_string(),
                    file: directory.display().to_string(),
                    message: format!("{error:#}"),
                    kind: DiagnosticKind::AnalysisError,
                }),
            }
        }
    }
    if !args.json {
        for diagnostic in &diagnostics {
            eprintln!("{}: {}: {}", diagnostic.package, diagnostic.file, diagnostic.message);
        }
        if !diagnostics.is_empty() {
            eprintln!(
                "Scan incomplete: {} diagnostics; affected packages are excluded from repair.",
                diagnostics.len(),
            );
        }
    }
    let mut reports = report::merge_reports(reports);
    if !args.package {
        configuration::apply(&workspace, &mut reports)?;
    }
    let status = if !diagnostics.is_empty() {
        ExitCode::from(2)
    } else if args.check && !reports.is_empty() {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    };

    if args.fix {
        write_repairs(&args, &workspace, &reports, &providers)?;
        return Ok(status);
    }

    let rendered = if args.json {
        report::as_json(&reports, packages.len(), &diagnostics)?
    } else if args.package_extensions {
        if args.package {
            report::as_package_extensions(&reports)
        } else {
            report::as_package_extensions(&repair_plan::select(&reports, &providers, &workspace)?)
        }
    } else if reports.is_empty() && !diagnostics.is_empty() {
        "Scan incomplete; no dependency findings in successfully analyzed files.\n".to_string()
    } else {
        report::as_text(&reports)
    };
    print!("{rendered}");

    Ok(status)
}

fn run_verified(args: &Args, workspace: &Path) -> Result<ExitCode> {
    verify::run(verify::Options {
        project: &args.path,
        workspace,
        command: &args.command,
        scope: if args.all_files { Scope::EveryFile } else { Scope::Declared },
        regenerate_types: args.regenerate_types,
        max_attempts: usize::from(args.verify_max_attempts),
        max_candidates: usize::from(args.verify_max_candidates),
    })
}

fn write_repairs(
    args: &Args,
    workspace: &Path,
    reports: &[PackageReport],
    packages: &[PathBuf],
) -> Result<()> {
    let selected = repair_plan::select(reports, packages, workspace)?;
    let regenerated = if args.regenerate_types {
        let proposal = regenerate::propose(&args.path, packages)?;
        for message in proposal.messages {
            eprintln!("Declaration verification: {message}");
        }
        proposal.patches
    } else {
        Vec::new()
    };
    let patch_plan = declaration_patch::plan(&regenerated, packages)?;
    let proposed =
        format!("{}{}", report::as_package_extensions(&selected), patch_plan.configuration);
    for patch in &regenerated {
        eprintln!(
            "Verified {} declaration replacements for {} with TypeScript {}.",
            patch.files.len(),
            patch.package,
            patch.compiler,
        );
    }
    let total: usize = reports.iter().map(|report| report.findings.len()).sum();
    let repaired: usize = selected.iter().map(|report| report.findings.len()).sum();
    if total > repaired {
        eprintln!(
            "{} findings have no selected repair; inspect xray --json for evidence.",
            total - repaired,
        );
    }
    if args.dry_run {
        declaration_patch::validate(
            &workspace.join("pnpm-workspace.yaml"),
            &proposed,
            &patch_plan,
        )?;
        print!("{proposed}");
        for (path, content) in &patch_plan.artifacts {
            eprintln!("Proposed {}:\n{content}", path.display());
        }
    } else {
        let workspace = workspace.join("pnpm-workspace.yaml");
        if declaration_patch::apply(&workspace, &proposed, &patch_plan)? {
            println!("Updated {}. Run pnpm install to apply the repairs.", workspace.display());
        } else {
            println!("No changes to workspace settings.");
        }
    }
    Ok(())
}

fn run_trace(args: &Args, workspace: &Path, providers: &[PathBuf]) -> Result<ExitCode> {
    let observed = trace::run(&args.command, providers, &args.path)?;
    if !observed.reports.is_empty() {
        eprintln!("Observed failed Node resolutions; optional imports may have been caught:");
        eprint!("{}", report::as_text(&observed.reports));
    }
    for diagnostic in &observed.diagnostics {
        eprintln!(
            "Observed resolution failure in {}: {}: {}",
            diagnostic.package, diagnostic.file, diagnostic.message,
        );
    }
    if args.fix {
        let mut reports = observed.reports;
        configuration::apply(workspace, &mut reports)?;
        write_repairs(args, workspace, &reports, providers)?;
    }
    Ok(ExitCode::from(trace::status_code(observed.status)))
}
