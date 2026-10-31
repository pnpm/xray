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

#[test]
fn keeps_workspace_members_when_the_root_lockfile_lists_only_the_root() {
    let root = tempfile::tempdir().unwrap();
    let lockfile = "lockfileVersion: '9.0'\nimporters:\n  .: {}\n";
    for directory in ["packages/app", "packages/lib", "unrelated"] {
        let path = root.path().join(directory);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("package.json"), "{}").unwrap();
        std::fs::write(path.join("pnpm-lock.yaml"), lockfile).unwrap();
    }
    std::fs::write(root.path().join("package.json"), "{}").unwrap();
    std::fs::write(root.path().join("pnpm-lock.yaml"), lockfile).unwrap();
    std::fs::write(
        root.path().join("pnpm-workspace.yaml"),
        "packages:\n  - 'packages/*'\nsharedWorkspaceLockfile: false\n",
    )
    .unwrap();
    assert_eq!(
        projects(root.path()).unwrap(),
        vec![
            root.path().canonicalize().unwrap(),
            root.path().join("packages/app").canonicalize().unwrap(),
            root.path().join("packages/lib").canonicalize().unwrap(),
        ],
    );
}

#[test]
fn unions_lockfile_importers_with_workspace_patterns() {
    let root = tempfile::tempdir().unwrap();
    for directory in ["packages/app", "tools/script", "fixtures/sample"] {
        let path = root.path().join(directory);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("package.json"), "{}").unwrap();
    }
    std::fs::write(
        root.path().join("pnpm-lock.yaml"),
        "lockfileVersion: '9.0'\nimporters:\n  .: {}\n  tools/script: {}\n",
    )
    .unwrap();
    std::fs::write(root.path().join("pnpm-workspace.yaml"), "packages: ['packages/*']\n").unwrap();
    let found = projects(root.path()).unwrap();
    assert_eq!(found.len(), 3);
    assert!(found.contains(&root.path().join("packages/app").canonicalize().unwrap()));
    assert!(found.contains(&root.path().join("tools/script").canonicalize().unwrap()));
}

#[test]
fn lockfile_importers_suffice_when_the_workspace_has_no_package_patterns() {
    let root = tempfile::tempdir().unwrap();
    for directory in ["tools/script", "fixtures/sample"] {
        let path = root.path().join(directory);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("package.json"), "{}").unwrap();
    }
    std::fs::write(
        root.path().join("pnpm-lock.yaml"),
        "lockfileVersion: '9.0'\nimporters:\n  .: {}\n  tools/script: {}\n",
    )
    .unwrap();
    std::fs::write(root.path().join("pnpm-workspace.yaml"), "catalog: {}\n").unwrap();
    assert_eq!(
        projects(root.path()).unwrap(),
        vec![
            root.path().canonicalize().unwrap(),
            root.path().join("tools/script").canonicalize().unwrap(),
        ],
    );
}
