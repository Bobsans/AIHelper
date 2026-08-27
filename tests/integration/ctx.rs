//! `ah ctx`, run in this process.
//!
//! Two tests here were named `..._has_no_ansi_when_captured`, and that is the
//! one property the harness cannot observe: the captured emitter has colour off
//! by construction, so it says nothing about what `Emitter::stdio` decides from
//! `is_terminal`. Rather than keep a test whose name claims a check it no longer
//! makes, both are renamed to what they do assert - the text a non-git
//! directory and a packed file produce - and the colour property is asserted
//! once, in `git.rs`, where it belongs to the emitter rather than to a domain.

use std::fs;

use aihelper::harness::Harness;
use serde_json::Value;
use tempfile::TempDir;

fn ctx_json(args: &[&str]) -> Value {
    let mut argv = vec!["--json", "ctx"];
    argv.extend_from_slice(args);
    Harness::new().run(&argv).json()
}

fn ctx_text(args: &[&str]) -> String {
    let mut argv = vec!["ctx"];
    argv.extend_from_slice(args);
    Harness::new().run(&argv).expect_success().to_owned()
}

#[test]
fn ctx_symbols_extracts_rust_symbols() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    let file_path = temp_dir.path().join("mod.rs");
    fs::write(
        &file_path,
        "pub struct User {}\n\npub fn create_user() {}\n",
    )
    .expect("test file should be written");

    let output = ctx_text(&["symbols", &file_path.to_string_lossy()]);

    assert!(output.contains("struct User"), "{output}");
    assert!(output.contains("fn create_user"), "{output}");
}

#[test]
fn ctx_pack_text_output_names_its_preset_and_files() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    let file_path = temp_dir.path().join("lib.rs");
    fs::write(&file_path, "pub fn run() {}\n").expect("test file should be written");

    let output = ctx_text(&["pack", &file_path.to_string_lossy()]);

    assert!(output.contains("preset: review"), "{output}");
    assert!(output.contains("file |"), "{output}");
}

#[test]
fn ctx_pack_emits_json_summary() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    let file_path = temp_dir.path().join("lib.rs");
    fs::write(&file_path, "pub fn run() {}\n").expect("test file should be written");

    let payload = ctx_json(&["pack", &temp_dir.path().to_string_lossy(), "--limit", "10"]);

    assert_eq!(payload["command"], "ctx.pack");
    assert_eq!(payload["file_count"], 1);
    assert!(payload["symbol_count"].is_u64(), "{payload}");
}

#[test]
fn ctx_symbols_json_reports_skipped_binary_and_large_files() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    fs::write(temp_dir.path().join("ok.rs"), "fn good() {}\n")
        .expect("text file should be written");
    fs::write(temp_dir.path().join("bin.rs"), [0u8, b'f', b'n', b' '])
        .expect("binary file should be written");
    fs::write(
        temp_dir.path().join("huge.rs"),
        "fn padded() {}\n".repeat(64),
    )
    .expect("large file should be written");

    let payload = ctx_json(&[
        "symbols",
        &temp_dir.path().to_string_lossy(),
        "--max-bytes",
        "64",
    ]);

    assert_eq!(payload["command"], "ctx.symbols");
    assert_eq!(payload["skipped_binary_files"], 1);
    assert_eq!(payload["skipped_large_files"], 1);
}

#[test]
fn ctx_symbols_skips_invalid_utf8_after_prefix_sniff() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    fs::write(temp_dir.path().join("ok.rs"), "fn good() {}\n")
        .expect("text file should be written");
    let mut invalid = vec![b' '; 8193];
    invalid.extend_from_slice(b"fn hidden() {}\n");
    invalid.push(0xff);
    fs::write(temp_dir.path().join("invalid.rs"), invalid)
        .expect("invalid UTF-8 file should be written");

    let payload = ctx_json(&["symbols", &temp_dir.path().to_string_lossy()]);

    assert_eq!(payload["skipped_binary_files"], 1);
    assert_eq!(payload["file_count"], 1);
    assert_eq!(payload["files"][0]["symbols"][0]["name"], "good");
}

#[test]
fn ctx_pack_skips_invalid_utf8_after_prefix_sniff() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    let invalid_path = temp_dir.path().join("invalid.rs");
    let mut invalid = vec![b' '; 8193];
    invalid.extend_from_slice(b"fn hidden() {}\n");
    invalid.push(0xff);
    fs::write(&invalid_path, invalid).expect("invalid UTF-8 file should be written");

    let payload = ctx_json(&["pack", &temp_dir.path().to_string_lossy()]);

    assert_eq!(payload["skipped_binary_files"], 1);
    assert_eq!(payload["file_count"], 1);
    assert_eq!(payload["items"][0]["line_count"], 0);
    assert_eq!(payload["items"][0]["symbol_count"], 0);
}

#[test]
fn ctx_changed_reports_non_git_directory() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");

    let payload = Harness::new()
        .in_directory(temp_dir.path())
        .run(&["--json", "ctx", "changed"])
        .json();

    assert_eq!(payload["command"], "ctx.changed");
    assert_eq!(payload["in_git_repo"], false);
    assert_eq!(payload["changed_count"], 0);
}

#[test]
fn ctx_changed_says_so_in_text_for_a_non_git_directory() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");

    let output = Harness::new()
        .in_directory(temp_dir.path())
        .run(&["ctx", "changed"])
        .expect_success()
        .to_owned();

    assert_eq!(output, "not a git repository\n");
}

#[test]
fn ctx_symbols_summary_preset_limits_symbols_per_file() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    let file_path = temp_dir.path().join("symbols.rs");
    let content = (1..=30)
        .map(|index| format!("fn func_{index}() {{}}\n"))
        .collect::<String>();
    fs::write(&file_path, content).expect("test file should be written");

    let payload = ctx_json(&[
        "symbols",
        &file_path.to_string_lossy(),
        "--preset",
        "summary",
    ]);

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

    let payload = ctx_json(&["pack", &file_path.to_string_lossy(), "--preset", "summary"]);

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

    let payload = ctx_json(&["symbols", &root.to_string_lossy()]);

    let mut names = Vec::new();
    for file in payload["files"].as_array().expect("files should be array") {
        for symbol in file["symbols"].as_array().expect("symbols should be array") {
            names.push(format!(
                "{} {}",
                symbol["kind"].as_str().unwrap_or_default(),
                symbol["name"].as_str().unwrap_or_default()
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

    let output = ctx_text(&["symbols", &root.to_string_lossy()]);

    for expected in [
        "section package",
        "section dependencies",
        "key services",
        "key volumes",
        "function build",
        "stage builder",
        "target test",
    ] {
        assert!(
            output.contains(expected),
            "{expected} missing from {output}"
        );
    }
}
