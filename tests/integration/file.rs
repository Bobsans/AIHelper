use std::fs;

use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use tempfile::{NamedTempFile, TempDir};

use crate::common::{create_dir_symlink, create_file_symlink};

#[test]
fn file_read_rejects_binary_file() {
    let temp = NamedTempFile::new().expect("temporary file should be created");
    fs::write(temp.path(), [0u8, b'a', b'b', b'c']).expect("binary content should be written");

    let file_path = temp.path().to_string_lossy().to_string();
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["file", "read", &file_path])
        .assert()
        .failure()
        .stderr(contains("binary or non-UTF8 file is not supported"));
}

#[test]
fn file_read_respects_max_bytes() {
    let temp = NamedTempFile::new().expect("temporary file should be created");
    fs::write(temp.path(), "0123456789\n").expect("content should be written");

    let file_path = temp.path().to_string_lossy().to_string();
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["file", "read", &file_path, "--max-bytes", "4"])
        .assert()
        .failure()
        .stderr(contains("file is too large"));
}

#[test]
fn file_read_rejects_zero_from() {
    let temp = NamedTempFile::new().expect("temporary file should be created");
    fs::write(temp.path(), "alpha\nbeta\n").expect("content should be written");

    let file_path = temp.path().to_string_lossy().to_string();
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["file", "read", &file_path, "--from", "0"])
        .assert()
        .failure()
        .stderr(contains("--from must be >= 1"));
}

#[test]
fn file_read_rejects_zero_to() {
    let temp = NamedTempFile::new().expect("temporary file should be created");
    fs::write(temp.path(), "alpha\nbeta\n").expect("content should be written");

    let file_path = temp.path().to_string_lossy().to_string();
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["file", "read", &file_path, "--to", "0"])
        .assert()
        .failure()
        .stderr(contains("--to must be >= 1"));
}

#[test]
fn file_read_rejects_to_before_from() {
    let temp = NamedTempFile::new().expect("temporary file should be created");
    fs::write(temp.path(), "alpha\nbeta\n").expect("content should be written");

    let file_path = temp.path().to_string_lossy().to_string();
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["file", "read", &file_path, "--from", "3", "--to", "2"])
        .assert()
        .failure()
        .stderr(contains("--to must be >= --from"));
}

#[test]
fn file_read_symlink_requires_follow_flag() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    let target_path = temp_dir.path().join("target.txt");
    let link_path = temp_dir.path().join("link.txt");
    fs::write(&target_path, "hello via symlink\n").expect("target file should be written");

    if !create_file_symlink(&link_path, &target_path) {
        return;
    }

    let link = link_path.to_string_lossy().to_string();
    Command::cargo_bin("ah")
        .expect("binary should compile")
        .args(["file", "read", &link])
        .assert()
        .failure()
        .stderr(contains("symlink traversal is disabled"));

    Command::cargo_bin("ah")
        .expect("binary should compile")
        .args(["file", "read", &link, "--follow-symlinks"])
        .assert()
        .success()
        .stdout(contains("hello via symlink"));
}

#[test]
fn file_read_limit_sets_truncated_warning() {
    let temp = NamedTempFile::new().expect("temporary file should be created");
    fs::write(temp.path(), "line-1\nline-2\nline-3\n")
        .expect("temporary content should be written");

    let file_path = temp.path().to_string_lossy().to_string();
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["--limit", "2", "file", "read", &file_path])
        .assert()
        .success()
        .stdout(contains("line-1\nline-2"))
        .stdout(predicates::str::contains("line-3").not())
        .stderr(contains("warning: output truncated by --limit"));
}

#[test]
fn file_read_quiet_suppresses_output_and_warnings() {
    let temp = NamedTempFile::new().expect("temporary file should be created");
    fs::write(temp.path(), "line-1\nline-2\nline-3\n")
        .expect("temporary content should be written");

    let file_path = temp.path().to_string_lossy().to_string();
    let assert = Command::cargo_bin("ah")
        .expect("binary should compile")
        .args(["--quiet", "--limit", "1", "file", "read", &file_path])
        .assert()
        .success();

    assert!(
        assert.get_output().stdout.is_empty(),
        "stdout should be empty in quiet mode"
    );
    assert!(
        assert.get_output().stderr.is_empty(),
        "stderr should be empty in quiet mode"
    );
}

#[test]
fn reads_range_with_line_numbers() {
    let temp = NamedTempFile::new().expect("temporary file should be created");
    fs::write(temp.path(), "alpha\nbeta\ngamma\ndelta\n")
        .expect("temporary content should be written");

    let file_path = temp.path().to_string_lossy().to_string();

    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["file", "read", &file_path, "-n", "--from", "2", "--to", "3"])
        .assert()
        .success()
        .stdout(contains("   2: beta\n   3: gamma"));
}

#[test]
fn emits_json_for_file_read() {
    let temp = NamedTempFile::new().expect("temporary file should be created");
    fs::write(temp.path(), "one\ntwo\n").expect("temporary content should be written");

    let file_path = temp.path().to_string_lossy().to_string();

    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args([
        "--json", "file", "read", &file_path, "--from", "1", "--to", "1",
    ])
    .assert()
    .success()
    .stdout(contains("\"command\": \"file.read\""))
    .stdout(contains("\"line_count\": 1"));
}

#[test]
fn reads_head_with_line_numbers() {
    let temp = NamedTempFile::new().expect("temporary file should be created");
    fs::write(temp.path(), "alpha\nbeta\ngamma\ndelta\n")
        .expect("temporary content should be written");

    let file_path = temp.path().to_string_lossy().to_string();

    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["file", "head", &file_path, "--lines", "2", "-n"])
        .assert()
        .success()
        .stdout(contains("   1: alpha\n   2: beta"));
}

#[test]
fn file_head_zero_lines_returns_empty_output() {
    let temp = NamedTempFile::new().expect("temporary file should be created");
    fs::write(temp.path(), "alpha\nbeta\ngamma\n").expect("temporary content should be written");

    let file_path = temp.path().to_string_lossy().to_string();
    let assert = Command::cargo_bin("ah")
        .expect("binary should compile")
        .args(["file", "head", &file_path, "--lines", "0"])
        .assert()
        .success();

    assert!(
        assert.get_output().stdout.is_empty(),
        "stdout should be empty when --lines is 0"
    );
}

#[test]
fn reads_tail_with_line_numbers() {
    let temp = NamedTempFile::new().expect("temporary file should be created");
    fs::write(temp.path(), "alpha\nbeta\ngamma\ndelta\n")
        .expect("temporary content should be written");

    let file_path = temp.path().to_string_lossy().to_string();

    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["file", "tail", &file_path, "--lines", "2", "-n"])
        .assert()
        .success()
        .stdout(contains("   3: gamma\n   4: delta"));
}

#[test]
fn file_tail_zero_lines_returns_empty_output() {
    let temp = NamedTempFile::new().expect("temporary file should be created");
    fs::write(temp.path(), "alpha\nbeta\ngamma\n").expect("temporary content should be written");

    let file_path = temp.path().to_string_lossy().to_string();
    let assert = Command::cargo_bin("ah")
        .expect("binary should compile")
        .args(["file", "tail", &file_path, "--lines", "0"])
        .assert()
        .success();

    assert!(
        assert.get_output().stdout.is_empty(),
        "stdout should be empty when --lines is 0"
    );
}

#[test]
fn emits_json_for_file_stat() {
    let temp = NamedTempFile::new().expect("temporary file should be created");
    fs::write(temp.path(), "abc\n").expect("temporary content should be written");

    let file_path = temp.path().to_string_lossy().to_string();

    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["--json", "file", "stat", &file_path])
        .assert()
        .success()
        .stdout(contains("\"command\": \"file.stat\""))
        .stdout(contains("\"kind\": \"file\""));
}

#[test]
fn file_stat_reports_symlink_kind() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    let target_path = temp_dir.path().join("target.txt");
    let link_path = temp_dir.path().join("link.txt");
    fs::write(&target_path, "hello\n").expect("target file should be written");

    if !create_file_symlink(&link_path, &target_path) {
        return;
    }

    let link = link_path.to_string_lossy().to_string();
    Command::cargo_bin("ah")
        .expect("binary should compile")
        .args(["--json", "file", "stat", &link])
        .assert()
        .success()
        .stdout(contains("\"command\": \"file.stat\""))
        .stdout(contains("\"kind\": \"symlink\""));
}

#[test]
fn emits_json_for_file_tree_with_depth() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    let root = temp_dir.path().join("root");
    let nested = root.join("nested");

    fs::create_dir(&root).expect("root directory should be created");
    fs::create_dir(&nested).expect("nested directory should be created");
    fs::write(root.join("top.txt"), "top").expect("top file should be created");
    fs::write(nested.join("deep.txt"), "deep").expect("deep file should be created");

    let root_path = root.to_string_lossy().to_string();

    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["--json", "file", "tree", &root_path, "--depth", "1"])
        .assert()
        .success()
        .stdout(contains("\"command\": \"file.tree\""))
        .stdout(contains("\"entry_count\": 3"));
}

#[test]
fn file_tree_symlink_directory_requires_follow_flag() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    let root = temp_dir.path().join("root");
    let target_dir = temp_dir.path().join("target");
    let link_dir = root.join("linked-target");

    fs::create_dir(&root).expect("root directory should be created");
    fs::create_dir(&target_dir).expect("target directory should be created");
    fs::write(target_dir.join("inside.txt"), "inside").expect("target file should be created");

    if !create_dir_symlink(&link_dir, &target_dir) {
        return;
    }

    let root_path = root.to_string_lossy().to_string();
    Command::cargo_bin("ah")
        .expect("binary should compile")
        .args(["file", "tree", &root_path])
        .assert()
        .success()
        .stdout(contains("linked-target"))
        .stdout(predicates::str::contains("inside.txt").not());

    Command::cargo_bin("ah")
        .expect("binary should compile")
        .args(["file", "tree", &root_path, "--follow-symlinks"])
        .assert()
        .success()
        .stdout(contains("linked-target"))
        .stdout(contains("inside.txt"));
}
