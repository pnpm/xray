//! Discover installed workspace importer locations, including separate lockfiles.
use anyhow::{Context, Result};
use globset::GlobBuilder;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};
use walkdir::WalkDir;
use yaml_rust2::{Yaml, YamlLoader};

/// The root, every importer of the root lockfile, and every package matched by
/// the workspace `packages` globs. Both sources are read: with
/// `sharedWorkspaceLockfile: false` the root lockfile lists only the root, and
/// each member records itself in its own lockfile.
pub fn projects(root: &Path) -> Result<Vec<PathBuf>> {
    let root = root.canonicalize()?;
    let mut projects = BTreeSet::from([root.clone()]);
    let mut listed_importers = false;
    let lockfile = root.join("pnpm-lock.yaml");
    if lockfile.is_file() {
        let raw = std::fs::read_to_string(&lockfile)
            .with_context(|| format!("reading {}", lockfile.display()))?;
        let documents = YamlLoader::load_from_str(&raw)
            .with_context(|| format!("parsing {}", lockfile.display()))?;
        if let Some(importers) =
            documents.first().and_then(|document| document["importers"].as_hash())
        {
            listed_importers = true;
            for importer in importers.keys().filter_map(Yaml::as_str) {
                if let Ok(project) = root.join(importer).canonicalize() {
                    if project.starts_with(&root) {
                        projects.insert(project);
                    }
                }
            }
        }
    }
    let config = root.join("pnpm-workspace.yaml");
    if !config.is_file() {
        return Ok(projects.into_iter().collect());
    }
    let raw = std::fs::read_to_string(&config)
        .with_context(|| format!("reading {}", config.display()))?;
    let documents =
        YamlLoader::load_from_str(&raw).with_context(|| format!("parsing {}", config.display()))?;
    let configured = documents.first().map(|document| &document["packages"]);
    let default = vec![Yaml::String("**".into())];
    let patterns = match configured {
        // A lockfile already names the importers of an implicit workspace.
        None | Some(Yaml::Null | Yaml::BadValue) if listed_importers => {
            return Ok(projects.into_iter().collect())
        }
        None | Some(Yaml::Null | Yaml::BadValue) => &default,
        Some(value) => value.as_vec().context("workspace packages must be an array")?,
    };
    let mut included = Vec::new();
    let mut excluded = Vec::new();
    for pattern in patterns {
        let pattern = pattern.as_str().context("workspace package patterns must be strings")?;
        let (negative, pattern) =
            pattern.strip_prefix('!').map_or((false, pattern), |pattern| (true, pattern));
        let pattern = pattern.strip_prefix("./").unwrap_or(pattern).trim_end_matches('/');
        let matcher = GlobBuilder::new(pattern)
            .literal_separator(true)
            .backslash_escape(false)
            .build()
            .with_context(|| format!("invalid workspace pattern {pattern}"))?
            .compile_matcher();
        if negative {
            excluded.push(matcher);
        } else {
            included.push(matcher);
        }
    }
    for entry in WalkDir::new(&root).follow_links(false).into_iter().filter_entry(|entry| {
        !matches!(entry.file_name().to_str(), Some("node_modules" | ".git" | "bower_components"))
    }) {
        let entry =
            entry.with_context(|| format!("discovering workspace under {}", root.display()))?;
        if entry.file_name() != "package.json" || !entry.path().is_file() {
            continue;
        }
        let Some(directory) = entry.path().parent() else { continue };
        let relative = directory.strip_prefix(&root)?;
        if included.iter().any(|pattern| pattern.is_match(relative))
            && !excluded.iter().any(|pattern| pattern.is_match(relative))
        {
            projects.insert(directory.to_path_buf());
        }
    }
    Ok(projects.into_iter().collect())
}

#[cfg(test)]
mod tests;
