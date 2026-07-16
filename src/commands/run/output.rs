use crate::{
    cli::GlobalOptions,
    commands::run::domain::RunCheckOutput,
    error::AppError,
    output::{OutputMode, TextFormatter, TextStyle, emit_warning},
};

pub(crate) fn emit_check_result(
    result: RunCheckOutput,
    options: &GlobalOptions,
) -> Result<(), AppError> {
    if options.quiet {
        return Ok(());
    }

    match options.output {
        OutputMode::Text => {
            let stdout_formatter = TextFormatter::stdout();
            let stderr_formatter = TextFormatter::stderr();
            println!("{}", render_status_line(&result, stdout_formatter));
            if !result.stdout.is_empty() {
                println!(
                    "{}\n{}",
                    stdout_formatter.paint(TextStyle::Key, "stdout:"),
                    result.stdout
                );
            }
            if !result.stderr.is_empty() {
                eprintln!(
                    "{}\n{}",
                    stderr_formatter.paint(TextStyle::Error, "stderr:"),
                    result.stderr
                );
            }
            if result.stdout_truncated {
                emit_warning("stdout truncated");
            }
            if result.stderr_truncated {
                emit_warning("stderr truncated");
            }
        }
        OutputMode::Json => {
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
    }

    Ok(())
}

fn render_status_line(result: &RunCheckOutput, formatter: TextFormatter) -> String {
    let success_style = if result.success {
        TextStyle::Success
    } else {
        TextStyle::Error
    };
    let timeout_style = if result.timed_out {
        TextStyle::Warning
    } else {
        TextStyle::Muted
    };
    let exit_code = result
        .exit_code
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_owned());

    format!(
        "{} {} {} {}",
        formatter.paint(success_style, format!("success={}", result.success)),
        formatter.paint(TextStyle::Muted, format!("exit_code={exit_code}")),
        formatter.paint(timeout_style, format!("timed_out={}", result.timed_out)),
        formatter.paint(
            TextStyle::Muted,
            format!("duration_ms={}", result.duration_ms)
        )
    )
}

#[cfg(test)]
mod tests {
    use super::render_status_line;
    use crate::{commands::run::domain::RunCheckOutput, output::TextFormatter};

    #[test]
    fn status_renderer_preserves_plain_contract() {
        let result = result(true, false, Some(0), 42);

        assert_eq!(
            render_status_line(&result, TextFormatter::with_color(false)),
            "success=true exit_code=0 timed_out=false duration_ms=42"
        );
    }

    #[test]
    fn status_renderer_applies_semantic_styles() {
        let result = result(false, true, None, 42);
        let rendered = render_status_line(&result, TextFormatter::with_color(true));

        assert!(rendered.contains("\u{1b}[1;31msuccess=false\u{1b}[0m"));
        assert!(rendered.contains("\u{1b}[33mtimed_out=true\u{1b}[0m"));
        assert!(rendered.contains("\u{1b}[2mexit_code=-\u{1b}[0m"));
    }

    fn result(
        success: bool,
        timed_out: bool,
        exit_code: Option<i32>,
        duration_ms: u128,
    ) -> RunCheckOutput {
        RunCheckOutput {
            command: "run.check",
            argv: Vec::new(),
            success,
            timed_out,
            exit_code,
            duration_ms,
            stdout: String::new(),
            stderr: String::new(),
            stdout_truncated: false,
            stderr_truncated: false,
        }
    }
}
