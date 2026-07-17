//! Ingestion engine — batch-imports external data sources into the memory platform.
//!
//! Sources:
//! - `sessions` — OpenCode session DB (`opencode.db` SQLite)
//! - `vault` — Obsidian vault `.md` files (delegates to vault-sync logic)
//! - `config` — OpenCode config, rules, skills
//! - `logs` — OpenCode log file

pub mod codex_sessions;
pub mod config;
pub mod logs;
pub mod sessions;
pub mod vault;

use anyhow::{Context, Result};
use chrono::Utc;
use serde_json::Value;
use sqlx::PgPool;
use std::collections::HashMap;
use tracing::info;
use uuid::Uuid;

/// Cumulative report of what was ingested.
#[derive(Debug, Default, Clone)]
pub struct IngestReport {
    pub sources_processed: HashMap<String, u64>,
    pub memories_created: u64,
    pub experiences_created: u64,
    pub documents_created: u64,
    pub sessions_created: u64,
    pub errors: u64,
    pub warnings: u64,
}

impl IngestReport {
    pub fn add(&mut self, other: &Self) {
        for (k, v) in &other.sources_processed {
            *self.sources_processed.entry(k.clone()).or_default() += v;
        }
        self.memories_created += other.memories_created;
        self.experiences_created += other.experiences_created;
        self.documents_created += other.documents_created;
        self.sessions_created += other.sessions_created;
        self.errors += other.errors;
        self.warnings += other.warnings;
    }

    pub fn print(&self) {
        println!("\n═══════════════════════════════════════");
        println!("  INGESTION REPORT");
        println!("═══════════════════════════════════════");
        for (source, count) in &self.sources_processed {
            println!("  • {source}: {count} items processed");
        }
        println!("─────────────────────────────────────");
        println!("  Memories:     {}", self.memories_created);
        println!("  Experiences:  {}", self.experiences_created);
        println!("  Documents:    {}", self.documents_created);
        println!("  Sessions:     {}", self.sessions_created);
        if self.errors > 0 {
            println!("  ❌ Errors:      {}", self.errors);
        }
        if self.warnings > 0 {
            println!("  ⚠️  Warnings:    {}", self.warnings);
        }
        println!("═══════════════════════════════════════\n");
    }
}

/// Shared ingestion engine wrapping a PgPool.
pub struct IngestEngine {
    pool: PgPool,
}

impl IngestEngine {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Returns the pool reference for sub-modules that need it.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Check whether a memory with the given opencode session ID already exists.
    pub async fn session_already_ingested(&self, opencode_session_id: &str) -> Result<bool> {
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM memories WHERE metadata @> $1::jsonb)")
                .bind(serde_json::json!({"opencode_session_id": opencode_session_id}).to_string())
                .fetch_one(&self.pool)
                .await
                .context("checking if session already ingested")?;
        Ok(exists)
    }

    /// Insert a session record. Returns the new session UUID.
    pub async fn insert_session(
        &self,
        goal: &str,
        status: &str,
        summary: Option<&str>,
        started_at: chrono::DateTime<Utc>,
        ended_at: Option<chrono::DateTime<Utc>>,
    ) -> Result<Uuid> {
        let id = Uuid::new_v4();
        sqlx::query(
            r#"
            INSERT INTO sessions (id, goal, status, summary, started_at, ended_at)
            VALUES ($1, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(id)
        .bind(goal)
        .bind(status)
        .bind(summary)
        .bind(started_at)
        .bind(ended_at)
        .execute(&self.pool)
        .await
        .context("inserting session")?;
        Ok(id)
    }

    /// Upsert a source-backed session without creating a duplicate row.
    pub async fn upsert_source_session(
        &self, source_key: &str, goal: &str, status: &str, summary: Option<&str>,
        started_at: chrono::DateTime<Utc>, ended_at: Option<chrono::DateTime<Utc>>,
    ) -> Result<Uuid> {
        sqlx::query_scalar(
            r#"INSERT INTO sessions (goal,status,summary,started_at,ended_at,source_key)
               VALUES ($1,$2,$3,$4,$5,$6)
               ON CONFLICT (source_key) DO UPDATE SET goal=EXCLUDED.goal,
                 status=EXCLUDED.status, summary=EXCLUDED.summary,
                 ended_at=EXCLUDED.ended_at, updated_at=now()
               RETURNING id"#,
        )
        .bind(goal).bind(status).bind(summary).bind(started_at).bind(ended_at).bind(source_key)
        .fetch_one(&self.pool).await.context("upserting source session")
    }

    /// Insert an experience record.
    #[allow(clippy::too_many_arguments)]
    pub async fn insert_experience(
        &self,
        session_id: Option<Uuid>,
        goal: &str,
        reasoning_summary: Option<&str>,
        result: Option<&str>,
        lessons_learned: Option<&str>,
        tags: &[String],
        duration_seconds: Option<i32>,
        related_project: Option<&str>,
    ) -> Result<Uuid> {
        let id = Uuid::new_v4();
        sqlx::query(
            r#"
            INSERT INTO experiences (id, session_id, goal, reasoning_summary, result,
                                     lessons_learned, tags, duration_seconds, related_project)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            "#,
        )
        .bind(id)
        .bind(session_id)
        .bind(goal)
        .bind(reasoning_summary)
        .bind(result)
        .bind(lessons_learned)
        .bind(tags)
        .bind(duration_seconds)
        .bind(related_project)
        .execute(&self.pool)
        .await
        .context("inserting experience")?;
        Ok(id)
    }

    /// Insert a memory record.
    #[allow(clippy::too_many_arguments)]
    pub async fn insert_memory(
        &self,
        session_id: Option<Uuid>,
        content: &str,
        content_type: &str,
        importance: f64,
        tags: &[String],
        metadata: &Value,
    ) -> Result<Uuid> {
        let id = Uuid::new_v4();
        sqlx::query(
            r#"
            INSERT INTO memories (id, session_id, content, content_type, importance, tags, metadata)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            "#,
        )
        .bind(id)
        .bind(session_id)
        .bind(content)
        .bind(content_type)
        .bind(importance)
        .bind(tags)
        .bind(metadata)
        .execute(&self.pool)
        .await
        .context("inserting memory")?;
        Ok(id)
    }

    /// Insert a document. Returns the new document UUID, or None if a duplicate path exists.
    pub async fn insert_document(
        &self,
        path: &str,
        vault_section: Option<&str>,
        title: Option<&str>,
        content: &str,
        checksum: Option<&str>,
        frontmatter: &Value,
        file_size_bytes: Option<i32>,
        file_modified_at: Option<chrono::DateTime<Utc>>,
    ) -> Result<Option<Uuid>> {
        // Check for duplicate path
        let exists: Option<Uuid> = sqlx::query_scalar("SELECT id FROM documents WHERE path = $1")
            .bind(path)
            .fetch_optional(&self.pool)
            .await
            .context("checking document path")?;

        if let Some(id) = exists {
            info!("Document already exists (path={path}), updating content");
            sqlx::query(
                r#"
                UPDATE documents SET content = $1, frontmatter = $2, file_size_bytes = $3,
                    file_modified_at = $4, checksum = COALESCE($5, checksum),
                    updated_at = now()
                WHERE id = $6
                "#,
            )
            .bind(content)
            .bind(frontmatter)
            .bind(file_size_bytes)
            .bind(file_modified_at)
            .bind(checksum)
            .bind(id)
            .execute(&self.pool)
            .await
            .context("updating existing document")?;
            return Ok(Some(id));
        }

        let id = Uuid::new_v4();
        sqlx::query(
            r#"
            INSERT INTO documents (id, path, vault_section, title, content, checksum,
                                   frontmatter, file_size_bytes, file_modified_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            "#,
        )
        .bind(id)
        .bind(path)
        .bind(vault_section)
        .bind(title)
        .bind(content)
        .bind(checksum)
        .bind(frontmatter)
        .bind(file_size_bytes)
        .bind(file_modified_at)
        .execute(&self.pool)
        .await
        .context("inserting document")?;
        Ok(Some(id))
    }

    /// Ensure an agents row exists for the given name. Returns the agent UUID.
    pub async fn ensure_agent(&self, name: &str) -> Result<Uuid> {
        let existing: Option<Uuid> = sqlx::query_scalar("SELECT id FROM agents WHERE name = $1")
            .bind(name)
            .fetch_optional(&self.pool)
            .await
            .context("looking up agent")?;

        if let Some(id) = existing {
            return Ok(id);
        }

        let id = Uuid::new_v4();
        sqlx::query(
            r#"
            INSERT INTO agents (id, name, capabilities, metadata)
            VALUES ($1, $2, $3, $4)
            "#,
        )
        .bind(id)
        .bind(name)
        .bind(serde_json::json!(["ingested"]))
        .bind(serde_json::json!({"source": "ingest", "imported_at": Utc::now()}))
        .execute(&self.pool)
        .await
        .context("inserting agent")?;
        Ok(id)
    }
}
