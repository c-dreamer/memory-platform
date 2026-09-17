//! Local SQLite WAL-mode pending-writes queue.
//!
//! A durable write cache ahead of PostgreSQL for `POST /events`: the caller's
//! payload is written here first, then applied to Postgres. Success removes
//! the row; on failure it stays queued for a background drain task, so a
//! momentary Postgres outage never silently loses an event the way returning
//! a bare error to the caller would.

use anyhow::{Context, Result};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Row, SqlitePool};
use std::path::{Path, PathBuf};
use std::str::FromStr;

// ponytail: row-count cap, not age-bounded — raise this or add real
// backpressure if a real outage is ever long enough to hit it.
const MAX_QUEUED_ROWS: i64 = 10_000;

/// One queued event awaiting replay into Postgres.
#[derive(Debug, Clone)]
pub struct QueuedWrite {
    pub id: i64,
    pub payload: serde_json::Value,
}

/// Local durability queue, independent of and ahead of Postgres.
#[derive(Debug, Clone)]
pub struct PendingWriteQueue {
    pool: SqlitePool,
}

impl PendingWriteQueue {
    /// Test-only placeholder that never opens a real database file, mirroring
    /// `PostgresDb::new_empty()` / `SearchEngine::new_empty()`.
    pub fn new_empty() -> Self {
        let options = SqliteConnectOptions::from_str("sqlite::memory:")
            .expect("in-memory sqlite URL should always parse");
        Self {
            pool: SqlitePoolOptions::new().connect_lazy_with(options),
        }
    }

    /// `dirs::data_local_dir()/memory-platform/pending-writes.db` — the same
    /// platform-appropriate resolution the rest of this crate uses (see
    /// CLAUDE.md: prefer `dirs`-style resolution over hardcoded paths).
    pub fn default_path() -> Result<PathBuf> {
        let dir = dirs::data_local_dir()
            .context("could not determine a local data directory for the pending-writes queue")?
            .join("memory-platform");
        Ok(dir.join("pending-writes.db"))
    }

    pub async fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("could not create {}", parent.display()))?;
        }
        let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))
            .with_context(|| format!("invalid pending-writes queue path {}", path.display()))?
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Full);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .with_context(|| {
                format!("could not open pending-writes queue at {}", path.display())
            })?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS pending_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                payload TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
            )",
        )
        .execute(&pool)
        .await
        .context("failed to create pending_events table")?;
        Ok(Self { pool })
    }

    /// Durably record `payload`. Returns the queue row id.
    ///
    /// The cap check and the insert are one statement (not a separate SELECT
    /// then INSERT) so two concurrent callers can't both pass the check
    /// before either row lands — with `max_connections(1)`, sqlx already
    /// serializes whole statements against the single connection, so a single
    /// statement is all that's needed for atomicity here.
    pub async fn enqueue(&self, payload: &serde_json::Value) -> Result<i64> {
        let text = serde_json::to_string(payload).context("failed to serialize queued payload")?;
        let id: Option<i64> = sqlx::query_scalar(
            "INSERT INTO pending_events(payload)
             SELECT ? WHERE (SELECT count(*) FROM pending_events) < ?
             RETURNING id",
        )
        .bind(text)
        .bind(MAX_QUEUED_ROWS)
        .fetch_optional(&self.pool)
        .await
        .context("failed to enqueue pending write")?;
        id.ok_or_else(|| {
            anyhow::anyhow!(
                "pending-writes queue is full ({MAX_QUEUED_ROWS} rows) — Postgres has been unreachable too long"
            )
        })
    }

    /// Drop a row once it has been durably applied to Postgres.
    pub async fn remove(&self, id: i64) -> Result<()> {
        sqlx::query("DELETE FROM pending_events WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await
            .context("failed to remove drained pending write")?;
        Ok(())
    }

    pub async fn depth(&self) -> Result<i64> {
        sqlx::query_scalar("SELECT count(*) FROM pending_events")
            .fetch_one(&self.pool)
            .await
            .context("failed to read pending-writes queue depth")
    }

    /// The oldest `limit` queued writes, for a background drain task.
    pub async fn oldest(&self, limit: i64) -> Result<Vec<QueuedWrite>> {
        let rows = sqlx::query("SELECT id, payload FROM pending_events ORDER BY id LIMIT ?")
            .bind(limit)
            .fetch_all(&self.pool)
            .await
            .context("failed to read pending writes")?;
        rows.into_iter()
            .map(|row| -> Result<QueuedWrite> {
                let id: i64 = row.try_get("id")?;
                let raw: String = row.try_get("payload")?;
                let payload = serde_json::from_str(&raw).context("corrupt queued payload")?;
                Ok(QueuedWrite { id, payload })
            })
            .collect()
    }

    /// The oldest `limit` queued writes that are durably older than
    /// `min_age_secs`, for a background drain task.
    ///
    /// There is no per-row lease/claim here — age is the cheap substitute.
    /// The request handler that enqueues a row also removes it once its own
    /// Postgres write succeeds, normally within milliseconds; a row is only
    /// still present past `min_age_secs` if that handler crashed or is
    /// genuinely stuck, which is exactly when a background replay is safe to
    /// attempt. Without this filter, a periodic drain tick can race a still
    /// in-flight request for the same row and double-apply it.
    pub async fn stale(&self, limit: i64, min_age_secs: i64) -> Result<Vec<QueuedWrite>> {
        // created_at is stored via strftime('%Y-%m-%dT%H:%M:%fZ', ...) (see the
        // CREATE TABLE above); the threshold below must use the exact same
        // format, not datetime()'s "YYYY-MM-DD HH:MM:SS" — the two are
        // compared as plain TEXT, and datetime()'s space separator sorts
        // before 'T', so a mismatched format silently never matches any row
        // created on the same UTC calendar day as `now`.
        let rows = sqlx::query(
            "SELECT id, payload FROM pending_events
             WHERE created_at <= strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-' || ? || ' seconds')
             ORDER BY id LIMIT ?",
        )
        .bind(min_age_secs)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .context("failed to read stale pending writes")?;
        rows.into_iter()
            .map(|row| -> Result<QueuedWrite> {
                let id: i64 = row.try_get("id")?;
                let raw: String = row.try_get("payload")?;
                let payload = serde_json::from_str(&raw).context("corrupt queued payload")?;
                Ok(QueuedWrite { id, payload })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn enqueue_remove_roundtrip() {
        let dir = std::env::temp_dir().join(format!(
            "memory-platform-queue-test-{}",
            uuid::Uuid::new_v4()
        ));
        let path = dir.join("pending-writes.db");
        let queue = PendingWriteQueue::open(&path).await.expect("open queue");

        assert_eq!(queue.depth().await.unwrap(), 0);
        let id = queue.enqueue(&serde_json::json!({"a": 1})).await.unwrap();
        assert_eq!(queue.depth().await.unwrap(), 1);

        let oldest = queue.oldest(10).await.unwrap();
        assert_eq!(oldest.len(), 1);
        assert_eq!(oldest[0].id, id);
        assert_eq!(oldest[0].payload, serde_json::json!({"a": 1}));

        queue.remove(id).await.unwrap();
        assert_eq!(queue.depth().await.unwrap(), 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn oldest_returns_rows_in_insertion_order() {
        let dir = std::env::temp_dir().join(format!(
            "memory-platform-queue-test-{}",
            uuid::Uuid::new_v4()
        ));
        let path = dir.join("pending-writes.db");
        let queue = PendingWriteQueue::open(&path).await.expect("open queue");

        for i in 0..5 {
            queue.enqueue(&serde_json::json!({"i": i})).await.unwrap();
        }
        assert_eq!(queue.depth().await.unwrap(), 5);
        let oldest = queue.oldest(2).await.unwrap();
        assert_eq!(oldest.len(), 2);
        assert_eq!(oldest[0].payload, serde_json::json!({"i": 0}));
        assert_eq!(oldest[1].payload, serde_json::json!({"i": 1}));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn stale_uses_matching_timestamp_format() {
        let dir = std::env::temp_dir().join(format!(
            "memory-platform-queue-test-{}",
            uuid::Uuid::new_v4()
        ));
        let path = dir.join("pending-writes.db");
        let queue = PendingWriteQueue::open(&path).await.expect("open queue");

        let recent_id = queue
            .enqueue(&serde_json::json!({"age": "recent"}))
            .await
            .unwrap();
        let old_id = queue
            .enqueue(&serde_json::json!({"age": "old"}))
            .await
            .unwrap();
        // Backdate `old_id` 5 minutes into the past, same UTC calendar day as
        // `now` — the exact case that silently never matched when the
        // threshold expression used a different string format than
        // created_at's own strftime format (space vs 'T' separator sorts
        // "old today" as newer than "now minus 30s").
        sqlx::query(
            "UPDATE pending_events
             SET created_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-5 minutes')
             WHERE id = ?",
        )
        .bind(old_id)
        .execute(&queue.pool)
        .await
        .unwrap();

        let stale = queue.stale(10, 30).await.unwrap();
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].id, old_id);
        assert_ne!(stale[0].id, recent_id);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
