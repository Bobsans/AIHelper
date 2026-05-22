use crate::{cli::GlobalOptions, error::AppError, output::OutputMode};

use crate::commands::file::domain::{
    FileLinesOutput, FileResult, FileStatOutput, FileTreeOutput, TreeEntry,
};

pub(crate) fn emit(result: FileResult, options: &GlobalOptions) -> Result<(), AppError> {
    if options.quiet {
        return Ok(());
    }

    match result {
        FileResult::Read(payload) => emit_lines(payload, options),
        FileResult::Head(payload) => emit_lines(payload, options),
        FileResult::Tail(payload) => emit_lines(payload, options),
        FileResult::Stat(payload) => emit_stat(payload, options),
        FileResult::Tree(payload) => emit_tree(payload, options),
    }
}

fn emit_lines(payload: FileLinesOutput, options: &GlobalOptions) -> Result<(), AppError> {
    match options.output {
        OutputMode::Text => {
            if !payload.content.is_empty() {
                println!("{}", payload.content);
            }
            if payload.truncated {
                eprintln!("warning: output truncated by --limit");
            }
            Ok(())
        }
        OutputMode::Json => {
            println!("{}", serde_json::to_string_pretty(&payload)?);
            Ok(())
        }
    }
}

fn emit_stat(payload: FileStatOutput, options: &GlobalOptions) -> Result<(), AppError> {
    match options.output {
        OutputMode::Text => {
            println!("path: {}", payload.path);
            println!("kind: {}", payload.kind);
            println!("size_bytes: {}", payload.size_bytes);
            println!("readonly: {}", payload.readonly);
            println!(
                "modified_unix_seconds: {}",
                optional_number(payload.modified_unix_seconds)
            );
            println!(
                "created_unix_seconds: {}",
                optional_number(payload.created_unix_seconds)
            );
            Ok(())
        }
        OutputMode::Json => {
            println!("{}", serde_json::to_string_pretty(&payload)?);
            Ok(())
        }
    }
}

fn emit_tree(payload: FileTreeOutput, options: &GlobalOptions) -> Result<(), AppError> {
    match options.output {
        OutputMode::Text => {
            let content = render_tree_text(&payload.entries);
            if !content.is_empty() {
                println!("{content}");
            }
            if payload.truncated {
                eprintln!("warning: output truncated by --limit");
            }
            Ok(())
        }
        OutputMode::Json => {
            println!("{}", serde_json::to_string_pretty(&payload)?);
            Ok(())
        }
    }
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

fn optional_number(value: Option<u64>) -> String {
    value
        .map(|number| number.to_string())
        .unwrap_or_else(|| "null".to_owned())
}
