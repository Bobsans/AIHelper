use std::{fs, sync::Arc};

use aihelper::secrets::{ExplicitMasterKey, NewSecret, VaultStore};
use assert_cmd::Command;
use predicates::prelude::*;

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
