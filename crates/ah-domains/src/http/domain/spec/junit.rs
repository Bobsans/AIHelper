//! The JUnit XML an assertion run can emit for CI to consume.

use super::*;

pub(crate) fn render_assert_junit(report: &HttpAssertOutput) -> String {
    let mut xml = String::new();
    xml.push_str(r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    xml.push('\n');
    xml.push_str(&format!(
        r#"<testsuite name="http.assert" tests="{}" failures="{}" time="{}">"#,
        report.summary.total,
        report.summary.failed,
        duration_secs_string(report.summary.duration_ms)
    ));
    xml.push('\n');
    for case in &report.cases {
        xml.push_str(&format!(
            r#"  <testcase name="{}" classname="http.assert" time="{}">"#,
            xml_escape(&case.name),
            duration_secs_string(case.duration_ms)
        ));
        xml.push('\n');
        if !case.passed {
            let message = case.failures.join("; ");
            xml.push_str(&format!(
                r#"    <failure message="{}">{}</failure>"#,
                xml_escape(&message),
                xml_escape(&message)
            ));
            xml.push('\n');
        }
        xml.push_str("  </testcase>\n");
    }
    xml.push_str("</testsuite>");
    xml
}

pub(super) fn xml_escape(raw: &str) -> String {
    raw.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub(super) fn duration_secs_string(duration_ms: u64) -> String {
    format!("{:.3}", (duration_ms as f64) / 1000.0)
}
