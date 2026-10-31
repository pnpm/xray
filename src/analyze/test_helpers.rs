use super::{analyze_with_diagnostics, Scope};
use crate::report::{DiagnosticKind, PackageReport};
use anyhow::Result;
use std::path::Path;

/// What one installed package reaches for but never declared, or `None` when it
/// declared everything its shipped files name.
pub fn analyze(package_dir: &Path, scope: Scope) -> Result<Option<PackageReport>> {
    let outcome = analyze_with_diagnostics(package_dir, scope)?;
    for diagnostic in &outcome.diagnostics {
        if diagnostic.kind == DiagnosticKind::ReadError {
            anyhow::bail!(
                "reading source {}: {}",
                package_dir.join(&diagnostic.file).display(),
                diagnostic.message,
            );
        }
        eprintln!(
            "warning: could not parse {}: {}",
            package_dir.join(&diagnostic.file).display(),
            diagnostic.message,
        );
    }
    Ok(outcome.report)
}
