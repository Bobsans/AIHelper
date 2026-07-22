use super::{
    super::*,
    harness::{not_ready_section, ready_section},
};
use crate::mcp_service::model::LastExit;

fn failed_runtime(exit_code: i32) -> RuntimeState {
    let timestamp = now_timestamp();
    RuntimeState {
        schema_version: SCHEMA_VERSION,
        service_id: Uuid::new_v4(),
        configuration_id: Uuid::new_v4(),
        phase: RuntimePhase::Failed,
        pid: 41,
        version: env!("CARGO_PKG_VERSION").to_owned(),
        instance_id: Uuid::new_v4(),
        endpoint: "http://127.0.0.1:8787/mcp".to_owned(),
        started_at: timestamp.clone(),
        updated_at: timestamp,
        last_exit: Some(LastExit {
            kind: ExitKind::RuntimeFailure,
            exit_code,
            diagnostic_code: Some("MCP_SERVER_FAILED".to_owned()),
        }),
    }
}

fn scheduler_evidence(
    state: SchedulerState,
    last_result: Option<i32>,
    canonical_restart_policy: bool,
) -> SchedulerRuntimeEvidence {
    SchedulerRuntimeEvidence {
        state,
        last_result,
        canonical_restart_policy,
    }
}

#[test]
fn runtime_reducer_requires_complete_restart_backoff_evidence() {
    let readiness = not_ready_section();
    let failed = failed_runtime(1);
    let backoff = scheduler_evidence(SchedulerState::Queued, Some(1), true);

    assert_eq!(
        reduce_runtime(
            Some(&failed),
            &readiness,
            backoff,
            LifecycleStatus::Idle,
            false,
        ),
        RuntimeStatus::RestartBackoff
    );

    for incomplete in [
        scheduler_evidence(SchedulerState::Queued, None, true),
        scheduler_evidence(SchedulerState::Queued, Some(0), true),
        scheduler_evidence(SchedulerState::Queued, Some(1), false),
    ] {
        assert_eq!(
            reduce_runtime(
                Some(&failed),
                &readiness,
                incomplete,
                LifecycleStatus::Idle,
                false,
            ),
            RuntimeStatus::Starting
        );
    }

    let zero_exit = failed_runtime(0);
    assert_eq!(
        reduce_runtime(
            Some(&zero_exit),
            &readiness,
            backoff,
            LifecycleStatus::Idle,
            false,
        ),
        RuntimeStatus::Starting
    );
    assert_eq!(
        reduce_runtime(
            Some(&failed),
            &readiness,
            backoff,
            LifecycleStatus::Busy,
            false,
        ),
        RuntimeStatus::Starting
    );

    let mut stopped = failed.clone();
    stopped.phase = RuntimePhase::Stopped;
    stopped.last_exit = Some(LastExit {
        kind: ExitKind::Clean,
        exit_code: 0,
        diagnostic_code: None,
    });
    assert_eq!(
        reduce_runtime(
            Some(&stopped),
            &readiness,
            backoff,
            LifecycleStatus::Idle,
            false,
        ),
        RuntimeStatus::Starting
    );
}

#[test]
fn runtime_reducer_preserves_live_evidence_precedence() {
    let failed = failed_runtime(1);
    let backoff = scheduler_evidence(SchedulerState::Queued, Some(1), true);
    let ready = ready_section(failed.instance_id, failed.pid);

    assert_eq!(
        reduce_runtime(Some(&failed), &ready, backoff, LifecycleStatus::Idle, false,),
        RuntimeStatus::Ready
    );
    assert_eq!(
        reduce_runtime(
            Some(&failed),
            &not_ready_section(),
            backoff,
            LifecycleStatus::Idle,
            true,
        ),
        RuntimeStatus::RunningNotReady
    );
    assert_eq!(
        reduce_runtime(
            Some(&failed),
            &not_ready_section(),
            scheduler_evidence(SchedulerState::Running, Some(1), true),
            LifecycleStatus::Idle,
            false,
        ),
        RuntimeStatus::RunningNotReady
    );
}

#[test]
fn scheduler_hresult_is_preserved_for_structured_status() {
    let error = AppError::external(
        "MCP_SERVICE_SCHEDULER_FAILED",
        "Task Scheduler operation 'inspect' failed (hresult=-2147024891 0x80070005)",
    );
    assert_eq!(
        scheduler_error_values(&error),
        (Some(-2_147_024_891), Some("0x80070005".to_owned()))
    );
}
