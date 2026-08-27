//! The `task` builtin: its CLI shape, its metadata, its manual, and the
//! `BuiltinPlugin` impl that ties them to `commands::task`.

use super::*;

#[derive(Debug, Parser)]
pub(super) struct TaskPluginCli {
    #[command(flatten)]
    pub(super) args: commands::task::TaskArgs,
}

pub(super) struct TaskBuiltinPlugin;

pub(super) fn task_metadata() -> PluginMetadata {
    PluginMetadata {
        plugin_name: "builtin-task".to_owned(),
        domain: "task".to_owned(),
        description: "Task recipe plugin (built-in)".to_owned(),
        abi_version: 1,
        required_tools: Vec::new(),
        compatibility: PluginCompatibility::current()
            .with_capability(plugin_capabilities::TYPED_COMMANDS_V1),
    }
}

pub(super) fn task_manual() -> PluginManual {
    PluginManual {
        plugin_name: task_metadata().plugin_name,
        domain: "task".to_owned(),
        description: "Save and execute reusable local command recipes.".to_owned(),
        commands: vec![
            ManualCommand {
                name: "save".to_owned(),
                summary: "Create or update task recipe.".to_owned(),
                usage: "save <name> <command>".to_owned(),
                examples: vec![ManualExample::new(
                    "Save quick status workflow",
                    &["save", "print-working-dir", "pwd"],
                )],
            },
            ManualCommand {
                name: "run".to_owned(),
                summary: "Run saved task by name.".to_owned(),
                usage: "run <name> [--timeout-secs SECONDS] [--max-output-bytes BYTES]".to_owned(),
                examples: vec![ManualExample::new(
                    "Execute saved workflow",
                    &["run", "print-working-dir"],
                )],
            },
            ManualCommand {
                name: "list".to_owned(),
                summary: "List available task recipes.".to_owned(),
                usage: "list".to_owned(),
                examples: vec![ManualExample::new("Inspect available recipes", &["list"])],
            },
        ],
        notes: vec!["Task commands execute through system shell.".to_owned()],
    }
}

impl BuiltinPlugin for TaskBuiltinPlugin {
    fn metadata(&self) -> PluginMetadata {
        task_metadata()
    }

    fn manual(&self) -> PluginManual {
        task_manual()
    }

    fn invoke(&self, request: &InvocationRequest) -> InvocationResponse {
        self.invoke_into(request, &OutputSink::Process)
    }

    fn invoke_into(&self, request: &InvocationRequest, sink: &OutputSink) -> InvocationResponse {
        let (parsed, options) =
            match parse_args::<TaskPluginCli>("task", &request.argv, request.globals.clone()) {
                ParseOutcome::Parsed(value, options) => (value, options),
                ParseOutcome::Response(response) => return response,
            };
        map_execute("task", commands::task::execute(parsed.args, &options, sink))
    }

    fn command_catalog(&self) -> Option<CommandCatalog> {
        Some(commands::task::command_catalog())
    }

    fn invoke_typed(&self, request: &TypedInvocationRequest) -> TypedInvocationResponse {
        commands::task::invoke_typed(request)
    }

    fn cancel_typed(&self, request_id: &str) -> bool {
        ah_plugin_api::cancellation::cancel(request_id)
    }
}
