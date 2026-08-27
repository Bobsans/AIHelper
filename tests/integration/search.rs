//! `ah search`, run in this process.
//!
//! Every test here was a subprocess asserting `contains` on stdout or stderr.
//! The text assertions are the same strings against the captured output; the
//! JSON ones index into the payload instead of looking for a quoted field
//! somewhere in pretty-printed text; and the two refusals assert the whole
//! rendered diagnostic, hint included, rather than fragments of it.

use std::{fs, path::Path};

use aihelper::harness::Harness;
use serde_json::Value;
use tempfile::TempDir;

fn search_text(args: &[&str]) -> String {
    let mut argv = vec!["search"];
    argv.extend_from_slice(args);
    Harness::new().run(&argv).expect_success().to_owned()
}

fn search_json(args: &[&str]) -> Value {
    let mut argv = vec!["--json", "search"];
    argv.extend_from_slice(args);
    Harness::new().run(&argv).json()
}

/// The rendered refusal, as `AppError::print` would have written it.
fn search_refusal(args: &[&str]) -> String {
    let mut argv = vec!["search"];
    argv.extend_from_slice(args);
    let run = Harness::new().run(&argv);
    let _ = run.expect_failure();
    run.diagnostic_text(&argv)
}

fn root_of(dir: &Path) -> String {
    dir.to_string_lossy().into_owned()
}

#[test]
fn search_path_not_found_error_is_rendered_without_nested_wrappers() {
    let rendered = search_refusal(&["text", "customFieldValues", "Fixdigital"]);

    assert_eq!(
        rendered,
        "ah: path does not exist: Fixdigital\n\n\
         Hint: Check the path or set a different working directory with --cwd."
    );
}

#[test]
fn search_invalid_regex_error_is_concise() {
    let rendered = search_refusal(&["text", "(", "src", "--regex"]);

    assert_eq!(
        rendered,
        "ah: invalid regular expression: unclosed group\n\n\
         Hint: Fix the expression or remove --regex to search literally."
    );
}

#[test]
fn search_text_plain_mode_treats_pattern_as_literal() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    fs::write(temp_dir.path().join("notes.txt"), "a.c\nabc\n")
        .expect("test file should be written");

    let output = search_text(&["text", "a.c", &root_of(temp_dir.path())]);

    assert!(output.contains("notes.txt:1:a.c"), "{output}");
    assert!(!output.contains("notes.txt:2:abc"), "{output}");
}

#[test]
fn search_text_regex_mode_matches_regex() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    fs::write(temp_dir.path().join("app.log"), "id=42\nid=ab\n")
        .expect("test file should be written");

    let output = search_text(&["text", "id=\\d+", &root_of(temp_dir.path()), "--regex"]);

    assert!(output.contains("app.log:1:id=42"), "{output}");
    assert!(!output.contains("app.log:2:id=ab"), "{output}");
}

#[test]
fn search_text_with_context_includes_neighbor_lines() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    fs::write(
        temp_dir.path().join("notes.txt"),
        "before\nmatch line\nafter\n",
    )
    .expect("test file should be written");

    let output = search_text(&["text", "match", &root_of(temp_dir.path()), "--context", "1"]);

    for expected in [
        "notes.txt-1-before",
        "notes.txt:2:match line",
        "notes.txt-3-after",
    ] {
        assert!(
            output.contains(expected),
            "{expected} missing from {output}"
        );
    }
}

#[test]
fn search_text_context_does_not_include_extra_after_line() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    fs::write(
        temp_dir.path().join("notes.txt"),
        "before-1\nbefore-2\nmatch-line\nafter-1\nafter-2\nafter-3\n",
    )
    .expect("test file should be written");

    let output = search_text(&[
        "text",
        "match-line",
        &root_of(temp_dir.path()),
        "--context",
        "2",
    ]);

    for expected in [
        "notes.txt-1-before-1",
        "notes.txt-2-before-2",
        "notes.txt:3:match-line",
        "notes.txt-4-after-1",
        "notes.txt-5-after-2",
    ] {
        assert!(
            output.contains(expected),
            "{expected} missing from {output}"
        );
    }
    assert!(
        !output.contains("notes.txt-6-after-3"),
        "the context is bounded: {output}"
    );
}

#[test]
fn search_text_json_contains_mode_fields() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    fs::write(temp_dir.path().join("one.txt"), "alpha\nbeta\n")
        .expect("test file should be written");

    let payload = search_json(&["text", "alpha", &root_of(temp_dir.path())]);

    assert_eq!(payload["command"], "search.text");
    assert_eq!(payload["regex"], false);
    assert_eq!(payload["match_count"], 1);
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

    let payload = search_json(&["text", "needle", &root_of(temp_dir.path())]);

    assert_eq!(payload["backend"], "ignore+rust");
    assert_eq!(payload["match_count"], 1);
    assert_eq!(payload["matches"][0]["path"], "visible.txt");
}

#[test]
fn search_text_json_reports_character_column_for_unicode_lines() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    fs::write(temp_dir.path().join("unicode.txt"), "a\u{00e9}needle\n")
        .expect("test file should be written");

    let payload = search_json(&["text", "needle", &root_of(temp_dir.path())]);

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

    let payload = search_json(&[
        "text",
        "match",
        &root_of(temp_dir.path()),
        "--max-bytes",
        "32",
    ]);

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

    let output = search_text(&["text", "needle", &root_of(&left), &root_of(&right)]);

    assert!(output.contains("one.txt"), "{output}");
    assert!(output.contains("two.txt"), "{output}");
}

#[test]
fn search_files_returns_matching_paths() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    fs::write(temp_dir.path().join("alpha.txt"), "a").expect("file should be written");
    fs::write(temp_dir.path().join("beta.txt"), "b").expect("file should be written");
    fs::create_dir(temp_dir.path().join("nested")).expect("dir should be created");
    fs::write(temp_dir.path().join("nested").join("alpha_notes.md"), "c")
        .expect("file should be written");

    let output = search_text(&["files", "alpha", &root_of(temp_dir.path())]);

    assert!(output.contains("alpha.txt"), "{output}");
    assert!(output.contains("alpha_notes.md"), "{output}");
    assert!(!output.contains("beta.txt"), "{output}");
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

    let output = search_text(&["files", "alpha", &root_of(&left), &root_of(&right)]);

    assert!(output.contains("alpha_left.txt"), "{output}");
    assert!(output.contains("alpha_right.txt"), "{output}");
    assert!(!output.contains("beta.txt"), "{output}");
}
