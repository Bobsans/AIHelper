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
        .stderr(contains("PATH_NOT_FOUND: path does not exist: Fixdigital"))
        .stderr(contains("hint: check path or --cwd"))
        .stderr(predicates::str::contains("invalid argument: [").not());
}

#[test]
fn search_invalid_regex_error_is_concise() {
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["search", "text", "(", "src", "--regex"])
        .assert()
        .failure()
        .stderr(contains("REGEX_INVALID: unclosed group"))
        .stderr(contains("hint: fix regex or drop --regex"))
        .stderr(predicates::str::contains("regex parse error").not());
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
fn search_uses_stable_ignore_aware_discovery() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    fs::create_dir(temp_dir.path().join(".git")).expect("git marker should be created");
    fs::write(temp_dir.path().join(".gitignore"), "ignored.txt\n")
        .expect("gitignore should be written");
    fs::write(temp_dir.path().join("visible.txt"), "needle\n")
        .expect("visible file should be written");
    fs::write(temp_dir.path().join("ignored.txt"), "needle\n")
        .expect("ignored file should be written");
    fs::write(temp_dir.path().join(".hidden.txt"), "needle\n")
        .expect("hidden file should be written");

    let root = temp_dir.path().to_string_lossy().to_string();
    let assert = Command::cargo_bin("ah")
        .expect("binary should compile")
        .args(["--json", "search", "text", "needle", &root])
        .assert()
        .success();
    let payload: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("valid JSON output");
    assert_eq!(payload["backend"], "ignore+rust");
    assert_eq!(payload["match_count"], 1);
    assert_eq!(payload["matches"][0]["path"], "visible.txt");
}

#[test]
fn search_text_json_reports_character_column_for_unicode_lines() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    fs::write(temp_dir.path().join("unicode.txt"), "a\u{00e9}needle\n")
        .expect("test file should be written");

    let root = temp_dir.path().to_string_lossy().to_string();
    let assert = Command::cargo_bin("ah")
        .expect("binary should compile")
        .args(["--json", "search", "text", "needle", &root])
        .assert()
        .success();

    let payload: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("valid json output expected");
    assert_eq!(payload["matches"][0]["column"], 3);
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
fn search_text_accepts_multiple_paths() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    let left = temp_dir.path().join("left");
    let right = temp_dir.path().join("right");
    fs::create_dir(&left).expect("left dir should be created");
    fs::create_dir(&right).expect("right dir should be created");
    fs::write(left.join("one.txt"), "needle in left\n").expect("left file should be written");
    fs::write(right.join("two.txt"), "needle in right\n").expect("right file should be written");

    let left_root = left.to_string_lossy().to_string();
    let right_root = right.to_string_lossy().to_string();
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["search", "text", "needle", &left_root, &right_root])
        .assert()
        .success()
        .stdout(contains("one.txt"))
        .stdout(contains("two.txt"));
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

#[test]
fn search_files_accepts_multiple_paths() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    let left = temp_dir.path().join("left");
    let right = temp_dir.path().join("right");
    fs::create_dir(&left).expect("left dir should be created");
    fs::create_dir(&right).expect("right dir should be created");
    fs::write(left.join("alpha_left.txt"), "a").expect("left file should be written");
    fs::write(right.join("alpha_right.txt"), "b").expect("right file should be written");
    fs::write(right.join("beta.txt"), "c").expect("beta file should be written");

    let left_root = left.to_string_lossy().to_string();
    let right_root = right.to_string_lossy().to_string();
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["search", "files", "alpha", &left_root, &right_root])
        .assert()
        .success()
        .stdout(contains("alpha_left.txt"))
        .stdout(contains("alpha_right.txt"))
        .stdout(predicates::str::contains("beta.txt").not());
}
