use axum::{extract::State, http::StatusCode, Json};
use std::sync::Arc;
use uuid::Uuid;

use crate::api::auth::Auth;
use crate::api::dto::{MemoryCreateRequest, MemoryCreateResponse};
use crate::AppState;

fn err(e: impl std::fmt::Display) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({"error": e.to_string()})),
    )
}

pub async fn store_memory(
    _auth: Auth,
    State(state): State<Arc<AppState>>,
    Json(body): Json<MemoryCreateRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let agent_id = match body.agent_id.as_deref().map(Uuid::parse_str) {
        Some(Ok(id)) => Some(id),
        Some(Err(_)) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "invalid agent_id: not a UUID"})),
            )
        }
        None => None,
    };
    let session_id = match body.session_id.as_deref().map(Uuid::parse_str) {
        Some(Ok(id)) => Some(id),
        Some(Err(_)) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "invalid session_id: not a UUID"})),
            )
        }
        None => None,
    };

    let embedding = match &state.embedding_service {
        Some(svc) => match svc.embed(&body.content).await {
            Ok(e) => Some(e.as_vec().to_vec()),
            Err(e) => return err(e),
        },
        None => None,
    };

    let metadata = serde_json::Value::Object(body.metadata.into_iter().collect());
    let memory = match state
        .db
        .store_memory(
            &body.content,
            &body.content_type,
            body.importance.clamp(0.0, 1.0),
            &body.tags,
            &metadata,
            agent_id,
            session_id,
            embedding.as_deref(),
            None,
            None,
        )
        .await
    {
        Ok(m) => m,
        Err(e) => return err(e),
    };

    // Contradiction detection needs an embedding (ContradictionDetector::detect
    // errors without one) and the detector service to be configured — both
    // optional, so skip gracefully rather than failing the whole store over a
    // side signal the caller didn't strictly ask to gate on.
    let (contradictions_detected, contradiction_with) =
        match (&state.contradiction_detector, &embedding) {
            (Some(detector), Some(_)) => match detector.detect(&memory.id.to_string()).await {
                Ok(found) if found.is_empty() => (Some(false), None),
                Ok(found) => (
                    Some(true),
                    Some(
                        found
                            .iter()
                            .map(|c| c.memory_id_b.to_string())
                            .collect::<Vec<_>>(),
                    ),
                ),
                Err(_) => (None, None),
            },
            _ => (None, None),
        };

    (
        StatusCode::OK,
        Json(
            serde_json::to_value(MemoryCreateResponse {
                id: memory.id.to_string(),
                status: "stored".into(),
                contradictions_detected,
                contradiction_with,
            })
            .expect("MemoryCreateResponse always serializes"),
        ),
    )
}

pub async fn get_memory(
    _auth: Auth,
    State(state): State<Arc<AppState>>,
    axum::extract::Path(memory_id): axum::extract::Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    let id = match Uuid::parse_str(&memory_id) {
        Ok(id) => id,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "invalid memory_id: not a UUID"})),
            )
        }
    };

    match state.db.get_memory(id).await {
        Ok(Some(memory)) => match serde_json::to_value(memory) {
            Ok(v) => (StatusCode::OK, Json(v)),
            Err(e) => err(e),
        },
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "memory not found"})),
        ),
        Err(e) => err(e),
    }
}
