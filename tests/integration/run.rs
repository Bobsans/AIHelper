use assert_cmd::Command;
use predicates::str::contains;

#[test]
fn run_check_reports_successful_command() {
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["--json", "run", "check", "rustc", "--version"])
        .assert()
        .success()
        .stdout(contains("\"command\": \"run.check\""))
        .stdout(contains("\"success\": true"))
        .stdout(contains("\"timed_out\": false"))
        .stdout(contains("rustc"));
}

#[test]
fn run_check_reports_failing_command_without_failing_ah() {
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args([
        "--json",
        "run",
        "check",
        "rustc",
        "--definitely-not-a-rustc-flag",
    ])
    .assert()
    .success()
    .stdout(contains("\"command\": \"run.check\""))
    .stdout(contains("\"success\": false"))
    .stdout(contains("\"timed_out\": false"));
}
