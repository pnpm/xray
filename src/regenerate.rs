//! Propose compiler-verified declaration replacements; never modify packages.
use crate::manifest::Manifest;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeclarationFile {
    pub path: PathBuf,
    pub original: String,
    pub replacement: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeclarationPatch {
    pub package: String,
    pub compiler: String,
    pub files: Vec<DeclarationFile>,
}

#[derive(Debug)]
pub struct RegenerationProposal {
    pub patches: Vec<DeclarationPatch>,
    pub messages: Vec<String>,
}

#[derive(Deserialize)]
struct BridgeResult {
    compiler: String,
    files: Vec<BridgeFile>,
    #[serde(default)]
    messages: Vec<String>,
}
#[derive(Deserialize)]
struct BridgeFile {
    root: PathBuf,
    file: PathBuf,
    original: String,
    replacement: String,
}

/// Require a local TypeScript compiler and project tsconfig. The bridge compiles
/// in memory, with `skipLibCheck` disabled, and returns nothing unless the
/// consumer has zero diagnostics after applying all proposed replacements and
/// the declaration export names match the shipped JavaScript's module symbols.
pub fn propose(project: &Path, packages: &[PathBuf]) -> Result<RegenerationProposal> {
    let project = project.canonicalize().context("resolving regeneration project")?;
    let mut roots = BTreeMap::new();
    for package in packages {
        let root = package.canonicalize()?;
        if !root.components().any(|part| part.as_os_str() == "node_modules") {
            continue;
        }
        let manifest = Manifest::read(&root)?;
        roots.insert(root, manifest.id());
    }
    let temp = tempfile::tempdir()?;
    let bridge = temp.path().join("regenerate.cjs");
    let list = temp.path().join("roots.json");
    std::fs::write(&bridge, include_str!("regenerate-bridge.cjs"))?;
    std::fs::write(&list, serde_json::to_vec(&roots.keys().collect::<Vec<_>>())?)?;
    let output = Command::new("node")
        .arg(&bridge)
        .arg(&project)
        .arg(&list)
        .current_dir(&project)
        .output()
        .context("starting local TypeScript compiler")?;
    if !output.status.success() {
        bail!(
            "declaration verification failed: {}",
            String::from_utf8_lossy(&output.stderr).trim(),
        );
    }
    let result: BridgeResult = serde_json::from_slice(&output.stdout)
        .context("reading declaration verification output")?;
    let messages = result.messages.clone();
    Ok(RegenerationProposal { patches: verified_patches(result, &roots)?, messages })
}

fn verified_patches(
    result: BridgeResult,
    roots: &BTreeMap<PathBuf, String>,
) -> Result<Vec<DeclarationPatch>> {
    let mut patches: BTreeMap<String, Vec<DeclarationFile>> = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for file in result.files {
        let Some(package) = roots.get(&file.root) else {
            bail!("regeneration returned an unknown package");
        };
        let canonical = file.file.canonicalize()?;
        let relative =
            canonical.strip_prefix(&file.root).context("regeneration escaped package root")?;
        if roots.keys().any(|other| {
            other != &file.root && other.starts_with(&file.root) && canonical.starts_with(other)
        }) {
            bail!("regeneration assigned a file to an outer package");
        }
        let name = relative.to_string_lossy();
        if ![".d.ts", ".d.mts", ".d.cts"].iter().any(|suffix| name.ends_with(suffix)) {
            bail!("regeneration returned a non-declaration file");
        }
        if file.original == file.replacement {
            bail!("regeneration returned an unchanged file");
        }
        if !seen.insert((package.clone(), relative.to_path_buf())) {
            bail!("regeneration returned a duplicate file");
        }
        if std::fs::read_to_string(&canonical)? != file.original {
            bail!("declaration changed during verification");
        }
        patches.entry(package.clone()).or_default().push(DeclarationFile {
            path: relative.into(),
            original: file.original,
            replacement: file.replacement,
        });
    }
    Ok(patches
        .into_iter()
        .map(|(package, files)| DeclarationPatch {
            package,
            compiler: result.compiler.clone(),
            files,
        })
        .collect())
}

#[cfg(test)]
mod tests;
