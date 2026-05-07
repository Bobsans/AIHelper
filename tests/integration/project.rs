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

#[test]
fn project_version_reports_manifest_versions() {
    let temp_dir = sample_project();
    let root = temp_dir.path();
    fs::write(
        root.join("pyproject.toml"),
        "[project]\nname = \"demo-python\"\nversion = \"2.3.4\"\n",
    )
    .expect("pyproject.toml should be written");
    let cwd = root.to_string_lossy().to_string();

    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["--json", "project", "version", &cwd])
        .assert()
        .success()
        .stdout(contains("\"command\": \"project.version\""))
        .stdout(contains("\"kind\": \"cargo\""))
        .stdout(contains("\"version\": \"0.1.0\""))
        .stdout(contains("\"kind\": \"npm\""))
        .stdout(contains("\"version\": \"1.2.3\""))
        .stdout(contains("\"kind\": \"python\""))
        .stdout(contains("\"version\": \"2.3.4\""));
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
        "{\"name\":\"demo-node\",\"version\":\"1.2.3\",\"scripts\":{\"build\":\"echo ok\"}}\n",
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
