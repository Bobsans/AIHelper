use std::time::{Duration, Instant};

use tempfile::TempDir;

use super::{super::*, harness::*};
use crate::model::LastExit;

#[test]
fn default_install_waits_for_exact_readiness_and_ready_reinstall_is_idempotent() {
    let harness = LifecycleHarness::new();
    harness.runtime.set_sections([
        not_ready_section(),
        ready_section(harness.ids.new_instance_id, harness.ids.new_pid),
    ]);

    let first = harness.service.install(&install_options(false)).unwrap();

    assert!(first.changed);
    assert_eq!(first.action, "installed");
    assert_eq!(first.runtime, RuntimeStatus::Ready);
    assert_eq!(harness.scheduler.register_count(), 1);
    assert_eq!(harness.scheduler.run_count(), 1);
    let (pointer, definition) = harness.installed_definition();
    assert_eq!(pointer.service_id, first.service_id);
    assert_eq!(pointer.configuration_id, first.configuration_id);
    assert_eq!(definition.configuration_id, pointer.configuration_id);
    let ServiceObservation::Owned(observed) = harness.scheduler.observation() else {
        panic!("registered task should be owned")
    };
    assert_eq!(observed.marker.service_id, pointer.service_id);
    assert_eq!(observed.marker.configuration_id, pointer.configuration_id);

    let second = harness.service.install(&install_options(false)).unwrap();

    assert!(!second.changed);
    assert_eq!(second.action, "unchanged");
    assert_eq!(second.runtime, RuntimeStatus::Ready);
    assert_eq!(harness.scheduler.register_count(), 1);
    assert_eq!(harness.scheduler.run_count(), 1);
}

#[test]
fn configuration_update_stops_exact_old_instance_before_activation() {
    let harness = LifecycleHarness::new();
    harness.install_no_start();
    let (old_pointer, old_definition) = harness.installed_definition();
    harness.write_ready_runtime(
        &old_definition,
        harness.ids.old_instance_id,
        harness.ids.old_pid,
    );
    let lease = harness.hold_instance_lease();
    harness.release_instance_on_shutdown(lease);
    harness.runtime.set_sections([
        ready_section(harness.ids.old_instance_id, harness.ids.old_pid),
        not_ready_section(),
        ready_section(harness.ids.new_instance_id, harness.ids.new_pid),
    ]);
    harness.clear_adapter_events();
    let mut options = install_options(false);
    options.port = 8788;

    let output = harness.service.install(&options).unwrap();

    assert!(output.changed);
    assert_eq!(output.action, "updated");
    assert_eq!(output.runtime, RuntimeStatus::Ready);
    assert_eq!(output.service_id, old_pointer.service_id);
    assert_ne!(output.configuration_id, old_pointer.configuration_id);
    assert_eq!(
        harness.runtime.shutdown_targets(),
        vec![harness.ids.old_instance_id]
    );
    assert_eq!(harness.scheduler.run_count(), 1);
    assert_eq!(harness.scheduler.stop_count(), 0);
    let events = harness.adapter_events();
    let shutdown_index = events
        .iter()
        .position(|event| {
            matches!(
                event,
                AdapterEvent::Runtime(RuntimeEvent::Shutdown { instance_id, .. })
                    if *instance_id == harness.ids.old_instance_id
            )
        })
        .expect("old instance should receive controlled shutdown");
    let run_index = events
        .iter()
        .position(|event| matches!(event, AdapterEvent::Scheduler(SchedulerEvent::Run { .. })))
        .expect("replacement should be activated");
    assert!(shutdown_index < run_index);
    let (new_pointer, _) = harness.installed_definition();
    assert_eq!(new_pointer.service_id, old_pointer.service_id);
    assert_eq!(new_pointer.configuration_id, output.configuration_id);
}

#[test]
fn no_start_configuration_update_preserves_live_old_instance() {
    let harness = LifecycleHarness::new();
    harness.install_no_start();
    let (old_pointer, old_definition) = harness.installed_definition();
    harness.write_ready_runtime(
        &old_definition,
        harness.ids.old_instance_id,
        harness.ids.old_pid,
    );
    harness.runtime.set_sections([ready_section(
        harness.ids.old_instance_id,
        harness.ids.old_pid,
    )]);
    let lease = harness.hold_instance_lease();
    let runtime_before = std::fs::read(&harness.paths.runtime).unwrap();
    harness.clear_adapter_events();
    let mut options = install_options(true);
    options.port = 8788;

    let output = harness.service.install(&options).unwrap();

    assert_eq!(output.action, "updated");
    assert_eq!(output.service_id, old_pointer.service_id);
    assert_ne!(output.configuration_id, old_pointer.configuration_id);
    assert_eq!(harness.scheduler.register_count(), 1);
    assert_eq!(harness.scheduler.run_count(), 0);
    assert_eq!(harness.scheduler.stop_count(), 0);
    assert_eq!(harness.scheduler.delete_count(), 0);
    assert!(harness.runtime.shutdown_targets().is_empty());
    assert_eq!(
        std::fs::read(&harness.paths.runtime).unwrap(),
        runtime_before
    );
    let (new_pointer, _) = harness.installed_definition();
    assert_eq!(new_pointer.configuration_id, output.configuration_id);
    drop(lease);
}

#[test]
fn register_failure_keeps_current_unpublished_and_retry_converges() {
    let harness = LifecycleHarness::new();
    harness.fail_next_scheduler_operation(SchedulerFaultPoint::Register);

    let error = harness.service.install(&install_options(true)).unwrap_err();

    assert_eq!(error.code(), "MCP_SERVICE_SCHEDULER_FAILED");
    assert!(matches!(harness.store().read_current(), Document::Missing));
    assert!(matches!(
        harness.scheduler.observation(),
        ServiceObservation::Missing
    ));
    assert_eq!(harness.scheduler.run_count(), 0);
    assert_eq!(harness.scheduler.stop_count(), 0);
    assert_eq!(harness.scheduler.delete_count(), 0);
    assert!(harness.runtime.shutdown_targets().is_empty());
    let Document::Valid(lifecycle) = harness.store().read_lifecycle() else {
        panic!("failed install should persist lifecycle diagnostics")
    };
    assert_eq!(lifecycle.state, LifecycleStateKind::Failed);
    assert_eq!(
        lifecycle.diagnostic_code.as_deref(),
        Some("MCP_SERVICE_SCHEDULER_FAILED")
    );
    harness.clear_adapter_events();

    let retry = harness.service.install(&install_options(true)).unwrap();

    assert_eq!(retry.action, "installed");
    assert!(matches!(harness.store().read_current(), Document::Valid(_)));
    assert_eq!(harness.scheduler.register_count(), 1);
    assert_eq!(harness.scheduler.run_count(), 0);
}

#[test]
fn drifted_register_readback_keeps_old_pointer_and_retry_converges() {
    let harness = LifecycleHarness::new();
    harness.install_no_start();
    let (old_pointer, _) = harness.installed_definition();
    let old_observed = match harness.scheduler.observation() {
        ServiceObservation::Owned(observed) => observed,
        _ => panic!("installed task should be owned"),
    };
    let old_current = std::fs::read(&harness.paths.current).unwrap();
    harness.scheduler.queue_register_readback(*old_observed);
    harness.clear_adapter_events();
    let mut options = install_options(true);
    options.port = 8788;

    let error = harness.service.install(&options).unwrap_err();

    assert_eq!(error.code(), "MCP_SERVICE_CONFIGURATION_DRIFT");
    assert_eq!(std::fs::read(&harness.paths.current).unwrap(), old_current);
    assert_eq!(harness.scheduler.register_count(), 1);
    assert_eq!(harness.scheduler.run_count(), 0);
    let Document::Valid(lifecycle) = harness.store().read_lifecycle() else {
        panic!("drifted readback should persist lifecycle diagnostics")
    };
    assert_eq!(lifecycle.state, LifecycleStateKind::Failed);
    assert_eq!(
        lifecycle.diagnostic_code.as_deref(),
        Some("MCP_SERVICE_CONFIGURATION_DRIFT")
    );
    harness.clear_adapter_events();

    let retry = harness.service.install(&options).unwrap();

    assert_eq!(retry.action, "updated");
    assert_eq!(retry.service_id, old_pointer.service_id);
    assert_ne!(retry.configuration_id, old_pointer.configuration_id);
    assert_eq!(harness.scheduler.register_count(), 1);
    assert_eq!(harness.scheduler.run_count(), 0);
    let (current, _) = harness.installed_definition();
    assert_eq!(current.configuration_id, retry.configuration_id);
}

#[test]
fn exact_ready_start_is_idempotent() {
    let harness = LifecycleHarness::new();
    harness.install_no_start();
    let (_, definition) = harness.installed_definition();
    harness.write_ready_runtime(
        &definition,
        harness.ids.old_instance_id,
        harness.ids.old_pid,
    );
    harness.runtime.set_sections([ready_section(
        harness.ids.old_instance_id,
        harness.ids.old_pid,
    )]);
    harness.clear_adapter_events();

    let output = harness.service.start().unwrap();

    assert!(!output.changed);
    assert_eq!(output.action, "already_ready");
    assert_eq!(output.runtime, RuntimeStatus::Ready);
    assert_eq!(harness.scheduler.run_count(), 0);
}

#[test]
fn run_failure_preserves_registration_and_retry_converges() {
    let harness = LifecycleHarness::new();
    harness.install_no_start();
    let current_before = std::fs::read(&harness.paths.current).unwrap();
    harness.clear_adapter_events();
    harness.fail_next_scheduler_operation(SchedulerFaultPoint::Run);

    let error = harness.service.start().unwrap_err();

    assert_eq!(error.code(), "MCP_SERVICE_SCHEDULER_FAILED");
    assert_eq!(
        std::fs::read(&harness.paths.current).unwrap(),
        current_before
    );
    assert_eq!(harness.scheduler.run_count(), 1);
    assert_eq!(harness.scheduler.stop_count(), 0);
    assert_eq!(harness.scheduler.delete_count(), 0);
    assert!(harness.runtime.shutdown_targets().is_empty());
    let Document::Valid(lifecycle) = harness.store().read_lifecycle() else {
        panic!("failed start should persist lifecycle diagnostics")
    };
    assert_eq!(lifecycle.state, LifecycleStateKind::Failed);
    assert_eq!(
        lifecycle.diagnostic_code.as_deref(),
        Some("MCP_SERVICE_SCHEDULER_FAILED")
    );
    harness.runtime.set_sections([
        not_ready_section(),
        ready_section(harness.ids.new_instance_id, harness.ids.new_pid),
    ]);

    let retry = harness.service.start().unwrap();

    assert!(retry.changed);
    assert_eq!(retry.action, "started");
    assert_eq!(retry.runtime, RuntimeStatus::Ready);
    assert_eq!(harness.scheduler.run_count(), 2);
    assert_eq!(
        std::fs::read(&harness.paths.current).unwrap(),
        current_before
    );
}

#[test]
fn foreign_and_drifted_tasks_never_run() {
    let harness = LifecycleHarness::new();
    harness.install_no_start();
    let exact = harness.scheduler.observation();
    let current_before = std::fs::read(&harness.paths.current).unwrap();
    harness
        .scheduler
        .set_observation(ServiceObservation::Foreign);
    harness.clear_adapter_events();

    let foreign_error = harness.service.start().unwrap_err();

    assert_eq!(foreign_error.code(), "MCP_SERVICE_TASK_CONFLICT");
    assert_eq!(harness.scheduler.run_count(), 0);
    assert_eq!(
        std::fs::read(&harness.paths.current).unwrap(),
        current_before
    );
    harness.scheduler.set_observation(exact);
    harness.scheduler.introduce_drift();
    harness.clear_adapter_events();

    let drift_error = harness.service.start().unwrap_err();

    assert_eq!(drift_error.code(), "MCP_SERVICE_CONFIGURATION_DRIFT");
    assert_eq!(harness.scheduler.run_count(), 0);
    assert_eq!(harness.scheduler.stop_count(), 0);
    assert_eq!(harness.scheduler.delete_count(), 0);
    assert!(harness.runtime.shutdown_targets().is_empty());
    assert_eq!(
        std::fs::read(&harness.paths.current).unwrap(),
        current_before
    );
}

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

#[test]
fn status_reduces_stopped_ready_running_failed_and_identity_mismatch() {
    let harness = LifecycleHarness::new();
    harness.install_no_start();
    let (_, definition) = harness.installed_definition();

    let stopped = harness.service.status();
    assert_eq!(stopped.runtime.status, RuntimeStatus::Stopped);

    harness.write_ready_runtime(
        &definition,
        harness.ids.old_instance_id,
        harness.ids.old_pid,
    );
    harness.runtime.set_sections([ready_section(
        harness.ids.old_instance_id,
        harness.ids.old_pid,
    )]);
    let ready = harness.service.status();
    assert_eq!(ready.runtime.status, RuntimeStatus::Ready);

    harness
        .scheduler
        .set_scheduler_state(SchedulerState::Running, Vec::new());
    harness.runtime.set_sections([not_ready_section()]);
    let running = harness.service.status();
    assert_eq!(running.runtime.status, RuntimeStatus::RunningNotReady);

    let mut failed_runtime = RuntimeState::starting(&definition, harness.ids.old_instance_id);
    failed_runtime.phase = RuntimePhase::Failed;
    failed_runtime.last_exit = Some(LastExit {
        kind: ExitKind::RuntimeFailure,
        exit_code: 1,
        diagnostic_code: Some("MCP_SERVER_FAILED".to_owned()),
    });
    harness.set_runtime(&failed_runtime);
    harness
        .scheduler
        .set_scheduler_state(SchedulerState::Ready, Vec::new());
    let failed = harness.service.status();
    assert_eq!(failed.runtime.status, RuntimeStatus::Failed);
    assert_eq!(
        failed.runtime.diagnostic_code.as_deref(),
        Some("MCP_SERVER_FAILED")
    );

    harness.runtime.set_sections([identity_mismatch_section(
        harness.ids.new_instance_id,
        harness.ids.new_pid,
    )]);
    let mismatch = harness.service.status();
    assert_eq!(mismatch.runtime.status, RuntimeStatus::IdentityMismatch);
    assert_eq!(
        mismatch.runtime.diagnostic_code.as_deref(),
        Some("MCP_SERVICE_IDENTITY_MISMATCH")
    );
}

#[test]
fn status_preserves_launcher_and_runtime_evidence_during_inferred_backoff() {
    let harness = LifecycleHarness::new();
    harness.install_no_start();
    let (_, definition) = harness.installed_definition();
    let mut runtime = RuntimeState::starting(&definition, Uuid::new_v4());
    runtime.phase = RuntimePhase::Failed;
    runtime.last_exit = Some(LastExit {
        kind: ExitKind::RuntimeFailure,
        exit_code: 1,
        diagnostic_code: Some("MCP_SERVER_FAILED".to_owned()),
    });
    harness.set_runtime(&runtime);
    harness.scheduler.update_observed(|observed| {
        observed.state = SchedulerState::Running;
        observed.last_result = Some(1);
    });

    let status = harness.service.status();

    assert_eq!(status.runtime.status, RuntimeStatus::RestartBackoff);
    assert_eq!(status.scheduler.last_result, Some(1));
    assert_eq!(
        status.runtime.diagnostic_code.as_deref(),
        Some("MCP_SERVER_FAILED")
    );
}

#[test]
fn status_reports_sorted_configuration_drift() {
    let harness = LifecycleHarness::new();
    harness.install_no_start();
    let drifted = harness.scheduler.introduce_drift();

    let status = harness.service.status();

    assert_eq!(
        status.registration.status,
        RegistrationStatus::ConfigurationDrift
    );
    assert_eq!(
        status.registration.diagnostic_code.as_deref(),
        Some("MCP_SERVICE_CONFIGURATION_DRIFT")
    );
    let fields: Vec<_> = status
        .drift
        .iter()
        .map(|entry| entry.field.as_str())
        .collect();
    let mut sorted = fields.clone();
    sorted.sort_unstable();
    assert_eq!(fields, sorted);
    for field in drifted {
        assert!(fields.contains(&field), "{field} should be reported");
    }
    assert!(
        status
            .drift
            .iter()
            .all(|entry| entry.diagnostic_code == "MCP_SERVICE_CONFIGURATION_DRIFT")
    );
}

#[test]
fn status_scheduler_error_does_not_change_durable_state() {
    let harness = LifecycleHarness::new();
    harness.install_no_start();
    let (_, definition) = harness.installed_definition();
    harness.write_ready_runtime(
        &definition,
        harness.ids.old_instance_id,
        harness.ids.old_pid,
    );
    harness.runtime.set_sections([ready_section(
        harness.ids.old_instance_id,
        harness.ids.old_pid,
    )]);
    let before = harness.durable_bytes();
    harness.fail_next_scheduler_operation(SchedulerFaultPoint::Inspect);

    let status = harness.service.status();

    assert_eq!(
        status.registration.status,
        RegistrationStatus::SchedulerError
    );
    assert_eq!(status.scheduler.state, SchedulerState::Error);
    assert_eq!(
        status.scheduler.diagnostic_code.as_deref(),
        Some("MCP_SERVICE_SCHEDULER_FAILED")
    );
    assert_eq!(status.readiness.status, ReadinessStatus::Ready);
    assert_eq!(status.runtime.status, RuntimeStatus::Ready);
    assert_eq!(harness.durable_bytes(), before);
}

#[test]
fn status_scheduler_error_does_not_probe_invalid_definition() {
    let harness = LifecycleHarness::new();
    harness.install_no_start();
    let (pointer, _) = harness.installed_definition();
    std::fs::write(&pointer.definition_path, b"invalid definition").unwrap();
    let before = harness.durable_bytes();
    harness.fail_next_scheduler_operation(SchedulerFaultPoint::Inspect);

    let status = harness.service.status();

    assert_eq!(
        status.registration.status,
        RegistrationStatus::SchedulerError
    );
    assert_eq!(status.readiness.status, ReadinessStatus::NotChecked);
    assert_eq!(status.runtime.status, RuntimeStatus::Stopped);
    assert_eq!(status.drift.len(), 1);
    assert_eq!(status.drift[0].field, "definition.invalid");
    assert_eq!(harness.durable_bytes(), before);
}

#[test]
fn status_reports_invalid_documents_without_rewriting_them() {
    let harness = LifecycleHarness::new();
    harness.store().ensure_directories().unwrap();
    std::fs::write(&harness.paths.current, b"invalid current").unwrap();
    std::fs::write(&harness.paths.lifecycle, b"invalid lifecycle").unwrap();
    let before = harness.durable_bytes();

    let status = harness.service.status();

    assert_eq!(
        status.registration.status,
        RegistrationStatus::ConfigurationDrift
    );
    assert_eq!(
        status.registration.diagnostic_code.as_deref(),
        Some("MCP_SERVICE_STATE_INVALID")
    );
    let fields: Vec<_> = status
        .drift
        .iter()
        .map(|entry| entry.field.as_str())
        .collect();
    assert_eq!(fields, vec!["current.invalid", "lifecycle.invalid"]);
    assert_eq!(harness.durable_bytes(), before);

    let installed = LifecycleHarness::new();
    installed.install_no_start();
    std::fs::write(&installed.paths.runtime, b"invalid runtime").unwrap();
    let runtime_before = installed.durable_bytes();

    let runtime_status = installed.service.status();

    assert_eq!(
        runtime_status.runtime.diagnostic_code.as_deref(),
        Some("MCP_SERVICE_STATE_INVALID")
    );
    assert_eq!(runtime_status.drift.len(), 1);
    assert_eq!(runtime_status.drift[0].field, "runtime.invalid");
    assert_eq!(installed.durable_bytes(), runtime_before);
}

#[test]
fn status_reports_busy_lifecycle_without_waiting_or_writing() {
    let harness = LifecycleHarness::new();
    harness.install_no_start();
    let (pointer, _) = harness.installed_definition();
    let active = LifecycleState::active(LifecycleOperation::Restart, Some(pointer.service_id));
    harness.store().write_lifecycle(&active).unwrap();
    let lifecycle_lease = lock::try_acquire(&harness.paths.lifecycle_lock)
        .unwrap()
        .expect("lifecycle lease should be available");
    let before = harness.durable_bytes();
    let started = Instant::now();
    let status = harness.service.status();

    assert!(
        started.elapsed() < Duration::from_secs(1),
        "status should not wait for the lifecycle lease"
    );
    assert_eq!(status.lifecycle.status, LifecycleStatus::Busy);
    let operation = status
        .lifecycle
        .operation
        .expect("active lifecycle operation should be reported");
    assert_eq!(operation.operation_id, active.operation_id);
    assert_eq!(operation.operation, LifecycleOperation::Restart);
    assert_eq!(operation.service_id, Some(pointer.service_id));
    assert_eq!(harness.durable_bytes(), before);
    drop(lifecycle_lease);
}

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
