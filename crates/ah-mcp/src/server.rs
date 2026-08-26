use std::{
    collections::HashMap,
    future::Future,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use crate::job_tools::{job_argument_error, job_registry_error, job_snapshot_result, take_job_id};
use crate::mapping::{
    build_catalog_snapshot, command_error_result, command_event_outcome,
    command_event_run_check_outcome, execution_request_id, extract_context, internal_catalog_error,
    requires_explicit_cwd, typed_response_result, unknown_tool_error,
};
use crate::plaintext_auth::{redact_mcp_plaintext_auth, validate_mcp_plaintext_auth};

use ah_plugin_api::{CommandError, TypedInvocationRequest};
use ah_runtime::{
    InvocationOutcome, PluginManager, RegisteredCommand, RuntimeError,
    executor::{ExecutionTelemetry, Executor},
};
use ah_setup_ui::SecretSetupService;
use rmcp::{
    Peer, RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResult, CancelledNotificationParam, Implementation,
        JsonObject, ListToolsResult, NumberOrString, PaginatedRequestParams, ServerCapabilities,
        ServerInfo, Tool,
    },
    service::{NotificationContext, RequestContext},
};
use serde_json::Value;
use thiserror::Error;

use crate::{
    events::EventDispatcher,
    jobs::{JobEventContext, JobRegistry},
};

pub(crate) const TOOL_PREFIX: &str = "ah.";
pub(crate) const RISK_META_KEY: &str = "dev.aihelper/risk";
pub(crate) const DIAGNOSTIC_META_KEY: &str = "dev.aihelper/diagnostic";
pub(crate) const EXECUTION_META_KEY: &str = "dev.aihelper/execution";
pub(crate) const JOB_TOOL_PREFIX: &str = "ah.job.";
pub(crate) const JOB_START_TOOL: &str = "ah.job.start";
pub(crate) const JOB_STATUS_TOOL: &str = "ah.job.status";
pub(crate) const JOB_RESULT_TOOL: &str = "ah.job.result";
pub(crate) const JOB_CANCEL_TOOL: &str = "ah.job.cancel";
pub(crate) const PEER_NOTIFICATION_TIMEOUT: Duration = Duration::from_secs(1);
pub(crate) const DEFAULT_SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpCommandStatus {
    Success,
    Error,
}

#[derive(Debug, Clone, PartialEq)]
pub struct McpCommandEvent {
    pub command: String,
    pub tool: String,
    pub request_id: String,
    pub job_id: Option<String>,
    pub parameters: Value,
    pub status: McpCommandStatus,
    pub duration_ms: u64,
    pub diagnostic: Option<CommandError>,
    pub outcome: Option<InvocationOutcome>,
}

pub trait EventSink: Send + Sync {
    fn record_command(&self, event: McpCommandEvent);

    fn record_command_with_telemetry(
        &self,
        event: McpCommandEvent,
        _telemetry: Option<ExecutionTelemetry>,
    ) {
        self.record_command(event);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerConfig {
    pub cwd: String,
    pub limit: Option<usize>,
    pub default_timeout_ms: u64,
}

impl McpServerConfig {
    pub fn new(
        cwd: impl Into<String>,
        limit: Option<usize>,
        default_timeout_ms: u64,
    ) -> Result<Self, McpAdapterError> {
        let cwd = cwd.into();
        if cwd.trim().is_empty() {
            return Err(McpAdapterError::InvalidConfig(
                "default cwd must not be empty".to_owned(),
            ));
        }
        if limit == Some(0) {
            return Err(McpAdapterError::InvalidConfig(
                "default limit must be greater than zero".to_owned(),
            ));
        }
        if default_timeout_ms == 0 {
            return Err(McpAdapterError::InvalidConfig(
                "default timeout must be greater than zero".to_owned(),
            ));
        }
        Ok(Self {
            cwd,
            limit,
            default_timeout_ms,
        })
    }
}

#[derive(Debug, Error)]
pub enum McpAdapterError {
    #[error("invalid MCP server configuration: {0}")]
    InvalidConfig(String),
    #[error("invalid typed command schema for '{command}': {reason}")]
    InvalidSchema { command: String, reason: String },
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
    #[error("MCP service failed: {0}")]
    Service(String),
    #[error("MCP shutdown exceeded the configured {grace_ms} ms grace period")]
    ShutdownTimeout { grace_ms: u128 },
}

#[must_use]
pub struct McpServeOutcome {
    pub(crate) result: Result<(), McpAdapterError>,
    pub(crate) remaining_shutdown_grace: Duration,
}

impl McpServeOutcome {
    pub fn into_parts(self) -> (Result<(), McpAdapterError>, Duration) {
        (self.result, self.remaining_shutdown_grace)
    }

    pub fn into_result(self) -> Result<(), McpAdapterError> {
        self.result
    }
}

pub struct McpServer {
    pub(crate) shared: Arc<McpShared>,
    pub(crate) active_executions: Mutex<HashMap<String, Vec<String>>>,
    pub(crate) require_explicit_cwd: bool,
    pub(crate) session_id: u64,
}

pub(crate) struct McpShared {
    pub(crate) manager: Arc<PluginManager>,
    pub(crate) executor: Arc<dyn Executor>,
    pub(crate) config: McpServerConfig,
    pub(crate) catalog_snapshot: Mutex<Arc<CatalogSnapshot>>,
    pub(crate) catalog_generation: AtomicU64,
    pub(crate) next_execution_id: AtomicU64,
    pub(crate) event_dispatcher: Option<Arc<EventDispatcher>>,
    pub(crate) secret_setup: Option<Arc<dyn SecretSetupService>>,
    pub(crate) jobs: Arc<JobRegistry>,
    pub(crate) peers: Mutex<HashMap<u64, RegisteredPeer>>,
    pub(crate) next_session_id: AtomicU64,
    pub(crate) next_peer_generation: AtomicU64,
}

pub(crate) struct RegisteredPeer {
    pub(crate) generation: u64,
    pub(crate) peer: Peer<RoleServer>,
}

pub(crate) struct CatalogSnapshot {
    pub(crate) runtime_revision: u64,
    pub(crate) tools: Vec<Tool>,
    pub(crate) tools_by_name: HashMap<String, Tool>,
    pub(crate) commands_by_name: HashMap<String, RegisteredCommand>,
}

pub(crate) struct ToolCallOutcome {
    result: Result<CallToolResult, rmcp::ErrorData>,
    telemetry: Option<ExecutionTelemetry>,
}

impl ToolCallOutcome {
    pub(crate) fn unobserved(result: Result<CallToolResult, rmcp::ErrorData>) -> Self {
        Self {
            result,
            telemetry: None,
        }
    }
}

impl McpServer {
    pub fn new(
        manager: Arc<PluginManager>,
        executor: Arc<dyn Executor>,
        config: McpServerConfig,
    ) -> Result<Self, McpAdapterError> {
        let catalog_snapshot = build_catalog_snapshot(&manager)?;
        Ok(Self {
            shared: Arc::new(McpShared {
                manager,
                executor,
                config,
                catalog_snapshot: Mutex::new(Arc::new(catalog_snapshot)),
                catalog_generation: AtomicU64::new(1),
                next_execution_id: AtomicU64::new(1),
                event_dispatcher: None,
                secret_setup: None,
                jobs: JobRegistry::standard(),
                peers: Mutex::new(HashMap::new()),
                next_session_id: AtomicU64::new(2),
                next_peer_generation: AtomicU64::new(1),
            }),
            active_executions: Mutex::new(HashMap::new()),
            require_explicit_cwd: false,
            session_id: 1,
        })
    }

    pub fn with_event_sink(mut self, event_sink: Arc<dyn EventSink>) -> Self {
        Arc::get_mut(&mut self.shared)
            .expect("event sink must be configured before MCP sessions are cloned")
            .event_dispatcher = Some(EventDispatcher::standard(event_sink));
        self
    }

    pub fn with_secret_setup(mut self, secret_setup: Arc<dyn SecretSetupService>) -> Self {
        Arc::get_mut(&mut self.shared)
            .expect("secret setup must be configured before MCP sessions are cloned")
            .secret_setup = Some(secret_setup);
        self
    }

    pub(crate) fn http_session(&self) -> Self {
        let session_id = self.shared.next_session_id.fetch_add(1, Ordering::Relaxed);
        Self {
            shared: Arc::clone(&self.shared),
            active_executions: Mutex::new(HashMap::new()),
            require_explicit_cwd: true,
            session_id,
        }
    }

    pub(crate) fn register_peer(&self, peer: Peer<RoleServer>) {
        let generation = self
            .shared
            .next_peer_generation
            .fetch_add(1, Ordering::Relaxed);
        self.shared
            .peers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(self.session_id, RegisteredPeer { generation, peer });
    }

    pub(crate) fn notify_tool_list_changed_all(&self) {
        notify_tool_list_changed_shared(&self.shared);
    }

    pub fn catalog_generation(&self) -> u64 {
        self.shared.catalog_generation.load(Ordering::Acquire)
    }

    pub fn tools(&self) -> Result<Vec<Tool>, McpAdapterError> {
        Ok(self.catalog_snapshot().tools.clone())
    }

    pub fn refresh_catalog_generation(&self) -> Result<bool, McpAdapterError> {
        refresh_catalog_generation_shared(&self.shared)
    }

    pub async fn refresh_catalog_and_notify(
        &self,
        peer: &Peer<RoleServer>,
    ) -> Result<bool, McpAdapterError> {
        let changed = self.refresh_catalog_generation()?;
        if changed {
            self.register_peer(peer.clone());
            self.notify_tool_list_changed_all();
        }
        Ok(changed)
    }

    pub(crate) fn find_command(
        &self,
        mcp_name: &str,
    ) -> Result<Option<RegisteredCommand>, McpAdapterError> {
        if !mcp_name.starts_with(TOOL_PREFIX) {
            return Ok(None);
        }
        Ok(self
            .catalog_snapshot()
            .commands_by_name
            .get(mcp_name)
            .cloned())
    }

    pub(crate) fn catalog_snapshot(&self) -> Arc<CatalogSnapshot> {
        Arc::clone(
            &self
                .shared
                .catalog_snapshot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        )
    }

    pub(crate) fn next_detached_execution_id(&self) -> String {
        let sequence = self
            .shared
            .next_execution_id
            .fetch_add(1, Ordering::Relaxed);
        format!("mcp:job:e:{sequence}")
    }

    pub(crate) fn call_job_tool(&self, request: CallToolRequestParams) -> ToolCallOutcome {
        let result = match request.name.as_ref() {
            JOB_START_TOOL => self.job_start(request.arguments.unwrap_or_default()),
            JOB_STATUS_TOOL => self.job_status(request.arguments.unwrap_or_default(), false),
            JOB_RESULT_TOOL => self.job_status(request.arguments.unwrap_or_default(), true),
            JOB_CANCEL_TOOL => self.job_cancel(request.arguments.unwrap_or_default()),
            _ => Err(unknown_tool_error(&request.name)),
        };
        ToolCallOutcome::unobserved(result)
    }

    pub(crate) fn job_start(
        &self,
        mut arguments: JsonObject,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let tool = match arguments.remove("tool") {
            Some(Value::String(tool)) if !tool.trim().is_empty() => tool,
            _ => {
                return Ok(command_error_result(job_argument_error(
                    "job.start requires a non-empty string property 'tool'",
                )));
            }
        };
        let mut target_arguments = match arguments.remove("arguments") {
            Some(Value::Object(arguments)) => arguments,
            _ => {
                return Ok(command_error_result(job_argument_error(
                    "job.start requires an object property 'arguments'",
                )));
            }
        };
        if !arguments.is_empty() {
            return Ok(command_error_result(job_argument_error(format!(
                "unknown job.start properties: {}",
                arguments.keys().cloned().collect::<Vec<_>>().join(", ")
            ))));
        }
        if matches!(
            tool.as_str(),
            JOB_START_TOOL | JOB_STATUS_TOOL | JOB_RESULT_TOOL | JOB_CANCEL_TOOL
        ) {
            return Ok(command_error_result(job_argument_error(
                "job.start cannot target ah.job.* tools",
            )));
        }
        let command = match self.find_command(&tool) {
            Ok(Some(command)) => command,
            Ok(None) => {
                return Ok(command_error_result(job_argument_error(format!(
                    "unknown target tool '{tool}'"
                ))));
            }
            Err(error) => return Err(internal_catalog_error(error)),
        };
        let execution_id = self.next_detached_execution_id();
        let event_context =
            self.shared
                .event_dispatcher
                .as_ref()
                .map(|dispatcher| JobEventContext {
                    dispatcher: Arc::clone(dispatcher),
                    command: command.descriptor.id.clone(),
                    tool: tool.clone(),
                    request_id: execution_id.clone(),
                    parameters: event_parameters(&target_arguments),
                    started: Instant::now(),
                });
        let context = match extract_context(
            &mut target_arguments,
            &execution_id,
            &self.shared.config,
            &command.descriptor,
            self.require_explicit_cwd,
        ) {
            Ok(context) => context,
            Err(error) => return Ok(command_error_result(error)),
        };
        if let Err(error) = validate_mcp_plaintext_auth(&command.descriptor.id, &target_arguments) {
            return Ok(command_error_result(error));
        }
        if let Err(error) = ah_runtime::typed::validate_mcp_arguments(
            &command.descriptor,
            &Value::Object(target_arguments.clone()),
        ) {
            return Ok(command_error_result(job_argument_error(error.to_string())));
        }
        let target_request = TypedInvocationRequest::new(
            command.descriptor.id,
            Value::Object(target_arguments),
            context,
        );
        let reservation = match self.shared.jobs.reserve(tool.clone(), execution_id) {
            Ok(reservation) => reservation,
            Err(error) => return Ok(command_error_result(job_registry_error(error))),
        };
        let handle = match self.shared.executor.try_submit(target_request) {
            Ok(handle) => handle,
            Err(error) => {
                self.shared.jobs.rollback(reservation);
                return Ok(command_error_result(CommandError::from(error)));
            }
        };
        let shared = Arc::downgrade(&self.shared);
        let completion_hook = Box::new(move || refresh_catalog_after_job(shared));
        let snapshot = self.shared.jobs.attach(
            reservation,
            handle,
            CommandError::from,
            event_context,
            Some(completion_hook),
        );
        Ok(job_snapshot_result(&snapshot, false))
    }

    pub(crate) fn job_status(
        &self,
        mut arguments: JsonObject,
        include_result: bool,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let job_id = match take_job_id(&mut arguments) {
            Ok(job_id) => job_id,
            Err(error) => return Ok(command_error_result(error)),
        };
        if !arguments.is_empty() {
            return Ok(command_error_result(job_argument_error(
                "job control tools accept only 'job_id'",
            )));
        }
        match self.shared.jobs.snapshot(&job_id) {
            Ok(snapshot) => Ok(job_snapshot_result(&snapshot, include_result)),
            Err(error) => Ok(command_error_result(job_registry_error(error))),
        }
    }

    pub(crate) fn job_cancel(
        &self,
        mut arguments: JsonObject,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let job_id = match take_job_id(&mut arguments) {
            Ok(job_id) => job_id,
            Err(error) => return Ok(command_error_result(error)),
        };
        if !arguments.is_empty() {
            return Ok(command_error_result(job_argument_error(
                "job control tools accept only 'job_id'",
            )));
        }
        match self
            .shared
            .jobs
            .cancel(&job_id, self.shared.executor.as_ref(), CommandError::from)
        {
            Ok(snapshot) => Ok(job_snapshot_result(&snapshot, false)),
            Err(error) => Ok(command_error_result(job_registry_error(error))),
        }
    }

    #[cfg(test)]
    pub(crate) async fn call_tool_inner(
        &self,
        request: CallToolRequestParams,
        request_id: String,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.call_tool_inner_observed(request, request_id)
            .await
            .result
    }

    pub(crate) async fn call_tool_inner_observed(
        &self,
        request: CallToolRequestParams,
        request_id: String,
    ) -> ToolCallOutcome {
        if request.name.starts_with("ah.job.") {
            return self.call_job_tool(request);
        }
        let command = match self.find_command(&request.name) {
            Ok(Some(command)) => command,
            Ok(None) => {
                return ToolCallOutcome::unobserved(Err(unknown_tool_error(&request.name)));
            }
            Err(error) => {
                return ToolCallOutcome::unobserved(Err(internal_catalog_error(error)));
            }
        };
        let mut arguments = request.arguments.unwrap_or_default();
        let require_explicit_cwd =
            requires_explicit_cwd(&command.descriptor, &arguments, self.require_explicit_cwd);
        let context = match extract_context(
            &mut arguments,
            &request_id,
            &self.shared.config,
            &command.descriptor,
            require_explicit_cwd,
        ) {
            Ok(context) => context,
            Err(error) => {
                return ToolCallOutcome::unobserved(Ok(command_error_result(error)));
            }
        };
        if let Err(error) = validate_mcp_plaintext_auth(&command.descriptor.id, &arguments) {
            return ToolCallOutcome::unobserved(Ok(command_error_result(error)));
        }
        let request =
            TypedInvocationRequest::new(command.descriptor.id, Value::Object(arguments), context);
        let observed = self.shared.executor.execute_observed(request).await;
        let result = match observed.result {
            Ok(response) => Ok(typed_response_result(response, &request_id)),
            Err(error) => Ok(command_error_result(CommandError::from(error))),
        };
        ToolCallOutcome {
            result,
            telemetry: observed.telemetry,
        }
    }

    pub(crate) fn cancel_request(&self, request_id: &NumberOrString) -> bool {
        let protocol_request_id = execution_request_id(request_id);
        let execution_ids = self
            .active_executions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&protocol_request_id)
            .cloned()
            .unwrap_or_default();
        let mut cancelled = false;
        for execution_id in execution_ids {
            if self.shared.executor.cancel(&execution_id) {
                cancelled = true;
            }
        }
        cancelled
    }

    pub(crate) fn begin_execution(&self, request_id: &NumberOrString) -> ActiveExecution<'_> {
        let protocol_request_id = execution_request_id(request_id);
        let sequence = self
            .shared
            .next_execution_id
            .fetch_add(1, Ordering::Relaxed);
        let execution_id = format!("{protocol_request_id}:e:{sequence}");
        self.active_executions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry(protocol_request_id.clone())
            .or_default()
            .push(execution_id.clone());
        ActiveExecution {
            active_executions: &self.active_executions,
            executor: Arc::clone(&self.shared.executor),
            protocol_request_id,
            execution_id,
        }
    }

    pub(crate) async fn call_tool_completed(
        &self,
        request: CallToolRequestParams,
        protocol_request_id: &NumberOrString,
        peer: Option<&Peer<RoleServer>>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let event_context = self.shared.event_dispatcher.as_ref().map(|_| {
            let tool = request.name.to_string();
            let parameters = event_parameters(&request.arguments.clone().unwrap_or_default());
            let canonical_command = self
                .find_command(&tool)
                .ok()
                .flatten()
                .map(|command| command.descriptor.id);
            (Instant::now(), tool, parameters, canonical_command)
        });
        let execution = self.begin_execution(protocol_request_id);
        let request_id = execution.execution_id().to_owned();
        let outcome = self
            .call_tool_inner_observed(request, request_id.clone())
            .await;
        drop(execution);
        if let (Some(dispatcher), Some((started, tool, parameters, canonical_command))) =
            (&self.shared.event_dispatcher, event_context)
        {
            let (status, diagnostic) =
                command_event_outcome(&outcome.result, canonical_command.as_deref());
            let command_outcome =
                command_event_run_check_outcome(&outcome.result, canonical_command.as_deref());
            dispatcher.dispatch(
                McpCommandEvent {
                    command: canonical_command.unwrap_or_else(|| tool.clone()),
                    tool,
                    request_id,
                    job_id: None,
                    parameters,
                    status,
                    duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                    diagnostic,
                    outcome: command_outcome,
                },
                outcome.telemetry,
            );
        }
        if self.refresh_catalog_generation().unwrap_or(false) {
            if let Some(peer) = peer {
                self.register_peer(peer.clone());
            }
            self.notify_tool_list_changed_all();
        }
        outcome.result
    }
}

pub(crate) fn refresh_catalog_generation_shared(
    shared: &McpShared,
) -> Result<bool, McpAdapterError> {
    let runtime_revision = shared.manager.catalog_revision();
    if shared
        .catalog_snapshot
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .runtime_revision
        == runtime_revision
    {
        return Ok(false);
    }
    let next = Arc::new(build_catalog_snapshot(&shared.manager)?);
    let mut current = shared
        .catalog_snapshot
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if current.runtime_revision >= next.runtime_revision {
        return Ok(false);
    }
    *current = next;
    shared.catalog_generation.fetch_add(1, Ordering::AcqRel);
    Ok(true)
}

pub(crate) fn notify_tool_list_changed_shared(shared: &Arc<McpShared>) {
    let peers = shared
        .peers
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter()
        .map(|(session_id, registered)| {
            (*session_id, registered.generation, registered.peer.clone())
        })
        .collect::<Vec<_>>();
    for (session_id, generation, peer) in peers {
        let shared = Arc::clone(shared);
        spawn_best_effort_notification(
            async move { peer.notify_tool_list_changed().await.is_ok() },
            PEER_NOTIFICATION_TIMEOUT,
            move || {
                remove_peer_generation(&shared, session_id, generation);
            },
        );
    }
}

pub(crate) fn refresh_catalog_after_job(shared: Weak<McpShared>) {
    let Some(shared) = shared.upgrade() else {
        return;
    };
    if refresh_catalog_generation_shared(&shared).unwrap_or(false) {
        notify_tool_list_changed_shared(&shared);
    }
}

pub(crate) fn remove_peer_generation(shared: &McpShared, session_id: u64, generation: u64) -> bool {
    let mut peers = shared
        .peers
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if peer_generation_matches(
        peers.get(&session_id).map(|peer| peer.generation),
        generation,
    ) {
        peers.remove(&session_id);
        true
    } else {
        false
    }
}

pub(crate) fn peer_generation_matches(current: Option<u64>, expected: u64) -> bool {
    current == Some(expected)
}

pub(crate) fn spawn_best_effort_notification<F, C>(notification: F, timeout: Duration, on_stale: C)
where
    F: Future<Output = bool> + Send + 'static,
    C: FnOnce() + Send + 'static,
{
    tokio::spawn(async move {
        let delivered = tokio::time::timeout(timeout, notification)
            .await
            .unwrap_or(false);
        if !delivered {
            on_stale();
        }
    });
}

pub(crate) struct ActiveExecution<'a> {
    active_executions: &'a Mutex<HashMap<String, Vec<String>>>,
    executor: Arc<dyn Executor>,
    protocol_request_id: String,
    execution_id: String,
}

impl ActiveExecution<'_> {
    pub(crate) fn execution_id(&self) -> &str {
        &self.execution_id
    }
}

impl Drop for ActiveExecution<'_> {
    fn drop(&mut self) {
        self.executor.cancel(&self.execution_id);
        let mut active_executions = self
            .active_executions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(execution_ids) = active_executions.get_mut(&self.protocol_request_id) else {
            return;
        };
        execution_ids.retain(|execution_id| execution_id != &self.execution_id);
        if execution_ids.is_empty() {
            active_executions.remove(&self.protocol_request_id);
        }
    }
}

impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_tool_list_changed()
                .build(),
        )
        .with_server_info(
            Implementation::new("aihelper", env!("CARGO_PKG_VERSION"))
                .with_title("AIHelper MCP")
                .with_description("Typed AIHelper commands over MCP stdio or local HTTP"),
        )
        .with_instructions(
            "Use ah.* tools. Inspect each tool's impact and risk metadata before execution.",
        )
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, rmcp::ErrorData> {
        self.refresh_catalog_generation()
            .map_err(internal_catalog_error)?;
        let tools = self.tools().map_err(internal_catalog_error)?;
        Ok(ListToolsResult::with_all_items(tools))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.catalog_snapshot().tools_by_name.get(name).cloned()
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.call_tool_completed(request, &context.id, Some(&context.peer))
            .await
    }

    async fn on_cancelled(
        &self,
        notification: CancelledNotificationParam,
        _context: NotificationContext<RoleServer>,
    ) {
        if let Some(request_id) = notification.request_id.as_ref() {
            self.cancel_request(request_id);
        }
    }

    async fn on_initialized(&self, context: NotificationContext<RoleServer>) {
        self.register_peer(context.peer);
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        self.shared
            .peers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.session_id);
    }
}

/// Clone tool arguments for event logging with the `context` wrapper removed.
///
/// The `context` object carries the caller's `cwd`, `limit`, and `timeout_ms`,
/// which are execution plumbing rather than tool inputs and must not leak into
/// the recorded parameter payload.
pub(crate) fn event_parameters(arguments: &JsonObject) -> Value {
    let mut parameters = arguments.clone();
    parameters.remove("context");
    redact_mcp_plaintext_auth(&mut parameters);
    Value::Object(parameters)
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        future::{pending, ready},
        sync::{
            Arc, Barrier, Mutex,
            atomic::{AtomicBool, AtomicU64, Ordering},
        },
        time::Duration,
    };

    use ah_plugin_api::{
        AH_PLUGIN_ABI_VERSION, CommandCatalog, CommandDescriptor, CommandEffect, CommandEffects,
        CommandExample, ExecutionContextWire, InvocationRequest, InvocationResponse,
        PluginCompatibility, PluginManual, PluginMetadata, ResolvedSecret, Reversibility,
        RiskLevel, SecretSlot, TypedInvocationRequest, TypedInvocationResponse,
        plugin_capabilities,
    };
    use ah_runtime::{
        BuiltinPlugin, InvocationOutcome, PluginManager, RunCheckOutcome, RuntimeError,
        SecretResolver, SecretResolverError,
        executor::{
            ExecutionFuture, ExecutionTelemetry, ExecutionTimeoutPhase, ObservedExecution,
            ObservedExecutionFuture, ParallelExecutor,
        },
    };
    use rmcp::model::{CallToolRequestParams, ErrorCode, JsonObject, NumberOrString};
    use serde_json::{Map, Value, json};
    use tokio::io::AsyncReadExt;
    use tokio_util::sync::CancellationToken;
    use uuid::Uuid;

    use ah_redact::REDACTED;

    use super::{
        EventSink, Executor, JOB_START_TOOL, McpAdapterError, McpCommandEvent, McpCommandStatus,
        McpServer, McpServerConfig, RISK_META_KEY, event_parameters, peer_generation_matches,
        refresh_catalog_after_job, spawn_best_effort_notification,
    };
    use crate::{
        mapping::{
            extract_context, requires_explicit_cwd, reserved_job_namespace_error, run_check_outcome,
        },
        plaintext_auth::{redact_mcp_plaintext_auth, validate_mcp_plaintext_auth},
        shutdown::{ShutdownReader, ShutdownTracker},
        transport::{HttpLifecycleController, HttpLifecycleState, wait_for_transport},
    };

    struct TypedPlugin;

    struct CredentialTypedPlugin {
        received: Arc<Mutex<Vec<TypedInvocationRequest>>>,
    }

    struct CredentialResolver;

    fn descriptor(id: &str) -> CommandDescriptor {
        CommandDescriptor::new(
            id,
            "Echo",
            "Echo a typed value.",
            json!({
                "type": "object",
                "properties": {"value": {"type": "string"}},
                "required": ["value"],
                "additionalProperties": false
            }),
            json!({
                "type": "object",
                "properties": {"value": {"type": "string"}},
                "required": ["value"],
                "additionalProperties": false
            }),
            CommandEffects::new(
                false,
                false,
                true,
                false,
                vec![CommandEffect::ExternalWrite],
                RiskLevel::Medium,
                "Writes to an in-memory test recorder.",
                Reversibility::Yes,
            ),
        )
        .with_example(CommandExample::new("Echo hello", json!({"value": "hello"})))
    }

    impl BuiltinPlugin for TypedPlugin {
        fn metadata(&self) -> PluginMetadata {
            PluginMetadata {
                plugin_name: "typed-test".to_owned(),
                domain: "test".to_owned(),
                description: "typed test plugin".to_owned(),
                abi_version: AH_PLUGIN_ABI_VERSION,
                required_tools: Vec::new(),
                compatibility: PluginCompatibility::current()
                    .with_capability(plugin_capabilities::TYPED_COMMANDS_V1),
            }
        }

        fn manual(&self) -> PluginManual {
            PluginManual {
                plugin_name: "typed-test".to_owned(),
                domain: "test".to_owned(),
                description: "typed test plugin".to_owned(),
                commands: Vec::new(),
                notes: Vec::new(),
            }
        }

        fn invoke(&self, _request: &InvocationRequest) -> InvocationResponse {
            InvocationResponse::ok(None)
        }

        fn command_catalog(&self) -> Option<CommandCatalog> {
            Some(CommandCatalog::new(
                "typed-test",
                "test",
                vec![descriptor("test.echo")],
            ))
        }
    }

    impl SecretResolver for CredentialResolver {
        fn resolve(&self, id: &str) -> Result<ResolvedSecret, SecretResolverError> {
            Ok(ResolvedSecret {
                id: id.to_owned(),
                kind: "postgres".to_owned(),
                values: BTreeMap::from([("password".to_owned(), "private-password".to_owned())]),
            })
        }
    }

    impl BuiltinPlugin for CredentialTypedPlugin {
        fn metadata(&self) -> PluginMetadata {
            PluginMetadata {
                plugin_name: "credential-test".to_owned(),
                domain: "test".to_owned(),
                description: "credential test plugin".to_owned(),
                abi_version: AH_PLUGIN_ABI_VERSION,
                required_tools: Vec::new(),
                compatibility: PluginCompatibility::current()
                    .with_capability(plugin_capabilities::TYPED_COMMANDS_V1),
            }
        }

        fn manual(&self) -> PluginManual {
            PluginManual {
                plugin_name: "credential-test".to_owned(),
                domain: "test".to_owned(),
                description: "credential test plugin".to_owned(),
                commands: Vec::new(),
                notes: Vec::new(),
            }
        }

        fn invoke(&self, _request: &InvocationRequest) -> InvocationResponse {
            InvocationResponse::ok(None)
        }

        fn command_catalog(&self) -> Option<CommandCatalog> {
            Some(CommandCatalog::new(
                "credential-test",
                "test",
                vec![
                    descriptor("test.secret").with_secret_slot(SecretSlot::optional(
                        "database",
                        ["postgres"],
                        "Database credential",
                    )),
                ],
            ))
        }

        fn invoke_typed(&self, request: &TypedInvocationRequest) -> TypedInvocationResponse {
            self.received.lock().unwrap().push(request.clone());
            TypedInvocationResponse::success(json!({"value": request.arguments["value"]}), None)
        }
    }

    struct RecordingExecutor {
        request: Mutex<Option<TypedInvocationRequest>>,
        cancelled: Mutex<Vec<String>>,
        fail_timeout: bool,
        telemetry: Option<ExecutionTelemetry>,
    }

    struct DefaultObservedExecutor;

    struct PendingExecutor {
        request: Mutex<Option<TypedInvocationRequest>>,
        cancelled: Mutex<Vec<String>>,
    }

    #[derive(Default)]
    struct ClosingExecutor {
        closed: AtomicBool,
        close_count: AtomicU64,
    }

    impl Executor for DefaultObservedExecutor {
        fn execute(&self, request: TypedInvocationRequest) -> ExecutionFuture<'_> {
            Box::pin(async move {
                Ok(TypedInvocationResponse::success(
                    json!({"value": request.arguments["value"]}),
                    None,
                ))
            })
        }

        fn cancel(&self, _request_id: &str) -> bool {
            false
        }
    }

    #[test]
    fn managed_http_lifecycle_uses_the_preselected_identity() {
        let instance_id = Uuid::new_v4();
        let pid = 42;
        let tracker = Arc::new(ShutdownTracker::new(Duration::from_secs(1)));
        let executor: Arc<dyn Executor> = Arc::new(ClosingExecutor::default());
        let controller = Arc::new(HttpLifecycleController::new(
            tracker,
            executor,
            CancellationToken::new(),
        ));
        let lifecycle = HttpLifecycleState::new(
            "1.2.3".to_owned(),
            instance_id,
            pid,
            "127.0.0.1:8787".to_owned(),
            "http://127.0.0.1:8787".to_owned(),
            controller,
            None,
        );
        assert_eq!(lifecycle.readiness.instance_id, instance_id);
        assert_eq!(lifecycle.readiness.pid, pid);
        assert_eq!(lifecycle.readiness.version, "1.2.3");
    }

    impl Executor for PendingExecutor {
        fn execute(&self, request: TypedInvocationRequest) -> ExecutionFuture<'_> {
            *self.request.lock().unwrap() = Some(request);
            Box::pin(std::future::pending())
        }

        fn cancel(&self, request_id: &str) -> bool {
            self.cancelled.lock().unwrap().push(request_id.to_owned());
            true
        }
    }

    impl Executor for ClosingExecutor {
        fn execute(&self, _request: TypedInvocationRequest) -> ExecutionFuture<'_> {
            Box::pin(std::future::pending())
        }

        fn cancel(&self, _request_id: &str) -> bool {
            false
        }

        fn close(&self) {
            self.closed.store(true, Ordering::Release);
            self.close_count.fetch_add(1, Ordering::AcqRel);
        }
    }

    impl RecordingExecutor {
        fn success() -> Arc<Self> {
            Arc::new(Self {
                request: Mutex::new(None),
                cancelled: Mutex::new(Vec::new()),
                fail_timeout: false,
                telemetry: None,
            })
        }

        fn timeout() -> Arc<Self> {
            Arc::new(Self {
                request: Mutex::new(None),
                cancelled: Mutex::new(Vec::new()),
                fail_timeout: true,
                telemetry: Some(ExecutionTelemetry {
                    queue_wait_ms: 250,
                    execution_ms: 0,
                    timeout_phase: Some(ExecutionTimeoutPhase::Queue),
                }),
            })
        }
    }

    impl Executor for RecordingExecutor {
        fn execute(&self, request: TypedInvocationRequest) -> ExecutionFuture<'_> {
            *self.request.lock().unwrap() = Some(request.clone());
            let fail_timeout = self.fail_timeout;
            Box::pin(async move {
                if fail_timeout {
                    Err(RuntimeError::ExecutionTimeout {
                        request_id: request.context.request_id,
                    })
                } else {
                    Ok(TypedInvocationResponse::success(
                        json!({"value": request.arguments["value"]}),
                        Some("echoed".to_owned()),
                    ))
                }
            })
        }

        fn execute_observed(&self, request: TypedInvocationRequest) -> ObservedExecutionFuture<'_> {
            let telemetry = self.telemetry;
            Box::pin(async move {
                ObservedExecution {
                    result: self.execute(request).await,
                    telemetry,
                }
            })
        }

        fn cancel(&self, request_id: &str) -> bool {
            self.cancelled.lock().unwrap().push(request_id.to_owned());
            true
        }
    }

    #[derive(Default)]
    struct RecordingEventSink {
        events: Mutex<Vec<McpCommandEvent>>,
        telemetry: Mutex<Vec<Option<ExecutionTelemetry>>>,
    }

    #[derive(Default)]
    struct DefaultTelemetryEventSink {
        events: Mutex<Vec<McpCommandEvent>>,
    }

    impl EventSink for DefaultTelemetryEventSink {
        fn record_command(&self, event: McpCommandEvent) {
            self.events.lock().unwrap().push(event);
        }
    }

    impl EventSink for RecordingEventSink {
        fn record_command(&self, event: McpCommandEvent) {
            self.events.lock().unwrap().push(event);
        }

        fn record_command_with_telemetry(
            &self,
            event: McpCommandEvent,
            telemetry: Option<ExecutionTelemetry>,
        ) {
            self.events.lock().unwrap().push(event);
            self.telemetry.lock().unwrap().push(telemetry);
        }
    }

    fn manager() -> Arc<PluginManager> {
        let mut manager = PluginManager::new();
        manager.register_builtin(Arc::new(TypedPlugin));
        Arc::new(manager)
    }

    fn server(executor: Arc<RecordingExecutor>) -> McpServer {
        McpServer::new(
            manager(),
            executor,
            McpServerConfig::new("default-cwd", Some(10), 300).unwrap(),
        )
        .unwrap()
    }

    fn server_with_sink(executor: Arc<RecordingExecutor>) -> (McpServer, Arc<RecordingEventSink>) {
        let sink = Arc::new(RecordingEventSink::default());
        let event_sink: Arc<dyn EventSink> = sink.clone();
        (server(executor).with_event_sink(event_sink), sink)
    }

    fn credential_server() -> (
        McpServer,
        Arc<Mutex<Vec<TypedInvocationRequest>>>,
        Arc<RecordingEventSink>,
    ) {
        let received = Arc::new(Mutex::new(Vec::new()));
        let mut manager = PluginManager::new();
        manager.set_secret_resolver(Arc::new(CredentialResolver));
        manager.register_builtin(Arc::new(CredentialTypedPlugin {
            received: Arc::clone(&received),
        }));
        let manager = Arc::new(manager);
        let executor: Arc<dyn Executor> =
            Arc::new(ParallelExecutor::new(Arc::clone(&manager), 2).unwrap());
        let sink = Arc::new(RecordingEventSink::default());
        let event_sink: Arc<dyn EventSink> = sink.clone();
        let server = McpServer::new(
            manager,
            executor,
            McpServerConfig::new("default-cwd", Some(10), 1_000).unwrap(),
        )
        .unwrap()
        .with_event_sink(event_sink);
        (server, received, sink)
    }

    async fn wait_for_recorded_events(sink: &RecordingEventSink, expected: usize) {
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                let event_count = sink.events.lock().unwrap().len();
                let telemetry_count = sink.telemetry.lock().unwrap().len();
                if event_count >= expected && telemetry_count >= expected {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("event dispatcher should deliver within one second");
    }

    fn arguments(value: Value) -> JsonObject {
        value.as_object().unwrap().clone()
    }

    fn test_event() -> McpCommandEvent {
        McpCommandEvent {
            command: "test.echo".to_owned(),
            tool: "ah.test.echo".to_owned(),
            request_id: "test-request".to_owned(),
            job_id: None,
            parameters: json!({"value": "hello"}),
            status: McpCommandStatus::Success,
            duration_ms: 1,
            diagnostic: None,
            outcome: None,
        }
    }

    #[test]
    fn run_check_outcome_extracts_only_allowlisted_fields() {
        let data = json!({
            "success": false,
            "timed_out": false,
            "exit_code": 7,
            "stdout": "secret output",
            "stderr": "secret error",
            "argv": ["secret-command"]
        });

        assert_eq!(
            run_check_outcome("run.check", Some(&data)),
            Some(InvocationOutcome::RunCheck(RunCheckOutcome {
                success: false,
                timed_out: false,
                exit_code: Some(7),
            }))
        );
        assert_eq!(run_check_outcome("search.text", Some(&data)), None);
        assert_eq!(run_check_outcome("run.check", Some(&json!({}))), None);
    }

    #[test]
    fn default_observation_and_sink_methods_remain_compatible() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let executor = DefaultObservedExecutor;
            let observed = executor
                .execute_observed(TypedInvocationRequest::new(
                    "test.echo",
                    json!({"value": "hello"}),
                    ExecutionContextWire::new("default-observed", ".", None, 100),
                ))
                .await;
            assert!(observed.result.unwrap().success);
            assert_eq!(observed.telemetry, None);

            let sink = DefaultTelemetryEventSink::default();
            EventSink::record_command_with_telemetry(
                &sink,
                test_event(),
                Some(ExecutionTelemetry {
                    queue_wait_ms: 1,
                    execution_ms: 1,
                    timeout_phase: None,
                }),
            );
            assert_eq!(sink.events.lock().unwrap().len(), 1);
        });
    }

    #[test]
    fn peer_notification_timeout_is_detached_and_cleans_up() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let (cleanup_tx, cleanup_rx) = tokio::sync::oneshot::channel();
            let started = std::time::Instant::now();

            spawn_best_effort_notification(
                std::future::pending(),
                std::time::Duration::from_millis(10),
                move || {
                    let _ = cleanup_tx.send(());
                },
            );

            assert!(started.elapsed() < std::time::Duration::from_millis(100));
            tokio::time::timeout(std::time::Duration::from_secs(1), cleanup_rx)
                .await
                .unwrap()
                .unwrap();
        });
    }

    #[test]
    fn stale_peer_generation_does_not_match_new_registration() {
        assert!(peer_generation_matches(Some(2), 2));
        assert!(!peer_generation_matches(Some(2), 1));
        assert!(!peer_generation_matches(None, 1));
    }

    #[test]
    fn lifecycle_controller_runs_shutdown_once_under_concurrency() {
        let tracker = Arc::new(ShutdownTracker::new(std::time::Duration::from_secs(1)));
        let executor = Arc::new(ClosingExecutor::default());
        let executor_dyn: Arc<dyn Executor> = executor.clone();
        let cancellation = CancellationToken::new();
        let controller = Arc::new(HttpLifecycleController::new(
            Arc::clone(&tracker),
            executor_dyn,
            cancellation.clone(),
        ));
        let barrier = Arc::new(Barrier::new(9));
        let handles = (0..8)
            .map(|_| {
                let barrier = Arc::clone(&barrier);
                let controller = Arc::clone(&controller);
                std::thread::spawn(move || {
                    barrier.wait();
                    controller.begin_shutdown()
                })
            })
            .collect::<Vec<_>>();

        barrier.wait();
        let winners = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .filter(|won| *won)
            .count();

        assert_eq!(winners, 1);
        assert!(executor.closed.load(Ordering::Acquire));
        assert_eq!(executor.close_count.load(Ordering::Acquire), 1);
        assert!(cancellation.is_cancelled());
        let started_at = *tracker.started_at.get().expect("shutdown should start");
        assert!(!controller.begin_shutdown());
        assert_eq!(tracker.started_at.get(), Some(&started_at));
    }

    #[test]
    fn transport_wait_returns_typed_shutdown_timeout() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let tracker = ShutdownTracker::new(std::time::Duration::from_millis(1));
            tracker.begin();

            let error = wait_for_transport(pending::<Result<(), McpAdapterError>>(), &tracker)
                .await
                .expect_err("pending transport should exceed shutdown grace");

            assert!(matches!(
                error,
                McpAdapterError::ShutdownTimeout { grace_ms: 1 }
            ));
            assert_eq!(
                error.to_string(),
                "MCP shutdown exceeded the configured 1 ms grace period"
            );
        });
    }

    #[test]
    fn transport_wait_preserves_ready_results_at_deadline() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let tracker = ShutdownTracker::new(std::time::Duration::ZERO);
            tracker.begin();

            assert!(wait_for_transport(ready(Ok(())), &tracker).await.is_ok());

            let error = wait_for_transport(
                ready(Err(McpAdapterError::Service("transport failed".to_owned()))),
                &tracker,
            )
            .await
            .expect_err("ready transport error should be preserved");
            assert!(matches!(
                error,
                McpAdapterError::Service(message) if message == "transport failed"
            ));
        });
    }

    #[test]
    fn stdio_eof_starts_shutdown_and_closes_executor() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let tracker = Arc::new(ShutdownTracker::new(std::time::Duration::from_secs(1)));
            let executor = Arc::new(ClosingExecutor::default());
            let executor_dyn: Arc<dyn Executor> = executor.clone();
            let (stream, peer) = tokio::io::duplex(16);
            drop(peer);
            let mut reader = ShutdownReader::new(stream, Arc::clone(&tracker), executor_dyn);
            let mut buffer = [0_u8; 1];

            assert_eq!(reader.read(&mut buffer).await.unwrap(), 0);
            assert!(executor.closed.load(Ordering::Acquire));
            assert!(tracker.started_at.get().is_some());
            assert!(tracker.remaining() <= std::time::Duration::from_secs(1));
        });
    }

    #[test]
    fn maps_descriptor_to_typed_mcp_tool() {
        let server = server(RecordingExecutor::success());
        let tools = server.tools().unwrap();
        assert_eq!(tools.len(), 5);
        let tool = tools
            .iter()
            .find(|tool| tool.name == "ah.test.echo")
            .expect("typed plugin tool must be published");
        assert_eq!(tool.name, "ah.test.echo");
        assert!(tool.input_schema["properties"]["context"].is_object());
        assert_eq!(tool.output_schema.as_ref().unwrap()["type"], "object");
        let annotations = tool.annotations.as_ref().unwrap();
        assert_eq!(annotations.read_only_hint, Some(false));
        assert_eq!(annotations.destructive_hint, Some(false));
        assert_eq!(annotations.idempotent_hint, Some(true));
        assert_eq!(annotations.open_world_hint, Some(false));
        assert_eq!(
            tool.meta.as_ref().unwrap().0["dev.aihelper/risk"]["level"],
            "medium"
        );
        assert!(tool.description.as_ref().unwrap().contains("Impact:"));
        assert!(
            tool.description
                .as_ref()
                .unwrap()
                .contains("Examples:\n- Echo hello: {\"value\":\"hello\"}")
        );
        assert!(
            tool.input_schema["properties"]["context"]["description"]
                .as_str()
                .unwrap()
                .contains("project-relative paths")
        );
        let job_start = tools
            .iter()
            .find(|tool| tool.name == JOB_START_TOOL)
            .expect("job.start must be published");
        assert_eq!(
            job_start.annotations.as_ref().unwrap().read_only_hint,
            Some(false)
        );
        assert_eq!(
            job_start.annotations.as_ref().unwrap().destructive_hint,
            Some(true)
        );
        assert_eq!(
            job_start.meta.as_ref().unwrap().0[RISK_META_KEY]["level"],
            "critical"
        );
    }

    #[tokio::test]
    async fn generated_tool_description_names_slot_kinds_and_discovery_tool() {
        let (server, _, _) = credential_server();
        let tools = server.tools().unwrap();
        let description = tools
            .iter()
            .find(|tool| tool.name == "ah.test.secret")
            .and_then(|tool| tool.description.as_deref())
            .expect("credential tool must have a description");

        assert!(description.contains("Credential slots: database accepts postgres."));
        assert!(description.contains("call secrets.list with kind=postgres"));
    }

    #[test]
    fn rejects_plugin_commands_in_reserved_job_namespace() {
        let error = reserved_job_namespace_error("job.extra")
            .expect("reserved job command should be rejected");
        assert!(error.to_string().contains("ah.job.*"));
        assert!(reserved_job_namespace_error("jobs.extra").is_none());
    }

    #[test]
    fn catalog_generation_is_stable_without_changes() {
        let server = server(RecordingExecutor::success());
        assert_eq!(server.catalog_generation(), 1);
        assert!(!server.refresh_catalog_generation().unwrap());
        assert_eq!(server.catalog_generation(), 1);
    }

    #[test]
    fn catalog_snapshot_refreshes_once_per_runtime_revision() {
        let manager = manager();
        let server = McpServer::new(
            Arc::clone(&manager),
            RecordingExecutor::success(),
            McpServerConfig::new("default-cwd", Some(10), 300).unwrap(),
        )
        .unwrap();
        assert_eq!(server.tools().unwrap().len(), 5);

        manager.set_disabled_domains(vec!["test".to_owned()]);
        assert!(server.refresh_catalog_generation().unwrap());
        assert_eq!(server.tools().unwrap().len(), 4);
        assert_eq!(server.catalog_generation(), 2);

        manager.set_disabled_domains(vec!["TEST".to_owned()]);
        assert!(!server.refresh_catalog_generation().unwrap());
        assert_eq!(server.catalog_generation(), 2);
    }

    #[test]
    fn detached_job_completion_refreshes_shared_catalog() {
        let manager = manager();
        let server = McpServer::new(
            Arc::clone(&manager),
            RecordingExecutor::success(),
            McpServerConfig::new("default-cwd", Some(10), 300).unwrap(),
        )
        .unwrap();
        assert_eq!(server.tools().unwrap().len(), 5);

        manager.set_disabled_domains(vec!["test".to_owned()]);
        refresh_catalog_after_job(Arc::downgrade(&server.shared));

        assert_eq!(server.tools().unwrap().len(), 4);
        assert_eq!(server.catalog_generation(), 2);
    }

    #[test]
    fn rejects_invalid_server_defaults() {
        assert!(McpServerConfig::new("", None, 1).is_err());
        assert!(McpServerConfig::new(".", Some(0), 1).is_err());
        assert!(McpServerConfig::new(".", None, 0).is_err());
    }

    #[test]
    fn execution_ids_are_unique_and_cancellation_mappings_are_scoped() {
        let executor = RecordingExecutor::success();
        let server = server(Arc::clone(&executor));
        let protocol_request_id = NumberOrString::String("same".to_owned().into());

        let first = server.begin_execution(&protocol_request_id);
        let second = server.begin_execution(&protocol_request_id);
        assert_ne!(first.execution_id(), second.execution_id());
        assert!(first.execution_id().starts_with("mcp:s:same:e:"));

        assert!(server.cancel_request(&protocol_request_id));
        let cancelled = executor.cancelled.lock().unwrap().clone();
        assert_eq!(
            cancelled,
            vec![
                first.execution_id().to_owned(),
                second.execution_id().to_owned()
            ]
        );
        executor.cancelled.lock().unwrap().clear();

        drop(first);
        drop(second);
        assert_eq!(
            executor.cancelled.lock().unwrap().as_slice(),
            cancelled.as_slice()
        );
        assert!(!server.cancel_request(&protocol_request_id));

        let reused = server.begin_execution(&protocol_request_id);
        assert!(reused.execution_id().starts_with("mcp:s:same:e:"));
        assert_eq!(
            server
                .active_executions
                .lock()
                .unwrap()
                .get("mcp:s:same")
                .map(Vec::len),
            Some(1)
        );
    }

    #[test]
    fn dropping_direct_call_future_cancels_executor_entry() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let executor = Arc::new(PendingExecutor {
                request: Mutex::new(None),
                cancelled: Mutex::new(Vec::new()),
            });
            let server = Arc::new(
                McpServer::new(
                    manager(),
                    executor.clone(),
                    McpServerConfig::new("default-cwd", Some(10), 300).unwrap(),
                )
                .unwrap(),
            );
            let request = CallToolRequestParams::new("ah.test.echo")
                .with_arguments(arguments(json!({"value": "hello"})));
            let protocol_request_id = NumberOrString::String("dropped".to_owned().into());
            let task = tokio::spawn({
                let server = Arc::clone(&server);
                async move {
                    server
                        .call_tool_completed(request, &protocol_request_id, None)
                        .await
                }
            });

            tokio::time::timeout(std::time::Duration::from_secs(1), async {
                while executor.request.lock().unwrap().is_none() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            task.abort();
            let _ = task.await;

            assert_eq!(
                executor.cancelled.lock().unwrap().as_slice(),
                &["mcp:s:dropped:e:1"]
            );
            assert!(server.active_executions.lock().unwrap().is_empty());
        });
    }

    #[test]
    fn records_one_successful_command_event() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let (server, sink) = server_with_sink(RecordingExecutor::success());
            let parameters = json!({
                "value": "hello",
                "context": {"cwd": "custom-cwd", "limit": 20, "timeout_ms": 500}
            });
            let request = CallToolRequestParams::new("ah.test.echo")
                .with_arguments(arguments(parameters.clone()));
            let protocol_request_id = NumberOrString::String("event-success".to_owned().into());

            let result = server
                .call_tool_completed(request, &protocol_request_id, None)
                .await
                .unwrap();

            assert_eq!(result.is_error, Some(false));
            wait_for_recorded_events(&sink, 1).await;
            let events = sink.events.lock().unwrap();
            assert_eq!(events.len(), 1);
            let event = &events[0];
            assert_eq!(event.command, "test.echo");
            assert_eq!(event.tool, "ah.test.echo");
            assert_eq!(event.request_id, "mcp:s:event-success:e:1");
            // The `context` wrapper is execution plumbing and must not leak into
            // the recorded parameters.
            assert_eq!(event.parameters, json!({"value": "hello"}));
            assert_eq!(event.status, McpCommandStatus::Success);
            assert_eq!(event.diagnostic, None);
            drop(events);
            assert_eq!(sink.telemetry.lock().unwrap().as_slice(), &[None]);
        });
    }

    #[test]
    fn records_typed_error_diagnostic_once() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let (server, sink) = server_with_sink(RecordingExecutor::timeout());
            let request = CallToolRequestParams::new("ah.test.echo")
                .with_arguments(arguments(json!({"value": "hello"})));
            let protocol_request_id = NumberOrString::String("event-error".to_owned().into());

            let result = server
                .call_tool_completed(request, &protocol_request_id, None)
                .await
                .unwrap();

            assert_eq!(result.is_error, Some(true));
            wait_for_recorded_events(&sink, 1).await;
            let events = sink.events.lock().unwrap();
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].command, "test.echo");
            assert_eq!(events[0].status, McpCommandStatus::Error);
            assert_eq!(events[0].diagnostic.as_ref().unwrap().code, "TIMEOUT");
            drop(events);
            assert_eq!(
                sink.telemetry.lock().unwrap().as_slice(),
                &[Some(ExecutionTelemetry {
                    queue_wait_ms: 250,
                    execution_ms: 0,
                    timeout_phase: Some(ExecutionTimeoutPhase::Queue),
                })]
            );
        });
    }

    #[test]
    fn records_unknown_tool_protocol_error_once() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let (server, sink) = server_with_sink(RecordingExecutor::success());
            let protocol_request_id = NumberOrString::String("event-unknown".to_owned().into());

            let error = server
                .call_tool_completed(
                    CallToolRequestParams::new("ah.test.missing"),
                    &protocol_request_id,
                    None,
                )
                .await
                .unwrap_err();

            assert_eq!(error.code, ErrorCode::METHOD_NOT_FOUND);
            wait_for_recorded_events(&sink, 1).await;
            let events = sink.events.lock().unwrap();
            assert_eq!(events.len(), 1);
            let event = &events[0];
            assert_eq!(event.command, "ah.test.missing");
            assert_eq!(event.tool, "ah.test.missing");
            assert_eq!(event.parameters, json!({}));
            assert_eq!(event.status, McpCommandStatus::Error);
            let diagnostic = event.diagnostic.as_ref().unwrap();
            assert_eq!(diagnostic.code, "MCP_PROTOCOL_ERROR");
            assert!(
                diagnostic
                    .cause
                    .contains("unknown MCP tool 'ah.test.missing'")
            );
        });
    }

    #[test]
    fn mcp_http_plaintext_auth_validation_covers_all_supported_forms() {
        for value in [
            json!({"url": "https://example.test", "bearer": "bearer-sentinel"}),
            json!({"url": "https://example.test", "basic": "user:basic-sentinel"}),
            json!({"url": "https://example.test", "headers": ["aUtHoRiZaTiOn: Bearer header-sentinel"]}),
            json!({"url": "https://user:url-field-sentinel@example.test"}),
            json!({"curl": "curl https://example.test --user user:curl-user-sentinel"}),
            json!({"curl": "curl https://example.test --header='Authorization: Bearer curl-header-sentinel'"}),
            json!({"curl": "curl https://example.test -H'Authorization: Bearer compact-header-sentinel'"}),
            json!({"curl": "curl https://example.test -uuser:compact-user-sentinel"}),
            json!({"curl": "curl https://user:url-userinfo-sentinel@example.test"}),
            json!({"curl": "curl --url=https://user:url-option-sentinel@example.test"}),
        ] {
            let error = validate_mcp_plaintext_auth("http.replay", &arguments(value))
                .expect_err("plaintext auth must be rejected");
            let rendered = serde_json::to_string(&error).unwrap();
            assert!(!rendered.contains("sentinel"));
        }
        validate_mcp_plaintext_auth(
            "http.get",
            &arguments(json!({
                "url": "https://example.test",
                "credentials": {"basic": "api-basic"}
            })),
        )
        .expect("vault credential ids remain allowed");
    }

    #[test]
    fn mcp_rejects_inline_forge_tokens_but_keeps_credential_ids() {
        for command in ["github.repo", "gitlab.project"] {
            let error = validate_mcp_plaintext_auth(
                command,
                &arguments(json!({"token": "forge-token-sentinel"})),
            )
            .expect_err("inline tokens must be rejected over MCP");
            let rendered = serde_json::to_string(&error).unwrap();
            assert_eq!(error.code, "INVALID_ARGUMENT");
            assert!(!rendered.contains("sentinel"));

            validate_mcp_plaintext_auth(
                command,
                &arguments(json!({"credentials": {"token": "work-pat"}})),
            )
            .expect("vault credential ids remain allowed");
        }

        // Inline tokens stay redacted in the events recorded for the rejection.
        let mut parameters = arguments(json!({"token": "event-token-sentinel"}));
        redact_mcp_plaintext_auth(&mut parameters);
        assert_eq!(parameters["token"], json!(REDACTED));
    }

    #[test]
    fn rejected_mcp_auth_is_redacted_from_immediate_and_job_events() {
        for value in [
            json!({"bearer": "immediate-event-sentinel"}),
            json!({"url": "https://user:url-field-event-sentinel@example.test"}),
            json!({
                "tool": "ah.http.replay",
                "arguments": {
                    "curl": "curl https://example.test -H 'Authorization: Bearer job-event-sentinel'"
                }
            }),
            json!({
                "tool": "ah.http.replay",
                "arguments": {
                    "curl": "curl https://user:url-event-sentinel@example.test"
                }
            }),
        ] {
            let rendered = event_parameters(&arguments(value)).to_string();
            assert!(!rendered.contains("event-sentinel"));
            assert!(rendered.contains(REDACTED));
        }
    }

    #[test]
    fn immediate_mcp_credential_invocation_resolves_and_redacts_structured_event() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let (server, received, sink) = credential_server();
            let request = CallToolRequestParams::new("ah.test.secret").with_arguments(arguments(
                json!({"value": "immediate", "credentials": {"database": "qa-lms"}}),
            ));
            let protocol_request_id =
                NumberOrString::String("credential-immediate".to_owned().into());

            let result = server
                .call_tool_completed(request, &protocol_request_id, None)
                .await
                .unwrap();

            assert_eq!(result.is_error, Some(false));
            {
                let requests = received.lock().unwrap();
                assert_eq!(requests.len(), 1);
                assert_eq!(requests[0].arguments["credentials"]["database"], "qa-lms");
                assert_eq!(
                    requests[0].resolved_secrets["database"].values["password"],
                    "private-password"
                );
            }

            wait_for_recorded_events(&sink, 1).await;
            let events = sink.events.lock().unwrap();
            assert_eq!(events.len(), 1);
            assert_eq!(
                events[0].parameters,
                json!({"value": "immediate", "credentials": {"database": "qa-lms"}})
            );
            assert!(
                !events[0]
                    .parameters
                    .to_string()
                    .contains("private-password")
            );
        });
    }

    #[test]
    fn detached_mcp_credential_invocation_resolves_when_job_executes() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let (server, received, sink) = credential_server();
            let request =
                CallToolRequestParams::new(JOB_START_TOOL).with_arguments(arguments(json!({
                    "tool": "ah.test.secret",
                    "arguments": {
                        "value": "detached",
                        "credentials": {"database": "qa-lms"}
                    }
                })));
            let protocol_request_id =
                NumberOrString::String("credential-detached".to_owned().into());

            let result = server
                .call_tool_completed(request, &protocol_request_id, None)
                .await
                .unwrap();
            assert_eq!(result.is_error, Some(false));

            tokio::time::timeout(Duration::from_secs(1), async {
                loop {
                    if !received.lock().unwrap().is_empty() {
                        return;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("detached credential invocation should execute");
            {
                let requests = received.lock().unwrap();
                assert_eq!(requests.len(), 1);
                assert_eq!(requests[0].arguments["credentials"]["database"], "qa-lms");
                assert_eq!(requests[0].resolved_secrets["database"].id, "qa-lms");
                assert_eq!(
                    requests[0].resolved_secrets["database"].values["password"],
                    "private-password"
                );
            }

            wait_for_recorded_events(&sink, 2).await;
            let events = sink.events.lock().unwrap();
            let target_event = events
                .iter()
                .find(|event| event.command == "test.secret")
                .expect("detached target event should be recorded");
            assert_eq!(
                target_event.parameters,
                json!({"value": "detached", "credentials": {"database": "qa-lms"}})
            );
            assert!(
                events
                    .iter()
                    .all(|event| !event.parameters.to_string().contains("private-password"))
            );
        });
    }

    #[test]
    fn call_extracts_context_and_returns_structured_content() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let executor = RecordingExecutor::success();
            let server = server(Arc::clone(&executor));
            let request =
                CallToolRequestParams::new("ah.test.echo").with_arguments(arguments(json!({
                    "value": "hello",
                    "context": {
                        "cwd": "custom-cwd",
                        "limit": 20,
                        "timeout_ms": 500
                    }
                })));
            let result = server
                .call_tool_inner(request, "mcp:n:7".to_owned())
                .await
                .unwrap();
            assert_eq!(result.is_error, Some(false));
            assert_eq!(result.structured_content, Some(json!({"value": "hello"})));
            let request = executor.request.lock().unwrap().clone().unwrap();
            assert_eq!(request.arguments, json!({"value": "hello"}));
            assert_eq!(request.context.request_id, "mcp:n:7");
            assert_eq!(request.context.cwd, "custom-cwd");
            assert_eq!(request.context.limit, Some(20));
            assert_eq!(request.context.remaining_timeout_ms, 500);
        });
    }

    #[test]
    fn call_uses_default_context() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let executor = RecordingExecutor::success();
            let server = server(Arc::clone(&executor));
            let request = CallToolRequestParams::new("ah.test.echo")
                .with_arguments(arguments(json!({"value": "hello"})));
            server
                .call_tool_inner(request, "mcp:s:req".to_owned())
                .await
                .unwrap();
            let request = executor.request.lock().unwrap().clone().unwrap();
            assert_eq!(request.context.cwd, "default-cwd");
            assert_eq!(request.context.limit, Some(10));
            assert_eq!(request.context.remaining_timeout_ms, 300);
        });
    }

    #[test]
    fn http_context_uses_defaults_for_stateless_commands() {
        for command in ["ai.info", "postgres.ping", "http.get"] {
            let arguments = Map::new();
            let context = extract_context(
                &mut arguments.clone(),
                "mcp:n:stateless",
                &McpServerConfig::new("default-cwd", Some(10), 300).unwrap(),
                &descriptor(command),
                requires_explicit_cwd(&descriptor(command), &arguments, true),
            )
            .unwrap();
            assert_eq!(context.cwd, "default-cwd", "{command}");
        }
    }

    #[test]
    fn http_context_requires_cwd_for_file_backed_commands() {
        let arguments = Map::new();
        let error = extract_context(
            &mut arguments.clone(),
            "mcp:n:http-assert",
            &McpServerConfig::new("default-cwd", Some(10), 300).unwrap(),
            &descriptor("http.assert"),
            requires_explicit_cwd(&descriptor("http.assert"), &arguments, true),
        )
        .unwrap_err();

        assert_eq!(error.code, "INVALID_CONTEXT");
    }

    #[test]
    fn http_context_is_optional_for_a_hosted_api_call_that_names_its_project() {
        for (command, arguments) in [
            (
                "gitlab.pipeline.wait",
                json!({"project": "group/tool", "pipeline_id": 1}),
            ),
            (
                "github.release.get",
                json!({"repo": "acme/tool", "tag": "v1"}),
            ),
        ] {
            let arguments = arguments.as_object().unwrap().clone();
            assert!(
                !requires_explicit_cwd(&descriptor(command), &arguments, true),
                "{command} needs no working directory"
            );
        }
    }

    #[test]
    fn http_context_is_required_when_a_hosted_api_call_needs_the_working_tree() {
        for (command, arguments) in [
            ("gitlab.pipeline.wait", json!({"pipeline_id": 1})),
            ("github.release.get", json!({"tag": "v1"})),
            (
                "github.release.create",
                json!({"repo": "acme/tool", "tag": "v1", "notes_file": "docs/notes.md"}),
            ),
            (
                "gitlab.issue.create",
                json!({"project": "group/tool", "description_file": "docs/issue.md"}),
            ),
        ] {
            let arguments = arguments.as_object().unwrap().clone();
            assert!(
                requires_explicit_cwd(&descriptor(command), &arguments, true),
                "{command} reads the working tree"
            );
        }
    }

    #[test]
    fn invalid_context_is_a_visible_tool_error() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let executor = RecordingExecutor::success();
            let server = server(Arc::clone(&executor));
            let request = CallToolRequestParams::new("ah.test.echo").with_arguments(arguments(
                json!({"value": "hello", "context": {"unknown": true}}),
            ));
            let result = server
                .call_tool_inner(request, "mcp:n:8".to_owned())
                .await
                .unwrap();
            assert_eq!(result.is_error, Some(true));
            assert!(result.structured_content.is_none());
            assert!(executor.request.lock().unwrap().is_none());
            assert_eq!(
                result.meta.unwrap().0["dev.aihelper/diagnostic"]["code"],
                "INVALID_CONTEXT"
            );
        });
    }

    #[test]
    fn runtime_failure_is_a_visible_tool_error() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let server = server(RecordingExecutor::timeout());
            let request = CallToolRequestParams::new("ah.test.echo")
                .with_arguments(arguments(json!({"value": "hello"})));
            let result = server
                .call_tool_inner(request, "mcp:n:9".to_owned())
                .await
                .unwrap();
            assert_eq!(result.is_error, Some(true));
            assert_eq!(
                result.meta.unwrap().0["dev.aihelper/diagnostic"]["code"],
                "TIMEOUT"
            );
        });
    }

    #[test]
    fn unknown_tool_is_a_protocol_error() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let server = server(RecordingExecutor::success());
            let error = server
                .call_tool_inner(
                    CallToolRequestParams::new("ah.test.missing"),
                    "mcp:n:10".to_owned(),
                )
                .await
                .unwrap_err();
            assert_eq!(error.code, ErrorCode::METHOD_NOT_FOUND);
        });
    }
}
