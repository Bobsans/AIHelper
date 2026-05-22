use crate::{cli::GlobalOptions, error::AppError, output::OutputMode};
use serde_json;

use super::super::domain::{
    DetectedFile, ProjectCommandsOutput, ProjectDetectOutput, ProjectVersionOutput,
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
            println!("root={}", payload.root);
            println!(
                "ecosystems={}",
                if payload.ecosystems.is_empty() {
                    "-".to_owned()
                } else {
                    payload.ecosystems.join(",")
                }
            );
            println!(
                "tools={}",
                if payload.tools.is_empty() {
                    "-".to_owned()
                } else {
                    payload.tools.join(",")
                }
            );
            println!(
                "roles={}",
                if payload.roles.is_empty() {
                    "-".to_owned()
                } else {
                    payload.roles.join(",")
                }
            );
            print_files("package", &payload.files.packages);
            print_files("lock", &payload.files.locks);
            print_files("ci", &payload.files.ci);
            print_files("docs", &payload.files.docs);
            print_files("changelog", &payload.files.changelogs);
            print_files("deploy", &payload.files.deploy);
            print_files("infra", &payload.files.infra);
            print_files("config", &payload.files.config);
            print_files("quality", &payload.files.quality);
            print_files("security", &payload.files.security);
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
            for item in &payload.commands {
                println!("{}: {}", item.kind, item.command.join(" "));
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
                println!("no project versions found");
                return Ok(());
            }
            for item in &payload.versions {
                println!(
                    "{} {} name={} version={} confidence={}",
                    item.kind,
                    item.path,
                    item.name.as_deref().unwrap_or("-"),
                    item.version.as_deref().unwrap_or("-"),
                    item.confidence
                );
            }
            if payload.truncated {
                eprintln!("warning: output truncated by --limit");
            }
        }
        OutputMode::Json => println!("{}", serde_json::to_string_pretty(&payload)?),
    }

    Ok(())
}

fn print_files(label: &str, files: &[DetectedFile]) {
    for file in files {
        println!("{label}:{} {}", file.kind, file.path);
    }
}
