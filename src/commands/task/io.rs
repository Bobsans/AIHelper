use std::{
    fs,
    path::{Path, PathBuf},
    process::Output,
    time::{SystemTime, UNIX_EPOCH},
};

use ah_runtime::core;
use serde_json;

use crate::commands::task::domain::TaskStore;
use crate::error::AppError;

const TASKS_DIR: &str = ".ah";
const TASKS_FILE: &str = "tasks.json";

pub(crate) fn task_store_path() -> PathBuf {
    PathBuf::from(TASKS_DIR).join(TASKS_FILE)
}

pub(crate) fn load_store(path: &Path) -> Result<TaskStore, AppError> {
    if !path.exists() {
        return Ok(TaskStore::default());
    }

    let raw = fs::read_to_string(path)
        .map_err(|source| AppError::file_read(path.to_path_buf(), source))?;
    if raw.trim().is_empty() {
        return Ok(TaskStore::default());
    }
    let store: TaskStore = serde_json::from_str(&raw)
        .map_err(|source| AppError::json_deserialization(path.to_path_buf(), source))?;
    Ok(store)
}

pub(crate) fn save_store(path: &Path, store: &TaskStore) -> Result<(), AppError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|source| AppError::file_write(parent.to_path_buf(), source))?;
    }
    let raw = serde_json::to_string_pretty(store)?;
    fs::write(path, raw).map_err(|source| AppError::file_write(path.to_path_buf(), source))
}

pub(crate) fn run_shell_command(command: &str) -> Result<Output, AppError> {
    core::run_shell_command(command)
        .map_err(|source| AppError::command_execution(format!("shell command: {command}"), source))
}

pub(crate) fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0)
}
