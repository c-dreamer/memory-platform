use axum::{extract::State, Json};
use std::sync::Arc;

use crate::api::auth::Auth;
use crate::api::dto::{MemoryCreateRequest, MemoryCreateResponse};
use crate::AppState;

pub async fn store_memory(
    _auth: Auth,
    State(_state): State<Arc<AppState>>,
    Json(_body): Json<MemoryCreateRequest>,
) -> Json<MemoryCreateResponse> {
    todo!("store_memory")
}

pub async fn get_memory(
    _auth: Auth,
    State(_state): State<Arc<AppState>>,
    axum::extract::Path(_memory_id): axum::extract::Path<String>,
) -> Json<serde_json::Value> {
    todo!("get_memory")
}
