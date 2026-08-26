//! File-system primitives whose implementation differs per platform.

use std::{
    fs::Metadata,
    io,
    path::Path,
    time::{Duration, Instant},
};

/// Whether the file is a reparse point — a junction, a mount point, or any
/// other redirection Windows resolves on open.
///
/// Load-bearing rather than cosmetic: an installed file that is a reparse point
/// points somewhere the signature did not cover. Always false off Windows,
/// which has no such thing; symlinks are checked separately through
/// [`Metadata::file_type`].
#[must_use]
pub fn is_reparse_point(metadata: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt as _;

        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        let _ = metadata;
        false
    }
}

/// Whether the file has exactly one name on disk.
///
/// The other half of the same check: a second hard link is another way for a
/// verified file to be written through a name nobody verified. Windows has to
/// open the file to find out; Unix reads it from the metadata already in hand.
///
/// A platform that is neither answers `true`: there is no way to create a
/// second link there either.
///
/// # Errors
///
/// [`io::Error`] when the file cannot be opened or inspected.
pub fn has_single_hard_link(path: &Path, metadata: &Metadata) -> io::Result<bool> {
    #[cfg(windows)]
    {
        use std::{fs::File, mem::MaybeUninit, os::windows::io::AsRawHandle as _};

        use windows_sys::Win32::{
            Foundation::HANDLE,
            Storage::FileSystem::{BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle},
        };

        let _ = metadata;
        let file = File::open(path)?;
        let mut information = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
        // SAFETY: the handle is open for the duration of the call and the
        // out-parameter points at a correctly sized, aligned allocation.
        let succeeded = unsafe {
            GetFileInformationByHandle(file.as_raw_handle() as HANDLE, information.as_mut_ptr())
        };
        if succeeded == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the call above reported success, so the structure is filled.
        let information = unsafe { information.assume_init() };
        Ok(information.nNumberOfLinks == 1)
    }
    #[cfg(all(unix, not(windows)))]
    {
        use std::os::unix::fs::MetadataExt as _;

        let _ = path;
        Ok(metadata.nlink() == 1)
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = (path, metadata);
        Ok(true)
    }
}

/// Replace `destination` with `source`, atomically as far as the platform
/// allows.
///
/// # Errors
///
/// [`io::Error`] from the underlying rename.
pub fn replace(source: &Path, destination: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Storage::FileSystem::{
            MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
        };

        let source = wide(source);
        let destination = wide(destination);
        // SAFETY: both buffers are NUL-terminated and outlive the call.
        let succeeded = unsafe {
            MoveFileExW(
                source.as_ptr(),
                destination.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        };
        if succeeded == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        std::fs::rename(source, destination)
    }
}

/// Replace `destination` with `source`, waiting out a reader that holds it.
///
/// Windows refuses a replacement while another process has the file open, and
/// two `ah` processes writing the same store is ordinary rather than
/// exceptional. Everywhere else a rename does not fail that way, so the retry
/// never runs.
///
/// # Errors
///
/// [`io::Error`] from the last attempt, once the deadline passes or the failure
/// is not one waiting could fix.
pub fn replace_waiting_for_readers(
    source: &Path,
    destination: &Path,
    timeout: Duration,
    interval: Duration,
) -> io::Result<()> {
    const ERROR_ACCESS_DENIED: i32 = 5;
    const ERROR_SHARING_VIOLATION: i32 = 32;

    let deadline = Instant::now() + timeout;
    loop {
        let Err(error) = replace(source, destination) else {
            return Ok(());
        };
        let retryable = matches!(
            error.raw_os_error(),
            Some(ERROR_ACCESS_DENIED | ERROR_SHARING_VIOLATION)
        );
        if !retryable || Instant::now() >= deadline {
            return Err(error);
        }
        std::thread::sleep(interval);
    }
}

/// Flush the directory entry a rename just created.
///
/// A rename is only durable once the *directory* is on disk, which Unix will
/// not do implicitly. Windows writes the entry through as part of the move, so
/// there is nothing to flush.
///
/// # Errors
///
/// [`io::Error`] when the directory cannot be opened or synced. Most callers
/// ignore it: a lost flush costs durability across a power failure, not
/// correctness now.
pub fn sync_directory(directory: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        std::fs::File::open(directory)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = directory;
        Ok(())
    }
}

#[cfg(windows)]
fn wide(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt as _;

    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn a_plain_file_is_neither_a_reparse_point_nor_multiply_linked() {
        let temp = tempfile::TempDir::new().expect("temporary dir should be created");
        let path = temp.path().join("plain.txt");
        fs::write(&path, "contents").expect("file should be written");
        let metadata = fs::symlink_metadata(&path).expect("metadata should be readable");

        assert!(!is_reparse_point(&metadata));
        assert!(
            has_single_hard_link(&path, &metadata).expect("link count should be readable"),
            "a file just created has exactly one name"
        );
    }

    #[test]
    fn a_second_hard_link_is_visible() {
        let temp = tempfile::TempDir::new().expect("temporary dir should be created");
        let path = temp.path().join("linked.txt");
        fs::write(&path, "contents").expect("file should be written");
        let link = temp.path().join("other-name.txt");
        if fs::hard_link(&path, &link).is_err() {
            // Some file systems refuse hard links; the check is meaningless there.
            return;
        }
        let metadata = fs::symlink_metadata(&path).expect("metadata should be readable");

        assert!(
            !has_single_hard_link(&path, &metadata).expect("link count should be readable"),
            "the file now has two names, which is what the check exists to catch"
        );
    }

    #[test]
    fn replacing_overwrites_the_destination() {
        let temp = tempfile::TempDir::new().expect("temporary dir should be created");
        let source = temp.path().join("new");
        let destination = temp.path().join("old");
        fs::write(&source, "new contents").expect("source should be written");
        fs::write(&destination, "old contents").expect("destination should be written");

        replace(&source, &destination).expect("replacement should succeed");

        assert_eq!(
            fs::read_to_string(&destination).expect("destination should be readable"),
            "new contents"
        );
        assert!(!source.exists(), "the source is consumed by the move");
    }

    /// A failure the retry cannot fix is returned immediately rather than
    /// waited out.
    #[test]
    fn a_missing_source_fails_without_waiting() {
        let temp = tempfile::TempDir::new().expect("temporary dir should be created");
        let started = Instant::now();

        let error = replace_waiting_for_readers(
            &temp.path().join("absent"),
            &temp.path().join("destination"),
            Duration::from_secs(30),
            Duration::from_millis(10),
        )
        .expect_err("a missing source cannot be moved");

        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the deadline is for readers, not for errors that will never clear"
        );
    }

    #[test]
    fn syncing_a_directory_that_exists_succeeds() {
        let temp = tempfile::TempDir::new().expect("temporary dir should be created");
        sync_directory(temp.path()).expect("an existing directory should sync");
    }
}
