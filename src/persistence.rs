use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock, Weak,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime},
};

use serde::Serialize;

use crate::error::AppError;

const LOCK_TIMEOUT: Duration = Duration::from_secs(2);
const LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(10);
const STALE_LOCK_AGE: Duration = Duration::from_secs(300);
#[cfg(windows)]
const REPLACE_RETRY_TIMEOUT: Duration = Duration::from_millis(250);

pub(crate) fn transaction<T>(
    path: &Path,
    operation: impl FnOnce() -> Result<T, AppError>,
) -> Result<T, AppError> {
    let path_lock = path_lock(path);
    let _path_guard = path_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _sidecar_lock = SidecarLock::acquire(path, LOCK_TIMEOUT)?;
    operation()
}

pub(crate) fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), AppError> {
    let mut payload = serde_json::to_vec_pretty(value)?;
    payload.push(b'\n');
    atomic_write(path, &payload)
}

fn atomic_write(path: &Path, payload: &[u8]) -> Result<(), AppError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    if let Some(parent) = parent {
        fs::create_dir_all(parent)
            .map_err(|source| AppError::file_write(parent.to_path_buf(), source))?;
    }
    let temporary_path = temporary_path(path);
    let mut temporary = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary_path)
        .map_err(|source| AppError::file_write(temporary_path.clone(), source))?;
    if let Ok(metadata) = fs::metadata(path)
        && let Err(source) = fs::set_permissions(&temporary_path, metadata.permissions())
    {
        drop(temporary);
        let _ = fs::remove_file(&temporary_path);
        return Err(AppError::file_write(temporary_path, source));
    }
    let write_result = (|| {
        temporary
            .write_all(payload)
            .map_err(|source| AppError::file_write(temporary_path.clone(), source))?;
        temporary
            .sync_all()
            .map_err(|source| AppError::file_write(temporary_path.clone(), source))?;
        drop(temporary);
        replace_file(&temporary_path, path)?;
        sync_parent_directory(parent);
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    write_result
}

fn path_lock(path: &Path) -> Arc<Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Weak<Mutex<()>>>>> = OnceLock::new();
    let canonical_key = absolute_lock_key(path);
    let mut locks = LOCKS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(&canonical_key).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(Mutex::new(()));
    locks.insert(canonical_key, Arc::downgrade(&lock));
    lock
}

fn absolute_lock_key(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    std::env::current_dir()
        .map(|cwd| cwd.join(path))
        .unwrap_or_else(|_| path.to_path_buf())
}

struct SidecarLock {
    path: PathBuf,
    file: Option<File>,
}

impl SidecarLock {
    fn acquire(target: &Path, timeout: Duration) -> Result<Self, AppError> {
        let path = sidecar_path(target);
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)
                .map_err(|source| AppError::file_write(parent.to_path_buf(), source))?;
        }
        let deadline = Instant::now() + timeout;
        loop {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    let marker = format!("pid={}\n", std::process::id());
                    if let Err(source) = file
                        .write_all(marker.as_bytes())
                        .and_then(|_| file.sync_all())
                    {
                        drop(file);
                        let _ = fs::remove_file(&path);
                        return Err(AppError::file_write(path, source));
                    }
                    return Ok(Self {
                        path,
                        file: Some(file),
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    remove_stale_lock(&path);
                    if Instant::now() >= deadline {
                        return Err(AppError::external(
                            "PERSISTENCE_LOCK_TIMEOUT",
                            format!(
                                "timed out waiting for persistence lock '{}'",
                                path.display()
                            ),
                        ));
                    }
                    thread::sleep(LOCK_RETRY_INTERVAL);
                }
                Err(source) => return Err(AppError::file_write(path, source)),
            }
        }
    }
}

impl Drop for SidecarLock {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = fs::remove_file(&self.path);
    }
}

fn remove_stale_lock(path: &Path) {
    let Ok(metadata) = fs::metadata(path) else {
        return;
    };
    let Ok(modified) = metadata.modified() else {
        return;
    };
    if SystemTime::now()
        .duration_since(modified)
        .is_ok_and(|age| age >= STALE_LOCK_AGE)
    {
        let _ = fs::remove_file(path);
    }
}

fn sidecar_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "store".to_owned());
    path.with_file_name(format!("{file_name}.lock"))
}

fn temporary_path(path: &Path) -> PathBuf {
    static NEXT_TEMPORARY_ID: AtomicU64 = AtomicU64::new(1);
    let sequence = NEXT_TEMPORARY_ID.fetch_add(1, Ordering::Relaxed);
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "store".to_owned());
    path.with_file_name(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        sequence
    ))
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> Result<(), AppError> {
    use std::os::windows::ffi::OsStrExt;

    const ERROR_ACCESS_DENIED: i32 = 5;
    const ERROR_SHARING_VIOLATION: i32 = 32;
    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, replacement: *const u16, flags: u32) -> i32;
    }
    let source_wide = source
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let destination_wide = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let deadline = Instant::now() + REPLACE_RETRY_TIMEOUT;
    loop {
        // SAFETY: both buffers are NUL-terminated and remain alive for the call.
        let replaced = unsafe {
            MoveFileExW(
                source_wide.as_ptr(),
                destination_wide.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        };
        if replaced != 0 {
            return Ok(());
        }

        let error = std::io::Error::last_os_error();
        let retryable = matches!(
            error.raw_os_error(),
            Some(ERROR_ACCESS_DENIED | ERROR_SHARING_VIOLATION)
        );
        if !retryable || Instant::now() >= deadline {
            return Err(AppError::file_write(destination.to_path_buf(), error));
        }
        thread::sleep(LOCK_RETRY_INTERVAL);
    }
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> Result<(), AppError> {
    fs::rename(source, destination)
        .map_err(|source| AppError::file_write(destination.to_path_buf(), source))
}

#[cfg(unix)]
fn sync_parent_directory(parent: Option<&Path>) {
    let parent = parent.unwrap_or_else(|| Path::new("."));
    let _ = File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| ());
}

#[cfg(not(unix))]
fn sync_parent_directory(_parent: Option<&Path>) {}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};

    use serde_json::json;

    use super::*;

    #[test]
    fn atomic_json_write_replaces_complete_document() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.json");
        atomic_write_json(&path, &json!({"value": "before"})).unwrap();
        atomic_write_json(&path, &json!({"value": "after"})).unwrap();

        let value: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(value, json!({"value": "after"}));
    }

    #[test]
    fn transaction_removes_sidecar_lock() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.json");

        transaction(&path, || atomic_write_json(&path, &json!({"ok": true}))).unwrap();

        assert!(!sidecar_path(&path).exists());
    }

    #[test]
    fn lock_acquisition_is_bounded() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.json");
        let _held = SidecarLock::acquire(&path, Duration::from_millis(20)).unwrap();

        let error = SidecarLock::acquire(&path, Duration::from_millis(20))
            .err()
            .expect("second lock should time out");

        assert_eq!(error.code(), "PERSISTENCE_LOCK_TIMEOUT");
    }

    #[test]
    fn concurrent_readers_only_observe_complete_json() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.json");
        atomic_write_json(&path, &json!({"value": 0})).unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let writer_path = path.clone();
        let writer_barrier = Arc::clone(&barrier);
        let writer = std::thread::spawn(move || {
            writer_barrier.wait();
            for value in 1..=100 {
                atomic_write_json(&writer_path, &json!({"value": value})).unwrap();
            }
        });

        barrier.wait();
        for _ in 0..200 {
            let raw = fs::read_to_string(&path).unwrap();
            let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
            assert!(value["value"].is_number());
        }
        writer.join().unwrap();
    }

    #[test]
    fn failed_replace_removes_temporary_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.json");
        fs::create_dir(&path).unwrap();

        atomic_write_json(&path, &json!({"value": true}))
            .expect_err("a regular file cannot replace a directory");

        let temporary_prefix = format!(
            ".{}.{}.",
            path.file_name().unwrap().to_string_lossy(),
            std::process::id()
        );
        assert!(
            fs::read_dir(directory.path())
                .unwrap()
                .filter_map(Result::ok)
                .all(|entry| {
                    !entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(&temporary_prefix)
                })
        );
    }

    #[cfg(windows)]
    #[test]
    fn replace_retries_transient_windows_sharing_denial() {
        use std::{os::windows::fs::OpenOptionsExt, sync::mpsc};

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.json");
        atomic_write_json(&path, &json!({"value": "before"})).unwrap();

        let reader_path = path.clone();
        let (ready_tx, ready_rx) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            let file = OpenOptions::new()
                .read(true)
                .share_mode(0)
                .open(reader_path)
                .unwrap();
            ready_tx.send(()).unwrap();
            thread::sleep(Duration::from_millis(50));
            drop(file);
        });

        ready_rx.recv().unwrap();
        atomic_write_json(&path, &json!({"value": "after"})).unwrap();
        reader.join().unwrap();

        let value: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(value, json!({"value": "after"}));
    }
}
