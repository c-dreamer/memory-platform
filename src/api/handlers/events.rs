use axum::{extract::State, Json};
use std::sync::Arc;

use crate::api::auth::Auth;
use crate::api::dto::{EventCreate, EventResponse};
use crate::AppState;
use serde_json::json;
use uuid::Uuid;

pub async fn ingest_event(
    _auth: Auth,
    State(state): State<Arc<AppState>>,
    Json(body): Json<EventCreate>,
) -> Json<EventResponse> {
    tracing::info!(
        "ingest_event called: agent_id={:?}, type={:?}",
        body.agent_id,
        body.event_type
    );

    // Parse session_id if provided
    let session_id = body.session_id.and_then(|s| Uuid::parse_str(&s).ok());
    let agent_id = body.agent_id.and_then(|s| Uuid::parse_str(&s).ok());

    // Build content from event
    let summary = body
        .payload
        .get("summary")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let details = body
        .payload
        .get("details")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let content = format!("{}\n\n{}", summary, details).trim().to_string();

    let event_type = if body.event_type.is_empty() {
        "task_complete".to_string()
    } else {
        body.event_type.clone()
    };

    tracing::info!(
        "Storing event as memory: content_len={}, event_type={}",
        content.len(),
        event_type
    );

    let embedding = match &state.embedding_service {
        Some(svc) => match svc.embed(&content).await {
            Ok(emb) => Some(emb.as_vec().to_vec()),
            Err(e) => {
                tracing::warn!("Failed to embed event content: {e}");
                None
            }
        },
        None => None,
    };

    // Store as a memory with event metadata
    let memory = state
        .db
        .store_memory(
            &content,
            "event",
            0.7,
            &[event_type],
            &json!(body.payload),
            agent_id,
            session_id,
            embedding.as_deref(),
        )
        .await;

    tracing::info!("store_memory result: {:?}", memory.is_ok());

    match memory {
        Ok(m) => Json(EventResponse {
            event_id: m.id.to_string(),
            memory_id: m.id.to_string(),
            status: "accepted".to_string(),
            table: "memories".to_string(),
        }),
        Err(e) => {
            tracing::error!("Failed to ingest event: {}", e);
            Json(EventResponse {
                event_id: Uuid::new_v4().to_string(),
                memory_id: Uuid::new_v4().to_string(),
                status: "error".to_string(),
                table: "memories".to_string(),
            })
        }
    }
}
