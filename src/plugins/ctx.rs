//! The `ctx` builtin: its CLI shape, its metadata, its manual, and the
//! `BuiltinPlugin` impl that ties them to `commands::ctx`.

use super::*;

#[derive(Debug, Parser)]
pub(super) struct CtxPluginCli {
    #[command(flatten)]
    pub(super) args: commands::ctx::CtxArgs,
}

pub(super) struct CtxBuiltinPlugin;

pub(super) fn ctx_metadata() -> PluginMetadata {
    PluginMetadata {
        plugin_name: "builtin-ctx".to_owned(),
        domain: "ctx".to_owned(),
        description: "Context utilities plugin (built-in)".to_owned(),
        abi_version: 1,
        required_tools: Vec::new(),
        compatibility: PluginCompatibility::current()
            .with_capability(plugin_capabilities::TYPED_COMMANDS_V1),
    }
}

pub(super) fn ctx_manual() -> PluginManual {
    PluginManual {
        plugin_name: ctx_metadata().plugin_name,
        domain: "ctx".to_owned(),
        description: "Context-reduction helpers for AI workflows.".to_owned(),
        commands: vec![
            ManualCommand {
                name: "pack".to_owned(),
                summary: "Create compact digest for files/directories.".to_owned(),
                usage: "pack <path...> [--preset <summary|review|debug>] [--max-bytes BYTES] [--follow-symlinks]".to_owned(),
                examples: vec![ManualExample::new(
                    "Pack codebase context for review",
                    &["pack", "src", "docs", "--preset", "review"],
                )],
            },
            ManualCommand {
                name: "symbols".to_owned(),
                summary:
                    "Extract code, config, and document symbols from file or directory.".to_owned(),
                usage: "symbols <path> [--preset <summary|review|debug>] [--max-bytes BYTES] [--follow-symlinks]".to_owned(),
                examples: vec![ManualExample::new(
                    "Extract symbols from commands module",
                    &["symbols", "src/commands", "--preset", "summary"],
                )],
            },
            ManualCommand {
                name: "changed".to_owned(),
                summary: "Show changed files from git status.".to_owned(),
                usage: "changed".to_owned(),
                examples: vec![ManualExample::new(
                    "Collect changed paths before review",
                    &["changed"],
                )],
            },
        ],
        notes: vec![
            "Presets tune default limits and symbol density.".to_owned(),
            "Symbol extraction uses lightweight heuristics across common programming, infra, config, and script files.".to_owned(),
            "Pair with --json for downstream prompt assembly.".to_owned(),
        ],
    }
}

impl BuiltinPlugin for CtxBuiltinPlugin {
    fn metadata(&self) -> PluginMetadata {
        ctx_metadata()
    }

    fn manual(&self) -> PluginManual {
        ctx_manual()
    }

    fn required_tools(&self, request: &InvocationRequest) -> Vec<RequiredTool> {
        if request_command(request).as_deref() == Some("changed") {
            vec![git_required_tool()]
        } else {
            Vec::new()
        }
    }

    fn command_catalog(&self) -> Option<CommandCatalog> {
        Some(commands::ctx::command_catalog())
    }

    fn required_tools_typed(&self, request: &TypedInvocationRequest) -> Vec<RequiredTool> {
        if request.command == "ctx.changed" {
            vec![git_required_tool()]
        } else {
            Vec::new()
        }
    }

    fn invoke_typed(&self, request: &TypedInvocationRequest) -> TypedInvocationResponse {
        commands::ctx::invoke_typed(request)
    }

    fn invoke(&self, request: &InvocationRequest) -> InvocationResponse {
        self.invoke_into(request, &OutputSink::Process)
    }

    fn invoke_into(&self, request: &InvocationRequest, sink: &OutputSink) -> InvocationResponse {
        let (parsed, options) =
            match parse_args::<CtxPluginCli>("ctx", &request.argv, request.globals.clone()) {
                ParseOutcome::Parsed(value, options) => (value, options),
                ParseOutcome::Response(response) => return response,
            };
        map_execute("ctx", commands::ctx::execute(parsed.args, &options, sink))
    }
}
