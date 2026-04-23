use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::{SystemTime, UNIX_EPOCH},
};

use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};

use crate::{cli::GlobalOptions, error::AppError, output::OutputMode};

const TASKS_DIR: &str = ".ah";
const TASKS_FILE: &str = "tasks.json";

#[derive(Debug, Args)]
pub struct TaskArgs {
    #[command(subcommand)]
    pub command: TaskCommand,
}

#[derive(Debug, Subcommand)]
pub enum TaskCommand {
    Save(SaveArgs),
    Run(RunArgs),
    List(ListArgs),
}

#[derive(Debug, Args)]
pub struct SaveArgs {
    pub name: String,
    pub command: String,
}

#[derive(Debug, Args)]
pub struct RunArgs {
    pub name: String,
}

#[derive(Debug, Args)]
pub struct ListArgs {}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TaskEntry {
    name: String,
    command: String,
    updated_unix_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TaskStore {
    version: u32,
    tasks: Vec<TaskEntry>,
}

impl Default for TaskStore {
    fn default() -> Self {
        Self {
            version: 1,
            tasks: Vec::new(),
        }
    }
}

#[derive(Debug, Serialize)]
struct TaskSaveOutput {
    command: &'static str,
    name: String,
    task_command: String,
    store_path: String,
    updated_unix_seconds: u64,
}

#[derive(Debug, Serialize)]
struct TaskListOutput {
    command: &'static str,
    store_path: String,
    count: usize,
    truncated: bool,
    tasks: Vec<TaskEntry>,
}

#[derive(Debug, Serialize)]
struct TaskRunOutput {
    command: &'static str,
    name: String,
    task_command: String,
    exit_code: i32,
    success: bool,
    truncated: bool,
    stdout: String,
    stderr: String,
}

pub fn execute(args: TaskArgs, options: &GlobalOptions) -> Result<(), AppError> {
    match args.command {
        TaskCommand::Save(save_args) => execute_save(save_args, options),
        TaskCommand::Run(run_args) => execute_run(run_args, options),
        TaskCommand::List(list_args) => execute_list(list_args, options),
    }
}

fn execute_save(args: SaveArgs, options: &GlobalOptions) -> Result<(), AppError> {
    validate_task_name(&args.name)?;

    let store_path = task_store_path();
    let mut store = load_store(&store_path)?;
    let updated_unix_seconds = now_unix_seconds();

    if let Some(existing) = store.tasks.iter_mut().find(|task| task.name == args.name) {
        existing.command = args.command.clone();
        existing.updated_unix_seconds = updated_unix_seconds;
    } else {
        store.tasks.push(TaskEntry {
            name: args.name.clone(),
            command: args.command.clone(),
            updated_unix_seconds,
        });
    }
    store
        .tasks
        .sort_by(|left, right| left.name.cmp(&right.name));
    save_store(&store_path, &store)?;

    if options.quiet {
        return Ok(());
    }

    match options.output {
        OutputMode::Text => {
            println!("saved task '{}' -> {}", args.name, args.command);
        }
        OutputMode::Json => {
            let payload = TaskSaveOutput {
                command: "task.save",
                name: args.name,
                task_command: args.command,
                store_path: normalize_path(&store_path),
                updated_unix_seconds,
            };
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
    }

    Ok(())
}

fn execute_list(_args: ListArgs, options: &GlobalOptions) -> Result<(), AppError> {
    let store_path = task_store_path();
    let mut store = load_store(&store_path)?;
    store
        .tasks
        .sort_by(|left, right| left.name.cmp(&right.name));
    let mut tasks = store.tasks;
    let truncated = apply_limit(&mut tasks, options.limit);

    if options.quiet {
        return Ok(());
    }

    match options.output {
        OutputMode::Text => {
            if tasks.is_empty() {
                println!("no tasks saved");
                return Ok(());
            }
            for task in &tasks {
                println!("{} => {}", task.name, task.command);
            }
            if truncated {
                eprintln!("warning: output truncated by --limit");
            }
        }
        OutputMode::Json => {
            let payload = TaskListOutput {
                command: "task.list",
                store_path: normalize_path(&store_path),
                count: tasks.len(),
                truncated,
                tasks,
            };
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
    }

    Ok(())
}

fn execute_run(args: RunArgs, options: &GlobalOptions) -> Result<(), AppError> {
    validate_task_name(&args.name)?;

    let store_path = task_store_path();
    let store = load_store(&store_path)?;
    let task = store
        .tasks
        .into_iter()
        .find(|entry| entry.name == args.name)
        .ok_or_else(|| AppError::invalid_argument(format!("task not found: {}", args.name)))?;

    let output = run_shell_command(&task.command)?;
    let success = output.status.success();
    let exit_code = output.status.code().unwrap_or_default();
    let stdout_raw = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr_raw = String::from_utf8_lossy(&output.stderr).into_owned();
    let (stdout, stdout_truncated) = truncate_lines(&stdout_raw, options.limit);
    let (stderr, stderr_truncated) = truncate_lines(&stderr_raw, options.limit);
    let truncated = stdout_truncated || stderr_truncated;

    if !success {
        let stderr_message = if stderr.trim().is_empty() {
            format!("task '{}' failed with exit code {}", args.name, exit_code)
        } else {
            stderr.trim().to_owned()
        };
        return Err(AppError::command_failed(
            format!("task {}", args.name),
            Some(exit_code),
            stderr_message,
        ));
    }

    if options.quiet {
        return Ok(());
    }

    match options.output {
        OutputMode::Text => {
            if !stdout.is_empty() {
                print!("{stdout}");
            }
            if !stderr.is_empty() {
                eprint!("{stderr}");
            }
            if truncated {
                eprintln!("warning: output truncated by --limit");
            }
        }
        OutputMode::Json => {
            let payload = TaskRunOutput {
                command: "task.run",
                name: args.name,
                task_command: task.command,
                exit_code,
                success,
                truncated,
                stdout,
                stderr,
            };
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
    }

    Ok(())
}

fn task_store_path() -> PathBuf {
    PathBuf::from(TASKS_DIR).join(TASKS_FILE)
}

fn load_store(path: &Path) -> Result<TaskStore, AppError> {
    if !path.exists() {
        return Ok(TaskStore::default());
    }

    let raw = fs::read_to_string(path)
        .map_err(|source| AppError::file_read(path.to_path_buf(), source))?;
    if raw.trim().is_empty() {
        return Ok(TaskStore::default());
    }
    let store: TaskStore = serde_json::from_str(&raw)
        .map_err(|source| AppError::json_deserialization(path.to_path_buf(), source))?;
    Ok(store)
}

fn save_store(path: &Path, store: &TaskStore) -> Result<(), AppError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|source| AppError::file_write(parent.to_path_buf(), source))?;
    }
    let raw = serde_json::to_string_pretty(store)?;
    fs::write(path, raw).map_err(|source| AppError::file_write(path.to_path_buf(), source))
}

fn run_shell_command(command: &str) -> Result<Output, AppError> {
    if cfg!(target_os = "windows") {
        Command::new("powershell")
            .args(["-NoProfile", "-Command", command])
            .output()
            .map_err(|source| {
                AppError::command_execution(format!("powershell -Command {command}"), source)
            })
    } else {
        Command::new("sh")
            .args(["-lc", command])
            .output()
            .map_err(|source| AppError::command_execution(format!("sh -lc {command}"), source))
    }
}

fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0)
}

fn validate_task_name(name: &str) -> Result<(), AppError> {
    if name.is_empty() {
        return Err(AppError::invalid_argument("task name must not be empty"));
    }
    if !name
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.')
    {
        return Err(AppError::invalid_argument(
            "task name may contain only letters, numbers, '-', '_' and '.'",
        ));
    }
    Ok(())
}

fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn apply_limit<T>(items: &mut Vec<T>, limit: Option<usize>) -> bool {
    if let Some(limit_value) = limit {
        if items.len() > limit_value {
            items.truncate(limit_value);
            return true;
        }
    }
    false
}

fn truncate_lines(content: &str, limit: Option<usize>) -> (String, bool) {
    let Some(limit_value) = limit else {
        return (content.to_owned(), false);
    };

    let mut lines: Vec<&str> = content.lines().collect();
    if lines.len() > limit_value {
        lines.truncate(limit_value);
        let mut truncated = lines.join("\n");
        if content.ends_with('\n') {
            truncated.push('\n');
        }
        return (truncated, true);
    }

    (content.to_owned(), false)
}
