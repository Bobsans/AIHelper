use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use tempfile::TempDir;

#[test]
fn upgrade_check_routes_before_configuration_and_plugins() {
    let cwd = TempDir::new().unwrap();
    let mut command = Command::cargo_bin("ah").unwrap();
    let assertion = command
        .current_dir(cwd.path())
        .args(["upgrade", "--check", "--json"])
        .env("AH_CONFIG_DIR", "")
        .assert()
        .failure();

    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    let assertion = assertion.stderr(contains("UPDATER_TRUST"));
    #[cfg(not(all(target_os = "windows", target_arch = "x86_64")))]
    let assertion = assertion.stderr(contains("UPDATER_UNSUPPORTED_PLATFORM"));

    assertion
        .stderr(contains("CONFIG_INVALID").not())
        .stdout("");
    assert_eq!(std::fs::read_dir(cwd.path()).unwrap().count(), 0);
}

#[test]
fn upgrade_rollback_routes_before_configuration_and_plugins() {
    let mut command = Command::cargo_bin("ah").unwrap();
    let assertion = command
        .args(["upgrade", "--rollback"])
        .env("AH_CONFIG_DIR", "")
        .assert()
        .failure();

    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    let assertion = assertion.stderr(contains("UPDATER_TRUST"));
    #[cfg(not(all(target_os = "windows", target_arch = "x86_64")))]
    let assertion = assertion.stderr(contains("UPDATER_UNSUPPORTED_PLATFORM"));

    assertion
        .stderr(contains("CONFIG_INVALID").not())
        .stdout("");
}
