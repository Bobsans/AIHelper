use std::{
    collections::{BTreeSet, HashSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use clap::{Args, Subcommand};
use globset::{Glob, GlobSet, GlobSetBuilder};
use regex::{Regex, RegexBuilder};
use serde::Serialize;
use walkdir::WalkDir;

use crate::{cli::GlobalOptions, error::AppError, output::OutputMode};

#[derive(Debug, Args)]
pub struct SearchArgs {
    #[command(subcommand)]
    pub command: SearchCommand,
}

#[derive(Debug, Subcommand)]
pub enum SearchCommand {
    Text(TextArgs),
    Files(FilesArgs),
}

#[derive(Debug, Args)]
pub struct TextArgs {
    pub pattern: String,
    pub path: Option<PathBuf>,
    #[arg(long = "glob")]
    pub globs: Vec<String>,
    #[arg(long)]
    pub ignore_case: bool,
    #[arg(long)]
    pub context: Option<usize>,
    #[arg(
        long,
        help = "Interpret pattern as regex (default: literal/plain search)"
    )]
    pub regex: bool,
}

#[derive(Debug, Args)]
pub struct FilesArgs {
    pub query: String,
    pub path: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
struct ContextLine {
    line: usize,
    text: String,
}

#[derive(Debug, Serialize)]
struct TextMatch {
    path: String,
    line: usize,
    column: usize,
    text: String,
    context_before: Vec<ContextLine>,
    context_after: Vec<ContextLine>,
}

#[derive(Debug, Serialize)]
struct SearchTextOutput {
    command: &'static str,
    backend: String,
    root: String,
    pattern: String,
    regex: bool,
    ignore_case: bool,
    context: usize,
    match_count: usize,
    file_count: usize,
    truncated: bool,
    matches: Vec<TextMatch>,
}

#[derive(Debug, Serialize)]
struct SearchFilesOutput {
    command: &'static str,
    backend: String,
    root: String,
    query: String,
    match_count: usize,
    truncated: bool,
    files: Vec<String>,
}

enum PatternMatcher {
    Literal {
        needle: String,
        needle_lower: Option<String>,
        ignore_case: bool,
    },
    Regex {
        pattern: Regex,
    },
}

pub fn execute(args: SearchArgs, options: &GlobalOptions) -> Result<(), AppError> {
    match args.command {
        SearchCommand::Text(text_args) => execute_text(text_args, options),
        SearchCommand::Files(files_args) => execute_files(files_args, options),
    }
}

fn execute_text(args: TextArgs, options: &GlobalOptions) -> Result<(), AppError> {
    let root = resolve_root(args.path.as_ref())?;
    let context_lines = args.context.unwrap_or(0);
    let matcher = build_matcher(&args.pattern, args.regex, args.ignore_case)?;
    let globset = build_globset(&args.globs)?;
    let backend_supports_rg = rg_is_available();

    let candidate_files = if backend_supports_rg {
        match candidate_files_with_rg(&args, &root) {
            Some(files) => files,
            None => collect_files_fallback(&root, globset.as_ref())?,
        }
    } else {
        collect_files_fallback(&root, globset.as_ref())?
    };

    let backend = if backend_supports_rg {
        "rg+rust"
    } else {
        "rust"
    };

    let (matches, truncated) = collect_text_matches(
        candidate_files,
        &root,
        &matcher,
        context_lines,
        options.limit,
    )?;

    if options.quiet {
        return Ok(());
    }

    let file_count = matches
        .iter()
        .map(|item| item.path.as_str())
        .collect::<BTreeSet<_>>()
        .len();

    match options.output {
        OutputMode::Text => {
            let rendered = render_text_matches(&matches, context_lines);
            if !rendered.is_empty() {
                println!("{rendered}");
            }
            if truncated {
                eprintln!("warning: output truncated by --limit");
            }
        }
        OutputMode::Json => {
            let payload = SearchTextOutput {
                command: "search.text",
                backend: backend.to_owned(),
                root: root.to_string_lossy().into_owned(),
                pattern: args.pattern,
                regex: args.regex,
                ignore_case: args.ignore_case,
                context: context_lines,
                match_count: matches.len(),
                file_count,
                truncated,
                matches,
            };
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
    }

    Ok(())
}

fn execute_files(args: FilesArgs, options: &GlobalOptions) -> Result<(), AppError> {
    let root = resolve_root(args.path.as_ref())?;
    let query = args.query;

    let (all_files, backend) = if rg_is_available() {
        match files_with_rg(&root) {
            Some(files) => (files, "rg+rust"),
            None => (collect_files_fallback(&root, None)?, "rust"),
        }
    } else {
        (collect_files_fallback(&root, None)?, "rust")
    };

    let mut matched = Vec::new();
    let mut truncated = false;
    let max_count = options.limit.unwrap_or(usize::MAX);

    for file_path in all_files {
        let relative_path = file_path.strip_prefix(&root).unwrap_or(file_path.as_path());
        let normalized_relative = normalize_path_for_match(relative_path);
        let haystack = normalized_relative.clone();
        if haystack.contains(&query) {
            if matched.len() < max_count {
                matched.push(normalized_relative);
            } else {
                truncated = true;
                break;
            }
        }
    }

    if options.quiet {
        return Ok(());
    }

    match options.output {
        OutputMode::Text => {
            if !matched.is_empty() {
                println!("{}", matched.join("\n"));
            }
            if truncated {
                eprintln!("warning: output truncated by --limit");
            }
        }
        OutputMode::Json => {
            let payload = SearchFilesOutput {
                command: "search.files",
                backend: backend.to_owned(),
                root: root.to_string_lossy().into_owned(),
                query,
                match_count: matched.len(),
                truncated,
                files: matched,
            };
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
    }

    Ok(())
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
            .map_err(|error| {
                AppError::invalid_argument(format!("invalid regex pattern: {error}"))
            })?;
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

fn resolve_root(root: Option<&PathBuf>) -> Result<PathBuf, AppError> {
    let path = root.cloned().unwrap_or_else(|| PathBuf::from("."));
    if path.exists() {
        Ok(path)
    } else {
        Err(AppError::invalid_argument(format!(
            "path does not exist: {}",
            path.to_string_lossy()
        )))
    }
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

fn rg_is_available() -> bool {
    Command::new("rg")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn candidate_files_with_rg(args: &TextArgs, root: &Path) -> Option<Vec<PathBuf>> {
    let mut command = Command::new("rg");
    command.arg("-l");
    command.arg("--color").arg("never");
    command.arg("--no-messages");
    if args.ignore_case {
        command.arg("-i");
    }
    for glob in &args.globs {
        command.arg("-g").arg(glob);
    }
    if !args.regex {
        command.arg("-F");
    }
    command.arg("--").arg(&args.pattern).arg(root);

    let output = command.output().ok()?;
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

fn files_with_rg(root: &Path) -> Option<Vec<PathBuf>> {
    let output = Command::new("rg").arg("--files").arg(root).output().ok()?;
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

fn collect_files_fallback(
    root: &Path,
    globset: Option<&GlobSet>,
) -> Result<Vec<PathBuf>, AppError> {
    if root.is_file() {
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
    for entry in WalkDir::new(root) {
        let entry = entry.map_err(|error| {
            AppError::directory_read(root.to_path_buf(), std::io::Error::other(error))
        })?;
        if entry.file_type().is_file() && file_matches_globs(entry.path(), root, globset) {
            files.push(entry.path().to_path_buf());
        }
    }
    files.sort();
    Ok(files)
}

fn file_matches_globs(path: &Path, root: &Path, globset: Option<&GlobSet>) -> bool {
    let Some(globset) = globset else {
        return true;
    };

    let relative = if root.is_dir() {
        path.strip_prefix(root).unwrap_or(path)
    } else {
        path
    };
    let normalized = normalize_path_for_match(relative);
    globset.is_match(normalized)
}

fn collect_text_matches(
    files: Vec<PathBuf>,
    root: &Path,
    matcher: &PatternMatcher,
    context_lines: usize,
    limit: Option<usize>,
) -> Result<(Vec<TextMatch>, bool), AppError> {
    let max_count = limit.unwrap_or(usize::MAX);
    let mut matches = Vec::new();
    let mut truncated = false;
    let mut seen: HashSet<(String, usize)> = HashSet::new();

    'file_loop: for path in files {
        let file_matches = match_file(&path, root, matcher, context_lines)?;
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

    Ok((matches, truncated))
}

fn match_file(
    path: &Path,
    root: &Path,
    matcher: &PatternMatcher,
    context_lines: usize,
) -> Result<Vec<TextMatch>, AppError> {
    let bytes = fs::read(path).map_err(|source| AppError::file_read(path.to_path_buf(), source))?;
    if bytes.contains(&0) {
        return Ok(Vec::new());
    }

    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<String> = text.lines().map(|line| line.to_owned()).collect();

    let mut matches = Vec::new();
    let normalized_path = normalize_path_for_match(path.strip_prefix(root).unwrap_or(path));

    for (index, line) in lines.iter().enumerate() {
        if let Some(column) = find_match_column(matcher, line) {
            let line_number = index + 1;
            let before_start = line_number.saturating_sub(context_lines + 1);
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

            let after_end = (index + context_lines + 1).min(lines.len().saturating_sub(1));
            let after = if context_lines == 0 || index + 1 >= lines.len() {
                Vec::new()
            } else {
                lines[(index + 1)..=after_end]
                    .iter()
                    .enumerate()
                    .map(|(offset, content)| ContextLine {
                        line: line_number + offset + 1,
                        text: content.clone(),
                    })
                    .collect()
            };

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

fn find_match_column(matcher: &PatternMatcher, line: &str) -> Option<usize> {
    match matcher {
        PatternMatcher::Regex { pattern } => pattern.find(line).map(|item| item.start() + 1),
        PatternMatcher::Literal {
            needle,
            needle_lower,
            ignore_case,
        } => {
            if *ignore_case {
                let haystack_lower = line.to_lowercase();
                haystack_lower
                    .find(needle_lower.as_ref()?)
                    .map(|index| index + 1)
            } else {
                line.find(needle).map(|index| index + 1)
            }
        }
    }
}

fn render_text_matches(matches: &[TextMatch], context_lines: usize) -> String {
    let mut lines = Vec::new();
    for (index, item) in matches.iter().enumerate() {
        if context_lines > 0 && index > 0 {
            lines.push("--".to_owned());
        }
        for context in &item.context_before {
            lines.push(format!("{}-{}-{}", item.path, context.line, context.text));
        }
        lines.push(format!("{}:{}:{}", item.path, item.line, item.text));
        for context in &item.context_after {
            lines.push(format!("{}-{}-{}", item.path, context.line, context.text));
        }
    }

    lines.join("\n")
}

fn normalize_path_for_match(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}
