//! The MCP server: one `McpServer` per session over shared state.
//!
//! The 833 production lines this came from also carried a 1300-line test
//! module, so the file was 2138 lines and the server itself was a fifth of it:
//!
//! | Module    | Owns                                                     |
//! |-----------|----------------------------------------------------------|
//! | `config`  | configuration, outcomes, and the wire metadata keys       |
//! | `state`   | what sessions share: peers, catalog snapshot, executions  |
//! | `catalog` | notifying peers that the tool list changed               |

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

mod catalog;
mod config;
mod state;
pub(crate) use catalog::{
    event_parameters, notify_tool_list_changed_shared, refresh_catalog_after_job,
    refresh_catalog_generation_shared,
};
pub(crate) use config::{
    DEFAULT_SHUTDOWN_GRACE, DIAGNOSTIC_META_KEY, EXECUTION_META_KEY, JOB_CANCEL_TOOL,
    JOB_RESULT_TOOL, JOB_START_TOOL, JOB_STATUS_TOOL, JOB_TOOL_PREFIX, PEER_NOTIFICATION_TIMEOUT,
    RISK_META_KEY, TOOL_PREFIX,
};
pub use config::{
    EventSink, McpAdapterError, McpCommandEvent, McpCommandStatus, McpServeOutcome, McpServerConfig,
};
pub(crate) use state::{
    ActiveExecution, CatalogSnapshot, McpShared, RegisteredPeer, ToolCallOutcome,
};
pub struct McpServer {
    pub(crate) shared: Arc<McpShared>,
    pub(crate) active_executions: Mutex<HashMap<String, Vec<String>>>,
    pub(crate) require_explicit_cwd: bool,
    pub(crate) session_id: u64,
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

#[cfg(test)]
mod tests;
