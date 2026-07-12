use assert_cmd::Command;
use predicates::str::contains;
use std::time::{Duration, Instant};
use tempfile::TempDir;

fn task_echo_command() -> &'static str {
    if cfg!(target_os = "windows") {
        "Write-Output task-ok"
    } else {
        "echo task-ok"
    }
}

#[test]
fn task_save_and_list_json() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    let cwd = temp_dir.path().to_string_lossy().to_string();

    let mut save_cmd = Command::cargo_bin("ah").expect("binary should compile");
    save_cmd
        .args(["--cwd", &cwd, "task", "save", "hello", task_echo_command()])
        .assert()
        .success()
        .stdout(contains("saved task 'hello'"));

    let mut list_cmd = Command::cargo_bin("ah").expect("binary should compile");
    list_cmd
        .args(["--json", "--cwd", &cwd, "task", "list"])
        .assert()
        .success()
        .stdout(contains("\"command\": \"task.list\""))
        .stdout(contains("\"name\": \"hello\""));
}

#[test]
fn task_run_executes_saved_command() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    let cwd = temp_dir.path().to_string_lossy().to_string();

    let mut save_cmd = Command::cargo_bin("ah").expect("binary should compile");
    save_cmd
        .args(["--cwd", &cwd, "task", "save", "echo", task_echo_command()])
        .assert()
        .success();

    let mut run_cmd = Command::cargo_bin("ah").expect("binary should compile");
    run_cmd
        .args(["--cwd", &cwd, "task", "run", "echo"])
        .assert()
        .success()
        .stdout(contains("task-ok"));
}

#[test]
fn task_run_unknown_task_fails() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    let cwd = temp_dir.path().to_string_lossy().to_string();

    let mut run_cmd = Command::cargo_bin("ah").expect("binary should compile");
    run_cmd
        .args(["--cwd", &cwd, "task", "run", "missing"])
        .assert()
        .failure()
        .stderr(contains("TASK_NOT_FOUND: task not found: missing"))
        .stderr(contains("hint: run ah task list"));
}

#[test]
fn task_run_bounds_output_while_reading() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    let cwd = temp_dir.path().to_string_lossy().to_string();

    let mut save_cmd = Command::cargo_bin("ah").expect("binary should compile");
    save_cmd
        .args(["--cwd", &cwd, "task", "save", "noisy", task_noisy_command()])
        .assert()
        .success();

    let mut run_cmd = Command::cargo_bin("ah").expect("binary should compile");
    let output = run_cmd
        .args([
            "--json",
            "--cwd",
            &cwd,
            "task",
            "run",
            "noisy",
            "--max-output-bytes",
            "32",
        ])
        .output()
        .expect("task should run");
    assert!(output.status.success(), "{output:?}");
    let payload: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("valid JSON output");
    assert_eq!(payload["truncated"], true);
    assert!(payload["stdout"].as_str().expect("stdout").len() <= 32);
}

#[test]
fn task_run_timeout_returns_stable_error() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    let cwd = temp_dir.path().to_string_lossy().to_string();

    let mut save_cmd = Command::cargo_bin("ah").expect("binary should compile");
    save_cmd
        .args(["--cwd", &cwd, "task", "save", "slow", task_slow_command()])
        .assert()
        .success();

    let started = Instant::now();
    let mut run_cmd = Command::cargo_bin("ah").expect("binary should compile");
    run_cmd
        .args(["--cwd", &cwd, "task", "run", "slow", "--timeout-secs", "1"])
        .assert()
        .failure()
        .stderr(contains("TASK_TIMEOUT"));
    assert!(started.elapsed() < Duration::from_secs(4));
}

#[cfg(windows)]
fn task_noisy_command() -> &'static str {
    "1..200 | ForEach-Object { Write-Output abcdefghij }"
}

#[cfg(not(windows))]
fn task_noisy_command() -> &'static str {
    "i=0; while [ $i -lt 200 ]; do echo abcdefghij; i=$((i+1)); done"
}

#[cfg(windows)]
fn task_slow_command() -> &'static str {
    "Start-Sleep -Seconds 5"
}

#[cfg(not(windows))]
fn task_slow_command() -> &'static str {
    "sleep 5"
}
