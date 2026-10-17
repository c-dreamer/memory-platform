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
///
/// Same `X-API-Key` rule as every other API route (the `Auth` extractor).
/// Beyond that, a browser must not be able to drive this endpoint: any web
/// page can POST a CORS "simple request" (no preflight) to loopback, and a
/// DNS-rebound host name makes that request same-origin. So the body must be
/// `Content-Type: application/json` (never a simple request), and a present
/// `Origin` must be a loopback origin. A missing `Origin` (non-browser client)
/// is allowed, per the MCP spec's DNS-rebinding guidance.
#[cfg(feature = "transport-http")]
pub async fn http_json_rpc_handler(
    _auth: crate::api::auth::Auth,
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    body: String,
) -> (axum::http::StatusCode, String) {
    use axum::http::{header, StatusCode};

    if let Some(origin) = headers.get(header::ORIGIN) {
        if !origin.to_str().is_ok_and(is_loopback_origin) {
            return (StatusCode::FORBIDDEN, "Forbidden origin".into());
        }
    }
    let is_json = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|m| m.trim().eq_ignore_ascii_case("application/json"));
    if !is_json {
        return (
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Content-Type must be application/json".into(),
        );
    }

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

/// `http://127.0.0.1[:port]`, `http://localhost[:port]` or `http://[::1][:port]`.
#[cfg(feature = "transport-http")]
fn is_loopback_origin(origin: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(origin) else {
        return false;
    };
    url.scheme() == "http"
        && url.path() == "/"
        && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))
}
