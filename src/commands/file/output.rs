use crate::{
    cli::GlobalOptions,
    error::AppError,
    output::{OutputMode, TextFormatter, TextStyle, emit_warning},
};

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
                emit_warning("output truncated by --limit");
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
            println!("{}", render_stat_text(&payload, TextFormatter::stdout()));
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
            let content = render_tree_text(&payload.entries, TextFormatter::stdout());
            if !content.is_empty() {
                println!("{content}");
            }
            if payload.truncated {
                emit_warning("output truncated by --limit");
            }
            Ok(())
        }
        OutputMode::Json => {
            println!("{}", serde_json::to_string_pretty(&payload)?);
            Ok(())
        }
    }
}

fn render_stat_text(payload: &FileStatOutput, formatter: TextFormatter) -> String {
    [
        format!(
            "{} {}",
            formatter.paint(TextStyle::Muted, "path:"),
            formatter.paint(TextStyle::Key, &payload.path)
        ),
        format!(
            "{} {}",
            formatter.paint(TextStyle::Muted, "kind:"),
            formatter.paint(file_kind_style(payload.kind), payload.kind)
        ),
        formatter.paint(
            TextStyle::Muted,
            format!("size_bytes: {}", payload.size_bytes),
        ),
        format!(
            "{} {}",
            formatter.paint(TextStyle::Muted, "readonly:"),
            formatter.paint(
                if payload.readonly {
                    TextStyle::Warning
                } else {
                    TextStyle::Muted
                },
                payload.readonly
            )
        ),
        formatter.paint(
            TextStyle::Muted,
            format!(
                "modified_unix_seconds: {}",
                optional_number(payload.modified_unix_seconds)
            ),
        ),
        formatter.paint(
            TextStyle::Muted,
            format!(
                "created_unix_seconds: {}",
                optional_number(payload.created_unix_seconds)
            ),
        ),
    ]
    .join("\n")
}

fn render_tree_text(entries: &[TreeEntry], formatter: TextFormatter) -> String {
    entries
        .iter()
        .map(|entry| {
            let mut label = entry.name.clone();
            if entry.kind == "directory" {
                label.push('/');
            }
            let label = formatter.paint(file_kind_style(entry.kind), label);
            if entry.depth == 0 {
                label
            } else {
                format!("{}- {label}", "  ".repeat(entry.depth))
            }
        })
        .collect::<Vec<String>>()
        .join("\n")
}

fn file_kind_style(kind: &str) -> TextStyle {
    match kind {
        "directory" => TextStyle::Heading,
        "symlink" => TextStyle::Warning,
        "file" => TextStyle::Key,
        _ => TextStyle::Muted,
    }
}

fn optional_number(value: Option<u64>) -> String {
    value
        .map(|number| number.to_string())
        .unwrap_or_else(|| "null".to_owned())
}

#[cfg(test)]
mod tests {
    use super::{file_kind_style, render_stat_text, render_tree_text};
    use crate::{
        commands::file::domain::{FileStatOutput, TreeEntry},
        output::{TextFormatter, TextStyle},
    };

    #[test]
    fn stat_renderer_preserves_plain_contract() {
        let payload = FileStatOutput {
            command: "file.stat",
            path: "src/lib.rs".to_owned(),
            kind: "file",
            size_bytes: 42,
            readonly: false,
            modified_unix_seconds: Some(10),
            created_unix_seconds: None,
        };

        assert_eq!(
            render_stat_text(&payload, TextFormatter::with_color(false)),
            "path: src/lib.rs\n\
             kind: file\n\
             size_bytes: 42\n\
             readonly: false\n\
             modified_unix_seconds: 10\n\
             created_unix_seconds: null"
        );
    }

    #[test]
    fn stat_renderer_styles_path_kind_and_readonly_state() {
        let payload = FileStatOutput {
            command: "file.stat",
            path: "cache".to_owned(),
            kind: "directory",
            size_bytes: 0,
            readonly: true,
            modified_unix_seconds: None,
            created_unix_seconds: None,
        };

        let rendered = render_stat_text(&payload, TextFormatter::with_color(true));

        assert!(rendered.contains("\u{1b}[36mcache\u{1b}[0m"));
        assert!(rendered.contains("\u{1b}[1;36mdirectory\u{1b}[0m"));
        assert!(rendered.contains("\u{1b}[33mtrue\u{1b}[0m"));
    }

    #[test]
    fn tree_renderer_preserves_plain_contract() {
        let entries = tree_entries();

        assert_eq!(
            render_tree_text(&entries, TextFormatter::with_color(false)),
            "root/\n  - link\n  - main.rs"
        );
    }

    #[test]
    fn tree_renderer_styles_node_kinds_without_styling_structure() {
        let rendered = render_tree_text(&tree_entries(), TextFormatter::with_color(true));

        assert!(rendered.starts_with("\u{1b}[1;36mroot/\u{1b}[0m\n  - "));
        assert!(rendered.contains("\u{1b}[33mlink\u{1b}[0m"));
        assert!(rendered.contains("\u{1b}[36mmain.rs\u{1b}[0m"));
    }

    #[test]
    fn file_kind_style_maps_semantic_kinds() {
        assert_eq!(file_kind_style("directory"), TextStyle::Heading);
        assert_eq!(file_kind_style("symlink"), TextStyle::Warning);
        assert_eq!(file_kind_style("file"), TextStyle::Key);
        assert_eq!(file_kind_style("other"), TextStyle::Muted);
    }

    fn tree_entries() -> Vec<TreeEntry> {
        vec![
            TreeEntry {
                depth: 0,
                kind: "directory",
                name: "root".to_owned(),
                path: "root".to_owned(),
            },
            TreeEntry {
                depth: 1,
                kind: "symlink",
                name: "link".to_owned(),
                path: "root/link".to_owned(),
            },
            TreeEntry {
                depth: 1,
                kind: "file",
                name: "main.rs".to_owned(),
                path: "root/main.rs".to_owned(),
            },
        ]
    }
}
