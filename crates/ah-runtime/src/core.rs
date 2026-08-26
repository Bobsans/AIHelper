use std::{ffi::OsStr, path::Path, process::Output};

pub fn apply_limit<T>(items: &mut Vec<T>, limit: Option<usize>) -> bool {
    if let Some(limit_value) = limit
        && items.len() > limit_value
    {
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

/// A path for JSON output, with separators forward and any Windows verbatim
/// prefix removed, so `\\?\C:\x` reads as `C:/x`.
pub fn normalize_path(path: &Path) -> String {
    let normalized = forward_slashes(path);
    if let Some(path) = normalized.strip_prefix("//?/UNC/") {
        format!("//{path}")
    } else if let Some(path) = normalized.strip_prefix("//?/") {
        path.to_owned()
    } else {
        normalized
    }
}

/// A path for JSON output with separators forward and nothing else changed.
///
/// Distinct from [`normalize_path`] on purpose: these are paths whose published
/// form predates the verbatim-prefix handling, and rewriting them would change
/// output that callers already parse.
pub fn forward_slashes(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

pub fn run_command<I, S>(program: &str, args: I) -> std::io::Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = ah_plugin_api::noninteractive_command(program);
    for value in args {
        command.arg(value.as_ref());
    }
    command.output()
}

pub fn run_command_in_dir<I, S>(
    program: &str,
    args: I,
    current_dir: &Path,
) -> std::io::Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = ah_plugin_api::noninteractive_command(program);
    command.current_dir(current_dir);
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
    run_command(program, args)
        .map(|output| output.status.success())
        .unwrap_or(false)
}
