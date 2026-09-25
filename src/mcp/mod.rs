//! MCP module — Model Context Protocol stdio server.
//!
//! Implements JSON-RPC 2.0 over stdin/stdout with 25 tools plus a small
//! read-only resources surface (`memory://...` URIs).
//! Logs to stderr via `tracing` (stdout is protocol-only).

use std::sync::Arc;

use anyhow::Result;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{debug, error, info, warn};

use crate::AppState;

/// Maximum line size accepted on stdin (10 MiB).
const MAX_LINE_SIZE: usize = 10 * 1024 * 1024;
/// Read timeout for the feature-gated HTTP transport (`transport.rs`) only —
/// the default stdio `listen` loop below has no read timeout, since that pipe
/// is expected to sit idle between tool calls for the life of the client
/// session.
#[cfg_attr(not(feature = "transport-http"), allow(dead_code))]
pub const STDIN_TIMEOUT_SECS: u64 = 900;

// ---------------------------------------------------------------------------
// JSON-RPC 2.0 error codes
// ---------------------------------------------------------------------------

/// Parse error (-32700): Invalid JSON was received by the server.
pub const ERROR_PARSE_ERROR: i64 = -32700;
/// Invalid Request (-32600): The JSON sent is not a valid Request object.
pub const ERROR_INVALID_REQUEST: i64 = -32600;
/// Method not found (-32601): The method does not exist / is not available.
pub const ERROR_METHOD_NOT_FOUND: i64 = -32601;
/// Invalid params (-32602): Invalid method parameter(s).
pub const ERROR_INVALID_PARAMS: i64 = -32602;
/// Internal error (-32603): Internal JSON-RPC error.
pub const ERROR_INTERNAL_ERROR: i64 = -32603;

mod resources;
mod tools;
#[cfg(feature = "transport-http")]
pub mod transport;

pub use tools::*;

/// MCP server — JSON-RPC 2.0 stdio transport.
#[derive(Debug)]
pub struct McpServer {
    /// Application state with DB, search, and services.
    state: Arc<AppState>,
}

impl McpServer {
    /// Create a new MCP server with application state.
    pub fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }

    /// Listen for JSON-RPC 2.0 requests on stdin, write responses to stdout.
    pub async fn listen<R, W>(&self, reader: R, writer: W) -> Result<()>
    where
        R: tokio::io::AsyncRead + Unpin,
        W: tokio::io::AsyncWrite + Unpin,
    {
        let mut reader = BufReader::new(reader);
        let mut writer = writer;
        let mut line = String::new();

        info!("MCP server listening on stdin");

        loop {
            line.clear();

            // No read timeout here: this pipe belongs to the client session for
            // its whole lifetime, and idle gaps between tool calls (minutes to
            // hours in normal agent use) are expected, not exceptional. A
            // timeout that tears down the process on idle would surface to the
            // client as a "transport closed" error on its next call even
            // though nothing went wrong. Real disconnects arrive as bytes_read
            // == 0 (EOF) below, which the OS delivers promptly once the
            // client's end of the pipe actually closes.
            let bytes_read = reader.read_line(&mut line).await?;

            if bytes_read == 0 {
                // EOF
                break;
            }

            if line.len() > MAX_LINE_SIZE {
                error!("Line exceeds maximum size ({MAX_LINE_SIZE} bytes)");
                let error_response = json!({
                    "jsonrpc": "2.0",
                    "id": Value::Null,
                    "error": {"code": ERROR_INVALID_REQUEST, "message": "Request too large"},
                });
                let response_str = error_response.to_string() + "\n";
                writer.write_all(response_str.as_bytes()).await?;
                writer.flush().await?;
                continue;
            }

            debug!("Received request: {}", line.trim());

            let request_value: Value = match serde_json::from_str(line.trim()) {
                Ok(v) => v,
                Err(e) => {
                    error!("Failed to parse request: {}", e);
                    let error_response = json!({
                        "jsonrpc": "2.0",
                        "id": Value::Null,
                        "error": {"code": ERROR_PARSE_ERROR, "message": e.to_string()},
                    });
                    let response_str = error_response.to_string() + "\n";
                    writer.write_all(response_str.as_bytes()).await?;
                    writer.flush().await?;
                    continue;
                }
            };

            // MCP spec: notifications have no "id" — the server must not send a response
            let is_notification = request_value.get("id").is_none();

            let invalid_request = request_value.get("jsonrpc").and_then(Value::as_str)
                != Some("2.0")
                || request_value
                    .get("method")
                    .and_then(Value::as_str)
                    .is_none();
            let response = self.handle_request(request_value).await;
            let response_json = match response {
                Ok(res) => res,
                Err(e) => {
                    error!("Error handling request: {}", e);
                    json!({
                        "jsonrpc": "2.0",
                        "id": Value::Null,
                        "error": {"code": if invalid_request { ERROR_INVALID_REQUEST } else { ERROR_INTERNAL_ERROR }, "message": e.to_string()},
                    })
                }
            };

            if is_notification {
                continue;
            }

            let response_str = response_json.to_string() + "\n";
            writer.write_all(response_str.as_bytes()).await?;
            writer.flush().await?;
        }

        Ok(())
    }

    /// Handle a JSON-RPC request and return a response.
    /// Handle a single JSON-RPC 2.0 request and return a response.
    pub async fn handle_request(&self, request: Value) -> Result<Value> {
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let method = request
            .get("method")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'method' in request"))?
            .to_string();
        let params = request.get("params").cloned().unwrap_or(Value::Null);

        // Validate JSON-RPC 2.0
        if request.get("jsonrpc").and_then(|v| v.as_str()) != Some("2.0") {
            return Err(anyhow::anyhow!("Invalid or missing jsonrpc field"));
        }

        match method.as_str() {
            "initialize" => {
                info!("MCP client initialized");
                Ok(self.handle_initialize(id).await)
            }
            "notifications/initialized" => {
                info!("MCP client initialized (notification)");
                Ok(json!({
                    "jsonrpc": "2.0",
                }))
            }
            "ping" => Ok(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {},
            })),
            "tools/list" => Ok(self.handle_tools_list(id)),
            "tools/call" => self.handle_tools_call(id, params).await,
            "resources/list" => Ok(self.handle_resources_list(id)),
            "resources/templates/list" => Ok(self.handle_resource_templates_list(id)),
            "resources/read" => self.handle_resources_read(id, params).await,
            _ => Ok(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": ERROR_METHOD_NOT_FOUND, "message": "Method not found"},
            })),
        }
    }

    /// Handle "initialize" method.
    async fn handle_initialize(&self, id: Value) -> Value {
        let storage_healthy = self.state.db.health().await;
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": "2025-06-18",
                "capabilities": {
                    "tools": {},
                    "resources": {},
                },
                "serverInfo": {
                    "name": "memory-mcp",
                    "version": "2.0.0",
                },
                "status": {
                    "storage": if storage_healthy { "healthy" } else { "degraded" },
                    "embedding": if self.state.embedding_service.is_some() { "available" } else { "keyword-only" },
                    "configured_embedding_dimension": self.state.config.embedding_dim,
                }
            },
        })
    }

    /// Handle "tools/list" method.
    fn handle_tools_list(&self, id: Value) -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "tools": tools::list_tools(),
            },
        })
    }

    /// Handle "tools/call" method.
    async fn handle_tools_call(&self, id: Value, params: Value) -> Result<Value> {
        let Some(name) = params.get("name").and_then(|v| v.as_str()) else {
            return Ok(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": ERROR_INVALID_PARAMS, "message": "Missing 'name' in tools/call params"},
            }));
        };
        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or(Value::Object(serde_json::Map::new()));

        debug!("Calling tool: {name} with args: {arguments}");

        let result = tools::call_tool(&self.state, name, arguments).await;

        match result {
            Ok(content) => Ok(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "content": [{"type": "text", "text": content}],
                },
            })),
            Err(e) => {
                warn!("Tool call failed: {e:#}");
                Ok(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "content": [{"type": "text", "text": json!({"error": format!("{e:#}")}).to_string()}],
                        "isError": true,
                    },
                }))
            }
        }
    }

    /// Handle "resources/list" method.
    fn handle_resources_list(&self, id: Value) -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "resources": resources::list_resources(),
            },
        })
    }

    /// Handle "resources/templates/list" method.
    fn handle_resource_templates_list(&self, id: Value) -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "resourceTemplates": resources::list_resource_templates(),
            },
        })
    }

    /// Handle "resources/read" method.
    async fn handle_resources_read(&self, id: Value, params: Value) -> Result<Value> {
        let Some(uri) = params.get("uri").and_then(|v| v.as_str()) else {
            return Ok(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": ERROR_INVALID_PARAMS, "message": "Missing 'uri' in resources/read params"},
            }));
        };
        let uri = uri.to_string();

        match resources::read_resource(&self.state, &uri).await {
            Ok(content) => Ok(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "contents": [{
                        "uri": uri,
                        "mimeType": "application/json",
                        "text": content.to_string(),
                    }],
                },
            })),
            Err(e) => {
                warn!("Resource read failed: {e:#}");
                // Distinguish a client-caused error (unknown/malformed URI, a
                // bail!() in resources.rs) from a server-caused one (a wrapped
                // sqlx failure) so the client doesn't see a DB outage reported
                // as "you sent an invalid request".
                let is_db_error = e
                    .chain()
                    .any(|cause| cause.downcast_ref::<sqlx::Error>().is_some());
                let code = if is_db_error {
                    ERROR_INTERNAL_ERROR
                } else {
                    ERROR_INVALID_PARAMS
                };
                Ok(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {"code": code, "message": format!("{e:#}")},
                }))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn minimal_state() -> Arc<AppState> {
        Arc::new(AppState {
            config: crate::config::Config::default(),
            db: Arc::new(crate::db::postgres::PostgresDb::new_empty()),
            search: Arc::new(crate::search::SearchEngine::new_empty()),
            neo4j_client: None,
            redis_cache: None,
            contradiction_detector: None,
            embedding_service: None,
            experience_service: None,
            ingestion_service: None,
            procedure_service: None,
            pending_writes: Arc::new(crate::queue::PendingWriteQueue::new_empty()),
        })
    }

    #[tokio::test]
    async fn test_handle_initialize() {
        let state = minimal_state();
        let server = McpServer::new(state);

        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {},
        });

        let response = server.handle_request(request).await.unwrap();
        assert_eq!(response["id"], 1);
        assert_eq!(response["jsonrpc"], "2.0");
        assert!(response["result"].get("serverInfo").is_some());
    }

    #[tokio::test]
    async fn test_handle_tools_list() {
        let state = minimal_state();
        let server = McpServer::new(state);

        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": {},
        });

        let response = server.handle_request(request).await.unwrap();
        assert_eq!(response["id"], 1);
        assert_eq!(response["jsonrpc"], "2.0");

        let tools = response["result"]["tools"].as_array().unwrap();
        assert!(!tools.is_empty(), "Should return non-empty tools list");
        assert_eq!(tools.len(), 25, "Should return exactly 25 tools");
    }

    #[tokio::test]
    async fn test_handle_resources_list() {
        let state = minimal_state();
        let server = McpServer::new(state);

        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "resources/list",
            "params": {},
        });

        let response = server.handle_request(request).await.unwrap();
        assert_eq!(response["id"], 1);
        let resources = response["result"]["resources"].as_array().unwrap();
        assert_eq!(
            resources.len(),
            3,
            "Should return exactly 3 static resources"
        );
    }

    #[tokio::test]
    async fn test_handle_resource_templates_list() {
        let state = minimal_state();
        let server = McpServer::new(state);

        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "resources/templates/list",
            "params": {},
        });

        let response = server.handle_request(request).await.unwrap();
        assert_eq!(response["id"], 1);
        let templates = response["result"]["resourceTemplates"].as_array().unwrap();
        assert_eq!(
            templates.len(),
            2,
            "Should return exactly 2 resource templates"
        );
    }

    #[tokio::test]
    async fn test_handle_resources_read_missing_uri() {
        let state = minimal_state();
        let server = McpServer::new(state);

        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "resources/read",
            "params": {},
        });

        let response = server.handle_request(request).await.unwrap();
        assert_eq!(response["id"], 1);
        assert_eq!(
            response["error"]["code"], ERROR_INVALID_PARAMS,
            "Missing uri should surface as a JSON-RPC -32602 error, not -32603"
        );
    }

    #[tokio::test]
    async fn test_handle_resources_read_unknown_uri() {
        let state = minimal_state();
        let server = McpServer::new(state);

        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "resources/read",
            "params": {"uri": "memory://nonsense"},
        });

        let response = server.handle_request(request).await.unwrap();
        assert_eq!(response["id"], 1);
        assert!(
            response.get("error").is_some(),
            "Unknown resource URI should return a JSON-RPC error, not a hard failure"
        );
        assert_eq!(response["error"]["code"], ERROR_INVALID_PARAMS);
    }

    #[tokio::test]
    async fn test_handle_unknown_method() {
        let state = minimal_state();
        let server = McpServer::new(state);

        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "nonexistent/method",
            "params": {},
        });

        let response = server.handle_request(request).await.unwrap();
        assert!(
            response.get("error").is_some(),
            "Should return error for unknown method"
        );
        assert_eq!(
            response["error"]["code"], ERROR_METHOD_NOT_FOUND,
            "Error code should be -32601"
        );
        assert_eq!(response["error"]["message"], "Method not found");
    }

    #[tokio::test]
    async fn test_notification_suppresses_response() {
        let state = minimal_state();
        let server = McpServer::new(state);

        // Notification without "id" — server must not send response
        // handle_request still returns a value, but the listen loop skips it
        let request = json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {},
        });
        let response = server.handle_request(request).await.unwrap();
        assert_eq!(response["jsonrpc"], "2.0");
        assert!(
            response.get("id").is_none(),
            "Notification response should have no id"
        );
    }

    #[tokio::test]
    async fn test_handle_tools_call_missing_name() {
        let state = minimal_state();
        let server = McpServer::new(state);

        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {"arguments": {}},
        });

        let response = server.handle_request(request).await.unwrap();
        assert_eq!(response["id"], 1);
        assert_eq!(
            response["error"]["code"], ERROR_INVALID_PARAMS,
            "Missing tool name should surface as a JSON-RPC -32602 error, not -32603"
        );
    }

    #[tokio::test]
    async fn test_handle_tools_call_unknown_tool() {
        let state = minimal_state();
        let server = McpServer::new(state);

        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {"name": "nonexistent_tool", "arguments": {}},
        });

        let response = server.handle_request(request).await.unwrap();
        let content = &response["result"]["content"][0]["text"];
        let content_str = content.as_str().unwrap();
        assert!(
            content_str.contains("error"),
            "Should contain error in content"
        );
        assert!(
            content_str.contains("Unknown tool"),
            "Should mention unknown tool"
        );
        assert_eq!(
            response["result"]["isError"], true,
            "A failed tool call must be flagged isError so clients don't read it as a success"
        );
    }

    #[tokio::test]
    async fn test_initialize_response_structure() {
        let state = minimal_state();
        let server = McpServer::new(state);

        let response = server.handle_initialize(json!(42)).await;
        assert_eq!(response["id"], 42);
        assert_eq!(response["jsonrpc"], "2.0");
        assert_eq!(response["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(response["result"]["serverInfo"]["name"], "memory-mcp");
        assert_eq!(response["result"]["serverInfo"]["version"], "2.0.0");
        assert!(response["result"]["capabilities"].get("tools").is_some());
        assert!(response["result"]["capabilities"]
            .get("resources")
            .is_some());
    }
}
