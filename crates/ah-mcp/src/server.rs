#![allow(clippy::result_large_err)]

use std::{
    borrow::Cow,
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use ah_plugin_api::{
    CommandDescriptor, CommandError, ExecutionContextWire, TypedInvocationRequest,
    TypedInvocationResponse,
};
use ah_runtime::{PluginManager, RegisteredCommand, RuntimeError, executor::Executor};
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
use serde_json::{Map, Value, json};
use thiserror::Error;

const TOOL_PREFIX: &str = "ah.";
const RISK_META_KEY: &str = "dev.aihelper/risk";
const DIAGNOSTIC_META_KEY: &str = "dev.aihelper/diagnostic";
const EXECUTION_META_KEY: &str = "dev.aihelper/execution";

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
    #[error("MCP stdio service failed: {0}")]
    Service(String),
}

pub struct McpServer {
    manager: Arc<PluginManager>,
    executor: Arc<dyn Executor>,
    config: McpServerConfig,
    catalog_snapshot: Mutex<Arc<CatalogSnapshot>>,
    catalog_generation: AtomicU64,
    active_executions: Mutex<HashMap<String, Vec<String>>>,
    next_execution_id: AtomicU64,
}

struct CatalogSnapshot {
    runtime_revision: u64,
    tools: Vec<Tool>,
    tools_by_name: HashMap<String, Tool>,
    commands_by_name: HashMap<String, RegisteredCommand>,
}

impl McpServer {
    pub fn new(
        manager: Arc<PluginManager>,
        executor: Arc<dyn Executor>,
        config: McpServerConfig,
    ) -> Result<Self, McpAdapterError> {
        let catalog_snapshot = build_catalog_snapshot(&manager)?;
        Ok(Self {
            manager,
            executor,
            config,
            catalog_snapshot: Mutex::new(Arc::new(catalog_snapshot)),
            catalog_generation: AtomicU64::new(1),
            active_executions: Mutex::new(HashMap::new()),
            next_execution_id: AtomicU64::new(1),
        })
    }

    pub fn catalog_generation(&self) -> u64 {
        self.catalog_generation.load(Ordering::Acquire)
    }

    pub fn tools(&self) -> Result<Vec<Tool>, McpAdapterError> {
        Ok(self.catalog_snapshot().tools.clone())
    }

    pub fn refresh_catalog_generation(&self) -> Result<bool, McpAdapterError> {
        let runtime_revision = self.manager.catalog_revision();
        if self.catalog_snapshot().runtime_revision == runtime_revision {
            return Ok(false);
        }
        let next = Arc::new(build_catalog_snapshot(&self.manager)?);
        let mut current = self
            .catalog_snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if current.runtime_revision >= next.runtime_revision {
            return Ok(false);
        }
        *current = next;
        self.catalog_generation.fetch_add(1, Ordering::AcqRel);
        Ok(true)
    }

    pub async fn refresh_catalog_and_notify(
        &self,
        peer: &Peer<RoleServer>,
    ) -> Result<bool, McpAdapterError> {
        let changed = self.refresh_catalog_generation()?;
        if changed {
            peer.notify_tool_list_changed()
                .await
                .map_err(|error| McpAdapterError::Service(error.to_string()))?;
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
                .catalog_snapshot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        )
    }

    async fn call_tool_inner(
        &self,
        request: CallToolRequestParams,
        request_id: String,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let command = self
            .find_command(&request.name)
            .map_err(internal_catalog_error)?
            .ok_or_else(|| unknown_tool_error(&request.name))?;
        let mut arguments = request.arguments.unwrap_or_default();
        let context = match extract_context(
            &mut arguments,
            &request_id,
            &self.config,
            &command.descriptor,
        ) {
            Ok(context) => context,
            Err(error) => return Ok(command_error_result(error)),
        };
        let request =
            TypedInvocationRequest::new(command.descriptor.id, Value::Object(arguments), context);
        match self.executor.execute(request).await {
            Ok(response) => Ok(typed_response_result(response, &request_id)),
            Err(error) => Ok(command_error_result(runtime_command_error(error))),
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
        execution_ids
            .iter()
            .fold(false, |cancelled, execution_id| {
                self.executor.cancel(execution_id) || cancelled
            })
    }

    fn begin_execution(&self, request_id: &NumberOrString) -> ActiveExecution<'_> {
        let protocol_request_id = execution_request_id(request_id);
        let sequence = self.next_execution_id.fetch_add(1, Ordering::Relaxed);
        let execution_id = format!("{protocol_request_id}:e:{sequence}");
        self.active_executions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry(protocol_request_id.clone())
            .or_default()
            .push(execution_id.clone());
        ActiveExecution {
            active_executions: &self.active_executions,
            protocol_request_id,
            execution_id,
        }
    }
}

struct ActiveExecution<'a> {
    active_executions: &'a Mutex<HashMap<String, Vec<String>>>,
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
                .with_description("Typed AIHelper commands over MCP stdio"),
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
        let execution = self.begin_execution(&context.id);
        let result = self
            .call_tool_inner(request, execution.execution_id().to_owned())
            .await;
        drop(execution);
        if self.refresh_catalog_generation().unwrap_or(false) {
            let _ = context.peer.notify_tool_list_changed().await;
        }
        result
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
}

pub async fn serve_stdio(server: McpServer) -> Result<(), McpAdapterError> {
    let service = server
        .serve(stdio())
        .await
        .map_err(|error| McpAdapterError::Service(error.to_string()))?;
    service
        .waiting()
        .await
        .map_err(|error| McpAdapterError::Service(error.to_string()))?;
    Ok(())
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
) -> Result<ExecutionContextWire, CommandError> {
    let context = arguments.remove("context");
    let Some(context) = context else {
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
        None => defaults.cwd.clone(),
    };
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
        RuntimeError::ExecutionQueueFull { capacity } => CommandError::new(
            None,
            None,
            "QUEUE_FULL",
            "MCP execution queue is full",
            format!("the bounded queue capacity is {capacity}"),
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
        RuntimeError::ExecutionDraining { request_id } => CommandError::new(
            None,
            None,
            "EXECUTOR_DRAINING",
            "Command execution is temporarily unavailable",
            format!("timed-out request '{request_id}' is still draining"),
            1,
            true,
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
        RuntimeError::ExecutionDraining { .. } => "EXECUTOR_DRAINING",
        RuntimeError::DomainNotFound(_)
        | RuntimeError::TypedCommandNotFound(_)
        | RuntimeError::DomainDisabled(_)
        | RuntimeError::DependencyMissing { .. }
        | RuntimeError::ExecutionQueueFull { .. }
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
        let mut tools = Vec::with_capacity(commands.len());
        let mut tools_by_name = HashMap::with_capacity(commands.len());
        let mut commands_by_name = HashMap::with_capacity(commands.len());
        for command in commands {
            let name = format!("{TOOL_PREFIX}{}", command.descriptor.id);
            let tool = command_to_tool(&command)?;
            tools_by_name.insert(name.clone(), tool.clone());
            commands_by_name.insert(name, command);
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

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use ah_plugin_api::{
        AH_PLUGIN_ABI_VERSION, CommandCatalog, CommandDescriptor, CommandEffect, CommandEffects,
        InvocationRequest, InvocationResponse, PluginCompatibility, PluginManual, PluginMetadata,
        Reversibility, RiskLevel, TypedInvocationRequest, TypedInvocationResponse,
        plugin_capabilities,
    };
    use ah_runtime::{BuiltinPlugin, PluginManager, RuntimeError, executor::ExecutionFuture};
    use rmcp::model::{CallToolRequestParams, ErrorCode, JsonObject};
    use serde_json::{Value, json};

    use super::{Executor, McpServer, McpServerConfig};

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
    }

    impl RecordingExecutor {
        fn success() -> Arc<Self> {
            Arc::new(Self {
                request: Mutex::new(None),
                cancelled: Mutex::new(Vec::new()),
                fail_timeout: false,
            })
        }

        fn timeout() -> Arc<Self> {
            Arc::new(Self {
                request: Mutex::new(None),
                cancelled: Mutex::new(Vec::new()),
                fail_timeout: true,
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

        fn cancel(&self, request_id: &str) -> bool {
            self.cancelled.lock().unwrap().push(request_id.to_owned());
            true
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

    fn arguments(value: Value) -> JsonObject {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn maps_descriptor_to_typed_mcp_tool() {
        let server = server(RecordingExecutor::success());
        let tools = server.tools().unwrap();
        assert_eq!(tools.len(), 1);
        let tool = &tools[0];
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
        assert_eq!(server.tools().unwrap().len(), 1);

        manager.set_disabled_domains(vec!["test".to_owned()]);
        assert!(server.refresh_catalog_generation().unwrap());
        assert!(server.tools().unwrap().is_empty());
        assert_eq!(server.catalog_generation(), 2);

        manager.set_disabled_domains(vec!["TEST".to_owned()]);
        assert!(!server.refresh_catalog_generation().unwrap());
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
        let protocol_request_id = NumberOrString::String("same".to_owned());

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

        drop(first);
        drop(second);
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
