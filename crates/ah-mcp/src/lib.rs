//! MCP stdio adapter for AIHelper typed commands.

mod server;

pub use server::{McpAdapterError, McpServer, McpServerConfig, serve_stdio};
