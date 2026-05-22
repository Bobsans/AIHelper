use crate::commands::ctx::domain::{CtxChangedOutput, CtxPackOutput, CtxResult, CtxSymbolsOutput};
use crate::{cli::GlobalOptions, error::AppError, output::OutputMode};

pub(crate) fn emit(result: CtxResult, options: &GlobalOptions) -> Result<(), AppError> {
    if options.quiet {
        return Ok(());
    }

    match result {
        CtxResult::Pack(payload) => emit_pack(payload, options),
        CtxResult::Symbols(payload) => emit_symbols(payload, options),
        CtxResult::Changed(payload) => emit_changed(payload, options),
    }
}

fn emit_pack(payload: CtxPackOutput, options: &GlobalOptions) -> Result<(), AppError> {
    match options.output {
        OutputMode::Text => {
            if !payload.items.is_empty() {
                println!("preset: {}", payload.preset);
                println!(
                    "items: {} (files: {}, directories: {}, symbols: {})",
                    payload.item_count,
                    payload.file_count,
                    payload.directory_count,
                    payload.symbol_count
                );
                println!(
                    "skipped: binary={} large={} symlink={}",
                    payload.skipped_binary_files, payload.skipped_large_files, payload.skipped_symlink_files
                );
                for item in &payload.items {
                    println!(
                        "{} | {} | size={} | lines={} | symbols={}",
                        item.kind, item.path, item.size_bytes, item.line_count, item.symbol_count
                    );
                    for symbol in &item.symbols {
                        println!("  - {}:{} {}", symbol.line, symbol.kind, symbol.name);
                    }
                }
            }
            if payload.truncated {
                eprintln!("warning: output truncated by --limit");
            }
        }
        OutputMode::Json => {
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
    }

    Ok(())
}

fn emit_symbols(payload: CtxSymbolsOutput, options: &GlobalOptions) -> Result<(), AppError> {
    match options.output {
        OutputMode::Text => {
            println!("preset: {}", payload.preset);
            println!(
                "skipped: binary={} large={} symlink={}",
                payload.skipped_binary_files, payload.skipped_large_files, payload.skipped_symlink_files
            );
            for file in &payload.files {
                println!("{}", file.path);
                for symbol in &file.symbols {
                    println!("  {}:{} {}", symbol.line, symbol.kind, symbol.name);
                }
            }
            if payload.truncated {
                eprintln!("warning: output truncated by --limit");
            }
        }
        OutputMode::Json => {
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
    }

    Ok(())
}

fn emit_changed(payload: CtxChangedOutput, options: &GlobalOptions) -> Result<(), AppError> {
    match options.output {
        OutputMode::Text => {
            if !payload.in_git_repo {
                println!("not a git repository");
                return Ok(());
            }
            if payload.entries.is_empty() {
                println!("working tree is clean");
                return Ok(());
            }
            for entry in &payload.entries {
                match &entry.old_path {
                    Some(old_path) => println!("{} {} -> {}", entry.status, old_path, entry.path),
                    None => println!("{} {}", entry.status, entry.path),
                }
            }
        }
        OutputMode::Json => {
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
    }

    Ok(())
}
