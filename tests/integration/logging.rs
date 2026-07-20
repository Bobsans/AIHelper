use std::{
    fs::{self, OpenOptions},
    process::Command as ProcessCommand,
};

use assert_cmd::Command;
use fs2::FileExt;
use serde_json::Value;
use tempfile::TempDir;

#[test]
fn logs_success_error_help_and_parse_outcomes() {
    let config_dir = TempDir::new().expect("temporary config dir should be created");

    Command::cargo_bin("ah")
        .expect("binary should compile")
        .env("AH_CONFIG_DIR", config_dir.path())
        .args(["plugins", "list", "--json"])
        .assert()
        .success();

    Command::cargo_bin("ah")
        .expect("binary should compile")
        .env("AH_CONFIG_DIR", config_dir.path())
        .arg("--version")
        .assert()
        .success();

    Command::cargo_bin("ah")
        .expect("binary should compile")
        .env("AH_CONFIG_DIR", config_dir.path())
        .args(["file", "stat", "missing-file"])
        .assert()
        .failure();

    Command::cargo_bin("ah")
        .expect("binary should compile")
        .env("AH_CONFIG_DIR", config_dir.path())
        .arg("--help")
        .assert()
        .success();

    Command::cargo_bin("ah")
        .expect("binary should compile")
        .env("AH_CONFIG_DIR", config_dir.path())
        .args([
            "--limit",
            "not-a-number",
            "http",
            "get",
            "not-a-url",
            "--bearer",
            "pre-routing-secret",
        ])
        .assert()
        .failure();

    let records = log_records(&config_dir);
    let commands = command_records(&records);
    assert_eq!(commands.len(), 4, "{records:#?}");
    assert_command(commands[0], "plugins.list", "success");
    assert_command(commands[1], "version", "success");
    assert_command(commands[2], "file.stat", "error");
    assert_eq!(commands[2]["diagnostic"]["code"], "FILE_NOT_FOUND");
    assert_command(commands[3], "help", "success");
    assert_eq!(
        commands[0]["parameters"]["argv"],
        serde_json::json!(["plugins", "list", "--json"])
    );

    let parse_event = records
        .iter()
        .find(|record| record["event"] == "system" && record["component"] == "cli_parse")
        .expect("parse failure should produce a system event");
    assert_eq!(parse_event["severity"], "error");
    let serialized_parse_event = serde_json::to_string(parse_event).unwrap();
    assert!(serialized_parse_event.contains("[REDACTED]"));
    assert!(!serialized_parse_event.contains("pre-routing-secret"));
    assert!(
        !commands
            .iter()
            .any(|record| record["command"] == "file.head")
    );
}

#[test]
fn logger_lock_and_directory_failures_do_not_change_command_output() {
    let baseline_config = TempDir::new().expect("baseline config should be created");
    let baseline = command_output(&baseline_config);
    assert!(baseline.status.success());

    let locked_config = TempDir::new().expect("locked config should be created");
    let warmup = command_output(&locked_config);
    assert!(warmup.status.success());
    let log_path = jsonl_paths(&locked_config)
        .into_iter()
        .next()
        .expect("warmup should create a log file");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .open(log_path)
        .expect("log file should open");
    lock.lock_exclusive()
        .expect("test should hold the log lock");
    let locked = command_output(&locked_config);
    FileExt::unlock(&lock).expect("test lock should release");
    assert_same_command_output(&baseline, &locked);

    let blocked_config = TempDir::new().expect("blocked config should be created");
    fs::write(blocked_config.path().join("logs"), "not-a-directory")
        .expect("blocking file should be created");
    let blocked = command_output(&blocked_config);
    assert_same_command_output(&baseline, &blocked);
}

#[test]
fn relative_config_dir_follows_applied_cwd_for_logs() {
    let root = TempDir::new().expect("temporary root should be created");
    let workspace = root.path().join("workspace");
    fs::create_dir(&workspace).expect("workspace should be created");

    Command::cargo_bin("ah")
        .expect("binary should compile")
        .current_dir(root.path())
        .env("AH_CONFIG_DIR", "relative-config")
        .args(["--cwd", "workspace", "plugins", "list", "--quiet"])
        .assert()
        .success();

    assert!(workspace.join("relative-config").join("logs").is_dir());
    assert!(!root.path().join("relative-config").exists());
}

#[test]
fn logs_external_check_failure_as_success_and_redacts_cli_secrets() {
    let config_dir = TempDir::new().expect("temporary config dir should be created");

    let mut check_args = vec!["run", "check"];
    check_args.extend(platform_exit_command(false));
    Command::cargo_bin("ah")
        .expect("binary should compile")
        .env("AH_CONFIG_DIR", config_dir.path())
        .args(check_args)
        .assert()
        .success();

    Command::cargo_bin("ah")
        .expect("binary should compile")
        .env("AH_CONFIG_DIR", config_dir.path())
        .args([
            "http",
            "get",
            "not-a-valid-url",
            "--bearer",
            "top-secret-token",
        ])
        .assert()
        .failure();

    let records = log_records(&config_dir);
    let commands = command_records(&records);
    let check = commands
        .iter()
        .find(|record| record["command"] == "run.check")
        .expect("run.check should be logged");
    assert_eq!(check["status"], "success");
    assert!(check.get("diagnostic").is_none());

    let http = commands
        .iter()
        .find(|record| record["command"] == "http.get")
        .expect("http.get should be logged");
    assert_eq!(http["status"], "error");
    assert!(serde_json::to_string(http).unwrap().contains("[REDACTED]"));
    assert!(
        !serde_json::to_string(http)
            .unwrap()
            .contains("top-secret-token")
    );
}

#[test]
fn logs_configuration_failure_as_system_event() {
    let config_dir = TempDir::new().expect("temporary config dir should be created");
    fs::write(config_dir.path().join("plugins.json"), "not-json")
        .expect("invalid settings should be written");

    Command::cargo_bin("ah")
        .expect("binary should compile")
        .env("AH_CONFIG_DIR", config_dir.path())
        .args(["plugins", "list"])
        .assert()
        .failure();

    let records = log_records(&config_dir);
    assert!(command_records(&records).is_empty());
    let event = records
        .iter()
        .find(|record| record["event"] == "system" && record["component"] == "config")
        .expect("configuration failure should be logged");
    assert_eq!(event["severity"], "error");
    assert_eq!(event["diagnostic"]["code"], "JSON_DESERIALIZATION_FAILED");
}

#[test]
fn concurrent_processes_append_parseable_records() {
    let config_dir = TempDir::new().expect("temporary config dir should be created");
    let binary = assert_cmd::cargo::cargo_bin("ah");
    let mut children = Vec::new();
    for _ in 0..6 {
        children.push(
            ProcessCommand::new(&binary)
                .env("AH_CONFIG_DIR", config_dir.path())
                .args(["--quiet", "plugins", "list"])
                .spawn()
                .expect("child should start"),
        );
    }
    for mut child in children {
        assert!(child.wait().expect("child should finish").success());
    }

    let records = log_records(&config_dir);
    let commands = command_records(&records);
    assert_eq!(commands.len(), 6, "{records:#?}");
    assert!(
        commands
            .iter()
            .all(|record| record["command"] == "plugins.list")
    );
}

fn assert_command(record: &Value, command: &str, status: &str) {
    assert_eq!(record["event"], "command.completed");
    assert_eq!(record["transport"], "cli");
    assert_eq!(record["command"], command);
    assert_eq!(record["status"], status);
}

fn command_records(records: &[Value]) -> Vec<&Value> {
    records
        .iter()
        .filter(|record| record["event"] == "command.completed")
        .collect()
}

fn command_output(config_dir: &TempDir) -> std::process::Output {
    Command::cargo_bin("ah")
        .expect("binary should compile")
        .env("AH_CONFIG_DIR", config_dir.path())
        .args(["--json", "plugins", "list"])
        .output()
        .expect("command should run")
}

fn assert_same_command_output(expected: &std::process::Output, actual: &std::process::Output) {
    assert_eq!(actual.status.code(), expected.status.code());
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stderr, expected.stderr);
}

fn jsonl_paths(config_dir: &TempDir) -> Vec<std::path::PathBuf> {
    let mut paths = fs::read_dir(config_dir.path().join("logs"))
        .expect("log directory should exist")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "jsonl")
        })
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

fn log_records(config_dir: &TempDir) -> Vec<Value> {
    jsonl_paths(config_dir)
        .into_iter()
        .flat_map(|path| {
            fs::read_to_string(path)
                .expect("log should be readable")
                .lines()
                .map(|line| serde_json::from_str(line).expect("log line should be JSON"))
                .collect::<Vec<Value>>()
        })
        .collect()
}

#[cfg(windows)]
fn platform_exit_command(success: bool) -> Vec<&'static str> {
    vec!["cmd.exe", "/C", if success { "exit 0" } else { "exit 7" }]
}

#[cfg(not(windows))]
fn platform_exit_command(success: bool) -> Vec<&'static str> {
    vec!["sh", "-c", if success { "exit 0" } else { "exit 7" }]
}
