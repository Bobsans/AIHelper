use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use regex::Regex;
use serde::Serialize;
use serde_json::Value;

use crate::error::AppError;

use super::{
    ProjectPathArgs, adapters,
    rules::{FileGroup, classify_file},
};

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DetectedFile {
    pub(crate) kind: String,
    pub(crate) path: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SuggestedCommand {
    pub(crate) kind: String,
    pub(crate) command: Vec<String>,
    pub(crate) confidence: String,
    pub(crate) reason: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct ProjectFileGroups {
    pub(crate) packages: Vec<DetectedFile>,
    pub(crate) locks: Vec<DetectedFile>,
    pub(crate) ci: Vec<DetectedFile>,
    pub(crate) docs: Vec<DetectedFile>,
    pub(crate) changelogs: Vec<DetectedFile>,
    pub(crate) deploy: Vec<DetectedFile>,
    pub(crate) infra: Vec<DetectedFile>,
    pub(crate) config: Vec<DetectedFile>,
    pub(crate) quality: Vec<DetectedFile>,
    pub(crate) security: Vec<DetectedFile>,
}

#[derive(Debug, Clone)]
struct ProjectSnapshot {
    root: String,
    ecosystems: Vec<String>,
    tools: Vec<String>,
    roles: Vec<String>,
    files: ProjectFileGroups,
    versions: Vec<ProjectVersionEntry>,
    commands: Vec<SuggestedCommand>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ProjectVersionEntry {
    pub(crate) kind: String,
    pub(crate) path: String,
    pub(crate) name: Option<String>,
    pub(crate) version: Option<String>,
    pub(crate) confidence: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct ProjectDetectOutput {
    pub(crate) command: &'static str,
    pub(crate) root: String,
    pub(crate) ecosystems: Vec<String>,
    pub(crate) tools: Vec<String>,
    pub(crate) roles: Vec<String>,
    pub(crate) files: ProjectFileGroups,
    pub(crate) versions: Vec<ProjectVersionEntry>,
    pub(crate) commands: Vec<SuggestedCommand>,
    #[serde(rename = "package_files")]
    pub(crate) package_files: Vec<DetectedFile>,
    #[serde(rename = "ci_files")]
    pub(crate) ci_files: Vec<DetectedFile>,
    #[serde(rename = "docs_files")]
    pub(crate) docs_files: Vec<DetectedFile>,
    #[serde(rename = "changelog_files")]
    pub(crate) changelog_files: Vec<DetectedFile>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ProjectCommandsOutput {
    pub(crate) command: &'static str,
    pub(crate) root: String,
    pub(crate) ecosystems: Vec<String>,
    pub(crate) tools: Vec<String>,
    pub(crate) roles: Vec<String>,
    pub(crate) commands: Vec<SuggestedCommand>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ProjectVersionOutput {
    pub(crate) command: &'static str,
    pub(crate) root: String,
    pub(crate) version_count: usize,
    pub(crate) truncated: bool,
    pub(crate) versions: Vec<ProjectVersionEntry>,
}

pub(crate) fn run_detect(args: ProjectPathArgs) -> Result<ProjectDetectOutput, AppError> {
    let snapshot = detect_project(&args.path)?;

    Ok(ProjectDetectOutput {
        command: "project.detect",
        root: snapshot.root,
        ecosystems: snapshot.ecosystems,
        tools: snapshot.tools,
        roles: snapshot.roles,
        package_files: snapshot.files.packages.clone(),
        ci_files: snapshot.files.ci.clone(),
        docs_files: snapshot.files.docs.clone(),
        changelog_files: snapshot.files.changelogs.clone(),
        files: snapshot.files,
        versions: snapshot.versions,
        commands: snapshot.commands,
    })
}

pub(crate) fn run_commands(args: ProjectPathArgs) -> Result<ProjectCommandsOutput, AppError> {
    let snapshot = detect_project(&args.path)?;

    Ok(ProjectCommandsOutput {
        command: "project.commands",
        root: snapshot.root,
        ecosystems: snapshot.ecosystems,
        tools: snapshot.tools,
        roles: snapshot.roles,
        commands: snapshot.commands,
    })
}

pub(crate) fn run_version(
    args: ProjectPathArgs,
    limit: Option<usize>,
) -> Result<ProjectVersionOutput, AppError> {
    let snapshot = detect_project(&args.path)?;
    let mut versions = snapshot.versions;
    let truncated = if let Some(limit) = limit {
        if versions.len() > limit {
            versions.truncate(limit);
            true
        } else {
            false
        }
    } else {
        false
    };
    Ok(ProjectVersionOutput {
        command: "project.version",
        root: snapshot.root,
        version_count: versions.len(),
        truncated,
        versions,
    })
}

fn detect_project(path: &Path) -> Result<ProjectSnapshot, AppError> {
    let root = adapters::io::canonical_project_root(path)?;

    let mut ecosystems = BTreeSet::new();
    let mut tools = BTreeSet::new();
    let mut roles = BTreeSet::new();
    let mut files = ProjectFileGroups::default();

    let candidates = adapters::io::collect_project_files(&root);
    for file in &candidates {
        let rel = normalize_relative(&root, file);
        let Some(name) = file.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        for detection in classify_file(&rel, name) {
            if let Some(ecosystem) = detection.ecosystem {
                ecosystems.insert(ecosystem.to_owned());
            }
            if let Some(tool) = detection.tool {
                tools.insert(tool.to_owned());
            }
            if let Some(role) = detection.role {
                roles.insert(role.to_owned());
            }
            push_detected_file(&mut files, detection.group, detection.kind, &rel);
        }
    }

    enrich_project(&root, &mut ecosystems, &mut tools, &mut roles, &files)?;
    let ecosystems = ecosystems.into_iter().collect::<Vec<_>>();
    let tools = tools.into_iter().collect::<Vec<_>>();
    let roles = roles.into_iter().collect::<Vec<_>>();
    let versions = detect_versions(&root, &candidates)?;
    let commands = suggest_commands(&ecosystems, &tools, &files, &root)?;

    Ok(ProjectSnapshot {
        root: normalize_path(&root),
        ecosystems,
        tools,
        roles,
        files,
        versions,
        commands,
    })
}

fn push_detected_file(files: &mut ProjectFileGroups, group: FileGroup, kind: &str, path: &str) {
    let target = match group {
        FileGroup::Package => &mut files.packages,
        FileGroup::Lock => &mut files.locks,
        FileGroup::Ci => &mut files.ci,
        FileGroup::Docs => &mut files.docs,
        FileGroup::Changelog => &mut files.changelogs,
        FileGroup::Deploy => &mut files.deploy,
        FileGroup::Infra => &mut files.infra,
        FileGroup::Config => &mut files.config,
        FileGroup::Quality => &mut files.quality,
        FileGroup::Security => &mut files.security,
    };
    push_unique_file(target, detected(kind, path));
}

fn push_unique_file(target: &mut Vec<DetectedFile>, file: DetectedFile) {
    if !target
        .iter()
        .any(|existing| existing.kind == file.kind && existing.path == file.path)
    {
        target.push(file);
    }
}

fn enrich_project(
    root: &Path,
    ecosystems: &mut BTreeSet<String>,
    tools: &mut BTreeSet<String>,
    roles: &mut BTreeSet<String>,
    files: &ProjectFileGroups,
) -> Result<(), AppError> {
    for file in &files.packages {
        if file.kind == "pub" && pubspec_looks_like_flutter(root, &file.path)? {
            ecosystems.insert("flutter".to_owned());
            tools.insert("flutter".to_owned());
            roles.insert("app".to_owned());
        }
        if file.kind == "npm" {
            enrich_package_json_roles(root, &file.path, ecosystems, tools, roles)?;
        }
    }
    if files.packages.len() > 1 {
        roles.insert("monorepo".to_owned());
    }
    if !files.deploy.is_empty() {
        roles.insert("deploy".to_owned());
    }
    if !files.infra.is_empty() {
        roles.insert("infra".to_owned());
    }
    if !files.quality.is_empty() {
        roles.insert("quality".to_owned());
    }
    if !files.security.is_empty() {
        roles.insert("security".to_owned());
    }
    Ok(())
}

fn enrich_package_json_roles(
    root: &Path,
    rel: &str,
    ecosystems: &mut BTreeSet<String>,
    tools: &mut BTreeSet<String>,
    roles: &mut BTreeSet<String>,
) -> Result<(), AppError> {
    let path = root.join(rel);
    let raw = adapters::io::read_to_string(&path)?;
    let value = serde_json::from_str::<Value>(&raw)
        .map_err(|source| AppError::json_deserialization(path, source))?;
    for dep in package_json_dependency_names(&value) {
        match dep.as_str() {
            "next" | "vite" | "astro" | "react" | "vue" | "svelte" => {
                roles.insert("web".to_owned());
                tools.insert(dep);
            }
            "docusaurus" | "@docusaurus/core" => {
                roles.insert("docs".to_owned());
                tools.insert("docusaurus".to_owned());
            }
            "express" | "fastify" | "@nestjs/core" | "nestjs" => {
                roles.insert("backend".to_owned());
                tools.insert(dep);
            }
            "react-native" | "expo" => {
                ecosystems.insert("mobile".to_owned());
                roles.insert("mobile".to_owned());
                tools.insert(dep);
            }
            "electron" => {
                ecosystems.insert("electron".to_owned());
                roles.insert("desktop".to_owned());
                tools.insert(dep);
            }
            "@tauri-apps/cli" | "@tauri-apps/api" => {
                ecosystems.insert("tauri".to_owned());
                roles.insert("desktop".to_owned());
                tools.insert("tauri".to_owned());
            }
            "playwright" | "@playwright/test" | "cypress" | "vitest" | "jest" | "eslint"
            | "prettier" => {
                roles.insert("quality".to_owned());
                tools.insert(dep);
            }
            "semgrep" => {
                roles.insert("security".to_owned());
                tools.insert(dep);
            }
            _ => {}
        }
    }
    Ok(())
}

fn package_json_dependency_names(value: &Value) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for section in [
        "dependencies",
        "devDependencies",
        "peerDependencies",
        "optionalDependencies",
    ] {
        if let Some(map) = value.get(section).and_then(Value::as_object) {
            names.extend(map.keys().cloned());
        }
    }
    names
}

fn pubspec_looks_like_flutter(root: &Path, rel: &str) -> Result<bool, AppError> {
    let path = root.join(rel);
    let raw = adapters::io::read_to_string(&path)?;
    Ok(raw.lines().any(|line| {
        let trimmed = line.trim();
        trimmed == "flutter:" || trimmed.contains("sdk: flutter")
    }))
}

fn node_package_manager(tools: &[&str]) -> &'static str {
    if tools.contains(&"bun") {
        "bun"
    } else if tools.contains(&"pnpm") {
        "pnpm"
    } else if tools.contains(&"yarn") {
        "yarn"
    } else {
        "npm"
    }
}

fn node_install_command(package_manager: &str) -> Vec<&'static str> {
    match package_manager {
        "bun" => vec!["bun", "install"],
        "pnpm" => vec!["pnpm", "install"],
        "yarn" => vec!["yarn", "install"],
        _ => vec!["npm", "install"],
    }
}

fn add_node_script_commands(
    root: &Path,
    files: &ProjectFileGroups,
    package_manager: &str,
    commands: &mut Vec<SuggestedCommand>,
) -> Result<(), AppError> {
    let mut scripts = BTreeSet::new();
    for file in &files.packages {
        if file.kind != "npm" {
            continue;
        }
        for script in read_package_json_scripts(root, &file.path)?.keys() {
            scripts.insert(script.clone());
        }
    }

    for script in ["test", "build", "lint", "format"] {
        if !scripts.contains(script) {
            continue;
        }
        let command = node_script_command(package_manager, script);
        commands.push(suggested(
            script,
            command.as_slice(),
            "medium",
            "package.json script detected",
        ));
    }
    if scripts.is_empty() {
        commands.push(suggested(
            "test",
            node_script_command(package_manager, "test").as_slice(),
            "low",
            "package.json detected",
        ));
        commands.push(suggested(
            "build",
            node_script_command(package_manager, "build").as_slice(),
            "low",
            "package.json detected",
        ));
    }
    Ok(())
}

fn read_package_json_scripts(root: &Path, rel: &str) -> Result<BTreeMap<String, String>, AppError> {
    let path = root.join(rel);
    let raw = adapters::io::read_to_string(&path)?;
    let value = serde_json::from_str::<Value>(&raw)
        .map_err(|source| AppError::json_deserialization(path, source))?;
    let mut scripts = BTreeMap::new();
    if let Some(map) = value.get("scripts").and_then(Value::as_object) {
        for (key, value) in map {
            if let Some(command) = value.as_str() {
                scripts.insert(key.clone(), command.to_owned());
            }
        }
    }
    Ok(scripts)
}

fn node_script_command(package_manager: &str, script: &str) -> Vec<&'static str> {
    match (package_manager, script) {
        ("npm", "test") => vec!["npm", "test"],
        ("npm", "build") => vec!["npm", "run", "build"],
        ("npm", "lint") => vec!["npm", "run", "lint"],
        ("npm", "format") => vec!["npm", "run", "format"],
        ("pnpm", "test") => vec!["pnpm", "test"],
        ("pnpm", "build") => vec!["pnpm", "build"],
        ("pnpm", "lint") => vec!["pnpm", "lint"],
        ("pnpm", "format") => vec!["pnpm", "format"],
        ("yarn", "test") => vec!["yarn", "test"],
        ("yarn", "build") => vec!["yarn", "build"],
        ("yarn", "lint") => vec!["yarn", "lint"],
        ("yarn", "format") => vec!["yarn", "format"],
        ("bun", "test") => vec!["bun", "test"],
        ("bun", "build") => vec!["bun", "run", "build"],
        ("bun", "lint") => vec!["bun", "run", "lint"],
        ("bun", "format") => vec!["bun", "run", "format"],
        _ => vec!["npm", "run", "test"],
    }
}

fn python_runner(tools: &[&str]) -> Vec<&'static str> {
    if tools.contains(&"uv") {
        vec!["uv", "run", "pytest"]
    } else if tools.contains(&"poetry") {
        vec!["poetry", "run", "pytest"]
    } else {
        vec!["pytest"]
    }
}

fn deduplicate_commands(commands: Vec<SuggestedCommand>) -> Vec<SuggestedCommand> {
    let mut seen = BTreeSet::new();
    let mut result = Vec::new();
    for command in commands {
        let key = format!("{}:{}", command.kind, command.command.join(" "));
        if seen.insert(key) {
            result.push(command);
        }
    }
    result
}

fn suggest_commands(
    ecosystems: &[String],
    tools: &[String],
    files: &ProjectFileGroups,
    root: &Path,
) -> Result<Vec<SuggestedCommand>, AppError> {
    let ecosystems = ecosystems.iter().map(String::as_str).collect::<Vec<_>>();
    let tools = tools.iter().map(String::as_str).collect::<Vec<_>>();
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
        let package_manager = node_package_manager(&tools);
        commands.push(suggested(
            "install",
            node_install_command(package_manager).as_slice(),
            "medium",
            "Node manifest detected",
        ));
        add_node_script_commands(root, files, package_manager, &mut commands)?;
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
        let runner = python_runner(&tools);
        commands.push(suggested(
            "test",
            runner.as_slice(),
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
    if ecosystems.contains(&"php") {
        commands.push(suggested(
            "install",
            &["composer", "install"],
            "medium",
            "composer.json detected",
        ));
        commands.push(suggested(
            "test",
            &["composer", "test"],
            "low",
            "composer.json detected",
        ));
    }
    if ecosystems.contains(&"ruby") {
        commands.push(suggested(
            "install",
            &["bundle", "install"],
            "medium",
            "Gemfile detected",
        ));
        commands.push(suggested(
            "test",
            &["bundle", "exec", "rspec"],
            "low",
            "Gemfile detected",
        ));
    }
    if ecosystems.contains(&"elixir") {
        commands.push(suggested(
            "deps",
            &["mix", "deps.get"],
            "medium",
            "mix.exs detected",
        ));
        commands.push(suggested(
            "test",
            &["mix", "test"],
            "medium",
            "mix.exs detected",
        ));
    }
    if ecosystems.contains(&"dart") {
        let dart_tool = if tools.contains(&"flutter") {
            "flutter"
        } else {
            "dart"
        };
        commands.push(suggested(
            "test",
            &[dart_tool, "test"],
            "medium",
            "pubspec.yaml detected",
        ));
    }
    if ecosystems.contains(&"swift") {
        commands.push(suggested(
            "test",
            &["swift", "test"],
            "medium",
            "Package.swift detected",
        ));
        commands.push(suggested(
            "build",
            &["swift", "build"],
            "medium",
            "Package.swift detected",
        ));
    }
    if ecosystems.contains(&"scala") {
        commands.push(suggested(
            "test",
            &["sbt", "test"],
            "medium",
            "build.sbt detected",
        ));
    }
    if tools.contains(&"clojure") {
        commands.push(suggested(
            "test",
            &["clojure", "-X:test"],
            "low",
            "deps.edn detected",
        ));
    }
    if tools.contains(&"leiningen") {
        commands.push(suggested(
            "test",
            &["lein", "test"],
            "medium",
            "project.clj detected",
        ));
    }
    if tools.contains(&"stack") {
        commands.push(suggested(
            "test",
            &["stack", "test"],
            "medium",
            "stack.yaml detected",
        ));
    }
    if tools.contains(&"cabal") {
        commands.push(suggested(
            "test",
            &["cabal", "test", "all"],
            "medium",
            "Cabal project detected",
        ));
        commands.push(suggested(
            "build",
            &["cabal", "build", "all"],
            "medium",
            "Cabal project detected",
        ));
    }
    if tools.contains(&"dune") {
        commands.push(suggested(
            "test",
            &["dune", "runtest"],
            "medium",
            "dune-project detected",
        ));
        commands.push(suggested(
            "build",
            &["dune", "build"],
            "medium",
            "dune-project detected",
        ));
    }
    if ecosystems.contains(&"julia") {
        commands.push(suggested(
            "test",
            &["julia", "--project=.", "-e", "using Pkg; Pkg.test()"],
            "medium",
            "Julia Project.toml detected",
        ));
    }
    if ecosystems.contains(&"r") {
        commands.push(suggested(
            "test",
            &["Rscript", "-e", "devtools::test()"],
            "low",
            "R DESCRIPTION detected",
        ));
    }
    if tools.contains(&"zig") {
        commands.push(suggested(
            "test",
            &["zig", "build", "test"],
            "medium",
            "build.zig detected",
        ));
        commands.push(suggested(
            "build",
            &["zig", "build"],
            "medium",
            "build.zig detected",
        ));
    }
    if tools.contains(&"platformio") {
        commands.push(suggested(
            "build",
            &["pio", "run"],
            "medium",
            "platformio.ini detected",
        ));
        commands.push(suggested(
            "test",
            &["pio", "test"],
            "low",
            "platformio.ini detected",
        ));
    }
    if tools.contains(&"meson") {
        commands.push(suggested(
            "configure",
            &["meson", "setup", "build"],
            "medium",
            "meson.build detected",
        ));
        commands.push(suggested(
            "test",
            &["meson", "test", "-C", "build"],
            "medium",
            "meson.build detected",
        ));
    }
    if tools.contains(&"bazel") {
        commands.push(suggested(
            "test",
            &["bazel", "test", "//..."],
            "medium",
            "Bazel workspace detected",
        ));
        commands.push(suggested(
            "build",
            &["bazel", "build", "//..."],
            "medium",
            "Bazel workspace detected",
        ));
    }
    if tools.contains(&"cmake") {
        commands.push(suggested(
            "configure",
            &["cmake", "-S", ".", "-B", "build"],
            "medium",
            "CMakeLists.txt detected",
        ));
        commands.push(suggested(
            "build",
            &["cmake", "--build", "build"],
            "medium",
            "CMakeLists.txt detected",
        ));
        commands.push(suggested(
            "test",
            &["ctest", "--test-dir", "build"],
            "low",
            "CMakeLists.txt detected",
        ));
    }
    if tools.contains(&"make") {
        commands.push(suggested("build", &["make"], "low", "Makefile detected"));
        commands.push(suggested(
            "test",
            &["make", "test"],
            "low",
            "Makefile detected",
        ));
    }
    if tools.contains(&"terraform") {
        commands.push(suggested(
            "init",
            &["terraform", "init"],
            "medium",
            "Terraform files detected",
        ));
        commands.push(suggested(
            "validate",
            &["terraform", "validate"],
            "medium",
            "Terraform files detected",
        ));
        commands.push(suggested(
            "plan",
            &["terraform", "plan"],
            "low",
            "Terraform files detected",
        ));
    }
    if tools.contains(&"docker") {
        commands.push(suggested(
            "container_build",
            &["docker", "build", "-t", "app", "."],
            "low",
            "Dockerfile detected",
        ));
    }
    if tools.contains(&"docker-compose") {
        commands.push(suggested(
            "compose_config",
            &["docker", "compose", "config"],
            "medium",
            "Compose file detected",
        ));
    }
    if tools.contains(&"pulumi") {
        commands.push(suggested(
            "preview",
            &["pulumi", "preview"],
            "medium",
            "Pulumi.yaml detected",
        ));
    }
    if tools.contains(&"tofu") {
        commands.push(suggested(
            "init",
            &["tofu", "init"],
            "medium",
            "OpenTofu files detected",
        ));
        commands.push(suggested(
            "plan",
            &["tofu", "plan"],
            "low",
            "OpenTofu files detected",
        ));
    }
    if tools.contains(&"nomad") {
        commands.push(suggested(
            "validate",
            &["nomad", "job", "validate"],
            "low",
            "Nomad job files detected",
        ));
    }
    if tools.contains(&"pre-commit") {
        commands.push(suggested(
            "quality",
            &["pre-commit", "run", "--all-files"],
            "medium",
            "pre-commit config detected",
        ));
    }
    if tools.contains(&"semgrep") {
        commands.push(suggested(
            "security",
            &["semgrep", "scan"],
            "medium",
            "Semgrep config detected",
        ));
    }
    if tools.contains(&"trivy") {
        commands.push(suggested(
            "security",
            &["trivy", "fs", "."],
            "medium",
            "Trivy config detected",
        ));
    }
    Ok(deduplicate_commands(commands))
}

fn detect_versions(
    root: &Path,
    candidates: &[PathBuf],
) -> Result<Vec<ProjectVersionEntry>, AppError> {
    let mut versions = Vec::new();
    for file in candidates {
        let Some(name) = file.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        let lower_name = name.to_ascii_lowercase();
        let rel = normalize_relative(root, file);
        let parsed = match lower_name.as_str() {
            "cargo.toml" => parse_cargo_version(file, &rel)?,
            "package.json" => parse_package_json_version(file, &rel)?,
            "composer.json" => parse_package_json_like_version(file, &rel, "composer")?,
            "pyproject.toml" => parse_pyproject_version(file, &rel)?,
            "pubspec.yaml" => parse_pubspec_version(file, &rel)?,
            "mix.exs" => parse_assignment_version(file, &rel, "mix", "medium")?,
            "pom.xml" => parse_xml_version(file, &rel, "maven", "medium")?,
            "build.gradle" | "build.gradle.kts" => parse_gradle_version(file, &rel)?,
            _ if lower_name.ends_with(".gemspec") => {
                parse_assignment_version(file, &rel, "gemspec", "medium")?
            }
            _ if lower_name.ends_with(".csproj") => {
                parse_xml_version(file, &rel, "dotnet", "high")?
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
    let raw = adapters::io::read_to_string(path)?;
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
    let raw = adapters::io::read_to_string(path)?;
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
    let raw = adapters::io::read_to_string(path)?;
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

fn parse_package_json_like_version(
    path: &Path,
    rel: &str,
    kind: &str,
) -> Result<Option<ProjectVersionEntry>, AppError> {
    let raw = adapters::io::read_to_string(path)?;
    let value = serde_json::from_str::<Value>(&raw)
        .map_err(|source| AppError::json_deserialization(path.into(), source))?;
    let version = value
        .get("version")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Ok(version.map(|version| ProjectVersionEntry {
        kind: kind.to_owned(),
        path: rel.to_owned(),
        name: value.get("name").and_then(Value::as_str).map(str::to_owned),
        version: Some(version),
        confidence: "high".to_owned(),
    }))
}

fn parse_pubspec_version(path: &Path, rel: &str) -> Result<Option<ProjectVersionEntry>, AppError> {
    let raw = adapters::io::read_to_string(path)?;
    let mut name = None;
    let mut version = None;
    for line in raw.lines() {
        let trimmed = line.trim();
        if let Some(value) = trimmed.strip_prefix("name:") {
            name = Some(value.trim().trim_matches(&['"', '\''][..]).to_owned());
        } else if let Some(value) = trimmed.strip_prefix("version:") {
            version = Some(value.trim().trim_matches(&['"', '\''][..]).to_owned());
        }
    }
    Ok(version.map(|version| ProjectVersionEntry {
        kind: "pub".to_owned(),
        path: rel.to_owned(),
        name,
        version: Some(version),
        confidence: "medium".to_owned(),
    }))
}

fn parse_assignment_version(
    path: &Path,
    rel: &str,
    kind: &str,
    confidence: &str,
) -> Result<Option<ProjectVersionEntry>, AppError> {
    let raw = adapters::io::read_to_string(path)?;
    let version_re = Regex::new(r#"(?m)\bversion\s*[:=]\s*['"]([^'"]+)['"]"#)
        .map_err(|error| AppError::invalid_argument(format!("internal regex error: {error}")))?;
    let name_re = Regex::new(r#"(?m)\bname\s*[:=]\s*['"]([^'"]+)['"]"#)
        .map_err(|error| AppError::invalid_argument(format!("internal regex error: {error}")))?;
    let version = version_re
        .captures(&raw)
        .and_then(|captures| captures.get(1))
        .map(|value| value.as_str().to_owned());
    Ok(version.map(|version| ProjectVersionEntry {
        kind: kind.to_owned(),
        path: rel.to_owned(),
        name: name_re
            .captures(&raw)
            .and_then(|captures| captures.get(1))
            .map(|value| value.as_str().to_owned()),
        version: Some(version),
        confidence: confidence.to_owned(),
    }))
}

fn parse_xml_version(
    path: &Path,
    rel: &str,
    kind: &str,
    confidence: &str,
) -> Result<Option<ProjectVersionEntry>, AppError> {
    let raw = adapters::io::read_to_string(path)?;
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
    let raw = adapters::io::read_to_string(path)?;
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
