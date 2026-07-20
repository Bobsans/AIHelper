use std::{
    env,
    path::{Path, PathBuf},
};

use crate::error::AppError;

const PLUGIN_SETTINGS_FILE: &str = "plugins.json";
const LOG_DIR: &str = "logs";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigSource {
    Env,
    Flags,
    Project,
    User,
    Defaults,
}

pub const CONFIG_SOURCE_PRIORITY: &[ConfigSource] = &[
    ConfigSource::Env,
    ConfigSource::Flags,
    ConfigSource::Project,
    ConfigSource::User,
    ConfigSource::Defaults,
];

#[derive(Debug, Clone)]
pub struct ConfigPaths {
    pub config_dir: PathBuf,
    pub plugin_settings_file: PathBuf,
    pub plugin_dirs: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct ConfigContext {
    paths: ConfigPaths,
    config_dir_source: ConfigSource,
    plugin_dirs_source: ConfigSource,
}

impl ConfigContext {
    pub fn load() -> Result<Self, AppError> {
        let (config_dir, config_dir_source) = resolve_config_dir()?;
        let plugin_dirs = resolve_plugin_dirs()?;
        let plugin_settings_file = config_dir.join(PLUGIN_SETTINGS_FILE);

        Ok(Self {
            paths: ConfigPaths {
                config_dir,
                plugin_settings_file,
                plugin_dirs,
            },
            config_dir_source,
            plugin_dirs_source: ConfigSource::Defaults,
        })
    }

    pub fn paths(&self) -> &ConfigPaths {
        &self.paths
    }

    pub fn config_dir_source(&self) -> ConfigSource {
        self.config_dir_source
    }

    pub fn plugin_dirs_source(&self) -> ConfigSource {
        self.plugin_dirs_source
    }

    pub fn source_priority() -> &'static [ConfigSource] {
        CONFIG_SOURCE_PRIORITY
    }
}

fn resolve_config_dir() -> Result<(PathBuf, ConfigSource), AppError> {
    if let Some(value) = env::var_os("AH_CONFIG_DIR") {
        let path = PathBuf::from(value);
        if path.as_os_str().is_empty() {
            return Err(AppError::invalid_argument(
                "AH_CONFIG_DIR must not be empty",
            ));
        }
        return Ok((path, ConfigSource::Env));
    }

    default_config_dir().map(|path| (path, ConfigSource::User))
}

pub(crate) fn resolve_log_dir() -> Option<PathBuf> {
    resolve_config_dir()
        .ok()
        .map(|(config_dir, _)| config_dir.join(LOG_DIR))
}

fn default_config_dir() -> Result<PathBuf, AppError> {
    #[cfg(target_os = "windows")]
    {
        env::var_os("APPDATA")
            .map(PathBuf::from)
            .filter(|path| !path.as_os_str().is_empty())
            .map(|path| path.join("AIHelper"))
            .ok_or_else(|| {
                AppError::invalid_argument(
                    "unable to resolve %APPDATA% for configuration; set AH_CONFIG_DIR",
                )
            })
    }

    #[cfg(target_os = "macos")]
    {
        env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|path| !path.as_os_str().is_empty())
            .map(|path| {
                path.join("Library")
                    .join("Application Support")
                    .join("AIHelper")
            })
            .ok_or_else(|| {
                AppError::invalid_argument(
                    "unable to resolve $HOME for configuration; set AH_CONFIG_DIR",
                )
            })
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        if let Some(value) = env::var_os("XDG_CONFIG_HOME") {
            let path = PathBuf::from(value);
            if !path.as_os_str().is_empty() {
                return Ok(path.join("aihelper"));
            }
        }
        env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|path| !path.as_os_str().is_empty())
            .map(|path| path.join(".config").join("aihelper"))
            .ok_or_else(|| {
                AppError::invalid_argument("unable to resolve config directory; set AH_CONFIG_DIR")
            })
    }
}

fn resolve_plugin_dirs() -> Result<Vec<PathBuf>, AppError> {
    let executable_path = env::current_exe().map_err(|source| {
        AppError::invalid_argument(format!("failed to resolve executable path: {source}"))
    })?;
    plugin_dirs_from_executable_path(&executable_path)
}

fn plugin_dirs_from_executable_path(executable_path: &Path) -> Result<Vec<PathBuf>, AppError> {
    let executable_dir = executable_path.parent().ok_or_else(|| {
        AppError::invalid_argument(format!(
            "failed to resolve executable directory for '{}'",
            executable_path.display()
        ))
    })?;

    let mut dirs = Vec::new();
    if is_cargo_profile_dir(executable_dir) {
        dirs.push(executable_dir.to_path_buf());
    }
    dirs.push(plugin_dir_from_executable_path(executable_path)?);
    dirs.dedup();
    Ok(dirs)
}

fn plugin_dir_from_executable_path(executable_path: &Path) -> Result<PathBuf, AppError> {
    let executable_dir = executable_path.parent().ok_or_else(|| {
        AppError::invalid_argument(format!(
            "failed to resolve executable directory for '{}'",
            executable_path.display()
        ))
    })?;
    Ok(executable_dir.join("plugins"))
}

fn is_cargo_profile_dir(path: &Path) -> bool {
    path.join(".cargo-lock").is_file()
        && matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some("debug") | Some("release")
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn plugin_dir_is_next_to_executable() {
        let executable = PathBuf::from_iter(["opt", "aihelper", "ah"]);
        let plugin_dir =
            plugin_dir_from_executable_path(&executable).expect("plugin dir should resolve");
        assert_eq!(
            plugin_dir,
            PathBuf::from_iter(["opt", "aihelper", "plugins"])
        );
    }

    #[test]
    fn plugin_dirs_include_cargo_profile_dir_before_plugins() {
        let temp_dir =
            env::temp_dir().join(format!("aihelper-plugin-dir-test-{}", std::process::id()));
        let profile_dir = temp_dir.join("target").join("debug");
        fs::create_dir_all(&profile_dir).expect("profile dir should be created");
        fs::write(profile_dir.join(".cargo-lock"), "")
            .expect("cargo lock marker should be written");

        let executable = profile_dir.join("ah.exe");
        let plugin_dirs =
            plugin_dirs_from_executable_path(&executable).expect("plugin dirs should resolve");
        assert_eq!(
            plugin_dirs,
            vec![profile_dir.clone(), profile_dir.join("plugins")]
        );

        fs::remove_dir_all(temp_dir).expect("temp dir should be removed");
    }
}
