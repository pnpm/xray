use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::Value;
use std::{collections::BTreeMap, path::Path};

type DepMap = BTreeMap<String, String>;

pub struct EntryPath {
    pub path: String,
    pub export_key: Option<String>,
}

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
    #[serde(default, deserialize_with = "deserialize_bundled")]
    pub bundle_dependencies: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_bundled")]
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
    pub types_versions: Value,
    #[serde(default)]
    pub bin: Value,
    #[serde(default)]
    pub exports: Value,
    #[serde(default)]
    pub imports: Value,
}

// npm also accepts `bundleDependencies: true`, meaning all dependencies are
// bundled, and `false`, meaning none are. Both are already accounted for by
// the dependency maps; only an explicit list contributes additional names.
fn deserialize_bundled<'de, Deserializer>(
    deserializer: Deserializer,
) -> Result<Vec<String>, Deserializer::Error>
where
    Deserializer: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Bundled {
        Names(Vec<String>),
        Flag(bool),
    }
    match Bundled::deserialize(deserializer)? {
        Bundled::Names(names) => Ok(names),
        Bundled::Flag(_flag) => Ok(Vec::new()),
    }
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

    /// Whether the package says which of its files a consumer may reach.
    pub fn is_encapsulated(&self) -> bool {
        !self.exports.is_null()
    }

    /// Every relative path the manifest names as a way into the package.
    ///
    /// `exports` supersedes `main` and `module` for resolution, so a stale
    /// `main` left beside it points at a file consumers cannot reach and must
    /// not be followed. `bin` and `types` are not superseded: a command is an
    /// entry however the package exports its modules, and a compiler still
    /// reads `types` when no export condition names one.
    pub fn entry_paths(&self) -> Vec<EntryPath> {
        let mut paths: Vec<EntryPath> = Vec::new();
        let mut add_paths = |value: &Value, export_key: Option<&str>| {
            let mut targets = Vec::new();
            collect_paths(value, &mut targets);
            paths.extend(
                targets
                    .into_iter()
                    .map(|path| EntryPath { path, export_key: export_key.map(str::to_string) }),
            );
        };
        if let Some(map) =
            self.exports.as_object().filter(|map| map.keys().any(|key| key.starts_with('.')))
        {
            for (key, value) in map {
                add_paths(value, Some(key));
            }
        } else if self.is_encapsulated() {
            add_paths(&self.exports, Some("."));
        } else {
            for field in [&self.main, &self.module, &self.browser] {
                add_paths(field, None);
            }
        }
        for field in [&self.types, &self.typings, &self.bin, &self.types_versions] {
            add_paths(field, None);
        }
        paths
    }

    /// Only the most specific export key applies to a requested subpath.
    pub fn selects_export(&self, key: &str, subpath: &str) -> bool {
        let Some(map) = self.exports.as_object() else {
            return true;
        };
        if map.contains_key(subpath) {
            return key == subpath;
        }
        let best = map
            .keys()
            .filter_map(|candidate| {
                let (prefix, suffix) = candidate.split_once('*')?;
                let middle = subpath.strip_prefix(prefix)?.strip_suffix(suffix)?;
                (!middle.is_empty() && !suffix.contains('*')).then_some((candidate, prefix.len()))
            })
            .max_by_key(|(candidate, prefix_len)| (*prefix_len, candidate.len()));
        best.is_none_or(|(candidate, _)| candidate == key)
    }

    /// Resolve the most specific package-import mapping, keeping each condition
    /// as a possible target because consumers may enable different conditions.
    pub fn import_targets(&self, specifier: &str) -> Vec<String> {
        let Some(map) = self.imports.as_object() else {
            return Vec::new();
        };
        let selected = if let Some(value) = map.get(specifier) {
            Some((value, None))
        } else {
            map.iter()
                .filter_map(|(key, value)| {
                    let (prefix, suffix) = key.split_once('*')?;
                    if suffix.contains('*') {
                        return None;
                    }
                    let matched = specifier.strip_prefix(prefix)?.strip_suffix(suffix)?;
                    (!matched.is_empty()).then_some((value, matched, prefix.len(), key.len()))
                })
                .max_by_key(|(_, _, prefix_len, key_len)| (*prefix_len, *key_len))
                .map(|(value, matched, _, _)| (value, Some(matched)))
        };
        let Some((value, matched)) = selected else {
            return Vec::new();
        };
        let mut targets = Vec::new();
        collect_paths(value, &mut targets);
        if let Some(matched) = matched {
            for target in &mut targets {
                *target = target.replace('*', matched);
            }
        }
        targets
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
