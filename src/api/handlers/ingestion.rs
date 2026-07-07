use axum::{Json, extract::State};
use std::sync::Arc;

use crate::AppState;
use crate::api::auth::Auth;
use crate::api::dto::IngestRequest;

pub async fn trigger_ingest(
    _auth: Auth,
    State(_state): State<Arc<AppState>>,
    Json(_body): Json<IngestRequest>,
) -> Json<serde_json::Value> {
    todo!("trigger_ingest")
}
