use std::{fs, process::Command as ProcessCommand};

use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use tempfile::{NamedTempFile, TempDir};

fn git_available() -> bool {
    ProcessCommand::new("git")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
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

fn try_git(cwd: &std::path::Path, args: &[&str]) -> bool {
    ProcessCommand::new("git")
        .current_dir(cwd)
        .args(args)
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn init_git_repo_with_one_commit() -> TempDir {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    let cwd = temp_dir.path();

    if !try_git(cwd, &["init", "-b", "main"]) {
        run_git(cwd, &["init"]);
    }
    run_git(cwd, &["config", "user.email", "test@example.com"]);
    run_git(cwd, &["config", "user.name", "Test User"]);

    fs::write(cwd.join("app.txt"), "line one\nline two\n").expect("file should be written");
    run_git(cwd, &["add", "app.txt"]);
    run_git(cwd, &["commit", "-m", "initial"]);

    temp_dir
}

#[test]
fn shows_top_level_help() {
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.arg("--help")
        .assert()
        .success()
        .stdout(contains("AIHelper CLI toolbox"));
}

#[test]
fn shows_file_subcommand_help() {
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["file", "--help"])
        .assert()
        .success()
        .stdout(contains("read"));
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

fn task_echo_command() -> &'static str {
    if cfg!(target_os = "windows") {
        "Write-Output task-ok"
    } else {
        "echo task-ok"
    }
}

#[test]
fn task_save_and_list_json() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    let cwd = temp_dir.path().to_string_lossy().to_string();

    let mut save_cmd = Command::cargo_bin("ah").expect("binary should compile");
    save_cmd
        .args(["--cwd", &cwd, "task", "save", "hello", task_echo_command()])
        .assert()
        .success()
        .stdout(contains("saved task 'hello'"));

    let mut list_cmd = Command::cargo_bin("ah").expect("binary should compile");
    list_cmd
        .args(["--json", "--cwd", &cwd, "task", "list"])
        .assert()
        .success()
        .stdout(contains("\"command\": \"task.list\""))
        .stdout(contains("\"name\": \"hello\""));
}

#[test]
fn task_run_executes_saved_command() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    let cwd = temp_dir.path().to_string_lossy().to_string();

    let mut save_cmd = Command::cargo_bin("ah").expect("binary should compile");
    save_cmd
        .args(["--cwd", &cwd, "task", "save", "echo", task_echo_command()])
        .assert()
        .success();

    let mut run_cmd = Command::cargo_bin("ah").expect("binary should compile");
    run_cmd
        .args(["--cwd", &cwd, "task", "run", "echo"])
        .assert()
        .success()
        .stdout(contains("task-ok"));
}

#[test]
fn task_run_unknown_task_fails() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    let cwd = temp_dir.path().to_string_lossy().to_string();

    let mut run_cmd = Command::cargo_bin("ah").expect("binary should compile");
    run_cmd
        .args(["--cwd", &cwd, "task", "run", "missing"])
        .assert()
        .failure()
        .stderr(contains("task not found: missing"));
}
