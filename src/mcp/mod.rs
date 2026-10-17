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
/// Upper bound on the storage health check inside `initialize`.
const INITIALIZE_HEALTH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
/// MCP protocol versions this server speaks, latest first. 2025-03-26 is
/// deliberately absent: it requires receiving JSON-RPC batches.
const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2024-11-05"];

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

            // MCP spec: notifications have no "id" — the server must not send a
            // response. Only a *valid* notification qualifies: a malformed
            // message without an id (`{}`, a batch array) still gets -32600.
            let is_notification =
                request_value.get("id").is_none() && validate_request(&request_value).is_ok();

            let id = request_value.get("id").cloned().unwrap_or(Value::Null);
            let response = self.handle_request(request_value).await;
            let response_json = match response {
                Ok(res) => res,
                Err(e) => {
                    error!("Error handling request: {}", e);
                    json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {"code": ERROR_INTERNAL_ERROR, "message": e.to_string()},
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

    /// Handle a single JSON-RPC 2.0 request and return a response. A message
    /// that is not a valid request (including a batch array) yields a -32600
    /// error response.
    pub async fn handle_request(&self, request: Value) -> Result<Value> {
        if let Err(response) = validate_request(&request) {
            return Ok(response);
        }
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let method = request["method"].as_str().unwrap_or_default();
        let params = request.get("params").cloned().unwrap_or(Value::Null);

        match method {
            "initialize" => {
                info!("MCP client initialized");
                Ok(self.handle_initialize(id, &params).await)
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
    async fn handle_initialize(&self, id: Value, params: &Value) -> Value {
        // Version negotiation: echo the client's version when supported,
        // otherwise answer with our latest and let the client decide.
        let requested = params.get("protocolVersion").and_then(Value::as_str);
        let protocol_version = SUPPORTED_PROTOCOL_VERSIONS
            .iter()
            .find(|v| Some(**v) == requested)
            .unwrap_or(&SUPPORTED_PROTOCOL_VERSIONS[0]);
        // Bounded: the handshake must not wait out the pool's acquire timeout
        // while the database is down. A slow answer just reports "degraded".
        let storage_healthy =
            tokio::time::timeout(INITIALIZE_HEALTH_TIMEOUT, self.state.db.health())
                .await
                .unwrap_or(false);
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": protocol_version,
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

        // MCP tools spec: an unknown tool is a protocol error (-32602), not a
        // tool execution error with isError.
        let known = tools::list_tools()
            .as_array()
            .is_some_and(|tools| tools.iter().any(|t| t["name"] == name));
        if !known {
            return Ok(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": ERROR_INVALID_PARAMS, "message": format!("Unknown tool: {name}")},
            }));
        }

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
                let message = if is_database_unavailable(&e) {
                    format!(
                        "PostgreSQL database is unavailable; the memory MCP server is running in \
                         degraded mode and reconnects automatically once the database is \
                         reachable ({e:#})"
                    )
                } else {
                    format!("{e:#}")
                };
                Ok(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "content": [{"type": "text", "text": json!({"error": message}).to_string()}],
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

/// `Err(-32600 response)` when `msg` is not a valid JSON-RPC 2.0 request or
/// notification. The id is echoed when it is a valid one, else null.
///
/// Every protocol version we advertise lacks batching (2024-11-05 never had
/// it; 2025-06-18 removed it), so a JSON array is an invalid request too.
fn validate_request(msg: &Value) -> Result<(), Value> {
    let invalid = |id: Value, message: &str| {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": ERROR_INVALID_REQUEST, "message": message},
        })
    };
    let Some(obj) = msg.as_object() else {
        let message = if msg.is_array() {
            "JSON-RPC batching is not supported (removed in MCP 2025-06-18)"
        } else {
            "Request must be a JSON object"
        };
        return Err(invalid(Value::Null, message));
    };
    // MCP: a request id is a string or integer and MUST NOT be null.
    let id = match obj.get("id") {
        None => Value::Null,
        Some(id @ (Value::String(_) | Value::Number(_))) => id.clone(),
        Some(_) => return Err(invalid(Value::Null, "id must be a string or number")),
    };
    if obj.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err(invalid(id, "Invalid or missing jsonrpc field"));
    }
    if !obj.get("method").is_some_and(Value::is_string) {
        return Err(invalid(id, "Missing or non-string 'method'"));
    }
    Ok(())
}

/// Whether a tool failure came from not being able to reach PostgreSQL at all
/// (as opposed to a query, data, or argument error).
fn is_database_unavailable(e: &anyhow::Error) -> bool {
    e.chain().any(|cause| {
        matches!(
            cause.downcast_ref::<sqlx::Error>(),
            Some(
                sqlx::Error::PoolTimedOut
                    | sqlx::Error::PoolClosed
                    | sqlx::Error::Io(_)
                    | sqlx::Error::Tls(_)
            )
        )
    })
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
        assert_eq!(response["id"], 1);
        assert!(response.get("result").is_none());
        assert_eq!(
            response["error"]["code"], ERROR_INVALID_PARAMS,
            "MCP tools spec: an unknown tool is a JSON-RPC -32602 protocol error"
        );
        assert_eq!(
            response["error"]["message"],
            "Unknown tool: nonexistent_tool"
        );
    }

    #[tokio::test]
    async fn test_handle_tools_call_known_tool_failure_is_tool_error() {
        let server = McpServer::new(minimal_state());
        // A real tool failing (here: missing required argument) stays a tool
        // execution error with isError, not a protocol error.
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {"name": "dashboard_control", "arguments": {}},
        });
        let response = server.handle_request(request).await.unwrap();
        assert!(response.get("error").is_none(), "{response}");
        assert_eq!(response["result"]["isError"], true, "{response}");
    }

    #[tokio::test]
    async fn test_initialize_negotiates_protocol_version() {
        let server = McpServer::new(minimal_state());
        for (requested, expected) in [
            (json!("2025-06-18"), "2025-06-18"),
            (json!("2024-11-05"), "2024-11-05"),
            // Requires batching, which we don't support: answer with our latest.
            (json!("2025-03-26"), "2025-06-18"),
            (json!("2099-01-01"), "2025-06-18"),
            (Value::Null, "2025-06-18"),
        ] {
            let response = server
                .handle_initialize(json!(1), &json!({"protocolVersion": requested}))
                .await;
            assert_eq!(
                response["result"]["protocolVersion"], expected,
                "requested {requested}"
            );
        }
    }

    #[tokio::test]
    async fn test_invalid_requests_get_invalid_request_error() {
        let server = McpServer::new(minimal_state());
        for (request, expected_id) in [
            (json!({}), Value::Null),
            (json!({"jsonrpc": "2.0", "method": 5}), Value::Null),
            (json!({"id": 3, "method": "ping"}), json!(3)),
            (
                json!({"jsonrpc": "2.0", "id": null, "method": "ping"}),
                Value::Null,
            ),
            (
                json!({"jsonrpc": "2.0", "id": {"a": 1}, "method": "ping"}),
                Value::Null,
            ),
            (
                json!([{"jsonrpc": "2.0", "id": 4, "method": "ping"}]),
                Value::Null,
            ),
            (json!([]), Value::Null),
            (json!("ping"), Value::Null),
        ] {
            let response = server.handle_request(request.clone()).await.unwrap();
            assert_eq!(
                response["error"]["code"], ERROR_INVALID_REQUEST,
                "{request}"
            );
            assert_eq!(response["id"], expected_id, "{request}");
            assert_eq!(response["jsonrpc"], "2.0");
        }
    }

    #[tokio::test]
    async fn test_listen_answers_invalid_and_never_notifications() {
        use tokio::io::AsyncReadExt;
        let server = McpServer::new(minimal_state());
        let input = [
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","method":"ping"}"#,
            r#"{"jsonrpc":"2.0","method":"no/such/notification"}"#,
            r#"{not json"#,
            r#"{}"#,
            r#"[{"jsonrpc":"2.0","id":4,"method":"ping"},{"jsonrpc":"2.0","method":"ping"}]"#,
            r#"{"jsonrpc":"2.0","id":7,"method":"ping"}"#,
        ]
        .join("\n")
            + "\n";
        let (mut out_wr, mut out_rd) = tokio::io::duplex(64 * 1024);
        server.listen(input.as_bytes(), &mut out_wr).await.unwrap();
        drop(out_wr);
        let mut output = String::new();
        out_rd.read_to_string(&mut output).await.unwrap();
        let responses: Vec<Value> = output
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        let codes: Vec<&Value> = responses.iter().map(|r| &r["error"]["code"]).collect();
        assert_eq!(responses.len(), 4, "{output}");
        assert_eq!(*codes[0], ERROR_PARSE_ERROR);
        assert_eq!(*codes[1], ERROR_INVALID_REQUEST);
        // A batch gets one error response, not per-element replies.
        assert_eq!(*codes[2], ERROR_INVALID_REQUEST);
        assert!(responses[..3].iter().all(|r| r["id"].is_null()));
        assert_eq!(responses[3]["id"], 7);
        assert!(responses[3]["result"].is_object());
    }

    #[test]
    fn database_unavailable_is_detected_through_context() {
        let unreachable = anyhow::Error::new(sqlx::Error::PoolTimedOut).context("Search failed");
        assert!(is_database_unavailable(&unreachable));
        let query_error = anyhow::Error::new(sqlx::Error::RowNotFound).context("lookup");
        assert!(!is_database_unavailable(&query_error));
        assert!(!is_database_unavailable(&anyhow::anyhow!(
            "Unknown tool: x"
        )));
    }

    #[tokio::test]
    async fn test_initialize_response_structure() {
        let state = minimal_state();
        let server = McpServer::new(state);

        let response = server.handle_initialize(json!(42), &json!({})).await;
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
