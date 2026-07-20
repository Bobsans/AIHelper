use std::{
    io,
    path::{Path, PathBuf},
    process::Output,
};

use ah_plugin_api::ErrorDiagnostic;
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
        let output = self.run_git(&command_args, &printable)?;

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
        let output = self.run_git(&command_args, &printable)?;
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
        let printable = format!("git {}", command_args.join(" "));
        let output = self.run_git(&command_args, &printable).ok()?;
        if !output.status.success() {
            return None;
        }
        let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if value.is_empty() { None } else { Some(value) }
    }

    pub(crate) fn is_inside_repo(&self) -> Result<bool, AppError> {
        let args = ["rev-parse".to_owned(), "--is-inside-work-tree".to_owned()];
        let output = self.run_git(&args, "git rev-parse --is-inside-work-tree")?;
        if !output.status.success() {
            return Ok(false);
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim() == "true")
    }

    pub(crate) fn read_status_snapshot(&self) -> Result<Option<Vec<u8>>, AppError> {
        let args = [
            "status".to_owned(),
            "--porcelain=v2".to_owned(),
            "--branch".to_owned(),
            "-z".to_owned(),
        ];
        let output = self.run_git(&args, "git status --porcelain=v2 --branch -z")?;
        if output.status.success() {
            Ok(Some(output.stdout))
        } else {
            Ok(None)
        }
    }

    fn run_git(&self, args: &[String], printable: &str) -> Result<Output, AppError> {
        core::run_command_in_dir("git", args, &self.cwd)
            .map_err(|source| map_git_spawn_error(args, printable, source))
    }
}

fn map_git_spawn_error(args: &[String], printable: &str, source: io::Error) -> AppError {
    if source.kind() == io::ErrorKind::NotFound {
        return AppError::from_diagnostic(ErrorDiagnostic::new(
            Some("git".to_owned()),
            args.first().map(|command| format!("git.{command}")),
            "DEPENDENCY_MISSING",
            "required external tool not found: git",
            "local git commands require the git executable on PATH",
            1,
        ));
    }
    AppError::command_execution(printable.to_owned(), source)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_git_preserves_dependency_diagnostic() {
        let error = map_git_spawn_error(
            &["status".to_owned()],
            "git status",
            io::Error::new(io::ErrorKind::NotFound, "missing git"),
        );

        assert_eq!(error.code(), "DEPENDENCY_MISSING");
        assert!(error.detail_message().contains("git executable on PATH"));
    }
}
