use std::fs;

use assert_cmd::Command;
use predicates::str::contains;
use tempfile::TempDir;

#[test]
fn ctx_symbols_extracts_rust_symbols() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    let file_path = temp_dir.path().join("mod.rs");
    fs::write(
        &file_path,
        "pub struct User {}\n\npub fn create_user() {}\n",
    )
    .expect("test file should be written");

    let file_path_str = file_path.to_string_lossy().to_string();
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["ctx", "symbols", &file_path_str])
        .assert()
        .success()
        .stdout(contains("struct User"))
        .stdout(contains("fn create_user"));
}

#[test]
fn ctx_pack_emits_json_summary() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    let file_path = temp_dir.path().join("lib.rs");
    fs::write(&file_path, "pub fn run() {}\n").expect("test file should be written");

    let root = temp_dir.path().to_string_lossy().to_string();
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["--json", "ctx", "pack", &root, "--limit", "10"])
        .assert()
        .success()
        .stdout(contains("\"command\": \"ctx.pack\""))
        .stdout(contains("\"file_count\""))
        .stdout(contains("\"symbol_count\""));
}

#[test]
fn ctx_symbols_json_reports_skipped_binary_and_large_files() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    fs::write(temp_dir.path().join("ok.rs"), "fn good() {}\n")
        .expect("rust file should be written");
    fs::write(temp_dir.path().join("bin.bin"), [0u8, 1u8, 2u8])
        .expect("binary file should be written");
    fs::write(
        temp_dir.path().join("huge.rs"),
        "fn huge() {}\n".repeat(200),
    )
    .expect("large file should be written");

    let root = temp_dir.path().to_string_lossy().to_string();
    let assert = Command::cargo_bin("ah")
        .expect("binary should compile")
        .args(["--json", "ctx", "symbols", &root, "--max-bytes", "64"])
        .assert()
        .success();

    let payload: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("valid json output expected");
    assert_eq!(payload["command"], "ctx.symbols");
    assert_eq!(payload["skipped_binary_files"], 1);
    assert_eq!(payload["skipped_large_files"], 1);
}

#[test]
fn ctx_changed_reports_non_git_directory() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    let cwd = temp_dir.path().to_string_lossy().to_string();

    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["--json", "--cwd", &cwd, "ctx", "changed"])
        .assert()
        .success()
        .stdout(contains("\"command\": \"ctx.changed\""))
        .stdout(contains("\"in_git_repo\": false"))
        .stdout(contains("\"changed_count\": 0"));
}

#[test]
fn ctx_symbols_summary_preset_limits_symbols_per_file() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    let file_path = temp_dir.path().join("symbols.rs");
    let content = (1..=30)
        .map(|index| format!("fn func_{index}() {{}}\n"))
        .collect::<String>();
    fs::write(&file_path, content).expect("test file should be written");

    let file_path_str = file_path.to_string_lossy().to_string();
    let assert = Command::cargo_bin("ah")
        .expect("binary should compile")
        .args([
            "--json",
            "ctx",
            "symbols",
            &file_path_str,
            "--preset",
            "summary",
        ])
        .assert()
        .success();

    let payload: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("valid json output expected");
    assert_eq!(payload["command"], "ctx.symbols");
    assert_eq!(payload["preset"], "summary");
    let files = payload["files"]
        .as_array()
        .expect("files should be array in ctx symbols output");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["symbol_count"], 20);
}

#[test]
fn ctx_pack_summary_preset_limits_symbol_preview() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    let file_path = temp_dir.path().join("pack.rs");
    let content = (1..=10)
        .map(|index| format!("fn pack_func_{index}() {{}}\n"))
        .collect::<String>();
    fs::write(&file_path, content).expect("test file should be written");

    let file_path_str = file_path.to_string_lossy().to_string();
    let assert = Command::cargo_bin("ah")
        .expect("binary should compile")
        .args([
            "--json",
            "ctx",
            "pack",
            &file_path_str,
            "--preset",
            "summary",
        ])
        .assert()
        .success();

    let payload: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("valid json output expected");
    assert_eq!(payload["command"], "ctx.pack");
    assert_eq!(payload["preset"], "summary");
    let items = payload["items"]
        .as_array()
        .expect("items should be array in ctx pack output");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["symbol_count"], 10);
    let symbols = items[0]["symbols"]
        .as_array()
        .expect("symbols should be array for pack item");
    assert_eq!(symbols.len(), 4);
}

#[test]
fn ctx_symbols_extracts_extended_language_symbols() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    let root = temp_dir.path();
    fs::write(
        root.join("Main.java"),
        "package demo;\npublic interface Service {}\npublic record User(String name) {}\npublic class App {\n  public void run() {}\n}\n",
    )
    .expect("java file should be written");
    fs::write(
        root.join("App.kt"),
        "package demo\n\ndata class Person(val name: String)\nfun boot() {}\n",
    )
    .expect("kotlin file should be written");
    fs::write(
        root.join("Program.cs"),
        "namespace Demo;\npublic record Item(string Name);\npublic class App {\n  public void Run() {}\n}\n",
    )
    .expect("csharp file should be written");
    fs::write(
        root.join("lib.php"),
        "<?php\nnamespace App;\ninterface Handler {}\nfunction handle() {}\n",
    )
    .expect("php file should be written");
    fs::write(
        root.join("worker.rb"),
        "module Demo\nclass Worker\n  def perform\n  end\nend\n",
    )
    .expect("ruby file should be written");
    fs::write(
        root.join("main.dart"),
        "class Widget {}\nvoid render() {}\n",
    )
    .expect("dart file should be written");
    fs::write(
        root.join("main.tf"),
        "resource \"aws_s3_bucket\" \"logs\" {}\nmodule \"network\" {}\nvariable \"region\" {}\n",
    )
    .expect("terraform file should be written");

    let cwd = root.to_string_lossy().to_string();
    let assert = Command::cargo_bin("ah")
        .expect("binary should compile")
        .args(["--json", "ctx", "symbols", &cwd])
        .assert()
        .success();

    let payload: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("valid json output expected");
    let mut names = Vec::new();
    for file in payload["files"].as_array().expect("files should be array") {
        for symbol in file["symbols"].as_array().expect("symbols should be array") {
            names.push(format!(
                "{} {}",
                symbol["kind"].as_str().unwrap(),
                symbol["name"].as_str().unwrap()
            ));
        }
    }

    for expected in [
        "package demo",
        "interface Service",
        "record User",
        "class App",
        "class Person",
        "fun boot",
        "namespace Demo",
        "record Item",
        "namespace App",
        "interface Handler",
        "function handle",
        "module Demo",
        "class Worker",
        "def perform",
        "class Widget",
        "function render",
        "resource aws_s3_bucket.logs",
        "module network",
        "variable region",
    ] {
        assert!(
            names.iter().any(|name| name == expected),
            "missing {expected}"
        );
    }
}

#[test]
fn ctx_symbols_extracts_config_and_script_symbols() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    let root = temp_dir.path();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"demo\"\n\n[dependencies]\nserde = \"1\"\n",
    )
    .expect("toml file should be written");
    fs::write(
        root.join("compose.yaml"),
        "services:\n  app:\n    image: demo\nvolumes:\n  data:\n",
    )
    .expect("yaml file should be written");
    fs::write(root.join("script.sh"), "build() {\n  echo ok\n}\n")
        .expect("shell file should be written");
    fs::write(root.join("Dockerfile"), "FROM rust:latest AS builder\n")
        .expect("Dockerfile should be written");
    fs::write(root.join("Makefile"), "test:\n\tcargo test\n").expect("Makefile should be written");

    let cwd = root.to_string_lossy().to_string();
    Command::cargo_bin("ah")
        .expect("binary should compile")
        .args(["ctx", "symbols", &cwd])
        .assert()
        .success()
        .stdout(contains("section package"))
        .stdout(contains("section dependencies"))
        .stdout(contains("key services"))
        .stdout(contains("key volumes"))
        .stdout(contains("function build"))
        .stdout(contains("stage builder"))
        .stdout(contains("target test"));
}
