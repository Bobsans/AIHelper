use tempfile::TempDir;

use super::{super::*, harness::*};
use crate::mcp_service::model::LastExit;

#[cfg(windows)]
#[test]
fn no_start_install_is_idempotent_and_publishes_verified_pointer() {
    let temp = TempDir::new().unwrap();
    let paths = ServicePaths::from_base(temp.path().join("managed")).unwrap();
    let service = LifecycleService::new(paths.clone(), ScriptedScheduler::missing(), not_ready());
    let first = service.install(&install_options(true)).unwrap();
    assert!(first.changed);
    assert_eq!(first.action, "installed");
    assert_eq!(service.scheduler.register_count(), 1);
    let second = service.install(&install_options(true)).unwrap();
    assert!(!second.changed);
    assert_eq!(second.action, "unchanged");
    assert_eq!(service.scheduler.register_count(), 1);
    let Document::Valid(pointer) = service.store.read_current() else {
        panic!("current pointer should be valid")
    };
    let Document::Valid(definition) = service.store.read_definition(&pointer.definition_path)
    else {
        panic!("definition should be valid")
    };
    assert_eq!(definition.configuration_id, pointer.configuration_id);
}

#[cfg(windows)]
#[test]
fn malformed_current_blocks_install_without_overwrite() {
    let temp = TempDir::new().unwrap();
    let paths = ServicePaths::from_base(temp.path().join("managed")).unwrap();
    std::fs::create_dir_all(&paths.base_dir).unwrap();
    std::fs::write(&paths.current, b"malformed").unwrap();
    let service = LifecycleService::new(paths.clone(), ScriptedScheduler::missing(), not_ready());
    let error = service.install(&install_options(true)).unwrap_err();
    assert_eq!(error.code(), "MCP_SERVICE_STATE_INVALID");
    assert_eq!(std::fs::read(&paths.current).unwrap(), b"malformed");
}

#[cfg(windows)]
#[test]
fn status_is_read_only_for_absent_service_and_reports_foreign_task() {
    let temp = TempDir::new().unwrap();
    let paths = ServicePaths::from_base(temp.path().join("managed")).unwrap();
    let missing = LifecycleService::new(paths.clone(), ScriptedScheduler::missing(), not_ready());
    let status = missing.status();
    assert_eq!(status.registration.status, RegistrationStatus::NotInstalled);
    assert!(!paths.base_dir.exists());

    let foreign = LifecycleService::new(paths, ScriptedScheduler::foreign(), not_ready());
    let status = foreign.status();
    assert_eq!(
        status.registration.status,
        RegistrationStatus::ConfigurationDrift
    );
    assert_eq!(
        status.registration.diagnostic_code.as_deref(),
        Some("MCP_SERVICE_TASK_CONFLICT")
    );
}

#[cfg(windows)]
#[test]
fn status_preserves_scheduler_and_runtime_evidence_during_inferred_backoff() {
    let temp = TempDir::new().unwrap();
    let paths = ServicePaths::from_base(temp.path().join("managed")).unwrap();
    let service = LifecycleService::new(paths, ScriptedScheduler::missing(), not_ready());
    service.install(&install_options(true)).unwrap();
    let (_, definition) = installed_definition(&service);
    let mut runtime = RuntimeState::starting(&definition, Uuid::new_v4());
    runtime.phase = RuntimePhase::Failed;
    runtime.last_exit = Some(LastExit {
        kind: ExitKind::RuntimeFailure,
        exit_code: 1,
        diagnostic_code: Some("MCP_SERVER_FAILED".to_owned()),
    });
    service.store.write_runtime(&runtime).unwrap();
    service.scheduler.update_observed(|observed| {
        observed.scheduler_state = SchedulerState::Queued;
        observed.last_result = Some(1);
    });

    let status = service.status();

    assert_eq!(status.runtime.status, RuntimeStatus::RestartBackoff);
    assert_eq!(status.scheduler.last_result, Some(1));
    assert_eq!(
        status.runtime.diagnostic_code.as_deref(),
        Some("MCP_SERVER_FAILED")
    );
}

#[cfg(windows)]
#[test]
fn start_does_not_treat_scheduler_submission_as_readiness() {
    let temp = TempDir::new().unwrap();
    let paths = ServicePaths::from_base(temp.path().join("managed")).unwrap();
    let mut service = LifecycleService::new(paths, ScriptedScheduler::missing(), not_ready());
    service.install(&install_options(true)).unwrap();
    service.start_timeout = Duration::from_millis(2);
    service.poll_interval = Duration::from_millis(1);
    let error = service.start().unwrap_err();
    assert_eq!(error.code(), "MCP_SERVICE_START_TIMEOUT");
    assert_eq!(service.scheduler.run_count(), 1);
}
