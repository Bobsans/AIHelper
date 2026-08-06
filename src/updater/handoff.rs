use std::{
    ffi::{OsStr, c_void},
    mem::{self, size_of},
    os::windows::ffi::OsStrExt as _,
    path::Path,
    ptr::{null, null_mut},
    time::Duration,
};

use windows_sys::Win32::{
    Foundation::{
        CloseHandle, HANDLE, HANDLE_FLAG_INHERIT, SetHandleInformation, WAIT_FAILED, WAIT_OBJECT_0,
        WAIT_TIMEOUT,
    },
    System::Threading::{
        CREATE_NO_WINDOW, CreateEventW, CreateProcessW, DeleteProcThreadAttributeList,
        EXTENDED_STARTUPINFO_PRESENT, INFINITE, InitializeProcThreadAttributeList,
        PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROCESS_INFORMATION, STARTUPINFOEXW, TerminateProcess,
        UpdateProcThreadAttribute, WaitForSingleObject,
    },
};

use crate::{
    commands::run::windows_job::CREATE_PROCESS_LOCK, error::AppError, mcp_service::lock::FileLease,
};

const ACK_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) fn launch_recovery(
    helper: &Path,
    arguments: &[&OsStr],
    lease: &FileLease,
) -> Result<(), LaunchFailure> {
    launch_helper(helper, arguments, lease, "UPDATER_RECOVERY", "recovery")
}

pub(super) fn launch_activation(
    helper: &Path,
    arguments: &[&OsStr],
    lease: &FileLease,
) -> Result<(), LaunchFailure> {
    launch_helper(helper, arguments, lease, "UPDATER_ACTIVATION", "activation")
}

pub(super) fn launch_rollback(
    helper: &Path,
    arguments: &[&OsStr],
    lease: &FileLease,
) -> Result<(), LaunchFailure> {
    launch_helper(helper, arguments, lease, "UPDATER_ROLLBACK", "rollback")
}

fn launch_helper(
    helper: &Path,
    arguments: &[&OsStr],
    lease: &FileLease,
    error_code: &'static str,
    operation: &'static str,
) -> Result<(), LaunchFailure> {
    let event_name = format!("Local\\AIHelper.Update.Handoff.{}", uuid::Uuid::new_v4());
    let wide_event = wide_null(
        OsStr::new(&event_name),
        error_code,
        "update helper event name contains an embedded NUL",
    )
    .map_err(LaunchFailure::safe)?;
    let event = OwnedHandle::new(
        unsafe { CreateEventW(null(), 1, 0, wide_event.as_ptr()) },
        error_code,
        "failed to create update helper handoff event",
    )
    .map_err(LaunchFailure::safe)?;
    let raw_lease = lease.raw_handle();
    let mut attributes = AttributeList::new(error_code).map_err(LaunchFailure::safe)?;
    attributes
        .set_handle(&raw_lease, error_code)
        .map_err(LaunchFailure::safe)?;
    let mut startup: STARTUPINFOEXW = unsafe { mem::zeroed() };
    startup.StartupInfo.cb =
        u32::try_from(size_of::<STARTUPINFOEXW>()).expect("STARTUPINFOEXW size should fit in u32");
    startup.lpAttributeList = attributes.raw();

    let handle_value = (raw_lease as usize).to_string();
    let parent_pid = std::process::id().to_string();
    let mut argv = arguments.to_vec();
    argv.extend([
        OsStr::new("--lifecycle-lock"),
        lease.path().as_os_str(),
        OsStr::new("--lifecycle-lock-handle"),
        OsStr::new(&handle_value),
        OsStr::new("--handoff-event"),
        OsStr::new(&event_name),
        OsStr::new(&parent_pid),
    ]);
    let mut command_line =
        command_line(helper.as_os_str(), &argv, error_code).map_err(LaunchFailure::safe)?;
    let application = wide_null(
        helper.as_os_str(),
        error_code,
        "update helper path contains an embedded NUL",
    )
    .map_err(LaunchFailure::safe)?;
    let mut process_info: PROCESS_INFORMATION = unsafe { mem::zeroed() };
    let spawn_guard = CREATE_PROCESS_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let inherit_guard = InheritGuard::new(raw_lease, error_code).map_err(LaunchFailure::safe)?;
    let created = unsafe {
        CreateProcessW(
            application.as_ptr(),
            command_line.as_mut_ptr(),
            null(),
            null(),
            1,
            EXTENDED_STARTUPINFO_PRESENT | CREATE_NO_WINDOW,
            null(),
            null(),
            &startup.StartupInfo,
            &mut process_info,
        )
    };
    drop(inherit_guard);
    drop(spawn_guard);
    if created == 0 {
        return Err(LaunchFailure::safe(handoff_for(
            error_code,
            format!("failed to launch verified update {operation} helper"),
        )));
    }
    let process = OwnedHandle::new(
        process_info.hProcess,
        error_code,
        "update helper process handle is invalid",
    )
    .map_err(LaunchFailure::unsafe_state)?;
    let _thread = match OwnedHandle::new(
        process_info.hThread,
        error_code,
        "update helper thread handle is invalid",
    ) {
        Ok(thread) => thread,
        Err(error) => {
            return Err(match terminate_and_wait(&process, error_code, operation) {
                Ok(()) => LaunchFailure::safe(error),
                Err(error) => LaunchFailure::unsafe_state(error),
            });
        }
    };
    let wait = unsafe {
        WaitForSingleObject(
            event.raw(),
            u32::try_from(ACK_TIMEOUT.as_millis()).unwrap_or(u32::MAX),
        )
    };
    match wait {
        WAIT_OBJECT_0 => Ok(()),
        WAIT_TIMEOUT | WAIT_FAILED => failed_acknowledgement(
            &process,
            error_code,
            operation,
            "did not acknowledge lifecycle lock handoff",
        ),
        _ => failed_acknowledgement(
            &process,
            error_code,
            operation,
            "returned an invalid handoff acknowledgement",
        ),
    }
}

fn failed_acknowledgement(
    process: &OwnedHandle,
    error_code: &'static str,
    operation: &'static str,
    detail: &'static str,
) -> Result<(), LaunchFailure> {
    match terminate_and_wait(process, error_code, operation) {
        Ok(()) => Err(LaunchFailure::safe(handoff_for(
            error_code,
            format!("verified update {operation} helper {detail}"),
        ))),
        Err(error) => Err(LaunchFailure::unsafe_state(error)),
    }
}

fn terminate_and_wait(
    process: &OwnedHandle,
    error_code: &'static str,
    operation: &'static str,
) -> Result<(), AppError> {
    match unsafe { WaitForSingleObject(process.raw(), 0) } {
        WAIT_OBJECT_0 => return Ok(()),
        WAIT_TIMEOUT => {}
        WAIT_FAILED => {
            return Err(handoff_for(
                error_code,
                format!(
                    "failed to inspect verified update {operation} helper after handoff failure"
                ),
            ));
        }
        result => {
            return Err(handoff_for(
                error_code,
                format!("invalid verified update {operation} helper wait result {result}"),
            ));
        }
    }
    if unsafe { TerminateProcess(process.raw(), 1) } == 0 {
        if unsafe { WaitForSingleObject(process.raw(), 0) } == WAIT_OBJECT_0 {
            return Ok(());
        }
        return Err(handoff_for(
            error_code,
            format!("failed to terminate verified update {operation} helper after handoff failure"),
        ));
    }
    match unsafe { WaitForSingleObject(process.raw(), INFINITE) } {
        WAIT_OBJECT_0 => Ok(()),
        WAIT_FAILED => Err(handoff_for(
            error_code,
            format!("failed to confirm verified update {operation} helper termination"),
        )),
        result => Err(handoff_for(
            error_code,
            format!("invalid verified update {operation} helper termination wait result {result}"),
        )),
    }
}

fn command_line(
    program: &OsStr,
    arguments: &[&OsStr],
    error_code: &'static str,
) -> Result<Vec<u16>, AppError> {
    let mut output = Vec::new();
    append_quoted(&mut output, program, error_code)?;
    for argument in arguments {
        output.push(b' ' as u16);
        append_quoted(&mut output, argument, error_code)?;
    }
    output.push(0);
    Ok(output)
}

fn append_quoted(
    output: &mut Vec<u16>,
    value: &OsStr,
    error_code: &'static str,
) -> Result<(), AppError> {
    let units = value.encode_wide().collect::<Vec<_>>();
    if units.contains(&0) {
        return Err(handoff_for(
            error_code,
            "update helper argument contains an embedded NUL",
        ));
    }
    output.push(b'"' as u16);
    let mut backslashes = 0usize;
    for unit in units {
        if unit == b'\\' as u16 {
            backslashes += 1;
        } else if unit == b'"' as u16 {
            output.extend(std::iter::repeat_n(b'\\' as u16, backslashes * 2 + 1));
            output.push(unit);
            backslashes = 0;
        } else {
            output.extend(std::iter::repeat_n(b'\\' as u16, backslashes));
            output.push(unit);
            backslashes = 0;
        }
    }
    output.extend(std::iter::repeat_n(b'\\' as u16, backslashes * 2));
    output.push(b'"' as u16);
    Ok(())
}

fn wide_null(
    value: &OsStr,
    error_code: &'static str,
    detail: &'static str,
) -> Result<Vec<u16>, AppError> {
    let mut wide = value.encode_wide().collect::<Vec<_>>();
    if wide.contains(&0) {
        return Err(handoff_for(error_code, detail));
    }
    wide.push(0);
    Ok(wide)
}

fn handoff_for(code: &'static str, detail: impl Into<String>) -> AppError {
    AppError::external(code, detail)
}

pub(super) struct LaunchFailure {
    error: AppError,
    cleanup_safe: bool,
}

impl LaunchFailure {
    fn safe(error: AppError) -> Self {
        Self {
            error,
            cleanup_safe: true,
        }
    }

    fn unsafe_state(error: AppError) -> Self {
        Self {
            error,
            cleanup_safe: false,
        }
    }

    pub(super) fn cleanup_safe(&self) -> bool {
        self.cleanup_safe
    }

    pub(super) fn into_error(self) -> AppError {
        self.error
    }
}

struct AttributeList {
    _storage: Vec<usize>,
    raw: *mut c_void,
}

impl AttributeList {
    fn new(error_code: &'static str) -> Result<Self, AppError> {
        let mut bytes = 0usize;
        unsafe { InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut bytes) };
        if bytes == 0 {
            return Err(handoff_for(
                error_code,
                "failed to size update helper process attributes",
            ));
        }
        let mut storage = vec![0usize; bytes.div_ceil(size_of::<usize>())];
        let raw = storage.as_mut_ptr().cast();
        if unsafe { InitializeProcThreadAttributeList(raw, 1, 0, &mut bytes) } == 0 {
            return Err(handoff_for(
                error_code,
                "failed to initialize update helper process attributes",
            ));
        }
        Ok(Self {
            _storage: storage,
            raw,
        })
    }

    fn set_handle(&mut self, handle: &HANDLE, error_code: &'static str) -> Result<(), AppError> {
        if unsafe {
            UpdateProcThreadAttribute(
                self.raw,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                (handle as *const HANDLE).cast_mut().cast(),
                size_of::<HANDLE>(),
                null_mut(),
                null_mut(),
            )
        } == 0
        {
            return Err(handoff_for(
                error_code,
                "failed to allowlist lifecycle lock handle",
            ));
        }
        Ok(())
    }

    fn raw(&self) -> *mut c_void {
        self.raw
    }
}

impl Drop for AttributeList {
    fn drop(&mut self) {
        unsafe { DeleteProcThreadAttributeList(self.raw) };
    }
}

struct InheritGuard(HANDLE);

impl InheritGuard {
    fn new(handle: HANDLE, error_code: &'static str) -> Result<Self, AppError> {
        if unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) } == 0 {
            Err(handoff_for(
                error_code,
                "failed to make lifecycle lock handle inheritable",
            ))
        } else {
            Ok(Self(handle))
        }
    }
}

impl Drop for InheritGuard {
    fn drop(&mut self) {
        unsafe { SetHandleInformation(self.0, HANDLE_FLAG_INHERIT, 0) };
    }
}

struct OwnedHandle(HANDLE);

impl OwnedHandle {
    fn new(
        handle: HANDLE,
        error_code: &'static str,
        detail: &'static str,
    ) -> Result<Self, AppError> {
        if handle.is_null() {
            Err(handoff_for(error_code, detail))
        } else {
            Ok(Self(handle))
        }
    }

    fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CloseHandle(self.0) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_windows_arguments_without_trailing_backslash_escape() {
        let line = command_line(
            OsStr::new(r"C:\Program Files\ah-update-helper.exe"),
            &[OsStr::new(r"C:\State\")],
            "UPDATER_ACTIVATION",
        )
        .unwrap();
        let rendered = String::from_utf16(&line[..line.len() - 1]).unwrap();
        assert_eq!(
            rendered,
            r#""C:\Program Files\ah-update-helper.exe" "C:\State\\""#
        );
    }

    #[test]
    fn launch_failure_keeps_parent_lifecycle_lease() {
        let temp = tempfile::TempDir::new().unwrap();
        let lock = temp.path().join("lifecycle.lock");
        let lease = FileLease::try_acquire(&lock).unwrap().unwrap();

        let error = launch_recovery(&temp.path().join("missing-helper.exe"), &[], &lease)
            .unwrap_err()
            .into_error();

        assert_eq!(error.code(), "UPDATER_RECOVERY");
        assert!(FileLease::try_acquire(&lock).unwrap().is_none());
        drop(lease);
        assert!(FileLease::acquire(&lock, Duration::from_secs(1)).is_ok());
    }

    #[test]
    fn setup_failures_keep_operation_error_code() {
        use std::os::windows::ffi::OsStringExt as _;

        let temp = tempfile::TempDir::new().unwrap();
        let lock = temp.path().join("lifecycle.lock");
        let lease = FileLease::try_acquire(&lock).unwrap().unwrap();
        let invalid = std::ffi::OsString::from_wide(&[b'x' as u16, 0, b'y' as u16]);
        let helper = Path::new(&invalid);

        for (error, code) in [
            (
                launch_activation(helper, &[], &lease)
                    .unwrap_err()
                    .into_error(),
                "UPDATER_ACTIVATION",
            ),
            (
                launch_rollback(helper, &[], &lease)
                    .unwrap_err()
                    .into_error(),
                "UPDATER_ROLLBACK",
            ),
            (
                launch_recovery(helper, &[], &lease)
                    .unwrap_err()
                    .into_error(),
                "UPDATER_RECOVERY",
            ),
        ] {
            assert_eq!(error.code(), code);
        }
    }
}
