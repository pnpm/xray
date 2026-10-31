use super::projects;

#[test]
fn falls_back_to_package_patterns_with_separate_lockfiles() {
    let root = tempfile::tempdir().unwrap();
    for directory in [
        "packages/app",
        "packages/excluded",
        "packages/app/fixture",
        "unrelated",
        "node_modules/vendor",
    ] {
        let path = root.path().join(directory);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("package.json"), "{}").unwrap();
    }
    std::fs::write(
        root.path().join("pnpm-workspace.yaml"),
        "packages:\n  - 'packages/*'\n  - '!packages/excluded'\nsharedWorkspaceLockfile: false\n",
    )
    .unwrap();
    assert_eq!(
        projects(root.path()).unwrap(),
        vec![
            root.path().canonicalize().unwrap(),
            root.path().join("packages/app").canonicalize().unwrap()
        ],
    );
}

#[test]
fn globstar_and_braces_include_the_expected_projects() {
    let root = tempfile::tempdir().unwrap();
    for directory in ["packages/deep/app", "tools/task", "examples/ignored"] {
        let path = root.path().join(directory);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("package.json"), "{}").unwrap();
    }
    std::fs::write(root.path().join("pnpm-workspace.yaml"), "packages: ['{packages,tools}/**']\n")
        .unwrap();
    let found = projects(root.path()).unwrap();
    assert_eq!(found.len(), 3);
    assert!(found.contains(&root.path().join("packages/deep/app").canonicalize().unwrap()));
    assert!(found.contains(&root.path().join("tools/task").canonicalize().unwrap()));
}
