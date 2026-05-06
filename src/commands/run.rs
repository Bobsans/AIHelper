use std::{
    io::{self, Read},
    process::{Command, Stdio},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use clap::Args;
use serde::Serialize;

use crate::{cli::GlobalOptions, error::AppError, output::OutputMode};

const DEFAULT_TIMEOUT_SECS: u64 = 600;
const DEFAULT_MAX_OUTPUT_BYTES: usize = 64 * 1024;

#[derive(Debug, Args)]
pub struct RunArgs {
    #[command(subcommand)]
    pub command: RunCommand,
}

#[derive(Debug, clap::Subcommand)]
pub enum RunCommand {
    #[command(about = "Run a command and return agent-friendly result")]
    Check(CheckArgs),
}

#[derive(Debug, Args)]
pub struct CheckArgs {
    #[arg(long, default_value_t = DEFAULT_TIMEOUT_SECS, value_name = "SECONDS")]
    pub timeout_secs: u64,
    #[arg(long, default_value_t = DEFAULT_MAX_OUTPUT_BYTES, value_name = "BYTES")]
    pub max_output_bytes: usize,
    #[arg(long, value_name = "N")]
    pub tail_lines: Option<usize>,
    #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
    pub command: Vec<String>,
}

#[derive(Debug, Serialize)]
struct RunCheckOutput {
    command: &'static str,
    argv: Vec<String>,
    success: bool,
    timed_out: bool,
    exit_code: Option<i32>,
    duration_ms: u128,
    stdout: String,
    stderr: String,
    stdout_truncated: bool,
    stderr_truncated: bool,
}

pub fn execute(args: RunArgs, options: &GlobalOptions) -> Result<(), AppError> {
    match args.command {
        RunCommand::Check(check_args) => execute_check(check_args, options),
    }
}

fn execute_check(args: CheckArgs, options: &GlobalOptions) -> Result<(), AppError> {
    if args.max_output_bytes == 0 {
        return Err(AppError::invalid_argument(
            "--max-output-bytes must be >= 1",
        ));
    }
    let program = args
        .command
        .first()
        .ok_or_else(|| AppError::invalid_argument("missing command"))?
        .clone();
    let command_label = args.command.join(" ");
    let command_args = args.command.iter().skip(1).cloned().collect::<Vec<_>>();
    let started = Instant::now();
    let mut child = Command::new(&program)
        .args(&command_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| AppError::command_execution(command_label.clone(), source))?;
    let stdout_handle = child.stdout.take().map(|mut stdout| {
        thread::spawn(move || {
            let mut output = Vec::new();
            stdout.read_to_end(&mut output)?;
            Ok::<Vec<u8>, io::Error>(output)
        })
    });
    let stderr_handle = child.stderr.take().map(|mut stderr| {
        thread::spawn(move || {
            let mut output = Vec::new();
            stderr.read_to_end(&mut output)?;
            Ok::<Vec<u8>, io::Error>(output)
        })
    });

    let timeout = Duration::from_secs(args.timeout_secs.max(1));
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|source| AppError::command_execution(command_label.clone(), source))?
        {
            break status;
        }
        if started.elapsed() >= timeout {
            timed_out = true;
            let _ = child.kill();
            break child
                .wait()
                .map_err(|source| AppError::command_execution(command_label.clone(), source))?;
        }
        thread::sleep(Duration::from_millis(25));
    };

    let stdout_raw = join_output_reader(stdout_handle, &command_label)?;
    let stderr_raw = join_output_reader(stderr_handle, &command_label)?;

    let (stdout, stdout_truncated) =
        prepare_output(&stdout_raw, args.max_output_bytes, args.tail_lines);
    let (stderr, stderr_truncated) =
        prepare_output(&stderr_raw, args.max_output_bytes, args.tail_lines);
    let payload = RunCheckOutput {
        command: "run.check",
        argv: args.command,
        success: status.success() && !timed_out,
        timed_out,
        exit_code: status.code(),
        duration_ms: started.elapsed().as_millis(),
        stdout,
        stderr,
        stdout_truncated,
        stderr_truncated,
    };

    if options.quiet {
        return Ok(());
    }

    match options.output {
        OutputMode::Text => {
            println!(
                "success={} exit_code={} timed_out={} duration_ms={}",
                payload.success,
                payload
                    .exit_code
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".to_owned()),
                payload.timed_out,
                payload.duration_ms
            );
            if !payload.stdout.is_empty() {
                println!("stdout:\n{}", payload.stdout);
            }
            if !payload.stderr.is_empty() {
                eprintln!("stderr:\n{}", payload.stderr);
            }
            if payload.stdout_truncated {
                eprintln!("warning: stdout truncated");
            }
            if payload.stderr_truncated {
                eprintln!("warning: stderr truncated");
            }
        }
        OutputMode::Json => println!("{}", serde_json::to_string_pretty(&payload)?),
    }

    Ok(())
}

fn join_output_reader(
    handle: Option<JoinHandle<io::Result<Vec<u8>>>>,
    command_label: &str,
) -> Result<Vec<u8>, AppError> {
    let Some(handle) = handle else {
        return Ok(Vec::new());
    };
    handle
        .join()
        .map_err(|_| {
            AppError::external(
                "COMMAND_OUTPUT_CAPTURE_FAILED",
                format!("failed to capture command output: {command_label}"),
            )
        })?
        .map_err(|source| AppError::command_execution(command_label.to_owned(), source))
}

fn prepare_output(
    raw: &[u8],
    max_output_bytes: usize,
    tail_lines: Option<usize>,
) -> (String, bool) {
    let mut text = String::from_utf8_lossy(raw).into_owned();
    if let Some(tail) = tail_lines {
        let lines = text.lines().collect::<Vec<_>>();
        if lines.len() > tail {
            text = lines[lines.len() - tail..].join("\n");
        }
    }
    let bytes = text.as_bytes();
    if bytes.len() <= max_output_bytes {
        return (text, false);
    }
    let mut end = max_output_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_owned(), true)
}
