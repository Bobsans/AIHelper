use crate::{cli::GlobalOptions, error::AppError};
use clap::Args;

const DEFAULT_TIMEOUT_SECS: u64 = 600;
const DEFAULT_MAX_OUTPUT_BYTES: usize = 64 * 1024;

#[derive(Debug, Args)]
pub struct RunArgs {
    #[command(subcommand)]
    pub command: RunCommand,
}

#[derive(Debug, clap::Subcommand)]
pub enum RunCommand {
    #[command(about = "Run a command and return agent-friendly result")]
    Check(CheckArgs),
}

#[derive(Debug, Args)]
pub struct CheckArgs {
    #[arg(long, default_value_t = DEFAULT_TIMEOUT_SECS, value_name = "SECONDS")]
    pub timeout_secs: u64,
    #[arg(long, default_value_t = DEFAULT_MAX_OUTPUT_BYTES, value_name = "BYTES")]
    pub max_output_bytes: usize,
    #[arg(long, value_name = "N")]
    pub tail_lines: Option<usize>,
    #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
    pub command: Vec<String>,
}

pub(crate) mod io;
pub(crate) mod output;

mod adapters {
    pub(crate) use super::io;
    pub(crate) use super::output;
}

mod domain;

pub fn execute(args: RunArgs, options: &GlobalOptions) -> Result<(), AppError> {
    match args.command {
        RunCommand::Check(check_args) => {
            let result = domain::run_check(check_args)?;
            adapters::output::emit_check_result(result, options)
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn resolves_extensionless_program_from_path_with_pathext_order() {
        let temp_dir = tempfile::tempdir().expect("temp dir should be created");
        let shim = temp_dir.path().join("npx.CMD");
        fs::write(&shim, "@echo off\r\n").expect("shim should be written");

        let resolved = resolve_windows_program_in(
            "npx",
            None,
            Some(&[temp_dir.path().to_path_buf()]),
            &[".EXE".to_owned(), ".CMD".to_owned()],
        )
        .expect("npx should resolve through PATHEXT");

        assert_eq!(resolved, shim);
    }

    #[test]
    fn does_not_rewrite_programs_that_already_have_an_extension() {
        assert!(resolve_windows_program("npx.cmd").is_none());
    }
}

#[cfg(windows)]
#[allow(dead_code)]
pub(crate) fn resolve_windows_program_in(
    program: &str,
    current_dir: Option<&std::path::Path>,
    path_dirs: Option<&[std::path::PathBuf]>,
    path_exts: &[String],
) -> Option<std::path::PathBuf> {
    adapters::io::resolve_windows_program_in(program, current_dir, path_dirs, path_exts)
}

#[cfg(windows)]
#[allow(dead_code)]
pub(crate) fn resolve_windows_program(program: &str) -> Option<std::path::PathBuf> {
    adapters::io::resolve_windows_program(program)
}
