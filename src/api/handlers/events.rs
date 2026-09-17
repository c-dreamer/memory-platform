use axum::{extract::State, Json};
use std::sync::Arc;

use crate::api::auth::Auth;
use crate::api::dto::{EventCreate, EventResponse};
use crate::models::memory::Memory;
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

    // Durably record the raw event before attempting Postgres, so a
    // momentary outage never silently loses it — see docs/WINDOWS_PORT_SYNTHESIS.md
    // decision #13. queue_id is None only if the queue itself is unavailable
    // (e.g. full or disk-full); ingestion still proceeds inline in that case.
    let raw = serde_json::to_value(&body).unwrap_or_else(|_| json!({}));
    let queue_id = match state.pending_writes.enqueue(&raw).await {
        Ok(id) => Some(id),
        Err(e) => {
            tracing::warn!("could not durably queue event before ingest: {e:#}");
            None
        }
    };

    match store_event(&state, &body).await {
        Ok(m) => {
            if let Some(id) = queue_id {
                if let Err(e) = state.pending_writes.remove(id).await {
                    tracing::warn!("failed to clear drained queue row {id}: {e:#}");
                }
            }
            Json(EventResponse {
                event_id: m.id.to_string(),
                memory_id: m.id.to_string(),
                status: "accepted".to_string(),
                table: "memories".to_string(),
            })
        }
        Err(e) => {
            tracing::error!("Failed to ingest event: {}", e);
            match queue_id {
                // Durably captured; a background drain task will retry it.
                Some(id) => Json(EventResponse {
                    event_id: id.to_string(),
                    memory_id: String::new(),
                    status: "queued".to_string(),
                    table: "memories".to_string(),
                }),
                None => Json(EventResponse {
                    event_id: Uuid::new_v4().to_string(),
                    memory_id: Uuid::new_v4().to_string(),
                    status: "error".to_string(),
                    table: "memories".to_string(),
                }),
            }
        }
    }
}

/// Embed + store one event as a memory. Shared by the request path above and
/// the background drain task below, so a queued write replays identically.
async fn store_event(state: &AppState, body: &EventCreate) -> anyhow::Result<Memory> {
    let session_id = body
        .session_id
        .as_deref()
        .and_then(|s| Uuid::parse_str(s).ok());
    let agent_id = body
        .agent_id
        .as_deref()
        .and_then(|s| Uuid::parse_str(s).ok());

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
            None,
            None,
        )
        .await?;
    Ok(memory)
}

/// Retry queued events left over from a past Postgres outage. Stops at the
/// first failure so a still-down database isn't hammered on every row; the
/// next scheduled tick tries again from where this left off.
///
/// Only rows older than `MIN_AGE_SECS` are drained (see `PendingWriteQueue::stale`)
/// so this never races the still in-flight request that originally enqueued a
/// row — that request removes its own row within milliseconds under normal
/// operation. ponytail: this still doesn't make the replay itself idempotent
/// (`store_memory` has no dedup key), so a crash between the Postgres commit
/// and the queue-row removal can still produce a duplicate `memories` row on
/// the next drain. Add a client-supplied event id + unique constraint if that
/// residual, crash-only case ever needs closing.
pub async fn drain_pending(state: &Arc<AppState>) -> anyhow::Result<()> {
    const BATCH: i64 = 50;
    const MIN_AGE_SECS: i64 = 30;
    let batch = state.pending_writes.stale(BATCH, MIN_AGE_SECS).await?;
    for queued in batch {
        let body: EventCreate = match serde_json::from_value(queued.payload) {
            Ok(body) => body,
            Err(e) => {
                tracing::error!("dropping unreadable queued event {}: {e:#}", queued.id);
                state.pending_writes.remove(queued.id).await?;
                continue;
            }
        };
        match store_event(state, &body).await {
            Ok(_) => {
                state.pending_writes.remove(queued.id).await?;
            }
            Err(e) => {
                tracing::warn!(
                    "pending-writes drain stopping early, event {} still failing: {e:#}",
                    queued.id
                );
                break;
            }
        }
    }
    Ok(())
}
