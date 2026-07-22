use std::{thread, time::Duration};

use super::{super::*, harness::*};

#[cfg(windows)]
#[test]
fn lifecycle_lease_serializes_mutators_while_status_remains_observable() {
    let harness = LifecycleHarness::new();
    harness.install_no_start();
    harness.runtime.set_sections([
        not_ready_section(),
        not_ready_section(),
        ready_section(harness.ids.new_instance_id, harness.ids.new_pid),
    ]);
    let gate = harness.block_next_run();
    harness.clear_adapter_events();
    let restart_service = harness.service.clone();
    let restart_worker = thread::spawn(move || restart_service.restart());

    let entered = gate.wait_until_entered(Duration::from_secs(1));
    if !entered {
        gate.release();
        let _ = restart_worker.join();
        panic!("restart did not reach the scripted run boundary");
    }
    let status = harness.service.status();
    let mutations_before_second = harness.scheduler.mutation_events();
    let second_mutator = harness.service.start();
    let mutations_after_second = harness.scheduler.mutation_events();
    gate.release();
    let restart = restart_worker
        .join()
        .expect("restart worker should not panic")
        .expect("restart should complete after the run gate is released");

    assert_eq!(status.lifecycle.status, LifecycleStatus::Busy);
    let operation = status
        .lifecycle
        .operation
        .expect("status should expose the active lifecycle operation");
    assert_eq!(operation.operation, LifecycleOperation::Restart);
    assert_eq!(second_mutator.unwrap_err().code(), "MCP_SERVICE_BUSY");
    assert_eq!(mutations_after_second, mutations_before_second);
    assert_eq!(
        mutations_before_second,
        vec![SchedulerEvent::Run {
            task_path: restart.task_path.clone(),
        }]
    );
    assert!(restart.changed);
    assert_eq!(restart.action, "started");
    assert_eq!(restart.runtime, RuntimeStatus::Ready);
    assert_eq!(harness.scheduler.run_count(), 1);
}
