use ah_runtime::core::truncate_lines;

use crate::{cli::GlobalOptions, error::AppError};

use super::super::domain::{
    AssertReportFormat, HttpAssertOutput, HttpRequestOutput, render_assert_junit,
    render_assert_text,
};

pub(crate) fn emit_request(
    payload: HttpRequestOutput,
    options: &GlobalOptions,
) -> Result<(), AppError> {
    if options.quiet {
        return Ok(());
    }

    let (body_rendered, line_truncated) = truncate_lines(&payload.body, options.limit);
    match options.output {
        crate::output::OutputMode::Text => {
            if !body_rendered.trim().is_empty() {
                println!("{body_rendered}");
            } else {
                println!("HTTP {} {}", payload.status, payload.status_text);
            }
            if payload.body_truncated {
                eprintln!("warning: response body truncated by --max-response-bytes");
            }
            if line_truncated {
                eprintln!("warning: output truncated by --limit");
            }
        }
        crate::output::OutputMode::Json => {
            let mut rendered = payload;
            rendered.body = body_rendered;
            rendered.truncated |= line_truncated;
            println!("{}", serde_json::to_string_pretty(&rendered)?);
        }
    }

    Ok(())
}

pub(crate) fn emit_assert(
    output: &HttpAssertOutput,
    report: AssertReportFormat,
    options: &GlobalOptions,
) -> Result<(), AppError> {
    if options.quiet {
        return Ok(());
    }

    match report {
        AssertReportFormat::Text => render_assert_text(output),
        AssertReportFormat::Json => println!("{}", serde_json::to_string_pretty(&output)?),
        AssertReportFormat::Junit => println!("{}", render_assert_junit(output)),
    }
    Ok(())
}
