//! Golden snapshot of this plugin's typed command catalog.
//!
//! The catalog is currently hand-written as `json!` literals; converting it to
//! derived schemas must not change what the plugin advertises. This test
//! freezes the current output so the conversion is provably behavior-preserving.
//!
//! A failing snapshot is not automatically a bug — it is a diff a human has to
//! read and either accept or fix. Regenerate with:
//!
//! ```text
//! AH_UPDATE_SNAPSHOTS=1 cargo test -p <this crate> snapshots
//! ```
//!
//! and review the resulting `tests/snapshots/*.snap` diff in the pull request.
//!
//! The helpers below are duplicated in each dynamic plugin crate: the natural
//! home for them would be `ah-plugin-api`, but that crate is a published ABI
//! surface, not a test-support library.

use std::{fs, path::Path};

const UPDATE_ENV: &str = "AH_UPDATE_SNAPSHOTS";

#[test]
fn command_catalog_is_stable() {
    let catalog = crate::typed::command_catalog();
    let rendered =
        serde_json::to_string_pretty(&catalog).expect("command catalog should serialize");
    assert_snapshot("command-catalog", &rendered);
}

/// Compare `actual` against the checked-in snapshot, or rewrite the snapshot
/// when `AH_UPDATE_SNAPSHOTS` is set.
#[track_caller]
fn assert_snapshot(name: &str, actual: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("snapshots")
        .join(format!("{name}.snap"));
    let actual = normalize(actual);
    let package = env!("CARGO_PKG_NAME");

    if std::env::var_os(UPDATE_ENV).is_some() {
        let parent = path.parent().expect("snapshot path should have a parent");
        fs::create_dir_all(parent).expect("snapshot directory should be creatable");
        fs::write(&path, actual.as_bytes()).expect("snapshot should be writable");
        return;
    }

    let expected = fs::read_to_string(&path).map(|raw| normalize(&raw)).unwrap_or_else(|error| {
        panic!(
            "missing snapshot '{}' ({error}); create it with `{UPDATE_ENV}=1 cargo test -p {package} snapshots`",
            path.display()
        )
    });

    if expected != actual {
        panic!(
            "snapshot '{}' does not match.\n{}\n\nAccept the change with `{UPDATE_ENV}=1 cargo test -p {package} snapshots` \
             and review the diff, or fix the regression.",
            path.display(),
            first_difference(&expected, &actual)
        );
    }
}

/// Snapshots are compared line-wise with LF endings and a single trailing
/// newline, so a checkout with `core.autocrlf` cannot fail them.
fn normalize(value: &str) -> String {
    let mut normalized = value.replace("\r\n", "\n");
    while normalized.ends_with('\n') {
        normalized.pop();
    }
    normalized.push('\n');
    normalized
}

fn first_difference(expected: &str, actual: &str) -> String {
    let expected_lines = expected.lines().collect::<Vec<_>>();
    let actual_lines = actual.lines().collect::<Vec<_>>();
    for (index, (expected_line, actual_line)) in
        expected_lines.iter().zip(actual_lines.iter()).enumerate()
    {
        if expected_line != actual_line {
            return format!(
                "first difference at line {}:\n  expected: {expected_line}\n  actual:   {actual_line}",
                index + 1
            );
        }
    }
    format!(
        "line counts differ: expected {} lines, got {}",
        expected_lines.len(),
        actual_lines.len()
    )
}
