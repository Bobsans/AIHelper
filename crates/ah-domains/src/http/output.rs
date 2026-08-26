use ah_runtime::core::truncate_lines;

use ah_error::AppError;
use ah_output::{Emitter, TextFormatter, TextStyle};

use super::domain::{AssertReportFormat, HttpAssertOutput, HttpRequestOutput, render_assert_junit};

pub(crate) fn emit_request(
    payload: HttpRequestOutput,
    limit: Option<usize>,
    emitter: &mut Emitter,
) -> Result<(), AppError> {
    let (body_rendered, line_truncated) = truncate_lines(&payload.body, limit);
    let status = payload.status;
    let status_text = payload.status_text.clone();
    let body_truncated = payload.body_truncated;

    let mut json = payload;
    json.body = body_rendered.clone();
    json.truncated |= line_truncated;

    emitter.value(&json, |formatter| {
        if body_rendered.trim().is_empty() {
            render_http_status_line(status, &status_text, formatter)
        } else {
            body_rendered.clone()
        }
    })?;
    if body_truncated {
        emitter.text_warning("response body truncated by --max-response-bytes");
    }
    if line_truncated {
        emitter.text_warning("output truncated by --limit");
    }
    Ok(())
}

/// The assert report has its own format flag, which decides the rendering
/// instead of the global output mode.
pub(crate) fn emit_assert(
    output: &HttpAssertOutput,
    report: AssertReportFormat,
    emitter: &mut Emitter,
) -> Result<(), AppError> {
    match report {
        AssertReportFormat::Text => {
            emitter.report(|formatter| Ok(render_assert_text(output, formatter)))
        }
        AssertReportFormat::Json => emitter.report(|_| Ok(serde_json::to_string_pretty(&output)?)),
        AssertReportFormat::Junit => emitter.report(|_| Ok(render_assert_junit(output))),
    }
}

fn render_http_status_line(status: u16, status_text: &str, formatter: TextFormatter) -> String {
    formatter.paint(
        http_status_style(status),
        format!("HTTP {status} {status_text}"),
    )
}

fn http_status_style(status: u16) -> TextStyle {
    match status {
        200..=299 => TextStyle::Success,
        300..=399 => TextStyle::Key,
        400..=499 => TextStyle::Warning,
        500..=599 => TextStyle::Error,
        _ => TextStyle::Muted,
    }
}

fn render_assert_text(report: &HttpAssertOutput, formatter: TextFormatter) -> String {
    let mut lines = Vec::new();
    lines.push(format!(
        "{} {}",
        formatter.paint(TextStyle::Key, "spec:"),
        formatter.paint(TextStyle::Key, &report.spec_path)
    ));

    for case in &report.cases {
        let (label, style) = if case.passed {
            ("PASS", TextStyle::Success)
        } else {
            ("FAIL", TextStyle::Error)
        };
        lines.push(format!(
            "{} {}",
            formatter.paint(style, label),
            formatter.paint(TextStyle::Key, &case.name)
        ));
        for failure in &case.failures {
            lines.push(format!(
                "  {} {}",
                formatter.paint(TextStyle::Error, "-"),
                failure
            ));
        }
    }

    let failed_style = if report.summary.failed == 0 {
        TextStyle::Muted
    } else {
        TextStyle::Error
    };
    lines.push(format!(
        "{} {}, {}, {}, {}",
        formatter.paint(TextStyle::Key, "summary:"),
        formatter.paint(TextStyle::Muted, format!("total={}", report.summary.total)),
        formatter.paint(
            TextStyle::Success,
            format!("passed={}", report.summary.passed)
        ),
        formatter.paint(failed_style, format!("failed={}", report.summary.failed)),
        formatter.paint(
            TextStyle::Muted,
            format!("duration_ms={}", report.summary.duration_ms)
        )
    ));

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::{render_assert_text, render_http_status_line};
    use ah_output::TextFormatter;

    use crate::http::domain::{HttpAssertCaseOutput, HttpAssertOutput, HttpAssertSummary};

    #[test]
    fn http_status_renderer_uses_status_class_styles() {
        assert_eq!(
            render_http_status_line(204, "No Content", TextFormatter::with_color(true)),
            "\u{1b}[32mHTTP 204 No Content\u{1b}[0m"
        );
        assert_eq!(
            render_http_status_line(404, "Not Found", TextFormatter::with_color(true)),
            "\u{1b}[33mHTTP 404 Not Found\u{1b}[0m"
        );
        assert_eq!(
            render_http_status_line(503, "Unavailable", TextFormatter::with_color(true)),
            "\u{1b}[1;31mHTTP 503 Unavailable\u{1b}[0m"
        );
    }

    #[test]
    fn assert_renderer_preserves_plain_contract() {
        let report = report();

        assert_eq!(
            render_assert_text(&report, TextFormatter::with_color(false)),
            "spec: api.yaml\nPASS health\nFAIL create\n  - expected 201\nsummary: total=2, passed=1, failed=1, duration_ms=12"
        );
    }

    #[test]
    fn assert_renderer_applies_semantic_styles() {
        let rendered = render_assert_text(&report(), TextFormatter::with_color(true));

        assert!(rendered.contains("\u{1b}[32mPASS\u{1b}[0m"));
        assert!(rendered.contains("\u{1b}[1;31mFAIL\u{1b}[0m"));
        assert!(rendered.contains("\u{1b}[32mpassed=1\u{1b}[0m"));
        assert!(rendered.contains("\u{1b}[1;31mfailed=1\u{1b}[0m"));
    }

    fn report() -> HttpAssertOutput {
        HttpAssertOutput {
            command: "http.assert",
            spec_path: "api.yaml".to_owned(),
            fail_fast: false,
            summary: HttpAssertSummary {
                total: 2,
                passed: 1,
                failed: 1,
                duration_ms: 12,
            },
            cases: vec![
                HttpAssertCaseOutput {
                    name: "health".to_owned(),
                    passed: true,
                    status: Some(200),
                    duration_ms: 5,
                    failures: Vec::new(),
                },
                HttpAssertCaseOutput {
                    name: "create".to_owned(),
                    passed: false,
                    status: Some(200),
                    duration_ms: 7,
                    failures: vec!["expected 201".to_owned()],
                },
            ],
        }
    }
}
