use std::path::{Path, PathBuf};

use ah_runtime::core;

use crate::error::AppError;

pub(crate) struct GitIo {
    cwd: PathBuf,
}

impl GitIo {
    pub(crate) fn current() -> Result<Self, AppError> {
        let cwd =
            std::env::current_dir().map_err(|source| AppError::cwd(PathBuf::from("."), source))?;
        Ok(Self { cwd })
    }

    pub(crate) fn at(cwd: &Path) -> Self {
        Self {
            cwd: cwd.to_path_buf(),
        }
    }

    pub(crate) fn resolve_path(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.cwd.join(path)
        }
    }

    pub(crate) fn read_output<I, S>(&self, args: I) -> Result<String, AppError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let command_args = collect_args(args);
        let printable = format!("git {}", command_args.join(" "));
        let output = core::run_command_in_dir("git", &command_args, &self.cwd)
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

    pub(crate) fn read_output_bytes<I, S>(&self, args: I) -> Result<Vec<u8>, AppError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let command_args = collect_args(args);
        let printable = format!("git {}", command_args.join(" "));
        let output = core::run_command_in_dir("git", &command_args, &self.cwd)
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

    pub(crate) fn read_trimmed<I, S>(&self, args: I) -> Option<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let command_args = collect_args(args);
        let output = core::run_command_in_dir("git", &command_args, &self.cwd).ok()?;
        if !output.status.success() {
            return None;
        }
        let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if value.is_empty() { None } else { Some(value) }
    }

    pub(crate) fn is_inside_repo(&self) -> Result<bool, AppError> {
        let output =
            core::run_command_in_dir("git", ["rev-parse", "--is-inside-work-tree"], &self.cwd)
                .map_err(|source| {
                    AppError::command_execution("git rev-parse --is-inside-work-tree", source)
                })?;
        if !output.status.success() {
            return Ok(false);
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim() == "true")
    }
}

fn collect_args<I, S>(args: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    args.into_iter()
        .map(|value| value.as_ref().to_owned())
        .collect()
}
