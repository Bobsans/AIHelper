//! The filesystem primitives the update path is allowed to use.
//!
//! Every one of them refuses redirection: symlinks, junctions, reparse points
//! and hard links are rejected rather than followed, and every read is bounded.
//! An update runs with the installation's privileges, so a path it is tricked
//! into following is a privilege escalation.

use std::{
    fmt::Write as _,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use ah_updater_core::{ManagedFile, UpdaterError};
use sha2::{Digest, Sha256};

use super::{recovery, transaction};

pub(super) fn ensure_directory_exists(path: &Path) -> Result<(), UpdaterError> {
    match fs::symlink_metadata(path) {
        Ok(_) => ensure_direct_directory(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => create_synced_directory(path),
        Err(_) => Err(recovery("failed to inspect permanent backup directory")),
    }
}

pub(super) fn path_exists(path: &Path) -> Result<bool, UpdaterError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            ensure_direct_directory_metadata(&metadata)?;
            Ok(true)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(recovery("failed to inspect permanent backup state")),
    }
}

pub(super) fn remove_optional_known_directory(path: &Path) -> Result<(), UpdaterError> {
    if path_exists(path)? {
        remove_known_directory(path)?;
    }
    Ok(())
}

pub(super) fn remove_known_directory(path: &Path) -> Result<(), UpdaterError> {
    ensure_direct_directory(path)?;
    fs::remove_dir_all(path).map_err(|_| recovery("failed to remove permanent backup state"))?;
    sync_parent(path).map_err(|_| recovery("failed to sync permanent backup cleanup"))
}

pub(super) fn file_matches(path: &Path, expected: &ManagedFile) -> Result<bool, UpdaterError> {
    let metadata = fs::metadata(path)
        .map_err(|_| transaction("failed to inspect managed destination file"))?;
    if metadata.len() != expected.size {
        return Ok(false);
    }
    let mut input =
        File::open(path).map_err(|_| transaction("failed to open managed destination file"))?;
    Ok(hash_exact(&mut input, expected.size)? == expected.sha256)
}

pub(super) fn read_optional_bounded(
    path: &Path,
    maximum: usize,
) -> Result<Option<Vec<u8>>, UpdaterError> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Ok(_) => read_bounded(path, maximum, "failed to read active release record").map(Some),
        Err(_) => Err(recovery("failed to inspect active release record")),
    }
}

pub(super) fn remove_file_synced(path: &Path) -> io::Result<()> {
    fs::remove_file(path)?;
    sync_parent(path)
}

pub(super) fn create_synced_directory(path: &Path) -> Result<(), UpdaterError> {
    fs::create_dir(path)
        .map_err(|_| transaction("failed to create private transaction directory"))?;
    ensure_direct_directory(path)?;
    sync_parent(path).map_err(|_| transaction("failed to sync transaction directory metadata"))
}

pub(super) fn write_new_synced(path: &Path, bytes: &[u8]) -> Result<(), UpdaterError> {
    let parent = path
        .parent()
        .ok_or_else(|| transaction("transaction state path has no parent"))?;
    ensure_direct_directory(parent)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| transaction("failed to create durable transaction state file"))?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| transaction("failed to write durable transaction state file"))?;
    drop(file);
    ensure_direct_file(path)?;
    sync_parent(path).map_err(|_| transaction("failed to sync durable transaction state metadata"))
}

pub(super) fn verify_file(path: &Path, expected: &ManagedFile) -> Result<(), UpdaterError> {
    let metadata = fs::metadata(path)
        .map_err(|_| transaction("failed to inspect transaction managed file"))?;
    if metadata.len() != expected.size {
        return Err(transaction(
            "transaction managed file size does not match the signed manifest",
        ));
    }
    let mut input =
        File::open(path).map_err(|_| transaction("failed to open transaction managed file"))?;
    let digest = hash_exact(&mut input, expected.size)?;
    if digest != expected.sha256 {
        return Err(transaction(
            "transaction managed file digest does not match the signed manifest",
        ));
    }
    Ok(())
}

pub(super) fn copy_exact_hash(
    input: &mut impl Read,
    output: &mut impl Write,
    expected_size: u64,
) -> Result<String, UpdaterError> {
    hash_stream(input, Some(output), expected_size)
}

pub(super) fn hash_exact(
    input: &mut impl Read,
    expected_size: u64,
) -> Result<String, UpdaterError> {
    hash_stream::<_, io::Sink>(input, None, expected_size)
}

pub(super) fn hash_stream<R: Read, W: Write>(
    input: &mut R,
    mut output: Option<&mut W>,
    expected_size: u64,
) -> Result<String, UpdaterError> {
    let mut remaining = expected_size;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    while remaining > 0 {
        let requested = usize::try_from(remaining.min(buffer.len() as u64))
            .expect("bounded transaction chunk fits usize");
        let read = input
            .read(&mut buffer[..requested])
            .map_err(|_| transaction("failed to read transaction managed file"))?;
        if read == 0 {
            return Err(transaction(
                "transaction managed file ended before its signed size",
            ));
        }
        if let Some(output) = output.as_deref_mut() {
            output
                .write_all(&buffer[..read])
                .map_err(|_| transaction("failed to write transaction copy"))?;
        }
        digest.update(&buffer[..read]);
        remaining -= read as u64;
    }
    let mut extra = [0_u8; 1];
    if input
        .read(&mut extra)
        .map_err(|_| transaction("failed to read transaction managed file"))?
        != 0
    {
        return Err(transaction(
            "transaction managed file exceeds its signed size",
        ));
    }
    Ok(encode_digest(digest.finalize()))
}

pub(super) fn encode_digest(bytes: impl AsRef<[u8]>) -> String {
    let bytes = bytes.as_ref();
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}

pub(super) fn existing_managed_file(root: &Path, relative: &str) -> Result<PathBuf, UpdaterError> {
    ensure_direct_directory(root)?;
    let path = prospective_managed_file(root, relative)?;
    ensure_direct_file(&path)?;
    Ok(path)
}

pub(super) fn prospective_managed_file(
    root: &Path,
    relative: &str,
) -> Result<PathBuf, UpdaterError> {
    let components = relative.split('/').collect::<Vec<_>>();
    let file_name = components
        .last()
        .filter(|component| !component.is_empty())
        .ok_or_else(|| transaction("transaction managed path is empty"))?;
    let mut path = root.to_path_buf();
    let mut missing_parent = false;
    for component in components.iter().take(components.len().saturating_sub(1)) {
        path.push(component);
        if missing_parent {
            continue;
        }
        match fs::symlink_metadata(&path) {
            Ok(metadata) => ensure_direct_directory_metadata(&metadata)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => missing_parent = true,
            Err(_) => return Err(transaction("failed to inspect transaction directory")),
        }
    }
    path.push(file_name);
    Ok(path)
}

pub(super) fn create_managed_destination(
    root: &Path,
    relative: &str,
) -> Result<PathBuf, UpdaterError> {
    ensure_direct_directory(root)?;
    let components = relative.split('/').collect::<Vec<_>>();
    let file_name = components
        .last()
        .filter(|component| !component.is_empty())
        .ok_or_else(|| transaction("transaction managed path is empty"))?;
    let mut path = root.to_path_buf();
    for component in components.iter().take(components.len().saturating_sub(1)) {
        path.push(component);
        match fs::symlink_metadata(&path) {
            Ok(metadata) => ensure_direct_directory_metadata(&metadata)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                create_synced_directory(&path)?;
            }
            Err(_) => return Err(transaction("failed to inspect transaction directory")),
        }
    }
    path.push(file_name);
    Ok(path)
}

pub(super) fn path_from_slashes(path: &str) -> PathBuf {
    path.split('/').collect()
}

#[cfg(windows)]
pub(super) fn paths_equal(left: &Path, right: &Path) -> bool {
    left.to_string_lossy()
        .replace('/', "\\")
        .eq_ignore_ascii_case(&right.to_string_lossy().replace('/', "\\"))
}

#[cfg(not(windows))]
pub(super) fn paths_equal(left: &Path, right: &Path) -> bool {
    left == right
}

pub(super) fn ensure_direct_directory(path: &Path) -> Result<(), UpdaterError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| transaction("failed to inspect transaction directory"))?;
    ensure_direct_directory_metadata(&metadata)
}

pub(super) fn ensure_direct_directory_metadata(
    metadata: &fs::Metadata,
) -> Result<(), UpdaterError> {
    if metadata.file_type().is_symlink() || !metadata.is_dir() || is_reparse_point(metadata) {
        return Err(transaction(
            "transaction path contains a redirected directory",
        ));
    }
    Ok(())
}

pub(super) fn ensure_direct_file(path: &Path) -> Result<(), UpdaterError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| transaction("failed to inspect transaction file"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || is_reparse_point(&metadata) {
        return Err(transaction("transaction path is not a direct regular file"));
    }
    if !has_single_hard_link(path, &metadata)? {
        return Err(transaction(
            "transaction managed file must not be a hard link",
        ));
    }
    Ok(())
}

pub(super) fn has_single_hard_link(
    path: &Path,
    metadata: &fs::Metadata,
) -> Result<bool, UpdaterError> {
    ah_platform::fs::has_single_hard_link(path, metadata)
        .map_err(|_| transaction("failed to inspect transaction file links"))
}

pub(super) fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    ah_platform::fs::is_reparse_point(metadata)
}

pub(super) fn atomic_replace(source: &Path, destination: &Path) -> io::Result<()> {
    ah_platform::fs::replace(source, destination)?;
    sync_parent(destination)
}

/// Flush the directory `path` sits in.
///
/// A no-op on Windows, where the move writes the entry through; the port
/// answers for both.
pub(super) fn sync_parent(path: &Path) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no parent"))?;
    ah_platform::fs::sync_directory(parent)
}

pub(super) fn read_bounded(
    path: &Path,
    maximum: usize,
    detail: &'static str,
) -> Result<Vec<u8>, UpdaterError> {
    ensure_direct_file(path)?;
    let maximum_u64 = u64::try_from(maximum).expect("transaction read limit fits u64");
    let metadata = fs::metadata(path).map_err(|_| recovery(detail))?;
    if metadata.len() > maximum_u64 {
        return Err(recovery(detail));
    }
    let file = File::open(path).map_err(|_| recovery(detail))?;
    let mut bytes = Vec::with_capacity(
        usize::try_from(metadata.len()).expect("bounded transaction file length fits usize"),
    );
    file.take(maximum_u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| recovery(detail))?;
    if bytes.len() > maximum {
        return Err(recovery(detail));
    }
    Ok(bytes)
}
