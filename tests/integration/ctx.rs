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
