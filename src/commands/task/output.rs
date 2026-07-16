use crate::{
    cli::GlobalOptions,
    error::AppError,
    output::{OutputMode, TextFormatter, TextStyle, emit_warning},
};

use crate::commands::task::domain::{
    TaskEntry, TaskListOutput, TaskResult, TaskRunOutput, TaskSaveOutput,
};

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
            println!(
                "{}",
                render_saved_task(
                    &payload.name,
                    &payload.task_command,
                    TextFormatter::stdout()
                )
            );
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
                println!(
                    "{}",
                    TextFormatter::stdout().paint(TextStyle::Muted, "no tasks saved")
                );
                return Ok(());
            }
            let formatter = TextFormatter::stdout();
            for task in &payload.tasks {
                println!("{}", render_task_entry(task, formatter));
            }
            if payload.truncated {
                emit_warning("output truncated by --limit");
            }
            Ok(())
        }
        OutputMode::Json => {
            println!("{}", serde_json::to_string_pretty(&payload)?);
            Ok(())
        }
    }
}

fn render_saved_task(name: &str, command: &str, formatter: TextFormatter) -> String {
    format!(
        "{} '{}' {} {}",
        formatter.paint(TextStyle::Success, "saved task"),
        formatter.paint(TextStyle::Key, name),
        formatter.paint(TextStyle::Muted, "->"),
        formatter.paint(TextStyle::Muted, command)
    )
}

fn render_task_entry(task: &TaskEntry, formatter: TextFormatter) -> String {
    format!(
        "{} {} {}",
        formatter.paint(TextStyle::Key, &task.name),
        formatter.paint(TextStyle::Muted, "=>"),
        formatter.paint(TextStyle::Muted, &task.command)
    )
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
                emit_warning("output truncated by --limit");
            }
            Ok(())
        }
        OutputMode::Json => {
            println!("{}", serde_json::to_string_pretty(&payload)?);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{render_saved_task, render_task_entry};
    use crate::{commands::task::domain::TaskEntry, output::TextFormatter};

    #[test]
    fn task_renderers_preserve_plain_contract() {
        let formatter = TextFormatter::with_color(false);
        let task = TaskEntry {
            name: "test".to_owned(),
            command: "cargo test".to_owned(),
            updated_unix_seconds: 1,
        };

        assert_eq!(
            render_saved_task("test", "cargo test", formatter),
            "saved task 'test' -> cargo test"
        );
        assert_eq!(render_task_entry(&task, formatter), "test => cargo test");
    }

    #[test]
    fn task_renderers_apply_semantic_styles() {
        let rendered = render_saved_task("test", "cargo test", TextFormatter::with_color(true));

        assert!(rendered.contains("\u{1b}[32msaved task\u{1b}[0m"));
        assert!(rendered.contains("\u{1b}[36mtest\u{1b}[0m"));
        assert!(rendered.contains("\u{1b}[2mcargo test\u{1b}[0m"));
    }
}
