use super::common::IsolatedAhCommand as Command;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;

#[test]
fn shows_top_level_help() {
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.arg("--help")
        .assert()
        .success()
        .stdout(contains("AIHelper CLI toolbox"))
        .stdout(contains("ai"))
        .stdout(contains("file"))
        .stdout(contains("search"))
        .stdout(contains("ctx"))
        .stdout(contains("git"))
        .stdout(contains("http"))
        .stdout(contains("project"))
        .stdout(contains("run"))
        .stdout(contains("task"))
        .stdout(contains("upgrade"));
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
fn unknown_version_command_suggests_the_version_flag() {
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.arg("version")
        .assert()
        .failure()
        .stderr(contains("ah: 'version' is not a command."))
        .stderr(contains(
            "Did you mean:\n  ah --version    Show the AIHelper version",
        ))
        .stderr(contains("Usage:\n  ah <domain> <command> [options]"))
        .stderr(contains("Run 'ah --help' for more information."))
        .stderr(contains("DOMAIN_NOT_FOUND").not());
}

#[test]
fn misspelled_domain_suggests_a_known_command() {
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.arg("serach")
        .assert()
        .failure()
        .stderr(contains("ah: 'serach' is not a command."))
        .stderr(contains("Did you mean:\n  ah search    Search utilities"))
        .stderr(contains("Run 'ah --help' for more information."));
}

#[test]
fn unrelated_unknown_domain_does_not_guess() {
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.arg("something-unrelated")
        .assert()
        .failure()
        .stderr(contains("ah: 'something-unrelated' is not a command."))
        .stderr(contains("Did you mean:").not())
        .stderr(contains("Run 'ah --help' for more information."));
}

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

#[test]
fn misspelled_host_subcommand_includes_its_description() {
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["plugins", "lsit"])
        .assert()
        .failure()
        .stderr(contains("ah: unrecognized subcommand 'lsit'."))
        .stderr(contains(
            "Did you mean:\n  ah plugins list    List registered plugins",
        ))
        .stderr(contains("Run 'ah plugins --help' for more information."));
}

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
