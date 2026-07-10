use assert_cmd::Command;
use predicates::str::contains;
use std::time::{Duration, Instant};

#[test]
fn run_check_reports_successful_command() {
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    let mut args = vec!["--json", "run", "check"];
    args.extend(platform_exit_command(true));
    cmd.args(args)
        .assert()
        .success()
        .stdout(contains("\"command\": \"run.check\""))
        .stdout(contains("\"success\": true"))
        .stdout(contains("\"timed_out\": false"));
}

#[test]
fn run_check_reports_failing_command_without_failing_ah() {
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    let mut args = vec!["--json", "run", "check"];
    args.extend(platform_exit_command(false));
    cmd.args(args)
        .assert()
        .success()
        .stdout(contains("\"command\": \"run.check\""))
        .stdout(contains("\"success\": false"))
        .stdout(contains("\"timed_out\": false"));
}

#[test]
fn run_check_timeout_terminates_process_tree() {
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    let mut args = vec!["--json", "run", "check", "--timeout-secs", "1"];
    args.extend(platform_process_tree_command());
    cmd.args(args);

    let started = Instant::now();
    let output = cmd.output().expect("ah should run");
    assert!(output.status.success(), "{output:?}");
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "process tree outlived timeout: {:?}",
        started.elapsed()
    );
    let payload: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("valid JSON output");
    assert_eq!(payload["timed_out"], true);
    assert_eq!(payload["success"], false);
}

#[cfg(windows)]
fn platform_exit_command(success: bool) -> Vec<&'static str> {
    vec!["cmd.exe", "/C", if success { "exit 0" } else { "exit 7" }]
}

#[cfg(not(windows))]
fn platform_exit_command(success: bool) -> Vec<&'static str> {
    vec!["sh", "-c", if success { "exit 0" } else { "exit 7" }]
}

#[cfg(windows)]
fn platform_process_tree_command() -> Vec<&'static str> {
    vec![
        "powershell.exe",
        "-NoProfile",
        "-Command",
        "Start-Process -FilePath ping.exe -ArgumentList '-n','6','127.0.0.1' -NoNewWindow -Wait",
    ]
}

#[cfg(not(windows))]
fn platform_process_tree_command() -> Vec<&'static str> {
    vec!["sh", "-c", "sleep 5 & wait"]
}
