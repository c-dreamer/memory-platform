//! MCP module — Model Context Protocol stdio server.
//!
//! Implements JSON-RPC 2.0 over stdin/stdout with 12 tools.
//! Logs to stderr via `tracing` (stdout is protocol-only).

use std::sync::Arc;

use anyhow::Result;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{debug, error, info};

use crate::AppState;

mod hooks;
mod tools;

pub use hooks::*;
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
            let bytes_read = reader.read_line(&mut line).await?;
            
            if bytes_read == 0 {
                // EOF
                break;
            }
            
            debug!("Received request: {}", line.trim());
            
            let request_value: Value = match serde_json::from_str(line.trim()) {
                Ok(v) => v,
                Err(e) => {
                    error!("Failed to parse request: {}", e);
                    let error_response = json!({
                        "jsonrpc": "2.0",
                        "id": Value::Null,
                        "error": {"message": e.to_string()},
                    });
                    let response_str = error_response.to_string() + "\n";
                    writer.write_all(response_str.as_bytes()).await?;
                    writer.flush().await?;
                    continue;
                }
            };

            let response = self.handle_request(request_value).await;
            let response_json = match response {
                Ok(res) => res,
                Err(e) => {
                    error!("Error handling request: {}", e);
                    json!({
                        "jsonrpc": "2.0",
                        "id": Value::Null,
                        "error": {"message": e.to_string()},
                    })
                }
            };

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
            "tools/list" => Ok(self.handle_tools_list(id)),
            "tools/call" => self.handle_tools_call(id, params).await,
            _ => Ok(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"message": "Method not found"},
            })),
        }
    }

    /// Handle "initialize" method.
    async fn handle_initialize(&self, id: Value) -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {
                        "list": true,
                        "call": true,
                    },
                },
                "serverInfo": {
                    "name": "memory-mcp",
                    "version": "2.0.0",
                },
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
        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'name' in tools/call params"))?;
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
            Err(e) => Ok(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "content": [{"type": "text", "text": json!({"error": e.to_string()}).to_string()}],
                },
            })),
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
            context_service: None,
            contradiction_detector: None,
            decay_engine: None,
            embedding_service: None,
            experience_service: None,
            ingestion_service: None,
            procedure_service: None,
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
        assert_eq!(tools.len(), 13, "Should return exactly 13 tools");
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
        assert!(response.get("error").is_some(), "Should return error for unknown method");
        assert_eq!(response["error"]["message"], "Method not found");
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
        
        let result = server.handle_request(request).await;
        assert!(result.is_err(), "Should error on missing tool name");
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
        assert!(content_str.contains("error"), "Should contain error in content");
        assert!(content_str.contains("Unknown tool"), "Should mention unknown tool");
    }

    #[tokio::test]
    async fn test_initialize_response_structure() {
        let state = minimal_state();
        let server = McpServer::new(state);
        
        let response = server.handle_initialize(json!(42)).await;
        assert_eq!(response["id"], 42);
        assert_eq!(response["jsonrpc"], "2.0");
        assert_eq!(response["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(response["result"]["serverInfo"]["name"], "memory-mcp");
        assert_eq!(response["result"]["serverInfo"]["version"], "2.0.0");
        assert!(response["result"]["capabilities"]["tools"]["list"] == true);
        assert!(response["result"]["capabilities"]["tools"]["call"] == true);
    }
}