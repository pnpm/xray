use super::command;

#[test]
fn extensionless_command_can_execute_a_windows_cmd_shim() {
    let temp = tempfile::tempdir().unwrap();
    let shim = temp.path().join("package-manager.cmd");
    std::fs::write(&shim, "@exit /b 7\r\n").unwrap();
    let requested = temp.path().join("package-manager");
    let status = command(requested.as_os_str()).status().unwrap();
    assert_eq!(status.code(), Some(7));
}
