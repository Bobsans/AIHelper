use std::path::Path;

use ah_updater_core::{UpdaterError, UpdaterErrorCode};

#[cfg(windows)]
pub struct HandoffLease {
    handle: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(not(windows))]
pub struct HandoffLease;

impl HandoffLease {
    pub fn claim(
        handle: usize,
        expected_path: &Path,
        event_name: &str,
    ) -> Result<Self, UpdaterError> {
        #[cfg(windows)]
        {
            windows::claim(handle, expected_path, event_name)
        }
        #[cfg(not(windows))]
        {
            let _ = (handle, expected_path, event_name);
            Err(UpdaterError::new(
                UpdaterErrorCode::UnsupportedPlatform,
                "lifecycle lock handoff requires Windows",
            ))
        }
    }
}

#[cfg(windows)]
impl Drop for HandoffLease {
    fn drop(&mut self) {
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.handle) };
    }
}

#[cfg(windows)]
mod windows {
    use std::{os::windows::ffi::OsStrExt as _, path::Path, ptr::null_mut};

    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE},
        Storage::FileSystem::{FILE_TYPE_DISK, GetFileType, GetFinalPathNameByHandleW},
        System::Threading::{EVENT_MODIFY_STATE, OpenEventW, SetEvent},
    };

    use super::{HandoffLease, handoff};

    pub(super) fn claim(
        raw: usize,
        expected_path: &Path,
        event_name: &str,
    ) -> Result<HandoffLease, ah_updater_core::UpdaterError> {
        let handle = raw as HANDLE;
        if handle.is_null() || unsafe { GetFileType(handle) } != FILE_TYPE_DISK {
            return Err(handoff("inherited lifecycle lock handle is invalid"));
        }
        let actual = final_path(handle)?;
        if normalize(&actual) != normalize(&expected_path.to_string_lossy()) {
            return Err(handoff(
                "inherited lifecycle lock handle does not match the expected path",
            ));
        }
        signal_event(event_name)?;
        Ok(HandoffLease { handle })
    }

    fn final_path(handle: HANDLE) -> Result<String, ah_updater_core::UpdaterError> {
        let needed = unsafe { GetFinalPathNameByHandleW(handle, null_mut(), 0, 0) };
        if needed == 0 || needed > 32_768 {
            return Err(handoff("failed to resolve inherited lifecycle lock path"));
        }
        let mut buffer = vec![0_u16; needed as usize + 1];
        let written = unsafe {
            GetFinalPathNameByHandleW(handle, buffer.as_mut_ptr(), buffer.len() as u32, 0)
        };
        if written == 0 || written as usize >= buffer.len() {
            return Err(handoff("failed to read inherited lifecycle lock path"));
        }
        String::from_utf16(&buffer[..written as usize])
            .map_err(|_| handoff("inherited lifecycle lock path is invalid UTF-16"))
    }

    fn signal_event(name: &str) -> Result<(), ah_updater_core::UpdaterError> {
        let wide = std::ffi::OsStr::new(name)
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let event = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, wide.as_ptr()) };
        if event.is_null() {
            return Err(handoff("failed to open lifecycle lock handoff event"));
        }
        let signaled = unsafe { SetEvent(event) };
        unsafe { CloseHandle(event) };
        if signaled == 0 {
            Err(handoff("failed to acknowledge lifecycle lock handoff"))
        } else {
            Ok(())
        }
    }

    fn normalize(path: &str) -> String {
        path.strip_prefix(r"\\?\UNC\")
            .map(|path| format!(r"\\{path}"))
            .or_else(|| path.strip_prefix(r"\\?\").map(str::to_owned))
            .unwrap_or_else(|| path.to_owned())
            .replace('/', r"\")
            .trim_end_matches('\u{5c}')
            .to_lowercase()
    }
}

fn handoff(detail: &'static str) -> UpdaterError {
    UpdaterError::new(UpdaterErrorCode::Recovery, detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_platform_fails_closed() {
        #[cfg(not(windows))]
        assert_eq!(
            HandoffLease::claim(1, Path::new("/tmp/lifecycle.lock"), "event")
                .unwrap_err()
                .code(),
            UpdaterErrorCode::UnsupportedPlatform
        );
    }

    #[cfg(windows)]
    #[test]
    fn claims_exact_disk_handle_and_acknowledges_event() {
        use std::{os::windows::ffi::OsStrExt as _, ptr::null};

        use windows_sys::Win32::{
            Foundation::{
                CloseHandle, GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
            },
            Storage::FileSystem::{CreateFileW, FILE_ATTRIBUTE_NORMAL, OPEN_ALWAYS},
            System::Threading::{CreateEventW, WaitForSingleObject},
        };

        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("lifecycle.lock");
        let wide_path = path
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let handle = unsafe {
            CreateFileW(
                wide_path.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                null(),
                OPEN_ALWAYS,
                FILE_ATTRIBUTE_NORMAL,
                std::ptr::null_mut(),
            )
        };
        assert_ne!(handle, INVALID_HANDLE_VALUE);
        let event_name = format!("Local\\AIHelper.Update.Handoff.test-{}", std::process::id());
        let wide_event = std::ffi::OsStr::new(&event_name)
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let event = unsafe { CreateEventW(null(), 1, 0, wide_event.as_ptr()) };
        assert!(!event.is_null());

        let lease = HandoffLease::claim(handle as usize, &path, &event_name).unwrap();

        assert_eq!(unsafe { WaitForSingleObject(event, 0) }, WAIT_OBJECT_0);
        let blocked = unsafe {
            CreateFileW(
                wide_path.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                null(),
                OPEN_ALWAYS,
                FILE_ATTRIBUTE_NORMAL,
                std::ptr::null_mut(),
            )
        };
        assert_eq!(blocked, INVALID_HANDLE_VALUE);
        drop(lease);
        let reopened = unsafe {
            CreateFileW(
                wide_path.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                null(),
                OPEN_ALWAYS,
                FILE_ATTRIBUTE_NORMAL,
                std::ptr::null_mut(),
            )
        };
        assert_ne!(reopened, INVALID_HANDLE_VALUE);
        unsafe { CloseHandle(reopened) };
        unsafe { CloseHandle(event) };
    }
}
