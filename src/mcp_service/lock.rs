use std::{
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use crate::error::AppError;

const RETRY_INTERVAL: Duration = Duration::from_millis(25);

#[derive(Debug)]
pub struct FileLease {
    path: PathBuf,
    #[cfg(windows)]
    handle: windows::Win32::Foundation::HANDLE,
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
        self.handle.0
    }
}

#[cfg(windows)]
impl Drop for FileLease {
    fn drop(&mut self) {
        use windows::Win32::Foundation::CloseHandle;

        // SAFETY: this object exclusively owns the handle returned by
        // CreateFileW and closes it exactly once.
        let _ = unsafe { CloseHandle(self.handle) };
    }
}

#[cfg(windows)]
fn try_acquire_windows(path: &Path) -> Result<Option<FileLease>, AppError> {
    use std::os::windows::ffi::OsStrExt;

    use windows::{
        Win32::Storage::FileSystem::{
            CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
            FILE_SHARE_MODE, OPEN_ALWAYS,
        },
        core::PCWSTR,
    };

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|source| AppError::file_write(parent.to_path_buf(), source))?;
    }
    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    // SAFETY: the filename is NUL-terminated and remains alive for the call.
    let handle = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            FILE_GENERIC_READ.0 | FILE_GENERIC_WRITE.0,
            FILE_SHARE_MODE(0),
            None,
            OPEN_ALWAYS,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    };
    match handle {
        Ok(handle) => Ok(Some(FileLease {
            path: path.to_path_buf(),
            handle,
        })),
        Err(error) if is_busy_hresult(error.code().0) => Ok(None),
        Err(error) => Err(AppError::external(
            "MCP_SERVICE_STATE_INVALID",
            format!(
                "failed to open managed MCP lease '{}': {error}",
                path.display()
            ),
        )),
    }
}

#[cfg(windows)]
fn is_busy_hresult(value: i32) -> bool {
    matches!(value as u32, 0x8007_0020 | 0x8007_0021)
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
        assert!(path.exists());
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
