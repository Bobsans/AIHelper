use std::{fs, path::Path, process::Command as ProcessCommand};

use tempfile::TempDir;

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
