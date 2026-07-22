use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::{error::AppError, persistence::atomic_write_json};

use super::{
    model::{
        CurrentPointer, LifecycleState, RuntimeState, ServiceDefinition, state_invalid,
        validate_uuid_json_fields,
    },
    paths::ServicePaths,
};

#[derive(Debug)]
pub enum Document<T> {
    Missing,
    Valid(T),
    Invalid(String),
    UnsupportedVersion(u32),
}

impl<T> Document<T> {
    pub fn valid(self) -> Result<Option<T>, AppError> {
        match self {
            Self::Missing => Ok(None),
            Self::Valid(value) => Ok(Some(value)),
            Self::Invalid(message) => Err(state_invalid(message)),
            Self::UnsupportedVersion(version) => Err(state_invalid(format!(
                "unsupported managed MCP schema version {version}"
            ))),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ServiceStore {
    paths: ServicePaths,
}

impl ServiceStore {
    pub fn new(paths: ServicePaths) -> Self {
        Self { paths }
    }

    pub fn paths(&self) -> &ServicePaths {
        &self.paths
    }

    pub fn ensure_directories(&self) -> Result<(), AppError> {
        fs::create_dir_all(&self.paths.definitions_dir)
            .map_err(|source| AppError::file_write(self.paths.definitions_dir.clone(), source))
    }

    pub fn read_current(&self) -> Document<CurrentPointer> {
        read_document(&self.paths.current, CurrentPointer::validate)
    }

    pub fn write_current(&self, value: &CurrentPointer) -> Result<(), AppError> {
        value.validate()?;
        atomic_write_json(&self.paths.current, value)
    }

    pub fn read_definition(&self, path: &Path) -> Document<ServiceDefinition> {
        if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
            return Document::Invalid(format!(
                "managed MCP definition '{}' must not be a symbolic link",
                path.display()
            ));
        }
        read_document(path, ServiceDefinition::validate)
    }

    pub fn write_immutable_definition(
        &self,
        path: &Path,
        value: &ServiceDefinition,
    ) -> Result<bool, AppError> {
        value.validate()?;
        self.ensure_directories()?;
        let mut payload = serde_json::to_vec_pretty(value)?;
        payload.push(b'\n');
        match OpenOptions::new().write(true).create_new(true).open(path) {
            Ok(mut file) => {
                file.write_all(&payload)
                    .map_err(|source| AppError::file_write(path.to_path_buf(), source))?;
                file.sync_all()
                    .map_err(|source| AppError::file_write(path.to_path_buf(), source))?;
                drop(file);
                match self.read_definition(path) {
                    Document::Valid(actual) if actual == *value => Ok(true),
                    Document::Valid(_) => Err(state_invalid(format!(
                        "immutable definition '{}' changed after creation",
                        path.display()
                    ))),
                    Document::Missing => Err(state_invalid(format!(
                        "immutable definition '{}' disappeared after creation",
                        path.display()
                    ))),
                    Document::Invalid(message) => Err(state_invalid(message)),
                    Document::UnsupportedVersion(version) => Err(state_invalid(format!(
                        "unsupported managed MCP schema version {version}"
                    ))),
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                match self.read_definition(path) {
                    Document::Valid(actual) if actual == *value => Ok(false),
                    Document::Valid(_) => Err(state_invalid(format!(
                        "immutable definition '{}' contains conflicting content",
                        path.display()
                    ))),
                    Document::Missing => Err(state_invalid(format!(
                        "immutable definition '{}' could not be reopened",
                        path.display()
                    ))),
                    Document::Invalid(message) => Err(state_invalid(message)),
                    Document::UnsupportedVersion(version) => Err(state_invalid(format!(
                        "unsupported managed MCP schema version {version}"
                    ))),
                }
            }
            Err(source) => Err(AppError::file_write(path.to_path_buf(), source)),
        }
    }

    pub fn read_runtime(&self) -> Document<RuntimeState> {
        read_document(&self.paths.runtime, RuntimeState::validate)
    }

    pub fn write_runtime(&self, value: &RuntimeState) -> Result<(), AppError> {
        value.validate()?;
        atomic_write_json(&self.paths.runtime, value)
    }

    pub fn read_lifecycle(&self) -> Document<LifecycleState> {
        read_document(&self.paths.lifecycle, LifecycleState::validate)
    }

    pub fn write_lifecycle(&self, value: &LifecycleState) -> Result<(), AppError> {
        value.validate()?;
        atomic_write_json(&self.paths.lifecycle, value)
    }

    pub fn definition_files(&self) -> Result<Vec<PathBuf>, AppError> {
        let entries = match fs::read_dir(&self.paths.definitions_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => {
                return Err(AppError::directory_read(
                    self.paths.definitions_dir.clone(),
                    source,
                ));
            }
        };
        let mut files = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "json")
            })
            .collect::<Vec<_>>();
        files.sort();
        Ok(files)
    }
}

fn read_document<T>(path: &Path, validate: impl FnOnce(&T) -> Result<(), AppError>) -> Document<T>
where
    T: DeserializeOwned,
{
    let payload = match fs::read(path) {
        Ok(payload) => payload,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Document::Missing,
        Err(error) => {
            return Document::Invalid(format!(
                "failed to read managed MCP state '{}': {error}",
                path.display()
            ));
        }
    };
    let value: Value = match serde_json::from_slice(&payload) {
        Ok(value) => value,
        Err(error) => {
            return Document::Invalid(format!(
                "failed to parse managed MCP state '{}': {error}",
                path.display()
            ));
        }
    };
    let Some(version) = value.get("schema_version").and_then(Value::as_u64) else {
        return Document::Invalid(format!(
            "managed MCP state '{}' has no numeric schema_version",
            path.display()
        ));
    };
    if version != 1 {
        return match u32::try_from(version) {
            Ok(version) => Document::UnsupportedVersion(version),
            Err(_) => Document::Invalid(format!(
                "managed MCP state '{}' has an out-of-range schema_version",
                path.display()
            )),
        };
    }
    if let Err(message) = validate_uuid_json_fields(&value) {
        return Document::Invalid(format!(
            "managed MCP state '{}' has invalid identity formatting: {message}",
            path.display()
        ));
    }
    let typed = match serde_json::from_value(value) {
        Ok(value) => value,
        Err(error) => {
            return Document::Invalid(format!(
                "managed MCP state '{}' does not match schema version 1: {error}",
                path.display()
            ));
        }
    };
    match validate(&typed) {
        Ok(()) => Document::Valid(typed),
        Err(error) => Document::Invalid(error.detail_message()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp_service::model::{
        SCHEMA_VERSION, ServerDefinition, ServiceEndpoint, TASK_SPEC_VERSION,
    };
    use tempfile::TempDir;
    use uuid::Uuid;

    fn definition(paths: &ServicePaths) -> ServiceDefinition {
        ServiceDefinition {
            schema_version: SCHEMA_VERSION,
            task_spec_version: TASK_SPEC_VERSION,
            service_id: Uuid::new_v4(),
            configuration_id: Uuid::new_v4(),
            user_sid: "S-1-5-21-1".to_owned(),
            executable_path: std::env::current_exe().unwrap(),
            working_directory: std::env::current_dir().unwrap(),
            config_directory: paths.base_dir.join("config"),
            runtime_state_path: paths.runtime.clone(),
            instance_lock_path: paths.instance_lock.clone(),
            expected_version: env!("CARGO_PKG_VERSION").to_owned(),
            endpoint: ServiceEndpoint::loopback(8787).unwrap(),
            server: ServerDefinition {
                limit: None,
                max_active: 32,
                default_timeout_ms: 300_000,
            },
        }
    }

    #[test]
    fn immutable_definition_is_idempotent_but_never_replaced() {
        let temp = TempDir::new().unwrap();
        let paths = ServicePaths::from_base(temp.path().join("managed")).unwrap();
        let store = ServiceStore::new(paths.clone());
        let first = definition(&paths);
        let path = paths.definition(first.configuration_id);
        assert!(store.write_immutable_definition(&path, &first).unwrap());
        assert!(!store.write_immutable_definition(&path, &first).unwrap());

        let mut conflicting = first.clone();
        conflicting.server.max_active = 7;
        assert_eq!(
            store
                .write_immutable_definition(&path, &conflicting)
                .unwrap_err()
                .code(),
            "MCP_SERVICE_STATE_INVALID"
        );
        let Document::Valid(actual) = store.read_definition(&path) else {
            panic!("definition should remain readable")
        };
        assert_eq!(actual, first);
    }

    #[test]
    fn reader_distinguishes_missing_invalid_and_newer_schema() {
        let temp = TempDir::new().unwrap();
        let paths = ServicePaths::from_base(temp.path().join("managed")).unwrap();
        let store = ServiceStore::new(paths.clone());
        assert!(matches!(store.read_current(), Document::Missing));
        fs::create_dir_all(&paths.base_dir).unwrap();
        fs::write(&paths.current, b"not json").unwrap();
        assert!(matches!(store.read_current(), Document::Invalid(_)));
        fs::write(&paths.current, br#"{"schema_version":2}"#).unwrap();
        assert!(matches!(
            store.read_current(),
            Document::UnsupportedVersion(2)
        ));
    }
}
