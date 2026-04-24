use assert_cmd::Command;
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
        .stdout(contains("task"));
}

#[test]
fn shows_file_subcommand_help() {
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["file", "--help"])
        .assert()
        .success()
        .stdout(contains("read"));
}
