pub mod cli;
pub mod commands;
pub mod error;
pub mod output;
pub mod plugins;
pub mod safety;

use std::path::PathBuf;

use ah_plugin_api::InvocationResponse;
use ah_runtime::{PluginManager, RuntimeError};
use clap::Parser;

use crate::{
    cli::{Cli, RuntimeCommand},
    error::AppError,
    output::OutputMode,
};

pub fn run() -> Result<(), AppError> {
    let cli = Cli::parse();
    let command = cli.into_runtime_command()?;

    let mut manager = PluginManager::new();
    for plugin in plugins::builtins() {
        manager.register_builtin(plugin);
    }

    let plugin_dir = PathBuf::from(".ah/plugins");
    let load_report = manager.load_dynamic_plugins_from_dir(&plugin_dir);
    if !command_is_quiet(&command) {
        for warning in &load_report.warnings {
            eprintln!(
                "warning: skipped plugin {}: {}",
                warning.path.display(),
                warning.error
            );
        }
    }

    match command {
        RuntimeCommand::PluginsList { options } => {
            if options.quiet {
                return Ok(());
            }
            let plugins = manager.list_plugins();
            match options.output {
                OutputMode::Text => {
                    if plugins.is_empty() {
                        println!("no plugins registered");
                    } else {
                        for plugin in plugins {
                            println!(
                                "{} ({}) - {}",
                                plugin.domain, plugin.plugin_name, plugin.description
                            );
                        }
                    }
                }
                OutputMode::Json => {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&plugins).map_err(AppError::from)?
                    );
                }
            }
            Ok(())
        }
        RuntimeCommand::Invoke {
            domain,
            argv,
            options,
        } => {
            let response = manager
                .invoke(&domain, argv, options.to_wire())
                .map_err(map_runtime_error)?;
            handle_response(response, options.output, options.quiet)
        }
    }
}

fn handle_response(
    response: InvocationResponse,
    output_mode: OutputMode,
    quiet: bool,
) -> Result<(), AppError> {
    if response.success {
        if quiet {
            return Ok(());
        }
        if let Some(message) = response.message {
            match output_mode {
                OutputMode::Text => println!("{message}"),
                OutputMode::Json => println!("{message}"),
            }
        }
        return Ok(());
    }

    let code = response
        .error_code
        .unwrap_or_else(|| "PLUGIN_EXECUTION_FAILED".to_owned());
    let message = response
        .error_message
        .unwrap_or_else(|| "plugin execution failed".to_owned());
    Err(AppError::invalid_argument(format!("[{code}] {message}")))
}

fn map_runtime_error(error: RuntimeError) -> AppError {
    AppError::invalid_argument(error.to_string())
}

fn command_is_quiet(command: &RuntimeCommand) -> bool {
    match command {
        RuntimeCommand::PluginsList { options } => options.quiet,
        RuntimeCommand::Invoke { options, .. } => options.quiet,
    }
}
