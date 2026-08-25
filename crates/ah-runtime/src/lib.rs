use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap, HashSet},
    ffi::{CString, c_char},
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, RwLock,
        atomic::{AtomicU64, Ordering},
    },
};

use ah_plugin_api::{
    AH_PLUGIN_ABI_VERSION, AH_PLUGIN_API_MAJOR_VERSION, AH_PLUGIN_API_MINOR_VERSION,
    AH_PLUGIN_CANCEL_COMMAND_V1_SYMBOL, AH_PLUGIN_COMMAND_CATALOG_JSON_V1_SYMBOL,
    AH_PLUGIN_ENTRY_V1_SYMBOL, AH_PLUGIN_INVOKE_COMMAND_JSON_V1_SYMBOL,
    AH_PLUGIN_MANUAL_JSON_V1_SYMBOL, AH_PLUGIN_METADATA_JSON_V1_SYMBOL, AhPluginCancelCommandV1,
    AhPluginCommandCatalogJsonV1, AhPluginEntryV1, AhPluginInvokeCommandJsonV1,
    AhPluginManualJsonV1, AhPluginMetadataJsonV1, CommandCatalog, CommandDescriptor, CommandError,
    ErrorDiagnostic, GlobalOptionsWire, InvocationRequest, InvocationResponse, PluginManual,
    PluginMetadata, RequiredTool, ResolvedSecret, SecretSlot, TypedInvocationRequest,
    TypedInvocationResponse, c_ptr_to_string, plugin_capabilities,
};
use libloading::Library;
use thiserror::Error;

pub mod core;
pub mod executor;
pub mod typed;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("plugin not found for domain '{0}'")]
    DomainNotFound(String),
    #[error("failed to load plugin library at {path:?}: {source}")]
    LibraryLoad {
        path: PathBuf,
        source: libloading::Error,
    },
    #[error("failed to load plugin entrypoint from {path:?}: {source}")]
    SymbolLoad {
        path: PathBuf,
        source: libloading::Error,
    },
    #[error("plugin at {path:?} has incompatible abi version {found}, expected {expected}")]
    AbiVersionMismatch {
        path: PathBuf,
        found: u32,
        expected: u32,
    },
    #[error(
        "plugin at {path:?} requires unsupported plugin api version {found_major}.{found_minor}, host supports {supported_major}.{supported_minor}"
    )]
    ApiVersionMismatch {
        path: PathBuf,
        found_major: u16,
        found_minor: u16,
        supported_major: u16,
        supported_minor: u16,
    },
    #[error("plugin at {path:?} returned invalid metadata: {reason}")]
    InvalidMetadata { path: PathBuf, reason: String },
    #[error("plugin invocation failed: {0}")]
    Invocation(String),
    #[error("plugin response parse failed: {0}")]
    ResponseParse(String),
    #[error("invalid typed command catalog for domain '{domain}': {reason}")]
    InvalidCommandCatalog { domain: String, reason: String },
    #[error("typed command not found: {0}")]
    TypedCommandNotFound(String),
    #[error("typed command invocation failed: {0}")]
    TypedInvocation(String),
    #[error("required credential slot '{slot}' is missing for '{command}'")]
    SecretRequired { command: String, slot: String },
    #[error("credential '{id}' for slot '{slot}' was not found for '{command}'")]
    SecretNotFound {
        command: String,
        slot: String,
        id: String,
    },
    #[error(
        "credential '{id}' for slot '{slot}' has kind '{kind}', expected one of {accepted_kinds:?} for '{command}'"
    )]
    SecretKindMismatch {
        command: String,
        slot: String,
        id: String,
        kind: String,
        accepted_kinds: Vec<String>,
    },
    #[error("vault is locked while resolving credential '{id}' for slot '{slot}' in '{command}'")]
    VaultLocked {
        command: String,
        slot: String,
        id: String,
    },
    #[error(
        "vault key is unavailable while resolving credential '{id}' for slot '{slot}' in '{command}'"
    )]
    VaultKeyUnavailable {
        command: String,
        slot: String,
        id: String,
    },
    #[error("typed command response failed validation for '{command}': {reason}")]
    TypedResponseValidation { command: String, reason: String },
    #[error("typed execution request is invalid: {0}")]
    InvalidExecutionRequest(String),
    #[error("typed execution capacity is full (maximum active {capacity})")]
    ExecutionCapacityFull { capacity: usize },
    #[error("typed execution request '{request_id}' was cancelled")]
    ExecutionCancelled { request_id: String },
    #[error("typed execution request '{request_id}' timed out")]
    ExecutionTimeout { request_id: String },
    #[error("typed executor is shutting down")]
    ExecutorShuttingDown,
    #[error("typed execution worker failed: {0}")]
    ExecutionWorker(String),
    #[error("typed execution handler panicked for request '{request_id}'")]
    ExecutionPanic { request_id: String },
    #[error("plugin domain '{0}' is disabled")]
    DomainDisabled(String),
    #[error("required external tool '{tool}' is not available for domain '{domain}'")]
    DependencyMissing {
        domain: String,
        operation: Option<String>,
        tool: String,
        reason: String,
    },
}

/// Per-variant error facts, produced by the single exhaustive match over
/// `RuntimeError` in [`RuntimeError::describe`].
///
/// `diagnostic` is the diagnostic currency every surface consumes. The
/// `agent_*` fields record the handful of places where the frozen MCP wire
/// contract disagrees with the host contract, and `retryable` is meaningful
/// only to MCP clients — the CLI never retries a failed command.
struct RuntimeDiagnostic {
    diagnostic: ErrorDiagnostic,
    agent_code: Option<&'static str>,
    agent_exit_code_hint: i32,
    retryable: bool,
}

impl RuntimeDiagnostic {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        let message = message.into();
        Self {
            diagnostic: ErrorDiagnostic::new(None, None, code, message.clone(), message, 1),
            agent_code: None,
            agent_exit_code_hint: 1,
            retryable: false,
        }
    }

    fn cause(mut self, cause: impl Into<String>) -> Self {
        self.diagnostic.cause = cause.into();
        self
    }

    fn domain(mut self, domain: Option<String>) -> Self {
        self.diagnostic.domain = domain;
        self
    }

    fn operation(mut self, operation: Option<String>) -> Self {
        self.diagnostic.operation = operation;
        self
    }

    fn agent_code(mut self, code: &'static str) -> Self {
        self.agent_code = Some(code);
        self
    }

    fn agent_exit_code_hint(mut self, hint: i32) -> Self {
        self.agent_exit_code_hint = hint;
        self
    }

    fn retryable(mut self) -> Self {
        self.retryable = true;
        self
    }
}

fn command_domain(command: &str) -> Option<String> {
    command.split_once('.').map(|(domain, _)| domain.to_owned())
}

impl RuntimeError {
    /// The one translation table from `RuntimeError` to the diagnostic
    /// currency. Every surface — CLI, MCP, event log — projects from here, so
    /// adding a variant breaks exactly this match.
    ///
    /// Credential identifiers, credential kinds and accepted-kind lists are
    /// deliberately dropped from the secret and vault arms: `Display` may name
    /// them for local debugging, but a diagnostic is rendered to users, written
    /// to the event log and handed to MCP clients, so it never carries them.
    fn describe(&self) -> RuntimeDiagnostic {
        match self {
            Self::DomainNotFound(domain) => RuntimeDiagnostic::new(
                "DOMAIN_NOT_FOUND",
                format!("unknown command domain: {domain}"),
            )
            .domain(Some(domain.clone()))
            .agent_exit_code_hint(2),
            Self::LibraryLoad { path, source } => RuntimeDiagnostic::new(
                "PLUGIN_LIBRARY_LOAD_FAILED",
                format!(
                    "failed to load plugin library '{}': {source}",
                    path.display()
                ),
            ),
            Self::SymbolLoad { path, source } => RuntimeDiagnostic::new(
                "PLUGIN_SYMBOL_LOAD_FAILED",
                format!(
                    "failed to load plugin entrypoint '{}': {source}",
                    path.display()
                ),
            ),
            Self::AbiVersionMismatch {
                path,
                found,
                expected,
            } => RuntimeDiagnostic::new(
                "PLUGIN_ABI_MISMATCH",
                format!(
                    "plugin '{}' has incompatible ABI version {found}; expected {expected}",
                    path.display()
                ),
            ),
            Self::ApiVersionMismatch {
                path,
                found_major,
                found_minor,
                supported_major,
                supported_minor,
            } => RuntimeDiagnostic::new(
                "PLUGIN_API_MISMATCH",
                format!(
                    "plugin '{}' requires unsupported Plugin API version {found_major}.{found_minor}; host supports {supported_major}.{supported_minor}",
                    path.display()
                ),
            ),
            Self::InvalidMetadata { path, reason } => RuntimeDiagnostic::new(
                "PLUGIN_METADATA_INVALID",
                format!(
                    "plugin '{}' returned invalid metadata: {reason}",
                    path.display()
                ),
            ),
            Self::Invocation(message) => {
                RuntimeDiagnostic::new("PLUGIN_INVOCATION_FAILED", message.clone())
            }
            Self::ResponseParse(message) => {
                RuntimeDiagnostic::new("PLUGIN_RESPONSE_PARSE_FAILED", message.clone())
                    .agent_code("PLUGIN_RESPONSE_INVALID")
            }
            Self::InvalidCommandCatalog { domain, reason } => RuntimeDiagnostic::new(
                "COMMAND_CATALOG_INVALID",
                format!("invalid typed command catalog for domain '{domain}': {reason}"),
            ),
            Self::TypedCommandNotFound(command) => RuntimeDiagnostic::new(
                "TYPED_COMMAND_NOT_FOUND",
                format!("typed command not found: {command}"),
            )
            .domain(command_domain(command))
            .operation(Some(command.clone()))
            .agent_code("COMMAND_NOT_FOUND")
            .agent_exit_code_hint(2)
            .retryable(),
            Self::TypedInvocation(message) => {
                RuntimeDiagnostic::new("TYPED_INVOCATION_FAILED", message.clone())
            }
            Self::SecretRequired { command, slot } => RuntimeDiagnostic::new(
                "SECRET_REQUIRED",
                format!("required credential slot '{slot}' is missing for '{command}'"),
            ),
            Self::SecretNotFound { command, slot, .. } => RuntimeDiagnostic::new(
                "SECRET_NOT_FOUND",
                format!("credential for slot '{slot}' was not found for '{command}'"),
            ),
            Self::SecretKindMismatch { .. } => RuntimeDiagnostic::new(
                "SECRET_KIND_MISMATCH",
                "credential kind does not match the required credential slot",
            ),
            Self::VaultLocked { command, slot, .. } => RuntimeDiagnostic::new(
                "VAULT_LOCKED",
                format!(
                    "vault is locked while resolving credential for slot '{slot}' in '{command}'"
                ),
            ),
            Self::VaultKeyUnavailable { command, slot, .. } => RuntimeDiagnostic::new(
                "VAULT_KEY_UNAVAILABLE",
                format!(
                    "vault key is unavailable while resolving credential for slot '{slot}' in '{command}'"
                ),
            ),
            Self::TypedResponseValidation { command, reason } => RuntimeDiagnostic::new(
                "OUTPUT_SCHEMA_VIOLATION",
                format!("typed command response failed validation for '{command}': {reason}"),
            ),
            Self::InvalidExecutionRequest(message) => {
                RuntimeDiagnostic::new("EXECUTION_REQUEST_INVALID", message.clone())
            }
            Self::ExecutionCapacityFull { capacity } => RuntimeDiagnostic::new(
                "EXECUTION_CAPACITY_FULL",
                format!("typed execution capacity is full (maximum active {capacity})"),
            )
            .retryable(),
            Self::ExecutionCancelled { request_id } => RuntimeDiagnostic::new(
                "EXECUTION_CANCELLED",
                format!("typed execution request '{request_id}' was cancelled"),
            )
            .agent_code("CANCELLED"),
            Self::ExecutionTimeout { request_id } => RuntimeDiagnostic::new(
                "EXECUTION_TIMEOUT",
                format!("typed execution request '{request_id}' timed out"),
            )
            .agent_code("TIMEOUT")
            .retryable(),
            Self::ExecutorShuttingDown => {
                RuntimeDiagnostic::new("EXECUTOR_SHUTTING_DOWN", "typed executor is shutting down")
            }
            Self::ExecutionWorker(message) => {
                RuntimeDiagnostic::new("EXECUTION_WORKER_FAILED", message.clone())
            }
            Self::ExecutionPanic { request_id } => RuntimeDiagnostic::new(
                "EXECUTION_HANDLER_PANIC",
                format!("typed execution handler panicked for request '{request_id}'"),
            )
            .agent_code("HANDLER_PANIC"),
            Self::DomainDisabled(domain) => RuntimeDiagnostic::new(
                "DOMAIN_DISABLED",
                format!("plugin domain is disabled: {domain}"),
            )
            .domain(Some(domain.clone()))
            .agent_exit_code_hint(2),
            Self::DependencyMissing {
                domain,
                operation,
                tool,
                reason,
            } => RuntimeDiagnostic::new(
                "DEPENDENCY_MISSING",
                format!("required external tool not found: {tool}"),
            )
            .cause(reason.clone())
            .domain(Some(domain.clone()))
            .operation(operation.clone()),
        }
    }

    /// Host-facing diagnostic: the currency `AppError` and the event log speak.
    pub fn diagnostic(&self) -> ErrorDiagnostic {
        self.describe().diagnostic
    }

    /// MCP-facing error: the same facts plus the retryability an agent needs,
    /// under the code strings the MCP wire contract froze.
    pub fn command_error(&self) -> CommandError {
        let described = self.describe();
        let agent_code = described.agent_code;
        let exit_code_hint = described.agent_exit_code_hint;
        let mut error = CommandError::from_diagnostic(described.diagnostic, described.retryable);
        if let Some(code) = agent_code {
            error.code = code.to_owned();
        }
        error.exit_code_hint = exit_code_hint;
        error
    }
}

impl From<RuntimeError> for ErrorDiagnostic {
    fn from(error: RuntimeError) -> Self {
        error.diagnostic()
    }
}

impl From<RuntimeError> for CommandError {
    fn from(error: RuntimeError) -> Self {
        error.command_error()
    }
}

pub trait BuiltinPlugin: Send + Sync {
    fn metadata(&self) -> PluginMetadata;
    fn manual(&self) -> PluginManual;
    fn required_tools(&self, _request: &InvocationRequest) -> Vec<RequiredTool> {
        self.metadata().required_tools
    }
    fn invoke(&self, request: &InvocationRequest) -> InvocationResponse;
    fn invoke_observed(&self, request: &InvocationRequest) -> InvocationObservation {
        InvocationObservation::without_outcome(self.invoke(request))
    }
    fn command_catalog(&self) -> Option<CommandCatalog> {
        None
    }
    fn required_tools_typed(&self, _request: &TypedInvocationRequest) -> Vec<RequiredTool> {
        self.metadata().required_tools
    }
    fn invoke_typed(&self, request: &TypedInvocationRequest) -> TypedInvocationResponse {
        let metadata = self.metadata();
        TypedInvocationResponse::error(CommandError::new(
            Some(metadata.domain),
            Some(request.command.clone()),
            "TYPED_COMMAND_UNSUPPORTED",
            "plugin does not support typed command invocation",
            "the plugin is available through the legacy CLI contract only",
            1,
            false,
        ))
    }
    fn cancel_typed(&self, _request_id: &str) -> bool {
        false
    }
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum SecretResolverError {
    #[error("secret was not found")]
    NotFound,
    #[error("vault is locked or contains invalid data")]
    VaultLocked,
    #[error("vault key is unavailable")]
    VaultKeyUnavailable,
}

pub trait SecretResolver: Send + Sync {
    fn resolve(&self, id: &str) -> Result<ResolvedSecret, SecretResolverError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunCheckOutcome {
    pub success: bool,
    pub timed_out: bool,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvocationOutcome {
    RunCheck(RunCheckOutcome),
}

#[derive(Debug, Clone)]
pub struct InvocationObservation {
    pub response: InvocationResponse,
    pub outcome: Option<InvocationOutcome>,
}

impl InvocationObservation {
    pub fn new(response: InvocationResponse, outcome: InvocationOutcome) -> Self {
        Self {
            response,
            outcome: Some(outcome),
        }
    }

    pub fn without_outcome(response: InvocationResponse) -> Self {
        Self {
            response,
            outcome: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginSource {
    Builtin,
    Dynamic,
}

#[derive(Debug, Clone)]
pub struct PluginLoadWarning {
    pub path: PathBuf,
    pub error: String,
}

#[derive(Debug, Clone)]
pub struct PluginLoadConflict {
    pub domain: String,
    pub winner: PluginMetadata,
    pub loser: PluginMetadata,
    pub winner_source: PluginSource,
    pub loser_source: PluginSource,
    pub reason: String,
}

#[derive(Debug, Default, Clone)]
pub struct PluginLoadReport {
    pub loaded: usize,
    pub skipped: usize,
    pub warnings: Vec<PluginLoadWarning>,
    pub conflicts: Vec<PluginLoadConflict>,
}

impl PluginLoadReport {
    fn push_warning(&mut self, path: PathBuf, error: impl Into<String>) {
        self.skipped += 1;
        self.warnings.push(PluginLoadWarning {
            path,
            error: error.into(),
        });
    }

    fn push_conflict(
        &mut self,
        winner: PluginMetadata,
        loser: PluginMetadata,
        winner_source: PluginSource,
        loser_source: PluginSource,
        reason: impl Into<String>,
    ) {
        let domain = winner.domain.clone();
        self.conflicts.push(PluginLoadConflict {
            domain,
            winner,
            loser,
            winner_source,
            loser_source,
            reason: reason.into(),
        });
    }
}

pub struct PluginManager {
    dynamic_plugins: HashMap<String, DynamicPlugin>,
    builtin_plugins: HashMap<String, Arc<dyn BuiltinPlugin>>,
    host_plugins: HashMap<String, Arc<dyn BuiltinPlugin>>,
    reserved_dynamic_domains: HashSet<String>,
    disabled_domains: RwLock<HashSet<String>>,
    typed_registry: RwLock<Option<Arc<TypedRegistry>>>,
    catalog_revision: AtomicU64,
    registry_build_count: AtomicU64,
    secret_resolver: Option<Arc<dyn SecretResolver>>,
}

#[derive(Debug, Clone)]
pub struct RegisteredPlugin {
    pub metadata: PluginMetadata,
    pub source: PluginSource,
    pub enabled: bool,
}

#[derive(Debug, Clone)]
pub struct RegisteredCommand {
    pub descriptor: CommandDescriptor,
    pub plugin: PluginMetadata,
    pub source: PluginSource,
}

struct TypedRegistry {
    commands: Vec<Arc<RegisteredTypedCommand>>,
    by_id: HashMap<String, Arc<RegisteredTypedCommand>>,
}

struct RegisteredTypedCommand {
    registered: RegisteredCommand,
    route: TypedCommandRoute,
    input_validator: jsonschema::Validator,
    output_validator: jsonschema::Validator,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TypedCommandRoute {
    Host,
    Builtin,
    Dynamic,
}

impl PluginManager {
    pub fn new() -> Self {
        Self {
            dynamic_plugins: HashMap::new(),
            builtin_plugins: HashMap::new(),
            host_plugins: HashMap::new(),
            reserved_dynamic_domains: HashSet::new(),
            disabled_domains: RwLock::new(HashSet::new()),
            typed_registry: RwLock::new(None),
            catalog_revision: AtomicU64::new(1),
            registry_build_count: AtomicU64::new(0),
            secret_resolver: None,
        }
    }

    pub fn set_secret_resolver(&mut self, resolver: Arc<dyn SecretResolver>) {
        self.secret_resolver = Some(resolver);
    }

    pub fn set_disabled_domains<I>(&self, domains: I)
    where
        I: IntoIterator<Item = String>,
    {
        let domains: HashSet<String> = domains
            .into_iter()
            .map(|domain| domain_key(&domain))
            .collect();
        let mut disabled_domains = write_disabled_domains(&self.disabled_domains);
        if *disabled_domains != domains {
            *disabled_domains = domains;
            self.catalog_revision.fetch_add(1, Ordering::AcqRel);
        }
    }

    pub fn is_domain_disabled(&self, domain: &str) -> bool {
        read_disabled_domains(&self.disabled_domains).contains(&domain_key(domain))
    }

    pub fn reserve_dynamic_domains<I, S>(&mut self, domains: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.reserved_dynamic_domains.extend(
            domains
                .into_iter()
                .map(|domain| domain_key(domain.as_ref())),
        );
    }

    pub fn register_builtin(&mut self, plugin: Arc<dyn BuiltinPlugin>) {
        let key = domain_key(&plugin.metadata().domain);
        self.builtin_plugins.insert(key, plugin);
        self.invalidate_typed_registry();
    }

    pub fn register_host_builtin(&mut self, plugin: Arc<dyn BuiltinPlugin>) {
        let key = domain_key(&plugin.metadata().domain);
        self.host_plugins.insert(key, plugin);
        self.invalidate_typed_registry();
    }

    pub fn load_dynamic_plugins_from_dir(&mut self, dir: &Path) -> PluginLoadReport {
        let mut report = PluginLoadReport::default();
        if !dir.exists() {
            return report;
        }
        let entries = match fs::read_dir(dir) {
            Ok(value) => value,
            Err(error) => {
                report.push_warning(
                    dir.to_path_buf(),
                    format!("failed to read plugin directory: {error}"),
                );
                return report;
            }
        };

        let mut plugin_paths = Vec::new();
        for entry in entries {
            let entry = match entry {
                Ok(value) => value,
                Err(error) => {
                    report.push_warning(
                        dir.to_path_buf(),
                        format!("failed to read plugin directory entry: {error}"),
                    );
                    continue;
                }
            };
            let path = entry.path();
            if !path.is_file() || !is_dynamic_lib_file(&path) {
                continue;
            }
            plugin_paths.push(path);
        }

        plugin_paths.sort_unstable_by(|left, right| left.file_name().cmp(&right.file_name()));
        let mut definitions_changed = false;
        for path in plugin_paths {
            match DynamicPlugin::load(path.clone()) {
                Ok(plugin) => {
                    let domain_key = domain_key(&plugin.metadata.domain);
                    if self.reserved_dynamic_domains.contains(&domain_key) {
                        report.push_warning(
                            path,
                            format!("dynamic plugin domain '{domain_key}' is reserved by the host"),
                        );
                        continue;
                    }
                    push_dynamic_plugin_conflicts(
                        &mut report,
                        &domain_key,
                        &plugin.metadata,
                        self.dynamic_plugins
                            .get(&domain_key)
                            .map(|existing| &existing.metadata),
                        self.builtin_plugins
                            .get(&domain_key)
                            .map(|existing| existing.metadata()),
                    );

                    self.dynamic_plugins.insert(domain_key, plugin);
                    report.loaded += 1;
                    definitions_changed = true;
                }
                Err(error) => {
                    report.push_warning(path, error.to_string());
                }
            }
        }

        if definitions_changed {
            self.invalidate_typed_registry();
        }

        report
    }

    pub fn invoke(
        &self,
        domain: &str,
        argv: Vec<String>,
        globals: GlobalOptionsWire,
    ) -> Result<InvocationResponse, RuntimeError> {
        self.invoke_observed(domain, argv, globals)
            .map(|observation| observation.response)
    }

    pub fn invoke_observed(
        &self,
        domain: &str,
        argv: Vec<String>,
        globals: GlobalOptionsWire,
    ) -> Result<InvocationObservation, RuntimeError> {
        self.invoke_credentialed(domain, argv, globals, &BTreeMap::new())
    }

    /// Runs one legacy argv invocation with `--credential SLOT=ID` mappings
    /// resolved host-side. The plugin sees only the resolved values, never the
    /// argv form, and validates that each slot is one it accepts.
    pub fn invoke_credentialed(
        &self,
        domain: &str,
        argv: Vec<String>,
        globals: GlobalOptionsWire,
        credentials: &BTreeMap<String, String>,
    ) -> Result<InvocationObservation, RuntimeError> {
        let domain = domain_key(domain);
        let request = InvocationRequest {
            domain: domain.clone(),
            argv,
            globals,
            resolved_secrets: self.resolve_credentials(&domain, credentials)?,
        };
        if let Some(plugin) = self.dynamic_plugins.get(&domain) {
            if self.is_domain_disabled(&domain) {
                return Err(RuntimeError::DomainDisabled(domain.clone()));
            }
            preflight_required_tools(&request, &plugin.metadata.required_tools)?;
            return plugin
                .invoke(&request)
                .map(InvocationObservation::without_outcome);
        }
        if let Some(plugin) = self.builtin_plugins.get(&domain) {
            if self.is_domain_disabled(&domain) {
                return Err(RuntimeError::DomainDisabled(domain.clone()));
            }
            let required_tools = plugin.required_tools(&request);
            preflight_required_tools(&request, &required_tools)?;
            return Ok(plugin.invoke_observed(&request));
        }

        Err(RuntimeError::DomainNotFound(domain))
    }

    pub fn list_enabled_commands(&self) -> Result<Vec<RegisteredCommand>, RuntimeError> {
        let registry = self.typed_registry()?;
        let disabled_domains = read_disabled_domains(&self.disabled_domains).clone();
        Ok(registry
            .commands
            .iter()
            .filter(|command| {
                command.route == TypedCommandRoute::Host
                    || !disabled_domains.contains(&domain_key(&command.registered.plugin.domain))
            })
            .map(|command| command.registered.clone())
            .collect())
    }

    pub fn catalog_revision(&self) -> u64 {
        self.catalog_revision.load(Ordering::Acquire)
    }

    fn typed_registry(&self) -> Result<Arc<TypedRegistry>, RuntimeError> {
        if let Some(registry) = self
            .typed_registry
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .cloned()
        {
            return Ok(registry);
        }

        let mut cached = self
            .typed_registry
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(registry) = cached.as_ref() {
            return Ok(Arc::clone(registry));
        }
        let built = Arc::new(self.build_typed_registry()?);
        self.registry_build_count.fetch_add(1, Ordering::AcqRel);
        *cached = Some(Arc::clone(&built));
        Ok(built)
    }

    fn build_typed_registry(&self) -> Result<TypedRegistry, RuntimeError> {
        let mut commands = Vec::new();
        for plugin in self.host_plugins.values() {
            let metadata = plugin.metadata();
            if let Some(catalog) = plugin.command_catalog() {
                append_typed_catalog(
                    &mut commands,
                    metadata,
                    PluginSource::Builtin,
                    TypedCommandRoute::Host,
                    catalog,
                )?;
            }
        }
        for (domain, plugin) in &self.builtin_plugins {
            if self.dynamic_plugins.contains_key(domain) {
                continue;
            }
            let metadata = plugin.metadata();
            if let Some(catalog) = plugin.command_catalog() {
                append_typed_catalog(
                    &mut commands,
                    metadata,
                    PluginSource::Builtin,
                    TypedCommandRoute::Builtin,
                    catalog,
                )?;
            }
        }
        for plugin in self.dynamic_plugins.values() {
            if let Some(catalog) = plugin.command_catalog.clone() {
                append_typed_catalog(
                    &mut commands,
                    plugin.metadata.clone(),
                    PluginSource::Dynamic,
                    TypedCommandRoute::Dynamic,
                    catalog,
                )?;
            }
        }

        commands.sort_by(|left, right| {
            left.registered
                .descriptor
                .id
                .cmp(&right.registered.descriptor.id)
                .then_with(|| {
                    left.registered
                        .plugin
                        .plugin_name
                        .cmp(&right.registered.plugin.plugin_name)
                })
        });
        let mut by_id = HashMap::with_capacity(commands.len());
        for command in &commands {
            let command_id = command.registered.descriptor.id.clone();
            if by_id
                .insert(command_id.clone(), Arc::clone(command))
                .is_some()
            {
                return Err(RuntimeError::InvalidCommandCatalog {
                    domain: command.registered.plugin.domain.clone(),
                    reason: format!(
                        "duplicate command id '{command_id}' across registered plugins"
                    ),
                });
            }
        }
        Ok(TypedRegistry { commands, by_id })
    }

    fn invalidate_typed_registry(&mut self) {
        *self
            .typed_registry
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        self.catalog_revision.fetch_add(1, Ordering::AcqRel);
    }

    #[cfg(test)]
    fn registry_build_count(&self) -> u64 {
        self.registry_build_count.load(Ordering::Acquire)
    }

    pub fn command_catalog_for_domain(
        &self,
        domain: &str,
    ) -> Result<Option<CommandCatalog>, RuntimeError> {
        let domain = domain_key(domain);
        let metadata = if let Some(plugin) = self.dynamic_plugins.get(&domain) {
            plugin.metadata.clone()
        } else if let Some(plugin) = self.builtin_plugins.get(&domain) {
            plugin.metadata()
        } else {
            return Ok(None);
        };
        let registry = self.typed_registry()?;
        let commands: Vec<CommandDescriptor> = registry
            .commands
            .iter()
            .filter(|command| {
                command.route != TypedCommandRoute::Host
                    && command
                        .registered
                        .plugin
                        .domain
                        .eq_ignore_ascii_case(&domain)
            })
            .map(|command| command.registered.descriptor.clone())
            .collect();
        if commands.is_empty() {
            return Ok(None);
        }
        Ok(Some(CommandCatalog::new(
            metadata.plugin_name,
            metadata.domain,
            commands,
        )))
    }

    pub fn invoke_typed(
        &self,
        request: &TypedInvocationRequest,
    ) -> Result<TypedInvocationResponse, RuntimeError> {
        let registry = self.typed_registry()?;
        let command = registry
            .by_id
            .get(&request.command)
            .cloned()
            .ok_or_else(|| RuntimeError::TypedCommandNotFound(request.command.clone()))?;
        let domain = command.registered.plugin.domain.as_str();
        if command.route != TypedCommandRoute::Host && self.is_domain_disabled(domain) {
            return Err(RuntimeError::DomainDisabled(domain.to_owned()));
        }
        let request = self.prepare_typed_request(&command, request)?;
        let request = request.as_ref();

        let response = match command.route {
            TypedCommandRoute::Host => {
                let plugin = self
                    .host_plugins
                    .get(&domain_key(domain))
                    .ok_or_else(|| RuntimeError::TypedCommandNotFound(request.command.clone()))?;
                let required_tools = plugin.required_tools_typed(request);
                preflight_typed_required_tools(&request.command, domain, &required_tools)?;
                plugin.invoke_typed(request)
            }
            TypedCommandRoute::Builtin => {
                let plugin = self
                    .builtin_plugins
                    .get(&domain_key(domain))
                    .ok_or_else(|| RuntimeError::TypedCommandNotFound(request.command.clone()))?;
                let required_tools = plugin.required_tools_typed(request);
                preflight_typed_required_tools(&request.command, domain, &required_tools)?;
                plugin.invoke_typed(request)
            }
            TypedCommandRoute::Dynamic => {
                let plugin = self
                    .dynamic_plugins
                    .get(&domain_key(domain))
                    .ok_or_else(|| RuntimeError::TypedCommandNotFound(request.command.clone()))?;
                preflight_typed_required_tools(
                    &request.command,
                    domain,
                    &plugin.metadata.required_tools,
                )?;
                plugin.invoke_typed(request)?
            }
        };
        typed::validate_response_with(&request.command, &command.output_validator, &response)?;
        Ok(response)
    }

    fn resolve_credentials(
        &self,
        command: &str,
        credentials: &BTreeMap<String, String>,
    ) -> Result<BTreeMap<String, ResolvedSecret>, RuntimeError> {
        let mut resolved = BTreeMap::new();
        for (slot, id) in credentials {
            let Some(resolver) = self.secret_resolver.as_ref() else {
                return Err(RuntimeError::VaultKeyUnavailable {
                    command: command.to_owned(),
                    slot: slot.clone(),
                    id: id.clone(),
                });
            };
            let secret = resolver
                .resolve(id)
                .map_err(|error| map_secret_resolver_error(error, command, slot, id))?;
            resolved.insert(slot.clone(), secret);
        }
        Ok(resolved)
    }

    fn prepare_typed_request<'a>(
        &self,
        command: &RegisteredTypedCommand,
        request: &'a TypedInvocationRequest,
    ) -> Result<Cow<'a, TypedInvocationRequest>, RuntimeError> {
        let request = if request.resolved_secrets.is_empty() {
            Cow::Borrowed(request)
        } else {
            Cow::Owned(request.clone().with_resolved_secrets(BTreeMap::new()))
        };
        let public_request = request.as_ref();
        let slots = &command.registered.descriptor.secret_slots;
        if slots.is_empty() {
            typed::validate_arguments_with(
                &public_request.command,
                &command.input_validator,
                &public_request.arguments,
            )?;
            return Ok(request);
        }

        let mut schema_arguments = public_request.arguments.clone();
        if let Some(arguments) = schema_arguments.as_object_mut() {
            arguments.remove("credentials");
        }
        typed::validate_arguments_with(
            &public_request.command,
            &command.input_validator,
            &schema_arguments,
        )?;

        let arguments = public_request.arguments.as_object().ok_or_else(|| {
            RuntimeError::TypedInvocation(format!(
                "arguments for '{}' must be a JSON object",
                public_request.command
            ))
        })?;
        let credentials = match arguments.get("credentials") {
            None => None,
            Some(serde_json::Value::Object(credentials)) => Some(credentials),
            Some(_) => {
                return Err(RuntimeError::TypedInvocation(format!(
                    "credentials for '{}' must be a JSON object",
                    public_request.command
                )));
            }
        };
        let Some(credentials) = credentials else {
            require_secret_slots(&public_request.command, slots, None)?;
            return Ok(request);
        };

        let declared = slots
            .iter()
            .map(|slot| slot.name.as_str())
            .collect::<HashSet<_>>();
        let mut undeclared = credentials
            .keys()
            .filter(|name| !declared.contains(name.as_str()))
            .collect::<Vec<_>>();
        undeclared.sort();
        if let Some(slot) = undeclared.first() {
            return Err(RuntimeError::TypedInvocation(format!(
                "credentials for '{}' contain undeclared slot '{}'",
                public_request.command, slot
            )));
        }
        require_secret_slots(&public_request.command, slots, Some(credentials))?;

        if credentials.is_empty() {
            return Ok(request);
        }
        let resolver = self.secret_resolver.as_ref();
        let mut resolved = BTreeMap::new();
        for slot in slots {
            let Some(id) = credentials.get(&slot.name) else {
                continue;
            };
            let id = id.as_str().ok_or_else(|| {
                RuntimeError::TypedInvocation(format!(
                    "credential id for slot '{}' in '{}' must be a string",
                    slot.name, public_request.command
                ))
            })?;
            let secret = match resolver {
                Some(resolver) => resolver.resolve(id).map_err(|error| {
                    map_secret_resolver_error(error, &public_request.command, &slot.name, id)
                })?,
                None => {
                    return Err(RuntimeError::VaultKeyUnavailable {
                        command: public_request.command.clone(),
                        slot: slot.name.clone(),
                        id: id.to_owned(),
                    });
                }
            };
            if !slot.accepted_kinds.contains(&secret.kind) {
                return Err(RuntimeError::SecretKindMismatch {
                    command: public_request.command.clone(),
                    slot: slot.name.clone(),
                    id: id.to_owned(),
                    kind: secret.kind,
                    accepted_kinds: slot.accepted_kinds.clone(),
                });
            }
            resolved.insert(slot.name.clone(), secret);
        }

        Ok(Cow::Owned(
            request.into_owned().with_resolved_secrets(resolved),
        ))
    }

    pub fn cancel_typed(&self, command: &str, request_id: &str) -> bool {
        let Ok(registry) = self.typed_registry() else {
            return false;
        };
        let Some(registered) = registry.by_id.get(command) else {
            return false;
        };
        let domain = domain_key(&registered.registered.plugin.domain);
        match registered.route {
            TypedCommandRoute::Host => self
                .host_plugins
                .get(&domain)
                .is_some_and(|plugin| plugin.cancel_typed(request_id)),
            TypedCommandRoute::Builtin => self
                .builtin_plugins
                .get(&domain)
                .is_some_and(|plugin| plugin.cancel_typed(request_id)),
            TypedCommandRoute::Dynamic => self
                .dynamic_plugins
                .get(&domain)
                .is_some_and(|plugin| plugin.cancel_typed(request_id)),
        }
    }

    pub fn list_plugins(&self) -> Vec<PluginMetadata> {
        self.list_registered_plugins()
            .into_iter()
            .map(|plugin| plugin.metadata)
            .collect()
    }

    pub fn list_registered_plugins(&self) -> Vec<RegisteredPlugin> {
        let mut plugins_by_domain = HashMap::new();
        for plugin in self.builtin_plugins.values() {
            let metadata = plugin.metadata();
            let domain = domain_key(&metadata.domain);
            plugins_by_domain.entry(domain).or_insert(RegisteredPlugin {
                enabled: !self.is_domain_disabled(&metadata.domain),
                metadata,
                source: PluginSource::Builtin,
            });
        }
        for plugin in self.dynamic_plugins.values() {
            let metadata = plugin.metadata.clone();
            let domain = domain_key(&metadata.domain);
            plugins_by_domain.insert(
                domain,
                RegisteredPlugin {
                    enabled: !self.is_domain_disabled(&metadata.domain),
                    metadata,
                    source: PluginSource::Dynamic,
                },
            );
        }
        let mut plugins = Vec::from_iter(plugins_by_domain.into_values());
        plugins.sort_by(|left, right| {
            left.metadata
                .domain
                .cmp(&right.metadata.domain)
                .then_with(|| left.metadata.plugin_name.cmp(&right.metadata.plugin_name))
        });
        plugins
    }

    pub fn collect_plugin_manuals(&self) -> Vec<PluginManual> {
        let mut manuals = Vec::new();
        for plugin in self.list_registered_plugins() {
            if !plugin.enabled {
                continue;
            }
            let domain = domain_key(&plugin.metadata.domain);
            match plugin.source {
                PluginSource::Builtin => {
                    let plugin = self
                        .builtin_plugins
                        .get(&domain)
                        .expect("builtin plugin should exist for listed domain");
                    manuals.push(plugin.manual());
                }
                PluginSource::Dynamic => {
                    let plugin = self
                        .dynamic_plugins
                        .get(&domain)
                        .expect("dynamic plugin should exist for listed domain");
                    match plugin.manual() {
                        Ok(Some(manual)) => manuals.push(manual),
                        Ok(None) => manuals.push(fallback_manual(
                            &plugin.metadata,
                            "manual is not provided by this dynamic plugin".to_owned(),
                        )),
                        Err(error) => manuals.push(fallback_manual(
                            &plugin.metadata,
                            format!("failed to load plugin manual: {error}"),
                        )),
                    }
                }
            }
        }
        manuals.sort_by(|left, right| left.domain.cmp(&right.domain));
        manuals
    }

    pub fn list_enabled_plugins(&self) -> Vec<PluginMetadata> {
        self.list_registered_plugins()
            .into_iter()
            .filter(|plugin| plugin.enabled)
            .map(|plugin| plugin.metadata)
            .collect()
    }
}

fn append_typed_catalog(
    commands: &mut Vec<Arc<RegisteredTypedCommand>>,
    metadata: PluginMetadata,
    source: PluginSource,
    route: TypedCommandRoute,
    catalog: CommandCatalog,
) -> Result<(), RuntimeError> {
    let validators = typed::compile_catalog(&metadata, &catalog)?;
    for (descriptor, validators) in catalog.commands.into_iter().zip(validators) {
        debug_assert_eq!(descriptor.id, validators.command_id);
        commands.push(Arc::new(RegisteredTypedCommand {
            registered: RegisteredCommand {
                descriptor,
                plugin: metadata.clone(),
                source,
            },
            route,
            input_validator: validators.input,
            output_validator: validators.output,
        }));
    }
    Ok(())
}

impl Default for PluginManager {
    fn default() -> Self {
        Self::new()
    }
}

fn push_dynamic_plugin_conflicts(
    report: &mut PluginLoadReport,
    domain_key: &str,
    new_metadata: &PluginMetadata,
    existing_dynamic: Option<&PluginMetadata>,
    existing_builtin: Option<PluginMetadata>,
) {
    if let Some(existing) = existing_dynamic
        && !is_same_plugin_identity(new_metadata, existing)
    {
        report.push_conflict(
            new_metadata.clone(),
            existing.clone(),
            PluginSource::Dynamic,
            PluginSource::Dynamic,
            format!(
                "multiple dynamic plugins for domain '{domain_key}', last loaded takes precedence",
            ),
        );
    }
    if let Some(existing) = existing_builtin {
        report.push_conflict(
            new_metadata.clone(),
            existing,
            PluginSource::Dynamic,
            PluginSource::Builtin,
            format!("dynamic plugin shadows builtin plugin for domain '{domain_key}'"),
        );
    }
}

fn is_same_plugin_identity(left: &PluginMetadata, right: &PluginMetadata) -> bool {
    left.plugin_name == right.plugin_name
        && left.domain.eq_ignore_ascii_case(&right.domain)
        && left.description == right.description
        && left.abi_version == right.abi_version
        && left.compatibility == right.compatibility
}

#[derive(Debug, Clone, Copy, Default)]
struct TypedSymbolAvailability {
    catalog: bool,
    invoke: bool,
    cancel: bool,
}

impl TypedSymbolAvailability {
    fn is_complete(self) -> bool {
        self.catalog && self.invoke && self.cancel
    }

    fn any(self) -> bool {
        self.catalog || self.invoke || self.cancel
    }
}

struct DynamicPlugin {
    _library: Library,
    metadata: PluginMetadata,
    invoke_json: unsafe extern "C" fn(*const c_char) -> *mut c_char,
    manual_json: Option<unsafe extern "C" fn() -> *mut c_char>,
    command_catalog: Option<CommandCatalog>,
    invoke_command_json: Option<AhPluginInvokeCommandJsonV1>,
    cancel_command: Option<AhPluginCancelCommandV1>,
    free_c_string: unsafe extern "C" fn(*mut c_char),
}

impl DynamicPlugin {
    fn load(path: PathBuf) -> Result<Self, RuntimeError> {
        let library =
            unsafe { Library::new(&path) }.map_err(|source| RuntimeError::LibraryLoad {
                path: path.clone(),
                source,
            })?;
        let entry = unsafe { library.get::<AhPluginEntryV1>(AH_PLUGIN_ENTRY_V1_SYMBOL) }.map_err(
            |source| RuntimeError::SymbolLoad {
                path: path.clone(),
                source,
            },
        )?;
        let api_ptr = unsafe { entry() };
        if api_ptr.is_null() {
            return Err(RuntimeError::InvalidMetadata {
                path,
                reason: "null plugin api pointer".to_owned(),
            });
        }
        let api = unsafe { &*api_ptr };
        if api.abi_version != AH_PLUGIN_ABI_VERSION {
            return Err(RuntimeError::AbiVersionMismatch {
                path,
                found: api.abi_version,
                expected: AH_PLUGIN_ABI_VERSION,
            });
        }

        let plugin_name = unsafe { c_ptr_to_string(api.plugin_name) }.map_err(|reason| {
            RuntimeError::InvalidMetadata {
                path: path.clone(),
                reason,
            }
        })?;
        let domain = unsafe { c_ptr_to_string(api.domain) }.map_err(|reason| {
            RuntimeError::InvalidMetadata {
                path: path.clone(),
                reason,
            }
        })?;
        let description = unsafe { c_ptr_to_string(api.description) }.map_err(|reason| {
            RuntimeError::InvalidMetadata {
                path: path.clone(),
                reason,
            }
        })?;
        let manual_json =
            unsafe { library.get::<AhPluginManualJsonV1>(AH_PLUGIN_MANUAL_JSON_V1_SYMBOL) }
                .ok()
                .map(|symbol| *symbol);
        let metadata_json =
            unsafe { library.get::<AhPluginMetadataJsonV1>(AH_PLUGIN_METADATA_JSON_V1_SYMBOL) }
                .ok()
                .map(|symbol| *symbol);
        let command_catalog_json = unsafe {
            library.get::<AhPluginCommandCatalogJsonV1>(AH_PLUGIN_COMMAND_CATALOG_JSON_V1_SYMBOL)
        }
        .ok()
        .map(|symbol| *symbol);
        let invoke_command_json = unsafe {
            library.get::<AhPluginInvokeCommandJsonV1>(AH_PLUGIN_INVOKE_COMMAND_JSON_V1_SYMBOL)
        }
        .ok()
        .map(|symbol| *symbol);
        let cancel_command =
            unsafe { library.get::<AhPluginCancelCommandV1>(AH_PLUGIN_CANCEL_COMMAND_V1_SYMBOL) }
                .ok()
                .map(|symbol| *symbol);
        let typed_symbols = TypedSymbolAvailability {
            catalog: command_catalog_json.is_some(),
            invoke: invoke_command_json.is_some(),
            cancel: cancel_command.is_some(),
        };
        let metadata = if let Some(metadata_json) = metadata_json {
            let metadata_ptr = unsafe { metadata_json() };
            if metadata_ptr.is_null() {
                return Err(RuntimeError::InvalidMetadata {
                    path,
                    reason: "metadata JSON symbol returned null".to_owned(),
                });
            }
            let metadata_raw = unsafe { c_ptr_to_string(metadata_ptr.cast_const()) };
            unsafe { (api.free_c_string)(metadata_ptr) };
            let metadata_raw = metadata_raw.map_err(|reason| RuntimeError::InvalidMetadata {
                path: path.clone(),
                reason,
            })?;
            let metadata =
                serde_json::from_str::<PluginMetadata>(&metadata_raw).map_err(|error| {
                    RuntimeError::InvalidMetadata {
                        path: path.clone(),
                        reason: format!("metadata JSON parse failed: {error}"),
                    }
                })?;
            validate_plugin_metadata_contract(
                &path,
                api.abi_version,
                &plugin_name,
                &domain,
                &description,
                &metadata,
                manual_json.is_some(),
                typed_symbols,
            )?;
            metadata
        } else {
            PluginMetadata {
                plugin_name,
                domain,
                description,
                abi_version: api.abi_version,
                required_tools: Vec::new(),
                compatibility: Default::default(),
            }
        };
        validate_plugin_api_contract(&path, &metadata, manual_json.is_some(), typed_symbols)?;
        let command_catalog =
            if metadata.supports_capability(plugin_capabilities::TYPED_COMMANDS_V1) {
                let command_catalog_json =
                    command_catalog_json.expect("typed catalog symbol should be validated");
                let response_ptr = unsafe { command_catalog_json() };
                if response_ptr.is_null() {
                    return Err(RuntimeError::InvalidMetadata {
                        path,
                        reason: "typed command catalog symbol returned null".to_owned(),
                    });
                }
                let response_raw = unsafe {
                    let decoded = c_ptr_to_string(response_ptr.cast_const());
                    (api.free_c_string)(response_ptr);
                    decoded
                }
                .map_err(|reason| RuntimeError::InvalidMetadata {
                    path: path.clone(),
                    reason,
                })?;
                let catalog =
                    serde_json::from_str::<CommandCatalog>(&response_raw).map_err(|error| {
                        RuntimeError::InvalidMetadata {
                            path: path.clone(),
                            reason: format!("typed command catalog parse failed: {error}"),
                        }
                    })?;
                typed::validate_catalog(&metadata, &catalog)?;
                Some(catalog)
            } else {
                None
            };

        Ok(Self {
            _library: library,
            metadata,
            invoke_json: api.invoke_json,
            manual_json,
            command_catalog,
            invoke_command_json,
            cancel_command,
            free_c_string: api.free_c_string,
        })
    }

    fn invoke(&self, request: &InvocationRequest) -> Result<InvocationResponse, RuntimeError> {
        let request_json = serde_json::to_string(request).map_err(|error| {
            RuntimeError::Invocation(format!("request serialization failed: {error}"))
        })?;
        let c_request = CString::new(request_json).map_err(|error| {
            RuntimeError::Invocation(format!("invalid request cstring: {error}"))
        })?;

        let response_ptr = unsafe { (self.invoke_json)(c_request.as_ptr()) };
        if response_ptr.is_null() {
            return Err(RuntimeError::Invocation(
                "plugin returned null response".to_owned(),
            ));
        }
        let response_raw = unsafe {
            let decoded = c_ptr_to_string(response_ptr);
            (self.free_c_string)(response_ptr);
            decoded
        }
        .map_err(RuntimeError::ResponseParse)?;

        serde_json::from_str::<InvocationResponse>(&response_raw)
            .map_err(|error| RuntimeError::ResponseParse(error.to_string()))
    }

    fn invoke_typed(
        &self,
        request: &TypedInvocationRequest,
    ) -> Result<TypedInvocationResponse, RuntimeError> {
        let invoke_command_json = self.invoke_command_json.ok_or_else(|| {
            RuntimeError::TypedInvocation(format!(
                "plugin '{}' does not expose typed invocation",
                self.metadata.plugin_name
            ))
        })?;
        let request_json = serde_json::to_string(request).map_err(|error| {
            RuntimeError::TypedInvocation(format!("request serialization failed: {error}"))
        })?;
        let c_request = CString::new(request_json).map_err(|error| {
            RuntimeError::TypedInvocation(format!("invalid request cstring: {error}"))
        })?;
        let response_ptr = unsafe { invoke_command_json(c_request.as_ptr()) };
        if response_ptr.is_null() {
            return Err(RuntimeError::TypedInvocation(
                "plugin returned null typed response".to_owned(),
            ));
        }
        let response_raw = unsafe {
            let decoded = c_ptr_to_string(response_ptr);
            (self.free_c_string)(response_ptr);
            decoded
        }
        .map_err(RuntimeError::ResponseParse)?;
        serde_json::from_str::<TypedInvocationResponse>(&response_raw)
            .map_err(|error| RuntimeError::ResponseParse(error.to_string()))
    }

    fn cancel_typed(&self, request_id: &str) -> bool {
        let Some(cancel_command) = self.cancel_command else {
            return false;
        };
        let Ok(request_id) = CString::new(request_id) else {
            return false;
        };
        unsafe { cancel_command(request_id.as_ptr()) != 0 }
    }

    fn manual(&self) -> Result<Option<PluginManual>, RuntimeError> {
        let Some(manual_json) = self.manual_json else {
            return Ok(None);
        };

        let response_ptr = unsafe { manual_json() };
        if response_ptr.is_null() {
            return Ok(None);
        }
        let response_raw = unsafe {
            let decoded = c_ptr_to_string(response_ptr);
            (self.free_c_string)(response_ptr);
            decoded
        }
        .map_err(RuntimeError::ResponseParse)?;
        let manual = serde_json::from_str::<PluginManual>(&response_raw)
            .map_err(|error| RuntimeError::ResponseParse(error.to_string()))?;
        Ok(Some(manual))
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_plugin_metadata_contract(
    path: &Path,
    abi_version: u32,
    plugin_name: &str,
    domain: &str,
    description: &str,
    metadata: &PluginMetadata,
    manual_json_available: bool,
    typed_symbols: TypedSymbolAvailability,
) -> Result<(), RuntimeError> {
    if metadata.plugin_name != plugin_name {
        return Err(RuntimeError::InvalidMetadata {
            path: path.to_path_buf(),
            reason: format!(
                "metadata plugin_name '{}' does not match ABI plugin_name '{}'",
                metadata.plugin_name, plugin_name
            ),
        });
    }
    if metadata.domain != domain {
        return Err(RuntimeError::InvalidMetadata {
            path: path.to_path_buf(),
            reason: format!(
                "metadata domain '{}' does not match ABI domain '{}'",
                metadata.domain, domain
            ),
        });
    }
    if metadata.description != description {
        return Err(RuntimeError::InvalidMetadata {
            path: path.to_path_buf(),
            reason: "metadata description does not match ABI description".to_owned(),
        });
    }
    if metadata.abi_version != abi_version {
        return Err(RuntimeError::InvalidMetadata {
            path: path.to_path_buf(),
            reason: format!(
                "metadata abi_version {} does not match ABI version {}",
                metadata.abi_version, abi_version
            ),
        });
    }
    validate_plugin_api_contract(path, metadata, manual_json_available, typed_symbols)
}

fn validate_plugin_api_contract(
    path: &Path,
    metadata: &PluginMetadata,
    manual_json_available: bool,
    typed_symbols: TypedSymbolAvailability,
) -> Result<(), RuntimeError> {
    if !metadata.is_api_compatible_with_host() {
        return Err(RuntimeError::ApiVersionMismatch {
            path: path.to_path_buf(),
            found_major: metadata.compatibility.api_version.major,
            found_minor: metadata.compatibility.api_version.minor,
            supported_major: AH_PLUGIN_API_MAJOR_VERSION,
            supported_minor: AH_PLUGIN_API_MINOR_VERSION,
        });
    }
    if metadata.supports_capability(plugin_capabilities::MANUAL_JSON) && !manual_json_available {
        return Err(RuntimeError::InvalidMetadata {
            path: path.to_path_buf(),
            reason: format!(
                "metadata declares '{}' capability but '{}' symbol is missing",
                plugin_capabilities::MANUAL_JSON,
                String::from_utf8_lossy(AH_PLUGIN_MANUAL_JSON_V1_SYMBOL).trim_end_matches('\0')
            ),
        });
    }
    let declares_typed = metadata.supports_capability(plugin_capabilities::TYPED_COMMANDS_V1);
    if declares_typed && !typed_symbols.is_complete() {
        let mut missing = Vec::new();
        if !typed_symbols.catalog {
            missing.push(
                String::from_utf8_lossy(AH_PLUGIN_COMMAND_CATALOG_JSON_V1_SYMBOL)
                    .trim_end_matches('\0')
                    .to_owned(),
            );
        }
        if !typed_symbols.invoke {
            missing.push(
                String::from_utf8_lossy(AH_PLUGIN_INVOKE_COMMAND_JSON_V1_SYMBOL)
                    .trim_end_matches('\0')
                    .to_owned(),
            );
        }
        if !typed_symbols.cancel {
            missing.push(
                String::from_utf8_lossy(AH_PLUGIN_CANCEL_COMMAND_V1_SYMBOL)
                    .trim_end_matches('\0')
                    .to_owned(),
            );
        }
        return Err(RuntimeError::InvalidMetadata {
            path: path.to_path_buf(),
            reason: format!(
                "metadata declares '{}' capability but required symbol(s) are missing: {}",
                plugin_capabilities::TYPED_COMMANDS_V1,
                missing.join(", ")
            ),
        });
    }
    if !declares_typed && typed_symbols.any() {
        return Err(RuntimeError::InvalidMetadata {
            path: path.to_path_buf(),
            reason: format!(
                "typed command symbols require '{}' capability",
                plugin_capabilities::TYPED_COMMANDS_V1
            ),
        });
    }
    Ok(())
}

fn is_dynamic_lib_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some("dll") | Some("so") | Some("dylib")
    )
}

fn fallback_manual(metadata: &PluginMetadata, note: String) -> PluginManual {
    PluginManual {
        plugin_name: metadata.plugin_name.clone(),
        domain: metadata.domain.clone(),
        description: metadata.description.clone(),
        commands: Vec::new(),
        notes: vec![note],
    }
}

fn preflight_required_tools(
    request: &InvocationRequest,
    required_tools: &[RequiredTool],
) -> Result<(), RuntimeError> {
    for tool in required_tools {
        if tool.name.trim().is_empty() {
            continue;
        }
        let check_args = if tool.check_args.is_empty() {
            vec!["--version".to_owned()]
        } else {
            tool.check_args.clone()
        };
        if !core::run_command_ok(&tool.name, &check_args) {
            return Err(RuntimeError::DependencyMissing {
                domain: request.domain.clone(),
                operation: infer_operation(&request.domain, &request.argv),
                tool: tool.name.clone(),
                reason: tool.reason.clone(),
            });
        }
    }
    Ok(())
}

fn preflight_typed_required_tools(
    command: &str,
    domain: &str,
    required_tools: &[RequiredTool],
) -> Result<(), RuntimeError> {
    for tool in required_tools {
        if tool.name.trim().is_empty() {
            continue;
        }
        let check_args = if tool.check_args.is_empty() {
            vec!["--version".to_owned()]
        } else {
            tool.check_args.clone()
        };
        if !core::run_command_ok(&tool.name, &check_args) {
            return Err(RuntimeError::DependencyMissing {
                domain: domain.to_owned(),
                operation: Some(command.to_owned()),
                tool: tool.name.clone(),
                reason: tool.reason.clone(),
            });
        }
    }
    Ok(())
}

fn require_secret_slots(
    command: &str,
    slots: &[SecretSlot],
    credentials: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Result<(), RuntimeError> {
    if let Some(slot) = slots.iter().find(|slot| {
        slot.required && !credentials.is_some_and(|value| value.contains_key(&slot.name))
    }) {
        return Err(RuntimeError::SecretRequired {
            command: command.to_owned(),
            slot: slot.name.clone(),
        });
    }
    Ok(())
}

fn map_secret_resolver_error(
    error: SecretResolverError,
    command: &str,
    slot: &str,
    id: &str,
) -> RuntimeError {
    match error {
        SecretResolverError::NotFound => RuntimeError::SecretNotFound {
            command: command.to_owned(),
            slot: slot.to_owned(),
            id: id.to_owned(),
        },
        SecretResolverError::VaultLocked => RuntimeError::VaultLocked {
            command: command.to_owned(),
            slot: slot.to_owned(),
            id: id.to_owned(),
        },
        SecretResolverError::VaultKeyUnavailable => RuntimeError::VaultKeyUnavailable {
            command: command.to_owned(),
            slot: slot.to_owned(),
            id: id.to_owned(),
        },
    }
}

fn infer_operation(domain: &str, argv: &[String]) -> Option<String> {
    argv.first().map(|command| format!("{domain}.{command}"))
}

fn domain_key(domain: &str) -> String {
    domain.trim().to_ascii_lowercase()
}

fn read_disabled_domains(
    domains: &RwLock<HashSet<String>>,
) -> std::sync::RwLockReadGuard<'_, HashSet<String>> {
    domains
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn write_disabled_domains(
    domains: &RwLock<HashSet<String>>,
) -> std::sync::RwLockWriteGuard<'_, HashSet<String>> {
    domains
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        ffi::CStr,
        fs,
        path::Path,
        sync::{
            Arc, Mutex, OnceLock,
            atomic::{AtomicUsize, Ordering as AtomicOrdering},
        },
        time::{SystemTime, UNIX_EPOCH},
    };

    use ah_plugin_api::{GlobalOptionsWire, ResolvedSecret, SecretSlot};

    use super::*;

    fn test_metadata(plugin_name: &str, domain: &str) -> PluginMetadata {
        PluginMetadata {
            plugin_name: plugin_name.to_owned(),
            domain: domain.to_owned(),
            description: format!("{domain} test plugin"),
            abi_version: AH_PLUGIN_ABI_VERSION,
            required_tools: Vec::new(),
            compatibility: Default::default(),
        }
    }

    struct EchoBuiltinPlugin;

    struct FixedSecretResolver {
        calls: AtomicUsize,
    }

    impl SecretResolver for FixedSecretResolver {
        fn resolve(&self, id: &str) -> Result<ResolvedSecret, SecretResolverError> {
            self.calls.fetch_add(1, AtomicOrdering::Relaxed);
            Ok(ResolvedSecret {
                id: id.to_owned(),
                kind: "postgres".to_owned(),
                values: BTreeMap::from([("password".to_owned(), "private-password".to_owned())]),
            })
        }
    }

    struct ErrorSecretResolver(SecretResolverError);

    impl SecretResolver for ErrorSecretResolver {
        fn resolve(&self, _id: &str) -> Result<ResolvedSecret, SecretResolverError> {
            Err(self.0)
        }
    }

    struct WrongKindSecretResolver;

    impl SecretResolver for WrongKindSecretResolver {
        fn resolve(&self, id: &str) -> Result<ResolvedSecret, SecretResolverError> {
            Ok(ResolvedSecret {
                id: id.to_owned(),
                kind: "http-basic".to_owned(),
                values: BTreeMap::from([("password".to_owned(), "redaction-sentinel".to_owned())]),
            })
        }
    }

    struct SecretProbePlugin {
        received: Arc<Mutex<Option<TypedInvocationRequest>>>,
        required: bool,
        include_slot: bool,
    }

    fn probe_descriptor(required: bool, include_slot: bool) -> CommandDescriptor {
        let mut slot = SecretSlot::optional("database", ["postgres"], "Database credential");
        slot.required = required;
        let descriptor = CommandDescriptor::new(
            "probe.run",
            "Probe",
            "Capture one typed request for runtime tests.",
            serde_json::json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            serde_json::json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            ah_plugin_api::CommandEffects::new(
                true,
                false,
                true,
                false,
                vec![ah_plugin_api::CommandEffect::ConfigurationRead],
                ah_plugin_api::RiskLevel::Low,
                "Captures an in-memory test request only.",
                ah_plugin_api::Reversibility::Yes,
            ),
        );
        if include_slot {
            descriptor.with_secret_slot(slot)
        } else {
            descriptor
        }
    }

    impl BuiltinPlugin for SecretProbePlugin {
        fn metadata(&self) -> PluginMetadata {
            let mut metadata = test_metadata("builtin-probe", "probe");
            metadata.compatibility = ah_plugin_api::PluginCompatibility::current()
                .with_capability(plugin_capabilities::TYPED_COMMANDS_V1);
            metadata
        }

        fn manual(&self) -> PluginManual {
            PluginManual {
                plugin_name: "builtin-probe".to_owned(),
                domain: "probe".to_owned(),
                description: "probe test plugin".to_owned(),
                commands: Vec::new(),
                notes: Vec::new(),
            }
        }

        fn invoke(&self, _request: &InvocationRequest) -> InvocationResponse {
            InvocationResponse::ok(None)
        }

        fn command_catalog(&self) -> Option<CommandCatalog> {
            Some(CommandCatalog::new(
                "builtin-probe",
                "probe",
                vec![probe_descriptor(self.required, self.include_slot)],
            ))
        }

        fn invoke_typed(&self, request: &TypedInvocationRequest) -> TypedInvocationResponse {
            *self.received.lock().unwrap() = Some(request.clone());
            TypedInvocationResponse::success(serde_json::json!({}), None)
        }
    }

    fn probe_manager(
        required: bool,
        resolver: Option<Arc<dyn SecretResolver>>,
        host: bool,
    ) -> (PluginManager, Arc<Mutex<Option<TypedInvocationRequest>>>) {
        let received = Arc::new(Mutex::new(None));
        let mut manager = PluginManager::new();
        if let Some(resolver) = resolver {
            manager.set_secret_resolver(resolver);
        }
        let plugin = Arc::new(SecretProbePlugin {
            received: Arc::clone(&received),
            required,
            include_slot: true,
        });
        if host {
            manager.register_host_builtin(plugin);
        } else {
            manager.register_builtin(plugin);
        }
        (manager, received)
    }

    static DYNAMIC_PROBE_REQUEST: OnceLock<Mutex<Option<TypedInvocationRequest>>> = OnceLock::new();

    unsafe extern "C" fn dynamic_probe_invoke(request: *const c_char) -> *mut c_char {
        let raw = unsafe { CStr::from_ptr(request) }.to_string_lossy();
        let request = serde_json::from_str::<TypedInvocationRequest>(&raw).unwrap();
        *DYNAMIC_PROBE_REQUEST
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap() = Some(request);
        let response = TypedInvocationResponse::success(serde_json::json!({}), None);
        CString::new(serde_json::to_string(&response).unwrap())
            .unwrap()
            .into_raw()
    }

    unsafe extern "C" fn dynamic_probe_free(value: *mut c_char) {
        if !value.is_null() {
            drop(unsafe { CString::from_raw(value) });
        }
    }

    unsafe extern "C" fn dynamic_probe_cancel(_request_id: *const c_char) -> i32 {
        0
    }

    #[cfg(unix)]
    fn current_process_library() -> Library {
        libloading::os::unix::Library::this().into()
    }

    #[cfg(windows)]
    fn current_process_library() -> Library {
        libloading::os::windows::Library::this().unwrap().into()
    }

    fn register_dynamic_probe(manager: &mut PluginManager) {
        let mut metadata = test_metadata("dynamic-probe", "probe");
        metadata.compatibility = ah_plugin_api::PluginCompatibility::current()
            .with_capability(plugin_capabilities::TYPED_COMMANDS_V1);
        manager.dynamic_plugins.insert(
            "probe".to_owned(),
            DynamicPlugin {
                _library: current_process_library(),
                metadata,
                invoke_json: dynamic_probe_invoke,
                manual_json: None,
                command_catalog: Some(CommandCatalog::new(
                    "dynamic-probe",
                    "probe",
                    vec![probe_descriptor(false, true)],
                )),
                invoke_command_json: Some(dynamic_probe_invoke),
                cancel_command: Some(dynamic_probe_cancel),
                free_c_string: dynamic_probe_free,
            },
        );
        manager.invalidate_typed_registry();
    }

    #[test]
    fn a_domain_without_a_secret_resolver_reports_the_vault_as_unavailable() {
        let mut manager = PluginManager::new();
        register_dynamic_probe(&mut manager);

        let error = manager
            .invoke_credentialed(
                "probe",
                vec!["run".to_owned()],
                GlobalOptionsWire {
                    json: false,
                    quiet: false,
                    limit: None,
                },
                &BTreeMap::from([("database".to_owned(), "app-db".to_owned())]),
            )
            .expect_err("no resolver means no credential");

        assert!(matches!(error, RuntimeError::VaultKeyUnavailable { .. }));
    }

    impl BuiltinPlugin for EchoBuiltinPlugin {
        fn metadata(&self) -> PluginMetadata {
            PluginMetadata {
                plugin_name: "builtin-echo".to_owned(),
                domain: "echo".to_owned(),
                description: "echo test plugin".to_owned(),
                abi_version: AH_PLUGIN_ABI_VERSION,
                required_tools: Vec::new(),
                compatibility: ah_plugin_api::PluginCompatibility::current()
                    .with_capability(plugin_capabilities::TYPED_COMMANDS_V1),
            }
        }

        fn manual(&self) -> PluginManual {
            PluginManual {
                plugin_name: "builtin-echo".to_owned(),
                domain: "echo".to_owned(),
                description: "echo test plugin".to_owned(),
                commands: Vec::new(),
                notes: vec!["test manual".to_owned()],
            }
        }

        fn invoke(&self, _request: &InvocationRequest) -> InvocationResponse {
            InvocationResponse::ok(Some("ok".to_owned()))
        }

        fn invoke_observed(&self, request: &InvocationRequest) -> InvocationObservation {
            InvocationObservation::new(
                self.invoke(request),
                InvocationOutcome::RunCheck(RunCheckOutcome {
                    success: false,
                    timed_out: false,
                    exit_code: Some(7),
                }),
            )
        }

        fn command_catalog(&self) -> Option<CommandCatalog> {
            Some(CommandCatalog::new(
                "builtin-echo",
                "echo",
                vec![CommandDescriptor::new(
                    "echo.value",
                    "Echo value",
                    "Echo one value. Impact: reads the supplied value only.",
                    serde_json::json!({
                        "type": "object",
                        "properties": {
                            "value": { "type": "string" }
                        },
                        "required": ["value"],
                        "additionalProperties": false
                    }),
                    serde_json::json!({
                        "type": "object",
                        "properties": {
                            "value": { "type": "string" }
                        },
                        "required": ["value"],
                        "additionalProperties": false
                    }),
                    ah_plugin_api::CommandEffects::new(
                        true,
                        false,
                        true,
                        false,
                        vec![ah_plugin_api::CommandEffect::ConfigurationRead],
                        ah_plugin_api::RiskLevel::Low,
                        "Reads the supplied value only.",
                        ah_plugin_api::Reversibility::Yes,
                    ),
                )],
            ))
        }

        fn invoke_typed(&self, request: &TypedInvocationRequest) -> TypedInvocationResponse {
            TypedInvocationResponse::success(request.arguments.clone(), None)
        }

        fn cancel_typed(&self, request_id: &str) -> bool {
            request_id == "cancel-me"
        }
    }

    #[test]
    fn load_dynamic_plugins_missing_dir_returns_empty_report() {
        let mut manager = PluginManager::new();
        let missing = unique_temp_path("missing");
        let report = manager.load_dynamic_plugins_from_dir(&missing);

        assert_eq!(report.loaded, 0);
        assert_eq!(report.skipped, 0);
        assert!(report.warnings.is_empty());
    }

    #[test]
    fn reserved_dynamic_domains_are_normalized() {
        let mut manager = PluginManager::new();
        manager.reserve_dynamic_domains(["AI", " plugins ", "mcp"]);

        assert!(manager.reserved_dynamic_domains.contains("ai"));
        assert!(manager.reserved_dynamic_domains.contains("plugins"));
        assert!(manager.reserved_dynamic_domains.contains("mcp"));
    }

    #[test]
    fn load_dynamic_plugins_skips_invalid_library_file() {
        let mut manager = PluginManager::new();
        let dir = unique_temp_path("invalid-plugin");
        fs::create_dir_all(&dir).expect("temp plugin dir should be created");
        let lib_path = dir.join(format!("broken.{}", dynamic_lib_extension()));
        fs::write(&lib_path, "not a dynamic library").expect("test plugin file should be written");

        let report = manager.load_dynamic_plugins_from_dir(&dir);
        assert_eq!(report.loaded, 0);
        assert_eq!(report.skipped, 1);
        assert_eq!(report.warnings.len(), 1);
        assert_eq!(report.warnings[0].path, lib_path);
        assert!(manager.list_plugins().is_empty());

        fs::remove_dir_all(&dir).expect("temp plugin dir should be removed");
    }

    #[test]
    fn builtin_invocation_works_after_skipped_dynamic_plugin() {
        let mut manager = PluginManager::new();
        manager.register_builtin(Arc::new(EchoBuiltinPlugin));

        let dir = unique_temp_path("invalid-plugin-with-builtin");
        fs::create_dir_all(&dir).expect("temp plugin dir should be created");
        let lib_path = dir.join(format!("broken.{}", dynamic_lib_extension()));
        fs::write(&lib_path, "not a dynamic library").expect("test plugin file should be written");

        let report = manager.load_dynamic_plugins_from_dir(&dir);
        assert_eq!(report.loaded, 0);
        assert_eq!(report.skipped, 1);

        let response = manager
            .invoke(
                "echo",
                Vec::new(),
                GlobalOptionsWire {
                    json: false,
                    quiet: false,
                    limit: None,
                },
            )
            .expect("builtin plugin should still be invokable");
        assert!(response.success);
        assert_eq!(response.message.as_deref(), Some("ok"));

        fs::remove_dir_all(&dir).expect("temp plugin dir should be removed");
    }

    #[test]
    fn builtin_observation_is_available_without_changing_legacy_response() {
        let mut manager = PluginManager::new();
        manager.register_builtin(Arc::new(EchoBuiltinPlugin));
        let globals = GlobalOptionsWire {
            json: false,
            quiet: false,
            limit: None,
        };

        let observation = manager
            .invoke_observed("echo", Vec::new(), globals.clone())
            .expect("builtin observation should be returned");
        assert!(observation.response.success);
        assert_eq!(
            observation.outcome,
            Some(InvocationOutcome::RunCheck(RunCheckOutcome {
                success: false,
                timed_out: false,
                exit_code: Some(7),
            }))
        );

        let response = manager
            .invoke("echo", Vec::new(), globals)
            .expect("legacy invocation should remain available");
        assert!(response.success);
        assert_eq!(response.message.as_deref(), Some("ok"));
    }

    #[test]
    fn builtin_typed_catalog_and_invocation_work() {
        let mut manager = PluginManager::new();
        manager.register_builtin(Arc::new(EchoBuiltinPlugin));

        let commands = manager
            .list_enabled_commands()
            .expect("typed catalog should load");
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].descriptor.id, "echo.value");

        let request = TypedInvocationRequest::new(
            "echo.value",
            serde_json::json!({ "value": "hello" }),
            ah_plugin_api::ExecutionContextWire::new("request-1", ".", None, 1_000),
        );
        let response = manager
            .invoke_typed(&request)
            .expect("typed invocation should work");
        assert_eq!(response.data, Some(serde_json::json!({ "value": "hello" })));
        assert!(manager.cancel_typed("echo.value", "cancel-me"));
    }

    #[test]
    fn resolver_keeps_public_id_and_injects_private_secret() {
        let resolver = Arc::new(FixedSecretResolver {
            calls: AtomicUsize::new(0),
        });
        let (manager, received) = probe_manager(false, Some(resolver.clone()), false);

        manager
            .invoke_typed(&TypedInvocationRequest::new(
                "probe.run",
                serde_json::json!({"credentials": {"database": "qa-lms"}}),
                ah_plugin_api::ExecutionContextWire::new("resolver-test", ".", None, 1_000),
            ))
            .expect("credential should resolve before builtin dispatch");

        let request = received.lock().unwrap().clone().unwrap();
        assert_eq!(request.arguments["credentials"]["database"], "qa-lms");
        assert_eq!(request.resolved_secrets["database"].id, "qa-lms");
        assert_eq!(request.resolved_secrets["database"].kind, "postgres");
        assert_eq!(
            request.resolved_secrets["database"].values["password"],
            "private-password"
        );
        assert_eq!(resolver.calls.load(AtomicOrdering::Relaxed), 1);
    }

    #[test]
    fn resolver_injects_before_host_dispatch() {
        let resolver = Arc::new(FixedSecretResolver {
            calls: AtomicUsize::new(0),
        });
        let (manager, received) = probe_manager(false, Some(resolver), true);

        manager
            .invoke_typed(&TypedInvocationRequest::new(
                "probe.run",
                serde_json::json!({"credentials": {"database": "qa-lms"}}),
                ah_plugin_api::ExecutionContextWire::new("host-resolver-test", ".", None, 1_000),
            ))
            .unwrap();

        assert_eq!(
            received.lock().unwrap().as_ref().unwrap().resolved_secrets["database"].id,
            "qa-lms"
        );
    }

    #[test]
    fn resolver_injects_before_dynamic_plugin_dispatch() {
        let resolver = Arc::new(FixedSecretResolver {
            calls: AtomicUsize::new(0),
        });
        let mut manager = PluginManager::new();
        manager.set_secret_resolver(resolver);
        register_dynamic_probe(&mut manager);
        *DYNAMIC_PROBE_REQUEST
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap() = None;

        manager
            .invoke_typed(&TypedInvocationRequest::new(
                "probe.run",
                serde_json::json!({"credentials": {"database": "qa-lms"}}),
                ah_plugin_api::ExecutionContextWire::new("dynamic-wire", ".", None, 1_000),
            ))
            .unwrap();
        let request = DYNAMIC_PROBE_REQUEST
            .get()
            .unwrap()
            .lock()
            .unwrap()
            .clone()
            .unwrap();

        assert_eq!(request.arguments["credentials"]["database"], "qa-lms");
        assert_eq!(request.resolved_secrets["database"].kind, "postgres");
    }

    #[test]
    fn resolver_is_not_touched_for_commands_without_slots() {
        let resolver = Arc::new(FixedSecretResolver {
            calls: AtomicUsize::new(0),
        });
        let mut manager = PluginManager::new();
        manager.set_secret_resolver(resolver.clone());
        manager.register_builtin(Arc::new(EchoBuiltinPlugin));

        manager
            .invoke_typed(&TypedInvocationRequest::new(
                "echo.value",
                serde_json::json!({"value": "hello"}),
                ah_plugin_api::ExecutionContextWire::new("no-slots", ".", None, 1_000),
            ))
            .unwrap();

        assert_eq!(resolver.calls.load(AtomicOrdering::Relaxed), 0);
    }

    #[test]
    fn caller_supplied_resolved_secrets_are_discarded_before_dispatch() {
        let injected = ResolvedSecret {
            id: "caller-controlled".to_owned(),
            kind: "postgres".to_owned(),
            values: BTreeMap::from([("password".to_owned(), "caller-private-value".to_owned())]),
        };

        let no_slot_received = Arc::new(Mutex::new(None));
        let mut no_slot_manager = PluginManager::new();
        no_slot_manager.register_builtin(Arc::new(SecretProbePlugin {
            received: Arc::clone(&no_slot_received),
            required: false,
            include_slot: false,
        }));
        no_slot_manager
            .invoke_typed(
                &TypedInvocationRequest::new(
                    "probe.run",
                    serde_json::json!({}),
                    ah_plugin_api::ExecutionContextWire::new("no-slot-injection", ".", None, 1_000),
                )
                .with_resolved_secrets(BTreeMap::from([("database".to_owned(), injected.clone())])),
            )
            .unwrap();
        assert!(
            no_slot_received
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .resolved_secrets
                .is_empty()
        );

        let (empty_credentials_manager, empty_credentials_received) =
            probe_manager(false, None, false);
        empty_credentials_manager
            .invoke_typed(
                &TypedInvocationRequest::new(
                    "probe.run",
                    serde_json::json!({"credentials": {}}),
                    ah_plugin_api::ExecutionContextWire::new(
                        "empty-credentials-injection",
                        ".",
                        None,
                        1_000,
                    ),
                )
                .with_resolved_secrets(BTreeMap::from([("database".to_owned(), injected)])),
            )
            .unwrap();
        assert!(
            empty_credentials_received
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .resolved_secrets
                .is_empty()
        );
    }

    #[test]
    fn caller_supplied_resolved_secrets_cannot_bypass_id_or_kind_validation() {
        let injected = BTreeMap::from([(
            "database".to_owned(),
            ResolvedSecret {
                id: "caller-controlled".to_owned(),
                kind: "postgres".to_owned(),
                values: BTreeMap::from([(
                    "password".to_owned(),
                    "caller-private-value".to_owned(),
                )]),
            },
        )]);

        let missing_resolver: Arc<dyn SecretResolver> =
            Arc::new(ErrorSecretResolver(SecretResolverError::NotFound));
        let (missing_manager, missing_received) =
            probe_manager(false, Some(missing_resolver), false);
        let missing = missing_manager
            .invoke_typed(
                &TypedInvocationRequest::new(
                    "probe.run",
                    serde_json::json!({"credentials": {"database": "missing-id"}}),
                    ah_plugin_api::ExecutionContextWire::new(
                        "injected-missing-id",
                        ".",
                        None,
                        1_000,
                    ),
                )
                .with_resolved_secrets(injected.clone()),
            )
            .unwrap_err();
        assert!(matches!(missing, RuntimeError::SecretNotFound { .. }));
        assert!(missing_received.lock().unwrap().is_none());

        let wrong_kind_resolver: Arc<dyn SecretResolver> = Arc::new(WrongKindSecretResolver);
        let (wrong_kind_manager, wrong_kind_received) =
            probe_manager(false, Some(wrong_kind_resolver), false);
        let wrong_kind = wrong_kind_manager
            .invoke_typed(
                &TypedInvocationRequest::new(
                    "probe.run",
                    serde_json::json!({"credentials": {"database": "wrong-kind-id"}}),
                    ah_plugin_api::ExecutionContextWire::new(
                        "injected-wrong-kind",
                        ".",
                        None,
                        1_000,
                    ),
                )
                .with_resolved_secrets(injected),
            )
            .unwrap_err();
        assert!(matches!(
            wrong_kind,
            RuntimeError::SecretKindMismatch { .. }
        ));
        assert!(wrong_kind_received.lock().unwrap().is_none());
    }

    #[test]
    fn resolver_returns_deterministic_redacted_errors() {
        let (required_manager, _) = probe_manager(true, None, false);
        let schema_error = required_manager
            .invoke_typed(&TypedInvocationRequest::new(
                "probe.run",
                serde_json::json!({"unexpected": true}),
                ah_plugin_api::ExecutionContextWire::new("schema-first", ".", None, 1_000),
            ))
            .unwrap_err();
        assert!(matches!(schema_error, RuntimeError::TypedInvocation(_)));

        let undeclared = required_manager
            .invoke_typed(&TypedInvocationRequest::new(
                "probe.run",
                serde_json::json!({"credentials": {"other": "qa-lms"}}),
                ah_plugin_api::ExecutionContextWire::new("undeclared", ".", None, 1_000),
            ))
            .unwrap_err();
        assert!(undeclared.to_string().contains("undeclared slot 'other'"));

        let required = required_manager
            .invoke_typed(&TypedInvocationRequest::new(
                "probe.run",
                serde_json::json!({}),
                ah_plugin_api::ExecutionContextWire::new("required", ".", None, 1_000),
            ))
            .unwrap_err();
        assert!(matches!(required, RuntimeError::SecretRequired { .. }));

        let missing_resolver: Arc<dyn SecretResolver> =
            Arc::new(ErrorSecretResolver(SecretResolverError::NotFound));
        let (missing_manager, _) = probe_manager(false, Some(missing_resolver), false);
        let missing = missing_manager
            .invoke_typed(&TypedInvocationRequest::new(
                "probe.run",
                serde_json::json!({"credentials": {"database": "missing-id"}}),
                ah_plugin_api::ExecutionContextWire::new("missing", ".", None, 1_000),
            ))
            .unwrap_err();
        assert!(matches!(missing, RuntimeError::SecretNotFound { .. }));

        let wrong_kind_resolver: Arc<dyn SecretResolver> = Arc::new(WrongKindSecretResolver);
        let (wrong_kind_manager, _) = probe_manager(false, Some(wrong_kind_resolver), false);
        let wrong_kind = wrong_kind_manager
            .invoke_typed(&TypedInvocationRequest::new(
                "probe.run",
                serde_json::json!({"credentials": {"database": "api-basic"}}),
                ah_plugin_api::ExecutionContextWire::new("wrong-kind", ".", None, 1_000),
            ))
            .unwrap_err();
        assert!(matches!(
            &wrong_kind,
            RuntimeError::SecretKindMismatch { .. }
        ));
        assert!(!wrong_kind.to_string().contains("redaction-sentinel"));

        let (unavailable_manager, _) = probe_manager(false, None, false);
        let unavailable = unavailable_manager
            .invoke_typed(&TypedInvocationRequest::new(
                "probe.run",
                serde_json::json!({"credentials": {"database": "qa-lms"}}),
                ah_plugin_api::ExecutionContextWire::new("unavailable", ".", None, 1_000),
            ))
            .unwrap_err();
        assert!(matches!(
            unavailable,
            RuntimeError::VaultKeyUnavailable { .. }
        ));

        let locked_resolver: Arc<dyn SecretResolver> =
            Arc::new(ErrorSecretResolver(SecretResolverError::VaultLocked));
        let (locked_manager, _) = probe_manager(false, Some(locked_resolver), false);
        let locked = locked_manager
            .invoke_typed(&TypedInvocationRequest::new(
                "probe.run",
                serde_json::json!({"credentials": {"database": "qa-lms"}}),
                ah_plugin_api::ExecutionContextWire::new("locked", ".", None, 1_000),
            ))
            .unwrap_err();
        assert!(matches!(locked, RuntimeError::VaultLocked { .. }));
    }

    #[test]
    fn typed_registry_is_built_once_per_definition_revision() {
        let mut manager = PluginManager::new();
        manager.register_builtin(Arc::new(EchoBuiltinPlugin));
        assert_eq!(manager.registry_build_count(), 0);

        manager.list_enabled_commands().unwrap();
        assert_eq!(manager.registry_build_count(), 1);
        let request = TypedInvocationRequest::new(
            "echo.value",
            serde_json::json!({"value": "hello"}),
            ah_plugin_api::ExecutionContextWire::new("request-1", ".", None, 1_000),
        );
        manager.invoke_typed(&request).unwrap();
        assert!(manager.cancel_typed("echo.value", "cancel-me"));
        assert_eq!(manager.registry_build_count(), 1);

        manager.register_builtin(Arc::new(EchoBuiltinPlugin));
        manager.list_enabled_commands().unwrap();
        assert_eq!(manager.registry_build_count(), 2);
    }

    #[test]
    fn catalog_revision_changes_only_for_real_enabled_state_mutations() {
        let mut manager = PluginManager::new();
        manager.register_builtin(Arc::new(EchoBuiltinPlugin));
        let initial_revision = manager.catalog_revision();

        manager.set_disabled_domains(Vec::new());
        assert_eq!(manager.catalog_revision(), initial_revision);

        manager.set_disabled_domains(vec!["ECHO".to_owned()]);
        let disabled_revision = manager.catalog_revision();
        assert!(disabled_revision > initial_revision);
        assert!(manager.list_enabled_commands().unwrap().is_empty());
        assert_eq!(manager.registry_build_count(), 1);

        manager.set_disabled_domains(vec!["echo".to_owned()]);
        assert_eq!(manager.catalog_revision(), disabled_revision);
        manager.set_disabled_domains(Vec::new());
        assert!(manager.catalog_revision() > disabled_revision);
        assert_eq!(manager.list_enabled_commands().unwrap().len(), 1);
        assert_eq!(manager.registry_build_count(), 1);
    }

    #[test]
    fn builtin_typed_invocation_rejects_invalid_arguments() {
        let mut manager = PluginManager::new();
        manager.register_builtin(Arc::new(EchoBuiltinPlugin));
        let request = TypedInvocationRequest::new(
            "echo.value",
            serde_json::json!({ "unexpected": true }),
            ah_plugin_api::ExecutionContextWire::new("request-1", ".", None, 1_000),
        );

        let error = manager
            .invoke_typed(&request)
            .expect_err("invalid arguments should fail");
        assert!(matches!(error, RuntimeError::TypedInvocation(_)));
    }

    #[test]
    fn collect_plugin_manuals_includes_builtin_plugins() {
        let mut manager = PluginManager::new();
        manager.register_builtin(Arc::new(EchoBuiltinPlugin));

        let manuals = manager.collect_plugin_manuals();
        assert_eq!(manuals.len(), 1);
        assert_eq!(manuals[0].domain, "echo");
        assert_eq!(manuals[0].plugin_name, "builtin-echo");
    }

    #[test]
    fn disabled_domain_blocks_invocation_and_manuals() {
        let mut manager = PluginManager::new();
        manager.register_builtin(Arc::new(EchoBuiltinPlugin));
        manager.set_disabled_domains(vec!["echo".to_owned()]);

        let invoke = manager.invoke(
            "echo",
            Vec::new(),
            GlobalOptionsWire {
                json: false,
                quiet: false,
                limit: None,
            },
        );
        let Err(RuntimeError::DomainDisabled(domain)) = invoke else {
            panic!("expected domain disabled error");
        };
        assert_eq!(domain, "echo");

        assert!(manager.collect_plugin_manuals().is_empty());
        assert!(manager.list_enabled_plugins().is_empty());
        assert_eq!(manager.list_plugins().len(), 1);
    }

    #[test]
    fn dynamic_domain_conflicts_record_sources_and_winner() {
        let mut report = PluginLoadReport::default();
        let new_dynamic = test_metadata("external-echo-v2", "echo");
        let existing_dynamic = test_metadata("external-echo-v1", "echo");
        let existing_builtin = test_metadata("builtin-echo", "echo");

        push_dynamic_plugin_conflicts(
            &mut report,
            "echo",
            &new_dynamic,
            Some(&existing_dynamic),
            Some(existing_builtin),
        );

        assert_eq!(report.conflicts.len(), 2);
        assert_eq!(report.conflicts[0].domain, "echo");
        assert_eq!(report.conflicts[0].winner.plugin_name, "external-echo-v2");
        assert_eq!(report.conflicts[0].loser.plugin_name, "external-echo-v1");
        assert_eq!(report.conflicts[0].winner_source, PluginSource::Dynamic);
        assert_eq!(report.conflicts[0].loser_source, PluginSource::Dynamic);
        assert!(report.conflicts[0].reason.contains("last loaded"));

        assert_eq!(report.conflicts[1].winner.plugin_name, "external-echo-v2");
        assert_eq!(report.conflicts[1].loser.plugin_name, "builtin-echo");
        assert_eq!(report.conflicts[1].winner_source, PluginSource::Dynamic);
        assert_eq!(report.conflicts[1].loser_source, PluginSource::Builtin);
        assert!(report.conflicts[1].reason.contains("shadows builtin"));
    }

    #[test]
    fn plugin_api_contract_rejects_unsupported_major_version() {
        let mut metadata = test_metadata("external-echo", "echo");
        metadata.compatibility.api_version = ah_plugin_api::PluginApiVersion { major: 2, minor: 0 };

        let error = validate_plugin_api_contract(
            Path::new("plugin.dll"),
            &metadata,
            false,
            TypedSymbolAvailability::default(),
        )
        .expect_err("unsupported major version should fail");
        let RuntimeError::ApiVersionMismatch {
            found_major,
            found_minor,
            supported_major,
            supported_minor,
            ..
        } = error
        else {
            panic!("expected API version mismatch");
        };
        assert_eq!(found_major, 2);
        assert_eq!(found_minor, 0);
        assert_eq!(supported_major, AH_PLUGIN_API_MAJOR_VERSION);
        assert_eq!(supported_minor, AH_PLUGIN_API_MINOR_VERSION);
    }

    #[test]
    fn plugin_metadata_contract_rejects_name_domain_and_abi_mismatch() {
        let metadata = test_metadata("external-echo", "echo");

        let name_error = validate_plugin_metadata_contract(
            Path::new("plugin.dll"),
            AH_PLUGIN_ABI_VERSION,
            "external-other",
            "echo",
            "echo test plugin",
            &metadata,
            false,
            TypedSymbolAvailability::default(),
        )
        .expect_err("name mismatch should fail");
        assert!(name_error.to_string().contains("plugin_name"));

        let domain_error = validate_plugin_metadata_contract(
            Path::new("plugin.dll"),
            AH_PLUGIN_ABI_VERSION,
            "external-echo",
            "other",
            "echo test plugin",
            &metadata,
            false,
            TypedSymbolAvailability::default(),
        )
        .expect_err("domain mismatch should fail");
        assert!(domain_error.to_string().contains("domain"));

        let abi_error = validate_plugin_metadata_contract(
            Path::new("plugin.dll"),
            AH_PLUGIN_ABI_VERSION + 1,
            "external-echo",
            "echo",
            "echo test plugin",
            &metadata,
            false,
            TypedSymbolAvailability::default(),
        )
        .expect_err("ABI mismatch should fail");
        assert!(abi_error.to_string().contains("abi_version"));
    }

    #[test]
    fn plugin_api_contract_rejects_missing_manual_symbol_when_capability_declared() {
        let mut metadata = test_metadata("external-echo", "echo");
        metadata.compatibility = ah_plugin_api::PluginCompatibility::current()
            .with_capability(plugin_capabilities::MANUAL_JSON);

        let error = validate_plugin_api_contract(
            Path::new("plugin.dll"),
            &metadata,
            false,
            TypedSymbolAvailability::default(),
        )
        .expect_err("declared manual_json capability without symbol should fail");
        assert!(error.to_string().contains(plugin_capabilities::MANUAL_JSON));
    }

    #[test]
    fn plugin_api_contract_requires_complete_typed_symbols() {
        let mut metadata = test_metadata("external-echo", "echo");
        metadata.compatibility = ah_plugin_api::PluginCompatibility::current()
            .with_capability(plugin_capabilities::TYPED_COMMANDS_V1);

        let error = validate_plugin_api_contract(
            Path::new("plugin.dll"),
            &metadata,
            false,
            TypedSymbolAvailability {
                catalog: true,
                invoke: false,
                cancel: false,
            },
        )
        .expect_err("partial typed symbols should fail");
        assert!(
            error
                .to_string()
                .contains("ah_plugin_invoke_command_json_v1")
        );
        assert!(error.to_string().contains("ah_plugin_cancel_command_v1"));
    }

    #[test]
    fn plugin_api_contract_rejects_unadvertised_typed_symbols() {
        let metadata = test_metadata("external-echo", "echo");
        let error = validate_plugin_api_contract(
            Path::new("plugin.dll"),
            &metadata,
            false,
            TypedSymbolAvailability {
                catalog: true,
                invoke: true,
                cancel: true,
            },
        )
        .expect_err("unadvertised typed symbols should fail");
        assert!(
            error
                .to_string()
                .contains(plugin_capabilities::TYPED_COMMANDS_V1)
        );
    }

    fn unique_temp_path(suffix: &str) -> PathBuf {
        let ticks = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be valid")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "ah-runtime-{suffix}-{ticks}-{}",
            std::process::id()
        ))
    }

    fn dynamic_lib_extension() -> &'static str {
        if cfg!(windows) {
            "dll"
        } else if cfg!(target_os = "macos") {
            "dylib"
        } else {
            "so"
        }
    }
    /// Credential ids, credential kinds and accepted-kind lists appear in
    /// `Display` but must never reach a diagnostic — neither the host-facing
    /// one nor the MCP-facing one, which is serialized into the tool response.
    #[test]
    fn secret_diagnostics_redact_credential_details_on_every_surface() {
        let id = "runtime-private-credential-id";
        let kind = "runtime-unexpected-private-kind";
        let accepted = "runtime-accepted-private-kind";
        let errors = [
            RuntimeError::SecretNotFound {
                command: "http.get".to_owned(),
                slot: "basic".to_owned(),
                id: id.to_owned(),
            },
            RuntimeError::SecretKindMismatch {
                command: "http.get".to_owned(),
                slot: "basic".to_owned(),
                id: id.to_owned(),
                kind: kind.to_owned(),
                accepted_kinds: vec![accepted.to_owned()],
            },
            RuntimeError::VaultLocked {
                command: "http.get".to_owned(),
                slot: "basic".to_owned(),
                id: id.to_owned(),
            },
            RuntimeError::VaultKeyUnavailable {
                command: "http.get".to_owned(),
                slot: "basic".to_owned(),
                id: id.to_owned(),
            },
        ];

        for error in errors {
            assert!(
                error.to_string().contains(id),
                "the test only proves something if Display still leaks: {error}"
            );
            let diagnostic = error.diagnostic();
            let command_error = error.command_error();
            for secret in [id, kind, accepted] {
                for field in [
                    &diagnostic.message,
                    &diagnostic.cause,
                    &command_error.message,
                    &command_error.cause,
                ] {
                    assert!(!field.contains(secret), "'{secret}' leaked into '{field}'");
                }
            }
        }
    }

    /// The MCP wire contract froze code strings that differ from the host's,
    /// and a retryability flag the host has no concept of.
    #[test]
    fn command_errors_keep_the_frozen_mcp_contract() {
        let cases = [
            (
                RuntimeError::ResponseParse("bad json".to_owned()),
                "PLUGIN_RESPONSE_PARSE_FAILED",
                "PLUGIN_RESPONSE_INVALID",
                false,
            ),
            (
                RuntimeError::TypedCommandNotFound("probe.run".to_owned()),
                "TYPED_COMMAND_NOT_FOUND",
                "COMMAND_NOT_FOUND",
                true,
            ),
            (
                RuntimeError::ExecutionCancelled {
                    request_id: "req-1".to_owned(),
                },
                "EXECUTION_CANCELLED",
                "CANCELLED",
                false,
            ),
            (
                RuntimeError::ExecutionTimeout {
                    request_id: "req-1".to_owned(),
                },
                "EXECUTION_TIMEOUT",
                "TIMEOUT",
                true,
            ),
            (
                RuntimeError::ExecutionPanic {
                    request_id: "req-1".to_owned(),
                },
                "EXECUTION_HANDLER_PANIC",
                "HANDLER_PANIC",
                false,
            ),
            (
                RuntimeError::ExecutionCapacityFull { capacity: 4 },
                "EXECUTION_CAPACITY_FULL",
                "EXECUTION_CAPACITY_FULL",
                true,
            ),
            (
                RuntimeError::DomainDisabled("git".to_owned()),
                "DOMAIN_DISABLED",
                "DOMAIN_DISABLED",
                false,
            ),
        ];

        for (error, host_code, agent_code, retryable) in cases {
            let diagnostic = error.diagnostic();
            // Exercise the `From` impls the call sites actually use.
            let command_error = CommandError::from(error);
            assert_eq!(diagnostic.code, host_code);
            assert_eq!(command_error.code, agent_code);
            assert_eq!(command_error.retryable, retryable, "for {agent_code}");
        }
    }

    /// Not-found and disabled keep the exit hint an MCP client sees, while the
    /// host diagnostic keeps the hint that matches `AppError::exit_code`.
    #[test]
    fn exit_code_hints_stay_per_surface() {
        let error = RuntimeError::DomainDisabled("git".to_owned());
        assert_eq!(error.diagnostic().exit_code_hint, 1);
        assert_eq!(error.command_error().exit_code_hint, 2);

        let error = RuntimeError::ExecutionWorker("worker gone".to_owned());
        assert_eq!(error.diagnostic().exit_code_hint, 1);
        assert_eq!(error.command_error().exit_code_hint, 1);
    }
}
