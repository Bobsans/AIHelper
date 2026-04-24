use std::fs;

use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use tempfile::TempDir;

#[test]
fn search_path_not_found_error_is_rendered_without_nested_wrappers() {
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["search", "text", "customFieldValues", "Fixdigital"])
        .assert()
        .failure()
        .stderr(contains("error[PATH_NOT_FOUND]: path does not exist"))
        .stderr(contains("path: Fixdigital"))
        .stderr(predicates::str::contains("invalid argument: [").not());
}

#[test]
fn search_text_plain_mode_treats_pattern_as_literal() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    fs::write(temp_dir.path().join("notes.txt"), "a.c\nabc\n")
        .expect("test file should be written");

    let root = temp_dir.path().to_string_lossy().to_string();
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["search", "text", "a.c", &root])
        .assert()
        .success()
        .stdout(contains("notes.txt:1:a.c"))
        .stdout(predicates::str::contains("notes.txt:2:abc").not());
}

#[test]
fn search_text_regex_mode_matches_regex() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    fs::write(temp_dir.path().join("app.log"), "id=42\nid=ab\n")
        .expect("test file should be written");

    let root = temp_dir.path().to_string_lossy().to_string();
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["search", "text", "id=\\d+", &root, "--regex"])
        .assert()
        .success()
        .stdout(contains("app.log:1:id=42"))
        .stdout(predicates::str::contains("app.log:2:id=ab").not());
}

#[test]
fn search_text_with_context_includes_neighbor_lines() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    fs::write(
        temp_dir.path().join("notes.txt"),
        "before\nmatch line\nafter\n",
    )
    .expect("test file should be written");

    let root = temp_dir.path().to_string_lossy().to_string();
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["search", "text", "match", &root, "--context", "1"])
        .assert()
        .success()
        .stdout(contains("notes.txt-1-before"))
        .stdout(contains("notes.txt:2:match line"))
        .stdout(contains("notes.txt-3-after"));
}

#[test]
fn search_text_context_does_not_include_extra_after_line() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    fs::write(
        temp_dir.path().join("notes.txt"),
        "before-1\nbefore-2\nmatch-line\nafter-1\nafter-2\nafter-3\n",
    )
    .expect("test file should be written");

    let root = temp_dir.path().to_string_lossy().to_string();
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["search", "text", "match-line", &root, "--context", "2"])
        .assert()
        .success()
        .stdout(contains("notes.txt-1-before-1"))
        .stdout(contains("notes.txt-2-before-2"))
        .stdout(contains("notes.txt:3:match-line"))
        .stdout(contains("notes.txt-4-after-1"))
        .stdout(contains("notes.txt-5-after-2"))
        .stdout(predicates::str::contains("notes.txt-6-after-3").not());
}

#[test]
fn search_text_json_contains_mode_fields() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    fs::write(temp_dir.path().join("one.txt"), "alpha\nbeta\n")
        .expect("test file should be written");

    let root = temp_dir.path().to_string_lossy().to_string();
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["--json", "search", "text", "alpha", &root])
        .assert()
        .success()
        .stdout(contains("\"command\": \"search.text\""))
        .stdout(contains("\"regex\": false"))
        .stdout(contains("\"match_count\": 1"));
}

#[test]
fn search_text_json_reports_skipped_binary_and_large_files() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    fs::write(temp_dir.path().join("ok.txt"), "match here\n").expect("text file should be written");
    fs::write(
        temp_dir.path().join("bin.dat"),
        [0u8, b'm', b'a', b't', b'c', b'h'],
    )
    .expect("binary file should be written");
    fs::write(temp_dir.path().join("huge.txt"), "match ".repeat(200))
        .expect("large file should be written");

    let root = temp_dir.path().to_string_lossy().to_string();
    let assert = Command::cargo_bin("ah")
        .expect("binary should compile")
        .args([
            "--json",
            "search",
            "text",
            "match",
            &root,
            "--max-bytes",
            "32",
        ])
        .assert()
        .success();

    let payload: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("valid json output expected");
    assert_eq!(payload["command"], "search.text");
    assert_eq!(payload["match_count"], 1);
    let skipped_binary = payload["skipped_binary_files"]
        .as_u64()
        .expect("skipped_binary_files should be a number");
    assert!(skipped_binary <= 1);
    assert_eq!(payload["skipped_large_files"], 1);
}

#[test]
fn search_files_returns_matching_paths() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    fs::write(temp_dir.path().join("alpha.txt"), "a").expect("file should be written");
    fs::write(temp_dir.path().join("beta.txt"), "b").expect("file should be written");
    fs::create_dir(temp_dir.path().join("nested")).expect("dir should be created");
    fs::write(temp_dir.path().join("nested").join("alpha_notes.md"), "c")
        .expect("file should be written");

    let root = temp_dir.path().to_string_lossy().to_string();
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["search", "files", "alpha", &root])
        .assert()
        .success()
        .stdout(contains("alpha.txt"))
        .stdout(contains("alpha_notes.md"))
        .stdout(predicates::str::contains("beta.txt").not());
}
