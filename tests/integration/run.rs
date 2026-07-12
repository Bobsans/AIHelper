use assert_cmd::Command;
use predicates::str::contains;
use std::fs;
use std::time::{Duration, Instant};
use tempfile::TempDir;

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

#[test]
fn relative_cwd_is_applied_once() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    let workspace = temp_dir.path().join("workspace");
    fs::create_dir(&workspace).expect("workspace should be created");
    fs::write(workspace.join("sample.txt"), "cwd-ok\n").expect("sample should be written");

    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.current_dir(temp_dir.path())
        .args([
            "--cwd",
            "workspace",
            "file",
            "head",
            "sample.txt",
            "--lines",
            "1",
        ])
        .assert()
        .success()
        .stdout(contains("cwd-ok"));
}

#[test]
fn run_check_preserves_global_like_child_arguments() {
    assert_child_arguments_preserved(false);
    assert_child_arguments_preserved(true);
}

fn assert_child_arguments_preserved(explicit_delimiter: bool) {
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    let mut args = vec!["--json", "run", "check"];
    if explicit_delimiter {
        args.push("--");
    }
    args.extend(platform_echo_arguments_command());
    let output = cmd.args(args).output().expect("ah should run");
    assert!(output.status.success(), "{output:?}");
    let payload: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("valid JSON output");
    let argv = payload["argv"].as_array().expect("argv should be an array");
    for expected in ["--json", "--quiet", "--limit", "child", "--cwd", "nested"] {
        assert!(
            argv.iter().any(|value| value == expected),
            "missing {expected}: {argv:?}"
        );
    }
    let stdout = payload["stdout"].as_str().expect("stdout should be text");
    assert!(stdout.contains("--json"), "{stdout:?}");
    assert!(stdout.contains("--cwd"), "{stdout:?}");
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

#[cfg(windows)]
fn platform_echo_arguments_command() -> Vec<&'static str> {
    vec![
        "cmd.exe", "/C", "echo", "--json", "--quiet", "--limit", "child", "--cwd", "nested",
    ]
}

#[cfg(not(windows))]
fn platform_echo_arguments_command() -> Vec<&'static str> {
    vec![
        "printf", "%s\\n", "--json", "--quiet", "--limit", "child", "--cwd", "nested",
    ]
}

#[cfg(not(windows))]
fn platform_process_tree_command() -> Vec<&'static str> {
    vec!["sh", "-c", "sleep 5 & wait"]
}
