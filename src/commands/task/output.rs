use crate::{cli::GlobalOptions, error::AppError, output::OutputMode};

use crate::commands::task::domain::{TaskListOutput, TaskResult, TaskRunOutput, TaskSaveOutput};

pub(crate) fn emit(result: TaskResult, options: &GlobalOptions) -> Result<(), AppError> {
    if options.quiet {
        return Ok(());
    }

    match result {
        TaskResult::Save(payload) => emit_save(payload, options),
        TaskResult::List(payload) => emit_list(payload, options),
        TaskResult::Run(payload) => emit_run(payload, options),
    }
}

fn emit_save(payload: TaskSaveOutput, options: &GlobalOptions) -> Result<(), AppError> {
    match options.output {
        OutputMode::Text => {
            println!("saved task '{}' -> {}", payload.name, payload.task_command);
            Ok(())
        }
        OutputMode::Json => {
            println!("{}", serde_json::to_string_pretty(&payload)?);
            Ok(())
        }
    }
}

fn emit_list(payload: TaskListOutput, options: &GlobalOptions) -> Result<(), AppError> {
    match options.output {
        OutputMode::Text => {
            if payload.tasks.is_empty() {
                println!("no tasks saved");
                return Ok(());
            }
            for task in &payload.tasks {
                println!("{} => {}", task.name, task.command);
            }
            if payload.truncated {
                eprintln!("warning: output truncated by --limit");
            }
            Ok(())
        }
        OutputMode::Json => {
            println!("{}", serde_json::to_string_pretty(&payload)?);
            Ok(())
        }
    }
}

fn emit_run(payload: TaskRunOutput, options: &GlobalOptions) -> Result<(), AppError> {
    match options.output {
        OutputMode::Text => {
            if !payload.stdout.is_empty() {
                print!("{}", payload.stdout);
            }
            if !payload.stderr.is_empty() {
                eprint!("{}", payload.stderr);
            }
            if payload.truncated {
                eprintln!("warning: output truncated by --limit");
            }
            Ok(())
        }
        OutputMode::Json => {
            println!("{}", serde_json::to_string_pretty(&payload)?);
            Ok(())
        }
    }
}
