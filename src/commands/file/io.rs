use std::collections::{HashSet, VecDeque};
use std::fs::{self, File, Metadata};
use std::io::{BufRead, BufReader, ErrorKind};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::error::AppError;
use crate::safety::{self, TextFileDecision, TextFilePolicy};
use crate::commands::file::domain::TreeEntry;
use ah_runtime::core::apply_limit;

pub(crate) fn inspect_text_file(
    path: &Path,
    policy: &TextFilePolicy,
) -> Result<TextFileDecision, AppError> {
    safety::inspect_text_file(path, *policy)
}

pub(crate) fn metadata(path: &Path) -> Result<Metadata, AppError> {
    fs::symlink_metadata(path).map_err(|source| AppError::file_metadata(path.to_path_buf(), source))
}

pub(crate) fn read_lines_in_range(
    path: &Path,
    from: usize,
    to: usize,
    line_cap: Option<usize>,
) -> Result<(Vec<(usize, String)>, bool), AppError> {
    if to == 0 || from > to {
        return Ok((Vec::new(), false));
    }

    let file = File::open(path).map_err(|source| AppError::file_read(path.to_path_buf(), source))?;
    let reader = BufReader::new(file);

    let mut selected: Vec<(usize, String)> = Vec::new();
    let mut truncated = false;

    for (index, line_result) in reader.lines().enumerate() {
        let line_number = index + 1;
        if line_number < from {
            continue;
        }
        if line_number > to {
            break;
        }

        let line = line_result.map_err(|source| map_text_line_error(path, source))?;
        if line_cap.map_or(true, |cap| selected.len() < cap) {
            selected.push((line_number, line));
            continue;
        }

        truncated = true;
        break;
    }

    Ok((selected, truncated))
}

pub(crate) fn read_tail_lines(
    path: &Path,
    requested_lines: usize,
    line_cap: Option<usize>,
) -> Result<(Vec<(usize, String)>, bool), AppError> {
    if requested_lines == 0 {
        return Ok((Vec::new(), false));
    }

    let file = File::open(path).map_err(|source| AppError::file_read(path.to_path_buf(), source))?;
    let reader = BufReader::new(file);
    let mut queue: VecDeque<(usize, String)> = VecDeque::new();

    for (index, line_result) in reader.lines().enumerate() {
        let line_number = index + 1;
        let line = line_result.map_err(|source| map_text_line_error(path, source))?;
        if queue.len() == requested_lines {
            queue.pop_front();
        }
        queue.push_back((line_number, line));
    }

    let mut selected: Vec<(usize, String)> = queue.into_iter().collect();
    let truncated = apply_limit(&mut selected, line_cap);
    Ok((selected, truncated))
}

pub(crate) fn collect_tree_entries(
    path: &Path,
    depth: usize,
    max_depth: Option<usize>,
    follow_symlinks: bool,
    visited_dirs: &mut HashSet<PathBuf>,
) -> Result<Vec<TreeEntry>, AppError> {
    let mut entries = Vec::new();
    let metadata = metadata(path)?;
    let kind = metadata_kind(&metadata);
    entries.push(TreeEntry {
        depth,
        kind,
        name: display_name_for_path(path),
        path: path.to_string_lossy().into_owned(),
    });

    let should_descend_as_directory = if metadata.is_dir() {
        true
    } else if metadata.file_type().is_symlink() && follow_symlinks {
        fs::metadata(path)
            .map_err(|source| AppError::file_metadata(path.to_path_buf(), source))?
            .is_dir()
    } else {
        false
    };

    if !should_descend_as_directory {
        return Ok(entries);
    }

    let should_descend = max_depth.map(|limit| depth < limit).unwrap_or(true);
    if !should_descend {
        return Ok(entries);
    }

    let canonical_dir = fs::canonicalize(path)
        .map_err(|source| AppError::directory_read(path.to_path_buf(), source))?;
    if !visited_dirs.insert(canonical_dir) {
        return Ok(entries);
    }

    let mut children = fs::read_dir(path)
        .map_err(|source| AppError::directory_read(path.to_path_buf(), source))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect::<Vec<_>>();
    children.sort_by(|left, right| {
        display_name_for_path(left)
            .to_lowercase()
            .cmp(&display_name_for_path(right).to_lowercase())
    });

    for child_path in children {
        let mut nested = collect_tree_entries(
            &child_path,
            depth + 1,
            max_depth,
            follow_symlinks,
            visited_dirs,
        )?;
        entries.append(&mut nested);
    }

    Ok(entries)
}

pub(crate) fn metadata_kind(metadata: &Metadata) -> &'static str {
    if metadata.file_type().is_symlink() {
        "symlink"
    } else if metadata.is_dir() {
        "directory"
    } else if metadata.is_file() {
        "file"
    } else {
        "other"
    }
}

pub(crate) fn system_time_to_unix_seconds(timestamp: SystemTime) -> Option<u64> {
    timestamp.duration_since(std::time::UNIX_EPOCH).ok().map(|value| value.as_secs())
}

fn display_name_for_path(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

fn map_text_line_error(path: &Path, source: std::io::Error) -> AppError {
    if source.kind() == ErrorKind::InvalidData {
        AppError::invalid_argument(format!(
            "binary or non-UTF8 file is not supported: {}",
            path.to_string_lossy()
        ))
    } else {
        AppError::file_read(path.to_path_buf(), source)
    }
}
