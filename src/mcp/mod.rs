//! MCP module — Model Context Protocol stdio server.
//!
//! Will implement JSON-RPC 2.0 over stdin/stdout with 8 tools.

/// Placeholder for the MCP server.
pub struct McpServer;

impl McpServer {
    /// Create a new MCP server (scaffolding).
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Default for McpServer {
    fn default() -> Self {
        Self::new()
    }
}
