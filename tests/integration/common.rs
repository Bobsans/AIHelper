use std::{
    ffi::OsStr,
    fs,
    ops::{Deref, DerefMut},
    path::Path,
    process::Command as ProcessCommand,
};

use assert_cmd::cargo::CargoError;
use tempfile::TempDir;

pub struct IsolatedAhCommand {
    command: assert_cmd::Command,
    config_dir: TempDir,
}

impl IsolatedAhCommand {
    pub fn cargo_bin<S: AsRef<str>>(name: S) -> Result<Self, CargoError> {
        let command = assert_cmd::Command::cargo_bin(name)?;
        let config_dir = TempDir::new().map_err(CargoError::with_cause)?;
        Ok(Self::with_config_dir(command, config_dir))
    }

    pub fn new<S: AsRef<OsStr>>(program: S) -> Self {
        let command = assert_cmd::Command::new(program);
        let config_dir = TempDir::new().expect("temporary config dir should be created");
        Self::with_config_dir(command, config_dir)
    }

    fn with_config_dir(mut command: assert_cmd::Command, config_dir: TempDir) -> Self {
        command.env("AH_CONFIG_DIR", config_dir.path());
        Self {
            command,
            config_dir,
        }
    }

    pub fn config_dir(&self) -> &Path {
        self.config_dir.path()
    }
}

impl Deref for IsolatedAhCommand {
    type Target = assert_cmd::Command;

    fn deref(&self) -> &Self::Target {
        &self.command
    }
}

impl DerefMut for IsolatedAhCommand {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.command
    }
}

pub fn git_available() -> bool {
    ProcessCommand::new("git")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn run_git(cwd: &Path, args: &[&str]) {
    let status = ProcessCommand::new("git")
        .current_dir(cwd)
        .args(args)
        .status()
        .expect("git should start");
    assert!(
        status.success(),
        "git command failed in {}: git {}",
        cwd.display(),
        args.join(" ")
    );
}

fn try_git(cwd: &Path, args: &[&str]) -> bool {
    ProcessCommand::new("git")
        .current_dir(cwd)
        .args(args)
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

pub fn init_git_repo_with_one_commit() -> TempDir {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    let cwd = temp_dir.path();

    if !try_git(cwd, &["init", "-b", "main"]) {
        run_git(cwd, &["init"]);
    }
    run_git(cwd, &["config", "user.email", "test@example.com"]);
    run_git(cwd, &["config", "user.name", "Test User"]);

    fs::write(cwd.join("app.txt"), "line one\nline two\n").expect("file should be written");
    run_git(cwd, &["add", "app.txt"]);
    run_git(cwd, &["commit", "-m", "initial"]);

    temp_dir
}

pub fn create_file_symlink(link: &Path, target: &Path) -> bool {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).is_ok()
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(target, link).is_ok()
    }
}

pub fn create_dir_symlink(link: &Path, target: &Path) -> bool {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).is_ok()
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_dir(target, link).is_ok()
    }
}

#[test]
fn isolated_ah_commands_use_distinct_owned_config_directories() {
    let first = IsolatedAhCommand::cargo_bin("ah").expect("binary should compile");
    let second = IsolatedAhCommand::cargo_bin("ah").expect("binary should compile");

    assert_ne!(first.config_dir(), second.config_dir());
    assert!(first.config_dir().exists());
    assert!(second.config_dir().exists());
}
