use std::{ffi::OsStr, path::Path, sync::Mutex};

use uuid::Uuid;

use ah_error::AppError;

use super::{
    lock,
    lock::FileLease,
    model::{ExitKind, LastExit, RuntimePhase, RuntimeState, ServiceDefinition, now_timestamp},
    paths::{ServicePaths, current_account, paths_equal},
    store::{Document, ServiceStore},
};

const MANAGED_SUPERVISOR_PID_ENV: &str = "AH_MCP_SERVICE_SUPERVISOR_PID";

#[derive(Debug)]
pub enum ManagedPreflight {
    AlreadyRunning,
    Ready(Box<ManagedRunner>),
}

#[derive(Debug)]
pub struct ManagedRunner {
    definition: ServiceDefinition,
    store: ServiceStore,
    state: Mutex<RuntimeState>,
    _instance_lease: FileLease,
}

// SAFETY: All fields of ManagedRunner are Send + Sync:
// - ServiceDefinition: all fields are Send + Sync
// - ServiceStore: contains only PathBufs
// - Mutex<RuntimeState>: Mutex is Send + Sync when inner is Send
// - FileLease: Mutex<HANDLE> on Windows, where HANDLE = *mut c_void; a locked
//   File on Unix. Both are Send + Sync.
unsafe impl Send for ManagedRunner {}
unsafe impl Sync for ManagedRunner {}

impl ManagedRunner {
    pub fn preflight(definition_path: &Path) -> Result<ManagedPreflight, AppError> {
        Self::preflight_with_environment(definition_path, true)
    }

    fn preflight_with_environment(
        definition_path: &Path,
        apply_environment: bool,
    ) -> Result<ManagedPreflight, AppError> {
        let definitions_dir = definition_path.parent().ok_or_else(|| {
            AppError::external(
                "MCP_SERVICE_STATE_INVALID",
                "managed definition path has no definitions directory",
            )
        })?;
        let base_dir = definitions_dir.parent().ok_or_else(|| {
            AppError::external(
                "MCP_SERVICE_STATE_INVALID",
                "managed definition path has no service base directory",
            )
        })?;
        let paths = ServicePaths::from_base(base_dir.to_path_buf())?;
        let store = ServiceStore::new(paths.clone());
        let definition = match store.read_definition(definition_path) {
            Document::Valid(definition) => definition,
            Document::Missing => {
                return Err(AppError::external(
                    "MCP_SERVICE_STATE_INVALID",
                    format!(
                        "managed MCP definition '{}' does not exist",
                        definition_path.display()
                    ),
                ));
            }
            Document::Invalid(message) => {
                return Err(AppError::external("MCP_SERVICE_STATE_INVALID", message));
            }
            Document::UnsupportedVersion(version) => {
                return Err(AppError::external(
                    "MCP_SERVICE_STATE_INVALID",
                    format!("unsupported managed MCP schema version {version}"),
                ));
            }
        };
        let expected_definition_path = paths.definition(definition.configuration_id);
        if !paths_equal(definition_path, &expected_definition_path)
            || !paths_equal(&definition.runtime_state_path, &paths.runtime)
            || !paths_equal(&definition.instance_lock_path, &paths.instance_lock)
        {
            return Err(AppError::external(
                "MCP_SERVICE_STATE_INVALID",
                "managed definition paths do not match the per-user service layout",
            ));
        }
        if definition.user_sid != current_account()? {
            return Err(AppError::external(
                "MCP_SERVICE_STATE_INVALID",
                "managed MCP definition belongs to a different user account",
            ));
        }
        let Some(instance_lease) = lock::try_acquire(&paths.instance_lock)? else {
            return Ok(ManagedPreflight::AlreadyRunning);
        };
        let runtime_pid = if apply_environment {
            take_managed_supervisor_pid()?.unwrap_or_else(std::process::id)
        } else {
            std::process::id()
        };
        if apply_environment {
            // The working directory is carried to the server as the default
            // request directory rather than applied to the process. A managed
            // server executes commands in parallel; a process-wide directory
            // cannot be per-request, and this one was also being read by every
            // command that resolved a relative path.
            // SAFETY: production preflight runs on the process main thread
            // before event logger construction, plugin discovery, or worker
            // thread creation.
            unsafe { std::env::set_var("AH_CONFIG_DIR", &definition.config_directory) };
        }
        let mut state = RuntimeState::starting(&definition, Uuid::new_v4());
        state.pid = runtime_pid;
        store.write_runtime(&state)?;
        Ok(ManagedPreflight::Ready(Box::new(Self {
            definition,
            store,
            state: Mutex::new(state),
            _instance_lease: instance_lease,
        })))
    }

    pub fn definition(&self) -> &ServiceDefinition {
        &self.definition
    }

    pub fn instance_id(&self) -> Uuid {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .instance_id
    }

    pub fn pid(&self) -> u32 {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pid
    }

    pub fn mark_ready(&self) -> Result<(), AppError> {
        self.update(RuntimePhase::Ready, None)
    }

    pub fn mark_stopped(&self) -> Result<(), AppError> {
        self.update(
            RuntimePhase::Stopped,
            Some(LastExit {
                kind: ExitKind::Clean,
                exit_code: 0,
                diagnostic_code: None,
            }),
        )
    }

    pub fn mark_stopping(&self) -> Result<(), AppError> {
        self.update(RuntimePhase::Stopping, None)
    }

    pub fn mark_failed(
        &self,
        kind: ExitKind,
        exit_code: i32,
        diagnostic_code: impl Into<String>,
    ) -> Result<(), AppError> {
        self.update(
            RuntimePhase::Failed,
            Some(LastExit {
                kind,
                exit_code,
                diagnostic_code: Some(diagnostic_code.into()),
            }),
        )
    }

    fn update(&self, phase: RuntimePhase, last_exit: Option<LastExit>) -> Result<(), AppError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.phase = phase;
        state.updated_at = now_timestamp();
        state.last_exit = last_exit;
        self.store.write_runtime(&state)
    }
}

fn take_managed_supervisor_pid() -> Result<Option<u32>, AppError> {
    let value = std::env::var_os(MANAGED_SUPERVISOR_PID_ENV);
    // SAFETY: production preflight runs on the process main thread before
    // event logger construction, plugin discovery, or worker thread creation.
    unsafe { std::env::remove_var(MANAGED_SUPERVISOR_PID_ENV) };
    value
        .as_deref()
        .map(parse_managed_supervisor_pid)
        .transpose()
}

fn parse_managed_supervisor_pid(value: &OsStr) -> Result<u32, AppError> {
    let pid = value
        .to_str()
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|pid| *pid != 0)
        .ok_or_else(|| {
            AppError::external(
                "MCP_SERVICE_STATE_INVALID",
                "managed MCP supervisor PID is invalid",
            )
        })?;
    Ok(pid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{SCHEMA_VERSION, ServerDefinition, ServiceEndpoint, TASK_SPEC_VERSION};
    use tempfile::TempDir;

    fn install_definition(temp: &TempDir) -> (ServiceStore, std::path::PathBuf) {
        let paths = ServicePaths::from_base(temp.path().join("managed")).unwrap();
        let store = ServiceStore::new(paths.clone());
        let configuration_id = Uuid::new_v4();
        let definition = ServiceDefinition {
            schema_version: SCHEMA_VERSION,
            task_spec_version: TASK_SPEC_VERSION,
            service_id: Uuid::new_v4(),
            configuration_id,
            user_sid: current_account().unwrap(),
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
        };
        let path = paths.definition(configuration_id);
        store
            .write_immutable_definition(&path, &definition)
            .unwrap();
        (store, path)
    }

    #[test]
    fn preflight_holds_single_instance_lease_and_writes_starting() {
        let temp = TempDir::new().unwrap();
        let (store, path) = install_definition(&temp);
        let ManagedPreflight::Ready(runner) =
            ManagedRunner::preflight_with_environment(&path, false).unwrap()
        else {
            panic!("first runner should acquire the instance lease")
        };
        assert!(matches!(
            ManagedRunner::preflight_with_environment(&path, false).unwrap(),
            ManagedPreflight::AlreadyRunning
        ));
        let Document::Valid(state) = store.read_runtime() else {
            panic!("runtime state should be valid")
        };
        assert_eq!(state.phase, RuntimePhase::Starting);
        runner.mark_ready().unwrap();
        drop(runner);
        assert!(matches!(
            ManagedRunner::preflight_with_environment(&path, false).unwrap(),
            ManagedPreflight::Ready(_)
        ));
    }

    #[test]
    fn managed_supervisor_pid_must_be_nonzero_decimal() {
        assert_eq!(parse_managed_supervisor_pid(OsStr::new("42")).unwrap(), 42);
        for value in ["", "0", "-1", " 42", "invalid"] {
            let error = parse_managed_supervisor_pid(OsStr::new(value)).unwrap_err();
            assert_eq!(error.code(), "MCP_SERVICE_STATE_INVALID");
        }
    }
}
