use ah_runtime::PluginManager;
use clap::{CommandFactory, Parser};

use super::*;

#[test]
fn builtin_catalogs_use_mcp_compatible_input_roots() {
    const ALLOWED_ROOT_KEYWORDS: &[&str] = &[
        "$schema",
        "$id",
        "$defs",
        "definitions",
        "title",
        "description",
        "default",
        "examples",
        "deprecated",
        "readOnly",
        "writeOnly",
        "type",
        "properties",
        "required",
        "additionalProperties",
    ];
    let mut manager = PluginManager::new();
    for plugin in builtins() {
        manager.register_builtin(plugin);
    }

    let commands = manager
        .list_enabled_commands()
        .expect("built-in typed catalogs should compile");

    assert!(!commands.is_empty());
    for command in commands {
        let root = command
            .descriptor
            .input_schema
            .as_object()
            .expect("input schema should have an object root");
        assert!(
            root.keys()
                .all(|keyword| ALLOWED_ROOT_KEYWORDS.contains(&keyword.as_str())),
            "{} has an incompatible input root",
            command.descriptor.id
        );
    }
}

#[test]
fn file_manual_examples_parse() {
    let manual = file_manual();
    assert_examples_parse::<FilePluginCli>(&manual);
}

#[test]
fn file_typed_read_uses_context_cwd_and_limit() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("sample.txt"), "alpha\nbeta\ngamma\n").unwrap();
    let mut manager = PluginManager::new();
    let file = builtins()
        .into_iter()
        .find(|plugin| plugin.metadata().domain == "file")
        .unwrap();
    manager.register_builtin(file);
    let response = manager
        .invoke_typed(&TypedInvocationRequest::new(
            "file.read",
            serde_json::json!({"path": "sample.txt", "number_lines": true}),
            ah_plugin_api::ExecutionContextWire::new(
                "file-test",
                temp.path().to_string_lossy(),
                Some(2),
                1_000,
            ),
        ))
        .unwrap();
    assert!(response.success);
    let data = response.data.unwrap();
    assert_eq!(data["command"], "file.read");
    assert_eq!(data["line_count"], 2);
    assert_eq!(data["truncated"], true);
    assert_eq!(data["content"], "   1: alpha\n   2: beta");
}

#[test]
fn file_typed_read_returns_structured_range_error() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("sample.txt"), "alpha\n").unwrap();
    let mut manager = PluginManager::new();
    let file = builtins()
        .into_iter()
        .find(|plugin| plugin.metadata().domain == "file")
        .unwrap();
    manager.register_builtin(file);
    let response = manager
        .invoke_typed(&TypedInvocationRequest::new(
            "file.read",
            serde_json::json!({"path": "sample.txt", "from": 3, "to": 2}),
            ah_plugin_api::ExecutionContextWire::new(
                "file-test-error",
                temp.path().to_string_lossy(),
                None,
                1_000,
            ),
        ))
        .unwrap();
    assert!(!response.success);
    let error = response.error.unwrap();
    assert_eq!(error.domain.as_deref(), Some("file"));
    assert_eq!(error.operation.as_deref(), Some("file.read"));
}

#[test]
fn search_manual_examples_parse() {
    let manual = search_manual();
    assert_examples_parse::<SearchPluginCli>(&manual);
}

#[test]
fn ctx_manual_examples_parse() {
    let manual = ctx_manual();
    assert_examples_parse::<CtxPluginCli>(&manual);
}

#[test]
fn ctx_typed_symbols_uses_explicit_context_cwd() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("sample.rs"), "fn sample() {}\n").unwrap();
    let mut manager = PluginManager::new();
    let ctx = builtins()
        .into_iter()
        .find(|plugin| plugin.metadata().domain == "ctx")
        .unwrap();
    manager.register_builtin(ctx);
    let response = manager
        .invoke_typed(&TypedInvocationRequest::new(
            "ctx.symbols",
            serde_json::json!({"path": "sample.rs", "preset": "summary"}),
            ah_plugin_api::ExecutionContextWire::new(
                "ctx-test",
                temp.path().to_string_lossy(),
                None,
                1_000,
            ),
        ))
        .unwrap();
    assert!(response.success);
    let data = response.data.unwrap();
    assert_eq!(data["command"], "ctx.symbols");
    assert_eq!(data["symbol_count"], 1);
    assert!(data["root"].as_str().unwrap().ends_with("sample.rs"));
}

#[test]
fn ctx_typed_symbols_rejects_missing_path() {
    let mut manager = PluginManager::new();
    let ctx = builtins()
        .into_iter()
        .find(|plugin| plugin.metadata().domain == "ctx")
        .unwrap();
    manager.register_builtin(ctx);
    let error = manager
        .invoke_typed(&TypedInvocationRequest::new(
            "ctx.symbols",
            serde_json::json!({}),
            ah_plugin_api::ExecutionContextWire::new("ctx-test-invalid", ".", None, 1_000),
        ))
        .unwrap_err();
    assert!(matches!(
        error,
        ah_runtime::RuntimeError::TypedInvocation(_)
    ));
}

#[test]
fn git_manual_examples_parse() {
    let manual = git_manual();
    assert_examples_parse::<GitPluginCli>(&manual);
}

#[test]
fn git_typed_status_uses_explicit_context_cwd() {
    if !git_is_available() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let mut manager = PluginManager::new();
    let git = builtins()
        .into_iter()
        .find(|plugin| plugin.metadata().domain == "git")
        .unwrap();
    manager.register_builtin(git);
    let response = manager
        .invoke_typed(&TypedInvocationRequest::new(
            "git.status",
            serde_json::json!({}),
            ah_plugin_api::ExecutionContextWire::new(
                "git-status-test",
                temp.path().to_string_lossy(),
                None,
                2_000,
            ),
        ))
        .unwrap();
    assert!(response.success);
    let data = response.data.unwrap();
    assert_eq!(data["command"], "git.status");
    assert_eq!(data["in_git_repo"], false);
}

#[test]
fn git_typed_tag_create_mutates_context_repository() {
    if !git_is_available() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    run_git(temp.path(), &["init"]);
    run_git(temp.path(), &["config", "user.email", "test@example.com"]);
    run_git(temp.path(), &["config", "user.name", "Test User"]);
    std::fs::write(temp.path().join("sample.txt"), "sample\n").unwrap();
    run_git(temp.path(), &["add", "sample.txt"]);
    run_git(temp.path(), &["commit", "-m", "initial"]);

    let mut manager = PluginManager::new();
    let git = builtins()
        .into_iter()
        .find(|plugin| plugin.metadata().domain == "git")
        .unwrap();
    manager.register_builtin(git);
    let response = manager
        .invoke_typed(&TypedInvocationRequest::new(
            "git.tag.create",
            serde_json::json!({"tag": "v-test", "message": "test tag"}),
            ah_plugin_api::ExecutionContextWire::new(
                "git-tag-test",
                temp.path().to_string_lossy(),
                None,
                2_000,
            ),
        ))
        .unwrap();
    assert!(response.success);
    let data = response.data.unwrap();
    assert_eq!(data["tag"], "v-test");
    assert_eq!(data["annotated"], true);
    let output = ah_plugin_api::noninteractive_command("git")
        .current_dir(temp.path())
        .args(["tag", "--list", "v-test"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "v-test");
}

#[test]
fn project_manual_examples_parse() {
    let manual = project_manual();
    assert_examples_parse::<ProjectPluginCli>(&manual);
}

#[test]
fn project_typed_detect_uses_explicit_context_cwd() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(
        temp.path().join("Cargo.toml"),
        "[package]\nname = \"sample\"\nversion = \"1.2.3\"\n",
    )
    .unwrap();
    let mut manager = PluginManager::new();
    let project = builtins()
        .into_iter()
        .find(|plugin| plugin.metadata().domain == "project")
        .unwrap();
    manager.register_builtin(project);
    let response = manager
        .invoke_typed(&TypedInvocationRequest::new(
            "project.detect",
            serde_json::json!({}),
            ah_plugin_api::ExecutionContextWire::new(
                "project-detect-test",
                temp.path().to_string_lossy(),
                None,
                2_000,
            ),
        ))
        .unwrap();
    assert!(response.success);
    let data = response.data.unwrap();
    assert_eq!(data["command"], "project.detect");
    assert!(
        data["ecosystems"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value == "rust")
    );
    assert!(
        data["versions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value["version"] == "1.2.3")
    );
}

#[test]
fn run_manual_examples_parse() {
    let manual = run_manual();
    assert_examples_parse::<RunPluginCli>(&manual);
}

#[test]
fn run_typed_check_runs_in_explicit_context_cwd() {
    let temp = tempfile::tempdir().unwrap();
    let command = if cfg!(windows) {
        serde_json::json!(["cmd", "/C", "cd"])
    } else {
        serde_json::json!(["pwd"])
    };
    let mut manager = PluginManager::new();
    let run = builtins()
        .into_iter()
        .find(|plugin| plugin.metadata().domain == "run")
        .unwrap();
    manager.register_builtin(run);
    let response = manager
        .invoke_typed(&TypedInvocationRequest::new(
            "run.check",
            serde_json::json!({"command": command}),
            ah_plugin_api::ExecutionContextWire::new(
                "run-check-test",
                temp.path().to_string_lossy(),
                None,
                5_000,
            ),
        ))
        .unwrap();
    assert!(response.success);
    let data = response.data.unwrap();
    assert_eq!(data["success"], true);
    let reported = std::path::PathBuf::from(data["stdout"].as_str().unwrap().trim());
    assert_eq!(
        std::fs::canonicalize(reported).unwrap(),
        std::fs::canonicalize(temp.path()).unwrap()
    );
}

#[test]
fn search_typed_text_uses_explicit_context_cwd_and_limit() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(
        temp.path().join("sample.txt"),
        "needle one\nother\nneedle two\n",
    )
    .unwrap();
    let mut manager = PluginManager::new();
    let search = builtins()
        .into_iter()
        .find(|plugin| plugin.metadata().domain == "search")
        .unwrap();
    manager.register_builtin(search);
    let response = manager
        .invoke_typed(&TypedInvocationRequest::new(
            "search.text",
            serde_json::json!({"pattern": "needle"}),
            ah_plugin_api::ExecutionContextWire::new(
                "search-text-test",
                temp.path().to_string_lossy(),
                Some(1),
                2_000,
            ),
        ))
        .unwrap();
    assert!(response.success);
    let data = response.data.unwrap();
    assert_eq!(data["command"], "search.text");
    assert_eq!(data["match_count"], 1);
    assert_eq!(data["truncated"], true);
    assert_eq!(data["matches"][0]["path"], "sample.txt");
}

#[test]
fn http_manual_examples_parse() {
    let manual = http_manual();
    assert_examples_parse::<HttpPluginCli>(&manual);
}

#[test]
fn http_typed_get_returns_valid_structured_response() {
    let (url, server) = serve_http_once(200, r#"{"status":"ok"}"#);
    let mut manager = PluginManager::new();
    let http = builtins()
        .into_iter()
        .find(|plugin| plugin.metadata().domain == "http")
        .unwrap();
    manager.register_builtin(http);
    let response = manager
        .invoke_typed(&TypedInvocationRequest::new(
            "http.get",
            serde_json::json!({
                "url": url,
                "expect_status": "200",
                "expect_json": ["status:eq:ok"]
            }),
            ah_plugin_api::ExecutionContextWire::new("http-get-test", ".", None, 2_000),
        ))
        .unwrap();
    server.join().unwrap();
    assert!(response.success);
    let data = response.data.unwrap();
    assert_eq!(data["command"], "http.get");
    assert_eq!(data["status"], 200);
    assert_eq!(data["ok"], true);
}

#[test]
fn http_typed_expectation_failure_is_structured_error() {
    let (url, server) = serve_http_once(200, "ok");
    let mut manager = PluginManager::new();
    let http = builtins()
        .into_iter()
        .find(|plugin| plugin.metadata().domain == "http")
        .unwrap();
    manager.register_builtin(http);
    let response = manager
        .invoke_typed(&TypedInvocationRequest::new(
            "http.get",
            serde_json::json!({"url": url, "expect_status": "201"}),
            ah_plugin_api::ExecutionContextWire::new("http-get-error-test", ".", None, 2_000),
        ))
        .unwrap();
    server.join().unwrap();
    assert!(!response.success);
    assert_eq!(response.error.unwrap().code, "HTTP_ASSERTION_FAILED");
}

#[test]
fn http_mcp_max_timeout_expectation_failure_is_structured_error() {
    let (url, server) = serve_http_once(200, "ok");
    let mut manager = PluginManager::new();
    let http = builtins()
        .into_iter()
        .find(|plugin| plugin.metadata().domain == "http")
        .unwrap();
    manager.register_builtin(http);
    let response = manager
        .invoke_typed(&TypedInvocationRequest::new(
            "http.get",
            serde_json::json!({"url": url, "expect_status": "201"}),
            ah_plugin_api::ExecutionContextWire::new(
                "http-mcp-max-timeout-error-test",
                ".",
                None,
                u64::MAX,
            ),
        ))
        .unwrap();
    server.join().unwrap();
    assert!(!response.success);
    assert_eq!(response.error.unwrap().code, "HTTP_ASSERTION_FAILED");
}

#[test]
fn task_manual_examples_parse() {
    let manual = task_manual();
    assert_examples_parse::<TaskPluginCli>(&manual);
}

#[test]
fn task_typed_save_and_list_use_explicit_context_cwd() {
    let temp = tempfile::tempdir().unwrap();
    let mut manager = PluginManager::new();
    let task = builtins()
        .into_iter()
        .find(|plugin| plugin.metadata().domain == "task")
        .unwrap();
    manager.register_builtin(task);
    let context = || {
        ah_plugin_api::ExecutionContextWire::new(
            "task-test",
            temp.path().to_string_lossy(),
            None,
            2_000,
        )
    };
    let saved = manager
        .invoke_typed(&TypedInvocationRequest::new(
            "task.save",
            serde_json::json!({"name": "check", "command": "cargo check"}),
            context(),
        ))
        .unwrap();
    assert!(saved.success);
    assert!(temp.path().join(".ah").join("tasks.json").is_file());

    let listed = manager
        .invoke_typed(&TypedInvocationRequest::new(
            "task.list",
            serde_json::json!({}),
            context(),
        ))
        .unwrap();
    assert!(listed.success);
    let data = listed.data.unwrap();
    assert_eq!(data["count"], 1);
    assert_eq!(data["tasks"][0]["name"], "check");
    assert_eq!(data["tasks"][0]["command"], "cargo check");
}

/// Check a hand-written manual against the parser it documents.
///
/// The manual is CLI-shaped prose that an agent reads before it picks a
/// command, and nothing in the type system ties it to the clap command it
/// describes. Every example already had to parse; a documented command that
/// no longer exists, or a `usage` line naming a flag that was renamed away,
/// used to go unnoticed.
///
/// Wording is deliberately not compared: the manual `summary` and the clap
/// `about` say the same thing differently on purpose.
fn assert_examples_parse<T>(manual: &PluginManual)
where
    T: Parser + CommandFactory,
{
    let root: clap::Command = T::command();
    for command in &manual.commands {
        let documented = resolve_subcommand(&root, &command.name);
        assert!(
            documented.is_some(),
            "manual documents '{} {}', which the CLI does not have",
            manual.domain,
            command.name
        );
        if let Some(documented) = documented {
            for flag in usage_flags(&command.usage) {
                assert!(
                    accepts_flag(documented, &flag),
                    "manual usage for '{} {}' names {flag}, which the command does not accept",
                    manual.domain,
                    command.name
                );
            }
        }

        for example in &command.examples {
            let mut args = Vec::with_capacity(example.argv.len() + 1);
            args.push(manual.domain.clone());
            args.extend(example.argv.iter().cloned());
            let parse_result = T::try_parse_from(args.clone());
            assert!(
                parse_result.is_ok(),
                "manual example failed to parse for domain '{}', command '{}': argv={:?}",
                manual.domain,
                command.name,
                args
            );
        }
    }
}

/// Walk a documented subcommand path, which may be nested like `tag create`.
fn resolve_subcommand<'a>(root: &'a clap::Command, name: &str) -> Option<&'a clap::Command> {
    let mut command = root;
    for segment in name.split_whitespace() {
        command = command.find_subcommand(segment)?;
    }
    Some(command)
}

/// Pull `--from` and `-n` out of a usage line, ignoring `<path>`, `N`, `BYTES`.
fn usage_flags(usage: &str) -> Vec<String> {
    usage
        .split_whitespace()
        .map(|token| token.trim_matches(['[', ']', '<', '>', '|', ',']))
        .filter(|token| token.starts_with('-') && token.len() > 1)
        .map(str::to_owned)
        .collect()
}

/// Global flags live on the root CLI, not on the per-domain parser.
const GLOBAL_FLAGS: &[&str] = &["--json", "--quiet", "--cwd", "--limit"];

fn accepts_flag(command: &clap::Command, flag: &str) -> bool {
    if GLOBAL_FLAGS.contains(&flag) {
        return true;
    }
    if let Some(long) = flag.strip_prefix("--") {
        return command
            .get_arguments()
            .any(|argument| argument.get_long() == Some(long));
    }
    flag.strip_prefix('-')
        .and_then(|rest| rest.chars().next())
        .is_some_and(|short| {
            command
                .get_arguments()
                .any(|argument| argument.get_short() == Some(short))
        })
}

fn git_is_available() -> bool {
    ah_plugin_api::noninteractive_command("git")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

fn run_git(cwd: &std::path::Path, args: &[&str]) {
    let status = ah_plugin_api::noninteractive_command("git")
        .current_dir(cwd)
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {} failed", args.join(" "));
}

fn serve_http_once(status: u16, body: &'static str) -> (String, std::thread::JoinHandle<()>) {
    use std::io::{Read, Write};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0u8; 4096];
        let _ = stream.read(&mut request);
        let reason = if status == 200 { "OK" } else { "Error" };
        let response = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).unwrap();
    });
    (format!("http://{address}/test"), server)
}
