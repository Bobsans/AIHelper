use std::{fs, process::Command as ProcessCommand};

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

#[test]
fn git_status_reports_compact_counts() {
    if !git_available() {
        return;
    }

    let temp_dir = init_git_repo_with_one_commit();
    let cwd = temp_dir.path();
    fs::write(cwd.join("app.txt"), "line one\nline two changed\n").expect("file should be written");
    fs::write(cwd.join("new.txt"), "new\n").expect("file should be written");

    let cwd_str = cwd.to_string_lossy().to_string();
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["--json", "--cwd", &cwd_str, "git", "status"])
        .assert()
        .success()
        .stdout(contains("\"command\": \"git.status\""))
        .stdout(contains("\"in_git_repo\": true"))
        .stdout(contains("\"clean\": false"))
        .stdout(contains("\"staged_count\": 0"))
        .stdout(contains("\"unstaged_count\": 1"))
        .stdout(contains("\"untracked_count\": 1"))
        .stdout(contains("\"changed_count\": 2"))
        .stdout(contains("\"subject\": \"initial\""));
}

#[test]
fn git_tags_reports_latest_tag() {
    if !git_available() {
        return;
    }

    let temp_dir = init_git_repo_with_one_commit();
    let cwd = temp_dir.path();
    run_git(cwd, &["tag", "v0.1.0"]);

    let cwd_str = cwd.to_string_lossy().to_string();
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["--json", "--cwd", &cwd_str, "git", "tags", "--latest"])
        .assert()
        .success()
        .stdout(contains("\"command\": \"git.tags\""))
        .stdout(contains("\"latest\": true"))
        .stdout(contains("\"tag_count\": 1"))
        .stdout(contains("\"name\": \"v0.1.0\""));
}

#[test]
fn git_remotes_reports_provider_hint() {
    if !git_available() {
        return;
    }

    let temp_dir = init_git_repo_with_one_commit();
    let cwd = temp_dir.path();
    run_git(
        cwd,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/example/repo.git",
        ],
    );

    let cwd_str = cwd.to_string_lossy().to_string();
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["--json", "--cwd", &cwd_str, "git", "remotes"])
        .assert()
        .success()
        .stdout(contains("\"command\": \"git.remotes\""))
        .stdout(contains("\"remote_count\": 1"))
        .stdout(contains("\"name\": \"origin\""))
        .stdout(contains("\"provider\": \"github\""));
}

fn run_git(cwd: &std::path::Path, args: &[&str]) {
    let status = ProcessCommand::new("git")
        .current_dir(cwd)
        .args(args)
        .status()
        .expect("git should start");
    assert!(
        status.success(),
        "git command failed in {}: git {}",
        cwd.display(),
        args.join(" ")
    );
}
