use crate::{
    cli::GlobalOptions,
    error::AppError,
    output::{OutputMode, TextFormatter, TextStyle, emit_warning},
};
use serde_json;

use super::super::domain::{
    DetectedFile, ProjectCommandsOutput, ProjectDetectOutput, ProjectVersionEntry,
    ProjectVersionOutput, SuggestedCommand,
};

pub(crate) fn emit_detect(
    payload: ProjectDetectOutput,
    options: &GlobalOptions,
) -> Result<(), AppError> {
    if options.quiet {
        return Ok(());
    }

    match options.output {
        OutputMode::Text => {
            let formatter = TextFormatter::stdout();
            println!(
                "{}",
                render_assignment("root", &payload.root, TextStyle::Key, formatter)
            );
            println!(
                "{}",
                render_list_assignment("ecosystems", &payload.ecosystems, formatter)
            );
            println!(
                "{}",
                render_list_assignment("tools", &payload.tools, formatter)
            );
            println!(
                "{}",
                render_list_assignment("roles", &payload.roles, formatter)
            );
            print_files("package", &payload.files.packages, formatter);
            print_files("lock", &payload.files.locks, formatter);
            print_files("ci", &payload.files.ci, formatter);
            print_files("docs", &payload.files.docs, formatter);
            print_files("changelog", &payload.files.changelogs, formatter);
            print_files("deploy", &payload.files.deploy, formatter);
            print_files("infra", &payload.files.infra, formatter);
            print_files("config", &payload.files.config, formatter);
            print_files("quality", &payload.files.quality, formatter);
            print_files("security", &payload.files.security, formatter);
        }
        OutputMode::Json => println!("{}", serde_json::to_string_pretty(&payload)?),
    }

    Ok(())
}

pub(crate) fn emit_commands(
    payload: ProjectCommandsOutput,
    options: &GlobalOptions,
) -> Result<(), AppError> {
    if options.quiet {
        return Ok(());
    }

    match options.output {
        OutputMode::Text => {
            let formatter = TextFormatter::stdout();
            for item in &payload.commands {
                println!("{}", render_command(item, formatter));
            }
        }
        OutputMode::Json => println!("{}", serde_json::to_string_pretty(&payload)?),
    }

    Ok(())
}

pub(crate) fn emit_version(
    payload: ProjectVersionOutput,
    options: &GlobalOptions,
) -> Result<(), AppError> {
    if options.quiet {
        return Ok(());
    }

    match options.output {
        OutputMode::Text => {
            if payload.versions.is_empty() {
                println!(
                    "{}",
                    TextFormatter::stdout().paint(TextStyle::Muted, "no project versions found")
                );
                return Ok(());
            }
            let formatter = TextFormatter::stdout();
            for item in &payload.versions {
                println!("{}", render_version(item, formatter));
            }
            if payload.truncated {
                emit_warning("output truncated by --limit");
            }
        }
        OutputMode::Json => println!("{}", serde_json::to_string_pretty(&payload)?),
    }

    Ok(())
}

fn print_files(label: &str, files: &[DetectedFile], formatter: TextFormatter) {
    for file in files {
        println!("{}", render_file(label, file, formatter));
    }
}

fn render_assignment(
    label: &str,
    value: &str,
    value_style: TextStyle,
    formatter: TextFormatter,
) -> String {
    format!(
        "{}{}",
        formatter.paint(TextStyle::Muted, format!("{label}=")),
        formatter.paint(value_style, value)
    )
}

fn render_list_assignment(label: &str, values: &[String], formatter: TextFormatter) -> String {
    if values.is_empty() {
        render_assignment(label, "-", TextStyle::Muted, formatter)
    } else {
        render_assignment(label, &values.join(","), TextStyle::Key, formatter)
    }
}

fn render_file(label: &str, file: &DetectedFile, formatter: TextFormatter) -> String {
    format!(
        "{}:{} {}",
        formatter.paint(TextStyle::Heading, label),
        formatter.paint(TextStyle::Key, &file.kind),
        formatter.paint(TextStyle::Key, &file.path)
    )
}

fn render_command(command: &SuggestedCommand, formatter: TextFormatter) -> String {
    format!(
        "{}: {}",
        formatter.paint(TextStyle::Heading, &command.kind),
        formatter.paint(TextStyle::Key, command.command.join(" "))
    )
}

fn render_version(version: &ProjectVersionEntry, formatter: TextFormatter) -> String {
    let name = version.name.as_deref().unwrap_or("-");
    let version_value = version.version.as_deref().unwrap_or("-");
    let name_style = if version.name.is_some() {
        TextStyle::Key
    } else {
        TextStyle::Muted
    };
    let version_style = if version.version.is_some() {
        TextStyle::Success
    } else {
        TextStyle::Muted
    };

    format!(
        "{} {} {}{} {}{} {}",
        formatter.paint(TextStyle::Key, &version.kind),
        formatter.paint(TextStyle::Key, &version.path),
        formatter.paint(TextStyle::Muted, "name="),
        formatter.paint(name_style, name),
        formatter.paint(TextStyle::Muted, "version="),
        formatter.paint(version_style, version_value),
        formatter.paint(
            confidence_style(&version.confidence),
            format!("confidence={}", version.confidence)
        )
    )
}

fn confidence_style(confidence: &str) -> TextStyle {
    match confidence {
        "high" => TextStyle::Success,
        "medium" => TextStyle::Warning,
        "low" => TextStyle::Error,
        _ => TextStyle::Muted,
    }
}

#[cfg(test)]
mod tests {
    use super::{confidence_style, render_command, render_file, render_version};
    use crate::{
        commands::project::domain::{DetectedFile, ProjectVersionEntry, SuggestedCommand},
        output::{TextFormatter, TextStyle},
    };

    #[test]
    fn confidence_styles_are_semantic() {
        assert_eq!(confidence_style("high"), TextStyle::Success);
        assert_eq!(confidence_style("medium"), TextStyle::Warning);
        assert_eq!(confidence_style("low"), TextStyle::Error);
        assert_eq!(confidence_style("unknown"), TextStyle::Muted);
    }

    #[test]
    fn project_renderers_preserve_plain_contract() {
        let formatter = TextFormatter::with_color(false);
        let file = DetectedFile {
            kind: "cargo".to_owned(),
            path: "Cargo.toml".to_owned(),
        };
        let command = SuggestedCommand {
            kind: "test".to_owned(),
            command: vec!["cargo".to_owned(), "test".to_owned()],
            confidence: "high".to_owned(),
            reason: "Rust project".to_owned(),
        };
        let version = ProjectVersionEntry {
            kind: "cargo".to_owned(),
            path: "Cargo.toml".to_owned(),
            name: Some("demo".to_owned()),
            version: Some("1.2.3".to_owned()),
            confidence: "high".to_owned(),
        };

        assert_eq!(
            render_file("package", &file, formatter),
            "package:cargo Cargo.toml"
        );
        assert_eq!(render_command(&command, formatter), "test: cargo test");
        assert_eq!(
            render_version(&version, formatter),
            "cargo Cargo.toml name=demo version=1.2.3 confidence=high"
        );
    }

    #[test]
    fn project_renderers_apply_styles() {
        let version = ProjectVersionEntry {
            kind: "pub".to_owned(),
            path: "pubspec.yaml".to_owned(),
            name: None,
            version: Some("1.0.0".to_owned()),
            confidence: "medium".to_owned(),
        };
        let rendered = render_version(&version, TextFormatter::with_color(true));

        assert!(rendered.contains("\u{1b}[32m1.0.0\u{1b}[0m"));
        assert!(rendered.contains("\u{1b}[33mconfidence=medium\u{1b}[0m"));
        assert!(rendered.contains("\u{1b}[2m-\u{1b}[0m"));
    }
}
