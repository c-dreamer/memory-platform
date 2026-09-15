//! MCP resources — read-only, URI-addressable views over memory-platform
//! state.
//!
//! Unlike a tool (an explicit call with arbitrary arguments), a resource is
//! a stable URI a client can list and read without constructing a request —
//! useful for a client's own context-attachment UI (e.g. Claude Code's
//! @-mention picker). All resources here are thin wrappers around queries
//! the `status`/`list`/`memory_search` MCP tools already run; no new
//! business logic.
//!
//! Tag values in `memory://tag/{tag}` are taken verbatim from the URI
//! segment, not percent-decoded — fine for the plain-word tags this
//! codebase uses today (`critical`, `windows`, ...). A tag containing a
//! character that needs percent-encoding won't round-trip through this
//! template; add decoding if that ever becomes a real tag naming style.

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::models::Memory;
use crate::AppState;

const MEMORY_COLUMNS: &str =
    "id, agent_id, session_id, content, content_type, embedding::TEXT AS embedding, \
     importance, tags, metadata, last_accessed_at, access_count, \
     decay_score, created_at, updated_at, expiration_date";

/// Static (non-templated) resources: URI is fixed and enumerable.
pub fn list_resources() -> Value {
    json!([
        {
            "uri": "memory://stats",
            "name": "Memory platform stats",
            "description": "Database health and per-table row counts.",
            "mimeType": "application/json",
        },
        {
            "uri": "memory://tags",
            "name": "All memory tags",
            "description": "Distinct tags across non-expired memories.",
            "mimeType": "application/json",
        },
        {
            "uri": "memory://recent/10",
            "name": "10 most recent memories",
            "description": "The 10 most recently created non-expired memories.",
            "mimeType": "application/json",
        },
    ])
}

/// URI templates (RFC 6570) — parameterized resources not worth statically
/// enumerating.
pub fn list_resource_templates() -> Value {
    json!([
        {
            "uriTemplate": "memory://recent/{n}",
            "name": "N most recent memories",
            "description": "The n most recently created non-expired memories.",
            "mimeType": "application/json",
        },
        {
            "uriTemplate": "memory://tag/{tag}",
            "name": "Memories with a tag",
            "description": "Non-expired memories carrying the given tag, newest first (max 50).",
            "mimeType": "application/json",
        },
    ])
}

/// Read one resource by URI, dispatching on its prefix.
pub async fn read_resource(state: &AppState, uri: &str) -> Result<Value> {
    if uri == "memory://stats" {
        return stats(state).await;
    }
    if uri == "memory://tags" {
        return tags(state).await;
    }
    if let Some(n) = uri.strip_prefix("memory://recent/") {
        let limit: i64 = n
            .parse()
            .with_context(|| format!("invalid recent count in resource URI {uri:?}"))?;
        return recent(state, limit).await;
    }
    if let Some(tag) = uri.strip_prefix("memory://tag/") {
        if tag.is_empty() {
            anyhow::bail!("empty tag in resource URI {uri:?}");
        }
        return by_tag(state, tag).await;
    }
    anyhow::bail!("Unknown resource URI: {uri}")
}

async fn stats(state: &AppState) -> Result<Value> {
    Ok(json!({ "healthy": state.db.health().await, "stats": state.db.get_stats().await }))
}

async fn tags(state: &AppState) -> Result<Value> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT DISTINCT unnest(tags) AS tag FROM memories \
         WHERE expiration_date IS NULL OR expiration_date >= CURRENT_DATE \
         ORDER BY 1",
    )
    .fetch_all(&state.db.pool)
    .await
    .context("Failed to list tags")?;
    let tags: Vec<String> = rows.into_iter().map(|(t,)| t).collect();
    Ok(json!({ "tags": tags }))
}

async fn recent(state: &AppState, limit: i64) -> Result<Value> {
    let rows = sqlx::query_as::<_, Memory>(&format!(
        "SELECT {MEMORY_COLUMNS} FROM memories \
         WHERE expiration_date IS NULL OR expiration_date >= CURRENT_DATE \
         ORDER BY created_at DESC LIMIT $1"
    ))
    .bind(limit)
    .fetch_all(&state.db.pool)
    .await
    .context("Failed to fetch recent memories")?;
    Ok(json!({ "memories": rows, "count": rows.len() }))
}

async fn by_tag(state: &AppState, tag: &str) -> Result<Value> {
    let rows = sqlx::query_as::<_, Memory>(&format!(
        "SELECT {MEMORY_COLUMNS} FROM memories \
         WHERE $1 = ANY(tags) AND (expiration_date IS NULL OR expiration_date >= CURRENT_DATE) \
         ORDER BY created_at DESC LIMIT 50"
    ))
    .bind(tag)
    .fetch_all(&state.db.pool)
    .await
    .context("Failed to fetch memories by tag")?;
    Ok(json!({ "tag": tag, "memories": rows, "count": rows.len() }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_resources_are_all_memory_scheme() {
        let resources = list_resources();
        let arr = resources.as_array().unwrap();
        assert!(!arr.is_empty());
        for r in arr {
            let uri = r["uri"].as_str().unwrap();
            assert!(uri.starts_with("memory://"), "unexpected uri: {uri}");
            assert_eq!(r["mimeType"], "application/json");
        }
    }

    #[test]
    fn list_resource_templates_cover_recent_and_tag() {
        let templates = list_resource_templates();
        let arr = templates.as_array().unwrap();
        let uris: Vec<&str> = arr
            .iter()
            .map(|t| t["uriTemplate"].as_str().unwrap())
            .collect();
        assert!(uris.contains(&"memory://recent/{n}"));
        assert!(uris.contains(&"memory://tag/{tag}"));
    }

    #[tokio::test]
    async fn read_resource_rejects_unknown_uri() {
        let state = AppState {
            config: crate::config::Config::default(),
            db: std::sync::Arc::new(crate::db::postgres::PostgresDb::new_empty()),
            search: std::sync::Arc::new(crate::search::SearchEngine::new_empty()),
            neo4j_client: None,
            redis_cache: None,
            context_service: None,
            contradiction_detector: None,
            decay_engine: None,
            embedding_service: None,
            experience_service: None,
            ingestion_service: None,
            procedure_service: None,
            pending_writes: std::sync::Arc::new(crate::queue::PendingWriteQueue::new_empty()),
        };
        let err = read_resource(&state, "memory://nonsense")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Unknown resource URI"));
    }

    #[tokio::test]
    async fn read_resource_rejects_empty_tag() {
        let state = AppState {
            config: crate::config::Config::default(),
            db: std::sync::Arc::new(crate::db::postgres::PostgresDb::new_empty()),
            search: std::sync::Arc::new(crate::search::SearchEngine::new_empty()),
            neo4j_client: None,
            redis_cache: None,
            context_service: None,
            contradiction_detector: None,
            decay_engine: None,
            embedding_service: None,
            experience_service: None,
            ingestion_service: None,
            procedure_service: None,
            pending_writes: std::sync::Arc::new(crate::queue::PendingWriteQueue::new_empty()),
        };
        let err = read_resource(&state, "memory://tag/").await.unwrap_err();
        assert!(err.to_string().contains("empty tag"));
    }
}
