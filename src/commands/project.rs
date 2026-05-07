use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use clap::{Args, Subcommand};
use regex::Regex;
use serde::Serialize;
use serde_json::Value;
use walkdir::WalkDir;

use crate::{cli::GlobalOptions, error::AppError, output::OutputMode};

#[derive(Debug, Args)]
pub struct ProjectArgs {
    #[command(subcommand)]
    pub command: ProjectCommand,
}

#[derive(Debug, Subcommand)]
pub enum ProjectCommand {
    #[command(about = "Detect project ecosystems and important files")]
    Detect(ProjectPathArgs),
    #[command(about = "Suggest common project commands")]
    Commands(ProjectPathArgs),
    #[command(about = "Detect project version from common manifest files")]
    Version(ProjectPathArgs),
}

#[derive(Debug, Args)]
pub struct ProjectPathArgs {
    #[arg(default_value = ".")]
    pub path: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
struct DetectedFile {
    kind: String,
    path: String,
}

#[derive(Debug, Clone, Serialize)]
struct SuggestedCommand {
    kind: String,
    command: Vec<String>,
    confidence: String,
    reason: String,
}

#[derive(Debug, Serialize)]
struct ProjectDetectOutput {
    command: &'static str,
    root: String,
    ecosystems: Vec<String>,
    package_files: Vec<DetectedFile>,
    ci_files: Vec<DetectedFile>,
    docs_files: Vec<DetectedFile>,
    changelog_files: Vec<DetectedFile>,
}

#[derive(Debug, Serialize)]
struct ProjectCommandsOutput {
    command: &'static str,
    root: String,
    ecosystems: Vec<String>,
    commands: Vec<SuggestedCommand>,
}

#[derive(Debug, Clone, Serialize)]
struct ProjectVersionEntry {
    kind: String,
    path: String,
    name: Option<String>,
    version: Option<String>,
    confidence: String,
}

#[derive(Debug, Serialize)]
struct ProjectVersionOutput {
    command: &'static str,
    root: String,
    version_count: usize,
    truncated: bool,
    versions: Vec<ProjectVersionEntry>,
}

pub fn execute(args: ProjectArgs, options: &GlobalOptions) -> Result<(), AppError> {
    match args.command {
        ProjectCommand::Detect(path_args) => execute_detect(path_args, options),
        ProjectCommand::Commands(path_args) => execute_commands(path_args, options),
        ProjectCommand::Version(path_args) => execute_version(path_args, options),
    }
}

fn execute_detect(args: ProjectPathArgs, options: &GlobalOptions) -> Result<(), AppError> {
    let snapshot = detect_project(&args.path)?;

    if options.quiet {
        return Ok(());
    }

    match options.output {
        OutputMode::Text => {
            println!("root={}", snapshot.root);
            println!(
                "ecosystems={}",
                if snapshot.ecosystems.is_empty() {
                    "-".to_owned()
                } else {
                    snapshot.ecosystems.join(",")
                }
            );
            print_files("package", &snapshot.package_files);
            print_files("ci", &snapshot.ci_files);
            print_files("docs", &snapshot.docs_files);
            print_files("changelog", &snapshot.changelog_files);
        }
        OutputMode::Json => println!("{}", serde_json::to_string_pretty(&snapshot)?),
    }

    Ok(())
}

fn execute_commands(args: ProjectPathArgs, options: &GlobalOptions) -> Result<(), AppError> {
    let snapshot = detect_project(&args.path)?;
    let commands = suggest_commands(&snapshot);
    let output = ProjectCommandsOutput {
        command: "project.commands",
        root: snapshot.root,
        ecosystems: snapshot.ecosystems,
        commands,
    };

    if options.quiet {
        return Ok(());
    }

    match options.output {
        OutputMode::Text => {
            for item in &output.commands {
                println!("{}: {}", item.kind, item.command.join(" "));
            }
        }
        OutputMode::Json => println!("{}", serde_json::to_string_pretty(&output)?),
    }

    Ok(())
}

fn execute_version(args: ProjectPathArgs, options: &GlobalOptions) -> Result<(), AppError> {
    let root = canonical_project_root(&args.path)?;
    let mut versions = detect_versions(&root)?;
    let truncated = if let Some(limit) = options.limit {
        if versions.len() > limit {
            versions.truncate(limit);
            true
        } else {
            false
        }
    } else {
        false
    };
    let output = ProjectVersionOutput {
        command: "project.version",
        root: normalize_path(&root),
        version_count: versions.len(),
        truncated,
        versions,
    };

    if options.quiet {
        return Ok(());
    }

    match options.output {
        OutputMode::Text => {
            if output.versions.is_empty() {
                println!("no project versions found");
                return Ok(());
            }
            for item in &output.versions {
                println!(
                    "{} {} name={} version={} confidence={}",
                    item.kind,
                    item.path,
                    item.name.as_deref().unwrap_or("-"),
                    item.version.as_deref().unwrap_or("-"),
                    item.confidence
                );
            }
            if truncated {
                eprintln!("warning: output truncated by --limit");
            }
        }
        OutputMode::Json => println!("{}", serde_json::to_string_pretty(&output)?),
    }

    Ok(())
}

fn detect_project(path: &Path) -> Result<ProjectDetectOutput, AppError> {
    let root = canonical_project_root(path)?;

    let mut ecosystems = BTreeSet::new();
    let mut package_files = Vec::new();
    let mut ci_files = Vec::new();
    let mut docs_files = Vec::new();
    let mut changelog_files = Vec::new();

    let candidates = collect_project_files(&root);
    for file in candidates {
        let rel = normalize_relative(&root, &file);
        let Some(name) = file.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        let lower_name = name.to_ascii_lowercase();
        match lower_name.as_str() {
            "cargo.toml" => {
                ecosystems.insert("rust".to_owned());
                package_files.push(detected("cargo", &rel));
            }
            "package.json" => {
                ecosystems.insert("node".to_owned());
                package_files.push(detected("npm", &rel));
            }
            "pyproject.toml" => {
                ecosystems.insert("python".to_owned());
                package_files.push(detected("python", &rel));
            }
            "go.mod" => {
                ecosystems.insert("go".to_owned());
                package_files.push(detected("go", &rel));
            }
            "pom.xml" => {
                ecosystems.insert("java-maven".to_owned());
                package_files.push(detected("maven", &rel));
            }
            "build.gradle" | "build.gradle.kts" => {
                ecosystems.insert("java-gradle".to_owned());
                package_files.push(detected("gradle", &rel));
            }
            "readme.md" | "readme" => docs_files.push(detected("readme", &rel)),
            "changelog.md" | "changes.md" | "history.md" => {
                changelog_files.push(detected("changelog", &rel))
            }
            _ if lower_name.ends_with(".csproj") => {
                ecosystems.insert("dotnet".to_owned());
                package_files.push(detected("dotnet", &rel));
            }
            _ => {}
        }
        if rel.starts_with(".github/workflows/") {
            ci_files.push(detected("github-actions", &rel));
        } else if lower_name == ".gitlab-ci.yml" {
            ci_files.push(detected("gitlab-ci", &rel));
        } else if lower_name == "azure-pipelines.yml" || lower_name == "azure-pipelines.yaml" {
            ci_files.push(detected("azure-pipelines", &rel));
        }
    }

    Ok(ProjectDetectOutput {
        command: "project.detect",
        root: normalize_path(&root),
        ecosystems: ecosystems.into_iter().collect(),
        package_files,
        ci_files,
        docs_files,
        changelog_files,
    })
}

fn canonical_project_root(path: &Path) -> Result<PathBuf, AppError> {
    let root = if path.exists() {
        path.canonicalize()
            .map_err(|source| AppError::file_metadata(path.to_path_buf(), source))?
    } else {
        return Err(AppError::invalid_argument(format!(
            "path does not exist: {}",
            path.display()
        )));
    };
    if !root.is_dir() {
        return Err(AppError::invalid_argument(format!(
            "path is not a directory: {}",
            path.display()
        )));
    }
    Ok(root)
}

fn collect_project_files(root: &Path) -> Vec<PathBuf> {
    WalkDir::new(root)
        .max_depth(4)
        .into_iter()
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            !matches!(
                name.as_ref(),
                "target" | "node_modules" | ".git" | ".venv" | "dist" | "build"
            )
        })
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.path().to_path_buf())
        .collect()
}

fn suggest_commands(snapshot: &ProjectDetectOutput) -> Vec<SuggestedCommand> {
    let ecosystems = snapshot
        .ecosystems
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let mut commands = Vec::new();
    if ecosystems.contains(&"rust") {
        commands.push(suggested(
            "format_check",
            &["cargo", "fmt", "--all", "--", "--check"],
            "high",
            "Cargo.toml detected",
        ));
        commands.push(suggested(
            "test",
            &["cargo", "test", "--workspace", "--all-targets", "--locked"],
            "high",
            "Cargo.toml detected",
        ));
        commands.push(suggested(
            "build",
            &["cargo", "build", "--locked"],
            "high",
            "Cargo.toml detected",
        ));
        commands.push(suggested(
            "release_build",
            &["cargo", "build", "--release", "--locked"],
            "high",
            "Cargo.toml detected",
        ));
    }
    if ecosystems.contains(&"node") {
        commands.push(suggested(
            "install",
            &["npm", "install"],
            "medium",
            "package.json detected",
        ));
        commands.push(suggested(
            "test",
            &["npm", "test"],
            "medium",
            "package.json detected",
        ));
        commands.push(suggested(
            "build",
            &["npm", "run", "build"],
            "medium",
            "package.json detected",
        ));
    }
    if ecosystems.contains(&"dotnet") {
        commands.push(suggested(
            "restore",
            &["dotnet", "restore"],
            "medium",
            ".csproj detected",
        ));
        commands.push(suggested(
            "test",
            &["dotnet", "test"],
            "medium",
            ".csproj detected",
        ));
        commands.push(suggested(
            "build",
            &["dotnet", "build"],
            "medium",
            ".csproj detected",
        ));
    }
    if ecosystems.contains(&"python") {
        commands.push(suggested(
            "test",
            &["pytest"],
            "low",
            "pyproject.toml detected",
        ));
    }
    if ecosystems.contains(&"go") {
        commands.push(suggested(
            "test",
            &["go", "test", "./..."],
            "high",
            "go.mod detected",
        ));
        commands.push(suggested(
            "build",
            &["go", "build", "./..."],
            "high",
            "go.mod detected",
        ));
    }
    if ecosystems.contains(&"java-maven") {
        commands.push(suggested(
            "test",
            &["mvn", "test"],
            "medium",
            "pom.xml detected",
        ));
        commands.push(suggested(
            "build",
            &["mvn", "package"],
            "medium",
            "pom.xml detected",
        ));
    }
    if ecosystems.contains(&"java-gradle") {
        commands.push(suggested(
            "test",
            &["gradle", "test"],
            "medium",
            "Gradle build file detected",
        ));
        commands.push(suggested(
            "build",
            &["gradle", "build"],
            "medium",
            "Gradle build file detected",
        ));
    }
    commands
}

fn detect_versions(root: &Path) -> Result<Vec<ProjectVersionEntry>, AppError> {
    let mut versions = Vec::new();
    for file in collect_project_files(root) {
        let Some(name) = file.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        let lower_name = name.to_ascii_lowercase();
        let rel = normalize_relative(root, &file);
        let parsed = match lower_name.as_str() {
            "cargo.toml" => parse_cargo_version(&file, &rel)?,
            "package.json" => parse_package_json_version(&file, &rel)?,
            "pyproject.toml" => parse_pyproject_version(&file, &rel)?,
            "pom.xml" => parse_xml_version(&file, &rel, "maven", "medium")?,
            "build.gradle" | "build.gradle.kts" => parse_gradle_version(&file, &rel)?,
            _ if lower_name.ends_with(".csproj") => {
                parse_xml_version(&file, &rel, "dotnet", "high")?
            }
            _ => None,
        };
        if let Some(entry) = parsed {
            versions.push(entry);
        }
    }
    Ok(versions)
}

fn parse_cargo_version(path: &Path, rel: &str) -> Result<Option<ProjectVersionEntry>, AppError> {
    let raw =
        fs::read_to_string(path).map_err(|source| AppError::file_read(path.into(), source))?;
    let values = parse_toml_section_values(&raw, "package", &["name", "version"]);
    Ok(values.get("version").map(|version| ProjectVersionEntry {
        kind: "cargo".to_owned(),
        path: rel.to_owned(),
        name: values.get("name").cloned(),
        version: Some(version.clone()),
        confidence: "high".to_owned(),
    }))
}

fn parse_pyproject_version(
    path: &Path,
    rel: &str,
) -> Result<Option<ProjectVersionEntry>, AppError> {
    let raw =
        fs::read_to_string(path).map_err(|source| AppError::file_read(path.into(), source))?;
    let values = parse_toml_section_values(&raw, "project", &["name", "version"]);
    Ok(values.get("version").map(|version| ProjectVersionEntry {
        kind: "python".to_owned(),
        path: rel.to_owned(),
        name: values.get("name").cloned(),
        version: Some(version.clone()),
        confidence: "high".to_owned(),
    }))
}

fn parse_package_json_version(
    path: &Path,
    rel: &str,
) -> Result<Option<ProjectVersionEntry>, AppError> {
    let raw =
        fs::read_to_string(path).map_err(|source| AppError::file_read(path.into(), source))?;
    let value = serde_json::from_str::<Value>(&raw)
        .map_err(|source| AppError::json_deserialization(path.into(), source))?;
    let version = value
        .get("version")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Ok(version.map(|version| ProjectVersionEntry {
        kind: "npm".to_owned(),
        path: rel.to_owned(),
        name: value.get("name").and_then(Value::as_str).map(str::to_owned),
        version: Some(version),
        confidence: "high".to_owned(),
    }))
}

fn parse_xml_version(
    path: &Path,
    rel: &str,
    kind: &str,
    confidence: &str,
) -> Result<Option<ProjectVersionEntry>, AppError> {
    let raw =
        fs::read_to_string(path).map_err(|source| AppError::file_read(path.into(), source))?;
    let version = capture_xml_tag(&raw, "Version").or_else(|| capture_xml_tag(&raw, "version"));
    let name =
        capture_xml_tag(&raw, "AssemblyName").or_else(|| capture_xml_tag(&raw, "artifactId"));
    Ok(version.map(|version| ProjectVersionEntry {
        kind: kind.to_owned(),
        path: rel.to_owned(),
        name,
        version: Some(version),
        confidence: confidence.to_owned(),
    }))
}

fn parse_gradle_version(path: &Path, rel: &str) -> Result<Option<ProjectVersionEntry>, AppError> {
    let raw =
        fs::read_to_string(path).map_err(|source| AppError::file_read(path.into(), source))?;
    let version_re = Regex::new(r#"(?m)^\s*version\s*(?:=|\s)\s*['"]([^'"]+)['"]"#)
        .map_err(|error| AppError::invalid_argument(format!("internal regex error: {error}")))?;
    let name_re = Regex::new(r#"(?m)^\s*rootProject\.name\s*=\s*['"]([^'"]+)['"]"#)
        .map_err(|error| AppError::invalid_argument(format!("internal regex error: {error}")))?;
    let version = version_re
        .captures(&raw)
        .and_then(|captures| captures.get(1))
        .map(|value| value.as_str().to_owned());
    Ok(version.map(|version| ProjectVersionEntry {
        kind: "gradle".to_owned(),
        path: rel.to_owned(),
        name: name_re
            .captures(&raw)
            .and_then(|captures| captures.get(1))
            .map(|value| value.as_str().to_owned()),
        version: Some(version),
        confidence: "medium".to_owned(),
    }))
}

fn parse_toml_section_values(
    raw: &str,
    section_name: &str,
    keys: &[&str],
) -> std::collections::BTreeMap<String, String> {
    let mut in_section = false;
    let mut values = std::collections::BTreeMap::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_section = trimmed.trim_matches(&['[', ']'][..]) == section_name;
            continue;
        }
        if !in_section || trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if !keys.contains(&key) {
            continue;
        }
        let value = value
            .split('#')
            .next()
            .unwrap_or("")
            .trim()
            .trim_matches(&['"', '\''][..])
            .to_owned();
        if !value.is_empty() {
            values.insert(key.to_owned(), value);
        }
    }
    values
}

fn capture_xml_tag(raw: &str, tag: &str) -> Option<String> {
    let pattern = format!(r"(?is)<{tag}>\s*([^<]+?)\s*</{tag}>");
    Regex::new(&pattern)
        .ok()?
        .captures(raw)
        .and_then(|captures| captures.get(1))
        .map(|value| value.as_str().trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn suggested(kind: &str, command: &[&str], confidence: &str, reason: &str) -> SuggestedCommand {
    SuggestedCommand {
        kind: kind.to_owned(),
        command: command.iter().map(|value| (*value).to_owned()).collect(),
        confidence: confidence.to_owned(),
        reason: reason.to_owned(),
    }
}

fn detected(kind: &str, path: &str) -> DetectedFile {
    DetectedFile {
        kind: kind.to_owned(),
        path: path.to_owned(),
    }
}

fn print_files(label: &str, files: &[DetectedFile]) {
    for file in files {
        println!("{label}:{} {}", file.kind, file.path);
    }
}

fn normalize_relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(normalize_path)
        .unwrap_or_else(|_| normalize_path(path))
}

fn normalize_path(path: &Path) -> String {
    let normalized = path.to_string_lossy().replace('\\', "/");
    if let Some(path) = normalized.strip_prefix("//?/UNC/") {
        format!("//{path}")
    } else if let Some(path) = normalized.strip_prefix("//?/") {
        path.to_owned()
    } else {
        normalized
    }
}
