#![allow(clippy::result_large_err)]

use std::{
    borrow::Cow,
    collections::HashMap,
    future::{Future, IntoFuture},
    net::Ipv4Addr,
    path::Path,
    pin::Pin,
    sync::{
        Arc, Mutex, OnceLock, Weak,
        atomic::{AtomicU64, Ordering},
    },
    task::{Context, Poll},
    time::{Duration, Instant},
};

use ah_plugin_api::{
    CommandDescriptor, CommandError, ExecutionContextWire, TypedInvocationRequest,
    TypedInvocationResponse,
};
use ah_runtime::{
    PluginManager, RegisteredCommand, RuntimeError,
    executor::{ExecutionTelemetry, Executor},
};
use axum::{
    Json, Router,
    extract::State,
    http::{
        HeaderMap, StatusCode,
        header::{HOST, ORIGIN},
    },
    routing::get,
};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use rmcp::{
    Peer, RoleServer, ServerHandler, ServiceExt,
    model::{
        CallToolRequestParams, CallToolResult, CancelledNotificationParam, ContentBlock, ErrorCode,
        Implementation, JsonObject, ListToolsResult, Meta, NumberOrString, PaginatedRequestParams,
        ServerCapabilities, ServerInfo, TaskSupport, Tool, ToolAnnotations, ToolExecution,
    },
    service::{NotificationContext, RequestContext},
    transport::stdio,
};
use serde::Serialize;
use serde_json::{Map, Value, json};
use thiserror::Error;
use tokio::{
    io::{AsyncRead, ReadBuf},
    sync::Notify,
};
use tower_http::limit::RequestBodyLimitLayer;
use uuid::Uuid;

use crate::{
    events::EventDispatcher,
    jobs::{JobEventContext, JobRegistry, JobRegistryError, JobSnapshot, JobStatus},
};

const TOOL_PREFIX: &str = "ah.";
const RISK_META_KEY: &str = "dev.aihelper/risk";
const DIAGNOSTIC_META_KEY: &str = "dev.aihelper/diagnostic";
const EXECUTION_META_KEY: &str = "dev.aihelper/execution";
const JOB_TOOL_PREFIX: &str = "ah.job.";
const JOB_START_TOOL: &str = "ah.job.start";
const JOB_STATUS_TOOL: &str = "ah.job.status";
const JOB_RESULT_TOOL: &str = "ah.job.result";
const JOB_CANCEL_TOOL: &str = "ah.job.cancel";
const PEER_NOTIFICATION_TIMEOUT: Duration = Duration::from_secs(1);
const DEFAULT_SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

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
}

#[must_use]
pub struct McpServeOutcome {
    result: Result<(), McpAdapterError>,
    remaining_shutdown_grace: Duration,
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
    shared: Arc<McpShared>,
    active_executions: Mutex<HashMap<String, Vec<String>>>,
    require_explicit_cwd: bool,
    session_id: u64,
}

struct McpShared {
    manager: Arc<PluginManager>,
    executor: Arc<dyn Executor>,
    config: McpServerConfig,
    catalog_snapshot: Mutex<Arc<CatalogSnapshot>>,
    catalog_generation: AtomicU64,
    next_execution_id: AtomicU64,
    event_dispatcher: Option<Arc<EventDispatcher>>,
    jobs: Arc<JobRegistry>,
    peers: Mutex<HashMap<u64, RegisteredPeer>>,
    next_session_id: AtomicU64,
    next_peer_generation: AtomicU64,
}

struct RegisteredPeer {
    generation: u64,
    peer: Peer<RoleServer>,
}

struct CatalogSnapshot {
    runtime_revision: u64,
    tools: Vec<Tool>,
    tools_by_name: HashMap<String, Tool>,
    commands_by_name: HashMap<String, RegisteredCommand>,
}

struct ToolCallOutcome {
    result: Result<CallToolResult, rmcp::ErrorData>,
    telemetry: Option<ExecutionTelemetry>,
}

impl ToolCallOutcome {
    fn unobserved(result: Result<CallToolResult, rmcp::ErrorData>) -> Self {
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

    pub(crate) fn http_session(&self) -> Self {
        let session_id = self.shared.next_session_id.fetch_add(1, Ordering::Relaxed);
        Self {
            shared: Arc::clone(&self.shared),
            active_executions: Mutex::new(HashMap::new()),
            require_explicit_cwd: true,
            session_id,
        }
    }

    fn register_peer(&self, peer: Peer<RoleServer>) {
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

    fn notify_tool_list_changed_all(&self) {
        notify_tool_list_changed_shared(&self.shared);
    }

    pub fn catalog_generation(&self) -> u64 {
        self.shared.catalog_generation.load(Ordering::Acquire)
    }

    pub fn dropped_event_count(&self) -> u64 {
        self.shared
            .event_dispatcher
            .as_ref()
            .map_or(0, |dispatcher| dispatcher.dropped_count())
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

    fn find_command(&self, mcp_name: &str) -> Result<Option<RegisteredCommand>, McpAdapterError> {
        if !mcp_name.starts_with(TOOL_PREFIX) {
            return Ok(None);
        }
        Ok(self
            .catalog_snapshot()
            .commands_by_name
            .get(mcp_name)
            .cloned())
    }

    fn catalog_snapshot(&self) -> Arc<CatalogSnapshot> {
        Arc::clone(
            &self
                .shared
                .catalog_snapshot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        )
    }

    fn next_detached_execution_id(&self) -> String {
        let sequence = self
            .shared
            .next_execution_id
            .fetch_add(1, Ordering::Relaxed);
        format!("mcp:job:e:{sequence}")
    }

    fn call_job_tool(&self, request: CallToolRequestParams) -> ToolCallOutcome {
        let result = match request.name.as_ref() {
            JOB_START_TOOL => self.job_start(request.arguments.unwrap_or_default()),
            JOB_STATUS_TOOL => self.job_status(request.arguments.unwrap_or_default(), false),
            JOB_RESULT_TOOL => self.job_status(request.arguments.unwrap_or_default(), true),
            JOB_CANCEL_TOOL => self.job_cancel(request.arguments.unwrap_or_default()),
            _ => Err(unknown_tool_error(&request.name)),
        };
        ToolCallOutcome::unobserved(result)
    }

    fn job_start(&self, mut arguments: JsonObject) -> Result<CallToolResult, rmcp::ErrorData> {
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
        if let Err(error) = ah_runtime::typed::validate_arguments(
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
                return Ok(command_error_result(runtime_command_error(error)));
            }
        };
        let shared = Arc::downgrade(&self.shared);
        let completion_hook = Box::new(move || refresh_catalog_after_job(shared));
        let snapshot = self.shared.jobs.attach(
            reservation,
            handle,
            runtime_command_error,
            event_context,
            Some(completion_hook),
        );
        Ok(job_snapshot_result(&snapshot, false))
    }

    fn job_status(
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

    fn job_cancel(&self, mut arguments: JsonObject) -> Result<CallToolResult, rmcp::ErrorData> {
        let job_id = match take_job_id(&mut arguments) {
            Ok(job_id) => job_id,
            Err(error) => return Ok(command_error_result(error)),
        };
        if !arguments.is_empty() {
            return Ok(command_error_result(job_argument_error(
                "job control tools accept only 'job_id'",
            )));
        }
        match self.shared.jobs.cancel(
            &job_id,
            self.shared.executor.as_ref(),
            runtime_command_error,
        ) {
            Ok(snapshot) => Ok(job_snapshot_result(&snapshot, false)),
            Err(error) => Ok(command_error_result(job_registry_error(error))),
        }
    }

    #[cfg(test)]
    async fn call_tool_inner(
        &self,
        request: CallToolRequestParams,
        request_id: String,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.call_tool_inner_observed(request, request_id)
            .await
            .result
    }

    async fn call_tool_inner_observed(
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
        let context = match extract_context(
            &mut arguments,
            &request_id,
            &self.shared.config,
            &command.descriptor,
            self.require_explicit_cwd,
        ) {
            Ok(context) => context,
            Err(error) => {
                return ToolCallOutcome::unobserved(Ok(command_error_result(error)));
            }
        };
        let request =
            TypedInvocationRequest::new(command.descriptor.id, Value::Object(arguments), context);
        let observed = self.shared.executor.execute_observed(request).await;
        let result = match observed.result {
            Ok(response) => Ok(typed_response_result(response, &request_id)),
            Err(error) => Ok(command_error_result(runtime_command_error(error))),
        };
        ToolCallOutcome {
            result,
            telemetry: observed.telemetry,
        }
    }

    fn cancel_request(&self, request_id: &NumberOrString) -> bool {
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

    fn begin_execution(&self, request_id: &NumberOrString) -> ActiveExecution<'_> {
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

    async fn call_tool_completed(
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

fn refresh_catalog_generation_shared(shared: &McpShared) -> Result<bool, McpAdapterError> {
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

fn notify_tool_list_changed_shared(shared: &Arc<McpShared>) {
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

fn refresh_catalog_after_job(shared: Weak<McpShared>) {
    let Some(shared) = shared.upgrade() else {
        return;
    };
    if refresh_catalog_generation_shared(&shared).unwrap_or(false) {
        notify_tool_list_changed_shared(&shared);
    }
}

fn remove_peer_generation(shared: &McpShared, session_id: u64, generation: u64) -> bool {
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

fn peer_generation_matches(current: Option<u64>, expected: u64) -> bool {
    current == Some(expected)
}

fn spawn_best_effort_notification<F, C>(notification: F, timeout: Duration, on_stale: C)
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

struct ActiveExecution<'a> {
    active_executions: &'a Mutex<HashMap<String, Vec<String>>>,
    executor: Arc<dyn Executor>,
    protocol_request_id: String,
    execution_id: String,
}

impl ActiveExecution<'_> {
    fn execution_id(&self) -> &str {
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

struct ShutdownTracker {
    started_at: OnceLock<Instant>,
    changed: Notify,
    grace: Duration,
}

#[derive(Clone)]
struct HttpLifecycleState {
    readiness: ReadinessResponse,
    authority: String,
    origin: String,
}

impl HttpLifecycleState {
    fn new(version: String, authority: String, origin: String) -> Self {
        Self {
            readiness: ReadinessResponse {
                status: "ready",
                version,
                pid: std::process::id(),
                instance_id: Uuid::new_v4().to_string(),
            },
            authority,
            origin,
        }
    }
}

#[derive(Clone, Serialize)]
struct ReadinessResponse {
    status: &'static str,
    version: String,
    pid: u32,
    instance_id: String,
}

impl ShutdownTracker {
    fn new(grace: Duration) -> Self {
        Self {
            started_at: OnceLock::new(),
            changed: Notify::new(),
            grace,
        }
    }

    fn begin(&self) {
        if self.started_at.set(Instant::now()).is_ok() {
            self.changed.notify_waiters();
        }
    }

    fn remaining(&self) -> Duration {
        self.started_at.get().map_or(self.grace, |started_at| {
            self.grace.saturating_sub(started_at.elapsed())
        })
    }

    async fn expired(&self) {
        loop {
            if let Some(started_at) = self.started_at.get() {
                tokio::time::sleep_until((*started_at + self.grace).into()).await;
                return;
            }
            let changed = self.changed.notified();
            if self.started_at.get().is_some() {
                continue;
            }
            changed.await;
        }
    }
}

struct ShutdownReader<R> {
    inner: R,
    tracker: Arc<ShutdownTracker>,
    executor: Arc<dyn Executor>,
    shutdown_started: bool,
}

impl<R> ShutdownReader<R> {
    fn new(inner: R, tracker: Arc<ShutdownTracker>, executor: Arc<dyn Executor>) -> Self {
        Self {
            inner,
            tracker,
            executor,
            shutdown_started: false,
        }
    }

    fn begin_shutdown(&mut self) {
        if !self.shutdown_started {
            self.shutdown_started = true;
            self.tracker.begin();
            self.executor.close();
        }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for ShutdownReader<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let had_capacity = buffer.remaining() > 0;
        let filled_before = buffer.filled().len();
        let result = Pin::new(&mut this.inner).poll_read(context, buffer);
        if matches!(&result, Poll::Ready(Err(_)))
            || matches!(&result, Poll::Ready(Ok(())) if had_capacity && buffer.filled().len() == filled_before)
        {
            this.begin_shutdown();
        }
        result
    }
}

pub async fn serve_stdio(server: McpServer) -> Result<(), McpAdapterError> {
    serve_stdio_bounded(server, DEFAULT_SHUTDOWN_GRACE)
        .await
        .into_result()
}

pub async fn serve_stdio_bounded(server: McpServer, grace: Duration) -> McpServeOutcome {
    let executor = Arc::clone(&server.shared.executor);
    let event_dispatcher = server.shared.event_dispatcher.clone();
    let tracker = Arc::new(ShutdownTracker::new(grace));
    let (stdin, stdout) = stdio();
    let reader = ShutdownReader::new(stdin, Arc::clone(&tracker), Arc::clone(&executor));
    let result = match server.serve((reader, stdout)).await {
        Ok(service) => {
            let waiting = service.waiting();
            tokio::pin!(waiting);
            tokio::select! {
                result = &mut waiting => {
                    tracker.begin();
                    executor.close();
                    result
                        .map(|_| ())
                        .map_err(|error| McpAdapterError::Service(error.to_string()))
                }
                _ = tracker.expired() => Ok(()),
            }
        }
        Err(error) => {
            tracker.begin();
            executor.close();
            Err(McpAdapterError::Service(error.to_string()))
        }
    };
    tracker.begin();
    executor.close();
    if let Some(dispatcher) = event_dispatcher {
        dispatcher.flush(tracker.remaining()).await;
    }
    McpServeOutcome {
        result,
        remaining_shutdown_grace: tracker.remaining(),
    }
}

pub async fn serve_http(server: McpServer, port: u16) -> Result<(), McpAdapterError> {
    serve_http_bounded(server, port, DEFAULT_SHUTDOWN_GRACE)
        .await
        .into_result()
}

pub async fn serve_http_bounded(server: McpServer, port: u16, grace: Duration) -> McpServeOutcome {
    serve_http_bounded_with_version(server, port, env!("CARGO_PKG_VERSION"), grace).await
}

pub async fn serve_http_bounded_with_version(
    server: McpServer,
    port: u16,
    version: impl Into<String>,
    grace: Duration,
) -> McpServeOutcome {
    let tracker = Arc::new(ShutdownTracker::new(grace));
    let executor = Arc::clone(&server.shared.executor);
    let event_dispatcher = server.shared.event_dispatcher.clone();
    let authority = format!("127.0.0.1:{port}");
    let origin = format!("http://{authority}");
    let lifecycle = HttpLifecycleState::new(version.into(), authority.clone(), origin.clone());
    let config = StreamableHttpServerConfig::default()
        .with_stateful_mode(true)
        .with_allowed_hosts([authority])
        .with_allowed_origins([origin]);
    let cancellation = config.cancellation_token.clone();
    let session_template = Arc::new(server);
    let factory_template = Arc::clone(&session_template);
    let service: StreamableHttpService<McpServer, LocalSessionManager> = StreamableHttpService::new(
        move || Ok(factory_template.http_session()),
        Arc::new(LocalSessionManager::default()),
        config,
    );
    let router = Router::new()
        .route("/health/ready", get(readiness))
        .nest_service("/mcp", service)
        .with_state(lifecycle)
        .layer(RequestBodyLimitLayer::new(1024 * 1024));
    let listener = match tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await {
        Ok(listener) => listener,
        Err(error) => {
            tracker.begin();
            executor.close();
            cancellation.cancel();
            return McpServeOutcome {
                result: Err(McpAdapterError::Service(error.to_string())),
                remaining_shutdown_grace: tracker.remaining(),
            };
        }
    };
    let shutdown_executor = Arc::clone(&executor);
    let shutdown_cancellation = cancellation.clone();
    let shutdown_tracker = Arc::clone(&tracker);
    let serving = axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            shutdown_tracker.begin();
            shutdown_executor.close();
            shutdown_cancellation.cancel();
        })
        .into_future();
    tokio::pin!(serving);
    let result = tokio::select! {
        result = &mut serving => result.map_err(|error| McpAdapterError::Service(error.to_string())),
        _ = tracker.expired() => Ok(()),
    };
    tracker.begin();
    executor.close();
    cancellation.cancel();
    if let Some(dispatcher) = event_dispatcher {
        dispatcher.flush(tracker.remaining()).await;
    }
    McpServeOutcome {
        result,
        remaining_shutdown_grace: tracker.remaining(),
    }
}

async fn readiness(
    State(state): State<HttpLifecycleState>,
    headers: HeaderMap,
) -> Result<Json<ReadinessResponse>, StatusCode> {
    validate_local_headers(&headers, &state)?;
    Ok(Json(state.readiness))
}

fn validate_local_headers(
    headers: &HeaderMap,
    state: &HttpLifecycleState,
) -> Result<(), StatusCode> {
    let host_matches = headers
        .get(HOST)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|host| host == state.authority);
    if !host_matches {
        return Err(StatusCode::FORBIDDEN);
    }

    let origin_matches = headers.get(ORIGIN).is_none_or(|value| {
        value
            .to_str()
            .ok()
            .is_some_and(|origin| origin == state.origin)
    });
    if !origin_matches {
        return Err(StatusCode::FORBIDDEN);
    }

    Ok(())
}

#[cfg(unix)]
async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();
    match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
        Ok(mut terminate) => {
            tokio::select! {
                _ = ctrl_c => {}
                _ = terminate.recv() => {}
            }
        }
        Err(_) => {
            let _ = ctrl_c.await;
        }
    }
}

#[cfg(not(unix))]
async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

fn job_tools() -> Vec<Tool> {
    vec![
        job_tool(
            JOB_START_TOOL,
            "Start AIHelper job",
            "Start any published AIHelper tool without waiting for its result. Inspect the target tool risk metadata before calling. Retries are not generally safe.",
            json!({
                "type": "object",
                "properties": {
                    "tool": {"type": "string", "minLength": 1},
                    "arguments": {"type": "object"}
                },
                "required": ["tool", "arguments"],
                "additionalProperties": false
            }),
            (false, true, false, true),
            "critical",
            "Can invoke any published AIHelper tool, including destructive commands.",
        ),
        job_tool(
            JOB_STATUS_TOOL,
            "Inspect AIHelper job",
            "Return the current state of an AIHelper background job without waiting.",
            job_id_schema(),
            (true, false, true, false),
            "low",
            "Reads process-local job metadata only.",
        ),
        job_tool(
            JOB_RESULT_TOOL,
            "Read AIHelper job result",
            "Return ready=false immediately while running, or the repeatable terminal result.",
            job_id_schema(),
            (true, false, true, false),
            "low",
            "Reads a retained process-local job result only.",
        ),
        job_tool(
            JOB_CANCEL_TOOL,
            "Cancel AIHelper job",
            "Request cooperative cancellation and return the job's current terminal state immediately.",
            job_id_schema(),
            (false, true, true, false),
            "medium",
            "Requests cancellation of one active command; the handler may continue draining.",
        ),
    ]
}

fn job_id_schema() -> Value {
    json!({
        "type": "object",
        "properties": {"job_id": {"type": "string", "minLength": 1}},
        "required": ["job_id"],
        "additionalProperties": false
    })
}

fn job_tool(
    name: &'static str,
    title: &'static str,
    description: &'static str,
    input_schema: Value,
    annotations: (bool, bool, bool, bool),
    risk: &'static str,
    impact: &'static str,
) -> Tool {
    let mut tool = Tool::new(
        name,
        description,
        input_schema
            .as_object()
            .cloned()
            .expect("job tool input schema must be an object"),
    );
    tool.title = Some(title.to_owned());
    tool.annotations = Some(ToolAnnotations::from_raw(
        Some(title.to_owned()),
        Some(annotations.0),
        Some(annotations.1),
        Some(annotations.2),
        Some(annotations.3),
    ));
    tool.execution = Some(ToolExecution::new().with_task_support(TaskSupport::Forbidden));
    tool.meta = Some(Meta(Map::from_iter([(
        RISK_META_KEY.to_owned(),
        json!({
            "level": risk,
            "impact": impact,
            "effects": ["process_state"],
            "reversibility": if name == JOB_START_TOOL { "unknown" } else { "yes" }
        }),
    )])));
    tool
}

/// Clone tool arguments for event logging with the `context` wrapper removed.
///
/// The `context` object carries the caller's `cwd`, `limit`, and `timeout_ms`,
/// which are execution plumbing rather than tool inputs and must not leak into
/// the recorded parameter payload.
fn event_parameters(arguments: &JsonObject) -> Value {
    let mut parameters = arguments.clone();
    parameters.remove("context");
    Value::Object(parameters)
}

fn take_job_id(arguments: &mut JsonObject) -> Result<String, CommandError> {
    match arguments.remove("job_id") {
        Some(Value::String(job_id)) if !job_id.trim().is_empty() => Ok(job_id),
        _ => Err(job_argument_error(
            "job control requires a non-empty string property 'job_id'",
        )),
    }
}

fn job_snapshot_result(snapshot: &JobSnapshot, include_result: bool) -> CallToolResult {
    let mut data = Map::from_iter([
        ("job_id".to_owned(), Value::String(snapshot.job_id.clone())),
        ("tool".to_owned(), Value::String(snapshot.tool.clone())),
        (
            "status".to_owned(),
            Value::String(snapshot.status.as_str().to_owned()),
        ),
        ("draining".to_owned(), Value::Bool(snapshot.draining)),
    ]);
    if include_result {
        let ready = snapshot.status != JobStatus::Running;
        data.insert("ready".to_owned(), Value::Bool(ready));
        if let Some(response) = &snapshot.response {
            data.insert(
                "response".to_owned(),
                serde_json::to_value(response)
                    .expect("typed invocation response must always serialize"),
            );
        }
    }
    let data = Value::Object(data);
    let mut result = CallToolResult::structured(data.clone());
    result.content = vec![ContentBlock::text(
        serde_json::to_string(&data).expect("job result must always serialize"),
    )];
    result
}

fn job_argument_error(cause: impl Into<String>) -> CommandError {
    CommandError::new(
        Some("job".to_owned()),
        None,
        "INVALID_ARGUMENT",
        "Invalid job tool arguments",
        cause,
        2,
        false,
    )
}

fn job_registry_error(error: JobRegistryError) -> CommandError {
    match error {
        JobRegistryError::CapacityFull { capacity } => CommandError::new(
            Some("job".to_owned()),
            Some("job.start".to_owned()),
            "JOB_CAPACITY_FULL",
            "AIHelper job registry is full",
            format!("all {capacity} retained records are active or draining"),
            1,
            true,
        ),
        JobRegistryError::NotFound { job_id } => CommandError::new(
            Some("job".to_owned()),
            None,
            "JOB_NOT_FOUND",
            "AIHelper job was not found",
            format!("job '{job_id}' is unknown, expired, or evicted"),
            2,
            false,
        ),
    }
}

fn command_to_tool(command: &RegisteredCommand) -> Result<Tool, McpAdapterError> {
    let descriptor = &command.descriptor;
    let input_schema = schema_object_with_context(descriptor)?;
    let output_schema = schema_object(&descriptor.id, "output", &descriptor.output_schema)?;
    let risk =
        serde_json::to_value(descriptor.effects.risk).expect("risk enum should always serialize");
    let reversibility = serde_json::to_value(descriptor.effects.reversibility)
        .expect("reversibility enum should always serialize");
    let effects = serde_json::to_value(&descriptor.effects.effects)
        .expect("effect enums should always serialize");
    let mut risk_meta = Map::new();
    risk_meta.insert("level".to_owned(), risk.clone());
    risk_meta.insert(
        "impact".to_owned(),
        Value::String(descriptor.effects.impact.clone()),
    );
    risk_meta.insert("effects".to_owned(), effects);
    risk_meta.insert("reversibility".to_owned(), reversibility);
    let mut meta = Map::new();
    meta.insert(RISK_META_KEY.to_owned(), Value::Object(risk_meta));

    let risk_label = risk.as_str().unwrap_or("unknown");
    let description = format!(
        "{}\n\nImpact: {}\nRisk: {risk_label}.",
        descriptor.description, descriptor.effects.impact
    );
    let mut tool = Tool::new(
        format!("{TOOL_PREFIX}{}", descriptor.id),
        description,
        input_schema,
    );
    tool.title = Some(descriptor.title.clone());
    tool.output_schema = Some(Arc::new(output_schema));
    tool.annotations = Some(ToolAnnotations::from_raw(
        Some(descriptor.title.clone()),
        Some(descriptor.effects.read_only),
        Some(descriptor.effects.destructive),
        Some(descriptor.effects.idempotent),
        Some(descriptor.effects.open_world),
    ));
    tool.execution = Some(ToolExecution::new().with_task_support(TaskSupport::Forbidden));
    tool.meta = Some(Meta(meta));
    Ok(tool)
}

fn schema_object_with_context(
    descriptor: &CommandDescriptor,
) -> Result<JsonObject, McpAdapterError> {
    let schema = ah_runtime::typed::mcp_input_schema(descriptor).map_err(|error| {
        McpAdapterError::InvalidSchema {
            command: descriptor.id.clone(),
            reason: error.to_string(),
        }
    })?;
    schema_object(&descriptor.id, "input", &schema)
}

fn schema_object(command: &str, kind: &str, schema: &Value) -> Result<JsonObject, McpAdapterError> {
    schema
        .as_object()
        .cloned()
        .ok_or_else(|| McpAdapterError::InvalidSchema {
            command: command.to_owned(),
            reason: format!("{kind} schema root must be an object"),
        })
}

fn extract_context(
    arguments: &mut JsonObject,
    request_id: &str,
    defaults: &McpServerConfig,
    descriptor: &CommandDescriptor,
    require_explicit_cwd: bool,
) -> Result<ExecutionContextWire, CommandError> {
    let context = arguments.remove("context");
    let Some(context) = context else {
        if require_explicit_cwd {
            return Err(context_error(
                descriptor,
                "context.cwd is required for HTTP execution",
            ));
        }
        return Ok(ExecutionContextWire::new(
            request_id,
            defaults.cwd.clone(),
            defaults.limit,
            defaults.default_timeout_ms,
        ));
    };
    let Some(context) = context.as_object() else {
        return Err(context_error(descriptor, "context must be a JSON object"));
    };
    for key in context.keys() {
        if !matches!(key.as_str(), "cwd" | "limit" | "timeout_ms") {
            return Err(context_error(
                descriptor,
                format!("unknown context property '{key}'"),
            ));
        }
    }

    let cwd = match context.get("cwd") {
        Some(Value::String(cwd)) if !cwd.trim().is_empty() => cwd.clone(),
        Some(_) => {
            return Err(context_error(
                descriptor,
                "context.cwd must be a non-empty string",
            ));
        }
        None if require_explicit_cwd => {
            return Err(context_error(
                descriptor,
                "context.cwd is required for HTTP execution",
            ));
        }
        None => defaults.cwd.clone(),
    };
    if require_explicit_cwd && !Path::new(&cwd).is_absolute() {
        return Err(context_error(
            descriptor,
            "context.cwd must be an absolute path for HTTP execution",
        ));
    }
    let limit = match context.get("limit") {
        Some(value) => Some(positive_usize(value, "context.limit", descriptor)?),
        None => defaults.limit,
    };
    let timeout_ms = match context.get("timeout_ms") {
        Some(value) => positive_u64(value, "context.timeout_ms", descriptor)?,
        None => defaults.default_timeout_ms,
    };
    Ok(ExecutionContextWire::new(
        request_id, cwd, limit, timeout_ms,
    ))
}

fn positive_usize(
    value: &Value,
    field: &str,
    descriptor: &CommandDescriptor,
) -> Result<usize, CommandError> {
    let value = positive_u64(value, field, descriptor)?;
    usize::try_from(value).map_err(|_| context_error(descriptor, format!("{field} is too large")))
}

fn positive_u64(
    value: &Value,
    field: &str,
    descriptor: &CommandDescriptor,
) -> Result<u64, CommandError> {
    value
        .as_u64()
        .filter(|value| *value > 0)
        .ok_or_else(|| context_error(descriptor, format!("{field} must be a positive integer")))
}

fn context_error(descriptor: &CommandDescriptor, cause: impl Into<String>) -> CommandError {
    CommandError::new(
        command_domain(&descriptor.id),
        Some(descriptor.id.clone()),
        "INVALID_CONTEXT",
        "Invalid MCP execution context",
        cause,
        2,
        false,
    )
}

fn typed_response_result(response: TypedInvocationResponse, request_id: &str) -> CallToolResult {
    if !response.success {
        return command_error_result(response.error.unwrap_or_else(|| {
            CommandError::new(
                None,
                None,
                "INVALID_TYPED_RESPONSE",
                "Typed command returned an invalid error response",
                "success=false without a diagnostic",
                1,
                false,
            )
        }));
    }
    let Some(data) = response.data else {
        return command_error_result(CommandError::new(
            None,
            None,
            "INVALID_TYPED_RESPONSE",
            "Typed command returned an invalid success response",
            "success=true without structured data",
            1,
            false,
        ));
    };
    let compact = serde_json::to_string(&data).unwrap_or_else(|_| "{}".to_owned());
    let mut result = CallToolResult::structured(data);
    result.content = vec![ContentBlock::text(compact)];
    let mut execution = Map::new();
    execution.insert(
        "request_id".to_owned(),
        Value::String(request_id.to_owned()),
    );
    if let Some(text) = response.text {
        execution.insert("text".to_owned(), Value::String(text));
    }
    if !response.notices.is_empty() {
        execution.insert(
            "notices".to_owned(),
            serde_json::to_value(response.notices)
                .expect("command notices should always serialize"),
        );
    }
    let mut meta = Map::new();
    meta.insert(EXECUTION_META_KEY.to_owned(), Value::Object(execution));
    result.meta = Some(Meta(meta));
    result
}

fn command_error_result(error: CommandError) -> CallToolResult {
    let text = format!("{}: {}", error.code, error.message);
    let mut result = CallToolResult::error(vec![ContentBlock::text(text)]);
    let mut meta = Map::new();
    meta.insert(
        DIAGNOSTIC_META_KEY.to_owned(),
        serde_json::to_value(error).expect("command error should always serialize"),
    );
    result.meta = Some(Meta(meta));
    result
}

fn command_event_outcome(
    result: &Result<CallToolResult, rmcp::ErrorData>,
    canonical_command: Option<&str>,
) -> (McpCommandStatus, Option<CommandError>) {
    match result {
        Ok(result) if result.is_error == Some(true) => {
            let diagnostic = result
                .meta
                .as_ref()
                .and_then(|meta| meta.0.get(DIAGNOSTIC_META_KEY))
                .and_then(|value| serde_json::from_value(value.clone()).ok())
                .unwrap_or_else(|| {
                    adapter_command_error(
                        canonical_command,
                        "MCP_ERROR_DIAGNOSTIC_MISSING",
                        "MCP tool returned an error without a diagnostic",
                        "the CallToolResult diagnostic metadata was missing or invalid",
                    )
                });
            (McpCommandStatus::Error, Some(diagnostic))
        }
        Ok(_) => (McpCommandStatus::Success, None),
        Err(error) => (
            McpCommandStatus::Error,
            Some(adapter_command_error(
                canonical_command,
                "MCP_PROTOCOL_ERROR",
                "MCP tool call failed",
                error.to_string(),
            )),
        ),
    }
}

fn adapter_command_error(
    canonical_command: Option<&str>,
    code: &'static str,
    message: &'static str,
    cause: impl Into<String>,
) -> CommandError {
    CommandError::new(
        canonical_command.and_then(command_domain),
        canonical_command.map(str::to_owned),
        code,
        message,
        cause,
        1,
        false,
    )
}

fn runtime_command_error(error: RuntimeError) -> CommandError {
    match error {
        RuntimeError::DomainNotFound(domain) => CommandError::new(
            Some(domain),
            None,
            "DOMAIN_NOT_FOUND",
            "Command domain was not found",
            "the plugin registry does not contain the requested domain",
            2,
            false,
        ),
        RuntimeError::TypedCommandNotFound(command) => CommandError::new(
            command_domain(&command),
            Some(command),
            "COMMAND_NOT_FOUND",
            "Typed command was not found",
            "the command catalog changed before execution",
            2,
            true,
        ),
        RuntimeError::DomainDisabled(domain) => CommandError::new(
            Some(domain),
            None,
            "DOMAIN_DISABLED",
            "Command domain is disabled",
            "enable the plugin domain before retrying",
            2,
            false,
        ),
        RuntimeError::DependencyMissing {
            domain,
            operation,
            tool,
            reason,
        } => CommandError::new(
            Some(domain),
            operation,
            "DEPENDENCY_MISSING",
            format!("Required external tool not found: {tool}"),
            reason,
            1,
            false,
        ),
        RuntimeError::ExecutionCapacityFull { capacity } => CommandError::new(
            None,
            None,
            "EXECUTION_CAPACITY_FULL",
            "MCP execution capacity is full",
            format!("all {capacity} execution slots are active or draining"),
            1,
            true,
        ),
        RuntimeError::ExecutionCancelled { request_id } => CommandError::new(
            None,
            None,
            "CANCELLED",
            "Command execution was cancelled",
            format!("request '{request_id}' was cancelled"),
            1,
            false,
        ),
        RuntimeError::ExecutionTimeout { request_id } => CommandError::new(
            None,
            None,
            "TIMEOUT",
            "Command execution timed out",
            format!("request '{request_id}' exceeded its deadline"),
            1,
            true,
        ),
        RuntimeError::ExecutorShuttingDown => CommandError::new(
            None,
            None,
            "EXECUTOR_SHUTTING_DOWN",
            "MCP executor is shutting down",
            "new execution admission is closed",
            1,
            false,
        ),
        RuntimeError::ExecutionPanic { request_id } => CommandError::new(
            None,
            None,
            "HANDLER_PANIC",
            "Command handler panicked",
            format!("request '{request_id}' ended with a handler panic"),
            1,
            false,
        ),
        other => CommandError::new(
            None,
            None,
            runtime_error_code(&other),
            "Command execution failed",
            other.to_string(),
            1,
            false,
        ),
    }
}

fn runtime_error_code(error: &RuntimeError) -> &'static str {
    match error {
        RuntimeError::LibraryLoad { .. } => "PLUGIN_LIBRARY_LOAD_FAILED",
        RuntimeError::SymbolLoad { .. } => "PLUGIN_SYMBOL_LOAD_FAILED",
        RuntimeError::AbiVersionMismatch { .. } => "PLUGIN_ABI_MISMATCH",
        RuntimeError::ApiVersionMismatch { .. } => "PLUGIN_API_MISMATCH",
        RuntimeError::InvalidMetadata { .. } => "PLUGIN_METADATA_INVALID",
        RuntimeError::Invocation(_) => "PLUGIN_INVOCATION_FAILED",
        RuntimeError::ResponseParse(_) => "PLUGIN_RESPONSE_INVALID",
        RuntimeError::InvalidCommandCatalog { .. } => "COMMAND_CATALOG_INVALID",
        RuntimeError::TypedInvocation(_) => "TYPED_INVOCATION_FAILED",
        RuntimeError::TypedResponseValidation { .. } => "OUTPUT_SCHEMA_VIOLATION",
        RuntimeError::InvalidExecutionRequest(_) => "EXECUTION_REQUEST_INVALID",
        RuntimeError::ExecutionWorker(_) => "EXECUTION_WORKER_FAILED",
        RuntimeError::ExecutorShuttingDown => "EXECUTOR_SHUTTING_DOWN",
        RuntimeError::DomainNotFound(_)
        | RuntimeError::TypedCommandNotFound(_)
        | RuntimeError::DomainDisabled(_)
        | RuntimeError::DependencyMissing { .. }
        | RuntimeError::ExecutionCapacityFull { .. }
        | RuntimeError::ExecutionCancelled { .. }
        | RuntimeError::ExecutionTimeout { .. }
        | RuntimeError::ExecutionPanic { .. } => "COMMAND_EXECUTION_FAILED",
    }
}

fn command_domain(command: &str) -> Option<String> {
    command.split_once('.').map(|(domain, _)| domain.to_owned())
}

fn execution_request_id(request_id: &NumberOrString) -> String {
    match request_id {
        NumberOrString::Number(value) => format!("mcp:n:{value}"),
        NumberOrString::String(value) => format!("mcp:s:{value}"),
    }
}

fn unknown_tool_error(name: &str) -> rmcp::ErrorData {
    rmcp::ErrorData::new(
        ErrorCode::METHOD_NOT_FOUND,
        format!("unknown MCP tool '{name}'"),
        None,
    )
}

fn internal_catalog_error(error: impl std::fmt::Display) -> rmcp::ErrorData {
    rmcp::ErrorData::internal_error(
        Cow::Owned(format!("AIHelper command catalog failed: {error}")),
        None,
    )
}

fn build_catalog_snapshot(manager: &PluginManager) -> Result<CatalogSnapshot, McpAdapterError> {
    loop {
        let runtime_revision = manager.catalog_revision();
        let commands = manager.list_enabled_commands()?;
        let mut tools = Vec::with_capacity(commands.len() + 4);
        let mut tools_by_name = HashMap::with_capacity(commands.len() + 4);
        let mut commands_by_name = HashMap::with_capacity(commands.len());
        for command in commands {
            let name = format!("{TOOL_PREFIX}{}", command.descriptor.id);
            if let Some(error) = reserved_job_namespace_error(&command.descriptor.id) {
                return Err(error);
            }
            let tool = command_to_tool(&command)?;
            tools_by_name.insert(name.clone(), tool.clone());
            commands_by_name.insert(name, command);
            tools.push(tool);
        }
        for tool in job_tools() {
            tools_by_name.insert(tool.name.to_string(), tool.clone());
            tools.push(tool);
        }
        if manager.catalog_revision() == runtime_revision {
            return Ok(CatalogSnapshot {
                runtime_revision,
                tools,
                tools_by_name,
                commands_by_name,
            });
        }
    }
}

fn reserved_job_namespace_error(command: &str) -> Option<McpAdapterError> {
    format!("{TOOL_PREFIX}{command}")
        .starts_with(JOB_TOOL_PREFIX)
        .then(|| McpAdapterError::InvalidSchema {
            command: command.to_owned(),
            reason: "the MCP namespace 'ah.job.*' is reserved for built-in job tools".to_owned(),
        })
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    };

    use ah_plugin_api::{
        AH_PLUGIN_ABI_VERSION, CommandCatalog, CommandDescriptor, CommandEffect, CommandEffects,
        ExecutionContextWire, InvocationRequest, InvocationResponse, PluginCompatibility,
        PluginManual, PluginMetadata, Reversibility, RiskLevel, TypedInvocationRequest,
        TypedInvocationResponse, plugin_capabilities,
    };
    use ah_runtime::{
        BuiltinPlugin, PluginManager, RuntimeError,
        executor::{
            ExecutionFuture, ExecutionTelemetry, ExecutionTimeoutPhase, ObservedExecution,
            ObservedExecutionFuture,
        },
    };
    use rmcp::model::{CallToolRequestParams, ErrorCode, JsonObject, NumberOrString};
    use serde_json::{Value, json};
    use tokio::io::AsyncReadExt;

    use super::{
        EventSink, Executor, JOB_START_TOOL, McpCommandEvent, McpCommandStatus, McpServer,
        McpServerConfig, RISK_META_KEY, ShutdownReader, ShutdownTracker, peer_generation_matches,
        refresh_catalog_after_job, spawn_best_effort_notification,
    };

    struct TypedPlugin;

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
                vec![CommandDescriptor::new(
                    "test.echo",
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
                )],
            ))
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
        }
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

    #[test]
    fn rejects_plugin_commands_in_reserved_job_namespace() {
        let error = super::reserved_job_namespace_error("job.extra")
            .expect("reserved job command should be rejected");
        assert!(error.to_string().contains("ah.job.*"));
        assert!(super::reserved_job_namespace_error("jobs.extra").is_none());
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
