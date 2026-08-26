#![allow(dead_code)]

use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex, MutexGuard, mpsc},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use tempfile::TempDir;

use super::super::*;
use crate::{
    lock,
    readiness::ReadinessProbe,
    scheduler::{SchedulerDeleteReceipt, SchedulerRunReceipt, SchedulerStopReceipt},
};

const TEST_START_TIMEOUT: Duration = Duration::from_millis(25);
const TEST_STOP_TIMEOUT: Duration = Duration::from_millis(25);
const TEST_STOP_GRACE_TIMEOUT: Duration = Duration::from_millis(5);
const TEST_POLL_INTERVAL: Duration = Duration::from_millis(1);
const TEST_GATE_TIMEOUT: Duration = Duration::from_secs(5);

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(crate) struct LeaseHolder {
    release: Option<mpsc::Sender<()>>,
    worker: Option<JoinHandle<()>>,
}

impl LeaseHolder {
    pub(crate) fn acquire(path: &Path) -> Self {
        let path = path.to_path_buf();
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let (release_sender, release_receiver) = mpsc::channel();
        let worker = thread::spawn(move || {
            let lease = match lock::try_acquire(&path) {
                Ok(Some(lease)) => {
                    let _ = ready_sender.send(Ok(()));
                    lease
                }
                Ok(None) => {
                    let _ = ready_sender.send(Err(format!(
                        "instance lease '{}' is already occupied",
                        path.display()
                    )));
                    return;
                }
                Err(error) => {
                    let _ = ready_sender.send(Err(format!(
                        "failed to acquire instance lease '{}': {}",
                        path.display(),
                        error.detail_message()
                    )));
                    return;
                }
            };
            let _ = release_receiver.recv();
            drop(lease);
        });
        match ready_receiver.recv_timeout(TEST_GATE_TIMEOUT) {
            Ok(Ok(())) => Self {
                release: Some(release_sender),
                worker: Some(worker),
            },
            Ok(Err(detail)) => {
                let _ = worker.join();
                panic!("{detail}")
            }
            Err(error) => {
                drop(release_sender);
                let _ = worker.join();
                panic!("timed out acquiring test instance lease: {error}")
            }
        }
    }
}

impl Drop for LeaseHolder {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SchedulerEvent {
    Inspect { task_path: String },
    Register { desired: Box<DesiredTaskSpec> },
    Run { task_path: String },
    Instances { task_path: String },
    StopInstance { target: SchedulerStopTarget },
    DeleteOwned { expected: ExpectedTaskOwnership },
}

impl SchedulerEvent {
    fn is_mutation(&self) -> bool {
        matches!(
            self,
            Self::Register { .. }
                | Self::Run { .. }
                | Self::StopInstance { .. }
                | Self::DeleteOwned { .. }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RuntimeEvent {
    Inspect {
        service_id: Uuid,
        configuration_id: Uuid,
        runtime_instance_id: Option<Uuid>,
    },
    Shutdown {
        service_id: Uuid,
        instance_id: Uuid,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AdapterEvent {
    Scheduler(SchedulerEvent),
    Runtime(RuntimeEvent),
}

type EventJournal = Arc<Mutex<Vec<AdapterEvent>>>;

fn push_event(journal: &EventJournal, event: AdapterEvent) {
    lock_unpoisoned(journal).push(event);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SchedulerFaultPoint {
    Inspect,
    Register,
    Run,
    Instances,
    Stop,
    Delete,
}

#[derive(Debug, Clone)]
struct ScriptedFailure {
    code: String,
    message: String,
}

impl ScriptedFailure {
    fn scheduler(point: SchedulerFaultPoint) -> Self {
        let operation = match point {
            SchedulerFaultPoint::Inspect => "inspect",
            SchedulerFaultPoint::Register => "register",
            SchedulerFaultPoint::Run => "run",
            SchedulerFaultPoint::Instances => "instances",
            SchedulerFaultPoint::Stop => "stop",
            SchedulerFaultPoint::Delete => "delete",
        };
        Self {
            code: "MCP_SERVICE_SCHEDULER_FAILED".to_owned(),
            message: format!("scripted Task Scheduler {operation} failure"),
        }
    }

    fn into_error(self) -> AppError {
        AppError::external(self.code, self.message)
    }
}

#[derive(Default)]
struct SchedulerFaults {
    inspect: VecDeque<ScriptedFailure>,
    register: VecDeque<ScriptedFailure>,
    run: VecDeque<ScriptedFailure>,
    instances: VecDeque<ScriptedFailure>,
    stop: VecDeque<ScriptedFailure>,
    delete: VecDeque<ScriptedFailure>,
}

impl SchedulerFaults {
    fn queue(&mut self, point: SchedulerFaultPoint, failure: ScriptedFailure) {
        match point {
            SchedulerFaultPoint::Inspect => self.inspect.push_back(failure),
            SchedulerFaultPoint::Register => self.register.push_back(failure),
            SchedulerFaultPoint::Run => self.run.push_back(failure),
            SchedulerFaultPoint::Instances => self.instances.push_back(failure),
            SchedulerFaultPoint::Stop => self.stop.push_back(failure),
            SchedulerFaultPoint::Delete => self.delete.push_back(failure),
        }
    }

    fn take(&mut self, point: SchedulerFaultPoint) -> Option<ScriptedFailure> {
        match point {
            SchedulerFaultPoint::Inspect => self.inspect.pop_front(),
            SchedulerFaultPoint::Register => self.register.pop_front(),
            SchedulerFaultPoint::Run => self.run.pop_front(),
            SchedulerFaultPoint::Instances => self.instances.pop_front(),
            SchedulerFaultPoint::Stop => self.stop.pop_front(),
            SchedulerFaultPoint::Delete => self.delete.pop_front(),
        }
    }
}

struct ScriptedSchedulerState {
    observation: TaskObservation,
    instances: Vec<SchedulerInstance>,
    faults: SchedulerFaults,
    register_readbacks: VecDeque<ObservedTask>,
    observations_after_stop: VecDeque<TaskObservation>,
    run_gates: VecDeque<RunGate>,
    release_on_stop: Option<LeaseHolder>,
}

#[derive(Clone)]
pub(crate) struct ScriptedScheduler {
    state: Arc<Mutex<ScriptedSchedulerState>>,
    journal: EventJournal,
}

impl ScriptedScheduler {
    pub(crate) fn missing() -> Self {
        Self::with_journal(TaskObservation::Missing, Arc::new(Mutex::new(Vec::new())))
    }

    pub(crate) fn foreign() -> Self {
        Self::with_journal(
            TaskObservation::Foreign {
                source: Some("Other".to_owned()),
                uri: None,
            },
            Arc::new(Mutex::new(Vec::new())),
        )
    }

    fn with_journal(observation: TaskObservation, journal: EventJournal) -> Self {
        Self {
            state: Arc::new(Mutex::new(ScriptedSchedulerState {
                observation,
                instances: Vec::new(),
                faults: SchedulerFaults::default(),
                register_readbacks: VecDeque::new(),
                observations_after_stop: VecDeque::new(),
                run_gates: VecDeque::new(),
                release_on_stop: None,
            })),
            journal,
        }
    }

    pub(crate) fn observation(&self) -> TaskObservation {
        lock_unpoisoned(&self.state).observation.clone()
    }

    pub(crate) fn set_observation(&self, observation: TaskObservation) {
        lock_unpoisoned(&self.state).observation = observation;
    }

    pub(crate) fn update_observed(&self, update: impl FnOnce(&mut ObservedTask)) {
        let mut state = lock_unpoisoned(&self.state);
        let TaskObservation::Owned(observed) = &mut state.observation else {
            panic!("task should be owned")
        };
        update(observed);
    }

    pub(crate) fn set_scheduler_state(
        &self,
        scheduler_state: SchedulerState,
        instances: Vec<SchedulerInstance>,
    ) {
        let mut state = lock_unpoisoned(&self.state);
        let TaskObservation::Owned(observed) = &mut state.observation else {
            panic!("task should be owned")
        };
        observed.scheduler_state = scheduler_state;
        state.instances = instances;
    }

    pub(crate) fn update_instances(&self, update: impl FnOnce(&mut Vec<SchedulerInstance>)) {
        update(&mut lock_unpoisoned(&self.state).instances);
    }

    pub(crate) fn queue_register_readback(&self, observed: ObservedTask) {
        lock_unpoisoned(&self.state)
            .register_readbacks
            .push_back(observed);
    }

    pub(crate) fn queue_observation_after_stop(&self, observation: TaskObservation) {
        lock_unpoisoned(&self.state)
            .observations_after_stop
            .push_back(observation);
    }

    pub(crate) fn fail_next(&self, point: SchedulerFaultPoint) {
        lock_unpoisoned(&self.state)
            .faults
            .queue(point, ScriptedFailure::scheduler(point));
    }

    pub(crate) fn fail_next_with(
        &self,
        point: SchedulerFaultPoint,
        code: impl Into<String>,
        message: impl Into<String>,
    ) {
        lock_unpoisoned(&self.state).faults.queue(
            point,
            ScriptedFailure {
                code: code.into(),
                message: message.into(),
            },
        );
    }

    pub(crate) fn block_next_run(&self, gate: RunGate) {
        lock_unpoisoned(&self.state).run_gates.push_back(gate);
    }

    pub(crate) fn release_instance_on_stop(&self, lease: LeaseHolder) {
        lock_unpoisoned(&self.state).release_on_stop = Some(lease);
    }

    pub(crate) fn scheduler_events(&self) -> Vec<SchedulerEvent> {
        lock_unpoisoned(&self.journal)
            .iter()
            .filter_map(|event| match event {
                AdapterEvent::Scheduler(event) => Some(event.clone()),
                AdapterEvent::Runtime(_) => None,
            })
            .collect()
    }

    pub(crate) fn mutation_events(&self) -> Vec<SchedulerEvent> {
        self.scheduler_events()
            .into_iter()
            .filter(SchedulerEvent::is_mutation)
            .collect()
    }

    pub(crate) fn register_count(&self) -> usize {
        self.scheduler_events()
            .iter()
            .filter(|event| matches!(event, SchedulerEvent::Register { .. }))
            .count()
    }

    pub(crate) fn run_count(&self) -> usize {
        self.scheduler_events()
            .iter()
            .filter(|event| matches!(event, SchedulerEvent::Run { .. }))
            .count()
    }

    pub(crate) fn stop_count(&self) -> usize {
        self.scheduler_events()
            .iter()
            .filter(|event| matches!(event, SchedulerEvent::StopInstance { .. }))
            .count()
    }

    pub(crate) fn delete_count(&self) -> usize {
        self.scheduler_events()
            .iter()
            .filter(|event| matches!(event, SchedulerEvent::DeleteOwned { .. }))
            .count()
    }

    pub(crate) fn stop_targets(&self) -> Vec<SchedulerStopTarget> {
        self.scheduler_events()
            .into_iter()
            .filter_map(|event| match event {
                SchedulerEvent::StopInstance { target } => Some(target),
                _ => None,
            })
            .collect()
    }
}

impl SchedulerAdapter for ScriptedScheduler {
    fn inspect(&self, task_path: &str) -> Result<TaskObservation, AppError> {
        push_event(
            &self.journal,
            AdapterEvent::Scheduler(SchedulerEvent::Inspect {
                task_path: task_path.to_owned(),
            }),
        );
        let mut state = lock_unpoisoned(&self.state);
        if let Some(failure) = state.faults.take(SchedulerFaultPoint::Inspect) {
            return Err(failure.into_error());
        }
        Ok(state.observation.clone())
    }

    fn register(&self, desired: &DesiredTaskSpec) -> Result<ObservedTask, AppError> {
        push_event(
            &self.journal,
            AdapterEvent::Scheduler(SchedulerEvent::Register {
                desired: Box::new(desired.clone()),
            }),
        );
        let mut state = lock_unpoisoned(&self.state);
        if let Some(failure) = state.faults.take(SchedulerFaultPoint::Register) {
            return Err(failure.into_error());
        }
        let observed = state
            .register_readbacks
            .pop_front()
            .unwrap_or_else(|| ObservedTask {
                spec: desired.clone(),
                scheduler_state: SchedulerState::Ready,
                last_result: Some(0),
                last_run_at: None,
            });
        state.observation = TaskObservation::Owned(Box::new(observed.clone()));
        Ok(observed)
    }

    fn run(&self, task_path: &str) -> Result<SchedulerRunReceipt, AppError> {
        push_event(
            &self.journal,
            AdapterEvent::Scheduler(SchedulerEvent::Run {
                task_path: task_path.to_owned(),
            }),
        );
        let (failure, gate) = {
            let mut state = lock_unpoisoned(&self.state);
            (
                state.faults.take(SchedulerFaultPoint::Run),
                state.run_gates.pop_front(),
            )
        };
        if let Some(failure) = failure {
            return Err(failure.into_error());
        }
        if let Some(gate) = gate {
            gate.enter_and_wait()?;
        }
        Ok(SchedulerRunReceipt { submitted: true })
    }

    fn instances(&self, expected: &DesiredTaskSpec) -> Result<Vec<SchedulerInstance>, AppError> {
        push_event(
            &self.journal,
            AdapterEvent::Scheduler(SchedulerEvent::Instances {
                task_path: expected.task_path.clone(),
            }),
        );
        let mut state = lock_unpoisoned(&self.state);
        if let Some(failure) = state.faults.take(SchedulerFaultPoint::Instances) {
            return Err(failure.into_error());
        }
        Ok(state.instances.clone())
    }

    fn stop_instance(
        &self,
        _expected: &DesiredTaskSpec,
        target: &SchedulerStopTarget,
    ) -> Result<SchedulerStopReceipt, AppError> {
        push_event(
            &self.journal,
            AdapterEvent::Scheduler(SchedulerEvent::StopInstance {
                target: target.clone(),
            }),
        );
        let release = {
            let mut state = lock_unpoisoned(&self.state);
            if let Some(failure) = state.faults.take(SchedulerFaultPoint::Stop) {
                return Err(failure.into_error());
            }
            let target_id = match target {
                SchedulerStopTarget::Running { instance_id, .. }
                | SchedulerStopTarget::Queued { instance_id } => *instance_id,
            };
            state
                .instances
                .retain(|instance| instance.instance_id != target_id);
            let has_active_instances = !state.instances.is_empty();
            if let TaskObservation::Owned(observed) = &mut state.observation
                && !has_active_instances
            {
                observed.scheduler_state = SchedulerState::Ready;
            }
            if let Some(observation) = state.observations_after_stop.pop_front() {
                state.observation = observation;
            }
            state.release_on_stop.take()
        };
        drop(release);
        Ok(SchedulerStopReceipt { stopped: true })
    }

    fn delete_owned(
        &self,
        expected: &ExpectedTaskOwnership,
    ) -> Result<SchedulerDeleteReceipt, AppError> {
        push_event(
            &self.journal,
            AdapterEvent::Scheduler(SchedulerEvent::DeleteOwned {
                expected: expected.clone(),
            }),
        );
        let mut state = lock_unpoisoned(&self.state);
        if let Some(failure) = state.faults.take(SchedulerFaultPoint::Delete) {
            return Err(failure.into_error());
        }
        state.observation = TaskObservation::Missing;
        Ok(SchedulerDeleteReceipt { deleted: true })
    }
}

#[derive(Clone)]
pub(crate) struct RunGate {
    inner: Arc<(Mutex<RunGateState>, Condvar)>,
    timeout: Duration,
}

#[derive(Default)]
struct RunGateState {
    entered: bool,
    released: bool,
}

impl RunGate {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new((Mutex::new(RunGateState::default()), Condvar::new())),
            timeout: TEST_GATE_TIMEOUT,
        }
    }

    pub(crate) fn wait_until_entered(&self, timeout: Duration) -> bool {
        let (state, signal) = &*self.inner;
        let deadline = Instant::now() + timeout;
        let mut state = lock_unpoisoned(state);
        while !state.entered {
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            let (next, result) = signal
                .wait_timeout(state, deadline.saturating_duration_since(now))
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state = next;
            if result.timed_out() && !state.entered {
                return false;
            }
        }
        true
    }

    pub(crate) fn release(&self) {
        let (state, signal) = &*self.inner;
        let mut state = lock_unpoisoned(state);
        state.released = true;
        signal.notify_all();
    }

    fn enter_and_wait(&self) -> Result<(), AppError> {
        let (state, signal) = &*self.inner;
        let deadline = Instant::now() + self.timeout;
        let mut state = lock_unpoisoned(state);
        state.entered = true;
        signal.notify_all();
        while !state.released {
            let now = Instant::now();
            if now >= deadline {
                return Err(AppError::external(
                    "MCP_TEST_GATE_TIMEOUT",
                    "scripted Scheduler run gate was not released",
                ));
            }
            let (next, result) = signal
                .wait_timeout(state, deadline.saturating_duration_since(now))
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state = next;
            if result.timed_out() && !state.released {
                return Err(AppError::external(
                    "MCP_TEST_GATE_TIMEOUT",
                    "scripted Scheduler run gate was not released",
                ));
            }
        }
        Ok(())
    }
}

struct ScriptedRuntimeState {
    sections: VecDeque<ReadinessSection>,
    shutdown_receipts: VecDeque<ShutdownReceipt>,
    release_on_shutdown: Option<LeaseHolder>,
}

#[derive(Clone)]
pub(crate) struct ScriptedRuntimeControl {
    state: Arc<Mutex<ScriptedRuntimeState>>,
    journal: EventJournal,
}

impl ScriptedRuntimeControl {
    pub(crate) fn new(sections: impl IntoIterator<Item = ReadinessSection>) -> Self {
        Self::with_journal(sections, Arc::new(Mutex::new(Vec::new())))
    }

    fn with_journal(
        sections: impl IntoIterator<Item = ReadinessSection>,
        journal: EventJournal,
    ) -> Self {
        Self {
            state: Arc::new(Mutex::new(ScriptedRuntimeState {
                sections: sections.into_iter().collect(),
                shutdown_receipts: VecDeque::new(),
                release_on_shutdown: None,
            })),
            journal,
        }
    }

    pub(crate) fn set_sections(&self, sections: impl IntoIterator<Item = ReadinessSection>) {
        lock_unpoisoned(&self.state).sections = sections.into_iter().collect();
    }

    pub(crate) fn queue_readiness(&self, section: ReadinessSection) {
        lock_unpoisoned(&self.state).sections.push_back(section);
    }

    pub(crate) fn queue_shutdown_receipt(&self, receipt: ShutdownReceipt) {
        lock_unpoisoned(&self.state)
            .shutdown_receipts
            .push_back(receipt);
    }

    pub(crate) fn fail_next_shutdown(&self, detail: impl Into<String>) {
        self.queue_shutdown_receipt(ShutdownReceipt::Failed {
            detail: detail.into(),
        });
    }

    pub(crate) fn release_instance_on_shutdown(&self, lease: Option<LeaseHolder>) {
        lock_unpoisoned(&self.state).release_on_shutdown = lease;
    }

    pub(crate) fn runtime_events(&self) -> Vec<RuntimeEvent> {
        lock_unpoisoned(&self.journal)
            .iter()
            .filter_map(|event| match event {
                AdapterEvent::Runtime(event) => Some(event.clone()),
                AdapterEvent::Scheduler(_) => None,
            })
            .collect()
    }

    pub(crate) fn shutdown_targets(&self) -> Vec<Uuid> {
        self.runtime_events()
            .into_iter()
            .filter_map(|event| match event {
                RuntimeEvent::Shutdown { instance_id, .. } => Some(instance_id),
                RuntimeEvent::Inspect { .. } => None,
            })
            .collect()
    }
}

impl ReadinessProbe for ScriptedRuntimeControl {
    fn inspect(
        &self,
        definition: &ServiceDefinition,
        runtime: Option<&RuntimeState>,
        _grace_period: bool,
    ) -> ReadinessSection {
        push_event(
            &self.journal,
            AdapterEvent::Runtime(RuntimeEvent::Inspect {
                service_id: definition.service_id,
                configuration_id: definition.configuration_id,
                runtime_instance_id: runtime.map(|runtime| runtime.instance_id),
            }),
        );
        let mut state = lock_unpoisoned(&self.state);
        if state.sections.len() > 1 {
            state.sections.pop_front().unwrap_or_else(not_ready_section)
        } else {
            state
                .sections
                .front()
                .cloned()
                .unwrap_or_else(not_ready_section)
        }
    }
}

impl RuntimeControl for ScriptedRuntimeControl {
    fn shutdown(&self, definition: &ServiceDefinition, instance_id: Uuid) -> ShutdownReceipt {
        push_event(
            &self.journal,
            AdapterEvent::Runtime(RuntimeEvent::Shutdown {
                service_id: definition.service_id,
                instance_id,
            }),
        );
        let (release, receipt) = {
            let mut state = lock_unpoisoned(&self.state);
            (
                state.release_on_shutdown.take(),
                state
                    .shutdown_receipts
                    .pop_front()
                    .unwrap_or(ShutdownReceipt::Accepted),
            )
        };
        drop(release);
        receipt
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScenarioIds {
    pub(crate) service_id: Uuid,
    pub(crate) configuration_id: Uuid,
    pub(crate) replacement_configuration_id: Uuid,
    pub(crate) old_instance_id: Uuid,
    pub(crate) new_instance_id: Uuid,
    pub(crate) scheduler_instance_id: Uuid,
    pub(crate) old_pid: u32,
    pub(crate) new_pid: u32,
}

impl Default for ScenarioIds {
    fn default() -> Self {
        Self {
            service_id: Uuid::from_u128(0x10000000000000000000000000000001),
            configuration_id: Uuid::from_u128(0x20000000000000000000000000000001),
            replacement_configuration_id: Uuid::from_u128(0x20000000000000000000000000000002),
            old_instance_id: Uuid::from_u128(0x30000000000000000000000000000001),
            new_instance_id: Uuid::from_u128(0x30000000000000000000000000000002),
            scheduler_instance_id: Uuid::from_u128(0x40000000000000000000000000000001),
            old_pid: 4101,
            new_pid: 4102,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DurableBytes {
    pub(crate) current: Option<Vec<u8>>,
    pub(crate) definitions: Vec<(PathBuf, Vec<u8>)>,
    pub(crate) runtime: Option<Vec<u8>>,
    pub(crate) lifecycle: Option<Vec<u8>>,
}

fn read_optional(path: &Path) -> Option<Vec<u8>> {
    match std::fs::read(path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => panic!("failed to read '{}': {error}", path.display()),
    }
}

pub(crate) struct LifecycleHarness {
    _temp: TempDir,
    pub(crate) paths: ServicePaths,
    pub(crate) scheduler: ScriptedScheduler,
    pub(crate) runtime: ScriptedRuntimeControl,
    pub(crate) service: Arc<LifecycleService<ScriptedScheduler, ScriptedRuntimeControl>>,
    pub(crate) ids: ScenarioIds,
}

impl LifecycleHarness {
    pub(crate) fn new() -> Self {
        let temp = TempDir::new().unwrap();
        let paths = ServicePaths::from_base(temp.path().join("managed")).unwrap();
        let journal = Arc::new(Mutex::new(Vec::new()));
        let scheduler = ScriptedScheduler::with_journal(TaskObservation::Missing, journal.clone());
        let runtime = ScriptedRuntimeControl::with_journal([not_ready_section()], journal);
        let mut service = LifecycleService::new(paths.clone(), scheduler.clone(), runtime.clone());
        service.start_timeout = TEST_START_TIMEOUT;
        service.stop_timeout = TEST_STOP_TIMEOUT;
        service.stop_grace_timeout = TEST_STOP_GRACE_TIMEOUT;
        service.poll_interval = TEST_POLL_INTERVAL;
        Self {
            _temp: temp,
            paths,
            scheduler,
            runtime,
            service: Arc::new(service),
            ids: ScenarioIds::default(),
        }
    }

    pub(crate) fn store(&self) -> &ServiceStore {
        &self.service.store
    }

    pub(crate) fn install_no_start(&self) -> MutationOutput {
        self.service.install(&install_options(true)).unwrap()
    }

    pub(crate) fn install_and_ready(&self, instance_id: Uuid, pid: u32) -> MutationOutput {
        self.runtime
            .set_sections([not_ready_section(), ready_section(instance_id, pid)]);
        self.service.install(&install_options(false)).unwrap()
    }

    pub(crate) fn installed_definition(&self) -> (CurrentPointer, ServiceDefinition) {
        installed_definition(&self.service)
    }

    pub(crate) fn set_runtime(&self, runtime: &RuntimeState) {
        self.store().write_runtime(runtime).unwrap();
    }

    pub(crate) fn write_ready_runtime(
        &self,
        definition: &ServiceDefinition,
        instance_id: Uuid,
        pid: u32,
    ) {
        write_ready_runtime(&self.service, definition, instance_id, pid);
    }

    pub(crate) fn set_owned_task(&self, observed: ObservedTask) {
        self.scheduler
            .set_observation(TaskObservation::Owned(Box::new(observed)));
    }

    pub(crate) fn queue_readiness(&self, section: ReadinessSection) {
        self.runtime.queue_readiness(section);
    }

    pub(crate) fn hold_instance_lease(&self) -> LeaseHolder {
        self.store().ensure_directories().unwrap();
        LeaseHolder::acquire(&self.paths.instance_lock)
    }

    pub(crate) fn release_instance_on_shutdown(&self, lease: LeaseHolder) {
        self.runtime.release_instance_on_shutdown(Some(lease));
    }

    pub(crate) fn release_instance_on_forced_stop(&self, lease: LeaseHolder) {
        self.scheduler.release_instance_on_stop(lease);
    }

    pub(crate) fn fail_next_scheduler_operation(&self, point: SchedulerFaultPoint) {
        self.scheduler.fail_next(point);
    }

    pub(crate) fn block_next_run(&self) -> RunGate {
        let gate = RunGate::new();
        self.scheduler.block_next_run(gate.clone());
        gate
    }

    pub(crate) fn adapter_events(&self) -> Vec<AdapterEvent> {
        lock_unpoisoned(&self.scheduler.journal).clone()
    }

    pub(crate) fn clear_adapter_events(&self) {
        lock_unpoisoned(&self.scheduler.journal).clear();
    }

    pub(crate) fn assert_no_destructive_events(&self) {
        let mutations = self.scheduler.mutation_events();
        assert!(
            mutations.is_empty(),
            "unexpected Scheduler mutation events: {mutations:?}"
        );
        assert!(
            self.runtime.shutdown_targets().is_empty(),
            "unexpected runtime shutdown event"
        );
    }

    pub(crate) fn durable_bytes(&self) -> DurableBytes {
        let definitions = self
            .store()
            .definition_files()
            .unwrap()
            .into_iter()
            .filter_map(|path| read_optional(&path).map(|bytes| (path, bytes)))
            .collect();
        DurableBytes {
            current: read_optional(&self.paths.current),
            definitions,
            runtime: read_optional(&self.paths.runtime),
            lifecycle: read_optional(&self.paths.lifecycle),
        }
    }
}

pub(crate) fn not_ready() -> ScriptedRuntimeControl {
    ScriptedRuntimeControl::new([not_ready_section()])
}

pub(crate) fn not_ready_section() -> ReadinessSection {
    ReadinessSection {
        status: ReadinessStatus::NotReady,
        http_status: None,
        version: None,
        instance_id: None,
        pid: None,
        diagnostic_code: Some("MCP_SERVICE_START_TIMEOUT".to_owned()),
    }
}

pub(crate) fn ready_section(instance_id: Uuid, pid: u32) -> ReadinessSection {
    ReadinessSection {
        status: ReadinessStatus::Ready,
        http_status: Some(200),
        version: Some(env!("CARGO_PKG_VERSION").to_owned()),
        instance_id: Some(instance_id),
        pid: Some(pid),
        diagnostic_code: None,
    }
}

pub(crate) fn install_options(no_start: bool) -> InstallSettings {
    InstallSettings {
        no_start,
        port: 8787,
        max_active: 32,
        default_timeout_ms: 300_000,
        limit: None,
    }
}

pub(crate) fn installed_definition(
    service: &LifecycleService<ScriptedScheduler, ScriptedRuntimeControl>,
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

pub(crate) fn write_ready_runtime(
    service: &LifecycleService<ScriptedScheduler, ScriptedRuntimeControl>,
    definition: &ServiceDefinition,
    instance_id: Uuid,
    pid: u32,
) {
    let mut runtime = RuntimeState::starting(definition, instance_id);
    runtime.phase = RuntimePhase::Ready;
    runtime.pid = pid;
    service.store.write_runtime(&runtime).unwrap();
}

pub(crate) fn set_scheduler_state(
    service: &LifecycleService<ScriptedScheduler, ScriptedRuntimeControl>,
    state: SchedulerState,
    instances: Vec<SchedulerInstance>,
) {
    service.scheduler.set_scheduler_state(state, instances);
}

pub(crate) fn identity_mismatch_section(instance_id: Uuid, pid: u32) -> ReadinessSection {
    ReadinessSection {
        status: ReadinessStatus::IdentityMismatch,
        http_status: Some(200),
        version: Some("foreign".to_owned()),
        instance_id: Some(instance_id),
        pid: Some(pid),
        diagnostic_code: Some("MCP_SERVICE_IDENTITY_MISMATCH".to_owned()),
    }
}
