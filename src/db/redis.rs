//! Redis cache layer.
//!
//! Provides a `RedisCache` struct backed by the `redis` crate with
//! `get`, `set`, `delete`, and `health_check` operations.

use anyhow::{Context, Result};
use redis::aio::ConnectionManager;
use std::time::Duration;

/// Redis cache client with a managed connection pool.
pub struct RedisCache {
    /// Managed async connection to Redis.
    conn: ConnectionManager,
}

impl RedisCache {
    /// Connect to a Redis server.
    pub async fn connect(url: &str) -> Result<Self> {
        let client = redis::Client::open(url)
            .with_context(|| format!("Failed to open Redis connection to {url}"))?;
        let conn = ConnectionManager::new(client)
            .await
            .with_context(|| format!("Failed to create Redis connection manager to {url}"))?;
        Ok(Self { conn })
    }

    /// Get a value by key. Returns `None` if the key does not exist.
    pub async fn get(&self, key: &str) -> Result<Option<String>> {
        let mut conn = self.conn.clone();
        redis::cmd("GET")
            .arg(key)
            .query_async(&mut conn)
            .await
            .with_context(|| format!("Redis GET {key:?} failed"))
    }

    /// Set a value with an optional TTL in seconds.
    pub async fn set(&self, key: &str, value: &str, ttl_secs: Option<u64>) -> Result<()> {
        let mut conn = self.conn.clone();
        if let Some(ttl) = ttl_secs {
            redis::cmd("SET")
                .arg(key)
                .arg(value)
                .arg("EX")
                .arg(ttl)
                .query_async::<()>(&mut conn)
                .await
                .with_context(|| format!("Redis SET {key:?} with TTL failed"))?;
        } else {
            redis::cmd("SET")
                .arg(key)
                .arg(value)
                .query_async::<()>(&mut conn)
                .await
                .with_context(|| format!("Redis SET {key:?} failed"))?;
        }
        Ok(())
    }

    /// Delete a key. Returns the number of keys removed (0 or 1).
    pub async fn delete(&self, key: &str) -> Result<u64> {
        let mut conn = self.conn.clone();
        redis::cmd("DEL")
            .arg(key)
            .query_async(&mut conn)
            .await
            .with_context(|| format!("Redis DEL {key:?} failed"))
    }

    /// Health check: runs PING.
    pub async fn health_check(&self) -> Result<()> {
        let mut conn = self.conn.clone();
        let pong: String = redis::cmd("PING")
            .query_async(&mut conn)
            .await
            .context("Redis health check PING failed")?;
        if pong.to_uppercase() != "PONG" {
            anyhow::bail!("Redis health check returned unexpected response: {pong}");
        }
        Ok(())
    }

    /// Set with a standard Duration TTL.
    pub async fn set_with_duration(&self, key: &str, value: &str, ttl: Duration) -> Result<()> {
        self.set(key, value, Some(ttl.as_secs())).await
    }
}
