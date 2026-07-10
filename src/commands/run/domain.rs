use serde::Serialize;

use crate::error::AppError;

use super::{CheckArgs, adapters};

#[derive(Debug, Serialize)]
pub(crate) struct RunCheckOutput {
    pub command: &'static str,
    pub argv: Vec<String>,
    pub success: bool,
    pub timed_out: bool,
    pub exit_code: Option<i32>,
    pub duration_ms: u128,
    pub stdout: String,
    pub stderr: String,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

pub(crate) fn run_check(args: CheckArgs) -> Result<RunCheckOutput, AppError> {
    if args.max_output_bytes == 0 {
        return Err(AppError::invalid_argument(
            "--max-output-bytes must be >= 1",
        ));
    }

    let program = args
        .command
        .first()
        .ok_or_else(|| AppError::invalid_argument("missing command"))?
        .to_owned();
    let command_args = args.command.iter().skip(1).cloned().collect::<Vec<_>>();
    let command_label = args.command.join(" ");

    let execution = adapters::io::run_command(
        &program,
        &command_args,
        args.timeout_secs,
        &command_label,
        args.max_output_bytes,
        args.tail_lines,
    )?;

    let stdout = adapters::io::render_output(&execution.stdout, args.tail_lines);
    let stderr = adapters::io::render_output(&execution.stderr, args.tail_lines);

    Ok(RunCheckOutput {
        command: "run.check",
        argv: args.command,
        success: execution.exit_code == Some(0) && !execution.timed_out,
        timed_out: execution.timed_out,
        exit_code: execution.exit_code,
        duration_ms: execution.duration_ms,
        stdout,
        stderr,
        stdout_truncated: execution.stdout.truncated,
        stderr_truncated: execution.stderr.truncated,
    })
}
