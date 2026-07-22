use std::{
    collections::VecDeque,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use super::super::*;
use crate::{
    cli::GlobalOptions,
    mcp_service::{
        readiness::ReadinessProbe,
        scheduler::{SchedulerDeleteReceipt, SchedulerRunReceipt, SchedulerStopReceipt},
    },
    output::OutputMode,
};

pub(super) struct FakeScheduler {
    pub(super) observation: Mutex<TaskObservation>,
    pub(super) register_count: AtomicUsize,
    pub(super) run_count: AtomicUsize,
    pub(super) stop_count: AtomicUsize,
    pub(super) delete_count: AtomicUsize,
    pub(super) instances: Mutex<Vec<SchedulerInstance>>,
    pub(super) stop_targets: Mutex<Vec<SchedulerStopTarget>>,
    pub(super) fail_inspect: bool,
}

impl FakeScheduler {
    pub(super) fn missing() -> Self {
        Self {
            observation: Mutex::new(TaskObservation::Missing),
            register_count: AtomicUsize::new(0),
            run_count: AtomicUsize::new(0),
            stop_count: AtomicUsize::new(0),
            delete_count: AtomicUsize::new(0),
            instances: Mutex::new(Vec::new()),
            stop_targets: Mutex::new(Vec::new()),
            fail_inspect: false,
        }
    }

    pub(super) fn foreign() -> Self {
        Self {
            observation: Mutex::new(TaskObservation::Foreign {
                source: Some("Other".to_owned()),
                uri: None,
            }),
            ..Self::missing()
        }
    }
}

impl SchedulerAdapter for FakeScheduler {
    fn inspect(&self, _task_path: &str) -> Result<TaskObservation, AppError> {
        if self.fail_inspect {
            return Err(AppError::external(
                "MCP_SERVICE_SCHEDULER_FAILED",
                "fake scheduler failure",
            ));
        }
        Ok(self
            .observation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone())
    }

    fn register(&self, desired: &DesiredTaskSpec) -> Result<ObservedTask, AppError> {
        self.register_count.fetch_add(1, Ordering::Relaxed);
        let observed = ObservedTask {
            spec: desired.clone(),
            scheduler_state: SchedulerState::Ready,
            last_result: Some(0),
            last_run_at: None,
        };
        *self
            .observation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            TaskObservation::Owned(observed.clone());
        Ok(observed)
    }

    fn run(&self, _task_path: &str) -> Result<SchedulerRunReceipt, AppError> {
        self.run_count.fetch_add(1, Ordering::Relaxed);
        Ok(SchedulerRunReceipt { submitted: true })
    }

    fn instances(&self, _expected: &DesiredTaskSpec) -> Result<Vec<SchedulerInstance>, AppError> {
        Ok(self
            .instances
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone())
    }

    fn stop_instance(
        &self,
        _expected: &DesiredTaskSpec,
        target: &SchedulerStopTarget,
    ) -> Result<SchedulerStopReceipt, AppError> {
        self.stop_count.fetch_add(1, Ordering::Relaxed);
        self.stop_targets
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(target.clone());
        self.instances
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        if let TaskObservation::Owned(observed) = &mut *self
            .observation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
        {
            observed.scheduler_state = SchedulerState::Ready;
        }
        Ok(SchedulerStopReceipt { stopped: true })
    }

    fn delete_owned(
        &self,
        _expected: &ExpectedTaskOwnership,
    ) -> Result<SchedulerDeleteReceipt, AppError> {
        self.delete_count.fetch_add(1, Ordering::Relaxed);
        *self
            .observation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = TaskObservation::Missing;
        Ok(SchedulerDeleteReceipt { deleted: true })
    }
}

pub(super) struct FakeReadiness {
    pub(super) sections: Mutex<VecDeque<ReadinessSection>>,
    pub(super) shutdown_receipt: Mutex<ShutdownReceipt>,
    pub(super) shutdown_targets: Mutex<Vec<Uuid>>,
    pub(super) release_on_shutdown: Mutex<Option<FileLease>>,
}

impl FakeReadiness {
    pub(super) fn new(sections: impl IntoIterator<Item = ReadinessSection>) -> Self {
        Self {
            sections: Mutex::new(sections.into_iter().collect()),
            shutdown_receipt: Mutex::new(ShutdownReceipt::Accepted),
            shutdown_targets: Mutex::new(Vec::new()),
            release_on_shutdown: Mutex::new(None),
        }
    }

    pub(super) fn set_sections(&self, sections: impl IntoIterator<Item = ReadinessSection>) {
        *self
            .sections
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = sections.into_iter().collect();
    }
}

impl ReadinessProbe for FakeReadiness {
    fn inspect(
        &self,
        _definition: &ServiceDefinition,
        _runtime: Option<&RuntimeState>,
    ) -> ReadinessSection {
        let mut sections = self
            .sections
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if sections.len() > 1 {
            sections.pop_front().unwrap_or_else(not_ready_section)
        } else {
            sections.front().cloned().unwrap_or_else(not_ready_section)
        }
    }
}

impl RuntimeControl for FakeReadiness {
    fn shutdown(&self, _definition: &ServiceDefinition, instance_id: Uuid) -> ShutdownReceipt {
        self.shutdown_targets
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(instance_id);
        drop(
            self.release_on_shutdown
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take(),
        );
        self.shutdown_receipt
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

pub(super) fn not_ready() -> FakeReadiness {
    FakeReadiness::new([not_ready_section()])
}

pub(super) fn not_ready_section() -> ReadinessSection {
    ReadinessSection {
        status: ReadinessStatus::NotReady,
        http_status: None,
        version: None,
        instance_id: None,
        pid: None,
        diagnostic_code: Some("MCP_SERVICE_START_TIMEOUT".to_owned()),
    }
}

pub(super) fn ready_section(instance_id: Uuid, pid: u32) -> ReadinessSection {
    ReadinessSection {
        status: ReadinessStatus::Ready,
        http_status: Some(200),
        version: Some(env!("CARGO_PKG_VERSION").to_owned()),
        instance_id: Some(instance_id),
        pid: Some(pid),
        diagnostic_code: None,
    }
}

pub(super) fn install_options(no_start: bool) -> InstallOptions {
    InstallOptions {
        no_start,
        port: 8787,
        max_active: 32,
        default_timeout_ms: 300_000,
        options: GlobalOptions {
            output: OutputMode::Json,
            quiet: false,
            limit: None,
        },
    }
}

#[cfg(windows)]
pub(super) fn installed_definition(
    service: &LifecycleService<FakeScheduler, FakeReadiness>,
) -> (CurrentPointer, ServiceDefinition) {
    let Document::Valid(pointer) = service.store.read_current() else {
        panic!("current pointer should be valid")
    };
    let Document::Valid(definition) = service.store.read_definition(&pointer.definition_path)
    else {
        panic!("definition should be valid")
    };
    (pointer, definition)
}

#[cfg(windows)]
pub(super) fn write_ready_runtime(
    service: &LifecycleService<FakeScheduler, FakeReadiness>,
    definition: &ServiceDefinition,
    instance_id: Uuid,
    pid: u32,
) {
    let mut runtime = RuntimeState::starting(definition, instance_id);
    runtime.phase = RuntimePhase::Ready;
    runtime.pid = pid;
    service.store.write_runtime(&runtime).unwrap();
}

#[cfg(windows)]
pub(super) fn set_scheduler_state(
    service: &LifecycleService<FakeScheduler, FakeReadiness>,
    state: SchedulerState,
    instances: Vec<SchedulerInstance>,
) {
    let mut observation = service
        .scheduler
        .observation
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let TaskObservation::Owned(observed) = &mut *observation else {
        panic!("task should be owned")
    };
    observed.scheduler_state = state;
    *service
        .scheduler
        .instances
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = instances;
}

pub(super) fn identity_mismatch_section(instance_id: Uuid, pid: u32) -> ReadinessSection {
    ReadinessSection {
        status: ReadinessStatus::IdentityMismatch,
        http_status: Some(200),
        version: Some("foreign".to_owned()),
        instance_id: Some(instance_id),
        pid: Some(pid),
        diagnostic_code: Some("MCP_SERVICE_IDENTITY_MISMATCH".to_owned()),
    }
}
