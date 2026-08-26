//! The `http` builtin: its CLI shape, its metadata, its manual, and the
//! `BuiltinPlugin` impl that ties them to `commands::http`.

use super::*;

#[derive(Debug, Parser)]
pub(super) struct HttpPluginCli {
    #[command(flatten)]
    pub(super) args: commands::http::HttpArgs,
}

pub(super) struct HttpBuiltinPlugin;

pub(super) fn http_metadata() -> PluginMetadata {
    PluginMetadata {
        plugin_name: "builtin-http".to_owned(),
        domain: "http".to_owned(),
        description: "HTTP workflow plugin (built-in)".to_owned(),
        abi_version: 1,
        required_tools: Vec::new(),
        compatibility: PluginCompatibility::current()
            .with_capability(plugin_capabilities::TYPED_COMMANDS_V1),
    }
}

pub(super) fn http_manual() -> PluginManual {
    PluginManual {
        plugin_name: http_metadata().plugin_name,
        domain: "http".to_owned(),
        description: "HTTP request and API assertion helpers.".to_owned(),
        commands: vec![
            ManualCommand {
                name: "request".to_owned(),
                summary: "Send HTTP request with explicit method.".to_owned(),
                usage: "request --method <METHOD> <url> [--header \"K: V\"] [--query \"KEY=VALUE\"] [--timeout-secs N] [--bearer TOKEN] [--basic USER:PASS] [--json <JSON>|--json-file <PATH>] [--body <TEXT>|--body-file <PATH>] [--expect-status <code|range>] [--expect-header \"K: V\"] [--expect-body-contains <TEXT>] [--expect-json <PATH:OP[:VALUE]>]".to_owned(),
                examples: vec![ManualExample::new(
                    "Basic request with status check",
                    &["request", "--method", "GET", "https://example.com/health", "--expect-status", "200"],
                )],
            },
            ManualCommand {
                name: "get".to_owned(),
                summary: "Shortcut for GET request.".to_owned(),
                usage: "get <url> [request/expect flags]".to_owned(),
                examples: vec![ManualExample::new(
                    "GET JSON endpoint",
                    &["get", "https://example.com/api/version", "--expect-status", "2xx"],
                )],
            },
            ManualCommand {
                name: "post".to_owned(),
                summary: "Shortcut for POST request.".to_owned(),
                usage: "post <url> [request/expect flags]".to_owned(),
                examples: vec![ManualExample::new(
                    "POST JSON payload",
                    &["post", "https://example.com/api/items", "--json", "{\"name\":\"demo\"}", "--expect-status", "201"],
                )],
            },
            ManualCommand {
                name: "replay".to_owned(),
                summary: "Replay supported curl command form.".to_owned(),
                usage: "replay --curl \"<curl ...>\" [request/expect flags]".to_owned(),
                examples: vec![ManualExample::new(
                    "Replay existing curl command",
                    &[
                        "replay",
                        "--curl",
                        "curl -X GET https://example.com/health -H 'accept: application/json'",
                    ],
                )],
            },
            ManualCommand {
                name: "assert".to_owned(),
                summary: "Run API assertions from YAML/JSON spec.".to_owned(),
                usage: "assert <spec-path> [--var KEY=VALUE ...] [--fail-fast] [--report text|json|junit]".to_owned(),
                examples: vec![ManualExample::new(
                    "Run assertions with machine output",
                    &["assert", "api/health.yaml", "--report", "json"],
                )],
            },
            ManualCommand {
                name: "run".to_owned(),
                summary: "Alias for assert.".to_owned(),
                usage: "run <spec-path> [--var KEY=VALUE ...] [--fail-fast] [--report text|json|junit]".to_owned(),
                examples: vec![ManualExample::new(
                    "Alias usage",
                    &["run", "api/health.yaml", "--fail-fast"],
                )],
            },
        ],
        notes: vec![
            "Spec format is YAML-first with JSON compatibility.".to_owned(),
            "Global --json maps assert/run report format to json.".to_owned(),
            "Retries and atomic cross-case extract variables are supported.".to_owned(),
        ],
    }
}

impl BuiltinPlugin for HttpBuiltinPlugin {
    fn metadata(&self) -> PluginMetadata {
        http_metadata()
    }

    fn manual(&self) -> PluginManual {
        http_manual()
    }

    fn invoke(&self, request: &InvocationRequest) -> InvocationResponse {
        let (mut parsed, options) =
            match parse_args::<HttpPluginCli>("http", &request.argv, request.globals.clone()) {
                ParseOutcome::Parsed(value, options) => (value, options),
                ParseOutcome::Response(response) => return response,
            };
        if let Err(error) =
            commands::http::bind_resolved_credentials(&mut parsed.args, &request.resolved_secrets)
        {
            return error;
        }
        map_execute("http", commands::http::execute(parsed.args, &options))
    }

    fn command_catalog(&self) -> Option<CommandCatalog> {
        Some(commands::http::command_catalog())
    }

    fn invoke_typed(&self, request: &TypedInvocationRequest) -> TypedInvocationResponse {
        commands::http::invoke_typed(request)
    }
}
