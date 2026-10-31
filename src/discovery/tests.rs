use crate::discovery::installed_packages;
#[cfg(unix)]
use crate::fixtures::global_virtual_store;

#[cfg(unix)]
#[test]
fn walks_from_the_project_into_every_store_entry() {
    let root = global_virtual_store();
    let found = installed_packages(&root.path().join("project")).unwrap();
    let names: Vec<_> =
        found.iter().map(|dir| dir.file_name().unwrap().to_str().unwrap()).collect();
    assert_eq!(names, ["app", "helper"]);
}

#[test]
fn a_project_with_nothing_installed_says_so() {
    let root = tempfile::tempdir().unwrap();
    let error = installed_packages(root.path()).unwrap_err().to_string();
    assert!(error.contains("no installed dependencies"), "{error}");
}

#[test]
fn discovers_nested_node_modules() {
    let root = tempfile::tempdir().unwrap();
    let parent = root.path().join("node_modules/parent");
    let child = parent.join("node_modules/child");
    std::fs::create_dir_all(&child).unwrap();
    std::fs::write(parent.join("package.json"), "{}").unwrap();
    std::fs::write(child.join("package.json"), "{}").unwrap();
    let found = installed_packages(root.path()).unwrap();
    assert!(found.contains(&parent.canonicalize().unwrap()));
    assert!(found.contains(&child.canonicalize().unwrap()));
}

#[test]
fn discovers_dependencies_used_only_by_workspace_importers() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("node_modules")).unwrap();
    let dependency = root.path().join("packages/member/node_modules/helper");
    std::fs::create_dir_all(&dependency).unwrap();
    std::fs::write(dependency.join("package.json"), "{}").unwrap();
    std::fs::write(
        root.path().join("pnpm-lock.yaml"),
        "importers:\n  .: {}\n  packages/member: {}\n",
    )
    .unwrap();
    let found = installed_packages(root.path()).unwrap();
    assert_eq!(found, vec![dependency.canonicalize().unwrap()]);
}

#[cfg(unix)]
#[test]
fn traverses_workspace_links_without_scanning_authored_packages() {
    let root = tempfile::tempdir().unwrap();
    let member = root.path().join("packages/member");
    let dependency = member.join("node_modules/helper");
    std::fs::create_dir_all(&dependency).unwrap();
    std::fs::create_dir(root.path().join("node_modules")).unwrap();
    std::fs::write(member.join("package.json"), r#"{"name":"member","private":true}"#).unwrap();
    std::fs::write(dependency.join("package.json"), r#"{"name":"helper","version":"1.0.0"}"#)
        .unwrap();
    std::fs::write(
        root.path().join("pnpm-lock.yaml"),
        "importers:\n  .: {}\n  packages/member: {}\n",
    )
    .unwrap();
    std::os::unix::fs::symlink(&member, root.path().join("node_modules/member")).unwrap();
    assert_eq!(installed_packages(root.path()).unwrap(), vec![dependency.canonicalize().unwrap()]);
}

#[cfg(unix)]
#[test]
fn workspace_sources_are_excluded_when_the_lockfile_has_no_importers() {
    let root = tempfile::tempdir().unwrap();
    let member = root.path().join("packages/member");
    let dependency = member.join("node_modules/helper");
    std::fs::create_dir_all(&dependency).unwrap();
    std::fs::create_dir(root.path().join("node_modules")).unwrap();
    std::fs::write(root.path().join("pnpm-workspace.yaml"), "packages: [packages/*]\n").unwrap();
    std::fs::write(member.join("package.json"), r#"{"name":"member","private":true}"#).unwrap();
    std::fs::write(dependency.join("package.json"), r#"{"name":"helper","version":"1.0.0"}"#)
        .unwrap();
    std::os::unix::fs::symlink(&member, root.path().join("node_modules/member")).unwrap();
    assert_eq!(installed_packages(root.path()).unwrap(), vec![dependency.canonicalize().unwrap()]);
}

#[test]
fn discovers_member_dependencies_without_a_root_modules_directory() {
    let root = tempfile::tempdir().unwrap();
    let dependency = root.path().join("packages/member/node_modules/helper");
    std::fs::create_dir_all(&dependency).unwrap();
    std::fs::write(
        root.path().join("pnpm-workspace.yaml"),
        "packages: [packages/*]\nsharedWorkspaceLockfile: false\n",
    )
    .unwrap();
    std::fs::write(root.path().join("packages/member/package.json"), "{}").unwrap();
    std::fs::write(dependency.join("package.json"), "{}").unwrap();
    assert_eq!(installed_packages(root.path()).unwrap(), vec![dependency.canonicalize().unwrap()]);
}

#[test]
fn hidden_tool_metadata_is_not_an_installed_package() {
    let root = tempfile::tempdir().unwrap();
    let cache = root.path().join("node_modules/.cache");
    std::fs::create_dir_all(&cache).unwrap();
    std::fs::write(cache.join("package.json"), r#"{"type":"module"}"#).unwrap();
    assert!(installed_packages(root.path()).unwrap().is_empty());
}

#[cfg(unix)]
#[test]
fn broken_package_links_cannot_produce_a_clean_scan() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("node_modules")).unwrap();
    std::os::unix::fs::symlink(root.path().join("gone"), root.path().join("node_modules/missing"))
        .unwrap();
    let error = installed_packages(root.path()).unwrap_err().to_string();
    assert!(error.contains("broken link"), "{error}");
}
