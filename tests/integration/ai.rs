use std::{fs, path::Path};

use super::common::IsolatedAhCommand as Command;
use predicates::{prelude::PredicateBooleanExt, str::contains};
use tempfile::TempDir;

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

/// Keeps every user-scope lookup inside the sandbox so a test can never read or
/// write the real agent configuration.
fn sandboxed(workspace: &Path, home: &Path) -> Command {
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.current_dir(workspace)
        .env("HOME", home)
        .env("USERPROFILE", home);
    cmd
}

fn workspace() -> (TempDir, TempDir) {
    (
        TempDir::new().expect("workspace should be created"),
        TempDir::new().expect("home should be created"),
    )
}

#[test]
fn ai_install_rules_only_creates_the_managed_block_and_is_idempotent() {
    let (project, home) = workspace();
    let rules = project.path().join("CLAUDE.md");

    sandboxed(project.path(), home.path())
        .args(["ai", "install", "claude", "--rules-only"])
        .assert()
        .success()
        .stdout(contains("rules installed"));

    let contents = fs::read_to_string(&rules).expect("rules file should exist");
    assert!(contents.contains("<!-- ah:begin (managed by `ah ai install`) -->"));
    assert!(contents.contains("ah ai info --json"));
    assert!(contents.contains("<!-- ah:end -->"));

    sandboxed(project.path(), home.path())
        .args(["ai", "install", "claude", "--rules-only"])
        .assert()
        .success()
        .stdout(contains("rules unchanged"));
    assert_eq!(
        fs::read_to_string(&rules).expect("rules file should exist"),
        contents
    );
}

#[test]
fn ai_install_preserves_user_authored_rules() {
    let (project, home) = workspace();
    let rules = project.path().join("CLAUDE.md");
    fs::write(&rules, "# House rules\n\nBe terse.\n").expect("seed rules file");

    sandboxed(project.path(), home.path())
        .args(["ai", "install", "claude", "--rules-only"])
        .assert()
        .success();

    let contents = fs::read_to_string(&rules).expect("rules file should exist");
    assert!(contents.starts_with("# House rules\n\nBe terse.\n"));
    assert!(contents.contains("<!-- ah:begin (managed by `ah ai install`) -->"));
}

#[test]
fn ai_install_dry_run_reports_without_writing() {
    let (project, home) = workspace();

    sandboxed(project.path(), home.path())
        .args(["ai", "install", "claude", "--rules-only", "--dry-run"])
        .assert()
        .success()
        .stdout(contains("would be installed"));

    assert!(!project.path().join("CLAUDE.md").exists());
}

#[test]
fn ai_uninstall_removes_the_block_and_the_generated_file() {
    let (project, home) = workspace();
    let rules = project.path().join("CLAUDE.md");

    sandboxed(project.path(), home.path())
        .args(["ai", "install", "claude", "--rules-only"])
        .assert()
        .success();
    assert!(rules.exists());

    sandboxed(project.path(), home.path())
        .args(["ai", "uninstall", "claude"])
        .assert()
        .success()
        .stdout(contains("rules removed"));
    assert!(
        !rules.exists(),
        "a rules file AIHelper created should not be left behind empty"
    );

    sandboxed(project.path(), home.path())
        .args(["ai", "uninstall", "claude"])
        .assert()
        .success()
        .stdout(contains("rules not present"));
}

#[test]
fn ai_install_json_reports_a_versioned_schema() {
    let (project, home) = workspace();

    sandboxed(project.path(), home.path())
        .args(["--json", "ai", "install", "claude", "--rules-only"])
        .assert()
        .success()
        .stdout(contains("\"command\": \"ai.install\""))
        .stdout(contains("\"schema_version\": 1"))
        .stdout(contains("\"changed\": true"))
        .stdout(contains("\"action\": \"skipped\""))
        .stdout(contains("\"action\": \"installed\""))
        .stdout(contains("\u{1b}").not());
}

#[test]
fn ai_install_rejects_an_unknown_target() {
    let (project, home) = workspace();

    sandboxed(project.path(), home.path())
        .args(["ai", "install", "emacs"])
        .assert()
        .failure()
        .stderr(contains("unknown agent target"));
}

#[test]
fn ai_install_rejects_a_scope_the_agent_does_not_have() {
    let (project, home) = workspace();

    sandboxed(project.path(), home.path())
        .args(["ai", "install", "codex", "--scope", "local", "--rules-only"])
        .assert()
        .failure()
        .stderr(contains("project, user"));
}

#[test]
fn ai_install_refuses_a_non_loopback_endpoint() {
    let (project, home) = workspace();

    sandboxed(project.path(), home.path())
        .args([
            "ai",
            "install",
            "claude",
            "--transport",
            "http",
            "--url",
            "http://10.0.0.5:8787/mcp",
        ])
        .assert()
        .failure()
        .stderr(contains("loopback"));
}

#[test]
fn ai_install_rejects_a_url_without_the_http_transport() {
    let (project, home) = workspace();

    sandboxed(project.path(), home.path())
        .args([
            "ai",
            "install",
            "claude",
            "--url",
            "http://127.0.0.1:8787/mcp",
        ])
        .assert()
        .failure()
        .stderr(contains("--url can be used only with --transport http"));
}

#[test]
fn ai_install_rejects_disabling_both_components() {
    let (project, home) = workspace();

    sandboxed(project.path(), home.path())
        .args(["ai", "install", "claude", "--mcp-only", "--rules-only"])
        .assert()
        .failure();
}

#[test]
fn ai_info_documents_the_integration_commands() {
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["--json", "ai", "info"])
        .assert()
        .success()
        .stdout(contains("\"name\": \"ai.install\""))
        .stdout(contains("\"name\": \"ai.uninstall\""))
        .stdout(contains("\"name\": \"ai.status\""));
}

fn read_json(path: &Path) -> serde_json::Value {
    serde_json::from_str(&fs::read_to_string(path).expect("config file should exist"))
        .expect("config file should be valid JSON")
}

#[test]
fn ai_install_cursor_merges_its_json_configuration() {
    let (project, home) = workspace();
    let config = project.path().join(".cursor").join("mcp.json");
    fs::create_dir_all(config.parent().expect("parent")).expect("create .cursor");
    fs::write(
        &config,
        r#"{"mcpServers":{"other":{"command":"node","args":["x.js"]}},"keepMe":true}"#,
    )
    .expect("seed cursor config");

    sandboxed(project.path(), home.path())
        .args(["ai", "install", "cursor", "--mcp-only"])
        .assert()
        .success()
        .stdout(contains("mcp installed"));

    let document = read_json(&config);
    assert_eq!(
        document["mcpServers"]["other"],
        serde_json::json!({"command": "node", "args": ["x.js"]}),
        "a foreign server must survive the merge"
    );
    assert_eq!(document["keepMe"], serde_json::json!(true));
    assert!(document["mcpServers"]["aihelper"]["command"].is_string());

    sandboxed(project.path(), home.path())
        .args(["ai", "install", "cursor", "--mcp-only"])
        .assert()
        .success()
        .stdout(contains("mcp unchanged"));

    sandboxed(project.path(), home.path())
        .args(["ai", "uninstall", "cursor"])
        .assert()
        .success()
        .stdout(contains("mcp removed"));

    let document = read_json(&config);
    assert!(document["mcpServers"].get("aihelper").is_none());
    assert!(document["mcpServers"]["other"].is_object());
    assert_eq!(document["keepMe"], serde_json::json!(true));
}

#[test]
fn ai_install_cursor_creates_the_nested_rules_file() {
    let (project, home) = workspace();

    sandboxed(project.path(), home.path())
        .args(["ai", "install", "cursor", "--rules-only"])
        .assert()
        .success();

    let rules = project.path().join(".cursor").join("rules").join("ah.mdc");
    let contents = fs::read_to_string(&rules).expect("nested rules file should be created");
    assert!(contents.contains("<!-- ah:begin (managed by `ah ai install`) -->"));
}

#[test]
fn ai_install_copilot_uses_its_own_server_key_and_path() {
    let (project, home) = workspace();

    sandboxed(project.path(), home.path())
        .args(["ai", "install", "copilot", "--mcp-only"])
        .assert()
        .success();

    let document = read_json(&project.path().join(".vscode").join("mcp.json"));
    assert!(
        document["servers"]["aihelper"].is_object(),
        "copilot stores servers under `servers`, not `mcpServers`"
    );
    assert!(document.get("mcpServers").is_none());
}

#[test]
fn ai_install_copilot_rejects_the_user_scope() {
    let (project, home) = workspace();

    sandboxed(project.path(), home.path())
        .args(["ai", "install", "copilot", "--scope", "user", "--mcp-only"])
        .assert()
        .failure()
        .stderr(contains("project"));
}

#[test]
fn ai_install_http_transport_writes_a_url_entry() {
    let (project, home) = workspace();

    sandboxed(project.path(), home.path())
        .args([
            "ai",
            "install",
            "cursor",
            "--mcp-only",
            "--transport",
            "http",
            "--url",
            "http://127.0.0.1:9123/mcp",
        ])
        .assert()
        .success();

    let document = read_json(&project.path().join(".cursor").join("mcp.json"));
    assert_eq!(
        document["mcpServers"]["aihelper"],
        serde_json::json!({"type": "http", "url": "http://127.0.0.1:9123/mcp"})
    );
}

#[test]
fn ai_install_removes_a_registration_left_under_the_previous_server_name() {
    let (project, home) = workspace();
    let config = project.path().join(".cursor").join("mcp.json");
    fs::create_dir_all(config.parent().expect("parent")).expect("create .cursor");
    fs::write(
        &config,
        r#"{"mcpServers":{"ah":{"type":"http","url":"http://127.0.0.1:8787/mcp"}}}"#,
    )
    .expect("seed a legacy registration");

    sandboxed(project.path(), home.path())
        .args(["ai", "install", "cursor", "--mcp-only"])
        .assert()
        .success()
        .stderr(contains("legacy `ah`"));

    let document = read_json(&config);
    assert!(
        document["mcpServers"].get("ah").is_none(),
        "the legacy entry must not survive as a duplicate"
    );
    assert!(document["mcpServers"]["aihelper"].is_object());
}

#[test]
fn ai_uninstall_also_clears_the_previous_server_name() {
    let (project, home) = workspace();
    let config = project.path().join(".cursor").join("mcp.json");
    fs::create_dir_all(config.parent().expect("parent")).expect("create .cursor");
    fs::write(&config, r#"{"mcpServers":{"ah":{"command":"old"}}}"#).expect("seed legacy");

    sandboxed(project.path(), home.path())
        .args(["ai", "uninstall", "cursor"])
        .assert()
        .success()
        .stdout(contains("mcp removed"));

    let document = read_json(&config);
    assert!(document["mcpServers"].get("ah").is_none());
}

#[test]
fn ai_install_dry_run_touches_no_json_configuration() {
    let (project, home) = workspace();

    sandboxed(project.path(), home.path())
        .args(["ai", "install", "cursor", "--mcp-only", "--dry-run"])
        .assert()
        .success()
        .stdout(contains("would be installed"));

    assert!(!project.path().join(".cursor").join("mcp.json").exists());
}

#[test]
fn ai_install_refuses_an_unparsable_configuration_without_touching_it() {
    let (project, home) = workspace();
    let config = project.path().join(".cursor").join("mcp.json");
    fs::create_dir_all(config.parent().expect("parent")).expect("create .cursor");
    fs::write(&config, "{ not json").expect("seed broken config");

    sandboxed(project.path(), home.path())
        .args(["ai", "install", "cursor", "--mcp-only"])
        .assert()
        .failure()
        .stderr(contains("not valid JSON"));

    assert_eq!(
        fs::read_to_string(&config).expect("file should still exist"),
        "{ not json"
    );
}

#[test]
fn ai_status_reports_file_backed_targets_and_legacy_registrations() {
    let (project, home) = workspace();
    let config = project.path().join(".cursor").join("mcp.json");
    fs::create_dir_all(config.parent().expect("parent")).expect("create .cursor");
    fs::write(&config, r#"{"mcpServers":{"ah":{"command":"old"}}}"#).expect("seed legacy");

    sandboxed(project.path(), home.path())
        .args(["--json", "ai", "status", "cursor"])
        .assert()
        .success()
        .stdout(contains("\"command\": \"ai.status\""))
        .stdout(contains("\"registrar\": \"file\""))
        .stdout(contains("\"legacy_server\": \"ah\""))
        .stdout(contains("\"cli\": null"));
}

#[cfg(windows)]
#[test]
fn ai_install_restores_the_previous_cli_registration_when_replacement_fails() {
    let (project, home) = workspace();
    let state = project.path().join("codex-state.json");
    let failed_once = project.path().join("codex-add-failed");
    let shim = project.path().join("codex.cmd");
    let old_url = "http://127.0.0.1:8787/mcp";
    let new_url = "http://127.0.0.1:9999/mcp";
    fs::write(
        &state,
        format!(
            r#"[{{"name":"aihelper","transport":{{"type":"streamable_http","url":"{old_url}"}}}}]"#
        ),
    )
    .expect("seed registration state");
    fs::write(
        &shim,
        format!(
            r#"@echo off
if /i "%2"=="list" (
  type "{}"
  exit /b 0
)
if /i "%2"=="remove" (
  >"{}" echo []
  exit /b 0
)
if /i "%2"=="add" (
  if not exist "{}" (
    type nul >"{}"
    >&2 echo replacement failed
    exit 9
  )
  >"{}" echo [{{"name":"aihelper","transport":{{"type":"streamable_http","url":"%5"}}}}]
  exit /b 0
)
exit /b 2
"#,
            state.display(),
            state.display(),
            failed_once.display(),
            failed_once.display(),
            state.display(),
        ),
    )
    .expect("write codex shim");
    let mut command = sandboxed(project.path(), home.path());
    let output = command
        .env("PATH", project.path())
        .env("PATHEXT", ".CMD")
        .args([
            "ai",
            "install",
            "codex",
            "--mcp-only",
            "--transport",
            "http",
            "--url",
            new_url,
        ])
        .output()
        .expect("AIHelper should run");

    assert!(failed_once.exists(), "the codex shim should handle add");
    assert!(!output.status.success(), "replacement should fail");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("replacement failed"),
        "the replacement error should be preserved"
    );

    let restored = read_json(&state);
    assert_eq!(restored[0]["transport"]["url"], old_url);
}
