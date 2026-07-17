use std::{
    collections::VecDeque,
    io::{self, ErrorKind, Read},
    path::PathBuf,
    process::{Command, Stdio},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use command_group::CommandGroup;

#[cfg(windows)]
use std::env;
#[cfg(windows)]
use std::path::Path;

use crate::error::AppError;

#[derive(Debug)]
pub(crate) struct CapturedOutput {
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub duration_ms: u128,
    pub stdout: CapturedStream,
    pub stderr: CapturedStream,
}

#[derive(Debug)]
pub(crate) struct CapturedStream {
    pub bytes: Vec<u8>,
    pub truncated: bool,
}

pub(crate) struct RunCommandOptions<'a> {
    pub timeout: Duration,
    pub command_label: &'a str,
    pub max_output_bytes: usize,
    pub tail_lines: Option<usize>,
    pub cwd: Option<&'a std::path::Path>,
    pub cancelled: fn() -> bool,
}

pub(crate) fn run_command(
    program: &str,
    args: &[String],
    options: RunCommandOptions<'_>,
) -> Result<CapturedOutput, AppError> {
    let RunCommandOptions {
        timeout,
        command_label,
        max_output_bytes,
        tail_lines,
        cwd,
        cancelled,
    } = options;
    let started = Instant::now();
    let spawn_program = resolve_program_for_spawn(program, cwd);
    let mut command = Command::new(&spawn_program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let mut child = command
        .group_spawn()
        .map_err(|source| AppError::command_execution(command_label.to_owned(), source))?;

    let stdout_handle = child.inner().stdout.take().map(|stdout| {
        thread::spawn(move || capture_reader(stdout, max_output_bytes, tail_lines.is_some()))
    });
    let stderr_handle = child.inner().stderr.take().map(|stderr| {
        thread::spawn(move || capture_reader(stderr, max_output_bytes, tail_lines.is_some()))
    });

    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|source| AppError::command_execution(command_label.to_owned(), source))?
        {
            break status;
        }

        if started.elapsed() >= timeout || cancelled() {
            timed_out = started.elapsed() >= timeout;
            if let Err(source) = child.kill()
                && source.kind() != ErrorKind::InvalidInput
            {
                return Err(AppError::command_execution(
                    format!("{command_label} (terminate process group)"),
                    source,
                ));
            }
            break child
                .wait()
                .map_err(|source| AppError::command_execution(command_label.to_owned(), source))?;
        }

        thread::sleep(Duration::from_millis(25));
    };

    let stdout = join_output_reader(stdout_handle, command_label)?;
    let stderr = join_output_reader(stderr_handle, command_label)?;

    Ok(CapturedOutput {
        exit_code: status.code(),
        timed_out,
        duration_ms: started.elapsed().as_millis(),
        stdout,
        stderr,
    })
}

pub(crate) fn render_output(stream: &CapturedStream, tail_lines: Option<usize>) -> String {
    let text = String::from_utf8_lossy(&stream.bytes).into_owned();
    let Some(tail) = tail_lines else {
        return text;
    };
    if tail == 0 {
        return String::new();
    }
    let lines = text.lines().collect::<Vec<_>>();
    if lines.len() <= tail {
        text
    } else {
        lines[lines.len() - tail..].join("\n")
    }
}

fn capture_reader<R: Read>(
    mut reader: R,
    max_output_bytes: usize,
    keep_tail: bool,
) -> io::Result<CapturedStream> {
    let mut prefix = Vec::with_capacity(max_output_bytes.min(8192));
    let mut suffix = VecDeque::with_capacity(max_output_bytes.min(8192));
    let mut buffer = [0_u8; 8192];
    let mut seen_bytes = 0usize;

    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        seen_bytes = seen_bytes.saturating_add(read);
        if keep_tail {
            for byte in &buffer[..read] {
                if suffix.len() == max_output_bytes {
                    suffix.pop_front();
                }
                suffix.push_back(*byte);
            }
        } else if prefix.len() < max_output_bytes {
            let remaining = max_output_bytes - prefix.len();
            prefix.extend_from_slice(&buffer[..read.min(remaining)]);
        }
    }

    Ok(CapturedStream {
        bytes: if keep_tail {
            suffix.into_iter().collect()
        } else {
            prefix
        },
        truncated: seen_bytes > max_output_bytes,
    })
}

fn resolve_program_for_spawn(program: &str, cwd: Option<&std::path::Path>) -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(resolved) = resolve_windows_program_from(program, cwd) {
            return resolved;
        }
    }

    let path = PathBuf::from(program);
    if path.is_relative()
        && (program.contains('/') || program.contains('\\'))
        && let Some(cwd) = cwd
    {
        return cwd.join(path);
    }
    path
}

#[cfg(windows)]
pub(crate) fn resolve_windows_program(program: &str) -> Option<PathBuf> {
    resolve_windows_program_from(program, env::current_dir().ok().as_deref())
}

#[cfg(windows)]
fn resolve_windows_program_from(program: &str, current_dir: Option<&Path>) -> Option<PathBuf> {
    let original = Path::new(program);
    if original.extension().is_some() {
        return None;
    }
    let path_exts = path_ext_candidates();
    if has_path_separator(program) {
        return find_existing_with_extensions(original, &path_exts);
    }

    let path_dirs = env::var_os("PATH").map(|paths| env::split_paths(&paths).collect::<Vec<_>>());
    resolve_windows_program_in(program, current_dir, path_dirs.as_deref(), &path_exts)
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
    handle: Option<JoinHandle<io::Result<CapturedStream>>>,
    command_label: &str,
) -> Result<CapturedStream, AppError> {
    let Some(handle) = handle else {
        return Ok(CapturedStream {
            bytes: Vec::new(),
            truncated: false,
        });
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

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    #[test]
    fn capture_reader_keeps_bounded_prefix() {
        let captured = capture_reader(Cursor::new(b"abcdef"), 4, false).expect("capture");
        assert_eq!(captured.bytes, b"abcd");
        assert!(captured.truncated);
    }

    #[test]
    fn capture_reader_keeps_bounded_suffix() {
        let captured = capture_reader(Cursor::new(b"abcdef"), 4, true).expect("capture");
        assert_eq!(captured.bytes, b"cdef");
        assert!(captured.truncated);
    }

    #[test]
    fn render_output_selects_tail_lines() {
        let captured =
            capture_reader(Cursor::new(b"one\ntwo\nthree\n"), 64, true).expect("capture");
        assert_eq!(render_output(&captured, Some(2)), "two\nthree");
        assert_eq!(render_output(&captured, Some(0)), "");
    }
}
