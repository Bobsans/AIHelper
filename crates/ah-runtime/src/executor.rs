use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
    time::Duration,
};

use ah_plugin_api::{TypedInvocationRequest, TypedInvocationResponse};
use tokio::{
    runtime::Handle,
    sync::{Notify, OwnedSemaphorePermit, Semaphore, oneshot},
    time::Instant,
};

use crate::{PluginManager, RuntimeError};

pub type ExecutionFuture<'a> =
    Pin<Box<dyn Future<Output = Result<TypedInvocationResponse, RuntimeError>> + Send + 'a>>;
pub type ObservedExecutionFuture<'a> = Pin<Box<dyn Future<Output = ObservedExecution> + Send + 'a>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionTimeoutPhase {
    Queue,
    Execution,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecutionTelemetry {
    pub queue_wait_ms: u64,
    pub execution_ms: u64,
    pub timeout_phase: Option<ExecutionTimeoutPhase>,
}

pub struct ObservedExecution {
    pub result: Result<TypedInvocationResponse, RuntimeError>,
    pub telemetry: Option<ExecutionTelemetry>,
}

impl ObservedExecution {
    fn unobserved(result: Result<TypedInvocationResponse, RuntimeError>) -> Self {
        Self {
            result,
            telemetry: None,
        }
    }
}

pub trait Executor: Send + Sync {
    fn execute(&self, request: TypedInvocationRequest) -> ExecutionFuture<'_>;

    fn execute_observed(&self, request: TypedInvocationRequest) -> ObservedExecutionFuture<'_> {
        Box::pin(async move { ObservedExecution::unobserved(self.execute(request).await) })
    }

    fn try_submit(
        &self,
        _request: TypedInvocationRequest,
    ) -> Result<ExecutionHandle, RuntimeError> {
        Err(RuntimeError::InvalidExecutionRequest(
            "executor does not support detached submission".to_owned(),
        ))
    }

    fn cancel(&self, request_id: &str) -> bool;

    fn close(&self) {}

    fn active_count(&self) -> usize {
        0
    }
}

pub struct ExecutionHandle {
    request_id: String,
    completion: oneshot::Receiver<ObservedExecution>,
    lifecycle: ExecutionLifecycle,
}

impl ExecutionHandle {
    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    pub fn lifecycle(&self) -> ExecutionLifecycle {
        self.lifecycle.clone()
    }

    pub async fn observe(self) -> ObservedExecution {
        self.completion.await.unwrap_or_else(|_| {
            ObservedExecution::unobserved(Err(RuntimeError::ExecutionWorker(format!(
                "execution completion channel closed for request '{}'",
                self.request_id
            ))))
        })
    }
}

#[derive(Clone)]
pub struct ExecutionLifecycle {
    request_id: Arc<str>,
    state: Arc<ExecutionState>,
}

impl ExecutionLifecycle {
    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    pub fn is_logically_complete(&self) -> bool {
        self.state.logical_done.load(Ordering::Acquire)
    }

    pub fn is_physically_complete(&self) -> bool {
        self.state.physical_done.load(Ordering::Acquire)
    }

    pub fn is_draining(&self) -> bool {
        self.is_logically_complete() && !self.is_physically_complete()
    }

    pub async fn wait_physical(&self) {
        loop {
            if self.is_physically_complete() {
                return;
            }
            let changed = self.state.physical_changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.is_physically_complete() {
                return;
            }
            changed.await;
        }
    }
}

/// Maximum time a cancelled or timed-out handler is allowed to keep occupying
/// its execution slot while it drains. Once this elapses the slot is abandoned:
/// the permit and tracking are released so new work can be admitted, even though
/// the underlying blocking handler may still be running (blocking tasks cannot be
/// forcibly aborted). This bounds resource consumption from handlers that ignore
/// cooperative cancellation and never return.
pub const DEFAULT_DRAIN_GRACE: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct ParallelExecutor {
    manager: Arc<PluginManager>,
    runtime: Handle,
    permits: Arc<Semaphore>,
    max_active: usize,
    drain_grace: Duration,
    coordinator: Arc<Mutex<ExecutionCoordinator>>,
    closed: Arc<AtomicBool>,
}

impl ParallelExecutor {
    pub fn new(manager: Arc<PluginManager>, max_active: usize) -> Result<Self, RuntimeError> {
        if max_active == 0 {
            return Err(RuntimeError::InvalidExecutionRequest(
                "maximum active execution count must be greater than zero".to_owned(),
            ));
        }
        if max_active > Semaphore::MAX_PERMITS {
            return Err(RuntimeError::InvalidExecutionRequest(format!(
                "maximum active execution count must not exceed {}",
                Semaphore::MAX_PERMITS
            )));
        }
        let runtime = Handle::try_current().map_err(|error| {
            RuntimeError::ExecutionWorker(format!(
                "executor must be created inside a Tokio runtime: {error}"
            ))
        })?;
        Ok(Self {
            manager,
            runtime,
            permits: Arc::new(Semaphore::new(max_active)),
            max_active,
            drain_grace: DEFAULT_DRAIN_GRACE,
            coordinator: Arc::new(Mutex::new(ExecutionCoordinator::default())),
            closed: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Override the drain grace period. See [`DEFAULT_DRAIN_GRACE`].
    #[must_use]
    pub fn with_drain_grace(mut self, drain_grace: Duration) -> Self {
        self.drain_grace = drain_grace;
        self
    }

    pub fn max_active(&self) -> usize {
        self.max_active
    }

    pub fn is_draining(&self, request_id: &str) -> bool {
        lock_coordinator(&self.coordinator)
            .tracked
            .get(request_id)
            .is_some_and(|request| {
                request.state.logical_done.load(Ordering::Acquire)
                    && !request.state.physical_done.load(Ordering::Acquire)
            })
    }

    fn submit(&self, request: TypedInvocationRequest) -> Result<ExecutionHandle, RuntimeError> {
        validate_request(&request)?;
        if self.closed.load(Ordering::Acquire) {
            return Err(RuntimeError::ExecutorShuttingDown);
        }

        let accepted_at = Instant::now();
        let Some(deadline) =
            accepted_at.checked_add(Duration::from_millis(request.context.remaining_timeout_ms))
        else {
            return Err(RuntimeError::InvalidExecutionRequest(format!(
                "timeout is too large for request '{}'",
                request.context.request_id
            )));
        };
        let permit = Arc::clone(&self.permits).try_acquire_owned().map_err(|_| {
            RuntimeError::ExecutionCapacityFull {
                capacity: self.max_active,
            }
        })?;

        let request_id = request.context.request_id.clone();
        let command = request.command.clone();
        let (completion, response) = oneshot::channel();
        let state = Arc::new(ExecutionState::new(accepted_at, completion));
        {
            let mut coordinator = lock_coordinator(&self.coordinator);
            if self.closed.load(Ordering::Acquire) {
                drop(permit);
                return Err(RuntimeError::ExecutorShuttingDown);
            }
            if coordinator.tracked.contains_key(&request_id) {
                drop(permit);
                return Err(RuntimeError::InvalidExecutionRequest(format!(
                    "duplicate active request id '{request_id}'"
                )));
            }
            coordinator.tracked.insert(
                request_id.clone(),
                TrackedExecution {
                    command,
                    state: Arc::clone(&state),
                },
            );
        }

        self.runtime.spawn(run_execution(
            Arc::clone(&self.manager),
            Arc::clone(&self.coordinator),
            request,
            deadline,
            Arc::clone(&state),
            permit,
            self.drain_grace,
        ));

        Ok(ExecutionHandle {
            request_id: request_id.clone(),
            completion: response,
            lifecycle: ExecutionLifecycle {
                request_id: Arc::from(request_id),
                state,
            },
        })
    }

    fn cancel_all(&self) {
        let tracked = {
            let coordinator = lock_coordinator(&self.coordinator);
            coordinator
                .tracked
                .iter()
                .map(|(request_id, execution)| {
                    (
                        request_id.clone(),
                        execution.command.clone(),
                        Arc::clone(&execution.state),
                    )
                })
                .collect::<Vec<_>>()
        };
        for (request_id, command, state) in tracked {
            if state.stop(StopReason::Cancelled, &request_id) {
                self.dispatch_cancellation(command, request_id);
            }
        }
    }

    fn dispatch_cancellation(&self, command: String, request_id: String) {
        let manager = Arc::clone(&self.manager);
        self.runtime.spawn_blocking(move || {
            manager.cancel_typed(&command, &request_id);
        });
    }
}

impl Executor for ParallelExecutor {
    fn execute(&self, request: TypedInvocationRequest) -> ExecutionFuture<'_> {
        Box::pin(async move { self.execute_observed(request).await.result })
    }

    fn execute_observed(&self, request: TypedInvocationRequest) -> ObservedExecutionFuture<'_> {
        Box::pin(async move {
            match self.submit(request) {
                Ok(handle) => handle.observe().await,
                Err(error) => ObservedExecution::unobserved(Err(error)),
            }
        })
    }

    fn try_submit(&self, request: TypedInvocationRequest) -> Result<ExecutionHandle, RuntimeError> {
        self.submit(request)
    }

    fn cancel(&self, request_id: &str) -> bool {
        let tracked = lock_coordinator(&self.coordinator)
            .tracked
            .get(request_id)
            .cloned();
        let Some(tracked) = tracked else {
            return false;
        };
        if !tracked.state.stop(StopReason::Cancelled, request_id) {
            return false;
        }
        self.dispatch_cancellation(tracked.command, request_id.to_owned());
        true
    }

    fn close(&self) {
        if !self.closed.swap(true, Ordering::AcqRel) {
            self.cancel_all();
        }
    }

    fn active_count(&self) -> usize {
        self.max_active - self.permits.available_permits()
    }
}

#[derive(Default)]
struct ExecutionCoordinator {
    tracked: HashMap<String, TrackedExecution>,
}

#[derive(Clone)]
struct TrackedExecution {
    command: String,
    state: Arc<ExecutionState>,
}

struct ExecutionState {
    accepted_at: Instant,
    logical: Mutex<LogicalCompletion>,
    logical_done: AtomicBool,
    stop_reason: AtomicU8,
    stop_changed: Notify,
    physical_done: AtomicBool,
    physical_changed: Notify,
}

struct LogicalCompletion {
    sender: Option<oneshot::Sender<ObservedExecution>>,
}

impl ExecutionState {
    fn new(accepted_at: Instant, sender: oneshot::Sender<ObservedExecution>) -> Self {
        Self {
            accepted_at,
            logical: Mutex::new(LogicalCompletion {
                sender: Some(sender),
            }),
            logical_done: AtomicBool::new(false),
            stop_reason: AtomicU8::new(0),
            stop_changed: Notify::new(),
            physical_done: AtomicBool::new(false),
            physical_changed: Notify::new(),
        }
    }

    fn finish(&self, result: Result<TypedInvocationResponse, RuntimeError>) -> bool {
        self.complete(result, None)
    }

    fn stop(&self, reason: StopReason, request_id: &str) -> bool {
        let result = Err(reason.into_error(request_id.to_owned()));
        if !self.complete(result, Some(reason)) {
            return false;
        }
        self.stop_changed.notify_waiters();
        true
    }

    fn complete(
        &self,
        result: Result<TypedInvocationResponse, RuntimeError>,
        stop_reason: Option<StopReason>,
    ) -> bool {
        let sender = {
            let mut logical = self
                .logical
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(sender) = logical.sender.take() else {
                return false;
            };
            if let Some(reason) = stop_reason {
                self.stop_reason.store(reason as u8, Ordering::Release);
            }
            sender
        };
        let terminal_at = Instant::now();
        self.logical_done.store(true, Ordering::Release);
        let timeout_phase = matches!(result, Err(RuntimeError::ExecutionTimeout { .. }))
            .then_some(ExecutionTimeoutPhase::Execution);
        let observed = ObservedExecution {
            result,
            telemetry: Some(ExecutionTelemetry {
                queue_wait_ms: 0,
                execution_ms: elapsed_ms(self.accepted_at, terminal_at),
                timeout_phase,
            }),
        };
        let _ = sender.send(observed);
        true
    }

    fn stop_reason(&self) -> Option<StopReason> {
        StopReason::from_raw(self.stop_reason.load(Ordering::Acquire))
    }

    async fn wait_for_stop(&self) -> StopReason {
        loop {
            if let Some(reason) = self.stop_reason() {
                return reason;
            }
            let changed = self.stop_changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if let Some(reason) = self.stop_reason() {
                return reason;
            }
            changed.await;
        }
    }

    fn mark_physical_complete(&self) {
        self.physical_done.store(true, Ordering::Release);
        self.physical_changed.notify_waiters();
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum StopReason {
    Cancelled = 1,
    TimedOut = 2,
}

impl StopReason {
    fn from_raw(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Cancelled),
            2 => Some(Self::TimedOut),
            _ => None,
        }
    }

    fn into_error(self, request_id: String) -> RuntimeError {
        match self {
            Self::Cancelled => RuntimeError::ExecutionCancelled { request_id },
            Self::TimedOut => RuntimeError::ExecutionTimeout { request_id },
        }
    }
}

async fn run_execution(
    manager: Arc<PluginManager>,
    coordinator: Arc<Mutex<ExecutionCoordinator>>,
    mut request: TypedInvocationRequest,
    deadline: Instant,
    state: Arc<ExecutionState>,
    permit: OwnedSemaphorePermit,
    drain_grace: Duration,
) {
    let request_id = request.context.request_id.clone();
    let command = request.command.clone();
    let remaining = deadline.saturating_duration_since(Instant::now());
    request.context.remaining_timeout_ms = u64::try_from(remaining.as_millis())
        .unwrap_or(u64::MAX)
        .max(1);

    let manager_for_call = Arc::clone(&manager);
    let mut handler = tokio::task::spawn_blocking(move || manager_for_call.invoke_typed(&request));
    tokio::select! {
        biased;
        _ = state.wait_for_stop() => {
            drain_or_abandon(handler, drain_grace).await;
        }
        _ = tokio::time::sleep_until(deadline) => {
            if state.stop(StopReason::TimedOut, &request_id) {
                let manager_for_cancel = Arc::clone(&manager);
                let command_for_cancel = command.clone();
                let request_id_for_cancel = request_id.clone();
                tokio::task::spawn_blocking(move || {
                    manager_for_cancel.cancel_typed(&command_for_cancel, &request_id_for_cancel);
                });
            }
            drain_or_abandon(handler, drain_grace).await;
        }
        joined = &mut handler => {
            let result = match joined {
                Ok(result) => result,
                Err(error) if error.is_panic() => Err(RuntimeError::ExecutionPanic {
                    request_id: request_id.clone(),
                }),
                Err(error) => Err(RuntimeError::ExecutionWorker(format!(
                    "handler join failed for request '{request_id}': {error}"
                ))),
            };
            state.finish(result);
        }
    }

    remove_tracked(&coordinator, &request_id, &state);
    state.mark_physical_complete();
    drop(permit);
}

/// Wait for a stopped handler to drain, but give up after `grace`.
///
/// On timeout the [`JoinHandle`] is dropped without awaiting. Dropping a
/// `spawn_blocking` handle does not abort the underlying thread — it keeps
/// running until the blocking call returns — but the caller is released so the
/// execution slot (permit) and coordinator entry can be reclaimed immediately.
async fn drain_or_abandon(
    handler: tokio::task::JoinHandle<Result<TypedInvocationResponse, RuntimeError>>,
    grace: Duration,
) {
    tokio::select! {
        _ = handler => {}
        _ = tokio::time::sleep(grace) => {}
    }
}

fn validate_request(request: &TypedInvocationRequest) -> Result<(), RuntimeError> {
    if request.context.request_id.trim().is_empty() {
        return Err(RuntimeError::InvalidExecutionRequest(
            "request id must not be empty".to_owned(),
        ));
    }
    if request.command.trim().is_empty() {
        return Err(RuntimeError::InvalidExecutionRequest(
            "command must not be empty".to_owned(),
        ));
    }
    if request.context.remaining_timeout_ms == 0 {
        return Err(RuntimeError::InvalidExecutionRequest(
            "remaining timeout must be greater than zero".to_owned(),
        ));
    }
    Ok(())
}

fn remove_tracked(
    coordinator: &Mutex<ExecutionCoordinator>,
    request_id: &str,
    state: &Arc<ExecutionState>,
) {
    let mut coordinator = lock_coordinator(coordinator);
    if coordinator
        .tracked
        .get(request_id)
        .is_some_and(|tracked| Arc::ptr_eq(&tracked.state, state))
    {
        coordinator.tracked.remove(request_id);
    }
}

fn lock_coordinator(
    coordinator: &Mutex<ExecutionCoordinator>,
) -> std::sync::MutexGuard<'_, ExecutionCoordinator> {
    coordinator
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn elapsed_ms(started_at: Instant, finished_at: Instant) -> u64 {
    u64::try_from(
        finished_at
            .saturating_duration_since(started_at)
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc, Condvar, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        thread,
        time::Duration,
    };

    use ah_plugin_api::{
        AH_PLUGIN_ABI_VERSION, CommandCatalog, CommandDescriptor, CommandEffect, CommandEffects,
        ExecutionContextWire, InvocationRequest, InvocationResponse, PluginCompatibility,
        PluginManual, PluginMetadata, Reversibility, RiskLevel, TypedInvocationRequest,
        TypedInvocationResponse, plugin_capabilities,
    };
    use serde_json::json;
    use tokio::sync::Semaphore;

    use super::{ExecutionTimeoutPhase, Executor, ParallelExecutor};
    use crate::{BuiltinPlugin, PluginManager, RuntimeError};

    struct ProbePlugin {
        active: AtomicUsize,
        max_active: AtomicUsize,
        gate: Gate,
        honor_cancel: bool,
    }

    #[derive(Default)]
    struct GateState {
        block: bool,
        started: usize,
        cancelled: bool,
    }

    type Gate = Arc<(Mutex<GateState>, Condvar)>;
    type ExecutorFixture = (ParallelExecutor, Arc<ProbePlugin>, Gate);

    /// How long a probe handler may stay parked before it gives up. Generous,
    /// because it exists only so a test cannot hang forever - never to make a
    /// timing assertion pass.
    const PROBE_BLOCK_LIMIT: Duration = Duration::from_secs(60);

    /// How long a test waits for handlers to reach the gate. Long enough for a
    /// loaded machine running the whole workspace, short enough to fail rather
    /// than stall.
    const PROBE_START_LIMIT: Duration = Duration::from_secs(30);

    impl ProbePlugin {
        fn new(block: bool, honor_cancel: bool) -> (Arc<Self>, Gate) {
            let gate = Arc::new((
                Mutex::new(GateState {
                    block,
                    ..GateState::default()
                }),
                Condvar::new(),
            ));
            (
                Arc::new(Self {
                    active: AtomicUsize::new(0),
                    max_active: AtomicUsize::new(0),
                    gate: Arc::clone(&gate),
                    honor_cancel,
                }),
                gate,
            )
        }
    }

    impl BuiltinPlugin for ProbePlugin {
        fn metadata(&self) -> PluginMetadata {
            PluginMetadata {
                plugin_name: "probe".to_owned(),
                domain: "probe".to_owned(),
                description: "executor probe".to_owned(),
                abi_version: AH_PLUGIN_ABI_VERSION,
                required_tools: Vec::new(),
                compatibility: PluginCompatibility::current()
                    .with_capability(plugin_capabilities::TYPED_COMMANDS_V1),
            }
        }

        fn manual(&self) -> PluginManual {
            PluginManual {
                plugin_name: "probe".to_owned(),
                domain: "probe".to_owned(),
                description: "executor probe".to_owned(),
                commands: Vec::new(),
                notes: Vec::new(),
            }
        }

        fn invoke(&self, _request: &InvocationRequest) -> InvocationResponse {
            InvocationResponse::ok(None)
        }

        fn command_catalog(&self) -> Option<CommandCatalog> {
            Some(CommandCatalog::new(
                "probe",
                "probe",
                vec![CommandDescriptor::new(
                    "probe.run",
                    "Run probe",
                    "Run an executor test probe.",
                    json!({"type": "object", "properties": {}, "additionalProperties": false}),
                    json!({
                        "type": "object",
                        "properties": {"completed": {"type": "boolean"}},
                        "required": ["completed"],
                        "additionalProperties": false
                    }),
                    CommandEffects::new(
                        true,
                        false,
                        true,
                        false,
                        vec![CommandEffect::ExternalRead],
                        RiskLevel::Low,
                        "Runs an in-memory test probe only.",
                        Reversibility::Yes,
                    ),
                )],
            ))
        }

        fn invoke_typed(&self, _request: &TypedInvocationRequest) -> TypedInvocationResponse {
            let active = self.active.fetch_add(1, Ordering::AcqRel) + 1;
            self.max_active.fetch_max(active, Ordering::AcqRel);
            {
                let (state, changed) = &*self.gate;
                let mut state = state.lock().unwrap();
                state.started += 1;
                changed.notify_all();
                // Bounded on purpose. A blocking task cannot be aborted, so an
                // unbounded wait here does not fail a test that never reaches
                // `release` - it hangs the test binary, which keeps holding its
                // own output file and turns the next build into a linker error.
                let deadline = std::time::Instant::now() + PROBE_BLOCK_LIMIT;
                while state.block && !state.cancelled {
                    let Some(remaining) =
                        deadline.checked_duration_since(std::time::Instant::now())
                    else {
                        break;
                    };
                    state = changed.wait_timeout(state, remaining).unwrap().0;
                }
            }
            thread::sleep(Duration::from_millis(5));
            self.active.fetch_sub(1, Ordering::AcqRel);
            TypedInvocationResponse::success(
                json!({"completed": true}),
                Some("completed".to_owned()),
            )
        }

        fn cancel_typed(&self, _request_id: &str) -> bool {
            if !self.honor_cancel {
                return false;
            }
            let (state, changed) = &*self.gate;
            let mut state = state.lock().unwrap();
            state.cancelled = true;
            changed.notify_all();
            true
        }
    }

    fn request(id: &str, timeout_ms: u64) -> TypedInvocationRequest {
        TypedInvocationRequest::new(
            "probe.run",
            json!({}),
            ExecutionContextWire::new(id, ".", None, timeout_ms),
        )
    }

    fn executor(block: bool, capacity: usize) -> ExecutorFixture {
        executor_with_cancel_policy(block, true, capacity)
    }

    fn executor_with_cancel_policy(
        block: bool,
        honor_cancel: bool,
        capacity: usize,
    ) -> ExecutorFixture {
        let (plugin, gate) = ProbePlugin::new(block, honor_cancel);
        let mut manager = PluginManager::new();
        manager.register_builtin(plugin.clone());
        (
            ParallelExecutor::new(Arc::new(manager), capacity).unwrap(),
            plugin,
            gate,
        )
    }

    async fn wait_for_started(gate: Gate, expected: usize) {
        tokio::task::spawn_blocking(move || {
            let (state, changed) = &*gate;
            let state = state.lock().unwrap();
            let (state, _) = changed
                .wait_timeout_while(state, PROBE_START_LIMIT, |state| state.started < expected)
                .unwrap();
            assert!(
                state.started >= expected,
                "expected {expected} handlers to start"
            );
        })
        .await
        .unwrap();
    }

    fn release(gate: &Gate) {
        let (state, changed) = &**gate;
        let mut state = state.lock().unwrap();
        state.block = false;
        changed.notify_all();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn rejects_zero_capacity() {
        let (plugin, _) = ProbePlugin::new(false, true);
        let mut manager = PluginManager::new();
        manager.register_builtin(plugin);
        let error = ParallelExecutor::new(Arc::new(manager), 0)
            .err()
            .expect("zero capacity should fail");
        assert!(matches!(error, RuntimeError::InvalidExecutionRequest(_)));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn validates_capacity_against_tokio_semaphore_limit() {
        let executor =
            ParallelExecutor::new(Arc::new(PluginManager::new()), Semaphore::MAX_PERMITS)
                .expect("Tokio semaphore limit should be accepted");
        assert_eq!(executor.max_active(), Semaphore::MAX_PERMITS);

        let error =
            ParallelExecutor::new(Arc::new(PluginManager::new()), Semaphore::MAX_PERMITS + 1)
                .err()
                .expect("capacity above Tokio semaphore limit should fail");
        assert!(matches!(error, RuntimeError::InvalidExecutionRequest(_)));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn accepted_handlers_overlap() {
        let (executor, plugin, gate) = executor(true, 2);
        let first = executor.try_submit(request("first", 1_000)).unwrap();
        let second = executor.try_submit(request("second", 1_000)).unwrap();
        wait_for_started(Arc::clone(&gate), 2).await;
        assert_eq!(plugin.max_active.load(Ordering::Acquire), 2);
        release(&gate);
        assert!(first.observe().await.result.unwrap().success);
        assert!(second.observe().await.result.unwrap().success);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn capacity_rejection_is_immediate_and_never_queued() {
        let (executor, _plugin, gate) = executor(true, 2);
        let first = executor.try_submit(request("first", 1_000)).unwrap();
        let second = executor.try_submit(request("second", 1_000)).unwrap();
        wait_for_started(Arc::clone(&gate), 2).await;
        let error = executor
            .try_submit(request("third", 1_000))
            .err()
            .expect("third execution must be rejected");
        assert!(matches!(
            error,
            RuntimeError::ExecutionCapacityFull { capacity: 2 }
        ));
        release(&gate);
        first.observe().await.result.unwrap();
        second.observe().await.result.unwrap();
        assert_eq!(gate.0.lock().unwrap().started, 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cancellation_completes_logically_before_physical_exit() {
        let (executor, _plugin, gate) = executor_with_cancel_policy(true, false, 1);
        let handle = executor.try_submit(request("cancel", 1_000)).unwrap();
        let lifecycle = handle.lifecycle();
        wait_for_started(Arc::clone(&gate), 1).await;
        assert!(executor.cancel("cancel"));
        let error = handle.observe().await.result.unwrap_err();
        assert!(matches!(
            error,
            RuntimeError::ExecutionCancelled { request_id } if request_id == "cancel"
        ));
        assert!(lifecycle.is_draining());
        assert_eq!(executor.active_count(), 1);
        release(&gate);
        lifecycle.wait_physical().await;
        assert_eq!(executor.active_count(), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn abandoned_drain_releases_capacity_after_grace() {
        let (executor, _plugin, gate) = executor_with_cancel_policy(true, false, 1);
        let executor = executor.with_drain_grace(Duration::from_millis(50));
        let handle = executor.try_submit(request("stuck", 10_000)).unwrap();
        let lifecycle = handle.lifecycle();
        wait_for_started(Arc::clone(&gate), 1).await;

        assert!(executor.cancel("stuck"));
        assert!(matches!(
            handle.observe().await.result.unwrap_err(),
            RuntimeError::ExecutionCancelled { .. }
        ));
        assert!(lifecycle.is_draining());
        assert_eq!(executor.active_count(), 1);

        // Handler ignores cancellation and never returns on its own, but the
        // drain watchdog abandons it after the grace period and frees the slot.
        lifecycle.wait_physical().await;
        assert_eq!(executor.active_count(), 0);
        assert!(!lifecycle.is_draining());

        release(&gate);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn one_draining_execution_does_not_block_other_capacity() {
        let (executor, plugin, gate) = executor_with_cancel_policy(true, false, 2);
        let first = executor.try_submit(request("first", 1_000)).unwrap();
        wait_for_started(Arc::clone(&gate), 1).await;
        assert!(executor.cancel("first"));
        first.observe().await.result.unwrap_err();

        let second = executor.try_submit(request("second", 1_000)).unwrap();
        wait_for_started(Arc::clone(&gate), 2).await;
        assert_eq!(plugin.max_active.load(Ordering::Acquire), 2);
        assert!(matches!(
            executor
                .try_submit(request("third", 1_000))
                .err()
                .expect("third execution must be rejected"),
            RuntimeError::ExecutionCapacityFull { capacity: 2 }
        ));
        release(&gate);
        second.observe().await.result.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn timeout_is_execution_phase_with_zero_queue_wait() {
        let (executor, _plugin, gate) = executor_with_cancel_policy(true, false, 1);
        let observed = executor.execute_observed(request("timeout", 20)).await;
        let error = observed.result.unwrap_err();
        assert!(matches!(
            error,
            RuntimeError::ExecutionTimeout { request_id } if request_id == "timeout"
        ));
        let telemetry = observed.telemetry.unwrap();
        assert_eq!(telemetry.queue_wait_ms, 0);
        assert_eq!(
            telemetry.timeout_phase,
            Some(ExecutionTimeoutPhase::Execution)
        );
        release(&gate);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn close_cancels_active_and_rejects_new_work() {
        let (executor, _plugin, gate) = executor(true, 1);
        let active = executor.try_submit(request("active", 1_000)).unwrap();
        let lifecycle = active.lifecycle();
        wait_for_started(gate, 1).await;
        executor.close();
        assert!(matches!(
            active.observe().await.result.unwrap_err(),
            RuntimeError::ExecutionCancelled { .. }
        ));
        assert!(matches!(
            executor
                .try_submit(request("late", 1_000))
                .err()
                .expect("late execution must be rejected"),
            RuntimeError::ExecutorShuttingDown
        ));
        lifecycle.wait_physical().await;
    }
}
