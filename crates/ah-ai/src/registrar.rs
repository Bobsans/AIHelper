use std::{
    io,
    path::{Path, PathBuf},
    process::Stdio,
};

use serde_json::Value;

use ah_error::AppError;

use super::{
    json_config, opencode_config,
    targets::{CliStyle, ProbeKind, Scope, ServerSpec, Target, home_dir},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub program: String,
    pub args: Vec<String>,
}

impl Invocation {
    pub fn render(&self) -> String {
        let mut rendered = String::from(&self.program);
        for argument in &self.args {
            rendered.push(' ');
            if argument.contains(' ') {
                rendered.push('"');
                rendered.push_str(argument);
                rendered.push('"');
            } else {
                rendered.push_str(argument);
            }
        }
        rendered
    }
}

fn owned(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

pub fn add_invocation(target: &Target, scope: Scope, name: &str, spec: &ServerSpec) -> Invocation {
    let program = target
        .cli_program()
        .expect("a CLI-registered target always has a program");
    let style = target
        .cli_style()
        .expect("a CLI-registered target always has a style");
    let mut args = owned(&["mcp", "add"]);
    match style {
        CliStyle::Claude => {
            args.extend(owned(&["-s", scope.as_str()]));
            match spec {
                ServerSpec::Http { url } => {
                    args.extend(owned(&["-t", "http"]));
                    args.push(name.to_owned());
                    args.push(url.clone());
                }
                ServerSpec::Stdio {
                    command,
                    args: rest,
                } => {
                    args.push(name.to_owned());
                    args.push("--".to_owned());
                    args.push(command.clone());
                    args.extend(rest.iter().cloned());
                }
            }
        }
        // Gemini takes the command or URL as a positional argument and has no
        // `--` separator; extra arguments follow directly.
        CliStyle::Gemini => {
            args.extend(owned(&["-s", scope.as_str()]));
            match spec {
                ServerSpec::Http { url } => {
                    args.extend(owned(&["-t", "http"]));
                    args.push(name.to_owned());
                    args.push(url.clone());
                }
                ServerSpec::Stdio {
                    command,
                    args: rest,
                } => {
                    args.push(name.to_owned());
                    args.push(command.clone());
                    args.extend(rest.iter().cloned());
                }
            }
        }
        CliStyle::Codex => {
            args.push(name.to_owned());
            match spec {
                ServerSpec::Http { url } => {
                    args.push("--url".to_owned());
                    args.push(url.clone());
                }
                ServerSpec::Stdio {
                    command,
                    args: rest,
                } => {
                    args.push("--".to_owned());
                    args.push(command.clone());
                    args.extend(rest.iter().cloned());
                }
            }
        }
    }
    Invocation {
        program: program.to_owned(),
        args,
    }
}

pub fn remove_invocation(target: &Target, scope: Scope, name: &str) -> Invocation {
    let program = target
        .cli_program()
        .expect("a CLI-registered target always has a program");
    let mut args = owned(&["mcp", "remove"]);
    if matches!(
        target.cli_style(),
        Some(CliStyle::Claude) | Some(CliStyle::Gemini)
    ) {
        args.extend(owned(&["-s", scope.as_str()]));
    }
    args.push(name.to_owned());
    Invocation {
        program: program.to_owned(),
        args,
    }
}

fn missing(program: &str) -> AppError {
    AppError::external(
        "AI_AGENT_CLI_MISSING",
        format!("`{program}` was not found on PATH; install the agent or use --rules-only"),
    )
}

/// Agent CLIs are usually npm shims, which on Windows exist only as `.cmd`
/// files. `Command` searches PATH for the bare name and `.exe` alone, so the
/// PATHEXT candidates have to be resolved before spawning.
fn resolve_program(program: &str) -> Option<PathBuf> {
    let candidate = Path::new(program);
    if candidate.components().count() > 1 {
        return candidate.is_file().then(|| candidate.to_path_buf());
    }
    ah_platform::exec::find_executable(program, &[])
}

pub fn available(program: &str) -> bool {
    resolve_program(program).is_some()
}

pub fn run(invocation: &Invocation) -> Result<String, AppError> {
    let program =
        resolve_program(&invocation.program).ok_or_else(|| missing(&invocation.program))?;
    let output = ah_plugin_api::noninteractive_command(&program)
        .args(&invocation.args)
        .stdin(Stdio::null())
        .output()
        .map_err(|source| {
            if source.kind() == io::ErrorKind::NotFound {
                return missing(&invocation.program);
            }
            AppError::external(
                "AI_AGENT_CLI_FAILED",
                format!("failed to run `{}`: {source}", invocation.render()),
            )
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        let code = output
            .status
            .code()
            .map(|code| code.to_string())
            .unwrap_or_else(|| "signal".to_owned());
        return Err(AppError::external(
            "AI_AGENT_CLI_FAILED",
            format!(
                "`{}` exited with status {code}: {stderr}",
                invocation.render()
            ),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub fn probe(
    target: &Target,
    scope: Scope,
    name: &str,
    project_root: &Path,
) -> Result<Option<ServerSpec>, AppError> {
    match target.probe {
        ProbeKind::ClaudeConfig => probe_claude_config(scope, name, project_root),
        ProbeKind::CodexList => Ok(probe_codex_list(target, &[name])?
            .into_iter()
            .next()
            .flatten()),
        ProbeKind::Json => {
            let config = target.json_config()?;
            let path = config.path(scope, project_root)?;
            Ok(json_config::read(&path)?
                .and_then(|document| json_config::lookup(&document, &config, name)))
        }
        ProbeKind::OpenCode => opencode_config::lookup(scope, project_root, name),
    }
}

fn probe_claude_config(
    scope: Scope,
    name: &str,
    project_root: &Path,
) -> Result<Option<ServerSpec>, AppError> {
    let path = claude_config_path(scope, project_root)?;
    let Some(document) = read_json(&path)? else {
        return Ok(None);
    };
    let servers = match scope {
        Scope::System | Scope::Project | Scope::User => document.get("mcpServers"),
        Scope::Local => {
            project_entry(&document, project_root).and_then(|entry| entry.get("mcpServers"))
        }
    };
    Ok(servers
        .and_then(|servers| servers.get(name))
        .and_then(claude_spec))
}

pub fn claude_config_path(scope: Scope, project_root: &Path) -> Result<PathBuf, AppError> {
    match scope {
        Scope::Project => Ok(project_root.join(".mcp.json")),
        Scope::Local | Scope::User => Ok(home_dir()?.join(".claude.json")),
        Scope::System => Err(AppError::external(
            "AI_TARGET_SCOPE_UNSUPPORTED",
            "managed Claude MCP is read only by ai status",
        )),
    }
}

/// Claude records project keys with the separator style that was current when
/// the project was first opened, so both spellings must be accepted.
fn project_entry<'a>(document: &'a Value, project_root: &Path) -> Option<&'a Value> {
    let wanted = normalize_path_key(&project_root.display().to_string());
    document
        .get("projects")?
        .as_object()?
        .iter()
        .find(|(key, _)| normalize_path_key(key) == wanted)
        .map(|(_, value)| value)
}

fn normalize_path_key(value: &str) -> String {
    let normalized = value.replace('\\', "/").trim_end_matches('/').to_owned();
    if cfg!(windows) {
        normalized.to_lowercase()
    } else {
        normalized
    }
}

fn claude_spec(entry: &Value) -> Option<ServerSpec> {
    if let Some(url) = entry.get("url").and_then(Value::as_str) {
        return Some(ServerSpec::Http {
            url: url.to_owned(),
        });
    }
    let command = entry.get("command").and_then(Value::as_str)?;
    Some(ServerSpec::Stdio {
        command: command.to_owned(),
        args: string_list(entry.get("args")),
    })
}

pub fn probe_codex_list(
    target: &Target,
    names: &[&str],
) -> Result<Vec<Option<ServerSpec>>, AppError> {
    let invocation = Invocation {
        program: target
            .cli_program()
            .expect("codex is a CLI-registered target")
            .to_owned(),
        args: owned(&["mcp", "list", "--json"]),
    };
    let stdout = run(&invocation)?;
    let servers: Value = serde_json::from_str(&stdout).map_err(|source| {
        AppError::external(
            "AI_AGENT_CLI_FAILED",
            format!(
                "`{}` returned unreadable JSON: {source}",
                invocation.render()
            ),
        )
    })?;
    let servers = servers.as_array();
    Ok(names
        .iter()
        .map(|name| {
            servers
                .and_then(|servers| {
                    servers
                        .iter()
                        .find(|server| server.get("name").and_then(Value::as_str) == Some(*name))
                })
                .and_then(codex_spec)
        })
        .collect())
}

fn codex_spec(entry: &Value) -> Option<ServerSpec> {
    let transport = entry.get("transport")?;
    match transport.get("type").and_then(Value::as_str)? {
        "streamable_http" | "http" | "sse" => Some(ServerSpec::Http {
            url: transport.get("url").and_then(Value::as_str)?.to_owned(),
        }),
        "stdio" => Some(ServerSpec::Stdio {
            command: transport.get("command").and_then(Value::as_str)?.to_owned(),
            args: string_list(transport.get("args")),
        }),
        _ => None,
    }
}

fn string_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn read_json(path: &Path) -> Result<Option<Value>, AppError> {
    let contents = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(AppError::file_read(path.to_path_buf(), source)),
    };
    serde_json::from_str(&contents).map(Some).map_err(|source| {
        AppError::external(
            "AI_CONFIG_UNPARSABLE",
            format!("{} is not valid JSON: {source}", path.display()),
        )
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        Invocation, ServerSpec, add_invocation, claude_spec, codex_spec, project_entry,
        remove_invocation, run,
    };
    use crate::targets::SERVER_NAME;
    use crate::targets::{Scope, find};

    fn stdio() -> ServerSpec {
        ServerSpec::Stdio {
            command: "C:\\bin\\ah.exe".to_owned(),
            args: vec!["mcp".to_owned(), "serve".to_owned()],
        }
    }

    fn http() -> ServerSpec {
        ServerSpec::Http {
            url: "http://127.0.0.1:8787/mcp".to_owned(),
        }
    }

    #[test]
    fn claude_stdio_uses_the_argument_separator_after_the_server_name() {
        let claude = find("claude").expect("claude target exists");
        assert_eq!(
            add_invocation(claude, Scope::Project, SERVER_NAME, &stdio()),
            Invocation {
                program: "claude".to_owned(),
                args: vec![
                    "mcp",
                    "add",
                    "-s",
                    "project",
                    SERVER_NAME,
                    "--",
                    "C:\\bin\\ah.exe",
                    "mcp",
                    "serve"
                ]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            }
        );
    }

    #[test]
    fn claude_http_passes_the_url_as_a_positional_argument() {
        let claude = find("claude").expect("claude target exists");
        let invocation = add_invocation(claude, Scope::User, SERVER_NAME, &http());
        assert_eq!(
            invocation.args,
            vec![
                "mcp",
                "add",
                "-s",
                "user",
                "-t",
                "http",
                SERVER_NAME,
                "http://127.0.0.1:8787/mcp"
            ]
        );
    }

    #[test]
    fn codex_omits_the_scope_flag_and_uses_url_option() {
        let codex = find("codex").expect("codex target exists");
        assert_eq!(
            add_invocation(codex, Scope::User, SERVER_NAME, &http()).args,
            vec![
                "mcp",
                "add",
                SERVER_NAME,
                "--url",
                "http://127.0.0.1:8787/mcp"
            ]
        );
        assert_eq!(
            add_invocation(codex, Scope::User, SERVER_NAME, &stdio()).args,
            vec![
                "mcp",
                "add",
                SERVER_NAME,
                "--",
                "C:\\bin\\ah.exe",
                "mcp",
                "serve"
            ]
        );
        assert_eq!(
            remove_invocation(codex, Scope::User, SERVER_NAME).args,
            vec!["mcp", "remove", SERVER_NAME]
        );
    }

    #[test]
    fn claude_remove_carries_the_scope() {
        let claude = find("claude").expect("claude target exists");
        assert_eq!(
            remove_invocation(claude, Scope::Local, SERVER_NAME).args,
            vec!["mcp", "remove", "-s", "local", SERVER_NAME]
        );
    }

    #[test]
    fn claude_entries_map_both_transports() {
        assert_eq!(
            claude_spec(&json!({"type": "http", "url": "http://127.0.0.1:8787/mcp"})),
            Some(http())
        );
        assert_eq!(
            claude_spec(&json!({"command": "C:\\bin\\ah.exe", "args": ["mcp", "serve"]})),
            Some(stdio())
        );
        assert_eq!(claude_spec(&json!({"type": "http"})), None);
    }

    #[test]
    fn codex_entries_map_both_transports() {
        assert_eq!(
            codex_spec(&json!({
                "transport": {"type": "streamable_http", "url": "http://127.0.0.1:8787/mcp"}
            })),
            Some(http())
        );
        assert_eq!(
            codex_spec(&json!({
                "transport": {"type": "stdio", "command": "C:\\bin\\ah.exe", "args": ["mcp", "serve"]}
            })),
            Some(stdio())
        );
    }

    #[test]
    fn a_missing_agent_binary_is_reported_as_such_rather_than_as_a_failure() {
        let error = run(&Invocation {
            program: "ah-agent-that-does-not-exist".to_owned(),
            args: vec!["mcp".to_owned(), "list".to_owned()],
        })
        .expect_err("a missing binary must not look like a successful probe");
        assert_eq!(error.code(), "AI_AGENT_CLI_MISSING");
        assert!(error.detail_message().contains("--rules-only"));
    }

    #[test]
    fn gemini_takes_the_command_positionally_with_no_separator() {
        let gemini = find("gemini").expect("gemini target exists");
        assert_eq!(
            add_invocation(gemini, Scope::Project, SERVER_NAME, &stdio()).args,
            vec![
                "mcp",
                "add",
                "-s",
                "project",
                SERVER_NAME,
                "C:\\bin\\ah.exe",
                "mcp",
                "serve"
            ],
            "gemini has no `--` separator; a stray one would become a server argument"
        );
        assert_eq!(
            add_invocation(gemini, Scope::User, SERVER_NAME, &http()).args,
            vec![
                "mcp",
                "add",
                "-s",
                "user",
                "-t",
                "http",
                SERVER_NAME,
                "http://127.0.0.1:8787/mcp"
            ]
        );
        assert_eq!(
            remove_invocation(gemini, Scope::Project, SERVER_NAME).args,
            vec!["mcp", "remove", "-s", "project", SERVER_NAME]
        );
    }

    #[test]
    fn a_file_backed_target_has_no_cli_program() {
        for name in ["cursor", "copilot", "opencode"] {
            let target = find(name).expect("target exists");
            assert!(
                target.cli_program().is_none(),
                "{name} is written as JSON and has no MCP CLI"
            );
        }
        for name in ["claude", "codex", "gemini"] {
            let target = find(name).expect("target exists");
            assert!(target.cli_program().is_some(), "{name} registers via a CLI");
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_resolution_finds_shims_that_are_not_exe_files() {
        use super::resolve_program;

        let resolved = resolve_program("cmd").expect("cmd is always on PATH");
        assert_eq!(
            resolved
                .extension()
                .and_then(|extension| extension.to_str())
                .map(str::to_lowercase),
            Some("exe".to_owned())
        );
        assert!(resolve_program("ah-agent-that-does-not-exist").is_none());
    }

    #[test]
    fn project_lookup_accepts_either_path_separator() {
        let document = json!({
            "projects": {
                "D:\\Work\\DarkBoy\\AIHelper": {"mcpServers": {SERVER_NAME: {"command": "x"}}}
            }
        });
        let entry = project_entry(&document, std::path::Path::new("D:/Work/DarkBoy/AIHelper"));
        assert!(
            entry.is_some(),
            "forward-slash root should match a backslash key"
        );
    }
}
