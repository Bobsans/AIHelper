use tempfile::TempDir;

use super::{super::*, harness::*};

#[cfg(windows)]
#[test]
fn uninstall_is_idempotent_and_removes_only_verified_metadata() {
    let temp = TempDir::new().unwrap();
    let paths = ServicePaths::from_base(temp.path().join("managed")).unwrap();
    let service = LifecycleService::new(paths.clone(), ScriptedScheduler::missing(), not_ready());
    service.install(&install_options(true)).unwrap();
    let (pointer, _) = installed_definition(&service);
    let unexpected = paths
        .definitions_dir
        .join(format!("{}.json", Uuid::new_v4()));
    std::fs::write(&unexpected, b"malformed").unwrap();
    std::fs::write(&pointer.definition_path, b"malformed").unwrap();

    let output = service.uninstall().unwrap();

    assert!(output.changed);
    assert_eq!(output.action, "uninstalled");
    assert_eq!(service.scheduler.delete_count(), 1);
    assert!(!paths.current.exists());
    assert!(!paths.runtime.exists());
    assert!(!paths.lifecycle.exists());
    assert!(!pointer.definition_path.exists());
    assert!(unexpected.exists());
    // lifecycle_lock and instance_lock are now named mutexes, not files
}

#[cfg(windows)]
#[test]
fn running_uninstall_stops_deletes_then_cleans_only_semantic_metadata() {
    let harness = LifecycleHarness::new();
    harness.install_no_start();
    let (pointer, definition) = harness.installed_definition();
    harness.write_ready_runtime(
        &definition,
        harness.ids.old_instance_id,
        harness.ids.old_pid,
    );
    harness.runtime.set_sections([
        ready_section(harness.ids.old_instance_id, harness.ids.old_pid),
        not_ready_section(),
    ]);
    let lease = harness.hold_instance_lease();
    harness.release_instance_on_shutdown(lease);
    let sentinels = [
        harness.paths.base_dir.join("executable-sentinel.exe"),
        harness.paths.base_dir.join("configuration-sentinel.json"),
        harness.paths.base_dir.join("logs").join("service.log"),
        harness.paths.base_dir.join("plugins").join("plugin.dll"),
        harness.paths.base_dir.join("unexpected.bin"),
    ];
    for sentinel in &sentinels {
        if let Some(parent) = sentinel.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(sentinel, b"preserve").unwrap();
    }
    harness.clear_adapter_events();

    let output = harness.service.uninstall().unwrap();

    assert!(output.changed);
    assert_eq!(output.action, "uninstalled");
    assert_eq!(
        harness.runtime.shutdown_targets(),
        vec![harness.ids.old_instance_id]
    );
    assert_eq!(harness.scheduler.stop_count(), 0);
    assert_eq!(harness.scheduler.delete_count(), 1);
    let events = harness.adapter_events();
    let shutdown_index = events
        .iter()
        .position(|event| matches!(event, AdapterEvent::Runtime(RuntimeEvent::Shutdown { .. })))
        .expect("running uninstall should stop the exact runtime");
    let delete_index = events
        .iter()
        .position(|event| {
            matches!(
                event,
                AdapterEvent::Scheduler(SchedulerEvent::DeleteOwned { .. })
            )
        })
        .expect("owned task should be deleted");
    assert!(shutdown_index < delete_index);
    assert!(matches!(
        harness.scheduler.observation(),
        TaskObservation::Missing
    ));
    assert!(!harness.paths.current.exists());
    assert!(!harness.paths.runtime.exists());
    assert!(!harness.paths.lifecycle.exists());
    assert!(!pointer.definition_path.exists());
    for sentinel in sentinels {
        assert!(
            sentinel.exists(),
            "sentinel '{}' was removed",
            sentinel.display()
        );
    }
    // lifecycle_lock and instance_lock are now named mutexes, not files
}

#[cfg(windows)]
#[test]
fn delete_failure_starts_no_cleanup_and_retry_converges() {
    let harness = LifecycleHarness::new();
    harness.install_no_start();
    let (pointer, _) = harness.installed_definition();
    let current_before = std::fs::read(&harness.paths.current).unwrap();
    let definition_before = std::fs::read(&pointer.definition_path).unwrap();
    harness.fail_next_scheduler_operation(SchedulerFaultPoint::Delete);
    harness.clear_adapter_events();

    let error = harness.service.uninstall().unwrap_err();

    assert_eq!(error.code(), "MCP_SERVICE_SCHEDULER_FAILED");
    assert_eq!(harness.scheduler.delete_count(), 1);
    assert_eq!(harness.scheduler.stop_count(), 0);
    assert!(harness.runtime.shutdown_targets().is_empty());
    assert_eq!(
        std::fs::read(&harness.paths.current).unwrap(),
        current_before
    );
    assert_eq!(
        std::fs::read(&pointer.definition_path).unwrap(),
        definition_before
    );
    assert!(matches!(
        harness.scheduler.observation(),
        TaskObservation::Owned(_)
    ));
    let Document::Valid(lifecycle) = harness.store().read_lifecycle() else {
        panic!("delete failure should persist lifecycle diagnostics")
    };
    assert_eq!(lifecycle.state, LifecycleStateKind::Failed);
    assert_eq!(
        lifecycle.diagnostic_code.as_deref(),
        Some("MCP_SERVICE_SCHEDULER_FAILED")
    );

    let retry = harness.service.uninstall().unwrap();

    assert!(retry.changed);
    assert_eq!(retry.action, "uninstalled");
    assert_eq!(harness.scheduler.delete_count(), 2);
    assert!(!harness.paths.current.exists());
    assert!(!pointer.definition_path.exists());
    assert!(!harness.paths.lifecycle.exists());
    assert!(matches!(
        harness.scheduler.observation(),
        TaskObservation::Missing
    ));
}

#[cfg(windows)]
#[test]
fn missing_task_recovers_stale_and_partially_removed_metadata() {
    let stale = LifecycleHarness::new();
    stale.install_no_start();
    let (stale_pointer, _) = stale.installed_definition();
    stale.scheduler.set_observation(TaskObservation::Missing);
    stale.clear_adapter_events();

    let stale_output = stale.service.uninstall().unwrap();

    assert!(stale_output.changed);
    assert_eq!(stale_output.action, "uninstalled");
    assert_eq!(stale_output.service_id, Some(stale_pointer.service_id));
    assert_eq!(stale.scheduler.delete_count(), 0);
    assert!(!stale.paths.current.exists());
    assert!(!stale_pointer.definition_path.exists());
    assert!(!stale.paths.lifecycle.exists());

    let partial = LifecycleHarness::new();
    partial.install_no_start();
    let (partial_pointer, _) = partial.installed_definition();
    std::fs::remove_file(&partial_pointer.definition_path).unwrap();
    partial.scheduler.set_observation(TaskObservation::Missing);
    partial.clear_adapter_events();

    let partial_output = partial.service.uninstall().unwrap();

    assert!(partial_output.changed);
    assert_eq!(partial_output.action, "uninstalled");
    assert_eq!(partial_output.service_id, Some(partial_pointer.service_id));
    assert_eq!(partial.scheduler.delete_count(), 0);
    assert!(!partial.paths.current.exists());
    assert!(!partial_pointer.definition_path.exists());
    assert!(!partial.paths.lifecycle.exists());
}

#[cfg(windows)]
#[test]
fn occupied_orphan_without_runtime_identity_blocks_cleanup() {
    let harness = LifecycleHarness::new();
    harness.install_no_start();
    let (pointer, _) = harness.installed_definition();
    harness.scheduler.set_observation(TaskObservation::Missing);
    let occupied = harness.hold_instance_lease();
    let current_before = std::fs::read(&harness.paths.current).unwrap();
    let definition_before = std::fs::read(&pointer.definition_path).unwrap();
    harness.clear_adapter_events();

    let error = harness.service.uninstall().unwrap_err();

    assert_eq!(error.code(), "MCP_SERVICE_STOP_UNSAFE");
    harness.assert_no_destructive_events();
    assert_eq!(
        std::fs::read(&harness.paths.current).unwrap(),
        current_before
    );
    assert_eq!(
        std::fs::read(&pointer.definition_path).unwrap(),
        definition_before
    );
    assert!(!harness.paths.runtime.exists());
    let Document::Valid(lifecycle) = harness.store().read_lifecycle() else {
        panic!("unsafe orphan should persist lifecycle diagnostics")
    };
    assert_eq!(lifecycle.state, LifecycleStateKind::Failed);
    assert_eq!(
        lifecycle.diagnostic_code.as_deref(),
        Some("MCP_SERVICE_STOP_UNSAFE")
    );
    drop(occupied);
}

#[cfg(windows)]
#[test]
fn already_absent_uninstall_has_nullable_identity_and_no_change() {
    let temp = TempDir::new().unwrap();
    let paths = ServicePaths::from_base(temp.path().join("managed")).unwrap();
    let service = LifecycleService::new(paths.clone(), ScriptedScheduler::missing(), not_ready());

    let output = service.uninstall().unwrap();

    assert!(!output.changed);
    assert_eq!(output.action, "already_uninstalled");
    assert_eq!(output.service_id, None);
    assert_eq!(output.configuration_id, None);
    assert_eq!(output.endpoint, None);
    assert!(!paths.lifecycle.exists());
}

#[cfg(windows)]
#[test]
fn foreign_task_and_newer_lifecycle_state_are_preserved() {
    let harness = LifecycleHarness::new();
    harness.install_no_start();
    let before = harness.durable_bytes();
    harness.scheduler.set_observation(TaskObservation::Foreign {
        source: Some("Other".to_owned()),
        uri: None,
    });
    harness.clear_adapter_events();

    let error = harness.service.uninstall().unwrap_err();

    assert_eq!(error.code(), "MCP_SERVICE_TASK_CONFLICT");
    harness.assert_no_destructive_events();
    let after = harness.durable_bytes();
    assert_eq!(after.current, before.current);
    assert_eq!(after.definitions, before.definitions);
    assert_eq!(after.runtime, before.runtime);

    let newer = br#"{"schema_version":2}"#;
    std::fs::write(&harness.paths.lifecycle, newer).unwrap();
    let error = harness.service.uninstall().unwrap_err();
    assert_eq!(error.code(), "MCP_SERVICE_STATE_INVALID");
    assert_eq!(std::fs::read(&harness.paths.lifecycle).unwrap(), newer);
    harness.assert_no_destructive_events();
}
