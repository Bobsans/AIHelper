use assert_cmd::Command;
use predicates::{prelude::PredicateBooleanExt, str::contains};

#[test]
fn ai_info_text_outputs_domain_manual() {
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["ai", "info", "--domain", "file"])
        .assert()
        .success()
        .stdout(contains("Domain: file"))
        .stdout(contains("ah file read"))
        .stdout(contains("\u{1b}").not());
}

#[test]
fn ai_info_json_outputs_structured_manual() {
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["--json", "ai", "info", "--domain", "search"])
        .assert()
        .success()
        .stdout(contains("\"command\": \"ai.info\""))
        .stdout(contains("\"domain\": \"search\""))
        .stdout(contains("\"name\": \"text\""))
        .stdout(contains("\u{1b}").not());
}

#[test]
fn ai_info_includes_managed_mcp_service_commands() {
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["--json", "ai", "info"])
        .assert()
        .success()
        .stdout(contains("\"name\": \"mcp.service.install\""))
        .stdout(contains("\"name\": \"mcp.service.start\""))
        .stdout(contains("\"name\": \"mcp.service.stop\""))
        .stdout(contains("\"name\": \"mcp.service.restart\""))
        .stdout(contains("\"name\": \"mcp.service.status\""))
        .stdout(contains("\"name\": \"mcp.service.uninstall\""));
}
