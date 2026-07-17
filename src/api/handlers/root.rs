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

pub async fn health_check(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let postgres = state.db.health().await;
    Json(serde_json::json!({
        "status": if postgres { "ok" } else { "degraded" },
        "postgres": if postgres { "connected" } else { "unavailable" },
        "embedding": {
            "model": state.config.embedding_model,
            "configured_dimension": state.config.embedding_dim,
            "available": state.embedding_service.is_some(),
            "mode": if state.embedding_service.is_some() { "vector" } else { "keyword-only" },
        },
    }))
}
