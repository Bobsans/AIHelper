//! `--cwd` is honoured by every command that resolves a relative path.
//!
//! Until now the flag worked by calling `std::env::set_current_dir` at startup,
//! so each command that read the ambient directory got the right answer by
//! accident of process state rather than by being told. That makes the answer
//! process-global, and `mcp serve` runs commands in parallel.
//!
//! These tests pin the *behaviour* so the mechanism can be replaced: every
//! ambient-directory reader in the tree is exercised through `--cwd` here.

use std::fs;

use super::common::{IsolatedAhCommand as Command, git_available, init_git_repo_with_one_commit};
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use tempfile::TempDir;

/// A directory holding one file, plus a distinctive name to search for.
fn workspace() -> TempDir {
    let temp = TempDir::new().expect("temporary dir should be created");
    fs::write(temp.path().join("marker.txt"), "needle-in-the-workspace\n")
        .expect("test file should be written");
    temp
}

/// `search text` with no path searches the request directory, not the shell's.
#[test]
fn search_text_without_a_path_searches_the_requested_directory() {
    let temp = workspace();
    let cwd = temp.path().to_string_lossy().to_string();

    Command::cargo_bin("ah")
        .expect("binary should compile")
        .args(["--json", "--cwd", &cwd, "search", "text", "needle-in-the"])
        .assert()
        .success()
        .stdout(contains("marker.txt"));
}

/// `search files` likewise.
#[test]
fn search_files_without_a_path_searches_the_requested_directory() {
    let temp = workspace();
    let cwd = temp.path().to_string_lossy().to_string();

    Command::cargo_bin("ah")
        .expect("binary should compile")
        .args(["--json", "--cwd", &cwd, "search", "files", "marker"])
        .assert()
        .success()
        .stdout(contains("marker.txt"));
}

/// A relative path argument resolves against the request directory.
#[test]
fn a_relative_path_resolves_against_the_requested_directory() {
    let temp = workspace();
    let cwd = temp.path().to_string_lossy().to_string();

    Command::cargo_bin("ah")
        .expect("binary should compile")
        .args([
            "--json",
            "--cwd",
            &cwd,
            "search",
            "text",
            "needle-in-the",
            "marker.txt",
        ])
        .assert()
        .success()
        .stdout(contains("marker.txt"));
}

/// `ctx changed` asks git about the request directory.
#[test]
fn ctx_changed_reads_the_requested_repository() {
    if !git_available() {
        return;
    }
    let temp = init_git_repo_with_one_commit();
    fs::write(
        temp.path().join("marker.txt"),
        "needle-in-the-workspace
",
    )
    .expect("test file should be written");
    let cwd = temp.path().to_string_lossy().to_string();

    Command::cargo_bin("ah")
        .expect("binary should compile")
        .args(["--json", "--cwd", &cwd, "ctx", "changed"])
        .assert()
        .success()
        .stdout(contains("marker.txt"));
}

/// `ai status` classifies the request directory, not the shell's.
///
/// A project scope appears only when the directory it was pointed at looks like
/// a project; anywhere else the same target reports user scope only.
#[test]
fn ai_status_classifies_the_requested_directory() {
    let temp = workspace();
    fs::create_dir(temp.path().join(".git")).expect("marker directory should be created");
    let cwd = temp.path().to_string_lossy().to_string();

    Command::cargo_bin("ah")
        .expect("binary should compile")
        .args(["--json", "--cwd", &cwd, "ai", "status", "claude"])
        .assert()
        .success()
        .stdout(contains("\"scope\": \"project\""));

    let bare = TempDir::new().expect("temporary dir should be created");
    let bare_cwd = bare.path().to_string_lossy().to_string();

    Command::cargo_bin("ah")
        .expect("binary should compile")
        .args(["--json", "--cwd", &bare_cwd, "ai", "status", "claude"])
        .assert()
        .success()
        .stdout(contains("\"scope\": \"project\"").not());
}

/// `file tree` with no path lists the request directory.
#[test]
fn file_tree_without_a_path_lists_the_requested_directory() {
    let temp = workspace();
    let cwd = temp.path().to_string_lossy().to_string();

    Command::cargo_bin("ah")
        .expect("binary should compile")
        .args(["--json", "--cwd", &cwd, "file", "tree", "--depth", "1"])
        .assert()
        .success()
        .stdout(contains("marker.txt"));
}

/// `git status` inspects the request directory's repository.
#[test]
fn git_status_reads_the_requested_repository() {
    if !git_available() {
        return;
    }
    let temp = init_git_repo_with_one_commit();
    fs::write(
        temp.path().join("marker.txt"),
        "needle-in-the-workspace
",
    )
    .expect("test file should be written");
    let cwd = temp.path().to_string_lossy().to_string();

    Command::cargo_bin("ah")
        .expect("binary should compile")
        .args(["--json", "--cwd", &cwd, "git", "status"])
        .assert()
        .success()
        // A wrong directory would report `in_git_repo: false`, or this
        // repository's own state rather than one untracked file.
        .stdout(contains("\"in_git_repo\": true"))
        .stdout(contains("\"untracked_count\": 1"));
}

/// `project detect` with no positional path uses the request directory.
#[test]
fn project_detect_without_a_path_inspects_the_requested_directory() {
    let temp = workspace();
    fs::write(
        temp.path().join("Cargo.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\n",
    )
    .expect("manifest should be written");
    let cwd = temp.path().to_string_lossy().to_string();

    Command::cargo_bin("ah")
        .expect("binary should compile")
        .args(["--json", "--cwd", &cwd, "project", "detect"])
        .assert()
        .success()
        .stdout(contains("rust"));
}
