//! MCP stdio server — standalone binary entry point.
//!
//! Implements the Model Context Protocol over stdin/stdout.

use memory_platform::mcp;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_writer(std::io::stderr) // stdout is for MCP protocol
        .init();

    tracing::info!("MCP server starting (scaffolding phase)");

    // Placeholder: MCP server will process stdin/stdout here
    let _server = mcp::McpServer::new();

    tracing::info!("MCP server ready (no tools implemented yet)");

    // Keep alive until stdin closes
    tokio::signal::ctrl_c().await?;
    tracing::info!("MCP server shutting down.");

    Ok(())
}
