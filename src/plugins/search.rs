//! The `search` builtin: its CLI shape, its metadata, its manual, and the
//! `BuiltinPlugin` impl that ties them to `commands::search`.

use super::*;

#[derive(Debug, Parser)]
pub(super) struct SearchPluginCli {
    #[command(flatten)]
    pub(super) args: commands::search::SearchArgs,
}

pub(super) struct SearchBuiltinPlugin;

pub(super) fn search_metadata() -> PluginMetadata {
    PluginMetadata {
        plugin_name: "builtin-search".to_owned(),
        domain: "search".to_owned(),
        description: "Search operations plugin (built-in)".to_owned(),
        abi_version: 1,
        required_tools: Vec::new(),
        compatibility: PluginCompatibility::current()
            .with_capability(plugin_capabilities::TYPED_COMMANDS_V1),
    }
}

pub(super) fn search_manual() -> PluginManual {
    PluginManual {
        plugin_name: search_metadata().plugin_name,
        domain: "search".to_owned(),
        description: "Find text matches and file paths quickly.".to_owned(),
        commands: vec![
            ManualCommand {
                name: "text".to_owned(),
                summary: "Search text in files (literal by default, regex with --regex).".to_owned(),
                usage: "text <pattern> [path...] [--glob ...] [--ignore-case] [--context N] [--regex] [--max-bytes BYTES] [--follow-symlinks]".to_owned(),
                examples: vec![
                    ManualExample::new(
                        "Literal search in Rust files",
                        &["text", "PluginManager", "src", "--glob", "*.rs", "--context", "1"],
                    ),
                    ManualExample::new(
                        "Regex search for function declarations",
                        &["text", "fn\\s+execute", "src", "--regex", "--context", "2"],
                    ),
                ],
            },
            ManualCommand {
                name: "files".to_owned(),
                summary: "Find file paths containing substring.".to_owned(),
                usage: "files <query> [path...] [--follow-symlinks]".to_owned(),
                examples: vec![ManualExample::new(
                    "Find docs related to plugins",
                    &["files", "plugin", "docs"],
                )],
            },
        ],
        notes: vec![
            "Binary and oversized files are skipped by policy.".to_owned(),
            "Use --regex only when needed; literal mode is usually faster.".to_owned(),
        ],
    }
}

impl BuiltinPlugin for SearchBuiltinPlugin {
    fn metadata(&self) -> PluginMetadata {
        search_metadata()
    }

    fn manual(&self) -> PluginManual {
        search_manual()
    }

    fn invoke(&self, request: &InvocationRequest) -> InvocationResponse {
        self.invoke_into(request, &OutputSink::Process)
    }

    fn invoke_into(&self, request: &InvocationRequest, sink: &OutputSink) -> InvocationResponse {
        let (parsed, options) =
            match parse_args::<SearchPluginCli>("search", &request.argv, request.globals.clone()) {
                ParseOutcome::Parsed(value, options) => (value, options),
                ParseOutcome::Response(response) => return response,
            };
        map_execute(
            "search",
            commands::search::execute(parsed.args, &options, sink),
        )
    }

    fn command_catalog(&self) -> Option<CommandCatalog> {
        Some(commands::search::command_catalog())
    }

    fn invoke_typed(&self, request: &TypedInvocationRequest) -> TypedInvocationResponse {
        commands::search::invoke_typed(request)
    }

    fn cancel_typed(&self, request_id: &str) -> bool {
        ah_plugin_api::cancellation::cancel(request_id)
    }
}
