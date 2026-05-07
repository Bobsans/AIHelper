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

#[test]
fn project_detect_reports_broad_ecosystems_tools_and_file_groups() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    let root = temp_dir.path();
    fs::write(
        root.join("package.json"),
        "{\"name\":\"web\",\"version\":\"1.2.3\",\"scripts\":{\"test\":\"vitest\",\"build\":\"vite build\"}}\n",
    )
    .expect("package.json should be written");
    fs::write(root.join("pnpm-lock.yaml"), "lockfileVersion: '9.0'\n")
        .expect("pnpm lock should be written");
    fs::write(
        root.join("composer.json"),
        "{\"name\":\"demo/app\",\"version\":\"4.5.6\"}\n",
    )
    .expect("composer.json should be written");
    fs::write(root.join("Dockerfile"), "FROM scratch\n").expect("Dockerfile should be written");
    fs::write(root.join("main.tf"), "terraform {}\n").expect("terraform file should be written");
    fs::write(
        root.join("pubspec.yaml"),
        "name: mobile\nversion: 7.8.9\nflutter:\n",
    )
    .expect("pubspec should be written");

    let cwd = root.to_string_lossy().to_string();
    let assert = Command::cargo_bin("ah")
        .expect("binary should compile")
        .args(["--json", "project", "detect", &cwd])
        .assert()
        .success();

    let payload: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("valid json output expected");
    assert!(
        payload["ecosystems"]
            .as_array()
            .unwrap()
            .contains(&"node".into())
    );
    assert!(
        payload["ecosystems"]
            .as_array()
            .unwrap()
            .contains(&"php".into())
    );
    assert!(
        payload["ecosystems"]
            .as_array()
            .unwrap()
            .contains(&"terraform".into())
    );
    assert!(
        payload["ecosystems"]
            .as_array()
            .unwrap()
            .contains(&"flutter".into())
    );
    assert!(
        payload["tools"]
            .as_array()
            .unwrap()
            .contains(&"pnpm".into())
    );
    assert!(
        payload["tools"]
            .as_array()
            .unwrap()
            .contains(&"docker".into())
    );
    assert_eq!(payload["files"]["locks"][0]["kind"], "pnpm-lock");
    assert_eq!(payload["files"]["deploy"][0]["kind"], "dockerfile");
    assert_eq!(payload["files"]["infra"][0]["kind"], "terraform");
    assert!(
        payload["versions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| { entry["kind"] == "composer" && entry["version"] == "4.5.6" })
    );
}

#[test]
fn project_commands_use_detected_package_managers_and_infra_tools() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    let root = temp_dir.path();
    fs::write(
        root.join("package.json"),
        "{\"name\":\"web\",\"scripts\":{\"test\":\"vitest\",\"build\":\"vite build\"}}\n",
    )
    .expect("package.json should be written");
    fs::write(root.join("pnpm-lock.yaml"), "lockfileVersion: '9.0'\n")
        .expect("pnpm lock should be written");
    fs::write(root.join("main.tf"), "terraform {}\n").expect("terraform file should be written");

    let cwd = root.to_string_lossy().to_string();
    Command::cargo_bin("ah")
        .expect("binary should compile")
        .args(["--json", "project", "commands", &cwd])
        .assert()
        .success()
        .stdout(contains("\"pnpm\""))
        .stdout(contains("\"terraform\""))
        .stdout(contains("\"validate\""));
}

#[test]
fn project_detect_infers_platform_and_tooling_roles() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    let root = temp_dir.path();
    fs::write(
        root.join("package.json"),
        r#"{
          "name":"multi",
          "dependencies":{
            "next":"latest",
            "express":"latest",
            "react-native":"latest",
            "electron":"latest"
          },
          "devDependencies":{
            "playwright":"latest"
          }
        }"#,
    )
    .expect("package.json should be written");
    fs::write(root.join("build.sbt"), "scalaVersion := \"3.5.0\"\n")
        .expect("build.sbt should be written");
    fs::write(root.join("Project.toml"), "name = \"Analysis\"\n")
        .expect("Project.toml should be written");
    fs::write(root.join("platformio.ini"), "[env:native]\n")
        .expect("platformio.ini should be written");
    fs::write(root.join("Pulumi.yaml"), "name: cloud\n").expect("Pulumi.yaml should be written");
    fs::write(root.join("semgrep.yml"), "rules: []\n").expect("semgrep config should be written");
    fs::write(root.join(".pre-commit-config.yaml"), "repos: []\n")
        .expect("pre-commit config should be written");
    fs::create_dir_all(root.join("ProjectSettings")).expect("ProjectSettings should be created");
    fs::write(
        root.join("ProjectSettings").join("ProjectVersion.txt"),
        "m_EditorVersion: 6000.0\n",
    )
    .expect("Unity project version should be written");

    let cwd = root.to_string_lossy().to_string();
    let assert = Command::cargo_bin("ah")
        .expect("binary should compile")
        .args(["--json", "project", "detect", &cwd])
        .assert()
        .success();
    let payload: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("valid json output expected");
    let roles = payload["roles"].as_array().expect("roles should be array");
    for role in [
        "web",
        "backend",
        "mobile",
        "desktop",
        "data-science",
        "embedded",
        "cloud",
        "game",
        "quality",
        "security",
    ] {
        assert!(roles.contains(&role.into()), "missing role {role}");
    }
    let tools = payload["tools"].as_array().expect("tools should be array");
    for tool in [
        "sbt",
        "julia",
        "platformio",
        "pulumi",
        "semgrep",
        "pre-commit",
        "next",
        "express",
        "react-native",
        "electron",
    ] {
        assert!(tools.contains(&tool.into()), "missing tool {tool}");
    }
    assert_eq!(payload["files"]["quality"][0]["kind"], "pre-commit");
    assert_eq!(payload["files"]["security"][0]["kind"], "semgrep");
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
