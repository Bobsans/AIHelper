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
        EXTENDED_STARTUPINFO_PRESENT, InitializeProcThreadAttributeList,
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
) -> Result<(), AppError> {
    let event_name = format!("Local\\AIHelper.Update.Handoff.{}", uuid::Uuid::new_v4());
    let wide_event = wide_null(OsStr::new(&event_name))?;
    let event = OwnedHandle::new(unsafe { CreateEventW(null(), 1, 0, wide_event.as_ptr()) })?;
    let raw_lease = lease.raw_handle();
    let mut attributes = AttributeList::new()?;
    attributes.set_handle(&raw_lease)?;
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
    let mut command_line = command_line(helper.as_os_str(), &argv)?;
    let application = wide_null(helper.as_os_str())?;
    let mut process_info: PROCESS_INFORMATION = unsafe { mem::zeroed() };
    let spawn_guard = CREATE_PROCESS_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let inherit_guard = InheritGuard::new(raw_lease)?;
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
        return Err(handoff("failed to launch verified update recovery helper"));
    }
    let process = OwnedHandle::new(process_info.hProcess)?;
    let _thread = OwnedHandle::new(process_info.hThread)?;
    let wait = unsafe {
        WaitForSingleObject(
            event.raw(),
            u32::try_from(ACK_TIMEOUT.as_millis()).unwrap_or(u32::MAX),
        )
    };
    match wait {
        WAIT_OBJECT_0 => Ok(()),
        WAIT_TIMEOUT | WAIT_FAILED => {
            unsafe { TerminateProcess(process.raw(), 1) };
            Err(handoff(
                "verified update recovery helper did not acknowledge lifecycle lock handoff",
            ))
        }
        _ => {
            unsafe { TerminateProcess(process.raw(), 1) };
            Err(handoff(
                "verified update recovery helper returned an invalid handoff acknowledgement",
            ))
        }
    }
}

fn command_line(program: &OsStr, arguments: &[&OsStr]) -> Result<Vec<u16>, AppError> {
    let mut output = Vec::new();
    append_quoted(&mut output, program)?;
    for argument in arguments {
        output.push(b' ' as u16);
        append_quoted(&mut output, argument)?;
    }
    output.push(0);
    Ok(output)
}

fn append_quoted(output: &mut Vec<u16>, value: &OsStr) -> Result<(), AppError> {
    let units = value.encode_wide().collect::<Vec<_>>();
    if units.contains(&0) {
        return Err(handoff("update helper argument contains an embedded NUL"));
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

fn wide_null(value: &OsStr) -> Result<Vec<u16>, AppError> {
    let mut wide = value.encode_wide().collect::<Vec<_>>();
    if wide.contains(&0) {
        return Err(handoff("update helper path contains an embedded NUL"));
    }
    wide.push(0);
    Ok(wide)
}

fn handoff(detail: &'static str) -> AppError {
    AppError::external("UPDATER_RECOVERY", detail)
}

struct AttributeList {
    _storage: Vec<usize>,
    raw: *mut c_void,
}

impl AttributeList {
    fn new() -> Result<Self, AppError> {
        let mut bytes = 0usize;
        unsafe { InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut bytes) };
        if bytes == 0 {
            return Err(handoff("failed to size update helper process attributes"));
        }
        let mut storage = vec![0usize; bytes.div_ceil(size_of::<usize>())];
        let raw = storage.as_mut_ptr().cast();
        if unsafe { InitializeProcThreadAttributeList(raw, 1, 0, &mut bytes) } == 0 {
            return Err(handoff(
                "failed to initialize update helper process attributes",
            ));
        }
        Ok(Self {
            _storage: storage,
            raw,
        })
    }

    fn set_handle(&mut self, handle: &HANDLE) -> Result<(), AppError> {
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
            return Err(handoff("failed to allowlist lifecycle lock handle"));
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
    fn new(handle: HANDLE) -> Result<Self, AppError> {
        if unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) } == 0 {
            Err(handoff("failed to make lifecycle lock handle inheritable"))
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
    fn new(handle: HANDLE) -> Result<Self, AppError> {
        if handle.is_null() {
            Err(handoff("failed to create update helper handoff handle"))
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

        let error =
            launch_recovery(&temp.path().join("missing-helper.exe"), &[], &lease).unwrap_err();

        assert_eq!(error.code(), "UPDATER_RECOVERY");
        assert!(FileLease::try_acquire(&lock).unwrap().is_none());
        drop(lease);
        assert!(FileLease::try_acquire(&lock).unwrap().is_some());
    }
}
