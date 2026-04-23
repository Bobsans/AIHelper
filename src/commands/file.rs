use std::{
    collections::VecDeque,
    fs::{self, File, Metadata},
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use clap::{Args, Subcommand};
use serde::Serialize;

use crate::{cli::GlobalOptions, error::AppError, output::OutputMode};

#[derive(Debug, Args)]
pub struct FileArgs {
    #[command(subcommand)]
    pub command: FileCommand,
}

#[derive(Debug, Subcommand)]
pub enum FileCommand {
    Read(ReadArgs),
    Head(HeadArgs),
    Tail(TailArgs),
    Stat(StatArgs),
    Tree(TreeArgs),
}

#[derive(Debug, Args)]
pub struct ReadArgs {
    pub path: PathBuf,
    #[arg(short = 'n', long = "number-lines", help = "Show line numbers")]
    pub number_lines: bool,
    #[arg(long, value_name = "N", help = "Start line (1-based)")]
    pub from: Option<usize>,
    #[arg(long, value_name = "N", help = "End line (1-based)")]
    pub to: Option<usize>,
}

#[derive(Debug, Args)]
pub struct HeadArgs {
    pub path: PathBuf,
    #[arg(long, default_value_t = 20)]
    pub lines: usize,
    #[arg(short = 'n', long = "number-lines", help = "Show line numbers")]
    pub number_lines: bool,
}

#[derive(Debug, Args)]
pub struct TailArgs {
    pub path: PathBuf,
    #[arg(long, default_value_t = 20)]
    pub lines: usize,
    #[arg(short = 'n', long = "number-lines", help = "Show line numbers")]
    pub number_lines: bool,
}

#[derive(Debug, Args)]
pub struct StatArgs {
    pub path: PathBuf,
}

#[derive(Debug, Args)]
pub struct TreeArgs {
    pub path: Option<PathBuf>,
    #[arg(long)]
    pub depth: Option<usize>,
}

#[derive(Debug, Serialize)]
struct FileLinesOutput {
    command: &'static str,
    path: String,
    from: Option<usize>,
    to: Option<usize>,
    numbered: bool,
    line_count: usize,
    truncated: bool,
    content: String,
}

#[derive(Debug, Serialize)]
struct FileStatOutput {
    command: &'static str,
    path: String,
    kind: &'static str,
    size_bytes: u64,
    readonly: bool,
    modified_unix_seconds: Option<u64>,
    created_unix_seconds: Option<u64>,
}

#[derive(Debug, Serialize)]
struct TreeEntry {
    depth: usize,
    kind: &'static str,
    name: String,
    path: String,
}

#[derive(Debug, Serialize)]
struct FileTreeOutput {
    command: &'static str,
    path: String,
    max_depth: Option<usize>,
    entry_count: usize,
    truncated: bool,
    entries: Vec<TreeEntry>,
}

pub fn execute(args: FileArgs, options: &GlobalOptions) -> Result<(), AppError> {
    match args.command {
        FileCommand::Read(read_args) => execute_read(read_args, options),
        FileCommand::Head(head_args) => execute_head(head_args, options),
        FileCommand::Tail(tail_args) => execute_tail(tail_args, options),
        FileCommand::Stat(stat_args) => execute_stat(stat_args, options),
        FileCommand::Tree(tree_args) => execute_tree(tree_args, options),
    }
}

fn execute_read(args: ReadArgs, options: &GlobalOptions) -> Result<(), AppError> {
    let from = args.from.unwrap_or(1);
    let to = args.to.unwrap_or(usize::MAX);

    validate_line_range(from, to)?;

    let (raw_lines, truncated) = read_lines_in_range(&args.path, from, to, line_cap(options))?;
    emit_lines_output(
        "file.read",
        &args.path,
        args.number_lines,
        Some(from),
        args.to,
        raw_lines,
        truncated,
        options,
    )
}

fn execute_head(args: HeadArgs, options: &GlobalOptions) -> Result<(), AppError> {
    let lines = args.lines;

    let (raw_lines, truncated) = read_lines_in_range(&args.path, 1, lines, line_cap(options))?;
    let from = if raw_lines.is_empty() { None } else { Some(1) };
    let to = raw_lines.last().map(|(line_number, _)| *line_number);
    emit_lines_output(
        "file.head",
        &args.path,
        args.number_lines,
        from,
        to,
        raw_lines,
        truncated,
        options,
    )
}

fn execute_tail(args: TailArgs, options: &GlobalOptions) -> Result<(), AppError> {
    let (raw_lines, truncated) = read_tail_lines(&args.path, args.lines, line_cap(options))?;
    let from = raw_lines.first().map(|(line_number, _)| *line_number);
    let to = raw_lines.last().map(|(line_number, _)| *line_number);
    emit_lines_output(
        "file.tail",
        &args.path,
        args.number_lines,
        from,
        to,
        raw_lines,
        truncated,
        options,
    )
}

fn execute_stat(args: StatArgs, options: &GlobalOptions) -> Result<(), AppError> {
    let metadata = fs::symlink_metadata(&args.path)
        .map_err(|source| AppError::file_metadata(args.path.clone(), source))?;
    let kind = metadata_kind(&metadata);
    let output = FileStatOutput {
        command: "file.stat",
        path: args.path.to_string_lossy().into_owned(),
        kind,
        size_bytes: metadata.len(),
        readonly: metadata.permissions().readonly(),
        modified_unix_seconds: metadata
            .modified()
            .ok()
            .and_then(system_time_to_unix_seconds),
        created_unix_seconds: metadata
            .created()
            .ok()
            .and_then(system_time_to_unix_seconds),
    };

    if options.quiet {
        return Ok(());
    }

    match options.output {
        OutputMode::Text => {
            println!("path: {}", output.path);
            println!("kind: {}", output.kind);
            println!("size_bytes: {}", output.size_bytes);
            println!("readonly: {}", output.readonly);
            println!(
                "modified_unix_seconds: {}",
                optional_number(output.modified_unix_seconds)
            );
            println!(
                "created_unix_seconds: {}",
                optional_number(output.created_unix_seconds)
            );
        }
        OutputMode::Json => {
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
    }

    Ok(())
}

fn execute_tree(args: TreeArgs, options: &GlobalOptions) -> Result<(), AppError> {
    let path = args.path.unwrap_or_else(|| PathBuf::from("."));
    let mut entries = Vec::new();
    collect_tree_entries(&path, 0, args.depth, &mut entries)?;

    let truncated = apply_limit(&mut entries, options.limit);

    if options.quiet {
        return Ok(());
    }

    match options.output {
        OutputMode::Text => {
            let content = render_tree_text(&entries);
            if !content.is_empty() {
                println!("{content}");
            }
            if truncated {
                eprintln!("warning: output truncated by --limit");
            }
        }
        OutputMode::Json => {
            let payload = FileTreeOutput {
                command: "file.tree",
                path: path.to_string_lossy().into_owned(),
                max_depth: args.depth,
                entry_count: entries.len(),
                truncated,
                entries,
            };
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
    }

    Ok(())
}

fn emit_lines_output(
    command: &'static str,
    path: &Path,
    number_lines: bool,
    from: Option<usize>,
    to: Option<usize>,
    raw_lines: Vec<(usize, String)>,
    truncated: bool,
    options: &GlobalOptions,
) -> Result<(), AppError> {
    if options.quiet {
        return Ok(());
    }

    let line_count = raw_lines.len();
    let rendered_lines: Vec<String> = raw_lines
        .into_iter()
        .map(|(line_number, line)| {
            if number_lines {
                format!("{line_number:>4}: {line}")
            } else {
                line
            }
        })
        .collect();
    let content = rendered_lines.join("\n");

    match options.output {
        OutputMode::Text => {
            if !content.is_empty() {
                println!("{content}");
            }
            if truncated {
                eprintln!("warning: output truncated by --limit");
            }
        }
        OutputMode::Json => {
            let payload = FileLinesOutput {
                command,
                path: path.to_string_lossy().into_owned(),
                from,
                to,
                numbered: number_lines,
                line_count,
                truncated,
                content,
            };
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
    }

    Ok(())
}

fn read_lines_in_range(
    path: &Path,
    from: usize,
    to: usize,
    line_cap: usize,
) -> Result<(Vec<(usize, String)>, bool), AppError> {
    if to == 0 || from > to {
        return Ok((Vec::new(), false));
    }

    let file =
        File::open(path).map_err(|source| AppError::file_read(path.to_path_buf(), source))?;
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

        let line = line_result.map_err(|source| AppError::file_read(path.to_path_buf(), source))?;
        if selected.len() < line_cap {
            selected.push((line_number, line));
        } else {
            truncated = true;
            break;
        }
    }

    Ok((selected, truncated))
}

fn read_tail_lines(
    path: &Path,
    requested_lines: usize,
    line_cap: usize,
) -> Result<(Vec<(usize, String)>, bool), AppError> {
    if requested_lines == 0 {
        return Ok((Vec::new(), false));
    }

    let file =
        File::open(path).map_err(|source| AppError::file_read(path.to_path_buf(), source))?;
    let reader = BufReader::new(file);
    let mut queue: VecDeque<(usize, String)> = VecDeque::new();

    for (index, line_result) in reader.lines().enumerate() {
        let line_number = index + 1;
        let line = line_result.map_err(|source| AppError::file_read(path.to_path_buf(), source))?;
        if queue.len() == requested_lines {
            queue.pop_front();
        }
        queue.push_back((line_number, line));
    }

    let mut selected: Vec<(usize, String)> = queue.into_iter().collect();
    let truncated = apply_limit(&mut selected, Some(line_cap));
    Ok((selected, truncated))
}

fn collect_tree_entries(
    path: &Path,
    depth: usize,
    max_depth: Option<usize>,
    entries: &mut Vec<TreeEntry>,
) -> Result<(), AppError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|source| AppError::file_metadata(path.to_path_buf(), source))?;
    let kind = metadata_kind(&metadata);
    entries.push(TreeEntry {
        depth,
        kind,
        name: display_name_for_path(path),
        path: path.to_string_lossy().into_owned(),
    });

    if kind != "directory" {
        return Ok(());
    }

    let should_descend = max_depth.map(|limit| depth < limit).unwrap_or(true);
    if !should_descend {
        return Ok(());
    }

    let mut children: Vec<PathBuf> = Vec::new();
    let read_dir = fs::read_dir(path)
        .map_err(|source| AppError::directory_read(path.to_path_buf(), source))?;
    for entry_result in read_dir {
        let entry =
            entry_result.map_err(|source| AppError::directory_read(path.to_path_buf(), source))?;
        children.push(entry.path());
    }
    children.sort_by(|left, right| {
        display_name_for_path(left)
            .to_lowercase()
            .cmp(&display_name_for_path(right).to_lowercase())
    });

    for child_path in children {
        collect_tree_entries(&child_path, depth + 1, max_depth, entries)?;
    }

    Ok(())
}

fn metadata_kind(metadata: &Metadata) -> &'static str {
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

fn display_name_for_path(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

fn render_tree_text(entries: &[TreeEntry]) -> String {
    entries
        .iter()
        .map(|entry| {
            let mut label = entry.name.clone();
            if entry.kind == "directory" {
                label.push('/');
            }
            if entry.depth == 0 {
                label
            } else {
                format!("{}- {label}", "  ".repeat(entry.depth))
            }
        })
        .collect::<Vec<String>>()
        .join("\n")
}

fn line_cap(options: &GlobalOptions) -> usize {
    options.limit.unwrap_or(usize::MAX)
}

fn apply_limit<T>(items: &mut Vec<T>, limit: Option<usize>) -> bool {
    if let Some(limit_value) = limit {
        if items.len() > limit_value {
            items.truncate(limit_value);
            return true;
        }
    }
    false
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

fn system_time_to_unix_seconds(timestamp: SystemTime) -> Option<u64> {
    timestamp
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|value| value.as_secs())
}

fn optional_number(value: Option<u64>) -> String {
    value
        .map(|number| number.to_string())
        .unwrap_or_else(|| "null".to_owned())
}
