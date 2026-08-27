//! `ah task`, run in this process.
//!
//! The child processes the tasks start are real - `task run` spawns them, and
//! that is the behaviour under test. What is no longer a process is `ah`
//! itself, so the recipe store, the output bounding and the timeout are
//! asserted against values rather than against a subprocess's streams.
//!
//! The `contains("\u{1b}").not()` checks these had are gone: the captured
//! emitter has colour off by construction. That property is asserted once, in
//! `git.rs`, because it belongs to the emitter rather than to any domain.

use std::time::{Duration, Instant};

use aihelper::harness::Harness;
use serde_json::Value;
use tempfile::TempDir;

fn task_echo_command() -> &'static str {
    if cfg!(target_os = "windows") {
        "Write-Output task-ok"
    } else {
        "echo task-ok"
    }
}

/// A harness rooted in its own directory, which is where the task store lives.
fn harness(dir: &TempDir) -> Harness {
    Harness::new().in_directory(dir.path())
}

fn save(dir: &TempDir, name: &str, command: &str) -> String {
    harness(dir)
        .run(&["task", "save", name, command])
        .expect_success()
        .to_owned()
}

#[test]
fn task_save_and_list_json() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");

    let saved = save(&temp_dir, "hello", task_echo_command());
    assert!(saved.contains("saved task 'hello'"), "{saved}");

    let payload = harness(&temp_dir).run(&["--json", "task", "list"]).json();
    assert_eq!(payload["command"], "task.list");
    let names = payload["tasks"]
        .as_array()
        .expect("tasks array")
        .iter()
        .filter_map(|task| task["name"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(names, ["hello"]);

    let text = harness(&temp_dir)
        .run(&["task", "list"])
        .expect_success()
        .to_owned();
    assert!(text.contains("hello =>"), "{text}");
}

#[test]
fn task_run_executes_saved_command() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    save(&temp_dir, "echo", task_echo_command());

    let output = harness(&temp_dir)
        .run(&["task", "run", "echo"])
        .expect_success()
        .to_owned();

    assert!(output.contains("task-ok"), "{output}");
}

#[test]
fn task_run_unknown_task_fails() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");

    let run = harness(&temp_dir).run(&["task", "run", "missing"]);

    let _ = run.expect_failure();
    assert_eq!(
        run.diagnostic_text(&["task", "run", "missing"]),
        "ah: task not found: missing\n\n\
         Hint: Run 'ah task list' to see saved tasks."
    );
}

#[test]
fn task_run_bounds_output_while_reading() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    save(&temp_dir, "noisy", task_noisy_command());

    let payload: Value = harness(&temp_dir)
        .run(&["--json", "task", "run", "noisy", "--max-output-bytes", "32"])
        .json();

    assert_eq!(payload["truncated"], true);
    assert!(payload["stdout"].as_str().expect("stdout").len() <= 32);
}

#[test]
fn task_run_timeout_returns_stable_error() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    save(&temp_dir, "slow", task_slow_command());

    let started = Instant::now();
    let run = harness(&temp_dir).run(&["task", "run", "slow", "--timeout-secs", "1"]);

    let _ = run.expect_failure();
    assert!(
        run.detail()
            .contains("task 'slow' did not complete within 1 seconds"),
        "{}",
        run.detail()
    );
    assert!(
        !run.diagnostic_text(&["task", "run", "slow"])
            .contains("TASK_TIMEOUT"),
        "no internal code leaks into the text"
    );
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
