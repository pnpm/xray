//! Detect conflicting extensions whose selectors overlap a proposed version.
use anyhow::{bail, Context, Result};
use node_semver::{Range, Version};
use yaml_rust2::{yaml::Hash, Yaml, YamlLoader};

pub fn check(existing: &str, proposed: &str) -> Result<()> {
    let existing = YamlLoader::load_from_str(existing).context("parsing existing workspace")?;
    let proposed = YamlLoader::load_from_str(proposed).context("parsing proposed extensions")?;
    if let Some(new) = proposed.first().and_then(|doc| doc["patchedDependencies"].as_hash()) {
        for (key, value) in new {
            let key = key.as_str().context("generated patch key must be a string")?;
            if let Some(user) = shadowed_patch(&existing, key, value) {
                bail!(
                    "existing patchedDependencies entry {user} already patches {key}; \
                     remove it or fold the declaration repair into that patch",
                );
            }
        }
    }
    let Some(old) = existing.first().and_then(|doc| doc["packageExtensions"].as_hash()) else {
        return Ok(());
    };
    let Some(new) = proposed.first().and_then(|doc| doc["packageExtensions"].as_hash()) else {
        return Ok(());
    };
    for (new_selector, new_value) in new {
        let selector =
            new_selector.as_str().context("generated extension selector must be a string")?;
        let (name, range) = split_selector(selector);
        // A selector without one exact version is never generated. If one is
        // proposed anyway, compare it only with selectors that name the
        // package without a range, instead of failing the whole repair.
        let version = range.and_then(|range| Version::parse(range).ok());
        for (old_selector, old_value) in old {
            let old_selector =
                old_selector.as_str().context("extension selector must be a string")?;
            let (old_name, old_range) = split_selector(old_selector);
            if old_name != name {
                continue;
            }
            if let Some(range) = old_range {
                let Some(version) = &version else { continue };
                let range = Range::parse(range)
                    .with_context(|| format!("cannot assess extension selector {old_selector}"))?;
                if !range.satisfies(version) {
                    continue;
                }
            }
            let old_map = old_value
                .as_hash()
                .with_context(|| format!("extension {old_selector} must be a mapping"))?;
            let new_map = new_value.as_hash().context("generated extension must be a mapping")?;
            check_settings(old_map, new_map, old_selector, selector)?;
        }
    }
    Ok(())
}

/// The user's `patchedDependencies` key that pnpm would weigh against a
/// generated `name@version` patch, if any. pnpm prefers an exact version, then
/// a range, then a bare name, and applies only one patch per package, so any
/// existing key that selects the same version would either be displaced by the
/// generated patch or displace it. An identical key with an identical value is
/// the same repair applied again. A key whose range cannot be assessed counts as
/// matching, so a user's patch is never silently overridden.
pub fn existing_patch(existing: &str, package: &str) -> Result<Option<String>> {
    let documents = YamlLoader::load_from_str(existing).context("parsing existing workspace")?;
    Ok(shadowed_patch(&documents, package, &Yaml::Null))
}

fn shadowed_patch(existing: &[Yaml], key: &str, value: &Yaml) -> Option<String> {
    let old = existing.first().and_then(|doc| doc["patchedDependencies"].as_hash())?;
    let (name, version) = split_selector(key);
    let version = version.map(Version::parse);
    for (old_key, old_value) in old {
        let Some(old_key) = old_key.as_str() else { continue };
        let (old_name, old_range) = split_selector(old_key);
        if old_name != name || old_key == key && old_value == value {
            continue;
        }
        let selects = match (old_range, &version) {
            (Some(range), Some(Ok(version))) => {
                Range::parse(range).map_or(true, |range| range.satisfies(version))
            }
            _ => true,
        };
        if selects {
            return Some(old_key.to_string());
        }
    }
    None
}

fn split_selector(selector: &str) -> (&str, Option<&str>) {
    match selector.rfind('@') {
        Some(index) if index > 0 => (&selector[..index], Some(&selector[index + 1..])),
        _ => (selector, None),
    }
}

fn check_settings(old: &Hash, new: &Hash, old_selector: &str, new_selector: &str) -> Result<()> {
    const SECTIONS: [&str; 3] = ["dependencies", "optionalDependencies", "peerDependencies"];
    for section in SECTIONS {
        let Some(additions) = new.get(&Yaml::String(section.into())).and_then(Yaml::as_hash) else {
            continue;
        };
        for (name, value) in additions {
            for old_section in SECTIONS {
                if let Some(existing) = old
                    .get(&Yaml::String(old_section.into()))
                    .and_then(Yaml::as_hash)
                    .and_then(|map| map.get(name))
                {
                    if old_section != section || existing != value {
                        bail!(
                            "existing extension {old_selector} conflicts with {new_selector} on {}",
                            name.as_str().unwrap_or("dependency"),
                        );
                    }
                }
            }
        }
    }
    if let (Some(old_meta), Some(new_meta)) = (
        old.get(&Yaml::String("peerDependenciesMeta".into())).and_then(Yaml::as_hash),
        new.get(&Yaml::String("peerDependenciesMeta".into())).and_then(Yaml::as_hash),
    ) {
        for (name, entry) in new_meta {
            if let Some(current) = old_meta.get(name) {
                check_nested(current, entry).with_context(|| format!("existing extension {old_selector} conflicts with {new_selector} on peer metadata {}", name.as_str().unwrap_or("")))?;
            }
        }
    }
    Ok(())
}

fn check_nested(old: &Yaml, new: &Yaml) -> Result<()> {
    match (old.as_hash(), new.as_hash()) {
        (Some(old), Some(new)) => {
            for (key, value) in new {
                if let Some(existing) = old.get(key) {
                    check_nested(existing, value)?;
                }
            }
        }
        _ if old != new => bail!("different values"),
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests;
