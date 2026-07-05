//! Context assembly service.
//!
//! Parallel-fetches memories, documents, experiences, sessions,
//! procedures, and trading results for a given query.

use std::sync::Arc;

use anyhow::Result;
use sqlx::PgPool;
use uuid::Uuid;

use crate::db::postgres::PostgresDb;
use crate::models::{Experience, Memory, Procedure, Session};
use crate::search::SearchEngine;

/// Context assembly service.
///
/// Aggregates related data from multiple tables to build a rich
/// context for an agent interaction.
pub struct ContextService {
    db: Arc<PostgresDb>,
    search: Arc<SearchEngine>,
}

impl ContextService {
    /// Create a new context service.
    #[must_use]
    pub fn new(pool: PgPool, search: Arc<SearchEngine>) -> Self {
        Self {
            db: Arc::new(PostgresDb { pool }),
            search,
        }
    }

    /// Fetch recent memories for an agent.
    pub async fn recent_memories(&self, agent_id: &str, limit: i64) -> Result<Vec<Memory>> {
        let agent_id = Uuid::parse_str(agent_id).unwrap_or_default();
        let memories = sqlx::query_as::<_, Memory>(
            "SELECT id, agent_id, session_id, content, content_type, embedding, importance, \
                    tags, metadata, last_accessed_at, access_count, decay_score, created_at, updated_at \
             FROM memories WHERE agent_id = $1 ORDER BY created_at DESC LIMIT $2",
        )
        .bind(agent_id)
        .bind(limit)
        .fetch_all(&self.db.pool)
        .await
        .map_err(anyhow::Error::from)?;
        Ok(memories)
    }

    /// Fetch recent sessions.
    pub async fn recent_sessions(&self, limit: i64) -> Result<Vec<Session>> {
        self.db.get_recent_sessions(limit).await.map_err(Into::into)
    }

    /// Search for relevant experiences.
    pub async fn relevant_experiences(&self, query: &str, limit: i64) -> Result<Vec<Experience>> {
        let dummy_emb = vec![0.0_f32; 384];
        let results = self
            .search
            .hybrid_search("experiences", query, &dummy_emb, "keyword", limit)
            .await?;

        if results.is_empty() {
            return Ok(Vec::new());
        }

        let ids: Vec<Uuid> = results.iter().map(|r| r.id).collect();
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
        for id in &ids {
            query_builder = query_builder.bind(*id);
        }

        query_builder
            .fetch_all(&self.db.pool)
            .await
            .map_err(Into::into)
    }

    /// Search for relevant procedures.
    pub async fn relevant_procedures(&self, query: &str, limit: i64) -> Result<Vec<Procedure>> {
        self.db
            .search_procedures(query, limit)
            .await
            .map_err(Into::into)
    }

    /// Build a context summary string for a given query.
    pub async fn build_context(&self, query: &str, agent_id: &str) -> Result<String> {
        let (memories, experiences, procedures) = tokio::try_join!(
            self.recent_memories(agent_id, 5),
            self.relevant_experiences(query, 3),
            self.relevant_procedures(query, 3),
        )?;

        let mut parts = Vec::new();

        if !memories.is_empty() {
            parts.push(format!("Recent memories ({} entries):", memories.len()));
            for m in &memories {
                parts.push(format!(
                    "  - [{}] {}",
                    m.content_type,
                    m.content.chars().take(100).collect::<String>()
                ));
            }
        }

        if !experiences.is_empty() {
            parts.push(format!(
                "\nRelevant experiences ({} entries):",
                experiences.len()
            ));
            for e in &experiences {
                parts.push(format!(
                    "  - [conf:{:.2}] {}",
                    e.confidence.unwrap_or(0.5),
                    e.goal
                ));
            }
        }

        if !procedures.is_empty() {
            parts.push(format!(
                "\nRelevant procedures ({} entries):",
                procedures.len()
            ));
            for p in &procedures {
                parts.push(format!("  - [used:{}] {}", p.times_used, p.name));
            }
        }

        if parts.is_empty() {
            return Ok("No context available.".to_string());
        }

        Ok(parts.join("\n"))
    }
}

impl Default for ContextService {
    fn default() -> Self {
        // Used only for tests; panics if the pool is actually used.
        panic!("ContextService::default() is not supported — use ContextService::new()");
    }
}
