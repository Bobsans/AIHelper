use std::{
    path::Path,
    time::Duration,
};

use serde::{Deserialize, Serialize};

use crate::error::AppError;
use ah_runtime::core::{apply_limit, normalize_path, truncate_lines};

use super::{TaskArgs, TaskCommand, adapters};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct TaskEntry {
    pub(crate) name: String,
    pub(crate) command: String,
    pub(crate) updated_unix_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct TaskStore {
    version: u32,
    tasks: Vec<TaskEntry>,
}

#[derive(Debug, Clone)]
pub(crate) enum TaskResult {
    Save(TaskSaveOutput),
    List(TaskListOutput),
    Run(TaskRunOutput),
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskSaveOutput {
    pub command: &'static str,
    pub name: String,
    pub task_command: String,
    pub store_path: String,
    pub updated_unix_seconds: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskListOutput {
    pub command: &'static str,
    pub store_path: String,
    pub count: usize,
    pub truncated: bool,
    pub tasks: Vec<TaskEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskRunOutput {
    pub command: &'static str,
    pub name: String,
    pub task_command: String,
    pub exit_code: i32,
    pub success: bool,
    pub truncated: bool,
    pub stdout: String,
    pub stderr: String,
}

impl Default for TaskStore {
    fn default() -> Self {
        Self {
            version: 1,
            tasks: Vec::new(),
        }
    }
}

pub(crate) fn execute(args: TaskArgs, limit: Option<usize>) -> Result<TaskResult, AppError> {
    match args.command {
        TaskCommand::Save(save_args) => Ok(TaskResult::Save(run_save(save_args)?)),
        TaskCommand::List(_list_args) => Ok(TaskResult::List(run_list(_list_args, limit)?)),
        TaskCommand::Run(run_args) => Ok(TaskResult::Run(run_run(run_args, limit)?)),
    }
}

fn run_save(args: super::SaveArgs) -> Result<TaskSaveOutput, AppError> {
    validation::validate_task_name(&args.name)?;
    let store_path = task_store_path(args.cwd.as_deref());
    let updated_unix_seconds = crate::persistence::transaction(&store_path, || {
        let mut store = adapters::io::load_store(&store_path)?;
        let updated_unix_seconds = adapters::io::now_unix_seconds();

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
        adapters::io::save_store(&store_path, &store)?;
        Ok(updated_unix_seconds)
    })?;

    Ok(TaskSaveOutput {
        command: "task.save",
        name: args.name,
        task_command: args.command,
        store_path: normalize_path(&store_path),
        updated_unix_seconds,
    })
}

fn run_list(_args: super::ListArgs, limit: Option<usize>) -> Result<TaskListOutput, AppError> {
    let store_path = task_store_path(_args.cwd.as_deref());
    let mut store = adapters::io::load_store(&store_path)?;
    store
        .tasks
        .sort_by(|left, right| left.name.cmp(&right.name));
    let mut tasks = store.tasks;
    let truncated = apply_limit(&mut tasks, limit);

    Ok(TaskListOutput {
        command: "task.list",
        store_path: normalize_path(&store_path),
        count: tasks.len(),
        truncated,
        tasks,
    })
}

fn run_run(args: super::RunArgs, limit: Option<usize>) -> Result<TaskRunOutput, AppError> {
    validation::validate_task_name(&args.name)?;
    if args.timeout_secs == 0 {
        return Err(AppError::invalid_argument("--timeout-secs must be >= 1"));
    }
    if args.max_output_bytes == 0 {
        return Err(AppError::invalid_argument(
            "--max-output-bytes must be >= 1",
        ));
    }

    let store = adapters::io::load_store(&task_store_path(args.cwd.as_deref()))?;
    let task = store
        .tasks
        .into_iter()
        .find(|entry| entry.name == args.name)
        .ok_or_else(|| AppError::invalid_argument(format!("task not found: {}", args.name)))?;

    let (program, command_args) = adapters::io::shell_command(&task.command);
    let output = crate::commands::run::io::run_command(
        &program,
        &command_args,
        crate::commands::run::io::RunCommandOptions {
            timeout: args
                .timeout_ms
                .map(Duration::from_millis)
                .unwrap_or_else(|| Duration::from_secs(args.timeout_secs.max(1))),
            command_label: &format!("task {}", args.name),
            max_output_bytes: args.max_output_bytes,
            tail_lines: None,
            cwd: args.cwd.as_deref(),
            cancelled: super::current_request_cancelled,
        },
    )?;
    let stdout_raw = crate::commands::run::io::render_output(&output.stdout, None);
    let stderr_raw = crate::commands::run::io::render_output(&output.stderr, None);
    let (stdout, stdout_truncated) = truncate_lines(&stdout_raw, limit);
    let (stderr, stderr_truncated) = truncate_lines(&stderr_raw, limit);
    let truncated =
        output.stdout.truncated || output.stderr.truncated || stdout_truncated || stderr_truncated;

    if output.timed_out {
        return Err(AppError::external(
            "TASK_TIMEOUT",
            format!(
                "task '{}' did not complete within {} seconds",
                args.name, args.timeout_secs
            ),
        ));
    }

    if output.exit_code != Some(0) {
        let stderr_message = if stderr.trim().is_empty() {
            match output.exit_code {
                Some(code) => format!("task '{}' failed with exit code {code}", args.name),
                None => format!("task '{}' terminated without an exit code", args.name),
            }
        } else {
            stderr.trim().to_owned()
        };
        return Err(AppError::command_failed(
            format!("task {}", args.name),
            output.exit_code,
            stderr_message,
        ));
    }

    Ok(TaskRunOutput {
        command: "task.run",
        name: args.name,
        task_command: task.command,
        exit_code: output.exit_code.unwrap_or(0),
        success: true,
        truncated,
        stdout,
        stderr,
    })
}

fn task_store_path(cwd: Option<&Path>) -> std::path::PathBuf {
    match cwd {
        Some(cwd) => adapters::io::task_store_path_at(cwd),
        None => adapters::io::task_store_path(),
    }
}

mod validation {
    use crate::error::AppError;

    pub(super) fn validate_task_name(name: &str) -> Result<(), AppError> {
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
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};

    use super::*;

    #[test]
    fn concurrent_task_saves_preserve_unrelated_entries() {
        let directory = tempfile::tempdir().unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let mut handles = Vec::new();
        for (name, command) in [("first", "echo first"), ("second", "echo second")] {
            let cwd = directory.path().to_path_buf();
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                run_save(super::super::SaveArgs {
                    name: name.to_owned(),
                    command: command.to_owned(),
                    cwd: Some(cwd),
                })
                .unwrap();
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }

        let store = adapters::io::load_store(&task_store_path(Some(directory.path()))).unwrap();
        assert_eq!(store.tasks.len(), 2);
        assert_eq!(store.tasks[0].name, "first");
        assert_eq!(store.tasks[1].name, "second");
    }
}
