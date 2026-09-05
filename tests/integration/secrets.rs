use std::{fs, sync::Arc};

use ah_plugin_testkit::{MockResponse, MockServer};
use aihelper::secrets::{ExplicitMasterKey, NewSecret, SecretKind, VaultStore};
use assert_cmd::Command;
use predicates::prelude::*;

use super::common::IsolatedAhCommand;

const TEST_MASTER_KEY: &str = "1111111111111111111111111111111111111111111111111111111111111111";

#[test]
fn secrets_init_and_list_use_isolated_vault() {
    let config_dir = tempfile::tempdir().expect("temporary config directory should be created");

    Command::cargo_bin("ah")
        .expect("ah binary should build")
        .env("AH_CONFIG_DIR", config_dir.path())
        .env("APPDATA", "")
        .env("AH_VAULT_MASTER_KEY", TEST_MASTER_KEY)
        .args(["secrets", "init"])
        .assert()
        .success();

    let secret_value = "integration-secret-value";
    let store = VaultStore::at(
        config_dir.path(),
        Arc::new(ExplicitMasterKey::parse(TEST_MASTER_KEY.to_owned()).unwrap()),
    );
    store
        .put(
            NewSecret::postgres("billing", "Billing", secret_value)
                .with_description(Some("Production billing database".to_owned())),
        )
        .unwrap();

    Command::cargo_bin("ah")
        .expect("ah binary should build")
        .env("AH_CONFIG_DIR", config_dir.path())
        .env("APPDATA", "")
        .env("AH_VAULT_MASTER_KEY", TEST_MASTER_KEY)
        .args(["--json", "secrets", "list", "--kind", "postgres"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"id\": \"billing\""))
        .stdout(predicate::str::contains("\"kind\": \"postgres\""))
        .stdout(predicate::str::contains(
            "\"description\": \"Production billing database\"",
        ))
        .stdout(predicate::str::contains(secret_value).not());

    for args in [
        vec![
            "secrets",
            "add",
            "forbidden",
            "--kind",
            "postgres",
            secret_value,
        ],
        vec![
            "secrets",
            "--json",
            "add",
            "forbidden-json",
            "--kind",
            "postgres",
            secret_value,
        ],
        vec!["secrets", "edit", "billing", secret_value],
    ] {
        Command::cargo_bin("ah")
            .expect("ah binary should build")
            .env("AH_CONFIG_DIR", config_dir.path())
            .env("APPDATA", "")
            .env("AH_VAULT_MASTER_KEY", TEST_MASTER_KEY)
            .args(args)
            .assert()
            .failure()
            .stdout(predicate::str::contains(secret_value).not())
            .stderr(predicate::str::contains(secret_value).not());
    }

    let logs = fs::read_dir(config_dir.path().join("logs"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| fs::read_to_string(entry.path()).unwrap())
        .collect::<String>();
    assert!(logs.contains("[REDACTED]"));
    assert!(!logs.contains(secret_value));

    Command::cargo_bin("ah")
        .expect("ah binary should build")
        .env("AH_CONFIG_DIR", config_dir.path())
        .env("APPDATA", "")
        .env("AH_VAULT_MASTER_KEY", TEST_MASTER_KEY)
        .args(["--json", "ai", "info"])
        .assert()
        .success()
        .stdout(predicate::str::contains("secrets.list"))
        .stdout(predicate::str::contains("typed/MCP secrets.list"));
}

#[test]
fn github_vault_token_reaches_the_dynamic_plugin_without_leaking() {
    let server = MockServer::new(vec![MockResponse::json(
        200,
        r#"{
            "id": 10,
            "tag_name": "v1.0.0",
            "name": "v1.0.0",
            "draft": false,
            "prerelease": false,
            "html_url": "https://github.com/acme/tool/releases/tag/v1.0.0",
            "published_at": "2026-05-06T00:00:00Z",
            "assets": []
        }"#,
    )]);
    let token = "github-vault-token-sentinel";
    let credential_id = "github-vault-credential";
    let mut command = IsolatedAhCommand::cargo_bin("ah").unwrap();
    let store = VaultStore::at(
        command.config_dir(),
        Arc::new(ExplicitMasterKey::parse(TEST_MASTER_KEY.to_owned()).unwrap()),
    );
    store.initialize().unwrap();
    store
        .put(NewSecret::api_token(
            credential_id,
            "GitHub PAT",
            SecretKind::GithubToken,
            token,
        ))
        .unwrap();

    let output = command
        .env("APPDATA", "")
        .env("AH_VAULT_MASTER_KEY", TEST_MASTER_KEY)
        .env("AH_LOG_UNREDACTED", "1")
        .args([
            "--json",
            "github",
            "--repo",
            "acme/tool",
            "--api-url",
            &server.url(),
            "--credential",
            &format!("token={credential_id}"),
            "release",
            "get",
            "v1.0.0",
        ])
        .assert()
        .success();

    let request = server.requests().pop().unwrap();
    let expected_authorization = format!("Bearer {token}");
    assert_eq!(
        request.header("authorization"),
        Some(expected_authorization.as_str())
    );
    assert!(!String::from_utf8_lossy(&output.get_output().stdout).contains(token));
    assert!(!String::from_utf8_lossy(&output.get_output().stderr).contains(token));
    assert!(!String::from_utf8_lossy(&output.get_output().stdout).contains(credential_id));
    assert!(!String::from_utf8_lossy(&output.get_output().stderr).contains(credential_id));
    let logs = fs::read_dir(command.config_dir().join("logs"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| fs::read_to_string(entry.path()).unwrap())
        .collect::<String>();
    assert!(!logs.contains(token));
    assert!(!logs.contains(credential_id));
}

#[test]
fn gitlab_vault_token_reaches_the_dynamic_plugin_without_leaking() {
    let server = MockServer::new(vec![MockResponse::json(
        200,
        r#"{
            "tag_name": "v1.0.0",
            "name": "v1.0.0",
            "description": "notes",
            "created_at": "2026-05-07T00:00:00Z",
            "released_at": "2026-05-07T00:01:00Z",
            "upcoming_release": false,
            "assets": {"links": []}
        }"#,
    )]);
    let token = "gitlab-vault-token-sentinel";
    let credential_id = "gitlab-vault-credential";
    let mut command = IsolatedAhCommand::cargo_bin("ah").unwrap();
    let store = VaultStore::at(
        command.config_dir(),
        Arc::new(ExplicitMasterKey::parse(TEST_MASTER_KEY.to_owned()).unwrap()),
    );
    store.initialize().unwrap();
    store
        .put(NewSecret::api_token(
            credential_id,
            "GitLab PAT",
            SecretKind::GitlabToken,
            token,
        ))
        .unwrap();

    let output = command
        .env("APPDATA", "")
        .env("AH_VAULT_MASTER_KEY", TEST_MASTER_KEY)
        .env("AH_LOG_UNREDACTED", "1")
        .args([
            "--json",
            "gitlab",
            "--project",
            "group/subgroup/tool",
            "--api-url",
            &server.url(),
            "--credential",
            &format!("token={credential_id}"),
            "release",
            "get",
            "v1.0.0",
        ])
        .assert()
        .success();

    let request = server.requests().pop().unwrap();
    assert_eq!(request.header("private-token"), Some(token));
    assert!(!String::from_utf8_lossy(&output.get_output().stdout).contains(token));
    assert!(!String::from_utf8_lossy(&output.get_output().stderr).contains(token));
    assert!(!String::from_utf8_lossy(&output.get_output().stdout).contains(credential_id));
    assert!(!String::from_utf8_lossy(&output.get_output().stderr).contains(credential_id));
    let logs = fs::read_dir(command.config_dir().join("logs"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| fs::read_to_string(entry.path()).unwrap())
        .collect::<String>();
    assert!(!logs.contains(token));
    assert!(!logs.contains(credential_id));
}
