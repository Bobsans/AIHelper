use serde::Serialize;

use crate::error::AppError;
use crate::safety::{TextFileDecision, TextFilePolicy};
use ah_runtime::core::apply_limit;

use super::{adapters, FileArgs, FileCommand, HeadArgs, ReadArgs, StatArgs, TailArgs, TreeArgs};

#[derive(Debug, Serialize)]
pub(crate) struct FileLinesOutput {
    pub command: &'static str,
    pub path: String,
    pub from: Option<usize>,
    pub to: Option<usize>,
    pub numbered: bool,
    pub line_count: usize,
    pub truncated: bool,
    pub content: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct FileStatOutput {
    pub command: &'static str,
    pub path: String,
    pub kind: &'static str,
    pub size_bytes: u64,
    pub readonly: bool,
    pub modified_unix_seconds: Option<u64>,
    pub created_unix_seconds: Option<u64>,
}

#[derive(Debug, Serialize)]
pub(crate) struct TreeEntry {
    pub depth: usize,
    pub kind: &'static str,
    pub name: String,
    pub path: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct FileTreeOutput {
    pub command: &'static str,
    pub path: String,
    pub max_depth: Option<usize>,
    pub entry_count: usize,
    pub truncated: bool,
    pub entries: Vec<TreeEntry>,
}

#[derive(Debug)]
pub(crate) enum FileResult {
    Read(FileLinesOutput),
    Head(FileLinesOutput),
    Tail(FileLinesOutput),
    Stat(FileStatOutput),
    Tree(FileTreeOutput),
}

pub(crate) fn execute(args: FileArgs, limit: Option<usize>) -> Result<FileResult, AppError> {
    match args.command {
        FileCommand::Read(read_args) => Ok(execute_read(read_args, limit)?),
        FileCommand::Head(head_args) => Ok(execute_head(head_args, limit)?),
        FileCommand::Tail(tail_args) => Ok(execute_tail(tail_args, limit)?),
        FileCommand::Stat(stat_args) => Ok(execute_stat(stat_args)?),
        FileCommand::Tree(tree_args) => Ok(execute_tree(tree_args, limit)?),
    }
}

fn execute_read(args: ReadArgs, limit: Option<usize>) -> Result<FileResult, AppError> {
    let from = args.from.unwrap_or(1);
    let to = args.to.unwrap_or(usize::MAX);
    validate_line_range(from, to)?;
    let policy = TextFilePolicy {
        max_bytes: args.max_bytes,
        follow_symlinks: args.follow_symlinks,
    };
    let decision = adapters::io::inspect_text_file(&args.path, &policy)?;
    if let TextFileDecision::Skip(reason) = decision {
        return Err(crate::safety::skip_reason_to_error(&args.path, reason));
    }

    let (raw_lines, truncated) = adapters::io::read_lines_in_range(&args.path, from, to, limit)?;
    Ok(FileResult::Read(FileLinesOutput {
        command: "file.read",
        path: args.path.to_string_lossy().into_owned(),
        from: Some(from),
        to: args.to,
        numbered: args.number_lines,
        line_count: raw_lines.len(),
        truncated,
        content: render_lines(raw_lines, args.number_lines),
    }))
}

fn execute_head(args: HeadArgs, limit: Option<usize>) -> Result<FileResult, AppError> {
    let policy = TextFilePolicy {
        max_bytes: args.max_bytes,
        follow_symlinks: args.follow_symlinks,
    };
    let decision = adapters::io::inspect_text_file(&args.path, &policy)?;
    if let TextFileDecision::Skip(reason) = decision {
        return Err(crate::safety::skip_reason_to_error(&args.path, reason));
    }

    let (raw_lines, truncated) = adapters::io::read_lines_in_range(&args.path, 1, args.lines, limit)?;
    Ok(FileResult::Head(FileLinesOutput {
        command: "file.head",
        path: args.path.to_string_lossy().into_owned(),
        from: if raw_lines.is_empty() { None } else { Some(1) },
        to: raw_lines.last().map(|(line_number, _)| *line_number),
        numbered: args.number_lines,
        line_count: raw_lines.len(),
        truncated,
        content: render_lines(raw_lines, args.number_lines),
    }))
}

fn execute_tail(args: TailArgs, limit: Option<usize>) -> Result<FileResult, AppError> {
    let policy = TextFilePolicy {
        max_bytes: args.max_bytes,
        follow_symlinks: args.follow_symlinks,
    };
    let decision = adapters::io::inspect_text_file(&args.path, &policy)?;
    if let TextFileDecision::Skip(reason) = decision {
        return Err(crate::safety::skip_reason_to_error(&args.path, reason));
    }

    let (raw_lines, truncated) = adapters::io::read_tail_lines(&args.path, args.lines, limit)?;
    Ok(FileResult::Tail(FileLinesOutput {
        command: "file.tail",
        path: args.path.to_string_lossy().into_owned(),
        from: raw_lines.first().map(|(line_number, _)| *line_number),
        to: raw_lines.last().map(|(line_number, _)| *line_number),
        numbered: args.number_lines,
        line_count: raw_lines.len(),
        truncated,
        content: render_lines(raw_lines, args.number_lines),
    }))
}

fn execute_stat(args: StatArgs) -> Result<FileResult, AppError> {
    let metadata = adapters::io::metadata(&args.path)?;
    let kind = adapters::io::metadata_kind(&metadata);
    Ok(FileResult::Stat(FileStatOutput {
        command: "file.stat",
        path: args.path.to_string_lossy().into_owned(),
        kind,
        size_bytes: metadata.len(),
        readonly: metadata.permissions().readonly(),
        modified_unix_seconds: metadata.modified().ok().and_then(adapters::io::system_time_to_unix_seconds),
        created_unix_seconds: metadata.created().ok().and_then(adapters::io::system_time_to_unix_seconds),
    }))
}

fn execute_tree(args: TreeArgs, limit: Option<usize>) -> Result<FileResult, AppError> {
    let path = args.path.unwrap_or_else(|| std::path::PathBuf::from("."));
    let entries = adapters::io::collect_tree_entries(
        &path,
        0,
        args.depth,
        args.follow_symlinks,
        &mut std::collections::HashSet::new(),
    )?;
    let mut entries = entries;
    let truncated = apply_limit(&mut entries, limit);

    Ok(FileResult::Tree(FileTreeOutput {
        command: "file.tree",
        path: path.to_string_lossy().into_owned(),
        max_depth: args.depth,
        entry_count: entries.len(),
        truncated,
        entries,
    }))
}

fn render_lines(lines: Vec<(usize, String)>, number_lines: bool) -> String {
    lines
        .into_iter()
        .map(|(line_number, line)| {
            if number_lines {
                format!("{line_number:>4}: {line}")
            } else {
                line
            }
        })
        .collect::<Vec<String>>()
        .join("\n")
}

fn validate_line_range(from: usize, to: usize) -> Result<(), AppError> {
    if from == 0 {
        return Err(AppError::invalid_argument("--from must be >= 1"));
    }
    if to == 0 {
        return Err(AppError::invalid_argument("--to must be >= 1"));
    }
    if to < from {
        return Err(AppError::invalid_argument("--to must be >= --from"));
    }
    Ok(())
}

// keep output DTOs and command-level decisions in one place
