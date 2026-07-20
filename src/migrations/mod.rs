//! Database migration runner for the Memory Platform.
//!
//! SQL files are embedded via `include_str!` at compile time so the binary
//! is self-contained and works from any working directory.

use anyhow::{Context, Result};
use sqlx::PgPool;

/// Migration runner.
///
/// Uses a `_migrations` table to track applied versions.
/// Migration SQL is embedded in the binary — no filesystem dependency at runtime.
pub struct Migrator;

impl Migrator {
    /// Run all pending migrations.
    pub async fn run(pool: &PgPool) -> Result<()> {
        // Create the _migrations table if it doesn't exist
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS _migrations (
                version TEXT PRIMARY KEY,
                applied_at TIMESTAMPTZ DEFAULT now()
            );
            "#,
        )
        .execute(pool)
        .await
        .context("Failed to create _migrations table")?;

        // Get already-applied migrations
        let applied: Vec<String> = sqlx::query_scalar("SELECT version FROM _migrations")
            .fetch_all(pool)
            .await
            .context("Failed to fetch applied migrations")?;

        // Embedded migrations — version -> SQL mapping via include_str!
        let embedded: &[(&str, &str)] = &[
            (
                "001_initial",
                include_str!("../../migrations/001_initial.sql"),
            ),
            (
                "002_hybrid_decay_contradiction",
                include_str!("../../migrations/002_hybrid_decay_contradiction.sql"),
            ),
            (
                "003_session_vault_xref",
                include_str!("../../migrations/003_session_vault_xref.sql"),
            ),
            (
                "004_embeddings_unique_source",
                include_str!("../../migrations/004_embeddings_unique_source.sql"),
            ),
            (
                "005_embeddings_2048",
                include_str!("../../migrations/005_embeddings_2048.sql"),
            ),
            (
                "006_updated_at_triggers",
                include_str!("../../migrations/006_updated_at_triggers.sql"),
            ),
            (
                "007_code_changes_embedding_2048",
                include_str!("../../migrations/007_code_changes_embedding_2048.sql"),
            ),
            (
                "008_storage_tiers_and_archive",
                include_str!("../../migrations/008_storage_tiers_and_archive.sql"),
            ),
            (
                "009_sync_key_uniqueness",
                include_str!("../../migrations/009_sync_key_uniqueness.sql"),
            ),
            (
                "010_shared_event_sync",
                include_str!("../../migrations/010_shared_event_sync.sql"),
            ),
            (
                "010_session_source_keys",
                include_str!("../../migrations/010_session_source_keys.sql"),
            ),
            (
                "011_derived_embeddings_2048",
                include_str!("../../migrations/011_derived_embeddings_2048.sql"),
            ),
            (
                "012_fts_input_size_guard",
                include_str!("../../migrations/012_fts_input_size_guard.sql"),
            ),
        ];

        // Apply pending migrations in order
        for (version, sql) in embedded {
            if applied.contains(&version.to_string()) {
                continue;
            }

            sqlx::raw_sql(sql)
                .execute(pool)
                .await
                .with_context(|| format!("Failed to execute migration: {}", version))?;

            sqlx::query("INSERT INTO _migrations (version) VALUES ($1)")
                .bind(version)
                .execute(pool)
                .await
                .with_context(|| format!("Failed to record migration: {}", version))?;

            tracing::info!("Applied migration: {version}");
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::postgres::PgPoolOptions;
    use std::env;

    #[tokio::test]
    #[ignore = "requires a reachable Postgres instance"]
    async fn test_migrations_table_creation() {
        let database_url =
            env::var("DATABASE_URL").expect("DATABASE_URL is required for migration tests");
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(&database_url)
            .await
            .expect("Failed to connect to database");

        // Ensure the _migrations table exists
        Migrator::run(&pool).await.expect("Migration run failed");

        // Verify the table exists
        let result = sqlx::query_scalar::<_, i32>("SELECT 1 FROM _migrations LIMIT 1")
            .fetch_optional(&pool)
            .await
            .expect("Query failed");
        assert!(result.is_none()); // Table exists but is empty
    }
}
