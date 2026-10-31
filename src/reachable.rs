#[cfg(test)]
mod tests;

use crate::{manifest::Manifest, scan};
use anyhow::{Context, Result};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};
use walkdir::WalkDir;

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
    reachable_files_inner(package_dir, manifest, false).unwrap_or_default()
}

/// Refuse to infer a complete reachable graph when a source file cannot be read
/// or parsed. Repairs must not be based on an incomplete graph.
pub fn reachable_files_for_repair(
    package_dir: &Path,
    manifest: &Manifest,
) -> Result<BTreeSet<PathBuf>> {
    reachable_files_inner(package_dir, manifest, true)
}

fn reachable_files_inner(
    package_dir: &Path,
    manifest: &Manifest,
    strict: bool,
) -> Result<BTreeSet<PathBuf>> {
    // Resolved paths are canonical, so the root has to be too for the
    // containment check below to mean anything.
    let root = package_dir.canonicalize().unwrap_or_else(|_| package_dir.to_path_buf());
    let mut queue: Vec<PathBuf> = entry_files(&root, manifest);
    let mut seen = BTreeSet::new();

    while let Some(file) = queue.pop() {
        if !seen.insert(file.clone()) {
            continue;
        }
        if let Some(companion) = declaration_companion(&file) {
            if companion.starts_with(&root) {
                queue.push(companion);
            }
        }
        let source = match std::fs::read_to_string(&file) {
            Ok(source) => source,
            Err(error) if strict => {
                return Err(error).with_context(|| format!("reading source {}", file.display()))
            }
            Err(_) => continue,
        };
        let found = match scan::specifiers(&source, &file) {
            Ok(found) => found,
            Err(reason) if strict => anyhow::bail!("could not parse {}: {reason}", file.display()),
            Err(_) => continue,
        };
        let parent = file.parent().unwrap_or(package_dir);
        for (requirement, _origin) in found {
            let specifier = match requirement {
                scan::Requirement::PathReference(specifier) => {
                    match resolve_relative(parent, &specifier) {
                        Some(target) if target.starts_with(&root) => queue.push(target),
                        _ => {}
                    }
                    continue;
                }
                scan::Requirement::Module(specifier) => specifier,
                scan::Requirement::TypesReference(_) => continue,
            };
            if specifier.starts_with('#') {
                for target in manifest.import_targets(&specifier) {
                    if target.starts_with("./") {
                        if let Some(file) = resolve_relative(&root, &target) {
                            if file.starts_with(&root) {
                                queue.push(file);
                            }
                        }
                    }
                }
                continue;
            }
            if !specifier.starts_with('.') {
                continue;
            }
            // `../../other-package/index.js` resolves to a real file whose
            // imports belong to that package, not this one.
            match resolve_relative(parent, &specifier) {
                Some(target) if target.starts_with(&root) => queue.push(target),
                _ => {}
            }
        }
    }
    Ok(seen)
}

fn entry_files(package_dir: &Path, manifest: &Manifest) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = manifest
        .entry_paths()
        .iter()
        .flat_map(|entry| {
            expand_entry(
                package_dir,
                entry.path.trim_start_matches("./"),
                entry.export_key.as_deref(),
                manifest,
            )
        })
        .filter(|path| path.starts_with(package_dir))
        .collect();

    // A package with no entry-point field is still importable through the name
    // Node falls back to. One whose `exports` names nothing scannable is not:
    // an unexported `index.js` beside it is exactly what the map excludes.
    if !manifest.is_encapsulated() && (entries.is_empty() || manifest.main.is_null()) {
        entries.extend(resolve_relative(package_dir, "index"));
    }
    entries
}

/// TypeScript substitutes the corresponding declaration for a JavaScript
/// module, even when the manifest omits an explicit types entry.
fn declaration_companion(file: &Path) -> Option<PathBuf> {
    let name = file.to_str()?;
    for (extension, declaration) in [(".mjs", ".d.mts"), (".cjs", ".d.cts"), (".js", ".d.ts")] {
        if let Some(stem) = name.strip_suffix(extension) {
            return existing_file(Path::new(&format!("{stem}{declaration}")));
        }
    }
    None
}

/// Export patterns substitute the same `*` with a subpath, including slashes.
/// Enumerating shipped files makes these entries visible without following
/// symlinks into other packages or scanning nested bundled dependencies.
fn expand_entry(
    root: &Path,
    pattern: &str,
    export_key: Option<&str>,
    manifest: &Manifest,
) -> Vec<PathBuf> {
    let Some((prefix, suffix)) = pattern.split_once('*') else {
        return resolve_relative(root, pattern).into_iter().collect();
    };
    if suffix.contains('*') || pattern.starts_with('/') {
        return Vec::new();
    }
    WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| entry.file_name() != "node_modules")
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file() && scan::is_scannable(entry.path()))
        .filter_map(|entry| {
            let relative = entry.path().strip_prefix(root).ok()?.to_str()?.replace('\\', "/");
            let middle = relative.strip_prefix(prefix)?.strip_suffix(suffix)?;
            // Node substitutes the same value for each star in a target.
            if middle.is_empty() {
                return None;
            }
            if let Some(key) = export_key {
                let subpath = key.replace('*', middle);
                if !manifest.selects_export(key, &subpath) {
                    return None;
                }
            }
            existing_file(entry.path())
        })
        .collect()
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
