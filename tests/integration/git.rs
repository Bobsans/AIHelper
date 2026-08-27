//! `ah git`, run in this process against a real repository.
//!
//! The repository and the `git` binary are real; what is no longer real is the
//! subprocess of `ah` itself. The JSON assertions here index into the payload
//! rather than looking for `"\"path\": \"app.txt\""` inside pretty-printed
//! text, which is what the process-level versions had to do.
//!
//! One assertion cannot move: text output carrying no terminal escape
//! sequences is a property of the *process* emitter deciding it is not writing
//! to a terminal, and the harness always captures without colour. That check
//! stays at the bottom of this file, once, rather than repeated per command.

use std::{fs, path::Path, process::Command as ProcessCommand};

use aihelper::harness::Harness;
use predicates::{prelude::PredicateBooleanExt, str::contains};
use serde_json::Value;

use super::common::IsolatedAhCommand as Command;
use crate::common::{git_available, init_git_repo_with_one_commit};

fn git_json(cwd: &Path, args: &[&str]) -> Value {
    let mut argv = vec!["--json", "git"];
    argv.extend_from_slice(args);
    Harness::new().in_directory(cwd).run(&argv).json()
}

fn git_text(cwd: &Path, args: &[&str]) -> String {
    let mut argv = vec!["git"];
    argv.extend_from_slice(args);
    Harness::new()
        .in_directory(cwd)
        .run(&argv)
        .expect_success()
        .to_owned()
}

fn entries(payload: &Value) -> &Vec<Value> {
    payload["entries"]
        .as_array()
        .expect("the payload should carry an entries array")
}

#[test]
fn git_changed_reports_modified_file() {
    if !git_available() {
        return;
    }

    let temp_dir = init_git_repo_with_one_commit();
    let cwd = temp_dir.path();
    fs::write(cwd.join("app.txt"), "line one\nline two changed\n").expect("file should be written");

    let payload = git_json(cwd, &["changed"]);
    assert_eq!(payload["command"], "git.changed");
    assert_eq!(payload["in_git_repo"], true);
    assert!(
        entries(&payload)
            .iter()
            .any(|entry| entry["path"] == "app.txt"),
        "{payload}"
    );

    assert!(git_text(cwd, &["changed"]).contains("app.txt"));
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
    run_git(cwd, &["add", "-A"]);

    let git_payload = git_json(cwd, &["changed"]);
    let rename = entries(&git_payload)
        .iter()
        .find(|entry| entry["status"] == "R")
        .expect("rename entry");
    assert_eq!(rename["path"], "renamed app.txt");
    assert_eq!(rename["old_path"], "app.txt");

    #[cfg(not(windows))]
    {
        let literal_arrow = entries(&git_payload)
            .iter()
            .find(|entry| entry["path"] == "literal -> arrow.txt")
            .expect("literal arrow entry");
        assert!(literal_arrow["old_path"].is_null());
    }

    // `ctx changed` reports the same set through a different domain.
    let ctx_payload = Harness::new()
        .in_directory(cwd)
        .run(&["--json", "ctx", "changed"])
        .json();
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

    let patch = git_text(cwd, &["diff", "--path", "app.txt"]);
    assert!(patch.contains("diff --git"), "{patch}");
    assert!(patch.contains("app.txt"), "{patch}");
}

#[test]
fn git_blame_line_returns_json_entry() {
    if !git_available() {
        return;
    }

    let temp_dir = init_git_repo_with_one_commit();
    let cwd = temp_dir.path();

    let payload = git_json(cwd, &["blame", "app.txt", "--line", "1"]);
    assert_eq!(payload["command"], "git.blame");
    assert_eq!(payload["line_filter"], 1);
    assert_eq!(payload["entry_count"], 1);
    assert_eq!(entries(&payload)[0]["author"], "Test User");
}

#[test]
fn git_blame_full_file_keeps_line_text() {
    if !git_available() {
        return;
    }

    let temp_dir = init_git_repo_with_one_commit();
    let cwd = temp_dir.path();

    let payload = git_json(cwd, &["blame", "app.txt"]);
    assert_eq!(payload["entry_count"], 2);
    let text = entries(&payload)
        .iter()
        .map(|entry| entry["text"].as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert_eq!(text, ["line one", "line two"]);
}

#[test]
fn git_commit_info_quiet_still_validates_reference() {
    if !git_available() {
        return;
    }

    let temp_dir = init_git_repo_with_one_commit();
    let cwd = temp_dir.path();

    let run = Harness::new().in_directory(cwd).run(&[
        "--quiet",
        "git",
        "commit-info",
        "definitely-missing-ref",
    ]);

    let _ = run.expect_failure();
    assert!(
        run.stdout().is_empty(),
        "--quiet must silence the output, not the validation"
    );
}

#[test]
fn git_commit_info_reports_metadata_and_files() {
    if !git_available() {
        return;
    }

    let temp_dir = init_git_repo_with_one_commit();
    let cwd = temp_dir.path();

    let payload = git_json(cwd, &["commit-info"]);
    assert_eq!(payload["command"], "git.commit-info");
    assert_eq!(payload["reference"], "HEAD");
    // Under `commit`, which the `contains` assertions this replaces could not
    // say: they matched the field anywhere in the pretty-printed payload.
    let commit = &payload["commit"];
    assert_eq!(commit["subject"], "initial");
    assert_eq!(commit["file_count"], 1);
    assert_eq!(
        commit["files"]
            .as_array()
            .expect("files array")
            .first()
            .map(|file| &file["path"]),
        Some(&Value::from("app.txt"))
    );
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

    let payload = git_json(cwd, &["status"]);
    assert_eq!(payload["command"], "git.status");
    assert_eq!(payload["in_git_repo"], true);
    assert_eq!(payload["clean"], false);
    assert_eq!(payload["staged_count"], 0);
    assert_eq!(payload["unstaged_count"], 1);
    assert_eq!(payload["untracked_count"], 1);
    assert_eq!(payload["changed_count"], 2);
    assert_eq!(payload["latest_commit"]["subject"], "initial");

    let text = git_text(cwd, &["status"]);
    for expected in ["branch=main", "clean=false", "unstaged=1", "untracked=1"] {
        assert!(text.contains(expected), "{expected} missing from {text}");
    }
}

#[test]
fn git_tag_create_creates_annotated_tag() {
    if !git_available() {
        return;
    }

    let temp_dir = init_git_repo_with_one_commit();
    let cwd = temp_dir.path();

    let payload = git_json(
        cwd,
        &[
            "tag",
            "create",
            "v0.2.0",
            "--message",
            "v0.2.0",
            "--ref",
            "HEAD",
        ],
    );
    assert_eq!(payload["command"], "git.tag.create");
    assert_eq!(payload["tag"], "v0.2.0");
    assert_eq!(payload["annotated"], true);

    // The tag exists in the repository, not only in the report.
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

    let run = Harness::new()
        .in_directory(cwd)
        .run(&["--json", "git", "tags", "--latest"]);
    let payload = run.json();

    assert_eq!(payload["command"], "git.tags");
    assert_eq!(payload["latest"], true);
    assert_eq!(payload["tag_count"], 1);
    assert_eq!(payload["truncated"], false);
    assert!(run.stderr().is_empty(), "a clean read warns about nothing");
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

    let payload = git_json(cwd, &["remotes"]);
    assert_eq!(payload["command"], "git.remotes");
    assert_eq!(payload["remote_count"], 1);
    let remote = &payload["remotes"].as_array().expect("remotes array")[0];
    assert_eq!(remote["name"], "origin");
    assert_eq!(remote["provider"], "github");
}

/// Text output written to something that is not a terminal carries no escape
/// sequences.
///
/// The only assertion in this file that needs a process: the harness captures
/// through an emitter that has colour off by construction, so it cannot observe
/// the decision `Emitter::stdio` makes from `is_terminal`. Asserted once, for
/// one command, because it is a property of the emitter rather than of `git`.
#[test]
fn text_output_to_a_pipe_carries_no_escape_sequences() {
    if !git_available() {
        return;
    }

    let temp_dir = init_git_repo_with_one_commit();
    let cwd = temp_dir.path();
    fs::write(cwd.join("app.txt"), "line one\nline two changed\n").expect("file should be written");

    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["--cwd", &cwd.to_string_lossy(), "git", "status"])
        .assert()
        .success()
        .stdout(contains("branch=main"))
        .stdout(contains("\u{1b}").not());
}

fn run_git(cwd: &Path, args: &[&str]) {
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
