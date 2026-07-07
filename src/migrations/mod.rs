//! Database migration runner for the Memory Platform.
//!
//! Uses a simple file-based migration system with a `_migrations` tracking table.
//! Migrations are applied in order, and already-applied versions are skipped.
//!
//! Design:
//! - Migrations are SQL files in the `migrations/` directory, named `NNN_name.sql`.
//! - The `_migrations` table tracks which versions have been applied.
//! - The `Migrator` struct provides an async `run(pool: &PgPool) -> Result<()>` method.
//! - Uses `sqlx::query` for raw SQL execution (no sqlx::migrate! macro).

use anyhow::{Context, Result};
use sqlx::PgPool;
use std::fs;
use std::path::Path;

/// Migration runner.
///
/// Scans the `migrations/` directory, reads SQL files, and applies them in order.
/// Uses a `_migrations` table to track applied versions.
pub struct Migrator;

impl Migrator {
    /// Run all pending migrations.
    ///
    /// Creates the `_migrations` table if it doesn't exist.
    /// Reads migration files from the `migrations/` directory (relative to CWD at runtime).
    /// Applies them in order, skipping already-applied versions.
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

        // Read migration files from the migrations/ directory
        let migrations_dir = Path::new("migrations");
        let mut migrations = fs::read_dir(migrations_dir)
            .context("Failed to read migrations directory")?
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let path = entry.path();
                if path.extension().map_or(false, |ext| ext == "sql") {
                    let version = path.file_stem()?.to_string_lossy().into_owned();
                    Some((version, path))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        // Sort migrations by version (lexicographic order)
        migrations.sort_by(|a, b| a.0.cmp(&b.0));

        // Apply pending migrations
        for (version, path) in migrations {
            if applied.contains(&version) {
                continue; // Skip already-applied migrations
            }

            let sql = fs::read_to_string(&path)
                .with_context(|| format!("Failed to read migration file: {}", path.display()))?;

            // Execute the migration (raw_sql supports multi-statement SQL)
            sqlx::raw_sql(&sql)
                .execute(pool)
                .await
                .with_context(|| format!("Failed to execute migration: {}", version))?;

            // Record the migration as applied
            sqlx::query("INSERT INTO _migrations (version) VALUES ($1)")
                .bind(&version)
                .execute(pool)
                .await
                .with_context(|| format!("Failed to record migration: {}", version))?;

            println!("Applied migration: {}", version);
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
        let database_url = env::var("DATABASE_URL")
            .unwrap_or_else(|_| {
                "postgresql://memory:YAft44tZyrG4DET0WeigY8BpZ%252BcqGgPtTXsPK4XFgXc%253D@127.0.0.1:5433/memory"
                    .to_string()
            });
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
