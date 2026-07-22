use tempfile::TempDir;

use super::{super::*, harness::*};

#[cfg(windows)]
#[test]
fn stop_reports_already_stopped_only_with_complete_quiescence_proof() {
    let temp = TempDir::new().unwrap();
    let paths = ServicePaths::from_base(temp.path().join("managed")).unwrap();
    let service = LifecycleService::new(paths, ScriptedScheduler::missing(), not_ready());
    service.install(&install_options(true)).unwrap();

    let output = service.stop().unwrap();

    assert!(!output.changed);
    assert_eq!(output.action, "already_stopped");
    assert_eq!(output.runtime, RuntimeStatus::Stopped);
    assert_eq!(service.scheduler.stop_count(), 0);
    assert!(service.readiness.shutdown_targets().is_empty());
}

#[cfg(windows)]
#[test]
fn exact_live_stop_uses_control_identity_and_retains_quiescence_guard() {
    let temp = TempDir::new().unwrap();
    let paths = ServicePaths::from_base(temp.path().join("managed")).unwrap();
    let service = LifecycleService::new(paths.clone(), ScriptedScheduler::missing(), not_ready());
    service.install(&install_options(true)).unwrap();
    let (_, definition) = installed_definition(&service);
    let instance_id = Uuid::new_v4();
    write_ready_runtime(&service, &definition, instance_id, 41);
    service
        .readiness
        .set_sections([ready_section(instance_id, 41), not_ready_section()]);
    service
        .readiness
        .release_instance_on_shutdown(FileLease::try_acquire(&paths.instance_lock).unwrap());

    let output = service.stop().unwrap();

    assert!(output.changed);
    assert_eq!(output.action, "stopped");
    assert_eq!(service.readiness.shutdown_targets(), vec![instance_id]);
    assert_eq!(service.scheduler.stop_count(), 0);
}

#[cfg(windows)]
#[test]
fn control_identity_mismatch_uses_only_exact_scheduler_fallback() {
    let temp = TempDir::new().unwrap();
    let paths = ServicePaths::from_base(temp.path().join("managed")).unwrap();
    let service = LifecycleService::new(paths, ScriptedScheduler::missing(), not_ready());
    service.install(&install_options(true)).unwrap();
    let (_, definition) = installed_definition(&service);
    let runtime_id = Uuid::new_v4();
    let scheduler_id = Uuid::new_v4();
    write_ready_runtime(&service, &definition, runtime_id, 52);
    service
        .readiness
        .set_sections([identity_mismatch_section(Uuid::new_v4(), 999)]);
    set_scheduler_state(
        &service,
        SchedulerState::Running,
        vec![SchedulerInstance {
            instance_id: scheduler_id,
            state: SchedulerState::Running,
            engine_pid: Some(52),
        }],
    );

    let output = service.stop().unwrap();

    assert_eq!(output.action, "forced_stopped");
    assert!(service.readiness.shutdown_targets().is_empty());
    assert_eq!(
        service.scheduler.stop_targets(),
        vec![SchedulerStopTarget::Running {
            instance_id: scheduler_id,
            expected_pid: 52,
        }]
    );
}

#[cfg(windows)]
#[test]
fn failed_control_request_falls_back_to_exact_revalidated_scheduler_instance() {
    let harness = LifecycleHarness::new();
    harness.install_no_start();
    let (_, definition) = harness.installed_definition();
    harness.write_ready_runtime(
        &definition,
        harness.ids.old_instance_id,
        harness.ids.old_pid,
    );
    harness.runtime.set_sections([
        ready_section(harness.ids.old_instance_id, harness.ids.old_pid),
        not_ready_section(),
    ]);
    harness
        .runtime
        .fail_next_shutdown("scripted control failure");
    harness.scheduler.set_scheduler_state(
        SchedulerState::Running,
        vec![SchedulerInstance {
            instance_id: harness.ids.scheduler_instance_id,
            state: SchedulerState::Running,
            engine_pid: Some(harness.ids.old_pid),
        }],
    );
    let lease = harness.hold_instance_lease();
    harness.release_instance_on_forced_stop(lease);
    harness.clear_adapter_events();

    let output = harness.service.stop().unwrap();

    assert!(output.changed);
    assert_eq!(output.action, "forced_stopped");
    assert_eq!(
        harness.runtime.shutdown_targets(),
        vec![harness.ids.old_instance_id]
    );
    assert_eq!(
        harness.scheduler.stop_targets(),
        vec![SchedulerStopTarget::Running {
            instance_id: harness.ids.scheduler_instance_id,
            expected_pid: harness.ids.old_pid,
        }]
    );
}

#[cfg(windows)]
#[test]
fn scheduler_stop_failure_preserves_identity_and_retry_converges() {
    let harness = LifecycleHarness::new();
    harness.install_no_start();
    let (_, definition) = harness.installed_definition();
    harness.write_ready_runtime(
        &definition,
        harness.ids.old_instance_id,
        harness.ids.old_pid,
    );
    harness.runtime.set_sections([identity_mismatch_section(
        harness.ids.new_instance_id,
        harness.ids.new_pid,
    )]);
    harness.scheduler.set_scheduler_state(
        SchedulerState::Running,
        vec![SchedulerInstance {
            instance_id: harness.ids.scheduler_instance_id,
            state: SchedulerState::Running,
            engine_pid: Some(harness.ids.old_pid),
        }],
    );
    let lease = harness.hold_instance_lease();
    harness.release_instance_on_forced_stop(lease);
    harness.fail_next_scheduler_operation(SchedulerFaultPoint::Stop);
    let before = harness.durable_bytes();
    harness.clear_adapter_events();

    let error = harness.service.stop().unwrap_err();

    assert_eq!(error.code(), "MCP_SERVICE_SCHEDULER_FAILED");
    assert_eq!(harness.scheduler.stop_count(), 1);
    assert_eq!(harness.scheduler.run_count(), 0);
    assert!(harness.runtime.shutdown_targets().is_empty());
    let failed = harness.durable_bytes();
    assert_eq!(failed.current, before.current);
    assert_eq!(failed.definitions, before.definitions);
    assert_eq!(failed.runtime, before.runtime);
    let TaskObservation::Owned(observed) = harness.scheduler.observation() else {
        panic!("failed stop should preserve the owned registration")
    };
    assert_eq!(observed.scheduler_state, SchedulerState::Running);
    assert_eq!(
        observed.spec.marker.configuration_id,
        definition.configuration_id
    );
    let Document::Valid(lifecycle) = harness.store().read_lifecycle() else {
        panic!("failed stop should persist lifecycle diagnostics")
    };
    assert_eq!(lifecycle.state, LifecycleStateKind::Failed);
    assert_eq!(
        lifecycle.diagnostic_code.as_deref(),
        Some("MCP_SERVICE_SCHEDULER_FAILED")
    );

    let retry = harness.service.stop().unwrap();

    assert!(retry.changed);
    assert_eq!(retry.action, "forced_stopped");
    assert_eq!(harness.scheduler.stop_count(), 2);
    assert_eq!(
        harness.scheduler.stop_targets(),
        vec![
            SchedulerStopTarget::Running {
                instance_id: harness.ids.scheduler_instance_id,
                expected_pid: harness.ids.old_pid,
            },
            SchedulerStopTarget::Running {
                instance_id: harness.ids.scheduler_instance_id,
                expected_pid: harness.ids.old_pid,
            },
        ]
    );
    assert_eq!(harness.durable_bytes().current, before.current);
}

#[cfg(windows)]
#[test]
fn queued_fallback_requires_the_exact_process_free_instance() {
    let temp = TempDir::new().unwrap();
    let paths = ServicePaths::from_base(temp.path().join("managed")).unwrap();
    let service = LifecycleService::new(paths, ScriptedScheduler::missing(), not_ready());
    service.install(&install_options(true)).unwrap();
    let instance_id = Uuid::new_v4();
    set_scheduler_state(
        &service,
        SchedulerState::Queued,
        vec![SchedulerInstance {
            instance_id,
            state: SchedulerState::Queued,
            engine_pid: None,
        }],
    );

    let output = service.stop().unwrap();

    assert_eq!(output.action, "forced_stopped");
    assert_eq!(
        service.scheduler.stop_targets(),
        vec![SchedulerStopTarget::Queued { instance_id }]
    );
}

#[cfg(windows)]
#[test]
fn pid_mismatch_and_task_drift_never_reach_scheduler_stop() {
    let temp = TempDir::new().unwrap();
    let paths = ServicePaths::from_base(temp.path().join("managed")).unwrap();
    let service = LifecycleService::new(paths, ScriptedScheduler::missing(), not_ready());
    service.install(&install_options(true)).unwrap();
    let (_, definition) = installed_definition(&service);
    write_ready_runtime(&service, &definition, Uuid::new_v4(), 63);
    set_scheduler_state(
        &service,
        SchedulerState::Running,
        vec![SchedulerInstance {
            instance_id: Uuid::new_v4(),
            state: SchedulerState::Running,
            engine_pid: Some(64),
        }],
    );
    let error = service.stop().unwrap_err();
    assert_eq!(error.code(), "MCP_SERVICE_STOP_UNSAFE");
    assert_eq!(service.scheduler.stop_count(), 0);

    service.scheduler.update_observed(|observed| {
        observed.spec.executable_path = temp.path().join("changed.exe");
    });
    service.scheduler.update_instances(|instances| {
        instances[0].engine_pid = Some(63);
    });
    let error = service.stop().unwrap_err();
    assert_eq!(error.code(), "MCP_SERVICE_STOP_UNSAFE");
    assert_eq!(service.scheduler.stop_count(), 0);
}

#[cfg(windows)]
#[test]
fn scheduler_stop_without_lease_release_times_out() {
    let temp = TempDir::new().unwrap();
    let paths = ServicePaths::from_base(temp.path().join("managed")).unwrap();
    let mut service =
        LifecycleService::new(paths.clone(), ScriptedScheduler::missing(), not_ready());
    service.install(&install_options(true)).unwrap();
    let (_, definition) = installed_definition(&service);
    write_ready_runtime(&service, &definition, Uuid::new_v4(), 75);
    set_scheduler_state(
        &service,
        SchedulerState::Running,
        vec![SchedulerInstance {
            instance_id: Uuid::new_v4(),
            state: SchedulerState::Running,
            engine_pid: Some(75),
        }],
    );
    let _occupied = FileLease::try_acquire(&paths.instance_lock)
        .unwrap()
        .unwrap();
    service.stop_timeout = Duration::from_millis(2);
    service.poll_interval = Duration::from_millis(1);

    let error = service.stop().unwrap_err();

    assert_eq!(error.code(), "MCP_SERVICE_STOP_TIMEOUT");
    assert_eq!(service.scheduler.stop_count(), 1);
}

#[cfg(windows)]
#[test]
fn restart_of_stopped_service_runs_start_phase_and_reports_started() {
    let harness = LifecycleHarness::new();
    harness.install_no_start();
    harness.runtime.set_sections([
        not_ready_section(),
        not_ready_section(),
        ready_section(harness.ids.new_instance_id, harness.ids.new_pid),
    ]);
    harness.clear_adapter_events();

    let output = harness.service.restart().unwrap();

    assert!(output.changed);
    assert_eq!(output.action, "started");
    assert_eq!(output.runtime, RuntimeStatus::Ready);
    assert_eq!(harness.scheduler.run_count(), 1);
    assert_eq!(harness.scheduler.stop_count(), 0);
    assert!(harness.runtime.shutdown_targets().is_empty());
}

#[cfg(windows)]
#[test]
fn restart_stop_failure_prevents_run() {
    let harness = LifecycleHarness::new();
    harness.install_no_start();
    let (_, definition) = harness.installed_definition();
    harness.write_ready_runtime(
        &definition,
        harness.ids.old_instance_id,
        harness.ids.old_pid,
    );
    harness.runtime.set_sections([identity_mismatch_section(
        harness.ids.new_instance_id,
        harness.ids.new_pid,
    )]);
    harness.scheduler.set_scheduler_state(
        SchedulerState::Running,
        vec![SchedulerInstance {
            instance_id: harness.ids.scheduler_instance_id,
            state: SchedulerState::Running,
            engine_pid: Some(harness.ids.old_pid),
        }],
    );
    let lease = harness.hold_instance_lease();
    harness.release_instance_on_forced_stop(lease);
    harness.fail_next_scheduler_operation(SchedulerFaultPoint::Stop);
    let before = harness.durable_bytes();
    harness.clear_adapter_events();

    let error = harness.service.restart().unwrap_err();

    assert_eq!(error.code(), "MCP_SERVICE_SCHEDULER_FAILED");
    assert_eq!(harness.scheduler.stop_count(), 1);
    assert_eq!(harness.scheduler.run_count(), 0);
    assert_eq!(harness.durable_bytes().current, before.current);
    assert_eq!(harness.durable_bytes().runtime, before.runtime);
    let Document::Valid(lifecycle) = harness.store().read_lifecycle() else {
        panic!("failed restart stop phase should persist diagnostics")
    };
    assert_eq!(lifecycle.state, LifecycleStateKind::Failed);
    assert_eq!(
        lifecycle.diagnostic_code.as_deref(),
        Some("MCP_SERVICE_SCHEDULER_FAILED")
    );
}

#[cfg(windows)]
#[test]
fn restart_start_failure_leaves_stopped_evidence_and_later_start_converges() {
    let harness = LifecycleHarness::new();
    harness.install_no_start();
    let (_, definition) = harness.installed_definition();
    harness.write_ready_runtime(
        &definition,
        harness.ids.old_instance_id,
        harness.ids.old_pid,
    );
    harness.runtime.set_sections([
        ready_section(harness.ids.old_instance_id, harness.ids.old_pid),
        not_ready_section(),
        not_ready_section(),
    ]);
    let lease = harness.hold_instance_lease();
    harness.release_instance_on_shutdown(lease);
    harness.fail_next_scheduler_operation(SchedulerFaultPoint::Run);
    let current_before = std::fs::read(&harness.paths.current).unwrap();
    harness.clear_adapter_events();

    let error = harness.service.restart().unwrap_err();

    assert_eq!(error.code(), "MCP_SERVICE_SCHEDULER_FAILED");
    assert_eq!(
        harness.runtime.shutdown_targets(),
        vec![harness.ids.old_instance_id]
    );
    assert_eq!(harness.scheduler.stop_count(), 0);
    assert_eq!(harness.scheduler.run_count(), 1);
    let lease_proof = FileLease::try_acquire(&harness.paths.instance_lock)
        .unwrap()
        .expect("successful stop should release the instance lease");
    drop(lease_proof);
    let status = harness.service.status();
    assert_eq!(status.runtime.status, RuntimeStatus::Stopped);
    let Document::Valid(lifecycle) = harness.store().read_lifecycle() else {
        panic!("failed restart start phase should persist diagnostics")
    };
    assert_eq!(lifecycle.state, LifecycleStateKind::Failed);
    assert_eq!(
        lifecycle.diagnostic_code.as_deref(),
        Some("MCP_SERVICE_SCHEDULER_FAILED")
    );
    assert_eq!(
        std::fs::read(&harness.paths.current).unwrap(),
        current_before
    );
    let events = harness.adapter_events();
    let shutdown_index = events
        .iter()
        .position(|event| matches!(event, AdapterEvent::Runtime(RuntimeEvent::Shutdown { .. })))
        .expect("restart should stop the old instance");
    let first_run_index = events
        .iter()
        .position(|event| matches!(event, AdapterEvent::Scheduler(SchedulerEvent::Run { .. })))
        .expect("restart should enter the start phase");
    assert!(shutdown_index < first_run_index);
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

#[cfg(windows)]
#[test]
fn task_drift_after_successful_stop_prevents_restart_run() {
    let harness = LifecycleHarness::new();
    harness.install_no_start();
    let (_, definition) = harness.installed_definition();
    harness.write_ready_runtime(
        &definition,
        harness.ids.old_instance_id,
        harness.ids.old_pid,
    );
    harness.runtime.set_sections([identity_mismatch_section(
        harness.ids.new_instance_id,
        harness.ids.new_pid,
    )]);
    harness.scheduler.set_scheduler_state(
        SchedulerState::Running,
        vec![SchedulerInstance {
            instance_id: harness.ids.scheduler_instance_id,
            state: SchedulerState::Running,
            engine_pid: Some(harness.ids.old_pid),
        }],
    );
    let mut drifted = match harness.scheduler.observation() {
        TaskObservation::Owned(observed) => observed,
        _ => panic!("installed task should be owned"),
    };
    drifted.spec.enabled = false;
    drifted.scheduler_state = SchedulerState::Ready;
    harness
        .scheduler
        .queue_observation_after_stop(TaskObservation::Owned(drifted));
    let lease = harness.hold_instance_lease();
    harness.release_instance_on_forced_stop(lease);
    let current_before = std::fs::read(&harness.paths.current).unwrap();
    harness.clear_adapter_events();

    let error = harness.service.restart().unwrap_err();

    assert_eq!(error.code(), "MCP_SERVICE_CONFIGURATION_DRIFT");
    assert_eq!(harness.scheduler.stop_count(), 1);
    assert_eq!(harness.scheduler.run_count(), 0);
    assert!(harness.runtime.shutdown_targets().is_empty());
    assert_eq!(
        std::fs::read(&harness.paths.current).unwrap(),
        current_before
    );
    let TaskObservation::Owned(observed) = harness.scheduler.observation() else {
        panic!("task should remain owned after drift")
    };
    assert!(!observed.spec.enabled);
    let Document::Valid(lifecycle) = harness.store().read_lifecycle() else {
        panic!("post-stop drift should persist lifecycle diagnostics")
    };
    assert_eq!(lifecycle.state, LifecycleStateKind::Failed);
    assert_eq!(
        lifecycle.diagnostic_code.as_deref(),
        Some("MCP_SERVICE_CONFIGURATION_DRIFT")
    );
}

#[cfg(windows)]
#[test]
fn restart_uses_one_stop_start_sequence_and_requires_a_new_instance() {
    let temp = TempDir::new().unwrap();
    let paths = ServicePaths::from_base(temp.path().join("managed")).unwrap();
    let service = LifecycleService::new(paths.clone(), ScriptedScheduler::missing(), not_ready());
    service.install(&install_options(true)).unwrap();
    let (_, definition) = installed_definition(&service);
    let old_instance = Uuid::new_v4();
    let new_instance = Uuid::new_v4();
    write_ready_runtime(&service, &definition, old_instance, 86);
    service.readiness.set_sections([
        ready_section(old_instance, 86),
        not_ready_section(),
        not_ready_section(),
        ready_section(new_instance, 87),
    ]);
    service
        .readiness
        .release_instance_on_shutdown(FileLease::try_acquire(&paths.instance_lock).unwrap());

    let output = service.restart().unwrap();

    assert!(output.changed);
    assert_eq!(output.action, "restarted");
    assert_eq!(service.scheduler.run_count(), 1);
    assert_eq!(service.readiness.shutdown_targets(), vec![old_instance]);
}

#[cfg(windows)]
#[test]
fn restart_rejects_readiness_that_reuses_the_old_instance_identity() {
    let temp = TempDir::new().unwrap();
    let paths = ServicePaths::from_base(temp.path().join("managed")).unwrap();
    let service = LifecycleService::new(paths.clone(), ScriptedScheduler::missing(), not_ready());
    service.install(&install_options(true)).unwrap();
    let (_, definition) = installed_definition(&service);
    let instance_id = Uuid::new_v4();
    write_ready_runtime(&service, &definition, instance_id, 91);
    service.readiness.set_sections([
        ready_section(instance_id, 91),
        not_ready_section(),
        not_ready_section(),
        ready_section(instance_id, 92),
    ]);
    service
        .readiness
        .release_instance_on_shutdown(FileLease::try_acquire(&paths.instance_lock).unwrap());

    let error = service.restart().unwrap_err();

    assert_eq!(error.code(), "MCP_SERVICE_RESTART_FAILED");
    assert_eq!(service.scheduler.run_count(), 1);
}
