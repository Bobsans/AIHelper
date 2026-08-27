//! Where the plugin's own files live: the tool config, and the cache.
//!
//! Every path comes from `ah-paths`, which is the module that owns the
//! platform's directories - this one only names what goes in them.

use super::*;

pub(crate) fn read_tool_config() -> Result<ToolConfig, InvocationResponse> {
    let path = tool_config_path()?;
    if !path.exists() {
        return Ok(ToolConfig {
            version: SETTINGS_VERSION,
            path: None,
        });
    }
    let raw = fs::read_to_string(&path).map_err(|error| {
        InvocationResponse::error(
            "POSTGRES_TOOL_CONFIG_FAILED",
            format!("failed to read '{}': {error}", path.display()),
        )
    })?;
    if raw.trim().is_empty() {
        return Ok(ToolConfig {
            version: SETTINGS_VERSION,
            path: None,
        });
    }
    let config = serde_json::from_str::<ToolConfig>(&raw).map_err(|error| {
        InvocationResponse::error(
            "POSTGRES_TOOL_CONFIG_FAILED",
            format!("failed to parse '{}': {error}", path.display()),
        )
    })?;
    if config.version != SETTINGS_VERSION {
        return Err(InvocationResponse::error(
            "POSTGRES_TOOL_CONFIG_FAILED",
            format!(
                "unsupported postgres tool config version {} in '{}'",
                config.version,
                path.display()
            ),
        ));
    }
    Ok(config)
}

pub(crate) fn write_tool_config(config: &ToolConfig) -> Result<(), InvocationResponse> {
    let path = tool_config_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            InvocationResponse::error(
                "POSTGRES_TOOL_CONFIG_FAILED",
                format!(
                    "failed to create config directory '{}': {error}",
                    parent.display()
                ),
            )
        })?;
    }
    let raw = serde_json::to_string_pretty(config).map_err(|error| {
        InvocationResponse::error(
            "JSON_SERIALIZATION_FAILED",
            format!("failed to serialize postgres tool config: {error}"),
        )
    })?;
    fs::write(&path, raw).map_err(|error| {
        InvocationResponse::error(
            "POSTGRES_TOOL_CONFIG_FAILED",
            format!("failed to write '{}': {error}", path.display()),
        )
    })
}

pub(crate) fn tool_config_path() -> Result<PathBuf, InvocationResponse> {
    Ok(config_dir()?.join("postgres-tool.json"))
}

pub(crate) fn managed_tool_path(version: &str) -> Result<PathBuf, InvocationResponse> {
    Ok(postgres_cache_root()?.join(version))
}

pub(crate) fn postgres_cache_root() -> Result<PathBuf, InvocationResponse> {
    Ok(cache_dir()?.join("tools").join("postgres"))
}

/// The plugin's own view of where AIHelper keeps configuration.
///
/// This used to be a character-for-character copy of the host's resolution -
/// two implementations that agreed only because nobody had edited one of them.
pub(crate) fn config_dir() -> Result<PathBuf, InvocationResponse> {
    ah_paths::config_dir(ah_paths::Layout::host(), &ah_paths::ProcessEnvironment)
        .map(|resolved| resolved.path)
        .map_err(|error| path_error(error, "config"))
}

pub(crate) fn cache_dir() -> Result<PathBuf, InvocationResponse> {
    ah_paths::cache_dir(ah_paths::Layout::host(), &ah_paths::ProcessEnvironment)
        .map(|resolved| resolved.path)
        .map_err(|error| path_error(error, "cache"))
}

/// The wording each failure had before the resolution moved out of this file.
pub(crate) fn path_error(error: ah_paths::Error, what: &str) -> InvocationResponse {
    if error.kind == ah_paths::ErrorKind::Empty {
        return InvocationResponse::error(
            "INVALID_ARGUMENT",
            format!("{} must not be empty", error.variable),
        );
    }
    let override_variable = if what == "cache" {
        "AH_CACHE_DIR"
    } else {
        "AH_CONFIG_DIR"
    };
    let detail = match ah_paths::Layout::host() {
        ah_paths::Layout::Windows => format!(
            "unable to resolve %{}% for postgres plugin {what}",
            error.variable
        ),
        ah_paths::Layout::MacOs => {
            format!("unable to resolve $HOME for postgres plugin {what}")
        }
        ah_paths::Layout::Xdg => {
            format!("unable to resolve {what} directory for postgres plugin")
        }
    };
    InvocationResponse::error(
        "INVALID_ARGUMENT",
        format!("{detail}; set {override_variable}"),
    )
}
