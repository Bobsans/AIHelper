use std::{
    collections::BTreeMap, fs, path::PathBuf, process::Command as ProcessCommand, sync::Arc,
};

use aihelper::secrets::{ExplicitMasterKey, NewSecret, SecretKind, VaultStore};
use predicates::str::contains;
use tempfile::TempDir;

use super::common::IsolatedAhCommand as Command;

const TEST_MASTER_KEY: &str = "2222222222222222222222222222222222222222222222222222222222222222";

#[test]
fn postgres_ping_uses_vault_credential_without_exposing_it() {
    assert!(
        postgres_plugin_path().is_file(),
        "build ah-plugin-postgres before running this focused integration test"
    );
    let password = "postgres-cli-secret-sentinel";
    let credential_id = "postgres-cli-private-id";
    let tool_dir = TempDir::new().expect("temporary tool directory should be created");
    let psql = write_fake_psql(&tool_dir, password);
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    let store = VaultStore::at(
        cmd.config_dir(),
        Arc::new(ExplicitMasterKey::parse(TEST_MASTER_KEY.to_owned()).unwrap()),
    );
    store.initialize().unwrap();
    store
        .put(NewSecret::new(
            credential_id,
            "Postgres CLI",
            SecretKind::Postgres,
            BTreeMap::from([
                ("database".to_owned(), "app".to_owned()),
                ("host".to_owned(), "db.internal".to_owned()),
                ("password".to_owned(), password.to_owned()),
                ("port".to_owned(), "5433".to_owned()),
                ("sslmode".to_owned(), "require".to_owned()),
                ("user".to_owned(), "alice".to_owned()),
            ]),
        ))
        .unwrap();

    let assert = cmd
        .env("APPDATA", "")
        .env("AH_VAULT_MASTER_KEY", TEST_MASTER_KEY)
        .env("AH_LOG_UNREDACTED", "1")
        .args([
            "postgres",
            "ping",
            "--tool-path",
            &psql.to_string_lossy(),
            "--credential",
            &format!("database={credential_id}"),
        ])
        .assert()
        .success()
        .stdout(contains("ok: PostgreSQL 18.4 database=app user=alice"));
    assert!(!String::from_utf8_lossy(&assert.get_output().stdout).contains(password));
    assert!(!String::from_utf8_lossy(&assert.get_output().stderr).contains(password));

    let logs = fs::read_dir(cmd.config_dir().join("logs"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| fs::read_to_string(entry.path()).unwrap())
        .collect::<String>();
    assert!(!logs.contains(credential_id));
    assert!(!logs.contains(password));
    assert!(!logs.contains("db.internal"));
}

#[test]
fn postgres_ping_keeps_password_only_vault_credentials_compatible() {
    assert!(postgres_plugin_path().is_file());
    let password = "postgres-legacy-secret-sentinel";
    let credential_id = "postgres-legacy-credential";
    let tool_dir = TempDir::new().expect("temporary tool directory should be created");
    let psql = write_fake_psql(&tool_dir, password);
    let mut command = Command::cargo_bin("ah").expect("binary should compile");
    let store = VaultStore::at(
        command.config_dir(),
        Arc::new(ExplicitMasterKey::parse(TEST_MASTER_KEY.to_owned()).unwrap()),
    );
    store.initialize().unwrap();
    store
        .put(NewSecret::postgres(
            credential_id,
            "Legacy Postgres CLI",
            password,
        ))
        .unwrap();

    command
        .env("APPDATA", "")
        .env("AH_VAULT_MASTER_KEY", TEST_MASTER_KEY)
        .args([
            "postgres",
            "ping",
            "--tool-path",
            &psql.to_string_lossy(),
            "--host",
            "db.internal",
            "--port",
            "5433",
            "--database",
            "app",
            "--user",
            "alice",
            "--sslmode",
            "require",
            "--credential",
            &format!("database={credential_id}"),
        ])
        .assert()
        .success()
        .stdout(contains("ok: PostgreSQL 18.4 database=app user=alice"));
}

fn postgres_plugin_path() -> PathBuf {
    let profile_dir = assert_cmd::cargo::cargo_bin("ah")
        .parent()
        .expect("ah binary should have a profile directory")
        .to_path_buf();
    if cfg!(windows) {
        profile_dir.join("ah_plugin_postgres.dll")
    } else if cfg!(target_os = "macos") {
        profile_dir.join("libah_plugin_postgres.dylib")
    } else {
        profile_dir.join("libah_plugin_postgres.so")
    }
}

fn write_fake_psql(directory: &TempDir, password: &str) -> PathBuf {
    let source = directory.path().join("fake_psql.rs");
    let executable = directory
        .path()
        .join(if cfg!(windows) { "psql.exe" } else { "psql" });
    fs::write(
        &source,
        format!(
            r##"fn main() {{
    if std::env::args().nth(1).as_deref() == Some("--version") {{
        println!("psql (PostgreSQL) 18.4");
        return;
    }}
    if std::env::var_os("AH_VAULT_MASTER_KEY").is_some() {{
        std::process::exit(10);
    }}
    if std::env::var("PGPASSWORD").as_deref() != Ok("{password}") {{
        std::process::exit(9);
    }}
    for (name, expected) in [
        ("PGHOST", "db.internal"),
        ("PGPORT", "5433"),
        ("PGDATABASE", "app"),
        ("PGUSER", "alice"),
        ("PGSSLMODE", "require"),
    ] {{
        if std::env::var(name).as_deref() != Ok(expected) {{
            std::process::exit(8);
        }}
    }}
    println!("{{}}", r#"{{"server_version":"18.4","current_database":"app","current_user":"alice","session_user":"alice","current_schema":"public","server_encoding":"UTF8","inet_server_addr":"127.0.0.1","inet_server_port":5432}}"#);
}}"##
        ),
    )
    .unwrap();
    let status = ProcessCommand::new("rustc")
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .status()
        .unwrap();
    assert!(status.success(), "fake psql fixture should compile");
    executable
}
