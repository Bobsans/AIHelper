use ah_runtime::core;

use crate::error::AppError;

pub(crate) fn read_git_output<I, S>(args: I) -> Result<String, AppError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let command_args: Vec<String> = args
        .into_iter()
        .map(|value| value.as_ref().to_owned())
        .collect();
    let printable = format!("git {}", command_args.join(" "));
    let output = core::run_command("git", &command_args)
        .map_err(|source| AppError::command_execution(printable.clone(), source))?;

    if !output.status.success() {
        return Err(AppError::command_failed(
            printable,
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub(crate) fn read_git_output_bytes<I, S>(args: I) -> Result<Vec<u8>, AppError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let command_args: Vec<String> = args
        .into_iter()
        .map(|value| value.as_ref().to_owned())
        .collect();
    let printable = format!("git {}", command_args.join(" "));
    let output = core::run_command("git", &command_args)
        .map_err(|source| AppError::command_execution(printable.clone(), source))?;
    if !output.status.success() {
        return Err(AppError::command_failed(
            printable,
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    Ok(output.stdout)
}

pub(crate) fn read_git_trimmed<I, S>(args: I) -> Option<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let command_args: Vec<String> = args
        .into_iter()
        .map(|value| value.as_ref().to_owned())
        .collect();
    let output = core::run_command("git", &command_args).ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if value.is_empty() { None } else { Some(value) }
}

pub(crate) fn is_inside_git_repo() -> Result<bool, AppError> {
    let output =
        core::run_command("git", ["rev-parse", "--is-inside-work-tree"]).map_err(|source| {
            AppError::command_execution("git rev-parse --is-inside-work-tree", source)
        })?;
    if !output.status.success() {
        return Ok(false);
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim() == "true")
}
