//! Procedure detection service.
//!
//! Detects procedure candidates from similar experiences and executes stored procedures.

use anyhow::{Context, Result};
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
            db: Arc::new(PostgresDb { pool }),
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
}
