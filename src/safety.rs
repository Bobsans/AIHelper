use std::{
    fs::{self, File},
    io::Read,
    path::Path,
};

use crate::error::AppError;

pub const DEFAULT_MAX_TEXT_BYTES: u64 = 8 * 1024 * 1024;
const BINARY_SNIFF_BYTES: usize = 8192;

#[derive(Debug, Clone, Copy)]
pub struct TextFilePolicy {
    pub max_bytes: u64,
    pub follow_symlinks: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct TextFileInfo {
    pub size_bytes: u64,
    pub is_symlink: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextFileSkipReason {
    NotAFile,
    SymlinkBlocked,
    TooLarge { size_bytes: u64, max_bytes: u64 },
    Binary,
}

#[derive(Debug, Clone, Copy)]
pub enum TextFileDecision {
    Allow(TextFileInfo),
    Skip(TextFileSkipReason),
}

pub fn validate_max_bytes(max_bytes: u64) -> Result<(), AppError> {
    if max_bytes == 0 {
        return Err(AppError::invalid_argument("--max-bytes must be >= 1"));
    }
    Ok(())
}

pub fn inspect_text_file(
    path: &Path,
    policy: TextFilePolicy,
) -> Result<TextFileDecision, AppError> {
    validate_max_bytes(policy.max_bytes)?;

    let symlink_metadata = fs::symlink_metadata(path)
        .map_err(|source| AppError::file_metadata(path.to_path_buf(), source))?;
    let is_symlink = symlink_metadata.file_type().is_symlink();
    if is_symlink && !policy.follow_symlinks {
        return Ok(TextFileDecision::Skip(TextFileSkipReason::SymlinkBlocked));
    }

    let metadata =
        fs::metadata(path).map_err(|source| AppError::file_metadata(path.to_path_buf(), source))?;
    if !metadata.is_file() {
        return Ok(TextFileDecision::Skip(TextFileSkipReason::NotAFile));
    }

    let size_bytes = metadata.len();
    if size_bytes > policy.max_bytes {
        return Ok(TextFileDecision::Skip(TextFileSkipReason::TooLarge {
            size_bytes,
            max_bytes: policy.max_bytes,
        }));
    }

    let prefix = read_prefix(path, BINARY_SNIFF_BYTES)?;
    if is_probably_binary(&prefix) {
        return Ok(TextFileDecision::Skip(TextFileSkipReason::Binary));
    }

    Ok(TextFileDecision::Allow(TextFileInfo {
        size_bytes,
        is_symlink,
    }))
}

pub fn is_probably_binary(prefix: &[u8]) -> bool {
    if prefix.is_empty() {
        return false;
    }
    if prefix.contains(&0) {
        return true;
    }
    std::str::from_utf8(prefix).is_err()
}

pub fn skip_reason_message(path: &Path, reason: TextFileSkipReason) -> String {
    match reason {
        TextFileSkipReason::NotAFile => {
            format!("path is not a file: {}", path.to_string_lossy())
        }
        TextFileSkipReason::SymlinkBlocked => format!(
            "path is a symlink and symlink traversal is disabled: {} (use --follow-symlinks)",
            path.to_string_lossy()
        ),
        TextFileSkipReason::TooLarge {
            size_bytes,
            max_bytes,
        } => format!(
            "file is too large: {} bytes > --max-bytes {} (path: {})",
            size_bytes,
            max_bytes,
            path.to_string_lossy()
        ),
        TextFileSkipReason::Binary => {
            format!(
                "binary or non-UTF8 file is not supported: {}",
                path.to_string_lossy()
            )
        }
    }
}

pub fn skip_reason_to_error(path: &Path, reason: TextFileSkipReason) -> AppError {
    AppError::invalid_argument(skip_reason_message(path, reason))
}

fn read_prefix(path: &Path, max_bytes: usize) -> Result<Vec<u8>, AppError> {
    let mut file =
        File::open(path).map_err(|source| AppError::file_read(path.to_path_buf(), source))?;
    let mut buffer = vec![0u8; max_bytes];
    let bytes_read = file
        .read(&mut buffer)
        .map_err(|source| AppError::file_read(path.to_path_buf(), source))?;
    buffer.truncate(bytes_read);
    Ok(buffer)
}
