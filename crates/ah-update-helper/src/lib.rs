#![deny(unsafe_op_in_unsafe_fn)]
// Off Windows this crate compiles but cannot run: the update handoff is a
// Windows mechanism, and every entry point refuses before it reaches the
// helpers below - see `bounded_process`'s stub. Those helpers being unreachable
// there is the design rather than an oversight, so the two lints that report it
// are relaxed for that build only. On Windows, where the code is live, both
// still apply.
#![cfg_attr(not(windows), allow(dead_code, unused_imports))]

pub mod activation_command;
#[cfg(windows)]
mod bounded_process;
#[cfg(not(windows))]
mod bounded_process {
    use std::{ffi::OsStr, io, path::Path, process::ExitStatus, time::Duration};

    pub(crate) struct EnvironmentOverride<'a> {
        pub(crate) name: &'a OsStr,
        pub(crate) value: Option<&'a OsStr>,
    }

    pub(crate) struct Output {
        pub(crate) status: ExitStatus,
        pub(crate) timed_out: bool,
        pub(crate) stdout: Vec<u8>,
        pub(crate) stderr: Vec<u8>,
        pub(crate) stdout_truncated: bool,
        pub(crate) stderr_truncated: bool,
    }

    pub(crate) fn run(
        _program: &Path,
        _arguments: &[&str],
        _cwd: &Path,
        environment: &[EnvironmentOverride<'_>],
        _timeout: Duration,
        _maximum_output: usize,
    ) -> io::Result<Output> {
        for entry in environment {
            let _ = (entry.name, entry.value);
        }
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "bounded update commands require Windows",
        ))
    }
}
pub mod apply;
pub mod handoff;
pub mod process;
pub mod recovery_command;
pub mod restart_manager;
