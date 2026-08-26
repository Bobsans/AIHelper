//! Wiring the updater into this process.
//!
//! `ah-updater`'s mechanism asks for two things it must not reach for itself:
//! a managed service to hold still, and a way to run a bounded child process.
//! This module supplies the second; `mcp_service::lifecycle::guard` supplies
//! the first.

use std::{ffi::OsStr, time::Duration};

use ah_updater_core::{UpdaterError, UpdaterErrorCode};

use crate::{
    commands::run::io::{EnvironmentOverride, RunCommandOptions, run_program},
    updater::smoke::{SmokeProcessOutput, SmokeRequest, SmokeRunner},
};

/// How long the candidate gets to answer one smoke command.
const SMOKE_TIMEOUT: Duration = Duration::from_secs(15);
/// Enough output to diagnose a failure, and a bound so a flooding candidate
/// cannot exhaust this process instead of failing the check.
const MAX_SMOKE_OUTPUT_BYTES: usize = 1024 * 1024;

/// The smoke runner backed by this CLI's own bounded process runner.
pub(crate) struct BoundedSmokeRunner;

impl SmokeRunner for BoundedSmokeRunner {
    fn run(&self, request: SmokeRequest<'_>) -> Result<SmokeProcessOutput, UpdaterError> {
        let arguments = request
            .arguments
            .iter()
            .map(|argument| (*argument).to_owned())
            .collect::<Vec<_>>();
        let environment = [EnvironmentOverride {
            name: OsStr::new("AH_CONFIG_DIR"),
            value: request.config_dir.as_os_str(),
        }];
        let output = run_program(
            request.program,
            &arguments,
            RunCommandOptions {
                timeout: SMOKE_TIMEOUT,
                command_label: "candidate offline smoke",
                max_output_bytes: MAX_SMOKE_OUTPUT_BYTES,
                tail_lines: None,
                cwd: Some(request.cwd),
                environment: &environment,
                cancelled: never_cancelled,
            },
        )
        .map_err(|_| candidate("failed to execute candidate smoke command"))?;
        Ok(SmokeProcessOutput {
            exit_code: output.exit_code,
            timed_out: output.timed_out,
            stdout: output.stdout.bytes,
            stderr: output.stderr.bytes,
            stdout_truncated: output.stdout.truncated,
            stderr_truncated: output.stderr.truncated,
        })
    }
}
/// The smoke check is not cancellable from outside: it runs before anything has
/// been changed, and abandoning it half-way tells us nothing.
fn never_cancelled() -> bool {
    false
}

fn candidate(detail: &'static str) -> UpdaterError {
    UpdaterError::new(UpdaterErrorCode::Candidate, detail)
}
