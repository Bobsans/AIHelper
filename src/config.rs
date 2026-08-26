use std::{
    env,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use crate::error::AppError;

const PLUGIN_SETTINGS_FILE: &str = "plugins.json";
const LOG_DIR: &str = "logs";

#[derive(Debug, Clone)]
pub struct ConfigPaths {
    pub config_dir: PathBuf,
    pub plugin_settings_file: PathBuf,
    pub plugin_dirs: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct ConfigContext {
    paths: ConfigPaths,
}

impl ConfigContext {
    pub fn load() -> Result<Self, AppError> {
        let config_dir = resolve_config_dir()?;
        let plugin_dirs = resolve_plugin_dirs()?;
        let plugin_settings_file = config_dir.join(PLUGIN_SETTINGS_FILE);

        Ok(Self {
            paths: ConfigPaths {
                config_dir,
                plugin_settings_file,
                plugin_dirs,
            },
        })
    }

    pub fn paths(&self) -> &ConfigPaths {
        &self.paths
    }
}

/// The directory a relative `AH_CONFIG_DIR` is resolved against.
///
/// Process-scoped on purpose: a process has one configuration and one log
/// directory whatever it is asked to do, so this is written once at startup and
/// only ever read afterwards. That is the whole difference from the `chdir` it
/// replaces, which every later reader had to consult as live state.
static BASE_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Fix the base directory for relative configuration.
///
/// Ignored after the first call, so a test that runs commands in-process does
/// not have later ones silently reinterpret earlier paths.
pub(crate) fn set_base_dir(directory: PathBuf) {
    let _ = BASE_DIR.set(directory);
}

fn resolve_config_dir() -> Result<PathBuf, AppError> {
    let resolved = ah_paths::config_dir(ah_paths::Layout::host(), &ah_paths::ProcessEnvironment)
        .map_err(config_error)?;
    // A relative override is taken relative to the request directory. Only an
    // override can be relative, which is why the resolution reports its source
    // rather than leaving this to a guess.
    if let (ah_paths::Source::Environment(_), Some(base)) = (resolved.source, BASE_DIR.get()) {
        return Ok(ah_paths::rebase(base, &resolved.path));
    }
    Ok(resolved.path)
}

/// The wording each failure had before this crate owned the resolution.
fn config_error(error: ah_paths::Error) -> AppError {
    AppError::invalid_argument(match error.kind {
        ah_paths::ErrorKind::Empty => format!("{} must not be empty", error.variable),
        ah_paths::ErrorKind::Unresolved => match error.variable {
            "APPDATA" => {
                "unable to resolve %APPDATA% for configuration; set AH_CONFIG_DIR".to_owned()
            }
            "HOME" => "unable to resolve $HOME for configuration; set AH_CONFIG_DIR".to_owned(),
            _ => "unable to resolve config directory; set AH_CONFIG_DIR".to_owned(),
        },
    })
}

pub(crate) fn resolve_log_dir() -> Option<PathBuf> {
    resolve_config_dir().ok().map(|dir| dir.join(LOG_DIR))
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
