//! MCP Server — integration tests.
//!
//! Tests JSON-RPC 2.0 protocol and tool dispatch via the public API.

use std::sync::Arc;

use memory_platform::{
    config::Config,
    db::postgres::PostgresDb,
    mcp::McpServer,
    search::SearchEngine,
    AppState,
};
use serde_json::{json, Value};

/// Build a minimal AppState with all services disabled.
fn minimal_state() -> Arc<AppState> {
    Arc::new(AppState {
        config: Config::default(),
        db: Arc::new(PostgresDb::new_empty()),
        search: Arc::new(SearchEngine::new_empty()),
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

/// Test that MCP server handles JSON-RPC 2.0 initialize.
#[tokio::test]
async fn test_mcp_initialize() {
    let state = minimal_state();
    let server = McpServer::new(state);

    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {}
    });

    let response = server.handle_request(request).await;
    assert!(response.is_ok(), "Initialize should succeed");
    let resp = response.unwrap();
    assert_eq!(resp["id"], 1);
    assert_eq!(resp["jsonrpc"], "2.0");
    assert!(resp["result"].get("serverInfo").is_some());
}

/// Test that MCP server returns the complete tool set for tools/list.
#[tokio::test]
async fn test_mcp_tools_list() {
    let state = minimal_state();
    let server = McpServer::new(state);

    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": {}
    });

    let response = server.handle_request(request).await;
    assert!(response.is_ok(), "tools/list should succeed");
    let resp = response.unwrap();

    let tools = resp["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 17, "Should return 17 tools");

    let names: Vec<&str> = tools.iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    for name in &["memory_search", "memory_store", "memory_context",
                  "memory_initialize", "experience_find", "procedure_run",
                  "session_start", "session_end", "recall", "forget", "list", "status", "session_context", "archive_status", "memory_health", "dashboard_status", "dashboard_control"] {
        assert!(names.contains(name), "Missing tool: {name}");
    }
}

/// Test that MCP server returns a helpful error for unknown tools.
#[tokio::test]
async fn test_mcp_unknown_tool() {
    let state = minimal_state();
    let server = McpServer::new(state);

    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": "does_not_exist", "arguments": {} }
    });

    let response = server.handle_request(request).await;
    assert!(response.is_ok(), "Unknown tool should still return 200 JSON-RPC");
    let resp = response.unwrap();
    let text = resp["result"]["content"][0]["text"].as_str().unwrap_or("");
    assert!(text.contains("Unknown tool"), "Should mention unknown tool");
}

/// Test that MCP server rejects requests without jsonrpc or id.
#[tokio::test]
async fn test_mcp_malformed_requests() {
    let state = minimal_state();
    let server = McpServer::new(state);

    // Missing jsonrpc
    let r1 = json!({ "id": 1, "method": "initialize" });
    assert!(server.handle_request(r1).await.is_err());

    // A request without an id is a valid notification and receives no response.
    let r2 = json!({ "jsonrpc": "2.0", "method": "initialize" });
    assert!(server.handle_request(r2).await.is_ok());
}

/// Test MCP server listen loop via public API.
#[tokio::test]
async fn test_mcp_listen_loop() {
    let state = minimal_state();
    let server = McpServer::new(state);

    // Use tokio pipe as mock stdin/stdout
    let (mut stdin_wr, stdin_rd) = tokio::io::duplex(4096);
    let (stdout_wr, mut stdout_rd) = tokio::io::duplex(4096);

    let listen = tokio::spawn(async move {
        server.listen(stdin_rd, stdout_wr).await.unwrap();
    });

    // Send initialize request
    let req = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}
    });
    let mut buf = req.to_string().into_bytes();
    buf.push(b'\n');
    stdin_wr.write_all(&buf).await.unwrap();

    // Read response
    let mut line = String::new();
    let mut reader = tokio::io::BufReader::new(&mut stdout_rd);
    reader.read_line(&mut line).await.unwrap();
    let resp: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(resp["id"], 1);
    assert_eq!(resp["jsonrpc"], "2.0");

    // Send tools/list request
    let req2 = json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}
    });
    let mut buf2 = req2.to_string().into_bytes();
    buf2.push(b'\n');
    stdin_wr.write_all(&buf2).await.unwrap();

    let mut line2 = String::new();
    let mut reader2 = tokio::io::BufReader::new(&mut stdout_rd);
    reader2.read_line(&mut line2).await.unwrap();
    let resp2: Value = serde_json::from_str(&line2).unwrap();
    assert_eq!(resp2["id"], 2);
    assert!(resp2["result"]["tools"].is_array());

    // Shutdown
    drop(stdin_wr);
    listen.await.unwrap();
}

use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
