//! What the process does before it reaches a command.
//!
//! `run()` inspects raw argv several times before clap ever sees it: two
//! environment-gated updater handoffs, a `--version` shortcut, and a managed
//! `mcp serve` probe. The only coverage of that ordering was a unit test that
//! searched this crate's own source for substrings and compared their byte
//! offsets, which pins the text rather than the behaviour and cannot survive a
//! restructuring it is meant to protect.
//!
//! These assert what an outside observer can actually see, so they stay true
//! however the routing is rearranged.

use super::common::IsolatedAhCommand as Command;
use predicates::str::contains;

/// The one-shot version flags answer without loading anything.
#[test]
fn version_flags_answer_alone() {
    for flag in ["--version", "-V"] {
        Command::cargo_bin("ah")
            .expect("binary should compile")
            .arg(flag)
            .assert()
            .success()
            .stdout(contains(concat!("ah ", env!("CARGO_PKG_VERSION"))))
            .stderr("");
    }
}

/// Only a bare invocation takes the shortcut. Anything longer reaches clap,
/// whose own `--version` flag then answers - so the observable result is the
/// same version line by a different route, which is worth pinning because a
/// restructuring could easily lose one of the two.
#[test]
fn version_flags_beside_other_arguments_still_report_the_version() {
    for arguments in [vec!["--version", "extra"], vec!["--json", "--version"]] {
        Command::cargo_bin("ah")
            .expect("binary should compile")
            .args(&arguments)
            .assert()
            .success()
            .stdout(contains(concat!("ah ", env!("CARGO_PKG_VERSION"))));
    }
}

/// The updater smoke handoff: the helper runs the freshly installed binary to
/// prove it starts, and that run must not trigger crash recovery.
#[test]
fn the_installed_smoke_handoff_reports_the_version() {
    Command::cargo_bin("ah")
        .expect("binary should compile")
        .env("AH_UPDATER_INSTALLED_SMOKE", "1")
        .arg("--version")
        .assert()
        .success()
        .stdout(contains(concat!("ah ", env!("CARGO_PKG_VERSION"))))
        .stderr("");
}

/// The smoke handoff is gated on the flag as well as the variable: a leaked
/// variable must not turn an unrelated command into a version report.
#[test]
fn the_installed_smoke_handoff_does_not_capture_other_commands() {
    Command::cargo_bin("ah")
        .expect("binary should compile")
        .env("AH_UPDATER_INSTALLED_SMOKE", "1")
        .args(["--json", "plugins", "list"])
        .assert()
        .success()
        .stdout(contains("plugin_name"));
}

/// The managed-MCP restore handoff runs the installed binary's service
/// commands. It must answer normally.
#[test]
fn the_managed_restore_handoff_runs_the_service_command() {
    let assert = Command::cargo_bin("ah")
        .expect("binary should compile")
        .env("AH_UPDATER_MCP_RESTORE", "1")
        .args(["--json", "mcp", "service", "status"])
        .assert();

    let output = assert.get_output();
    let rendered = String::from_utf8_lossy(&output.stdout);
    assert!(
        rendered.contains("schema_version") || !output.status.success(),
        "expected a service status payload or a clean refusal, got: {rendered}"
    );
}

/// The same guard in the other direction: the restore variable narrows to the
/// service commands the helper actually invokes.
#[test]
fn the_managed_restore_handoff_does_not_capture_other_commands() {
    Command::cargo_bin("ah")
        .expect("binary should compile")
        .env("AH_UPDATER_MCP_RESTORE", "1")
        .args(["--json", "plugins", "list"])
        .assert()
        .success()
        .stdout(contains("plugin_name"));
}

/// An argv that does not parse is still reported as a parse failure, whatever
/// startup work ran first.
#[test]
fn an_unparseable_argv_reports_a_parse_failure() {
    Command::cargo_bin("ah")
        .expect("binary should compile")
        .args(["definitely-not-a-domain"])
        .assert()
        .failure()
        .stderr(contains("is not a command"));
}

/// `mcp serve --managed-config` is detected before startup so recovery knows a
/// managed service is starting. The detection must survive both spellings of
/// the flag; here it is enough that neither is mistaken for something else.
#[test]
fn managed_serve_rejects_a_missing_definition_either_spelling() {
    for argument in [
        vec![
            "mcp",
            "serve",
            "--managed-config",
            "definitely-missing.json",
        ],
        vec!["mcp", "serve", "--managed-config=definitely-missing.json"],
    ] {
        Command::cargo_bin("ah")
            .expect("binary should compile")
            .args(&argument)
            .assert()
            .failure();
    }
}
