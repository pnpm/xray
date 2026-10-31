//! Compare the installed pnpm resolution layout across verification steps.
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
use yaml_rust2::{Yaml, YamlLoader};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Signature {
    node_linker: String,
    hoist_pattern: Option<Vec<String>>,
    public_hoist_pattern: Option<Vec<String>>,
    store_dir: PathBuf,
    virtual_store_dir: PathBuf,
    global_virtual_store: bool,
    included: Option<(bool, bool, bool)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Layout {
    root: PathBuf,
    modules: BTreeMap<PathBuf, Option<Signature>>,
}

impl Layout {
    /// Check journal paths before using a persisted layout for recovery.
    pub fn validate_recovery(&self, workspace_root: &Path) -> Result<()> {
        let root = workspace_root.canonicalize()?;
        if self.root != root || !self.modules.contains_key(&root) {
            bail!("recovery layout workspace does not match the selected workspace");
        }
        for (project, signature) in &self.modules {
            if !project.starts_with(&root) || project.canonicalize()? != *project {
                bail!("invalid importer path in recovery layout");
            }
            if let Some(signature) = signature {
                if !signature.store_dir.is_absolute()
                    || !signature.virtual_store_dir.is_absolute()
                    || signature.store_dir.canonicalize()? != signature.store_dir
                    || signature.virtual_store_dir.canonicalize()? != signature.virtual_store_dir
                {
                    bail!("invalid store path in recovery layout");
                }
                let global = signature
                    .store_dir
                    .join("links")
                    .canonicalize()
                    .is_ok_and(|path| path == signature.virtual_store_dir);
                if global != signature.global_virtual_store {
                    bail!("invalid global store state in recovery layout");
                }
            }
        }
        Ok(())
    }

    pub fn capture(workspace_root: &Path) -> Result<Self> {
        let root = workspace_root.canonicalize()?;
        let modules = read_layout(&root)?;
        if !modules.values().any(Option::is_some) {
            bail!("cannot verify repair: pnpm layout metadata is missing at {}", root.display());
        }
        Ok(Self { root, modules })
    }

    /// Reject a change in layout, including a newly added or removed importer.
    pub fn verify_unchanged(&self) -> Result<()> {
        if read_layout(&self.root)? != self.modules {
            bail!("pnpm installation layout changed during verification; persist the original hoist and virtual store settings and reinstall before retrying");
        }
        Ok(())
    }

    /// Refuse before executing user commands if pnpm's current configuration
    /// would reinstall into a different resolution layout.
    pub fn preflight(&self) -> Result<()> {
        for (project, signature) in &self.modules {
            let Some(signature) = signature else { continue };
            let mut config = crate::process::command(std::ffi::OsStr::new("pnpm"));
            let output = config
                .args(["config", "list", "--json"])
                .current_dir(project)
                .output()
                .context("reading effective pnpm layout settings")?;
            if !output.status.success() {
                bail!("cannot read effective pnpm layout settings");
            }
            // The complete config can contain credentials; never print it.
            let config: serde_json::Value = serde_json::from_slice(&output.stdout)
                .context("parsing effective pnpm layout settings")?;
            let mut store = crate::process::command(std::ffi::OsStr::new("pnpm"));
            let output = store
                .args(["store", "path"])
                .current_dir(project)
                .output()
                .context("reading effective pnpm store path")?;
            if !output.status.success() {
                bail!("cannot read effective pnpm store path");
            }
            let store =
                std::str::from_utf8(&output.stdout).context("invalid pnpm store path")?.trim();
            let store =
                Path::new(store).canonicalize().context("resolving effective pnpm store path")?;
            if !matches_configuration(signature, &config, &store, project)? {
                bail!("installed pnpm layout differs from effective configuration at {}; persist the original hoist and virtual store settings and reinstall before using --verify", project.display());
            }
        }
        Ok(())
    }
}

fn matches_configuration(
    signature: &Signature,
    config: &serde_json::Value,
    store: &Path,
    project: &Path,
) -> Result<bool> {
    let field = |camel: &str, kebab: &str| config.get(camel).or_else(|| config.get(kebab));
    let linker = field("nodeLinker", "node-linker")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("isolated");
    let hoist = field("hoist", "hoist").and_then(serde_json::Value::as_bool).unwrap_or(true);
    let patterns = |camel: &str, kebab: &str| -> Result<Option<Vec<String>>> {
        field(camel, kebab)
            .map(|value| {
                value
                    .as_array()
                    .context("invalid effective pnpm hoist patterns")?
                    .iter()
                    .map(|item| {
                        item.as_str()
                            .map(str::to_owned)
                            .context("invalid effective pnpm hoist pattern")
                    })
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()
    };
    let hoist_pattern = if hoist {
        Some(patterns("hoistPattern", "hoist-pattern")?.unwrap_or_else(|| vec!["*".into()]))
    } else {
        None
    };
    let public = if field("shamefullyHoist", "shamefully-hoist")
        .and_then(serde_json::Value::as_bool)
        == Some(true)
    {
        vec!["*".into()]
    } else {
        patterns("publicHoistPattern", "public-hoist-pattern")?.unwrap_or_default()
    };
    let global = field("enableGlobalVirtualStore", "enable-global-virtual-store")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or_else(|| {
            field("virtualStoreType", "virtual-store-type").and_then(serde_json::Value::as_str)
                == Some("global")
        });
    let expected_virtual = if global {
        store.join("links")
    } else if let Some(dir) =
        field("virtualStoreDir", "virtual-store-dir").and_then(serde_json::Value::as_str)
    {
        project.join(dir)
    } else {
        project.join("node_modules/.pnpm")
    };
    let Ok(expected_virtual) = expected_virtual.canonicalize() else {
        return Ok(false);
    };
    Ok(linker == signature.node_linker
        && hoist_pattern == signature.hoist_pattern
        && Some(public) == signature.public_hoist_pattern
        && global == signature.global_virtual_store
        && store == signature.store_dir
        && expected_virtual == signature.virtual_store_dir)
}

fn read_layout(root: &Path) -> Result<BTreeMap<PathBuf, Option<Signature>>> {
    let mut result = BTreeMap::new();
    for project in crate::workspace::projects(root)? {
        let modules = project.join("node_modules");
        let file = modules.join(".modules.yaml");
        let signature = if file.exists() { Some(read_signature(&file, &modules)?) } else { None };
        result.insert(project, signature);
    }
    Ok(result)
}

fn read_signature(file: &Path, modules: &Path) -> Result<Signature> {
    let raw = std::fs::read_to_string(file)
        .with_context(|| format!("reading pnpm layout {}", file.display()))?;
    let documents = YamlLoader::load_from_str(&raw)
        .with_context(|| format!("parsing pnpm layout {}", file.display()))?;
    let doc = documents.first().context("empty pnpm layout metadata")?;
    let string = |key: &str| -> Result<&str> {
        doc[key].as_str().with_context(|| format!("missing {key} in pnpm layout metadata"))
    };
    let pattern = |key: &str| -> Result<Option<Vec<String>>> {
        match &doc[key] {
            Yaml::BadValue | Yaml::Null => Ok(None),
            value => Ok(Some(
                value
                    .as_vec()
                    .with_context(|| format!("invalid {key} in pnpm layout metadata"))?
                    .iter()
                    .map(|item| {
                        item.as_str().map(str::to_owned).context("non-string pnpm hoist pattern")
                    })
                    .collect::<Result<Vec<_>>>()?,
            )),
        }
    };
    let store_dir =
        Path::new(string("storeDir")?).canonicalize().context("resolving pnpm store directory")?;
    let virtual_store_dir = modules
        .join(string("virtualStoreDir")?)
        .canonicalize()
        .context("resolving pnpm virtual store directory")?;
    let included = match &doc["included"] {
        Yaml::BadValue | Yaml::Null => None,
        value => Some((
            value["dependencies"].as_bool().context("invalid included dependencies")?,
            value["devDependencies"].as_bool().context("invalid included dev dependencies")?,
            value["optionalDependencies"]
                .as_bool()
                .context("invalid included optional dependencies")?,
        )),
    };
    let global_virtual_store =
        store_dir.join("links").canonicalize().is_ok_and(|path| path == virtual_store_dir);
    Ok(Signature {
        node_linker: string("nodeLinker")?.to_owned(),
        hoist_pattern: pattern("hoistPattern")?,
        public_hoist_pattern: pattern("publicHoistPattern")?,
        store_dir,
        virtual_store_dir,
        global_virtual_store,
        included,
    })
}

#[cfg(test)]
#[path = "verify_layout/tests.rs"]
mod tests;
