use axum::{extract::State, http::StatusCode, Json};
use std::path::PathBuf;
use std::sync::Arc;

use crate::api::auth::Auth;
use crate::api::dto::IngestRequest;
use crate::ingest::vault::ingest_vault;
use crate::ingest::{IngestEngine, IngestReport};
use crate::AppState;

pub async fn trigger_ingest(
    _auth: Auth,
    State(state): State<Arc<AppState>>,
    Json(body): Json<IngestRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    if body.source != "filesystem" && body.source != "vault" {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": format!("unsupported ingest source: {} (only \"vault\"/\"filesystem\" are wired)", body.source)
            })),
        );
    }

    let engine = IngestEngine::new(state.db.pool.clone());
    let mut report = IngestReport::default();

    match ingest_vault(&engine, &PathBuf::from(&body.path), 0, false, &mut report).await {
        Ok(summary) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "status": "completed",
                "scanned": summary.scanned,
                "imported": summary.imported,
                "errors": summary.errors,
            })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        ),
    }
}
