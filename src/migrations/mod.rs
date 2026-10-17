//! Database migration runner for the Memory Platform.
//!
//! SQL files are embedded via `include_str!` at compile time so the binary
//! is self-contained and works from any working directory.

use anyhow::{Context, Result};
use sqlx::PgPool;

/// `pg_advisory_xact_lock` key for migration runs (arbitrary, fixed).
const MIGRATION_LOCK_KEY: i64 = 0x6d65_6d6f_7279_6d67; // "memorymg"

/// Migration runner.
///
/// Uses a `_migrations` table to track applied versions.
/// Migration SQL is embedded in the binary — no filesystem dependency at runtime.
pub struct Migrator;

impl Migrator {
    /// Run all pending migrations.
    pub async fn run(pool: &PgPool) -> Result<()> {
        // Create the _migrations table if it doesn't exist. Under the same
        // lock as the migrations below: two concurrent CREATE TABLE IF NOT
        // EXISTS can still collide in the catalog.
        let mut tx = pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(MIGRATION_LOCK_KEY)
            .execute(&mut *tx)
            .await
            .context("Failed to take the migration lock")?;
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS _migrations (
                version TEXT PRIMARY KEY,
                applied_at TIMESTAMPTZ DEFAULT now()
            );
            "#,
        )
        .execute(&mut *tx)
        .await
        .context("Failed to create _migrations table")?;
        tx.commit().await?;

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
            (
                "013_neon_event_seq",
                include_str!("../../migrations/013_neon_event_seq.sql"),
            ),
            (
                "014_sync_scope",
                include_str!("../../migrations/014_sync_scope.sql"),
            ),
            (
                "015_embeddings_multi_model",
                include_str!("../../migrations/015_embeddings_multi_model.sql"),
            ),
            (
                "016_contradiction_resolution",
                include_str!("../../migrations/016_contradiction_resolution.sql"),
            ),
            (
                "017_memory_expiration",
                include_str!("../../migrations/017_memory_expiration.sql"),
            ),
            (
                "018_bitemporal_validity",
                include_str!("../../migrations/018_bitemporal_validity.sql"),
            ),
            (
                "019_fix_partial_index_direction",
                include_str!("../../migrations/019_fix_partial_index_direction.sql"),
            ),
            (
                "020_contradictions_unique_pair",
                include_str!("../../migrations/020_contradictions_unique_pair.sql"),
            ),
            (
                "021_index_and_importance_bounds",
                include_str!("../../migrations/021_index_and_importance_bounds.sql"),
            ),
        ];

        // Apply pending migrations in order. Each migration and its ledger row
        // commit together, so a failure part-way through a multi-statement
        // migration leaves nothing half-applied and unrecorded. The advisory
        // lock serializes concurrent runners (API daemon, ingest, neon-sync on
        // one database); the re-check under it skips what another runner
        // applied in the meantime.
        for (version, sql) in embedded {
            if applied.contains(&version.to_string()) {
                continue;
            }

            let mut tx = pool.begin().await?;
            sqlx::query("SELECT pg_advisory_xact_lock($1)")
                .bind(MIGRATION_LOCK_KEY)
                .execute(&mut *tx)
                .await
                .context("Failed to take the migration lock")?;
            let already: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM _migrations WHERE version = $1)")
                    .bind(version)
                    .fetch_one(&mut *tx)
                    .await?;
            if already {
                continue;
            }

            sqlx::raw_sql(sql)
                .execute(&mut *tx)
                .await
                .with_context(|| format!("Failed to execute migration: {}", version))?;

            sqlx::query("INSERT INTO _migrations (version) VALUES ($1)")
                .bind(version)
                .execute(&mut *tx)
                .await
                .with_context(|| format!("Failed to record migration: {}", version))?;
            tx.commit()
                .await
                .with_context(|| format!("Failed to commit migration: {}", version))?;

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

        Migrator::run(&pool).await.expect("Migration run failed");
        let count = || async {
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM _migrations")
                .fetch_one(&pool)
                .await
                .expect("Query failed")
        };
        let applied = count().await;
        assert!(applied > 0, "every applied migration must be recorded");

        // A second run must be a no-op, not a re-apply or an error.
        Migrator::run(&pool).await.expect("Re-run failed");
        assert_eq!(count().await, applied);
    }

    /// Two runners on one fresh database (API daemon and ingest both
    /// migrate at startup) must both succeed and apply each migration once.
    /// Point DATABASE_URL at an empty throwaway database.
    #[tokio::test]
    #[ignore = "requires an empty, reachable Postgres database"]
    async fn concurrent_runs_apply_each_migration_once() {
        let database_url =
            env::var("DATABASE_URL").expect("DATABASE_URL is required for migration tests");
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .connect(&database_url)
            .await
            .expect("Failed to connect to database");
        let (a, b) = tokio::join!(Migrator::run(&pool), Migrator::run(&pool));
        a.expect("first runner failed");
        b.expect("second runner failed");
        let duplicates: i64 =
            sqlx::query_scalar("SELECT count(*) - count(DISTINCT version) FROM _migrations")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(duplicates, 0);
    }
}
