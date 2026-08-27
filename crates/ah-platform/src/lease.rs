//! An exclusive lease on a named OS object, identified to callers by a path.
//!
//! One process holds it at a time, machine-wide, and the holder's handle can be
//! inherited by a child so the child can prove which lease it was given. That
//! is what the update handoff needs: the running `ah` holds the managed
//! service's lifecycle lease, launches the update helper, and the helper
//! verifies the inherited handle names the lease it expected before touching
//! anything.
//!
//! On Windows the lease is a named mutex rather than an open file, because a
//! file lease outlives a crashed holder there and a mutex does not. On Unix it
//! is `flock` on a file, which the kernel releases when the descriptor closes -
//! the same "owned by the holder, gone when it dies" property, reached the
//! other way round.
//!
//! Two things identify one lease, and the split is deliberate. `path` is what
//! every process calls it and what a parent tells a child. `mutex_name` is the
//! OS object, derived from the path by a rule that is a cross-version contract
//! between `ah` and the update helper - so the rule stays with them
//! (`ah_updater_core::lifecycle_mutex_name`) and this module is told the answer
//! rather than deciding it.
//!
//! Which of the two is the identity differs by platform, and callers must not
//! rely on the difference: on Windows the name is (two paths naming one mutex
//! are one lease), on Unix the path is. Every caller derives the name from the
//! path, so the two agree in practice.
//!
//! Like the rest of this crate, the errors are [`std::io::Error`] and say
//! nothing about what the caller was doing:
//!
//! | Kind          | Means                                                   |
//! |---------------|---------------------------------------------------------|
//! | `Unsupported` | neither Windows nor Unix, so there is no implementation |
//! | `TimedOut`    | someone else held it for the whole timeout               |
//! | anything else | the OS refused to create the object                     |

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
    /// The descriptor holding the `flock`. Closing it is the release, so this
    /// field is the lease even though nothing reads it.
    #[cfg(unix)]
    _file: std::fs::File,
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

        #[cfg(unix)]
        {
            let _ = mutex_name;
            try_acquire_unix(path)
        }

        #[cfg(not(any(windows, unix)))]
        {
            let _ = (path, mutex_name);
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "process leases are implemented for Windows and Unix only",
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

/// The lease as an advisory exclusive lock on its own file.
///
/// `flock` is per open descriptor, so a second attempt from this same process
/// is refused exactly as another process's would be - which is what the
/// lifecycle's own tests rely on. The directory is created because the lease
/// can be asked for before anything else has written to the service's state
/// directory, and on Windows the mutex namespace needs no such preparation.
#[cfg(unix)]
fn try_acquire_unix(path: &Path) -> io::Result<Option<FileLease>> {
    use std::os::{fd::AsRawFd, unix::fs::OpenOptionsExt};

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        // The file is only a name for the lock. Truncating it would race a
        // holder that is writing nothing anyway.
        .truncate(false)
        .mode(0o600)
        .open(path)?;
    // SAFETY: the descriptor is owned by `file`, which outlives the call and,
    // when it is stored below, outlives the lease itself.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(Some(FileLease {
            path: path.to_path_buf(),
            _file: file,
        }));
    }
    let error = io::Error::last_os_error();
    if error.kind() == io::ErrorKind::WouldBlock {
        return Ok(None);
    }
    Err(error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    fn lease_name(label: &str) -> String {
        format!("Local\\ah-platform-{label}-{}", std::process::id())
    }

    /// Unix identifies the lease by its path, so the name is unused there.
    #[cfg(unix)]
    fn lease_name(_label: &str) -> String {
        String::new()
    }

    /// The lease is owned by whatever the holder keeps open - a mutex handle on
    /// Windows, a locked descriptor on Unix. Either way it does not survive its
    /// holder and cannot be inherited by age.
    #[test]
    fn the_lease_is_owned_by_the_open_handle() {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("lifecycle.lock");
        let name = lease_name("owned");

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
        let name = lease_name("timeout");
        let _held = FileLease::try_acquire(&path, &name).unwrap().unwrap();

        let error = FileLease::acquire(&path, &name, Duration::from_millis(1)).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }

    /// On Windows the name is the identity, not the path: two paths naming the
    /// same mutex are the same lease. Unix has no equivalent - see the module
    /// documentation.
    #[cfg(windows)]
    #[test]
    fn the_name_decides_which_lease_is_held() {
        let temp = tempfile::TempDir::new().unwrap();
        let name = lease_name("shared");
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
