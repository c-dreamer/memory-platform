use axum::{extract::State, http::StatusCode, Json};
use std::sync::Arc;
use uuid::Uuid;

use crate::api::auth::Auth;
use crate::api::dto::{SessionCreateRequest, SessionCreateResponse, SummarizeRequest};
use crate::AppState;

fn err(e: impl std::fmt::Display) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({"error": e.to_string()})),
    )
}

fn bad_uuid(field: &str) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({"error": format!("invalid {field}: not a UUID")})),
    )
}

pub async fn create_session(
    _auth: Auth,
    State(state): State<Arc<AppState>>,
    Json(body): Json<SessionCreateRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let agent_id = match Uuid::parse_str(&body.agent_id) {
        Ok(id) => id,
        Err(_) => return bad_uuid("agent_id"),
    };
    let parent_session_id = match body.parent_session_id.as_deref().map(Uuid::parse_str) {
        Some(Ok(id)) => Some(id),
        Some(Err(_)) => return bad_uuid("parent_session_id"),
        None => None,
    };

    match state
        .db
        .create_session(Some(agent_id), body.goal.as_deref(), parent_session_id)
        .await
    {
        Ok(session) => (
            StatusCode::OK,
            Json(
                serde_json::to_value(SessionCreateResponse {
                    id: session.id.to_string(),
                    goal: session.goal,
                    status: session.status,
                })
                .expect("SessionCreateResponse always serializes"),
            ),
        ),
        Err(e) => err(e),
    }
}

pub async fn get_session(
    _auth: Auth,
    State(state): State<Arc<AppState>>,
    axum::extract::Path(session_id): axum::extract::Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    let id = match Uuid::parse_str(&session_id) {
        Ok(id) => id,
        Err(_) => return bad_uuid("session_id"),
    };

    match state.db.get_session(id).await {
        Ok(Some(session)) => match serde_json::to_value(session) {
            Ok(v) => (StatusCode::OK, Json(v)),
            Err(e) => err(e),
        },
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "session not found"})),
        ),
        Err(e) => err(e),
    }
}

pub async fn summarize_session(
    _auth: Auth,
    State(state): State<Arc<AppState>>,
    axum::extract::Path(session_id): axum::extract::Path<String>,
    Json(body): Json<SummarizeRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let id = match Uuid::parse_str(&session_id) {
        Ok(id) => id,
        Err(_) => return bad_uuid("session_id"),
    };
    let summary = body.summary.unwrap_or_default();

    match state.db.end_session(id, &summary).await {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({"id": id, "status": "ended", "summary": summary})),
        ),
        Err(e) => err(e),
    }
}
