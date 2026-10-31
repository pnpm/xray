//! Detect conflicting extensions whose selectors overlap a proposed version.
use anyhow::{bail, Context, Result};
use node_semver::{Range, Version};
use yaml_rust2::{yaml::Hash, Yaml, YamlLoader};

pub fn check(existing: &str, proposed: &str) -> Result<()> {
    let existing = YamlLoader::load_from_str(existing).context("parsing existing workspace")?;
    let proposed = YamlLoader::load_from_str(proposed).context("parsing proposed extensions")?;
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
        let version = Version::parse(range.context("generated extension must pin a version")?)
            .context("generated extension must pin a valid version")?;
        for (old_selector, old_value) in old {
            let old_selector =
                old_selector.as_str().context("extension selector must be a string")?;
            let (old_name, old_range) = split_selector(old_selector);
            if old_name != name {
                continue;
            }
            if let Some(range) = old_range {
                let range = Range::parse(range)
                    .with_context(|| format!("cannot assess extension selector {old_selector}"))?;
                if !range.satisfies(&version) {
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
                            name.as_str().unwrap_or("dependency")
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
mod tests {
    use super::check;
    const PROPOSED: &str = "packageExtensions:\n  'example@1.2.3':\n    dependencies:\n      dep: '^2'\n    peerDependencies:\n      peer: '*'\n    peerDependenciesMeta:\n      peer:\n        optional: true\n";

    #[test]
    fn rejects_overlapping_ranges_and_metadata() {
        let conflict = "packageExtensions:\n  'example@^1':\n    dependencies:\n      dep: '^1'\n";
        assert!(check(conflict, PROPOSED).is_err());
        let metadata = "packageExtensions:\n  'example@*':\n    peerDependenciesMeta:\n      peer:\n        optional: false\n";
        assert!(check(metadata, PROPOSED).is_err());
        let different_kind =
            "packageExtensions:\n  example:\n    peerDependencies:\n      dep: '^2'\n";
        assert!(check(different_kind, PROPOSED).is_err());
    }

    #[test]
    fn permits_nonoverlap_and_equal_settings() {
        let other = "packageExtensions:\n  'example@^2':\n    dependencies:\n      dep: '^1'\n";
        assert!(check(other, PROPOSED).is_ok());
        let equal = "packageExtensions:\n  'example@^1':\n    dependencies:\n      dep: '^2'\n";
        assert!(check(equal, PROPOSED).is_ok());
        assert!(check(PROPOSED, PROPOSED).is_ok());
    }

    #[test]
    fn supports_scoped_selectors_and_prereleases() {
        let existing = "packageExtensions:\n  '@scope/pkg@>=1.0.0-beta.1 <1.0.0':\n    dependencies:\n      dep: '1'\n";
        let proposed =
            "packageExtensions:\n  '@scope/pkg@1.0.0-beta.2':\n    dependencies:\n      dep: '2'\n";
        assert!(check(existing, proposed).is_err());
    }
}
