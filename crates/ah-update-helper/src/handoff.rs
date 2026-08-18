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
    use std::{os::windows::ffi::OsStrExt as _, path::Path};

    use ah_updater_core::lifecycle_mutex_name;
    use windows_sys::Win32::{
        Foundation::{CloseHandle, CompareObjectHandles, HANDLE},
        Storage::FileSystem::SYNCHRONIZE,
        System::Threading::{EVENT_MODIFY_STATE, OpenEventW, OpenMutexW, SetEvent},
    };

    use super::{HandoffLease, handoff};

    pub(super) fn claim(
        raw: usize,
        expected_path: &Path,
        event_name: &str,
    ) -> Result<HandoffLease, ah_updater_core::UpdaterError> {
        let handle = raw as HANDLE;
        if handle.is_null() {
            return Err(handoff("inherited lifecycle lock handle is invalid"));
        }
        let mutex_name = lifecycle_mutex_name(expected_path);
        let wide_mutex = std::ffi::OsStr::new(&mutex_name)
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let expected = unsafe { OpenMutexW(SYNCHRONIZE, 0, wide_mutex.as_ptr()) };
        if expected.is_null() {
            return Err(handoff("failed to open expected lifecycle lock mutex"));
        }
        let matches = unsafe { CompareObjectHandles(handle, expected) } != 0;
        unsafe { CloseHandle(expected) };
        if !matches {
            return Err(handoff(
                "inherited lifecycle lock handle does not match the expected path",
            ));
        }
        signal_event(event_name)?;
        Ok(HandoffLease { handle })
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
}

fn handoff(detail: &'static str) -> UpdaterError {
    UpdaterError::new(UpdaterErrorCode::Recovery, detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(windows))]
    #[test]
    fn unsupported_platform_fails_closed() {
        let error = HandoffLease::claim(1, Path::new("/tmp/lifecycle.lock"), "event")
            .err()
            .expect("unsupported platform must reject lifecycle handoff");
        assert_eq!(error.code(), UpdaterErrorCode::UnsupportedPlatform);
    }

    #[cfg(windows)]
    #[test]
    fn claims_exact_named_mutex_handle_and_acknowledges_event() {
        use std::{os::windows::ffi::OsStrExt as _, ptr::null};

        use ah_updater_core::lifecycle_mutex_name;
        use windows_sys::Win32::{
            Foundation::{CloseHandle, WAIT_OBJECT_0},
            System::Threading::{CreateEventW, CreateMutexW, WaitForSingleObject},
        };

        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("lifecycle.lock");
        let mutex_name = lifecycle_mutex_name(&path);
        let wide_mutex = std::ffi::OsStr::new(&mutex_name)
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let handle = unsafe { CreateMutexW(null(), 0, wide_mutex.as_ptr()) };
        assert!(!handle.is_null());
        let event_name = format!("Local\\AIHelper.Update.Handoff.test-{}", std::process::id());
        let wide_event = std::ffi::OsStr::new(&event_name)
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let event = unsafe { CreateEventW(null(), 1, 0, wide_event.as_ptr()) };
        assert!(!event.is_null());

        let lease = HandoffLease::claim(handle as usize, &path, &event_name).unwrap();

        assert_eq!(unsafe { WaitForSingleObject(event, 0) }, WAIT_OBJECT_0);
        drop(lease);
        unsafe { CloseHandle(event) };
    }

    #[cfg(windows)]
    #[test]
    fn rejects_named_mutex_for_a_different_lifecycle_path() {
        use std::{os::windows::ffi::OsStrExt as _, ptr::null};

        use ah_updater_core::lifecycle_mutex_name;
        use windows_sys::Win32::{
            Foundation::CloseHandle,
            System::Threading::{CreateEventW, CreateMutexW},
        };

        let temp = tempfile::TempDir::new().unwrap();
        let expected = temp.path().join("expected.lock");
        let other = temp.path().join("other.lock");
        let mutex_name = lifecycle_mutex_name(&other);
        let wide_mutex = std::ffi::OsStr::new(&mutex_name)
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let handle = unsafe { CreateMutexW(null(), 0, wide_mutex.as_ptr()) };
        assert!(!handle.is_null());
        let event_name = format!(
            "Local\\AIHelper.Update.Handoff.mismatch-{}",
            std::process::id()
        );
        let wide_event = std::ffi::OsStr::new(&event_name)
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let event = unsafe { CreateEventW(null(), 1, 0, wide_event.as_ptr()) };
        assert!(!event.is_null());

        let error = HandoffLease::claim(handle as usize, &expected, &event_name)
            .err()
            .expect("mismatched mutex must be rejected");

        assert_eq!(error.code(), UpdaterErrorCode::Recovery);
        unsafe { CloseHandle(handle) };
        unsafe { CloseHandle(event) };
    }
}
