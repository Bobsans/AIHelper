use std::{fs, process::Command as ProcessCommand};

use assert_cmd::Command;
use predicates::{prelude::PredicateBooleanExt, str::contains};

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

    let mut text_cmd = Command::cargo_bin("ah").expect("binary should compile");
    text_cmd
        .args(["--cwd", &cwd_str, "git", "changed"])
        .assert()
        .success()
        .stdout(contains("app.txt"))
        .stdout(contains("\u{1b}").not());
}

#[test]
fn git_and_ctx_changed_preserve_unusual_paths_and_renames() {
    if !git_available() {
        return;
    }

    let temp_dir = init_git_repo_with_one_commit();
    let cwd = temp_dir.path();
    fs::rename(cwd.join("app.txt"), cwd.join("renamed app.txt"))
        .expect("tracked file should be renamed");
    #[cfg(not(windows))]
    fs::write(cwd.join("literal -> arrow.txt"), "new\n").expect("untracked file should be written");
    let status = ProcessCommand::new("git")
        .current_dir(cwd)
        .args(["add", "-A"])
        .status()
        .expect("git add should run");
    assert!(status.success());

    let cwd_str = cwd.to_string_lossy().to_string();
    let git_output = Command::cargo_bin("ah")
        .expect("binary should compile")
        .args(["--json", "--cwd", &cwd_str, "git", "changed"])
        .output()
        .expect("git changed should run");
    assert!(git_output.status.success(), "{git_output:?}");
    let git_payload: serde_json::Value =
        serde_json::from_slice(&git_output.stdout).expect("git output should be JSON");

    let rename = git_payload["entries"]
        .as_array()
        .expect("entries array")
        .iter()
        .find(|entry| entry["status"] == "R")
        .expect("rename entry");
    assert_eq!(rename["path"], "renamed app.txt");
    assert_eq!(rename["old_path"], "app.txt");

    #[cfg(not(windows))]
    {
        let literal_arrow = git_payload["entries"]
            .as_array()
            .expect("entries array")
            .iter()
            .find(|entry| entry["path"] == "literal -> arrow.txt")
            .expect("literal arrow entry");
        assert!(literal_arrow["old_path"].is_null());
    }

    let ctx_output = Command::cargo_bin("ah")
        .expect("binary should compile")
        .args(["--json", "--cwd", &cwd_str, "ctx", "changed"])
        .output()
        .expect("ctx changed should run");
    assert!(ctx_output.status.success(), "{ctx_output:?}");
    let ctx_payload: serde_json::Value =
        serde_json::from_slice(&ctx_output.stdout).expect("ctx output should be JSON");
    assert_eq!(git_payload["entries"], ctx_payload["entries"]);
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
        .stdout(contains("app.txt"))
        .stdout(contains("\u{1b}").not());
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
fn git_blame_full_file_keeps_line_text() {
    if !git_available() {
        return;
    }

    let temp_dir = init_git_repo_with_one_commit();
    let cwd = temp_dir.path();
    let cwd_str = cwd.to_string_lossy().to_string();

    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["--json", "--cwd", &cwd_str, "git", "blame", "app.txt"])
        .assert()
        .success()
        .stdout(contains("\"entry_count\": 2"))
        .stdout(contains("\"text\": \"line one\""))
        .stdout(contains("\"text\": \"line two\""));
}

#[test]
fn git_commit_info_quiet_still_validates_reference() {
    if !git_available() {
        return;
    }

    let temp_dir = init_git_repo_with_one_commit();
    let cwd = temp_dir.path();
    let cwd_str = cwd.to_string_lossy().to_string();

    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args([
        "--quiet",
        "--cwd",
        &cwd_str,
        "git",
        "commit-info",
        "definitely-missing-ref",
    ])
    .assert()
    .failure();
}

#[test]
fn git_commit_info_reports_metadata_and_files() {
    if !git_available() {
        return;
    }

    let temp_dir = init_git_repo_with_one_commit();
    let cwd = temp_dir.path();
    let cwd_str = cwd.to_string_lossy().to_string();

    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["--json", "--cwd", &cwd_str, "git", "commit-info"])
        .assert()
        .success()
        .stdout(contains("\"command\": \"git.commit-info\""))
        .stdout(contains("\"reference\": \"HEAD\""))
        .stdout(contains("\"subject\": \"initial\""))
        .stdout(contains("\"file_count\": 1"))
        .stdout(contains("\"path\": \"app.txt\""));
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

    let mut text_cmd = Command::cargo_bin("ah").expect("binary should compile");
    text_cmd
        .args(["--cwd", &cwd_str, "git", "status"])
        .assert()
        .success()
        .stdout(contains("branch=main"))
        .stdout(contains("clean=false"))
        .stdout(contains("unstaged=1"))
        .stdout(contains("untracked=1"))
        .stdout(contains("\u{1b}").not());
}

#[test]
fn git_tag_create_creates_annotated_tag() {
    if !git_available() {
        return;
    }

    let temp_dir = init_git_repo_with_one_commit();
    let cwd = temp_dir.path();
    let cwd_str = cwd.to_string_lossy().to_string();

    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args([
        "--json",
        "--cwd",
        &cwd_str,
        "git",
        "tag",
        "create",
        "v0.2.0",
        "--message",
        "v0.2.0",
        "--ref",
        "HEAD",
    ])
    .assert()
    .success()
    .stdout(contains("\"command\": \"git.tag.create\""))
    .stdout(contains("\"tag\": \"v0.2.0\""))
    .stdout(contains("\"annotated\": true"));

    let tags = ProcessCommand::new("git")
        .current_dir(cwd)
        .args(["tag", "--list", "v0.2.0"])
        .output()
        .expect("git should start");
    assert!(tags.status.success());
    assert_eq!(String::from_utf8_lossy(&tags.stdout).trim(), "v0.2.0");
}

#[test]
fn git_tags_reports_latest_tag() {
    if !git_available() {
        return;
    }

    let temp_dir = init_git_repo_with_one_commit();
    let cwd = temp_dir.path();
    run_git(cwd, &["tag", "v0.1.0"]);
    run_git(cwd, &["tag", "v0.2.0"]);

    let cwd_str = cwd.to_string_lossy().to_string();
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["--json", "--cwd", &cwd_str, "git", "tags", "--latest"])
        .assert()
        .success()
        .stdout(contains("\"command\": \"git.tags\""))
        .stdout(contains("\"latest\": true"))
        .stdout(contains("\"tag_count\": 1"))
        .stdout(contains("\"truncated\": false"))
        .stderr("");
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
