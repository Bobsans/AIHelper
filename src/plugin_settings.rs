use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{config::ConfigContext, error::AppError};

const SETTINGS_VERSION: u32 = 1;

#[derive(Debug, Clone)]
pub struct PluginSettings {
    path: PathBuf,
    disabled_domains: BTreeSet<String>,
}

impl PluginSettings {
    pub fn load() -> Result<Self, AppError> {
        let context = ConfigContext::load()?;
        Self::load_from_path(context.paths().plugin_settings_file.clone())
    }

    pub fn load_from_path(path: PathBuf) -> Result<Self, AppError> {
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

    pub fn update(
        &mut self,
        mutation: impl FnOnce(&mut PluginSettings) -> Result<bool, AppError>,
    ) -> Result<bool, AppError> {
        let path = self.path.clone();
        let (candidate, changed) = crate::persistence::transaction(&path, || {
            let mut candidate = Self::load_from_path(path.clone())?;
            let changed = mutation(&mut candidate)?;
            if changed {
                candidate.save_unlocked()?;
            }
            Ok((candidate, changed))
        })?;
        *self = candidate;
        Ok(changed)
    }

    fn save_unlocked(&self) -> Result<(), AppError> {
        let payload = PluginSettingsStore {
            version: SETTINGS_VERSION,
            disabled_domains: self.disabled_domains.iter().cloned().collect(),
        };
        crate::persistence::atomic_write_json(&self.path, &payload)
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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PluginSettingsStore {
    version: u32,
    disabled_domains: Vec<String>,
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};

    use super::*;

    #[test]
    fn failed_update_leaves_in_memory_settings_unchanged() {
        let directory = tempfile::tempdir().unwrap();
        let blocked_parent = directory.path().join("blocked");
        fs::write(&blocked_parent, "not a directory").unwrap();
        let mut settings =
            PluginSettings::load_from_path(blocked_parent.join("plugins.json")).unwrap();

        let error = settings
            .update(|candidate| candidate.disable_domain("search"))
            .expect_err("update should fail before publishing the candidate");

        assert_eq!(error.code(), "FILE_WRITE_FAILED");
        assert!(!settings.is_disabled("search"));
    }

    #[test]
    fn concurrent_updates_preserve_unrelated_domains() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("plugins.json");
        let barrier = Arc::new(Barrier::new(2));
        let mut handles = Vec::new();
        for domain in ["search", "task"] {
            let path = path.clone();
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                let mut settings = PluginSettings::load_from_path(path).unwrap();
                barrier.wait();
                settings
                    .update(|candidate| candidate.disable_domain(domain))
                    .unwrap();
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }

        let settings = PluginSettings::load_from_path(path).unwrap();
        assert!(settings.is_disabled("search"));
        assert!(settings.is_disabled("task"));
    }
}
