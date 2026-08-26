//! The `project` builtin: its CLI shape, its metadata, its manual, and the
//! `BuiltinPlugin` impl that ties them to `commands::project`.

use super::*;

#[derive(Debug, Parser)]
pub(super) struct ProjectPluginCli {
    #[command(flatten)]
    pub(super) args: commands::project::ProjectArgs,
}

pub(super) struct ProjectBuiltinPlugin;

pub(super) fn project_metadata() -> PluginMetadata {
    PluginMetadata {
        plugin_name: "builtin-project".to_owned(),
        domain: "project".to_owned(),
        description: "Project detection plugin (built-in)".to_owned(),
        abi_version: 1,
        required_tools: Vec::new(),
        compatibility: PluginCompatibility::current()
            .with_capability(plugin_capabilities::TYPED_COMMANDS_V1),
    }
}

pub(super) fn project_manual() -> PluginManual {
    PluginManual {
        plugin_name: project_metadata().plugin_name,
        domain: "project".to_owned(),
        description: "Detect project ecosystems, tools, roles, versions, and suggested commands."
            .to_owned(),
        commands: vec![
            ManualCommand {
                name: "detect".to_owned(),
                summary: "Detect ecosystems, tools, roles, grouped files, versions, and commands."
                    .to_owned(),
                usage: "detect [path]".to_owned(),
                examples: vec![ManualExample::new("Detect current project", &["detect"])],
            },
            ManualCommand {
                name: "commands".to_owned(),
                summary: "Suggest likely install, test, build, release, and infra commands."
                    .to_owned(),
                usage: "commands [path]".to_owned(),
                examples: vec![ManualExample::new(
                    "Suggest commands for current project",
                    &["commands"],
                )],
            },
            ManualCommand {
                name: "version".to_owned(),
                summary: "Detect project version from common manifest files.".to_owned(),
                usage: "version [path]".to_owned(),
                examples: vec![ManualExample::new(
                    "Detect current project version",
                    &["version"],
                )],
            },
        ],
        notes: vec![
            "Detection is heuristic and does not execute package managers or infra tools."
                .to_owned(),
            "JSON detect output includes compatibility fields plus richer grouped snapshot fields."
                .to_owned(),
            "Use with ah run check to execute suggested commands explicitly.".to_owned(),
        ],
    }
}

impl BuiltinPlugin for ProjectBuiltinPlugin {
    fn metadata(&self) -> PluginMetadata {
        project_metadata()
    }

    fn manual(&self) -> PluginManual {
        project_manual()
    }

    fn invoke(&self, request: &InvocationRequest) -> InvocationResponse {
        let (parsed, options) =
            match parse_args::<ProjectPluginCli>("project", &request.argv, request.globals.clone())
            {
                ParseOutcome::Parsed(value, options) => (value, options),
                ParseOutcome::Response(response) => return response,
            };
        map_execute("project", commands::project::execute(parsed.args, &options))
    }

    fn command_catalog(&self) -> Option<CommandCatalog> {
        Some(commands::project::command_catalog())
    }

    fn invoke_typed(&self, request: &TypedInvocationRequest) -> TypedInvocationResponse {
        commands::project::invoke_typed(request)
    }
}
