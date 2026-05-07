use std::sync::Arc;

use ah_plugin_api::{
    InvocationRequest, InvocationResponse, ManualCommand, ManualExample, PluginManual,
    PluginMetadata,
};
use ah_runtime::BuiltinPlugin;
use clap::{CommandFactory, Parser, error::ErrorKind};

use crate::{cli::GlobalOptions, commands, error::AppError};

#[derive(Debug, Parser)]
struct FilePluginCli {
    #[command(flatten)]
    args: commands::file::FileArgs,
}

#[derive(Debug, Parser)]
struct SearchPluginCli {
    #[command(flatten)]
    args: commands::search::SearchArgs,
}

#[derive(Debug, Parser)]
struct CtxPluginCli {
    #[command(flatten)]
    args: commands::ctx::CtxArgs,
}

#[derive(Debug, Parser)]
struct GitPluginCli {
    #[command(flatten)]
    args: commands::git::GitArgs,
}

#[derive(Debug, Parser)]
struct ProjectPluginCli {
    #[command(flatten)]
    args: commands::project::ProjectArgs,
}

#[derive(Debug, Parser)]
struct RunPluginCli {
    #[command(flatten)]
    args: commands::run::RunArgs,
}

#[derive(Debug, Parser)]
struct HttpPluginCli {
    #[command(flatten)]
    args: commands::http::HttpArgs,
}

#[derive(Debug, Parser)]
struct TaskPluginCli {
    #[command(flatten)]
    args: commands::task::TaskArgs,
}

pub fn builtins() -> Vec<Arc<dyn BuiltinPlugin>> {
    vec![
        Arc::new(FileBuiltinPlugin),
        Arc::new(SearchBuiltinPlugin),
        Arc::new(CtxBuiltinPlugin),
        Arc::new(GitBuiltinPlugin),
        Arc::new(ProjectBuiltinPlugin),
        Arc::new(RunBuiltinPlugin),
        Arc::new(HttpBuiltinPlugin),
        Arc::new(TaskBuiltinPlugin),
    ]
}

struct FileBuiltinPlugin;
struct SearchBuiltinPlugin;
struct CtxBuiltinPlugin;
struct GitBuiltinPlugin;
struct ProjectBuiltinPlugin;
struct RunBuiltinPlugin;
struct HttpBuiltinPlugin;
struct TaskBuiltinPlugin;

fn file_metadata() -> PluginMetadata {
    PluginMetadata {
        plugin_name: "builtin-file".to_owned(),
        domain: "file".to_owned(),
        description: "File operations plugin (built-in)".to_owned(),
        abi_version: 1,
    }
}

fn search_metadata() -> PluginMetadata {
    PluginMetadata {
        plugin_name: "builtin-search".to_owned(),
        domain: "search".to_owned(),
        description: "Search operations plugin (built-in)".to_owned(),
        abi_version: 1,
    }
}

fn ctx_metadata() -> PluginMetadata {
    PluginMetadata {
        plugin_name: "builtin-ctx".to_owned(),
        domain: "ctx".to_owned(),
        description: "Context utilities plugin (built-in)".to_owned(),
        abi_version: 1,
    }
}

fn git_metadata() -> PluginMetadata {
    PluginMetadata {
        plugin_name: "builtin-git".to_owned(),
        domain: "git".to_owned(),
        description: "Git utilities plugin (built-in)".to_owned(),
        abi_version: 1,
    }
}

fn project_metadata() -> PluginMetadata {
    PluginMetadata {
        plugin_name: "builtin-project".to_owned(),
        domain: "project".to_owned(),
        description: "Project detection plugin (built-in)".to_owned(),
        abi_version: 1,
    }
}

fn run_metadata() -> PluginMetadata {
    PluginMetadata {
        plugin_name: "builtin-run".to_owned(),
        domain: "run".to_owned(),
        description: "Command execution check plugin (built-in)".to_owned(),
        abi_version: 1,
    }
}

fn task_metadata() -> PluginMetadata {
    PluginMetadata {
        plugin_name: "builtin-task".to_owned(),
        domain: "task".to_owned(),
        description: "Task recipe plugin (built-in)".to_owned(),
        abi_version: 1,
    }
}

fn http_metadata() -> PluginMetadata {
    PluginMetadata {
        plugin_name: "builtin-http".to_owned(),
        domain: "http".to_owned(),
        description: "HTTP workflow plugin (built-in)".to_owned(),
        abi_version: 1,
    }
}

fn file_manual() -> PluginManual {
    PluginManual {
        plugin_name: file_metadata().plugin_name,
        domain: "file".to_owned(),
        description: "Read and inspect files and directory trees.".to_owned(),
        commands: vec![
            ManualCommand {
                name: "read".to_owned(),
                summary: "Read file content with optional line range and numbering.".to_owned(),
                usage:
                    "read <path> [-n] [--from N] [--to N] [--max-bytes BYTES] [--follow-symlinks]"
                        .to_owned(),
                examples: vec![
                    manual_example(
                        "Read first 120 lines with source numbers",
                        &["read", "src/main.rs", "-n", "--from", "1", "--to", "120"],
                    ),
                    manual_example(
                        "Read with explicit size/symlink policy",
                        &[
                            "read",
                            "README.md",
                            "--max-bytes",
                            "1048576",
                            "--follow-symlinks",
                        ],
                    ),
                ],
            },
            ManualCommand {
                name: "head".to_owned(),
                summary: "Return first N lines.".to_owned(),
                usage: "head <path> [--lines N] [-n] [--max-bytes BYTES] [--follow-symlinks]"
                    .to_owned(),
                examples: vec![manual_example(
                    "Preview first 40 lines",
                    &["head", "src/lib.rs", "--lines", "40", "-n"],
                )],
            },
            ManualCommand {
                name: "tail".to_owned(),
                summary: "Return last N lines.".to_owned(),
                usage: "tail <path> [--lines N] [-n] [--max-bytes BYTES] [--follow-symlinks]"
                    .to_owned(),
                examples: vec![manual_example(
                    "Inspect file tail",
                    &["tail", "CHANGELOG.md", "--lines", "30"],
                )],
            },
            ManualCommand {
                name: "stat".to_owned(),
                summary: "Show file metadata.".to_owned(),
                usage: "stat <path>".to_owned(),
                examples: vec![manual_example("Inspect metadata", &["stat", "Cargo.toml"])],
            },
            ManualCommand {
                name: "tree".to_owned(),
                summary: "Render directory tree.".to_owned(),
                usage: "tree [path] [--depth N] [--follow-symlinks]".to_owned(),
                examples: vec![manual_example(
                    "Show compact source tree",
                    &["tree", "src", "--depth", "2"],
                )],
            },
        ],
        notes: vec![
            "Prefer narrow ranges and --limit to reduce context size.".to_owned(),
            "Use --json for machine-readable chaining.".to_owned(),
        ],
    }
}

fn search_manual() -> PluginManual {
    PluginManual {
        plugin_name: search_metadata().plugin_name,
        domain: "search".to_owned(),
        description: "Find text matches and file paths quickly.".to_owned(),
        commands: vec![
            ManualCommand {
                name: "text".to_owned(),
                summary: "Search text in files (literal by default, regex with --regex).".to_owned(),
                usage: "text <pattern> [path] [--glob ...] [--ignore-case] [--context N] [--regex] [--max-bytes BYTES] [--follow-symlinks]".to_owned(),
                examples: vec![
                    manual_example(
                        "Literal search in Rust files",
                        &["text", "PluginManager", "src", "--glob", "*.rs", "--context", "1"],
                    ),
                    manual_example(
                        "Regex search for function declarations",
                        &["text", "fn\\s+execute", "src", "--regex", "--context", "2"],
                    ),
                ],
            },
            ManualCommand {
                name: "files".to_owned(),
                summary: "Find file paths containing substring.".to_owned(),
                usage: "files <query> [path] [--follow-symlinks]".to_owned(),
                examples: vec![manual_example(
                    "Find docs related to plugins",
                    &["files", "plugin", "docs"],
                )],
            },
        ],
        notes: vec![
            "Binary and oversized files are skipped by policy.".to_owned(),
            "Use --regex only when needed; literal mode is usually faster.".to_owned(),
        ],
    }
}

fn ctx_manual() -> PluginManual {
    PluginManual {
        plugin_name: ctx_metadata().plugin_name,
        domain: "ctx".to_owned(),
        description: "Context-reduction helpers for AI workflows.".to_owned(),
        commands: vec![
            ManualCommand {
                name: "pack".to_owned(),
                summary: "Create compact digest for files/directories.".to_owned(),
                usage: "pack <path...> [--preset <summary|review|debug>] [--max-bytes BYTES] [--follow-symlinks]".to_owned(),
                examples: vec![manual_example(
                    "Pack codebase context for review",
                    &["pack", "src", "docs", "--preset", "review"],
                )],
            },
            ManualCommand {
                name: "symbols".to_owned(),
                summary:
                    "Extract code, config, and document symbols from file or directory.".to_owned(),
                usage: "symbols <path> [--preset <summary|review|debug>] [--max-bytes BYTES] [--follow-symlinks]".to_owned(),
                examples: vec![manual_example(
                    "Extract symbols from commands module",
                    &["symbols", "src/commands", "--preset", "summary"],
                )],
            },
            ManualCommand {
                name: "changed".to_owned(),
                summary: "Show changed files from git status.".to_owned(),
                usage: "changed".to_owned(),
                examples: vec![manual_example(
                    "Collect changed paths before review",
                    &["changed"],
                )],
            },
        ],
        notes: vec![
            "Presets tune default limits and symbol density.".to_owned(),
            "Symbol extraction uses lightweight heuristics across common programming, infra, config, and script files.".to_owned(),
            "Pair with --json for downstream prompt assembly.".to_owned(),
        ],
    }
}

fn git_manual() -> PluginManual {
    PluginManual {
        plugin_name: git_metadata().plugin_name,
        domain: "git".to_owned(),
        description: "Git-oriented context helpers for working tree and history.".to_owned(),
        commands: vec![
            ManualCommand {
                name: "status".to_owned(),
                summary: "Show compact repository status summary.".to_owned(),
                usage: "status".to_owned(),
                examples: vec![manual_example(
                    "Inspect branch, upstream, counts, commit, and tag",
                    &["status"],
                )],
            },
            ManualCommand {
                name: "tags".to_owned(),
                summary: "List tags newest-first.".to_owned(),
                usage: "tags [--latest]".to_owned(),
                examples: vec![
                    manual_example("List tags", &["tags"]),
                    manual_example("Show latest tag only", &["tags", "--latest"]),
                ],
            },
            ManualCommand {
                name: "tag create".to_owned(),
                summary: "Create a lightweight or annotated git tag.".to_owned(),
                usage: "tag create <tag> [--message TEXT] [--ref REF]".to_owned(),
                examples: vec![manual_example(
                    "Create an annotated release tag",
                    &["tag", "create", "v1.0.0", "--message", "v1.0.0"],
                )],
            },
            ManualCommand {
                name: "remotes".to_owned(),
                summary: "List configured git remotes with provider hint.".to_owned(),
                usage: "remotes".to_owned(),
                examples: vec![manual_example("Inspect remotes", &["remotes"])],
            },
            ManualCommand {
                name: "changed".to_owned(),
                summary: "Summarize working tree changes.".to_owned(),
                usage: "changed".to_owned(),
                examples: vec![manual_example("List changed files", &["changed"])],
            },
            ManualCommand {
                name: "diff".to_owned(),
                summary: "Show local diff (optionally filtered by path).".to_owned(),
                usage: "diff [--path <path>]".to_owned(),
                examples: vec![manual_example(
                    "Review diff for one file",
                    &["diff", "--path", "src/cli.rs"],
                )],
            },
            ManualCommand {
                name: "blame".to_owned(),
                summary: "Show blame data for file or single line.".to_owned(),
                usage: "blame <path> [--line N]".to_owned(),
                examples: vec![manual_example(
                    "Inspect ownership of a specific line",
                    &["blame", "src/commands/search.rs", "--line", "120"],
                )],
            },
            ManualCommand {
                name: "commit-info".to_owned(),
                summary: "Show commit metadata, touched files, and line stats.".to_owned(),
                usage: "commit-info [ref]".to_owned(),
                examples: vec![manual_example(
                    "Inspect the latest commit",
                    &["commit-info", "HEAD"],
                )],
            },
        ],
        notes: vec!["Useful for quick change-attribution in AI review loops.".to_owned()],
    }
}

fn project_manual() -> PluginManual {
    PluginManual {
        plugin_name: project_metadata().plugin_name,
        domain: "project".to_owned(),
        description: "Detect project ecosystems, tools, roles, versions, and suggested commands."
            .to_owned(),
        commands: vec![
            ManualCommand {
                name: "detect".to_owned(),
                summary: "Detect ecosystems, tools, roles, grouped files, versions, and commands."
                    .to_owned(),
                usage: "detect [path]".to_owned(),
                examples: vec![manual_example("Detect current project", &["detect"])],
            },
            ManualCommand {
                name: "commands".to_owned(),
                summary: "Suggest likely install, test, build, release, and infra commands."
                    .to_owned(),
                usage: "commands [path]".to_owned(),
                examples: vec![manual_example(
                    "Suggest commands for current project",
                    &["commands"],
                )],
            },
            ManualCommand {
                name: "version".to_owned(),
                summary: "Detect project version from common manifest files.".to_owned(),
                usage: "version [path]".to_owned(),
                examples: vec![manual_example(
                    "Detect current project version",
                    &["version"],
                )],
            },
        ],
        notes: vec![
            "Detection is heuristic and does not execute package managers or infra tools."
                .to_owned(),
            "JSON detect output includes compatibility fields plus richer grouped snapshot fields."
                .to_owned(),
            "Use with ah run check to execute suggested commands explicitly.".to_owned(),
        ],
    }
}

fn run_manual() -> PluginManual {
    PluginManual {
        plugin_name: run_metadata().plugin_name,
        domain: "run".to_owned(),
        description: "Run explicit commands with timeout and bounded output.".to_owned(),
        commands: vec![ManualCommand {
            name: "check".to_owned(),
            summary: "Run a command and report success, exit code, duration, stdout, and stderr."
                .to_owned(),
            usage:
                "check [--timeout-secs SECONDS] [--max-output-bytes BYTES] [--tail-lines N] <command...>"
                    .to_owned(),
            examples: vec![
                manual_example("Run cargo tests", &["check", "cargo", "test"]),
                manual_example(
                    "Run command with timeout",
                    &["check", "--timeout-secs", "60", "cargo", "build"],
                ),
            ],
        }],
        notes: vec![
            "Command is executed directly without a shell.".to_owned(),
            "The ah command itself exits successfully; inspect success=false for checked command failures.".to_owned(),
        ],
    }
}

fn task_manual() -> PluginManual {
    PluginManual {
        plugin_name: task_metadata().plugin_name,
        domain: "task".to_owned(),
        description: "Save and execute reusable local command recipes.".to_owned(),
        commands: vec![
            ManualCommand {
                name: "save".to_owned(),
                summary: "Create or update task recipe.".to_owned(),
                usage: "save <name> <command>".to_owned(),
                examples: vec![manual_example(
                    "Save quick status workflow",
                    &["save", "print-working-dir", "pwd"],
                )],
            },
            ManualCommand {
                name: "run".to_owned(),
                summary: "Run saved task by name.".to_owned(),
                usage: "run <name>".to_owned(),
                examples: vec![manual_example(
                    "Execute saved workflow",
                    &["run", "print-working-dir"],
                )],
            },
            ManualCommand {
                name: "list".to_owned(),
                summary: "List available task recipes.".to_owned(),
                usage: "list".to_owned(),
                examples: vec![manual_example("Inspect available recipes", &["list"])],
            },
        ],
        notes: vec!["Task commands execute through system shell.".to_owned()],
    }
}

fn http_manual() -> PluginManual {
    PluginManual {
        plugin_name: http_metadata().plugin_name,
        domain: "http".to_owned(),
        description: "HTTP request and API assertion helpers.".to_owned(),
        commands: vec![
            ManualCommand {
                name: "request".to_owned(),
                summary: "Send HTTP request with explicit method.".to_owned(),
                usage: "request --method <METHOD> <url> [--header \"K: V\"] [--query \"KEY=VALUE\"] [--timeout-secs N] [--bearer TOKEN] [--basic USER:PASS] [--json <JSON>|--json-file <PATH>] [--body <TEXT>|--body-file <PATH>] [--expect-status <code|range>] [--expect-header \"K: V\"] [--expect-body-contains <TEXT>] [--expect-json <PATH:OP[:VALUE]>]".to_owned(),
                examples: vec![manual_example(
                    "Basic request with status check",
                    &["request", "--method", "GET", "https://example.com/health", "--expect-status", "200"],
                )],
            },
            ManualCommand {
                name: "get".to_owned(),
                summary: "Shortcut for GET request.".to_owned(),
                usage: "get <url> [request/expect flags]".to_owned(),
                examples: vec![manual_example(
                    "GET JSON endpoint",
                    &["get", "https://example.com/api/version", "--expect-status", "2xx"],
                )],
            },
            ManualCommand {
                name: "post".to_owned(),
                summary: "Shortcut for POST request.".to_owned(),
                usage: "post <url> [request/expect flags]".to_owned(),
                examples: vec![manual_example(
                    "POST JSON payload",
                    &["post", "https://example.com/api/items", "--json", "{\"name\":\"demo\"}", "--expect-status", "201"],
                )],
            },
            ManualCommand {
                name: "replay".to_owned(),
                summary: "Replay supported curl command form.".to_owned(),
                usage: "replay --curl \"<curl ...>\" [request/expect flags]".to_owned(),
                examples: vec![manual_example(
                    "Replay existing curl command",
                    &[
                        "replay",
                        "--curl",
                        "curl -X GET https://example.com/health -H 'accept: application/json'",
                    ],
                )],
            },
            ManualCommand {
                name: "assert".to_owned(),
                summary: "Run API assertions from YAML/JSON spec.".to_owned(),
                usage: "assert <spec-path> [--var KEY=VALUE ...] [--fail-fast] [--report text|json|junit]".to_owned(),
                examples: vec![manual_example(
                    "Run assertions with machine output",
                    &["assert", "api/health.yaml", "--report", "json"],
                )],
            },
            ManualCommand {
                name: "run".to_owned(),
                summary: "Alias for assert.".to_owned(),
                usage: "run <spec-path> [--var KEY=VALUE ...] [--fail-fast] [--report text|json|junit]".to_owned(),
                examples: vec![manual_example(
                    "Alias usage",
                    &["run", "api/health.yaml", "--fail-fast"],
                )],
            },
        ],
        notes: vec![
            "Spec format is YAML-first with JSON compatibility.".to_owned(),
            "Global --json maps assert/run report format to json.".to_owned(),
            "Retries and cross-case extract variables are planned for v1.1.".to_owned(),
        ],
    }
}

fn manual_example(description: &str, argv: &[&str]) -> ManualExample {
    ManualExample {
        description: description.to_owned(),
        argv: argv.iter().map(|value| (*value).to_owned()).collect(),
    }
}

impl BuiltinPlugin for FileBuiltinPlugin {
    fn metadata(&self) -> PluginMetadata {
        file_metadata()
    }

    fn manual(&self) -> PluginManual {
        file_manual()
    }

    fn invoke(&self, request: &InvocationRequest) -> InvocationResponse {
        let parsed = match parse_args::<FilePluginCli>("file", &request.argv) {
            ParseOutcome::Parsed(value) => value,
            ParseOutcome::Response(response) => return response,
        };
        let options = GlobalOptions::from(request.globals.clone());
        map_execute(commands::file::execute(parsed.args, &options))
    }
}

impl BuiltinPlugin for SearchBuiltinPlugin {
    fn metadata(&self) -> PluginMetadata {
        search_metadata()
    }

    fn manual(&self) -> PluginManual {
        search_manual()
    }

    fn invoke(&self, request: &InvocationRequest) -> InvocationResponse {
        let parsed = match parse_args::<SearchPluginCli>("search", &request.argv) {
            ParseOutcome::Parsed(value) => value,
            ParseOutcome::Response(response) => return response,
        };
        let options = GlobalOptions::from(request.globals.clone());
        map_execute(commands::search::execute(parsed.args, &options))
    }
}

impl BuiltinPlugin for CtxBuiltinPlugin {
    fn metadata(&self) -> PluginMetadata {
        ctx_metadata()
    }

    fn manual(&self) -> PluginManual {
        ctx_manual()
    }

    fn invoke(&self, request: &InvocationRequest) -> InvocationResponse {
        let parsed = match parse_args::<CtxPluginCli>("ctx", &request.argv) {
            ParseOutcome::Parsed(value) => value,
            ParseOutcome::Response(response) => return response,
        };
        let options = GlobalOptions::from(request.globals.clone());
        map_execute(commands::ctx::execute(parsed.args, &options))
    }
}

impl BuiltinPlugin for GitBuiltinPlugin {
    fn metadata(&self) -> PluginMetadata {
        git_metadata()
    }

    fn manual(&self) -> PluginManual {
        git_manual()
    }

    fn invoke(&self, request: &InvocationRequest) -> InvocationResponse {
        let parsed = match parse_args::<GitPluginCli>("git", &request.argv) {
            ParseOutcome::Parsed(value) => value,
            ParseOutcome::Response(response) => return response,
        };
        let options = GlobalOptions::from(request.globals.clone());
        map_execute(commands::git::execute(parsed.args, &options))
    }
}

impl BuiltinPlugin for ProjectBuiltinPlugin {
    fn metadata(&self) -> PluginMetadata {
        project_metadata()
    }

    fn manual(&self) -> PluginManual {
        project_manual()
    }

    fn invoke(&self, request: &InvocationRequest) -> InvocationResponse {
        let parsed = match parse_args::<ProjectPluginCli>("project", &request.argv) {
            ParseOutcome::Parsed(value) => value,
            ParseOutcome::Response(response) => return response,
        };
        let options = GlobalOptions::from(request.globals.clone());
        map_execute(commands::project::execute(parsed.args, &options))
    }
}

impl BuiltinPlugin for RunBuiltinPlugin {
    fn metadata(&self) -> PluginMetadata {
        run_metadata()
    }

    fn manual(&self) -> PluginManual {
        run_manual()
    }

    fn invoke(&self, request: &InvocationRequest) -> InvocationResponse {
        let parsed = match parse_args::<RunPluginCli>("run", &request.argv) {
            ParseOutcome::Parsed(value) => value,
            ParseOutcome::Response(response) => return response,
        };
        let options = GlobalOptions::from(request.globals.clone());
        map_execute(commands::run::execute(parsed.args, &options))
    }
}

impl BuiltinPlugin for HttpBuiltinPlugin {
    fn metadata(&self) -> PluginMetadata {
        http_metadata()
    }

    fn manual(&self) -> PluginManual {
        http_manual()
    }

    fn invoke(&self, request: &InvocationRequest) -> InvocationResponse {
        let parsed = match parse_args::<HttpPluginCli>("http", &request.argv) {
            ParseOutcome::Parsed(value) => value,
            ParseOutcome::Response(response) => return response,
        };
        let options = GlobalOptions::from(request.globals.clone());
        map_execute(commands::http::execute(parsed.args, &options))
    }
}

impl BuiltinPlugin for TaskBuiltinPlugin {
    fn metadata(&self) -> PluginMetadata {
        task_metadata()
    }

    fn manual(&self) -> PluginManual {
        task_manual()
    }

    fn invoke(&self, request: &InvocationRequest) -> InvocationResponse {
        let parsed = match parse_args::<TaskPluginCli>("task", &request.argv) {
            ParseOutcome::Parsed(value) => value,
            ParseOutcome::Response(response) => return response,
        };
        let options = GlobalOptions::from(request.globals.clone());
        map_execute(commands::task::execute(parsed.args, &options))
    }
}

enum ParseOutcome<T> {
    Parsed(T),
    Response(InvocationResponse),
}

fn parse_args<T: Parser + CommandFactory>(domain: &str, argv: &[String]) -> ParseOutcome<T> {
    let mut args = Vec::with_capacity(argv.len() + 1);
    args.push(domain.to_owned());
    args.extend(argv.iter().cloned());

    match T::try_parse_from(args) {
        Ok(value) => ParseOutcome::Parsed(value),
        Err(error) => {
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) {
                ParseOutcome::Response(InvocationResponse::ok(Some(error.to_string())))
            } else {
                ParseOutcome::Response(InvocationResponse::error(
                    "INVALID_ARGUMENT",
                    error.to_string(),
                ))
            }
        }
    }
}

fn map_execute(result: Result<(), AppError>) -> InvocationResponse {
    match result {
        Ok(()) => InvocationResponse::ok(None),
        Err(error) => InvocationResponse::error(error.code(), error.detail_message()),
    }
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    use super::*;

    #[test]
    fn file_manual_examples_parse() {
        let manual = file_manual();
        assert_examples_parse::<FilePluginCli>(&manual);
    }

    #[test]
    fn search_manual_examples_parse() {
        let manual = search_manual();
        assert_examples_parse::<SearchPluginCli>(&manual);
    }

    #[test]
    fn ctx_manual_examples_parse() {
        let manual = ctx_manual();
        assert_examples_parse::<CtxPluginCli>(&manual);
    }

    #[test]
    fn git_manual_examples_parse() {
        let manual = git_manual();
        assert_examples_parse::<GitPluginCli>(&manual);
    }

    #[test]
    fn project_manual_examples_parse() {
        let manual = project_manual();
        assert_examples_parse::<ProjectPluginCli>(&manual);
    }

    #[test]
    fn run_manual_examples_parse() {
        let manual = run_manual();
        assert_examples_parse::<RunPluginCli>(&manual);
    }

    #[test]
    fn http_manual_examples_parse() {
        let manual = http_manual();
        assert_examples_parse::<HttpPluginCli>(&manual);
    }

    #[test]
    fn task_manual_examples_parse() {
        let manual = task_manual();
        assert_examples_parse::<TaskPluginCli>(&manual);
    }

    fn assert_examples_parse<T>(manual: &PluginManual)
    where
        T: Parser + CommandFactory,
    {
        for command in &manual.commands {
            for example in &command.examples {
                let mut args = Vec::with_capacity(example.argv.len() + 1);
                args.push(manual.domain.clone());
                args.extend(example.argv.iter().cloned());
                let parse_result = T::try_parse_from(args.clone());
                assert!(
                    parse_result.is_ok(),
                    "manual example failed to parse for domain '{}', command '{}': argv={:?}",
                    manual.domain,
                    command.name,
                    args
                );
            }
        }
    }
}
