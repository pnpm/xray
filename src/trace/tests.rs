use super::{
    file_url_for_platform, reports_from_events, run_interactive, trace_owners, TraceResult,
};
use anyhow::Result;
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::AtomicBool,
};

/// Run a traced command that is never cancelled.
fn run(command: &[OsString], packages: &[PathBuf], project: &Path) -> Result<TraceResult> {
    run_interactive(command, packages, project, &AtomicBool::new(false))
}

#[test]
fn file_urls_support_windows_drive_verbatim_unc_and_escape_special_characters() {
    assert_eq!(
        file_url_for_platform(r"C:\Temp\file #1.mjs", true).unwrap(),
        "file:///C:/Temp/file%20%231.mjs",
    );
    assert_eq!(
        file_url_for_platform(r"\\?\C:\Temp\hook.mjs", true).unwrap(),
        "file:///C:/Temp/hook.mjs",
    );
    assert_eq!(
        file_url_for_platform(r"\\server\share\hook.mjs", true).unwrap(),
        "file://server/share/hook.mjs",
    );
    assert_eq!(
        file_url_for_platform(r"\\?\UNC\server\share\hook.mjs", true).unwrap(),
        "file://server/share/hook.mjs",
    );
    assert_eq!(
        file_url_for_platform("/tmp/é?#.mjs", false).unwrap(),
        "file:///tmp/%C3%A9%3F%23.mjs",
    );
    assert!(file_url_for_platform("relative/hook.mjs", true).is_err());
}

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
    let owners = trace_owners(&[package], &[]).unwrap();
    let observed = reports_from_events(raw.as_bytes(), &owners);
    let (reports, diagnostics) = (observed.reports, observed.diagnostics);
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

#[cfg(unix)]
#[test]
fn shell_signal_status_is_preserved() {
    let status = Command::new("sh").args(["-c", "kill -TERM $$"]).status().unwrap();
    assert_eq!(super::status_code(status), 143);
}

#[test]
fn custom_resolver_options_are_diagnostic_only_and_preserve_getter_behavior() {
    if !Command::new("node")
        .args([
            "-e",
            "process.exit(typeof require('node:module').registerHooks === 'function' ? 0 : 1)",
        ])
        .status()
        .is_ok_and(|status| status.success())
    {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let package = temp.path().join("node_modules/example");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(package.join("package.json"), r#"{"name":"example","version":"1.0.0"}"#)
        .unwrap();
    let script = package.join("index.cjs");
    let marker = temp.path().join("observed.json");
    std::fs::write(
        &script,
        r"
let reads = 0;
const errors = [];
try { require.resolve('custom-missing', { get paths() { reads++; return []; } }); }
catch (error) { errors.push([error.code, error.message]); }
try { require('ordinary-missing'); }
catch (error) { errors.push([error.code, error.message]); }
require('node:fs').writeFileSync(process.argv[2], JSON.stringify({reads, errors}));
process.exitCode = 7;
",
    )
    .unwrap();
    // Node's hook-enabled resolver itself reads options differently from its
    // legacy path. Compare against an inert public hook to detect extra reads
    // introduced by xray's context tracking rather than by Node's hook API.
    let baseline = Command::new("node")
        .args(["--import", "data:text/javascript,import{registerHooks}from'node:module';registerHooks({resolve(s,c,n){return n(s,c)}})"])
        .arg(&script)
        .arg(&marker)
        .status()
        .unwrap();
    let expected = std::fs::read_to_string(&marker).unwrap();
    let observed = run(
        &["node".into(), script.into_os_string(), marker.clone().into_os_string()],
        &[package],
        temp.path(),
    )
    .unwrap();
    assert_eq!(baseline.code(), Some(7));
    assert_eq!(observed.status.code(), baseline.code());
    assert_eq!(std::fs::read_to_string(marker).unwrap(), expected);
    assert_eq!(observed.reports.len(), 1);
    assert_eq!(observed.reports[0].findings.len(), 1);
    assert_eq!(observed.reports[0].findings[0].dependency, "ordinary-missing");
    assert_eq!(observed.diagnostics.len(), 1);
    assert!(observed.diagnostics[0].message.contains("Explicit resolver options"));
    assert!(observed.diagnostics[0].message.contains("custom-missing"));
}

#[test]
fn diagnoses_workspace_failures_without_proposing_dependency_extensions() {
    if !Command::new("node")
        .args([
            "-e",
            "process.exit(typeof require('node:module').registerHooks === 'function' ? 0 : 1)",
        ])
        .status()
        .is_ok_and(|status| status.success())
    {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    // Explicit workspace membership wins even inside a node_modules directory.
    let project = temp.path().join("node_modules/workspace-checkout");
    let workspace = project.join("packages/app");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(project.join("pnpm-workspace.yaml"), "packages: ['packages/*']\n").unwrap();
    std::fs::write(
        workspace.join("package.json"),
        r#"{"name":"workspace-fixture","version":"1.0.0"}"#,
    )
    .unwrap();
    let script = workspace.join("index.cjs");
    std::fs::write(
        &script,
        "try { require('workspace-missing'); } catch {} process.exitCode = 7;\n",
    )
    .unwrap();
    let observed = run(&["node".into(), script.into_os_string()], &[], &project).unwrap();
    assert_eq!(observed.status.code(), Some(7));
    assert!(observed.reports.is_empty());
    assert_eq!(observed.diagnostics.len(), 1);
    assert_eq!(observed.diagnostics[0].package, "workspace-fixture@1.0.0");
    assert_eq!(observed.diagnostics[0].file, "index.cjs");
    assert!(observed.diagnostics[0].message.contains("Workspace source"));
    assert!(observed.diagnostics[0].message.contains("workspace-missing"));
}

#[test]
fn command_can_remove_a_manifest_without_changing_its_status_or_trace_ownership() {
    if !Command::new("node")
        .args([
            "-e",
            "process.exit(typeof require('node:module').registerHooks === 'function' ? 0 : 1)",
        ])
        .status()
        .is_ok_and(|status| status.success())
    {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("package.json"),
        r#"{"name":"changing-workspace","version":"1.0.0"}"#,
    )
    .unwrap();
    let entry = root.path().join("index.cjs");
    std::fs::write(&entry, "try { require('workspace-missing'); } catch {} require('node:fs').unlinkSync('package.json'); process.exitCode = 7;\n").unwrap();
    let observed = run(&["node".into(), entry.into_os_string()], &[], root.path()).unwrap();
    assert_eq!(observed.status.code(), Some(7));
    assert!(!root.path().join("package.json").exists());
    assert!(observed.reports.is_empty());
    assert_eq!(observed.diagnostics.len(), 1);
    assert_eq!(observed.diagnostics[0].package, "changing-workspace@1.0.0");
}

fn node_supports_hooks() -> bool {
    Command::new("node")
        .args([
            "-e",
            "process.exit(typeof require('node:module').registerHooks === 'function' ? 0 : 1)",
        ])
        .status()
        .is_ok_and(|status| status.success())
}

#[test]
fn skips_unreadable_events_and_keeps_the_rest() {
    let temp = tempfile::tempdir().unwrap();
    let package = temp.path().join("node_modules/example");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(package.join("package.json"), r#"{"name":"example","version":"1.0.0"}"#)
        .unwrap();
    let file = package.join("index.js");
    std::fs::write(&file, "").unwrap();
    let mut raw = br#"{"specifier":"\ud800","parent":"/x"}"#.to_vec();
    raw.extend_from_slice(b"\n{\"specifier\":\"cut sh\n\xff\xfe\n\n");
    raw.extend_from_slice(
        format!("{}\n", serde_json::json!({"specifier":"missing", "parent":file})).as_bytes(),
    );
    let owners = trace_owners(&[package], &[]).unwrap();
    let observed = reports_from_events(&raw, &owners);
    assert_eq!(observed.skipped, 3);
    assert_eq!(observed.reports.len(), 1);
    assert_eq!(observed.reports[0].findings[0].dependency, "missing");
}

#[test]
fn hook_lives_at_a_stable_path_that_outlives_the_trace() {
    let cache = tempfile::tempdir().unwrap();
    let first = super::install_hook_in(cache.path()).unwrap();
    let second = super::install_hook_in(cache.path()).unwrap();
    assert_eq!(first, second);
    assert_eq!(std::fs::read_to_string(&first).unwrap(), include_str!("../trace-hook.mjs"));
    std::fs::write(&first, "tampered").unwrap();
    assert_eq!(super::install_hook_in(cache.path()).unwrap(), first);
    assert_eq!(std::fs::read_to_string(&first).unwrap(), include_str!("../trace-hook.mjs"));
}

#[test]
fn lingering_processes_ignore_a_removed_event_file() {
    if !node_supports_hooks() {
        return;
    }
    let cache = tempfile::tempdir().unwrap();
    let hook = super::install_hook_in(cache.path()).unwrap();
    let project = tempfile::tempdir().unwrap();
    let events = project.path().join("gone/events.jsonl");
    let gone = project.path().join("events.jsonl");
    for file in [&events, &gone] {
        let output = Command::new("node")
            .args(["-e", "try { require('missing-' + 'dep') } catch {}"])
            .env("NODE_OPTIONS", format!("--import={}", super::file_url(&hook).unwrap()))
            .env("XRAY_TRACE_FILE", file)
            .current_dir(project.path())
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(output.stderr.is_empty(), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(!file.exists());
    }
}

#[test]
fn preserves_existing_node_options() {
    assert_eq!(super::node_options(None, "file:///hook.mjs"), "--import=file:///hook.mjs");
    assert_eq!(
        super::node_options(Some("--max-old-space-size=4096".into()), "file:///hook.mjs"),
        "--max-old-space-size=4096 --import=file:///hook.mjs",
    );
}

#[test]
fn interrupted_foreground_command_may_finish_or_is_killed_after_the_grace_period() {
    use std::{sync::atomic::AtomicBool, time::Duration};
    let cancelled = AtomicBool::new(true);
    let mut quick = Command::new("node");
    quick.args(["-e", ""]);
    let status = super::wait_foreground(&mut quick, &cancelled, Duration::from_secs(30)).unwrap();
    assert!(status.success());
    assert_eq!(super::exit_code(status, true), 130);
    assert_eq!(super::exit_code(status, false), 0);
    let mut stuck = Command::new("node");
    stuck.args(["-e", "setTimeout(() => {}, 60000)"]);
    let started = std::time::Instant::now();
    let status = super::wait_foreground(&mut stuck, &cancelled, Duration::ZERO).unwrap();
    assert!(!status.success());
    assert!(started.elapsed() < Duration::from_secs(30));
}
