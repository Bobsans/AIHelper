#![forbid(unsafe_code)]

use std::{env, io, process::ExitCode};

use ah_update_helper::activation_command::{
    ManagedMcpRestoration, execute_activation, restore_managed_mcp_and_finalize,
};
use ah_update_helper::handoff::HandoffLease;
use ah_update_helper::recovery_command::{
    RecoveryExecution, execute_recovery, parse_activation_arguments, parse_recovery_arguments,
    parse_rollback_arguments,
};
use ah_updater_core::{UpdateHelperSelfCheckV1, production_release_trust};

const USAGE: &str = "usage: ah-update-helper --self-check | <activate|recover|rollback> --installation-root <PATH> --installation-state-root <PATH> --transaction-root <PATH> --lifecycle-lock <PATH> --lifecycle-lock-handle <HANDLE> --handoff-event <NAME> <PARENT_PID>";

fn main() -> ExitCode {
    let arguments = env::args_os().skip(1).collect::<Vec<_>>();
    if arguments.len() == 1 && arguments[0] == "--self-check" {
        return match write_self_check(io::stdout().lock()) {
            Ok(()) => ExitCode::SUCCESS,
            Err(()) => {
                eprintln!("failed to write update helper self-check");
                ExitCode::FAILURE
            }
        };
    }
    let activation = arguments.first().is_some_and(|value| value == "activate");
    let rollback = arguments.first().is_some_and(|value| value == "rollback");
    let command = match if activation {
        parse_activation_arguments(&arguments)
    } else if rollback {
        parse_rollback_arguments(&arguments)
    } else {
        parse_recovery_arguments(&arguments)
    } {
        Ok(command) => command,
        Err(_) => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    let handoff = match HandoffLease::claim(
        command.lifecycle_lock_handle,
        &command.lifecycle_lock,
        &command.handoff_event,
    ) {
        Ok(lease) => lease,
        Err(error) => {
            eprintln!("{}", error.code());
            return ExitCode::FAILURE;
        }
    };
    let trust = match production_release_trust() {
        Ok(trust) => trust,
        Err(error) => {
            eprintln!("{}", error.code());
            return ExitCode::FAILURE;
        }
    };
    let execution = if activation || rollback {
        execute_activation(&command, &trust).map(Some)
    } else {
        match execute_recovery(&command, &trust) {
            Ok(RecoveryExecution::AlreadyRunning) => return ExitCode::SUCCESS,
            Ok(RecoveryExecution::Recovered(state)) => Ok(Some(state)),
            Err(error) => Err(error),
        }
    };
    drop(handoff);
    let restoration = restore_managed_mcp_and_finalize(&command, &trust);
    match (execution, restoration) {
        (Ok(Some(state)), Ok(restoration)) => {
            let response = serde_json::json!({
                "schema_version": 1,
                "state": state,
                "managed_mcp_restoration": restoration_name(restoration),
            });
            if serde_json::to_writer(io::stdout().lock(), &response).is_err() {
                eprintln!("failed to write update recovery result");
                return ExitCode::FAILURE;
            }
            println!();
            ExitCode::SUCCESS
        }
        (Err(error), _) | (Ok(_), Err(error)) => {
            eprintln!("{}", error.code());
            ExitCode::FAILURE
        }
        (Ok(None), Ok(_)) => ExitCode::SUCCESS,
    }
}

fn restoration_name(restoration: ManagedMcpRestoration) -> &'static str {
    match restoration {
        ManagedMcpRestoration::NotRequired => "not_required",
        ManagedMcpRestoration::Restored => "restored",
    }
}

fn write_self_check(mut output: impl io::Write) -> Result<(), ()> {
    let response =
        UpdateHelperSelfCheckV1::new(env!("CARGO_PKG_VERSION"), BUILD_TARGET, env::consts::ARCH);
    serde_json::to_writer(&mut output, &response).map_err(|_| ())?;
    output.write_all(b"\n").map_err(|_| ())
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
const BUILD_TARGET: &str = "x86_64-pc-windows-msvc";
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const BUILD_TARGET: &str = "x86_64-unknown-linux-gnu";
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const BUILD_TARGET: &str = "aarch64-apple-darwin";
#[cfg(not(any(
    all(target_os = "windows", target_arch = "x86_64"),
    all(target_os = "linux", target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64")
)))]
const BUILD_TARGET: &str = "unsupported";
