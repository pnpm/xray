mod analyze;
mod classify;
mod discovery;
mod manifest;
mod reachable;
mod report;
mod scan;

#[cfg(test)]
mod fixtures;

use crate::{
    analyze::{analyze, Scope},
    discovery::installed_packages,
    report::PackageReport,
};
use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

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
    #[arg(long)]
    json: bool,

    /// Emit a `packageExtensions` block that declares every finding as an
    /// optional peer dependency, ready to paste into pnpm-workspace.yaml.
    #[arg(long)]
    package_extensions: bool,

    /// Read every file in the package, even when its `exports` map says which
    /// files a consumer may reach. Finds more, including what tests and
    /// generator templates import on their own behalf.
    #[arg(long)]
    all_files: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let packages =
        if args.package { vec![args.path.clone()] } else { installed_packages(&args.path)? };

    let scope = if args.all_files { Scope::EveryFile } else { Scope::Declared };
    let mut reports: Vec<PackageReport> =
        packages.iter().filter_map(|dir| analyze(dir, scope).transpose()).collect::<Result<_>>()?;
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
