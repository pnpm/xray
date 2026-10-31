//! Apply generated extensions without replacing unrelated workspace settings.

use anyhow::{bail, Context, Result};
use std::{io::Write as _, path::Path, str::FromStr};
use yaml_edit::{Document, Mapping};
use yaml_rust2::{Yaml, YamlLoader};

/// Merge a proposed block into existing YAML. Conflicts are errors, so existing
/// user decisions never silently change. Both parsers must accept the result.
pub fn merge(existing: &str, proposed: &str) -> Result<String> {
    if proposed.is_empty() {
        return Ok(existing.to_string());
    }
    crate::repair_conflicts::check(existing, proposed)?;
    let before = YamlLoader::load_from_str(existing).context("parsing workspace settings")?;
    if before.len() > 1 {
        bail!("workspace settings must contain one YAML document");
    }
    if before.first().is_some_and(|value| !matches!(value, Yaml::Hash(_) | Yaml::Null)) {
        bail!("workspace settings must be a YAML mapping");
    }
    let input = if before.first().is_none_or(|value| matches!(value, Yaml::Null)) {
        format!("{existing}\n{{}}\n")
    } else {
        existing.to_string()
    };
    let document = Document::from_str(&input).context("reading workspace syntax")?;
    let root = document.as_mapping().context("workspace settings must be a mapping")?;
    let proposals = Document::from_str(proposed).context("reading generated extensions")?;
    let additions = proposals.as_mapping().context("generated extensions must be a mapping")?;
    merge_mapping(&root, &additions, "")?;
    let proposed_documents = YamlLoader::load_from_str(proposed)?;
    if proposed_documents.len() != 1 {
        bail!("generated extensions must contain one document");
    }
    let mut expected = before
        .first()
        .filter(|value| !matches!(value, Yaml::Null))
        .cloned()
        .unwrap_or_else(|| Yaml::Hash(yaml_rust2::yaml::Hash::new()));
    merge_values(&mut expected, &proposed_documents[0])?;
    let result = document.to_string();
    let parsed =
        YamlLoader::load_from_str(&result).context("validating merged workspace settings")?;
    if parsed.len() != 1 || parsed[0].as_hash().is_none() {
        bail!("merged workspace settings must be a single YAML mapping");
    }
    if parsed[0] != expected {
        bail!("workspace edit did not preserve the original settings and proposed repairs");
    }
    Ok(result)
}

fn merge_values(target: &mut Yaml, additions: &Yaml) -> Result<()> {
    match (target, additions) {
        (Yaml::Hash(target), Yaml::Hash(additions)) => {
            for (key, value) in additions {
                if let Some(existing) = target.get_mut(key) {
                    merge_values(existing, value)?;
                } else {
                    target.insert(key.clone(), value.clone());
                }
            }
        }
        (existing, proposed) if existing != proposed => bail!("conflicting workspace setting"),
        _ => {}
    }
    Ok(())
}

fn merge_mapping(target: &Mapping, additions: &Mapping, parent: &str) -> Result<()> {
    for (key, value) in additions.iter() {
        let label = key.to_string();
        let path = if parent.is_empty() { label } else { format!("{parent}.{label}") };
        if let Some(current) = target.get(&key) {
            if let (Some(current), Some(nested)) = (current.as_mapping(), value.as_mapping()) {
                merge_mapping(current, nested, &path)?;
            } else {
                let current = YamlLoader::load_from_str(&current.to_string())?;
                let new = YamlLoader::load_from_str(&value.to_string())?;
                if current != new {
                    bail!("existing setting conflicts with generated repair at {path}");
                }
            }
        } else {
            target.set(&key, &value);
        }
    }
    Ok(())
}

/// Replace the workspace file only after all proposed changes have been checked.
/// A temporary file in the same directory makes the replacement atomic.
pub fn apply(path: &Path, proposed: &str) -> Result<bool> {
    let target = if path.exists() { path.canonicalize()? } else { path.to_path_buf() };
    let original = match std::fs::read_to_string(&target) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    let merged = merge(&original, proposed)?;
    if merged == original {
        return Ok(false);
    }
    let parent = target.parent().context("workspace file has no parent directory")?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    if let Ok(metadata) = std::fs::metadata(&target) {
        temporary.as_file().set_permissions(metadata.permissions())?;
    }
    temporary.write_all(merged.as_bytes())?;
    temporary.as_file().sync_all()?;
    let current = match std::fs::read_to_string(&target) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.into()),
    };
    if current != original {
        bail!("{} changed during repair; rerun xray", path.display());
    }
    temporary.persist(&target).with_context(|| format!("writing {}", path.display()))?;
    Ok(true)
}

#[cfg(test)]
mod tests;
