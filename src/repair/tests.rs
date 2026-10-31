use super::{apply, merge};
use yaml_rust2::YamlLoader;

const PROPOSAL: &str =
    "packageExtensions:\n  'example@1.0.0':\n    dependencies:\n      helper: '^2'\n";

#[test]
fn preserves_comments_settings_and_existing_extensions() {
    let existing = "# workspace comment\npackages: # package list\n  - packages/*\nhoist: false\npackageExtensions:\n  'other@2': # retain this\n    dependencies:\n      existing: '~1'\n";
    let merged = merge(existing, PROPOSAL).unwrap();
    for text in
        ["# workspace comment", "# package list", "# retain this", "hoist: false", "- packages/*"]
    {
        assert!(merged.contains(text), "{merged}");
    }
    let parsed = YamlLoader::load_from_str(&merged).unwrap();
    assert_eq!(
        parsed[0]["packageExtensions"]["example@1.0.0"]["dependencies"]["helper"].as_str(),
        Some("^2"),
    );
    assert_eq!(
        parsed[0]["packageExtensions"]["other@2"]["dependencies"]["existing"].as_str(),
        Some("~1"),
    );
    assert_eq!(merge(&merged, PROPOSAL).unwrap(), merged);
}

#[test]
fn merges_into_existing_selector_without_overwriting_other_fields() {
    let original =
        "packageExtensions:\n  'example@1.0.0':\n    peerDependencies:\n      integration: '*'\n";
    let merged = merge(original, PROPOSAL).unwrap();
    let parsed = YamlLoader::load_from_str(&merged).unwrap();
    let extension = &parsed[0]["packageExtensions"]["example@1.0.0"];
    assert_eq!(extension["peerDependencies"]["integration"].as_str(), Some("*"));
    assert_eq!(extension["dependencies"]["helper"].as_str(), Some("^2"));
}

#[test]
fn conflicting_or_invalid_settings_leave_file_untouched() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pnpm-workspace.yaml");
    for original in [
        "packageExtensions:\n  'example@1.0.0':\n    dependencies:\n      helper: '^1'\n",
        "packageExtensions: false\n",
        "packages: [\n",
        "a: 1\n---\nb: 2\n",
    ] {
        std::fs::write(&path, original).unwrap();
        assert!(apply(&path, PROPOSAL).is_err(), "{original}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    }
}

#[test]
fn creates_workspace_file_and_does_not_rewrite_an_identical_repair() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pnpm-workspace.yaml");
    assert!(!apply(&path, "").unwrap());
    assert!(!path.exists());
    assert!(apply(&path, PROPOSAL).unwrap());
    assert!(!apply(&path, PROPOSAL).unwrap());
}

#[test]
fn supports_flow_maps_and_retains_anchor_settings() {
    let existing = "shared: &settings {enabled: true}\ncopy: *settings\npackageExtensions: {'other@1': {dependencies: {existing: '^1'}}}\n";
    let merged = merge(existing, PROPOSAL).unwrap();
    assert!(merged.contains("&settings"), "{merged}");
    assert!(merged.contains("*settings"), "{merged}");
    let parsed = YamlLoader::load_from_str(&merged).unwrap();
    assert_eq!(parsed[0]["copy"]["enabled"].as_bool(), Some(true));
    assert_eq!(
        parsed[0]["packageExtensions"]["other@1"]["dependencies"]["existing"].as_str(),
        Some("^1"),
    );
}

#[test]
fn preserves_comments_when_creating_first_setting() {
    let merged = merge("# keep this note\n", PROPOSAL).unwrap();
    assert!(merged.starts_with("# keep this note\n"), "{merged}");
    assert_eq!(merge(&merged, PROPOSAL).unwrap(), merged);
}
