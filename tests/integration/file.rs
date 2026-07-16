use std::{collections::BTreeSet, fs, path::Path};

use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use serde_json::Value;
use tempfile::{NamedTempFile, TempDir};

use crate::common::{create_dir_symlink, create_file_symlink};

const LINE_OUTPUT_FIELDS: &[&str] = &[
    "command",
    "content",
    "from",
    "line_count",
    "numbered",
    "path",
    "to",
    "truncated",
];
const STAT_OUTPUT_FIELDS: &[&str] = &[
    "command",
    "created_unix_seconds",
    "kind",
    "modified_unix_seconds",
    "path",
    "readonly",
    "size_bytes",
];
const TREE_OUTPUT_FIELDS: &[&str] = &[
    "command",
    "entries",
    "entry_count",
    "max_depth",
    "path",
    "truncated",
];
const TREE_ENTRY_FIELDS: &[&str] = &["depth", "kind", "name", "path"];

fn path_arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn file_json(args: &[&str]) -> Value {
    let assert = Command::cargo_bin("ah")
        .expect("binary should compile")
        .args(["--json", "file"])
        .args(args)
        .assert()
        .success();

    serde_json::from_slice(&assert.get_output().stdout).expect("valid JSON output expected")
}

fn assert_object_fields(value: &Value, expected: &[&str]) {
    let object = value.as_object().expect("JSON value should be an object");
    let actual = object.keys().map(String::as_str).collect::<BTreeSet<_>>();
    let expected = expected.iter().copied().collect::<BTreeSet<_>>();
    assert_eq!(actual, expected, "unexpected JSON object fields");
}

fn assert_optional_timestamp(value: &Value) {
    assert!(
        value.is_null() || value.is_u64(),
        "timestamp should be an unsigned integer or null: {value}"
    );
}

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
fn file_read_accepts_utf8_character_split_at_sniff_boundary() {
    let temp = NamedTempFile::new().expect("temporary file should be created");
    let mut content = vec![b'a'; 8191];
    content.extend_from_slice("а\n".as_bytes());
    fs::write(temp.path(), content).expect("UTF-8 content should be written");

    let file_path = temp.path().to_string_lossy().to_string();
    Command::cargo_bin("ah")
        .expect("binary should compile")
        .args(["file", "read", &file_path])
        .assert()
        .success()
        .stdout(contains("а"));
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
fn file_line_commands_symlink_requires_follow_flag() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    let target_path = temp_dir.path().join("target.txt");
    let link_path = temp_dir.path().join("link.txt");
    fs::write(&target_path, "hello via symlink\n").expect("target file should be written");

    if !create_file_symlink(&link_path, &target_path) {
        return;
    }

    let link = path_arg(&link_path);
    for command in ["read", "head", "tail"] {
        Command::cargo_bin("ah")
            .expect("binary should compile")
            .args(["file", command, &link])
            .assert()
            .failure()
            .stderr(contains("SYMLINK_TRAVERSAL_BLOCKED: symlink blocked"))
            .stderr(contains("hint: use --follow-symlinks"));

        Command::cargo_bin("ah")
            .expect("binary should compile")
            .args(["file", command, &link, "--follow-symlinks"])
            .assert()
            .success()
            .stdout("hello via symlink\n");
    }
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
fn file_read_reports_empty_and_out_of_range_results() {
    let empty = NamedTempFile::new().expect("temporary file should be created");
    let empty_path = path_arg(empty.path());
    let empty_payload = file_json(&["read", &empty_path]);
    assert_eq!(empty_payload["from"], 1);
    assert_eq!(empty_payload["to"], Value::Null);
    assert_eq!(empty_payload["line_count"], 0);
    assert_eq!(empty_payload["content"], "");

    let populated = NamedTempFile::new().expect("temporary file should be created");
    fs::write(populated.path(), "alpha\nbeta\n").expect("temporary content should be written");
    let populated_path = path_arg(populated.path());
    let out_of_range = file_json(&["read", &populated_path, "--from", "5"]);
    assert_eq!(out_of_range["from"], 5);
    assert_eq!(out_of_range["to"], Value::Null);
    assert_eq!(out_of_range["line_count"], 0);
    assert_eq!(out_of_range["truncated"], false);
    assert_eq!(out_of_range["content"], "");
}

#[test]
fn file_read_json_reports_truncation() {
    let temp = NamedTempFile::new().expect("temporary file should be created");
    fs::write(temp.path(), "line-1\nline-2\nline-3\n")
        .expect("temporary content should be written");

    let file_path = path_arg(temp.path());
    let assert = Command::cargo_bin("ah")
        .expect("binary should compile")
        .args(["--json", "--limit", "2", "file", "read", &file_path])
        .assert()
        .success();
    let payload: Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("valid JSON output expected");

    assert_object_fields(&payload, LINE_OUTPUT_FIELDS);
    assert_eq!(payload["line_count"], 2);
    assert_eq!(payload["truncated"], true);
    assert_eq!(payload["content"], "line-1\nline-2");
    assert!(assert.get_output().stderr.is_empty());
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
        .stdout("   2: beta\n   3: gamma\n");
}

#[test]
fn file_read_emits_complete_json_contract() {
    let temp = NamedTempFile::new().expect("temporary file should be created");
    fs::write(temp.path(), "one\ntwo\n").expect("temporary content should be written");

    let file_path = temp.path().to_string_lossy().to_string();

    let payload = file_json(&["read", &file_path, "--from", "1", "--to", "1"]);

    assert_object_fields(&payload, LINE_OUTPUT_FIELDS);
    assert_eq!(payload["command"], "file.read");
    assert_eq!(payload["path"], file_path);
    assert_eq!(payload["from"], 1);
    assert_eq!(payload["to"], 1);
    assert_eq!(payload["numbered"], false);
    assert_eq!(payload["line_count"], 1);
    assert_eq!(payload["truncated"], false);
    assert_eq!(payload["content"], "one");
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
        .stdout("   1: alpha\n   2: beta\n");
}

#[test]
fn file_head_emits_complete_json_contract() {
    let temp = NamedTempFile::new().expect("temporary file should be created");
    fs::write(temp.path(), "alpha\nbeta\ngamma\n").expect("temporary content should be written");

    let file_path = path_arg(temp.path());
    let payload = file_json(&["head", &file_path, "--lines", "2", "-n"]);

    assert_object_fields(&payload, LINE_OUTPUT_FIELDS);
    assert_eq!(payload["command"], "file.head");
    assert_eq!(payload["path"], file_path);
    assert_eq!(payload["from"], 1);
    assert_eq!(payload["to"], 2);
    assert_eq!(payload["numbered"], true);
    assert_eq!(payload["line_count"], 2);
    assert_eq!(payload["truncated"], false);
    assert_eq!(payload["content"], "   1: alpha\n   2: beta");
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
fn file_head_uses_twenty_lines_by_default() {
    let temp = NamedTempFile::new().expect("temporary file should be created");
    let content = (1..=25)
        .map(|line| format!("line-{line}\n"))
        .collect::<String>();
    fs::write(temp.path(), content).expect("temporary content should be written");

    let file_path = path_arg(temp.path());
    let payload = file_json(&["head", &file_path]);
    assert_eq!(payload["from"], 1);
    assert_eq!(payload["to"], 20);
    assert_eq!(payload["line_count"], 20);
    assert!(
        payload["content"]
            .as_str()
            .expect("content should be text")
            .ends_with("line-20")
    );
}

#[test]
fn file_head_limit_takes_precedence_over_lines() {
    let temp = NamedTempFile::new().expect("temporary file should be created");
    fs::write(temp.path(), "alpha\nbeta\ngamma\n").expect("temporary content should be written");

    let file_path = path_arg(temp.path());
    let assert = Command::cargo_bin("ah")
        .expect("binary should compile")
        .args([
            "--json", "--limit", "1", "file", "head", &file_path, "--lines", "3",
        ])
        .assert()
        .success();
    let payload: Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("valid JSON output expected");
    assert_eq!(payload["from"], 1);
    assert_eq!(payload["to"], 1);
    assert_eq!(payload["line_count"], 1);
    assert_eq!(payload["truncated"], true);
    assert_eq!(payload["content"], "alpha");
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
        .stdout("   3: gamma\n   4: delta\n");
}

#[test]
fn file_tail_emits_complete_json_contract() {
    let temp = NamedTempFile::new().expect("temporary file should be created");
    fs::write(temp.path(), "alpha\nbeta\ngamma\ndelta\n")
        .expect("temporary content should be written");

    let file_path = path_arg(temp.path());
    let payload = file_json(&["tail", &file_path, "--lines", "2", "-n"]);

    assert_object_fields(&payload, LINE_OUTPUT_FIELDS);
    assert_eq!(payload["command"], "file.tail");
    assert_eq!(payload["path"], file_path);
    assert_eq!(payload["from"], 3);
    assert_eq!(payload["to"], 4);
    assert_eq!(payload["numbered"], true);
    assert_eq!(payload["line_count"], 2);
    assert_eq!(payload["truncated"], false);
    assert_eq!(payload["content"], "   3: gamma\n   4: delta");
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
fn file_tail_uses_twenty_lines_by_default() {
    let temp = NamedTempFile::new().expect("temporary file should be created");
    let content = (1..=25)
        .map(|line| format!("line-{line}\n"))
        .collect::<String>();
    fs::write(temp.path(), content).expect("temporary content should be written");

    let file_path = path_arg(temp.path());
    let payload = file_json(&["tail", &file_path]);
    assert_eq!(payload["from"], 6);
    assert_eq!(payload["to"], 25);
    assert_eq!(payload["line_count"], 20);
    assert!(
        payload["content"]
            .as_str()
            .expect("content should be text")
            .starts_with("line-6")
    );
}

#[test]
fn file_tail_limit_keeps_first_lines_of_tail_selection() {
    let temp = NamedTempFile::new().expect("temporary file should be created");
    fs::write(temp.path(), "alpha\nbeta\ngamma\ndelta\n")
        .expect("temporary content should be written");

    let file_path = path_arg(temp.path());
    let assert = Command::cargo_bin("ah")
        .expect("binary should compile")
        .args([
            "--json", "--limit", "2", "file", "tail", &file_path, "--lines", "3",
        ])
        .assert()
        .success();
    let payload: Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("valid JSON output expected");
    assert_eq!(payload["from"], 2);
    assert_eq!(payload["to"], 3);
    assert_eq!(payload["line_count"], 2);
    assert_eq!(payload["truncated"], true);
    assert_eq!(payload["content"], "beta\ngamma");
}

#[test]
fn file_line_commands_reject_missing_and_directory_paths() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    let missing = path_arg(&temp_dir.path().join("missing.txt"));
    let directory = path_arg(temp_dir.path());

    for command in ["read", "head", "tail"] {
        Command::cargo_bin("ah")
            .expect("binary should compile")
            .args(["file", command, &missing])
            .assert()
            .failure()
            .stderr(contains("failed to read file metadata"));

        Command::cargo_bin("ah")
            .expect("binary should compile")
            .args(["file", command, &directory])
            .assert()
            .failure()
            .stderr(contains("path is not a file"));
    }
}

#[test]
fn file_head_and_tail_apply_binary_and_size_policies() {
    let binary = NamedTempFile::new().expect("temporary file should be created");
    fs::write(binary.path(), [0u8, b'a', b'b', b'c']).expect("binary content should be written");
    let binary_path = path_arg(binary.path());

    let large = NamedTempFile::new().expect("temporary file should be created");
    fs::write(large.path(), "0123456789\n").expect("text content should be written");
    let large_path = path_arg(large.path());

    for command in ["head", "tail"] {
        Command::cargo_bin("ah")
            .expect("binary should compile")
            .args(["file", command, &binary_path])
            .assert()
            .failure()
            .stderr(contains("binary or non-UTF8 file is not supported"));

        Command::cargo_bin("ah")
            .expect("binary should compile")
            .args(["file", command, &large_path, "--max-bytes", "4"])
            .assert()
            .failure()
            .stderr(contains("file is too large"));
    }
}

#[test]
fn file_stat_emits_complete_json_contract() {
    let temp = NamedTempFile::new().expect("temporary file should be created");
    fs::write(temp.path(), "abc\n").expect("temporary content should be written");

    let file_path = path_arg(temp.path());
    let payload = file_json(&["stat", &file_path]);

    assert_object_fields(&payload, STAT_OUTPUT_FIELDS);
    assert_eq!(payload["command"], "file.stat");
    assert_eq!(payload["path"], file_path);
    assert_eq!(payload["kind"], "file");
    assert_eq!(payload["size_bytes"], 4);
    assert_eq!(payload["readonly"], false);
    assert_optional_timestamp(&payload["modified_unix_seconds"]);
    assert_optional_timestamp(&payload["created_unix_seconds"]);
}

#[test]
fn file_stat_emits_text_contract() {
    let temp = NamedTempFile::new().expect("temporary file should be created");
    fs::write(temp.path(), "abc\n").expect("temporary content should be written");
    let file_path = path_arg(temp.path());

    let assert = Command::cargo_bin("ah")
        .expect("binary should compile")
        .args(["file", "stat", &file_path])
        .assert()
        .success()
        .stdout(contains("\u{1b}").not());
    let stdout =
        String::from_utf8(assert.get_output().stdout.clone()).expect("stdout should be UTF-8");
    let lines = stdout.lines().collect::<Vec<_>>();

    assert_eq!(lines.len(), 6);
    assert_eq!(lines[0], format!("path: {file_path}"));
    assert_eq!(lines[1], "kind: file");
    assert_eq!(lines[2], "size_bytes: 4");
    assert_eq!(lines[3], "readonly: false");
    for (line, prefix) in [
        (lines[4], "modified_unix_seconds: "),
        (lines[5], "created_unix_seconds: "),
    ] {
        let value = line
            .strip_prefix(prefix)
            .expect("timestamp field should keep its label");
        assert!(value == "null" || value.parse::<u64>().is_ok());
    }
}

#[test]
fn file_stat_reports_directory_kind_and_missing_path() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    let directory = path_arg(temp_dir.path());
    let payload = file_json(&["stat", &directory]);
    assert_eq!(payload["kind"], "directory");

    let missing = path_arg(&temp_dir.path().join("missing"));
    Command::cargo_bin("ah")
        .expect("binary should compile")
        .args(["file", "stat", &missing])
        .assert()
        .failure()
        .stderr(contains("failed to read file metadata"));
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
fn file_tree_emits_complete_json_contract_with_depth() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    let root = temp_dir.path().join("root");
    let nested = root.join("nested");

    fs::create_dir(&root).expect("root directory should be created");
    fs::create_dir(&nested).expect("nested directory should be created");
    fs::write(root.join("top.txt"), "top").expect("top file should be created");
    fs::write(nested.join("deep.txt"), "deep").expect("deep file should be created");

    let root_path = path_arg(&root);
    let payload = file_json(&["tree", &root_path, "--depth", "1"]);

    assert_object_fields(&payload, TREE_OUTPUT_FIELDS);
    assert_eq!(payload["command"], "file.tree");
    assert_eq!(payload["path"], root_path);
    assert_eq!(payload["max_depth"], 1);
    assert_eq!(payload["entry_count"], 3);
    assert_eq!(payload["truncated"], false);

    let entries = payload["entries"]
        .as_array()
        .expect("entries should be an array");
    assert_eq!(entries.len(), 3);
    for entry in entries {
        assert_object_fields(entry, TREE_ENTRY_FIELDS);
    }
    assert_eq!(entries[0]["depth"], 0);
    assert_eq!(entries[0]["kind"], "directory");
    assert_eq!(entries[0]["name"], "root");
    assert_eq!(entries[0]["path"], root_path);
    assert_eq!(entries[1]["depth"], 1);
    assert_eq!(entries[1]["kind"], "directory");
    assert_eq!(entries[1]["name"], "nested");
    assert_eq!(entries[2]["depth"], 1);
    assert_eq!(entries[2]["kind"], "file");
    assert_eq!(entries[2]["name"], "top.txt");
}

#[test]
fn file_tree_text_output_is_deterministic() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    let root = temp_dir.path().join("root");
    let nested = root.join("nested");
    fs::create_dir(&root).expect("root directory should be created");
    fs::create_dir(&nested).expect("nested directory should be created");
    fs::write(root.join("Beta.txt"), "beta").expect("beta file should be created");
    fs::write(root.join("alpha.txt"), "alpha").expect("alpha file should be created");
    fs::write(nested.join("deep.txt"), "deep").expect("deep file should be created");

    let root_path = path_arg(&root);
    Command::cargo_bin("ah")
        .expect("binary should compile")
        .args(["file", "tree", &root_path])
        .assert()
        .success()
        .stdout(contains("\u{1b}").not())
        .stdout("root/\n  - alpha.txt\n  - Beta.txt\n  - nested/\n    - deep.txt\n");
}

#[test]
fn file_tree_handles_depth_zero_and_single_file_roots() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    let root = temp_dir.path().join("root");
    fs::create_dir(&root).expect("root directory should be created");
    fs::write(root.join("child.txt"), "child").expect("child file should be created");
    let root_path = path_arg(&root);

    let depth_zero = file_json(&["tree", &root_path, "--depth", "0"]);
    assert_eq!(depth_zero["entry_count"], 1);
    assert_eq!(depth_zero["entries"][0]["name"], "root");

    let file_path = path_arg(&root.join("child.txt"));
    Command::cargo_bin("ah")
        .expect("binary should compile")
        .args(["file", "tree", &file_path])
        .assert()
        .success()
        .stdout("child.txt\n");
}

#[test]
fn file_tree_defaults_to_global_cwd() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    let cwd = path_arg(temp_dir.path());
    let assert = Command::cargo_bin("ah")
        .expect("binary should compile")
        .args(["--cwd", &cwd, "--json", "file", "tree", "--depth", "0"])
        .assert()
        .success();
    let payload: Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("valid JSON output expected");

    assert_eq!(payload["path"], ".");
    assert_eq!(payload["entry_count"], 1);
    assert_eq!(payload["entries"][0]["kind"], "directory");
    assert_eq!(payload["entries"][0]["name"], ".");
    assert_eq!(payload["entries"][0]["path"], ".");
}

#[test]
fn file_tree_limit_reports_truncation_in_text_and_json() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    let root = temp_dir.path().join("root");
    fs::create_dir(&root).expect("root directory should be created");
    fs::write(root.join("alpha.txt"), "alpha").expect("alpha file should be created");
    fs::write(root.join("beta.txt"), "beta").expect("beta file should be created");
    let root_path = path_arg(&root);

    Command::cargo_bin("ah")
        .expect("binary should compile")
        .args(["--limit", "2", "file", "tree", &root_path])
        .assert()
        .success()
        .stdout("root/\n  - alpha.txt\n")
        .stderr("warning: output truncated by --limit\n");

    let assert = Command::cargo_bin("ah")
        .expect("binary should compile")
        .args(["--json", "--limit", "2", "file", "tree", &root_path])
        .assert()
        .success();
    let payload: Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("valid JSON output expected");
    assert_eq!(payload["entry_count"], 2);
    assert_eq!(payload["truncated"], true);
    assert_eq!(payload["entries"][1]["name"], "alpha.txt");
    assert!(assert.get_output().stderr.is_empty());
}

#[test]
fn file_tree_rejects_missing_path() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    let missing = path_arg(&temp_dir.path().join("missing"));
    Command::cargo_bin("ah")
        .expect("binary should compile")
        .args(["file", "tree", &missing])
        .assert()
        .failure()
        .stderr(contains("failed to read file metadata"));
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

#[test]
fn file_tree_follow_symlinks_stops_at_directory_cycles() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    let root = temp_dir.path().join("root");
    let child = root.join("child");
    let back_link = child.join("back-to-root");
    fs::create_dir(&root).expect("root directory should be created");
    fs::create_dir(&child).expect("child directory should be created");

    if !create_dir_symlink(&back_link, &root) {
        return;
    }

    let root_path = path_arg(&root);
    let payload = file_json(&["tree", &root_path, "--follow-symlinks"]);
    let entries = payload["entries"]
        .as_array()
        .expect("entries should be an array");

    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0]["name"], "root");
    assert_eq!(entries[1]["name"], "child");
    assert_eq!(entries[2]["name"], "back-to-root");
    assert_eq!(entries[2]["kind"], "symlink");
}
