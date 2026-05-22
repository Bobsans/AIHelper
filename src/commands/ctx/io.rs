use std::fs;
use std::path::{Path, PathBuf};

use ah_runtime::core;

use walkdir::WalkDir;

use crate::error::AppError;
use crate::safety::{self, TextFileDecision, TextFilePolicy};

#[derive(Debug, Clone)]
pub(crate) struct WalkEntry {
    pub(crate) path: PathBuf,
    pub(crate) is_file: bool,
}

pub(crate) fn inspect_text_file(
    path: &Path,
    policy: &TextFilePolicy,
) -> Result<TextFileDecision, AppError> {
    safety::inspect_text_file(path, *policy)
}

pub(crate) fn read_to_string(path: &Path) -> Result<String, AppError> {
    fs::read_to_string(path).map_err(|source| AppError::file_read(path.to_path_buf(), source))
}

pub(crate) fn symlink_metadata(path: &Path) -> Result<fs::Metadata, AppError> {
    fs::symlink_metadata(path).map_err(|source| AppError::file_metadata(path.to_path_buf(), source))
}

pub(crate) fn walk_entries(root: &Path, follow_symlinks: bool) -> Result<Vec<WalkEntry>, AppError> {
    let mut entries = Vec::new();
    for entry in WalkDir::new(root)
        .follow_links(follow_symlinks)
        .sort_by_file_name()
    {
        let entry = match entry {
            Ok(value) => value,
            Err(error) if error.loop_ancestor().is_some() => continue,
            Err(error) => {
                return Err(AppError::directory_read(
                    root.to_path_buf(),
                    std::io::Error::other(error),
                ));
            }
        };
        entries.push(WalkEntry {
            path: entry.path().to_path_buf(),
            is_file: entry.file_type().is_file(),
        });
    }
    Ok(entries)
}

pub(crate) fn is_inside_git_repo() -> Result<bool, AppError> {
    let output = core::run_command("git", ["rev-parse", "--is-inside-work-tree"])
        .map_err(|error| AppError::invalid_argument(format!("failed to run git: {error}")))?;
    if !output.status.success() {
        return Ok(false);
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.trim() == "true")
}

pub(crate) fn read_git_status_lines() -> Result<Vec<String>, AppError> {
    let output = core::run_command("git", ["status", "--porcelain"]).map_err(|error| {
        AppError::invalid_argument(format!("failed to run git status: {error}"))
    })?;
    if !output.status.success() {
        return Err(AppError::invalid_argument(
            "git status failed for current repository",
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_owned)
        .collect())
}

pub(crate) fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}
