#![allow(clippy::result_large_err)]

use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap},
    future::{Future, IntoFuture},
    net::Ipv4Addr,
    path::Path,
    pin::Pin,
    sync::{
        Arc, Mutex, OnceLock, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    task::{Context, Poll},
    time::{Duration, Instant},
};

use ah_plugin_api::{
    CommandDescriptor, CommandError, ExecutionContextWire, TypedInvocationRequest,
    TypedInvocationResponse,
};
use ah_redact::{REDACTED, curl_contains_auth, is_authorization_header, url_contains_userinfo};
use ah_runtime::{
    InvocationOutcome, PluginManager, RegisteredCommand, RunCheckOutcome, RuntimeError,
    executor::{ExecutionTelemetry, Executor},
};
use axum::{
    Form, Json, Router,
    extract::{
        Query, State,
        rejection::{FormRejection, JsonRejection, QueryRejection},
    },
    http::{
        HeaderMap, HeaderValue, StatusCode,
        header::{ACCEPT, CACHE_CONTROL, CONTENT_SECURITY_POLICY, HOST, ORIGIN, REFERRER_POLICY},
    },
    response::{Html, IntoResponse, Response},
    routing::{get, post},
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
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use thiserror::Error;
use tokio::{
    io::{AsyncRead, ReadBuf},
    sync::Notify,
};
use tokio_util::sync::CancellationToken;
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

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum SecretSetupRequest {
    Create {
        id: String,
        kind: String,
        label: Option<String>,
        description: Option<String>,
    },
    Edit {
        id: String,
        label: Option<String>,
        description: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct SecretSetupField {
    pub name: &'static str,
    pub label: &'static str,
    pub optional: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SecretSetupForm {
    pub id: String,
    pub kind: String,
    pub fields: Vec<SecretSetupField>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SecretSetupMetadata {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct SecretSetupError {
    pub code: &'static str,
    pub message: &'static str,
}

impl SecretSetupError {
    pub const fn new(code: &'static str, message: &'static str) -> Self {
        Self { code, message }
    }
}

pub trait SecretSetupService: Send + Sync {
    fn issue(&self, request: SecretSetupRequest) -> Result<String, SecretSetupError>;

    fn form(&self, capability: &str) -> Result<SecretSetupForm, SecretSetupError>;

    fn submit(
        &self,
        capability: &str,
        values: BTreeMap<String, String>,
    ) -> Result<SecretSetupMetadata, SecretSetupError>;
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
    secret_setup: Option<Arc<dyn SecretSetupService>>,
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
    lifecycle: Arc<HttpLifecycleController>,
    secret_setup: Option<Arc<dyn SecretSetupService>>,
}

impl HttpLifecycleState {
    fn new(
        version: String,
        instance_id: Uuid,
        pid: u32,
        authority: String,
        origin: String,
        lifecycle: Arc<HttpLifecycleController>,
        secret_setup: Option<Arc<dyn SecretSetupService>>,
    ) -> Self {
        Self {
            readiness: ReadinessResponse {
                status: "ready",
                version,
                pid,
                instance_id,
            },
            authority,
            origin,
            lifecycle,
            secret_setup,
        }
    }
}

#[derive(Clone, Serialize)]
struct ReadinessResponse {
    status: &'static str,
    version: String,
    pid: u32,
    instance_id: Uuid,
}

struct HttpLifecycleController {
    shutdown_started: AtomicBool,
    tracker: Arc<ShutdownTracker>,
    executor: Arc<dyn Executor>,
    cancellation: CancellationToken,
}

impl HttpLifecycleController {
    fn new(
        tracker: Arc<ShutdownTracker>,
        executor: Arc<dyn Executor>,
        cancellation: CancellationToken,
    ) -> Self {
        Self {
            shutdown_started: AtomicBool::new(false),
            tracker,
            executor,
            cancellation,
        }
    }

    fn begin_shutdown(&self) -> bool {
        if self
            .shutdown_started
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }
        self.tracker.begin();
        self.executor.close();
        self.cancellation.cancel();
        true
    }

    async fn cancelled(&self) {
        self.cancellation.cancelled().await;
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ShutdownRequest {
    instance_id: Uuid,
}

#[derive(Serialize)]
struct ShutdownAccepted {
    status: &'static str,
    instance_id: Uuid,
}

#[derive(Serialize)]
struct ControlErrorResponse {
    error: ControlError,
}

#[derive(Serialize)]
struct ControlError {
    code: &'static str,
    message: &'static str,
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
            tokio::pin!(changed);
            changed.as_mut().enable();
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
            wait_for_transport(
                async {
                    let result = service.waiting().await;
                    tracker.begin();
                    executor.close();
                    result
                        .map(|_| ())
                        .map_err(|error| McpAdapterError::Service(error.to_string()))
                },
                tracker.as_ref(),
            )
            .await
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
    serve_http_bounded_with_identity_and_listener(
        server,
        port,
        version,
        Uuid::new_v4(),
        std::process::id(),
        grace,
        || Ok(()),
    )
    .await
}

pub async fn serve_http_bounded_with_identity_and_listener<F>(
    server: McpServer,
    port: u16,
    version: impl Into<String>,
    instance_id: Uuid,
    pid: u32,
    grace: Duration,
    on_listener_bound: F,
) -> McpServeOutcome
where
    F: FnOnce() -> Result<(), McpAdapterError>,
{
    let tracker = Arc::new(ShutdownTracker::new(grace));
    let executor = Arc::clone(&server.shared.executor);
    let event_dispatcher = server.shared.event_dispatcher.clone();
    let authority = format!("127.0.0.1:{port}");
    let origin = format!("http://{authority}");
    let cancellation = CancellationToken::new();
    let lifecycle_controller = Arc::new(HttpLifecycleController::new(
        Arc::clone(&tracker),
        Arc::clone(&executor),
        cancellation.clone(),
    ));
    let lifecycle = HttpLifecycleState::new(
        version.into(),
        instance_id,
        pid,
        authority.clone(),
        origin.clone(),
        Arc::clone(&lifecycle_controller),
        server.shared.secret_setup.clone(),
    );
    let config = StreamableHttpServerConfig::default()
        .with_stateful_mode(true)
        .with_allowed_hosts([authority])
        .with_allowed_origins([origin])
        .with_cancellation_token(cancellation);
    let session_template = Arc::new(server);
    let factory_template = Arc::clone(&session_template);
    let service: StreamableHttpService<McpServer, LocalSessionManager> = StreamableHttpService::new(
        move || Ok(factory_template.http_session()),
        Arc::new(LocalSessionManager::default()),
        config,
    );
    let router = Router::new()
        .route("/health/ready", get(readiness))
        .route("/control/shutdown", post(control_shutdown))
        .route("/secrets/setup/capability", post(secret_setup_capability))
        .route(
            "/secrets/setup",
            get(secret_setup_form).post(secret_setup_submit),
        )
        .nest_service("/mcp", service)
        .with_state(lifecycle)
        .layer(RequestBodyLimitLayer::new(1024 * 1024));
    let listener = match tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await {
        Ok(listener) => listener,
        Err(error) => {
            lifecycle_controller.begin_shutdown();
            return McpServeOutcome {
                result: Err(McpAdapterError::Service(error.to_string())),
                remaining_shutdown_grace: tracker.remaining(),
            };
        }
    };
    if let Err(error) = on_listener_bound() {
        lifecycle_controller.begin_shutdown();
        return McpServeOutcome {
            result: Err(error),
            remaining_shutdown_grace: tracker.remaining(),
        };
    }
    let shutdown_controller = Arc::clone(&lifecycle_controller);
    let serving = axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            tokio::select! {
                _ = shutdown_signal() => {
                    shutdown_controller.begin_shutdown();
                }
                _ = shutdown_controller.cancelled() => {}
            }
        })
        .into_future();
    let result = wait_for_transport(
        async move {
            serving
                .await
                .map_err(|error| McpAdapterError::Service(error.to_string()))
        },
        tracker.as_ref(),
    )
    .await;
    lifecycle_controller.begin_shutdown();
    if let Some(dispatcher) = event_dispatcher {
        dispatcher.flush(tracker.remaining()).await;
    }
    McpServeOutcome {
        result,
        remaining_shutdown_grace: tracker.remaining(),
    }
}

async fn wait_for_transport<F>(
    transport: F,
    tracker: &ShutdownTracker,
) -> Result<(), McpAdapterError>
where
    F: Future<Output = Result<(), McpAdapterError>>,
{
    tokio::pin!(transport);
    tokio::select! {
        biased;
        result = &mut transport => result,
        _ = tracker.expired() => Err(McpAdapterError::ShutdownTimeout {
            grace_ms: tracker.grace.as_millis(),
        }),
    }
}

async fn readiness(
    State(state): State<HttpLifecycleState>,
    headers: HeaderMap,
) -> Result<Json<ReadinessResponse>, StatusCode> {
    validate_local_headers(&headers, &state)?;
    Ok(Json(state.readiness))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetupCapabilityQuery {
    capability: String,
}

#[derive(Serialize)]
struct SetupCapabilityResponse {
    setup_url: String,
}

async fn secret_setup_capability(
    State(state): State<HttpLifecycleState>,
    headers: HeaderMap,
    request: Result<Json<SecretSetupRequest>, JsonRejection>,
) -> Response {
    if validate_local_headers(&headers, &state).is_err() {
        return control_error(
            StatusCode::FORBIDDEN,
            "LOCAL_REQUEST_REJECTED",
            "request does not match the local HTTP policy",
        );
    }
    let Some(service) = state.secret_setup else {
        return control_error(
            StatusCode::NOT_FOUND,
            "VAULT_SETUP_UNAVAILABLE",
            "secret setup is unavailable",
        );
    };
    let Ok(Json(request)) = request else {
        return control_error(
            StatusCode::BAD_REQUEST,
            "VAULT_SETUP_REQUEST_INVALID",
            "secret setup request is invalid",
        );
    };
    match service.issue(request) {
        Ok(capability) => no_store(
            Json(SetupCapabilityResponse {
                setup_url: format!(
                    "http://{}/secrets/setup?capability={capability}",
                    state.authority
                ),
            })
            .into_response(),
        ),
        Err(error) => setup_error(error),
    }
}

async fn secret_setup_form(
    State(state): State<HttpLifecycleState>,
    headers: HeaderMap,
    query: Result<Query<SetupCapabilityQuery>, QueryRejection>,
) -> Response {
    if validate_local_headers(&headers, &state).is_err() {
        return control_error(
            StatusCode::FORBIDDEN,
            "LOCAL_REQUEST_REJECTED",
            "request does not match the local HTTP policy",
        );
    }
    let Some(service) = state.secret_setup else {
        return control_error(
            StatusCode::NOT_FOUND,
            "VAULT_SETUP_UNAVAILABLE",
            "secret setup is unavailable",
        );
    };
    let Ok(Query(query)) = query else {
        return setup_error(SecretSetupError::new(
            "VAULT_SETUP_CAPABILITY_INVALID",
            "secret setup capability is invalid, expired, or already used",
        ));
    };
    match service.form(&query.capability) {
        Ok(form) => {
            let nonce = page_nonce();
            no_store_form(
                Html(render_secret_setup_form(&form, &nonce)).into_response(),
                &nonce,
            )
        }
        Err(error) => setup_error(error),
    }
}

async fn secret_setup_submit(
    State(state): State<HttpLifecycleState>,
    headers: HeaderMap,
    query: Result<Query<SetupCapabilityQuery>, QueryRejection>,
    form: Result<Form<BTreeMap<String, String>>, FormRejection>,
) -> Response {
    if validate_local_headers(&headers, &state).is_err() {
        return control_error(
            StatusCode::FORBIDDEN,
            "LOCAL_REQUEST_REJECTED",
            "request does not match the local HTTP policy",
        );
    }
    let Some(service) = state.secret_setup else {
        return control_error(
            StatusCode::NOT_FOUND,
            "VAULT_SETUP_UNAVAILABLE",
            "secret setup is unavailable",
        );
    };
    let Ok(Query(query)) = query else {
        return setup_error(SecretSetupError::new(
            "VAULT_SETUP_CAPABILITY_INVALID",
            "secret setup capability is invalid, expired, or already used",
        ));
    };
    let Ok(Form(values)) = form else {
        return control_error(
            StatusCode::BAD_REQUEST,
            "VAULT_SETUP_SUBMISSION_INVALID",
            "secret setup form is invalid",
        );
    };
    match service.submit(&query.capability, values) {
        Ok(metadata) if wants_html(&headers) => {
            let nonce = page_nonce();
            no_store_form(
                Html(render_secret_setup_success(&metadata, &nonce)).into_response(),
                &nonce,
            )
        }
        Ok(metadata) => no_store(Json(metadata).into_response()),
        Err(error) => setup_error(error),
    }
}

fn setup_error(error: SecretSetupError) -> Response {
    let status = if error.code == "VAULT_SETUP_CAPABILITY_INVALID" {
        StatusCode::FORBIDDEN
    } else {
        StatusCode::BAD_REQUEST
    };
    control_error(status, error.code, error.message)
}

fn no_store(response: Response) -> Response {
    no_store_with_referrer_policy(response, "no-referrer")
}

/// The setup pages are served with `same-origin` rather than `no-referrer`: under
/// `no-referrer` a browser sends `Origin: null` on the form POST, which the local
/// HTTP policy rejects. The page loads no third-party resources, so `same-origin`
/// keeps the capability out of every referrer that leaves this server.
///
/// The nonce-based policy pins the page to its own inline style and script and
/// blocks every outbound load, so nothing on a secret entry page can reach out.
fn no_store_form(mut response: Response, nonce: &str) -> Response {
    let policy = format!(
        "default-src 'none'; style-src 'nonce-{nonce}'; script-src 'nonce-{nonce}'; form-action 'self'; base-uri 'none'"
    );
    if let Ok(value) = HeaderValue::from_str(&policy) {
        response
            .headers_mut()
            .insert(CONTENT_SECURITY_POLICY, value);
    }
    no_store_with_referrer_policy(response, "same-origin")
}

/// Single-use nonce so the inline style and script survive the page's own CSP.
fn page_nonce() -> String {
    Uuid::new_v4().simple().to_string()
}

fn no_store_with_referrer_policy(mut response: Response, policy: &'static str) -> Response {
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
        .headers_mut()
        .insert(REFERRER_POLICY, HeaderValue::from_static(policy));
    response
}

const SETUP_PAGE_STYLE: &str = "\
*,::before,::after{box-sizing:border-box}\
:root{color-scheme:light dark;\
--bg:#f4f5f7;--card:#fff;--ink:#1b1f24;--muted:#5b6570;--line:#d8dde3;\
--field:#fff;--accent:#2b6cb0;--accent-ink:#fff;--ok:#1f7a4d;--ok-bg:#e8f5ee}\
@media (prefers-color-scheme:dark){:root{\
--bg:#15181d;--card:#1e232a;--ink:#e8ecf1;--muted:#9aa5b1;--line:#333b45;\
--field:#161a20;--accent:#4a90d9;--accent-ink:#0f1216;--ok:#63d19b;--ok-bg:#163024}}\
body{margin:0;min-height:100vh;display:flex;align-items:center;justify-content:center;\
padding:24px;background:var(--bg);color:var(--ink);\
font:15px/1.5 system-ui,-apple-system,Segoe UI,Roboto,sans-serif}\
main{width:100%;max-width:26rem;background:var(--card);border:1px solid var(--line);\
border-radius:12px;padding:28px}\
h1{margin:0 0 4px;font-size:1.25rem;letter-spacing:-.01em}\
.kind{display:inline-block;margin-bottom:20px;padding:2px 8px;border-radius:999px;\
background:var(--bg);border:1px solid var(--line);color:var(--muted);\
font-size:.75rem;font-family:ui-monospace,SFMono-Regular,Consolas,monospace}\
label{display:block;margin-bottom:16px;font-size:.8125rem;font-weight:600;color:var(--muted)}\
input,textarea{display:block;width:100%;margin-top:6px;padding:9px 11px;\
border:1px solid var(--line);border-radius:8px;background:var(--field);color:var(--ink);\
font:inherit}\
textarea{min-height:8rem;resize:vertical;font-family:ui-monospace,SFMono-Regular,Consolas,monospace;\
font-size:.8125rem}\
input:focus,textarea:focus{outline:2px solid var(--accent);outline-offset:1px;border-color:transparent}\
.optional{font-weight:400;text-transform:none}\
button{width:100%;padding:10px 16px;border:0;border-radius:8px;\
background:var(--accent);color:var(--accent-ink);font:inherit;font-weight:600;cursor:pointer}\
button:hover{filter:brightness(1.08)}\
button:focus-visible{outline:2px solid var(--ink);outline-offset:2px}\
.done{display:flex;align-items:center;gap:10px;margin-bottom:16px;padding:10px 12px;\
border-radius:8px;background:var(--ok-bg);color:var(--ok);font-weight:600}\
.done svg{flex:none}\
dl{margin:0 0 20px;display:grid;grid-template-columns:auto 1fr;gap:6px 16px;font-size:.875rem}\
dt{color:var(--muted)}\
dd{margin:0;font-family:ui-monospace,SFMono-Regular,Consolas,monospace;word-break:break-all}\
.hint{margin:14px 0 0;font-size:.8125rem;color:var(--muted);text-align:center}\
[hidden]{display:none}";

fn setup_page(nonce: &str, title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
<meta name=\"referrer\" content=\"same-origin\"><title>{}</title>\
<style nonce=\"{}\">{SETUP_PAGE_STYLE}</style></head><body><main>{body}</main></body></html>",
        html_escape(title),
        html_escape(nonce),
    )
}

fn render_secret_setup_form(form: &SecretSetupForm, nonce: &str) -> String {
    let fields = form
        .fields
        .iter()
        .map(|field| {
            let required = if field.optional { "" } else { " required" };
            let label = if field.optional {
                format!(
                    "{} <span class=\"optional\">(optional)</span>",
                    html_escape(field.label)
                )
            } else {
                html_escape(field.label)
            };
            if field.name == "private_key" {
                format!(
                    "<label>{label}<textarea name=\"{}\" autocomplete=\"off\" spellcheck=\"false\"{required}></textarea></label>",
                    html_escape(field.name),
                )
            } else {
                format!(
                    "<label>{label}<input type=\"password\" name=\"{}\" autocomplete=\"new-password\"{required}></label>",
                    html_escape(field.name),
                )
            }
        })
        .collect::<String>();
    setup_page(
        nonce,
        "AIHelper secret setup",
        &format!(
            "<h1>Set up {}</h1><span class=\"kind\">{}</span>\
<form method=\"post\">{fields}<button type=\"submit\">Save</button></form>",
            html_escape(&form.id),
            html_escape(&form.kind),
        ),
    )
}

fn render_secret_setup_success(metadata: &SecretSetupMetadata, nonce: &str) -> String {
    let description = metadata
        .description
        .as_deref()
        .map_or_else(String::new, |description| {
            format!("<dt>Description</dt><dd>{}</dd>", html_escape(description))
        });
    setup_page(
        nonce,
        "AIHelper secret saved",
        &format!(
            "<p class=\"done\">\
<svg width=\"18\" height=\"18\" viewBox=\"0 0 18 18\" fill=\"none\" stroke=\"currentColor\" \
stroke-width=\"2.2\" stroke-linecap=\"round\" stroke-linejoin=\"round\" aria-hidden=\"true\">\
<path d=\"M3.5 9.5l3.5 3.5 7.5-8\"/></svg>Secret saved</p>\
<h1>{}</h1><span class=\"kind\">{}</span>\
<dl><dt>Label</dt><dd>{}</dd>{description}</dl>\
<button type=\"button\" id=\"close\">Close</button>\
<p class=\"hint\" id=\"hint\" hidden>This tab can be closed now.</p>\
<script nonce=\"{}\">\
document.getElementById('close').addEventListener('click',function(){{\
window.close();\
document.getElementById('hint').hidden=false;\
}});\
</script>",
            html_escape(&metadata.id),
            html_escape(&metadata.kind),
            html_escape(&metadata.label),
            html_escape(nonce),
        ),
    )
}

/// Browsers submitting the form get the confirmation page; every other caller
/// keeps the documented redacted-metadata JSON.
fn wants_html(headers: &HeaderMap) -> bool {
    headers
        .get(ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|accept| accept.contains("text/html"))
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

async fn control_shutdown(
    State(state): State<HttpLifecycleState>,
    headers: HeaderMap,
    request: Result<Json<ShutdownRequest>, JsonRejection>,
) -> Response {
    if validate_local_headers(&headers, &state).is_err() {
        return control_error(
            StatusCode::FORBIDDEN,
            "LOCAL_REQUEST_REJECTED",
            "request does not match the local HTTP policy",
        );
    }

    let request = match request {
        Ok(Json(request)) => request,
        Err(rejection) if rejection.status() == StatusCode::UNSUPPORTED_MEDIA_TYPE => {
            return control_error(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "UNSUPPORTED_MEDIA_TYPE",
                "request content type must be application/json",
            );
        }
        Err(_) => {
            return control_error(
                StatusCode::BAD_REQUEST,
                "INVALID_SHUTDOWN_REQUEST",
                "request body must contain exactly one valid instance_id",
            );
        }
    };

    if request.instance_id != state.readiness.instance_id {
        return control_error(
            StatusCode::CONFLICT,
            "INSTANCE_ID_MISMATCH",
            "instance_id does not match the running AIHelper instance",
        );
    }

    state.lifecycle.begin_shutdown();
    (
        StatusCode::ACCEPTED,
        Json(ShutdownAccepted {
            status: "shutting_down",
            instance_id: request.instance_id,
        }),
    )
        .into_response()
}

fn control_error(status: StatusCode, code: &'static str, message: &'static str) -> Response {
    (
        status,
        Json(ControlErrorResponse {
            error: ControlError { code, message },
        }),
    )
        .into_response()
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
    redact_mcp_plaintext_auth(&mut parameters);
    Value::Object(parameters)
}

fn validate_mcp_plaintext_auth(command: &str, arguments: &JsonObject) -> Result<(), CommandError> {
    if let Some(domain) = forge_token_domain(command) {
        return if arguments.contains_key("token") {
            Err(mcp_plaintext_token_error(domain))
        } else {
            Ok(())
        };
    }
    if !command.starts_with("http.") {
        return Ok(());
    }
    if arguments.contains_key("bearer") || arguments.contains_key("basic") {
        return Err(mcp_plaintext_auth_error());
    }
    if arguments
        .get("headers")
        .and_then(Value::as_array)
        .is_some_and(|headers| {
            headers
                .iter()
                .filter_map(Value::as_str)
                .any(is_authorization_header)
        })
    {
        return Err(mcp_plaintext_auth_error());
    }
    if arguments
        .get("url")
        .and_then(Value::as_str)
        .is_some_and(url_contains_userinfo)
    {
        return Err(mcp_plaintext_auth_error());
    }
    if arguments
        .get("curl")
        .and_then(Value::as_str)
        .is_some_and(curl_contains_auth)
    {
        return Err(mcp_plaintext_auth_error());
    }
    Ok(())
}

/// GitHub and GitLab carry one opaque token argument; over MCP it must come from
/// the vault instead of riding along in the tool call.
fn forge_token_domain(command: &str) -> Option<&'static str> {
    if command.starts_with("github.") {
        Some("github")
    } else if command.starts_with("gitlab.") {
        Some("gitlab")
    } else {
        None
    }
}

fn mcp_plaintext_token_error(domain: &'static str) -> CommandError {
    CommandError::new(
        Some(domain.to_owned()),
        None,
        "INVALID_ARGUMENT",
        "Inline API tokens are not accepted over MCP",
        format!(
            "Store the token in the AH vault and pass its id through credentials.token, or let AIHelper use the host-bound {domain} environment variables"
        ),
        2,
        false,
    )
}

fn mcp_plaintext_auth_error() -> CommandError {
    CommandError::new(
        Some("http".to_owned()),
        None,
        "INVALID_ARGUMENT",
        "Inline HTTP credentials are not accepted over MCP",
        "Store the credential in the AH vault and pass its id through credentials.basic",
        2,
        false,
    )
}

fn redact_mcp_plaintext_auth(arguments: &mut JsonObject) {
    for name in ["bearer", "basic", "token"] {
        if arguments.contains_key(name) {
            arguments.insert(name.to_owned(), Value::String(REDACTED.to_owned()));
        }
    }
    if let Some(Value::Array(headers)) = arguments.get_mut("headers") {
        for header in headers {
            if header.as_str().is_some_and(is_authorization_header) {
                *header = Value::String(format!("Authorization: {REDACTED}"));
            }
        }
    }
    if let Some(Value::String(url)) = arguments.get_mut("url")
        && url_contains_userinfo(url)
    {
        *url = REDACTED.to_owned();
    }
    if let Some(Value::String(curl)) = arguments.get_mut("curl")
        && curl_contains_auth(curl)
    {
        *curl = REDACTED.to_owned();
    }
    if let Some(Value::Object(nested)) = arguments.get_mut("arguments") {
        redact_mcp_plaintext_auth(nested);
    }
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
    let examples = descriptor
        .examples
        .iter()
        .map(|example| format!("- {}: {}", example.description, example.arguments))
        .collect::<Vec<_>>()
        .join("\n");
    let example_section = (!examples.is_empty()).then(|| format!("\n\nExamples:\n{examples}"));
    let credential_section = (!descriptor.secret_slots.is_empty()).then(|| {
        let slots = descriptor
            .secret_slots
            .iter()
            .map(|slot| {
                let kinds = slot.accepted_kinds.join(", ");
                let filters = slot
                    .accepted_kinds
                    .iter()
                    .map(|kind| format!("kind={kind}"))
                    .collect::<Vec<_>>()
                    .join(" or ");
                format!(
                    "{} accepts {kinds}. If the ID is unknown, call secrets.list with {filters}.",
                    slot.name
                )
            })
            .collect::<Vec<_>>()
            .join(" ");
        format!("\n\nCredential slots: {slots}")
    });
    let description = format!(
        "{}{}\n\nFor project-relative paths, include context.cwd. HTTP job targets always require an absolute context.cwd.{}\n\nImpact: {}\nRisk: {risk_label}.",
        descriptor.description,
        credential_section.unwrap_or_default(),
        example_section.unwrap_or_default(),
        descriptor.effects.impact
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

fn requires_explicit_cwd(
    descriptor: &CommandDescriptor,
    arguments: &JsonObject,
    http_transport: bool,
) -> bool {
    if !http_transport {
        return false;
    }

    match descriptor.id.as_str() {
        "ai.info" | "plugins.list" | "plugins.enable" | "plugins.disable" | "plugins.reset" => {
            false
        }
        command if command.starts_with("ollama.") => false,
        command if command.starts_with("postgres.") => {
            has_relative_path(arguments, "tool_path")
                || (command == "postgres.tool.use" && has_relative_path(arguments, "path"))
        }
        "http.assert" | "http.run" => true,
        command if command.starts_with("http.") => {
            has_relative_path(arguments, "json_file") || has_relative_path(arguments, "body_file")
        }
        // A hosted API call reads nothing from disk once the caller names the
        // project itself, and its credential lookup is bound to the API host
        // rather than to a git remote.
        command if command.starts_with("github.") => {
            !has_text(arguments, "repo") || has_relative_input_file(arguments)
        }
        command if command.starts_with("gitlab.") => {
            !has_text(arguments, "project") || has_relative_input_file(arguments)
        }
        _ => true,
    }
}

/// The file-backed inputs a hosted API call can carry, all of them resolved
/// against the working directory.
fn has_relative_input_file(arguments: &JsonObject) -> bool {
    [
        "body_file",
        "comment_file",
        "description_file",
        "notes_file",
    ]
    .into_iter()
    .any(|field| has_relative_path(arguments, field))
}

fn has_text(arguments: &JsonObject, field: &str) -> bool {
    arguments
        .get(field)
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
}

fn has_relative_path(arguments: &JsonObject, field: &str) -> bool {
    arguments
        .get(field)
        .and_then(Value::as_str)
        .is_some_and(|path| !Path::new(path).is_absolute())
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

fn command_event_run_check_outcome(
    result: &Result<CallToolResult, rmcp::ErrorData>,
    canonical_command: Option<&str>,
) -> Option<InvocationOutcome> {
    let result = result.as_ref().ok()?;
    if result.is_error == Some(true) {
        return None;
    }
    run_check_outcome(canonical_command?, result.structured_content.as_ref())
}

pub(crate) fn run_check_outcome(
    canonical_command: &str,
    data: Option<&Value>,
) -> Option<InvocationOutcome> {
    if canonical_command != "run.check" {
        return None;
    }
    let data = data?;
    let success = data.get("success")?.as_bool()?;
    let timed_out = data.get("timed_out")?.as_bool()?;
    let exit_code = match data.get("exit_code")? {
        Value::Null => None,
        value => Some(i32::try_from(value.as_i64()?).ok()?),
    };
    Some(InvocationOutcome::RunCheck(RunCheckOutcome {
        success,
        timed_out,
        exit_code,
    }))
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

    use super::{
        EventSink, Executor, HttpLifecycleController, HttpLifecycleState, JOB_START_TOOL,
        McpAdapterError, McpCommandEvent, McpCommandStatus, McpServer, McpServerConfig, REDACTED,
        RISK_META_KEY, SecretSetupField, SecretSetupForm, SecretSetupMetadata, ShutdownReader,
        ShutdownTracker, event_parameters, extract_context, peer_generation_matches,
        redact_mcp_plaintext_auth, refresh_catalog_after_job, render_secret_setup_form,
        render_secret_setup_success, requires_explicit_cwd, run_check_outcome,
        spawn_best_effort_notification, validate_mcp_plaintext_auth, wait_for_transport,
        wants_html,
    };
    use axum::http::{HeaderMap, HeaderValue, header::ACCEPT};

    #[test]
    fn private_key_setup_field_uses_a_multiline_textarea() {
        let html = render_secret_setup_form(&ssh_key_form(), "test-nonce");

        assert!(html.contains("<textarea name=\"private_key\""));
        assert!(!html.contains("type=\"password\" name=\"private_key\""));
        assert!(html.contains("<input type=\"password\" name=\"passphrase\""));
        assert!(html.contains("(optional)"));
    }

    #[test]
    fn setup_pages_carry_the_nonce_on_every_inline_block() {
        let form = render_secret_setup_form(&ssh_key_form(), "form-nonce");
        let success = render_secret_setup_success(
            &SecretSetupMetadata {
                id: "deployment-key".to_owned(),
                kind: "ssh-key".to_owned(),
                label: "Deployment key".to_owned(),
                description: None,
            },
            "success-nonce",
        );

        assert!(form.contains("<style nonce=\"form-nonce\">"));
        assert!(!form.contains("<script"));
        assert!(success.contains("<style nonce=\"success-nonce\">"));
        assert!(success.contains("<script nonce=\"success-nonce\">"));
    }

    #[test]
    fn success_page_confirms_the_secret_and_offers_a_close_button() {
        let html = render_secret_setup_success(
            &SecretSetupMetadata {
                id: "deployment-key".to_owned(),
                kind: "ssh-key".to_owned(),
                label: "Deployment <key>".to_owned(),
                description: Some("Release runner".to_owned()),
            },
            "test-nonce",
        );

        assert!(html.contains("Secret saved"));
        assert!(html.contains("id=\"close\""));
        assert!(html.contains("window.close()"));
        assert!(html.contains("Release runner"));
        // Metadata is escaped, and the page never offers a way back to the form.
        assert!(html.contains("Deployment &lt;key&gt;"));
        assert!(!html.contains("<form"));
    }

    #[test]
    fn only_browsers_receive_the_confirmation_page() {
        let mut html_headers = HeaderMap::new();
        html_headers.insert(
            ACCEPT,
            HeaderValue::from_static("text/html,application/xhtml+xml"),
        );
        let mut json_headers = HeaderMap::new();
        json_headers.insert(ACCEPT, HeaderValue::from_static("application/json"));

        assert!(wants_html(&html_headers));
        assert!(!wants_html(&json_headers));
        assert!(!wants_html(&HeaderMap::new()));
    }

    fn ssh_key_form() -> SecretSetupForm {
        SecretSetupForm {
            id: "deployment-key".to_owned(),
            kind: "ssh-key".to_owned(),
            fields: vec![
                SecretSetupField {
                    name: "private_key",
                    label: "SSH private key",
                    optional: false,
                },
                SecretSetupField {
                    name: "passphrase",
                    label: "SSH key passphrase",
                    optional: true,
                },
            ],
        }
    }

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
