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
        for (requirement, origin) in found {
            let specifier = match requirement {
                scan::Requirement::Style(specifier, kind) => {
                    for target in resolve_style(parent, &specifier, kind) {
                        if target.starts_with(&root) {
                            queue.push(target);
                        }
                    }
                    continue;
                }
                scan::Requirement::PathReference(specifier) => {
                    match resolve_relative(parent, &specifier) {
                        Some(target) if target.starts_with(&root) => queue.push(target),
                        _ => {}
                    }
                    continue;
                }
                scan::Requirement::EsmRelative(specifier) => {
                    match resolve_esm_relative(parent, &specifier) {
                        Some(target) if target.starts_with(&root) => queue.push(target),
                        _ => {}
                    }
                    continue;
                }
                scan::Requirement::Module(specifier) => specifier,
                scan::Requirement::TypesReference(_) => continue,
            };
            if origin == scan::Origin::Runtime {
                for target in browser_replacements(manifest, &root, parent, &specifier) {
                    if target.starts_with("./") {
                        if let Some(file) = resolve_relative(&root, &target) {
                            if file.starts_with(&root) {
                                queue.push(file);
                            }
                        }
                    }
                }
            }
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

fn resolve_esm_relative(from: &Path, specifier: &str) -> Option<PathBuf> {
    if let Some(exact) = resolve_relative(from, specifier) {
        return Some(exact);
    }
    let path = specifier.split(['?', '#']).next()?;
    let mut decoded = Vec::with_capacity(path.len());
    let mut bytes = path.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let high = char::from(bytes.next()?).to_digit(16)?;
            let low = char::from(bytes.next()?).to_digit(16)?;
            let byte = u8::try_from(high * 16 + low).ok()?;
            if matches!(byte, b'/' | b'\\' | 0) {
                return None;
            }
            decoded.push(byte);
        } else {
            decoded.push(byte);
        }
    }
    resolve_relative(from, std::str::from_utf8(&decoded).ok()?)
}

/// Return browser-field replacements for a reference from a reached file. Keys
/// beginning with `./` are package-root paths, unlike source imports.
pub fn browser_replacements(
    manifest: &Manifest,
    root: &Path,
    parent: &Path,
    specifier: &str,
) -> Vec<String> {
    let Some(map) = manifest.browser.as_object() else {
        return Vec::new();
    };
    let source = specifier.starts_with('.').then(|| resolve_relative(parent, specifier)).flatten();
    map.iter()
        .filter_map(|(key, value)| {
            let matches = if key.starts_with("./") {
                source
                    .as_ref()
                    .is_some_and(|file| resolve_relative(root, key).as_ref() == Some(file))
            } else {
                key == specifier
            };
            matches.then(|| value.as_str().map(str::to_string)).flatten()
        })
        .collect()
}

/// Resolve stylesheet references relative to their source. Sass may omit an
/// extension, use an underscore partial, or refer to a directory index.
pub fn resolve_style(from: &Path, specifier: &str, kind: crate::style::StyleKind) -> Vec<PathBuf> {
    if specifier.starts_with("pkg:") || specifier.starts_with('~') {
        return Vec::new();
    }
    let base = from.join(specifier);
    if kind == crate::style::StyleKind::Import {
        let mut import_only = BTreeSet::new();
        if let Some(name) = base.file_name().and_then(|name| name.to_str()) {
            let explicit = base.extension().and_then(|extension| extension.to_str());
            for extension in ["scss", "sass"] {
                if explicit.is_some_and(|existing| existing != extension) {
                    continue;
                }
                let stem = explicit
                    .and_then(|_| name.strip_suffix(&format!(".{extension}")))
                    .unwrap_or(name);
                for candidate in
                    [format!("{stem}.import.{extension}"), format!("_{stem}.import.{extension}")]
                {
                    if let Some(file) = existing_file(&base.with_file_name(candidate)) {
                        import_only.insert(file);
                    }
                }
            }
        }
        if !import_only.is_empty() {
            return import_only.into_iter().collect();
        }
    }
    let mut paths = BTreeSet::new();
    let mut add = |path: &Path| {
        if let Some(file) = existing_file(path) {
            paths.insert(file);
        }
    };
    add(&base);
    if let Some(name) = base.file_name().and_then(|name| name.to_str()) {
        add(&base.with_file_name(format!("_{name}")));
        for extension in ["scss", "sass", "css", "less"] {
            add(&append(&base, &format!(".{extension}")));
            add(&base.with_file_name(format!("_{name}.{extension}")));
        }
    }
    for extension in ["scss", "sass", "css", "less"] {
        add(&base.join(format!("index.{extension}")));
        add(&base.join(format!("_index.{extension}")));
    }
    paths.into_iter().collect()
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
    // Stylesheets are commonly published solely for subpath imports without a
    // manifest entry. Without exports, all of those subpaths are public.
    if !manifest.is_encapsulated() {
        entries.extend(
            WalkDir::new(package_dir)
                .follow_links(false)
                .into_iter()
                .filter_entry(|entry| entry.file_name() != "node_modules")
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().is_file() && scan::is_stylesheet(entry.path()))
                .filter_map(|entry| existing_file(entry.path()))
                .filter(|path| path.starts_with(package_dir)),
        );
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
