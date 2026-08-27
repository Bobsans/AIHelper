//! Reading a git remote, for a plugin that infers its repository from one.
//!
//! Unlike [`crate::logs`], this module renders its own failures. The two SCM
//! plugins had this function byte for byte identically - same codes, same
//! wording - so returning a typed error would move the duplication into two
//! `match` arms rather than remove it. The diagnostic is part of what is being
//! shared here.

use std::path::Path;

use ah_plugin_api::{InvocationResponse, noninteractive_command};

/// The URL `git remote get-url <remote>` reports, run in `cwd`.
///
/// # Errors
///
/// `COMMAND_EXECUTION_FAILED` when git could not be started at all, and
/// `COMMAND_FAILED` with git's own stderr when it ran and refused.
pub fn remote_url(remote: &str, cwd: Option<&Path>) -> Result<String, InvocationResponse> {
    let mut command = noninteractive_command("git");
    command.args(["remote", "get-url", remote]);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let output = command.output().map_err(|error| {
        InvocationResponse::error(
            "COMMAND_EXECUTION_FAILED",
            format!("failed to execute git remote get-url {remote}: {error}"),
        )
    })?;
    if !output.status.success() {
        return Err(InvocationResponse::error(
            "COMMAND_FAILED",
            format!(
                "git remote get-url {remote} failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory that is not a repository is the failure a user actually
    /// hits, and it has to arrive as `COMMAND_FAILED` rather than as a panic or
    /// an empty string.
    #[test]
    fn a_directory_without_a_repository_reports_the_command_failure() {
        let temp = std::env::temp_dir().join("ah-plugin-sdk-git-no-repo");
        std::fs::create_dir_all(&temp).expect("the temporary directory should be created");

        let error = remote_url("origin", Some(&temp)).expect_err("there is no remote to read");

        assert_eq!(error.error_code.as_deref(), Some("COMMAND_FAILED"));
    }

    /// The URL comes back trimmed: `git` writes a trailing newline, and every
    /// caller parses the result as a URL.
    #[test]
    fn the_repositorys_own_remote_comes_back_without_its_newline() {
        let repository = Path::new(env!("CARGO_MANIFEST_DIR"));

        let Ok(url) = remote_url("origin", Some(repository)) else {
            // A checkout with no `origin` is a valid environment; the failure
            // path is covered above.
            return;
        };

        assert_eq!(url.trim(), url);
        assert!(!url.is_empty());
    }
}
