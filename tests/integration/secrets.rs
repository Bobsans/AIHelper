use std::sync::Arc;

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

    Command::cargo_bin("ah")
        .expect("ah binary should build")
        .env("AH_CONFIG_DIR", config_dir.path())
        .env("AH_VAULT_MASTER_KEY", TEST_MASTER_KEY)
        .args([
            "secrets",
            "add",
            "forbidden",
            "--kind",
            "postgres",
            "--password",
            secret_value,
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains(secret_value).not())
        .stderr(predicate::str::contains(secret_value).not());

    Command::cargo_bin("ah")
        .expect("ah binary should build")
        .env("AH_CONFIG_DIR", config_dir.path())
        .env("AH_VAULT_MASTER_KEY", TEST_MASTER_KEY)
        .args(["--json", "ai", "info"])
        .assert()
        .success()
        .stdout(predicate::str::contains("secrets.list"))
        .stdout(predicate::str::contains("typed/MCP secrets.list"));
}
