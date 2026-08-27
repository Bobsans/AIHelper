//! What the shipped binary does with a command line it cannot serve.
//!
//! Deliberately thin. Six of the nine tests that used to live here asserted
//! text fragments with `contains`, and the same text is now asserted in full,
//! where it is produced:
//!
//! | Was here | Now |
//! |----------|-----|
//! | `--help` and `file --help` contain some names | `tests/snapshots/cli-help.snap`, which freezes the whole help tree byte for byte |
//! | three unknown-domain suggestions | `cli::tests::diagnostics`, whole-string, against the real domain list |
//! | a misspelled host subcommand | `cli::tests::diagnostics`, including the usage line the `contains` version never pinned |
//!
//! What is left is what only a process can show: that a refusal reaches
//! *stderr* with a failing exit code, that the plugin dispatch path renders the
//! plugin's own parse error, and that `--json` switches the whole thing to a
//! structured payload. Reaching the first two in-process needs a
//! `PluginManager`; that is the next step of group 08's step 6, not this one.

use super::common::IsolatedAhCommand as Command;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;

/// A plugin domain parses its own arguments, so this typo is caught inside the
/// plugin and rendered by the host - a path no in-process test reaches yet.
#[test]
fn misspelled_subcommand_keeps_clap_suggestion_and_scoped_help() {
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["project", "versoin"])
        .assert()
        .failure()
        .stderr(contains("ah: unrecognized subcommand 'versoin'."))
        .stderr(contains(
            "Did you mean:\n  ah project version    Detect project versions",
        ))
        .stderr(contains(
            "To show the AIHelper version, run:\n  ah --version",
        ))
        .stderr(contains("Usage:\n  ah project <COMMAND>"))
        .stderr(contains("Run 'ah project --help' for more information."))
        .stderr(contains("INVALID_ARGUMENT").not());
}

/// Same path, for the argument a plugin requires rather than the subcommand it
/// does not have.
#[test]
fn missing_required_argument_shows_name_usage_and_scoped_help() {
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["search", "text"])
        .assert()
        .failure()
        .stderr(contains(
            "ah: the following required arguments were not provided:\n  <PATTERN>",
        ))
        .stderr(contains("Usage:\n  ah search text <PATTERN> [PATH]..."))
        .stderr(contains(
            "Run 'ah search text --help' for more information.",
        ));
}

/// `--json` is read from the environment by the error printer, so which of the
/// two forms a refusal takes can only be observed from outside.
#[test]
fn unknown_command_json_contract_remains_structured() {
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    let assert = cmd.args(["--json", "version"]).assert().failure();
    let payload: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stderr).expect("valid JSON error expected");

    assert_eq!(payload["domain"], "plugins");
    assert_eq!(payload["operation"], "plugin.runtime");
    assert_eq!(payload["code"], "DOMAIN_NOT_FOUND");
    assert_eq!(payload["message"], "unknown command domain: version");
    assert_eq!(payload["cause"], "unknown command domain: version");
    assert_eq!(payload["exit_code_hint"], 1);
}

/// And that the text form goes to stderr rather than stdout, with a failing
/// exit code. The wording itself is asserted in `cli::tests::diagnostics`.
#[test]
fn a_refused_command_writes_to_stderr_and_fails() {
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.arg("serach")
        .assert()
        .failure()
        .stdout("")
        .stderr(contains("ah: 'serach' is not a command."))
        .stderr(contains("Did you mean:\n  ah search    Search utilities"));
}
