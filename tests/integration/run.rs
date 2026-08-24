use super::common::IsolatedAhCommand as Command;
use predicates::{prelude::PredicateBooleanExt, str::contains};
use std::fs;
#[cfg(windows)]
use std::process::Command as ProcessCommand;
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
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    let mut args = vec!["run", "check"];
    args.extend(platform_exit_command(true));
    cmd.args(args)
        .assert()
        .success()
        .stdout(contains(
            "success=true exit_code=0 timed_out=false duration_ms=",
        ))
        .stdout(contains("\u{1b}").not());
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

#[cfg(windows)]
#[test]
fn run_check_preserves_exit_code_259() {
    let output = Command::cargo_bin("ah")
        .expect("binary should compile")
        .args(["--json", "run", "check", "cmd.exe", "/C", "exit 259"])
        .output()
        .expect("ah should run");
    assert!(output.status.success(), "{output:?}");
    let payload: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("valid JSON output");
    assert_eq!(payload["exit_code"], 259);
    assert_eq!(payload["timed_out"], false);
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

    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    let output = cmd
        .args([
            "--json",
            "run",
            "check",
            "--timeout-secs",
            "1",
            "powershell.exe",
            "-NoProfile",
            "-Command",
            &launch,
        ])
        .output()
        .expect("ah should run");
    assert!(output.status.success(), "{output:?}");
    let payload: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("valid JSON output");
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

    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["run", "check", script.to_string_lossy().as_ref()])
        .assert()
        .success()
        .stdout(contains("batch-ok"));
}

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

    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["run", "check", &verbatim])
        .assert()
        .success()
        .stdout(contains("verbatim-ok"));
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

    let output = Command::cargo_bin("ah")
        .expect("binary should compile")
        .args(["--json", "run", "check", script.to_string_lossy().as_ref()])
        .args(arguments)
        .output()
        .expect("ah should run");
    assert!(output.status.success(), "{output:?}");
    let payload: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("valid JSON output");
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

    let output = Command::cargo_bin("ah")
        .expect("binary should compile")
        .args([
            "--json",
            "run",
            "check",
            "--timeout-secs",
            "1",
            script.to_string_lossy().as_ref(),
        ])
        .output()
        .expect("ah should run");
    assert!(output.status.success(), "{output:?}");
    let payload: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("valid JSON output");
    assert_eq!(payload["timed_out"], true);

    std::thread::sleep(Duration::from_secs(3));
    assert!(
        !marker.exists(),
        "batch descendant survived job termination"
    );
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
