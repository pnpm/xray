#[cfg(test)]
mod tests;

use crate::{manifest::Manifest, scan};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

/// Tried in turn against a specifier that named no file directly. `.d.ts` comes
/// before `.ts` so a package shipping both resolves to the declarations, which
/// is what a consumer's compiler reads.
const EXTENSIONS: &[&str] =
    &[".js", ".mjs", ".cjs", ".jsx", ".d.ts", ".d.mts", ".d.cts", ".ts", ".mts", ".cts", ".tsx"];

/// The files a package can actually reach, starting from what its manifest
/// advertises and following relative specifiers outward.
///
/// A published tarball holds more than the package: tests, generator templates,
/// example projects. Their imports are real imports of files that are really
/// there, so nothing short of reachability tells them apart from the package's
/// own code — and their dependencies belong to whoever runs them, not to
/// whoever installs this.
pub fn reachable_files(package_dir: &Path, manifest: &Manifest) -> BTreeSet<PathBuf> {
    let mut queue: Vec<PathBuf> = entry_files(package_dir, manifest);
    let mut seen = BTreeSet::new();

    while let Some(file) = queue.pop() {
        if !seen.insert(file.clone()) {
            continue;
        }
        let Ok(source) = std::fs::read_to_string(&file) else {
            continue;
        };
        let Ok(found) = scan::specifiers(&source, &file) else {
            continue;
        };
        let parent = file.parent().unwrap_or(package_dir);
        for (requirement, _origin) in found {
            let scan::Requirement::Module(specifier) = requirement else {
                continue;
            };
            if !specifier.starts_with('.') {
                continue;
            }
            if let Some(target) = resolve_relative(parent, &specifier) {
                queue.push(target);
            }
        }
    }
    seen
}

fn entry_files(package_dir: &Path, manifest: &Manifest) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = manifest
        .entry_paths()
        .iter()
        .filter_map(|path| resolve_relative(package_dir, path.trim_start_matches("./")))
        .collect();

    // A package with no entry-point field at all is still importable through the
    // name Node falls back to.
    if entries.is_empty() {
        entries.extend(resolve_relative(package_dir, "index"));
    }
    entries
}

/// Node's file lookup, narrowed to what a published package needs: the path as
/// written, the path with an extension appended, and the directory's index.
///
/// The result is canonical. Joining a specifier onto a directory keeps every
/// `.` and `..` it was written with, so `./a` reached twice by different routes
/// yields two different paths for the same file — and a cycle between two files
/// then walks forever, growing a path segment each time.
fn resolve_relative(from: &Path, specifier: &str) -> Option<PathBuf> {
    let base = from.join(specifier);
    if let Some(file) = existing_file(&base) {
        return Some(file);
    }

    // A TypeScript package importing `./thing.js` means the file it compiles to,
    // which is `./thing.ts` in the tarball when sources ship alongside.
    let rewritten = ["js", "mjs", "cjs"].iter().find_map(|extension| {
        let stem = base.to_str()?.strip_suffix(&format!(".{extension}"))?;
        with_extension(Path::new(stem))
    });
    if rewritten.is_some() {
        return rewritten;
    }

    with_extension(&base).or_else(|| with_extension(&base.join("index")))
}

fn with_extension(base: &Path) -> Option<PathBuf> {
    EXTENSIONS.iter().find_map(|extension| existing_file(&append(base, extension)))
}

fn existing_file(path: &Path) -> Option<PathBuf> {
    (path.is_file() && scan::is_scannable(path))
        .then(|| path.canonicalize().unwrap_or_else(|_| path.to_path_buf()))
}

fn append(base: &Path, extension: &str) -> PathBuf {
    let mut name = base.as_os_str().to_os_string();
    name.push(extension);
    PathBuf::from(name)
}
