use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::error::AppError;

pub(crate) fn canonical_project_root(path: &Path) -> Result<PathBuf, AppError> {
    let root = if path.exists() {
        path.canonicalize()
            .map_err(|source| AppError::file_metadata(path.to_path_buf(), source))?
    } else {
        return Err(AppError::invalid_argument(format!(
            "path does not exist: {}",
            path.display()
        )));
    };
    if !root.is_dir() {
        return Err(AppError::invalid_argument(format!(
            "path is not a directory: {}",
            path.display()
        )));
    }
    Ok(root)
}

pub(crate) fn collect_project_files(root: &Path) -> Vec<PathBuf> {
    WalkDir::new(root)
        .max_depth(4)
        .into_iter()
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            !matches!(
                name.as_ref(),
                "target" | "node_modules" | ".git" | ".venv" | "dist" | "build"
            )
        })
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.path().to_path_buf())
        .collect()
}

pub(crate) fn read_to_string(path: impl AsRef<Path>) -> Result<String, AppError> {
    let path = path.as_ref();
    std::fs::read_to_string(path).map_err(|source| AppError::file_read(path.to_path_buf(), source))
}
