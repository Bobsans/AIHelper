use ah_error::AppError;
use ah_output::{Emitter, TextFormatter, TextStyle};

use crate::file::domain::{
    FileKind, FileLinesOutput, FileResult, FileStatOutput, FileTreeOutput, TreeEntry,
};

const TRUNCATED: &str = "output truncated by --limit";

pub(crate) fn emit(result: FileResult, emitter: &mut Emitter) -> Result<(), AppError> {
    match result {
        FileResult::Read(payload) => emit_lines(payload, emitter),
        FileResult::Head(payload) => emit_lines(payload, emitter),
        FileResult::Tail(payload) => emit_lines(payload, emitter),
        FileResult::Stat(payload) => emit_stat(payload, emitter),
        FileResult::Tree(payload) => emit_tree(payload, emitter),
    }
}

fn emit_lines(payload: FileLinesOutput, emitter: &mut Emitter) -> Result<(), AppError> {
    emitter.value(&payload, |_| payload.content.clone())?;
    if payload.truncated {
        emitter.text_warning(TRUNCATED);
    }
    Ok(())
}

fn emit_stat(payload: FileStatOutput, emitter: &mut Emitter) -> Result<(), AppError> {
    emitter.value(&payload, |formatter| render_stat_text(&payload, formatter))
}

fn emit_tree(payload: FileTreeOutput, emitter: &mut Emitter) -> Result<(), AppError> {
    emitter.value(&payload, |formatter| {
        render_tree_text(&payload.entries, formatter)
    })?;
    if payload.truncated {
        emitter.text_warning(TRUNCATED);
    }
    Ok(())
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
            formatter.paint(file_kind_style(payload.kind), payload.kind.as_str())
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
            if entry.kind == FileKind::Directory {
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

fn file_kind_style(kind: FileKind) -> TextStyle {
    match kind {
        FileKind::Directory => TextStyle::Heading,
        FileKind::Symlink => TextStyle::Warning,
        FileKind::File => TextStyle::Key,
        FileKind::Other => TextStyle::Muted,
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
    use ah_output::{TextFormatter, TextStyle};

    use crate::file::domain::{FileKind, FileStatOutput, TreeEntry};

    #[test]
    fn stat_renderer_preserves_plain_contract() {
        let payload = FileStatOutput {
            command: "file.stat",
            path: "src/lib.rs".to_owned(),
            kind: FileKind::File,
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
            kind: FileKind::Directory,
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
        assert_eq!(file_kind_style(FileKind::Directory), TextStyle::Heading);
        assert_eq!(file_kind_style(FileKind::Symlink), TextStyle::Warning);
        assert_eq!(file_kind_style(FileKind::File), TextStyle::Key);
        assert_eq!(file_kind_style(FileKind::Other), TextStyle::Muted);
    }

    fn tree_entries() -> Vec<TreeEntry> {
        vec![
            TreeEntry {
                depth: 0,
                kind: FileKind::Directory,
                name: "root".to_owned(),
                path: "root".to_owned(),
            },
            TreeEntry {
                depth: 1,
                kind: FileKind::Symlink,
                name: "link".to_owned(),
                path: "root/link".to_owned(),
            },
            TreeEntry {
                depth: 1,
                kind: FileKind::File,
                name: "main.rs".to_owned(),
                path: "root/main.rs".to_owned(),
            },
        ]
    }
}
