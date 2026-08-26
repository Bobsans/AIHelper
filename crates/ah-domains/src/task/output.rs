use ah_error::AppError;
use ah_output::{Emitter, TextFormatter, TextStyle};

use crate::task::domain::{TaskEntry, TaskListOutput, TaskResult, TaskRunOutput, TaskSaveOutput};

const TRUNCATED: &str = "output truncated by --limit";

pub(crate) fn emit(result: TaskResult, emitter: &mut Emitter) -> Result<(), AppError> {
    match result {
        TaskResult::Save(payload) => emit_save(payload, emitter),
        TaskResult::List(payload) => emit_list(payload, emitter),
        TaskResult::Run(payload) => emit_run(payload, emitter),
    }
}

fn emit_save(payload: TaskSaveOutput, emitter: &mut Emitter) -> Result<(), AppError> {
    emitter.value(&payload, |formatter| {
        render_saved_task(&payload.name, &payload.task_command, formatter)
    })
}

fn emit_list(payload: TaskListOutput, emitter: &mut Emitter) -> Result<(), AppError> {
    emitter.value(&payload, |formatter| {
        if payload.tasks.is_empty() {
            return formatter.paint(TextStyle::Muted, "no tasks saved");
        }
        payload
            .tasks
            .iter()
            .map(|task| render_task_entry(task, formatter))
            .collect::<Vec<_>>()
            .join("\n")
    })?;
    if !payload.tasks.is_empty() && payload.truncated {
        emitter.text_warning(TRUNCATED);
    }
    Ok(())
}

/// The task's own stdout and stderr are passed through byte for byte, so they
/// stay usable by whatever the caller pipes them into.
fn emit_run(payload: TaskRunOutput, emitter: &mut Emitter) -> Result<(), AppError> {
    if emitter.is_text() {
        emitter.raw(&payload.stdout)?;
        emitter.raw_err(&payload.stderr);
    } else {
        emitter.value(&payload, |_| String::new())?;
    }
    if payload.truncated {
        emitter.text_warning(TRUNCATED);
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::{render_saved_task, render_task_entry};
    use ah_output::TextFormatter;

    use crate::task::domain::TaskEntry;

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
