//! An exclusive lease on a named OS object, identified to callers by a path.
//!
//! One process holds it at a time, machine-wide, and the holder's handle can be
//! inherited by a child so the child can prove which lease it was given. That
//! is what the update handoff needs: the running `ah` holds the managed
//! service's lifecycle lease, launches the update helper, and the helper
//! verifies the inherited handle names the lease it expected before touching
//! anything.
//!
//! The lease is a named mutex rather than an open file, because a file lease
//! outlives a crashed holder on Windows and a mutex does not.
//!
//! Two things identify one lease, and the split is deliberate. `path` is what
//! every process calls it and what a parent tells a child. `mutex_name` is the
//! OS object, derived from the path by a rule that is a cross-version contract
//! between `ah` and the update helper - so the rule stays with them
//! (`ah_updater_core::lifecycle_mutex_name`) and this module is told the answer
//! rather than deciding it.
//!
//! Like the rest of this crate, the errors are [`std::io::Error`] and say
//! nothing about what the caller was doing:
//!
//! | Kind          | Means                                          |
//! |---------------|------------------------------------------------|
//! | `Unsupported` | this platform has no implementation            |
//! | `TimedOut`    | someone else held it for the whole timeout      |
//! | anything else | the OS refused to create the object            |

use std::{
    io,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

#[cfg(windows)]
use std::sync::Mutex;
#[cfg(windows)]
use windows_sys::Win32::Foundation::HANDLE;

const RETRY_INTERVAL: Duration = Duration::from_millis(25);

/// A held lease. Dropping it releases the lease.
#[derive(Debug)]
pub struct FileLease {
    path: PathBuf,
    #[cfg(windows)]
    mutex_handle: Mutex<HANDLE>,
}

impl FileLease {
    /// Take the lease if it is free, reporting `Ok(None)` if it is held.
    ///
    /// # Errors
    ///
    /// [`io::Error`] per the table in the module documentation. A lease held
    /// elsewhere is not an error.
    pub fn try_acquire(path: &Path, mutex_name: &str) -> io::Result<Option<Self>> {
        #[cfg(windows)]
        {
            try_acquire_windows(path, mutex_name)
        }

        #[cfg(not(windows))]
        {
            let _ = (path, mutex_name);
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "process leases are implemented for Windows only",
            ))
        }
    }

    /// Take the lease, waiting up to `timeout` for whoever holds it.
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::TimedOut`] when the lease is still held at the
    /// deadline; otherwise as [`Self::try_acquire`].
    pub fn acquire(path: &Path, mutex_name: &str, timeout: Duration) -> io::Result<Self> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(lease) = Self::try_acquire(path, mutex_name)? {
                return Ok(lease);
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("lease '{}' is held elsewhere", path.display()),
                ));
            }
            thread::sleep(RETRY_INTERVAL);
        }
    }

    /// The path this lease is named by, which is what a child process is told.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The inheritable handle, for handing the lease to a child process.
    #[cfg(windows)]
    pub fn raw_handle(&self) -> HANDLE {
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
fn try_acquire_windows(path: &Path, mutex_name: &str) -> io::Result<Option<FileLease>> {
    use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    use windows_sys::core::PCWSTR;

    let mutex_name_wide: Vec<u16> = mutex_name.encode_utf16().chain(Some(0)).collect();

    let handle =
        unsafe { CreateMutexW(std::ptr::null_mut(), 0, mutex_name_wide.as_ptr() as PCWSTR) };

    let error_code = unsafe { GetLastError() };
    if handle.is_null() {
        return Err(io::Error::from_raw_os_error(error_code as i32));
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

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// A named mutex, unlike a lock file, is owned by the open handle: it does
    /// not survive its holder and cannot be inherited by age.
    #[test]
    fn the_lease_is_owned_by_the_open_handle() {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("lifecycle.lock");
        let name = format!("Local\\ah-platform-test-{}", std::process::id());

        let first = FileLease::try_acquire(&path, &name).unwrap().unwrap();
        assert!(FileLease::try_acquire(&path, &name).unwrap().is_none());

        drop(first);

        let second = FileLease::try_acquire(&path, &name).unwrap().unwrap();
        assert_eq!(second.path(), path);
    }

    #[test]
    fn a_held_lease_times_out_rather_than_waiting_forever() {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("lifecycle.lock");
        let name = format!("Local\\ah-platform-timeout-{}", std::process::id());
        let _held = FileLease::try_acquire(&path, &name).unwrap().unwrap();

        let error = FileLease::acquire(&path, &name, Duration::from_millis(1)).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }

    /// The name is the identity, not the path: two paths naming the same mutex
    /// are the same lease.
    #[test]
    fn the_name_decides_which_lease_is_held() {
        let temp = tempfile::TempDir::new().unwrap();
        let name = format!("Local\\ah-platform-shared-{}", std::process::id());
        let _held = FileLease::try_acquire(&temp.path().join("one.lock"), &name)
            .unwrap()
            .unwrap();

        assert!(
            FileLease::try_acquire(&temp.path().join("two.lock"), &name)
                .unwrap()
                .is_none()
        );
    }
}
