use std::{fs, process::Command};

use ah_updater_core::{
    UPDATE_HELPER_PROTOCOL_VERSION, UPDATE_HELPER_SELF_CHECK_SCHEMA_VERSION,
    UpdateHelperSelfCheckV1,
};
use tempfile::TempDir;

#[test]
fn self_check_is_strict_deterministic_and_side_effect_free() {
    let temp = TempDir::new().unwrap();
    let sentinel = temp.path().join("user-owned.txt");
    fs::write(&sentinel, b"unchanged").unwrap();

    let first = Command::new(env!("CARGO_BIN_EXE_ah-update-helper"))
        .arg("--self-check")
        .current_dir(temp.path())
        .output()
        .unwrap();
    let second = Command::new(env!("CARGO_BIN_EXE_ah-update-helper"))
        .arg("--self-check")
        .current_dir(temp.path())
        .output()
        .unwrap();

    assert!(first.status.success());
    assert!(first.stderr.is_empty());
    assert_eq!(first.stdout, second.stdout);
    assert!(first.stdout.ends_with(b"\n"));
    let response: UpdateHelperSelfCheckV1 = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(
        response.schema_version,
        UPDATE_HELPER_SELF_CHECK_SCHEMA_VERSION
    );
    assert_eq!(response.protocol_version, UPDATE_HELPER_PROTOCOL_VERSION);
    assert_eq!(response.helper_version, env!("CARGO_PKG_VERSION"));
    assert!(!response.target.is_empty());
    assert!(!response.architecture.is_empty());
    assert_eq!(fs::read(sentinel).unwrap(), b"unchanged");
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
}

#[test]
fn unknown_commands_fail_without_structured_output() {
    let output = Command::new(env!("CARGO_BIN_EXE_ah-update-helper"))
        .arg("--unknown")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8(output.stderr).unwrap().contains("usage:"));
}
