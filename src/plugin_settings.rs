use std::{
    collections::BTreeSet,
    env, fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::error::AppError;

const PLUGIN_SETTINGS_FILE: &str = "plugins.json";
const SETTINGS_VERSION: u32 = 1;

#[derive(Debug, Clone)]
pub struct PluginSettings {
    path: PathBuf,
    disabled_domains: BTreeSet<String>,
}

impl PluginSettings {
    pub fn load() -> Result<Self, AppError> {
        let path = settings_file_path()?;
        if !path.exists() {
            return Ok(Self {
                path,
                disabled_domains: BTreeSet::new(),
            });
        }

        let raw = fs::read_to_string(&path)
            .map_err(|source| AppError::file_read(path.clone(), source))?;
        if raw.trim().is_empty() {
            return Ok(Self {
                path,
                disabled_domains: BTreeSet::new(),
            });
        }

        let store: PluginSettingsStore = serde_json::from_str(&raw)
            .map_err(|source| AppError::json_deserialization(path.clone(), source))?;
        if store.version != SETTINGS_VERSION {
            return Err(AppError::invalid_argument(format!(
                "unsupported plugin settings version {} in '{}'; expected {}",
                store.version,
                path.display(),
                SETTINGS_VERSION
            )));
        }
        let mut disabled_domains = BTreeSet::new();
        for domain in store.disabled_domains {
            let normalized = normalize_domain(&domain).map_err(|error| {
                AppError::invalid_argument(format!(
                    "invalid domain '{}' in plugin settings '{}': {}",
                    domain,
                    path.display(),
                    error.detail_message()
                ))
            })?;
            disabled_domains.insert(normalized);
        }

        Ok(Self {
            path,
            disabled_domains,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn disabled_domains(&self) -> impl Iterator<Item = &String> {
        self.disabled_domains.iter()
    }

    pub fn is_disabled(&self, domain: &str) -> bool {
        self.disabled_domains
            .contains(&normalize_domain_key(domain))
    }

    pub fn disable_domain(&mut self, domain: &str) -> Result<bool, AppError> {
        let normalized = normalize_domain(domain)?;
        Ok(self.disabled_domains.insert(normalized))
    }

    pub fn enable_domain(&mut self, domain: &str) -> Result<bool, AppError> {
        let normalized = normalize_domain(domain)?;
        Ok(self.disabled_domains.remove(&normalized))
    }

    pub fn reset_domain(&mut self, domain: &str) -> Result<bool, AppError> {
        self.enable_domain(domain)
    }

    pub fn clear_all(&mut self) -> bool {
        if self.disabled_domains.is_empty() {
            return false;
        }
        self.disabled_domains.clear();
        true
    }

    pub fn save(&self) -> Result<(), AppError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|source| AppError::file_write(parent.to_path_buf(), source))?;
        }
        let payload = PluginSettingsStore {
            version: SETTINGS_VERSION,
            disabled_domains: self.disabled_domains.iter().cloned().collect(),
        };
        let raw = serde_json::to_string_pretty(&payload)?;
        fs::write(&self.path, raw).map_err(|source| AppError::file_write(self.path.clone(), source))
    }
}

pub fn normalize_domain(domain: &str) -> Result<String, AppError> {
    let normalized = normalize_domain_key(domain);
    if normalized.is_empty() {
        return Err(AppError::invalid_argument("domain must not be empty"));
    }
    if !normalized
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.')
    {
        return Err(AppError::invalid_argument(
            "domain may contain only letters, numbers, '-', '_' and '.'",
        ));
    }
    Ok(normalized)
}

fn normalize_domain_key(domain: &str) -> String {
    domain.trim().to_ascii_lowercase()
}

fn settings_file_path() -> Result<PathBuf, AppError> {
    let config_dir = if let Some(value) = env::var_os("AH_CONFIG_DIR") {
        let path = PathBuf::from(value);
        if path.as_os_str().is_empty() {
            return Err(AppError::invalid_argument(
                "AH_CONFIG_DIR must not be empty",
            ));
        }
        path
    } else {
        default_config_dir()?
    };
    Ok(config_dir.join(PLUGIN_SETTINGS_FILE))
}

fn default_config_dir() -> Result<PathBuf, AppError> {
    #[cfg(target_os = "windows")]
    {
        return env::var_os("APPDATA")
            .map(PathBuf::from)
            .filter(|path| !path.as_os_str().is_empty())
            .map(|path| path.join("AIHelper"))
            .ok_or_else(|| {
                AppError::invalid_argument(
                    "unable to resolve %APPDATA% for plugin settings; set AH_CONFIG_DIR",
                )
            });
    }

    #[cfg(target_os = "macos")]
    {
        return env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|path| !path.as_os_str().is_empty())
            .map(|path| {
                path.join("Library")
                    .join("Application Support")
                    .join("AIHelper")
            })
            .ok_or_else(|| {
                AppError::invalid_argument(
                    "unable to resolve $HOME for plugin settings; set AH_CONFIG_DIR",
                )
            });
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
                AppError::invalid_argument(
                    "unable to resolve config directory for plugin settings; set AH_CONFIG_DIR",
                )
            })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PluginSettingsStore {
    version: u32,
    disabled_domains: Vec<String>,
}
