//! MCP stdio and local Streamable HTTP adapter for AIHelper typed commands.

mod events;
mod jobs;
mod server;

pub use server::{
    EventSink, McpAdapterError, McpCommandEvent, McpCommandStatus, McpServeOutcome, McpServer,
    McpServerConfig, serve_http, serve_http_bounded, serve_http_bounded_with_identity_and_listener,
    serve_http_bounded_with_version, serve_stdio, serve_stdio_bounded,
};
