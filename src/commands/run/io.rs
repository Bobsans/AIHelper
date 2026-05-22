use std::{
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[cfg(windows)]
use std::env;

use crate::error::AppError;

#[derive(Debug)]
pub(crate) struct CapturedOutput {
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub duration_ms: u128,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

pub(crate) fn run_command(
    program: &str,
    args: &[String],
    timeout_secs: u64,
    command_label: &str,
) -> Result<CapturedOutput, AppError> {
    let started = Instant::now();
    let spawn_program = resolve_program_for_spawn(program);
    let mut child = Command::new(&spawn_program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| AppError::command_execution(command_label.to_owned(), source))?;

    let stdout_handle = child.stdout.take().map(|mut stdout| {
        thread::spawn(move || {
            let mut output = Vec::new();
            stdout.read_to_end(&mut output)?;
            Ok::<Vec<u8>, io::Error>(output)
        })
    });
    let stderr_handle = child.stderr.take().map(|mut stderr| {
        thread::spawn(move || {
            let mut output = Vec::new();
            stderr.read_to_end(&mut output)?;
            Ok::<Vec<u8>, io::Error>(output)
        })
    });

    let timeout = Duration::from_secs(timeout_secs.max(1));
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|source| AppError::command_execution(command_label.to_owned(), source))?
        {
            break status;
        }

        if started.elapsed() >= timeout {
            timed_out = true;
            let _ = child.kill();
            break child
                .wait()
                .map_err(|source| AppError::command_execution(command_label.to_owned(), source))?;
        }

        thread::sleep(Duration::from_millis(25));
    };

    let stdout_raw = join_output_reader(stdout_handle, command_label)?;
    let stderr_raw = join_output_reader(stderr_handle, command_label)?;

    Ok(CapturedOutput {
        exit_code: status.code(),
        timed_out,
        duration_ms: started.elapsed().as_millis(),
        stdout: stdout_raw,
        stderr: stderr_raw,
    })
}

pub(crate) fn prepare_output(
    raw: &[u8],
    max_output_bytes: usize,
    tail_lines: Option<usize>,
) -> (String, bool) {
    let mut text = String::from_utf8_lossy(raw).into_owned();
    if let Some(tail) = tail_lines {
        let lines = text.lines().collect::<Vec<_>>();
        if lines.len() > tail {
            text = lines[lines.len() - tail..].join("\n");
        }
    }
    let bytes = text.as_bytes();
    if bytes.len() <= max_output_bytes {
        return (text, false);
    }
    let mut end = max_output_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_owned(), true)
}

fn resolve_program_for_spawn(program: &str) -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(resolved) = resolve_windows_program(program) {
            return resolved;
        }
    }

    PathBuf::from(program)
}

#[cfg(windows)]
pub(crate) fn resolve_windows_program(program: &str) -> Option<PathBuf> {
    let original = Path::new(program);
    if original.extension().is_some() {
        return None;
    }
    let path_exts = path_ext_candidates();
    if has_path_separator(program) {
        return find_existing_with_extensions(original, &path_exts);
    }

    let current_dir = env::current_dir().ok();
    let path_dirs = env::var_os("PATH").map(|paths| env::split_paths(&paths).collect::<Vec<_>>());
    resolve_windows_program_in(
        program,
        current_dir.as_deref(),
        path_dirs.as_deref(),
        &path_exts,
    )
}

#[cfg(not(windows))]
pub(crate) fn resolve_windows_program(_program: &str) -> Option<PathBuf> {
    None
}

#[cfg(windows)]
pub(crate) fn resolve_windows_program_in(
    program: &str,
    current_dir: Option<&Path>,
    path_dirs: Option<&[PathBuf]>,
    path_exts: &[String],
) -> Option<PathBuf> {
    if let Some(current_dir) = current_dir {
        let candidate = current_dir.join(program);
        if let Some(resolved) = find_existing_with_extensions(&candidate, path_exts) {
            return Some(resolved);
        }
    }

    for dir in path_dirs.unwrap_or_default() {
        let candidate = dir.join(program);
        if let Some(resolved) = find_existing_with_extensions(&candidate, path_exts) {
            return Some(resolved);
        }
    }

    None
}

#[cfg(windows)]
fn find_existing_with_extensions(candidate: &Path, path_exts: &[String]) -> Option<PathBuf> {
    for extension in path_exts {
        let mut extended = candidate.to_path_buf();
        extended.set_extension(extension.trim_start_matches('.'));
        if extended.is_file() {
            return Some(extended);
        }
    }

    None
}

#[cfg(windows)]
fn path_ext_candidates() -> Vec<String> {
    env::var_os("PATHEXT")
        .map(|raw| {
            raw.to_string_lossy()
                .split(';')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| {
                    if value.starts_with('.') {
                        value.to_owned()
                    } else {
                        format!(".{value}")
                    }
                })
                .collect::<Vec<_>>()
        })
        .filter(|values| !values.is_empty())
        .unwrap_or_else(|| {
            [".COM", ".EXE", ".BAT", ".CMD"]
                .into_iter()
                .map(str::to_owned)
                .collect()
        })
}

#[cfg(windows)]
fn has_path_separator(program: &str) -> bool {
    program.contains('\\') || program.contains('/')
}

fn join_output_reader(
    handle: Option<JoinHandle<io::Result<Vec<u8>>>>,
    command_label: &str,
) -> Result<Vec<u8>, AppError> {
    let Some(handle) = handle else {
        return Ok(Vec::new());
    };
    handle
        .join()
        .map_err(|_| {
            AppError::external(
                "COMMAND_OUTPUT_CAPTURE_FAILED",
                format!("failed to capture command output: {command_label}"),
            )
        })?
        .map_err(|source| AppError::command_execution(command_label.to_owned(), source))
}
