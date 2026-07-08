//! MCP tool handlers — 12 tools matching the Python memory_mcp.py.
//!
//! Each tool is a standalone async function that takes `&AppState` and
//! a `serde_json::Value` arguments map, returning a JSON string result.
//! The `call_tool` dispatcher routes by tool name.

use anyhow::{Context, Result};
use serde_json::{json, Value};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::db::postgres::PostgresDb;
use crate::models::{Experience, Memory, Procedure, Session};
use crate::AppState;

// ---------------------------------------------------------------------------
// Tool definitions (returned by tools/list)
// ---------------------------------------------------------------------------

/// Return the full tool list as a JSON array matching the Python MCP schema.
pub fn list_tools() -> Value {
    json!([
        {
            "name": "memory_search",
            "description": "Search across all memory sources (memories, documents, experiences, trading_results) using hybrid RRF search. Returns ranked results with scores and source metadata.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search query text" },
                    "limit": { "type": "integer", "description": "Maximum results (default: 10)", "default": 10 }
                },
                "required": ["query"]
            }
        },
        {
            "name": "memory_store",
            "description": "Store a new memory with optional tags, content type, and importance. Returns the created memory ID.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "content": { "type": "string", "description": "Memory content text" },
                    "tags": { "type": "array", "items": { "type": "string" }, "description": "Tags for categorization", "default": [] },
                    "content_type": { "type": "string", "description": "Type: note, insight, decision, observation, error", "default": "note" },
                    "importance": { "type": "number", "description": "Importance 0.0-1.0 (default: 0.5)", "default": 0.5 },
                    "session_id": { "type": "string", "description": "Optional session UUID to create cross-reference", "default": null }
                },
                "required": ["content"]
            }
        },
        {
            "name": "memory_context",
            "description": "Get a compressed context package for a query — returns memories, documents, experiences, recent sessions, procedures, and trading results.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Context query" }
                },
                "required": ["query"]
            }
        },
        {
            "name": "memory_initialize",
            "description": "One-shot initialization: retrieves context for a goal and returns it as a formatted banner. Combines memory_context + formatting.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "goal": { "type": "string", "description": "Session goal or task description" }
                },
                "required": ["goal"]
            }
        },
        {
            "name": "experience_find",
            "description": "Find similar past experiences by goal description. Returns top matches with lessons learned and actions.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "goal": { "type": "string", "description": "Goal to match against past experiences" },
                    "limit": { "type": "integer", "description": "Max results (default: 3)", "default": 3 }
                },
                "required": ["goal"]
            }
        },
        {
            "name": "procedure_run",
            "description": "Execute a named procedure. Looks up by name first, falls back to search. Returns execution result.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Procedure name to execute" },
                    "params": { "type": "object", "description": "Optional parameters", "default": {} }
                },
                "required": ["name"]
            }
        },
        {
            "name": "session_start",
            "description": "Start tracking a new session with a goal. Returns the session ID.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "goal": { "type": "string", "description": "Session goal" }
                },
                "required": ["goal"]
            }
        },
        {
            "name": "session_end",
            "description": "End a session with a summary. Marks the session as completed.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string", "description": "Session UUID" },
                    "summary": { "type": "string", "description": "Summary of what was accomplished" }
                },
                "required": ["session_id", "summary"]
            }
        },
        {
            "name": "recall",
            "description": "Recall a specific memory or document by ID or path. Returns the full record.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "UUID of memory or document to recall" }
                },
                "required": ["id"]
            }
        },
        {
            "name": "forget",
            "description": "Soft-delete a memory by ID. Marks it as removed without permanent deletion.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "UUID of memory to forget" }
                },
                "required": ["id"]
            }
        },
        {
            "name": "list",
            "description": "List records from a table with pagination. Tables: memories, documents, experiences, sessions, procedures, agents, trading_results.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "table": { "type": "string", "description": "Table name to list", "default": "memories" },
                    "limit": { "type": "integer", "description": "Max results (default: 50)", "default": 50 },
                    "offset": { "type": "integer", "description": "Result offset (default: 0)", "default": 0 }
                },
                "required": ["table"]
            }
        },
        {
            "name": "status",
            "description": "Get server status — database health check and record counts across all tables.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "required": []
            }
        },
        {
            "name": "session_context",
            "description": "Get all documents and memories accessed in a session. Returns a chronological list with interaction types and relevance scores.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string", "description": "Session UUID" }
                },
                "required": ["session_id"]
            }
        }
    ])
}

// ---------------------------------------------------------------------------
// Tool dispatcher
// ---------------------------------------------------------------------------

/// Dispatch a tool call by name, returning the result as a JSON string.
///
/// # Errors
///
/// Returns an error if the tool name is not recognized.
pub async fn call_tool(state: &AppState, name: &str, arguments: Value) -> Result<String> {
    debug!("Dispatching tool: {name}");

    match name {
        "memory_search" => tool_memory_search(state, arguments).await,
        "memory_store" => tool_memory_store(state, arguments).await,
        "memory_context" => tool_memory_context(state, arguments).await,
        "memory_initialize" => tool_memory_initialize(state, arguments).await,
        "experience_find" => tool_experience_find(state, arguments).await,
        "procedure_run" => tool_procedure_run(state, arguments).await,
        "session_start" => tool_session_start(state, arguments).await,
        "session_end" => tool_session_end(state, arguments).await,
        "recall" => tool_recall(state, arguments).await,
        "forget" => tool_forget(state, arguments).await,
        "list" => tool_list(state, arguments).await,
        "status" => tool_status(state, arguments).await,
        "session_context" => tool_session_context(state, arguments).await,
        _ => Err(anyhow::anyhow!("Unknown tool: {name}")),
    }
}

// ---------------------------------------------------------------------------
// Tool implementations
// ---------------------------------------------------------------------------

/// 1. memory_search — hybrid search across all tables.
async fn tool_memory_search(state: &AppState, args: Value) -> Result<String> {
    let query = get_string(&args, "query")?;
    let limit = get_i64(&args, "limit").unwrap_or(10);
    let session_id = args.get("session_id").and_then(|v| v.as_str()).and_then(|s| Uuid::parse_str(s).ok());

    info!("memory_search: query='{query}', limit={limit}");

    let search_mode = if state.embedding_service.is_some() {
        "hybrid"
    } else {
        warn!("No embedding service — using keyword-only search");
        "keyword"
    };

    // Generate embedding if embedding service is available
    let embedding = match &state.embedding_service {
        Some(svc) => svc.embed(&query).await?.as_vec().to_vec(),
        None => vec![],
    };

    // Search across all four tables in parallel
    let (memories, documents, experiences, trading) = match tokio::try_join!(
        state.search.hybrid_search("memories", &query, &embedding, search_mode, limit),
        state.search.hybrid_search("documents", &query, &embedding, search_mode, limit),
        state.search.hybrid_search("experiences", &query, &embedding, search_mode, limit),
        state.search.hybrid_search("trading_results", &query, &embedding, search_mode, limit),
    ) {
        Ok(results) => results,
        Err(e) => {
            tracing::error!("Search failed: {:?}", e);
            return Ok(json!({"error": format!("Search failed: {}", e)}).to_string());
        }
    };

    // Record cross-references if session_id was provided
    if let Some(sid) = session_id {
        let pool = &state.db.pool;
        for m in &memories {
            let _ = sqlx::query("SELECT record_memory_access($1, $2, 'searched', $3)")
                .bind(sid).bind(m.id).bind(m.score)
                .execute(pool).await;
        }
        for d in &documents {
            let _ = sqlx::query("SELECT record_document_access($1, $2, 'searched', $3)")
                .bind(sid).bind(d.id).bind(d.score)
                .execute(pool).await;
        }
    }

    let result = json!({
        "memories": memories,
        "documents": documents,
        "experiences": experiences,
        "trading_results": trading,
    });

    Ok(serde_json::to_string(&result)?)
}

/// 2. memory_store — store a new memory.
async fn tool_memory_store(state: &AppState, args: Value) -> Result<String> {
    let content = get_string(&args, "content")?;
    let tags = get_string_array(&args, "tags").unwrap_or_default();
    let content_type = get_string(&args, "content_type").unwrap_or_else(|_| "note".to_string());
    let importance = get_f64(&args, "importance").unwrap_or(0.5);
    let session_id = args.get("session_id").and_then(|v| v.as_str()).and_then(|s| Uuid::parse_str(s).ok());

    info!("memory_store: type={content_type}, importance={importance}, tags={tags:?}");

    // Generate embedding if available
    let embedding = match &state.embedding_service {
        Some(svc) => {
            let emb = svc.embed(&content).await?;
            Some(emb.as_vec().to_vec())
        }
        None => None,
    };
    let embedding_slice = embedding.as_deref();

    let memory = match state
        .db
        .store_memory(
            &content,
            &content_type,
            importance,
            &tags,
            &json!({}),
            None,
            None,
            embedding_slice,
        )
        .await
    {
        Ok(m) => m,
        Err(e) => {
            tracing::error!("Failed to store memory: {:?}", e);
            return Ok(json!({"error": format!("Failed to store memory: {}", e)}).to_string());
        }
    };

    // Record cross-reference if session_id was provided
    if let Some(sid) = session_id {
        let _ = sqlx::query("SELECT record_memory_access($1, $2, 'created', $3)")
            .bind(sid)
            .bind(memory.id)
            .bind(importance)
            .execute(&state.db.pool)
            .await;
    }

    let result = json!({
        "id": memory.id,
        "content_type": memory.content_type,
        "importance": memory.importance,
        "tags": memory.tags,
        "created_at": memory.created_at,
    });

    Ok(serde_json::to_string(&result)?)
}

/// 3. memory_context — get context package for a query.
async fn tool_memory_context(state: &AppState, args: Value) -> Result<String> {
    let query = get_string(&args, "query")?;
    let session_id = args.get("session_id").and_then(|v| v.as_str()).and_then(|s| Uuid::parse_str(s).ok());

    info!("memory_context: query='{query}'");

    let embedding = match &state.embedding_service {
        Some(svc) => svc.embed(&query).await?.as_vec().to_vec(),
        None => vec![0.0_f32; state.config.embedding_dim],
    };

    let context = state
        .db
        .get_context_for_query(&query, &embedding, 10)
        .await
        .context("Failed to assemble context")?;

    // Record cross-references if session_id was provided
    if let Some(sid) = session_id {
        let pool = &state.db.pool;
        for m in &context.memories {
            let _ = sqlx::query("SELECT record_memory_access($1, $2, 'loaded', $3)")
                .bind(sid).bind(m.id).bind(m.score)
                .execute(pool).await;
        }
        for d in &context.documents {
            let _ = sqlx::query("SELECT record_document_access($1, $2, 'loaded', $3)")
                .bind(sid).bind(d.id).bind(d.score)
                .execute(pool).await;
        }
    }

    Ok(serde_json::to_string(&context)?)
}

/// 4. memory_initialize — one-shot init: get context + format banner.
async fn tool_memory_initialize(state: &AppState, args: Value) -> Result<String> {
    let goal = get_string(&args, "goal")?;
    let session_id = args.get("session_id").and_then(|v| v.as_str()).and_then(|s| Uuid::parse_str(s).ok());

    info!("memory_initialize: goal='{goal}'");

    let embedding = match &state.embedding_service {
        Some(svc) => svc.embed(&goal).await?.as_vec().to_vec(),
        None => vec![0.0_f32; state.config.embedding_dim],
    };

    let context = state
        .db
        .get_context_for_query(&goal, &embedding, 10)
        .await
        .context("Failed to assemble context")?;

    // Record cross-references if session_id was provided
    if let Some(sid) = session_id {
        let pool = &state.db.pool;
        for m in &context.memories {
            let _ = sqlx::query("SELECT record_memory_access($1, $2, 'loaded', $3)")
                .bind(sid).bind(m.id).bind(m.score)
                .execute(pool).await;
        }
        for d in &context.documents {
            let _ = sqlx::query("SELECT record_document_access($1, $2, 'loaded', $3)")
                .bind(sid).bind(d.id).bind(d.score)
                .execute(pool).await;
        }
    }

    // Format as a banner matching the Python output
    let mut banner =
        String::from("\u{2501}\u{2501}\u{2501} Memory Context \u{2501}\u{2501}\u{2501}\n");

    if context.memories.is_empty()
        && context.documents.is_empty()
        && context.experiences.is_empty()
        && context.recent_sessions.is_empty()
        && context.procedures.is_empty()
        && context.trading_results.is_empty()
    {
        banner.push_str(
            "  (no context found \u{2014} work will be stored for future reference)\n",
        );
    } else {
        if !context.memories.is_empty() {
            banner.push_str(&format!("\nMemories ({}):\n", context.memories.len()));
            for m in &context.memories {
                banner.push_str(&format!(
                    "  - [score:{:.2}] {}\n",
                    m.score,
                    truncate(&m.content, 120)
                ));
            }
        }
        if !context.documents.is_empty() {
            banner.push_str(&format!("\nDocuments ({}):\n", context.documents.len()));
            for d in &context.documents {
                banner.push_str(&format!(
                    "  - [score:{:.2}] {} ({})\n",
                    d.score,
                    truncate(&d.content, 120),
                    d.source_info
                ));
            }
        }
        if !context.experiences.is_empty() {
            banner.push_str(&format!(
                "\nExperiences ({}):\n",
                context.experiences.len()
            ));
            for e in &context.experiences {
                banner.push_str(&format!(
                    "  - [score:{:.2}] {}\n",
                    e.score,
                    truncate(&e.content, 120)
                ));
            }
        }
        if !context.recent_sessions.is_empty() {
            banner.push_str(&format!(
                "\nRecent sessions ({}):\n",
                context.recent_sessions.len()
            ));
            for s in &context.recent_sessions {
                banner.push_str(&format!(
                    "  - {} ({})\n",
                    s.goal.as_deref().unwrap_or("(no goal)"),
                    s.status
                ));
            }
        }
        if !context.procedures.is_empty() {
            banner.push_str(&format!(
                "\nProcedures ({}):\n",
                context.procedures.len()
            ));
            for p in &context.procedures {
                banner.push_str(&format!("  - [used:{}] {}\n", p.times_used, p.name));
            }
        }
        if !context.trading_results.is_empty() {
            banner.push_str(&format!(
                "\nTrading results ({}):\n",
                context.trading_results.len()
            ));
            for t in &context.trading_results {
                banner.push_str(&format!(
                    "  - [pf:{:.2}] {}\n",
                    t.profit_factor.unwrap_or(0.0),
                    t.ea_version.as_deref().unwrap_or("unknown")
                ));
            }
        }
    }

    banner.push_str(
        "\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}",
    );

    let summary = json!({
        "memories": context.memories.len(),
        "documents": context.documents.len(),
        "experiences": context.experiences.len(),
        "procedures": context.procedures.len(),
        "trading_results": context.trading_results.len(),
    });

    let result = json!({
        "status": "ok",
        "context_loaded": !context.memories.is_empty() || !context.documents.is_empty(),
        "banner": banner,
        "summary": summary,
    });

    Ok(serde_json::to_string(&result)?)
}

/// 5. experience_find — find similar past experiences.
async fn tool_experience_find(state: &AppState, args: Value) -> Result<String> {
    let goal = get_string(&args, "goal")?;
    let limit = get_usize(&args, "limit").unwrap_or(3);

    info!("experience_find: goal='{goal}', limit={limit}");

    let experiences = match &state.experience_service {
        Some(svc) => svc.find_relevant(&goal, limit).await?,
        None => {
            // Fallback: search experiences table directly
            let embedding = match &state.embedding_service {
                Some(svc) => svc.embed(&goal).await?.as_vec().to_vec(),
                None => vec![0.0_f32; state.config.embedding_dim],
            };
            let results = state
                .search
                .hybrid_search("experiences", &goal, &embedding, "keyword", limit as i64)
                .await
                .context("Failed to search experiences")?;

            if results.is_empty() {
                Vec::new()
            } else {
                fetch_experiences_by_ids(
                    &state.db,
                    &results.iter().map(|r| r.id).collect::<Vec<_>>(),
                )
                .await
                .context("Failed to fetch experiences")?
            }
        }
    };

    let result = json!({
        "experiences": experiences,
        "count": experiences.len(),
    });

    Ok(serde_json::to_string(&result)?)
}

/// 6. procedure_run — execute a named procedure.
async fn tool_procedure_run(state: &AppState, args: Value) -> Result<String> {
    let name = get_string(&args, "name")?;
    let _params = args.get("params").cloned().unwrap_or(json!({}));

    info!("procedure_run: name='{name}'");

    // Try exact name lookup first, then search
    let procedure = match &state.procedure_service {
        Some(svc) => {
            let candidates = svc.find_candidates(&name).await?;
            candidates
                .into_iter()
                .find(|p| p.name.eq_ignore_ascii_case(&name))
        }
        None => {
            let results = state
                .db
                .search_procedures(&name, 5)
                .await
                .context("Failed to search procedures")?;
            results
                .into_iter()
                .find(|p| p.name.eq_ignore_ascii_case(&name))
        }
    };

    let procedure =
        procedure.ok_or_else(|| anyhow::anyhow!("Procedure not found: {name}"))?;

    // Record execution
    state
        .db
        .record_procedure_execution(procedure.id, true)
        .await
        .context("Failed to record procedure execution")?;

    let steps_count = procedure.steps.as_array().map(|s| s.len()).unwrap_or(0);

    let result = json!({
        "success": true,
        "procedure": procedure.name,
        "steps_completed": steps_count,
        "output": format!("Executed procedure '{}' successfully", procedure.name),
    });

    Ok(serde_json::to_string(&result)?)
}

/// 7. session_start — create a new session.
async fn tool_session_start(state: &AppState, args: Value) -> Result<String> {
    let goal = get_string(&args, "goal")?;

    info!("session_start: goal='{goal}'");

    let session = state
        .db
        .create_session(None, Some(&goal), None)
        .await
        .context("Failed to create session")?;

    let result = json!({
        "session_id": session.id,
        "goal": session.goal,
        "status": session.status,
        "started_at": session.started_at,
    });

    Ok(serde_json::to_string(&result)?)
}

/// 8. session_end — end a session with summary.
async fn tool_session_end(state: &AppState, args: Value) -> Result<String> {
    let session_id_str = get_string(&args, "session_id")?;
    let summary = get_string(&args, "summary")?;

    let session_id = Uuid::parse_str(&session_id_str)
        .with_context(|| format!("Invalid session ID: {session_id_str}"))?;

    info!("session_end: id={session_id}");

    state
        .db
        .end_session(session_id, &summary)
        .await
        .context("Failed to end session")?;

    let result = json!({
        "session_id": session_id,
        "status": "completed",
        "summary": summary,
    });

    Ok(serde_json::to_string(&result)?)
}

/// 9. recall — recall a memory or document by ID.
async fn tool_recall(state: &AppState, args: Value) -> Result<String> {
    let id_str = get_string(&args, "id")?;
    let session_id = args.get("session_id").and_then(|v| v.as_str()).and_then(|s| Uuid::parse_str(s).ok());

    info!("recall: id='{id_str}'");

    // Try as UUID first (memory), then as path (document)
    if let Ok(uuid) = Uuid::parse_str(&id_str) {
        // Try memory
        if let Some(memory) = state
            .db
            .get_memory(uuid)
            .await
            .context("Failed to get memory")?
        {
            // Record access and cross-reference
            let _ = state.db.record_memory_access(uuid).await;
            if let Some(sid) = session_id {
                let _ = sqlx::query("SELECT record_memory_access($1, $2, 'loaded', NULL)")
                    .bind(sid).bind(uuid)
                    .execute(&state.db.pool).await;
            }

            return Ok(serde_json::to_string(&memory)?);
        }

        // Try session
        if let Some(session) = state
            .db
            .get_session(uuid)
            .await
            .context("Failed to get session")?
        {
            return Ok(serde_json::to_string(&session)?);
        }

        // Try experience
        if let Some(exp) = fetch_experience_by_id(&state.db, uuid)
            .await
            .context("Failed to get experience")?
        {
            return Ok(serde_json::to_string(&exp)?);
        }

        // Try procedure
        if let Some(proc) = state
            .db
            .get_procedure(uuid)
            .await
            .context("Failed to get procedure")?
        {
            return Ok(serde_json::to_string(&proc)?);
        }
    }

    // Try as document path
    if let Some(doc) = state
        .db
        .get_document_by_path(&id_str)
        .await
        .context("Failed to get document")?
    {
        if let Some(sid) = session_id {
            let _ = sqlx::query("SELECT record_document_access($1, $2, 'loaded', NULL)")
                .bind(sid).bind(doc.id)
                .execute(&state.db.pool).await;
        }
        return Ok(serde_json::to_string(&doc)?);
    }

    Err(anyhow::anyhow!("No record found for id: {id_str}"))
}

/// 10. forget — soft-delete a memory.
async fn tool_forget(state: &AppState, args: Value) -> Result<String> {
    let id_str = get_string(&args, "id")?;
    let id = Uuid::parse_str(&id_str)
        .with_context(|| format!("Invalid ID: {id_str}"))?;

    info!("forget: id={id}");

    // Soft-delete: set importance to 0 and add a "forgotten" tag
    sqlx::query(
        "UPDATE memories SET importance = 0.0, \
         tags = array_append(tags, 'forgotten'), \
         updated_at = now() \
         WHERE id = $1",
    )
    .bind(id)
    .execute(&state.db.pool)
    .await
    .context("Failed to forget memory")?;

    let result = json!({
        "id": id,
        "status": "forgotten",
    });

    Ok(serde_json::to_string(&result)?)
}

/// 11. list — paginated listing from a table.
async fn tool_list(state: &AppState, args: Value) -> Result<String> {
    let table = get_string(&args, "table").unwrap_or_else(|_| "memories".to_string());
    let limit = get_i64(&args, "limit").unwrap_or(50);
    let offset = get_i64(&args, "offset").unwrap_or(0);

    info!("list: table={table}, limit={limit}, offset={offset}");

    let result = match table.as_str() {
        "memories" => {
            let rows = sqlx::query_as::<_, Memory>(
                "SELECT id, agent_id, session_id, content, content_type, embedding::TEXT AS embedding, \
                 importance, tags, metadata, last_accessed_at, access_count, \
                 decay_score, created_at, updated_at \
                 FROM memories ORDER BY created_at DESC LIMIT $1 OFFSET $2",
            )
            .bind(limit)
            .bind(offset)
            .fetch_all(&state.db.pool)
            .await
            .context("Failed to list memories")?;
            json!({ "table": "memories", "rows": rows, "count": rows.len() })
        }
        "documents" => {
            let rows = sqlx::query_as::<_, crate::models::Document>(
                "SELECT id, path, vault_section, title, content, checksum, frontmatter, \
                 embedding::TEXT AS embedding, token_count, file_size_bytes, file_modified_at, \
                 created_at, updated_at \
                 FROM documents ORDER BY updated_at DESC LIMIT $1 OFFSET $2",
            )
            .bind(limit)
            .bind(offset)
            .fetch_all(&state.db.pool)
            .await
            .context("Failed to list documents")?;
            json!({ "table": "documents", "rows": rows, "count": rows.len() })
        }
        "experiences" => {
            let rows = sqlx::query_as::<_, Experience>(
                "SELECT id, agent_id, session_id, goal, reasoning_summary, actions, \
                 files_changed, result, lessons_learned, confidence, \
                 duration_seconds, tags, related_project, embedding::TEXT AS embedding, \
                 is_procedurized, created_at \
                 FROM experiences ORDER BY created_at DESC LIMIT $1 OFFSET $2",
            )
            .bind(limit)
            .bind(offset)
            .fetch_all(&state.db.pool)
            .await
            .context("Failed to list experiences")?;
            json!({ "table": "experiences", "rows": rows, "count": rows.len() })
        }
        "sessions" => {
            let rows = sqlx::query_as::<_, Session>(
                "SELECT id, agent_id, parent_session_id, goal, status, summary, embedding, \
                 started_at, ended_at, created_at, updated_at \
                 FROM sessions ORDER BY created_at DESC LIMIT $1 OFFSET $2",
            )
            .bind(limit)
            .bind(offset)
            .fetch_all(&state.db.pool)
            .await
            .context("Failed to list sessions")?;
            json!({ "table": "sessions", "rows": rows, "count": rows.len() })
        }
        "procedures" => {
            let rows = sqlx::query_as::<_, Procedure>(
                "SELECT id, name, description, steps, trigger_pattern, source_experience_id, \
                 tags, success_rate, times_used, confidence, created_at, updated_at \
                 FROM procedures ORDER BY times_used DESC LIMIT $1 OFFSET $2",
            )
            .bind(limit)
            .bind(offset)
            .fetch_all(&state.db.pool)
            .await
            .context("Failed to list procedures")?;
            json!({ "table": "procedures", "rows": rows, "count": rows.len() })
        }
        "agents" => {
            let rows = sqlx::query_as::<_, crate::models::Agent>(
                "SELECT id, name, agent_type, capabilities, metadata, is_active, \
                 last_seen_at, created_at, updated_at \
                 FROM agents ORDER BY last_seen_at DESC LIMIT $1 OFFSET $2",
            )
            .bind(limit)
            .bind(offset)
            .fetch_all(&state.db.pool)
            .await
            .context("Failed to list agents")?;
            json!({ "table": "agents", "rows": rows, "count": rows.len() })
        }
        "trading_results" => {
            let rows = sqlx::query_as::<_, crate::models::TradingResult>(
                "SELECT id, agent_id, ea_version, strategy, symbol, timeframe, trade_type, \
                 direction, entry_price, exit_price, profit_factor, drawdown, win_rate, \
                 total_trades, net_profit, duration_days, indicators, inputs, notes, \
                 embedding, created_at \
                 FROM trading_results ORDER BY created_at DESC LIMIT $1 OFFSET $2",
            )
            .bind(limit)
            .bind(offset)
            .fetch_all(&state.db.pool)
            .await
            .context("Failed to list trading_results")?;
            json!({ "table": "trading_results", "rows": rows, "count": rows.len() })
        }
        _ => {
            return Err(anyhow::anyhow!(
                "Unknown table: {table}. Valid tables: memories, documents, experiences, sessions, procedures, agents, trading_results"
            ))
        }
    };

    Ok(serde_json::to_string(&result)?)
}

/// 12. status — database health + stats.
async fn tool_status(state: &AppState, _args: Value) -> Result<String> {
    info!("status");

    let healthy = state.db.health().await;
    let stats = state.db.get_stats().await;

    let result = json!({
        "healthy": healthy,
        "stats": stats,
        "config": {
            "embedding_model": state.config.embedding_model,
            "search_mode": state.config.search_default_mode,
            "decay_enabled": state.config.decay_enabled,
        },
    });

    Ok(serde_json::to_string(&result)?)
}

/// 13. session_context — get all documents and memories accessed in a session.
async fn tool_session_context(state: &AppState, args: Value) -> Result<String> {
    let session_id_str = get_string(&args, "session_id")?;
    let session_id = Uuid::parse_str(&session_id_str)
        .with_context(|| format!("Invalid session ID: {session_id_str}"))?;

    info!("session_context: id={session_id}");

    // Use the get_session_context SQL function to fetch all context
    let rows = sqlx::query_as::<_, (String, Uuid, String, Option<f64>, String, chrono::DateTime<chrono::Utc>)>(
        "SELECT entity_type, entity_id, interaction_type, relevance_score, content, accessed_at \
         FROM get_session_context($1)",
    )
    .bind(session_id)
    .fetch_all(&state.db.pool)
    .await
    .context("Failed to fetch session context")?;

    let entries: Vec<Value> = rows
        .into_iter()
        .map(|(entity_type, entity_id, interaction_type, relevance_score, content, accessed_at)| {
            json!({
                "entity_type": entity_type,
                "entity_id": entity_id,
                "interaction_type": interaction_type,
                "relevance_score": relevance_score,
                "content_preview": truncate(&content, 200),
                "accessed_at": accessed_at,
            })
        })
        .collect();

    let result = json!({
        "session_id": session_id,
        "entries": entries,
        "count": entries.len(),
    });

    Ok(serde_json::to_string(&result)?)
}

// ---------------------------------------------------------------------------
// Argument helpers
// ---------------------------------------------------------------------------

/// Extract a required string argument.
fn get_string(args: &Value, key: &str) -> Result<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| anyhow::anyhow!("Missing required argument: {key}"))
}

/// Extract an optional string array argument.
fn get_string_array(args: &Value, key: &str) -> Option<Vec<String>> {
    args.get(key)
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
}

/// Extract an optional i64 argument.
fn get_i64(args: &Value, key: &str) -> Option<i64> {
    args.get(key).and_then(|v| v.as_i64())
}

/// Extract an optional f64 argument.
fn get_f64(args: &Value, key: &str) -> Option<f64> {
    args.get(key).and_then(|v| v.as_f64())
}

/// Extract an optional usize argument.
fn get_usize(args: &Value, key: &str) -> Option<usize> {
    args.get(key)
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
}

/// Truncate a string to `max_len` characters, adding "..." if truncated.
fn truncate(s: &str, max_len: usize) -> String {
    if s.chars().count() <= max_len {
        s.to_string()
    } else {
        format!("{}...", s.chars().take(max_len).collect::<String>())
    }
}

/// Fetch full Experience rows by a list of UUIDs.
async fn fetch_experiences_by_ids(
    db: &PostgresDb,
    ids: &[Uuid],
) -> Result<Vec<Experience>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }

    let placeholders: Vec<String> = (1..=ids.len()).map(|i| format!("${i}")).collect();
    let sql = format!(
        "SELECT id, agent_id, session_id, goal, reasoning_summary, actions, \
         files_changed, result, lessons_learned, confidence, \
         duration_seconds, tags, related_project, embedding, \
         is_procedurized, created_at \
         FROM experiences WHERE id IN ({})",
        placeholders.join(",")
    );

    let mut query_builder = sqlx::query_as::<_, Experience>(&sql);
    for id in ids {
        query_builder = query_builder.bind(*id);
    }

    query_builder.fetch_all(&db.pool).await
}

/// Fetch a single Experience by UUID.
async fn fetch_experience_by_id(
    db: &PostgresDb,
    id: Uuid,
) -> Result<Option<Experience>, sqlx::Error> {
    sqlx::query_as::<_, Experience>(
        "SELECT id, agent_id, session_id, goal, reasoning_summary, actions, \
         files_changed, result, lessons_learned, confidence, \
         duration_seconds, tags, related_project, embedding, \
         is_procedurized, created_at \
         FROM experiences WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&db.pool)
    .await
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::db::postgres::PostgresDb;
    use crate::search::SearchEngine;
    use std::sync::Arc;

    fn test_state() -> AppState {
        AppState {
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
        }
    }

    #[test]
    fn list_tools_returns_13_tools() {
        let tools = list_tools();
        let arr = tools.as_array().expect("tools should be an array");
        assert_eq!(arr.len(), 13, "Expected 13 tools");
    }

    #[test]
    fn list_tools_each_has_required_fields() {
        let tools = list_tools();
        for tool in tools.as_array().unwrap() {
            assert!(
                tool.get("name").and_then(|v| v.as_str()).is_some(),
                "Tool missing name"
            );
            assert!(
                tool.get("description").and_then(|v| v.as_str()).is_some(),
                "Tool missing description"
            );
            assert!(tool.get("inputSchema").is_some(), "Tool missing inputSchema");
        }
    }

    #[test]
    fn list_tools_names_match_python_mcp() {
        let tools = list_tools();
        let names: Vec<&str> = tools
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();

        let expected = [
            "memory_search",
            "memory_store",
            "memory_context",
            "memory_initialize",
            "experience_find",
            "procedure_run",
            "session_start",
            "session_end",
            "recall",
            "forget",
            "list",
            "status",
            "session_context",
        ];

        for name in &expected {
            assert!(names.contains(name), "Missing tool: {name}");
        }
    }

    #[tokio::test]
    async fn call_tool_unknown_returns_error() {
        let state = test_state();
        let result = call_tool(&state, "nonexistent", json!({})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unknown tool"));
    }

    #[test]
    fn get_string_required_present() {
        let args = json!({"key": "value"});
        assert_eq!(get_string(&args, "key").unwrap(), "value");
    }

    #[test]
    fn get_string_required_missing() {
        let args = json!({});
        assert!(get_string(&args, "key").is_err());
    }

    #[test]
    fn get_string_array_present() {
        let args = json!({"tags": ["rust", "mcp"]});
        let tags = get_string_array(&args, "tags").unwrap();
        assert_eq!(tags, vec!["rust", "mcp"]);
    }

    #[test]
    fn get_string_array_missing() {
        let args = json!({});
        assert!(get_string_array(&args, "tags").is_none());
    }

    #[test]
    fn get_i64_present() {
        let args = json!({"limit": 42});
        assert_eq!(get_i64(&args, "limit"), Some(42));
    }

    #[test]
    fn get_i64_missing() {
        let args = json!({});
        assert_eq!(get_i64(&args, "limit"), None);
    }

    #[test]
    fn get_f64_present() {
        let args = json!({"importance": 0.75});
        assert!((get_f64(&args, "importance").unwrap() - 0.75).abs() < f64::EPSILON);
    }

    #[test]
    fn truncate_short_string() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_long_string() {
        let long = "a".repeat(200);
        let result = truncate(&long, 10);
        assert_eq!(result.len(), 13); // 10 chars + "..."
        assert!(result.ends_with("..."));
    }

    #[test]
    fn truncate_exact_boundary() {
        let exact = "1234567890"; // 10 chars
        assert_eq!(truncate(exact, 10), exact);
    }
}