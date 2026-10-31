use super::{reports_from_events, run};
use std::process::Command;

#[test]
fn observes_caught_dynamic_resolution_failures_from_installed_packages() {
    let supported = Command::new("node")
        .args([
            "-e",
            "process.exit(typeof require('node:module').registerHooks === 'function' ? 0 : 1)",
        ])
        .status()
        .is_ok_and(|status| status.success());
    if !supported {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let package = temp.path().join("node_modules/example");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        package.join("package.json"),
        r#"{"name":"example","version":"1.0.0","devDependencies":{"missing-runtime":"^2"}}"#,
    )
    .unwrap();
    std::fs::write(
        package.join("index.cjs"),
        "const dep = 'missing-' + 'runtime'; try { require(dep); } catch {}\n",
    )
    .unwrap();
    let result =
        run(&["node".into(), package.join("index.cjs").into_os_string()], &[package], temp.path())
            .unwrap();
    assert!(result.status.success());
    assert_eq!(result.reports.len(), 1);
    let finding = &result.reports[0].findings[0];
    assert_eq!(finding.dependency, "missing-runtime");
    assert_eq!(finding.declared_range.as_deref(), Some("^2"));
    assert_eq!(finding.evidence.first().unwrap().file, "index.cjs");
}

#[test]
fn diagnoses_declared_dependencies_and_ignores_workspace_imports() {
    let temp = tempfile::tempdir().unwrap();
    let package = temp.path().join("node_modules/example");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        package.join("package.json"),
        r#"{"name":"example","version":"1.0.0","dependencies":{"declared":"1"}}"#,
    )
    .unwrap();
    let installed_file = package.join("index.js");
    let workspace_file = temp.path().join("app.js");
    std::fs::write(&installed_file, "").unwrap();
    std::fs::write(&workspace_file, "").unwrap();
    let raw = format!(
        "{}\n{}\n",
        serde_json::json!({"specifier":"declared", "parent":installed_file}),
        serde_json::json!({"specifier":"missing", "parent":workspace_file}),
    );
    let (reports, diagnostics) = reports_from_events(&raw, &[package]).unwrap();
    assert!(reports.is_empty());
    assert_eq!(diagnostics.len(), 1);
    assert!(diagnostics[0].message.contains("declared"));
}
#[test]
fn preserves_nonzero_exit_status_and_observes_dynamic_esm_import() {
    if !Command::new("node")
        .args([
            "-e",
            "process.exit(typeof require('node:module').registerHooks === 'function' ? 0 : 1)",
        ])
        .status()
        .is_ok_and(|s| s.success())
    {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let package = temp.path().join("node_modules/example");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(package.join("package.json"), r#"{"name":"example","version":"1.0.0"}"#)
        .unwrap();
    std::fs::write(
        package.join("index.mjs"),
        "try { await import('missing-' + 'esm'); } catch {} process.exitCode = 7;\n",
    )
    .unwrap();
    let result =
        run(&["node".into(), package.join("index.mjs").into_os_string()], &[package], temp.path())
            .unwrap();
    assert_eq!(result.status.code(), Some(7));
    assert_eq!(result.reports[0].findings[0].dependency, "missing-esm");
}
#[test]
fn child_command_runs_from_project_directory() {
    let temp = tempfile::tempdir().unwrap();
    let marker = temp.path().join("marker");
    std::fs::write(marker, "").unwrap();
    let result = run(
        &[
            "node".into(),
            "-e".into(),
            "process.exit(require('fs').existsSync('marker') ? 0 : 8)".into(),
        ],
        &[],
        temp.path(),
    )
    .unwrap();
    assert!(result.status.success());
}

#[test]
fn diagnoses_blocked_declared_subpath_without_a_phantom_fix() {
    if !Command::new("node")
        .args([
            "-e",
            "process.exit(typeof require('node:module').registerHooks === 'function' ? 0 : 1)",
        ])
        .status()
        .is_ok_and(|s| s.success())
    {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let modules = temp.path().join("node_modules");
    let package = modules.join("example");
    let dependency = package.join("node_modules/dep");
    std::fs::create_dir_all(&dependency).unwrap();
    std::fs::write(
        package.join("package.json"),
        r#"{"name":"example","version":"1.0.0","dependencies":{"dep":"1"}}"#,
    )
    .unwrap();
    std::fs::write(package.join("index.cjs"), "try { require('dep/private'); } catch {}\n")
        .unwrap();
    std::fs::write(
        dependency.join("package.json"),
        r#"{"name":"dep","version":"1.0.0","exports":{}}"#,
    )
    .unwrap();
    let result =
        run(&["node".into(), package.join("index.cjs").into_os_string()], &[package], temp.path())
            .unwrap();
    assert!(result.status.success());
    assert!(result.reports.is_empty());
    assert_eq!(result.diagnostics.len(), 1);
    assert!(result.diagnostics[0].message.contains("ERR_PACKAGE_PATH_NOT_EXPORTED"));
    assert!(result.diagnostics[0].message.contains("dep/private"));
}

#[test]
fn diagnoses_undefined_package_import_mapping() {
    if !Command::new("node")
        .args([
            "-e",
            "process.exit(typeof require('node:module').registerHooks === 'function' ? 0 : 1)",
        ])
        .status()
        .is_ok_and(|s| s.success())
    {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let package = temp.path().join("node_modules/example");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(package.join("package.json"), r#"{"name":"example","version":"1.0.0"}"#)
        .unwrap();
    std::fs::write(package.join("index.mjs"), "try { await import('#missing'); } catch {}\n")
        .unwrap();
    let result =
        run(&["node".into(), package.join("index.mjs").into_os_string()], &[package], temp.path())
            .unwrap();
    assert!(result.reports.is_empty());
    assert_eq!(result.diagnostics.len(), 1);
    assert!(result.diagnostics[0].message.contains("ERR_PACKAGE_IMPORT_NOT_DEFINED"));
}
