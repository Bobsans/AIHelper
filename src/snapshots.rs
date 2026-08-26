//! Golden snapshots of the artifacts the refactoring program must not change
//! by accident: the typed command catalog, the plugin manuals, the rendered CLI
//! help tree, and the error-code table.
//!
//! These tests exist to make structural refactors provably behavior-preserving.
//! A failing snapshot is not automatically a bug — it is a diff that a human has
//! to look at and either accept or fix. Regenerate with:
//!
//! ```text
//! AH_UPDATE_SNAPSHOTS=1 cargo test --lib snapshots
//! ```
//!
//! and review the resulting `tests/snapshots/*.snap` diff in the pull request.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use ah_runtime::PluginManager;
use serde_json::{Value, json};

use crate::{
    plugin_settings::PluginSettings,
    secrets::{KeyProvider, VaultError, VaultStore},
};

const UPDATE_ENV: &str = "AH_UPDATE_SNAPSHOTS";
const HELP_TERM_WIDTH: usize = 100;

fn snapshot_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("snapshots")
        .join(format!("{name}.snap"))
}

/// Compare `actual` against the checked-in snapshot, or rewrite the snapshot
/// when `AH_UPDATE_SNAPSHOTS` is set.
#[track_caller]
fn assert_snapshot(name: &str, actual: &str) {
    let path = snapshot_path(name);
    let actual = normalize(actual);

    if std::env::var_os(UPDATE_ENV).is_some() {
        let parent = path.parent().expect("snapshot path should have a parent");
        fs::create_dir_all(parent).expect("snapshot directory should be creatable");
        fs::write(&path, actual.as_bytes()).expect("snapshot should be writable");
        return;
    }

    let expected = fs::read_to_string(&path).map(|raw| normalize(&raw)).unwrap_or_else(|error| {
        panic!(
            "missing snapshot '{}' ({error}); create it with `{UPDATE_ENV}=1 cargo test --lib snapshots`",
            path.display()
        )
    });

    if expected != actual {
        panic!(
            "snapshot '{}' does not match.\n{}\n\nAccept the change with `{UPDATE_ENV}=1 cargo test --lib snapshots` \
             and review the diff, or fix the regression.",
            path.display(),
            first_difference(&expected, &actual)
        );
    }
}

/// Snapshots are compared line-wise with LF endings and a single trailing
/// newline, so a checkout with `core.autocrlf` cannot fail them.
fn normalize(value: &str) -> String {
    let mut normalized = value.replace("\r\n", "\n");
    while normalized.ends_with('\n') {
        normalized.pop();
    }
    normalized.push('\n');
    normalized
}

fn first_difference(expected: &str, actual: &str) -> String {
    let expected_lines = expected.lines().collect::<Vec<_>>();
    let actual_lines = actual.lines().collect::<Vec<_>>();
    for (index, (expected_line, actual_line)) in
        expected_lines.iter().zip(actual_lines.iter()).enumerate()
    {
        if expected_line != actual_line {
            return format!(
                "first difference at line {}:\n  expected: {expected_line}\n  actual:   {actual_line}",
                index + 1
            );
        }
    }
    format!(
        "line counts differ: expected {} lines, got {}",
        expected_lines.len(),
        actual_lines.len()
    )
}

struct StubKeyProvider;

impl KeyProvider for StubKeyProvider {
    fn load_or_create(&self) -> Result<[u8; 32], VaultError> {
        Ok([0_u8; 32])
    }
}

/// The same registration sequence `runtime_flow` performs for `ah mcp serve`,
/// minus dynamic plugin discovery so the result does not depend on what happens
/// to sit next to the test binary.
fn snapshot_manager() -> Arc<PluginManager> {
    let missing = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("snapshot-nonexistent");
    let settings = Arc::new(Mutex::new(
        PluginSettings::load_from_path(missing.join("plugins.json"))
            .expect("absent plugin settings should load as defaults"),
    ));
    let vault = Arc::new(VaultStore::at(&missing, Arc::new(StubKeyProvider)));

    Arc::new_cyclic(|weak| {
        let mut manager = PluginManager::new();
        manager.reserve_dynamic_domains(["ai", "plugins", "mcp", "secrets", "upgrade"]);
        for plugin in crate::plugins::builtins() {
            manager.register_builtin(plugin);
        }
        for plugin in
            crate::host_commands::builtins(weak.clone(), Arc::clone(&settings), Arc::clone(&vault))
        {
            manager.register_host_builtin(plugin);
        }
        manager
    })
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).expect("snapshot payload should serialize")
}

#[test]
fn typed_command_catalog_is_stable() {
    let manager = snapshot_manager();
    let commands = manager
        .list_enabled_commands()
        .expect("the built-in catalog should validate");

    let payload = commands
        .into_iter()
        .map(|command| {
            let descriptor =
                serde_json::to_value(&command.descriptor).expect("descriptor should serialize");
            json!({
                "domain": command.plugin.domain,
                "plugin": command.plugin.plugin_name,
                "descriptor": descriptor,
            })
        })
        .collect::<Vec<_>>();

    assert_snapshot("typed-command-catalog", &pretty(&Value::Array(payload)));
}

#[test]
fn plugin_manuals_are_stable() {
    let manager = snapshot_manager();
    let manuals =
        serde_json::to_value(manager.collect_plugin_manuals()).expect("manuals should serialize");
    assert_snapshot("plugin-manuals", &pretty(&manuals));
}

#[test]
fn cli_help_tree_is_stable() {
    let manager = snapshot_manager();
    let mut command =
        crate::cli::build_cli_command(&manager.list_enabled_plugins()).term_width(HELP_TERM_WIDTH);

    let mut rendered = String::new();
    render_help_tree(&mut command, "ah", &mut rendered);
    assert_snapshot("cli-help", &rendered);
}

fn render_help_tree(command: &mut clap::Command, path: &str, out: &mut String) {
    out.push_str(&format!("$ {path} --help\n"));
    out.push_str(command.render_help().to_string().trim_end());
    out.push_str("\n\n");

    let mut names = command
        .get_subcommands()
        .map(|sub| sub.get_name().to_owned())
        .collect::<Vec<_>>();
    names.sort();
    for name in names {
        let child_path = format!("{path} {name}");
        let child = command
            .find_subcommand_mut(&name)
            .expect("listed subcommand should be resolvable");
        render_help_tree(child, &child_path, out);
    }
}

/// The corpora these two snapshots run over live with the code they pin -
/// `symbols.rs` and `rules.rs` - because each is the set of inputs that reaches
/// every arm of one table, and a table gaining an arm has to gain a fixture in
/// the same edit.
#[test]
fn symbol_extraction_is_stable() {
    let payload = crate::commands::ctx::symbols::SYMBOL_FIXTURES
        .iter()
        .map(|(name, content)| {
            let symbols = crate::commands::ctx::symbols::extract_symbols(Path::new(name), content);
            json!({
                "file": name,
                "symbols": serde_json::to_value(&symbols)
                    .expect("symbols should serialize"),
            })
        })
        .collect::<Vec<_>>();

    assert_snapshot("symbol-extraction", &pretty(&Value::Array(payload)));
}

#[test]
fn project_file_classification_is_stable() {
    let payload = crate::commands::project::rules::PROJECT_RULE_FIXTURES
        .iter()
        .map(|(rel, name)| {
            let detections = crate::commands::project::rules::classify_file(rel, name)
                .into_iter()
                .map(|detection| {
                    json!({
                        "group": format!("{:?}", detection.group),
                        "kind": detection.kind,
                        "ecosystem": detection.ecosystem,
                        "tool": detection.tool,
                        "role": detection.role,
                    })
                })
                .collect::<Vec<_>>();
            json!({ "path": rel, "detections": detections })
        })
        .collect::<Vec<_>>();

    assert_snapshot("project-file-rules", &pretty(&Value::Array(payload)));
}

#[test]
fn error_codes_are_stable() {
    use ah_runtime::RuntimeError;
    use std::path::PathBuf as StdPathBuf;

    let runtime_errors: Vec<RuntimeError> = vec![
        RuntimeError::DomainNotFound("nope".to_owned()),
        RuntimeError::AbiVersionMismatch {
            path: StdPathBuf::from("plugin.dll"),
            found: 2,
            expected: 1,
        },
        RuntimeError::ApiVersionMismatch {
            path: StdPathBuf::from("plugin.dll"),
            found_major: 9,
            found_minor: 1,
            supported_major: 1,
            supported_minor: 0,
        },
        RuntimeError::InvalidMetadata {
            path: StdPathBuf::from("plugin.dll"),
            reason: "empty domain".to_owned(),
        },
        RuntimeError::Invocation("boom".to_owned()),
        RuntimeError::ResponseParse("bad json".to_owned()),
        RuntimeError::InvalidCommandCatalog {
            domain: "probe".to_owned(),
            reason: "duplicate id".to_owned(),
        },
        RuntimeError::TypedCommandNotFound("probe.missing".to_owned()),
        RuntimeError::TypedInvocation("bad arguments".to_owned()),
        RuntimeError::SecretRequired {
            command: "http.get".to_owned(),
            slot: "basic".to_owned(),
        },
        RuntimeError::SecretNotFound {
            command: "http.get".to_owned(),
            slot: "basic".to_owned(),
            id: "private-id".to_owned(),
        },
        RuntimeError::SecretKindMismatch {
            command: "http.get".to_owned(),
            slot: "basic".to_owned(),
            id: "private-id".to_owned(),
            kind: "private-kind".to_owned(),
            accepted_kinds: vec!["private-accepted".to_owned()],
        },
        RuntimeError::VaultLocked {
            command: "http.get".to_owned(),
            slot: "basic".to_owned(),
            id: "private-id".to_owned(),
        },
        RuntimeError::VaultKeyUnavailable {
            command: "http.get".to_owned(),
            slot: "basic".to_owned(),
            id: "private-id".to_owned(),
        },
        RuntimeError::TypedResponseValidation {
            command: "probe.run".to_owned(),
            reason: "missing field".to_owned(),
        },
        RuntimeError::InvalidExecutionRequest("empty command".to_owned()),
        RuntimeError::ExecutionCapacityFull { capacity: 4 },
        RuntimeError::ExecutionCancelled {
            request_id: "req-1".to_owned(),
        },
        RuntimeError::ExecutionTimeout {
            request_id: "req-1".to_owned(),
        },
        RuntimeError::ExecutorShuttingDown,
        RuntimeError::ExecutionWorker("worker gone".to_owned()),
        RuntimeError::ExecutionPanic {
            request_id: "req-1".to_owned(),
        },
        RuntimeError::DomainDisabled("git".to_owned()),
        RuntimeError::DependencyMissing {
            domain: "git".to_owned(),
            operation: Some("git.status".to_owned()),
            tool: "git".to_owned(),
            reason: "not on PATH".to_owned(),
        },
    ];

    let mut rows = runtime_errors
        .into_iter()
        .map(|error| error_row("RuntimeError", crate::map_runtime_error(error)))
        .collect::<Vec<_>>();

    let io_error = || std::io::Error::from(std::io::ErrorKind::NotFound);
    let app_errors = vec![
        crate::error::AppError::invalid_argument("--limit must be >= 1"),
        crate::error::AppError::external("PROBE_FAILED", "probe failed"),
        crate::error::AppError::unknown_command("serach", None),
        crate::error::AppError::cwd(StdPathBuf::from("missing"), io_error()),
        crate::error::AppError::file_read(StdPathBuf::from("missing.txt"), io_error()),
        crate::error::AppError::file_write(StdPathBuf::from("missing.txt"), io_error()),
        crate::error::AppError::file_metadata(StdPathBuf::from("missing.txt"), io_error()),
        crate::error::AppError::directory_read(StdPathBuf::from("missing"), io_error()),
        crate::error::AppError::command_execution("git status", io_error()),
        crate::error::AppError::command_failed("git status", Some(128), "not a repository"),
    ];
    rows.extend(
        app_errors
            .into_iter()
            .map(|error| error_row("AppError", error)),
    );

    assert_snapshot("error-codes", &pretty(&Value::Array(rows)));
}

fn error_row(origin: &str, error: crate::error::AppError) -> Value {
    json!({
        "origin": origin,
        "code": error.code(),
        "user_message": error.user_message(),
        "detail_message": error.detail_message(),
    })
}
