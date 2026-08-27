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

    /// The same invocation, rendering into the sink the host chose.
    ///
    /// A built-in is Rust, so the host can tell it where its output goes; a
    /// dynamic plugin cannot be told across the C ABI and returns its text in
    /// the response instead. That difference is why built-in output used to be
    /// observable only by spawning `ah`: the domain built its own `stdio`
    /// emitter after parsing, and nothing above it could reach in.
    ///
    /// The default ignores the sink, for a built-in that prints nothing of its
    /// own.
    fn invoke_into(&self, request: &InvocationRequest, sink: &OutputSink) -> InvocationResponse {
        let _ = sink;
        self.invoke(request)
    }

    /// The sink-less shorthand. **Do not override this one**: the runtime calls
    /// [`Self::invoke_observed_into`], so an override here would be bypassed.
    fn invoke_observed(&self, request: &InvocationRequest) -> InvocationObservation {
        self.invoke_observed_into(request, &OutputSink::Process)
    }

    /// For a built-in whose invocation reports an outcome as well as a
    /// response. This is the one to override; the default reports no outcome.
    fn invoke_observed_into(
        &self,
        request: &InvocationRequest,
        sink: &OutputSink,
    ) -> InvocationObservation {
        InvocationObservation::without_outcome(self.invoke_into(request, sink))
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
