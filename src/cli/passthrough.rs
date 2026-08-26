//! Arguments this process must read before clap can: the working directory,
//! and the `run check` tail that belongs to the child process rather than to
//! us.

use super::*;

/// The request directory named by `--cwd`, resolved against the process
/// directory so that later consumers never have to.
///
/// This used to call `std::env::set_current_dir`, which made the answer a
/// property of the process rather than of the request - and `mcp serve` runs
/// requests in parallel. The directory is now read here and carried.
///
/// # Errors
///
/// [`AppError`] when `--cwd` ends argv with no value, or names something that
/// is not a readable directory.
pub fn initial_cwd_from_raw_args(raw_args: &[OsString]) -> Result<Option<PathBuf>, AppError> {
    let Some(cwd) = extract_last_cwd(raw_args)? else {
        return Ok(None);
    };
    resolve_request_dir(cwd).map(Some)
}

/// Turn a `--cwd` value into the absolute directory consumers resolve against.
///
/// One rule, used by both the early resolution and the full parse, so the two
/// cannot disagree about what `--cwd` meant.
///
/// # Errors
///
/// [`AppError`] when the path is not a readable directory. The `chdir` this
/// replaces failed the same way, so a missing `--cwd` still fails rather than
/// being silently ignored.
pub fn resolve_request_dir(cwd: PathBuf) -> Result<PathBuf, AppError> {
    // Absolute, so a consumer joining a relative path onto it cannot fall back
    // to the process directory without saying so.
    let resolved = cwd
        .canonicalize()
        .map_err(|source| AppError::cwd(cwd.clone(), source))?;
    if !resolved.is_dir() {
        return Err(AppError::invalid_argument(format!(
            "--cwd is not a directory: {}",
            cwd.display()
        )));
    }
    Ok(resolved)
}

#[derive(Debug, Clone, Copy)]
pub(super) struct RunCheckLayout {
    pub(super) domain_index: usize,
    pub(super) prefix_end: usize,
    pub(super) child_start: usize,
}

pub(super) fn prepare_run_check_passthrough(
    raw_args: &mut [OsString],
) -> Result<Option<Vec<String>>, AppError> {
    let Some(layout) = run_check_layout(raw_args) else {
        return Ok(None);
    };

    let mut argv = os_args_to_strings(&raw_args[(layout.domain_index + 1)..layout.prefix_end])?;
    argv.push("--".to_owned());
    argv.extend(os_args_to_strings(&raw_args[layout.child_start..])?);

    for (offset, value) in raw_args[layout.child_start..].iter_mut().enumerate() {
        *value = OsString::from(format!("__ah_opaque_child_arg_{offset}__"));
    }

    Ok(Some(argv))
}

pub(super) fn os_args_to_strings(values: &[OsString]) -> Result<Vec<String>, AppError> {
    values
        .iter()
        .map(|value| {
            value.to_str().map(str::to_owned).ok_or_else(|| {
                AppError::invalid_argument("external subcommand contains non-UTF8 argument")
            })
        })
        .collect()
}

pub(super) fn run_check_layout(raw_args: &[OsString]) -> Option<RunCheckLayout> {
    let mut index = 1usize;
    while index < raw_args.len() {
        if let Some(next) = host_option_end(raw_args, index) {
            index = next;
            continue;
        }
        break;
    }
    if raw_args.get(index)? != "run" {
        return None;
    }
    let domain_index = index;
    index += 1;

    while index < raw_args.len() {
        if let Some(next) = host_option_end(raw_args, index) {
            index = next;
            continue;
        }
        break;
    }
    if raw_args.get(index)? != "check" {
        return None;
    }
    index += 1;

    while index < raw_args.len() {
        if raw_args[index] == "--" {
            return Some(RunCheckLayout {
                domain_index,
                prefix_end: index,
                child_start: index + 1,
            });
        }
        if let Some(next) =
            host_option_end(raw_args, index).or_else(|| run_check_option_end(raw_args, index))
        {
            index = next;
            continue;
        }
        return Some(RunCheckLayout {
            domain_index,
            prefix_end: index,
            child_start: index,
        });
    }

    None
}

pub(super) fn host_option_end(raw_args: &[OsString], index: usize) -> Option<usize> {
    let value = raw_args.get(index)?.to_str()?;
    match value {
        "--json" | "--quiet" => Some(index + 1),
        "--cwd" | "--limit" => Some((index + 2).min(raw_args.len())),
        _ if value.starts_with("--cwd=") || value.starts_with("--limit=") => Some(index + 1),
        _ => None,
    }
}

pub(super) fn run_check_option_end(raw_args: &[OsString], index: usize) -> Option<usize> {
    let value = raw_args.get(index)?.to_str()?;
    match value {
        "--timeout-secs" | "--max-output-bytes" | "--tail-lines" => {
            Some((index + 2).min(raw_args.len()))
        }
        _ if value.starts_with("--timeout-secs=")
            || value.starts_with("--max-output-bytes=")
            || value.starts_with("--tail-lines=") =>
        {
            Some(index + 1)
        }
        _ => None,
    }
}

pub(super) fn extract_last_cwd(raw_args: &[OsString]) -> Result<Option<PathBuf>, AppError> {
    let mut cwd = None;
    let mut index = 1usize;
    let scan_end = run_check_layout(raw_args)
        .map(|layout| layout.prefix_end)
        .unwrap_or(raw_args.len());
    while index < scan_end {
        let arg = &raw_args[index];
        if arg == "--" {
            break;
        }
        if arg == "--cwd" {
            let Some(value) = raw_args.get(index + 1).filter(|_| index + 1 < scan_end) else {
                return Err(AppError::invalid_argument(
                    "missing value for trailing --cwd",
                ));
            };
            cwd = Some(PathBuf::from(value));
            index += 2;
            continue;
        }
        if let Some(value) = arg.to_str().and_then(|raw| raw.strip_prefix("--cwd=")) {
            cwd = Some(PathBuf::from(value));
        }
        index += 1;
    }
    Ok(cwd)
}
