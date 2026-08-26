//! MCP stdio and local Streamable HTTP adapter for AIHelper typed commands.

mod events;
mod job_tools;
mod jobs;
mod mapping;
mod plaintext_auth;
mod server;
mod shutdown;
mod transport;

/// The secret setup pages are their own crate; they are re-exported here so a
/// caller wiring the HTTP transport still sees one contract.
pub use ah_setup_ui::{
    SecretSetupError, SecretSetupField, SecretSetupForm, SecretSetupMetadata, SecretSetupRequest,
    SecretSetupService,
};
pub use server::{
    EventSink, McpAdapterError, McpCommandEvent, McpCommandStatus, McpServeOutcome, McpServer,
    McpServerConfig,
};
pub use transport::{
    serve_http, serve_http_bounded, serve_http_bounded_with_identity_and_listener,
    serve_http_bounded_with_version, serve_stdio, serve_stdio_bounded,
};
