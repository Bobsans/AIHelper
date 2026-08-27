//! The `git` builtin: its CLI shape, its metadata, its manual, and the
//! `BuiltinPlugin` impl that ties them to `commands::git`.

use super::*;

#[derive(Debug, Parser)]
pub(super) struct GitPluginCli {
    #[command(flatten)]
    pub(super) args: commands::git::GitArgs,
}

pub(super) struct GitBuiltinPlugin;

pub(super) fn git_metadata() -> PluginMetadata {
    PluginMetadata {
        plugin_name: "builtin-git".to_owned(),
        domain: "git".to_owned(),
        description: "Git utilities plugin (built-in)".to_owned(),
        abi_version: 1,
        required_tools: vec![git_required_tool()],
        compatibility: PluginCompatibility::current()
            .with_capability(plugin_capabilities::TYPED_COMMANDS_V1),
    }
}

pub(super) fn git_required_tool() -> RequiredTool {
    RequiredTool::new(
        "git",
        "local git commands require the git executable on PATH",
    )
}

pub(super) fn git_manual() -> PluginManual {
    PluginManual {
        plugin_name: git_metadata().plugin_name,
        domain: "git".to_owned(),
        description: "Git-oriented context helpers for working tree and history.".to_owned(),
        commands: vec![
            ManualCommand {
                name: "status".to_owned(),
                summary: "Show compact repository status summary.".to_owned(),
                usage: "status".to_owned(),
                examples: vec![ManualExample::new(
                    "Inspect branch, upstream, counts, commit, and tag",
                    &["status"],
                )],
            },
            ManualCommand {
                name: "tags".to_owned(),
                summary: "List tags newest-first.".to_owned(),
                usage: "tags [--latest]".to_owned(),
                examples: vec![
                    ManualExample::new("List tags", &["tags"]),
                    ManualExample::new("Show latest tag only", &["tags", "--latest"]),
                ],
            },
            ManualCommand {
                name: "tag create".to_owned(),
                summary: "Create a lightweight or annotated git tag.".to_owned(),
                usage: "tag create <tag> [--message TEXT] [--ref REF]".to_owned(),
                examples: vec![ManualExample::new(
                    "Create an annotated release tag",
                    &["tag", "create", "v1.0.0", "--message", "v1.0.0"],
                )],
            },
            ManualCommand {
                name: "remotes".to_owned(),
                summary: "List configured git remotes with provider hint.".to_owned(),
                usage: "remotes".to_owned(),
                examples: vec![ManualExample::new("Inspect remotes", &["remotes"])],
            },
            ManualCommand {
                name: "changed".to_owned(),
                summary: "Summarize working tree changes.".to_owned(),
                usage: "changed".to_owned(),
                examples: vec![ManualExample::new("List changed files", &["changed"])],
            },
            ManualCommand {
                name: "diff".to_owned(),
                summary: "Show local diff (optionally filtered by path).".to_owned(),
                usage: "diff [--path <path>]".to_owned(),
                examples: vec![ManualExample::new(
                    "Review diff for one file",
                    &["diff", "--path", "src/cli.rs"],
                )],
            },
            ManualCommand {
                name: "blame".to_owned(),
                summary: "Show blame data for file or single line.".to_owned(),
                usage: "blame <path> [--line N]".to_owned(),
                examples: vec![ManualExample::new(
                    "Inspect ownership of a specific line",
                    &["blame", "src/commands/search.rs", "--line", "120"],
                )],
            },
            ManualCommand {
                name: "commit-info".to_owned(),
                summary: "Show commit metadata, touched files, and line stats.".to_owned(),
                usage: "commit-info [ref]".to_owned(),
                examples: vec![ManualExample::new(
                    "Inspect the latest commit",
                    &["commit-info", "HEAD"],
                )],
            },
        ],
        notes: vec!["Useful for quick change-attribution in AI review loops.".to_owned()],
    }
}

impl BuiltinPlugin for GitBuiltinPlugin {
    fn metadata(&self) -> PluginMetadata {
        git_metadata()
    }

    fn manual(&self) -> PluginManual {
        git_manual()
    }

    fn required_tools(&self, _request: &InvocationRequest) -> Vec<RequiredTool> {
        Vec::new()
    }

    fn invoke(&self, request: &InvocationRequest) -> InvocationResponse {
        self.invoke_into(request, &OutputSink::Process)
    }

    fn invoke_into(&self, request: &InvocationRequest, sink: &OutputSink) -> InvocationResponse {
        let (parsed, options) =
            match parse_args::<GitPluginCli>("git", &request.argv, request.globals.clone()) {
                ParseOutcome::Parsed(value, options) => (value, options),
                ParseOutcome::Response(response) => return response,
            };
        map_execute("git", commands::git::execute(parsed.args, &options, sink))
    }

    fn command_catalog(&self) -> Option<CommandCatalog> {
        Some(commands::git::command_catalog())
    }

    fn required_tools_typed(&self, _request: &TypedInvocationRequest) -> Vec<RequiredTool> {
        Vec::new()
    }

    fn invoke_typed(&self, request: &TypedInvocationRequest) -> TypedInvocationResponse {
        commands::git::invoke_typed(request)
    }
}
