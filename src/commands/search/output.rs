use crate::commands::search::domain::{SearchFilesOutput, SearchResult, SearchTextOutput};
use crate::{
    cli::GlobalOptions,
    error::AppError,
    output::{OutputMode, TextFormatter, TextStyle, emit_warning},
};

pub(crate) fn emit(result: SearchResult, options: &GlobalOptions) -> Result<(), AppError> {
    if options.quiet {
        return Ok(());
    }

    match result {
        SearchResult::Text(payload) => emit_text(payload, options),
        SearchResult::Files(payload) => emit_files(payload, options),
    }
}

fn emit_text(payload: SearchTextOutput, options: &GlobalOptions) -> Result<(), AppError> {
    match options.output {
        OutputMode::Text => {
            let rendered =
                render_text_matches(&payload.matches, payload.context, TextFormatter::stdout());
            if !rendered.is_empty() {
                println!("{rendered}");
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

fn emit_files(payload: SearchFilesOutput, options: &GlobalOptions) -> Result<(), AppError> {
    match options.output {
        OutputMode::Text => {
            let rendered = render_file_matches(&payload.files, TextFormatter::stdout());
            if !rendered.is_empty() {
                println!("{rendered}");
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

fn render_text_matches(
    matches: &[crate::commands::search::domain::TextMatch],
    context_lines: usize,
    formatter: TextFormatter,
) -> String {
    let mut lines = Vec::new();
    for (index, item) in matches.iter().enumerate() {
        if context_lines > 0 && index > 0 {
            lines.push(formatter.paint(TextStyle::Muted, "--"));
        }
        for context in &item.context_before {
            lines.push(format!(
                "{}{}",
                formatter.paint(TextStyle::Muted, format!("{}-{}-", item.path, context.line)),
                context.text
            ));
        }
        lines.push(format!(
            "{}:{}:{}",
            formatter.paint(TextStyle::Key, &item.path),
            formatter.paint(TextStyle::Muted, item.line),
            item.text
        ));
        for context in &item.context_after {
            lines.push(format!(
                "{}{}",
                formatter.paint(TextStyle::Muted, format!("{}-{}-", item.path, context.line)),
                context.text
            ));
        }
    }

    lines.join("\n")
}

fn render_file_matches(files: &[String], formatter: TextFormatter) -> String {
    files
        .iter()
        .map(|path| formatter.paint(TextStyle::Key, path))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::{render_file_matches, render_text_matches};
    use crate::{
        commands::search::domain::{ContextLine, TextMatch},
        output::TextFormatter,
    };

    #[test]
    fn text_renderer_preserves_plain_contract() {
        assert_eq!(
            render_text_matches(&matches(), 1, TextFormatter::with_color(false)),
            "src/lib.rs-9-before\n\
             src/lib.rs:10:let value = target();\n\
             src/lib.rs-11-after\n\
             --\n\
             tests/app.rs:3:assert!(target());"
        );
    }

    #[test]
    fn text_renderer_styles_locations_but_not_source_text() {
        let rendered = render_text_matches(&matches(), 1, TextFormatter::with_color(true));

        assert!(rendered.contains("\u{1b}[2msrc/lib.rs-9-\u{1b}[0mbefore"));
        assert!(rendered.contains("\u{1b}[36msrc/lib.rs\u{1b}[0m"));
        assert!(rendered.contains("\u{1b}[2m10\u{1b}[0m:let value = target();"));
        assert!(rendered.contains("\u{1b}[2m--\u{1b}[0m"));
        assert!(!rendered.contains("\u{1b}[0mlet value"));
    }

    #[test]
    fn file_renderer_preserves_plain_contract_and_styles_paths() {
        let files = vec!["src/lib.rs".to_owned(), "src/main.rs".to_owned()];

        assert_eq!(
            render_file_matches(&files, TextFormatter::with_color(false)),
            "src/lib.rs\nsrc/main.rs"
        );
        assert_eq!(
            render_file_matches(&files, TextFormatter::with_color(true)),
            "\u{1b}[36msrc/lib.rs\u{1b}[0m\n\u{1b}[36msrc/main.rs\u{1b}[0m"
        );
    }

    fn matches() -> Vec<TextMatch> {
        vec![
            TextMatch {
                path: "src/lib.rs".to_owned(),
                line: 10,
                column: 13,
                text: "let value = target();".to_owned(),
                context_before: vec![ContextLine {
                    line: 9,
                    text: "before".to_owned(),
                }],
                context_after: vec![ContextLine {
                    line: 11,
                    text: "after".to_owned(),
                }],
            },
            TextMatch {
                path: "tests/app.rs".to_owned(),
                line: 3,
                column: 9,
                text: "assert!(target());".to_owned(),
                context_before: Vec::new(),
                context_after: Vec::new(),
            },
        ]
    }
}
