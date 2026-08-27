//! The `run` builtin: its CLI shape, its metadata, its manual, and the
//! `BuiltinPlugin` impl that ties them to `commands::run`.

use super::*;

#[derive(Debug, Parser)]
pub(super) struct RunPluginCli {
    #[command(flatten)]
    pub(super) args: commands::run::RunArgs,
}

pub(super) struct RunBuiltinPlugin;

pub(super) fn run_metadata() -> PluginMetadata {
    PluginMetadata {
        plugin_name: "builtin-run".to_owned(),
        domain: "run".to_owned(),
        description: "Command execution check plugin (built-in)".to_owned(),
        abi_version: 1,
        required_tools: Vec::new(),
        compatibility: PluginCompatibility::current()
            .with_capability(plugin_capabilities::TYPED_COMMANDS_V1),
    }
}

pub(super) fn run_manual() -> PluginManual {
    PluginManual {
        plugin_name: run_metadata().plugin_name,
        domain: "run".to_owned(),
        description: "Run explicit commands with timeout and bounded output.".to_owned(),
        commands: vec![ManualCommand {
            name: "check".to_owned(),
            summary: "Run a command and report success, exit code, duration, stdout, and stderr."
                .to_owned(),
            usage:
                "check [--timeout-secs SECONDS] [--max-output-bytes BYTES] [--tail-lines N] <command...>"
                    .to_owned(),
            examples: vec![
                ManualExample::new("Run cargo tests", &["check", "cargo", "test"]),
                ManualExample::new(
                    "Run command with timeout",
                    &["check", "--timeout-secs", "60", "cargo", "build"],
                ),
            ],
        }],
        notes: vec![
            "Command is executed directly without a shell.".to_owned(),
            "The ah command itself exits successfully; inspect success=false for checked command failures.".to_owned(),
        ],
    }
}

impl BuiltinPlugin for RunBuiltinPlugin {
    fn metadata(&self) -> PluginMetadata {
        run_metadata()
    }

    fn manual(&self) -> PluginManual {
        run_manual()
    }

    fn invoke(&self, request: &InvocationRequest) -> InvocationResponse {
        self.invoke_observed(request).response
    }

    fn invoke_observed(&self, request: &InvocationRequest) -> ah_runtime::InvocationObservation {
        self.invoke_observed_into(request, &OutputSink::Process)
    }

    /// `run` is the one built-in whose invocation reports an outcome as well as
    /// a response, so it overrides the observed form rather than `invoke_into`.
    fn invoke_observed_into(
        &self,
        request: &InvocationRequest,
        sink: &OutputSink,
    ) -> ah_runtime::InvocationObservation {
        let (parsed, options) =
            match parse_args::<RunPluginCli>("run", &request.argv, request.globals.clone()) {
                ParseOutcome::Parsed(value, options) => (value, options),
                ParseOutcome::Response(response) => {
                    return ah_runtime::InvocationObservation::without_outcome(response);
                }
            };
        match commands::run::execute_observed(parsed.args, &options, sink) {
            Ok(outcome) => ah_runtime::InvocationObservation::new(
                InvocationResponse::ok(None),
                ah_runtime::InvocationOutcome::RunCheck(outcome),
            ),
            Err(error) => {
                ah_runtime::InvocationObservation::without_outcome(map_execute("run", Err(error)))
            }
        }
    }

    fn command_catalog(&self) -> Option<CommandCatalog> {
        Some(commands::run::command_catalog())
    }

    fn invoke_typed(&self, request: &TypedInvocationRequest) -> TypedInvocationResponse {
        commands::run::invoke_typed(request)
    }

    fn cancel_typed(&self, request_id: &str) -> bool {
        ah_plugin_api::cancellation::cancel(request_id)
    }
}
