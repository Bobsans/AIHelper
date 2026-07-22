use std::sync::atomic::Ordering;

use tempfile::TempDir;

use super::{super::*, harness::*};

#[cfg(windows)]
#[test]
fn stop_reports_already_stopped_only_with_complete_quiescence_proof() {
    let temp = TempDir::new().unwrap();
    let paths = ServicePaths::from_base(temp.path().join("managed")).unwrap();
    let service = LifecycleService::new(paths, FakeScheduler::missing(), not_ready());
    service.install(&install_options(true)).unwrap();

    let output = service.stop().unwrap();

    assert!(!output.changed);
    assert_eq!(output.action, "already_stopped");
    assert_eq!(output.runtime, RuntimeStatus::Stopped);
    assert_eq!(service.scheduler.stop_count.load(Ordering::Relaxed), 0);
    assert!(
        service
            .readiness
            .shutdown_targets
            .lock()
            .unwrap()
            .is_empty()
    );
}

#[cfg(windows)]
#[test]
fn exact_live_stop_uses_control_identity_and_retains_quiescence_guard() {
    let temp = TempDir::new().unwrap();
    let paths = ServicePaths::from_base(temp.path().join("managed")).unwrap();
    let service = LifecycleService::new(paths.clone(), FakeScheduler::missing(), not_ready());
    service.install(&install_options(true)).unwrap();
    let (_, definition) = installed_definition(&service);
    let instance_id = Uuid::new_v4();
    write_ready_runtime(&service, &definition, instance_id, 41);
    service
        .readiness
        .set_sections([ready_section(instance_id, 41), not_ready_section()]);
    *service.readiness.release_on_shutdown.lock().unwrap() =
        FileLease::try_acquire(&paths.instance_lock).unwrap();

    let output = service.stop().unwrap();

    assert!(output.changed);
    assert_eq!(output.action, "stopped");
    assert_eq!(
        *service.readiness.shutdown_targets.lock().unwrap(),
        vec![instance_id]
    );
    assert_eq!(service.scheduler.stop_count.load(Ordering::Relaxed), 0);
}

#[cfg(windows)]
#[test]
fn control_identity_mismatch_uses_only_exact_scheduler_fallback() {
    let temp = TempDir::new().unwrap();
    let paths = ServicePaths::from_base(temp.path().join("managed")).unwrap();
    let service = LifecycleService::new(paths, FakeScheduler::missing(), not_ready());
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
    assert!(
        service
            .readiness
            .shutdown_targets
            .lock()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        *service.scheduler.stop_targets.lock().unwrap(),
        vec![SchedulerStopTarget::Running {
            instance_id: scheduler_id,
            expected_pid: 52,
        }]
    );
}

#[cfg(windows)]
#[test]
fn queued_fallback_requires_the_exact_process_free_instance() {
    let temp = TempDir::new().unwrap();
    let paths = ServicePaths::from_base(temp.path().join("managed")).unwrap();
    let service = LifecycleService::new(paths, FakeScheduler::missing(), not_ready());
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
        *service.scheduler.stop_targets.lock().unwrap(),
        vec![SchedulerStopTarget::Queued { instance_id }]
    );
}

#[cfg(windows)]
#[test]
fn pid_mismatch_and_task_drift_never_reach_scheduler_stop() {
    let temp = TempDir::new().unwrap();
    let paths = ServicePaths::from_base(temp.path().join("managed")).unwrap();
    let service = LifecycleService::new(paths, FakeScheduler::missing(), not_ready());
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
    assert_eq!(service.scheduler.stop_count.load(Ordering::Relaxed), 0);

    let mut observation = service.scheduler.observation.lock().unwrap();
    let TaskObservation::Owned(observed) = &mut *observation else {
        panic!("task should be owned")
    };
    observed.spec.executable_path = temp.path().join("changed.exe");
    drop(observation);
    service.scheduler.instances.lock().unwrap()[0].engine_pid = Some(63);
    let error = service.stop().unwrap_err();
    assert_eq!(error.code(), "MCP_SERVICE_STOP_UNSAFE");
    assert_eq!(service.scheduler.stop_count.load(Ordering::Relaxed), 0);
}

#[cfg(windows)]
#[test]
fn scheduler_stop_without_lease_release_times_out() {
    let temp = TempDir::new().unwrap();
    let paths = ServicePaths::from_base(temp.path().join("managed")).unwrap();
    let mut service = LifecycleService::new(paths.clone(), FakeScheduler::missing(), not_ready());
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
    assert_eq!(service.scheduler.stop_count.load(Ordering::Relaxed), 1);
}

#[cfg(windows)]
#[test]
fn restart_uses_one_stop_start_sequence_and_requires_a_new_instance() {
    let temp = TempDir::new().unwrap();
    let paths = ServicePaths::from_base(temp.path().join("managed")).unwrap();
    let service = LifecycleService::new(paths.clone(), FakeScheduler::missing(), not_ready());
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
    *service.readiness.release_on_shutdown.lock().unwrap() =
        FileLease::try_acquire(&paths.instance_lock).unwrap();

    let output = service.restart().unwrap();

    assert!(output.changed);
    assert_eq!(output.action, "restarted");
    assert_eq!(service.scheduler.run_count.load(Ordering::Relaxed), 1);
    assert_eq!(
        *service.readiness.shutdown_targets.lock().unwrap(),
        vec![old_instance]
    );
}

#[cfg(windows)]
#[test]
fn restart_rejects_readiness_that_reuses_the_old_instance_identity() {
    let temp = TempDir::new().unwrap();
    let paths = ServicePaths::from_base(temp.path().join("managed")).unwrap();
    let service = LifecycleService::new(paths.clone(), FakeScheduler::missing(), not_ready());
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
    *service.readiness.release_on_shutdown.lock().unwrap() =
        FileLease::try_acquire(&paths.instance_lock).unwrap();

    let error = service.restart().unwrap_err();

    assert_eq!(error.code(), "MCP_SERVICE_RESTART_FAILED");
    assert_eq!(service.scheduler.run_count.load(Ordering::Relaxed), 1);
}
