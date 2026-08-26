//! The state one server shares across its sessions: the peers, the catalog
//! snapshot, and the executions currently in flight.

use super::*;

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
    pub(super) result: Result<CallToolResult, rmcp::ErrorData>,
    pub(super) telemetry: Option<ExecutionTelemetry>,
}

impl ToolCallOutcome {
    pub(crate) fn unobserved(result: Result<CallToolResult, rmcp::ErrorData>) -> Self {
        Self {
            result,
            telemetry: None,
        }
    }
}

pub(crate) struct ActiveExecution<'a> {
    pub(super) active_executions: &'a Mutex<HashMap<String, Vec<String>>>,
    pub(super) executor: Arc<dyn Executor>,
    pub(super) protocol_request_id: String,
    pub(super) execution_id: String,
}
