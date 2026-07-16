use crate::commands::{
    ctx::domain::{
        ChangedEntry, CtxChangedOutput, CtxPackOutput, CtxResult, CtxSymbolsOutput, PackItem,
    },
    ctx_symbols::Symbol,
};
use crate::{
    cli::GlobalOptions,
    error::AppError,
    output::{
        OutputMode, TextFormatter, TextStyle, emit_warning, git_status_style, render_semantic_count,
    },
};

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
                let formatter = TextFormatter::stdout();
                println!(
                    "{} {}",
                    formatter.paint(TextStyle::Muted, "preset:"),
                    formatter.paint(TextStyle::Key, &payload.preset)
                );
                println!("{}", render_pack_counts(&payload, formatter));
                println!(
                    "{}",
                    render_skipped_counts(
                        payload.skipped_binary_files,
                        payload.skipped_large_files,
                        payload.skipped_symlink_files,
                        formatter
                    )
                );
                for item in &payload.items {
                    println!("{}", render_pack_item(item, formatter));
                    for symbol in &item.symbols {
                        println!("{}", render_symbol(symbol, true, formatter));
                    }
                }
            }
            if payload.truncated {
                emit_warning("output truncated by --limit");
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
            let formatter = TextFormatter::stdout();
            println!(
                "{} {}",
                formatter.paint(TextStyle::Muted, "preset:"),
                formatter.paint(TextStyle::Key, &payload.preset)
            );
            println!(
                "{}",
                render_skipped_counts(
                    payload.skipped_binary_files,
                    payload.skipped_large_files,
                    payload.skipped_symlink_files,
                    formatter
                )
            );
            for file in &payload.files {
                println!("{}", formatter.paint(TextStyle::Key, &file.path));
                for symbol in &file.symbols {
                    println!("{}", render_symbol(symbol, false, formatter));
                }
            }
            if payload.truncated {
                emit_warning("output truncated by --limit");
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
            let formatter = TextFormatter::stdout();
            if !payload.in_git_repo {
                println!(
                    "{}",
                    formatter.paint(TextStyle::Warning, "not a git repository")
                );
                return Ok(());
            }
            if payload.entries.is_empty() {
                println!(
                    "{}",
                    formatter.paint(TextStyle::Success, "working tree is clean")
                );
                return Ok(());
            }
            for entry in &payload.entries {
                println!("{}", render_changed_entry(entry, formatter));
            }
        }
        OutputMode::Json => {
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
    }

    Ok(())
}

fn render_pack_counts(payload: &CtxPackOutput, formatter: TextFormatter) -> String {
    formatter.paint(
        TextStyle::Muted,
        format!(
            "items: {} (files: {}, directories: {}, symbols: {})",
            payload.item_count, payload.file_count, payload.directory_count, payload.symbol_count
        ),
    )
}

fn render_skipped_counts(
    binary: usize,
    large: usize,
    symlink: usize,
    formatter: TextFormatter,
) -> String {
    format!(
        "{} {} {} {}",
        formatter.paint(TextStyle::Muted, "skipped:"),
        render_semantic_count("binary", binary, TextStyle::Warning, formatter),
        render_semantic_count("large", large, TextStyle::Warning, formatter),
        render_semantic_count("symlink", symlink, TextStyle::Warning, formatter)
    )
}

fn render_pack_item(item: &PackItem, formatter: TextFormatter) -> String {
    format!(
        "{} | {} | {} | {} | {}",
        formatter.paint(TextStyle::Heading, &item.kind),
        formatter.paint(TextStyle::Key, &item.path),
        formatter.paint(TextStyle::Muted, format!("size={}", item.size_bytes)),
        formatter.paint(TextStyle::Muted, format!("lines={}", item.line_count)),
        formatter.paint(TextStyle::Muted, format!("symbols={}", item.symbol_count))
    )
}

fn render_symbol(symbol: &Symbol, bullet: bool, formatter: TextFormatter) -> String {
    let prefix = if bullet { "  - " } else { "  " };
    format!(
        "{}{}:{} {}",
        prefix,
        formatter.paint(TextStyle::Muted, symbol.line),
        formatter.paint(TextStyle::Heading, &symbol.kind),
        formatter.paint(TextStyle::Key, &symbol.name)
    )
}

fn render_changed_entry(entry: &ChangedEntry, formatter: TextFormatter) -> String {
    let status = formatter.paint(git_status_style(&entry.status), &entry.status);
    match &entry.old_path {
        Some(old_path) => format!(
            "{} {} -> {}",
            status,
            formatter.paint(TextStyle::Key, old_path),
            formatter.paint(TextStyle::Key, &entry.path)
        ),
        None => format!(
            "{} {}",
            status,
            formatter.paint(TextStyle::Key, &entry.path)
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{render_changed_entry, render_pack_item, render_skipped_counts, render_symbol};
    use crate::{
        commands::{
            ctx::domain::{ChangedEntry, PackItem},
            ctx_symbols::Symbol,
        },
        output::TextFormatter,
    };

    #[test]
    fn skipped_counts_preserve_plain_contract_and_warn_for_non_zero_values() {
        assert_eq!(
            render_skipped_counts(0, 2, 0, TextFormatter::with_color(false)),
            "skipped: binary=0 large=2 symlink=0"
        );

        let rendered = render_skipped_counts(0, 2, 0, TextFormatter::with_color(true));
        assert!(rendered.contains("\u{1b}[33mlarge=2\u{1b}[0m"));
        assert!(rendered.contains("\u{1b}[2mbinary=0\u{1b}[0m"));
    }

    #[test]
    fn pack_item_renderer_preserves_plain_contract() {
        let item = PackItem {
            path: "src/lib.rs".to_owned(),
            kind: "file".to_owned(),
            size_bytes: 42,
            line_count: 3,
            symbol_count: 1,
            symbols: Vec::new(),
        };

        assert_eq!(
            render_pack_item(&item, TextFormatter::with_color(false)),
            "file | src/lib.rs | size=42 | lines=3 | symbols=1"
        );
    }

    #[test]
    fn symbol_renderer_preserves_plain_contract_and_styles_fields() {
        let symbol = Symbol {
            line: 12,
            kind: "fn".to_owned(),
            name: "run".to_owned(),
        };

        assert_eq!(
            render_symbol(&symbol, true, TextFormatter::with_color(false)),
            "  - 12:fn run"
        );
        let rendered = render_symbol(&symbol, false, TextFormatter::with_color(true));
        assert!(rendered.starts_with("  \u{1b}[2m12\u{1b}[0m:"));
        assert!(rendered.contains("\u{1b}[1;36mfn\u{1b}[0m"));
        assert!(rendered.contains("\u{1b}[36mrun\u{1b}[0m"));
    }

    #[test]
    fn changed_renderer_styles_status_and_paths() {
        let entry = ChangedEntry {
            status: "R ".to_owned(),
            path: "new.rs".to_owned(),
            old_path: Some("old.rs".to_owned()),
        };

        assert_eq!(
            render_changed_entry(&entry, TextFormatter::with_color(false)),
            "R  old.rs -> new.rs"
        );
        let rendered = render_changed_entry(&entry, TextFormatter::with_color(true));
        assert!(rendered.contains("\u{1b}[36mR \u{1b}[0m"));
        assert!(rendered.contains("\u{1b}[36mold.rs\u{1b}[0m"));
        assert!(rendered.contains("\u{1b}[36mnew.rs\u{1b}[0m"));
    }
}
