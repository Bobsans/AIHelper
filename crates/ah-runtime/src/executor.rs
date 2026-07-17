use std::{
    collections::{BTreeMap, HashMap},
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, AtomicU64, Ordering},
    },
    time::Duration,
};

use ah_plugin_api::{TypedInvocationRequest, TypedInvocationResponse};
use tokio::{
    sync::{Notify, mpsc, oneshot},
    time::Instant,
};

use crate::{PluginManager, RuntimeError};

pub type ExecutionFuture<'a> =
    Pin<Box<dyn Future<Output = Result<TypedInvocationResponse, RuntimeError>> + Send + 'a>>;

pub trait Executor: Send + Sync {
    fn execute(&self, request: TypedInvocationRequest) -> ExecutionFuture<'_>;
    fn cancel(&self, request_id: &str) -> bool;
}

#[derive(Clone)]
pub struct SequentialExecutor {
    sender: mpsc::Sender<ExecutionJob>,
    coordinator: Arc<Mutex<ExecutionCoordinator>>,
    manager: Arc<PluginManager>,
    queue_capacity: usize,
    next_generation: Arc<AtomicU64>,
}

impl SequentialExecutor {
    pub fn new(manager: Arc<PluginManager>, queue_capacity: usize) -> Result<Self, RuntimeError> {
        if queue_capacity == 0 {
            return Err(RuntimeError::InvalidExecutionRequest(
                "queue capacity must be greater than zero".to_owned(),
            ));
        }
        let runtime = tokio::runtime::Handle::try_current().map_err(|error| {
            RuntimeError::ExecutionWorker(format!(
                "executor must be created inside a Tokio runtime: {error}"
            ))
        })?;
        let (sender, receiver) = mpsc::channel(queue_capacity);
        let coordinator = Arc::new(Mutex::new(ExecutionCoordinator::default()));
        runtime.spawn(run_worker(
            Arc::clone(&manager),
            receiver,
            Arc::clone(&coordinator),
        ));
        Ok(Self {
            sender,
            coordinator,
            manager,
            queue_capacity,
            next_generation: Arc::new(AtomicU64::new(1)),
        })
    }

    pub fn queue_capacity(&self) -> usize {
        self.queue_capacity
    }

    #[cfg(test)]
    fn is_queued(&self, request_id: &str) -> bool {
        let coordinator = lock_coordinator(&self.coordinator);
        coordinator
            .tracked
            .get(request_id)
            .is_some_and(|request| request.phase == RequestPhase::Queued)
    }

    #[cfg(test)]
    fn is_draining(&self) -> bool {
        !lock_coordinator(&self.coordinator).draining.is_empty()
    }

    async fn execute_inner(
        &self,
        request: TypedInvocationRequest,
    ) -> Result<TypedInvocationResponse, RuntimeError> {
        validate_request(&request)?;
        let request_id = request.context.request_id.clone();
        let command = request.command.clone();
        let accepted_at = Instant::now();
        let deadline = accepted_at
            .checked_add(Duration::from_millis(request.context.remaining_timeout_ms))
            .ok_or_else(|| {
                RuntimeError::InvalidExecutionRequest(format!(
                    "timeout is too large for request '{request_id}'"
                ))
            })?;
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
        let signal = Arc::new(ExecutionSignal::default());
        {
            let mut coordinator = lock_coordinator(&self.coordinator);
            if let Some(blocking_request_id) = coordinator.draining.values().next() {
                return Err(RuntimeError::ExecutionDraining {
                    request_id: blocking_request_id.clone(),
                });
            }
            if coordinator.tracked.contains_key(&request_id) {
                return Err(RuntimeError::InvalidExecutionRequest(format!(
                    "duplicate active request id '{request_id}'"
                )));
            }
            coordinator.tracked.insert(
                request_id.clone(),
                TrackedRequest {
                    generation,
                    command,
                    signal: Arc::clone(&signal),
                    phase: RequestPhase::Queued,
                },
            );
        }

        let (completion, response) = oneshot::channel();
        let job = ExecutionJob {
            request,
            generation,
            deadline,
            signal: Arc::clone(&signal),
            completion,
        };
        match self.sender.try_send(job) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                remove_tracked(&self.coordinator, &request_id, generation);
                return Err(RuntimeError::ExecutionQueueFull {
                    capacity: self.queue_capacity,
                });
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                remove_tracked(&self.coordinator, &request_id, generation);
                return Err(RuntimeError::ExecutionWorker(
                    "execution queue is closed".to_owned(),
                ));
            }
        }

        self.await_completion(request_id, generation, deadline, signal, response)
            .await
    }

    async fn await_completion(
        &self,
        request_id: String,
        generation: u64,
        deadline: Instant,
        signal: Arc<ExecutionSignal>,
        mut response: oneshot::Receiver<Result<TypedInvocationResponse, RuntimeError>>,
    ) -> Result<TypedInvocationResponse, RuntimeError> {
        tokio::select! {
            biased;
            reason = signal.wait() => Err(reason.into_error(request_id.clone())),
            result = &mut response => map_completion(result, &self.coordinator, &request_id, generation),
            _ = tokio::time::sleep_until(deadline) => {
                if timeout_request(
                    &self.coordinator,
                    &self.manager,
                    &request_id,
                    generation,
                ) {
                    Err(RuntimeError::ExecutionTimeout {
                        request_id: request_id.clone(),
                    })
                } else {
                    map_completion(
                        response.await,
                        &self.coordinator,
                        &request_id,
                        generation,
                    )
                }
            }
        }
    }
}

impl Executor for SequentialExecutor {
    fn execute(&self, request: TypedInvocationRequest) -> ExecutionFuture<'_> {
        Box::pin(self.execute_inner(request))
    }

    fn cancel(&self, request_id: &str) -> bool {
        let cancel_command = {
            let mut coordinator = lock_coordinator(&self.coordinator);
            let Some(tracked) = coordinator.tracked.get(request_id).cloned() else {
                return false;
            };
            tracked.signal.set(StopReason::Cancelled);
            match tracked.phase {
                RequestPhase::Queued => {
                    coordinator.tracked.remove(request_id);
                    None
                }
                RequestPhase::Active => Some(tracked.command),
                RequestPhase::TimedOutDraining => None,
            }
        };
        if let Some(command) = cancel_command {
            self.manager.cancel_typed(&command, request_id);
        }
        true
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RequestPhase {
    Queued,
    Active,
    TimedOutDraining,
}

#[derive(Clone)]
struct TrackedRequest {
    generation: u64,
    command: String,
    signal: Arc<ExecutionSignal>,
    phase: RequestPhase,
}

struct ExecutionJob {
    request: TypedInvocationRequest,
    generation: u64,
    deadline: Instant,
    signal: Arc<ExecutionSignal>,
    completion: oneshot::Sender<Result<TypedInvocationResponse, RuntimeError>>,
}

#[derive(Default)]
struct ExecutionCoordinator {
    tracked: HashMap<String, TrackedRequest>,
    draining: BTreeMap<u64, String>,
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

#[derive(Default)]
struct ExecutionSignal {
    reason: AtomicU8,
    changed: Notify,
}

impl ExecutionSignal {
    fn reason(&self) -> Option<StopReason> {
        StopReason::from_raw(self.reason.load(Ordering::Acquire))
    }

    fn set(&self, reason: StopReason) -> bool {
        if self
            .reason
            .compare_exchange(0, reason as u8, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            self.changed.notify_one();
            true
        } else {
            false
        }
    }

    async fn wait(&self) -> StopReason {
        loop {
            if let Some(reason) = self.reason() {
                return reason;
            }
            let changed = self.changed.notified();
            if let Some(reason) = self.reason() {
                return reason;
            }
            changed.await;
        }
    }
}

async fn run_worker(
    manager: Arc<PluginManager>,
    mut receiver: mpsc::Receiver<ExecutionJob>,
    coordinator: Arc<Mutex<ExecutionCoordinator>>,
) {
    while let Some(mut job) = receiver.recv().await {
        let request_id = job.request.context.request_id.clone();
        match activate_request(
            &coordinator,
            &request_id,
            job.generation,
            job.deadline,
        ) {
            Activation::Start => {}
            Activation::Cancelled => {
                let _ = job.completion.send(Err(RuntimeError::ExecutionCancelled {
                    request_id,
                }));
                continue;
            }
            Activation::TimedOut => {
                job.signal.set(StopReason::TimedOut);
                let _ = job.completion.send(Err(RuntimeError::ExecutionTimeout {
                    request_id,
                }));
                continue;
            }
            Activation::Discard => continue,
        }

        let now = Instant::now();
        let remaining = job.deadline.saturating_duration_since(now);
        job.request.context.remaining_timeout_ms = u64::try_from(remaining.as_millis())
            .unwrap_or(u64::MAX)
            .max(1);

        let manager_for_call = Arc::clone(&manager);
        let mut handler =
            tokio::task::spawn_blocking(move || manager_for_call.invoke_typed(&job.request));
        match tokio::time::timeout_at(job.deadline, &mut handler).await {
            Ok(joined) => {
                let handler_result = match joined {
                    Ok(result) => result,
                    Err(error) if error.is_panic() => Err(RuntimeError::ExecutionPanic {
                        request_id: request_id.clone(),
                    }),
                    Err(error) => Err(RuntimeError::ExecutionWorker(format!(
                        "handler join failed for request '{request_id}': {error}"
                    ))),
                };
                if let Some(result) = finish_request(
                    &coordinator,
                    &request_id,
                    job.generation,
                    &job.signal,
                    handler_result,
                ) {
                    let _ = job.completion.send(result);
                }
            }
            Err(_) => {
                timeout_request(&coordinator, &manager, &request_id, job.generation);
                let _ = handler.await;
                if let Some(result) = finish_request(
                    &coordinator,
                    &request_id,
                    job.generation,
                    &job.signal,
                    Err(RuntimeError::ExecutionTimeout {
                        request_id: request_id.clone(),
                    }),
                ) {
                    let _ = job.completion.send(result);
                }
            }
        }
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
    Ok(())
}

enum Activation {
    Start,
    Cancelled,
    TimedOut,
    Discard,
}

fn activate_request(
    coordinator: &Mutex<ExecutionCoordinator>,
    request_id: &str,
    generation: u64,
    deadline: Instant,
) -> Activation {
    let mut coordinator = lock_coordinator(coordinator);
    let Some(request) = coordinator.tracked.get(request_id) else {
        return Activation::Discard;
    };
    if request.generation != generation {
        return Activation::Discard;
    }
    if request.signal.reason() == Some(StopReason::Cancelled) {
        coordinator.tracked.remove(request_id);
        return Activation::Cancelled;
    }
    if Instant::now() >= deadline {
        coordinator.tracked.remove(request_id);
        return Activation::TimedOut;
    }
    coordinator
        .tracked
        .get_mut(request_id)
        .expect("tracked request should still exist")
        .phase = RequestPhase::Active;
    Activation::Start
}

fn timeout_request(
    coordinator: &Mutex<ExecutionCoordinator>,
    manager: &PluginManager,
    request_id: &str,
    generation: u64,
) -> bool {
    let cancel_command = {
        let mut coordinator = lock_coordinator(coordinator);
        let Some(request) = coordinator.tracked.get(request_id).cloned() else {
            return false;
        };
        if request.generation != generation {
            return false;
        }
        request.signal.set(StopReason::TimedOut);
        match request.phase {
            RequestPhase::Queued => {
                coordinator.tracked.remove(request_id);
                None
            }
            RequestPhase::Active => {
                coordinator
                    .draining
                    .insert(generation, request_id.to_owned());
                coordinator
                    .tracked
                    .get_mut(request_id)
                    .expect("tracked request should still exist")
                    .phase = RequestPhase::TimedOutDraining;
                Some(request.command)
            }
            RequestPhase::TimedOutDraining => None,
        }
    };
    if let Some(command) = cancel_command {
        manager.cancel_typed(&command, request_id);
    }
    true
}

fn finish_request(
    coordinator: &Mutex<ExecutionCoordinator>,
    request_id: &str,
    generation: u64,
    signal: &ExecutionSignal,
    handler_result: Result<TypedInvocationResponse, RuntimeError>,
) -> Option<Result<TypedInvocationResponse, RuntimeError>> {
    let phase = {
        let mut coordinator = lock_coordinator(coordinator);
        let request = coordinator.tracked.get(request_id)?;
        if request.generation != generation {
            return None;
        }
        let phase = request.phase;
        coordinator.tracked.remove(request_id);
        coordinator.draining.remove(&generation);
        phase
    };
    Some(match phase {
        RequestPhase::TimedOutDraining => Err(RuntimeError::ExecutionTimeout {
            request_id: request_id.to_owned(),
        }),
        RequestPhase::Queued | RequestPhase::Active => match signal.reason() {
            Some(reason) => Err(reason.into_error(request_id.to_owned())),
            None => handler_result,
        },
    })
}

fn remove_tracked(
    coordinator: &Mutex<ExecutionCoordinator>,
    request_id: &str,
    generation: u64,
) {
    let mut coordinator = lock_coordinator(coordinator);
    if coordinator
        .tracked
        .get(request_id)
        .is_some_and(|request| request.generation == generation)
    {
        coordinator.tracked.remove(request_id);
        coordinator.draining.remove(&generation);
    }
}

fn map_completion(
    result: Result<Result<TypedInvocationResponse, RuntimeError>, oneshot::error::RecvError>,
    coordinator: &Mutex<ExecutionCoordinator>,
    request_id: &str,
    generation: u64,
) -> Result<TypedInvocationResponse, RuntimeError> {
    result.unwrap_or_else(|_| {
        remove_tracked(coordinator, request_id, generation);
        Err(RuntimeError::ExecutionWorker(format!(
            "worker dropped response channel for request '{request_id}'"
        )))
    })
}

fn lock_coordinator(
    coordinator: &Mutex<ExecutionCoordinator>,
) -> std::sync::MutexGuard<'_, ExecutionCoordinator> {
    coordinator
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc, Condvar, Mutex,
            atomic::{AtomicBool, AtomicUsize, Ordering},
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

    use super::{Executor, SequentialExecutor};
    use crate::{BuiltinPlugin, PluginManager, RuntimeError};

    struct ProbePlugin {
        active: AtomicUsize,
        max_active: AtomicUsize,
        gate: Gate,
        honor_cancel: bool,
        panic_next: AtomicBool,
    }

    #[derive(Default)]
    struct GateState {
        block: bool,
        started: usize,
        cancelled: bool,
    }

    type Gate = Arc<(Mutex<GateState>, Condvar)>;
    type ExecutorFixture = (SequentialExecutor, Arc<ProbePlugin>, Gate);

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
                    panic_next: AtomicBool::new(false),
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
                    json!({
                        "type": "object",
                        "properties": {},
                        "additionalProperties": false
                    }),
                    json!({
                        "type": "object",
                        "properties": {
                            "completed": {"type": "boolean"}
                        },
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
                while state.block && !state.cancelled {
                    state = changed.wait(state).unwrap();
                }
            }
            if self.panic_next.swap(false, Ordering::AcqRel) {
                self.active.fetch_sub(1, Ordering::AcqRel);
                panic!("probe handler panic");
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
            SequentialExecutor::new(Arc::new(manager), capacity).unwrap(),
            plugin,
            gate,
        )
    }

    async fn wait_for_started(gate: Gate, expected: usize) {
        tokio::task::spawn_blocking(move || {
            let (state, changed) = &*gate;
            let state = state.lock().unwrap();
            let _ = changed
                .wait_timeout_while(state, Duration::from_secs(1), |state| {
                    state.started < expected
                })
                .unwrap();
        })
        .await
        .unwrap();
    }

    async fn wait_for_queued(executor: &SequentialExecutor, request_id: &str) {
        tokio::time::timeout(Duration::from_secs(1), async {
            while !executor.is_queued(request_id) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("request should enter the queue");
    }

    fn release(gate: &Arc<(Mutex<GateState>, Condvar)>) {
        let (state, changed) = &**gate;
        let mut state = state.lock().unwrap();
        state.block = false;
        changed.notify_all();
    }

    #[test]
    fn rejects_zero_capacity() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let (plugin, _) = ProbePlugin::new(false, true);
            let mut manager = PluginManager::new();
            manager.register_builtin(plugin);
            let error = SequentialExecutor::new(Arc::new(manager), 0)
                .err()
                .expect("zero capacity should fail");
            assert!(matches!(error, RuntimeError::InvalidExecutionRequest(_)));
        });
    }

    #[test]
    fn handlers_never_overlap() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let (executor, plugin, _) = executor(false, 4);
            let first_executor = executor.clone();
            let first =
                tokio::spawn(async move { first_executor.execute(request("first", 1_000)).await });
            let second_executor = executor.clone();
            let second =
                tokio::spawn(
                    async move { second_executor.execute(request("second", 1_000)).await },
                );
            assert!(first.await.unwrap().unwrap().success);
            assert!(second.await.unwrap().unwrap().success);
            assert_eq!(plugin.max_active.load(Ordering::Acquire), 1);
        });
    }

    #[test]
    fn reports_queue_full_deterministically() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let (executor, _plugin, gate) = executor(true, 1);
            let first_executor = executor.clone();
            let first =
                tokio::spawn(async move { first_executor.execute(request("first", 1_000)).await });
            wait_for_started(Arc::clone(&gate), 1).await;
            let second_executor = executor.clone();
            let second =
                tokio::spawn(
                    async move { second_executor.execute(request("second", 1_000)).await },
                );
            wait_for_queued(&executor, "second").await;
            let result = executor.execute(request("third", 1_000)).await;
            release(&gate);
            let error = result.unwrap_err();
            assert!(matches!(
                error,
                RuntimeError::ExecutionQueueFull { capacity: 1 }
            ));
            assert!(first.await.unwrap().unwrap().success);
            assert!(second.await.unwrap().unwrap().success);
        });
    }

    #[test]
    fn cancels_active_request() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let (executor, _plugin, gate) = executor(true, 1);
            let task_executor = executor.clone();
            let task =
                tokio::spawn(async move { task_executor.execute(request("cancel", 1_000)).await });
            wait_for_started(gate, 1).await;
            assert!(executor.cancel("cancel"));
            let error = task.await.unwrap().unwrap_err();
            assert!(matches!(
                error,
                RuntimeError::ExecutionCancelled { request_id } if request_id == "cancel"
            ));
        });
    }

    #[test]
    fn times_out_and_cancels_handler() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let (executor, _plugin, _gate) = executor(true, 1);
            let error = executor.execute(request("timeout", 10)).await.unwrap_err();
            assert!(matches!(
                error,
                RuntimeError::ExecutionTimeout { request_id } if request_id == "timeout"
            ));
        });
    }

    #[test]
    fn queued_request_can_be_cancelled() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let (executor, _plugin, gate) = executor(true, 2);
            let first_executor = executor.clone();
            let first =
                tokio::spawn(async move { first_executor.execute(request("first", 1_000)).await });
            wait_for_started(Arc::clone(&gate), 1).await;
            let second_executor = executor.clone();
            let second =
                tokio::spawn(
                    async move { second_executor.execute(request("queued", 1_000)).await },
                );
            wait_for_queued(&executor, "queued").await;
            let cancelled = executor.cancel("queued");
            assert!(cancelled);
            let error = tokio::time::timeout(Duration::from_millis(100), second)
                .await
                .expect("queued cancellation should complete promptly")
                .unwrap()
                .unwrap_err();
            assert!(matches!(
                error,
                RuntimeError::ExecutionCancelled { request_id } if request_id == "queued"
            ));
            release(&gate);
            assert!(first.await.unwrap().unwrap().success);
        });
    }

    #[test]
    fn queued_request_times_out_while_active_handler_runs() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let (executor, _plugin, gate) = executor(true, 2);
            let first_executor = executor.clone();
            let first =
                tokio::spawn(async move { first_executor.execute(request("first", 1_000)).await });
            wait_for_started(Arc::clone(&gate), 1).await;

            let second_executor = executor.clone();
            let second = tokio::spawn(async move {
                second_executor.execute(request("queued-timeout", 10)).await
            });
            wait_for_queued(&executor, "queued-timeout").await;

            let error = tokio::time::timeout(Duration::from_millis(100), second)
                .await
                .expect("queued timeout should complete promptly")
                .unwrap()
                .unwrap_err();
            assert!(matches!(
                error,
                RuntimeError::ExecutionTimeout { request_id } if request_id == "queued-timeout"
            ));

            release(&gate);
            assert!(first.await.unwrap().unwrap().success);
        });
    }

    #[test]
    fn uncooperative_timeout_closes_admission_until_handler_finishes() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let (executor, plugin, gate) = executor_with_cancel_policy(true, false, 2);
            let error = executor
                .execute(request("uncooperative", 10))
                .await
                .unwrap_err();
            assert!(matches!(
                error,
                RuntimeError::ExecutionTimeout { request_id } if request_id == "uncooperative"
            ));
            assert!(executor.is_draining());

            let error = executor
                .execute(request("rejected", 1_000))
                .await
                .unwrap_err();
            assert!(matches!(
                error,
                RuntimeError::ExecutionDraining { request_id } if request_id == "uncooperative"
            ));

            release(&gate);
            tokio::time::timeout(Duration::from_secs(1), async {
                while executor.is_draining() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("executor should reopen after the handler exits");

            assert!(executor
                .execute(request("accepted", 1_000))
                .await
                .unwrap()
                .success);
            assert_eq!(plugin.max_active.load(Ordering::Acquire), 1);
        });
    }

    #[test]
    fn rejects_duplicate_request_ids() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let (executor, _plugin, gate) = executor(true, 2);
            let first_executor = executor.clone();
            let first =
                tokio::spawn(async move { first_executor.execute(request("same", 1_000)).await });
            wait_for_started(Arc::clone(&gate), 1).await;
            let error = executor.execute(request("same", 1_000)).await.unwrap_err();
            assert!(matches!(error, RuntimeError::InvalidExecutionRequest(_)));
            release(&gate);
            assert!(first.await.unwrap().unwrap().success);
        });
    }

    #[test]
    fn completed_request_id_can_be_reused() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let (executor, _plugin, _gate) = executor(false, 1);
            assert!(executor.execute(request("same", 1_000)).await.unwrap().success);
            assert!(executor.execute(request("same", 1_000)).await.unwrap().success);
        });
    }

    #[test]
    fn handler_panic_cleans_up_execution_state() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let (executor, plugin, _gate) = executor(false, 1);
            plugin.panic_next.store(true, Ordering::Release);

            let error = executor.execute(request("panic", 1_000)).await.unwrap_err();
            assert!(matches!(
                error,
                RuntimeError::ExecutionPanic { request_id } if request_id == "panic"
            ));
            assert!(executor
                .execute(request("after-panic", 1_000))
                .await
                .unwrap()
                .success);
        });
    }

    #[test]
    fn cancel_unknown_request_returns_false() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let (executor, _plugin, _gate) = executor(false, 1);
            assert!(!executor.cancel("missing"));
        });
    }
}
