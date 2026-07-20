use std::sync::Arc;

use anyhow::Result;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::time::{timeout, Duration};

use crate::AppState;

use super::{McpServer, MAX_LINE_SIZE, STDIN_TIMEOUT_SECS};

// ---------------------------------------------------------------------------
// Transport trait
// ---------------------------------------------------------------------------

/// Abstract transport for MCP — allows both stdio and HTTP backends.
pub trait McpTransport: Send + Sync {
    /// Receive a single JSON-RPC message (as raw JSON string).
    fn recv(&self) -> impl std::future::Future<Output = Result<String>> + Send;
    /// Send a single JSON-RPC response (raw JSON string).
    fn send(&self, response: &str) -> impl std::future::Future<Output = Result<()>> + Send;
}

// ---------------------------------------------------------------------------
// StdioTransport
// ---------------------------------------------------------------------------

/// Stdio-based MCP transport — reads from stdin, writes to stdout.
pub struct StdioTransport;

impl McpTransport for StdioTransport {
    async fn recv(&self) -> Result<String> {
        let mut reader = BufReader::new(tokio::io::stdin());
        let mut line = String::new();
        timeout(
            Duration::from_secs(STDIN_TIMEOUT_SECS),
            reader.read_line(&mut line),
        )
        .await
        .map_err(|_| anyhow::anyhow!("stdin read timed out"))??;

        if line.len() > MAX_LINE_SIZE {
            anyhow::bail!("Request too large");
        }
        Ok(line)
    }

    async fn send(&self, response: &str) -> Result<()> {
        let mut writer = tokio::io::stdout();
        writer.write_all(response.as_bytes()).await?;
        writer.flush().await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// HTTP handler (feature-gated)
// ---------------------------------------------------------------------------

#[cfg(feature = "transport-http")]
/// Handle a single JSON-RPC request received over HTTP and return a response.
pub async fn handle_http_request(state: Arc<AppState>, body: String) -> Result<String> {
    let server = McpServer::new(state);

    let request_value: Value =
        serde_json::from_str(&body).map_err(|e| anyhow::anyhow!("Parse error: {e}"))?;

    let is_notification = request_value.get("id").is_none();
    let response = server.handle_request(request_value).await?;

    // Notifications must not receive a JSON-RPC response.
    if is_notification {
        return Ok(String::new());
    }
    let response_str = serde_json::to_string(&response)?;
    Ok(response_str)
}

/// Handle a single JSON-RPC request over HTTP and return a JSON-RPC response string.
///
/// Used in the Axum endpoint behind the `transport-http` feature flag.
#[cfg(feature = "transport-http")]
pub async fn http_json_rpc_handler(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    body: String,
) -> (axum::http::StatusCode, String) {
    match handle_http_request(state, body).await {
        Ok(response) => {
            if response.is_empty() {
                // Notification — no content
                (axum::http::StatusCode::NO_CONTENT, response)
            } else {
                (axum::http::StatusCode::OK, response)
            }
        }
        Err(e) => {
            let error_response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": null,
                "error": {
                    "code": super::ERROR_INTERNAL_ERROR,
                    "message": e.to_string(),
                },
            });
            (
                axum::http::StatusCode::OK,
                serde_json::to_string(&error_response).unwrap_or_default(),
            )
        }
    }
}
