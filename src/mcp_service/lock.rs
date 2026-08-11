use std::{
    path::{Path, PathBuf},
    sync::Mutex,
    thread,
    time::{Duration, Instant},
};

use crate::error::AppError;
use windows_sys::Win32::Foundation::HANDLE;

const RETRY_INTERVAL: Duration = Duration::from_millis(25);

#[derive(Debug)]
pub struct FileLease {
    path: PathBuf,
    #[cfg(windows)]
    mutex_handle: Mutex<HANDLE>,
}

impl FileLease {
    pub fn try_acquire(path: &Path) -> Result<Option<Self>, AppError> {
        #[cfg(windows)]
        {
            try_acquire_windows(path)
        }

        #[cfg(not(windows))]
        {
            let _ = path;
            Err(AppError::external(
                "MCP_SERVICE_UNSUPPORTED_PLATFORM",
                "managed MCP service lifecycle is supported only on Windows",
            ))
        }
    }

    pub fn acquire(path: &Path, timeout: Duration) -> Result<Self, AppError> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(lease) = Self::try_acquire(path)? {
                return Ok(lease);
            }
            if Instant::now() >= deadline {
                return Err(AppError::external(
                    "MCP_SERVICE_BUSY",
                    format!(
                        "timed out waiting for managed MCP lifecycle lease '{}'",
                        path.display()
                    ),
                ));
            }
            thread::sleep(RETRY_INTERVAL);
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    #[cfg(windows)]
    pub(crate) fn raw_handle(&self) -> windows_sys::Win32::Foundation::HANDLE {
        *self.mutex_handle.lock().unwrap()
    }
}

#[cfg(windows)]
impl Drop for FileLease {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::CloseHandle;

        let handle = *self.mutex_handle.lock().unwrap();
        if !handle.is_null() {
            let _ = unsafe { CloseHandle(handle) };
        }
    }
}

#[cfg(windows)]
fn try_acquire_windows(path: &Path) -> Result<Option<FileLease>, AppError> {
    use std::sync::Mutex;

    use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    use windows_sys::core::PCWSTR;

    // Convert path to a valid Windows mutex name (no backslashes, limited length)
    let mutex_name = format!(
        "Global\\AIHelper-MCP-{}",
        path.to_string_lossy().replace(['\\', '/', ':'], "-")
    );
    let mutex_name_wide: Vec<u16> = mutex_name.encode_utf16().chain(Some(0)).collect();

    let handle =
        unsafe { CreateMutexW(std::ptr::null_mut(), 0, mutex_name_wide.as_ptr() as PCWSTR) };

    let error_code = unsafe { GetLastError() };
    if handle.is_null() {
        return Err(AppError::external(
            "MCP_SERVICE_STATE_INVALID",
            format!(
                "failed to create managed MCP mutex '{}': {}",
                path.display(),
                std::io::Error::from_raw_os_error(error_code as i32)
            ),
        ));
    }

    if error_code == ERROR_ALREADY_EXISTS {
        unsafe { windows_sys::Win32::Foundation::CloseHandle(handle) };
        return Ok(None);
    }

    Ok(Some(FileLease {
        path: path.to_path_buf(),
        mutex_handle: Mutex::new(handle),
    }))
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    use super::*;

    #[cfg(windows)]
    #[test]
    fn lease_is_owned_by_the_open_handle_not_file_age() {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("lifecycle.lock");
        let first = FileLease::try_acquire(&path).unwrap().unwrap();
        // With named mutex, no lock file is created; test intra-process blocking
        assert!(FileLease::try_acquire(&path).unwrap().is_none());
        drop(first);
        let second = FileLease::try_acquire(&path).unwrap().unwrap();
        assert_eq!(second.path(), path);
    }

    #[cfg(windows)]
    #[test]
    fn bounded_acquire_reports_stable_busy_diagnostic() {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("lifecycle.lock");
        let _first = FileLease::try_acquire(&path).unwrap().unwrap();
        let error = FileLease::acquire(&path, Duration::from_millis(1)).unwrap_err();
        assert_eq!(error.code(), "MCP_SERVICE_BUSY");
    }
}
