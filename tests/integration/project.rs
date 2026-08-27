//! `ah project`, run in this process.
//!
//! The JSON assertions index into the payload. The versions of these tests that
//! spawned `ah` asserted `contains("\"rust\"")` and `contains("\"cargo\"")`,
//! which match a quoted string *anywhere* in the payload - an ecosystem name
//! matching because it appeared in a file path would have passed just as well.
//!
//! The three `contains("\u{1b}").not()` checks these had are gone: the captured
//! emitter has colour off by construction, so it cannot observe the decision
//! `Emitter::stdio` makes from `is_terminal`. That property is asserted once, in
//! `git.rs`, because it belongs to the emitter rather than to any domain.

use std::{fs, path::Path};

use aihelper::harness::Harness;
use serde_json::Value;
use tempfile::TempDir;

fn project_json(args: &[&str]) -> Value {
    let mut argv = vec!["--json", "project"];
    argv.extend_from_slice(args);
    Harness::new().run(&argv).json()
}

fn project_text(args: &[&str]) -> String {
    let mut argv = vec!["project"];
    argv.extend_from_slice(args);
    Harness::new().run(&argv).expect_success().to_owned()
}

fn root_of(dir: &Path) -> String {
    dir.to_string_lossy().into_owned()
}

/// The named strings of an array field, so a membership assertion reads as one.
fn names<'a>(payload: &'a Value, field: &str) -> Vec<&'a str> {
    payload[field]
        .as_array()
        .unwrap_or_else(|| panic!("{field} should be an array: {payload}"))
        .iter()
        .filter_map(Value::as_str)
        .collect()
}

fn assert_contains_all(actual: &[&str], expected: &[&str], field: &str) {
    for value in expected {
        assert!(
            actual.contains(value),
            "{field} should include {value}: {actual:?}"
        );
    }
}

#[test]
fn project_detect_reports_ecosystems_and_key_files() {
    let temp_dir = sample_project();
    let cwd = root_of(temp_dir.path());

    let payload = project_json(&["detect", &cwd]);
    assert_eq!(payload["command"], "project.detect");
    assert_contains_all(
        &names(&payload, "ecosystems"),
        &["rust", "node"],
        "ecosystems",
    );
    assert_contains_all(&names(&payload, "tools"), &["github-actions"], "tools");
    // Two different file groups, which the `contains` assertions this replaces
    // could not distinguish: both names matched anywhere in the payload.
    let paths = |group: &str| {
        payload["files"][group]
            .as_array()
            .unwrap_or_else(|| panic!("{group} file group: {payload}"))
            .iter()
            .filter_map(|file| file["path"].as_str())
            .collect::<Vec<_>>()
    };
    assert_contains_all(&paths("docs"), &["README.md"], "docs");
    assert_contains_all(&paths("changelogs"), &["CHANGELOG.md"], "changelogs");

    let text = project_text(&["detect", &cwd]);
    for expected in ["root=", "ecosystems=", "rust", "node"] {
        assert!(text.contains(expected), "{expected} missing from {text}");
    }
}

#[test]
fn project_commands_suggests_common_commands() {
    let temp_dir = sample_project();
    let cwd = root_of(temp_dir.path());

    let payload = project_json(&["commands", &cwd]);
    assert_eq!(payload["command"], "project.commands");
    let programs = payload["commands"]
        .as_array()
        .expect("commands array")
        .iter()
        .filter_map(|entry| entry["command"][0].as_str())
        .collect::<Vec<_>>();
    assert_contains_all(&programs, &["cargo", "npm"], "command programs");
    let kinds = payload["commands"]
        .as_array()
        .expect("commands array")
        .iter()
        .filter_map(|entry| entry["kind"].as_str())
        .collect::<Vec<_>>();
    assert_contains_all(&kinds, &["test", "build"], "command kinds");

    assert!(project_text(&["commands", &cwd]).contains("test:"));
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
    let cwd = root_of(root);

    let payload = project_json(&["version", &cwd]);
    assert_eq!(payload["command"], "project.version");
    let versions = payload["versions"].as_array().expect("versions array");
    for (kind, version) in [("cargo", "0.1.0"), ("npm", "1.2.3"), ("python", "2.3.4")] {
        assert!(
            versions
                .iter()
                .any(|entry| entry["kind"] == kind && entry["version"] == version),
            "{kind} {version} missing from {payload}"
        );
    }

    let text = project_text(&["version", &cwd]);
    assert!(text.contains("version=0.1.0"), "{text}");
    assert!(text.contains("confidence=high"), "{text}");
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

    let payload = project_json(&["detect", &root_of(root)]);

    assert_contains_all(
        &names(&payload, "ecosystems"),
        &["node", "php", "terraform", "flutter"],
        "ecosystems",
    );
    assert_contains_all(&names(&payload, "tools"), &["pnpm", "docker"], "tools");
    assert_eq!(payload["files"]["locks"][0]["kind"], "pnpm-lock");
    assert_eq!(payload["files"]["deploy"][0]["kind"], "dockerfile");
    assert_eq!(payload["files"]["infra"][0]["kind"], "terraform");
    assert!(
        payload["versions"]
            .as_array()
            .expect("versions array")
            .iter()
            .any(|entry| entry["kind"] == "composer" && entry["version"] == "4.5.6"),
        "{payload}"
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

    let payload = project_json(&["commands", &root_of(root)]);

    let entries = payload["commands"].as_array().expect("commands array");
    let programs = entries
        .iter()
        .filter_map(|entry| entry["command"][0].as_str())
        .collect::<Vec<_>>();
    assert_contains_all(&programs, &["pnpm", "terraform"], "command programs");
    let kinds = entries
        .iter()
        .filter_map(|entry| entry["kind"].as_str())
        .collect::<Vec<_>>();
    assert_contains_all(&kinds, &["validate"], "command kinds");
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

    let payload = project_json(&["detect", &root_of(root)]);

    assert_contains_all(
        &names(&payload, "roles"),
        &[
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
        ],
        "roles",
    );
    assert_contains_all(
        &names(&payload, "tools"),
        &[
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
        ],
        "tools",
    );
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
