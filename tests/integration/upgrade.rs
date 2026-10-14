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
    let assertion = assertion
        .stderr(contains(
            "ah: self-update rollback requires a managed installation",
        ))
        .stderr(contains("UPDATER_INSTALLATION").not());
    #[cfg(not(all(target_os = "windows", target_arch = "x86_64")))]
    let assertion = assertion
        .stderr(contains("ah:"))
        .stderr(contains("UPDATER_UNSUPPORTED_PLATFORM").not());

    assertion
        .stderr(contains("CONFIG_INVALID").not())
        .stdout(contains("AIHelper rollback"))
        .stdout(contains("✗ Verifying backup"))
        .stdout(contains("\u{1b}").not());
}

#[test]
fn upgrade_failure_json_and_quiet_have_no_progress_output() {
    for flags in [vec!["--json"], vec!["--quiet"], vec!["--json", "--quiet"]] {
        let output = Command::cargo_bin("ah")
            .unwrap()
            .args(["upgrade", "--rollback"])
            .args(flags)
            .env("AH_CONFIG_DIR", "")
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("Verifying backup"));
    }
}
