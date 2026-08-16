use anyhow::{Context, Result};
use serde::Deserialize;
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
}
