//! Where a day's log lives, how it is locked, and when it is removed.
//!
//! Several processes append to the same file, so a write takes an advisory lock
//! with a bounded retry rather than blocking a command on the log.

use std::{
    fs::{self, File},
    io,
    path::Path,
    thread,
    time::{Duration, Instant},
};

use chrono::{Days, NaiveDate};
use fs2::FileExt;

pub(crate) const FILE_PREFIX: &str = "aihelper-";

pub(crate) const FILE_SUFFIX: &str = ".jsonl";

pub(crate) const LOCK_TIMEOUT: Duration = Duration::from_millis(50);

pub(crate) const LOCK_RETRY_DELAY: Duration = Duration::from_millis(2);

pub(crate) fn acquire_lock(file: &File) -> io::Result<()> {
    let started = Instant::now();
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(()),
            Err(error) if retryable_lock_error(&error) => {
                if started.elapsed() >= LOCK_TIMEOUT {
                    return Err(error);
                }
                thread::sleep(LOCK_RETRY_DELAY.min(LOCK_TIMEOUT.saturating_sub(started.elapsed())));
            }
            Err(error) => return Err(error),
        }
    }
}

pub(crate) fn retryable_lock_error(error: &io::Error) -> bool {
    if error.kind() == io::ErrorKind::WouldBlock {
        return true;
    }
    #[cfg(windows)]
    {
        const ERROR_ACCESS_DENIED: i32 = 5;
        const ERROR_SHARING_VIOLATION: i32 = 32;
        const ERROR_LOCK_VIOLATION: i32 = 33;
        matches!(
            error.raw_os_error(),
            Some(ERROR_ACCESS_DENIED | ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION)
        )
    }
    #[cfg(not(windows))]
    {
        false
    }
}

pub(crate) fn log_filename(date: NaiveDate) -> String {
    format!("{FILE_PREFIX}{}{FILE_SUFFIX}", date.format("%Y-%m-%d"))
}

pub(crate) fn cleanup_old_logs(log_dir: &Path, current_date: NaiveDate) -> io::Result<()> {
    let oldest_retained = current_date
        .checked_sub_days(Days::new(9))
        .unwrap_or(NaiveDate::MIN);
    for entry in fs::read_dir(log_dir)? {
        let Ok(entry) = entry else {
            continue;
        };
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() || file_type.is_symlink() {
            continue;
        }
        let Some(date) = entry.file_name().to_str().and_then(parse_log_filename) else {
            continue;
        };
        if date < oldest_retained {
            let _ = fs::remove_file(entry.path());
        }
    }
    Ok(())
}

pub(crate) fn parse_log_filename(name: &str) -> Option<NaiveDate> {
    if name.len() != FILE_PREFIX.len() + 10 + FILE_SUFFIX.len()
        || !name.starts_with(FILE_PREFIX)
        || !name.ends_with(FILE_SUFFIX)
    {
        return None;
    }
    NaiveDate::parse_from_str(&name[FILE_PREFIX.len()..FILE_PREFIX.len() + 10], "%Y-%m-%d").ok()
}
