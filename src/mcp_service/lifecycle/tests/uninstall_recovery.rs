use std::sync::atomic::Ordering;

use tempfile::TempDir;

use super::{super::*, harness::*};

#[cfg(windows)]
#[test]
fn uninstall_is_idempotent_and_removes_only_verified_metadata() {
    let temp = TempDir::new().unwrap();
    let paths = ServicePaths::from_base(temp.path().join("managed")).unwrap();
    let service = LifecycleService::new(paths.clone(), FakeScheduler::missing(), not_ready());
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
    assert_eq!(service.scheduler.delete_count.load(Ordering::Relaxed), 1);
    assert!(!paths.current.exists());
    assert!(!paths.runtime.exists());
    assert!(!paths.lifecycle.exists());
    assert!(!pointer.definition_path.exists());
    assert!(unexpected.exists());
    assert!(paths.lifecycle_lock.exists());
    assert!(paths.instance_lock.exists());
}

#[cfg(windows)]
#[test]
fn already_absent_uninstall_has_nullable_identity_and_no_change() {
    let temp = TempDir::new().unwrap();
    let paths = ServicePaths::from_base(temp.path().join("managed")).unwrap();
    let service = LifecycleService::new(paths.clone(), FakeScheduler::missing(), not_ready());

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
    let temp = TempDir::new().unwrap();
    let paths = ServicePaths::from_base(temp.path().join("managed")).unwrap();
    let service = LifecycleService::new(paths.clone(), FakeScheduler::missing(), not_ready());
    service.install(&install_options(true)).unwrap();
    let current = std::fs::read(&paths.current).unwrap();
    *service.scheduler.observation.lock().unwrap() = TaskObservation::Foreign {
        source: Some("Other".to_owned()),
        uri: None,
    };
    let error = service.uninstall().unwrap_err();
    assert_eq!(error.code(), "MCP_SERVICE_TASK_CONFLICT");
    assert_eq!(std::fs::read(&paths.current).unwrap(), current);
    assert_eq!(service.scheduler.delete_count.load(Ordering::Relaxed), 0);

    let newer = br#"{"schema_version":2}"#;
    std::fs::write(&paths.lifecycle, newer).unwrap();
    let error = service.uninstall().unwrap_err();
    assert_eq!(error.code(), "MCP_SERVICE_STATE_INVALID");
    assert_eq!(std::fs::read(&paths.lifecycle).unwrap(), newer);
}
