mod analyze;
mod classify;
mod discovery;
mod manifest;
mod reachable;
mod repair;
mod repair_conflicts;
mod repair_plan;
mod report;
mod scan;
mod trace;

#[cfg(test)]
mod fixtures;

use crate::{
    analyze::{analyze, analyze_for_repair, Scope},
    discovery::installed_packages,
    report::PackageReport,
};
use anyhow::Result;
use clap::Parser;
use std::{ffi::OsString, path::PathBuf, process::ExitCode};

/// Packages published to npm routinely import things they never declared. Under
/// a hoisted `node_modules` those imports resolve by accident; under a strict or
/// global virtual store they do not. This finds them before your users do.
#[derive(Debug, Parser)]
#[command(name = "xray", version)]
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

    /// Observe failed Node module resolutions while running a command after --.
    /// Requires Node 22.15+; observations may be caught optional integrations.
    #[arg(long, requires = "command", conflicts_with_all = ["fix", "package", "json", "package_extensions", "all_files"])]
    trace: bool,

    /// Command and arguments to execute with --trace.
    #[arg(last = true, requires = "trace")]
    command: Vec<OsString>,
}

fn main() -> Result<ExitCode> {
    let args = Args::parse();
    let packages =
        if args.package { vec![args.path.clone()] } else { installed_packages(&args.path)? };

    if args.trace {
        let observed = trace::run(&args.command, &packages, &args.path)?;
        if !observed.reports.is_empty() {
            eprintln!("Observed failed Node resolutions; optional imports may have been caught:");
            eprint!("{}", report::as_text(&observed.reports));
        }
        return Ok(ExitCode::from(u8::try_from(observed.status.code().unwrap_or(1)).unwrap_or(1)));
    }

    let scope = if args.all_files { Scope::EveryFile } else { Scope::Declared };
    let reports: Vec<PackageReport> = packages
        .iter()
        .filter_map(|dir| {
            if args.fix { analyze_for_repair(dir, scope) } else { analyze(dir, scope) }.transpose()
        })
        .collect::<Result<_>>()?;
    let reports = report::merge_reports(reports);

    if args.fix {
        let selected = repair_plan::select(&reports, &packages, &args.path)?;
        let proposed = report::as_package_extensions(&selected);
        let total: usize = reports.iter().map(|report| report.findings.len()).sum();
        let repaired: usize = selected.iter().map(|report| report.findings.len()).sum();
        if total > repaired {
            eprintln!(
                "{} findings have no selected repair; inspect xray --json for evidence.",
                total - repaired
            );
        }
        if args.dry_run {
            print!("{proposed}");
        } else {
            let workspace = args.path.join("pnpm-workspace.yaml");
            if repair::apply(&workspace, &proposed)? {
                println!("Updated {}. Run pnpm install to apply the repairs.", workspace.display());
            } else {
                println!("No changes to workspace settings.");
            }
        }
        return Ok(ExitCode::SUCCESS);
    }

    let rendered = if args.json {
        report::as_json(&reports)?
    } else if args.package_extensions {
        if args.package {
            report::as_package_extensions(&reports)
        } else {
            report::as_package_extensions(&repair_plan::select(&reports, &packages, &args.path)?)
        }
    } else {
        report::as_text(&reports)
    };
    print!("{rendered}");

    Ok(ExitCode::SUCCESS)
}
