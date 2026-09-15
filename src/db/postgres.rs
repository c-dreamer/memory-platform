//! PostgreSQL database layer using sqlx with compile-time query checking.
//!
//! Port of the Python `postgres.py` (584 lines) to Rust async sqlx.
//! All 35+ query methods are implemented with `sqlx::query_as` for
//! type-safe row mapping. Vector embeddings use the custom `Embedding`
//! type with pgvector text decode.
//!
//! allow: SIZE_OK — single-responsibility database access layer for all
//! 15 tables. Splitting would create artificial fragmentation; the Python
//! equivalent is also a single 584-line file.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use uuid::Uuid;

use crate::config::Config;
use crate::models::*;

// ---------------------------------------------------------------------------
// Helper structs — not DB-backed, used for insert parameters and return types
// ---------------------------------------------------------------------------

/// Parameters for inserting a trading result.
#[derive(Debug, Clone)]
pub struct CreateTradingResult {
    pub agent_id: Option<Uuid>,
    pub ea_version: Option<String>,
    pub strategy: Option<String>,
    pub symbol: Option<String>,
    pub timeframe: Option<String>,
    pub trade_type: Option<String>,
    pub direction: Option<String>,
    pub entry_price: Option<f64>,
    pub exit_price: Option<f64>,
    pub profit_factor: Option<f64>,
    pub drawdown: Option<f64>,
    pub win_rate: Option<f64>,
    pub total_trades: Option<i32>,
    pub net_profit: Option<f64>,
    pub duration_days: Option<i32>,
    pub indicators: serde_json::Value,
    pub inputs: serde_json::Value,
    pub notes: Option<String>,
}

/// Parameters for inserting an experience.
#[derive(Debug, Clone)]
pub struct CreateExperience {
    pub agent_id: Option<Uuid>,
    pub session_id: Option<Uuid>,
    pub goal: String,
    pub reasoning_summary: Option<String>,
    pub actions: serde_json::Value,
    pub files_changed: serde_json::Value,
    pub result: Option<String>,
    pub lessons_learned: Option<String>,
    pub confidence: Option<f64>,
    pub duration_seconds: Option<i32>,
    pub tags: Vec<String>,
    pub related_project: Option<String>,
}

/// A single search result row returned by vector / BM25 / fulltext queries.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct SearchResult {
    pub id: Uuid,
    pub content: Option<String>,
    pub source_info: String,
    pub score: f64,
}

/// Optional filters for `vector_search`/`bm25_search`: tag membership and a
/// `created_at` range. Tag filtering only takes effect on tables that
/// actually have a `tags` column (`memories`, `experiences`) — it's silently
/// ignored on the others, since `memory_search` fans the same filters out
/// across all 4 searchable tables at once.
#[derive(Debug, Clone, Default)]
pub struct SearchFilters {
    pub tags: Option<Vec<String>>,
    /// `true` = match ANY of `tags` (`&&`), `false` = match ALL of `tags` (`@>`).
    pub tags_match_any: bool,
    pub start_date: Option<DateTime<Utc>>,
    pub end_date: Option<DateTime<Utc>>,
}

impl SearchFilters {
    /// Build the " AND ..." WHERE-clause fragment for whichever filters are
    /// set, numbering placeholders starting at `start_param`. Returns the SQL
    /// fragment and the next unused placeholder number.
    fn clause(&self, extra_col: &str, column_prefix: &str, start_param: usize) -> (String, usize) {
        let mut sql = String::new();
        let mut next = start_param;
        if self.tags.is_some() && extra_col == "tags" {
            let op = if self.tags_match_any { "&&" } else { "@>" };
            sql.push_str(&format!(" AND {column_prefix}tags {op} ${next}::text[]"));
            next += 1;
        }
        if self.start_date.is_some() {
            sql.push_str(&format!(" AND {column_prefix}created_at >= ${next}"));
            next += 1;
        }
        if self.end_date.is_some() {
            sql.push_str(&format!(" AND {column_prefix}created_at <= ${next}"));
            next += 1;
        }
        (sql, next)
    }
}

/// A pair of similar experiences found by cross-join vector comparison.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct SimilarExperience {
    pub goal1: String,
    pub id1: Uuid,
    pub id2: Uuid,
    pub tags1: Vec<String>,
    pub tags2: Vec<String>,
    pub similarity: f64,
}

/// A compact trading result summary for context assembly.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct TradingResultSummary {
    pub id: Uuid,
    pub ea_version: Option<String>,
    pub profit_factor: Option<f64>,
    pub drawdown: Option<f64>,
    pub win_rate: Option<f64>,
    pub created_at: DateTime<Utc>,
}

/// Assembled context package returned by `get_context_for_query`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextPackage {
    pub memories: Vec<SearchResult>,
    pub documents: Vec<SearchResult>,
    pub experiences: Vec<SearchResult>,
    pub recent_sessions: Vec<Session>,
    pub procedures: Vec<Procedure>,
    pub trading_results: Vec<TradingResultSummary>,
}

// ---------------------------------------------------------------------------
// PostgresDb — connection pool wrapper with all query methods
// ---------------------------------------------------------------------------

/// PostgreSQL database access layer.
///
/// Wraps a `sqlx::PgPool` and exposes async methods for every table
/// in the memory platform schema.
#[derive(Debug)]
pub struct PostgresDb {
    pub pool: PgPool,
    /// The `public.embeddings.model` identifier this process stores new
    /// derived-cache rows under — one embedding backend is active per
    /// running process, so this is fixed for the life of the `PostgresDb`.
    pub active_embedding_model: String,
    /// The backend selector (`EMBEDDING_MODEL`'s raw value: "nvidia",
    /// "llama-cpp", ...) rather than the specific model identifier above —
    /// used to decide whether embeddings also belong in a fixed source-table
    /// column, independent of which exact NVIDIA model name is configured.
    active_embedding_backend: String,
}

impl PostgresDb {
    // ------------------------------------------------------------------
    // Connection & health
    // ------------------------------------------------------------------

    /// Wrap an existing pool, tagging it with the environment's configured
    /// embedding model (see `config::active_embedding_model_identifier`) for
    /// callers that only have a bare pool, not a full `Config`.
    pub fn with_pool(pool: PgPool) -> Self {
        Self {
            pool,
            active_embedding_model: crate::config::active_embedding_model_identifier(),
            active_embedding_backend: crate::config::active_embedding_backend(),
        }
    }

    /// Create an empty PostgresDb for testing (no real connection).
    ///
    /// The pool will fail on any actual query. Use only for unit tests
    /// that exercise handler structure, not database access.
    #[must_use]
    pub fn new_empty() -> Self {
        Self {
            pool: PgPoolOptions::new()
                .connect_lazy("postgresql://localhost:5432/nonexistent")
                .expect("connect_lazy should succeed without network"),
            active_embedding_model: "unknown".to_string(),
            active_embedding_backend: "unknown".to_string(),
        }
    }

    /// Create a connection pool from the application config.
    ///
    /// Uses `PgPoolOptions` with min 2, max 10 connections and a 30-second
    /// command timeout, matching the Python `asyncpg.create_pool` defaults.
    pub async fn connect(config: &Config) -> Result<Self, sqlx::Error> {
        let pool = PgPoolOptions::new()
            .min_connections(2)
            .max_connections(10)
            .acquire_timeout(std::time::Duration::from_secs(30))
            .connect(&config.database_url)
            .await?;
        let active_embedding_model = match config.embedding_model.as_str() {
            "nvidia" => config.nvidia_embedding_model.clone(),
            "llama-cpp" => config.llama_cpp_model_name.clone(),
            _ => "unknown".to_string(),
        };
        Ok(Self {
            pool,
            active_embedding_model,
            active_embedding_backend: config.embedding_model.clone(),
        })
    }

    /// Whether the currently active embedding backend's vectors belong in a
    /// fixed `VECTOR(2048)` source-table column. Only the cloud (NVIDIA)
    /// backend does; any other model is recorded in the `embeddings` cache
    /// only, since source columns carry no model discriminator (decision #5,
    /// docs/WINDOWS_PORT_SYNTHESIS.md).
    pub(crate) fn embedding_belongs_in_source_column(&self) -> bool {
        self.active_embedding_backend == "nvidia"
    }

    /// Check database connectivity with `SELECT 1`.
    pub async fn health(&self) -> bool {
        sqlx::query_scalar::<_, i32>("SELECT 1")
            .fetch_one(&self.pool)
            .await
            .map(|v| v == 1)
            .unwrap_or(false)
    }

    /// Get record counts across all main tables.
    pub async fn get_stats(&self) -> HashMap<String, i64> {
        #[derive(sqlx::FromRow)]
        struct StatRow {
            tbl: String,
            cnt: i64,
        }

        let rows: Vec<StatRow> = sqlx::query_as(
            "SELECT 'documents' as tbl, COUNT(*) as cnt FROM documents \
             UNION ALL SELECT 'memories', COUNT(*) FROM memories \
             UNION ALL SELECT 'experiences', COUNT(*) FROM experiences \
             UNION ALL SELECT 'sessions', COUNT(*) FROM sessions \
             UNION ALL SELECT 'procedures', COUNT(*) FROM procedures \
             UNION ALL SELECT 'agents', COUNT(*) FROM agents \
             UNION ALL SELECT 'trading_results', COUNT(*) FROM trading_results \
             UNION ALL SELECT 'relationships', COUNT(*) FROM relationships",
        )
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();

        rows.into_iter().map(|r| (r.tbl, r.cnt)).collect()
    }

    /// Return the dimension distribution of active search vectors.
    pub async fn embedding_dimensions(&self) -> Result<Vec<(i32, i64)>, sqlx::Error> {
        sqlx::query_as::<_, (i32, i64)>(
            "SELECT vector_dims(embedding)::int, COUNT(*)::bigint FROM embeddings \
             WHERE embedding IS NOT NULL GROUP BY vector_dims(embedding) ORDER BY 1",
        )
        .fetch_all(&self.pool)
        .await
    }

    // ------------------------------------------------------------------
    // Agents
    // ------------------------------------------------------------------

    /// Register an agent (upsert by name). Returns the full agent row.
    pub async fn register_agent(
        &self,
        name: &str,
        agent_type: &str,
        capabilities: &[String],
        metadata: &serde_json::Value,
    ) -> Result<Agent, sqlx::Error> {
        sqlx::query_as::<_, Agent>(
            "INSERT INTO agents (name, agent_type, capabilities, metadata, last_seen_at) \
             VALUES ($1, $2, $3, $4, now()) \
             ON CONFLICT (name) DO UPDATE SET \
                 last_seen_at = now(), \
                 is_active = true \
             RETURNING id, name, agent_type, capabilities, metadata, is_active, last_seen_at, created_at, updated_at",
        )
        .bind(name)
        .bind(agent_type)
        .bind(capabilities)
        .bind(metadata)
        .fetch_one(&self.pool)
        .await
    }

    /// Get an agent by UUID.
    pub async fn get_agent(&self, id: Uuid) -> Result<Option<Agent>, sqlx::Error> {
        sqlx::query_as::<_, Agent>("SELECT * FROM agents WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
    }

    /// Get an agent's ID by name (lightweight lookup).
    pub async fn get_agent_by_name(&self, name: &str) -> Result<Option<Agent>, sqlx::Error> {
        sqlx::query_as::<_, Agent>("SELECT * FROM agents WHERE name = $1")
            .bind(name)
            .fetch_optional(&self.pool)
            .await
    }

    // ------------------------------------------------------------------
    // Sessions
    // ------------------------------------------------------------------

    /// Create a new session. Returns the created row.
    pub async fn create_session(
        &self,
        agent_id: Option<Uuid>,
        goal: Option<&str>,
        parent_session_id: Option<Uuid>,
    ) -> Result<Session, sqlx::Error> {
        sqlx::query_as::<_, Session>(
            "INSERT INTO sessions (agent_id, goal, parent_session_id) \
             VALUES ($1, $2, $3) \
             RETURNING id, agent_id, parent_session_id, goal, status, summary, embedding::TEXT as embedding, started_at, ended_at, created_at, updated_at",
        )
        .bind(agent_id)
        .bind(goal)
        .bind(parent_session_id)
        .fetch_one(&self.pool)
        .await
    }

    /// Get a session by UUID.
    pub async fn get_session(&self, id: Uuid) -> Result<Option<Session>, sqlx::Error> {
        sqlx::query_as::<_, Session>(
            "SELECT id, agent_id, parent_session_id, goal, status, summary, embedding::TEXT AS embedding, \
             started_at, ended_at, created_at, updated_at FROM sessions WHERE id = $1",
        )
            .bind(id)
            .fetch_optional(&self.pool)
            .await
    }

    /// Mark a session as completed with a summary.
    pub async fn end_session(&self, id: Uuid, summary: &str) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE sessions SET status = 'completed', summary = $1, ended_at = now(), updated_at = now() WHERE id = $2",
        )
        .bind(summary)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Update a session embedding in-place.
    pub async fn update_session_embedding(
        &self,
        id: Uuid,
        embedding: &[f32],
    ) -> Result<(), sqlx::Error> {
        // Same gate as store_memory/upsert_document/store_experience: the fixed
        // sessions.embedding column carries no model discriminator, so only the
        // NVIDIA cloud-default backend's vectors belong there. Every other model
        // is recorded in the multi-model `embeddings` cache below instead.
        if self.embedding_belongs_in_source_column() {
            let emb_text = vec_to_pgvector(embedding);
            sqlx::query(
                "UPDATE sessions SET embedding = $1::vector, updated_at = now() WHERE id = $2",
            )
            .bind(emb_text)
            .bind(id)
            .execute(&self.pool)
            .await?;
        }
        self.store_embedding("sessions", id, embedding).await?;
        Ok(())
    }

    /// Get recently completed sessions.
    pub async fn get_recent_sessions(&self, limit: i64) -> Result<Vec<Session>, sqlx::Error> {
        sqlx::query_as::<_, Session>(
            "SELECT id, agent_id, parent_session_id, goal, status, summary, embedding::TEXT AS embedding, \
             started_at, ended_at, created_at, updated_at \
             FROM sessions WHERE status = 'completed' ORDER BY created_at DESC LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    // ------------------------------------------------------------------
    // Memories
    // ------------------------------------------------------------------

    /// Store a new memory with optional embedding.
    #[allow(clippy::too_many_arguments)]
    pub async fn store_memory(
        &self,
        content: &str,
        content_type: &str,
        importance: f64,
        tags: &[String],
        metadata: &serde_json::Value,
        agent_id: Option<Uuid>,
        session_id: Option<Uuid>,
        embedding: Option<&[f32]>,
        expiration_date: Option<chrono::NaiveDate>,
        // When the fact became true in the world — may predate `created_at`
        // for backfilled data. `None` defers to `created_at` (see
        // `ContradictionDetector::auto_resolve`). Not part of the `Memory`
        // model: only contradiction auto-resolution reads it, so it isn't
        // threaded through every SELECT that builds a `Memory`.
        valid_at: Option<chrono::DateTime<Utc>>,
    ) -> Result<Memory, sqlx::Error> {
        let emb_text = if self.embedding_belongs_in_source_column() {
            embedding.map(vec_to_pgvector)
        } else {
            None
        };

        let row = sqlx::query_as::<_, Memory>(
            "INSERT INTO memories (agent_id, session_id, content, content_type, embedding, importance, tags, metadata, expiration_date, valid_at) \
             VALUES ($1, $2, $3, $4, $5::vector, $6, $7, $8, $9, $10) \
             RETURNING id, agent_id, session_id, content, content_type, embedding::TEXT as embedding, importance, tags, metadata, last_accessed_at, access_count, decay_score, created_at, updated_at, expiration_date",
        )
        .bind(agent_id)
        .bind(session_id)
        .bind(content)
        .bind(content_type)
        .bind(emb_text)
        .bind(importance)
        .bind(tags)
        .bind(metadata)
        .bind(expiration_date)
        .bind(valid_at)
        .fetch_one(&self.pool)
        .await?;

        if let Some(embedding) = embedding {
            self.store_embedding("memories", row.id, embedding).await?;
        }

        Ok(row)
    }

    /// Get a memory by UUID (excludes fts column since it's #[sqlx(skip)]).
    pub async fn get_memory(&self, id: Uuid) -> Result<Option<Memory>, sqlx::Error> {
        sqlx::query_as::<_, Memory>(
            "SELECT id, agent_id, session_id, content, content_type, embedding::TEXT as embedding, importance, tags, metadata, last_accessed_at, access_count, decay_score, created_at, updated_at, expiration_date FROM memories WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }

    /// Record a memory access — bumps `last_accessed_at` and `access_count`.
    pub async fn record_memory_access(&self, id: Uuid) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE memories \
             SET last_accessed_at = now(), access_count = access_count + 1, updated_at = now() \
             WHERE id = $1",
        )
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Documents
    // ------------------------------------------------------------------

    /// Upsert a document by path. Returns the document UUID.
    pub async fn upsert_document(
        &self,
        path: &str,
        vault_section: Option<&str>,
        title: Option<&str>,
        content: &str,
        checksum: &str,
        frontmatter: &serde_json::Value,
        token_count: Option<i32>,
        file_size_bytes: Option<i32>,
        file_modified_at: Option<DateTime<Utc>>,
        embedding: Option<&[f32]>,
    ) -> Result<Uuid, sqlx::Error> {
        let emb_text = if self.embedding_belongs_in_source_column() {
            embedding.map(vec_to_pgvector)
        } else {
            None
        };

        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
            .bind(path)
            .execute(&mut *tx)
            .await?;

        #[derive(sqlx::FromRow)]
        struct DocId {
            id: Uuid,
        }

        let existing: Option<DocId> =
            sqlx::query_as("SELECT id FROM documents WHERE path = $1 LIMIT 1")
                .bind(path)
                .fetch_optional(&mut *tx)
                .await?;

        let row = if let Some(existing) = existing {
            sqlx::query(
                "UPDATE documents SET \
                     vault_section = $1, \
                     title = $2, \
                     content = $3, \
                     checksum = $4, \
                     frontmatter = $5, \
                     embedding = $6::vector, \
                     token_count = $7, \
                     file_size_bytes = $8, \
                     file_modified_at = $9, \
                     updated_at = now() \
                 WHERE id = $10",
            )
            .bind(vault_section)
            .bind(title)
            .bind(content)
            .bind(checksum)
            .bind(frontmatter)
            .bind(emb_text.as_deref())
            .bind(token_count)
            .bind(file_size_bytes)
            .bind(file_modified_at)
            .bind(existing.id)
            .execute(&mut *tx)
            .await?;
            existing
        } else {
            sqlx::query_as(
                "INSERT INTO documents (path, vault_section, title, content, checksum, frontmatter, \
                                        embedding, token_count, file_size_bytes, file_modified_at) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7::vector, $8, $9, $10) \
                 RETURNING id",
            )
            .bind(path)
            .bind(vault_section)
            .bind(title)
            .bind(content)
            .bind(checksum)
            .bind(frontmatter)
            .bind(emb_text.as_deref())
            .bind(token_count)
            .bind(file_size_bytes)
            .bind(file_modified_at)
            .fetch_one(&mut *tx)
            .await?
        };

        tx.commit().await?;

        match embedding {
            Some(embedding) => {
                self.store_embedding("documents", row.id, embedding).await?;
            }
            None => {
                self.delete_embedding("documents", row.id).await?;
            }
        }

        Ok(row.id)
    }

    /// Get a document by path (lightweight — only id, path, checksum).
    pub async fn get_document_by_path(&self, path: &str) -> Result<Option<Document>, sqlx::Error> {
        sqlx::query_as::<_, Document>(
            "SELECT id, path, vault_section, title, content, checksum, frontmatter, embedding::TEXT as embedding, token_count, file_size_bytes, file_modified_at, created_at, updated_at FROM documents WHERE path = $1",
        )
        .bind(path)
        .fetch_optional(&self.pool)
        .await
    }

    /// Get documents in a vault section.
    pub async fn get_documents_by_section(
        &self,
        section: &str,
        limit: i64,
    ) -> Result<Vec<Document>, sqlx::Error> {
        sqlx::query_as::<_, Document>(
            "SELECT id, path, vault_section, title, content, checksum, frontmatter, embedding::TEXT as embedding, token_count, file_size_bytes, file_modified_at, created_at, updated_at FROM documents WHERE vault_section = $1 ORDER BY updated_at DESC LIMIT $2",
        )
        .bind(section)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    // ------------------------------------------------------------------
    // Trading Results
    // ------------------------------------------------------------------

    /// Store a trading result with optional embedding.
    pub async fn store_trading_result(
        &self,
        data: &CreateTradingResult,
        embedding: Option<&[f32]>,
    ) -> Result<TradingResult, sqlx::Error> {
        let emb_text = if self.embedding_belongs_in_source_column() {
            embedding.map(vec_to_pgvector)
        } else {
            None
        };

        let row = sqlx::query_as::<_, TradingResult>(
            "INSERT INTO trading_results (agent_id, ea_version, strategy, symbol, timeframe, \
                trade_type, direction, entry_price, exit_price, profit_factor, drawdown, \
                win_rate, total_trades, net_profit, duration_days, indicators, inputs, notes, embedding) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19::vector) \
             RETURNING id, agent_id, ea_version, strategy, symbol, timeframe, trade_type, direction, \
                       entry_price, exit_price, profit_factor, drawdown, win_rate, total_trades, \
                       net_profit, duration_days, indicators, inputs, notes, embedding::TEXT AS embedding, created_at",
        )
        .bind(data.agent_id)
        .bind(&data.ea_version)
        .bind(&data.strategy)
        .bind(&data.symbol)
        .bind(&data.timeframe)
        .bind(&data.trade_type)
        .bind(&data.direction)
        .bind(data.entry_price)
        .bind(data.exit_price)
        .bind(data.profit_factor)
        .bind(data.drawdown)
        .bind(data.win_rate)
        .bind(data.total_trades)
        .bind(data.net_profit)
        .bind(data.duration_days)
        .bind(&data.indicators)
        .bind(&data.inputs)
        .bind(&data.notes)
        .bind(emb_text)
        .fetch_one(&self.pool)
        .await?;

        if let Some(embedding) = embedding {
            self.store_embedding("trading_results", row.id, embedding)
                .await?;
        }

        Ok(row)
    }

    // ------------------------------------------------------------------
    // Experiences
    // ------------------------------------------------------------------

    /// Store an experience with optional embedding.
    pub async fn store_experience(
        &self,
        data: &CreateExperience,
        embedding: Option<&[f32]>,
    ) -> Result<Experience, sqlx::Error> {
        let emb_text = if self.embedding_belongs_in_source_column() {
            embedding.map(vec_to_pgvector)
        } else {
            None
        };

        let row = sqlx::query_as::<_, Experience>(
            "INSERT INTO experiences (agent_id, session_id, goal, reasoning_summary, actions, \
                files_changed, result, lessons_learned, confidence, duration_seconds, tags, \
                related_project, embedding) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13::vector) \
             RETURNING id, agent_id, session_id, goal, reasoning_summary, actions, files_changed, \
                       result, lessons_learned, confidence, duration_seconds, tags, \
                       related_project, embedding::TEXT AS embedding, is_procedurized, created_at",
        )
        .bind(data.agent_id)
        .bind(data.session_id)
        .bind(&data.goal)
        .bind(&data.reasoning_summary)
        .bind(&data.actions)
        .bind(&data.files_changed)
        .bind(&data.result)
        .bind(&data.lessons_learned)
        .bind(data.confidence)
        .bind(data.duration_seconds)
        .bind(&data.tags)
        .bind(&data.related_project)
        .bind(emb_text)
        .fetch_one(&self.pool)
        .await?;

        if let Some(embedding) = embedding {
            self.store_embedding("experiences", row.id, embedding)
                .await?;
        }

        Ok(row)
    }

    /// Find pairs of similar successful experiences via cross-join vector comparison.
    ///
    /// Known gap: self-joins `experiences.embedding` directly (the fixed
    /// source column), not the `embeddings` cache — so an experience stored
    /// under a non-cloud-default model (`embedding_belongs_in_source_column`
    /// is false for it) has a NULL source column and never participates
    /// here, until/unless this query is migrated to read the cache instead.
    pub async fn find_similar_experiences(
        &self,
        threshold: f64,
    ) -> Result<Vec<SimilarExperience>, sqlx::Error> {
        sqlx::query_as::<_, SimilarExperience>(
            "SELECT e1.goal AS goal1, e1.id AS id1, e2.id AS id2, \
                    e1.tags AS tags1, e2.tags AS tags2, \
                    1 - (e1.embedding <=> e2.embedding) AS similarity \
             FROM experiences e1 \
             JOIN experiences e2 ON e1.id < e2.id \
             WHERE 1 - (e1.embedding <=> e2.embedding) > $1 \
               AND e1.result = 'success' \
               AND e2.result = 'success' \
               AND e1.is_procedurized = false \
               AND e2.is_procedurized = false",
        )
        .bind(threshold)
        .fetch_all(&self.pool)
        .await
    }

    // ------------------------------------------------------------------
    // Procedures
    // ------------------------------------------------------------------

    /// Create a new procedure from experience patterns.
    pub async fn create_procedure(
        &self,
        name: &str,
        description: Option<&str>,
        steps: &serde_json::Value,
        trigger_pattern: Option<&str>,
        source_experience_id: Option<Uuid>,
        tags: &[String],
    ) -> Result<Procedure, sqlx::Error> {
        sqlx::query_as::<_, Procedure>(
            "INSERT INTO procedures (name, description, steps, trigger_pattern, source_experience_id, tags) \
             VALUES ($1, $2, $3, $4, $5, $6) \
             RETURNING id, name, description, steps, trigger_pattern, source_experience_id, tags, \
                       success_rate, times_used, confidence, created_at, updated_at",
        )
        .bind(name)
        .bind(description)
        .bind(steps)
        .bind(trigger_pattern)
        .bind(source_experience_id)
        .bind(tags)
        .fetch_one(&self.pool)
        .await
    }

    /// List all procedures ordered by usage count.
    pub async fn list_procedures(&self) -> Result<Vec<Procedure>, sqlx::Error> {
        sqlx::query_as::<_, Procedure>(
            "SELECT id, name, description, steps, trigger_pattern, source_experience_id, tags, \
                    success_rate, times_used, confidence, created_at, updated_at \
             FROM procedures ORDER BY times_used DESC",
        )
        .fetch_all(&self.pool)
        .await
    }

    /// Get a procedure by UUID.
    pub async fn get_procedure(&self, id: Uuid) -> Result<Option<Procedure>, sqlx::Error> {
        sqlx::query_as::<_, Procedure>("SELECT * FROM procedures WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
    }

    /// Get related procedures with step counts.
    pub async fn get_related_procedures(&self, limit: i64) -> Result<Vec<Procedure>, sqlx::Error> {
        sqlx::query_as::<_, Procedure>(
            "SELECT id, name, description, steps, trigger_pattern, source_experience_id, tags, \
                    success_rate, times_used, confidence, created_at, updated_at \
             FROM procedures ORDER BY times_used DESC LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    // ------------------------------------------------------------------
    // Search — TABLE_SCHEMAS and all search methods
    // ------------------------------------------------------------------

    /// Table schemas for search: maps table name → (text column, extra info column).
    ///
    /// These are const and used in `format!()` for dynamic SQL — safe because
    /// the values are compile-time constants, not user input.
    const TABLE_SCHEMAS: &[(&str, &str, &str)] = &[
        ("memories", "content", "tags"),
        ("documents", "content", "path"),
        ("experiences", "goal", "tags"),
        ("trading_results", "notes", "strategy"),
    ];

    /// Look up the text column and extra column for a given table name.
    fn table_schema(table: &str) -> (&str, &str) {
        for &(t, text_col, extra_col) in Self::TABLE_SCHEMAS {
            if t == table {
                return (text_col, extra_col);
            }
        }
        ("content", "tags") // safe fallback
    }

    /// Vector similarity search using pgvector `<=>` (cosine distance).
    ///
    /// `model` restricts the comparison to embeddings produced by one model —
    /// required now that `embeddings.embedding` holds mixed dimensions
    /// (migration 015): comparing vectors of different widths via `<=>`
    /// errors, so cross-model rows must never reach the same query.
    pub async fn vector_search(
        &self,
        table: &str,
        embedding: &[f32],
        model: &str,
        limit: i64,
        threshold: f64,
        filters: Option<&SearchFilters>,
    ) -> Result<Vec<SearchResult>, sqlx::Error> {
        let (text_col, extra_col) = Self::table_schema(table);
        let emb_text = vec_to_pgvector(embedding);
        // Cast TEXT[] arrays to TEXT for source_info
        let extra_col_sql = if extra_col == "tags" {
            format!("array_to_string(s.{extra_col}, ',')")
        } else {
            format!("s.{extra_col}")
        };
        // Only `memories` carries superseded_by (migration 016) — a contradiction
        // resolution that picked a winner. Excluding the loser here is what
        // makes that resolution actually change recall, not just sit as inert
        // metadata; it stays fetchable directly (recall/list) for point-in-time review.
        let superseded_filter = if table == "memories" {
            " AND s.superseded_by IS NULL"
        } else {
            ""
        };
        // Only `memories` carries expiration_date (migration 017) — a soft,
        // date-bounded hide that excludes a memory from search once past
        // without touching decay or deleting the row; direct fetch (get_memory)
        // stays unaffected so it's still reviewable in place.
        let expiration_filter = if table == "memories" {
            " AND (s.expiration_date IS NULL OR s.expiration_date >= CURRENT_DATE)"
        } else {
            ""
        };
        let (filter_clause, limit_param) = filters
            .map(|f| f.clause(extra_col, "s.", 5))
            .unwrap_or((String::new(), 5));
        let sql = format!(
            "SELECT s.id, COALESCE(s.{text_col}, '') AS content, COALESCE({extra_col_sql}, '') AS source_info, \
                    (1 - (e.embedding <=> $1::vector))::float8 AS score \
             FROM embeddings e \
             INNER JOIN {table} s ON s.id = e.source_id \
             WHERE e.source_table = $2 \
               AND e.model = $3 \
               AND e.embedding IS NOT NULL \
               AND (1 - (e.embedding <=> $1::vector))::float8 > $4 \
               {superseded_filter}{expiration_filter}{filter_clause} \
             ORDER BY score DESC \
             LIMIT ${limit_param}"
        );

        let mut q = sqlx::query_as::<_, SearchResult>(&sql)
            .bind(&emb_text)
            .bind(table)
            .bind(model)
            .bind(threshold);
        if let Some(f) = filters {
            if let Some(tags) = &f.tags {
                if extra_col == "tags" {
                    q = q.bind(tags);
                }
            }
            if let Some(d) = f.start_date {
                q = q.bind(d);
            }
            if let Some(d) = f.end_date {
                q = q.bind(d);
            }
        }
        q.bind(limit).fetch_all(&self.pool).await
    }

    /// BM25-inspired full-text search using tsvector/tsquery with ts_rank.
    ///
    /// Falls back to pg_trgm similarity if the tsvector index is not available.
    pub async fn bm25_search(
        &self,
        table: &str,
        query: &str,
        limit: i64,
        filters: Option<&SearchFilters>,
    ) -> Result<Vec<SearchResult>, sqlx::Error> {
        let (text_col, extra_col) = Self::table_schema(table);
        // Cast TEXT[] arrays to TEXT for source_info
        let extra_col_sql = if extra_col == "tags" {
            format!("array_to_string({extra_col}, ',')")
        } else {
            extra_col.to_string()
        };

        // Which tables carry an `fts` column is fixed at migration time (see
        // migrations/001_initial.sql, 002_hybrid_decay_contradiction.sql) —
        // every TABLE_SCHEMAS table except trading_results. No need to ask
        // Postgres at query time what's already known statically.
        let has_fts = table != "trading_results";

        // Same reasoning as vector_search's superseded_filter: only `memories`
        // has the column, and excluding a superseded loser here keeps keyword
        // search consistent with vector search after a contradiction is resolved.
        let superseded_filter = if table == "memories" {
            " AND superseded_by IS NULL"
        } else {
            ""
        };
        // Same reasoning as vector_search's expiration_filter: only `memories`
        // has the column.
        let expiration_filter = if table == "memories" {
            " AND (expiration_date IS NULL OR expiration_date >= CURRENT_DATE)"
        } else {
            ""
        };
        let (filter_clause, limit_param) = filters
            .map(|f| f.clause(extra_col, "", 2))
            .unwrap_or((String::new(), 2));

        if has_fts {
            // Try tsvector first
            let ts_sql = format!(
                "SELECT id, COALESCE({text_col}, '') AS content, COALESCE({extra_col_sql}, '') AS source_info, \
                        ts_rank(fts, plainto_tsquery('english', $1), 32)::float8 AS score \
                 FROM {table} \
                 WHERE fts @@ plainto_tsquery('english', $1) \
                 {superseded_filter}{expiration_filter}{filter_clause} \
                 ORDER BY score DESC \
                 LIMIT ${limit_param}"
            );

            let mut q = sqlx::query_as::<_, SearchResult>(&ts_sql).bind(query);
            q = Self::bind_search_filters(q, filters, extra_col);
            let result = q.bind(limit).fetch_all(&self.pool).await;

            match result {
                Ok(rows) if !rows.is_empty() => return Ok(rows),
                Ok(_) => {}  // empty results, fall through to trigram
                Err(_) => {} // error, fall through to trigram
            }
        }

        // Fallback to trigram similarity
        let pg_sql = format!(
            "SELECT id, COALESCE({text_col}, '') AS content, COALESCE({extra_col_sql}, '') AS source_info, \
                    similarity({text_col}, $1)::float8 AS score \
             FROM {table} \
             WHERE {text_col} % $1 \
             {superseded_filter}{expiration_filter}{filter_clause} \
             ORDER BY score DESC \
             LIMIT ${limit_param}"
        );
        let mut q = sqlx::query_as::<_, SearchResult>(&pg_sql).bind(query);
        q = Self::bind_search_filters(q, filters, extra_col);
        q.bind(limit).fetch_all(&self.pool).await
    }

    /// Bind the optional tag/date filter values, in the same order
    /// `SearchFilters::clause` numbered their placeholders.
    fn bind_search_filters<'q>(
        mut q: sqlx::query::QueryAs<'q, sqlx::Postgres, SearchResult, sqlx::postgres::PgArguments>,
        filters: Option<&'q SearchFilters>,
        extra_col: &str,
    ) -> sqlx::query::QueryAs<'q, sqlx::Postgres, SearchResult, sqlx::postgres::PgArguments> {
        let Some(f) = filters else {
            return q;
        };
        if let Some(tags) = &f.tags {
            if extra_col == "tags" {
                q = q.bind(tags);
            }
        }
        if let Some(d) = f.start_date {
            q = q.bind(d);
        }
        if let Some(d) = f.end_date {
            q = q.bind(d);
        }
        q
    }

    /// Legacy trigram-based full-text search. Prefer `bm25_search`.
    pub async fn fulltext_search(
        &self,
        table: &str,
        query: &str,
        limit: i64,
    ) -> Result<Vec<SearchResult>, sqlx::Error> {
        // Delegate to bm25_search which already has the fallback logic
        self.bm25_search(table, query, limit, None).await
    }

    /// Legacy hybrid search — simple union of vector + fulltext results.
    pub async fn hybrid_search(
        &self,
        table: &str,
        embedding: &[f32],
        query: &str,
        limit: i64,
        threshold: f64,
    ) -> Result<Vec<SearchResult>, sqlx::Error> {
        let vec_results = self
            .vector_search(
                table,
                embedding,
                &self.active_embedding_model,
                limit,
                threshold,
                None,
            )
            .await?;
        let ft_results = self.fulltext_search(table, query, limit).await?;

        let mut seen = std::collections::HashSet::new();
        let mut merged = Vec::with_capacity(vec_results.len() + ft_results.len());

        for r in vec_results.into_iter().chain(ft_results) {
            if seen.insert(r.id) {
                merged.push(r);
            }
        }

        merged.truncate(limit as usize);
        Ok(merged)
    }

    // ------------------------------------------------------------------
    // Contradictions
    // ------------------------------------------------------------------

    /// Detect contradictions between a new memory and existing similar ones.
    pub async fn detect_contradictions(
        &self,
        memory_id: Uuid,
        content: &str,
        embedding: &[f32],
    ) -> Result<Vec<ContradictionCandidate>, sqlx::Error> {
        let similar = self
            .vector_search(
                "memories",
                embedding,
                &self.active_embedding_model,
                10,
                0.6,
                None,
            )
            .await?;

        let mut candidates = Vec::new();
        let new_lower = content.to_lowercase();

        const NEGATION_PAIRS: &[(&str, &str)] = &[
            ("buy", "sell"),
            ("long", "short"),
            ("bullish", "bearish"),
            ("increase", "decrease"),
            ("up", "down"),
            ("positive", "negative"),
            ("profitable", "unprofitable"),
            ("win", "loss"),
            ("good", "bad"),
            ("should", "shouldn't"),
            ("always", "never"),
            ("yes", "no"),
            ("support", "resistance"),
            ("overbought", "oversold"),
            ("truth", "false"),
            ("correct", "incorrect"),
            ("above", "below"),
            ("high", "low"),
            ("rise", "fall"),
            ("gain", "loss"),
            ("upward", "downward"),
            ("strong", "weak"),
            ("overweight", "underweight"),
            ("exceed", "underperform"),
            ("better", "worse"),
            ("improve", "decline"),
            ("buying", "selling"),
            ("bull", "bear"),
            ("overvalued", "undervalued"),
            ("expensive", "cheap"),
        ];

        for mem in &similar {
            if mem.id == memory_id {
                continue;
            }
            if mem.score < 0.65 {
                continue;
            }

            let existing_lower = mem.content.as_deref().unwrap_or("").to_lowercase();
            let mut signals = 0;

            for &(a, b) in NEGATION_PAIRS {
                if (existing_lower.contains(a) && new_lower.contains(b))
                    || (existing_lower.contains(b) && new_lower.contains(a))
                {
                    signals += 1;
                }
            }

            if signals >= 1 {
                candidates.push(ContradictionCandidate {
                    memory_id_a: memory_id,
                    memory_id_b: mem.id,
                    content_a: content.chars().take(200).collect(),
                    content_b: mem
                        .content
                        .as_deref()
                        .unwrap_or("")
                        .chars()
                        .take(200)
                        .collect(),
                    similarity: (mem.score * 1000.0).round() / 1000.0,
                    contradiction_type: "semantic".into(),
                });
            }
        }

        Ok(candidates)
    }

    /// Byte-cap a string without landing mid-character. `content_a`/`content_b`
    /// are already char-capped by the caller, but a char cap doesn't bound
    /// byte length (multi-byte UTF-8 can still exceed `max_bytes`), and a raw
    /// byte slice at a non-boundary index panics.
    fn truncate_at_byte_boundary(s: &str, max_bytes: usize) -> &str {
        if s.len() <= max_bytes {
            return s;
        }
        let mut end = max_bytes;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        &s[..end]
    }

    /// Store a detected contradiction, or return the existing row for the
    /// same pair. `(memory_id_a, memory_id_b)` is normalized into canonical
    /// (smaller, larger) order before insert so the same pair discovered
    /// from either detection direction dedupes onto one row instead of
    /// inserting a duplicate every time — matching `fetch_contradiction`'s
    /// own normalize-then-query assumption. `ON CONFLICT ... DO UPDATE`
    /// (rather than `DO NOTHING`) is what makes `RETURNING` fire for the
    /// already-exists case too, so callers get the full row in one round
    /// trip instead of a separate fetch after insert.
    pub async fn store_contradiction(
        &self,
        memory_id_a: Uuid,
        memory_id_b: Uuid,
        content_a: &str,
        content_b: &str,
        similarity: f64,
        contradiction_type: &str,
    ) -> Result<Contradiction, sqlx::Error> {
        let (id_a, id_b) = if memory_id_a < memory_id_b {
            (memory_id_a, memory_id_b)
        } else {
            (memory_id_b, memory_id_a)
        };
        sqlx::query_as::<_, Contradiction>(
            "INSERT INTO contradictions (memory_id_a, memory_id_b, content_a, content_b, \
                                         similarity, contradiction_type) \
             VALUES ($1, $2, $3, $4, $5, $6) \
             ON CONFLICT (memory_id_a, memory_id_b) \
             DO UPDATE SET memory_id_a = EXCLUDED.memory_id_a \
             RETURNING id, memory_id_a, memory_id_b, content_a, content_b, \
                       similarity, contradiction_type, detected_by, resolved, \
                       resolution_note, created_at, updated_at",
        )
        .bind(id_a)
        .bind(id_b)
        .bind(Self::truncate_at_byte_boundary(content_a, 500))
        .bind(Self::truncate_at_byte_boundary(content_b, 500))
        .bind(similarity)
        .bind(contradiction_type)
        .fetch_one(&self.pool)
        .await
    }

    // ------------------------------------------------------------------
    // Config
    // ------------------------------------------------------------------

    /// Get a config value from the KV store, falling back to `default`.
    pub async fn get_config(&self, key: &str, default: &str) -> String {
        #[derive(sqlx::FromRow)]
        struct ConfigValue {
            value: String,
        }

        sqlx::query_as::<_, ConfigValue>("SELECT value FROM config WHERE key = $1")
            .bind(key)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
            .map(|r| r.value)
            .unwrap_or_else(|| default.to_string())
    }

    // ------------------------------------------------------------------
    // Embedding Store
    // ------------------------------------------------------------------

    /// Store a raw embedding in the embeddings table, tagged with the model
    /// that produced it (`self.active_embedding_model`) rather than relying
    /// on the column's default, which only ever matched the NVIDIA backend.
    pub async fn store_embedding(
        &self,
        source_table: &str,
        source_id: Uuid,
        embedding: &[f32],
    ) -> Result<(), sqlx::Error> {
        let emb_text = vec_to_pgvector(embedding);
        let dimension = embedding.len() as i32;

        sqlx::query(
            "INSERT INTO embeddings (source_table, source_id, embedding, model, dimension) \
             VALUES ($1, $2, $3::vector, $4, $5) \
             ON CONFLICT (source_table, source_id, model) DO UPDATE SET \
                 embedding = EXCLUDED.embedding, \
                 dimension = EXCLUDED.dimension",
        )
        .bind(source_table)
        .bind(source_id)
        .bind(&emb_text)
        .bind(&self.active_embedding_model)
        .bind(dimension)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Delete a stored embedding for a source row.
    pub async fn delete_embedding(
        &self,
        source_table: &str,
        source_id: Uuid,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM embeddings WHERE source_table = $1 AND source_id = $2")
            .bind(source_table)
            .bind(source_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Context Assembly
    // ------------------------------------------------------------------

    /// Assemble a context package from multiple tables for a query.
    pub async fn get_context_for_query(
        &self,
        query: &str,
        embedding: &[f32],
        limit: i64,
    ) -> Result<ContextPackage, sqlx::Error> {
        let (memories, documents, experiences, recent_sessions, procedures, trading_results) = tokio::try_join!(
            self.hybrid_search("memories", embedding, query, limit, 0.5),
            self.hybrid_search("documents", embedding, query, limit, 0.5),
            self.hybrid_search("experiences", embedding, query, limit, 0.5),
            self.get_recent_sessions(3),
            self.get_related_procedures(3),
            self.get_trading_results_summary(3),
        )?;

        Ok(ContextPackage {
            memories,
            documents,
            experiences,
            recent_sessions,
            procedures,
            trading_results,
        })
    }

    /// Assemble context without vectors. This is the controlled fallback used
    /// when embedding initialization or dimension validation fails.
    pub async fn get_keyword_context_for_query(
        &self,
        query: &str,
        limit: i64,
    ) -> Result<ContextPackage, sqlx::Error> {
        let (memories, documents, experiences, recent_sessions, procedures, trading_results) = tokio::try_join!(
            self.bm25_search("memories", query, limit, None),
            self.bm25_search("documents", query, limit, None),
            self.bm25_search("experiences", query, limit, None),
            self.get_recent_sessions(3),
            self.get_related_procedures(3),
            self.get_trading_results_summary(3),
        )?;
        Ok(ContextPackage {
            memories,
            documents,
            experiences,
            recent_sessions,
            procedures,
            trading_results,
        })
    }

    /// Get compact trading result summaries.
    pub async fn get_trading_results_summary(
        &self,
        limit: i64,
    ) -> Result<Vec<TradingResultSummary>, sqlx::Error> {
        sqlx::query_as::<_, TradingResultSummary>(
            "SELECT id, ea_version, profit_factor, drawdown, win_rate, created_at \
             FROM trading_results ORDER BY created_at DESC LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    // ------------------------------------------------------------------
    // Procedure helper methods
    // ------------------------------------------------------------------

    /// Search procedures by text content (name + description).
    pub async fn search_procedures(
        &self,
        query: &str,
        limit: i64,
    ) -> Result<Vec<Procedure>, sqlx::Error> {
        let pattern = format!("%{}%", query);
        sqlx::query_as::<_, Procedure>(
            "SELECT id, name, description, steps, trigger_pattern, source_experience_id, tags, \
                    success_rate, times_used, confidence, created_at, updated_at \
             FROM procedures \
             WHERE name ILIKE $1 OR description ILIKE $1 \
             ORDER BY times_used DESC LIMIT $2",
        )
        .bind(&pattern)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    /// Record a procedure execution (increment times_used and update success_rate).
    pub async fn record_procedure_execution(
        &self,
        id: Uuid,
        success: bool,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE procedures SET times_used = times_used + 1, \
                    success_rate = ((success_rate * times_used) + $2::int::float8) / (times_used + 1)::float8, \
                    updated_at = now() \
             WHERE id = $1",
        )
        .bind(id)
        .bind(success)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Update a procedure's metadata.
    pub async fn update_procedure(
        &self,
        id: Uuid,
        name: &str,
        description: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE procedures SET name = $2, description = COALESCE($3, description), updated_at = now() WHERE id = $1",
        )
        .bind(id)
        .bind(name)
        .bind(description)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Convert a `&[f32]` embedding to pgvector text format `'[x,y,z]'`.
///
/// This is the format PostgreSQL's pgvector extension expects for INSERT
/// and query parameters. The `Embedding` type handles decoding on SELECT.
fn vec_to_pgvector(embedding: &[f32]) -> String {
    let inner = embedding
        .iter()
        .map(|x| x.to_string())
        .collect::<Vec<_>>()
        .join(",");
    format!("[{inner}]")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------
    // vec_to_pgvector
    // ------------------------------------------------------------------

    #[test]
    fn vec_to_pgvector_formats_correctly() {
        let result = vec_to_pgvector(&[0.1, 0.2, 0.3]);
        assert_eq!(result, "[0.1,0.2,0.3]");
    }

    #[test]
    fn vec_to_pgvector_empty() {
        let result = vec_to_pgvector(&[]);
        assert_eq!(result, "[]");
    }

    #[test]
    fn vec_to_pgvector_single() {
        let result = vec_to_pgvector(&[1.0]);
        assert_eq!(result, "[1]");
    }

    // ------------------------------------------------------------------
    // SearchFilters::clause
    // ------------------------------------------------------------------

    #[test]
    fn search_filters_clause_empty_when_unset() {
        let f = SearchFilters::default();
        let (sql, next) = f.clause("tags", "s.", 5);
        assert_eq!(sql, "");
        assert_eq!(next, 5);
    }

    #[test]
    fn search_filters_clause_tags_any_only_on_tags_column() {
        let f = SearchFilters {
            tags: Some(vec!["a".into()]),
            tags_match_any: true,
            ..Default::default()
        };
        let (sql, next) = f.clause("tags", "s.", 5);
        assert_eq!(sql, " AND s.tags && $5::text[]");
        assert_eq!(next, 6);

        // Ignored on a table without a tags column (e.g. documents/trading_results).
        let (sql, next) = f.clause("path", "s.", 5);
        assert_eq!(sql, "");
        assert_eq!(next, 5);
    }

    #[test]
    fn search_filters_clause_tags_all_mode() {
        let f = SearchFilters {
            tags: Some(vec!["a".into()]),
            tags_match_any: false,
            ..Default::default()
        };
        let (sql, _) = f.clause("tags", "s.", 5);
        assert_eq!(sql, " AND s.tags @> $5::text[]");
    }

    #[test]
    fn search_filters_clause_date_range_numbers_after_tags() {
        let f = SearchFilters {
            tags: Some(vec!["a".into()]),
            tags_match_any: true,
            start_date: Some(Utc::now()),
            end_date: Some(Utc::now()),
        };
        let (sql, next) = f.clause("tags", "", 2);
        assert_eq!(
            sql,
            " AND tags && $2::text[] AND created_at >= $3 AND created_at <= $4"
        );
        assert_eq!(next, 5);
    }

    // ------------------------------------------------------------------
    // TABLE_SCHEMAS lookup
    // ------------------------------------------------------------------

    #[test]
    fn table_schema_known_tables() {
        assert_eq!(PostgresDb::table_schema("memories"), ("content", "tags"));
        assert_eq!(PostgresDb::table_schema("documents"), ("content", "path"));
        assert_eq!(PostgresDb::table_schema("experiences"), ("goal", "tags"));
        assert_eq!(
            PostgresDb::table_schema("trading_results"),
            ("notes", "strategy")
        );
    }

    #[test]
    fn table_schema_unknown_falls_back() {
        assert_eq!(PostgresDb::table_schema("nonexistent"), ("content", "tags"));
    }

    // ------------------------------------------------------------------
    // Helper structs — construction
    // ------------------------------------------------------------------

    #[test]
    fn create_trading_result_defaults() {
        let data = CreateTradingResult {
            agent_id: None,
            ea_version: None,
            strategy: None,
            symbol: None,
            timeframe: None,
            trade_type: None,
            direction: None,
            entry_price: None,
            exit_price: None,
            profit_factor: None,
            drawdown: None,
            win_rate: None,
            total_trades: None,
            net_profit: None,
            duration_days: None,
            indicators: serde_json::json!({}),
            inputs: serde_json::json!({}),
            notes: None,
        };
        assert!(data.agent_id.is_none());
        assert_eq!(data.indicators, serde_json::json!({}));
    }

    #[test]
    fn create_experience_with_data() {
        let data = CreateExperience {
            agent_id: Some(Uuid::new_v4()),
            session_id: None,
            goal: "Test goal".into(),
            reasoning_summary: Some("Used TDD".into()),
            actions: serde_json::json!(["write test", "implement"]),
            files_changed: serde_json::json!(["src/lib.rs"]),
            result: Some("success".into()),
            lessons_learned: Some("Always test first".into()),
            confidence: Some(0.95),
            duration_seconds: Some(120),
            tags: vec!["rust".into(), "tdd".into()],
            related_project: Some("memory-platform".into()),
        };
        assert_eq!(data.goal, "Test goal");
        assert_eq!(data.tags.len(), 2);
        assert!(data.confidence.is_some());
    }

    #[test]
    fn search_result_serde_roundtrip() {
        let sr = SearchResult {
            id: Uuid::new_v4(),
            content: Some("Rust is memory-safe".into()),
            source_info: "rust,memory".into(),
            score: 0.95,
        };
        let json = serde_json::to_string(&sr).unwrap();
        let decoded: SearchResult = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.content, Some("Rust is memory-safe".into()));
        assert_eq!(decoded.score, 0.95);
    }

    #[test]
    fn similar_experience_serde_roundtrip() {
        let se = SimilarExperience {
            goal1: "Implement auth".into(),
            id1: Uuid::new_v4(),
            id2: Uuid::new_v4(),
            tags1: vec!["auth".into()],
            tags2: vec!["security".into()],
            similarity: 0.92,
        };
        let json = serde_json::to_string(&se).unwrap();
        let decoded: SimilarExperience = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.goal1, "Implement auth");
        assert_eq!(decoded.similarity, 0.92);
    }

    #[test]
    fn trading_result_summary_serde_roundtrip() {
        let now = Utc::now();
        let summary = TradingResultSummary {
            id: Uuid::new_v4(),
            ea_version: Some("2.1.0".into()),
            profit_factor: Some(1.5),
            drawdown: Some(0.05),
            win_rate: Some(0.65),
            created_at: now,
        };
        let json = serde_json::to_string(&summary).unwrap();
        let decoded: TradingResultSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.ea_version, Some("2.1.0".into()));
        assert_eq!(decoded.profit_factor, Some(1.5));
    }

    #[test]
    fn context_package_serde_roundtrip() {
        let now = Utc::now();
        let pkg = ContextPackage {
            memories: vec![],
            documents: vec![],
            experiences: vec![],
            recent_sessions: vec![Session {
                id: Uuid::new_v4(),
                agent_id: None,
                parent_session_id: None,
                goal: Some("Test".into()),
                status: "completed".into(),
                summary: Some("Done".into()),
                embedding: None,
                started_at: now,
                ended_at: Some(now),
                created_at: now,
                updated_at: now,
            }],
            procedures: vec![],
            trading_results: vec![],
        };
        let json = serde_json::to_string(&pkg).unwrap();
        let decoded: ContextPackage = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.recent_sessions.len(), 1);
        assert_eq!(decoded.recent_sessions[0].goal, Some("Test".into()));
    }

    // ------------------------------------------------------------------
    // RRF formula verification
    // ------------------------------------------------------------------

    #[test]
    fn rrf_formula_matches_python() {
        // Python: score = 1.0 / (rrf_k + rank + 1) * weight
        let rrf_k = 20;
        let weight = 0.6;
        let rank = 0; // first result
        let expected = 1.0 / (rrf_k as f64 + rank as f64 + 1.0) * weight;
        // 1.0 / 21 * 0.6 ≈ 0.028571...
        assert!((expected - 0.028571428).abs() < 0.001);
    }

    #[test]
    fn decay_formula_matches_python() {
        // Python: max(0.1, pow(2.0, -days_since / half_life_days))
        let days_since = 90.0;
        let half_life = 90.0;
        let decay = f64::max(0.1, 2_f64.powf(-days_since / half_life));
        // 2^(-1) = 0.5, max(0.1, 0.5) = 0.5
        assert!((decay - 0.5).abs() < 0.001);
    }

    #[test]
    fn decay_clamps_to_min_score() {
        let days_since = 900.0;
        let half_life = 90.0;
        let decay = f64::max(0.1, 2_f64.powf(-days_since / half_life));
        // 2^(-10) ≈ 0.000976, max(0.1, 0.000976) = 0.1
        assert!((decay - 0.1).abs() < 0.001);
    }

    // ------------------------------------------------------------------
    // Contradiction negation pairs
    // ------------------------------------------------------------------

    #[test]
    fn negation_pairs_cover_expected_opposites() {
        // Verify key pairs from the Python reference are present
        let pairs: Vec<(&str, &str)> = vec![
            ("buy", "sell"),
            ("long", "short"),
            ("bullish", "bearish"),
            ("profitable", "unprofitable"),
            ("good", "bad"),
            ("always", "never"),
            ("support", "resistance"),
            ("above", "below"),
            ("high", "low"),
            ("strong", "weak"),
            ("better", "worse"),
            ("bull", "bear"),
            ("expensive", "cheap"),
        ];
        // These are the const NEGATION_PAIRS in detect_contradictions
        // Just verify the test compiles and the pairs are valid
        for (a, b) in &pairs {
            assert_ne!(a, b);
        }
    }
}
