//! Where each agent keeps its configuration.
//!
//! One function per target, because every one of them chose differently.

use super::*;

pub(crate) fn copilot_user_mcp_path() -> Result<PathBuf, AppError> {
    #[cfg(windows)]
    {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .filter(|path| !path.as_os_str().is_empty())
            .map(|path| path.join("Code").join("User").join("mcp.json"))
            .ok_or_else(|| {
                AppError::external(
                    "AI_HOME_UNRESOLVED",
                    "unable to resolve %APPDATA% for VS Code configuration",
                )
            })
    }
    #[cfg(target_os = "macos")]
    {
        Ok(targets::home_dir()?
            .join("Library")
            .join("Application Support")
            .join("Code")
            .join("User")
            .join("mcp.json"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(targets::home_dir()?.join(".config"));
        Ok(base.join("Code").join("User").join("mcp.json"))
    }
}

pub(crate) fn codex_config_path(scope: Scope, root: &std::path::Path) -> Result<PathBuf, AppError> {
    match scope {
        Scope::System => system_mcp_path(targets::find("codex")?),
        Scope::User => Ok(targets::home_dir()?.join(".codex").join("config.toml")),
        Scope::Project | Scope::Local => Ok(root.join(".codex").join("config.toml")),
    }
}

pub(crate) fn codex_mcp_entry(
    path: &std::path::Path,
    name: &str,
) -> Result<Option<ServerSpec>, AppError> {
    let contents = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(AppError::file_read(path.to_path_buf(), source)),
    };
    let headers = [
        format!("[mcp_servers.{name}]"),
        format!("[mcp_servers.\"{name}\"]"),
        format!("[mcp_servers.'{name}']"),
    ];
    let mut entry = false;
    for line in contents.lines().map(str::trim) {
        if line.starts_with('[') {
            if entry {
                break;
            }
            entry = headers.iter().any(|header| line == header);
            continue;
        }
        if !entry {
            continue;
        }
        if let Some(url) = toml_string(line, "url") {
            return Ok(Some(ServerSpec::Http { url }));
        }
        if let Some(command) = toml_string(line, "command") {
            return Ok(Some(ServerSpec::Stdio {
                command,
                args: Vec::new(),
            }));
        }
    }
    Ok(None)
}

pub(super) fn toml_string(line: &str, key: &str) -> Option<String> {
    let (candidate, value) = line.split_once('=')?;
    if candidate.trim() != key {
        return None;
    }
    let value = value.trim();
    value
        .strip_prefix('"')
        .and_then(|value| value.split_once('"').map(|(value, _)| value.to_owned()))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|value| value.split_once('\'').map(|(value, _)| value.to_owned()))
        })
}

pub(super) fn system_mcp_path(target: &Target) -> Result<PathBuf, AppError> {
    Ok(system_mcp_paths(target)?[0].clone())
}

pub(crate) fn system_mcp_paths(target: &Target) -> Result<Vec<PathBuf>, AppError> {
    let directory = system_config_directory(target)?;
    Ok(match target.name {
        "claude" => vec![directory.join("managed-mcp.json")],
        "gemini" => vec![
            directory.join("settings.json"),
            directory.join("system-defaults.json"),
        ],
        "opencode" => ["config.json", "opencode.json", "opencode.jsonc"]
            .map(|name| directory.join(name))
            .to_vec(),
        "codex" => vec![directory.join("config.toml")],
        _ => vec![directory.join("mcp.json")],
    })
}

pub(crate) fn system_config_directory(target: &Target) -> Result<PathBuf, AppError> {
    #[cfg(windows)]
    {
        // Every agent keeps its machine-wide configuration under %ProgramData%,
        // the writable counterpart of the read-only install directory.
        let base = std::env::var_os("ProgramData")
            .map(PathBuf::from)
            .ok_or_else(|| {
                AppError::external(
                    "AI_SYSTEM_CONFIG_UNRESOLVED",
                    format!("unable to resolve %ProgramData% for {}", target.name),
                )
            })?;
        Ok(base.join(match target.name {
            "claude" => "ClaudeCode",
            "gemini" => "gemini-cli",
            "opencode" => "opencode",
            _ => target.name,
        }))
    }
    #[cfg(target_os = "macos")]
    {
        Ok(
            PathBuf::from("/Library/Application Support").join(match target.name {
                "claude" => "ClaudeCode",
                "gemini" => "GeminiCli",
                _ => target.name,
            }),
        )
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Ok(PathBuf::from("/etc").join(match target.name {
            "claude" => "claude-code",
            "gemini" => "gemini-cli",
            _ => target.name,
        }))
    }
}
