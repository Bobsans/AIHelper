//! `ah run check`, mostly run in this process.
//!
//! The child processes are real - spawning them, bounding their output and
//! killing their descendants is the behaviour under test. What stopped being a
//! process is `ah` itself, so the `run.check` payload is read as a value.
//!
//! Two tests here still need a process of their own, and say why: one sets an
//! environment variable to prove the child does not inherit it, and one
//! replaces `PATH`. Both are process state, and setting either in the test
//! process would leak into every other test in this binary.

use super::common::IsolatedAhCommand as Command;
use aihelper::harness::Harness;
use predicates::str::contains;
use serde_json::Value;
use std::fs;
#[cfg(windows)]
use std::process::Command as ProcessCommand;
use std::time::{Duration, Instant};
use tempfile::TempDir;

/// `run check` answering in JSON, in this process.
fn run_check_json(args: &[&str]) -> Value {
    let mut argv = vec!["--json", "run", "check"];
    argv.extend_from_slice(args);
    Harness::new().run(&argv).json()
}

#[test]
fn run_check_reports_successful_command() {
    let payload = run_check_json(&platform_exit_command(true));

    assert_eq!(payload["command"], "run.check");
    assert_eq!(payload["success"], true);
    assert_eq!(payload["timed_out"], false);
}

/// Process-level: the variable has to be in `ah`'s own environment, and setting
/// it in the test process would put it in every other test's too.
#[test]
fn run_check_child_cannot_inherit_vault_master_key() {
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    let mut args = vec!["run", "check"];
    args.extend(platform_missing_vault_key_command());
    cmd.env(
        "AH_VAULT_MASTER_KEY",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    )
    .args(args)
    .assert()
    .success();
}

#[test]
fn run_check_text_output_preserves_plain_contract() {
    let mut argv = vec!["run", "check"];
    argv.extend(platform_exit_command(true));
    let output = Harness::new().run(&argv).expect_success().to_owned();

    assert!(
        output.contains("success=true exit_code=0 timed_out=false duration_ms="),
        "{output}"
    );
}

#[test]
fn run_check_reports_failing_command_without_failing_ah() {
    let payload = run_check_json(&platform_exit_command(false));

    assert_eq!(payload["command"], "run.check");
    assert_eq!(payload["success"], false);
    assert_eq!(payload["timed_out"], false);
}

#[cfg(windows)]
#[test]
fn run_check_preserves_exit_code_259() {
    let payload = run_check_json(&["cmd.exe", "/C", "exit 259"]);

    assert_eq!(payload["exit_code"], 259);
    assert_eq!(payload["timed_out"], false);
}

#[test]
fn run_check_timeout_terminates_process_tree() {
    let mut args = vec!["--timeout-secs", "1"];
    args.extend(platform_process_tree_command());

    let started = Instant::now();
    let payload = run_check_json(&args);

    assert!(
        started.elapsed() < Duration::from_secs(4),
        "process tree outlived timeout: {:?}",
        started.elapsed()
    );
    assert_eq!(payload["timed_out"], true);
    assert_eq!(payload["success"], false);
}

#[cfg(windows)]
#[test]
fn run_check_timeout_leaves_no_surviving_descendant() {
    let temp = TempDir::new().expect("temporary directory should be created");
    let marker = temp.path().join("descendant-survived.txt");
    let script = temp.path().join("delayed-marker.cmd");
    fs::write(
        &script,
        format!(
            "@ping.exe -n 3 127.0.0.1 >nul\r\n@echo survived>\"{}\"\r\n",
            marker.display()
        ),
    )
    .expect("descendant script should be written");
    let launch = format!(
        "Start-Process -FilePath '{}' -NoNewWindow -Wait",
        script.display()
    );

    let payload = run_check_json(&[
        "--timeout-secs",
        "1",
        "powershell.exe",
        "-NoProfile",
        "-Command",
        &launch,
    ]);

    assert_eq!(payload["timed_out"], true);

    std::thread::sleep(Duration::from_secs(3));
    assert!(
        !marker.exists(),
        "descendant process survived job termination"
    );
}

#[cfg(windows)]
#[test]
fn run_check_preserves_batch_script_execution() {
    let temp = TempDir::new().expect("temporary directory should be created");
    let script = temp.path().join("echo-result.cmd");
    fs::write(&script, "@echo batch-ok\r\n").expect("batch script should be written");

    let output = Harness::new()
        .run(&["run", "check", script.to_string_lossy().as_ref()])
        .expect_success()
        .to_owned();

    assert!(output.contains("batch-ok"), "{output}");
}

/// Process-level: it replaces `PATH`, which is process state.
#[cfg(windows)]
#[test]
fn run_check_batch_uses_system_command_prompt() {
    let temp = TempDir::new().expect("temporary directory should be created");
    let fake_command_prompt = temp.path().join("cmd.exe");
    fs::write(&fake_command_prompt, "not an executable").expect("fake cmd should be written");
    let script = temp.path().join("system-cmd.cmd");
    fs::write(&script, "@echo system-cmd-ok\r\n").expect("batch script should be written");
    let path = std::env::join_paths(std::iter::once(temp.path().to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .expect("PATH should be joinable");

    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.env("PATH", path)
        .args(["run", "check", script.to_string_lossy().as_ref()])
        .assert()
        .success()
        .stdout(contains("system-cmd-ok"));
}

#[cfg(windows)]
#[test]
fn run_check_batch_accepts_verbatim_script_path() {
    let temp = TempDir::new().expect("temporary directory should be created");
    let script = temp.path().join("verbatim.cmd");
    fs::write(&script, "@echo verbatim-ok\r\n").expect("batch script should be written");
    let verbatim = format!(r"\\?\{}", script.display());

    let output = Harness::new()
        .run(&["run", "check", &verbatim])
        .expect_success()
        .to_owned();

    assert!(output.contains("verbatim-ok"), "{output}");
}

#[cfg(windows)]
#[test]
fn run_check_batch_preserves_escaped_arguments() {
    let temp = TempDir::new().expect("temporary directory should be created");
    let script = temp.path().join("echo-args.cmd");
    fs::write(
        &script,
        "@echo [%~1]\r\n@echo [%~2]\r\n@echo [%~3]\r\n@echo [%~4]\r\n@echo [%~5]\r\n",
    )
    .expect("batch script should be written");

    let arguments = ["", "two words", "trailing\\", "100%", "quote\"inside"];
    let baseline = ProcessCommand::new(&script)
        .args(arguments)
        .output()
        .expect("batch script should run through std::process");
    assert!(baseline.status.success(), "{baseline:?}");

    let mut args = vec![script.to_string_lossy().into_owned()];
    args.extend(arguments.iter().map(|value| (*value).to_owned()));
    let args = args.iter().map(String::as_str).collect::<Vec<_>>();
    let payload = run_check_json(&args);
    let stdout = payload["stdout"].as_str().expect("stdout should be text");
    assert_eq!(stdout.as_bytes(), baseline.stdout);
}

#[cfg(windows)]
#[test]
fn run_check_timeout_tracks_descendants_after_batch_root_exits() {
    let temp = TempDir::new().expect("temporary directory should be created");
    let marker = temp.path().join("batch-descendant-survived.txt");
    let script = temp.path().join("spawn-background.cmd");
    fs::write(
        &script,
        format!(
            "@start \"\" /b powershell.exe -NoProfile -Command \"Start-Sleep -Seconds 2; Set-Content -LiteralPath '{}' -Value survived\"\r\n@exit /b 0\r\n",
            marker.display()
        ),
    )
    .expect("batch script should be written");

    let payload = run_check_json(&["--timeout-secs", "1", script.to_string_lossy().as_ref()]);

    assert_eq!(payload["timed_out"], true);

    std::thread::sleep(Duration::from_secs(3));
    assert!(
        !marker.exists(),
        "batch descendant survived job termination"
    );
}

/// Process-level: `--cwd workspace` is relative, so it has to resolve against a
/// process's own working directory rather than the test binary's.
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
    let mut args = Vec::new();
    if explicit_delimiter {
        args.push("--");
    }
    args.extend(platform_echo_arguments_command());
    let payload = run_check_json(&args);
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
fn platform_missing_vault_key_command() -> Vec<&'static str> {
    vec![
        "cmd.exe",
        "/C",
        "if defined AH_VAULT_MASTER_KEY (exit 9) else (exit 0)",
    ]
}

#[cfg(not(windows))]
fn platform_missing_vault_key_command() -> Vec<&'static str> {
    vec!["sh", "-c", "test -z \"$AH_VAULT_MASTER_KEY\""]
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
