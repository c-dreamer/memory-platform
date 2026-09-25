//! Procedure detection service.
//!
//! Detects procedure candidates from similar experiences and executes stored procedures.

use anyhow::{Context, Result};
use serde::Serialize;
use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;

use crate::db::postgres::PostgresDb;
use crate::models::Procedure;

/// Result of executing a procedure.
#[derive(Debug, Clone)]
pub struct ProcedureResult {
    pub success: bool,
    pub output: String,
    pub duration_ms: u64,
    pub steps_completed: usize,
}

/// One experience pair promoted into a procedure by `promote_from_experiences`.
#[derive(Debug, Clone, Serialize)]
pub struct PromotedProcedure {
    pub procedure_id: Uuid,
    pub name: String,
    pub source_experience_ids: [Uuid; 2],
    pub similarity: f64,
}

/// Procedure service.
///
/// Detects relevant procedures from context and executes them.
#[derive(Debug)]
pub struct ProcedureService {
    db: Arc<PostgresDb>,
}

impl ProcedureService {
    /// Create a new procedure service.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self {
            db: Arc::new(PostgresDb::with_pool(pool)),
        }
    }

    /// Find procedure candidates relevant to a context.
    pub async fn find_candidates(&self, context: &str) -> Result<Vec<Procedure>> {
        // Use the search engine to find procedures matching the context.
        // For now, we use a simple keyword search on the procedure name and description.
        let procedures = self
            .db
            .search_procedures(context, 5)
            .await
            .context("Failed to search procedures")?;
        Ok(procedures)
    }

    /// Execute a procedure by ID, recording execution history.
    pub async fn execute(&self, procedure_id: &str) -> Result<ProcedureResult> {
        let start_time = std::time::Instant::now();
        let id = Uuid::parse_str(procedure_id).context("Invalid procedure ID format")?;

        let procedure = self
            .db
            .get_procedure(id)
            .await
            .context("Failed to fetch procedure")?
            .ok_or_else(|| anyhow::anyhow!("Procedure not found"))?;

        // Simulate procedure execution by "running" its steps.
        // In a real implementation, this would interpret and execute the steps.
        let steps_completed = procedure
            .steps
            .as_array()
            .map(|steps| steps.len())
            .unwrap_or(0);

        // Record execution history.
        self.db
            .record_procedure_execution(id, true)
            .await
            .context("Failed to record procedure execution")?;

        Ok(ProcedureResult {
            success: true,
            output: format!("Executed procedure '{}' successfully", procedure.name),
            duration_ms: start_time.elapsed().as_millis() as u64,
            steps_completed,
        })
    }

    /// Save or update a procedure.
    pub async fn save(&self, procedure: &Procedure) -> Result<()> {
        self.db
            .update_procedure(
                procedure.id,
                &procedure.name,
                procedure.description.as_deref(),
            )
            .await
            .context("Failed to save procedure")?;
        Ok(())
    }

    /// Cluster successful experiences by embedding similarity and promote
    /// each matched pair into a reusable procedure (Memp/ProcMEM-style
    /// procedural memory). Purely mechanical: the earlier experience's own
    /// `actions`/`lessons_learned` become the procedure's steps/description
    /// verbatim, no LLM summarization. Idempotent — promoted experiences are
    /// marked so the same pair is never promoted twice. Shared by the
    /// `procedure_promote` MCP tool and the `detect_procedure_candidates`
    /// HTTP endpoint.
    pub async fn promote_from_experiences(&self, threshold: f64) -> Result<Vec<PromotedProcedure>> {
        let pairs = self
            .db
            .find_similar_experiences(threshold)
            .await
            .context("Failed to find similar experiences")?;

        let mut promoted = Vec::new();
        for pair in &pairs {
            let mut tags: Vec<String> = pair
                .tags1
                .iter()
                .chain(pair.tags2.iter())
                .cloned()
                .collect();
            tags.sort();
            tags.dedup();

            let procedure = self
                .db
                .create_procedure(
                    &pair.goal1,
                    pair.lessons_learned1.as_deref(),
                    &pair.actions1,
                    None,
                    Some(pair.id1),
                    &tags,
                )
                .await
                .context("Failed to create procedure")?;

            self.db
                .mark_experiences_procedurized(&[pair.id1, pair.id2])
                .await
                .context("Failed to mark experiences procedurized")?;

            promoted.push(PromotedProcedure {
                procedure_id: procedure.id,
                name: procedure.name,
                source_experience_ids: [pair.id1, pair.id2],
                similarity: pair.similarity,
            });
        }

        Ok(promoted)
    }
}
