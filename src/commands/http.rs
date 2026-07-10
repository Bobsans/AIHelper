use std::path::PathBuf;

use clap::{Args, Subcommand, ValueEnum};

use crate::{cli::GlobalOptions, error::AppError};

mod adapters {
    pub mod io;
    pub mod output;
}

mod domain;

#[derive(Debug, Args)]
pub struct HttpArgs {
    #[command(subcommand)]
    pub command: HttpCommand,
}

#[derive(Debug, Subcommand)]
pub enum HttpCommand {
    #[command(about = "Send HTTP request with explicit method")]
    Request(RequestArgs),
    #[command(about = "Send HTTP GET request")]
    Get(MethodShortcutArgs),
    #[command(about = "Send HTTP POST request")]
    Post(MethodShortcutArgs),
    #[command(about = "Send HTTP PUT request")]
    Put(MethodShortcutArgs),
    #[command(about = "Send HTTP PATCH request")]
    Patch(MethodShortcutArgs),
    #[command(about = "Send HTTP DELETE request")]
    Delete(MethodShortcutArgs),
    #[command(about = "Replay curl command through stable CLI contract")]
    Replay(ReplayArgs),
    #[command(about = "Run API assertions from spec file")]
    Assert(AssertArgs),
    #[command(about = "Alias for assert")]
    Run(AssertArgs),
}

#[derive(Debug, Args)]
pub struct RequestArgs {
    #[arg(long, value_name = "METHOD")]
    pub method: String,
    pub url: String,
    #[command(flatten)]
    pub request: RequestOptionsArgs,
    #[command(flatten)]
    pub expect: RequestExpectArgs,
}

#[derive(Debug, Args)]
pub struct MethodShortcutArgs {
    pub url: String,
    #[command(flatten)]
    pub request: RequestOptionsArgs,
    #[command(flatten)]
    pub expect: RequestExpectArgs,
}

#[derive(Debug, Args)]
pub struct ReplayArgs {
    #[arg(long, value_name = "CURL", help = "curl command to replay")]
    pub curl: String,
    #[command(flatten)]
    pub request: RequestOptionsArgs,
    #[command(flatten)]
    pub expect: RequestExpectArgs,
}

#[derive(Debug, Args)]
pub struct AssertArgs {
    pub spec_path: PathBuf,
    #[arg(long = "var", value_name = "KEY=VALUE")]
    pub vars: Vec<String>,
    #[arg(long)]
    pub fail_fast: bool,
    #[arg(long, value_enum, value_name = "FORMAT")]
    pub report: Option<AssertReportArg>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, ValueEnum)]
pub enum AssertReportArg {
    Text,
    Json,
    Junit,
}

#[derive(Debug, Args, Clone)]
pub struct RequestOptionsArgs {
    #[arg(long = "header", value_name = "K: V")]
    pub headers: Vec<String>,
    #[arg(long = "query", value_name = "KEY=VALUE")]
    pub query: Vec<String>,
    #[arg(long, value_name = "SECONDS")]
    pub timeout_secs: Option<u64>,
    #[arg(long, value_name = "BYTES")]
    pub max_response_bytes: Option<usize>,
    #[arg(long, value_name = "TOKEN")]
    pub bearer: Option<String>,
    #[arg(long, value_name = "USER:PASS")]
    pub basic: Option<String>,
    #[arg(long, value_name = "JSON")]
    pub json: Option<String>,
    #[arg(long, value_name = "PATH")]
    pub json_file: Option<PathBuf>,
    #[arg(long, value_name = "TEXT")]
    pub body: Option<String>,
    #[arg(long, value_name = "PATH")]
    pub body_file: Option<PathBuf>,
}

#[derive(Debug, Args, Clone, Default)]
pub struct RequestExpectArgs {
    #[arg(long = "expect-status", value_name = "CODE_OR_RANGE")]
    pub expect_status: Option<String>,
    #[arg(long = "expect-header", value_name = "K: V")]
    pub expect_headers: Vec<String>,
    #[arg(long = "expect-body-contains", value_name = "TEXT")]
    pub expect_body_contains: Vec<String>,
    #[arg(
        long = "expect-json",
        value_name = "PATH:OP[:VALUE]",
        help = "JSON expectation expression, for example status:eq:ok"
    )]
    pub expect_json: Vec<String>,
}

pub fn execute(args: HttpArgs, options: &GlobalOptions) -> Result<(), AppError> {
    match args.command {
        HttpCommand::Request(request_args) => execute_request(
            domain::run_request_command(request_args, "request"),
            options,
        ),
        HttpCommand::Get(method_args) => execute_shortcut("get", "GET", method_args, options),
        HttpCommand::Post(method_args) => execute_shortcut("post", "POST", method_args, options),
        HttpCommand::Put(method_args) => execute_shortcut("put", "PUT", method_args, options),
        HttpCommand::Patch(method_args) => execute_shortcut("patch", "PATCH", method_args, options),
        HttpCommand::Delete(method_args) => {
            execute_shortcut("delete", "DELETE", method_args, options)
        }
        HttpCommand::Replay(replay_args) => {
            execute_request(domain::run_replay(replay_args, "replay"), options)
        }
        HttpCommand::Assert(assert_args) => execute_assert(assert_args, options, "assert"),
        HttpCommand::Run(assert_args) => execute_assert(assert_args, options, "run"),
    }
}

fn execute_shortcut(
    command_name: &'static str,
    method: &str,
    args: MethodShortcutArgs,
    options: &GlobalOptions,
) -> Result<(), AppError> {
    execute_request(
        domain::run_request_shortcut(command_name, method, args),
        options,
    )
}

fn execute_request(
    request: Result<domain::HttpRequestOutput, AppError>,
    options: &GlobalOptions,
) -> Result<(), AppError> {
    let payload = request?;
    let failed = !payload.ok;
    adapters::output::emit_request(payload, options)?;
    if failed {
        return Err(AppError::external(
            "HTTP_ASSERTION_FAILED",
            "request expectations failed",
        ));
    }
    Ok(())
}

fn execute_assert(
    args: AssertArgs,
    options: &GlobalOptions,
    command_name: &'static str,
) -> Result<(), AppError> {
    let (output, report_format) = domain::run_assert(args, options.output, command_name)?;
    let failed = output.summary.failed > 0;
    adapters::output::emit_assert(&output, report_format, options)?;
    if failed {
        return Err(AppError::external(
            "HTTP_ASSERTION_FAILED",
            format!("{} of {} case(s) failed", failed, output.summary.total),
        ));
    }
    Ok(())
}
