use super::check;
const PROPOSED: &str = "packageExtensions:\n  'example@1.2.3':\n    dependencies:\n      dep: '^2'\n    peerDependencies:\n      peer: '*'\n    peerDependenciesMeta:\n      peer:\n        optional: true\n";

#[test]
fn rejects_overlapping_ranges_and_metadata() {
    let conflict = "packageExtensions:\n  'example@^1':\n    dependencies:\n      dep: '^1'\n";
    assert!(check(conflict, PROPOSED).is_err());
    let metadata = "packageExtensions:\n  'example@*':\n    peerDependenciesMeta:\n      peer:\n        optional: false\n";
    assert!(check(metadata, PROPOSED).is_err());
    let different_kind = "packageExtensions:\n  example:\n    peerDependencies:\n      dep: '^2'\n";
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

#[test]
fn wildcard_does_not_overlap_prerelease_but_unversioned_selector_does() {
    let proposed =
        "packageExtensions:\n  'example@1.0.0-beta.1':\n    dependencies:\n      dep: '2'\n";
    let wildcard = "packageExtensions:\n  'example@*':\n    dependencies:\n      dep: '1'\n";
    let unversioned = "packageExtensions:\n  example:\n    dependencies:\n      dep: '1'\n";
    assert!(check(wildcard, proposed).is_ok());
    assert!(check(unversioned, proposed).is_err());
}

#[test]
fn a_selector_without_an_exact_version_does_not_fail_the_check() {
    let existing = "packageExtensions:\n  'example@^1':\n    dependencies:\n      dep: '1'\n";
    for selector in ["example@", "example@1.0", "example"] {
        let proposed =
            format!("packageExtensions:\n  '{selector}':\n    dependencies:\n      dep: '2'\n");
        assert!(check(existing, &proposed).is_ok(), "{selector}");
    }
    let unversioned = "packageExtensions:\n  example:\n    dependencies:\n      dep: '1'\n";
    let proposed = "packageExtensions:\n  'example@1.0':\n    dependencies:\n      dep: '2'\n";
    assert!(check(unversioned, proposed).is_err());
}

#[test]
fn refuses_a_generated_patch_that_a_user_patch_already_selects() {
    let proposed = "patchedDependencies:\n  'example@1.2.3': 'patches/example@1.2.3.patch'\n";
    for existing in [
        "patchedDependencies:\n  example: patches/user.patch\n",
        "patchedDependencies:\n  'example@^1': patches/user.patch\n",
        "patchedDependencies:\n  'example@1.2.3': patches/user.patch\n",
        "patchedDependencies:\n  'example@not a range': patches/user.patch\n",
    ] {
        let error = check(existing, proposed).unwrap_err().to_string();
        assert!(error.contains("already patches example@1.2.3"), "{existing}: {error}");
        assert!(super::existing_patch(existing, "example@1.2.3").unwrap().is_some());
    }
}

#[test]
fn permits_patches_for_other_versions_and_the_same_patch_again() {
    let proposed = "patchedDependencies:\n  'example@1.2.3': 'patches/example@1.2.3.patch'\n";
    for existing in [
        "patchedDependencies:\n  'example@^2': patches/user.patch\n",
        "patchedDependencies:\n  'example@1.2.4': patches/user.patch\n",
        "patchedDependencies:\n  example-other: patches/user.patch\n",
        "patchedDependencies:\n  '@scope/example': patches/user.patch\n",
        proposed,
    ] {
        assert!(check(existing, proposed).is_ok(), "{existing}");
    }
    assert!(super::existing_patch("hoist: false\n", "example@1.2.3").unwrap().is_none());
}
