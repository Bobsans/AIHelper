use std::collections::BTreeSet;

use globset::{Glob, GlobSet, GlobSetBuilder};
use regex::{Regex, RegexBuilder};
use serde::Serialize;

use crate::error::AppError;

use super::{adapters, FilesArgs, TextArgs};

#[derive(Debug, Serialize)]
pub(crate) struct SearchTextOutput {
    pub command: &'static str,
    pub backend: String,
    pub root: String,
    pub roots: Vec<String>,
    pub pattern: String,
    pub regex: bool,
    pub ignore_case: bool,
    pub context: usize,
    pub match_count: usize,
    pub file_count: usize,
    pub skipped_binary_files: usize,
    pub skipped_large_files: usize,
    pub skipped_symlink_files: usize,
    pub truncated: bool,
    pub matches: Vec<TextMatch>,
}

#[derive(Debug, Serialize)]
pub(crate) struct SearchFilesOutput {
    pub command: &'static str,
    pub backend: String,
    pub root: String,
    pub roots: Vec<String>,
    pub query: String,
    pub match_count: usize,
    pub truncated: bool,
    pub files: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct TextMatch {
    pub path: String,
    pub line: usize,
    pub column: usize,
    pub text: String,
    pub context_before: Vec<ContextLine>,
    pub context_after: Vec<ContextLine>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ContextLine {
    pub line: usize,
    pub text: String,
}

#[derive(Default)]
pub(crate) struct TextCollectStats {
    pub(crate) skipped_binary_files: usize,
    pub(crate) skipped_large_files: usize,
    pub(crate) skipped_symlink_files: usize,
}

pub(crate) enum PatternMatcher {
    Literal {
        needle: String,
        needle_lower: Option<String>,
        ignore_case: bool,
    },
    Regex {
        pattern: Regex,
    },
}

pub(crate) enum SearchResult {
    Text(SearchTextOutput),
    Files(SearchFilesOutput),
}

pub(crate) fn execute_text(
    args: TextArgs,
    limit: Option<usize>,
) -> Result<SearchResult, AppError> {
    crate::safety::validate_max_bytes(args.max_bytes)?;
    let scope = adapters::io::resolve_scope(&args.paths, args.follow_symlinks)?;
    let context_lines = args.context.unwrap_or(0);
    let matcher = build_matcher(&args.pattern, args.regex, args.ignore_case)?;
    let globset = build_globset(&args.globs)?;

    let backend_supports_rg = adapters::io::rg_is_available();
    let candidate_files = if backend_supports_rg {
        if let Some(files) = adapters::io::candidate_files_with_rg(&args, &scope.roots) {
            files
        } else {
            adapters::io::collect_files_from_roots(
                &scope.roots,
                globset.as_ref(),
                args.follow_symlinks,
            )?
        }
    } else {
        adapters::io::collect_files_from_roots(&scope.roots, globset.as_ref(), args.follow_symlinks)?
    };

    let backend = if backend_supports_rg {
        "rg+rust".to_owned()
    } else {
        "rust".to_owned()
    };

    let (matches, stats, truncated) = adapters::io::collect_text_matches(
        candidate_files,
        &scope.display_root,
        &matcher,
        context_lines,
        args.max_bytes,
        args.follow_symlinks,
        limit,
    )?;

    let file_count = matches.iter().map(|item| item.path.as_str()).collect::<BTreeSet<_>>().len();
    Ok(SearchResult::Text(SearchTextOutput {
        command: "search.text",
        backend,
        root: scope.root_label.clone(),
        roots: scope.root_labels.clone(),
        pattern: args.pattern,
        regex: args.regex,
        ignore_case: args.ignore_case,
        context: context_lines,
        match_count: matches.len(),
        file_count,
        skipped_binary_files: stats.skipped_binary_files,
        skipped_large_files: stats.skipped_large_files,
        skipped_symlink_files: stats.skipped_symlink_files,
        truncated,
        matches,
    }))
}

pub(crate) fn execute_files(args: FilesArgs, limit: Option<usize>) -> Result<SearchResult, AppError> {
    let scope = adapters::io::resolve_scope(&args.paths, args.follow_symlinks)?;

    let (all_files, backend) = if adapters::io::rg_is_available() {
        if let Some(files) = adapters::io::files_with_rg(&scope.roots, args.follow_symlinks) {
            (files, "rg+rust".to_owned())
        } else {
            (
                adapters::io::collect_files_from_roots(&scope.roots, None, args.follow_symlinks)?,
                "rust".to_owned(),
            )
        }
    } else {
        (
            adapters::io::collect_files_from_roots(&scope.roots, None, args.follow_symlinks)?,
            "rust".to_owned(),
        )
    };

    let mut matched = Vec::new();
    let mut truncated = false;
    let max_count = limit.unwrap_or(usize::MAX);

    for file_path in all_files {
        let normalized_relative = adapters::io::display_path(&file_path, &scope.display_root);
        if normalized_relative.contains(&args.query) {
            if matched.len() < max_count {
                matched.push(normalized_relative);
            } else {
                truncated = true;
                break;
            }
        }
    }

    Ok(SearchResult::Files(SearchFilesOutput {
        command: "search.files",
        backend,
        root: scope.root_label,
        roots: scope.root_labels,
        query: args.query,
        match_count: matched.len(),
        truncated,
        files: matched,
    }))
}

fn build_matcher(
    pattern: &str,
    is_regex: bool,
    ignore_case: bool,
) -> Result<PatternMatcher, AppError> {
    if pattern.is_empty() {
        return Err(AppError::invalid_argument("pattern must not be empty"));
    }
    if is_regex {
        let compiled = RegexBuilder::new(pattern)
            .case_insensitive(ignore_case)
            .build()
            .map_err(|error| AppError::invalid_argument(format!("invalid regex pattern: {error}")))?;
        return Ok(PatternMatcher::Regex { pattern: compiled });
    }
    let needle_lower = if ignore_case {
        Some(pattern.to_lowercase())
    } else {
        None
    };
    Ok(PatternMatcher::Literal {
        needle: pattern.to_owned(),
        needle_lower,
        ignore_case,
    })
}

fn build_globset(globs: &[String]) -> Result<Option<GlobSet>, AppError> {
    if globs.is_empty() {
        return Ok(None);
    }
    let mut builder = GlobSetBuilder::new();
    for pattern in globs {
        let glob = Glob::new(pattern).map_err(|error| {
            AppError::invalid_argument(format!("invalid --glob '{pattern}': {error}"))
        })?;
        builder.add(glob);
    }
    let globset = builder
        .build()
        .map_err(|error| AppError::invalid_argument(format!("invalid glob set: {error}")))?;
    Ok(Some(globset))
}

pub(crate) fn find_match_column(matcher: &PatternMatcher, line: &str) -> Option<usize> {
    match matcher {
        PatternMatcher::Regex { pattern } => pattern
            .find(line)
            .map(|item| byte_index_to_column(line, item.start())),
        PatternMatcher::Literal {
            needle,
            needle_lower,
            ignore_case,
        } => {
            if *ignore_case {
                let needle_lower = needle_lower.as_ref()?;
                line.char_indices()
                    .find(|(index, _)| line[*index..].to_lowercase().starts_with(needle_lower))
                    .map(|(index, _)| byte_index_to_column(line, index))
            } else {
                line.find(needle)
                    .map(|index| byte_index_to_column(line, index))
            }
        }
    }
}

fn byte_index_to_column(line: &str, byte_index: usize) -> usize {
    line[..byte_index].chars().count() + 1
}
