use super::{apply, diff, plan};
use crate::regenerate::{DeclarationFile, DeclarationPatch};
use std::process::Command;

#[test]
fn emits_an_applicable_patch_including_missing_final_newlines() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("index.d.ts"), "export declare const;").unwrap();
    let text = diff("index.d.ts", "export declare const;", "export declare const value: number;\n");
    let path = root.path().join("test.patch");
    std::fs::write(&path, text).unwrap();
    let output = Command::new("git")
        .args(["apply", "--check"])
        .arg(&path)
        .current_dir(root.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
}

#[test]
fn persists_patch_and_settings_idempotently_and_rejects_source_changes() {
    let root = tempfile::tempdir().unwrap();
    let package = root.path().join("node_modules/example");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(package.join("package.json"), r#"{"name":"example","version":"1.0.0"}"#)
        .unwrap();
    std::fs::write(package.join("index.d.ts"), "export declare const;\n").unwrap();
    let patch = DeclarationPatch {
        package: "example@1.0.0".into(),
        compiler: "5.9.3".into(),
        files: vec![DeclarationFile {
            path: "index.d.ts".into(),
            original: "export declare const;\n".into(),
            replacement: "export declare const value: number;\n".into(),
        }],
    };
    let proposal = plan(std::slice::from_ref(&patch), std::slice::from_ref(&package)).unwrap();
    let workspace = root.path().join("pnpm-workspace.yaml");
    std::fs::write(&workspace, "# keep\nhoist: false\n").unwrap();
    assert!(apply(&workspace, &proposal.configuration, &proposal).unwrap());
    assert!(!apply(&workspace, &proposal.configuration, &proposal).unwrap());
    assert!(std::fs::read_to_string(&workspace).unwrap().starts_with("# keep\nhoist: false\n"));
    assert!(root.path().join("patches/example@1.0.0.patch").is_file());
    std::fs::write(package.join("index.d.ts"), "changed\n").unwrap();
    assert!(plan(&[patch], &[package]).is_err());
}

#[test]
fn refuses_conflicting_artifacts_and_paths_without_changing_settings() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("pnpm-workspace.yaml");
    let original = "# existing\nhoist: false\n";
    std::fs::write(&workspace, original).unwrap();
    let proposal = super::PatchPlan {
        configuration: "patchedDependencies:\n  'example@1.0.0': 'patches/example.patch'\n".into(),
        artifacts: std::collections::BTreeMap::from([("../outside.patch".into(), "patch".into())]),
    };
    assert!(apply(&workspace, &proposal.configuration, &proposal).is_err());
    assert_eq!(std::fs::read_to_string(&workspace).unwrap(), original);
    assert!(!root.path().join("patches").exists());
    std::fs::create_dir(root.path().join("patches")).unwrap();
    let path = root.path().join("patches/example.patch");
    std::fs::write(&path, "user patch").unwrap();
    let conflicting = super::PatchPlan {
        configuration: proposal.configuration,
        artifacts: std::collections::BTreeMap::from([(
            "patches/example.patch".into(),
            "generated patch".into(),
        )]),
    };
    assert!(apply(&workspace, &conflicting.configuration, &conflicting).is_err());
    assert_eq!(std::fs::read_to_string(&workspace).unwrap(), original);
    assert_eq!(std::fs::read_to_string(path).unwrap(), "user patch");
}

#[cfg(unix)]
#[test]
fn rejects_symlinked_declarations_and_patch_directories() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let package = root.path().join("node_modules/example");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(package.join("package.json"), r#"{"name":"example","version":"1.0.0"}"#)
        .unwrap();
    std::fs::write(package.join("real.d.ts"), "bad").unwrap();
    symlink("real.d.ts", package.join("index.d.ts")).unwrap();
    let patch = DeclarationPatch {
        package: "example@1.0.0".into(),
        compiler: "5.0.0".into(),
        files: vec![DeclarationFile {
            path: "index.d.ts".into(),
            original: "bad".into(),
            replacement: "good".into(),
        }],
    };
    assert!(plan(&[patch], &[package]).is_err());
    let actual = root.path().join("actual");
    std::fs::create_dir(&actual).unwrap();
    symlink(&actual, root.path().join("patches")).unwrap();
    let proposal = super::PatchPlan {
        configuration: "patchedDependencies:\n  example: patches/example.patch\n".into(),
        artifacts: std::collections::BTreeMap::from([(
            "patches/example.patch".into(),
            "patch".into(),
        )]),
    };
    assert!(apply(&root.path().join("pnpm-workspace.yaml"), &proposal.configuration, &proposal)
        .is_err());
    assert!(!actual.join("example.patch").exists());
}

#[test]
fn dry_run_preflight_rejects_the_same_conflicting_patch_as_apply() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("pnpm-workspace.yaml");
    let original = "packages:\n  - packages/*\n";
    std::fs::write(&workspace, original).unwrap();
    std::fs::create_dir(root.path().join("patches")).unwrap();
    let path = root.path().join("patches/example.patch");
    std::fs::write(&path, "existing patch").unwrap();
    let proposed = "patchedDependencies:\n  'example@1.0.0': patches/example.patch\n";
    let plan = super::PatchPlan {
        configuration: proposed.into(),
        artifacts: std::collections::BTreeMap::from([(
            "patches/example.patch".into(),
            "different patch".into(),
        )]),
    };
    assert!(super::validate(&workspace, proposed, &plan).is_err());
    assert!(apply(&workspace, proposed, &plan).is_err());
    assert_eq!(std::fs::read_to_string(&workspace).unwrap(), original);
    assert_eq!(std::fs::read_to_string(path).unwrap(), "existing patch");
}

#[cfg(unix)]
#[test]
fn unrelated_symlinked_patch_directory_does_not_block_extension_only_repair() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    symlink(outside.path(), root.path().join("patches")).unwrap();
    let workspace = root.path().join("pnpm-workspace.yaml");
    let proposed = "packageExtensions:\n  'example@1.0.0':\n    dependencies:\n      other: '*'\n";
    let plan = super::PatchPlan {
        configuration: String::new(),
        artifacts: std::collections::BTreeMap::new(),
    };
    super::validate(&workspace, proposed, &plan).unwrap();
    assert!(apply(&workspace, proposed, &plan).unwrap());
    assert!(std::fs::read_to_string(workspace).unwrap().contains("example@1.0.0"));
}
