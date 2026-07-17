use std::fs;
use std::path::{Path, PathBuf};

use ah_runtime::core;
use globset::GlobSet;
use ignore::WalkBuilder;

use crate::commands::search::domain::{ContextLine, PatternMatcher, TextCollectStats, TextMatch};
use crate::error::AppError;
use crate::safety::{self, TextFileDecision, TextFilePolicy, TextFileSkipReason};

#[derive(Debug)]
pub(crate) struct SearchScope {
    pub(crate) roots: Vec<PathBuf>,
    pub(crate) display_root: PathBuf,
    pub(crate) root_label: String,
    pub(crate) root_labels: Vec<String>,
}

pub(crate) fn resolve_scope_at(
    paths: &[PathBuf],
    follow_symlinks: bool,
    cwd: Option<&Path>,
) -> Result<SearchScope, AppError> {
    let current_dir = match cwd {
        Some(path) => path.to_path_buf(),
        None => current_dir()?,
    };
    let requested_roots = if paths.is_empty() {
        vec![current_dir.clone()]
    } else {
        paths.to_vec()
    };

    for root in &requested_roots {
        if !root.exists() {
            return Err(AppError::invalid_argument(format!(
                "path does not exist: {}",
                root.to_string_lossy()
            )));
        }
        if !follow_symlinks && is_symlink_path(root)? {
            return Err(AppError::invalid_argument(format!(
                "path is a symlink and symlink traversal is disabled: {} (use --follow-symlinks)",
                root.to_string_lossy()
            )));
        }
    }

    let mut roots = requested_roots
        .iter()
        .map(|root| absolutize_path_at(root, &current_dir))
        .collect::<Result<Vec<_>, _>>()?;
    roots.sort();
    roots.dedup();

    let display_root = if roots.len() == 1 {
        roots[0].clone()
    } else {
        current_dir.clone()
    };
    let root_labels = requested_roots
        .iter()
        .map(|root| root.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let root_label = if root_labels.len() == 1 {
        root_labels[0].clone()
    } else {
        current_dir.to_string_lossy().into_owned()
    };

    Ok(SearchScope {
        roots,
        display_root,
        root_label,
        root_labels,
    })
}

pub(crate) fn collect_files_from_roots(
    roots: &[PathBuf],
    globset: Option<&GlobSet>,
    follow_symlinks: bool,
) -> Result<Vec<PathBuf>, AppError> {
    let mut files = Vec::new();
    for root in roots {
        if crate::commands::search::current_request_cancelled() {
            return Err(AppError::external(
                "EXECUTION_CANCELLED",
                "search request was cancelled",
            ));
        }
        files.extend(collect_files_ignore_aware(root, globset, follow_symlinks)?);
    }
    files.sort();
    files.dedup();
    Ok(files)
}

fn collect_files_ignore_aware(
    root: &Path,
    globset: Option<&GlobSet>,
    follow_symlinks: bool,
) -> Result<Vec<PathBuf>, AppError> {
    if root.is_file() {
        if is_symlink_path(root)? && !follow_symlinks {
            return Ok(Vec::new());
        }
        if file_matches_globs(root, root, globset) {
            return Ok(vec![root.to_path_buf()]);
        }
        return Ok(Vec::new());
    }
    if !root.is_dir() {
        return Err(AppError::invalid_argument(format!(
            "path is not a file or directory: {}",
            root.to_string_lossy()
        )));
    }

    let mut builder = WalkBuilder::new(root);
    builder
        .follow_links(follow_symlinks)
        .add_custom_ignore_filename(".rgignore");
    let mut files = Vec::new();
    for entry in builder.build() {
        if crate::commands::search::current_request_cancelled() {
            return Err(AppError::external(
                "EXECUTION_CANCELLED",
                "search request was cancelled",
            ));
        }
        let entry = entry.map_err(|error| {
            AppError::directory_read(root.to_path_buf(), std::io::Error::other(error))
        })?;
        if entry.file_type().is_some_and(|kind| kind.is_file())
            && file_matches_globs(entry.path(), root, globset)
        {
            files.push(entry.path().to_path_buf());
        }
    }
    files.sort();
    Ok(files)
}

pub(crate) fn collect_text_matches(
    files: Vec<PathBuf>,
    root: &Path,
    matcher: &PatternMatcher,
    context_lines: usize,
    max_bytes: u64,
    follow_symlinks: bool,
    limit: Option<usize>,
) -> Result<(Vec<TextMatch>, TextCollectStats, bool), AppError> {
    let max_count = limit.unwrap_or(usize::MAX);
    let mut matches = Vec::new();
    let mut stats = TextCollectStats::default();
    let mut truncated = false;

    for path in files {
        if crate::commands::search::current_request_cancelled() {
            return Err(AppError::external(
                "EXECUTION_CANCELLED",
                "search request was cancelled",
            ));
        }
        let (file_matches, file_truncated) = match_file(
            &path,
            root,
            matcher,
            context_lines,
            max_bytes,
            follow_symlinks,
            &mut stats,
            max_count.saturating_sub(matches.len()),
        )?;
        matches.extend(file_matches);
        if file_truncated {
            truncated = true;
            break;
        }
    }

    Ok((matches, stats, truncated))
}

fn match_file(
    path: &Path,
    root: &Path,
    matcher: &PatternMatcher,
    context_lines: usize,
    max_bytes: u64,
    follow_symlinks: bool,
    stats: &mut TextCollectStats,
    match_limit: usize,
) -> Result<(Vec<TextMatch>, bool), AppError> {
    let policy = TextFilePolicy {
        max_bytes,
        follow_symlinks,
    };
    match safety::inspect_text_file(path, policy)? {
        TextFileDecision::Allow(_) => {}
        TextFileDecision::Skip(reason) => {
            register_skip_reason(stats, reason);
            return Ok((Vec::new(), false));
        }
    }

    let bytes = fs::read(path).map_err(|source| AppError::file_read(path.to_path_buf(), source))?;
    let text = match String::from_utf8(bytes) {
        Ok(value) => value,
        Err(_) => {
            register_skip_reason(stats, TextFileSkipReason::Binary);
            return Ok((Vec::new(), false));
        }
    };
    if text.is_empty() {
        return Ok((Vec::new(), false));
    }
    let lines = text.lines().collect::<Vec<_>>();

    let mut matches = Vec::new();
    let normalized_path = display_path(path, root);
    for (index, line) in lines.iter().enumerate() {
        if crate::commands::search::current_request_cancelled() {
            return Err(AppError::external(
                "EXECUTION_CANCELLED",
                "search request was cancelled",
            ));
        }
        if let Some(column) = crate::commands::search::domain::find_match_column(matcher, line) {
            if matches.len() >= match_limit {
                return Ok((matches, true));
            }
            let line_number = index + 1;
            let (before, after) = build_context_lines(context_lines, index, &lines);
            matches.push(TextMatch {
                path: normalized_path.clone(),
                line: line_number,
                column,
                text: (*line).to_owned(),
                context_before: before,
                context_after: after,
            });
        }
    }

    Ok((matches, false))
}

fn build_context_lines(
    context_lines: usize,
    index: usize,
    lines: &[&str],
) -> (Vec<ContextLine>, Vec<ContextLine>) {
    let before_start = index.saturating_sub(context_lines);
    let before = if context_lines == 0 {
        Vec::new()
    } else {
        lines[before_start..index]
            .iter()
            .enumerate()
            .map(|(offset, content)| ContextLine {
                line: before_start + offset + 1,
                text: (*content).to_owned(),
            })
            .collect()
    };

    let after_end = index
        .saturating_add(context_lines)
        .min(lines.len().saturating_sub(1));
    let after = if context_lines == 0 || index + 1 >= lines.len() {
        Vec::new()
    } else {
        lines[(index + 1)..=after_end]
            .iter()
            .enumerate()
            .map(|(offset, content)| ContextLine {
                line: index + offset + 2,
                text: (*content).to_owned(),
            })
            .collect()
    };

    (before, after)
}

fn file_matches_globs(path: &Path, root: &Path, globset: Option<&GlobSet>) -> bool {
    let Some(globset) = globset else {
        return true;
    };

    let relative = if root.is_file() {
        path
    } else {
        path.strip_prefix(root).unwrap_or(path)
    };
    let normalized = core::normalize_path(relative);
    globset.is_match(normalized)
}

fn is_symlink_path(path: &Path) -> Result<bool, AppError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|source| AppError::file_metadata(path.to_path_buf(), source))?;
    Ok(metadata.file_type().is_symlink())
}

pub(crate) fn display_path(path: &Path, root: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path);
    if relative.as_os_str().is_empty() {
        return path
            .file_name()
            .map(|name| core::normalize_path(Path::new(name)))
            .unwrap_or_else(|| core::normalize_path(path));
    }
    core::normalize_path(relative)
}

fn absolutize_path_at(path: &Path, current_dir: &Path) -> Result<PathBuf, AppError> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    Ok(current_dir.join(path))
}

fn current_dir() -> Result<PathBuf, AppError> {
    std::env::current_dir().map_err(|source| AppError::cwd(PathBuf::from("."), source))
}
fn register_skip_reason(stats: &mut TextCollectStats, reason: TextFileSkipReason) {
    match reason {
        TextFileSkipReason::Binary => {
            stats.skipped_binary_files += 1;
        }
        TextFileSkipReason::TooLarge { .. } => {
            stats.skipped_large_files += 1;
        }
        TextFileSkipReason::SymlinkBlocked => {
            stats.skipped_symlink_files += 1;
        }
        TextFileSkipReason::NotAFile => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn literal(needle: &str) -> PatternMatcher {
        PatternMatcher::Literal {
            needle: needle.to_owned(),
            needle_lower: None,
            ignore_case: false,
        }
    }

    #[test]
    fn context_before_contains_exactly_requested_lines() {
        let lines = vec!["one", "two", "hit", "four"];

        let (before, after) = build_context_lines(1, 2, &lines);

        assert_eq!(before.len(), 1);
        assert_eq!(before[0].line, 2);
        assert_eq!(before[0].text, "two");
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].line, 4);
        assert_eq!(after[0].text, "four");
    }

    #[test]
    fn context_is_bounded_at_first_and_last_lines() {
        let lines = vec!["first", "second", "third", "last"];

        let (first_before, first_after) = build_context_lines(2, 0, &lines);
        assert!(first_before.is_empty());
        assert_eq!(
            first_after.iter().map(|line| line.line).collect::<Vec<_>>(),
            vec![2, 3]
        );

        let (last_before, last_after) = build_context_lines(2, 3, &lines);
        assert_eq!(
            last_before.iter().map(|line| line.line).collect::<Vec<_>>(),
            vec![2, 3]
        );
        assert!(last_after.is_empty());
    }

    #[test]
    fn text_match_limit_uses_one_extra_result_for_truncation() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("matches.txt");
        fs::write(&path, "hit one\nhit two\nhit three\nhit four\n").unwrap();

        let (matches, _, truncated) = collect_text_matches(
            vec![path],
            directory.path(),
            &literal("hit"),
            0,
            1_024,
            false,
            Some(2),
        )
        .unwrap();

        assert_eq!(matches.len(), 2);
        assert!(truncated);
    }

    #[test]
    fn text_match_limit_is_not_truncated_without_an_extra_result() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("matches.txt");
        fs::write(&path, "hit one\nhit two\n").unwrap();

        let (matches, _, truncated) = collect_text_matches(
            vec![path],
            directory.path(),
            &literal("hit"),
            0,
            1_024,
            false,
            Some(2),
        )
        .unwrap();

        assert_eq!(matches.len(), 2);
        assert!(!truncated);
    }
}
