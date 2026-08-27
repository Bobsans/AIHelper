//! The `file` builtin: its CLI shape, its metadata, its manual, and the
//! `BuiltinPlugin` impl that ties them to `commands::file`.

use super::*;

#[derive(Debug, Parser)]
pub(super) struct FilePluginCli {
    #[command(flatten)]
    pub(super) args: commands::file::FileArgs,
}

pub(super) struct FileBuiltinPlugin;

pub(super) fn file_metadata() -> PluginMetadata {
    PluginMetadata {
        plugin_name: "builtin-file".to_owned(),
        domain: "file".to_owned(),
        description: "File operations plugin (built-in)".to_owned(),
        abi_version: 1,
        required_tools: Vec::new(),
        compatibility: PluginCompatibility::current()
            .with_capability(plugin_capabilities::TYPED_COMMANDS_V1),
    }
}

pub(super) fn file_manual() -> PluginManual {
    PluginManual {
        plugin_name: file_metadata().plugin_name,
        domain: "file".to_owned(),
        description: "Read and inspect files and directory trees.".to_owned(),
        commands: vec![
            ManualCommand {
                name: "read".to_owned(),
                summary: "Read file content with optional line range and numbering.".to_owned(),
                usage:
                    "read <path> [-n] [--from N] [--to N] [--max-bytes BYTES] [--follow-symlinks]"
                        .to_owned(),
                examples: vec![
                    ManualExample::new(
                        "Read first 120 lines with source numbers",
                        &["read", "src/main.rs", "-n", "--from", "1", "--to", "120"],
                    ),
                    ManualExample::new(
                        "Read with explicit size/symlink policy",
                        &[
                            "read",
                            "README.md",
                            "--max-bytes",
                            "1048576",
                            "--follow-symlinks",
                        ],
                    ),
                ],
            },
            ManualCommand {
                name: "head".to_owned(),
                summary: "Return first N lines.".to_owned(),
                usage: "head <path> [--lines N] [-n] [--max-bytes BYTES] [--follow-symlinks]"
                    .to_owned(),
                examples: vec![ManualExample::new(
                    "Preview first 40 lines",
                    &["head", "src/lib.rs", "--lines", "40", "-n"],
                )],
            },
            ManualCommand {
                name: "tail".to_owned(),
                summary: "Return last N lines.".to_owned(),
                usage: "tail <path> [--lines N] [-n] [--max-bytes BYTES] [--follow-symlinks]"
                    .to_owned(),
                examples: vec![ManualExample::new(
                    "Inspect file tail",
                    &["tail", "CHANGELOG.md", "--lines", "30"],
                )],
            },
            ManualCommand {
                name: "stat".to_owned(),
                summary: "Show file metadata.".to_owned(),
                usage: "stat <path>".to_owned(),
                examples: vec![ManualExample::new(
                    "Inspect metadata",
                    &["stat", "Cargo.toml"],
                )],
            },
            ManualCommand {
                name: "tree".to_owned(),
                summary: "Render directory tree.".to_owned(),
                usage: "tree [path] [--depth N] [--follow-symlinks]".to_owned(),
                examples: vec![ManualExample::new(
                    "Show compact source tree",
                    &["tree", "src", "--depth", "2"],
                )],
            },
        ],
        notes: vec![
            "Prefer narrow ranges and --limit to reduce context size.".to_owned(),
            "Use --json for machine-readable chaining.".to_owned(),
        ],
    }
}

impl BuiltinPlugin for FileBuiltinPlugin {
    fn metadata(&self) -> PluginMetadata {
        file_metadata()
    }

    fn manual(&self) -> PluginManual {
        file_manual()
    }

    fn invoke(&self, request: &InvocationRequest) -> InvocationResponse {
        self.invoke_into(request, &OutputSink::Process)
    }

    fn invoke_into(&self, request: &InvocationRequest, sink: &OutputSink) -> InvocationResponse {
        let (parsed, options) =
            match parse_args::<FilePluginCli>("file", &request.argv, request.globals.clone()) {
                ParseOutcome::Parsed(value, options) => (value, options),
                ParseOutcome::Response(response) => return response,
            };
        map_execute("file", commands::file::execute(parsed.args, &options, sink))
    }

    fn command_catalog(&self) -> Option<CommandCatalog> {
        Some(commands::file::command_catalog())
    }

    fn invoke_typed(&self, request: &TypedInvocationRequest) -> TypedInvocationResponse {
        commands::file::invoke_typed(request)
    }
}
