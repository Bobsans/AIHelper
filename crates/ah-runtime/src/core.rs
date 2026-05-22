use std::{
    ffi::OsStr,
    path::Path,
    process::{Command, Output},
};

pub fn apply_limit<T>(items: &mut Vec<T>, limit: Option<usize>) -> bool {
    if let Some(limit_value) = limit && items.len() > limit_value {
        items.truncate(limit_value);
        return true;
    }
    false
}

pub fn truncate_lines(content: &str, limit: Option<usize>) -> (String, bool) {
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

pub fn normalize_path(path: &Path) -> String {
    let normalized = path.to_string_lossy().replace('\\', "/");
    if let Some(path) = normalized.strip_prefix("//?/UNC/") {
        format!("//{path}")
    } else if let Some(path) = normalized.strip_prefix("//?/") {
        path.to_owned()
    } else {
        normalized
    }
}

pub fn run_command<I, S>(program: &str, args: I) -> std::io::Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new(program);
    for value in args {
        command.arg(value.as_ref());
    }
    command.output()
}

pub fn run_command_ok<I, S>(program: &str, args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run_command(program, args).map(|output| output.status.success()).unwrap_or(false)
}

pub fn run_shell_command(command: &str) -> std::io::Result<Output> {
    if cfg!(target_os = "windows") {
        run_command("powershell", ["-NoProfile", "-Command", command])
    } else {
        run_command("sh", ["-lc", command])
    }
}
