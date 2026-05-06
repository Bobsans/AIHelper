use std::fs;

use assert_cmd::Command;
use predicates::str::contains;
use tempfile::TempDir;

#[test]
fn project_detect_reports_ecosystems_and_key_files() {
    let temp_dir = sample_project();
    let cwd = temp_dir.path().to_string_lossy().to_string();

    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["--json", "project", "detect", &cwd])
        .assert()
        .success()
        .stdout(contains("\"command\": \"project.detect\""))
        .stdout(contains("\"rust\""))
        .stdout(contains("\"node\""))
        .stdout(contains("\"github-actions\""))
        .stdout(contains("\"README.md\""))
        .stdout(contains("\"CHANGELOG.md\""));
}

#[test]
fn project_commands_suggests_common_commands() {
    let temp_dir = sample_project();
    let cwd = temp_dir.path().to_string_lossy().to_string();

    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["--json", "project", "commands", &cwd])
        .assert()
        .success()
        .stdout(contains("\"command\": \"project.commands\""))
        .stdout(contains("\"cargo\""))
        .stdout(contains("\"test\""))
        .stdout(contains("\"npm\""))
        .stdout(contains("\"build\""));
}

fn sample_project() -> TempDir {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    let root = temp_dir.path();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n",
    )
    .expect("Cargo.toml should be written");
    fs::write(
        root.join("package.json"),
        "{\"scripts\":{\"build\":\"echo ok\"}}\n",
    )
    .expect("package.json should be written");
    fs::write(root.join("README.md"), "# Demo\n").expect("README should be written");
    fs::write(root.join("CHANGELOG.md"), "# Changelog\n").expect("changelog should be written");
    fs::create_dir_all(root.join(".github").join("workflows"))
        .expect("workflow directory should be created");
    fs::write(
        root.join(".github").join("workflows").join("ci.yml"),
        "name: CI\n",
    )
    .expect("workflow should be written");
    temp_dir
}
