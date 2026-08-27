use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::Duration,
};

use ah_updater_core::{
    HANDOFF_ARGUMENT_COUNT, HANDOFF_FLAGS, ReleaseTrust, TransactionStateV1, UpdaterError,
    UpdaterErrorCode,
};

use crate::{
    apply::{TransactionPaths, load_recovery_transaction, recover_transaction},
    process::quiesce_transaction_blockers,
};

const PARENT_EXIT_TIMEOUT: Duration = Duration::from_secs(30);
const BLOCKER_GRACE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryCommand {
    pub paths: TransactionPaths,
    pub lifecycle_lock: PathBuf,
    pub lifecycle_lock_handle: usize,
    pub handoff_event: String,
    pub parent_pid: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryExecution {
    AlreadyRunning,
    Recovered(TransactionStateV1),
}

pub fn parse_recovery_arguments(arguments: &[OsString]) -> Result<RecoveryCommand, UpdaterError> {
    parse_arguments(arguments, "recover")
}

pub fn parse_activation_arguments(arguments: &[OsString]) -> Result<RecoveryCommand, UpdaterError> {
    parse_arguments(arguments, "activate")
}

pub fn parse_rollback_arguments(arguments: &[OsString]) -> Result<RecoveryCommand, UpdaterError> {
    parse_arguments(arguments, "rollback")
}

fn parse_arguments(
    arguments: &[OsString],
    operation: &str,
) -> Result<RecoveryCommand, UpdaterError> {
    // The order is `ah_updater_core::HANDOFF_FLAGS`, which is also what `ah`
    // builds the command line from. Validating against it rather than against
    // literals is what keeps the two ends of a cross-version handoff from
    // drifting apart.
    if arguments.len() != HANDOFF_ARGUMENT_COUNT
        || arguments[0] != operation
        || arguments[HANDOFF_ARGUMENT_COUNT - 1].to_str().is_none()
        || HANDOFF_FLAGS
            .iter()
            .enumerate()
            .any(|(position, flag)| arguments[1 + position * 2] != *flag)
    {
        return Err(argument("update helper arguments are invalid"));
    }
    let lifecycle_lock_handle = arguments[10]
        .to_str()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|handle| *handle != 0)
        .ok_or_else(|| argument("update helper lifecycle lock handle is invalid"))?;
    let handoff_event = arguments[12]
        .to_str()
        .filter(|value| value.starts_with("Local\\AIHelper.Update.Handoff."))
        .filter(|value| value.len() <= 128)
        .ok_or_else(|| argument("update helper handoff event is invalid"))?
        .to_owned();
    let parent_pid = arguments[13]
        .to_str()
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|pid| *pid != 0)
        .ok_or_else(|| argument("update helper parent PID is invalid"))?;
    Ok(RecoveryCommand {
        paths: TransactionPaths::new(
            PathBuf::from(&arguments[2]),
            PathBuf::from(&arguments[4]),
            PathBuf::from(&arguments[6]),
        ),
        lifecycle_lock: PathBuf::from(&arguments[8]),
        lifecycle_lock_handle,
        handoff_event,
        parent_pid,
    })
}

pub fn execute_recovery(
    command: &RecoveryCommand,
    trust: &ReleaseTrust,
) -> Result<RecoveryExecution, UpdaterError> {
    #[cfg(windows)]
    {
        let Some(_lease) =
            RecoveryLease::try_acquire(&command.paths.transaction_root().join("recovery.lock"))?
        else {
            return Ok(RecoveryExecution::AlreadyRunning);
        };
        wait_for_process_exit(command.parent_pid, PARENT_EXIT_TIMEOUT)?;
        let transaction = load_recovery_transaction(&command.paths, trust)?;
        quiesce_transaction_blockers(&transaction, BLOCKER_GRACE_TIMEOUT)?;
        let transaction = recover_transaction(&command.paths, trust)?;
        Ok(RecoveryExecution::Recovered(transaction.journal().state))
    }
    #[cfg(not(windows))]
    {
        let _ = (command, trust);
        Err(UpdaterError::new(
            UpdaterErrorCode::UnsupportedPlatform,
            "update transaction recovery requires Windows",
        ))
    }
}

#[cfg(windows)]
struct RecoveryLease {
    handle: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
impl RecoveryLease {
    fn try_acquire(path: &Path) -> Result<Option<Self>, UpdaterError> {
        use std::{os::windows::ffi::OsStrExt as _, ptr};

        use windows_sys::Win32::{
            Foundation::{
                ERROR_LOCK_VIOLATION, ERROR_SHARING_VIOLATION, GENERIC_READ, GENERIC_WRITE,
                GetLastError, INVALID_HANDLE_VALUE,
            },
            Storage::FileSystem::{CreateFileW, FILE_ATTRIBUTE_NORMAL, OPEN_ALWAYS},
        };

        let wide = path
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                ptr::null(),
                OPEN_ALWAYS,
                FILE_ATTRIBUTE_NORMAL,
                ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            let code = unsafe { GetLastError() };
            if matches!(code, ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION) {
                return Ok(None);
            }
            return Err(recovery(
                "failed to acquire update transaction recovery lease",
            ));
        }
        Ok(Some(Self { handle }))
    }
}

#[cfg(windows)]
impl Drop for RecoveryLease {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::CloseHandle;

        unsafe {
            CloseHandle(self.handle);
        }
    }
}

#[cfg(windows)]
pub(crate) fn wait_for_process_exit(pid: u32, timeout: Duration) -> Result<(), UpdaterError> {
    use windows_sys::Win32::{
        Foundation::{
            CloseHandle, ERROR_INVALID_PARAMETER, GetLastError, WAIT_FAILED, WAIT_OBJECT_0,
            WAIT_TIMEOUT,
        },
        Storage::FileSystem::SYNCHRONIZE,
        System::Threading::{OpenProcess, WaitForSingleObject},
    };

    let handle = unsafe { OpenProcess(SYNCHRONIZE, 0, pid) };
    if handle.is_null() {
        let code = unsafe { GetLastError() };
        if code == ERROR_INVALID_PARAMETER {
            return Ok(());
        }
        return Err(recovery("failed to observe update recovery parent process"));
    }
    let milliseconds = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
    let result = unsafe { WaitForSingleObject(handle, milliseconds) };
    unsafe {
        CloseHandle(handle);
    }
    match result {
        WAIT_OBJECT_0 => Ok(()),
        WAIT_TIMEOUT => Err(recovery(
            "timed out waiting for update recovery parent process",
        )),
        WAIT_FAILED => Err(recovery("failed while waiting for update recovery parent")),
        _ => Err(recovery(
            "update recovery parent wait returned an unexpected result",
        )),
    }
}

fn argument(detail: &'static str) -> UpdaterError {
    UpdaterError::new(UpdaterErrorCode::Argument, detail)
}

fn recovery(detail: &'static str) -> UpdaterError {
    UpdaterError::new(UpdaterErrorCode::Recovery, detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_only_exact_recovery_contract() {
        let valid = [
            "recover",
            "--installation-root",
            r"C:\AIHelper",
            "--installation-state-root",
            r"C:\State",
            "--transaction-root",
            r"C:\State\transactions\00000000-0000-0000-0000-000000000001",
            "--lifecycle-lock",
            r"C:\State\managed-mcp\lifecycle.lock",
            "--lifecycle-lock-handle",
            "1234",
            "--handoff-event",
            "Local\\AIHelper.Update.Handoff.00000000-0000-0000-0000-000000000001",
            "42",
        ]
        .map(OsString::from);
        let parsed = parse_recovery_arguments(&valid).unwrap();
        assert_eq!(parsed.parent_pid, 42);
        assert_eq!(parsed.paths.installation_root(), Path::new(r"C:\AIHelper"));
        let mut activation = valid.clone();
        activation[0] = OsString::from("activate");
        assert_eq!(
            parse_activation_arguments(&activation).unwrap().parent_pid,
            42
        );
        let mut rollback = valid.clone();
        rollback[0] = OsString::from("rollback");
        assert_eq!(parse_rollback_arguments(&rollback).unwrap().parent_pid, 42);

        for invalid in [
            valid[..13].to_vec(),
            {
                let mut invalid = valid.to_vec();
                invalid[1] = OsString::from("--root");
                invalid
            },
            {
                let mut invalid = valid.to_vec();
                invalid[13] = OsString::from("0");
                invalid
            },
            {
                let mut invalid = valid.to_vec();
                invalid[10] = OsString::from("0");
                invalid
            },
        ] {
            assert!(parse_recovery_arguments(&invalid).is_err());
        }
    }
}
