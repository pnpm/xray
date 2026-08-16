use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::Value;
use std::{collections::BTreeMap, path::Path};

type DepMap = BTreeMap<String, String>;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub dependencies: DepMap,
    #[serde(default)]
    pub dev_dependencies: DepMap,
    #[serde(default)]
    pub peer_dependencies: DepMap,
    #[serde(default)]
    pub optional_dependencies: DepMap,
    #[serde(default)]
    pub bundle_dependencies: Vec<String>,
    #[serde(default)]
    pub bundled_dependencies: Vec<String>,
    /// Entry-point fields, kept as raw JSON because each one is a string in some
    /// packages and a nested map of conditions or subpaths in others. Only the
    /// string leaves matter here: they name files.
    #[serde(default)]
    pub main: Value,
    #[serde(default)]
    pub module: Value,
    #[serde(default)]
    pub browser: Value,
    #[serde(default)]
    pub types: Value,
    #[serde(default)]
    pub typings: Value,
    #[serde(default)]
    pub bin: Value,
    #[serde(default)]
    pub exports: Value,
}

impl Manifest {
    pub fn read(package_dir: &Path) -> Result<Self> {
        let path = package_dir.join("package.json");
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))
    }

    /// Every name the package may reach at runtime without pnpm considering it a
    /// phantom: what it declared, plus its own name for self-referencing imports.
    pub fn reachable(&self) -> impl Iterator<Item = &str> {
        self.dependencies
            .keys()
            .chain(self.peer_dependencies.keys())
            .chain(self.optional_dependencies.keys())
            .chain(self.bundle_dependencies.iter())
            .chain(self.bundled_dependencies.iter())
            .map(String::as_str)
            .chain(std::iter::once(self.name.as_str()))
    }

    pub fn id(&self) -> String {
        format!("{}@{}", self.name, self.version)
    }

    /// Every relative path the manifest names as a way into the package.
    pub fn entry_paths(&self) -> Vec<String> {
        let mut paths = Vec::new();
        for field in [
            &self.main,
            &self.module,
            &self.browser,
            &self.types,
            &self.typings,
            &self.bin,
            &self.exports,
        ] {
            collect_paths(field, &mut paths);
        }
        paths
    }
}

/// Every string leaf under an entry-point field. `exports` nests subpaths inside
/// conditions to arbitrary depth and `bin` maps command names to files, so the
/// shape varies; what is wanted from all of them is the same.
fn collect_paths(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(path) => out.push(path.clone()),
        Value::Object(map) => {
            for nested in map.values() {
                collect_paths(nested, out);
            }
        }
        Value::Array(items) => {
            for nested in items {
                collect_paths(nested, out);
            }
        }
        _ => {}
    }
}
