use serde::Serialize;

use crate::error::AppError;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum OutputMode {
    Text,
    Json,
}

#[derive(Debug, Serialize)]
struct MessagePayload<'a> {
    command: &'a str,
    status: &'a str,
    message: &'a str,
}

pub fn emit_message(
    mode: OutputMode,
    quiet: bool,
    command: &str,
    message: &str,
) -> Result<(), AppError> {
    if quiet {
        return Ok(());
    }

    match mode {
        OutputMode::Text => println!("{message}"),
        OutputMode::Json => {
            let payload = MessagePayload {
                command,
                status: "ok",
                message,
            };
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
    }

    Ok(())
}

pub fn emit_not_implemented(mode: OutputMode, quiet: bool, command: &str) -> Result<(), AppError> {
    emit_message(
        mode,
        quiet,
        command,
        "This command is part of the roadmap and is not implemented yet.",
    )
}
