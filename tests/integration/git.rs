use std::fs;

use assert_cmd::Command;
use predicates::str::contains;

use crate::common::{git_available, init_git_repo_with_one_commit};

#[test]
fn git_changed_reports_modified_file() {
    if !git_available() {
        return;
    }

    let temp_dir = init_git_repo_with_one_commit();
    let cwd = temp_dir.path();
    fs::write(cwd.join("app.txt"), "line one\nline two changed\n").expect("file should be written");

    let cwd_str = cwd.to_string_lossy().to_string();
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["--json", "--cwd", &cwd_str, "git", "changed"])
        .assert()
        .success()
        .stdout(contains("\"command\": \"git.changed\""))
        .stdout(contains("\"in_git_repo\": true"))
        .stdout(contains("\"path\": \"app.txt\""));
}

#[test]
fn git_diff_with_path_returns_patch() {
    if !git_available() {
        return;
    }

    let temp_dir = init_git_repo_with_one_commit();
    let cwd = temp_dir.path();
    fs::write(cwd.join("app.txt"), "line one updated\nline two\n").expect("file should be written");

    let cwd_str = cwd.to_string_lossy().to_string();
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["--cwd", &cwd_str, "git", "diff", "--path", "app.txt"])
        .assert()
        .success()
        .stdout(contains("diff --git"))
        .stdout(contains("app.txt"));
}

#[test]
fn git_blame_line_returns_json_entry() {
    if !git_available() {
        return;
    }

    let temp_dir = init_git_repo_with_one_commit();
    let cwd = temp_dir.path();
    let cwd_str = cwd.to_string_lossy().to_string();

    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args([
        "--json", "--cwd", &cwd_str, "git", "blame", "app.txt", "--line", "1",
    ])
    .assert()
    .success()
    .stdout(contains("\"command\": \"git.blame\""))
    .stdout(contains("\"line_filter\": 1"))
    .stdout(contains("\"entry_count\": 1"))
    .stdout(contains("\"author\": \"Test User\""));
}
