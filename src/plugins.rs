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
        Arc::new(TaskBuiltinPlugin),
    ]
}

struct FileBuiltinPlugin;
struct SearchBuiltinPlugin;
struct CtxBuiltinPlugin;
struct GitBuiltinPlugin;
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

fn task_metadata() -> PluginMetadata {
    PluginMetadata {
        plugin_name: "builtin-task".to_owned(),
        domain: "task".to_owned(),
        description: "Task recipe plugin (built-in)".to_owned(),
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
                summary: "Extract symbols from file or directory.".to_owned(),
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
        ],
        notes: vec!["Useful for quick change-attribution in AI review loops.".to_owned()],
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
