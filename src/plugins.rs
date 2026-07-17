use std::sync::Arc;

use ah_plugin_api::{
    CommandCatalog, GlobalOptionsWire, InvocationRequest, InvocationResponse, ManualCommand,
    ManualExample, PluginCompatibility, PluginManual, PluginMetadata, RequiredTool,
    TypedInvocationRequest, TypedInvocationResponse, normalize_invocation_argv,
    plugin_capabilities,
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
        required_tools: Vec::new(),
        compatibility: PluginCompatibility::current()
            .with_capability(plugin_capabilities::TYPED_COMMANDS_V1),
    }
}

fn search_metadata() -> PluginMetadata {
    PluginMetadata {
        plugin_name: "builtin-search".to_owned(),
        domain: "search".to_owned(),
        description: "Search operations plugin (built-in)".to_owned(),
        abi_version: 1,
        required_tools: Vec::new(),
        compatibility: PluginCompatibility::current()
            .with_capability(plugin_capabilities::TYPED_COMMANDS_V1),
    }
}

fn ctx_metadata() -> PluginMetadata {
    PluginMetadata {
        plugin_name: "builtin-ctx".to_owned(),
        domain: "ctx".to_owned(),
        description: "Context utilities plugin (built-in)".to_owned(),
        abi_version: 1,
        required_tools: Vec::new(),
        compatibility: PluginCompatibility::current()
            .with_capability(plugin_capabilities::TYPED_COMMANDS_V1),
    }
}

fn git_metadata() -> PluginMetadata {
    PluginMetadata {
        plugin_name: "builtin-git".to_owned(),
        domain: "git".to_owned(),
        description: "Git utilities plugin (built-in)".to_owned(),
        abi_version: 1,
        required_tools: vec![git_required_tool()],
        compatibility: PluginCompatibility::current()
            .with_capability(plugin_capabilities::TYPED_COMMANDS_V1),
    }
}

fn project_metadata() -> PluginMetadata {
    PluginMetadata {
        plugin_name: "builtin-project".to_owned(),
        domain: "project".to_owned(),
        description: "Project detection plugin (built-in)".to_owned(),
        abi_version: 1,
        required_tools: Vec::new(),
        compatibility: PluginCompatibility::current()
            .with_capability(plugin_capabilities::TYPED_COMMANDS_V1),
    }
}

fn run_metadata() -> PluginMetadata {
    PluginMetadata {
        plugin_name: "builtin-run".to_owned(),
        domain: "run".to_owned(),
        description: "Command execution check plugin (built-in)".to_owned(),
        abi_version: 1,
        required_tools: Vec::new(),
        compatibility: PluginCompatibility::current()
            .with_capability(plugin_capabilities::TYPED_COMMANDS_V1),
    }
}

fn task_metadata() -> PluginMetadata {
    PluginMetadata {
        plugin_name: "builtin-task".to_owned(),
        domain: "task".to_owned(),
        description: "Task recipe plugin (built-in)".to_owned(),
        abi_version: 1,
        required_tools: Vec::new(),
        compatibility: PluginCompatibility::current()
            .with_capability(plugin_capabilities::TYPED_COMMANDS_V1),
    }
}

fn http_metadata() -> PluginMetadata {
    PluginMetadata {
        plugin_name: "builtin-http".to_owned(),
        domain: "http".to_owned(),
        description: "HTTP workflow plugin (built-in)".to_owned(),
        abi_version: 1,
        required_tools: Vec::new(),
        compatibility: PluginCompatibility::current()
            .with_capability(plugin_capabilities::TYPED_COMMANDS_V1),
    }
}

fn git_required_tool() -> RequiredTool {
    RequiredTool::new(
        "git",
        "local git commands require the git executable on PATH",
    )
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
                usage: "text <pattern> [path...] [--glob ...] [--ignore-case] [--context N] [--regex] [--max-bytes BYTES] [--follow-symlinks]".to_owned(),
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
                usage: "files <query> [path...] [--follow-symlinks]".to_owned(),
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
                usage: "run <name> [--timeout-secs SECONDS] [--max-output-bytes BYTES]".to_owned(),
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
        let (parsed, options) =
            match parse_args::<FilePluginCli>("file", &request.argv, request.globals.clone()) {
                ParseOutcome::Parsed(value, options) => (value, options),
                ParseOutcome::Response(response) => return response,
            };
        map_execute("file", commands::file::execute(parsed.args, &options))
    }

    fn command_catalog(&self) -> Option<CommandCatalog> {
        Some(commands::file::command_catalog())
    }

    fn invoke_typed(&self, request: &TypedInvocationRequest) -> TypedInvocationResponse {
        commands::file::invoke_typed(request)
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
        let (parsed, options) =
            match parse_args::<SearchPluginCli>("search", &request.argv, request.globals.clone()) {
                ParseOutcome::Parsed(value, options) => (value, options),
                ParseOutcome::Response(response) => return response,
            };
        map_execute("search", commands::search::execute(parsed.args, &options))
    }

    fn command_catalog(&self) -> Option<CommandCatalog> {
        Some(commands::search::command_catalog())
    }

    fn invoke_typed(&self, request: &TypedInvocationRequest) -> TypedInvocationResponse {
        commands::search::invoke_typed(request)
    }

    fn cancel_typed(&self, request_id: &str) -> bool {
        commands::search::cancel_typed(request_id)
    }
}

impl BuiltinPlugin for CtxBuiltinPlugin {
    fn metadata(&self) -> PluginMetadata {
        ctx_metadata()
    }

    fn manual(&self) -> PluginManual {
        ctx_manual()
    }

    fn required_tools(&self, request: &InvocationRequest) -> Vec<RequiredTool> {
        if request_command(request).as_deref() == Some("changed") {
            vec![git_required_tool()]
        } else {
            Vec::new()
        }
    }

    fn command_catalog(&self) -> Option<CommandCatalog> {
        Some(commands::ctx::command_catalog())
    }

    fn required_tools_typed(&self, request: &TypedInvocationRequest) -> Vec<RequiredTool> {
        if request.command == "ctx.changed" {
            vec![git_required_tool()]
        } else {
            Vec::new()
        }
    }

    fn invoke_typed(&self, request: &TypedInvocationRequest) -> TypedInvocationResponse {
        commands::ctx::invoke_typed(request)
    }

    fn invoke(&self, request: &InvocationRequest) -> InvocationResponse {
        let (parsed, options) =
            match parse_args::<CtxPluginCli>("ctx", &request.argv, request.globals.clone()) {
                ParseOutcome::Parsed(value, options) => (value, options),
                ParseOutcome::Response(response) => return response,
            };
        map_execute("ctx", commands::ctx::execute(parsed.args, &options))
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
        let (parsed, options) =
            match parse_args::<GitPluginCli>("git", &request.argv, request.globals.clone()) {
                ParseOutcome::Parsed(value, options) => (value, options),
                ParseOutcome::Response(response) => return response,
            };
        map_execute("git", commands::git::execute(parsed.args, &options))
    }

    fn command_catalog(&self) -> Option<CommandCatalog> {
        Some(commands::git::command_catalog())
    }

    fn invoke_typed(&self, request: &TypedInvocationRequest) -> TypedInvocationResponse {
        commands::git::invoke_typed(request)
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
        let (parsed, options) =
            match parse_args::<ProjectPluginCli>("project", &request.argv, request.globals.clone())
            {
                ParseOutcome::Parsed(value, options) => (value, options),
                ParseOutcome::Response(response) => return response,
            };
        map_execute("project", commands::project::execute(parsed.args, &options))
    }

    fn command_catalog(&self) -> Option<CommandCatalog> {
        Some(commands::project::command_catalog())
    }

    fn invoke_typed(&self, request: &TypedInvocationRequest) -> TypedInvocationResponse {
        commands::project::invoke_typed(request)
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
        let (parsed, options) =
            match parse_args::<RunPluginCli>("run", &request.argv, request.globals.clone()) {
                ParseOutcome::Parsed(value, options) => (value, options),
                ParseOutcome::Response(response) => return response,
            };
        map_execute("run", commands::run::execute(parsed.args, &options))
    }

    fn command_catalog(&self) -> Option<CommandCatalog> {
        Some(commands::run::command_catalog())
    }

    fn invoke_typed(&self, request: &TypedInvocationRequest) -> TypedInvocationResponse {
        commands::run::invoke_typed(request)
    }

    fn cancel_typed(&self, request_id: &str) -> bool {
        commands::run::cancel_typed(request_id)
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
        let (parsed, options) =
            match parse_args::<HttpPluginCli>("http", &request.argv, request.globals.clone()) {
                ParseOutcome::Parsed(value, options) => (value, options),
                ParseOutcome::Response(response) => return response,
            };
        map_execute("http", commands::http::execute(parsed.args, &options))
    }

    fn command_catalog(&self) -> Option<CommandCatalog> {
        Some(commands::http::command_catalog())
    }

    fn invoke_typed(&self, request: &TypedInvocationRequest) -> TypedInvocationResponse {
        commands::http::invoke_typed(request)
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
        let (parsed, options) =
            match parse_args::<TaskPluginCli>("task", &request.argv, request.globals.clone()) {
                ParseOutcome::Parsed(value, options) => (value, options),
                ParseOutcome::Response(response) => return response,
            };
        map_execute("task", commands::task::execute(parsed.args, &options))
    }

    fn command_catalog(&self) -> Option<CommandCatalog> {
        Some(commands::task::command_catalog())
    }

    fn invoke_typed(&self, request: &TypedInvocationRequest) -> TypedInvocationResponse {
        commands::task::invoke_typed(request)
    }

    fn cancel_typed(&self, request_id: &str) -> bool {
        commands::task::cancel_typed(request_id)
    }
}

enum ParseOutcome<T> {
    Parsed(T, GlobalOptions),
    Response(InvocationResponse),
}

fn request_command(request: &InvocationRequest) -> Option<String> {
    normalize_invocation_argv(&request.argv, request.globals.clone())
        .ok()
        .and_then(|normalized| normalized.argv.into_iter().next())
}

fn parse_args<T: Parser + CommandFactory>(
    domain: &str,
    argv: &[String],
    globals: GlobalOptionsWire,
) -> ParseOutcome<T> {
    let normalized = match normalize_invocation_argv(argv, globals) {
        Ok(value) => value,
        Err(error) => return ParseOutcome::Response(error.with_error_domain(domain)),
    };

    let options = GlobalOptions::from(normalized.globals);
    let mut args = Vec::with_capacity(argv.len() + 1);
    args.push(domain.to_owned());
    args.extend(normalized.argv);

    match T::try_parse_from(args) {
        Ok(value) => ParseOutcome::Parsed(value, options),
        Err(error) => {
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) {
                ParseOutcome::Response(InvocationResponse::ok(Some(error.to_string())))
            } else {
                ParseOutcome::Response(
                    InvocationResponse::error("INVALID_ARGUMENT", error.to_string())
                        .with_error_domain(domain),
                )
            }
        }
    }
}

fn map_execute(domain: &str, result: Result<(), AppError>) -> InvocationResponse {
    match result {
        Ok(()) => InvocationResponse::ok(None),
        Err(error) => InvocationResponse::error_diagnostic(error.diagnostic().with_domain(domain)),
    }
}

#[cfg(test)]
mod tests {
    use ah_runtime::PluginManager;
    use clap::{CommandFactory, Parser};

    use super::*;

    #[test]
    fn builtin_catalogs_use_mcp_compatible_input_roots() {
        const ALLOWED_ROOT_KEYWORDS: &[&str] = &[
            "$schema",
            "$id",
            "$defs",
            "definitions",
            "title",
            "description",
            "default",
            "examples",
            "deprecated",
            "readOnly",
            "writeOnly",
            "type",
            "properties",
            "required",
            "additionalProperties",
        ];
        let mut manager = PluginManager::new();
        for plugin in builtins() {
            manager.register_builtin(plugin);
        }

        let commands = manager
            .list_enabled_commands()
            .expect("built-in typed catalogs should compile");

        assert!(!commands.is_empty());
        for command in commands {
            let root = command
                .descriptor
                .input_schema
                .as_object()
                .expect("input schema should have an object root");
            assert!(
                root.keys()
                    .all(|keyword| ALLOWED_ROOT_KEYWORDS.contains(&keyword.as_str())),
                "{} has an incompatible input root",
                command.descriptor.id
            );
        }
    }

    #[test]
    fn file_manual_examples_parse() {
        let manual = file_manual();
        assert_examples_parse::<FilePluginCli>(&manual);
    }

    #[test]
    fn file_typed_read_uses_context_cwd_and_limit() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("sample.txt"), "alpha\nbeta\ngamma\n").unwrap();
        let mut manager = PluginManager::new();
        let file = builtins()
            .into_iter()
            .find(|plugin| plugin.metadata().domain == "file")
            .unwrap();
        manager.register_builtin(file);
        let response = manager
            .invoke_typed(&TypedInvocationRequest::new(
                "file.read",
                serde_json::json!({"path": "sample.txt", "number_lines": true}),
                ah_plugin_api::ExecutionContextWire::new(
                    "file-test",
                    temp.path().to_string_lossy(),
                    Some(2),
                    1_000,
                ),
            ))
            .unwrap();
        assert!(response.success);
        let data = response.data.unwrap();
        assert_eq!(data["command"], "file.read");
        assert_eq!(data["line_count"], 2);
        assert_eq!(data["truncated"], true);
        assert_eq!(data["content"], "   1: alpha\n   2: beta");
    }

    #[test]
    fn file_typed_read_returns_structured_range_error() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("sample.txt"), "alpha\n").unwrap();
        let mut manager = PluginManager::new();
        let file = builtins()
            .into_iter()
            .find(|plugin| plugin.metadata().domain == "file")
            .unwrap();
        manager.register_builtin(file);
        let response = manager
            .invoke_typed(&TypedInvocationRequest::new(
                "file.read",
                serde_json::json!({"path": "sample.txt", "from": 3, "to": 2}),
                ah_plugin_api::ExecutionContextWire::new(
                    "file-test-error",
                    temp.path().to_string_lossy(),
                    None,
                    1_000,
                ),
            ))
            .unwrap();
        assert!(!response.success);
        let error = response.error.unwrap();
        assert_eq!(error.domain.as_deref(), Some("file"));
        assert_eq!(error.operation.as_deref(), Some("file.read"));
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
    fn ctx_typed_symbols_uses_explicit_context_cwd() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("sample.rs"), "fn sample() {}\n").unwrap();
        let mut manager = PluginManager::new();
        let ctx = builtins()
            .into_iter()
            .find(|plugin| plugin.metadata().domain == "ctx")
            .unwrap();
        manager.register_builtin(ctx);
        let response = manager
            .invoke_typed(&TypedInvocationRequest::new(
                "ctx.symbols",
                serde_json::json!({"path": "sample.rs", "preset": "summary"}),
                ah_plugin_api::ExecutionContextWire::new(
                    "ctx-test",
                    temp.path().to_string_lossy(),
                    None,
                    1_000,
                ),
            ))
            .unwrap();
        assert!(response.success);
        let data = response.data.unwrap();
        assert_eq!(data["command"], "ctx.symbols");
        assert_eq!(data["symbol_count"], 1);
        assert!(data["root"].as_str().unwrap().ends_with("sample.rs"));
    }

    #[test]
    fn ctx_typed_symbols_rejects_missing_path() {
        let mut manager = PluginManager::new();
        let ctx = builtins()
            .into_iter()
            .find(|plugin| plugin.metadata().domain == "ctx")
            .unwrap();
        manager.register_builtin(ctx);
        let error = manager
            .invoke_typed(&TypedInvocationRequest::new(
                "ctx.symbols",
                serde_json::json!({}),
                ah_plugin_api::ExecutionContextWire::new("ctx-test-invalid", ".", None, 1_000),
            ))
            .unwrap_err();
        assert!(matches!(
            error,
            ah_runtime::RuntimeError::TypedInvocation(_)
        ));
    }

    #[test]
    fn git_manual_examples_parse() {
        let manual = git_manual();
        assert_examples_parse::<GitPluginCli>(&manual);
    }

    #[test]
    fn git_typed_status_uses_explicit_context_cwd() {
        if !git_is_available() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let mut manager = PluginManager::new();
        let git = builtins()
            .into_iter()
            .find(|plugin| plugin.metadata().domain == "git")
            .unwrap();
        manager.register_builtin(git);
        let response = manager
            .invoke_typed(&TypedInvocationRequest::new(
                "git.status",
                serde_json::json!({}),
                ah_plugin_api::ExecutionContextWire::new(
                    "git-status-test",
                    temp.path().to_string_lossy(),
                    None,
                    2_000,
                ),
            ))
            .unwrap();
        assert!(response.success);
        let data = response.data.unwrap();
        assert_eq!(data["command"], "git.status");
        assert_eq!(data["in_git_repo"], false);
    }

    #[test]
    fn git_typed_tag_create_mutates_context_repository() {
        if !git_is_available() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        run_git(temp.path(), &["init"]);
        run_git(temp.path(), &["config", "user.email", "test@example.com"]);
        run_git(temp.path(), &["config", "user.name", "Test User"]);
        std::fs::write(temp.path().join("sample.txt"), "sample\n").unwrap();
        run_git(temp.path(), &["add", "sample.txt"]);
        run_git(temp.path(), &["commit", "-m", "initial"]);

        let mut manager = PluginManager::new();
        let git = builtins()
            .into_iter()
            .find(|plugin| plugin.metadata().domain == "git")
            .unwrap();
        manager.register_builtin(git);
        let response = manager
            .invoke_typed(&TypedInvocationRequest::new(
                "git.tag.create",
                serde_json::json!({"tag": "v-test", "message": "test tag"}),
                ah_plugin_api::ExecutionContextWire::new(
                    "git-tag-test",
                    temp.path().to_string_lossy(),
                    None,
                    2_000,
                ),
            ))
            .unwrap();
        assert!(response.success);
        let data = response.data.unwrap();
        assert_eq!(data["tag"], "v-test");
        assert_eq!(data["annotated"], true);
        let output = std::process::Command::new("git")
            .current_dir(temp.path())
            .args(["tag", "--list", "v-test"])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "v-test");
    }

    #[test]
    fn project_manual_examples_parse() {
        let manual = project_manual();
        assert_examples_parse::<ProjectPluginCli>(&manual);
    }

    #[test]
    fn project_typed_detect_uses_explicit_context_cwd() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("Cargo.toml"),
            "[package]\nname = \"sample\"\nversion = \"1.2.3\"\n",
        )
        .unwrap();
        let mut manager = PluginManager::new();
        let project = builtins()
            .into_iter()
            .find(|plugin| plugin.metadata().domain == "project")
            .unwrap();
        manager.register_builtin(project);
        let response = manager
            .invoke_typed(&TypedInvocationRequest::new(
                "project.detect",
                serde_json::json!({}),
                ah_plugin_api::ExecutionContextWire::new(
                    "project-detect-test",
                    temp.path().to_string_lossy(),
                    None,
                    2_000,
                ),
            ))
            .unwrap();
        assert!(response.success);
        let data = response.data.unwrap();
        assert_eq!(data["command"], "project.detect");
        assert!(
            data["ecosystems"]
                .as_array()
                .unwrap()
                .iter()
                .any(|value| value == "rust")
        );
        assert!(
            data["versions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|value| value["version"] == "1.2.3")
        );
    }

    #[test]
    fn run_manual_examples_parse() {
        let manual = run_manual();
        assert_examples_parse::<RunPluginCli>(&manual);
    }

    #[test]
    fn run_typed_check_runs_in_explicit_context_cwd() {
        let temp = tempfile::tempdir().unwrap();
        let command = if cfg!(windows) {
            serde_json::json!(["cmd", "/C", "cd"])
        } else {
            serde_json::json!(["pwd"])
        };
        let mut manager = PluginManager::new();
        let run = builtins()
            .into_iter()
            .find(|plugin| plugin.metadata().domain == "run")
            .unwrap();
        manager.register_builtin(run);
        let response = manager
            .invoke_typed(&TypedInvocationRequest::new(
                "run.check",
                serde_json::json!({"command": command}),
                ah_plugin_api::ExecutionContextWire::new(
                    "run-check-test",
                    temp.path().to_string_lossy(),
                    None,
                    5_000,
                ),
            ))
            .unwrap();
        assert!(response.success);
        let data = response.data.unwrap();
        assert_eq!(data["success"], true);
        let reported = std::path::PathBuf::from(data["stdout"].as_str().unwrap().trim());
        assert_eq!(
            std::fs::canonicalize(reported).unwrap(),
            std::fs::canonicalize(temp.path()).unwrap()
        );
    }

    #[test]
    fn search_typed_text_uses_explicit_context_cwd_and_limit() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("sample.txt"),
            "needle one\nother\nneedle two\n",
        )
        .unwrap();
        let mut manager = PluginManager::new();
        let search = builtins()
            .into_iter()
            .find(|plugin| plugin.metadata().domain == "search")
            .unwrap();
        manager.register_builtin(search);
        let response = manager
            .invoke_typed(&TypedInvocationRequest::new(
                "search.text",
                serde_json::json!({"pattern": "needle"}),
                ah_plugin_api::ExecutionContextWire::new(
                    "search-text-test",
                    temp.path().to_string_lossy(),
                    Some(1),
                    2_000,
                ),
            ))
            .unwrap();
        assert!(response.success);
        let data = response.data.unwrap();
        assert_eq!(data["command"], "search.text");
        assert_eq!(data["match_count"], 1);
        assert_eq!(data["truncated"], true);
        assert_eq!(data["matches"][0]["path"], "sample.txt");
    }

    #[test]
    fn http_manual_examples_parse() {
        let manual = http_manual();
        assert_examples_parse::<HttpPluginCli>(&manual);
    }

    #[test]
    fn http_typed_get_returns_valid_structured_response() {
        let (url, server) = serve_http_once(200, r#"{"status":"ok"}"#);
        let mut manager = PluginManager::new();
        let http = builtins()
            .into_iter()
            .find(|plugin| plugin.metadata().domain == "http")
            .unwrap();
        manager.register_builtin(http);
        let response = manager
            .invoke_typed(&TypedInvocationRequest::new(
                "http.get",
                serde_json::json!({
                    "url": url,
                    "expect_status": "200",
                    "expect_json": ["status:eq:ok"]
                }),
                ah_plugin_api::ExecutionContextWire::new("http-get-test", ".", None, 2_000),
            ))
            .unwrap();
        server.join().unwrap();
        assert!(response.success);
        let data = response.data.unwrap();
        assert_eq!(data["command"], "http.get");
        assert_eq!(data["status"], 200);
        assert_eq!(data["ok"], true);
    }

    #[test]
    fn http_typed_expectation_failure_is_structured_error() {
        let (url, server) = serve_http_once(200, "ok");
        let mut manager = PluginManager::new();
        let http = builtins()
            .into_iter()
            .find(|plugin| plugin.metadata().domain == "http")
            .unwrap();
        manager.register_builtin(http);
        let response = manager
            .invoke_typed(&TypedInvocationRequest::new(
                "http.get",
                serde_json::json!({"url": url, "expect_status": "201"}),
                ah_plugin_api::ExecutionContextWire::new("http-get-error-test", ".", None, 2_000),
            ))
            .unwrap();
        server.join().unwrap();
        assert!(!response.success);
        assert_eq!(response.error.unwrap().code, "HTTP_ASSERTION_FAILED");
    }

    #[test]
    fn task_manual_examples_parse() {
        let manual = task_manual();
        assert_examples_parse::<TaskPluginCli>(&manual);
    }

    #[test]
    fn task_typed_save_and_list_use_explicit_context_cwd() {
        let temp = tempfile::tempdir().unwrap();
        let mut manager = PluginManager::new();
        let task = builtins()
            .into_iter()
            .find(|plugin| plugin.metadata().domain == "task")
            .unwrap();
        manager.register_builtin(task);
        let context = || {
            ah_plugin_api::ExecutionContextWire::new(
                "task-test",
                temp.path().to_string_lossy(),
                None,
                2_000,
            )
        };
        let saved = manager
            .invoke_typed(&TypedInvocationRequest::new(
                "task.save",
                serde_json::json!({"name": "check", "command": "cargo check"}),
                context(),
            ))
            .unwrap();
        assert!(saved.success);
        assert!(temp.path().join(".ah").join("tasks.json").is_file());

        let listed = manager
            .invoke_typed(&TypedInvocationRequest::new(
                "task.list",
                serde_json::json!({}),
                context(),
            ))
            .unwrap();
        assert!(listed.success);
        let data = listed.data.unwrap();
        assert_eq!(data["count"], 1);
        assert_eq!(data["tasks"][0]["name"], "check");
        assert_eq!(data["tasks"][0]["command"], "cargo check");
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

    fn git_is_available() -> bool {
        std::process::Command::new("git")
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success())
    }

    fn run_git(cwd: &std::path::Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .current_dir(cwd)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {} failed", args.join(" "));
    }

    fn serve_http_once(status: u16, body: &'static str) -> (String, std::thread::JoinHandle<()>) {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 4096];
            let _ = stream.read(&mut request);
            let reason = if status == 200 { "OK" } else { "Error" };
            let response = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
        });
        (format!("http://{address}/test"), server)
    }
}
