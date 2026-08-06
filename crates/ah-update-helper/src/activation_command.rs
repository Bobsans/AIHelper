use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    time::Duration,
};

use ah_updater_core::{
    FilePurpose, ReleaseTrust, TransactionStateV1, UpdaterError, UpdaterErrorCode,
};

use crate::{
    bounded_process::{self, EnvironmentOverride},
    process::quiesce_transaction_blockers,
    recovery_command::{RecoveryCommand, wait_for_process_exit},
    transaction::{
        activate_transaction, commit_transaction, inspect_transaction, load_prepared_transaction,
        rollback_transaction,
    },
};

const PARENT_EXIT_TIMEOUT: Duration = Duration::from_secs(30);
const BLOCKER_GRACE_TIMEOUT: Duration = Duration::from_secs(5);
const CHILD_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_CHILD_OUTPUT_BYTES: usize = 64 * 1024;
const INSTALLED_SMOKE_ENV: &str = "AH_UPDATER_INSTALLED_SMOKE";
const MCP_RESTORE_ENV: &str = "AH_UPDATER_MCP_RESTORE";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedMcpRestoration {
    NotRequired,
    Restored,
}

pub fn execute_activation(
    command: &RecoveryCommand,
    trust: &ReleaseTrust,
) -> Result<TransactionStateV1, UpdaterError> {
    #[cfg(windows)]
    {
        wait_for_process_exit(command.parent_pid, PARENT_EXIT_TIMEOUT)?;
        let transaction = load_prepared_transaction(&command.paths, trust)?;
        quiesce_transaction_blockers(&transaction, BLOCKER_GRACE_TIMEOUT)?;
        let transaction = activate_transaction(&command.paths, trust)?;
        if let Err(error) = run_installed_smoke(&transaction) {
            rollback_after_failure(command, trust)?;
            return Err(error);
        }
        if let Err(error) = commit_transaction(&command.paths, trust) {
            rollback_after_failure(command, trust)?;
            return Err(error);
        }
        Ok(TransactionStateV1::Committed)
    }
    #[cfg(not(windows))]
    {
        let _ = (command, trust);
        Err(UpdaterError::new(
            UpdaterErrorCode::UnsupportedPlatform,
            "update activation requires Windows",
        ))
    }
}

pub fn restore_managed_mcp(
    command: &RecoveryCommand,
    trust: &ReleaseTrust,
) -> Result<ManagedMcpRestoration, UpdaterError> {
    let transaction = inspect_transaction(&command.paths, trust)?;
    let state = transaction.journal().state;
    let (manifest, version) = match state {
        TransactionStateV1::Committed => {
            (transaction.new_manifest(), &transaction.plan().new_version)
        }
        TransactionStateV1::BackupPrepared | TransactionStateV1::RolledBack => {
            (transaction.old_manifest(), &transaction.plan().old_version)
        }
        _ => {
            return Err(recovery(
                "managed MCP cannot be restored while installation state is uncertain",
            ));
        }
    };
    if !transaction.plan().managed_mcp_was_running {
        return Ok(ManagedMcpRestoration::NotRequired);
    }
    let executable = main_executable(command.paths.installation_root(), manifest)?;
    run_service_command(&executable, &["--json", "mcp", "service", "start"])?;
    let status = run_service_command(&executable, &["--json", "mcp", "service", "status"])?;
    validate_restored_status(
        &status,
        version,
        transaction.plan().managed_mcp_previous_instance_id,
    )?;
    Ok(ManagedMcpRestoration::Restored)
}

fn rollback_after_failure(
    command: &RecoveryCommand,
    trust: &ReleaseTrust,
) -> Result<(), UpdaterError> {
    rollback_transaction(&command.paths, trust)
        .map(|_| ())
        .map_err(|_| {
            UpdaterError::new(
                UpdaterErrorCode::Rollback,
                "update failed and automatic rollback did not complete",
            )
        })
}

fn run_installed_smoke(
    transaction: &crate::transaction::LoadedTransaction,
) -> Result<(), UpdaterError> {
    let executable = main_executable(
        transaction.paths().installation_root(),
        transaction.new_manifest(),
    )?;
    let output = bounded_process::run(
        &executable,
        &["--version"],
        transaction.paths().installation_root(),
        &[
            EnvironmentOverride {
                name: OsStr::new(INSTALLED_SMOKE_ENV),
                value: Some(OsStr::new("1")),
            },
            EnvironmentOverride {
                name: OsStr::new(MCP_RESTORE_ENV),
                value: None,
            },
        ],
        CHILD_COMMAND_TIMEOUT,
        MAX_CHILD_OUTPUT_BYTES,
    )
    .map_err(|_| activation("failed to launch installed release smoke check"))?;
    if output.timed_out {
        return Err(activation("installed release smoke check timed out"));
    }
    if output.stdout_truncated || output.stderr_truncated {
        return Err(activation(
            "installed release smoke output exceeds its limit",
        ));
    }
    if !output.status.success() || !output.stderr.is_empty() {
        return Err(activation("installed release smoke check failed"));
    }
    let stdout = std::str::from_utf8(&output.stdout)
        .map_err(|_| activation("installed release smoke output is not valid UTF-8"))?;
    let stdout = stdout
        .strip_suffix("\r\n")
        .or_else(|| stdout.strip_suffix('\n'))
        .unwrap_or(stdout);
    if stdout != format!("ah {}", transaction.plan().new_version) {
        return Err(activation(
            "installed release smoke version does not match the transaction",
        ));
    }
    Ok(())
}

fn main_executable(
    root: &Path,
    manifest: &ah_updater_core::ReleaseManifest,
) -> Result<PathBuf, UpdaterError> {
    let executables = manifest
        .files
        .iter()
        .filter(|file| file.purpose == FilePurpose::Executable)
        .collect::<Vec<_>>();
    let [executable] = executables.as_slice() else {
        return Err(activation(
            "installed release must contain exactly one main executable",
        ));
    };
    Ok(root.join(executable.path.split('/').collect::<PathBuf>()))
}

fn run_service_command(executable: &Path, arguments: &[&str]) -> Result<Vec<u8>, UpdaterError> {
    let cwd = executable
        .parent()
        .ok_or_else(|| recovery("installed executable has no parent directory"))?;
    let output = bounded_process::run(
        executable,
        arguments,
        cwd,
        &[
            EnvironmentOverride {
                name: OsStr::new(INSTALLED_SMOKE_ENV),
                value: None,
            },
            EnvironmentOverride {
                name: OsStr::new(MCP_RESTORE_ENV),
                value: Some(OsStr::new("1")),
            },
        ],
        CHILD_COMMAND_TIMEOUT,
        MAX_CHILD_OUTPUT_BYTES,
    )
    .map_err(|_| recovery("failed to launch installed managed MCP command"))?;
    if output.timed_out {
        return Err(recovery("installed managed MCP command timed out"));
    }
    if output.stdout_truncated || output.stderr_truncated {
        return Err(recovery(
            "installed managed MCP command output exceeds its limit",
        ));
    }
    if !output.status.success() || !output.stderr.is_empty() {
        return Err(recovery("installed managed MCP command failed"));
    }
    Ok(output.stdout)
}

fn validate_restored_status(
    bytes: &[u8],
    expected_version: &str,
    previous_instance_id: Option<uuid::Uuid>,
) -> Result<(), UpdaterError> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|_| recovery("managed MCP status is not valid JSON"))?;
    let readiness = value
        .get("readiness")
        .ok_or_else(|| recovery("managed MCP status is missing readiness"))?;
    let instance_id = readiness
        .get("instance_id")
        .and_then(serde_json::Value::as_str)
        .and_then(|value| uuid::Uuid::parse_str(value).ok())
        .ok_or_else(|| recovery("restored managed MCP has no valid instance identity"))?;
    if readiness.get("status").and_then(serde_json::Value::as_str) != Some("ready")
        || readiness.get("version").and_then(serde_json::Value::as_str) != Some(expected_version)
        || previous_instance_id == Some(instance_id)
    {
        return Err(recovery(
            "managed MCP did not return ready with the new version and instance identity",
        ));
    }
    Ok(())
}

fn activation(detail: &'static str) -> UpdaterError {
    UpdaterError::new(UpdaterErrorCode::Activation, detail)
}

fn recovery(detail: &'static str) -> UpdaterError {
    UpdaterError::new(UpdaterErrorCode::Recovery, detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restored_status_requires_new_ready_instance_and_version() {
        let previous = uuid::Uuid::new_v4();
        let current = uuid::Uuid::new_v4();
        let status = serde_json::json!({
            "readiness": {
                "status": "ready",
                "version": "1.2.0",
                "instance_id": current,
            }
        });
        let bytes = serde_json::to_vec(&status).unwrap();
        validate_restored_status(&bytes, "1.2.0", Some(previous)).unwrap();
        assert!(validate_restored_status(&bytes, "1.3.0", Some(previous)).is_err());
        assert!(validate_restored_status(&bytes, "1.2.0", Some(current)).is_err());
    }
}
