//! MCP stdio adapter for AIHelper typed commands.

mod server;

pub use server::{
    EventSink, McpAdapterError, McpCommandEvent, McpCommandStatus, McpServer, McpServerConfig,
    serve_stdio,
};
