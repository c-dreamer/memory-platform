//! Neo4j graph client for relationship and graph queries.
//!
//! Wraps the `neo4rs` crate with connect/query/health operations.
//! The `neo4j` Cargo feature enables the `neo4rs` dependency.

use anyhow::{Context, Result};

/// Neo4j graph database client.
pub struct GraphClient {
    /// The underlying neo4rs graph connection (optional behind `neo4j` feature).
    #[cfg(feature = "neo4j")]
    graph: neo4rs::Graph,
}

impl GraphClient {
    /// Connect to a Neo4j database.
    ///
    /// # Arguments
    ///
    /// * `uri` — the Neo4j bolt URI (e.g. `bolt://localhost:7687`).
    /// * `user` — the Neo4j user name.
    /// * `password` — the Neo4j password.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection fails or the `neo4j` feature is disabled.
    pub async fn connect(uri: &str, user: &str, password: &str) -> Result<Self> {
        #[cfg(feature = "neo4j")]
        {
            let config = neo4rs::ConfigBuilder::new()
                .uri(uri)
                .user(user)
                .password(password)
                .build()
                .context("Failed to build Neo4j config")?;
            let graph = neo4rs::Graph::connect(config)
                .with_context(|| format!("Failed to connect to Neo4j at {uri}"))?;
            Ok(Self { graph })
        }

        #[cfg(not(feature = "neo4j"))]
        {
            let _ = (uri, user, password);
            anyhow::bail!("Neo4j support is disabled: compile with `--features neo4j`")
        }
    }

    /// Run a Cypher query and return result rows.
    #[cfg(feature = "neo4j")]
    pub async fn run_query(&self, cypher: &str) -> Result<Vec<neo4rs::Row>> {
        let mut stream = self
            .graph
            .execute(cypher)
            .await
            .context("Neo4j query execution failed")?;

        let mut rows = Vec::new();
        while let Ok(Some(row)) = stream.next().await {
            rows.push(row);
        }
        Ok(rows)
    }

    /// Health check: runs `RETURN 1` and verifies connectivity.
    pub async fn health_check(&self) -> Result<()> {
        #[cfg(feature = "neo4j")]
        {
            let mut stream = self
                .graph
                .execute(neo4rs::query("RETURN 1 AS ok"))
                .await
                .context("Neo4j health check query failed")?;

            let _ = stream
                .next()
                .await
                .context("Neo4j health check returned no rows")?;
            Ok(())
        }

        #[cfg(not(feature = "neo4j"))]
        {
            anyhow::bail!("Neo4j support is disabled")
        }
    }
}
