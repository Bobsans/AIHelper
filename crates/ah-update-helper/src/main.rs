#![forbid(unsafe_code)]

use std::{env, io, process::ExitCode};

use ah_update_helper::recovery_command::{
    RecoveryExecution, execute_recovery, parse_recovery_arguments,
};
use ah_updater_core::{UpdateHelperSelfCheckV1, production_release_trust};

const USAGE: &str = "usage: ah-update-helper --self-check | recover --installation-root <PATH> --installation-state-root <PATH> --transaction-root <PATH> <PARENT_PID>";

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
    let command = match parse_recovery_arguments(&arguments) {
        Ok(command) => command,
        Err(_) => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    let result = production_release_trust().and_then(|trust| execute_recovery(&command, &trust));
    match result {
        Ok(RecoveryExecution::AlreadyRunning) => ExitCode::SUCCESS,
        Ok(RecoveryExecution::Recovered(state)) => {
            let response = serde_json::json!({
                "schema_version": 1,
                "state": state,
            });
            if serde_json::to_writer(io::stdout().lock(), &response).is_err() {
                eprintln!("failed to write update recovery result");
                return ExitCode::FAILURE;
            }
            println!();
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{}", error.code());
            ExitCode::FAILURE
        }
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
