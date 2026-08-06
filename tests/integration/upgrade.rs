use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;

#[test]
fn upgrade_rollback_routes_before_configuration_and_plugins() {
    let mut command = Command::cargo_bin("ah").unwrap();
    let assertion = command
        .args(["upgrade", "--rollback"])
        .env("AH_CONFIG_DIR", "")
        .assert()
        .failure();

    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    let assertion = assertion.stderr(contains("UPDATER_INSTALLATION"));
    #[cfg(not(all(target_os = "windows", target_arch = "x86_64")))]
    let assertion = assertion.stderr(contains("UPDATER_UNSUPPORTED_PLATFORM"));

    assertion
        .stderr(contains("CONFIG_INVALID").not())
        .stdout("");
}
