//! What the runtime needs from its host: a builtin plugin, and somewhere to
//! resolve a secret from.

use super::*;

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
