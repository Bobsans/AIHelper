use crate::commands::search::domain::{SearchFilesOutput, SearchResult, SearchTextOutput};
use crate::{cli::GlobalOptions, error::AppError, output::OutputMode};

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
            let rendered = render_text_matches(&payload.matches, payload.context);
            if !rendered.is_empty() {
                println!("{rendered}");
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

fn emit_files(payload: SearchFilesOutput, options: &GlobalOptions) -> Result<(), AppError> {
    match options.output {
        OutputMode::Text => {
            if !payload.files.is_empty() {
                println!("{}", payload.files.join("\n"));
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

fn render_text_matches(
    matches: &[crate::commands::search::domain::TextMatch],
    context_lines: usize,
) -> String {
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
