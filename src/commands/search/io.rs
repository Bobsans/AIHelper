use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use ah_runtime::core;

use globset::GlobSet;
use walkdir::WalkDir;

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

pub(crate) fn rg_is_available() -> bool {
    core::run_command_ok("rg", ["--version"])
}

pub(crate) fn resolve_scope(
    paths: &[PathBuf],
    follow_symlinks: bool,
) -> Result<SearchScope, AppError> {
    let requested_roots = if paths.is_empty() {
        vec![PathBuf::from(".")]
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
        .map(|root| absolutize_path(root))
        .collect::<Result<Vec<_>, _>>()?;
    roots.sort();
    roots.dedup();

    let current_dir = current_dir()?;
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

pub(crate) fn candidate_files_with_rg(
    args: &crate::commands::search::TextArgs,
    roots: &[PathBuf],
) -> Option<Vec<PathBuf>> {
    let mut command_args = vec![
        "-l".to_owned(),
        "--color".to_owned(),
        "never".to_owned(),
        "--no-messages".to_owned(),
    ];
    if args.follow_symlinks {
        command_args.push("-L".to_owned());
    }
    if args.ignore_case {
        command_args.push("-i".to_owned());
    }
    for glob in &args.globs {
        command_args.push("-g".to_owned());
        command_args.push(glob.to_owned());
    }
    if !args.regex {
        command_args.push("-F".to_owned());
    }
    command_args.push("--".to_owned());
    command_args.push(args.pattern.clone());
    for root in roots {
        command_args.push(root.to_string_lossy().to_string());
    }

    let output = core::run_command("rg", &command_args).ok()?;
    if !output.status.success() && output.status.code() != Some(1) {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut files: Vec<PathBuf> = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(PathBuf::from)
        .collect();
    files.sort();
    files.dedup();
    Some(files)
}

pub(crate) fn files_with_rg(roots: &[PathBuf], follow_symlinks: bool) -> Option<Vec<PathBuf>> {
    let mut command_args = vec!["--files".to_owned()];
    if follow_symlinks {
        command_args.push("-L".to_owned());
    }
    for root in roots {
        command_args.push(root.to_string_lossy().to_string());
    }
    let output = core::run_command("rg", &command_args).ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut files: Vec<PathBuf> = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(PathBuf::from)
        .collect();
    files.sort();
    files.dedup();
    Some(files)
}

pub(crate) fn collect_files_from_roots(
    roots: &[PathBuf],
    globset: Option<&GlobSet>,
    follow_symlinks: bool,
) -> Result<Vec<PathBuf>, AppError> {
    let mut files = Vec::new();
    for root in roots {
        files.extend(collect_files_fallback(root, globset, follow_symlinks)?);
    }
    files.sort();
    files.dedup();
    Ok(files)
}

fn collect_files_fallback(
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

    let mut files = Vec::new();
    for entry in WalkDir::new(root).follow_links(follow_symlinks) {
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
        if entry.file_type().is_file() && file_matches_globs(entry.path(), root, globset) {
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
    let mut seen: HashSet<(String, usize)> = HashSet::new();

    'file_loop: for path in files {
        let file_matches = match_file(
            &path,
            root,
            matcher,
            context_lines,
            max_bytes,
            follow_symlinks,
            &mut stats,
        )?;
        for item in file_matches {
            let key = (item.path.clone(), item.line);
            if seen.contains(&key) {
                continue;
            }
            if matches.len() < max_count {
                seen.insert(key);
                matches.push(item);
            } else {
                truncated = true;
                break 'file_loop;
            }
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
) -> Result<Vec<TextMatch>, AppError> {
    let policy = TextFilePolicy {
        max_bytes,
        follow_symlinks,
    };
    match safety::inspect_text_file(path, policy)? {
        TextFileDecision::Allow(_) => {}
        TextFileDecision::Skip(reason) => {
            register_skip_reason(stats, reason);
            return Ok(Vec::new());
        }
    }

    let bytes = fs::read(path).map_err(|source| AppError::file_read(path.to_path_buf(), source))?;
    let text = match String::from_utf8(bytes) {
        Ok(value) => value,
        Err(_) => {
            register_skip_reason(stats, TextFileSkipReason::Binary);
            return Ok(Vec::new());
        }
    };
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let lines: Vec<String> = text.lines().map(|line| line.to_owned()).collect();

    let mut matches = Vec::new();
    let normalized_path = display_path(path, root);
    for (index, line) in lines.iter().enumerate() {
        if let Some(column) = crate::commands::search::domain::find_match_column(matcher, line) {
            let line_number = index + 1;
            let (before, after) = build_context_lines(context_lines, index, &lines);
            matches.push(TextMatch {
                path: normalized_path.clone(),
                line: line_number,
                column,
                text: line.clone(),
                context_before: before,
                context_after: after,
            });
        }
    }

    Ok(matches)
}

fn build_context_lines(
    context_lines: usize,
    index: usize,
    lines: &[String],
) -> (Vec<ContextLine>, Vec<ContextLine>) {
    let before_start = index.saturating_sub(context_lines + 1);
    let before = if context_lines == 0 {
        Vec::new()
    } else {
        lines[before_start..index]
            .iter()
            .enumerate()
            .map(|(offset, content)| ContextLine {
                line: before_start + offset + 1,
                text: content.clone(),
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
                text: content.clone(),
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

fn absolutize_path(path: &Path) -> Result<PathBuf, AppError> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    let current_dir = current_dir()?;
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
