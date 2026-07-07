use axum::{Json, extract::State};
use std::sync::Arc;

use crate::AppState;
use crate::api::dto::RootResponse;

pub async fn root() -> Json<RootResponse> {
    Json(RootResponse {
        service: "memory-platform".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        docs: "/docs".to_string(),
        health: "/health".to_string(),
    })
}

pub async fn health_check(State(_state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "postgres": "connected",
    }))
}
