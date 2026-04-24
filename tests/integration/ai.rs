use assert_cmd::Command;
use predicates::str::contains;

#[test]
fn ai_info_text_outputs_domain_manual() {
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["ai", "info", "--domain", "file"])
        .assert()
        .success()
        .stdout(contains("Domain: file"))
        .stdout(contains("ah file read"));
}

#[test]
fn ai_info_json_outputs_structured_manual() {
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["--json", "ai", "info", "--domain", "search"])
        .assert()
        .success()
        .stdout(contains("\"command\": \"ai.info\""))
        .stdout(contains("\"domain\": \"search\""))
        .stdout(contains("\"name\": \"text\""));
}
